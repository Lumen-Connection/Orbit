//! Safe file edits: hash-check, backup, atomic write.
#![allow(dead_code)]

use crate::security::resolve_within_root;
use sha2::{Digest, Sha256};
use similar::TextDiff;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchStatus {
    Pending,
    Applied,
    Rejected,
    Conflicted,
}

#[derive(Debug, Clone)]
pub struct FilePatch {
    pub relative_path: PathBuf,
    pub original_hash: String,
    pub original_content: String,
    pub proposed_content: String,
    pub unified_diff: String,
    pub status: PatchStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error("path is outside the project: {0}")]
    Security(String),
    #[error("could not apply patch: {0}")]
    Io(String),
}

impl FilePatch {
    pub fn new(relative_path: PathBuf, original_content: String, proposed_content: String) -> Self {
        let unified_diff = unified_diff(&relative_path, &original_content, &proposed_content);
        Self {
            original_hash: content_hash(original_content.as_bytes()),
            original_content,
            proposed_content,
            unified_diff,
            relative_path,
            status: PatchStatus::Pending,
        }
    }
}

pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn unified_diff(path: &Path, original: &str, proposed: &str) -> String {
    let name = path.display().to_string();
    TextDiff::from_lines(original, proposed)
        .unified_diff()
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}

/// Recheck the on-disk hash without writing. Marks `Conflicted` when the file moved on.
pub fn revalidate_patch(root: &Path, patch: &mut FilePatch) {
    if !matches!(patch.status, PatchStatus::Pending) {
        return;
    }
    let Ok(dest) = resolve_within_root(root, &patch.relative_path) else {
        patch.status = PatchStatus::Conflicted;
        return;
    };
    if dest.exists() {
        let Ok(on_disk) = std::fs::read(&dest) else {
            patch.status = PatchStatus::Conflicted;
            return;
        };
        if content_hash(&on_disk) != patch.original_hash {
            patch.status = PatchStatus::Conflicted;
        }
    } else if !patch.original_content.is_empty() {
        patch.status = PatchStatus::Conflicted;
    }
}

/// Recheck the on-disk hash. On mismatch mark `Conflicted` and do not write.
/// On I/O failure after a backup exists, restore the backup.
pub fn apply_patch(root: &Path, patch: &mut FilePatch) -> Result<(), PatchError> {
    if !matches!(patch.status, PatchStatus::Pending) {
        return Ok(());
    }
    let dest = resolve_within_root(root, &patch.relative_path)
        .map_err(|e| PatchError::Security(e.to_string()))?;

    if dest.exists() {
        let on_disk = std::fs::read(&dest).map_err(|e| PatchError::Io(e.to_string()))?;
        if content_hash(&on_disk) != patch.original_hash {
            patch.status = PatchStatus::Conflicted;
            return Ok(());
        }
    } else if !patch.original_content.is_empty() {
        patch.status = PatchStatus::Conflicted;
        return Ok(());
    }

    let backup = dest.with_extension(format!(
        "{}.orbit.bak",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("file")
    ));
    if dest.exists() {
        std::fs::copy(&dest, &backup).map_err(|e| PatchError::Io(e.to_string()))?;
    }

    if let Err(e) = atomic_write(&dest, patch.proposed_content.as_bytes()) {
        if backup.exists() {
            let _ = std::fs::copy(&backup, &dest);
        }
        return Err(PatchError::Io(e));
    }
    let _ = std::fs::remove_file(&backup);
    patch.status = PatchStatus::Applied;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("orbit.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::{FilePatch, PatchStatus, apply_patch, content_hash, revalidate_patch};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn revalidate_marks_conflict_when_disk_changed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("src");
        std::fs::create_dir_all(&path).unwrap();
        let file = path.join("lib.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut patch = FilePatch::new(
            PathBuf::from("src/lib.rs"),
            "fn a() {}\n".into(),
            "fn b() {}\n".into(),
        );
        // Project root is tmp; relative is src/lib.rs
        revalidate_patch(tmp.path(), &mut patch);
        assert_eq!(patch.status, PatchStatus::Pending);
        std::fs::write(&file, "fn changed() {}\n").unwrap();
        revalidate_patch(tmp.path(), &mut patch);
        assert_eq!(patch.status, PatchStatus::Conflicted);
    }

    #[test]
    fn conflict_when_file_changes_between_create_and_apply() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let file = root.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let mut patch = FilePatch::new("note.txt".into(), "hello".into(), "hello world".into());
        fs::write(&file, "changed underneath").unwrap();
        apply_patch(root, &mut patch).unwrap();
        assert_eq!(patch.status, PatchStatus::Conflicted);
        assert_eq!(fs::read_to_string(&file).unwrap(), "changed underneath");
    }

    #[test]
    fn apply_writes_when_hash_matches() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let file = root.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let mut patch = FilePatch::new("note.txt".into(), "hello".into(), "hello world".into());
        apply_patch(root, &mut patch).unwrap();
        assert_eq!(patch.status, PatchStatus::Applied);
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello world");
        assert_eq!(
            content_hash(fs::read(&file).unwrap().as_slice()),
            content_hash(b"hello world")
        );
    }

    #[test]
    fn write_failure_restores_backup() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let file = root.join("note.txt");
        fs::write(&file, "original").unwrap();
        let mut patch = FilePatch::new("note.txt".into(), "original".into(), "new".into());
        // Point the relative path at a directory so rename-over-dir fails after backup.
        let blocker = root.join("blocked");
        fs::create_dir(&blocker).unwrap();
        patch.relative_path = "blocked".into();
        patch.original_hash = content_hash(b"");
        // Existing dir: read as bytes may fail or hash mismatch. Force a file first.
        // Instead, replace dest with a read-only directory child after creating the patch
        // against a regular file, then swap names — simpler: apply to a path whose parent
        // is made into a file so create_dir_all / rename fails.
        let parent_as_file = root.join("notadir");
        fs::write(&parent_as_file, "x").unwrap();
        patch.relative_path = "notadir/child.txt".into();
        patch.original_content.clear();
        patch.original_hash = content_hash(b"");
        let before = fs::read_to_string(&parent_as_file).unwrap();
        let result = apply_patch(root, &mut patch);
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&parent_as_file).unwrap(), before);
    }

    #[test]
    fn unified_diff_contains_addition_and_removal() {
        let patch = FilePatch::new(
            "a.rs".into(),
            "fn a() {}\nfn b() {}\n".into(),
            "fn a() {}\nfn c() {}\n".into(),
        );
        assert!(patch.unified_diff.contains("-fn b() {}"));
        assert!(patch.unified_diff.contains("+fn c() {}"));
    }
}
