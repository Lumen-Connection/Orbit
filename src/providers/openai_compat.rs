//! OpenAI-compatible `/v1` servers: OpenAI, Ollama, LM Studio, vLLM, LiteLLM.

use crate::providers::chat_completions::ChatCompletionsClient;
use crate::providers::{
    AiModel, AiProvider, ChatRequest, ModelId, OPENAI_COMPAT, ProviderError, ProviderEvent,
};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_LOCAL_BASE_URL: &str = "http://127.0.0.1:11434/v1";

#[derive(Clone)]
pub struct OpenAiCompatClient {
    inner: ChatCompletionsClient,
}

impl std::fmt::Debug for OpenAiCompatClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatClient")
            .field("base_url", &self.inner.base_url())
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatClient {
    pub fn new(
        api_key: Option<String>,
        base_url: String,
        timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        let key = api_key.filter(|k| !k.trim().is_empty());
        Ok(Self {
            inner: ChatCompletionsClient::new(
                OPENAI_COMPAT,
                key,
                base_url,
                Vec::new(),
                timeout,
                "[REDACTED]",
            )?,
        })
    }

    #[cfg(test)]
    pub fn with_base_url(base_url: String) -> Result<Self, reqwest::Error> {
        Self::new(None, base_url, Duration::from_secs(120))
    }
}

#[async_trait]
impl AiProvider for OpenAiCompatClient {
    fn id(&self) -> &'static str {
        OPENAI_COMPAT
    }

    async fn list_models(&self) -> Result<Vec<AiModel>, ProviderError> {
        self.inner.list_models().await
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError> {
        self.inner.stream_chat(request, cancel).await
    }

    fn supports_tools(&self, model: &ModelId) -> bool {
        self.inner.supports_tools(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ChatMessage, ChatRequest, FinishReason, ProviderEvent};
    use futures_util::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const RECORDED_STREAM: &str = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: [DONE]\n\n",
    );

    #[tokio::test]
    async fn local_stream_replays_recorded_sse() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(RECORDED_STREAM, "text/event-stream"),
            )
            .mount(&server)
            .await;
        let client = OpenAiCompatClient::with_base_url(server.uri()).unwrap();
        let mut stream = client
            .stream_chat(
                ChatRequest {
                    model: "llama3".into(),
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
                .any(|e| matches!(e, ProviderEvent::TextDelta(t) if t == "hi"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Finished(FinishReason::Stop)))
        );
    }

    #[tokio::test]
    async fn local_models_are_priced_at_zero() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "id": "llama3.2" }]
            })))
            .mount(&server)
            .await;
        let client = OpenAiCompatClient::with_base_url(server.uri()).unwrap();
        let models = client.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].prompt_price, Some(0.0));
        assert_eq!(models[0].completion_price, Some(0.0));
        assert_eq!(models[0].provider_id, OPENAI_COMPAT);
        assert!(models[0].supports_tools);
        assert!(client.supports_tools(&"llama3.2".into()));
    }

    #[tokio::test]
    async fn unauthorized_is_permanent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        let client =
            OpenAiCompatClient::new(Some("sk-bad".into()), server.uri(), Duration::from_secs(30))
                .unwrap();
        let mut stream = client
            .stream_chat(
                ChatRequest {
                    model: "gpt-4o".into(),
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
        let first = stream.next().await.unwrap();
        assert!(matches!(first, Err(ProviderError::Unauthorized)));
    }
}
