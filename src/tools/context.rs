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
pub struct RecordPlan;
pub struct ApproveStage;

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
            role: ctx.session_role.label().to_string(),
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
            role: ctx.session_role.label().to_string(),
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

#[async_trait]
impl Tool for RecordPlan {
    fn name(&self) -> &'static str {
        "record_plan"
    }
    fn description(&self) -> &'static str {
        "Write the Planner stage artifact: tasks, architectural decision, \
         acceptance criteria, scope and non-goals. Acceptance criteria become \
         immutable. Architect/Planner only."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "decision": { "type": "string" },
                "scope": { "type": "string" },
                "non_goals": { "type": "string" },
                "tasks": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "acceptance_criteria": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "text": { "type": "string" }
                        },
                        "required": ["id", "text"]
                    }
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
        if ctx.session_role != crate::session::AgentRole::Architect {
            return Err(ToolError::Message(
                "record_plan is only available to the Planner (Architect).".into(),
            ));
        }
        let root = ctx
            .project
            .as_ref()
            .ok_or_else(|| ToolError::Message("no project is open".into()))?;
        let store = crate::pipeline::contract::ContractStore::open(&root.canonical_root);
        if store
            .planner()
            .map_err(ToolError::Message)?
            .is_some_and(|p| !p.acceptance_criteria.is_empty())
        {
            return Err(ToolError::Message(
                "acceptance criteria are immutable after the Planner writes them".into(),
            ));
        }
        let decision = arg_str(&args, "decision")?.to_string();
        let scope = args
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let non_goals = args
            .get("non_goals")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tasks: Vec<String> = args
            .get("tasks")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let acceptance_criteria = args
            .get("acceptance_criteria")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| {
                        Some(crate::pipeline::contract::AcceptanceCriterion {
                            id: v.get("id")?.as_str()?.to_string(),
                            text: v.get("text")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let output = crate::pipeline::contract::PlannerOutput {
            tasks: tasks.clone(),
            decision: decision.clone(),
            acceptance_criteria,
            scope,
            non_goals,
        };
        store.write_planner(&output).map_err(ToolError::Message)?;
        {
            let mut ctx_store = store_of(ctx)?;
            ctx_store
                .append_decision(Decision {
                    at: Utc::now(),
                    model: ctx.session_model.clone(),
                    session: ctx.session_label.clone(),
                    role: ctx.session_role.label().to_string(),
                    decision: decision.clone(),
                    rationale: "Planner approach".into(),
                    files: Vec::new(),
                    pinned: true,
                })
                .map_err(ToolError::Message)?;
            for task in &tasks {
                ctx_store
                    .upsert_task(None, TaskStatus::Open, task.clone())
                    .map_err(ToolError::Message)?;
            }
        }
        Ok(truncate_output(format!(
            "Recorded plan with {} task(s) and {} acceptance criteria.",
            output.tasks.len(),
            output.acceptance_criteria.len()
        )))
    }
}

#[async_trait]
impl Tool for ApproveStage {
    fn name(&self) -> &'static str {
        "approve_stage"
    }
    fn description(&self) -> &'static str {
        "Write the Reviewer verdict: pass or fail, plus per-acceptance-criterion status. \
         The orchestrator reads this structured record; do not use record_decision for this."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "verdict": { "type": "string" },
                "commit_message": { "type": "string" },
                "findings": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "required_fixes": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "ac_status": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "status": { "type": "string" },
                            "detail": { "type": "string" }
                        },
                        "required": ["id", "status"]
                    }
                }
            },
            "required": ["verdict"]
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
        if ctx.session_role != crate::session::AgentRole::Reviewer {
            return Err(ToolError::Message(
                "approve_stage is only available to the Reviewer.".into(),
            ));
        }
        let root = ctx
            .project
            .as_ref()
            .ok_or_else(|| ToolError::Message("no project is open".into()))?;
        let verdict = args
            .get("verdict")
            .and_then(|v| v.as_str())
            .and_then(crate::pipeline::contract::ReviewVerdict::parse)
            .ok_or_else(|| ToolError::InvalidArgs("verdict must be pass or fail".into()))?;
        let ac_status = args
            .get("ac_status")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| {
                        let id = v.get("id")?.as_str()?.to_string();
                        let status = v.get("status")?.as_str()?;
                        let check = if status.eq_ignore_ascii_case("ok")
                            || status.eq_ignore_ascii_case("pass")
                        {
                            crate::pipeline::contract::AcCheck::Ok
                        } else {
                            crate::pipeline::contract::AcCheck::Failed {
                                detail: v
                                    .get("detail")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            }
                        };
                        Some(crate::pipeline::contract::AcStatus { id, check })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let findings = string_list(&args, "findings");
        let required_fixes = string_list(&args, "required_fixes");
        let commit_message = args
            .get("commit_message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let output = crate::pipeline::contract::ReviewerOutput {
            verdict,
            ac_status,
            findings: findings.clone(),
            required_fixes,
            commit_message,
        };
        crate::pipeline::contract::ContractStore::open(&root.canonical_root)
            .write_reviewer(&output)
            .map_err(ToolError::Message)?;
        if !findings.is_empty() {
            let mut ctx_store = store_of(ctx)?;
            for finding in findings {
                ctx_store
                    .append_finding(crate::context::store::Finding {
                        at: Utc::now(),
                        model: ctx.session_model.clone(),
                        session: ctx.session_label.clone(),
                        role: ctx.session_role.label().to_string(),
                        description: finding,
                        severity: "review".into(),
                        location: None,
                    })
                    .map_err(ToolError::Message)?;
            }
        }
        Ok(truncate_output(format!(
            "Stage verdict recorded: {:?}",
            output.verdict
        )))
    }
}

