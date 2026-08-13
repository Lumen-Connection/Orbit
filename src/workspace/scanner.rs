//! Incremental project walk that respects `.gitignore` plus a hard denylist.
#![allow(dead_code)]

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

const BATCH_SIZE: usize = 256;
const DENY_NAMES: &[&str] = &["target", "node_modules", ".git", "dist", "build"];

#[derive(Debug, Clone)]
pub struct FileNode {
    pub name: String,
    pub relative: PathBuf,
    pub is_dir: bool,
    pub children: Vec<FileNode>,
    pub expanded: bool,
}

impl FileNode {
    fn new(name: String, relative: PathBuf, is_dir: bool) -> Self {
        Self {
            name,
            relative,
            is_dir,
            children: Vec::new(),
            expanded: false,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct FileTree {
    pub children: Vec<FileNode>,
}

#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub relative: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    Batch(Vec<ScanEntry>),
    Done,
    Failed(String),
}

pub fn scan_project(root: PathBuf, tx: Sender<ScanEvent>, cancel: CancellationToken) {
    let walk = WalkBuilder::new(&root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .filter_entry(|entry| {
            let name = entry.file_name();
            !DENY_NAMES.iter().any(|denied| name == *denied)
        })
        .build();

    let mut batch = Vec::with_capacity(BATCH_SIZE);
    for result in walk {
        if cancel.is_cancelled() {
            let _ = tx.send(ScanEvent::Done);
            return;
        }
        let entry = match result {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!("scan skipped an entry: {e}");
                continue;
            }
        };
        let path = entry.path();
        if path == root {
            continue;
        }
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        batch.push(ScanEntry {
            relative: relative.to_path_buf(),
            is_dir: entry.file_type().is_some_and(|t| t.is_dir()),
        });
        if batch.len() >= BATCH_SIZE
            && tx
                .send(ScanEvent::Batch(std::mem::take(&mut batch)))
                .is_err()
        {
            return;
        }
    }
    if !batch.is_empty() && tx.send(ScanEvent::Batch(batch)).is_err() {
        return;
    }
    let _ = tx.send(ScanEvent::Done);
}

impl FileTree {
    pub fn insert(&mut self, relative: &Path, is_dir: bool) {
        let mut comps: Vec<&std::ffi::OsStr> = relative.iter().collect();
        if comps.is_empty() {
            return;
        }
        let leaf = comps.pop().unwrap();
        let mut siblings = &mut self.children;
        let mut prefix = PathBuf::new();
        for comp in comps {
            prefix.push(comp);
            let name = comp.to_string_lossy().into_owned();
            let pos = siblings.iter().position(|n| n.name == name);
            let idx = if let Some(idx) = pos {
                idx
            } else {
                siblings.push(FileNode::new(name, prefix.clone(), true));
                siblings.len() - 1
            };
            siblings = &mut siblings[idx].children;
        }
        let name = leaf.to_string_lossy().into_owned();
        if siblings.iter().any(|n| n.name == name) {
            return;
        }
        siblings.push(FileNode::new(name, relative.to_path_buf(), is_dir));
        siblings.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a
                .name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase()),
        });
    }

    pub fn len(&self) -> usize {
        fn count(nodes: &[FileNode]) -> usize {
            nodes.iter().map(|n| 1 + count(&n.children)).sum()
        }
        count(&self.children)
    }
}
