//! `run_command`: spawn a process without a shell.

use super::{Tool, ToolContext, ToolError, ToolOutcome, ToolRisk, truncate_output};
use crate::security::ProposedCommand;
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub enum TerminalEvent {
    Started {
        command: String,
        cancel: CancellationToken,
    },
    Chunk(String),
    Finished {
        exit_code: Option<i32>,
        duration_ms: u128,
        timed_out: bool,
        cancelled: bool,
    },
}

pub struct RunCommand;

#[async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn description(&self) -> &'static str {
        "Run a program with separate arguments in the project root. \
Never pass a shell string. Commands need approval unless already allowed."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "program": { "type": "string" },
                "args": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "required": ["program"]
        })
    }

    fn risk(&self) -> ToolRisk {
        ToolRisk::Executing
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        if !ctx.allow_execute {
            return Err(ToolError::Message(
                "command was not approved for execution".into(),
            ));
        }
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::Message("cancelled".into()));
        }
        let cmd = ProposedCommand::from_value(&args).map_err(ToolError::InvalidArgs)?;
        let project = ctx
            .project
            .as_deref()
            .ok_or_else(|| ToolError::Message("no project is open".into()))?;

        let mut child_cmd = Command::new(&cmd.program);
        child_cmd
            .args(&cmd.args)
            .current_dir(&project.canonical_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .stdin(Stdio::null());
        apply_filtered_env(&mut child_cmd);

        let display = cmd.display();
        let proc_cancel = CancellationToken::new();
        if let Some(tx) = &ctx.terminal {
            let _ = tx.send(TerminalEvent::Started {
                command: display.clone(),
                cancel: proc_cancel.clone(),
            });
        }

        let mut child = child_cmd
            .spawn()
            .map_err(|e| ToolError::Message(format!("failed to start `{display}`: {e}")))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let sink = Arc::new(Mutex::new(String::new()));
        let out_task =
            stdout.map(|pipe| tokio::spawn(pump_reader(pipe, ctx.terminal.clone(), sink.clone())));
        let err_task =
            stderr.map(|pipe| tokio::spawn(pump_reader(pipe, ctx.terminal.clone(), sink.clone())));

        let started = Instant::now();
        let timeout = ctx.command_timeout;
        let outcome = tokio::select! {
            status = child.wait() => {
                CommandFinish {
                    status: status.ok().and_then(|s| s.code()),
                    timed_out: false,
                    cancelled: false,
                }
            }
            _ = tokio::time::sleep(timeout) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                CommandFinish {
                    status: None,
                    timed_out: true,
                    cancelled: false,
                }
            }
            _ = ctx.cancel.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                CommandFinish {
                    status: None,
                    timed_out: false,
                    cancelled: true,
                }
            }
            _ = proc_cancel.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                CommandFinish {
                    status: None,
                    timed_out: false,
                    cancelled: true,
                }
            }
        };

        if let Some(task) = out_task {
            let _ = task.await;
        }
        if let Some(task) = err_task {
            let _ = task.await;
        }

        let duration_ms = started.elapsed().as_millis();
        let body = sink.lock().map(|s| s.clone()).unwrap_or_default();
        if let Some(tx) = &ctx.terminal {
            let _ = tx.send(TerminalEvent::Finished {
                exit_code: outcome.status,
                duration_ms,
                timed_out: outcome.timed_out,
                cancelled: outcome.cancelled,
            });
        }

        let mut report = body;
        if !report.is_empty() && !report.ends_with('\n') {
            report.push('\n');
        }
        if outcome.timed_out {
            report.push_str(&format!(
                "[timed out after {}s; process killed]\n",
                timeout.as_secs()
            ));
        } else if outcome.cancelled {
            report.push_str("[process cancelled]\n");
        }
        match outcome.status {
            Some(code) => report.push_str(&format!("exit_code: {code}\n")),
            None if outcome.timed_out || outcome.cancelled => {}
            None => report.push_str("exit_code: unknown\n"),
        }
        report.push_str(&format!("duration: {:.2}s\n", duration_ms as f64 / 1000.0));

        let is_failure = outcome.timed_out || outcome.cancelled || outcome.status.unwrap_or(1) != 0;
        let truncated = truncate_output(report);
        if is_failure {
            Err(ToolError::Message(truncated.content))
        } else {
            Ok(truncated)
        }
    }
}

