//! Non-Linux stub with the same surface as the Landlock backend.

use super::{SandboxProfile, SandboxStatus};
use std::path::Path;

pub fn probe() -> SandboxStatus {
    SandboxStatus::Unsupported
}

pub fn apply_to_tokio(
    _cmd: &mut tokio::process::Command,
    _profile: SandboxProfile,
    _project_root: &Path,
) {
}
