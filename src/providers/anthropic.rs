//! Anthropic `/v1/messages` dialect. Not forced into the OpenAI chat-completions mould.

use crate::providers::sse::{SseEvent, SseParser};
use crate::providers::{
    ANTHROPIC, AiModel, AiProvider, ChatMessage, ChatRequest, FinishReason, ModelId, ProviderError,
    ProviderEvent, TokenUsage,
};
use async_trait::async_trait;
use futures_util::stream::{BoxStream, Stream};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8192;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub const CURATED: &[(&str, &str, bool)] = &[
    ("claude-opus-4-6", "Claude Opus 4.6", true),
    ("claude-sonnet-4-6", "Claude Sonnet 4.6", true),
    ("claude-haiku-4-5", "Claude Haiku 4.5", true),
];

#[derive(Clone)]
pub struct AnthropicClient {
    http: Client,
    api_key: String,
    base_url: String,
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"sk-ant-[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl AnthropicClient {
    pub fn new(api_key: String, timeout: Duration) -> Result<Self, reqwest::Error> {
        Self::with_base_url(api_key, BASE_URL.to_string(), timeout)
    }

    pub fn with_base_url(
        api_key: String,
        base_url: String,
        timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(timeout)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    pub async fn validate_key(&self) -> Result<(), ProviderError> {
        let resp = self
            .http
            .get(format!("{}/v1/models", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .send()
            .await
            .map_err(map_reqwest)?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(ProviderError::Unauthorized);
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Message(format!("HTTP {status}: {body}")));
        }
        Ok(())
    }
}

fn map_reqwest(err: reqwest::Error) -> ProviderError {
    if err.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::Message(err.to_string())
    }
}

pub fn encode_request(request: &ChatRequest) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": request.model,
        "max_tokens": request.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "stream": true,
        "messages": encode_messages(&request.messages),
    });
    if let Some(system) = request.system.as_deref() {
        body["system"] = encode_system(system, request.system_cache_chars);
    }
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect();
    }
    if let Some(temp) = request.temperature {
        body["temperature"] = serde_json::json!(temp);
    }
    body
}

fn encode_system(system: &str, cache_chars: usize) -> serde_json::Value {
    if cache_chars > 0 && cache_chars < system.len() {
        let (prefix, rest) = system.split_at(cache_chars);
        serde_json::json!([
            {
                "type": "text",
                "text": prefix,
                "cache_control": { "type": "ephemeral" }
            },
            { "type": "text", "text": rest }
        ])
    } else {
        serde_json::json!(system)
    }
}

fn encode_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut pending_tools: Vec<serde_json::Value> = Vec::new();
    let flush_tools = |out: &mut Vec<serde_json::Value>, pending: &mut Vec<serde_json::Value>| {
        if pending.is_empty() {
            return;
        }
        out.push(serde_json::json!({
            "role": "user",
            "content": std::mem::take(pending),
        }));
    };
    for message in messages {
        match message {
            ChatMessage::User { content, images } => {
                flush_tools(&mut out, &mut pending_tools);
                if images.is_empty() {
                    out.push(serde_json::json!({ "role": "user", "content": content }));
                } else {
                    let mut parts = vec![serde_json::json!({ "type": "text", "text": content })];
                    for image in images {
                        parts.push(serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": image.mime,
                                "data": image.data,
                            }
                        }));
                    }
                    out.push(serde_json::json!({ "role": "user", "content": parts }));
                }
            }
            ChatMessage::Assistant {
                content,
                tool_calls,
            } => {
                flush_tools(&mut out, &mut pending_tools);
                let mut parts = Vec::new();
                if !content.is_empty() {
                    parts.push(serde_json::json!({ "type": "text", "text": content }));
                }
                for call in tool_calls {
                    parts.push(serde_json::json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments,
                    }));
                }
                out.push(serde_json::json!({
                    "role": "assistant",
                    "content": parts,
                }));
            }
            ChatMessage::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                let wrapped = crate::providers::chat_completions::wrap_tool_data(content);
                pending_tools.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": wrapped,
                    "is_error": is_error,
                }));
            }
        }
    }
    flush_tools(&mut out, &mut pending_tools);
    out
}

