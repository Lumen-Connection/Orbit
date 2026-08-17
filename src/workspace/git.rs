//! Shared `git` process helper. Tools and worktree isolation both use this
//! so we do not grow a second `Command::new("git")` stack.

use std::path::Path;
use std::process::Command;

/// Run `git` with `args` in `root` and return combined stdout+stderr.
pub fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("git failed to start: {e}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        return Err(text);
    }
    Ok(text)
}

pub fn is_repository(root: &Path) -> bool {
    git(root, &["rev-parse", "--git-dir"]).is_ok()
}

pub fn is_dirty(root: &Path) -> bool {
    git(root, &["status", "--porcelain"])
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{git, is_dirty, is_repository};
    use std::process::Command;
    use tempfile::TempDir;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn detects_repo_and_dirty_state() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("p");
        std::fs::create_dir_all(&root).unwrap();
        assert!(!is_repository(&root));
        assert!(
            Command::new("git")
                .args(["init"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        assert!(is_repository(&root));
        std::fs::write(root.join("a.txt"), "x\n").unwrap();
        assert!(is_dirty(&root));
        let _ = git(&root, &["config", "user.email", "orbit@test"]);
        let _ = git(&root, &["config", "user.name", "orbit"]);
        git(&root, &["add", "a.txt"]).unwrap();
        git(&root, &["commit", "-m", "init"]).unwrap();
        assert!(!is_dirty(&root));
    }
}
