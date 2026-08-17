//! MCP server declarations and machine-local trust / risk overrides.

use crate::security::ProposedCommand;
use crate::security::policy::is_absolutely_denied;
use crate::storage;
use crate::tools::ToolRisk;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const TRUST_FILE: &str = "mcp_trust.json";
const RISK_FILE: &str = "mcp_risk.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl McpServerConfig {
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.command.as_bytes());
        hasher.update([0]);
        for arg in &self.args {
            hasher.update(arg.as_bytes());
            hasher.update([0]);
        }
        let mut env = self.env.clone();
        env.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in env {
            hasher.update(k.as_bytes());
            hasher.update(b"=");
            hasher.update(v.as_bytes());
            hasher.update([0]);
        }
        format!("{:x}", hasher.finalize())
    }

    pub fn as_command(&self) -> ProposedCommand {
        ProposedCommand {
            program: self.command.clone(),
            args: self.args.clone(),
        }
    }

    pub fn display(&self) -> String {
        if self.args.is_empty() {
            self.command.clone()
        } else {
            format!("{} {}", self.command, self.args.join(" "))
        }
    }

    pub fn is_denied(&self) -> bool {
        is_absolutely_denied(&self.as_command())
    }
}

pub fn load_servers(root: &Path) -> Vec<McpServerConfig> {
    let path = crate::context::store::OrbitStore::dir_for(root).join("config.toml");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    value
        .get("mcp")
        .and_then(|m| m.get("servers"))
        .and_then(|v| v.clone().try_into().ok())
        .unwrap_or_default()
}

pub fn is_trusted(config: &McpServerConfig) -> bool {
    load_map(TRUST_FILE)
        .get(&config.fingerprint())
        .copied()
        .unwrap_or(false)
}

pub fn trust_on_this_machine(config: &McpServerConfig) -> Result<(), String> {
    let mut map = load_map(TRUST_FILE);
    map.insert(config.fingerprint(), true);
    save_map(TRUST_FILE, &map)
}

pub fn risk_override(qualified: &str) -> Option<ToolRisk> {
    let map = load_string_map(RISK_FILE);
    match map.get(qualified).map(|s| s.as_str()) {
        Some("readonly") => Some(ToolRisk::ReadOnly),
        Some("executing") => Some(ToolRisk::Executing),
        _ => None,
    }
}

pub fn set_risk_override(qualified: &str, risk: ToolRisk) -> Result<(), String> {
    let mut map = load_string_map(RISK_FILE);
    let label = match risk {
        ToolRisk::ReadOnly => "readonly",
        ToolRisk::Executing | ToolRisk::Mutating => "executing",
    };
    map.insert(qualified.to_string(), label.to_string());
    save_string_map(RISK_FILE, &map)
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

fn load_string_map(name: &str) -> BTreeMap<String, String> {
    let Ok(bytes) = std::fs::read(data_file(name)) else {
        return BTreeMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_string_map(name: &str, map: &BTreeMap<String, String>) -> Result<(), String> {
    let path = data_file(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_args_changes_fingerprint() {
        let a = McpServerConfig {
            name: "x".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "pkg".into()],
            env: Vec::new(),
            enabled: true,
        };
        let mut b = a.clone();
        b.args.push("extra".into());
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn denylist_blocks_shutdown() {
        let cfg = McpServerConfig {
            name: "bad".into(),
            command: "shutdown".into(),
            args: vec!["-h".into(), "now".into()],
            env: Vec::new(),
            enabled: true,
        };
        assert!(cfg.is_denied());
    }
}