struct CommandFinish {
    status: Option<i32>,
    timed_out: bool,
    cancelled: bool,
}

async fn pump_reader<R: AsyncRead + Unpin>(
    reader: R,
    tx: Option<Sender<TerminalEvent>>,
    sink: Arc<Mutex<String>>,
) {
    let mut reader = reader;
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                if let Some(tx) = &tx {
                    let _ = tx.send(TerminalEvent::Chunk(text.clone()));
                }
                if let Ok(mut out) = sink.lock() {
                    out.push_str(&text);
                }
            }
            Err(_) => break,
        }
    }
}

pub fn is_secret_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.contains("KEY")
        || upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
}

fn apply_filtered_env(cmd: &mut Command) {
    cmd.env_clear();
    for (key, value) in std::env::vars() {
        if is_secret_env(&key) {
            continue;
        }
        cmd.env(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::{COMMAND_TIMEOUT, RunCommand, TerminalEvent, is_secret_env};
    use crate::session::SessionId;
    use crate::tools::{Tool, ToolContext};
    use crate::workspace::Project;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn fixture() -> (TempDir, ToolContext) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let project = Arc::new(Project::open(&root).unwrap());
        let ctx = ToolContext {
            session: SessionId::new("t"),
            cancel: CancellationToken::new(),
            project: Some(project),
            allow_sensitive: false,
            proposed_patches: Arc::new(Mutex::new(Vec::new())),
            allow_execute: true,
            command_timeout: COMMAND_TIMEOUT,
            terminal: None,
            store: None,
            session_label: String::new(),
            session_model: String::new(),
            runner: None,
            run_configs: None,
            run_starts: None,
        };
        (tmp, ctx)
    }

    #[test]
    fn secret_env_names_are_filtered() {
        assert!(is_secret_env("OPENROUTER_API_KEY"));
        assert!(is_secret_env("github_token"));
        assert!(is_secret_env("DB_PASSWORD"));
        assert!(is_secret_env("client_secret"));
        assert!(!is_secret_env("PATH"));
        assert!(!is_secret_env("HOME"));
    }

    #[tokio::test]
    async fn refuses_without_approval_flag() {
        let (_tmp, mut ctx) = fixture();
        ctx.allow_execute = false;
        let err = RunCommand
            .execute(
                serde_json::json!({"program": "cargo", "args": ["--version"]}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not approved"));
    }

    #[tokio::test]
    async fn cargo_version_streams_output() {
        let (_tmp, mut ctx) = fixture();
        let (tx, rx) = mpsc::channel();
        ctx.terminal = Some(tx);
        let out = RunCommand
            .execute(
                serde_json::json!({"program": "cargo", "args": ["--version"]}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            out.content.to_ascii_lowercase().contains("cargo"),
            "{}",
            out.content
        );
        let events: Vec<_> = rx.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TerminalEvent::Chunk(text) if !text.is_empty())),
            "expected incremental chunks, got {events:?}"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            TerminalEvent::Finished {
                exit_code: Some(0),
                ..
            }
        )));
    }

    #[tokio::test]
    async fn timeout_kills_a_long_sleep() {
        let (_tmp, mut ctx) = fixture();
        ctx.command_timeout = Duration::from_millis(250);
        let args = if cfg!(windows) {
            serde_json::json!({"program": "ping", "args": ["-n", "30", "127.0.0.1"]})
        } else {
            serde_json::json!({"program": "sleep", "args": ["30"]})
        };
        let started = std::time::Instant::now();
        let err = RunCommand.execute(args, &ctx).await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout should not wait for the child to finish naturally"
        );
        assert!(err.to_string().contains("timed out"), "{}", err.to_string());
    }
}
