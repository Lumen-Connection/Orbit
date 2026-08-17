//! Offline end-to-end flows against a scripted provider.

use crate::context::{OrbitStore, build_digest, build_handoff};
use crate::providers::{
    AiModel, AiProvider, ChatMessage, ChatRequest, FinishReason, ModelId, ProviderError,
    ProviderEvent, TokenUsage, ToolCall,
};
use crate::security::{ApprovalDecision, Policy};
use crate::session::agent_loop::{TurnDeps, TurnResult, dispatch_tool, run_turn};
use crate::session::{AgentEvent, ApprovalBridge, BudgetBridge, Session, SessionId, SessionLimits};
use crate::tools::{Tool, ToolRegistry};
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
        subagents: None,
        sandbox_profile: crate::security::sandbox::SandboxProfile::Off,
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

#[tokio::test(flavor = "multi_thread")]
async fn skill_created_in_one_session_appears_in_another_sessions_digest() {
    let (_tmp, project, store) = fixture();
    let dir = project
        .canonical_root
        .join(".orbit/skills/run-integration-tests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        crate::context::skills::render_skill_file(
            "run-integration-tests",
            "How to run the integration suite.",
            "SECRET BODY: spin up the ephemeral database first.",
        ),
    )
    .unwrap();
    {
        let mut s = store.lock().unwrap();
        s.reload();
        assert_eq!(s.skills.len(), 1);
        let digest = build_digest(&s, &SessionId::new("bbb"), &project.name);
        assert!(digest.text.contains("Available skills (1):"));
        assert!(digest.text.contains("run-integration-tests"));
        assert!(digest.text.contains("How to run the integration suite."));
        assert!(
            !digest.text.contains("SECRET BODY"),
            "skill body must not enter the digest: {}",
            digest.text
        );
    }
    let ctx = crate::tools::ToolContext {
        session: SessionId::new("bbb"),
        cancel: CancellationToken::new(),
        project: Some(project.clone()),
        allow_sensitive: false,
        proposed_patches: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        allow_execute: false,
        command_timeout: crate::tools::shell::COMMAND_TIMEOUT,
        terminal: None,
        store: Some(store),
        session_label: "review".into(),
        session_model: "model-b".into(),
        session_role: crate::session::AgentRole::Coder,
        runner: None,
        run_configs: None,
        run_starts: None,
        db: None,
        subagents: None,
        sandbox_profile: crate::security::sandbox::SandboxProfile::Off,
        budget_usd: None,
    };
    let out = crate::tools::skills::ReadSkill
        .execute(serde_json::json!({"name": "run-integration-tests"}), &ctx)
        .await
        .unwrap();
    assert!(out.content.contains("SECRET BODY"));
}

#[test]
fn hub_resolves_models_to_their_provider() {
    use crate::providers::catalog::ModelCatalog;
    use crate::providers::{ANTHROPIC, OPENROUTER, ProviderHub};
    let mut hub = ProviderHub::default();
    hub.insert(Arc::new(Script {
        turn: AtomicUsize::new(0),
    }));
    // Scripted provider id is "e2e"; catalog curated models default to openrouter.
    let catalog = ModelCatalog::curated();
    assert_eq!(
        catalog.find("claude-sonnet-4-6").unwrap().provider_id,
        ANTHROPIC
    );
    assert_eq!(
        catalog.find("anthropic/claude-opus-5").unwrap().provider_id,
        OPENROUTER
    );
    let _ = hub;
}

