//! Run a single hook command. Fail-open on timeout or unreadable output.

use super::trust::HookConfig;
use crate::tools::shell::is_secret_env;
use command_group::AsyncCommandGroup;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize)]
pub struct HookPayload {
    pub event: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub session_id: String,
    pub role: String,
    pub project_root: String,
}

#[derive(Debug, Clone, Deserialize)]
struct HookStdout {
    decision: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookRun {
    Allow { warning: Option<String> },
    Deny { reason: String },
}

pub async fn run_hook(
    hook: &HookConfig,
    payload: &HookPayload,
    cancel: &CancellationToken,
) -> HookRun {
    let mut cmd = Command::new(&hook.command);
    cmd.args(&hook.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .current_dir(&payload.project_root);
    cmd.env_clear();
    for (key, value) in std::env::vars() {
        if is_secret_env(&key) {
            continue;
        }
        cmd.env(key, value);
    }

    let mut child = match cmd.group_spawn() {
        Ok(child) => child,
        Err(e) => {
            let warning = format!("hook `{}` failed to start ({e}); allowing", hook.display());
            tracing::warn!("{warning}");
            return HookRun::Allow {
                warning: Some(warning),
            };
        }
    };

    if let Some(mut stdin) = child.inner().stdin.take() {
        let body = serde_json::to_vec(payload).unwrap_or_else(|_| b"{}".to_vec());
        let _ = stdin.write_all(&body).await;
        let _ = stdin.shutdown().await;
        drop(stdin);
    }

    let stdout = child.inner().stdout.take();
    let stderr = child.inner().stderr.take();
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut reader) = stdout {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf).await;
        }
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut reader) = stderr {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf).await;
        }
        buf
    });

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            return HookRun::Allow {
                warning: Some(format!("hook `{}` cancelled; allowing", hook.display())),
            };
        }
        _ = tokio::time::sleep(HOOK_TIMEOUT) => {
            let _ = child.kill().await;
            let warning = format!(
                "hook `{}` timed out after {}s; allowing",
                hook.display(),
                HOOK_TIMEOUT.as_secs()
            );
            tracing::warn!("{warning}");
            return HookRun::Allow {
                warning: Some(warning),
            };
        }
        status = child.wait() => status,
    };

    let stdout = String::from_utf8_lossy(&stdout_task.await.unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_task.await.unwrap_or_default()).into_owned();
    let status = match status {
        Ok(status) => status,
        Err(e) => {
            tracing::warn!("hook `{}` wait failed: {e}", hook.display());
            return HookRun::Allow {
                warning: Some(format!(
                    "hook `{}` produced no output; allowing",
                    hook.display()
                )),
            };
        }
    };
    if !status.success() {
        let reason = if stderr.trim().is_empty() {
            format!(
                "hook `{}` exited {}",
                hook.display(),
                status.code().unwrap_or(-1)
            )
        } else {
            stderr.trim().to_string()
        };
        return HookRun::Deny { reason };
    }

    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        let warning = format!("hook `{}` printed nothing; allowing", hook.display());
        tracing::warn!("{warning}");
        return HookRun::Allow {
            warning: Some(warning),
        };
    }
    match serde_json::from_str::<HookStdout>(trimmed) {
        Ok(parsed) if parsed.decision.eq_ignore_ascii_case("deny") => HookRun::Deny {
            reason: parsed
                .reason
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("hook `{}` denied the tool", hook.display())),
        },
        Ok(parsed) if parsed.decision.eq_ignore_ascii_case("allow") => {
            HookRun::Allow { warning: None }
        }
        Ok(parsed) => {
            let warning = format!(
                "hook `{}` returned unknown decision `{}`; allowing",
                hook.display(),
                parsed.decision
            );
            tracing::warn!("{warning}");
            HookRun::Allow {
                warning: Some(warning),
            }
        }
        Err(_) => {
            let warning = format!(
                "hook `{}` printed unreadable JSON; allowing",
                hook.display()
            );
            tracing::warn!("{warning}");
            HookRun::Allow {
                warning: Some(warning),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HOOK_TIMEOUT, HookPayload, HookRun, run_hook};
    use crate::hooks::trust::HookConfig;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    fn payload() -> HookPayload {
        HookPayload {
            event: "PreToolUse".into(),
            tool_name: "write_file".into(),
            arguments: serde_json::json!({"path": "src/lib.rs"}),
            session_id: "s".into(),
            role: "Coder".into(),
            project_root: std::env::temp_dir().display().to_string(),
        }
    }

    fn stub() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?.parent()?;
        let path = dir.join(format!("orbit-hook-stub{}", std::env::consts::EXE_SUFFIX));
        path.exists().then(|| path.to_string_lossy().into_owned())
    }

    fn hook(args: &[&str]) -> Option<HookConfig> {
        Some(HookConfig {
            event: "PreToolUse".into(),
            matcher: "write_file".into(),
            command: stub()?,
            args: args.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    #[tokio::test]
    async fn deny_json_is_a_deny() {
        let Some(hook) = hook(&["deny", "no migrations"]) else {
            return;
        };
        let run = run_hook(&hook, &payload(), &CancellationToken::new()).await;
        match run {
            HookRun::Deny { reason } => assert!(reason.contains("migrations"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn exit_one_is_a_deny() {
        let Some(hook) = hook(&["exit1"]) else {
            return;
        };
        let run = run_hook(&hook, &payload(), &CancellationToken::new()).await;
        match run {
            HookRun::Deny { reason } => assert!(reason.contains("crashed") || !reason.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn hang_is_killed_and_allows() {
        let started = Instant::now();
        let Some(hook) = hook(&["hang"]) else {
            return;
        };
        let run = run_hook(&hook, &payload(), &CancellationToken::new()).await;
        assert!(started.elapsed() < HOOK_TIMEOUT + Duration::from_secs(3));
        match run {
            HookRun::Allow { warning } => {
                assert!(
                    warning.as_deref().is_some_and(|w| w.contains("timed out")),
                    "{warning:?}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_stdout_allows_with_warning() {
        let Some(hook) = hook(&["empty"]) else {
            return;
        };
        let run = run_hook(&hook, &payload(), &CancellationToken::new()).await;
        match run {
            HookRun::Allow { warning } => assert!(warning.is_some()),
            other => panic!("{other:?}"),
        }
    }
}
