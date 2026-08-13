//! Pure history mutations for Chat and Coder message actions (M2.1).

use crate::app::{Message, Role};
use crate::providers::ChatMessage;
use crate::session::TranscriptItem;
use chrono::{DateTime, Local, Utc};

/// Inclusive start / exclusive end of the Q/A pair that contains `index`.
pub fn chat_pair_range(messages: &[Message], index: usize) -> Option<std::ops::Range<usize>> {
    let msg = messages.get(index)?;
    match msg.role {
        Role::User => {
            let end = if messages
                .get(index + 1)
                .is_some_and(|m| matches!(m.role, Role::Assistant))
            {
                index + 2
            } else {
                index + 1
            };
            Some(index..end)
        }
        Role::Assistant => {
            let start = if index > 0 && matches!(messages[index - 1].role, Role::User) {
                index - 1
            } else {
                index
            };
            Some(start..index + 1)
        }
    }
}

pub fn delete_chat_pair(messages: &mut Vec<Message>, index: usize) -> usize {
    let Some(range) = chat_pair_range(messages, index) else {
        return 0;
    };
    let count = range.end - range.start;
    messages.drain(range);
    count
}

/// Replace the user message at `user_index` and drop everything after it.
/// Returns how many messages were discarded (not counting the edited one).
pub fn truncate_chat_from(
    messages: &mut Vec<Message>,
    user_index: usize,
    new_text: String,
) -> usize {
    if user_index >= messages.len() || !matches!(messages[user_index].role, Role::User) {
        return 0;
    }
    let discarded = messages.len().saturating_sub(user_index + 1);
    messages.truncate(user_index + 1);
    if let Some(msg) = messages.get_mut(user_index) {
        msg.content = new_text;
        msg.interrupted = false;
    }
    discarded
}

/// Drop the last assistant reply so the last user turn can be resent.
pub fn discard_last_chat_assistant(messages: &mut Vec<Message>) -> bool {
    if matches!(messages.last().map(|m| &m.role), Some(Role::Assistant)) {
        messages.pop();
        true
    } else {
        false
    }
}

pub fn needs_confirm(affected: usize) -> bool {
    affected > 1
}

/// Turn that contains `index`: from the nearest preceding User through the
/// next User (exclusive). Includes tool rows and approvals.
pub fn coder_turn_range(items: &[TranscriptItem], index: usize) -> Option<std::ops::Range<usize>> {
    if index >= items.len() {
        return None;
    }
    let start = (0..=index)
        .rev()
        .find(|&i| matches!(items[i], TranscriptItem::User(_)))?;
    let end = ((index + 1)..items.len())
        .find(|&i| matches!(items[i], TranscriptItem::User(_)))
        .unwrap_or(items.len());
    Some(start..end)
}

pub fn last_user_transcript(items: &[TranscriptItem]) -> Option<usize> {
    items
        .iter()
        .rposition(|item| matches!(item, TranscriptItem::User(_)))
}

pub fn last_user_message(msgs: &[ChatMessage]) -> Option<usize> {
    msgs.iter()
        .rposition(|m| matches!(m, ChatMessage::User { .. }))
}

pub fn user_ordinal_in_transcript(items: &[TranscriptItem], index: usize) -> Option<usize> {
    if !matches!(items.get(index), Some(TranscriptItem::User(_))) {
        return None;
    }
    Some(
        items[..=index]
            .iter()
            .filter(|item| matches!(item, TranscriptItem::User(_)))
            .count()
            - 1,
    )
}

pub fn nth_user_message(msgs: &[ChatMessage], n: usize) -> Option<usize> {
    msgs.iter()
        .enumerate()
        .filter(|(_, m)| matches!(m, ChatMessage::User { .. }))
        .nth(n)
        .map(|(i, _)| i)
}

/// Discard the last agent turn (everything after the last user), keeping the user.
pub fn discard_last_coder_turn(items: &mut Vec<TranscriptItem>) -> usize {
    let Some(user) = last_user_transcript(items) else {
        return 0;
    };
    let discarded = items.len().saturating_sub(user + 1);
    items.truncate(user + 1);
    discarded
}

pub fn discard_after_user_message(msgs: &mut Vec<ChatMessage>) -> usize {
    let Some(user) = last_user_message(msgs) else {
        return 0;
    };
    let discarded = msgs.len().saturating_sub(user + 1);
    msgs.truncate(user + 1);
    discarded
}

pub fn truncate_coder_from_user(
    items: &mut Vec<TranscriptItem>,
    user_index: usize,
    new_text: String,
) -> usize {
    if !matches!(items.get(user_index), Some(TranscriptItem::User(_))) {
        return 0;
    }
    let discarded = items.len().saturating_sub(user_index + 1);
    items.truncate(user_index + 1);
    if let Some(TranscriptItem::User(text)) = items.get_mut(user_index) {
        *text = new_text;
    }
    discarded
}

pub fn delete_coder_turn(items: &mut Vec<TranscriptItem>, index: usize) -> usize {
    let Some(range) = coder_turn_range(items, index) else {
        return 0;
    };
    let count = range.end - range.start;
    items.drain(range);
    count
}

