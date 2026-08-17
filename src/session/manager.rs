//! Several independent sessions on one project.

use super::{
    AgentEvent, AgentRole, ApprovalBridge, BudgetBridge, Session, SessionId, TranscriptItem,
    apply_agent_event,
};
use crate::context::HandoffSummary;
use crate::security::{ApprovalDecision, ApprovalId};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_SESSION_SLOTS: usize = 3;

#[derive(Debug, Clone, Copy, Default)]
pub struct SessionPoll {
    pub unauthorized: bool,
}

pub struct LiveSession {
    pub handle: Arc<tokio::sync::Mutex<Session>>,
    pub id: SessionId,
    pub label: String,
    pub model: String,
    pub role: AgentRole,
    pub transcript: Vec<TranscriptItem>,
    pub input: String,
    pub busy: bool,
    pub agent_rx: Option<Receiver<crate::session::AgentEvent>>,
    pub agent_cancel: Option<CancellationToken>,
    pub approvals: Arc<ApprovalBridge>,
    pub handoff: Option<HandoffSummary>,
    pub handoff_dismissed: bool,
    pub editing_label: bool,
    pub spent_usd: f64,
    pub budget_usd: f64,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub cached_tokens: u32,
    pub last_latency_ms: Option<u64>,
    pub iteration: u32,
    pub budget_prompt: Option<(f64, f64)>,
    pub budget_bridge: Arc<BudgetBridge>,
    pub context_occupancy: Option<f32>,
    pub retry_hint: Option<String>,
    pub parent_id: Option<SessionId>,
    pub parent_label: Option<String>,
    pub isolation: crate::session::worktree::Isolation,
}

impl LiveSession {
    pub fn from_session(session: Session) -> Self {
        let budget_usd = session.limits.budget_usd;
        let role = session.role;
        Self {
            id: session.id.clone(),
            label: session.label.clone(),
            model: session.model.clone(),
            role,
            handle: Arc::new(tokio::sync::Mutex::new(session)),
            transcript: Vec::new(),
            input: String::new(),
            busy: false,
            agent_rx: None,
            agent_cancel: None,
            approvals: ApprovalBridge::new(),
            handoff: None,
            handoff_dismissed: false,
            editing_label: false,
            spent_usd: 0.0,
            budget_usd,
            prompt_tokens: 0,
            completion_tokens: 0,
            cached_tokens: 0,
            last_latency_ms: None,
            iteration: 0,
            budget_prompt: None,
            budget_bridge: BudgetBridge::new(),
            context_occupancy: None,
            retry_hint: None,
            parent_id: None,
            parent_label: None,
            isolation: crate::session::worktree::Isolation::None,
        }
    }

    pub fn cancel(&mut self) {
        if let Some(cancel) = &self.agent_cancel {
            cancel.cancel();
        }
        self.approvals.deny_all();
        self.budget_bridge.cancel();
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model.clone();
        if let Ok(mut session) = self.handle.try_lock() {
            session.model = model;
        }
    }

    pub fn set_role(&mut self, role: AgentRole) {
        self.role = role;
        if let Ok(mut session) = self.handle.try_lock() {
            session.role = role;
        }
    }

    pub fn set_label(&mut self, label: String) {
        self.label = label.clone();
        if let Ok(mut session) = self.handle.try_lock() {
            session.label = label;
        }
        self.editing_label = false;
    }
}

pub struct SessionManager {
    pub sessions: Vec<LiveSession>,
    pub active: usize,
    pub slots: Arc<Semaphore>,
    pub pending_subagents: Arc<std::sync::Mutex<Vec<crate::session::subagent::PendingSubagent>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            active: 0,
            slots: Arc::new(Semaphore::new(DEFAULT_SESSION_SLOTS)),
            pending_subagents: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn create(&mut self, label: impl Into<String>, model: impl Into<String>) -> SessionId {
        self.create_with_role(label, model, AgentRole::Coder)
    }

    pub fn create_with_role(
        &mut self,
        label: impl Into<String>,
        model: impl Into<String>,
        role: AgentRole,
    ) -> SessionId {
        let session = Session::new(label, model).with_role(role);
        let id = session.id.clone();
        self.sessions.push(LiveSession::from_session(session));
        self.active = self.sessions.len() - 1;
        id
    }

    pub fn next_label(&self, project_name: &str) -> String {
        if self.sessions.is_empty() {
            return project_name.to_string();
        }
        let n = self.sessions.len() + 1;
        format!("Session {n}")
    }

