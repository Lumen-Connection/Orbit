//! Host used by `spawn_subagent` to register a visible child session.

use super::worktree::Isolation;
use super::{AgentRole, ApprovalBridge, Session, SessionId};
use crate::providers::AiProvider;
use crate::session::AgentEvent;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub const SUBAGENT_BUDGET_FRACTION: f64 = 0.25;
pub const SUBAGENT_MAX_ITER: u32 = 10;

pub struct PendingSubagent {
    pub id: SessionId,
    pub label: String,
    pub model: String,
    pub role: AgentRole,
    pub parent_id: SessionId,
    pub parent_label: String,
    pub isolation: Isolation,
    pub handle: Arc<tokio::sync::Mutex<Session>>,
    pub agent_rx: Receiver<AgentEvent>,
    pub agent_cancel: CancellationToken,
    pub approvals: Arc<ApprovalBridge>,
    pub budget_usd: f64,
}

pub struct SubagentHost {
    pub provider: Arc<dyn AiProvider>,
    pub slots: Arc<Semaphore>,
    pub shared_spent: Arc<Mutex<f64>>,
    pub budget_usd: Option<f64>,
    pub pending: Arc<Mutex<Vec<PendingSubagent>>>,
    pub fraction: f64,
}

impl SubagentHost {
    pub fn remaining(&self) -> f64 {
        let spent = self.shared_spent.lock().map(|g| *g).unwrap_or(0.0);
        match self.budget_usd {
            Some(cap) => (cap - spent).max(0.0),
            None => f64::MAX,
        }
    }

    pub fn slice(&self) -> f64 {
        let fraction = if self.fraction.is_finite() && self.fraction > 0.0 {
            self.fraction.clamp(0.05, 0.5)
        } else {
            SUBAGENT_BUDGET_FRACTION
        };
        let remaining = self.remaining();
        if remaining.is_finite() {
            remaining * fraction
        } else {
            0.0
        }
    }

    pub fn debit(&self, amount: f64) {
        if !amount.is_finite() || amount <= 0.0 {
            return;
        }
        if let Ok(mut spent) = self.shared_spent.lock() {
            *spent += amount;
        }
    }

    pub fn push(&self, pending: PendingSubagent) {
        if let Ok(mut queue) = self.pending.lock() {
            queue.push(pending);
        }
    }
}
