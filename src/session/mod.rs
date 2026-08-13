//! Independent agent sessions and the tool-dispatch loop.

pub mod agent_loop;
pub mod context_window;
pub mod export;
pub mod manager;
pub mod message_ops;

pub use manager::SessionManager;

use crate::providers::ChatMessage;
use crate::security::{ApprovalDecision, ApprovalId, ProposedCommand};
use crate::workspace::FilePatch;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn fresh() -> Self {
        Self::new(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct SessionLimits {
    pub max_iterations: u32,
    pub budget_usd: f64,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            max_iterations: 25,
            budget_usd: DEFAULT_SESSION_BUDGET_USD,
        }
    }
}

pub const DEFAULT_SESSION_BUDGET_USD: f64 = 2.0;

#[derive(Debug, Clone)]
pub struct Session {
    pub id: SessionId,
    pub label: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub limits: SessionLimits,
    pub context_summary: Option<String>,
    pub context_summary_upto: usize,
}

impl Session {
    pub fn new(label: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            id: SessionId::fresh(),
            label: label.into(),
            model: model.into(),
            messages: Vec::new(),
            limits: SessionLimits::default(),
            context_summary: None,
            context_summary_upto: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApprovalHandle {
    pub id: ApprovalId,
    pub tool_name: String,
    pub summary: String,
    pub patch: Option<FilePatch>,
    pub command: Option<ProposedCommand>,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Delta(String),
    ToolStarted {
        call_id: String,
        name: String,
        summary: String,
    },
    ToolFinished {
        call_id: String,
        name: String,
        output: String,
        is_error: bool,
    },
    ApprovalRequired(ApprovalHandle),
    Usage {
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: f64,
        latency_ms: u64,
        iteration: u32,
        spent_usd: f64,
    },
    BudgetExceeded {
        spent: f64,
        cap: f64,
    },
    TurnFinished,
    IterationLimitReached,
    Failed(String),
    Unauthorized,
    ContextOccupancy(f32),
    Retrying {
        attempt: u32,
        max_attempts: u32,
        wait_secs: u64,
    },
}

pub const AUTH_REJECTED_NOTICE: &str =
    "⚠ The API key was rejected. Update it above to continue this conversation.";

#[derive(Debug, Default)]
pub struct ApprovalBridge {
    pending: Mutex<HashMap<ApprovalId, oneshot::Sender<ApprovalDecision>>>,
}

impl ApprovalBridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn register(&self, id: ApprovalId) -> oneshot::Receiver<ApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut map) = self.pending.lock() {
            map.insert(id, tx);
        }
        rx
    }

    pub fn resolve(&self, id: ApprovalId, decision: ApprovalDecision) {
        if let Ok(mut map) = self.pending.lock()
            && let Some(tx) = map.remove(&id)
        {
            let _ = tx.send(decision);
        }
    }

    pub fn deny_all(&self) {
        if let Ok(mut map) = self.pending.lock() {
            for (_, tx) in map.drain() {
                let _ = tx.send(ApprovalDecision::Denied);
            }
        }
    }

    pub fn is_pending(&self, id: ApprovalId) -> bool {
        self.pending
            .lock()
            .map(|map| map.contains_key(&id))
            .unwrap_or(false)
    }
}

#[derive(Debug, Default)]
pub struct BudgetBridge {
    pending: Mutex<Option<oneshot::Sender<Option<f64>>>>,
}

impl BudgetBridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn register(&self) -> oneshot::Receiver<Option<f64>> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut slot) = self.pending.lock() {
            *slot = Some(tx);
        }
        rx
    }

    pub fn resolve(&self, new_cap: Option<f64>) {
        if let Ok(mut slot) = self.pending.lock()
            && let Some(tx) = slot.take()
        {
            let _ = tx.send(new_cap);
        }
    }

    pub fn cancel(&self) {
        self.resolve(None);
    }
}

#[derive(Debug, Clone)]
pub enum TranscriptItem {
    User(String),
    Assistant(String),
    Tool {
        call_id: String,
        name: String,
        summary: String,
        output: String,
        is_error: bool,
        running: bool,
        expanded: bool,
    },
    Approval {
        handle: ApprovalHandle,
        resolved: Option<ApprovalDecision>,
    },
}

