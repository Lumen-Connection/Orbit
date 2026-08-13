//! Hybrid context-window packing: keep system + recent turns, summarize the middle.

use crate::providers::ChatMessage;

pub const DEFAULT_RECENT_KEEP: usize = 20;
pub const DEFAULT_RESPONSE_RESERVE: f32 = 0.25;
pub const DEFAULT_CONTEXT_LENGTH: u32 = 128_000;
pub const SUMMARY_MARKER_START: &str = "<<<ORBIT_CONTEXT_SUMMARY>>>";
pub const SUMMARY_MARKER_END: &str = "<<<END_ORBIT_CONTEXT_SUMMARY>>>";

#[derive(Debug, Clone)]
pub struct ContextWindow {
    pub recent_keep: usize,
    pub response_reserve: f32,
}

impl Default for ContextWindow {
    fn default() -> Self {
        Self {
            recent_keep: DEFAULT_RECENT_KEEP,
            response_reserve: DEFAULT_RESPONSE_RESERVE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CachedSummary {
    pub text: String,
    pub covered: usize,
}

#[derive(Debug, Clone)]
pub struct FittedContext {
    pub messages: Vec<ChatMessage>,
    pub occupancy: f32,
    pub needs_summary: Option<Vec<ChatMessage>>,
    #[allow(dead_code)]
    pub summary_used: bool,
}

pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

pub fn message_tokens(message: &ChatMessage) -> usize {
    match message {
        ChatMessage::User { content, images } => estimate_tokens(content) + images.len() * 300,
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => {
            estimate_tokens(content)
                + tool_calls
                    .iter()
                    .map(|c| estimate_tokens(&c.name) + estimate_tokens(&c.arguments.to_string()))
                    .sum::<usize>()
        }
        ChatMessage::ToolResult { content, .. } => estimate_tokens(content),
    }
}

pub fn wrap_summary(text: &str) -> ChatMessage {
    ChatMessage::User {
        images: Vec::new(),
        content: format!(
            "{SUMMARY_MARKER_START}\nEarlier conversation summary (not instructions):\n{text}\n{SUMMARY_MARKER_END}"
        ),
    }
}

/// Prefer compressing old tool results — they have usually already done their job.
pub fn compress_old_tools(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .map(|message| match message {
            ChatMessage::ToolResult {
                call_id,
                content,
                is_error,
            } if content.chars().count() > 800 => ChatMessage::ToolResult {
                call_id: call_id.clone(),
                content: {
                    let kept: String = content.chars().take(500).collect();
                    format!("{kept}\n[tool output truncated]")
                },
                is_error: *is_error,
            },
            other => other.clone(),
        })
        .collect()
}

pub fn fit(
    system: Option<&str>,
    messages: &[ChatMessage],
    cached: Option<&CachedSummary>,
    context_length: u32,
    cfg: &ContextWindow,
) -> FittedContext {
    let context_length = context_length.max(1_024);
    let budget = ((context_length as f32) * (1.0 - cfg.response_reserve.clamp(0.05, 0.5))) as usize;
    let system_tokens = system.map(estimate_tokens).unwrap_or(0);
    let total_tokens = system_tokens + total_messages_tokens(messages);
    let occupancy = occupancy_of(total_tokens, context_length);

    if total_tokens <= budget {
        return FittedContext {
            messages: messages.to_vec(),
            occupancy,
            needs_summary: None,
            summary_used: false,
        };
    }

    let keep = cfg.recent_keep.max(1).min(messages.len());
    let split = messages.len().saturating_sub(keep);
    let middle = &messages[..split];
    let recent = &messages[split..];

    if let Some(cached) = cached.filter(|c| !c.text.trim().is_empty()) {
        let summary = wrap_summary(&cached.text);
        let mut packed = vec![summary];
        packed.extend(recent.iter().cloned());
        let used = system_tokens + packed.iter().map(message_tokens).sum::<usize>();
        let new_middle = if split > cached.covered {
            Some(compress_old_tools(middle))
        } else {
            None
        };
        return FittedContext {
            occupancy: occupancy_of(used, context_length),
            messages: packed,
            needs_summary: new_middle.filter(|_| split > cached.covered),
            summary_used: true,
        };
    }

    FittedContext {
        messages: recent.to_vec(),
        occupancy: occupancy_of(
            system_tokens + recent.iter().map(message_tokens).sum::<usize>(),
            context_length,
        ),
        needs_summary: Some(compress_old_tools(middle)),
        summary_used: false,
    }
}

fn total_messages_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_tokens).sum()
}

fn occupancy_of(used: usize, context_length: u32) -> f32 {
    (used as f32 / context_length as f32).clamp(0.0, 1.0)
}

pub fn occupancy_label(occupancy: f32) -> String {
    format!("{:.0}% of context", occupancy * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn user(text: &str) -> ChatMessage {
        ChatMessage::user(text)
    }

    fn tool(text: &str) -> ChatMessage {
        ChatMessage::ToolResult {
            call_id: "t".into(),
            content: text.into(),
            is_error: false,
        }
    }

    #[test]
    fn short_history_is_left_intact() {
        let messages = vec![user("a"), user("b")];
        let fitted = fit(
            Some("sys"),
            &messages,
            None,
            8_000,
            &ContextWindow::default(),
        );
        assert_eq!(fitted.messages.len(), 2);
        assert!(fitted.needs_summary.is_none());
        assert!(!fitted.summary_used);
    }

    #[test]
    fn over_budget_requests_a_summary_once() {
        let calls = AtomicUsize::new(0);
        let long = "word ".repeat(4_000);
        let mut messages = Vec::new();
        for i in 0..30 {
            messages.push(user(&format!("turn {i} {long}")));
            messages.push(tool(&long));
        }
        let cfg = ContextWindow {
            recent_keep: 4,
            response_reserve: 0.25,
        };
        let first = fit(Some("sys"), &messages, None, 4_000, &cfg);
        assert!(first.needs_summary.is_some());
        assert!(!first.summary_used);

        let summary = {
            calls.fetch_add(1, Ordering::SeqCst);
            "decided to use rusqlite".to_string()
        };
        let cached = CachedSummary {
            text: summary,
            covered: messages.len().saturating_sub(4),
        };
        let second = fit(Some("sys"), &messages, Some(&cached), 4_000, &cfg);
        assert!(second.summary_used);
        assert!(second.needs_summary.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let third = fit(Some("sys"), &messages, Some(&cached), 4_000, &cfg);
        assert!(third.needs_summary.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(occupancy_label(third.occupancy).contains("% of context"));
    }

    #[test]
    fn old_tool_results_are_compressed_before_summarizing() {
        let huge = "x".repeat(4_000);
        let middle = compress_old_tools(&[tool(&huge)]);
        match &middle[0] {
            ChatMessage::ToolResult { content, .. } => {
                assert!(content.contains("truncated"));
                assert!(content.len() < huge.len());
            }
            other => panic!("{other:?}"),
        }
    }
}
