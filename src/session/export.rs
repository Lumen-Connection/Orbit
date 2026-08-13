//! Export a Chat or Coder session as GitHub-flavored Markdown.

use crate::app::{Chat, Message, Role};
use crate::session::TranscriptItem;
use crate::session::message_ops::redact_secrets;
use chrono::{DateTime, Utc};

pub struct ExportMeta<'a> {
    pub title: &'a str,
    pub project: Option<&'a str>,
    pub model: &'a str,
    pub date: DateTime<Utc>,
    pub cost_usd: Option<f64>,
}

pub fn render_header(meta: &ExportMeta<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", escape_heading(meta.title)));
    if let Some(project) = meta.project {
        out.push_str(&format!("- **Project:** {}\n", project));
    }
    out.push_str(&format!("- **Model:** {}\n", meta.model));
    out.push_str(&format!(
        "- **Date:** {}\n",
        meta.date.format("%Y-%m-%d %H:%M UTC")
    ));
    if let Some(cost) = meta.cost_usd {
        out.push_str(&format!("- **Cost:** ${cost:.4}\n"));
    }
    out.push('\n');
    out
}

pub fn chat_to_markdown(chat: &Chat) -> String {
    let mut out = render_header(&ExportMeta {
        title: &chat.title,
        project: None,
        model: &chat.model,
        date: chat.created_at,
        cost_usd: None,
    });
    if let Some(system) = chat.request_system() {
        out.push_str("<details>\n<summary>System prompt</summary>\n\n");
        out.push_str(&fenced(&redact_secrets(&system)));
        out.push_str("\n</details>\n\n");
    }
    for msg in &chat.messages {
        out.push_str(&format_chat_message(msg));
    }
    out
}

pub fn transcript_to_markdown(
    meta: &ExportMeta<'_>,
    items: &[TranscriptItem],
    system: Option<&str>,
) -> String {
    let mut out = render_header(meta);
    if let Some(system) = system.filter(|s| !s.trim().is_empty()) {
        out.push_str("<details>\n<summary>System prompt</summary>\n\n");
        out.push_str(&fenced(&redact_secrets(system)));
        out.push_str("\n</details>\n\n");
    }
    for item in items {
        match item {
            TranscriptItem::User(text) => {
                out.push_str("## User\n\n");
                out.push_str(&redact_secrets(text));
                out.push_str("\n\n");
            }
            TranscriptItem::Assistant(text) if !text.is_empty() => {
                out.push_str("## Assistant\n\n");
                out.push_str(&redact_secrets(text));
                out.push_str("\n\n");
            }
            TranscriptItem::Assistant(_) => {}
            TranscriptItem::Tool {
                name,
                summary,
                output,
                is_error,
                ..
            } => {
                let mark = if *is_error { " (error)" } else { "" };
                out.push_str(&format!(
                    "<details>\n<summary>Tool: {name} — {summary}{mark}</summary>\n\n"
                ));
                out.push_str(&fenced(&redact_secrets(output)));
                out.push_str("\n</details>\n\n");
            }
            TranscriptItem::Approval { handle, resolved } => {
                let status = match resolved {
                    None => "pending",
                    Some(crate::security::ApprovalDecision::Approved) => "approved",
                    Some(crate::security::ApprovalDecision::Denied) => "denied",
                };
                out.push_str(&format!(
                    "<details>\n<summary>Approval: {} ({status})</summary>\n\n",
                    handle.summary
                ));
                if let Some(patch) = &handle.patch {
                    out.push_str("```diff\n");
                    out.push_str(&redact_secrets(&patch.unified_diff));
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("```\n");
                }
                out.push_str("\n</details>\n\n");
            }
        }
    }
    out
}

fn format_chat_message(msg: &Message) -> String {
    let mut out = String::new();
    match msg.role {
        Role::User => out.push_str("## User\n\n"),
        Role::Assistant => out.push_str("## Assistant\n\n"),
    }
    if !msg.content.is_empty() {
        out.push_str(&redact_secrets(&msg.content));
        out.push_str("\n\n");
    }
    if !msg.images.is_empty() {
        out.push_str(&format!("*({} image attachment(s))*\n\n", msg.images.len()));
    }
    if msg.interrupted {
        out.push_str("*Interrupted*\n\n");
    }
    out
}

fn fenced(body: &str) -> String {
    format!("```\n{}\n```\n", body.trim_end())
}

fn escape_heading(title: &str) -> String {
    title.replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Chat;
    use crate::session::{ApprovalHandle, TranscriptItem};
    use crate::workspace::FilePatch;
    use chrono::TimeZone;
    use std::path::PathBuf;
    use uuid::Uuid;

    #[test]
    fn chat_export_renders_on_github() {
        let mut chat = Chat::new("openai/gpt-4.1".into());
        chat.id = Uuid::nil();
        chat.title = "Fix login".into();
        chat.created_at = Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap();
        chat.system = Some("Be terse. key=sk-or-v1-SUPERSECRET99".into());
        chat.messages.push(Message {
            role: Role::User,
            content: "Why does auth fail?".into(),
            appeared_at: None,
            interrupted: false,
            images: Vec::new(),
        });
        chat.messages.push(Message {
            role: Role::Assistant,
            content: "Check the token.".into(),
            appeared_at: None,
            interrupted: false,
            images: Vec::new(),
        });
        let md = chat_to_markdown(&chat);
        assert!(md.starts_with("# Fix login"));
        assert!(md.contains("**Model:** openai/gpt-4.1"));
        assert!(md.contains("## User"));
        assert!(md.contains("## Assistant"));
        assert!(md.contains("<details>"));
        assert!(!md.contains("SUPERSECRET99"));
        assert!(md.contains("sk-or-v1-••••"));
    }

    #[test]
    fn coder_export_collapses_tools_and_diffs() {
        let items = vec![
            TranscriptItem::User("patch it".into()),
            TranscriptItem::Assistant("sure".into()),
            TranscriptItem::Tool {
                call_id: "1".into(),
                name: "read_file".into(),
                summary: "src/lib.rs".into(),
                output: "fn x() {}".into(),
                is_error: false,
                running: false,
                expanded: false,
            },
            TranscriptItem::Approval {
                handle: ApprovalHandle {
                    id: crate::security::ApprovalId::new(),
                    tool_name: "write_file".into(),
                    summary: "src/lib.rs".into(),
                    patch: Some(FilePatch {
                        relative_path: PathBuf::from("src/lib.rs"),
                        original_hash: "abc".into(),
                        original_content: "secret".into(),
                        proposed_content: "new".into(),
                        unified_diff: "-old\n+new\n".into(),
                        status: crate::workspace::PatchStatus::Pending,
                    }),
                    command: None,
                },
                resolved: None,
            },
        ];
        let md = transcript_to_markdown(
            &ExportMeta {
                title: "Session 1",
                project: Some("orbit"),
                model: "anthropic/claude-sonnet-4",
                date: Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap(),
                cost_usd: Some(0.0123),
            },
            &items,
            None,
        );
        assert!(md.contains("**Project:** orbit"));
        assert!(md.contains("**Cost:** $0.0123"));
        assert!(md.contains("<details>"));
        assert!(md.contains("```diff"));
        assert!(md.contains("-old"));
        assert!(!md.contains("secret"));
    }
}