/// Fold a runtime event into the UI transcript.
///
/// Returns `true` while the turn is still running.
pub fn apply_agent_event(items: &mut Vec<TranscriptItem>, event: AgentEvent) -> bool {
    match event {
        AgentEvent::Delta(text) => {
            match items.last_mut() {
                Some(TranscriptItem::Assistant(buf)) => buf.push_str(&text),
                _ => items.push(TranscriptItem::Assistant(text)),
            }
            true
        }
        AgentEvent::ToolStarted {
            call_id,
            name,
            summary,
        } => {
            items.push(TranscriptItem::Tool {
                call_id,
                name,
                summary,
                output: String::new(),
                is_error: false,
                running: true,
                expanded: false,
            });
            true
        }
        AgentEvent::ToolFinished {
            call_id,
            name,
            output,
            is_error,
        } => {
            if let Some(TranscriptItem::Tool {
                output: existing,
                is_error: err,
                running,
                ..
            }) = items.iter_mut().rev().find(
                |item| matches!(item, TranscriptItem::Tool { call_id: id, .. } if *id == call_id),
            ) {
                *existing = output;
                *err = is_error;
                *running = false;
            } else {
                items.push(TranscriptItem::Tool {
                    call_id,
                    name: name.clone(),
                    summary: name,
                    output,
                    is_error,
                    running: false,
                    expanded: false,
                });
            }
            true
        }
        AgentEvent::ApprovalRequired(handle) => {
            items.push(TranscriptItem::Approval {
                handle,
                resolved: None,
            });
            true
        }
        AgentEvent::Usage { .. }
        | AgentEvent::BudgetExceeded { .. }
        | AgentEvent::ContextOccupancy(_)
        | AgentEvent::Retrying { .. } => true,
        AgentEvent::TurnFinished => false,
        AgentEvent::IterationLimitReached => {
            items.push(TranscriptItem::Assistant(
                "Stopped: iteration limit reached for this turn.".into(),
            ));
            false
        }
        AgentEvent::Failed(msg) => {
            items.push(TranscriptItem::Assistant(format!("⚠ {msg}")));
            false
        }
        AgentEvent::Unauthorized => {
            items.push(TranscriptItem::Assistant(AUTH_REJECTED_NOTICE.into()));
            false
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveWork {
    pub generating: u32,
    pub pending_approvals: u32,
    pub running_processes: u32,
    pub unapplied_patches: u32,
}

impl ActiveWork {
    pub fn is_empty(&self) -> bool {
        self.generating == 0
            && self.pending_approvals == 0
            && self.running_processes == 0
            && self.unapplied_patches == 0
    }

    pub fn summary_line(&self) -> String {
        let mut parts = Vec::new();
        if self.generating > 0 {
            parts.push(format!(
                "{} session{} generating",
                self.generating,
                if self.generating == 1 { "" } else { "s" }
            ));
        }
        if self.pending_approvals > 0 {
            parts.push(format!(
                "{} pending approval{}",
                self.pending_approvals,
                if self.pending_approvals == 1 { "" } else { "s" }
            ));
        }
        if self.running_processes > 0 {
            parts.push(format!(
                "{} process{} running",
                self.running_processes,
                if self.running_processes == 1 {
                    ""
                } else {
                    "es"
                }
            ));
        }
        if self.unapplied_patches > 0 {
            parts.push(format!(
                "{} unapplied patch{}",
                self.unapplied_patches,
                if self.unapplied_patches == 1 {
                    ""
                } else {
                    "es"
                }
            ));
        }
        parts.join(" · ")
    }
}

pub fn summarize_active_work(
    sessions: &SessionManager,
    pending_patches: &[crate::workspace::FilePatch],
    running_processes: u32,
) -> ActiveWork {
    let generating = sessions.sessions.iter().filter(|s| s.busy).count() as u32;
    let pending_approvals = sessions
        .sessions
        .iter()
        .map(|s| {
            s.transcript
                .iter()
                .filter(|item| matches!(item, TranscriptItem::Approval { resolved: None, .. }))
                .count()
        })
        .sum::<usize>() as u32;
    let unapplied_patches = pending_patches
        .iter()
        .filter(|p| matches!(p.status, crate::workspace::PatchStatus::Pending))
        .count() as u32;
    ActiveWork {
        generating,
        pending_approvals,
        running_processes,
        unapplied_patches,
    }
}

pub fn mark_approval(items: &mut [TranscriptItem], id: ApprovalId, decision: ApprovalDecision) {
    for item in items.iter_mut() {
        if let TranscriptItem::Approval { handle, resolved } = item
            && handle.id == id
        {
            *resolved = Some(decision);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveWork, AgentEvent, ApprovalHandle, TranscriptItem, apply_agent_event, mark_approval,
    };
    use crate::security::{ApprovalDecision, ApprovalId};

    #[test]
    fn deltas_append_to_one_assistant_bubble() {
        let mut items = Vec::new();
        assert!(apply_agent_event(
            &mut items,
            AgentEvent::Delta("hel".into())
        ));
        assert!(apply_agent_event(
            &mut items,
            AgentEvent::Delta("lo".into())
        ));
        assert!(matches!(
            items.as_slice(),
            [TranscriptItem::Assistant(text)] if text == "hello"
        ));
    }

    #[test]
    fn tool_started_then_finished_updates_same_row() {
        let mut items = Vec::new();
        apply_agent_event(
            &mut items,
            AgentEvent::ToolStarted {
                call_id: "c1".into(),
                name: "grep".into(),
                summary: "grep(\"fn authenticate\")".into(),
            },
        );
        apply_agent_event(
            &mut items,
            AgentEvent::ToolFinished {
                call_id: "c1".into(),
                name: "grep".into(),
                output: "a\nb\nc".into(),
                is_error: false,
            },
        );
        match &items[0] {
            TranscriptItem::Tool {
                running,
                is_error,
                output,
                summary,
                ..
            } => {
                assert!(!*running);
                assert!(!*is_error);
                assert_eq!(output, "a\nb\nc");
                assert_eq!(summary, "grep(\"fn authenticate\")");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn approval_can_be_resolved_inline() {
        let id = ApprovalId::new();
        let mut items = vec![TranscriptItem::Approval {
            handle: ApprovalHandle {
                id,
                tool_name: "write_file".into(),
                summary: "write_file(\"src/lib.rs\")".into(),
                patch: None,
                command: None,
            },
            resolved: None,
        }];
        mark_approval(&mut items, id, ApprovalDecision::Denied);
        assert!(matches!(
            items[0],
            TranscriptItem::Approval {
                resolved: Some(ApprovalDecision::Denied),
                ..
            }
        ));
    }

    #[test]
    fn turn_finished_clears_busy() {
        let mut items = Vec::new();
        assert!(!apply_agent_event(&mut items, AgentEvent::TurnFinished));
    }

    #[test]
    fn active_work_summary_counts_busy_and_approvals() {
        use super::{ApprovalHandle, summarize_active_work};
        use crate::session::manager::SessionManager;
        use crate::workspace::FilePatch;
        let mut mgr = SessionManager::new();
        mgr.create("a", "m");
        mgr.sessions[0].busy = true;
        mgr.sessions[0].transcript.push(TranscriptItem::Approval {
            handle: ApprovalHandle {
                id: ApprovalId::new(),
                tool_name: "write_file".into(),
                summary: "write_file(\"a.rs\")".into(),
                patch: None,
                command: None,
            },
            resolved: None,
        });
        let patch = FilePatch::new(std::path::PathBuf::from("a.rs"), String::new(), "x".into());
        let work = summarize_active_work(&mgr, &[patch], 0);
        assert_eq!(work.generating, 1);
        assert_eq!(work.pending_approvals, 1);
        assert_eq!(work.unapplied_patches, 1);
        assert!(work.summary_line().contains("generating"));
        assert!(!work.is_empty());
        assert!(ActiveWork::default().is_empty());
    }

    #[test]
    fn unauthorized_keeps_the_transcript_and_marks_the_turn_done() {
        let mut items = Vec::new();
        apply_agent_event(&mut items, AgentEvent::Delta("partial".into()));
        assert!(!apply_agent_event(&mut items, AgentEvent::Unauthorized));
        assert!(matches!(
            items.last(),
            Some(TranscriptItem::Assistant(text))
                if text.contains("API key was rejected")
        ));
    }
}
