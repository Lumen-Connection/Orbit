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

/// `pre_exec` exige `io::Result`, mas toda chamada do landlock devolve
/// `RulesetError`, que não tem `From` para `io::Error`. `to_string()` evita
/// depender de `RulesetError: Send + Sync`.
fn io(e: landlock::RulesetError) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

pub fn apply_to_tokio(
    cmd: &mut tokio::process::Command,
    profile: SandboxProfile,
    project_root: &Path,
) {
    if matches!(profile, SandboxProfile::Off) {
        return;
    }
    // Tudo que aloca, lê o ambiente ou toca o disco acontece aqui, antes do
    // fork. O closure abaixo roda entre fork e exec, onde só as syscalls do
    // landlock são seguras.
    let Some(abi) = active_abi() else {
        return;
    };
    let readable = readable_paths(profile, project_root);
    let writable = writable_paths(profile, project_root);
    let strict = matches!(profile, SandboxProfile::Strict);
    unsafe {
        cmd.pre_exec(move || apply_ruleset(abi, strict, &readable, &writable));
    }
}

fn active_abi() -> Option<ABI> {
    match probe() {
        SandboxStatus::Active { abi: 4 } => Some(ABI::V4),
        SandboxStatus::Active { abi: 3 } => Some(ABI::V3),
        SandboxStatus::Active { abi: 2 } => Some(ABI::V2),
        SandboxStatus::Active { .. } => Some(ABI::V1),
        _ => None,
    }
}

fn apply_ruleset(
    abi: ABI,
    strict: bool,
    readable: &[PathBuf],
    writable: &[PathBuf],
) -> std::io::Result<()> {
    let mut builder = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))
        .map_err(io)?;
    if strict {
        builder = builder.handle_access(AccessNet::ConnectTcp).map_err(io)?;
        builder = builder.handle_access(AccessNet::BindTcp).map_err(io)?;
    }
    builder
        .create()
        .map_err(io)?
        .add_rules(path_beneath_rules(readable, AccessFs::from_read(abi)))
        .map_err(io)?
        .add_rules(path_beneath_rules(writable, AccessFs::from_all(abi)))
        .map_err(io)?
        .restrict_self()
        .map_err(io)?;
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
    async fn workspace_allows_project_and_denies_outside() {
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
        let outside = tempfile::TempDir::new().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "secret\n").unwrap();

        let mut ok = Command::new("cat");
        ok.arg(root.join("ok.txt")).current_dir(root);
        apply_to_tokio(&mut ok, SandboxProfile::Workspace, root);
        let out = ok.output().await.expect("spawn cat project");
        assert!(out.status.success(), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("project-ok"));

        let mut bad = Command::new("cat");
        bad.arg(&secret).current_dir(root);
        apply_to_tokio(&mut bad, SandboxProfile::Strict, root);
        let out = bad.output().await.expect("spawn cat outside");
        assert!(
            !out.status.success(),
            "reading outside the project must fail under strict: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
