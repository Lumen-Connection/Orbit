//! Agent roles: the persona and tool permissions for a Coder Mode session.
//!
//! A role is not an instruction asking the model to behave well — it is the
//! actual tool catalog sent to the model plus the posture described in its
//! system prompt. A Reviewer does not receive `write_file` in its catalog at
//! all, so it cannot write, rather than promising not to.

use crate::tools::ToolRisk;

/// The four agent roles. Default is `Coder`: a new session needs no choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentRole {
    #[default]
    Coder,
    Reviewer,
    Tester,
    Architect,
}

impl AgentRole {
    /// All roles, in display order: Coder, Reviewer, Tester, Architect.
    pub const ALL: [AgentRole; 4] = [
        AgentRole::Coder,
        AgentRole::Reviewer,
        AgentRole::Tester,
        AgentRole::Architect,
    ];

    /// Stable identifier used for persistence. Round-trips with `from_id`.
    pub fn id(self) -> &'static str {
        match self {
            AgentRole::Coder => "coder",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Tester => "tester",
            AgentRole::Architect => "architect",
        }
    }

    /// Recover a role from its `id()`. Any unknown value degrades to `Coder`
    /// so a corrupt or legacy database never breaks restoration.
    pub fn from_id(id: &str) -> Self {
        match id {
            "reviewer" => AgentRole::Reviewer,
            "tester" => AgentRole::Tester,
            "architect" => AgentRole::Architect,
            _ => AgentRole::Coder,
        }
    }

    /// Human-readable label, for the session tab and role selector.
    pub fn label(self) -> &'static str {
        match self {
            AgentRole::Coder => "Coder",
            AgentRole::Reviewer => "Reviewer",
            AgentRole::Tester => "Tester",
            AgentRole::Architect => "Architect",
        }
    }

    /// A small emoji for the session tab, to identify the role at a glance.
    pub fn icon(self) -> &'static str {
        match self {
            AgentRole::Coder => "🛠️",
            AgentRole::Reviewer => "🔍",
            AgentRole::Tester => "🧪",
            AgentRole::Architect => "🏗️",
        }
    }

    /// The tools this role is allowed to call, exactly per the permission
    /// matrix: Coder/Tester get the writing set, Reviewer loses writes,
    /// Architect keeps reading + context (including documenting skills).
    pub fn allowed_tools(self) -> &'static [&'static str] {
        match self {
            AgentRole::Coder | AgentRole::Tester => &[
                "read_file",
                "list_dir",
                "grep",
                "glob",
                "write_file",
                "edit_file",
                "multi_edit",
                "run_command",
                "list_run_configs",
                "read_run_output",
                "start_run",
                "stop_run",
                "record_decision",
                "add_finding",
                "update_task",
                "read_skill",
                "create_skill",
                "search_history",
                "spawn_subagent",
            ],
            AgentRole::Reviewer => &[
                "read_file",
                "list_dir",
                "grep",
                "glob",
                "run_command",
                "list_run_configs",
                "read_run_output",
                "start_run",
                "stop_run",
                "record_decision",
                "add_finding",
                "update_task",
                "approve_stage",
                "read_skill",
                "search_history",
            ],
            AgentRole::Architect => &[
                "read_file",
                "list_dir",
                "grep",
                "glob",
                "record_decision",
                "add_finding",
                "update_task",
                "record_plan",
                "read_skill",
                "create_skill",
                "search_history",
            ],
        }
    }

    /// Risk ceiling for tools that are not in `CANONICAL_TOOLS` (MCP, test
    /// doubles, spawned helpers). Canonical tools still go through
    /// `allowed_tools()` by name, so Architect may call `create_skill`
    /// (Mutating) while still being barred from an unknown Mutating tool.
    pub fn allows_risk(self, risk: ToolRisk) -> bool {
        match (self, risk) {
            (AgentRole::Coder | AgentRole::Tester, _) => true,
            (AgentRole::Reviewer, ToolRisk::Mutating) => false,
            (AgentRole::Reviewer, ToolRisk::ReadOnly | ToolRisk::Executing) => true,
            (AgentRole::Architect, ToolRisk::ReadOnly) => true,
            (AgentRole::Architect, ToolRisk::Executing | ToolRisk::Mutating) => false,
        }
    }

    /// Short posture fragment appended after `CODER_SYSTEM_PROMPT`. Kept to
    /// 2–4 sentences in English; it describes how the role thinks, and for
    /// Reviewer/Architect it states explicitly that writing tools are absent.
    pub fn prompt_fragment(self) -> &'static str {
        match self {
            AgentRole::Coder => {
                "You implement the requested change: write the code and verify it compiles. \
                 Prefer small, focused edits and confirm behavior with the test suite."
            }
            AgentRole::Reviewer => {
                "You review existing code and point out concrete defects with file and line. \
                 You do not have any writing tools, so propose the fix in text rather than \
                 applying it. You may run tests to confirm a claim instead of speculating."
            }
            AgentRole::Tester => {
                "You focus on coverage: write tests that fail before the fix and pass after, \
                 then run the suite. Keep assertions specific and the test names descriptive."
            }
            AgentRole::Architect => {
                "You analyze structure and trade-offs without writing application code or executing anything. \
                 You have no code-writing or execution tools; record conclusions with record_decision \
                 and document procedures with create_skill."
            }
        }
    }
}

