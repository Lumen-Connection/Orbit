//! Hook declarations in `.orbit/config.toml` and their machine-local trust.

use crate::security::declared::DeclaredCommand;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "PreToolUse" => Some(Self::PreToolUse),
            "PostToolUse" => Some(Self::PostToolUse),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookConfig {
    pub event: String,
    pub matcher: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl HookConfig {
    pub fn kind(&self) -> Option<HookEvent> {
        HookEvent::parse(&self.event)
    }

    pub fn fingerprint(&self) -> String {
        self.declared().fingerprint()
    }

    pub fn declared(&self) -> DeclaredCommand {
        DeclaredCommand {
            command: self.command.clone(),
            args: self.args.clone(),
        }
    }

    pub fn is_denied(&self) -> bool {
        self.declared().is_denied()
    }

    pub fn display(&self) -> String {
        self.declared().display()
    }

    pub fn matches_tool(&self, tool_name: &str) -> bool {
        let Ok(re) = regex::Regex::new(&self.matcher) else {
            return false;
        };
        re.is_match(tool_name)
    }
}

pub fn load_hooks(root: &Path) -> Vec<HookConfig> {
    let path = crate::context::store::OrbitStore::dir_for(root).join("config.toml");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    value
        .get("hooks")
        .cloned()
        .and_then(|v| v.try_into().ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{HookConfig, HookEvent, load_hooks};

    #[test]
    fn matcher_is_a_regex_on_the_tool_name() {
        let hook = HookConfig {
            event: "PreToolUse".into(),
            matcher: "write_file|edit_file|multi_edit".into(),
            command: "python".into(),
            args: vec!["scripts/guard.py".into()],
        };
        assert!(hook.matches_tool("write_file"));
        assert!(hook.matches_tool("edit_file"));
        assert!(!hook.matches_tool("read_file"));
        assert_eq!(hook.kind(), Some(HookEvent::PreToolUse));
    }

    #[test]
    fn load_hooks_reads_array_of_tables() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join(".orbit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            r#"
[[hooks]]
event = "PreToolUse"
matcher = "write_file"
command = "python"
args = ["scripts/guard.py"]
"#,
        )
        .unwrap();
        let hooks = load_hooks(tmp.path());
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].command, "python");
        assert_eq!(hooks[0].args, ["scripts/guard.py"]);
    }

    #[test]
    fn editing_args_changes_fingerprint() {
        let a = HookConfig {
            event: "PreToolUse".into(),
            matcher: "write_file".into(),
            command: "python".into(),
            args: vec!["guard.py".into()],
        };
        let mut b = a.clone();
        b.args.push("--strict".into());
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
