//! SQLite store. Call methods from `spawn_blocking`, never the UI thread.

use crate::providers::{ChatMessage, TokenUsage, ToolCall};
use crate::session::SessionId;
use crate::workspace::{FilePatch, PatchStatus, Project};
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../../migrations/0001_init.sql")),
    (2, include_str!("../../migrations/0002_context_summary.sql")),
    (3, include_str!("../../migrations/0003_project_hidden.sql")),
    (4, include_str!("../../migrations/0004_history_fts.sql")),
];

#[derive(Clone)]
pub struct Db {
    path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
pub struct StoredSession {
    pub id: String,
    pub label: String,
    pub model_id: String,
    #[allow(dead_code)]
    pub last_active_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
}

#[derive(Debug, Clone)]
pub struct UsageBucket {
    pub key: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct HistoryHit {
    pub item_id: String,
    pub source: String,
    pub title: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Default)]
pub struct UsageReport {
    pub by_project: Vec<UsageBucket>,
    pub by_model: Vec<UsageBucket>,
    pub by_day: Vec<UsageBucket>,
    pub total_cost: f64,
    pub total_input: i64,
    pub total_output: i64,
}

impl Db {
    pub fn open() -> Result<Self> {
        let path = crate::storage::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("orbit.db");
        Self::open_at(path)
    }

    pub fn open_at(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn =
            Connection::open(&path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&conn, &path)?;
        tracing::info!(path = %path.display(), "opened orbit database");
        Ok(Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
        f(&guard)
    }

    pub fn upsert_project(&self, project: &Project) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO project (id, name, canonical_root, created_at, last_opened_at, hidden)
                 VALUES (?1, ?2, ?3, ?4, ?4, 0)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    canonical_root = excluded.canonical_root,
                    last_opened_at = excluded.last_opened_at,
                    hidden = 0",
                params![
                    project.id,
                    project.name,
                    project.canonical_root.to_string_lossy().to_string(),
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_recent_projects(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::workspace::registry::ProjectEntry>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.name, p.canonical_root, p.created_at, p.last_opened_at,
                        (SELECT COUNT(*) FROM session s WHERE s.project_id = p.id),
                        (SELECT COUNT(*) FROM file_change c
                          WHERE c.project_id = p.id AND c.status = 'pending')
                 FROM project p
                 WHERE COALESCE(p.hidden, 0) = 0
                 ORDER BY p.last_opened_at DESC
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], |row| {
                Ok(crate::workspace::registry::ProjectEntry {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: PathBuf::from(row.get::<_, String>(2)?),
                    first_opened_at: row.get(3)?,
                    last_opened_at: row.get(4)?,
                    session_count: row.get::<_, i64>(5)? as u32,
                    pending_patches: row.get::<_, i64>(6)? as u32,
                    availability: crate::workspace::registry::ProjectAvailability::Ready,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn hide_project(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("UPDATE project SET hidden = 1 WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn rebind_project(&self, project: &Project) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE project SET name = ?2, canonical_root = ?3, last_opened_at = ?4, hidden = 0
                 WHERE id = ?1",
                params![
                    project.id,
                    project.name,
                    project.canonical_root.to_string_lossy().to_string(),
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn upsert_session(
        &self,
        project_id: &str,
        id: &SessionId,
        label: &str,
        model: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO session (id, project_id, label, model_id, created_at, last_active_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    label = excluded.label,
                    model_id = excluded.model_id,
                    last_active_at = excluded.last_active_at",
                params![id.as_str(), project_id, label, model, now],
            )?;
            Ok(())
        })
    }

    pub fn replace_messages(&self, session_id: &SessionId, messages: &[ChatMessage]) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM tool_call WHERE session_id = ?1",
                params![session_id.as_str()],
            )?;
            tx.execute(
                "DELETE FROM message WHERE session_id = ?1",
                params![session_id.as_str()],
            )?;
            let now = Utc::now().to_rfc3339();
            for (seq, message) in messages.iter().enumerate() {
                let (role, content, tools) = encode_message(message);
                let mid = Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO message (id, session_id, seq, role, content, tool_calls_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![mid, session_id.as_str(), seq as i64, role, content, tools, now],
                )?;
            }
            tx.commit()?;
            Ok(())
        })?;
        self.reindex_session(session_id, messages)
    }

    pub fn reindex_chats(&self, chats: &[crate::app::Chat]) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM history_fts WHERE source = 'chat'", [])?;
            let mut stmt = conn.prepare(
                "INSERT INTO history_fts (source, item_id, title, body) VALUES ('chat', ?1, ?2, ?3)",
            )?;
            for chat in chats {
                let body: String = chat
                    .messages
                    .iter()
                    .map(|m| m.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                stmt.execute(params![chat.id.to_string(), chat.title.as_str(), body])?;
            }
            Ok(())
        })
    }

