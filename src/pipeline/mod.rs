//! N3 — Pipeline orchestrator, contract, verification and stage signal.

pub mod contract;
pub mod verify;

use crate::session::SessionId;
use crate::session::agent_loop::TurnResult;
use contract::{ReviewVerdict, ReviewerOutput, StageKind};

/// Result of a pipeline stage, derived from `TurnResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageResult {
    Completed,
    IterationLimitReached,
    BudgetExceeded,
    Cancelled,
    Failed(String),
}

impl From<TurnResult> for StageResult {
    fn from(result: TurnResult) -> Self {
        match result {
            TurnResult::Completed => StageResult::Completed,
            TurnResult::IterationLimitReached => StageResult::IterationLimitReached,
            TurnResult::BudgetExceeded => StageResult::BudgetExceeded,
            TurnResult::Cancelled => StageResult::Cancelled,
            TurnResult::Failed(msg) => StageResult::Failed(msg),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineEvent {
    StageFinished {
        session_id: SessionId,
        result: StageResult,
    },
}

impl PipelineEvent {
    pub fn stage_finished(session_id: SessionId, result: TurnResult) -> Self {
        PipelineEvent::StageFinished {
            session_id,
            result: result.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Complexity {
    Trivial,
    Normal,
    Complex,
}

impl Complexity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Trivial => "Trivial",
            Self::Normal => "Normal",
            Self::Complex => "Complex",
        }
    }

    pub fn all() -> [Complexity; 3] {
        [Self::Trivial, Self::Normal, Self::Complex]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GitGateMode {
    #[default]
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageModel {
    pub auto: bool,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineConfig {
    pub feature: String,
    pub complexity: Complexity,
    pub planner: StageModel,
    pub coder: StageModel,
    pub reviewer: StageModel,
    pub git_gate: GitGateMode,
    pub auto_planner_to_coder: bool,
    pub auto_coder_to_reviewer: bool,
}

impl PipelineConfig {
    pub fn stages(&self) -> Vec<StageKind> {
        match self.complexity {
            Complexity::Trivial => vec![StageKind::Coder],
            Complexity::Normal => vec![StageKind::Planner, StageKind::Coder, StageKind::Verify],
            Complexity::Complex => vec![
                StageKind::Planner,
                StageKind::Coder,
                StageKind::Verify,
                StageKind::Reviewer,
                StageKind::GitGate,
            ],
        }
    }

    pub fn intelligence_stages(&self) -> Vec<StageKind> {
        self.stages()
            .into_iter()
            .filter(|s| {
                matches!(
                    s,
                    StageKind::Planner | StageKind::Coder | StageKind::Reviewer
                )
            })
            .collect()
    }

    pub fn model_for(&self, stage: StageKind) -> &str {
        match stage {
            StageKind::Planner => &self.planner.model,
            StageKind::Coder => &self.coder.model,
            StageKind::Reviewer => &self.reviewer.model,
            StageKind::Verify | StageKind::GitGate => "",
        }
    }

    pub fn prompt_for(&self, stage: StageKind) -> String {
        match stage {
            StageKind::Planner => format!(
                "Plan this feature as Architect. Call record_plan with tasks, decision, \
                 acceptance criteria, scope and non-goals. Do not write code.\n\n{}",
                self.feature
            ),
            StageKind::Coder => format!(
                "Implement the planned feature. Follow the acceptance criteria; do not change them.\n\n{}",
                self.feature
            ),
            StageKind::Reviewer => format!(
                "Review the implementation against the acceptance criteria. \
                 Call approve_stage with pass or fail and per-AC status.\n\n{}",
                self.feature
            ),
            StageKind::Verify | StageKind::GitGate => String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PipelineBlock {
    pub stage: StageKind,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Pipeline {
    pub config: PipelineConfig,
    pub current: StageKind,
    pub review_cycles: u32,
    pub planner_id: Option<SessionId>,
    pub coder_id: Option<SessionId>,
    pub reviewer_id: Option<SessionId>,
    pub waiting_git_gate: bool,
    pub stopped_reason: Option<String>,
    pub transcript: Vec<PipelineBlock>,
    pub cancel: tokio_util::sync::CancellationToken,
}

pub const MAX_REVIEW_CYCLES: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction {
    None,
    Start {
        stage: StageKind,
        session: SessionId,
        prompt: String,
    },
    RunVerify,
    WaitGitGate,
    Stop {
        reason: String,
    },
}

impl Pipeline {
    pub fn new(config: PipelineConfig) -> Self {
        let current = config
            .stages()
            .into_iter()
            .next()
            .unwrap_or(StageKind::Coder);
        Self {
            config,
            current,
            review_cycles: 0,
            planner_id: None,
            coder_id: None,
            reviewer_id: None,
            waiting_git_gate: false,
            stopped_reason: None,
            transcript: Vec::new(),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    pub fn bind_session(&mut self, stage: StageKind, id: SessionId) {
        match stage {
            StageKind::Planner => self.planner_id = Some(id),
            StageKind::Coder => self.coder_id = Some(id),
            StageKind::Reviewer => self.reviewer_id = Some(id),
            StageKind::Verify | StageKind::GitGate => {}
        }
    }

    pub fn session_for(&self, stage: StageKind) -> Option<&SessionId> {
        match stage {
            StageKind::Planner => self.planner_id.as_ref(),
            StageKind::Coder => self.coder_id.as_ref(),
            StageKind::Reviewer => self.reviewer_id.as_ref(),
            StageKind::Verify | StageKind::GitGate => None,
        }
    }

    pub fn first_start(&self) -> Option<NextAction> {
        let stage = self.current;
        let session = self.session_for(stage)?.clone();
        Some(NextAction::Start {
            stage,
            session,
            prompt: self.config.prompt_for(stage),
        })
    }

    pub fn note(&mut self, stage: StageKind, text: impl Into<String>) {
        self.transcript.push(PipelineBlock {
            stage,
            text: text.into(),
        });
    }

    pub fn on_stage_finished(
        &mut self,
        event: &PipelineEvent,
        review: Option<&ReviewerOutput>,
    ) -> NextAction {
        if self.stopped_reason.is_some() {
            return NextAction::None;
        }
        let PipelineEvent::StageFinished { session_id, result } = event;
        let Some(stage) = self.stage_of(session_id) else {
            return NextAction::None;
        };
        if !matches!(result, StageResult::Completed) {
            let reason = format!("{stage:?} stopped: {result:?}");
            self.stopped_reason = Some(reason.clone());
            self.note(stage, &reason);
            return NextAction::Stop { reason };
        }
        self.note(stage, format!("{} finished", stage.label()));
        self.advance_after(stage, review)
    }

    pub fn on_verify_finished(&mut self, passed: bool, summary: &str) -> NextAction {
        self.note(
            StageKind::Verify,
            if passed {
                "Verification passed".into()
            } else {
                format!("Verification reported failures:\n{summary}")
            },
        );
        self.advance_after(StageKind::Verify, None)
    }

    fn advance_after(
        &mut self,
        finished: StageKind,
        review: Option<&ReviewerOutput>,
    ) -> NextAction {
        let stages = self.config.stages();
        match finished {
            StageKind::Planner => {
                if !self.config.auto_planner_to_coder {
                    return NextAction::None;
                }
                self.start_or_skip(&stages, StageKind::Coder)
            }
            StageKind::Coder => {
                if stages.contains(&StageKind::Verify) {
                    self.current = StageKind::Verify;
                    return NextAction::RunVerify;
                }
                self.start_or_skip(&stages, StageKind::Reviewer)
            }
            StageKind::Verify => {
                if !stages.contains(&StageKind::Reviewer) {
                    if stages.contains(&StageKind::GitGate) {
                        return self.wait_git_gate();
                    }
                    self.stopped_reason = Some("Pipeline complete".into());
                    return NextAction::Stop {
                        reason: "Pipeline complete".into(),
                    };
                }
                if !self.config.auto_coder_to_reviewer {
                    return NextAction::None;
                }
                self.start_or_skip(&stages, StageKind::Reviewer)
            }
            StageKind::Reviewer => self.after_reviewer(review),
            StageKind::GitGate => NextAction::None,
        }
    }

    fn after_reviewer(&mut self, review: Option<&ReviewerOutput>) -> NextAction {
        let pass = review.is_some_and(|r| r.verdict == ReviewVerdict::Pass);
        if pass {
            return self.wait_git_gate();
        }
        self.review_cycles += 1;
        if self.review_cycles >= MAX_REVIEW_CYCLES {
            let reason =
                format!("Review loop stopped after {MAX_REVIEW_CYCLES} Coder→Reviewer cycles.");
            self.stopped_reason = Some(reason.clone());
            self.note(StageKind::Reviewer, &reason);
            return NextAction::Stop { reason };
        }
        self.note(
            StageKind::Reviewer,
            format!("Rejected (cycle {}). Re-running Coder.", self.review_cycles),
        );
        self.start_or_skip(&self.config.stages(), StageKind::Coder)
    }

    fn wait_git_gate(&mut self) -> NextAction {
        if !self.config.stages().contains(&StageKind::GitGate) {
            self.stopped_reason = Some("Pipeline complete".into());
            return NextAction::Stop {
                reason: "Pipeline complete".into(),
            };
        }
        self.current = StageKind::GitGate;
        self.waiting_git_gate = true;
        self.note(StageKind::GitGate, "Waiting for manual Git Gate approval.");
        NextAction::WaitGitGate
    }

    fn start_or_skip(&mut self, stages: &[StageKind], want: StageKind) -> NextAction {
        if !stages.contains(&want) {
            return match want {
                StageKind::Coder => self.advance_after(StageKind::Coder, None),
                StageKind::Reviewer => self.after_reviewer(None),
                _ => NextAction::None,
            };
        }
        self.current = want;
        let Some(session) = self.session_for(want).cloned() else {
            return NextAction::None;
        };
        NextAction::Start {
            stage: want,
            session,
            prompt: self.config.prompt_for(want),
        }
    }

    fn stage_of(&self, id: &SessionId) -> Option<StageKind> {
        if self.planner_id.as_ref() == Some(id) {
            Some(StageKind::Planner)
        } else if self.coder_id.as_ref() == Some(id) {
            Some(StageKind::Coder)
        } else if self.reviewer_id.as_ref() == Some(id) {
            Some(StageKind::Reviewer)
        } else {
            None
        }
    }

    pub fn cancel_all(&mut self) {
        self.cancel.cancel();
        self.stopped_reason = Some("Pipeline cancelled".into());
        self.waiting_git_gate = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::contract::{AcCheck, AcStatus};

    fn cfg(complexity: Complexity) -> PipelineConfig {
        PipelineConfig {
            feature: "add search".into(),
            complexity,
            planner: StageModel {
                auto: true,
                model: "planner-model".into(),
            },
            coder: StageModel {
                auto: true,
                model: "coder-model".into(),
            },
            reviewer: StageModel {
                auto: true,
                model: "reviewer-model".into(),
            },
            git_gate: GitGateMode::Manual,
            auto_planner_to_coder: true,
            auto_coder_to_reviewer: true,
        }
    }

    fn pipe(complexity: Complexity) -> Pipeline {
        let mut p = Pipeline::new(cfg(complexity));
        p.bind_session(StageKind::Planner, SessionId::new("p"));
        p.bind_session(StageKind::Coder, SessionId::new("c"));
        p.bind_session(StageKind::Reviewer, SessionId::new("r"));
        p
    }

    fn done(id: &str) -> PipelineEvent {
        PipelineEvent::stage_finished(SessionId::new(id), TurnResult::Completed)
    }

    #[test]
    fn completed_turn_maps_to_completed_stage() {
        let ev = PipelineEvent::stage_finished(SessionId::new("s1"), TurnResult::Completed);
        assert_eq!(
            ev,
            PipelineEvent::StageFinished {
                session_id: SessionId::new("s1"),
                result: StageResult::Completed,
            }
        );
    }

    #[test]
    fn failed_turn_carries_the_message() {
        let ev = PipelineEvent::stage_finished(
            SessionId::new("s2"),
            TurnResult::Failed("timeout".into()),
        );
        match ev {
            PipelineEvent::StageFinished { result, .. } => {
                assert_eq!(result, StageResult::Failed("timeout".into()))
            }
        }
    }

    #[test]
    fn cancelled_turn_maps_to_cancelled_stage() {
        let ev = PipelineEvent::stage_finished(SessionId::new("s3"), TurnResult::Cancelled);
        assert!(matches!(
            ev,
            PipelineEvent::StageFinished {
                result: StageResult::Cancelled,
                ..
            }
        ));
    }

    #[test]
    fn trivial_flow_is_coder_only() {
        let p = pipe(Complexity::Trivial);
        assert_eq!(p.config.stages(), vec![StageKind::Coder]);
        assert_eq!(p.config.intelligence_stages(), vec![StageKind::Coder]);
    }

    #[test]
    fn complex_walks_planner_coder_verify_reviewer_git_gate() {
        let mut p = pipe(Complexity::Complex);
        let next = p.on_stage_finished(&done("p"), None);
        assert!(matches!(
            next,
            NextAction::Start {
                stage: StageKind::Coder,
                ..
            }
        ));
        let next = p.on_stage_finished(&done("c"), None);
        assert_eq!(next, NextAction::RunVerify);
        let next = p.on_verify_finished(true, "ok");
        assert!(matches!(
            next,
            NextAction::Start {
                stage: StageKind::Reviewer,
                ..
            }
        ));
        let pass = ReviewerOutput {
            verdict: ReviewVerdict::Pass,
            ac_status: vec![AcStatus {
                id: "AC1".into(),
                check: AcCheck::Ok,
            }],
            findings: Vec::new(),
            required_fixes: Vec::new(),
            commit_message: "feat: search".into(),
        };
        let next = p.on_stage_finished(&done("r"), Some(&pass));
        assert_eq!(next, NextAction::WaitGitGate);
        assert!(p.waiting_git_gate);
    }

    #[test]
    fn reviewer_reject_reruns_coder_then_stops_at_three() {
        let mut p = pipe(Complexity::Complex);
        p.current = StageKind::Reviewer;
        let fail = ReviewerOutput {
            verdict: ReviewVerdict::Fail,
            ac_status: Vec::new(),
            findings: vec!["bug".into()],
            required_fixes: Vec::new(),
            commit_message: String::new(),
        };
        for cycle in 1..=2 {
            let next = p.on_stage_finished(&done("r"), Some(&fail));
            assert!(
                matches!(
                    next,
                    NextAction::Start {
                        stage: StageKind::Coder,
                        ..
                    }
                ),
                "cycle {cycle}: {next:?}"
            );
            let _ = p.on_stage_finished(&done("c"), None);
            let _ = p.on_verify_finished(true, "ok");
        }
        let next = p.on_stage_finished(&done("r"), Some(&fail));
        assert!(matches!(next, NextAction::Stop { .. }));
        assert_eq!(p.review_cycles, 3);
        assert!(p.stopped_reason.as_deref().is_some_and(|s| s.contains("3")));
    }

    #[test]
    fn git_gate_waits_for_manual_approval() {
        let mut p = pipe(Complexity::Complex);
        p.current = StageKind::Reviewer;
        let pass = ReviewerOutput {
            verdict: ReviewVerdict::Pass,
            ac_status: Vec::new(),
            findings: Vec::new(),
            required_fixes: Vec::new(),
            commit_message: "ok".into(),
        };
        assert_eq!(
            p.on_stage_finished(&done("r"), Some(&pass)),
            NextAction::WaitGitGate
        );
    }
}
