//! Project-scoped history search over the FTS5 index.

use super::{Tool, ToolContext, ToolError, ToolOutcome, ToolRisk, truncate_output};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

pub const DEFAULT_LIMIT: usize = 10;
pub const MAX_LIMIT: usize = 30;

pub struct SearchHistory;

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArgs(format!("missing `{key}`")))
}

fn format_when(raw: Option<&str>) -> String {
    raw.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc).format("%d/%m %H:%M").to_string())
        .unwrap_or_else(|| "unknown date".into())
}

#[async_trait]
impl Tool for SearchHistory {
    fn name(&self) -> &'static str {
        "search_history"
    }

    fn description(&self) -> &'static str {
        "Search prior Coder sessions in this project by content. \
         Returns session label, date and a snippet — not full transcripts. \
         Use this before reinventing a solution that may already have been decided."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 30 }
            },
            "required": ["query"]
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
        let query = arg_str(&args, "query")?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);
        let project = ctx
            .project
            .as_ref()
            .ok_or_else(|| ToolError::Message("no project is open".into()))?;
        let db = ctx
            .db
            .as_ref()
            .ok_or_else(|| ToolError::Message("history search is unavailable".into()))?;
        let hits = db
            .search_history_scoped(&project.id, query, limit)
            .map_err(|e| ToolError::Message(e.to_string()))?;
        if hits.is_empty() {
            return Ok(truncate_output(format!(
                "No prior sessions in this project matched `{query}`."
            )));
        }
        let mut lines = vec![format!("{} hit(s) in this project:", hits.len())];
        for hit in hits {
            lines.push(format!(
                "- {} · {} · {}",
                hit.title,
                format_when(hit.last_active_at.as_deref()),
                hit.snippet
            ));
        }
        Ok(truncate_output(lines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChatMessage;
    use crate::session::SessionId;
    use crate::storage::db::Db;
    use crate::tools::ToolContext;
    use crate::workspace::Project;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn two_projects() -> (TempDir, ToolContext, String) {
        let tmp = TempDir::new().unwrap();
        let db = Arc::new(Db::open_at(tmp.path().join("orbit.db")).unwrap());
        let root_a = tmp.path().join("proj-a");
        let root_b = tmp.path().join("proj-b");
        std::fs::create_dir_all(&root_a).unwrap();
        std::fs::create_dir_all(&root_b).unwrap();
        let project_a = Arc::new(Project::open(&root_a).unwrap());
        let project_b = Project::open(&root_b).unwrap();
        db.upsert_project(&project_a).unwrap();
        db.upsert_project(&project_b).unwrap();
        let id_a = SessionId::new("sess-a");
        let id_b = SessionId::new("sess-b");
        db.upsert_session(&project_a.id, &id_a, "implementation", "m")
            .unwrap();
        db.upsert_session(&project_b.id, &id_b, "other-project", "m")
            .unwrap();
        db.replace_messages(
            &id_a,
            &[ChatMessage::user("we chose rusqlite for the zebra store")],
        )
        .unwrap();
        db.replace_messages(
            &id_b,
            &[ChatMessage::user("we chose postgres for the zebra store")],
        )
        .unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO history_fts (source, item_id, title, body) \
                 VALUES ('chat', 'chat-1', 'chat mode', 'zebra in a chat')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let ctx = ToolContext {
            session: id_a,
            cancel: CancellationToken::new(),
            project: Some(project_a),
            allow_sensitive: false,
            proposed_patches: Arc::new(Mutex::new(Vec::new())),
            allow_execute: false,
            command_timeout: crate::tools::shell::COMMAND_TIMEOUT,
            terminal: None,
            store: None,
            session_label: "implementation".into(),
            session_model: "m".into(),
            session_role: crate::session::AgentRole::Coder,
            runner: None,
            run_configs: None,
            run_starts: None,
            db: Some(db),
            subagents: None,
            sandbox_profile: crate::security::sandbox::SandboxProfile::Off,
            budget_usd: None,
        };
        (tmp, ctx, project_b.id)
    }

    #[tokio::test]
    async fn scoped_search_hides_other_projects_and_chats() {
        let (_tmp, ctx, other_id) = two_projects();
        let db = ctx.db.as_ref().unwrap();
        let scoped = db
            .search_history_scoped(&ctx.project.as_ref().unwrap().id, "zebra", 10)
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].item_id, "sess-a");
        assert!(scoped[0].snippet.to_lowercase().contains("zebra"));

        let other = db.search_history_scoped(&other_id, "zebra", 10).unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].item_id, "sess-b");

        let unscoped = db.search_history("zebra", 10).unwrap();
        assert!(unscoped.len() >= 3, "{unscoped:?}");

        let out = SearchHistory
            .execute(serde_json::json!({"query": "zebra"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("implementation"));
        assert!(!out.content.contains("other-project"));
        assert!(!out.content.contains("chat mode"));
        assert!(!out.content.contains("postgres"));
    }

    #[tokio::test]
    async fn missing_db_is_a_clear_error() {
        let (_tmp, mut ctx, _) = two_projects();
        ctx.db = None;
        let err = SearchHistory
            .execute(serde_json::json!({"query": "zebra"}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unavailable"));
    }
}
