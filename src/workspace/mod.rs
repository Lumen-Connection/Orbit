//! Project workspace: open, scan, and patch files.
#![allow(dead_code)]

pub mod git;
mod patch;
pub mod registry;
pub mod run_config;
mod scanner;

#[allow(unused_imports)]
pub use patch::{FilePatch, PatchError, PatchStatus, apply_patch, content_hash, revalidate_patch};
pub use scanner::{FileNode, FileTree, ScanEntry, ScanEvent, scan_project};

use crate::storage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("could not open project: {0}")]
    Io(String),
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub canonical_root: PathBuf,
}

impl Project {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let root = path.as_ref().to_path_buf();
        let canonical_root = root
            .canonicalize()
            .map_err(|e| WorkspaceError::Io(e.to_string()))?;
        if !canonical_root.is_dir() {
            return Err(WorkspaceError::Io("path is not a directory".into()));
        }
        let name = canonical_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string();
        let id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            canonical_root.to_string_lossy().as_bytes(),
        )
        .to_string();
        Ok(Self {
            id,
            name,
            root,
            canonical_root,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub name: String,
    pub path: PathBuf,
    pub last_opened: String,
}

const RECENT_LIMIT: usize = 10;

pub fn recent_projects_path() -> PathBuf {
    storage::data_dir()
        .map(|dir| dir.join("recent_projects.json"))
        .unwrap_or_else(|| PathBuf::from("recent_projects.json"))
}

pub fn load_recent_projects() -> Vec<RecentProject> {
    let path = recent_projects_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn remember_project(project: &Project) -> Vec<RecentProject> {
    let mut recent = load_recent_projects();
    recent.retain(|item| item.path != project.canonical_root);
    recent.insert(
        0,
        RecentProject {
            name: project.name.clone(),
            path: project.canonical_root.clone(),
            last_opened: chrono::Utc::now().to_rfc3339(),
        },
    );
    recent.truncate(RECENT_LIMIT);
    if let Ok(json) = serde_json::to_vec_pretty(&recent) {
        if let Some(parent) = recent_projects_path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(recent_projects_path(), json);
    }
    recent
}
