//! stdio JSON-RPC 2.0 client. Process groups via command-group.

use command_group::AsyncCommandGroup;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("{0}")]
    Message(String),
    #[error("MCP call timed out")]
    Timeout,
    #[error("MCP server is not running")]
    Dead,
}

pub struct McpClient {
    pub name: String,
    writer: Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, McpError>>>>,
    next_id: AtomicU64,
    dead: AtomicBool,
    kill: Mutex<Option<mpsc::Sender<()>>>,
}

impl McpClient {
    pub async fn spawn(config: &super::trust::McpServerConfig) -> Result<Arc<Self>, McpError> {
        if config.is_denied() {
            return Err(McpError::Message(format!(
                "MCP server `{}` is blocked by the command denylist",
                config.name
            )));
        }
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        let mut child = cmd.group_spawn().map_err(|e| {
            McpError::Message(format!("could not start MCP server `{}`: {e}", config.name))
        })?;
        let stdin = child
            .inner()
            .stdin
            .take()
            .ok_or_else(|| McpError::Message("MCP server stdin missing".into()))?;
        let stdout = child
            .inner()
            .stdout
            .take()
            .ok_or_else(|| McpError::Message("MCP server stdout missing".into()))?;
        let (kill_tx, mut kill_rx) = mpsc::channel(1);
        let client = Arc::new(Self {
            name: config.name.clone(),
            writer: Mutex::new(Some(stdin)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            dead: AtomicBool::new(false),
            kill: Mutex::new(Some(kill_tx)),
        });
        let reader_client = client.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader).await {
                    Ok(value) => reader_client.dispatch_message(value).await,
                    Err(_) => {
                        reader_client.mark_dead();
                        break;
                    }
                }
            }
        });
        tokio::spawn(async move {
            tokio::select! {
                _ = kill_rx.recv() => {
                    let _ = child.start_kill();
                }
                _ = child.wait() => {}
            }
        });
        Ok(client)
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    pub fn mark_dead(&self) {
        self.dead.store(true, Ordering::SeqCst);
        if let Ok(mut pending) = self.pending.try_lock() {
            for (_, tx) in pending.drain() {
                let _ = tx.send(Err(McpError::Dead));
            }
        }
    }

    pub async fn shutdown(&self) {
        self.mark_dead();
        if let Some(tx) = self.kill.lock().await.take() {
            let _ = tx.send(()).await;
        }
        *self.writer.lock().await = None;
    }

    pub async fn initialize(&self) -> Result<Value, McpError> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "orbit", "version": "0.8.0" }
                }),
            )
            .await?;
        self.notify("notifications/initialized", json!({})).await?;
        Ok(result)
    }

    pub async fn list_tools(&self) -> Result<Vec<RemoteTool>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(tools
            .into_iter()
            .filter_map(|t| {
                Some(RemoteTool {
                    name: t.get("name")?.as_str()?.to_string(),
                    description: t
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({})),
                })
            })
            .collect())
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String, McpError> {
        let result = tokio::time::timeout(
            DEFAULT_CALL_TIMEOUT,
            self.request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            ),
        )
        .await
        .map_err(|_| McpError::Timeout)??;
        if result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Err(McpError::Message(extract_text(&result)));
        }
        Ok(extract_text(&result))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.write(&msg).await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        if self.is_dead() {
            return Err(McpError::Dead);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        if let Err(e) = self.write(&msg).await {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }
        rx.await.map_err(|_| McpError::Dead)?
    }

    async fn write(&self, msg: &Value) -> Result<(), McpError> {
        let mut guard = self.writer.lock().await;
        let stdin = guard.as_mut().ok_or(McpError::Dead)?;
        let frame = encode_message(msg);
        stdin
            .write_all(&frame)
            .await
            .map_err(|e| McpError::Message(e.to_string()))?;
        stdin
            .flush()
            .await
            .map_err(|e| McpError::Message(e.to_string()))
    }

    async fn dispatch_message(&self, value: Value) {
        let Some(id) = value.get("id").and_then(|v| v.as_u64()) else {
            return;
        };
        let Some(tx) = self.pending.lock().await.remove(&id) else {
            return;
        };
        if let Some(err) = value.get("error") {
            let msg = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("MCP error")
                .to_string();
            let _ = tx.send(Err(McpError::Message(msg)));
            return;
        }
        let _ = tx.send(Ok(value.get("result").cloned().unwrap_or(Value::Null)));
    }
}

#[derive(Debug, Clone)]
pub struct RemoteTool {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

pub fn encode_message(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).unwrap_or_else(|_| b"{}".to_vec());
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

async fn read_message<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<Value, McpError> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| McpError::Message(e.to_string()))?;
        if n == 0 {
            return Err(McpError::Dead);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok();
        }
    }
    let Some(len) = content_length else {
        return Err(McpError::Message(
            "MCP message missing Content-Length".into(),
        ));
    };
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .await
        .map_err(|e| McpError::Message(e.to_string()))?;
    serde_json::from_slice(&buf).map_err(|e| McpError::Message(e.to_string()))
}

fn extract_text(result: &Value) -> String {
    if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
        let parts: Vec<String> = content
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    item.get("text")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                } else {
                    Some(item.to_string())
                }
            })
            .collect();
        if !parts.is_empty() {
            return parts.join("\n");
        }
    }
    result.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_includes_content_length() {
        let msg = json!({"jsonrpc":"2.0","id":1,"method":"initialize"});
        let bytes = encode_message(&msg);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("Content-Length:"));
        assert!(text.contains("\r\n\r\n"));
        assert!(text.contains("initialize"));
    }

    fn stub_config() -> Option<super::super::trust::McpServerConfig> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?.parent()?;
        let path = dir.join(format!("orbit-mcp-stub{}", std::env::consts::EXE_SUFFIX));
        if !path.exists() {
            return None;
        }
        Some(super::super::trust::McpServerConfig {
            name: "stub".into(),
            command: path.to_string_lossy().into_owned(),
            args: Vec::new(),
            env: Vec::new(),
            enabled: true,
        })
    }

    #[tokio::test]
    async fn stub_lists_and_echoes() {
        let Some(cfg) = stub_config() else {
            return;
        };
        let client = McpClient::spawn(&cfg).await.unwrap();
        client.initialize().await.unwrap();
        let tools = client.list_tools().await.unwrap();
        assert!(tools.iter().any(|t| t.name == "echo"));
        let out = client
            .call_tool("echo", json!({"text": "pong"}))
            .await
            .unwrap();
        assert!(out.contains("pong"), "{out}");
        client.shutdown().await;
    }

    #[tokio::test]
    async fn stub_hang_times_out() {
        let Some(cfg) = stub_config() else {
            return;
        };
        let client = McpClient::spawn(&cfg).await.unwrap();
        client.initialize().await.unwrap();
        let started = std::time::Instant::now();
        let err = client.call_tool("hang", json!({})).await.unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(35));
        assert!(
            err.to_string().to_lowercase().contains("timed out"),
            "{err}"
        );
        client.shutdown().await;
    }
}
