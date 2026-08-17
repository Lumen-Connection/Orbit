//! Disposable git worktrees for writing subagents.
//!
//! The child session is rooted in the worktree, so path confinement keeps
//! working unchanged. Writes never touch the user's tree until the merge
//! patches are approved.

use super::SessionId;
use crate::workspace::git::{self, git};
use crate::workspace::{FilePatch, PatchStatus, Project, content_hash};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    None,
    Worktree,
}

impl Isolation {
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.unwrap_or("none").trim().to_ascii_lowercase().as_str() {
            "" | "none" => Ok(Self::None),
            "worktree" => Ok(Self::Worktree),
            other => Err(format!(
                "isolation must be \"none\" or \"worktree\", not `{other}`"
            )),
        }
    }
}

#[derive(Debug)]
pub struct Worktree {
    pub path: PathBuf,
    project_root: PathBuf,
    start_hashes: HashMap<PathBuf, String>,
    removed: bool,
}

impl Worktree {
    pub fn create(project: &Project, session_id: &SessionId) -> Result<Self, String> {
        let base = crate::storage::data_dir()
            .ok_or_else(|| "could not resolve the Orbit data directory".to_string())?;
        Self::create_under(&base.join("worktrees"), project, session_id)
    }

    pub fn create_under(
        base: &Path,
        project: &Project,
        session_id: &SessionId,
    ) -> Result<Self, String> {
        if !git::is_repository(&project.canonical_root) {
            return Err(
                "isolation: \"worktree\" requires a git repository (`git rev-parse --git-dir` failed)."
                    .into(),
            );
        }
        let path = base.join(&project.id).join(session_id.as_str());
        if path.exists() {
            let _ = git(
                &project.canonical_root,
                &["worktree", "remove", "--force", &path.to_string_lossy()],
            );
            let _ = std::fs::remove_dir_all(&path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let start_hashes = snapshot_hashes(&project.canonical_root);
        git(
            &project.canonical_root,
            &[
                "worktree",
                "add",
                "--detach",
                &path.to_string_lossy(),
                "HEAD",
            ],
        )?;
        Ok(Self {
            path,
            project_root: project.canonical_root.clone(),
            start_hashes,
            removed: false,
        })
    }

    pub fn changes(&self) -> Vec<PathBuf> {
        let text = match git(&self.path, &["status", "--porcelain"]) {
            Ok(text) => text,
            Err(_) => return Vec::new(),
        };
        parse_porcelain(&text)
    }

    pub fn patches_against(&self, parent_root: &Path) -> Vec<FilePatch> {
        let mut patches = Vec::new();
        for rel in self.changes() {
            let parent_path = parent_root.join(&rel);
            let child_path = self.path.join(&rel);
            let original = std::fs::read_to_string(&parent_path).unwrap_or_default();
            let proposed = std::fs::read_to_string(&child_path).unwrap_or_default();
            if original == proposed {
                continue;
            }
            let mut patch = FilePatch::new(rel.clone(), original, proposed);
            let now_hash = std::fs::read(&parent_path).ok().map(|b| content_hash(&b));
            let start = self.start_hashes.get(&normalize_rel(&rel));
            if start != now_hash.as_ref() {
                patch.status = PatchStatus::Conflicted;
            }
            patches.push(patch);
        }
        patches
    }

    fn remove_inner(&mut self) {
        if self.removed {
            return;
        }
        let path = self.path.to_string_lossy().into_owned();
        let _ = git(
            &self.project_root,
            &["worktree", "remove", "--force", &path],
        );
        let _ = git(&self.project_root, &["worktree", "prune"]);
        let _ = std::fs::remove_dir_all(&self.path);
        self.removed = true;
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        self.remove_inner();
    }
}

/// Drop leftover worktrees for this project. Called on project open and unload
/// so a crash cannot leave an attached worktree behind.
pub fn prune(project: &Project) {
    prune_under(
        &crate::storage::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("worktrees"),
        project,
    );
}

pub fn prune_under(base: &Path, project: &Project) {
    let dir = base.join(&project.id);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let rendered = path.to_string_lossy().into_owned();
            let _ = git(
                &project.canonical_root,
                &["worktree", "remove", "--force", &rendered],
            );
        }
    }
    let _ = git(&project.canonical_root, &["worktree", "prune"]);
    let _ = std::fs::remove_dir_all(&dir);
}

fn snapshot_hashes(root: &Path) -> HashMap<PathBuf, String> {
    let mut map = HashMap::new();
    let Ok(list) = git(root, &["ls-files", "-oc", "--exclude-standard"]) else {
        return map;
    };
    for line in list.lines() {
        let rel = normalize_rel(Path::new(line.trim()));
        if rel.as_os_str().is_empty() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(root.join(&rel)) {
            map.insert(rel, content_hash(&bytes));
        }
    }
    map
}

