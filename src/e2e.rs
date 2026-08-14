//! Offline end-to-end flows against a scripted provider.

use crate::context::{OrbitStore, build_digest, build_handoff};
use crate::providers::{
    AiModel, AiProvider, ChatMessage, ChatRequest, FinishReason, ModelId, ProviderError,
    ProviderEvent, TokenUsage, ToolCall,
};
use crate::security::{ApprovalDecision, Policy};
use crate::session::agent_loop::{TurnDeps, TurnResult, dispatch_tool, run_turn};
use crate::session::{AgentEvent, ApprovalBridge, BudgetBridge, Session, SessionId, SessionLimits};
use crate::tools::ToolRegistry;
use crate::workspace::{FilePatch, Project, apply_patch};
use async_trait::async_trait;
use futures_util::stream::{self, BoxStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct Script {
    turn: AtomicUsize,
}

#[async_trait]
impl AiProvider for Script {
    fn id(&self) -> &'static str {
        "e2e"
    }
    async fn list_models(&self) -> Result<Vec<AiModel>, ProviderError> {
        Ok(Vec::new())
    }
    fn supports_tools(&self, _: &ModelId) -> bool {
        true
    }
    async fn stream_chat(
        &self,
        request: ChatRequest,
        _: CancellationToken,
    ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError> {
        let n = self.turn.fetch_add(1, Ordering::SeqCst);
        let events = match n {
            0 => vec![
                Ok(ProviderEvent::ToolCallDelta {
                    index: 0,
                    id: Some("g1".into()),
                    name: Some("grep".into()),
                    args_delta: r#"{"pattern":"fn authenticate"}"#.into(),
                }),
                Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
            ],
            1 => vec![
                Ok(ProviderEvent::ToolCallDelta {
                    index: 0,
                    id: Some("e1".into()),
                    name: Some("edit_file".into()),
                    args_delta: r#"{"path":"src/lib.rs","old_string":"fn authenticate() {}","new_string":"fn login() {}"}"#.into(),
                }),
                Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
            ],
            2 => vec![
                Ok(ProviderEvent::ToolCallDelta {
                    index: 0,
                    id: Some("c1".into()),
                    name: Some("run_command".into()),
                    args_delta: r#"{"program":"cargo","args":["--version"]}"#.into(),
                }),
                Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
            ],
            3 => vec![
                Ok(ProviderEvent::ToolCallDelta {
                    index: 0,
                    id: Some("d1".into()),
                    name: Some("record_decision".into()),
                    args_delta: r#"{"decision":"Rename authenticate to login","rationale":"Clearer name","files":["src/lib.rs"]}"#.into(),
                }),
                Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
            ],
            _ => {
                let _ = request;
                vec![
                    Ok(ProviderEvent::TextDelta("done".into())),
                    Ok(ProviderEvent::Finished(FinishReason::Stop)),
                ]
            }
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

fn fixture() -> (TempDir, Arc<Project>, Arc<std::sync::Mutex<OrbitStore>>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn authenticate() {}\n").unwrap();
    let project = Arc::new(Project::open(&root).unwrap());
    let store = Arc::new(std::sync::Mutex::new(OrbitStore::open(&root)));
    (tmp, project, store)
}

fn terminal_sink() -> std::sync::mpsc::Sender<crate::tools::shell::TerminalEvent> {
    let (tx, _rx) = mpsc::channel();
    tx
}

#[allow(clippy::too_many_arguments)]
fn deps(
    provider: Arc<dyn AiProvider>,
    project: Arc<Project>,
    store: Option<Arc<std::sync::Mutex<OrbitStore>>>,
    cancel: CancellationToken,
    policy: Policy,
    events: std::sync::mpsc::Sender<AgentEvent>,
    approvals: Arc<ApprovalBridge>,
    budget: Option<f64>,
    prices: (Option<f64>, Option<f64>),
    budget_bridge: Arc<BudgetBridge>,
) -> TurnDeps {
    TurnDeps {
        provider,
        registry: Arc::new(ToolRegistry::workspace_tools()),
        project,
        events,
        approvals,
        policy,
        cancel,
        session_id: SessionId::new("e2e"),
        terminal: terminal_sink(),
        store,
        session_label: "implementation".into(),
        session_model: "e2e".into(),
        session_role: crate::session::AgentRole::Coder,
        summary_model: None,
        db: None,
        prompt_price: prices.0,
        completion_price: prices.1,
        budget_usd: budget,
        budget_bridge,
        spent_start: 0.0,
        context_length: crate::session::context_window::DEFAULT_CONTEXT_LENGTH,
        recent_keep: crate::session::context_window::DEFAULT_RECENT_KEEP,
        run_env: crate::session::agent_loop::RunEnv::default(),
        user_images: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn coding_flow_search_edit_approve_command_decision() {
    let (_tmp, project, store) = fixture();
    let (tx, rx) = mpsc::channel();
    let approvals = ApprovalBridge::new();
    let session = Arc::new(tokio::sync::Mutex::new(Session::new(
        "implementation",
        "e2e",
    )));
    let turn = tokio::spawn(run_turn(
        session.clone(),
        Some("rename authenticate".into()),
        deps(
            Arc::new(Script {
                turn: AtomicUsize::new(0),
            }),
            project.clone(),
            Some(store.clone()),
            CancellationToken::new(),
            Policy::default(),
            tx,
            approvals.clone(),
            None,
            (None, None),
            BudgetBridge::new(),
        ),
    ));

    let resolver = approvals.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if let AgentEvent::ApprovalRequired(h) = event {
                resolver.resolve(h.id, ApprovalDecision::Approved);
            }
        }
    });

    let result = tokio::time::timeout(Duration::from_secs(30), turn)
        .await
        .expect("timeout")
        .unwrap();
    assert_eq!(result, TurnResult::Completed);
    let text = std::fs::read_to_string(project.canonical_root.join("src/lib.rs")).unwrap();
    assert!(text.contains("fn login()"));
    let decisions =
        std::fs::read_to_string(project.canonical_root.join(".orbit/decisions.md")).unwrap();
    assert!(decisions.contains("Rename authenticate to login"));
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_finished_completed_reaches_the_orchestrator() {
    struct StopOnce;
    #[async_trait]
    impl AiProvider for StopOnce {
        fn id(&self) -> &'static str {
            "stop"
        }
        async fn list_models(&self) -> Result<Vec<AiModel>, ProviderError> {
            Ok(Vec::new())
        }
        fn supports_tools(&self, _: &ModelId) -> bool {
            true
        }
        async fn stream_chat(
            &self,
            _: ChatRequest,
            _: CancellationToken,
        ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError>
        {
            Ok(Box::pin(stream::iter(vec![
                Ok(ProviderEvent::TextDelta("ok".into())),
                Ok(ProviderEvent::Finished(FinishReason::Stop)),
            ])))
        }
    }

    let (_tmp, project, store) = fixture();
    let (events_tx, _events_rx) = mpsc::channel();
    let (pipe_tx, pipe_rx) = mpsc::channel();
    let session = Arc::new(tokio::sync::Mutex::new(Session::new("coder", "e2e")));
    let sid = SessionId::new("coder-stage");
    let result = run_turn(
        session,
        Some("go".into()),
        deps(
            Arc::new(StopOnce),
            project,
            Some(store),
            CancellationToken::new(),
            Policy::default(),
            events_tx,
            ApprovalBridge::new(),
            None,
            (None, None),
            BudgetBridge::new(),
        ),
    )
    .await;
    let _ = pipe_tx.send(crate::pipeline::PipelineEvent::stage_finished(
        sid.clone(),
        result,
    ));
    let ev = pipe_rx.recv().unwrap();
    assert_eq!(
        ev,
        crate::pipeline::PipelineEvent::StageFinished {
            session_id: sid,
            result: crate::pipeline::StageResult::Completed,
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_handoff_matches_digest() {
    let (_tmp, project, store) = fixture();
    {
        let mut s = store.lock().unwrap();
        s.record_touch(
            &SessionId::new("aaa"),
            "implementation",
            "e2e",
            std::path::Path::new("src/lib.rs"),
        )
        .unwrap();
        s.mark_active(&SessionId::new("aaa"), "implementation", "e2e")
            .unwrap();
        s.upsert_session(crate::context::SessionRecord {
            id: "bbb".into(),
            label: "review".into(),
            model: "e2e".into(),
            last_active_at: Some("2020-01-01T00:00:00Z".into()),
            touched: Vec::new(),
        })
        .unwrap();
    }
    let store_g = store.lock().unwrap();
    let digest = build_digest(&store_g, &SessionId::new("bbb"), &project.name);
    let handoff = build_handoff(&store_g, &SessionId::new("bbb"));
    assert!(handoff.is_interesting());
    assert!(digest.text.contains(handoff.digest_section.trim()));
    assert!(handoff.digest_section.contains("src/lib.rs"));
}

#[tokio::test]
async fn cancel_during_stream() {
    struct Hang;
    #[async_trait]
    impl AiProvider for Hang {
        fn id(&self) -> &'static str {
            "hang"
        }
        async fn list_models(&self) -> Result<Vec<AiModel>, ProviderError> {
            Ok(Vec::new())
        }
        fn supports_tools(&self, _: &ModelId) -> bool {
            true
        }
        async fn stream_chat(
            &self,
            _: ChatRequest,
            cancel: CancellationToken,
        ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError>
        {
            tokio::select! {
                _ = cancel.cancelled() => Err(ProviderError::Cancelled),
                _ = tokio::time::sleep(Duration::from_secs(30)) => Ok(Box::pin(stream::iter([]))),
            }
        }
    }
    let (_tmp, project, _) = fixture();
    let (tx, _rx) = mpsc::channel();
    let cancel = CancellationToken::new();
    let session = Arc::new(tokio::sync::Mutex::new(Session::new("t", "hang")));
    let handle = tokio::spawn(run_turn(
        session,
        Some("go".into()),
        deps(
            Arc::new(Hang),
            project,
            None,
            cancel.clone(),
            Policy::default(),
            tx,
            ApprovalBridge::new(),
            None,
            (None, None),
            BudgetBridge::new(),
        ),
    ));
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("join")
        .unwrap();
    assert!(matches!(
        result,
        TurnResult::Cancelled | TurnResult::Failed(_)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_during_write_approval() {
    let (_tmp, project, _) = fixture();
    let cancel = CancellationToken::new();
    let (tx, rx) = mpsc::channel();
    let approvals = ApprovalBridge::new();
    let call = ToolCall {
        id: "w".into(),
        name: "write_file".into(),
        arguments: serde_json::json!({"path":"src/lib.rs","content":"x"}),
    };
    let deps = deps(
        Arc::new(Script {
            turn: AtomicUsize::new(99),
        }),
        project.clone(),
        None,
        cancel.clone(),
        Policy::default(),
        tx,
        approvals,
        None,
        (None, None),
        BudgetBridge::new(),
    );
    let task = tokio::spawn(async move { dispatch_tool(&call, &deps).await });
    loop {
        match rx.recv() {
            Ok(AgentEvent::ApprovalRequired(_)) => break,
            Ok(_) => continue,
            Err(_) => panic!("closed"),
        }
    }
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("join")
        .unwrap();
    assert!(matches!(
        result,
        ChatMessage::ToolResult { is_error: true, .. }
    ));
}

#[test]
fn patch_conflict_is_detected() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("note.txt"), "hello").unwrap();
    let mut patch = FilePatch::new("note.txt".into(), "hello".into(), "hello world".into());
    std::fs::write(root.join("note.txt"), "changed").unwrap();
    apply_patch(root, &mut patch).unwrap();
    assert_eq!(patch.status, crate::workspace::PatchStatus::Conflicted);
}

#[tokio::test(flavor = "multi_thread")]
async fn budget_cap_stops_the_turn() {
    struct Pricey {
        turn: AtomicUsize,
    }
    #[async_trait]
    impl AiProvider for Pricey {
        fn id(&self) -> &'static str {
            "p"
        }
        async fn list_models(&self) -> Result<Vec<AiModel>, ProviderError> {
            Ok(Vec::new())
        }
        fn supports_tools(&self, _: &ModelId) -> bool {
            true
        }
        async fn stream_chat(
            &self,
            _: ChatRequest,
            _: CancellationToken,
        ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError>
        {
            let n = self.turn.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok(Box::pin(stream::iter([
                    Ok(ProviderEvent::Usage(TokenUsage {
                        prompt_tokens: 100,
                        completion_tokens: 50,
                        total_tokens: 150,
                        cached_tokens: 0,
                    })),
                    Ok(ProviderEvent::ToolCallDelta {
                        index: 0,
                        id: Some("x".into()),
                        name: Some("missing".into()),
                        args_delta: "{}".into(),
                    }),
                    Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
                ])))
            } else {
                Ok(Box::pin(stream::iter([Ok(ProviderEvent::Finished(
                    FinishReason::Stop,
                ))])))
            }
        }
    }
    let (_tmp, project, _) = fixture();
    let (tx, rx) = mpsc::channel();
    let bridge = BudgetBridge::new();
    let mut session = Session::new("t", "p");
    session.limits = SessionLimits {
        max_iterations: 25,
        budget_usd: 0.5,
    };
    let task = tokio::spawn(run_turn(
        Arc::new(tokio::sync::Mutex::new(session)),
        Some("go".into()),
        deps(
            Arc::new(Pricey {
                turn: AtomicUsize::new(0),
            }),
            project,
            None,
            CancellationToken::new(),
            Policy::default(),
            tx,
            ApprovalBridge::new(),
            Some(0.5),
            (Some(0.01), Some(0.02)),
            bridge.clone(),
        ),
    ));
    loop {
        match rx.recv() {
            Ok(AgentEvent::BudgetExceeded { .. }) => break,
            Ok(_) => continue,
            Err(_) => panic!("closed"),
        }
    }
    bridge.resolve(None);
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("timeout")
        .unwrap();
    assert_eq!(result, TurnResult::BudgetExceeded);
}
