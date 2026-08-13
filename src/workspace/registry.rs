//! Known projects and their on-disk availability.

use super::{Project, WorkspaceError};
use crate::storage::Db;
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectAvailability {
    Ready,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub first_opened_at: String,
    pub last_opened_at: String,
    pub session_count: u32,
    pub pending_patches: u32,
    pub availability: ProjectAvailability,
}

pub fn list_recent(db: &Db, limit: usize) -> Result<Vec<ProjectEntry>> {
    let mut entries = db.list_recent_projects(limit)?;
    for entry in &mut entries {
        entry.availability = if entry.path.is_dir() {
            ProjectAvailability::Ready
        } else {
            ProjectAvailability::Unavailable
        };
    }
    Ok(entries)
}

pub fn forget(db: &Db, id: &str) -> Result<()> {
    db.hide_project(id)
}

/// Re-bind a registry row to a new folder. Sessions stay attached to `id`.
pub fn rebind(db: &Db, id: &str, new_path: &Path) -> Result<Project, WorkspaceError> {
    let opened = Project::open(new_path)?;
    let rebound = Project {
        id: id.to_string(),
        name: opened.name,
        root: opened.root,
        canonical_root: opened.canonical_root,
    };
    db.rebind_project(&rebound)
        .map_err(|e| WorkspaceError::Io(e.to_string()))?;
    Ok(rebound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;
    use crate::workspace::Project;
    use tempfile::TempDir;

    #[test]
    fn missing_folder_is_unavailable_and_rebind_preserves_id() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open_at(tmp.path().join("orbit.db")).unwrap();
        let original = tmp.path().join("app");
        std::fs::create_dir_all(&original).unwrap();
        let project = Project::open(&original).unwrap();
        db.upsert_project(&project).unwrap();

        std::fs::remove_dir_all(&original).unwrap();
        let listed = list_recent(&db, 10).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].availability, ProjectAvailability::Unavailable);
        assert_eq!(listed[0].id, project.id);

        let relocated = tmp.path().join("app-moved");
        std::fs::create_dir_all(&relocated).unwrap();
        let rebound = rebind(&db, &project.id, &relocated).unwrap();
        assert_eq!(rebound.id, project.id);
        let listed = list_recent(&db, 10).unwrap();
        assert_eq!(listed[0].availability, ProjectAvailability::Ready);
        assert_eq!(listed[0].id, project.id);
        assert_eq!(listed[0].path, rebound.canonical_root);
    }

    #[test]
    fn forget_hides_without_dropping_the_row() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open_at(tmp.path().join("orbit.db")).unwrap();
        let root = tmp.path().join("p");
        std::fs::create_dir_all(&root).unwrap();
        let project = Project::open(&root).unwrap();
        db.upsert_project(&project).unwrap();
        forget(&db, &project.id).unwrap();
        assert!(list_recent(&db, 10).unwrap().is_empty());
        let still = db
            .with_conn(|conn| {
                let n: i32 = conn.query_row(
                    "SELECT COUNT(*) FROM project WHERE id = ?1",
                    rusqlite::params![project.id],
                    |r| r.get(0),
                )?;
                Ok(n)
            })
            .unwrap();
        assert_eq!(still, 1);
    }
}
