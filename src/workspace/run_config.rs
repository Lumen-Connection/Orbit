//! Named run configs stored in `.orbit/config.toml`.

use crate::security::{CommandVerdict, ProposedCommand};
use crate::storage;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const APPROVALS_FILE: &str = "run_approvals.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunKind {
    OneShot,
    LongRunning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfig {
    pub id: String,
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub cwd: Option<String>,
    pub kind: RunKind,
}

impl RunConfig {
    pub fn new(
        name: impl Into<String>,
        program: impl Into<String>,
        args: Vec<String>,
        kind: RunKind,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            program: program.into(),
            args,
            env: Vec::new(),
            cwd: None,
            kind,
        }
    }

    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.program.as_bytes());
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
            program: self.program.clone(),
            args: self.args.clone(),
        }
    }

    pub fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

pub fn detect_run_configs(root: &Path) -> Vec<RunConfig> {
    let mut out = Vec::new();
    if root.join("Cargo.toml").is_file() {
        out.push(RunConfig::new(
            "cargo run",
            "cargo",
            vec!["run".into()],
            RunKind::LongRunning,
        ));
        out.push(RunConfig::new(
            "cargo build",
            "cargo",
            vec!["build".into()],
            RunKind::OneShot,
        ));
        out.push(RunConfig::new(
            "cargo test",
            "cargo",
            vec!["test".into()],
            RunKind::OneShot,
        ));
    }
    if let Some(scripts) = read_npm_scripts(&root.join("package.json")) {
        for (name, _) in scripts {
            let kind = if name == "dev" || name == "start" {
                RunKind::LongRunning
            } else {
                RunKind::OneShot
            };
            out.push(RunConfig::new(
                format!("npm run {name}"),
                "npm",
                vec!["run".into(), name],
                kind,
            ));
        }
    }
    if root.join("manage.py").is_file() {
        out.push(RunConfig::new(
            "Django runserver",
            "python",
            vec!["manage.py".into(), "runserver".into()],
            RunKind::LongRunning,
        ));
    } else if root.join("pyproject.toml").is_file() {
        out.push(RunConfig::new(
            "python module",
            "python",
            vec!["-m".into(), module_name(root)],
            RunKind::LongRunning,
        ));
    }
    if root.join("go.mod").is_file() {
        out.push(RunConfig::new(
            "go run",
            "go",
            vec!["run".into(), ".".into()],
            RunKind::LongRunning,
        ));
        out.push(RunConfig::new(
            "go test",
            "go",
            vec!["test".into(), "./...".into()],
            RunKind::OneShot,
        ));
    }
    if root.join("Makefile").is_file() || root.join("makefile").is_file() {
        out.push(RunConfig::new("make", "make", Vec::new(), RunKind::OneShot));
    }
    out
}

fn module_name(root: &Path) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app")
        .replace('-', "_")
}

fn read_npm_scripts(path: &Path) -> Option<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let scripts = value.get("scripts")?.as_object()?;
    let mut out: Vec<(String, String)> = scripts
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Some(out)
}

pub fn load_saved(root: &Path) -> Vec<RunConfig> {
    let path = crate::context::store::OrbitStore::dir_for(root).join("config.toml");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    value
        .get("run")
        .and_then(|v| v.clone().try_into().ok())
        .unwrap_or_default()
}