    pub fn active(&self) -> Option<&LiveSession> {
        self.sessions.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut LiveSession> {
        self.sessions.get_mut(self.active)
    }

    pub fn get_mut(&mut self, id: &SessionId) -> Option<&mut LiveSession> {
        self.sessions.iter_mut().find(|s| s.id == *id)
    }

    pub fn select(&mut self, id: &SessionId) {
        if let Some(idx) = self.sessions.iter().position(|s| s.id == *id) {
            self.active = idx;
        }
    }

    pub fn any_busy(&self) -> bool {
        self.sessions.iter().any(|s| s.busy)
    }

    pub fn close(&mut self, id: &SessionId) -> bool {
        if self.sessions.len() <= 1 {
            return false;
        }
        let Some(idx) = self.sessions.iter().position(|s| s.id == *id) else {
            return false;
        };
        self.sessions[idx].cancel();
        self.sessions.remove(idx);
        if self.active >= self.sessions.len() {
            self.active = self.sessions.len().saturating_sub(1);
        } else if idx < self.active {
            self.active -= 1;
        }
        true
    }

    pub fn shutdown(&mut self) {
        for session in &mut self.sessions {
            session.cancel();
        }
        self.sessions.clear();
        self.active = 0;
    }

    pub fn drain_pending_subagents(&mut self) {
        let pending = self
            .pending_subagents
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default();
        for item in pending {
            if self.sessions.iter().any(|s| s.id == item.id) {
                continue;
            }
            let mut live =
                LiveSession::from_session(Session::new(item.label.clone(), item.model.clone()));
            live.id = item.id.clone();
            live.label = item.label;
            live.model = item.model;
            live.role = item.role;
            live.handle = item.handle;
            live.agent_rx = Some(item.agent_rx);
            live.agent_cancel = Some(item.agent_cancel);
            live.approvals = item.approvals;
            live.busy = true;
            live.budget_usd = item.budget_usd;
            live.parent_id = Some(item.parent_id);
            live.parent_label = Some(item.parent_label);
            live.isolation = item.isolation;
            self.sessions.push(live);
        }
    }

    pub fn poll_all_detailed(&mut self) -> SessionPoll {
        self.drain_pending_subagents();
        let mut unauthorized = false;
        for session in &mut self.sessions {
            let Some(rx) = &session.agent_rx else {
                continue;
            };
            let mut events = Vec::new();
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            if events.is_empty() {
                continue;
            }
            let mut still_busy = session.busy;
            for event in events {
                match event {
                    AgentEvent::Usage {
                        input_tokens,
                        output_tokens,
                        cost_usd,
                        latency_ms,
                        iteration,
                        spent_usd,
                        cached_tokens,
                    } => {
                        session.prompt_tokens = session.prompt_tokens.saturating_add(input_tokens);
                        session.completion_tokens =
                            session.completion_tokens.saturating_add(output_tokens);
                        session.cached_tokens = session.cached_tokens.saturating_add(cached_tokens);
                        session.spent_usd = spent_usd;
                        let _ = cost_usd;
                        session.last_latency_ms = Some(latency_ms);
                        session.iteration = iteration;
                        still_busy = true;
                    }
                    AgentEvent::BudgetExceeded { spent, cap } => {
                        session.budget_prompt = Some((spent, cap));
                        still_busy = true;
                    }
                    AgentEvent::ContextOccupancy(ratio) => {
                        session.context_occupancy = Some(ratio);
                        still_busy = true;
                    }
                    AgentEvent::Retrying {
                        attempt,
                        max_attempts,
                        wait_secs,
                    } => {
                        session.retry_hint = Some(format!(
                            "Retrying in {wait_secs}s… ({attempt}/{max_attempts})"
                        ));
                        still_busy = true;
                    }
                    other => {
                        if matches!(other, AgentEvent::Unauthorized) {
                            unauthorized = true;
                        }
                        still_busy = apply_agent_event(&mut session.transcript, other);
                    }
                }
            }
            session.busy = still_busy;
            if !still_busy {
                session.agent_rx = None;
                session.agent_cancel = None;
                session.retry_hint = None;
            }
        }
        SessionPoll { unauthorized }
    }

    pub fn resolve_approval(
        &mut self,
        id: ApprovalId,
        decision: ApprovalDecision,
    ) -> Option<(bool, Vec<crate::workspace::FilePatch>, SessionId)> {
        for session in &mut self.sessions {
            let patches = session.transcript.iter().find_map(|item| match item {
                TranscriptItem::Approval { handle, .. } if handle.id == id => Some(handle.files()),
                _ => None,
            });
            let has = patches.is_some()
                || session.transcript.iter().any(|item| {
                    matches!(item, TranscriptItem::Approval { handle, .. } if handle.id == id)
                });
            if has {
                let live = session.approvals.is_pending(id);
                crate::session::mark_approval(&mut session.transcript, id, decision);
                session.approvals.resolve(id, decision);
                return Some((live, patches.unwrap_or_default(), session.id.clone()));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{LiveSession, SessionManager};
    use crate::providers::{
        AiModel, AiProvider, ChatRequest, FinishReason, ModelId, ProviderError, ProviderEvent,
    };
    use crate::security::Policy;
    use crate::session::agent_loop::{TurnDeps, TurnResult, run_turn};
    use crate::session::roles::AgentRole;
    use crate::session::{AgentEvent, ApprovalBridge, Session, SessionId};
    use crate::tools::ToolRegistry;
    use crate::workspace::Project;
    use async_trait::async_trait;
    use futures_util::stream::{self, BoxStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn create_select_rename_and_close() {
        let mut mgr = SessionManager::new();
        let a = mgr.create("one", "model-a");
        let b = mgr.create("two", "model-b");
        let c = mgr.create("three", "model-c");
        assert_eq!(mgr.sessions.len(), 3);
        mgr.select(&a);
        assert_eq!(mgr.active().unwrap().id, a);
        mgr.get_mut(&b).unwrap().set_label("implementation".into());
        assert_eq!(mgr.get_mut(&b).unwrap().label, "implementation");
        assert!(mgr.close(&c));
        assert_eq!(mgr.sessions.len(), 2);
        assert!(mgr.close(&a));
        assert_eq!(mgr.sessions.len(), 1);
        assert!(!mgr.close(&b));
    }

    #[test]
    fn changing_model_keeps_transcript() {
        let mut live = LiveSession::from_session(Session::new("t", "old"));
        live.transcript
            .push(crate::session::TranscriptItem::User("hello".into()));
        live.set_model("new-model".into());
        assert_eq!(live.model, "new-model");
        assert_eq!(live.transcript.len(), 1);
        assert!(live.handle.try_lock().unwrap().messages.is_empty());
    }

    struct OkProvider {
        gate: Option<Arc<AtomicUsize>>,
    }
    impl OkProvider {
        fn new() -> Self {
            Self { gate: None }
        }
        fn gated(gate: Arc<AtomicUsize>) -> Self {
            Self { gate: Some(gate) }
        }
    }
    #[async_trait]
    impl AiProvider for OkProvider {
        fn id(&self) -> &'static str {
            "ok"
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
            if let Some(gate) = &self.gate {
                gate.fetch_add(1, Ordering::SeqCst);
                let started = std::time::Instant::now();
                while gate.load(Ordering::SeqCst) < 2 {
                    if started.elapsed() > Duration::from_secs(2) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            } else {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(Box::pin(stream::iter([
                Ok(ProviderEvent::TextDelta("ok".into())),
                Ok(ProviderEvent::Finished(FinishReason::Stop)),
            ])))
        }
    }

    struct FailProvider;
    #[async_trait]
    impl AiProvider for FailProvider {
        fn id(&self) -> &'static str {
            "fail"
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
            Err(ProviderError::Message("boom".into()))
        }
    }

    struct SlowProvider {
        started: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl AiProvider for SlowProvider {
        fn id(&self) -> &'static str {
            "slow"
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
            self.started.fetch_add(1, Ordering::SeqCst);
            tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(ProviderError::Cancelled);
                }
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            }
            Ok(Box::pin(stream::iter([Ok(ProviderEvent::Finished(
                FinishReason::Stop,
            ))])))
        }
    }

    fn project() -> (TempDir, Arc<Project>) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("p");
        std::fs::create_dir_all(&root).unwrap();
        (tmp, Arc::new(Project::open(&root).unwrap()))
    }

    fn terminal_sink() -> std::sync::mpsc::Sender<crate::tools::shell::TerminalEvent> {
        let (tx, _rx) = mpsc::channel();
        tx
    }

    fn deps_for(
        provider: Arc<dyn AiProvider>,
        project: Arc<Project>,
        cancel: CancellationToken,
        id: SessionId,
        events: std::sync::mpsc::Sender<AgentEvent>,
    ) -> TurnDeps {
        TurnDeps {
            provider,
            registry: Arc::new(ToolRegistry::new()),
            project,
            events,
            approvals: ApprovalBridge::new(),
            policy: Policy::default(),
            cancel,
            session_id: id,
            terminal: terminal_sink(),
            store: None,
            session_label: "t".into(),
            session_model: "m".into(),
            session_role: AgentRole::Coder,
            summary_model: None,
            db: None,
            prompt_price: None,
            completion_price: None,
            budget_usd: None,
            budget_bridge: crate::session::BudgetBridge::new(),
            spent_start: 0.0,
            context_length: crate::session::context_window::DEFAULT_CONTEXT_LENGTH,
            recent_keep: crate::session::context_window::DEFAULT_RECENT_KEEP,
            run_env: crate::session::agent_loop::RunEnv::default(),
            user_images: Vec::new(),
            subagents: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_sessions_run_in_parallel() {
        let (_tmp, project) = project();
        let slots = Arc::new(tokio::sync::Semaphore::new(3));
        let a = Session::new("a", "ok");
        let b = Session::new("b", "ok");
        let a_id = a.id.clone();
        let b_id = b.id.clone();
        let a = Arc::new(tokio::sync::Mutex::new(a));
        let b = Arc::new(tokio::sync::Mutex::new(b));
        let (tx_a, _rx_a) = mpsc::channel();
        let (tx_b, _rx_b) = mpsc::channel();
        let gate = Arc::new(AtomicUsize::new(0));
        let ha = {
            let slots = slots.clone();
            let project = project.clone();
            let cancel = CancellationToken::new();
            let deps = deps_for(
                Arc::new(OkProvider::gated(gate.clone())),
                project,
                cancel,
                a_id,
                tx_a,
            );
            tokio::spawn(async move {
                let _p = slots.acquire_owned().await.unwrap();
                run_turn(a, Some("go".into()), deps).await
            })
        };
        let hb = {
            let slots = slots.clone();
            let cancel = CancellationToken::new();
            let deps = deps_for(
                Arc::new(OkProvider::gated(gate.clone())),
                project,
                cancel,
                b_id,
                tx_b,
            );
            tokio::spawn(async move {
                let _p = slots.acquire_owned().await.unwrap();
                run_turn(b, Some("go".into()), deps).await
            })
        };
        let ra = ha.await.unwrap();
        let rb = hb.await.unwrap();
        assert_eq!(ra, TurnResult::Completed);
        assert_eq!(rb, TurnResult::Completed);
        assert_eq!(gate.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_one_does_not_stop_the_other() {
        let (_tmp, project) = project();
        let started = Arc::new(AtomicUsize::new(0));
        let a = Session::new("a", "slow");
        let b = Session::new("b", "ok");
        let a_id = a.id.clone();
        let b_id = b.id.clone();
        let a = Arc::new(tokio::sync::Mutex::new(a));
        let b = Arc::new(tokio::sync::Mutex::new(b));
        let cancel_a = CancellationToken::new();
        let (tx_a, _rx_a) = mpsc::channel();
        let (tx_b, _rx_b) = mpsc::channel();
        let ha = tokio::spawn(run_turn(
            a,
            Some("slow".into()),
            deps_for(
                Arc::new(SlowProvider {
                    started: started.clone(),
                }),
                project.clone(),
                cancel_a.clone(),
                a_id,
                tx_a,
            ),
        ));
        let hb = tokio::spawn(run_turn(
            b,
            Some("fast".into()),
            deps_for(
                Arc::new(OkProvider::new()),
                project,
                CancellationToken::new(),
                b_id,
                tx_b,
            ),
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_a.cancel();
        let ra = tokio::time::timeout(Duration::from_secs(2), ha)
            .await
            .expect("a join")
            .unwrap();
        let rb = hb.await.unwrap();
        assert!(
            matches!(ra, TurnResult::Cancelled | TurnResult::Failed(_)),
            "{ra:?}"
        );
        assert_eq!(rb, TurnResult::Completed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn provider_failure_is_isolated() {
        let (_tmp, project) = project();
        let a = Session::new("a", "fail");
        let b = Session::new("b", "ok");
        let a_id = a.id.clone();
        let b_id = b.id.clone();
        let a = Arc::new(tokio::sync::Mutex::new(a));
        let b = Arc::new(tokio::sync::Mutex::new(b));
        let (tx_a, _rx_a) = mpsc::channel();
        let (tx_b, _rx_b) = mpsc::channel();
        let ra = run_turn(
            a,
            Some("x".into()),
            deps_for(
                Arc::new(FailProvider),
                project.clone(),
                CancellationToken::new(),
                a_id,
                tx_a,
            ),
        )
        .await;
        let rb = run_turn(
            b,
            Some("y".into()),
            deps_for(
                Arc::new(OkProvider::new()),
                project,
                CancellationToken::new(),
                b_id,
                tx_b,
            ),
        )
        .await;
        assert!(matches!(ra, TurnResult::Failed(_)));
        assert_eq!(rb, TurnResult::Completed);
    }
}
