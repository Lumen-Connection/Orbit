//! Tiny MCP stdio stub used by tests and manual checks.

use serde_json::{Value, json};
use std::io::{self, Read, Write};

fn main() {
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    loop {
        let msg = match read_message(&mut stdin) {
            Ok(v) => v,
            Err(_) => return,
        };
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();
        match method {
            "initialize" => reply(
                &mut stdout,
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "orbit-mcp-stub", "version": "0.8.0" }
                }),
            ),
            "notifications/initialized" => {}
            "tools/list" => reply(
                &mut stdout,
                id,
                json!({
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Return the provided text.",
                            "inputSchema": {
                                "type": "object",
                                "properties": { "text": { "type": "string" } },
                                "required": ["text"]
                            }
                        },
                        {
                            "name": "hang",
                            "description": "Sleep forever.",
                            "inputSchema": { "type": "object", "properties": {} }
                        },
                        {
                            "name": "die",
                            "description": "Exit the process.",
                            "inputSchema": { "type": "object", "properties": {} }
                        }
                    ]
                }),
            ),
            "tools/call" => {
                let name = msg
                    .pointer("/params/name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match name {
                    "echo" => {
                        let text = msg
                            .pointer("/params/arguments/text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        reply(
                            &mut stdout,
                            id,
                            json!({
                                "content": [{ "type": "text", "text": text }]
                            }),
                        );
                    }
                    "hang" => loop {
                        std::thread::sleep(std::time::Duration::from_secs(60));
                    },
                    "die" => std::process::exit(1),
                    other => reply_err(&mut stdout, id, format!("unknown tool {other}")),
                }
            }
            _ => {
                if id.is_some() {
                    reply_err(&mut stdout, id, format!("unknown method {method}"));
                }
            }
        }
    }
}

fn read_message(stdin: &mut impl Read) -> io::Result<Value> {
    let mut headers = Vec::new();
    let mut buf = [0u8; 1];
    loop {
        stdin.read_exact(&mut buf)?;
        headers.push(buf[0]);
        if headers.ends_with(b"\r\n\r\n") {
            break;
        }
        if headers.len() > 4096 {
            return Err(io::Error::other("header too large"));
        }
    }
    let header = String::from_utf8_lossy(&headers);
    let len = header
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .ok_or_else(|| io::Error::other("missing Content-Length"))?;
    let mut body = vec![0u8; len];
    stdin.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| io::Error::other(e.to_string()))
}

fn reply(stdout: &mut impl Write, id: Option<Value>, result: Value) {
    let Some(id) = id else {
        return;
    };
    write_msg(stdout, json!({"jsonrpc":"2.0","id":id,"result":result}));
}

fn reply_err(stdout: &mut impl Write, id: Option<Value>, message: String) {
    let Some(id) = id else {
        return;
    };
    write_msg(
        stdout,
        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":message}}),
    );
}

fn write_msg(stdout: &mut impl Write, msg: Value) {
    let body = serde_json::to_vec(&msg).unwrap_or_default();
    let _ = write!(stdout, "Content-Length: {}\r\n\r\n", body.len());
    let _ = stdout.write_all(&body);
    let _ = stdout.flush();
}
