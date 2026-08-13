//! Tools that write the shared Project Context. No human approval.

use super::{Tool, ToolContext, ToolError, ToolOutcome, ToolRisk, truncate_output};
use crate::context::store::{Decision, Finding, TaskStatus};
use async_trait::async_trait;
use chrono::Utc;

fn store_of(
    ctx: &ToolContext,
) -> Result<std::sync::MutexGuard<'_, crate::context::OrbitStore>, ToolError> {
    let store = ctx
        .store
        .as_ref()
        .ok_or_else(|| ToolError::Message("no project context is open".into()))?;
    store
        .lock()
        .map_err(|_| ToolError::Message("context store lock poisoned".into()))
}

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArgs(format!("missing `{key}`")))
}

fn files_of(args: &serde_json::Value) -> Vec<String> {
    match args.get("files") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(serde_json::Value::String(s)) => s
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

pub struct RecordDecision;
pub struct AddFinding;
pub struct UpdateTask;

#[async_trait]
impl Tool for RecordDecision {
    fn name(&self) -> &'static str {
        "record_decision"
    }
    fn description(&self) -> &'static str {
        "Append an architectural decision to the shared Project Context. No approval needed."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "decision": { "type": "string" },
                "rationale": { "type": "string" },
                "files": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "required": ["decision"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let decision = arg_str(&args, "decision")?.to_string();
        let rationale = args
            .get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let entry = Decision {
            at: Utc::now(),
            model: ctx.session_model.clone(),
            session: ctx.session_label.clone(),
            decision: decision.clone(),
            rationale,
            files: files_of(&args),
            pinned: false,
        };
        store_of(ctx)?
            .append_decision(entry)
            .map_err(ToolError::Message)?;
        Ok(truncate_output(format!("Recorded decision: {decision}")))
    }
}

#[async_trait]
impl Tool for AddFinding {
    fn name(&self) -> &'static str {
        "add_finding"
    }
    fn description(&self) -> &'static str {
        "Append a finding to the shared Project Context. No approval needed."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": { "type": "string" },
                "severity": { "type": "string" },
                "location": { "type": "string" }
            },
            "required": ["description"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let description = arg_str(&args, "description")?.to_string();
        let severity = args
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or("info")
            .to_string();
        let location = args
            .get("location")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let entry = Finding {
            at: Utc::now(),
            model: ctx.session_model.clone(),
            session: ctx.session_label.clone(),
            description: description.clone(),
            severity,
            location,
        };
        store_of(ctx)?
            .append_finding(entry)
            .map_err(ToolError::Message)?;
        Ok(truncate_output(format!("Recorded finding: {description}")))
    }
}

#[async_trait]
impl Tool for UpdateTask {
    fn name(&self) -> &'static str {
        "update_task"
    }
    fn description(&self) -> &'static str {
        "Create or update a task in the shared Project Context. Pass id \"new\" to create."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "status": { "type": "string" },
                "description": { "type": "string" }
            },
            "required": ["status"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let status_raw = arg_str(&args, "status")?;
        let status = TaskStatus::parse(status_raw).ok_or_else(|| {
            ToolError::InvalidArgs(format!(
                "unknown status `{status_raw}` (use open, in_progress, done)"
            ))
        })?;
        let id = args.get("id").and_then(|v| v.as_str()).map(str::to_string);
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let item = store_of(ctx)?
            .upsert_task(id, status, description)
            .map_err(ToolError::Message)?;
        Ok(truncate_output(format!(
            "Task `{}` is now {} — {}",
            item.id,
            item.status.as_str(),
            item.description
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::OrbitStore;
    use crate::session::SessionId;
    use crate::tools::ToolContext;
    use crate::workspace::Project;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn fixture() -> (TempDir, ToolContext) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let project = Arc::new(Project::open(&root).unwrap());
        let store = Arc::new(Mutex::new(OrbitStore::open(&root)));
        let ctx = ToolContext {
            session: SessionId::new("sid"),
            cancel: CancellationToken::new(),
            project: Some(project),
            allow_sensitive: false,
            proposed_patches: Arc::new(Mutex::new(Vec::new())),
            allow_execute: false,
            command_timeout: crate::tools::shell::COMMAND_TIMEOUT,
            terminal: None,
            store: Some(store),
            session_label: "architecture".into(),
            session_model: "claude-opus-5".into(),
            runner: None,
            run_configs: None,
            run_starts: None,
        };
        (tmp, ctx)
    }

    #[tokio::test]
    async fn record_decision_writes_authorship_and_timestamp() {
        let (_tmp, ctx) = fixture();
        RecordDecision
            .execute(
                serde_json::json!({
                    "decision": "Use JWT with refresh tokens.",
                    "rationale": "Stateless API.",
                    "files": ["src/auth/token.rs"]
                }),
                &ctx,
            )
            .await
            .unwrap();
        let text = std::fs::read_to_string(
            ctx.project
                .as_ref()
                .unwrap()
                .canonical_root
                .join(".orbit/decisions.md"),
        )
        .unwrap();
        assert!(text.contains("claude-opus-5"));
        assert!(text.contains("session \"architecture\""));
        assert!(text.contains("Use JWT with refresh tokens."));
        assert!(text.contains("2026-") || text.contains("20"));
        assert!(text.contains("**Decision:**"));
    }
}