    pub fn reindex_session(&self, session_id: &SessionId, messages: &[ChatMessage]) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM history_fts WHERE source = 'session' AND item_id = ?1",
                params![session_id.as_str()],
            )?;
            let title: String = conn
                .query_row(
                    "SELECT label FROM session WHERE id = ?1",
                    params![session_id.as_str()],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or_else(|| session_id.as_str().to_string());
            let body = flatten_message_text(messages);
            conn.execute(
                "INSERT INTO history_fts (source, item_id, title, body) VALUES ('session', ?1, ?2, ?3)",
                params![session_id.as_str(), title, body],
            )?;
            Ok(())
        })
    }

    #[allow(dead_code)]
    pub fn search_history(&self, query: &str, limit: usize) -> Result<Vec<HistoryHit>> {
        let match_query = fts_query(query);
        if match_query.is_empty() {
            return Ok(Vec::new());
        }
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT item_id, source, title, snippet(history_fts, 3, '[', ']', '…', 12)
                 FROM history_fts
                 WHERE history_fts MATCH ?1
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![match_query, limit as i64], |row| {
                Ok(HistoryHit {
                    item_id: row.get(0)?,
                    source: row.get(1)?,
                    title: row.get(2)?,
                    snippet: row.get(3)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    #[allow(dead_code)]
    pub fn insert_tool_call(
        &self,
        session_id: &SessionId,
        call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
        status: &str,
        output: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            let message_id: String = conn
                .query_row(
                    "SELECT id FROM message WHERE session_id = ?1 ORDER BY seq DESC LIMIT 1",
                    params![session_id.as_str()],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or_else(|| session_id.as_str().to_string());
            conn.execute(
                "INSERT OR REPLACE INTO tool_call
                    (id, session_id, message_id, tool_name, arguments_json, status, output, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    call_id,
                    session_id.as_str(),
                    message_id,
                    name,
                    arguments.to_string(),
                    status,
                    output,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn upsert_file_change(
        &self,
        project_id: &str,
        session_id: &SessionId,
        patch: &FilePatch,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let status = match patch.status {
            PatchStatus::Pending => "pending",
            PatchStatus::Applied => "applied",
            PatchStatus::Rejected => "rejected",
            PatchStatus::Conflicted => "conflicted",
        };
        let applied_at = if matches!(patch.status, PatchStatus::Applied) {
            Some(now.clone())
        } else {
            None
        };
        let id = format!("{}:{}", session_id.as_str(), patch.relative_path.display());
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO file_change
                    (id, project_id, session_id, relative_path, original_hash, unified_diff,
                     original_content, proposed_content, status, created_at, applied_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(id) DO UPDATE SET
                    status = excluded.status,
                    applied_at = excluded.applied_at,
                    unified_diff = excluded.unified_diff,
                    proposed_content = excluded.proposed_content",
                params![
                    id,
                    project_id,
                    session_id.as_str(),
                    patch.relative_path.display().to_string(),
                    patch.original_hash,
                    patch.unified_diff,
                    patch.original_content,
                    patch.proposed_content,
                    status,
                    now,
                    applied_at
                ],
            )?;
            Ok(())
        })
    }

    pub fn pending_patches(&self, project_id: &str) -> Result<Vec<FilePatch>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT relative_path, original_hash, unified_diff, original_content, proposed_content, status
                 FROM file_change WHERE project_id = ?1 AND status = 'pending'",
            )?;
            let rows = stmt.query_map(params![project_id], |row| {
                let path: String = row.get(0)?;
                let original_hash: String = row.get(1)?;
                let unified_diff: String = row.get(2)?;
                let original_content: String = row.get(3)?;
                let proposed_content: String = row.get(4)?;
                Ok(FilePatch {
                    relative_path: PathBuf::from(path),
                    original_hash,
                    original_content,
                    proposed_content,
                    unified_diff,
                    status: PatchStatus::Pending,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn load_context_summary(&self, session_id: &SessionId) -> Result<Option<(String, usize)>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT context_summary, context_summary_upto FROM session WHERE id = ?1",
                params![session_id.as_str()],
                |row| {
                    let text: Option<String> = row.get(0)?;
                    let upto: i64 = row.get(1)?;
                    Ok(text.filter(|s| !s.is_empty()).map(|s| (s, upto as usize)))
                },
            )
            .optional()
            .map(|row| row.flatten())
            .map_err(Into::into)
        })
    }

    pub fn save_context_summary(
        &self,
        session_id: &SessionId,
        summary: &str,
        covered: usize,
    ) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE session SET context_summary = ?2, context_summary_upto = ?3 WHERE id = ?1",
                params![session_id.as_str(), summary, covered as i64],
            )?;
            Ok(())
        })
    }

    pub fn insert_usage(
        &self,
        session_id: &SessionId,
        model_id: &str,
        usage: &TokenUsage,
        cost: f64,
        latency_ms: Option<u64>,
    ) -> Result<()> {
        self.insert_usage_kind(session_id, model_id, usage, cost, latency_ms, "turn")
    }

    pub fn insert_usage_kind(
        &self,
        session_id: &SessionId,
        model_id: &str,
        usage: &TokenUsage,
        cost: f64,
        latency_ms: Option<u64>,
        kind: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO usage_record
                    (id, session_id, model_id, input_tokens, output_tokens, cached_tokens,
                     estimated_cost, latency_ms, created_at, kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    Uuid::new_v4().to_string(),
                    session_id.as_str(),
                    model_id,
                    usage.prompt_tokens as i64,
                    usage.completion_tokens as i64,
                    usage.cached_tokens as i64,
                    cost,
                    latency_ms.map(|n| n as i64),
                    now,
                    kind
                ],
            )?;
            Ok(())
        })
    }

    pub fn session_usage(&self, session_id: &SessionId) -> Result<UsageTotals> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(estimated_cost),0)
                 FROM usage_record WHERE session_id = ?1",
                params![session_id.as_str()],
                |row| {
                    Ok(UsageTotals {
                        input_tokens: row.get::<_, i64>(0)? as u32,
                        output_tokens: row.get::<_, i64>(1)? as u32,
                        cost_usd: row.get(2)?,
                    })
                },
            )
            .map_err(Into::into)
        })
    }

    pub fn load_sessions(&self, project_id: &str) -> Result<Vec<StoredSession>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, label, model_id, last_active_at FROM session
                 WHERE project_id = ?1 ORDER BY last_active_at DESC",
            )?;
            let rows = stmt.query_map(params![project_id], |row| {
                Ok(StoredSession {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    model_id: row.get(2)?,
                    last_active_at: row.get(3)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn load_messages(&self, session_id: &SessionId) -> Result<Vec<ChatMessage>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT role, content, tool_calls_json FROM message
                 WHERE session_id = ?1 ORDER BY seq ASC",
            )?;
            let rows = stmt.query_map(params![session_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (role, content, tools) = row?;
                out.push(decode_message(&role, content, tools.as_deref()));
            }
            Ok(out)
        })
    }

    pub fn usage_report(&self) -> Result<UsageReport> {
        self.with_conn(|conn| {
            let by_project = buckets(
                conn,
                "SELECT COALESCE(p.name, 'unknown'),
                        COALESCE(SUM(u.input_tokens),0),
                        COALESCE(SUM(u.output_tokens),0),
                        COALESCE(SUM(u.estimated_cost),0)
                 FROM usage_record u
                 LEFT JOIN session s ON s.id = u.session_id
                 LEFT JOIN project p ON p.id = s.project_id
                 GROUP BY COALESCE(p.name, 'unknown')
                 ORDER BY 4 DESC",
            )?;
            let by_model = buckets(
                conn,
                "SELECT model_id,
                        COALESCE(SUM(input_tokens),0),
                        COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(estimated_cost),0)
                 FROM usage_record
                 GROUP BY model_id
                 ORDER BY 4 DESC",
            )?;
            let by_day = buckets(
                conn,
                "SELECT substr(created_at, 1, 10),
                        COALESCE(SUM(input_tokens),0),
                        COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(estimated_cost),0)
                 FROM usage_record
                 GROUP BY substr(created_at, 1, 10)
                 ORDER BY 1 DESC",
            )?;
            let (total_input, total_output, total_cost) = conn.query_row(
                "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(estimated_cost),0)
                 FROM usage_record",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get(2)?)),
            )?;
            Ok(UsageReport {
                by_project,
                by_model,
                by_day,
                total_cost,
                total_input,
                total_output,
            })
        })
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn buckets(conn: &Connection, sql: &str) -> Result<Vec<UsageBucket>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(UsageBucket {
            key: row.get(0)?,
            input_tokens: row.get(1)?,
            output_tokens: row.get(2)?,
            cost_usd: row.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn migrate(conn: &Connection, db_path: &Path) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;
    let current: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let pending: Vec<(i32, &str)> = MIGRATIONS
        .iter()
        .copied()
        .filter(|(v, _)| *v > current)
        .collect();
    if !pending.is_empty() && current > 0 {
        backup_db(db_path)?;
    }
    for (version, sql) in pending {
        conn.execute_batch(sql)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![version, Utc::now().to_rfc3339()],
        )?;
        tracing::info!(version, "applied database migration");
    }
    Ok(())
}

