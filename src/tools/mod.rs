//! Tool contract and in-process registry.
#![allow(dead_code)]

pub mod context;
pub mod fs;
pub mod git;
pub mod history;
pub mod run;
pub mod shell;
pub mod skills;
pub mod subagent;

use crate::providers::ToolSchema;
use crate::session::SessionId;
use crate::session::roles::AgentRole;
use crate::workspace::{FilePatch, Project};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const TOOL_OUTPUT_CHAR_LIMIT: usize = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    ReadOnly,
    Mutating,
    Executing,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Message(String),
    #[error("unknown tool `{0}`")]
    Unknown(String),
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("reading `{0}` requires confirmation because the file looks sensitive")]
    NeedsConfirmation(String),
    #[error("the user denied this action")]
    Denied,
}

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub content: String,
    pub truncated: bool,
}

pub struct ToolContext {
    pub session: SessionId,
    pub cancel: CancellationToken,
    pub project: Option<Arc<Project>>,
    pub allow_sensitive: bool,
    pub proposed_patches: Arc<Mutex<Vec<FilePatch>>>,
    pub allow_execute: bool,
    pub command_timeout: Duration,
    pub terminal: Option<std::sync::mpsc::Sender<shell::TerminalEvent>>,
    pub store: Option<Arc<Mutex<crate::context::OrbitStore>>>,
    pub session_label: String,
    pub session_model: String,
    pub session_role: AgentRole,
    pub runner: Option<std::sync::Arc<std::sync::Mutex<crate::runner::ProcessRegistry>>>,
    pub run_configs: Option<Vec<crate::workspace::run_config::RunConfig>>,
    pub run_starts:
        Option<std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, u32>>>>,
    pub db: Option<std::sync::Arc<crate::storage::db::Db>>,
    pub subagents: Option<std::sync::Arc<crate::session::subagent::SubagentHost>>,
    pub budget_usd: Option<f64>,
    pub sandbox_profile: crate::security::sandbox::SandboxProfile,
}

impl ToolContext {
    pub fn for_tests(session: SessionId, cancel: CancellationToken) -> Self {
        Self {
            session,
            cancel,
            project: None,
            allow_sensitive: false,
            proposed_patches: Arc::new(Mutex::new(Vec::new())),
            allow_execute: false,
            command_timeout: shell::COMMAND_TIMEOUT,
            terminal: None,
            store: None,
            session_label: String::new(),
            session_model: String::new(),
            session_role: AgentRole::Coder,
            runner: None,
            run_configs: None,
            run_starts: None,
            db: None,
            subagents: None,
            budget_usd: None,
            sandbox_profile: crate::security::sandbox::SandboxProfile::Off,
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> serde_json::Value;
    fn risk(&self) -> ToolRisk;

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError>;
}

pub fn truncate_output(content: String) -> ToolOutcome {
    let count = content.chars().count();
    if count <= TOOL_OUTPUT_CHAR_LIMIT {
        return ToolOutcome {
            content,
            truncated: false,
        };
    }
    let kept: String = content.chars().take(TOOL_OUTPUT_CHAR_LIMIT).collect();
    ToolOutcome {
        content: format!(
            "{kept}\n\n[truncated: output exceeded {TOOL_OUTPUT_CHAR_LIMIT} characters]"
        ),
        truncated: true,
    }
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn unregister(&mut self, name: &str) {
        self.tools.remove(name);
    }

    pub fn retain_readonly(&mut self) {
        self.tools
            .retain(|_, tool| tool.risk() == ToolRisk::ReadOnly);
    }

    pub fn schemas(&self) -> Vec<ToolSchema> {
        let mut schemas: Vec<ToolSchema> = self
            .tools
            .values()
            .map(|tool| ToolSchema {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.schema(),
            })
            .collect();
        schemas.sort_by(|a, b| a.name.cmp(&b.name));
        schemas
    }

    pub fn workspace_tools() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(fs::ReadFile));
        registry.register(Arc::new(fs::ListDir));
        registry.register(Arc::new(fs::Grep));
        registry.register(Arc::new(fs::Glob));
        registry.register(Arc::new(fs::WriteFile));
        registry.register(Arc::new(fs::EditFile));
        registry.register(Arc::new(fs::MultiEdit));
        registry.register(Arc::new(shell::RunCommand));
        registry.register(Arc::new(run::ListRunConfigs));
        registry.register(Arc::new(run::ReadRunOutput));
        registry.register(Arc::new(run::StartRun));
        registry.register(Arc::new(run::StopRun));
        registry.register(Arc::new(context::RecordDecision));
        registry.register(Arc::new(context::AddFinding));
        registry.register(Arc::new(context::UpdateTask));
        registry.register(Arc::new(context::RecordPlan));
        registry.register(Arc::new(context::ApproveStage));
        registry.register(Arc::new(git::GitStatus));
        registry.register(Arc::new(git::GitCommit));
        registry.register(Arc::new(git::GitPush));
        registry.register(Arc::new(skills::ReadSkill));
        registry.register(Arc::new(skills::CreateSkill));
        registry.register(Arc::new(history::SearchHistory));
        registry.register(Arc::new(subagent::SpawnSubagent));
        registry
    }

    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::Unknown(name.into()))?;
        let outcome = tool.execute(args, ctx).await?;
        Ok(if outcome.truncated {
            outcome
        } else {
            truncate_output(outcome.content)
        })
    }
}

