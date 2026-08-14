//! Structured stage artifacts. The orchestrator reads these, not free text.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    Planner,
    Coder,
    Verify,
    Reviewer,
    GitGate,
}

impl StageKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Planner => "Planner",
            Self::Coder => "Coder",
            Self::Verify => "Verify",
            Self::Reviewer => "Reviewer",
            Self::GitGate => "Git Gate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerOutput {
    pub tasks: Vec<String>,
    pub decision: String,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub scope: String,
    pub non_goals: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CoderOutput {
    pub files_changed: Vec<String>,
    pub summary: String,
    pub tests_executed: Vec<String>,
    pub test_results: String,
    pub lint_results: String,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcCheck {
    Ok,
    Failed { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcStatus {
    pub id: String,
    pub check: AcCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Pass,
    Fail,
}

impl ReviewVerdict {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pass" | "approved" | "ok" => Some(Self::Pass),
            "fail" | "reject" | "rejected" => Some(Self::Fail),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewerOutput {
    pub verdict: ReviewVerdict,
    pub ac_status: Vec<AcStatus>,
    pub findings: Vec<String>,
    pub required_fixes: Vec<String>,
    pub commit_message: String,
}

#[derive(Debug, Clone)]
pub struct ContractStore {
    dir: PathBuf,
}

impl ContractStore {
    pub fn open(project_root: impl AsRef<Path>) -> Self {
        Self {
            dir: project_root.as_ref().join(".orbit").join("pipeline"),
        }
    }

    pub fn write_planner(&self, output: &PlannerOutput) -> Result<(), String> {
        self.write_json("planner.json", output)
    }

    pub fn planner(&self) -> Result<Option<PlannerOutput>, String> {
        self.read_json("planner.json")
    }

    pub fn write_coder(&self, output: &CoderOutput) -> Result<(), String> {
        self.write_json("coder.json", output)
    }

    pub fn coder(&self) -> Result<Option<CoderOutput>, String> {
        self.read_json("coder.json")
    }

    pub fn write_reviewer(&self, output: &ReviewerOutput) -> Result<(), String> {
        self.write_json("reviewer.json", output)
    }

    pub fn reviewer(&self) -> Result<Option<ReviewerOutput>, String> {
        self.read_json("reviewer.json")
    }

    /// Acceptance criteria are immutable once the Planner has written them.
    #[cfg(test)]
    pub fn replace_acceptance_criteria(
        &self,
        criteria: Vec<AcceptanceCriterion>,
        allow_overwrite: bool,
    ) -> Result<PlannerOutput, String> {
        let mut plan = self.planner()?.ok_or_else(|| {
            "no planner output yet — record the plan before changing acceptance criteria"
                .to_string()
        })?;
        if !plan.acceptance_criteria.is_empty() && !allow_overwrite {
            return Err("acceptance criteria are immutable after the Planner writes them".into());
        }
        plan.acceptance_criteria = criteria;
        self.write_planner(&plan)?;
        Ok(plan)
    }

    fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.dir.join(name);
        let json = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    fn read_json<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<Option<T>, String> {
        let path = self.dir.join(name);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, ContractStore) {
        let tmp = TempDir::new().unwrap();
        let store = ContractStore::open(tmp.path());
        (tmp, store)
    }

    #[test]
    fn planner_emits_acs_and_coder_cannot_change_them() {
        let (_tmp, store) = store();
        store
            .write_planner(&PlannerOutput {
                tasks: vec!["add search".into()],
                decision: "Reuse FTS".into(),
                acceptance_criteria: vec![AcceptanceCriterion {
                    id: "AC1".into(),
                    text: "Ctrl+K focuses search".into(),
                }],
                scope: "sidebar".into(),
                non_goals: "folders".into(),
            })
            .unwrap();
        let err = store
            .replace_acceptance_criteria(
                vec![AcceptanceCriterion {
                    id: "AC1".into(),
                    text: "changed".into(),
                }],
                false,
            )
            .unwrap_err();
        assert!(err.contains("immutable"));
        let plan = store.planner().unwrap().unwrap();
        assert_eq!(plan.acceptance_criteria[0].text, "Ctrl+K focuses search");
    }

    #[test]
    fn reviewer_verdict_is_structured_not_a_string_parse() {
        let (_tmp, store) = store();
        store
            .write_reviewer(&ReviewerOutput {
                verdict: ReviewVerdict::Fail,
                ac_status: vec![AcStatus {
                    id: "AC3".into(),
                    check: AcCheck::Failed {
                        detail: "Migration can overwrite existing records.".into(),
                    },
                }],
                findings: vec!["data loss risk".into()],
                required_fixes: vec!["add upsert guard".into()],
                commit_message: String::new(),
            })
            .unwrap();
        let review = store.reviewer().unwrap().unwrap();
        assert_eq!(review.verdict, ReviewVerdict::Fail);
        assert!(matches!(
            &review.ac_status[0].check,
            AcCheck::Failed { detail } if detail.contains("overwrite")
        ));
    }
}
