//! Incremental Server-Sent Events parser.
//!
//! Decision: a small custom parser instead of `eventsource-stream` 0.2.3.
//! OpenRouter emits `: OPENROUTER PROCESSING` keep-alive comments and a
//! `data: [DONE]` sentinel. The crate handles the common SSE path, but it
//! hides comment events and makes it awkward to unit-test UTF-8 fragments
//! split across HTTP chunks. ~80 lines here keep those cases explicit.

use std::str;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseEvent {
    /// A `data:` field (joined if the event had several).
    Data(String),
    /// The OpenRouter stream terminator `data: [DONE]`.
    Done,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SseError {
    #[error("SSE stream contained invalid UTF-8")]
    InvalidUtf8,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the next HTTP body chunk. Returns every complete event found.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        self.buffer.extend_from_slice(chunk);
        self.drain_events()
    }

    /// Flush a trailing event that was not terminated by a blank line.
    pub fn finish(&mut self) -> Result<Vec<SseEvent>, SseError> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }
        if !self.buffer.ends_with(b"\n\n") && !self.buffer.ends_with(b"\r\n\r\n") {
            self.buffer.extend_from_slice(b"\n\n");
        }
        self.drain_events()
    }

    fn drain_events(&mut self) -> Result<Vec<SseEvent>, SseError> {
        let mut events = Vec::new();
        while let Some(split_at) = find_event_boundary(&self.buffer) {
            let raw: Vec<u8> = self.buffer.drain(..split_at).collect();
            // Drop the blank-line separator that follows the event.
            if self.buffer.starts_with(b"\r\n\r\n") {
                self.buffer.drain(..4);
            } else if self.buffer.starts_with(b"\n\n") {
                self.buffer.drain(..2);
            }
            if let Some(event) = parse_event(&raw)? {
                events.push(event);
            }
        }
        Ok(events)
    }
}

fn find_event_boundary(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n"))
}

fn parse_event(raw: &[u8]) -> Result<Option<SseEvent>, SseError> {
    let text = str::from_utf8(raw).map_err(|_| SseError::InvalidUtf8)?;
    let mut data_lines: Vec<&str> = Vec::new();

    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let value = if let Some(stripped) = rest.strip_prefix(' ') {
                stripped
            } else {
                rest
            };
            data_lines.push(value);
        }
        // Other fields (`event:`, `id:`, `retry:`) are ignored.
    }

    if data_lines.is_empty() {
        return Ok(None);
    }
    if data_lines.len() == 1 && data_lines[0] == "[DONE]" {
        return Ok(Some(SseEvent::Done));
    }
    Ok(Some(SseEvent::Data(data_lines.join("\n"))))
}

#[cfg(test)]
mod tests {
    use super::{SseEvent, SseParser};

    #[test]
    fn single_chunk_event() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: {\"delta\":\"hi\"}\n\n").expect("parse");
        assert_eq!(events, vec![SseEvent::Data("{\"delta\":\"hi\"}".into())]);
    }

    #[test]
    fn event_split_across_three_chunks() {
        let mut parser = SseParser::new();
        assert!(parser.push(b"data: {\"x\"").unwrap().is_empty());
        assert!(parser.push(b":1").unwrap().is_empty());
        let events = parser.push(b"}\n\n").unwrap();
        assert_eq!(events, vec![SseEvent::Data("{\"x\":1}".into())]);
    }

    #[test]
    fn keep_alive_comment_is_ignored() {
        let mut parser = SseParser::new();
        let events = parser.push(b": OPENROUTER PROCESSING\n\n").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn done_sentinel() {
        let mut parser = SseParser::new();
        let events = parser.push(b"data: [DONE]\n\n").unwrap();
        assert_eq!(events, vec![SseEvent::Done]);
    }

    #[test]
    fn multibyte_character_split_on_chunk_boundary() {
        // U+00E9 LATIN SMALL LETTER E WITH ACUTE is [0xC3, 0xA9].
        let mut parser = SseParser::new();
        let first = b"data: caf\xc3";
        let second = b"\xa9\n\n";
        assert!(parser.push(first).unwrap().is_empty());
        let events = parser.push(second).unwrap();
        assert_eq!(events, vec![SseEvent::Data("caf\u{e9}".into())]);
    }

    #[test]
    fn comment_then_data_in_same_feed() {
        let mut parser = SseParser::new();
        let events = parser
            .push(b": OPENROUTER PROCESSING\n\ndata: hello\n\ndata: [DONE]\n\n")
            .unwrap();
        assert_eq!(events, vec![SseEvent::Data("hello".into()), SseEvent::Done]);
    }
}
