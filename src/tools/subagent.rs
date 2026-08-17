//! Spawn a read-only child session and wait for its conclusion.

use super::{Tool, ToolContext, ToolError, ToolOutcome, ToolRegistry, ToolRisk, truncate_output};
use crate::providers::ChatMessage;
use crate::session::agent_loop::{RunEnv, TurnDeps, run_turn};
use crate::session::subagent::{PendingSubagent, SUBAGENT_MAX_ITER};
use crate::session::{AgentEvent, AgentRole, ApprovalBridge, Session};
use crate::tools::TOOL_OUTPUT_CHAR_LIMIT;
use async_trait::async_trait;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

pub struct SpawnSubagent;

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArgs(format!("missing `{key}`")))
}

fn parse_role(raw: &str) -> Result<AgentRole, ToolError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "architect" => Ok(AgentRole::Architect),
        "reviewer" => Ok(AgentRole::Reviewer),
        other => Err(ToolError::InvalidArgs(format!(
            "subagent role must be architect or reviewer, not `{other}`"
        ))),
    }
}

fn last_assistant_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find_map(|m| match m {
            ChatMessage::Assistant { content, .. } if !content.trim().is_empty() => {
                Some(content.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| "(subagent produced no final text)".into())
}

fn child_registry(role: AgentRole) -> ToolRegistry {
    let mut registry = ToolRegistry::for_role(role);
    registry.unregister("spawn_subagent");
    registry.retain_readonly();
    registry
}

fn tee_spent(ui_tx: mpsc::Sender<AgentEvent>, spent: Arc<Mutex<f64>>) -> mpsc::Sender<AgentEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if let AgentEvent::Usage { spent_usd, .. } = &event
                && let Ok(mut slot) = spent.lock()
            {
                *slot = *spent_usd;
            }
            if ui_tx.send(event).is_err() {
                break;
            }
        }
    });
    tx
}

#[async_trait]
impl Tool for SpawnSubagent {
    fn name(&self) -> &'static str {
        "spawn_subagent"
    }

    fn description(&self) -> &'static str {
        "Delegate a read-only investigation to an Architect or Reviewer subagent. \
         The child runs in its own session; you receive only the final summary. \
         Requires approval. The child's budget is a slice of this session's remaining cap."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "role": { "type": "string", "enum": ["architect", "reviewer"] },
                "task": { "type": "string" },
                "model": { "type": "string" }
            },
            "required": ["role", "task"]
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
                "spawn_subagent requires approval before it can run.".into(),
            ));
        }
        let host = ctx.subagents.as_ref().ok_or_else(|| {
            ToolError::Message("subagents are unavailable in this session".into())
        })?;
        let role = parse_role(arg_str(&args, "role")?)?;
        let task = arg_str(&args, "task")?.to_string();
        if task.trim().is_empty() {
            return Err(ToolError::InvalidArgs("`task` must not be empty".into()));
        }
        let model = args
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&ctx.session_model)
            .to_string();
        let project = ctx
            .project
            .clone()
            .ok_or_else(|| ToolError::Message("no project is open".into()))?;
        let slice = host.slice();
        if ctx.budget_usd.is_some() && slice <= 0.0 {
            return Err(ToolError::Message(
                "no remaining budget to slice for a subagent".into(),
            ));
        }

        let mut child = Session::new(format!("sub: {task}"), model.clone()).with_role(role);
        child.limits.max_iterations = SUBAGENT_MAX_ITER;
        if slice.is_finite() {
            child.limits.budget_usd = slice;
        }
        let child_id = child.id.clone();
        let handle = Arc::new(tokio::sync::Mutex::new(child));
        let (ui_tx, ui_rx) = mpsc::channel();
        let child_spent = Arc::new(Mutex::new(0.0f64));
        let events = tee_spent(ui_tx, child_spent.clone());
        let cancel = ctx.cancel.child_token();
        let approvals = ApprovalBridge::new();
        host.push(PendingSubagent {
            id: child_id.clone(),
            label: format!("sub: {task}"),
            model: model.clone(),
            role,
            parent_id: ctx.session.clone(),
            parent_label: ctx.session_label.clone(),
            handle: handle.clone(),
            agent_rx: ui_rx,
            agent_cancel: cancel.clone(),
            approvals: approvals.clone(),
            budget_usd: if slice.is_finite() { slice } else { 0.0 },
        });

        let _permit = host
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ToolError::Message("session slot closed".into()))?;
        if ctx.cancel.is_cancelled() || cancel.is_cancelled() {
            return Err(ToolError::Message("cancelled".into()));
        }

        let (term_tx, _term_rx) = mpsc::channel();
        let deps = TurnDeps {
            provider: host.provider.clone(),
            registry: Arc::new(child_registry(role)),
            project,
            events,
            approvals,
            policy: crate::security::Policy::default(),
            cancel,
            session_id: child_id,
            terminal: term_tx,
            store: ctx.store.clone(),
            session_label: format!("sub: {task}"),
            session_model: model.clone(),
            session_role: role,
            summary_model: None,
            db: ctx.db.clone(),
            prompt_price: None,
            completion_price: None,
            budget_usd: if slice.is_finite() { Some(slice) } else { None },
            budget_bridge: crate::session::BudgetBridge::new(),
            spent_start: 0.0,
            context_length: crate::session::context_window::DEFAULT_CONTEXT_LENGTH,
            recent_keep: crate::session::context_window::DEFAULT_RECENT_KEEP,
            run_env: RunEnv::default(),
            user_images: Vec::new(),
            subagents: None,
        };
        let result = run_turn(handle.clone(), Some(task), deps).await;
        let spent = child_spent.lock().map(|g| *g).unwrap_or(0.0);
        host.debit(spent);
        let text = {
            let session = handle.lock().await;
            last_assistant_text(&session.messages)
        };
        let status = match result {
            crate::session::agent_loop::TurnResult::Completed => String::new(),
            other => format!("\n\n(subagent ended: {other:?})"),
        };
        let body = format!("{text}{status}");
        let _ = TOOL_OUTPUT_CHAR_LIMIT;
        Ok(truncate_output(body))
    }
}

#[cfg(test)]
mod tests {
    use super::child_registry;
    use crate::session::AgentRole;

    #[test]
    fn child_registry_never_includes_spawn() {
        for role in [AgentRole::Architect, AgentRole::Reviewer] {
            let registry = child_registry(role);
            assert!(registry.get("spawn_subagent").is_none());
            assert!(registry.get("write_file").is_none());
            assert!(registry.get("run_command").is_none());
            assert!(registry.get("read_file").is_some());
        }
    }
}