#[async_trait]
impl AiProvider for AnthropicClient {
    fn id(&self) -> &'static str {
        ANTHROPIC
    }

    async fn list_models(&self) -> Result<Vec<AiModel>, ProviderError> {
        Ok(CURATED
            .iter()
            .map(|(id, name, tools)| AiModel {
                id: (*id).to_string(),
                name: (*name).to_string(),
                context_length: Some(200_000),
                supports_tools: *tools,
                supports_vision: true,
                prompt_price: None,
                completion_price: None,
                provider_id: ANTHROPIC.to_string(),
            })
            .collect())
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError> {
        let body = encode_request(&request);
        let req = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body);
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = req.send() => result.map_err(map_reqwest)?,
        };
        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(ProviderError::Unauthorized);
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            if status == 429 {
                return Err(ProviderError::RateLimited(body));
            }
            return Err(ProviderError::Message(format!("HTTP {status}: {body}")));
        }
        Ok(Box::pin(AnthropicStream {
            bytes: Box::pin(resp.bytes_stream()),
            parser: SseParser::new(),
            cancel,
            pending: std::collections::VecDeque::new(),
            done: false,
            finish: None,
            input_tokens: 0,
        }))
    }

    fn supports_tools(&self, model: &ModelId) -> bool {
        CURATED
            .iter()
            .find(|(id, _, _)| *id == model)
            .map(|(_, _, tools)| *tools)
            .unwrap_or(true)
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicEvent {
    #[serde(rename = "type")]
    kind: String,
    index: Option<usize>,
    delta: Option<serde_json::Value>,
    content_block: Option<serde_json::Value>,
    message: Option<serde_json::Value>,
    usage: Option<serde_json::Value>,
}

struct AnthropicStream {
    bytes: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    parser: SseParser,
    cancel: CancellationToken,
    pending: std::collections::VecDeque<Result<ProviderEvent, ProviderError>>,
    done: bool,
    finish: Option<FinishReason>,
    input_tokens: u32,
}

impl AnthropicStream {
    fn ingest(&mut self, chunk: &[u8]) {
        match self.parser.push(chunk) {
            Ok(events) => {
                for event in events {
                    self.dispatch(event);
                }
            }
            Err(e) => self
                .pending
                .push_back(Err(ProviderError::Message(e.to_string()))),
        }
    }

    fn dispatch(&mut self, event: SseEvent) {
        let payload = match event {
            SseEvent::Done => {
                self.pending.push_back(Ok(ProviderEvent::Finished(
                    self.finish.unwrap_or(FinishReason::Stop),
                )));
                self.done = true;
                return;
            }
            SseEvent::Data(payload) => payload,
        };
        let parsed: AnthropicEvent = match serde_json::from_str(&payload) {
            Ok(v) => v,
            Err(e) => {
                self.pending.push_back(Err(ProviderError::Message(format!(
                    "{e}; payload={payload}"
                ))));
                return;
            }
        };
        match parsed.kind.as_str() {
            "message_start" => {
                if let Some(usage) = parsed
                    .message
                    .as_ref()
                    .and_then(|m| m.get("usage"))
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|v| v.as_u64())
                {
                    self.input_tokens = usage as u32;
                }
            }
            "content_block_start" => {
                if let Some(block) = parsed.content_block
                    && block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                {
                    self.pending.push_back(Ok(ProviderEvent::ToolCallDelta {
                        index: parsed.index.unwrap_or(0),
                        id: block.get("id").and_then(|v| v.as_str()).map(str::to_string),
                        name: block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        args_delta: String::new(),
                    }));
                }
            }
            "content_block_delta" => {
                if let Some(delta) = parsed.delta {
                    match delta.get("type").and_then(|t| t.as_str()) {
                        Some("text_delta") => {
                            if let Some(text) = delta.get("text").and_then(|t| t.as_str())
                                && !text.is_empty()
                            {
                                self.pending
                                    .push_back(Ok(ProviderEvent::TextDelta(text.to_string())));
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(partial) =
                                delta.get("partial_json").and_then(|t| t.as_str())
                            {
                                self.pending.push_back(Ok(ProviderEvent::ToolCallDelta {
                                    index: parsed.index.unwrap_or(0),
                                    id: None,
                                    name: None,
                                    args_delta: partial.to_string(),
                                }));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = parsed
                    .delta
                    .as_ref()
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|v| v.as_str())
                {
                    self.finish = Some(match reason {
                        "end_turn" | "stop_sequence" => FinishReason::Stop,
                        "tool_use" => FinishReason::ToolCalls,
                        "max_tokens" => FinishReason::Length,
                        _ => FinishReason::Unknown,
                    });
                }
                if let Some(usage) = parsed.usage {
                    let out = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    let cached = usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    self.pending.push_back(Ok(ProviderEvent::Usage(TokenUsage {
                        prompt_tokens: self.input_tokens,
                        completion_tokens: out,
                        total_tokens: self.input_tokens + out,
                        cached_tokens: cached,
                    })));
                }
            }
            "message_stop" => {
                self.pending.push_back(Ok(ProviderEvent::Finished(
                    self.finish.unwrap_or(FinishReason::Stop),
                )));
                self.done = true;
            }
            _ => {}
        }
    }
}

impl Stream for AnthropicStream {
    type Item = Result<ProviderEvent, ProviderError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.cancel.is_cancelled() {
            this.done = true;
            return Poll::Ready(Some(Err(ProviderError::Cancelled)));
        }
        if let Some(item) = this.pending.pop_front() {
            return Poll::Ready(Some(item));
        }
        if this.done {
            return Poll::Ready(None);
        }
        match Pin::new(&mut this.bytes).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                this.ingest(&chunk);
                if let Some(item) = this.pending.pop_front() {
                    Poll::Ready(Some(item))
                } else if this.done {
                    Poll::Ready(None)
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(map_reqwest(e)))),
            Poll::Ready(None) => {
                this.done = true;
                if let Ok(events) = this.parser.finish() {
                    for event in events {
                        this.dispatch(event);
                    }
                }
                if let Some(item) = this.pending.pop_front() {
                    Poll::Ready(Some(item))
                } else {
                    Poll::Ready(None)
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ToolSchema;
    use crate::providers::accumulate::AssistantAccumulator;
    use futures_util::StreamExt;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEXT_STREAM: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":4}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    const TOOL_STREAM: &str = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_file\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"src/lib.rs\\\"}\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":8}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    #[test]
    fn system_cache_maps_to_native_cache_control() {
        let req = ChatRequest {
            model: "claude-sonnet-4-6".into(),
            system: Some("STABLE\n\ndigest".into()),
            messages: vec![ChatMessage::user("hi")],
            tools: Vec::new(),
            temperature: None,
            max_output_tokens: None,
            system_cache_chars: "STABLE\n\n".len(),
        };
        let body = encode_request(&req);
        assert!(body["system"].is_array());
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body.get("system").is_some());
        assert!(
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["role"] != "system")
        );
    }

    #[test]
    fn tool_results_become_user_tool_result_blocks() {
        let req = ChatRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![
                ChatMessage::Assistant {
                    content: String::new(),
                    tool_calls: vec![crate::providers::ToolCall {
                        id: "t1".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.rs"}),
                    }],
                },
                ChatMessage::ToolResult {
                    call_id: "t1".into(),
                    content: "fn a() {}".into(),
                    is_error: false,
                },
            ],
            tools: vec![ToolSchema {
                name: "read_file".into(),
                description: "read".into(),
                parameters: serde_json::json!({"type":"object"}),
            }],
            temperature: None,
            max_output_tokens: None,
            system_cache_chars: 0,
        };
        let body = encode_request(&req);
        assert_eq!(body["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(body["tools"][0]["name"], "read_file");
    }

    #[tokio::test]
    async fn recorded_text_stream() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("anthropic-version", API_VERSION))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(TEXT_STREAM, "text/event-stream"),
            )
            .mount(&server)
            .await;
        let client = AnthropicClient::with_base_url(
            "sk-ant-test".into(),
            server.uri(),
            Duration::from_secs(30),
        )
        .unwrap();
        let mut stream = client
            .stream_chat(
                ChatRequest {
                    model: "claude-sonnet-4-6".into(),
                    system: None,
                    messages: vec![ChatMessage::user("hi")],
                    tools: Vec::new(),
                    temperature: None,
                    max_output_tokens: None,
                    system_cache_chars: 0,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev.unwrap());
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::TextDelta(t) if t == "Hel"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Usage(u) if u.prompt_tokens == 4 && u.completion_tokens == 2))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Finished(FinishReason::Stop)))
        );
    }

    #[tokio::test]
    async fn recorded_tool_use_stream() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(TOOL_STREAM, "text/event-stream"),
            )
            .mount(&server)
            .await;
        let client = AnthropicClient::with_base_url(
            "sk-ant-test".into(),
            server.uri(),
            Duration::from_secs(30),
        )
        .unwrap();
        let mut stream = client
            .stream_chat(
                ChatRequest {
                    model: "claude-sonnet-4-6".into(),
                    system: None,
                    messages: vec![ChatMessage::user("hi")],
                    tools: Vec::new(),
                    temperature: None,
                    max_output_tokens: None,
                    system_cache_chars: 0,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let mut acc = AssistantAccumulator::new();
        while let Some(ev) = stream.next().await {
            acc.push_event(ev.unwrap());
        }
        let finished = acc.finish().unwrap();
        assert_eq!(finished.tool_calls.len(), 1);
        assert_eq!(finished.tool_calls[0].name, "read_file");
        assert_eq!(finished.tool_calls[0].arguments["path"], "src/lib.rs");
        assert_eq!(finished.finish, FinishReason::ToolCalls);
    }

    #[tokio::test]
    async fn unauthorized_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        let client = AnthropicClient::with_base_url(
            "sk-ant-bad".into(),
            server.uri(),
            Duration::from_secs(30),
        )
        .unwrap();
        let err = match client
            .stream_chat(
                ChatRequest {
                    model: "claude-sonnet-4-6".into(),
                    system: None,
                    messages: vec![ChatMessage::user("hi")],
                    tools: Vec::new(),
                    temperature: None,
                    max_output_tokens: None,
                    system_cache_chars: 0,
                },
                CancellationToken::new(),
            )
            .await
        {
            Ok(_) => panic!("expected unauthorized"),
            Err(e) => e,
        };
        assert!(matches!(err, ProviderError::Unauthorized));
    }
}