fn parse_porcelain(text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.len() < 4 {
            continue;
        }
        let rest = line[3..].trim();
        let path = if let Some((_, to)) = rest.split_once(" -> ") {
            to.trim()
        } else {
            rest.trim_matches('"')
        };
        if !path.is_empty() {
            out.push(normalize_rel(Path::new(path)));
        }
    }
    out
}

fn normalize_rel(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::{Isolation, Worktree, parse_porcelain, prune_under};
    use crate::session::SessionId;
    use crate::workspace::Project;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn repo() -> Option<(TempDir, Project)> {
        if !git_available() {
            return None;
        }
        let tmp = TempDir::new().ok()?;
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join("src")).ok()?;
        std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").ok()?;
        if !Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .status()
            .ok()?
            .success()
        {
            return None;
        }
        let _ = Command::new("git")
            .args(["config", "user.email", "orbit@test"])
            .current_dir(&root)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "orbit"])
            .current_dir(&root)
            .status();
        let _ = Command::new("git")
            .args(["add", "."])
            .current_dir(&root)
            .status();
        if !Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&root)
            .status()
            .ok()?
            .success()
        {
            return None;
        }
        let project = Project::open(&root).ok()?;
        Some((tmp, project))
    }

    #[test]
    fn isolation_parse_rejects_unknown() {
        assert_eq!(Isolation::parse(None).unwrap(), Isolation::None);
        assert_eq!(
            Isolation::parse(Some("worktree")).unwrap(),
            Isolation::Worktree
        );
        assert!(Isolation::parse(Some("jail")).is_err());
    }

    #[test]
    fn porcelain_reads_paths_and_renames() {
        let paths = parse_porcelain(" M src/lib.rs\n?? src/new.rs\nR  old.rs -> src/renamed.rs\n");
        assert_eq!(
            paths,
            vec![
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/new.rs"),
                PathBuf::from("src/renamed.rs"),
            ]
        );
    }

    #[test]
    fn create_lists_changes_and_remove_detaches() {
        let Some((tmp, project)) = repo() else {
            return;
        };
        let base = tmp.path().join("wts");
        let wt = Worktree::create_under(&base, &project, &SessionId::new("child")).unwrap();
        std::fs::write(wt.path.join("src/lib.rs"), "fn b() {}\n").unwrap();
        std::fs::write(wt.path.join("src/new.rs"), "fn n() {}\n").unwrap();
        let changes = wt.changes();
        assert!(
            changes.iter().any(|p| p == Path::new("src/lib.rs")),
            "{changes:?}"
        );
        assert!(
            changes.iter().any(|p| p == Path::new("src/new.rs")),
            "{changes:?}"
        );
        let patches = wt.patches_against(&project.canonical_root);
        assert_eq!(patches.len(), 2);
        drop(wt);
        let list =
            crate::workspace::git::git(&project.canonical_root, &["worktree", "list"]).unwrap();
        assert!(
            !list.contains("child"),
            "worktree still listed after remove: {list}"
        );
    }

    #[test]
    fn parent_edit_during_child_marks_conflict() {
        let Some((tmp, project)) = repo() else {
            return;
        };
        let base = tmp.path().join("wts");
        let wt = Worktree::create_under(&base, &project, &SessionId::new("child")).unwrap();
        std::fs::write(wt.path.join("src/lib.rs"), "fn child() {}\n").unwrap();
        std::fs::write(
            project.canonical_root.join("src/lib.rs"),
            "fn parent() {}\n",
        )
        .unwrap();
        let patches = wt.patches_against(&project.canonical_root);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].status, crate::workspace::PatchStatus::Conflicted);
        drop(wt);
    }

    #[test]
    fn refuses_a_folder_that_is_not_a_repo() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("bare");
        std::fs::create_dir_all(&root).unwrap();
        let project = Project::open(&root).unwrap();
        let err = Worktree::create_under(tmp.path(), &project, &SessionId::new("x")).unwrap_err();
        assert!(err.contains("git repository"), "{err}");
    }

    #[test]
    fn prune_removes_orphans() {
        let Some((tmp, project)) = repo() else {
            return;
        };
        let base = tmp.path().join("wts");
        let wt = Worktree::create_under(&base, &project, &SessionId::new("orphan")).unwrap();
        let path = wt.path.clone();
        std::mem::forget(wt);
        assert!(path.exists());
        prune_under(&base, &project);
        let list =
            crate::workspace::git::git(&project.canonical_root, &["worktree", "list"]).unwrap();
        assert!(!list.contains("orphan"), "{list}");
        assert!(!path.exists());
    }
}
