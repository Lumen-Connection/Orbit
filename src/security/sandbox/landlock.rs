//! Landlock ruleset applied in the child between fork and exec.

use super::{SandboxProfile, SandboxStatus};
use landlock::{
    ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, Ruleset, RulesetAttr,
    RulesetCreatedAttr, path_beneath_rules,
};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

pub fn probe() -> SandboxStatus {
    for (n, abi) in [(4_u32, ABI::V4), (3, ABI::V3), (2, ABI::V2), (1, ABI::V1)] {
        if Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(abi))
            .and_then(|r| r.create())
            .is_ok()
        {
            return SandboxStatus::Active { abi: n };
        }
    }
    SandboxStatus::Unavailable {
        reason: "kernel < 5.13".into(),
    }
}

pub fn apply_to_tokio(
    cmd: &mut tokio::process::Command,
    profile: SandboxProfile,
    project_root: &Path,
) {
    if matches!(profile, SandboxProfile::Off) {
        return;
    }
    let root = project_root.to_path_buf();
    unsafe {
        cmd.pre_exec(move || apply_ruleset(profile, &root));
    }
}

fn apply_ruleset(profile: SandboxProfile, project_root: &Path) -> std::io::Result<()> {
    let abi = match probe() {
        SandboxStatus::Active { abi: 4 } => ABI::V4,
        SandboxStatus::Active { abi: 3 } => ABI::V3,
        SandboxStatus::Active { abi: 2 } => ABI::V2,
        SandboxStatus::Active { .. } => ABI::V1,
        _ => return Ok(()),
    };
    let writable = writable_paths(profile, project_root);
    match profile {
        SandboxProfile::Off => Ok(()),
        SandboxProfile::Workspace | SandboxProfile::Strict => {
            let readable = readable_paths(profile, project_root);
            let mut builder = Ruleset::default()
                .set_compatibility(CompatLevel::BestEffort)
                .handle_access(AccessFs::from_all(abi))?;
            if matches!(profile, SandboxProfile::Strict) {
                builder = match builder.handle_access(AccessNet::ConnectTcp) {
                    Ok(b) => b,
                    Err(_) => return finish_fs(builder, &readable, &writable, abi),
                };
                builder = match builder.handle_access(AccessNet::BindTcp) {
                    Ok(b) => b,
                    Err(_) => return finish_fs(builder, &readable, &writable, abi),
                };
            }
            finish_fs(builder, &readable, &writable, abi)
        }
    }
}

fn finish_fs(
    builder: Ruleset,
    readable: &[PathBuf],
    writable: &[PathBuf],
    abi: ABI,
) -> std::io::Result<()> {
    builder
        .create()?
        .add_rules(path_beneath_rules(readable, AccessFs::from_read(abi)))?
        .add_rules(path_beneath_rules(writable, AccessFs::from_all(abi)))?
        .restrict_self()?;
    Ok(())
}

fn writable_paths(profile: SandboxProfile, project_root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![project_root.to_path_buf(), PathBuf::from("/tmp")];
    if let Ok(tmp) = std::env::var("TMPDIR") {
        paths.push(PathBuf::from(tmp));
    }
    if matches!(profile, SandboxProfile::Workspace) {
        if let Ok(cargo) = std::env::var("CARGO_HOME") {
            paths.push(PathBuf::from(cargo));
        } else if let Some(home) = dirs_home() {
            paths.push(home.join(".cargo"));
        }
        if let Some(home) = dirs_home() {
            paths.push(home.join(".cache"));
        }
    }
    paths.retain(|p| p.exists());
    paths
}

fn readable_paths(profile: SandboxProfile, project_root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![project_root.to_path_buf()];
    for sys in [
        "/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/proc", "/dev",
    ] {
        paths.push(PathBuf::from(sys));
    }
    if matches!(profile, SandboxProfile::Workspace)
        && let Some(home) = dirs_home()
    {
        paths.push(home.join(".rustup"));
        paths.push(home.join(".cargo"));
    }
    paths.retain(|p| p.exists());
    paths
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::apply_to_tokio;
    use crate::security::sandbox::{SandboxProfile, SandboxStatus};
    use tokio::process::Command;

    #[tokio::test]
    async fn workspace_allows_project_and_denies_ssh() {
        match crate::security::sandbox::probe() {
            SandboxStatus::Active { .. } => {}
            other => {
                eprintln!("skipping landlock test: {}", other.display());
                return;
            }
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("ok.txt"), "project-ok\n").unwrap();
        let ssh = dirs_home()
            .map(|h| h.join(".ssh/id_rsa"))
            .unwrap_or_else(|| PathBuf::from("/root/.ssh/id_rsa"));

        let mut ok = Command::new("cat");
        ok.arg(root.join("ok.txt")).current_dir(root);
        apply_to_tokio(&mut ok, SandboxProfile::Workspace, root);
        let out = ok.output().await.expect("spawn cat project");
        assert!(out.status.success(), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("project-ok"));

        if !ssh.exists() {
            eprintln!("no ~/.ssh/id_rsa; skipped the deny assertion");
            return;
        }
        let mut bad = Command::new("cat");
        bad.arg(&ssh).current_dir(root);
        apply_to_tokio(&mut bad, SandboxProfile::Workspace, root);
        let out = bad.output().await.expect("spawn cat ssh");
        assert!(
            !out.status.success(),
            "reading ~/.ssh must fail under workspace: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn dirs_home() -> Option<std::path::PathBuf> {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}