impl ToolRegistry {
    /// The full workspace registry restricted to the tools a role may call.
    /// This is the catalog sent to the model, so a role without a writing tool
    /// simply does not receive one. Equivalent to `workspace_tools()` for Coder.
    pub fn for_role(role: AgentRole) -> Self {
        let mut registry = Self::workspace_tools();
        let allowed = role.allowed_tools();
        registry
            .tools
            .retain(|name, _| allowed.contains(&name.as_str()));
        registry
    }
}

/// Test double used to exercise the registry. Not exposed to real models.
pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Return the provided text unchanged."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        })
    }

    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing `text`".into()))?;
        Ok(truncate_output(text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{EchoTool, TOOL_OUTPUT_CHAR_LIMIT, ToolContext, ToolRegistry};
    use crate::providers::accumulate::AssistantAccumulator;
    use crate::providers::{
        AiProvider, ChatMessage, ChatRequest, FinishReason, ProviderError, ProviderEvent,
    };
    use crate::session::SessionId;
    use crate::session::roles::AgentRole;
    use async_trait::async_trait;
    use futures_util::stream::{self, BoxStream, StreamExt};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn echo_registers_and_emits_schema() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        let schemas = registry.schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "echo");
        assert_eq!(schemas[0].parameters["required"][0], "text");
    }

    #[test]
    fn truncate_marks_oversized_output() {
        let huge: String = "x".repeat(TOOL_OUTPUT_CHAR_LIMIT + 50);
        let outcome = super::truncate_output(huge);
        assert!(outcome.truncated);
        assert!(outcome.content.contains("[truncated:"));
        assert!(outcome.content.chars().count() > TOOL_OUTPUT_CHAR_LIMIT);
    }

    #[test]
    fn for_role_narrows_the_catalog_size() {
        assert_eq!(ToolRegistry::for_role(AgentRole::Coder).schemas().len(), 19);
        assert_eq!(
            ToolRegistry::for_role(AgentRole::Reviewer).schemas().len(),
            15
        );
        assert_eq!(
            ToolRegistry::for_role(AgentRole::Architect).schemas().len(),
            11
        );
        assert_eq!(
            ToolRegistry::for_role(AgentRole::Tester).schemas().len(),
            19
        );
    }

    #[test]
    fn for_role_removes_writing_and_execution_tools() {
        let reviewer = ToolRegistry::for_role(AgentRole::Reviewer);
        assert!(reviewer.get("write_file").is_none());
        assert!(reviewer.get("edit_file").is_none());
        assert!(reviewer.get("read_file").is_some());
        assert!(reviewer.get("read_skill").is_some());
        assert!(reviewer.get("create_skill").is_none());

        let architect = ToolRegistry::for_role(AgentRole::Architect);
        assert!(architect.get("run_command").is_none());
        assert!(architect.get("write_file").is_none());
        assert!(architect.get("record_decision").is_some());
        assert!(architect.get("read_skill").is_some());
        assert!(architect.get("create_skill").is_some());
        assert!(architect.get("spawn_subagent").is_none());

        let coder = ToolRegistry::for_role(AgentRole::Coder);
        assert!(coder.get("spawn_subagent").is_some());
    }

    #[test]
    fn for_role_removes_schemas_sent_to_the_model() {
        let schemas = ToolRegistry::for_role(AgentRole::Reviewer).schemas();
        for schema in &schemas {
            assert_ne!(schema.name, "write_file");
            assert_ne!(schema.name, "edit_file");
        }
    }

    #[test]
    fn for_role_coder_is_a_subset_of_workspace_tools() {
        let coder = ToolRegistry::for_role(AgentRole::Coder);
        let workspace = ToolRegistry::workspace_tools();
        let a: Vec<String> = coder.schemas().into_iter().map(|s| s.name).collect();
        let b: Vec<String> = workspace.schemas().into_iter().map(|s| s.name).collect();
        for name in &a {
            assert!(
                b.contains(name),
                "coder tool `{name}` missing from workspace"
            );
        }
        assert!(b.contains(&"git_status".into()));
        assert!(b.contains(&"record_plan".into()));
        assert!(b.contains(&"approve_stage".into()));
        assert!(!a.contains(&"git_status".into()));
        assert!(!a.contains(&"record_plan".into()));
        assert!(!a.contains(&"approve_stage".into()));
    }

    struct ScriptedProvider {
        turn: AtomicUsize,
    }

    #[async_trait]
    impl AiProvider for ScriptedProvider {
        fn id(&self) -> &'static str {
            "scripted"
        }

        async fn list_models(&self) -> Result<Vec<crate::providers::AiModel>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(
            &self,
            request: ChatRequest,
            _cancel: CancellationToken,
        ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError>
        {
            let turn = self.turn.fetch_add(1, Ordering::SeqCst);
            let events = if turn == 0 {
                assert!(request.tools.iter().any(|t| t.name == "echo"));
                vec![
                    Ok(ProviderEvent::ToolCallDelta {
                        index: 0,
                        id: Some("call_echo".into()),
                        name: Some("echo".into()),
                        args_delta: r#"{"text":"pong"}"#.into(),
                    }),
                    Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
                ]
            } else {
                let ChatMessage::ToolResult {
                    content, is_error, ..
                } = &request.messages[request.messages.len() - 1]
                else {
                    panic!("second turn must include the tool result");
                };
                assert!(!is_error);
                assert_eq!(content, "pong");
                vec![
                    Ok(ProviderEvent::TextDelta(format!("echoed: {content}"))),
                    Ok(ProviderEvent::Finished(FinishReason::Stop)),
                ]
            };
            Ok(Box::pin(stream::iter(events)))
        }

        fn supports_tools(&self, _model: &crate::providers::ModelId) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn full_echo_cycle_against_scripted_provider() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        let provider = ScriptedProvider {
            turn: AtomicUsize::new(0),
        };
        let cancel = CancellationToken::new();

        let first = ChatRequest {
            model: "scripted".into(),
            system: None,
            messages: vec![ChatMessage::user("ping")],
            tools: registry.schemas(),
            temperature: None,
            max_output_tokens: None,
            system_cache_chars: 0,
        };
        let mut stream = provider.stream_chat(first, cancel.clone()).await.unwrap();
        let mut acc = AssistantAccumulator::new();
        while let Some(event) = stream.next().await {
            acc.push_event(event.unwrap());
        }
        let assistant = acc.finish().unwrap();
        assert_eq!(assistant.tool_calls.len(), 1);
        assert_eq!(assistant.tool_calls[0].name, "echo");

        let ctx = ToolContext::for_tests(SessionId::new("test"), cancel.clone());
        let outcome = registry
            .execute(
                &assistant.tool_calls[0].name,
                assistant.tool_calls[0].arguments.clone(),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(outcome.content, "pong");

        let second = ChatRequest {
            model: "scripted".into(),
            system: None,
            messages: vec![
                ChatMessage::user("ping"),
                ChatMessage::Assistant {
                    content: assistant.content,
                    tool_calls: assistant.tool_calls,
                },
                ChatMessage::ToolResult {
                    call_id: "call_echo".into(),
                    content: outcome.content,
                    is_error: false,
                },
            ],
            tools: registry.schemas(),
            temperature: None,
            max_output_tokens: None,
            system_cache_chars: 0,
        };
        let mut stream = provider.stream_chat(second, cancel).await.unwrap();
        let mut acc = AssistantAccumulator::new();
        while let Some(event) = stream.next().await {
            acc.push_event(event.unwrap());
        }
        let final_msg = acc.finish().unwrap();
        assert_eq!(final_msg.content, "echoed: pong");
        assert!(final_msg.tool_calls.is_empty());
    }
}
