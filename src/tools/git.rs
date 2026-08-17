//! Git Gate tools. Execution is mechanical; the Reviewer already decided
//! the message and the files. Sensitive paths cannot be committed.

use super::{Tool, ToolContext, ToolError, ToolOutcome, ToolRisk, truncate_output};
use crate::security::is_sensitive;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

pub struct GitStatus;
pub struct GitCommit;
pub struct GitPush;

fn project_root(ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    ctx.project
        .as_ref()
        .map(|p| p.canonical_root.clone())
        .ok_or_else(|| ToolError::Message("no project is open".into()))
}

fn git(root: &Path, args: &[&str]) -> Result<String, ToolError> {
    crate::workspace::git::git(root, args).map_err(ToolError::Message)
}

fn paths_of(args: &serde_json::Value) -> Result<Vec<String>, ToolError> {
    match args.get("paths") {
        Some(serde_json::Value::Array(items)) => Ok(items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()),
        Some(serde_json::Value::String(s)) => Ok(s
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()),
        _ => Err(ToolError::InvalidArgs(
            "git_commit requires an explicit `paths` list".into(),
        )),
    }
}

#[async_trait]
impl Tool for GitStatus {
    fn name(&self) -> &'static str {
        "git_status"
    }
    fn description(&self) -> &'static str {
        "List modified files in the project working tree."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let root = project_root(ctx)?;
        let out = git(&root, &["status", "--porcelain"])?;
        if out.trim().is_empty() {
            return Ok(truncate_output("working tree clean".into()));
        }
        Ok(truncate_output(out))
    }
}

#[async_trait]
impl Tool for GitCommit {
    fn name(&self) -> &'static str {
        "git_commit"
    }
    fn description(&self) -> &'static str {
        "Stage the given paths and commit with the Reviewer's message. \
         Sensitive files (.env, keys, .git/config) are refused with no override."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "required": ["message", "paths"]
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
            return Err(ToolError::Denied);
        }
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing `message`".into()))?;
        let paths = paths_of(&args)?;
        if paths.is_empty() {
            return Err(ToolError::InvalidArgs("paths must not be empty".into()));
        }
        for path in &paths {
            if is_sensitive(Path::new(path)) {
                return Err(ToolError::Message(format!(
                    "refused: `{path}` looks sensitive and cannot be committed"
                )));
            }
        }
        let root = project_root(ctx)?;
        let mut add = vec!["add".to_string()];
        add.extend(paths);
        let add_refs: Vec<&str> = add.iter().map(String::as_str).collect();
        git(&root, &add_refs)?;
        let out = git(&root, &["commit", "-m", message])?;
        Ok(truncate_output(out))
    }
}

#[async_trait]
impl Tool for GitPush {
    fn name(&self) -> &'static str {
        "git_push"
    }
    fn description(&self) -> &'static str {
        "Push the current branch. --force is denied unless force and confirm_force are both true."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "force": { "type": "boolean" },
                "confirm_force": { "type": "boolean" }
            }
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
            return Err(ToolError::Denied);
        }
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        let confirm = args
            .get("confirm_force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if force && !confirm {
            return Err(ToolError::Message(
                "--force is denied unless confirm_force is also true".into(),
            ));
        }
        let root = project_root(ctx)?;
        let out = if force {
            git(&root, &["push", "--force"])?
        } else {
            git(&root, &["push"])?
        };
        Ok(truncate_output(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;
    use crate::tools::ToolContext;
    use crate::workspace::Project;
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn repo() -> Option<(TempDir, ToolContext)> {
        if !git_available() {
            return None;
        }
        let tmp = TempDir::new().ok()?;
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join("src")).ok()?;
        std::fs::write(root.join("src/lib.rs"), "fn x() {}\n").ok()?;
        let status = Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .ok()?;
        if !status.status.success() {
            return None;
        }
        let _ = Command::new("git")
            .args(["config", "user.email", "orbit@test"])
            .current_dir(&root)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "orbit"])
            .current_dir(&root)
            .status();
        let project = Arc::new(Project::open(&root).ok()?);
        let ctx = ToolContext {
            session: SessionId::new("git"),
            cancel: CancellationToken::new(),
            project: Some(project),
            allow_sensitive: false,
            proposed_patches: Arc::new(Mutex::new(Vec::new())),
            allow_execute: true,
            command_timeout: crate::tools::shell::COMMAND_TIMEOUT,
            terminal: None,
            store: None,
            session_label: "git-gate".into(),
            session_model: String::new(),
            session_role: crate::session::AgentRole::Coder,
            runner: None,
            run_configs: None,
            run_starts: None,
            db: None,
            subagents: None,
            sandbox_profile: crate::security::sandbox::SandboxProfile::Off,
            budget_usd: None,
        };
        Some((tmp, ctx))
    }

    #[tokio::test]
    async fn status_lists_modified_files() {
        let Some((_tmp, ctx)) = repo() else {
            return;
        };
        let out = GitStatus
            .execute(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert!(
            out.content.contains("lib.rs") || out.content.contains("??"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn commit_refuses_env_without_approval_path() {
        let Some((_tmp, ctx)) = repo() else {
            return;
        };
        let err = GitCommit
            .execute(
                serde_json::json!({
                    "message": "leak",
                    "paths": [".env"]
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("sensitive"), "{err}");
    }

    #[tokio::test]
    async fn force_push_is_denied_without_second_gate() {
        let Some((_tmp, ctx)) = repo() else {
            return;
        };
        let err = GitPush
            .execute(serde_json::json!({ "force": true }), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("--force"), "{err}");
    }
}