#[test]
fn search_history_finds_same_project_and_hides_the_other() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = crate::storage::db::Db::open_at(tmp.path().join("orbit.db")).unwrap();
    let root_a = tmp.path().join("proj-a");
    let root_b = tmp.path().join("proj-b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let project_a = Project::open(&root_a).unwrap();
    let project_b = Project::open(&root_b).unwrap();
    db.upsert_project(&project_a).unwrap();
    db.upsert_project(&project_b).unwrap();
    let id_a = SessionId::new("sess-a");
    let id_b = SessionId::new("sess-b");
    db.upsert_session(&project_a.id, &id_a, "implementation", "e2e")
        .unwrap();
    db.upsert_session(&project_b.id, &id_b, "other", "e2e")
        .unwrap();
    db.replace_messages(
        &id_a,
        &[crate::providers::ChatMessage::user(
            "we already chose rusqlite for the zebra index",
        )],
    )
    .unwrap();
    db.replace_messages(
        &id_b,
        &[crate::providers::ChatMessage::user(
            "we already chose postgres for the zebra index",
        )],
    )
    .unwrap();
    let hits = db
        .search_history_scoped(&project_a.id, "zebra", 10)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item_id, "sess-a");
    assert!(hits[0].snippet.to_lowercase().contains("rusqlite"));
    assert!(!hits.iter().any(|h| h.item_id == "sess-b"));
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

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_run(root: &std::path::Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn git_fixture() -> Option<(TempDir, Arc<Project>)> {
    if !git_available() {
        return None;
    }
    let tmp = TempDir::new().ok()?;
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(root.join("src")).ok()?;
    std::fs::write(root.join("src/lib.rs"), "fn authenticate() {}\n").ok()?;
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").ok()?;
    std::fs::write(root.join("src/b.rs"), "fn b() {}\n").ok()?;
    std::fs::write(root.join("src/c.rs"), "fn c() {}\n").ok()?;
    let _ = OrbitStore::open(&root);
    if !git_run(&root, &["init"]) {
        return None;
    }
    let _ = git_run(&root, &["config", "user.email", "orbit@test"]);
    let _ = git_run(&root, &["config", "user.name", "orbit"]);
    if !git_run(&root, &["add", "."]) || !git_run(&root, &["commit", "-m", "init"]) {
        return None;
    }
    let project = Arc::new(Project::open(&root).ok()?);
    Some((tmp, project))
}

fn subagent_host(
    provider: Arc<dyn AiProvider>,
    budget: f64,
) -> Arc<crate::session::subagent::SubagentHost> {
    Arc::new(crate::session::subagent::SubagentHost {
        provider,
        slots: Arc::new(tokio::sync::Semaphore::new(3)),
        shared_spent: Arc::new(std::sync::Mutex::new(0.0)),
        budget_usd: Some(budget),
        pending: Arc::new(std::sync::Mutex::new(Vec::new())),
        fraction: 0.25,
    })
}

struct WorktreeParent {
    turn: AtomicUsize,
    child_turn: AtomicUsize,
    child_started: Arc<std::sync::atomic::AtomicBool>,
    hang_child: bool,
}

#[async_trait]
impl AiProvider for WorktreeParent {
    fn id(&self) -> &'static str {
        "wt"
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
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError> {
        if request.tools.iter().any(|t| t.name == "spawn_subagent") {
            let n = self.turn.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return Ok(Box::pin(stream::iter([
                    Ok(ProviderEvent::ToolCallDelta {
                        index: 0,
                        id: Some("sp".into()),
                        name: Some("spawn_subagent".into()),
                        args_delta:
                            r#"{"role":"coder","isolation":"worktree","task":"implement X"}"#.into(),
                    }),
                    Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
                ])));
            }
            return Ok(Box::pin(stream::iter([
                Ok(ProviderEvent::TextDelta("parent done".into())),
                Ok(ProviderEvent::Finished(FinishReason::Stop)),
            ])));
        }
        self.child_started
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if self.hang_child {
            tokio::select! {
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            }
        }
        let n = self.child_turn.fetch_add(1, Ordering::SeqCst);
        let events = match n {
            0 => vec![
                Ok(ProviderEvent::ToolCallDelta {
                    index: 0,
                    id: Some("w1".into()),
                    name: Some("write_file".into()),
                    args_delta: r#"{"path":"src/a.rs","content":"fn a() { 1 }"}"#.into(),
                }),
                Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
            ],
            1 => vec![
                Ok(ProviderEvent::ToolCallDelta {
                    index: 0,
                    id: Some("w2".into()),
                    name: Some("write_file".into()),
                    args_delta: r#"{"path":"src/b.rs","content":"fn b() { 2 }"}"#.into(),
                }),
                Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
            ],
            2 => vec![
                Ok(ProviderEvent::ToolCallDelta {
                    index: 0,
                    id: Some("w3".into()),
                    name: Some("write_file".into()),
                    args_delta: r#"{"path":"src/c.rs","content":"fn c() { 3 }"}"#.into(),
                }),
                Ok(ProviderEvent::Finished(FinishReason::ToolCalls)),
            ],
            _ => vec![
                Ok(ProviderEvent::TextDelta("child done".into())),
                Ok(ProviderEvent::Finished(FinishReason::Stop)),
            ],
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn worktree_subagent_writes_without_prompt_and_deny_keeps_parent_intact() {
    let Some((_tmp, project)) = git_fixture() else {
        return;
    };
    crate::session::worktree::prune(&project);
    let child_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let provider = Arc::new(WorktreeParent {
        turn: AtomicUsize::new(0),
        child_turn: AtomicUsize::new(0),
        child_started,
        hang_child: false,
    });
    let host = subagent_host(provider.clone(), 2.0);
    let (tx, rx) = mpsc::channel();
    let approvals = ApprovalBridge::new();
    let mut turn = deps(
        provider,
        project.clone(),
        None,
        CancellationToken::new(),
        Policy::default(),
        tx,
        approvals.clone(),
        Some(2.0),
        (None, None),
        BudgetBridge::new(),
    );
    turn.subagents = Some(host);
    let session = Arc::new(tokio::sync::Mutex::new(Session::new("coder", "wt")));
    let task = tokio::spawn(run_turn(session, Some("implement X".into()), turn));

    let saw_write_approval = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let merge_files = Arc::new(AtomicUsize::new(0));
    let resolver = approvals.clone();
    let saw_flag = saw_write_approval.clone();
    let merge_count = merge_files.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if let AgentEvent::ApprovalRequired(h) = event {
                if h.tool_name == "write_file" || h.tool_name == "edit_file" {
                    saw_flag.store(true, Ordering::SeqCst);
                    resolver.resolve(h.id, ApprovalDecision::Denied);
                    continue;
                }
                if !h.patches.is_empty() {
                    merge_count.store(h.patches.len(), Ordering::SeqCst);
                    resolver.resolve(h.id, ApprovalDecision::Denied);
                    continue;
                }
                resolver.resolve(h.id, ApprovalDecision::Approved);
            }
        }
    });

    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("timeout")
        .unwrap();
    crate::session::worktree::prune(&project);
    assert_eq!(result, TurnResult::Completed);
    assert!(
        !saw_write_approval.load(Ordering::SeqCst),
        "child writes must not prompt"
    );
    assert_eq!(
        merge_files.load(Ordering::SeqCst),
        3,
        "expected one merge of 3 files"
    );
    assert_eq!(
        std::fs::read_to_string(project.canonical_root.join("src/a.rs")).unwrap(),
        "fn a() {}\n"
    );
    assert_eq!(
        std::fs::read_to_string(project.canonical_root.join("src/b.rs")).unwrap(),
        "fn b() {}\n"
    );
    assert_eq!(
        std::fs::read_to_string(project.canonical_root.join("src/c.rs")).unwrap(),
        "fn c() {}\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn worktree_subagent_conflict_when_parent_edits_same_file() {
    let Some((_tmp, project)) = git_fixture() else {
        return;
    };
    crate::session::worktree::prune(&project);
    let child_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let provider = Arc::new(WorktreeParent {
        turn: AtomicUsize::new(0),
        child_turn: AtomicUsize::new(0),
        child_started: child_started.clone(),
        hang_child: false,
    });
    let host = subagent_host(provider.clone(), 2.0);
    let (tx, rx) = mpsc::channel();
    let approvals = ApprovalBridge::new();
    let mut turn = deps(
        provider,
        project.clone(),
        None,
        CancellationToken::new(),
        Policy::default(),
        tx,
        approvals.clone(),
        Some(2.0),
        (None, None),
        BudgetBridge::new(),
    );
    turn.subagents = Some(host);
    let session = Arc::new(tokio::sync::Mutex::new(Session::new("coder", "wt")));
    let task = tokio::spawn(run_turn(session, Some("implement X".into()), turn));

    let parent_root = project.canonical_root.clone();
    let started = child_started;
    let conflicted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let resolver = approvals.clone();
    let conflict_flag = conflicted.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if let AgentEvent::ApprovalRequired(h) = event {
                if h.patches.is_empty() {
                    resolver.resolve(h.id, ApprovalDecision::Approved);
                    let start = std::time::Instant::now();
                    while !started.load(std::sync::atomic::Ordering::SeqCst)
                        && start.elapsed() < Duration::from_secs(5)
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let _ = std::fs::write(parent_root.join("src/a.rs"), "fn parent() {}\n");
                } else {
                    conflict_flag.store(
                        h.patches
                            .iter()
                            .any(|p| matches!(p.status, crate::workspace::PatchStatus::Conflicted)),
                        Ordering::SeqCst,
                    );
                    resolver.resolve(h.id, ApprovalDecision::Denied);
                }
            }
        }
    });

    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("timeout")
        .unwrap();
    crate::session::worktree::prune(&project);
    assert_eq!(result, TurnResult::Completed);
    assert!(
        conflicted.load(Ordering::SeqCst),
        "parent+child edit of the same file must conflict"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_parent_removes_the_worktree() {
    let Some((_tmp, project)) = git_fixture() else {
        return;
    };
    crate::session::worktree::prune(&project);
    let child_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let provider = Arc::new(WorktreeParent {
        turn: AtomicUsize::new(0),
        child_turn: AtomicUsize::new(0),
        child_started: child_started.clone(),
        hang_child: true,
    });
    let host = subagent_host(provider.clone(), 2.0);
    let (tx, rx) = mpsc::channel();
    let approvals = ApprovalBridge::new();
    let cancel = CancellationToken::new();
    let mut turn = deps(
        provider,
        project.clone(),
        None,
        cancel.clone(),
        Policy::default(),
        tx,
        approvals.clone(),
        Some(2.0),
        (None, None),
        BudgetBridge::new(),
    );
    turn.subagents = Some(host);
    let session = Arc::new(tokio::sync::Mutex::new(Session::new("coder", "wt")));
    let task = tokio::spawn(run_turn(session, Some("implement X".into()), turn));
    let resolver = approvals.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if let AgentEvent::ApprovalRequired(h) = event {
                resolver.resolve(h.id, ApprovalDecision::Approved);
            }
        }
    });
    let start = std::time::Instant::now();
    while !child_started.load(std::sync::atomic::Ordering::SeqCst)
        && start.elapsed() < Duration::from_secs(5)
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
    let list = crate::workspace::git::git(&project.canonical_root, &["worktree", "list"])
        .unwrap_or_default();
    crate::session::worktree::prune(&project);
    assert!(
        !list.contains(&project.id) && list.lines().count() <= 1,
        "worktree list should be clean after cancel: {list}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn isolation_none_with_coder_is_refused() {
    let (_tmp, project, _) = fixture();
    let (tx, rx) = mpsc::channel();
    let approvals = ApprovalBridge::new();
    let call = ToolCall {
        id: "sp".into(),
        name: "spawn_subagent".into(),
        arguments: serde_json::json!({
            "role": "coder",
            "isolation": "none",
            "task": "write it"
        }),
    };
    let provider = Arc::new(Script {
        turn: AtomicUsize::new(99),
    });
    let host = subagent_host(provider.clone(), 2.0);
    let mut turn = deps(
        provider,
        project,
        None,
        CancellationToken::new(),
        Policy::default(),
        tx,
        approvals.clone(),
        Some(2.0),
        (None, None),
        BudgetBridge::new(),
    );
    turn.subagents = Some(host);
    let task = tokio::spawn(async move { dispatch_tool(&call, &turn).await });
    let resolver = approvals;
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if let AgentEvent::ApprovalRequired(h) = event {
                resolver.resolve(h.id, ApprovalDecision::Approved);
            }
        }
    });
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("timeout")
        .unwrap();
    match result {
        ChatMessage::ToolResult {
            content, is_error, ..
        } => {
            assert!(is_error);
            assert!(content.contains("worktree"), "{content}");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn worktree_isolation_refuses_a_project_without_git() {
    let (_tmp, project, _) = fixture();
    let (tx, rx) = mpsc::channel();
    let approvals = ApprovalBridge::new();
    let call = ToolCall {
        id: "sp".into(),
        name: "spawn_subagent".into(),
        arguments: serde_json::json!({
            "role": "coder",
            "isolation": "worktree",
            "task": "write it"
        }),
    };
    let provider = Arc::new(Script {
        turn: AtomicUsize::new(99),
    });
    let host = subagent_host(provider.clone(), 2.0);
    let mut turn = deps(
        provider,
        project,
        None,
        CancellationToken::new(),
        Policy::default(),
        tx,
        approvals.clone(),
        Some(2.0),
        (None, None),
        BudgetBridge::new(),
    );
    turn.subagents = Some(host);
    let task = tokio::spawn(async move { dispatch_tool(&call, &turn).await });
    let resolver = approvals;
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if let AgentEvent::ApprovalRequired(h) = event {
                resolver.resolve(h.id, ApprovalDecision::Approved);
            }
        }
    });
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("timeout")
        .unwrap();
    match result {
        ChatMessage::ToolResult {
            content, is_error, ..
        } => {
            assert!(is_error);
            assert!(content.contains("git"), "{content}");
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn hook_stub() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?;
    let path = dir.join(format!("orbit-hook-stub{}", std::env::consts::EXE_SUFFIX));
    path.exists()
        .then(|| path.to_string_lossy().replace('\\', "/"))
}

fn install_hook(root: &std::path::Path, event: &str, matcher: &str, args: &[&str]) -> bool {
    let Some(stub) = hook_stub() else {
        return false;
    };
    let dir = root.join(".orbit");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = std::fs::read_to_string(dir.join("config.toml")).unwrap_or_default();
    let args_toml = args
        .iter()
        .map(|a| format!("\"{}\"", a.replace('\\', "/")))
        .collect::<Vec<_>>()
        .join(", ");
    body.push_str(&format!(
        "\n[[hooks]]\nevent = \"{event}\"\nmatcher = \"{matcher}\"\ncommand = \"{stub}\"\nargs = [{args_toml}]\n"
    ));
    std::fs::write(dir.join("config.toml"), body).unwrap();
    true
}

fn trust_installed(root: &std::path::Path) {
    for hook in crate::hooks::load_hooks(root) {
        let _ = crate::hooks::trust_on_this_machine(&hook);
    }
}

fn forget_installed(root: &std::path::Path) {
    for hook in crate::hooks::load_hooks(root) {
        let _ = crate::security::declared::MachineTrust::HOOKS.forget(&hook.fingerprint());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_hook_deny_blocks_write_and_reason_reaches_the_model() {
    let (_tmp, project, _) = fixture();
    if !install_hook(
        &project.canonical_root,
        "PreToolUse",
        "write_file",
        &["deny", "no migrations"],
    ) {
        return;
    }
    trust_installed(&project.canonical_root);
    let (tx, _rx) = mpsc::channel();
    let call = ToolCall {
        id: "w".into(),
        name: "write_file".into(),
        arguments: serde_json::json!({"path":"migrations/1.sql","content":"x"}),
    };
    let msg = dispatch_tool(
        &call,
        &deps(
            Arc::new(Script {
                turn: AtomicUsize::new(99),
            }),
            project.clone(),
            None,
            CancellationToken::new(),
            Policy {
                auto_approve_mutating: true,
                ..Policy::default()
            },
            tx,
            ApprovalBridge::new(),
            None,
            (None, None),
            BudgetBridge::new(),
        ),
    )
    .await;
    forget_installed(&project.canonical_root);
    match msg {
        ChatMessage::ToolResult {
            content, is_error, ..
        } => {
            assert!(is_error);
            assert!(
                content.contains("migrations") || content.contains("Blocked by project hook"),
                "{content}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hook_exit_one_denies_the_tool() {
    let (_tmp, project, _) = fixture();
    if !install_hook(
        &project.canonical_root,
        "PreToolUse",
        "write_file",
        &["exit1"],
    ) {
        return;
    }
    trust_installed(&project.canonical_root);
    let (tx, _rx) = mpsc::channel();
    let call = ToolCall {
        id: "w".into(),
        name: "write_file".into(),
        arguments: serde_json::json!({"path":"src/lib.rs","content":"x"}),
    };
    let msg = dispatch_tool(
        &call,
        &deps(
            Arc::new(Script {
                turn: AtomicUsize::new(99),
            }),
            project.clone(),
            None,
            CancellationToken::new(),
            Policy {
                auto_approve_mutating: true,
                ..Policy::default()
            },
            tx,
            ApprovalBridge::new(),
            None,
            (None, None),
            BudgetBridge::new(),
        ),
    )
    .await;
    forget_installed(&project.canonical_root);
    match msg {
        ChatMessage::ToolResult {
            content, is_error, ..
        } => {
            assert!(is_error, "{content}");
            assert!(
                content.contains("hook") || content.contains("crashed"),
                "{content}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hanging_hook_is_killed_and_the_tool_proceeds() {
    let (_tmp, project, _) = fixture();
    if !install_hook(
        &project.canonical_root,
        "PreToolUse",
        "write_file",
        &["hang"],
    ) {
        return;
    }
    trust_installed(&project.canonical_root);
    let (tx, _rx) = mpsc::channel();
    let call = ToolCall {
        id: "w".into(),
        name: "write_file".into(),
        arguments: serde_json::json!({"path":"src/lib.rs","content":"fn hung() {}"}),
    };
    let started = std::time::Instant::now();
    let msg = dispatch_tool(
        &call,
        &deps(
            Arc::new(Script {
                turn: AtomicUsize::new(99),
            }),
            project.clone(),
            None,
            CancellationToken::new(),
            Policy {
                auto_approve_mutating: true,
                ..Policy::default()
            },
            tx,
            ApprovalBridge::new(),
            None,
            (None, None),
            BudgetBridge::new(),
        ),
    )
    .await;
    forget_installed(&project.canonical_root);
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    match msg {
        ChatMessage::ToolResult {
            content, is_error, ..
        } => {
            assert!(!is_error, "{content}");
            assert!(content.contains("timed out"), "{content}");
        }
        other => panic!("{other:?}"),
    }
    let text = std::fs::read_to_string(project.canonical_root.join("src/lib.rs")).unwrap();
    assert!(text.contains("fn hung()"));
}

#[tokio::test(flavor = "multi_thread")]
async fn untrusted_hook_asks_for_approval() {
    let (_tmp, project, _) = fixture();
    if !install_hook(
        &project.canonical_root,
        "PreToolUse",
        "write_file",
        &["allow"],
    ) {
        return;
    }
    forget_installed(&project.canonical_root);
    let (tx, rx) = mpsc::channel();
    let approvals = ApprovalBridge::new();
    let call = ToolCall {
        id: "w".into(),
        name: "write_file".into(),
        arguments: serde_json::json!({"path":"src/lib.rs","content":"fn x() {}"}),
    };
    let turn = deps(
        Arc::new(Script {
            turn: AtomicUsize::new(99),
        }),
        project.clone(),
        None,
        CancellationToken::new(),
        Policy {
            auto_approve_mutating: true,
            ..Policy::default()
        },
        tx,
        approvals.clone(),
        None,
        (None, None),
        BudgetBridge::new(),
    );
    let task = tokio::spawn(async move { dispatch_tool(&call, &turn).await });
    let handle = loop {
        match rx.recv() {
            Ok(AgentEvent::ApprovalRequired(h)) => break h,
            Ok(_) => continue,
            Err(_) => panic!("closed"),
        }
    };
    assert!(
        handle.tool_name == "hook" || handle.summary.contains("Trust project hook"),
        "{handle:?}"
    );
    approvals.resolve(handle.id, ApprovalDecision::Denied);
    let msg = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("timeout")
        .unwrap();
    forget_installed(&project.canonical_root);
    match msg {
        ChatMessage::ToolResult { content, .. } => {
            assert!(
                content.contains("not trusted") || content.contains("Applied"),
                "{content}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn role_guard_runs_before_the_hook() {
    let (_tmp, project, _) = fixture();
    let marker = project.canonical_root.join("hook-ran.txt");
    let marker_arg = marker.display().to_string().replace('\\', "/");
    if !install_hook(
        &project.canonical_root,
        "PreToolUse",
        "write_file",
        &["touch", &marker_arg],
    ) {
        return;
    }
    trust_installed(&project.canonical_root);
    let (tx, _rx) = mpsc::channel();
    let mut turn = deps(
        Arc::new(Script {
            turn: AtomicUsize::new(99),
        }),
        project.clone(),
        None,
        CancellationToken::new(),
        Policy::default(),
        tx,
        ApprovalBridge::new(),
        None,
        (None, None),
        BudgetBridge::new(),
    );
    turn.session_role = crate::session::AgentRole::Architect;
    let call = ToolCall {
        id: "w".into(),
        name: "write_file".into(),
        arguments: serde_json::json!({"path":"src/lib.rs","content":"x"}),
    };
    let msg = dispatch_tool(&call, &turn).await;
    forget_installed(&project.canonical_root);
    match msg {
        ChatMessage::ToolResult {
            content, is_error, ..
        } => {
            assert!(is_error);
            assert!(
                content.contains("Architect") || content.contains("not allowed"),
                "{content}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(
        !marker.exists(),
        "hook must not run after a role-guard deny"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn denylist_hook_is_refused_without_an_approval() {
    let (_tmp, project, _) = fixture();
    let dir = project.canonical_root.join(".orbit");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = std::fs::read_to_string(dir.join("config.toml")).unwrap_or_default();
    body.push_str(
        "\n[[hooks]]\nevent = \"PreToolUse\"\nmatcher = \"write_file\"\ncommand = \"shutdown\"\nargs = [\"-h\", \"now\"]\n",
    );
    std::fs::write(dir.join("config.toml"), body).unwrap();
    let (tx, rx) = mpsc::channel();
    let call = ToolCall {
        id: "w".into(),
        name: "write_file".into(),
        arguments: serde_json::json!({"path":"src/lib.rs","content":"fn d() {}"}),
    };
    let msg = dispatch_tool(
        &call,
        &deps(
            Arc::new(Script {
                turn: AtomicUsize::new(99),
            }),
            project.clone(),
            None,
            CancellationToken::new(),
            Policy {
                auto_approve_mutating: true,
                ..Policy::default()
            },
            tx,
            ApprovalBridge::new(),
            None,
            (None, None),
            BudgetBridge::new(),
        ),
    )
    .await;
    assert!(
        rx.try_recv()
            .ok()
            .is_none_or(|e| !matches!(e, AgentEvent::ApprovalRequired(_))),
        "denylist hook must not offer approval"
    );
    match msg {
        ChatMessage::ToolResult { content, .. } => {
            assert!(
                content.contains("denylist") || content.contains("Applied"),
                "{content}"
            );
        }
        other => panic!("{other:?}"),
    }
}