/// Remove the Nth user message and everything up to the next user.
pub fn delete_coder_messages_turn(msgs: &mut Vec<ChatMessage>, user_ordinal: usize) -> usize {
    let Some(start) = nth_user_message(msgs, user_ordinal) else {
        return 0;
    };
    let end = nth_user_message(msgs, user_ordinal + 1).unwrap_or(msgs.len());
    let count = end - start;
    msgs.drain(start..end);
    count
}

/// Edit the Nth user message and drop later history.
pub fn truncate_coder_messages_from(
    msgs: &mut Vec<ChatMessage>,
    user_ordinal: usize,
    new_text: String,
) -> usize {
    let Some(start) = nth_user_message(msgs, user_ordinal) else {
        return 0;
    };
    let discarded = msgs.len().saturating_sub(start + 1);
    msgs.truncate(start + 1);
    if let ChatMessage::User { content, .. } = &mut msgs[start] {
        *content = new_text;
    }
    discarded
}

/// Which user-turn a transcript row belongs to.
pub fn turn_user_ordinal(items: &[TranscriptItem], index: usize) -> Option<usize> {
    let range = coder_turn_range(items, index)?;
    user_ordinal_in_transcript(items, range.start)
}

/// Fenced code blocks as `(language, body)`.
#[cfg(test)]
pub fn extract_code_blocks(md: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = md;
    while let Some(start) = rest.find("```") {
        rest = &rest[start + 3..];
        let (lang, after_lang) = match rest.find('\n') {
            Some(n) => (rest[..n].trim().to_string(), &rest[n + 1..]),
            None => break,
        };
        if let Some(end) = after_lang.find("```") {
            out.push((lang, after_lang[..end].trim_end_matches('\n').to_string()));
            rest = &after_lang[end + 3..];
        } else {
            break;
        }
    }
    out
}

/// Split markdown into prose and fenced code so each fence can grow a Copy button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdPart<'a> {
    Text(&'a str),
    Code { lang: &'a str, body: &'a str },
}

pub fn split_fenced_code(src: &str) -> Vec<MdPart<'_>> {
    let mut parts = Vec::new();
    let mut rest = src;
    while let Some(start) = rest.find("```") {
        if start > 0 {
            parts.push(MdPart::Text(&rest[..start]));
        }
        let after = &rest[start + 3..];
        let Some(nl) = after.find('\n') else {
            parts.push(MdPart::Text(&rest[start..]));
            return parts;
        };
        let lang = after[..nl].trim();
        let body_start = &after[nl + 1..];
        if let Some(end) = body_start.find("```") {
            let body = body_start[..end].trim_end_matches('\n');
            parts.push(MdPart::Code { lang, body });
            rest = &body_start[end + 3..];
            if rest.starts_with('\n') {
                rest = &rest[1..];
            }
        } else {
            parts.push(MdPart::Text(&rest[start..]));
            return parts;
        }
    }
    if !rest.is_empty() {
        parts.push(MdPart::Text(rest));
    }
    parts
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DateGroup {
    Pinned,
    Today,
    Yesterday,
    ThisWeek,
    Older,
}

impl DateGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pinned => "Pinned",
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::ThisWeek => "This week",
            Self::Older => "Older",
        }
    }
}

pub fn date_group(created: DateTime<Utc>, now: DateTime<Utc>) -> DateGroup {
    let created = created.with_timezone(&Local).date_naive();
    let today = now.with_timezone(&Local).date_naive();
    let days = (today - created).num_days();
    match days {
        ..=-1 | 0 => DateGroup::Today,
        1 => DateGroup::Yesterday,
        2..=6 => DateGroup::ThisWeek,
        _ => DateGroup::Older,
    }
}

pub fn chat_matches(title: &str, messages: &[Message], query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    if title.to_lowercase().contains(&q) {
        return true;
    }
    messages
        .iter()
        .any(|m| m.content.to_lowercase().contains(&q))
}