fn string_list(args: &serde_json::Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
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
            session_role: crate::session::AgentRole::Coder,
            runner: None,
            run_configs: None,
            run_starts: None,
            db: None,
            subagents: None,
            budget_usd: None,
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

    #[tokio::test]
    async fn record_plan_writes_contract_and_coder_cannot_overwrite_acs() {
        let (_tmp, mut ctx) = fixture();
        ctx.session_role = crate::session::AgentRole::Architect;
        RecordPlan
            .execute(
                serde_json::json!({
                    "decision": "Keep the digest.",
                    "scope": "context",
                    "non_goals": "new files",
                    "tasks": ["emit ACs"],
                    "acceptance_criteria": [{"id": "AC1", "text": "digest shows tasks"}]
                }),
                &ctx,
            )
            .await
            .unwrap();
        ctx.session_role = crate::session::AgentRole::Coder;
        let err = RecordPlan
            .execute(serde_json::json!({ "decision": "changed" }), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Architect") || err.to_string().contains("immutable"));
    }

    #[tokio::test]
    async fn approve_stage_writes_structured_verdict() {
        let (_tmp, mut ctx) = fixture();
        ctx.session_role = crate::session::AgentRole::Reviewer;
        ApproveStage
            .execute(
                serde_json::json!({
                    "verdict": "fail",
                    "ac_status": [{"id": "AC1", "status": "fail", "detail": "missing test"}],
                    "findings": ["no coverage"],
                    "commit_message": ""
                }),
                &ctx,
            )
            .await
            .unwrap();
        let review = crate::pipeline::contract::ContractStore::open(
            &ctx.project.as_ref().unwrap().canonical_root,
        )
        .reviewer()
        .unwrap()
        .unwrap();
        assert_eq!(
            review.verdict,
            crate::pipeline::contract::ReviewVerdict::Fail
        );
        assert!(matches!(
            review.ac_status[0].check,
            crate::pipeline::contract::AcCheck::Failed { .. }
        ));
    }
}