fn backup_db(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let bak = path.with_extension("db.bak");
    std::fs::copy(path, &bak)
        .with_context(|| format!("backing up {} to {}", path.display(), bak.display()))?;
    tracing::info!("database backup written to {}", bak.display());
    Ok(())
}

pub fn estimate_cost(
    prompt_price: Option<f64>,
    completion_price: Option<f64>,
    usage: &TokenUsage,
) -> f64 {
    prompt_price.unwrap_or(0.0) * f64::from(usage.prompt_tokens)
        + completion_price.unwrap_or(0.0) * f64::from(usage.completion_tokens)
}

fn flatten_message_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .map(|m| match m {
            ChatMessage::User { content, .. } | ChatMessage::Assistant { content, .. } => {
                content.as_str()
            }
            ChatMessage::ToolResult { content, .. } => content.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(dead_code)]
fn fts_query(raw: &str) -> String {
    raw.split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| {
            let escaped = t.replace('"', "");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn encode_message(message: &ChatMessage) -> (&'static str, String, Option<String>) {
    match message {
        ChatMessage::User { content, images } => {
            let extra = if images.is_empty() {
                None
            } else {
                Some(serde_json::json!({ "images": images }).to_string())
            };
            ("user", content.clone(), extra)
        }
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => {
            let json = if tool_calls.is_empty() {
                None
            } else {
                Some(serde_json::to_string(tool_calls).unwrap_or_else(|_| "[]".into()))
            };
            ("assistant", content.clone(), json)
        }
        ChatMessage::ToolResult {
            call_id,
            content,
            is_error,
        } => (
            "tool_result",
            content.clone(),
            Some(serde_json::json!({ "call_id": call_id, "is_error": is_error }).to_string()),
        ),
    }
}

fn decode_message(role: &str, content: String, tools: Option<&str>) -> ChatMessage {
    match role {
        "assistant" => {
            let tool_calls = tools
                .and_then(|t| serde_json::from_str::<Vec<ToolCall>>(t).ok())
                .unwrap_or_default();
            ChatMessage::Assistant {
                content,
                tool_calls,
            }
        }
        "tool_result" => {
            let (call_id, is_error) = tools
                .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                .map(|v| {
                    (
                        v.get("call_id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false),
                    )
                })
                .unwrap_or_else(|| (String::new(), false));
            ChatMessage::ToolResult {
                call_id,
                content,
                is_error,
            }
        }
        _ => {
            let images = tools
                .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                .and_then(|v| serde_json::from_value(v.get("images")?.clone()).ok())
                .unwrap_or_default();
            ChatMessage::User { content, images }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Db, estimate_cost};
    use crate::providers::{ChatMessage, TokenUsage};
    use crate::session::SessionId;
    use crate::workspace::{FilePatch, Project};
    use tempfile::TempDir;

    fn db() -> (TempDir, Db, Project) {
        let tmp = TempDir::new().unwrap();
        let db = Db::open_at(tmp.path().join("orbit.db")).unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let project = Project::open(&root).unwrap();
        db.upsert_project(&project).unwrap();
        (tmp, db, project)
    }

    #[test]
    fn migrate_empty_and_populated() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("orbit.db");
        let first = Db::open_at(&path).unwrap();
        drop(first);
        let second = Db::open_at(&path).unwrap();
        assert!(second.path().exists());
        second
            .with_conn(|conn| {
                let n: i32 =
                    conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))?;
                assert_eq!(n, 4);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn messages_survive_round_trip() {
        let (_tmp, db, project) = db();
        let id = SessionId::new("s1");
        db.upsert_session(&project.id, &id, "lab", "model-a")
            .unwrap();
        let messages = vec![
            ChatMessage::user("hi"),
            ChatMessage::Assistant {
                content: "hello".into(),
                tool_calls: Vec::new(),
            },
        ];
        db.replace_messages(&id, &messages).unwrap();
        db.insert_tool_call(
            &id,
            "c1",
            "grep",
            &serde_json::json!({"pattern": "x"}),
            "completed",
            "ok",
        )
        .unwrap();
        let loaded = db.load_messages(&id).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(matches!(loaded[0], ChatMessage::User { .. }));
        assert!(matches!(
            loaded[1],
            ChatMessage::Assistant { ref content, .. } if content == "hello"
        ));
    }

    #[test]
    fn pending_patch_round_trips() {
        let (_tmp, db, project) = db();
        let id = SessionId::new("s1");
        db.upsert_session(&project.id, &id, "lab", "m").unwrap();
        let patch = FilePatch::new("a.rs".into(), "old".into(), "new".into());
        db.upsert_file_change(&project.id, &id, &patch).unwrap();
        let pending = db.pending_patches(&project.id).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].proposed_content, "new");
    }

    #[test]
    fn usage_aggregates_match_rows() {
        let (_tmp, db, project) = db();
        let id = SessionId::new("s1");
        db.upsert_session(&project.id, &id, "lab", "model-a")
            .unwrap();
        let u1 = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 20,
            total_tokens: 120,
            cached_tokens: 0,
        };
        let u2 = TokenUsage {
            prompt_tokens: 50,
            completion_tokens: 10,
            total_tokens: 60,
            cached_tokens: 0,
        };
        db.insert_usage(&id, "model-a", &u1, 0.10, Some(80))
            .unwrap();
        db.insert_usage(&id, "model-a", &u2, 0.05, Some(40))
            .unwrap();
        let totals = db.session_usage(&id).unwrap();
        assert_eq!(totals.input_tokens, 150);
        assert_eq!(totals.output_tokens, 30);
        assert!((totals.cost_usd - 0.15).abs() < 1e-9);
        let report = db.usage_report().unwrap();
        assert!((report.total_cost - 0.15).abs() < 1e-9);
        assert_eq!(report.total_input, 150);
        assert_eq!(report.by_model[0].key, "model-a");
        assert_eq!(report.by_project[0].key, project.name);
    }

    #[test]
    fn estimate_cost_uses_catalog_prices() {
        let usage = TokenUsage {
            prompt_tokens: 1_000,
            completion_tokens: 500,
            total_tokens: 1_500,
            cached_tokens: 0,
        };
        let cost = estimate_cost(Some(0.000001), Some(0.000002), &usage);
        assert!((cost - 0.002).abs() < 1e-12);
    }

    #[test]
    fn fts_finds_body_only_term() {
        let (_tmp, db, project) = db();
        let id = SessionId::new("s-search");
        db.upsert_session(&project.id, &id, "lab", "m").unwrap();
        db.replace_messages(
            &id,
            &[ChatMessage::user("the unique zebra token lives here")],
        )
        .unwrap();
        let hits = db.search_history("zebra", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item_id, "s-search");
        assert_eq!(hits[0].source, "session");
        assert_eq!(hits[0].title, "lab");
        assert!(hits[0].snippet.to_lowercase().contains("zebra"));
    }
}