pub fn search_snippet(haystack: &str, needle: &str, radius: usize) -> Option<String> {
    let q = needle.trim();
    if q.is_empty() {
        return None;
    }
    let lower = haystack.to_lowercase();
    let q_lower = q.to_lowercase();
    let pos = lower.find(&q_lower)?;
    let start = floor_char_boundary(haystack, pos.saturating_sub(radius));
    let end = ceil_char_boundary(haystack, (pos + q.len() + radius).min(haystack.len()));
    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.push_str(haystack[start..end].trim());
    if end < haystack.len() {
        snippet.push('…');
    }
    Some(snippet)
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Strip API-key shaped tokens so exports never leak credentials.
pub fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find("sk-or-v1-") {
        out.push_str(&rest[..idx]);
        out.push_str("sk-or-v1-••••");
        let after = &rest[idx + "sk-or-v1-".len()..];
        let skip = after
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .unwrap_or(after.len());
        rest = &after[skip..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Message;
    use chrono::TimeZone;

    fn msg(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.into(),
            appeared_at: None,
            interrupted: false,
            images: Vec::new(),
        }
    }

    #[test]
    fn delete_pair_removes_user_and_assistant() {
        let mut messages = vec![
            msg(Role::User, "a"),
            msg(Role::Assistant, "b"),
            msg(Role::User, "c"),
            msg(Role::Assistant, "d"),
        ];
        assert_eq!(delete_chat_pair(&mut messages, 0), 2);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "c");
    }

    #[test]
    fn delete_pair_from_assistant_index() {
        let mut messages = vec![msg(Role::User, "a"), msg(Role::Assistant, "b")];
        assert_eq!(delete_chat_pair(&mut messages, 1), 2);
        assert!(messages.is_empty());
    }

    #[test]
    fn edit_truncates_following_history() {
        let mut messages = vec![
            msg(Role::User, "old"),
            msg(Role::Assistant, "ans"),
            msg(Role::User, "later"),
        ];
        let discarded = truncate_chat_from(&mut messages, 0, "new".into());
        assert_eq!(discarded, 2);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "new");
        assert!(needs_confirm(discarded));
    }

    #[test]
    fn regenerate_drops_only_last_assistant() {
        let mut messages = vec![msg(Role::User, "q"), msg(Role::Assistant, "a")];
        assert!(discard_last_chat_assistant(&mut messages));
        assert_eq!(messages.len(), 1);
        assert!(!needs_confirm(1));
    }

    #[test]
    fn coder_turn_includes_tools() {
        let items = vec![
            TranscriptItem::User("fix it".into()),
            TranscriptItem::Assistant("ok".into()),
            TranscriptItem::Tool {
                call_id: "1".into(),
                name: "read_file".into(),
                summary: "read".into(),
                output: "fn main".into(),
                is_error: false,
                running: false,
                expanded: false,
            },
            TranscriptItem::User("again".into()),
        ];
        assert_eq!(coder_turn_range(&items, 2), Some(0..3));
        assert_eq!(coder_turn_range(&items, 3), Some(3..4));
        assert_eq!(user_ordinal_in_transcript(&items, 3), Some(1));
    }

    #[test]
    fn split_and_extract_code_fences() {
        let md = "intro\n```rust\nfn x() {}\n```\noutro";
        let blocks = extract_code_blocks(md);
        assert_eq!(blocks, vec![("rust".into(), "fn x() {}".into())]);
        let parts = split_fenced_code(md);
        assert!(matches!(parts[0], MdPart::Text(t) if t.starts_with("intro")));
        assert!(matches!(parts[1], MdPart::Code { lang: "rust", .. }));
    }

    #[test]
    fn date_groups_today_yesterday_week() {
        let now = Utc.with_ymd_and_hms(2026, 8, 13, 15, 0, 0).unwrap();
        let today = Utc.with_ymd_and_hms(2026, 8, 13, 8, 0, 0).unwrap();
        let yesterday = Utc.with_ymd_and_hms(2026, 8, 12, 8, 0, 0).unwrap();
        let week = Utc.with_ymd_and_hms(2026, 8, 10, 8, 0, 0).unwrap();
        let older = Utc.with_ymd_and_hms(2026, 7, 1, 8, 0, 0).unwrap();
        assert_eq!(date_group(today, now), DateGroup::Today);
        assert_eq!(date_group(yesterday, now), DateGroup::Yesterday);
        assert_eq!(date_group(week, now), DateGroup::ThisWeek);
        assert_eq!(date_group(older, now), DateGroup::Older);
    }

    #[test]
    fn search_finds_body_only_term() {
        let messages = vec![msg(Role::User, "the unique zebra token")];
        assert!(chat_matches("New chat", &messages, "zebra"));
        assert!(!chat_matches("New chat", &messages, "giraffe"));
        let snippet = search_snippet(&messages[0].content, "zebra", 6).unwrap();
        assert!(snippet.to_lowercase().contains("zebra"));
    }

    #[test]
    fn redacts_openrouter_keys() {
        let text = "key=sk-or-v1-abcdefghijklmnop leftover";
        let redacted = redact_secrets(text);
        assert!(!redacted.contains("abcdefghijklmnop"));
        assert!(redacted.contains("sk-or-v1-••••"));
        assert!(redacted.contains("leftover"));
    }

    #[test]
    fn five_hundred_chats_search_under_200ms() {
        let chats: Vec<(String, Vec<Message>)> = (0..500)
            .map(|i| {
                let body = if i == 317 {
                    "needle-only-in-body-xyz".to_string()
                } else {
                    format!("ordinary message {i}")
                };
                (format!("Chat {i}"), vec![msg(Role::User, &body)])
            })
            .collect();
        let start = std::time::Instant::now();
        let hits: Vec<usize> = chats
            .iter()
            .enumerate()
            .filter(|(_, (title, msgs))| chat_matches(title, msgs, "needle-only-in-body-xyz"))
            .map(|(i, _)| i)
            .collect();
        let elapsed = start.elapsed();
        assert_eq!(hits, vec![317]);
        assert!(
            elapsed.as_millis() < 200,
            "search took {}ms",
            elapsed.as_millis()
        );
    }
}