/// All canonical workspace tool names. The dispatch guard (N0.1) only enforces
/// the role boundary for these; custom tools registered by tests (echo, slow,
/// …) are not part of the workspace matrix and pass through untouched.
pub const CANONICAL_TOOLS: [&str; 24] = [
    "read_file",
    "list_dir",
    "grep",
    "glob",
    "write_file",
    "edit_file",
    "multi_edit",
    "run_command",
    "list_run_configs",
    "read_run_output",
    "start_run",
    "stop_run",
    "record_decision",
    "add_finding",
    "update_task",
    "record_plan",
    "approve_stage",
    "git_status",
    "git_commit",
    "git_push",
    "read_skill",
    "create_skill",
    "search_history",
    "spawn_subagent",
];

#[cfg(test)]
mod tests {
    use super::{AgentRole, CANONICAL_TOOLS};
    use crate::tools::{ToolRegistry, ToolRisk};

    #[test]
    fn from_id_round_trips_for_all_roles() {
        for role in AgentRole::ALL {
            assert_eq!(AgentRole::from_id(role.id()), role);
        }
    }

    #[test]
    fn unknown_and_empty_ids_degrade_to_coder() {
        assert_eq!(AgentRole::from_id("nonsense"), AgentRole::Coder);
        assert_eq!(AgentRole::from_id(""), AgentRole::Coder);
        assert_eq!(AgentRole::from_id("architector"), AgentRole::Coder);
    }

    #[test]
    fn coder_and_tester_allow_all_nineteen_tools() {
        for role in [AgentRole::Coder, AgentRole::Tester] {
            assert_eq!(role.allowed_tools().len(), 19);
            assert!(role.allowed_tools().contains(&"read_skill"));
            assert!(role.allowed_tools().contains(&"create_skill"));
            assert!(role.allowed_tools().contains(&"search_history"));
            assert!(role.allowed_tools().contains(&"spawn_subagent"));
        }
    }

    #[test]
    fn reviewer_lacks_the_writing_pair() {
        let tools = AgentRole::Reviewer.allowed_tools();
        assert_eq!(tools.len(), 15);
        assert!(!tools.contains(&"write_file"));
        assert!(!tools.contains(&"edit_file"));
        assert!(!tools.contains(&"create_skill"));
        assert!(tools.contains(&"read_file"));
        assert!(tools.contains(&"read_skill"));
        assert!(tools.contains(&"search_history"));
        assert!(tools.contains(&"approve_stage"));
        assert!(!tools.contains(&"spawn_subagent"));
    }

    #[test]
    fn architect_is_read_only_plus_context() {
        let tools = AgentRole::Architect.allowed_tools();
        assert_eq!(tools.len(), 11);
        for t in ["read_file", "list_dir", "grep", "glob"] {
            assert!(tools.contains(&t), "missing read tool {t}");
        }
        for t in [
            "record_decision",
            "add_finding",
            "update_task",
            "record_plan",
            "read_skill",
            "create_skill",
            "search_history",
        ] {
            assert!(tools.contains(&t), "missing context tool {t}");
        }
        for t in [
            "write_file",
            "edit_file",
            "run_command",
            "start_run",
            "stop_run",
        ] {
            assert!(!tools.contains(&t), "architect must not have {t}");
        }
        assert!(!tools.contains(&"spawn_subagent"));
    }

    #[test]
    fn every_allowed_tool_is_canonical() {
        for role in AgentRole::ALL {
            for tool in role.allowed_tools() {
                assert!(
                    CANONICAL_TOOLS.contains(tool),
                    "`{tool}` for {role:?} is not a canonical tool"
                );
            }
        }
    }

    #[test]
    fn allows_risk_matches_the_role_table() {
        for role in [AgentRole::Coder, AgentRole::Tester] {
            assert!(role.allows_risk(ToolRisk::ReadOnly));
            assert!(role.allows_risk(ToolRisk::Executing));
            assert!(role.allows_risk(ToolRisk::Mutating));
        }
        assert!(AgentRole::Reviewer.allows_risk(ToolRisk::ReadOnly));
        assert!(AgentRole::Reviewer.allows_risk(ToolRisk::Executing));
        assert!(!AgentRole::Reviewer.allows_risk(ToolRisk::Mutating));
        assert!(AgentRole::Architect.allows_risk(ToolRisk::ReadOnly));
        assert!(!AgentRole::Architect.allows_risk(ToolRisk::Executing));
        assert!(!AgentRole::Architect.allows_risk(ToolRisk::Mutating));
    }

    #[test]
    fn allowed_canonical_tools_satisfy_allows_risk() {
        let registry = ToolRegistry::workspace_tools();
        for role in AgentRole::ALL {
            for name in role.allowed_tools() {
                let tool = registry.get(name).unwrap_or_else(|| panic!("{name}"));
                // Architect may document procedures via the named `create_skill`
                // catalog entry. `allows_risk` still rejects unknown Mutating
                // tools (MCP, test doubles) for that role.
                if role == AgentRole::Architect && *name == "create_skill" {
                    assert_eq!(tool.risk(), ToolRisk::Mutating);
                    assert!(!role.allows_risk(tool.risk()));
                    continue;
                }
                assert!(
                    role.allows_risk(tool.risk()),
                    "{role:?} allows `{name}` ({:?}) but allows_risk is false",
                    tool.risk()
                );
            }
        }
    }
}
