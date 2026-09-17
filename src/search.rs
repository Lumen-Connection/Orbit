//! Web retrieval used by Chat Mode. Search results are always treated as
//! untrusted tool data before they are returned to a model.

use crate::providers::ProviderError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub const TAVILY: &str = "tavily";
pub const HOSTED_WEB_SEARCH_TOOL: &str = "__orbit_hosted_web_search";
pub const MAX_SEARCHES_PER_TURN: usize = 3;
const MAX_RESULTS: usize = 5;
const MAX_EXCERPT_CHARS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSource {
    pub id: String,
    pub title: String,
    pub url: String,
    pub excerpt: String,
}

#[async_trait]
pub trait SearchBackend: Send + Sync {
    fn id(&self) -> &'static str;
    async fn search(
        &self,
        query: &str,
        cancel: CancellationToken,
    ) -> Result<Vec<WebSource>, ProviderError>;
}

#[derive(Clone)]
pub struct TavilySearch {
    key: String,
    http: reqwest::Client,
}

impl TavilySearch {
    pub fn new(key: String) -> Result<Self, reqwest::Error> {
        Ok(Self {
            key,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()?,
        })
    }

    fn result_to_source(index: usize, result: TavilyResult) -> Option<WebSource> {
        let url = result.url?.trim().to_string();
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return None;
        }
        let excerpt: String = result
            .content
            .unwrap_or_default()
            .chars()
            .take(MAX_EXCERPT_CHARS)
            .collect();
        Some(WebSource {
            id: format!("S{}", index + 1),
            title: result.title.unwrap_or_else(|| url.clone()),
            url,
            excerpt,
        })
    }
}

#[async_trait]
impl SearchBackend for TavilySearch {
    fn id(&self) -> &'static str {
        TAVILY
    }

    async fn search(
        &self,
        query: &str,
        cancel: CancellationToken,
    ) -> Result<Vec<WebSource>, ProviderError> {
        let body = serde_json::json!({
            "query": query,
            "search_depth": "basic",
            "max_results": MAX_RESULTS,
            "include_answer": false,
            "include_raw_content": false,
            "include_usage": true,
            "auto_parameters": false,
        });
        let request = self
            .http
            .post("https://api.tavily.com/search")
            .bearer_auth(&self.key)
            .json(&body);
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = request.send() => result.map_err(|e| if e.is_timeout() { ProviderError::Timeout } else { ProviderError::Transient(e.to_string()) })?,
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::Unauthorized);
        }
        if !response.status().is_success() {
            let status = response.status();
            return Err(ProviderError::Message(format!(
                "Tavily search failed with HTTP {status}."
            )));
        }
        let parsed: TavilyResponse = response.json().await.map_err(|e| {
            ProviderError::Message(format!("Tavily returned an invalid response: {e}"))
        })?;
        Ok(parsed
            .results
            .into_iter()
            .enumerate()
            .filter_map(|(i, r)| Self::result_to_source(i, r))
            .collect())
    }
}

#[derive(Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
}

pub fn web_search_schema() -> crate::providers::ToolSchema {
    crate::providers::ToolSchema {
        name: "web_search".into(),
        description: "Search the public web for current facts. Use it only when web evidence is needed. Cite returned source IDs such as [S1] in your answer.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string", "description": "Focused web search query" } },
            "required": ["query"],
            "additionalProperties": false
        }),
    }
}

/// Internal marker consumed by native provider adapters. It is never exposed as
/// a function to a model.
pub fn hosted_web_search_schema() -> crate::providers::ToolSchema {
    crate::providers::ToolSchema {
        name: HOSTED_WEB_SEARCH_TOOL.into(),
        description: String::new(),
        parameters: serde_json::Value::Null,
    }
}

pub fn format_tool_result(sources: &[WebSource]) -> String {
    if sources.is_empty() {
        return "No web results were found.".into();
    }
    sources
        .iter()
        .map(|source| {
            format!(
                "[{}] {}\nURL: {}\nExcerpt: {}",
                source.id, source.title, source.url, source.excerpt
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_rejects_non_web_urls_and_clips_excerpt() {
        assert!(
            TavilySearch::result_to_source(
                0,
                TavilyResult {
                    title: None,
                    url: Some("file:///secret".into()),
                    content: None
                }
            )
            .is_none()
        );
        let source = TavilySearch::result_to_source(
            0,
            TavilyResult {
                title: Some("Example".into()),
                url: Some("https://example.com".into()),
                content: Some("x".repeat(MAX_EXCERPT_CHARS + 1)),
            },
        )
        .unwrap();
        assert_eq!(source.id, "S1");
        assert_eq!(source.excerpt.chars().count(), MAX_EXCERPT_CHARS);
    }
}
