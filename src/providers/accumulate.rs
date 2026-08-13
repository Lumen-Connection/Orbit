//! Rebuilds a streamed assistant message, including fragmented tool-call JSON.
#![allow(dead_code)]
//!
//! OpenAI-compatible APIs emit `tool_calls[i].function.arguments` as successive
//! string fragments keyed by `index`. Two parallel calls can interleave. JSON
//! is parsed only in [`AssistantAccumulator::finish`].

use super::{FinishReason, ProviderEvent, ToolCall, ToolCallId};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AccumulateError {
    #[error("tool call {index} is missing an id")]
    MissingId { index: usize },
    #[error("tool call {index} is missing a name")]
    MissingName { index: usize },
    #[error("tool call {index} arguments are not valid JSON: {detail}")]
    InvalidArgs { index: usize, detail: String },
}

#[derive(Debug, Default)]
struct ToolSlot {
    id: Option<ToolCallId>,
    name: Option<String>,
    args: String,
}

#[derive(Debug, Default)]
pub struct AssistantAccumulator {
    text: String,
    tools: BTreeMap<usize, ToolSlot>,
    finish: Option<FinishReason>,
    usage: Option<super::TokenUsage>,
}

#[derive(Debug, Clone)]
pub struct FinishedAssistant {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish: FinishReason,
    pub usage: Option<super::TokenUsage>,
}

impl AssistantAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_text(&mut self, text: &str) {
        self.text.push_str(text);
    }

    pub fn push_tool_delta(
        &mut self,
        index: usize,
        id: Option<ToolCallId>,
        name: Option<String>,
        args_delta: &str,
    ) {
        let slot = self.tools.entry(index).or_default();
        if let Some(id) = id {
            slot.id = Some(id);
        }
        if let Some(name) = name {
            slot.name = Some(name);
        }
        slot.args.push_str(args_delta);
    }

    pub fn push_event(&mut self, event: ProviderEvent) {
        match event {
            ProviderEvent::TextDelta(text) => self.push_text(&text),
            ProviderEvent::ToolCallDelta {
                index,
                id,
                name,
                args_delta,
            } => self.push_tool_delta(index, id, name, &args_delta),
            ProviderEvent::Usage(usage) => self.usage = Some(usage),
            ProviderEvent::Finished(reason) => self.finish = Some(reason),
            ProviderEvent::Retrying { .. } => {}
        }
    }

    pub fn finish(self) -> Result<FinishedAssistant, AccumulateError> {
        let mut tool_calls = Vec::with_capacity(self.tools.len());
        for (index, slot) in self.tools {
            let id = slot.id.ok_or(AccumulateError::MissingId { index })?;
            let name = slot.name.ok_or(AccumulateError::MissingName { index })?;
            let arguments = if slot.args.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&slot.args).map_err(|e| AccumulateError::InvalidArgs {
                    index,
                    detail: e.to_string(),
                })?
            };
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
        let finish = if !tool_calls.is_empty() {
            FinishReason::ToolCalls
        } else {
            self.finish.unwrap_or(FinishReason::Stop)
        };
        Ok(FinishedAssistant {
            content: self.text,
            tool_calls,
            finish,
            usage: self.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AssistantAccumulator;
    use crate::providers::ProviderEvent;

    #[test]
    fn remounts_two_interleaved_calls_with_five_arg_fragments() {
        // Call 0 arguments arrive in five fragments; call 1 is interleaved.
        let events = [
            ProviderEvent::ToolCallDelta {
                index: 0,
                id: Some("call_read".into()),
                name: Some("read_file".into()),
                args_delta: String::new(),
            },
            ProviderEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                args_delta: "{\"pa".into(),
            },
            ProviderEvent::ToolCallDelta {
                index: 1,
                id: Some("call_echo".into()),
                name: Some("echo".into()),
                args_delta: "{\"text\":\"hi\"}".into(),
            },
            ProviderEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                args_delta: "th\":\"".into(),
            },
            ProviderEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                args_delta: "src/l".into(),
            },
            ProviderEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                args_delta: "ib.rs\"}".into(),
            },
            ProviderEvent::Finished(crate::providers::FinishReason::ToolCalls),
        ];

        let mut acc = AssistantAccumulator::new();
        for event in events {
            acc.push_event(event);
        }
        let finished = acc.finish().expect("accumulate");

        assert_eq!(finished.tool_calls.len(), 2);
        assert_eq!(finished.tool_calls[0].id, "call_read");
        assert_eq!(finished.tool_calls[0].name, "read_file");
        assert_eq!(finished.tool_calls[0].arguments["path"], "src/lib.rs");
        assert_eq!(finished.tool_calls[1].id, "call_echo");
        assert_eq!(finished.tool_calls[1].name, "echo");
        assert_eq!(finished.tool_calls[1].arguments["text"], "hi");
    }
}
