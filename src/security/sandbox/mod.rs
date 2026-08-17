//! Kernel sandbox for child processes. Linux uses Landlock; other OSes
//! expose the same types so the UI can say the feature is unavailable.

#[cfg(target_os = "linux")]
mod landlock;
#[cfg(not(target_os = "linux"))]
mod unsupported;

#[cfg(target_os = "linux")]
use landlock as backend;
#[cfg(not(target_os = "linux"))]
use unsupported as backend;

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SandboxProfile {
    #[default]
    Off,
    Workspace,
    Strict,
}

impl SandboxProfile {
    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Workspace => "workspace",
            Self::Strict => "strict",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Workspace => "Workspace",
            Self::Strict => "Strict",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "workspace" => Self::Workspace,
            "strict" => Self::Strict,
            _ => Self::Off,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SandboxStatus {
    Off,
    Active { abi: u32 },
    Unavailable { reason: String },
    Unsupported,
}

impl SandboxStatus {
    pub fn display(&self) -> String {
        match self {
            Self::Off => "Off".into(),
            Self::Active { abi } => format!("Active (ABI {abi})"),
            Self::Unavailable { reason } => format!("Unavailable: {reason}"),
            Self::Unsupported => "Not supported on this platform".into(),
        }
    }
}

/// Project config may only tighten the machine default, never loosen it.
pub fn effective(machine: SandboxProfile, project: Option<SandboxProfile>) -> SandboxProfile {
    match project {
        Some(project) if project > machine => project,
        _ => machine,
    }
}

pub fn load_project_profile(root: &Path) -> Option<SandboxProfile> {
    let path = root.join(".orbit").join("config.toml");
    let text = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = text.parse().ok()?;
    value
        .get("sandbox")
        .and_then(|t| t.get("profile"))
        .and_then(|v| v.as_str())
        .map(SandboxProfile::parse)
}

pub fn probe() -> SandboxStatus {
    backend::probe()
}

pub fn apply_to_tokio(
    cmd: &mut tokio::process::Command,
    profile: SandboxProfile,
    project_root: &Path,
) {
    backend::apply_to_tokio(cmd, profile, project_root);
}

#[cfg(test)]
mod tests {
    use super::{SandboxProfile, effective, load_project_profile};

    #[test]
    fn project_config_can_only_tighten() {
        assert_eq!(
            effective(SandboxProfile::Workspace, Some(SandboxProfile::Off)),
            SandboxProfile::Workspace
        );
        assert_eq!(
            effective(SandboxProfile::Off, Some(SandboxProfile::Strict)),
            SandboxProfile::Strict
        );
        assert_eq!(
            effective(SandboxProfile::Strict, Some(SandboxProfile::Workspace)),
            SandboxProfile::Strict
        );
        assert_eq!(effective(SandboxProfile::Off, None), SandboxProfile::Off);
    }

    #[test]
    fn probe_names_the_host_os() {
        let status = super::probe();
        if cfg!(target_os = "linux") {
            assert!(!matches!(status, super::SandboxStatus::Unsupported));
        } else {
            assert_eq!(status, super::SandboxStatus::Unsupported);
        }
    }

    #[test]
    fn load_project_profile_reads_toml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join(".orbit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "[sandbox]\nprofile = \"strict\"\n").unwrap();
        assert_eq!(
            load_project_profile(tmp.path()),
            Some(SandboxProfile::Strict)
        );
    }
}