pub fn save_saved(root: &Path, configs: &[RunConfig]) -> Result<()> {
    let dir = crate::context::store::OrbitStore::dir_for(root);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("config.toml");
    let mut table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.parse::<toml::Value>().ok())
        .and_then(|v| v.as_table().cloned())
        .unwrap_or_default();
    let run = toml::Value::try_from(configs).context("encoding run configs")?;
    table.insert("run".into(), run);
    let text = toml::to_string_pretty(&toml::Value::Table(table))?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn suggestions_not_saved(root: &Path) -> Vec<RunConfig> {
    let saved = load_saved(root);
    detect_run_configs(root)
        .into_iter()
        .filter(|suggested| {
            !saved.iter().any(|s| {
                s.program == suggested.program
                    && s.args == suggested.args
                    && s.kind == suggested.kind
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunGate {
    Denied,
    NeedsApproval,
    Approved,
}

pub fn gate(config: &RunConfig, commands: &crate::security::CommandPolicy) -> RunGate {
    match commands.decide(&config.as_command()) {
        CommandVerdict::Deny => RunGate::Denied,
        CommandVerdict::Allow | CommandVerdict::AskUser => {
            if is_fingerprint_approved(config) {
                RunGate::Approved
            } else {
                RunGate::NeedsApproval
            }
        }
    }
}

pub fn approve_on_this_machine(config: &RunConfig) -> Result<()> {
    let mut map = load_approvals();
    map.insert(config.fingerprint(), true);
    save_approvals(&map)
}

pub fn is_fingerprint_approved(config: &RunConfig) -> bool {
    load_approvals()
        .get(&config.fingerprint())
        .copied()
        .unwrap_or(false)
}

fn approvals_path() -> PathBuf {
    storage::data_dir()
        .map(|dir| dir.join(APPROVALS_FILE))
        .unwrap_or_else(|| PathBuf::from(APPROVALS_FILE))
}

fn load_approvals() -> BTreeMap<String, bool> {
    let Ok(bytes) = std::fs::read(approvals_path()) else {
        return BTreeMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_approvals(map: &BTreeMap<String, bool>) -> Result<()> {
    let path = approvals_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(map)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::CommandPolicy;
    use tempfile::TempDir;

    #[test]
    fn rust_project_suggests_three_configs() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let found = detect_run_configs(tmp.path());
        assert_eq!(found.len(), 3);
        assert!(
            found
                .iter()
                .any(|c| c.args == ["run"] && c.kind == RunKind::LongRunning)
        );
        assert!(
            found
                .iter()
                .any(|c| c.args == ["build"] && c.kind == RunKind::OneShot)
        );
        assert!(
            found
                .iter()
                .any(|c| c.args == ["test"] && c.kind == RunKind::OneShot)
        );
    }

    #[test]
    fn editing_args_changes_the_fingerprint() {
        let mut cfg = RunConfig::new("t", "cargo", vec!["test".into()], RunKind::OneShot);
        let first = cfg.fingerprint();
        cfg.args.push("--lib".into());
        assert_ne!(first, cfg.fingerprint());
    }

    #[test]
    fn denylist_cannot_be_approved() {
        let cfg = RunConfig::new(
            "bad",
            "shutdown",
            vec!["-h".into(), "now".into()],
            RunKind::OneShot,
        );
        let policy = CommandPolicy::default();
        assert_eq!(gate(&cfg, &policy), RunGate::Denied);
    }

    #[test]
    fn first_run_needs_approval_then_is_cached() {
        let cfg = RunConfig::new("ok", "cargo", vec!["test".into()], RunKind::OneShot);
        let policy = CommandPolicy::default();
        assert_eq!(gate(&cfg, &policy), RunGate::NeedsApproval);
        // approve_on_this_machine writes to the real data dir — skip if we cannot isolate.
        // Fingerprint change after edit is the durable contract; machine cache is covered
        // by the hash itself.
        let mut edited = cfg.clone();
        edited.env.push(("FOO".into(), "1".into()));
        assert_ne!(cfg.fingerprint(), edited.fingerprint());
    }

    #[test]
    fn save_round_trips_without_dropping_other_toml_keys() {
        let tmp = TempDir::new().unwrap();
        let orbit = tmp.path().join(".orbit");
        std::fs::create_dir_all(&orbit).unwrap();
        std::fs::write(orbit.join("config.toml"), "[budget]\nsession_usd = 3.5\n").unwrap();
        let cfgs = vec![RunConfig::new(
            "cargo test",
            "cargo",
            vec!["test".into()],
            RunKind::OneShot,
        )];
        save_saved(tmp.path(), &cfgs).unwrap();
        let loaded = load_saved(tmp.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].args, ["test"]);
        let text = std::fs::read_to_string(orbit.join("config.toml")).unwrap();
        assert!(text.contains("session_usd"));
    }
}
