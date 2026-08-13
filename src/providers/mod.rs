//! Normalized AI provider contract.
//!
//! HTTP clients, SSE details and vendor JSON stay behind this module.
//! Trait signatures never mention `reqwest`.

pub mod accumulate;
pub mod catalog;
pub mod openrouter;
pub mod retry;
pub mod sse;

use async_trait::async_trait;
use catalog::ModelInfo;
use futures_util::stream::BoxStream;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub type ModelId = String;
pub type ToolCallId = String;

/// JSON Schema wrapper sent to the model as a tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cached_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    Cancelled,
    Unknown,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("API key was rejected")]
    Unauthorized,
    #[error("The request timed out. Check your connection and try again.")]
    Timeout,
    #[error("The request was cancelled")]
    Cancelled,
    #[error("Rate limited. {0}")]
    RateLimited(String),
    #[error("{0}")]
    Transient(String),
    #[error("{message}")]
    Permanent { message: String, hint: String },
    #[error("The stream was interrupted after text started. Continue or regenerate.")]
    StreamInterrupted,
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: ModelId,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSchema>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageAttachment {
    pub mime: String,
    pub data: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

impl ImageAttachment {
    pub fn data_uri(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.data)
    }
}

#[derive(Debug, Clone)]
pub enum ChatMessage {
    User {
        content: String,
        images: Vec<ImageAttachment>,
    },
    Assistant {
        content: String,
        tool_calls: Vec<ToolCall>,
    },
    #[allow(dead_code)]
    ToolResult {
        call_id: ToolCallId,
        content: String,
        is_error: bool,
    },
}

impl ChatMessage {
    #[allow(dead_code)]
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
            images: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    TextDelta(String),
    ToolCallDelta {
        index: usize,
        id: Option<ToolCallId>,
        name: Option<String>,
        args_delta: String,
    },
    Usage(TokenUsage),
    Finished(FinishReason),
    Retrying {
        attempt: u32,
        max_attempts: u32,
        wait_secs: u64,
    },
}

#[derive(Debug, Clone)]
pub struct AiModel {
    pub id: ModelId,
    pub name: String,
    pub context_length: Option<u32>,
    pub supports_tools: bool,
    pub supports_vision: bool,
    /// Per-token prompt price when the provider publishes one.
    pub prompt_price: Option<f64>,
    /// Per-token completion price when the provider publishes one.
    pub completion_price: Option<f64>,
}

impl From<AiModel> for ModelInfo {
    fn from(model: AiModel) -> Self {
        Self {
            id: model.id,
            name: model.name,
            descriptor: None,
            context_length: model.context_length,
            prompt_price: model.prompt_price,
            completion_price: model.completion_price,
            supports_tools: model.supports_tools,
            supports_vision: model.supports_vision,
        }
    }
}

/// Streaming chat provider. Implementations must not leak HTTP types.
#[async_trait]
pub trait AiProvider: Send + Sync {
    #[allow(dead_code)]
    fn id(&self) -> &'static str;

    async fn list_models(&self) -> Result<Vec<AiModel>, ProviderError>;

    /// Normalized event stream. Cancelled by `cancel`.
    async fn stream_chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError>;

    #[allow(dead_code)]
    fn supports_tools(&self, model: &ModelId) -> bool;
}

/// Build the OpenRouter adapter without exposing its concrete type.
#[allow(dead_code)]
pub fn connect_openrouter(api_key: String) -> Result<Arc<dyn AiProvider>, ProviderError> {
    connect_openrouter_timed(api_key, Duration::from_secs(120))
}

pub fn connect_openrouter_timed(
    api_key: String,
    timeout: Duration,
) -> Result<Arc<dyn AiProvider>, ProviderError> {
    let client = openrouter::OpenRouterClient::with_timeout(api_key, timeout)
        .map_err(|e| ProviderError::Message(format!("Couldn't build HTTP client: {e}")))?;
    Ok(Arc::new(client))
}

/// Validate an OpenRouter key without leaking the client type.
pub async fn validate_openrouter_key(api_key: String) -> Result<(), ProviderError> {
    let client = openrouter::OpenRouterClient::new(api_key)
        .map_err(|e| ProviderError::Message(format!("Couldn't build HTTP client: {e}")))?;
    client
        .validate_key()
        .await
        .map(|_| ())
        .map_err(openrouter::into_provider_error)
}
