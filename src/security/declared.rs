//! A command declared in project-owned config (MCP server, hook, …).
//!
//! `config.toml` is commitable, so a cloned repo can bring a malicious
//! command. Fingerprint + machine-local trust is the same problem for
//! every such declaration.

use super::ProposedCommand;
use super::policy::is_absolutely_denied;
use crate::storage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredCommand {
    pub command: String,
    pub args: Vec<String>,
}

impl DeclaredCommand {
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.command, &self.args, &[])
    }

    pub fn as_command(&self) -> ProposedCommand {
        ProposedCommand {
            program: self.command.clone(),
            args: self.args.clone(),
        }
    }

    pub fn is_denied(&self) -> bool {
        is_absolutely_denied(&self.as_command())
    }

    pub fn display(&self) -> String {
        if self.args.is_empty() {
            self.command.clone()
        } else {
            format!("{} {}", self.command, self.args.join(" "))
        }
    }
}

pub fn fingerprint(command: &str, args: &[String], env: &[(String, String)]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(command.as_bytes());
    hasher.update([0]);
    for arg in args {
        hasher.update(arg.as_bytes());
        hasher.update([0]);
    }
    let mut env = env.to_vec();
    env.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, v) in env {
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub struct MachineTrust {
    file: &'static str,
}

impl MachineTrust {
    pub const MCP: Self = Self {
        file: "mcp_trust.json",
    };
    pub const HOOKS: Self = Self {
        file: "hook_trust.json",
    };

    pub fn is_trusted(&self, fingerprint: &str) -> bool {
        load_map(self.file)
            .get(fingerprint)
            .copied()
            .unwrap_or(false)
    }

    pub fn trust(&self, fingerprint: &str) -> Result<(), String> {
        let mut map = load_map(self.file);
        map.insert(fingerprint.to_string(), true);
        save_map(self.file, &map)
    }

    #[cfg(test)]
    pub fn forget(&self, fingerprint: &str) -> Result<(), String> {
        let mut map = load_map(self.file);
        map.remove(fingerprint);
        save_map(self.file, &map)
    }
}

fn data_file(name: &str) -> PathBuf {
    storage::data_dir()
        .map(|dir| dir.join(name))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn load_map(name: &str) -> BTreeMap<String, bool> {
    let Ok(bytes) = std::fs::read(data_file(name)) else {
        return BTreeMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_map(name: &str, map: &BTreeMap<String, bool>) -> Result<(), String> {
    let path = data_file(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookPref {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

const HOOK_PREFS: &str = "hook_prefs.json";

pub fn hook_enabled(fingerprint: &str) -> bool {
    load_prefs()
        .get(fingerprint)
        .map(|p| p.enabled)
        .unwrap_or(true)
}

pub fn set_hook_enabled(fingerprint: &str, enabled: bool) -> Result<(), String> {
    let mut map = load_prefs();
    map.insert(fingerprint.to_string(), HookPref { enabled });
    let path = data_file(HOOK_PREFS);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&map).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

fn load_prefs() -> BTreeMap<String, HookPref> {
    let Ok(bytes) = std::fs::read(data_file(HOOK_PREFS)) else {
        return BTreeMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{DeclaredCommand, fingerprint};

    #[test]
    fn editing_args_changes_fingerprint() {
        let a = DeclaredCommand {
            command: "python".into(),
            args: vec!["guard.py".into()],
        };
        let mut b = a.clone();
        b.args.push("--strict".into());
        assert_ne!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.fingerprint(), fingerprint(&a.command, &a.args, &[]));
    }

    #[test]
    fn denylist_covers_declared_shutdown() {
        let cmd = DeclaredCommand {
            command: "shutdown".into(),
            args: vec!["-h".into(), "now".into()],
        };
        assert!(cmd.is_denied());
    }
}
