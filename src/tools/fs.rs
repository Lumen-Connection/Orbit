//! Filesystem tools. Read-only tools run immediately; writes become patches.

use super::{Tool, ToolContext, ToolError, ToolOutcome, ToolRisk, truncate_output};
use crate::security::{is_sensitive, resolve_within_root};
use crate::workspace::{FilePatch, Project};
use async_trait::async_trait;
use globset::Glob as GlobPattern;
use ignore::WalkBuilder;
use regex::Regex;
use std::path::{Path, PathBuf};

const GREP_LIMIT: usize = 50;
const GLOB_LIMIT: usize = 200;
const ANCHOR_WINDOW: usize = 20;
const DENY_NAMES: &[&str] = &["target", "node_modules", ".git", "dist", "build"];
const WALK_CACHE_TTL: std::time::Duration = std::time::Duration::from_millis(250);

/// Short-lived cache of the walked file list, keyed by canonical project root.
/// A human approval round-trip is far slower than 250 ms, so a newly-written
/// file is always visible to the next grep/glob while consecutive same-turn
/// calls reuse a single traversal (N2.3).
static WALK_CACHE: std::sync::Mutex<Option<(PathBuf, std::time::Instant, Vec<PathBuf>)>> =
    std::sync::Mutex::new(None);

fn cached_walk(root: &Path) -> Vec<PathBuf> {
    let now = std::time::Instant::now();
    if let Ok(mut guard) = WALK_CACHE.lock() {
        if let Some((cached_root, at, files)) = guard.as_ref()
            && cached_root == root
            && at.elapsed() < WALK_CACHE_TTL
        {
            return files.clone();
        }
        let mut files = Vec::new();
        let walk = WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .filter_entry(|e| !DENY_NAMES.iter().any(|d| e.file_name() == *d))
            .build();
        for entry in walk.flatten() {
            if entry.file_type().is_some_and(|t| t.is_file()) {
                files.push(entry.into_path());
            }
        }
        *guard = Some((root.to_path_buf(), now, files.clone()));
        return files;
    }
    // Lock contention fallback: walk without caching.
    let mut files = Vec::new();
    let walk = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .filter_entry(|e| !DENY_NAMES.iter().any(|d| e.file_name() == *d))
        .build();
    for entry in walk.flatten() {
        if entry.file_type().is_some_and(|t| t.is_file()) {
            files.push(entry.into_path());
        }
    }
    files
}

fn project(ctx: &ToolContext) -> Result<&Project, ToolError> {
    ctx.project
        .as_deref()
        .ok_or_else(|| ToolError::Message("no project is open".into()))
}

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArgs(format!("missing `{key}`")))
}

fn resolve(ctx: &ToolContext, relative: &str) -> Result<PathBuf, ToolError> {
    let project = project(ctx)?;
    let rel = Path::new(relative);
    if is_sensitive(rel) && !ctx.allow_sensitive {
        return Err(ToolError::NeedsConfirmation(relative.into()));
    }
    resolve_within_root(&project.canonical_root, rel).map_err(|e| ToolError::Message(e.to_string()))
}

fn relative_of(project: &Project, path: &Path) -> String {
    path.strip_prefix(&project.canonical_root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

pub struct ReadFile;
pub struct ListDir;
pub struct Grep;
pub struct Glob;
pub struct WriteFile;
pub struct EditFile;
pub struct MultiEdit;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn description(&self) -> &'static str {
        "Read a UTF-8 file. Optional 1-based `offset` and `limit` select lines."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "offset": { "type": "integer", "minimum": 1 },
                "limit": { "type": "integer", "minimum": 1 }
            },
            "required": ["path"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::Message("cancelled".into()));
        }
        let path = resolve(ctx, arg_str(&args, "path")?)?;
        let bytes = std::fs::read(&path).map_err(|e| ToolError::Message(e.to_string()))?;
        let probe = &bytes[..bytes.len().min(8192)];
        if probe.contains(&0) {
            return Ok(truncate_output(format!(
                "binary file, {} bytes ({} is a binary file, not UTF-8 text)",
                bytes.len(),
                relative_of(project(ctx)?, &path)
            )));
        }
        let text =
            String::from_utf8(bytes).map_err(|_| ToolError::Message("binary file".into()))?;
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let lines: Vec<&str> = text.lines().collect();
        let start = offset.saturating_sub(1).min(lines.len());
        let end = match limit {
            Some(n) => (start + n).min(lines.len()),
            None => lines.len(),
        };
        let slice = lines[start..end].join("\n");
        Ok(truncate_output(format!(
            "# {} (lines {}–{} of {})\n{slice}",
            relative_of(project(ctx)?, &path),
            start + 1,
            end,
            lines.len()
        )))
    }
}

#[async_trait]
impl Tool for ListDir {
    fn name(&self) -> &'static str {
        "list_dir"
    }
    fn description(&self) -> &'static str {
        "List entries in a directory relative to the project root."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let path = resolve(ctx, arg_str(&args, "path")?)?;
        if !path.is_dir() {
            return Err(ToolError::Message(format!(
                "{} is not a directory",
                path.display()
            )));
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(|e| ToolError::Message(e.to_string()))? {
            let entry = entry.map_err(|e| ToolError::Message(e.to_string()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let suffix = if entry.path().is_dir() { "/" } else { "" };
            names.push(format!("{name}{suffix}"));
        }
        names.sort();
        Ok(truncate_output(names.join("\n")))
    }
}

#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &'static str {
        "grep"
    }
    fn description(&self) -> &'static str {
        "Search the project with a regular expression. Optional `path` limits the walk."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string" },
                "context": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Number of surrounding lines to show before and after each match (like grep -C). Default 0."
                }
            },
            "required": ["pattern"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let project = project(ctx)?;
        let regex = Regex::new(arg_str(&args, "pattern")?)
            .map_err(|e| ToolError::InvalidArgs(e.to_string()))?;
        let start = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = resolve(ctx, start)?;
        let mut hits = Vec::new();
        for path in cached_walk(&root) {
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::Message("cancelled".into()));
            }
            let rel = PathBuf::from(relative_of(project, &path));
            if is_sensitive(&rel) && !ctx.allow_sensitive {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let context = args.get("context").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let lines: Vec<&str> = text.lines().collect();
            for (idx, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    if context > 0 {
                        let start = idx.saturating_sub(context);
                        let end = (idx + 1 + context).min(lines.len());
                        for (c_idx, c_line) in lines[start..end].iter().enumerate() {
                            let real = start + c_idx;
                            let marker = if real == idx { ":" } else { "-" };
                            hits.push(format!("{}:{}{marker}{c_line}", rel.display(), real + 1));
                        }
                        hits.push("".into());
                    } else {
                        hits.push(format!("{}:{}:{line}", rel.display(), idx + 1));
                    }
                    if hits.len() >= GREP_LIMIT {
                        hits.push(format!("… truncated at {GREP_LIMIT} matches"));
                        return Ok(truncate_output(hits.join("\n")));
                    }
                }
            }
        }
        if hits.is_empty() {
            Ok(truncate_output("No matches.".into()))
        } else {
            Ok(truncate_output(hits.join("\n")))
        }
    }
}

#[async_trait]
impl Tool for Glob {
    fn name(&self) -> &'static str {
        "glob"
    }
    fn description(&self) -> &'static str {
        "Find files by glob pattern relative to the project root (e.g. `src/**/*.rs`)."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "pattern": { "type": "string" } },
            "required": ["pattern"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let project = project(ctx)?;
        let matcher = GlobPattern::new(arg_str(&args, "pattern")?)
            .map_err(|e| ToolError::InvalidArgs(e.to_string()))?
            .compile_matcher();
        let mut hits = Vec::new();
        for path in cached_walk(&project.canonical_root) {
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::Message("cancelled".into()));
            }
            let path = &path;
            if path == &project.canonical_root {
                continue;
            }
            let rel = relative_of(project, path);
            if matcher.is_match(&rel) {
                if is_sensitive(Path::new(&rel)) && !ctx.allow_sensitive {
                    continue;
                }
                hits.push(rel);
                if hits.len() >= GLOB_LIMIT {
                    hits.push(format!("… truncated at {GLOB_LIMIT} paths"));
                    break;
                }
            }
        }
        Ok(truncate_output(hits.join("\n")))
    }
}

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn description(&self) -> &'static str {
        "Propose creating or replacing a file. The write stays pending until approved."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::Mutating
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let rel = arg_str(&args, "path")?;
        let content = arg_str(&args, "content")?.to_string();
        let dest = resolve(ctx, rel)?;
        let original = if dest.exists() {
            std::fs::read_to_string(&dest).map_err(|e| ToolError::Message(e.to_string()))?
        } else {
            String::new()
        };
        let patch = FilePatch::new(PathBuf::from(rel.replace('\\', "/")), original, content);
        let summary = format!(
            "Proposed write to {rel} ({} bytes). Waiting for approval.",
            patch.proposed_content.len()
        );
        ctx.proposed_patches
            .lock()
            .map_err(|_| ToolError::Message("patch lock poisoned".into()))?
            .push(patch);
        Ok(truncate_output(summary))
    }
}

#[async_trait]
impl Tool for EditFile {
    fn name(&self) -> &'static str {
        "edit_file"
    }
    fn description(&self) -> &'static str {
        "Propose replacing an exact unique substring. Fails if the snippet is missing or ambiguous."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" },
                "anchor_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based line that narrows the search to a window around it. Uniqueness is only required within that window."
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::Mutating
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let rel = arg_str(&args, "path")?;
        let old = arg_str(&args, "old_string")?;
        let new = arg_str(&args, "new_string")?;
        if old.is_empty() {
            return Err(ToolError::InvalidArgs("`old_string` is empty".into()));
        }
        let dest = resolve(ctx, rel)?;
        let original =
            std::fs::read_to_string(&dest).map_err(|e| ToolError::Message(e.to_string()))?;

        // Optional 1-based anchor line narrows the search to a byte window around
        // it (like grep -C): we slice the file between (anchor-window) and
        // (anchor+window) line boundaries and require uniqueness only in that
        // slice. Default absent = whole file (unchanged behavior).
        let anchor = args
            .get("anchor_line")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let (win_start, win_end) = if anchor > 0 {
            let lines: Vec<&str> = original.lines().collect();
            let a = anchor.saturating_sub(1);
            let lo = a.saturating_sub(ANCHOR_WINDOW).min(lines.len());
            let hi = (a + 1 + ANCHOR_WINDOW).min(lines.len());
            let start_byte = lines[..lo].iter().map(|l| l.len() + 1).sum::<usize>();
            let end_byte = if hi == 0 {
                0
            } else {
                lines[..hi].iter().map(|l| l.len() + 1).sum::<usize>()
            };
            (start_byte, end_byte)
        } else {
            (0, original.len())
        };
        let window = &original[win_start..win_end];
        let matches = window.matches(old).count();
        if matches == 0 {
            return Err(ToolError::Message(format!("snippet not found in {rel}")));
        }
        if matches > 1 {
            return Err(ToolError::Message(format!(
                "snippet is not unique near line {anchor} in {rel} ({matches} occurrences); \
                 refuse to guess"
            )));
        }
        let proposed = if anchor > 0 && matches == 1 {
            // Replace the single occurrence inside the window, leaving the rest of
            // the file untouched.
            let window_new = window.replacen(old, new, 1);
            format!(
                "{}{}{}",
                &original[..win_start],
                window_new,
                &original[win_end..]
            )
        } else {
            original.replacen(old, new, 1)
        };
        let patch = FilePatch::new(PathBuf::from(rel.replace('\\', "/")), original, proposed);
        ctx.proposed_patches
            .lock()
            .map_err(|_| ToolError::Message("patch lock poisoned".into()))?
            .push(patch);
        Ok(truncate_output(format!(
            "Proposed unique edit in {rel}. Waiting for approval."
        )))
    }
}

#[async_trait]
impl Tool for MultiEdit {
    fn name(&self) -> &'static str {
        "multi_edit"
    }
    fn description(&self) -> &'static str {
        "Apply several unique substring replacements in one file in a single call. \
         Each entry is {old_string, new_string}. All replacements are applied to the \
         original content; the operation fails entirely (atomically) if any snippet is \
         missing or ambiguous, producing one patch and one approval."
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": { "type": "string" },
                            "new_string": { "type": "string" }
                        },
                        "required": ["old_string", "new_string"]
                    }
                }
            },
            "required": ["path", "edits"]
        })
    }
    fn risk(&self) -> ToolRisk {
        ToolRisk::Mutating
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let rel = arg_str(&args, "path")?;
        let edits = args
            .get("edits")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                ToolError::InvalidArgs(
                    "`edits` must be an array of {old_string, new_string}".into(),
                )
            })?;
        if edits.is_empty() {
            return Err(ToolError::InvalidArgs("`edits` is empty".into()));
        }
        for e in edits {
            if e.get("old_string").and_then(|v| v.as_str()).is_none() {
                return Err(ToolError::InvalidArgs(
                    "each edit needs a non-empty `old_string`".into(),
                ));
            }
        }
        let dest = resolve(ctx, rel)?;
        let original =
            std::fs::read_to_string(&dest).map_err(|e| ToolError::Message(e.to_string()))?;
        let mut working = original.clone();
        for e in edits {
            let old = e.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
            let new = e.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
            let count = working.matches(old).count();
            if count == 0 {
                return Err(ToolError::Message(format!(
                    "snippet not found in {rel}: {old:?}"
                )));
            }
            if count > 1 {
                return Err(ToolError::Message(format!(
                    "snippet is not unique in {rel} ({count} occurrences): {old:?}"
                )));
            }
            working = working.replacen(old, new, 1);
        }
        let patch = FilePatch::new(PathBuf::from(rel.replace('\\', "/")), original, working);
        ctx.proposed_patches
            .lock()
            .map_err(|_| ToolError::Message("patch lock poisoned".into()))?
            .push(patch);
        Ok(truncate_output(format!(
            "Proposed {} unique edits in {rel}. Waiting for approval.",
            edits.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;
    use crate::workspace::Project;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn fixture() -> (TempDir, ToolContext) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "fn authenticate() {}\nfn other() {}\n",
        )
        .unwrap();
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        let project = Arc::new(Project::open(&root).unwrap());
        let ctx = ToolContext {
            session: SessionId::new("t"),
            cancel: CancellationToken::new(),
            project: Some(project),
            allow_sensitive: false,
            proposed_patches: Arc::new(Mutex::new(Vec::new())),
            allow_execute: false,
            command_timeout: crate::tools::shell::COMMAND_TIMEOUT,
            terminal: None,
            store: None,
            session_label: String::new(),
            session_model: String::new(),
            session_role: crate::session::AgentRole::Coder,
            runner: None,
            run_configs: None,
            run_starts: None,
            db: None,
            subagents: None,
            budget_usd: None,
        };
        (tmp, ctx)
    }

    #[tokio::test]
    async fn read_file_supports_offset_limit() {
        let (_tmp, ctx) = fixture();
        let out = ReadFile
            .execute(
                serde_json::json!({"path": "src/lib.rs", "offset": 2, "limit": 1}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("fn other()"));
        assert!(!out.content.contains("fn authenticate()"));
    }

    #[tokio::test]
    async fn read_file_detects_binary() {
        let (_tmp, ctx) = fixture();
        let root = ctx.project.as_ref().unwrap().canonical_root.clone();
        std::fs::write(root.join("img.bin"), b"\x89PNG\r\n\x1a\n\x00\x00\x00").unwrap();
        let out = ReadFile
            .execute(serde_json::json!({"path": "img.bin"}), &ctx)
            .await
            .unwrap();
        assert!(
            out.content.contains("binary file"),
            "expected binary message, got: {}",
            out.content
        );
        assert!(out.content.contains("bytes"));
    }

    #[tokio::test]
    async fn list_dir_lists_src() {
        let (_tmp, ctx) = fixture();
        let out = ListDir
            .execute(serde_json::json!({"path": "src"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("lib.rs"));
    }

    #[tokio::test]
    async fn grep_with_context_shows_surrounding_lines() {
        let (_tmp, ctx) = fixture();
        let root = ctx.project.as_ref().unwrap().canonical_root.clone();
        std::fs::write(
            root.join("src/ctx.txt"),
            "before1\nbefore2\nthe target\nafter1\nafter2\n",
        )
        .unwrap();
        let out = Grep
            .execute(
                serde_json::json!({"pattern": "the target", "path": "src/ctx.txt", "context": 2}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("before1"), "got: {}", out.content);
        assert!(out.content.contains("after2"), "got: {}", out.content);
        assert!(
            out.content.contains(":3:the target"),
            "got: {}",
            out.content
        );
        // context lines carry the "-" marker, match lines carry ":"
        assert!(out.content.contains(":1-before1"), "got: {}", out.content);
    }

    #[tokio::test]
    async fn multi_edit_applies_multiple_edits_atomically() {
        let (_tmp, ctx) = fixture();
        let root = ctx.project.as_ref().unwrap().canonical_root.clone();
        std::fs::write(root.join("src/multi.txt"), "foo\nbar\nbaz\n").unwrap();
        let out = MultiEdit
            .execute(
                serde_json::json!({
                    "path": "src/multi.txt",
                    "edits": [
                        {"old_string": "foo", "new_string": "FOO"},
                        {"old_string": "bar", "new_string": "BAR"},
                        {"old_string": "baz", "new_string": "BAZ"}
                    ]
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            out.content.contains("3 unique edits"),
            "got: {}",
            out.content
        );
        let patches = ctx.proposed_patches.lock().unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].proposed_content, "FOO\nBAR\nBAZ\n");
    }

    #[tokio::test]
    async fn multi_edit_fails_atomically_on_ambiguous_snippet() {
        let (_tmp, ctx) = fixture();
        let root = ctx.project.as_ref().unwrap().canonical_root.clone();
        // "other" appears once in lib.rs; duplicate a line to make ambiguity
        std::fs::write(root.join("src/multi.txt"), "same\nsame\n").unwrap();
        let err = MultiEdit
            .execute(
                serde_json::json!({
                    "path": "src/multi.txt",
                    "edits": [
                        {"old_string": "same", "new_string": "changed"}
                    ]
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not unique"),
            "unexpected error: {err}"
        );
        // Nothing was proposed
        assert!(ctx.proposed_patches.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn edit_file_anchor_line_disambiguates_repeated_snippet() {
        let (_tmp, ctx) = fixture();
        let root = ctx.project.as_ref().unwrap().canonical_root.clone();
        // "handle() {" appears 3 times; only the one near line 5 should change.
        std::fs::write(
            root.join("src/anchored.txt"),
            "fn a() { handle() { } }\nfn b() { handle() { } }\nfn c() { handle() { } }\n\
             fn d() { handle() { } }\nfn target() { handle() { } }\n",
        )
        .unwrap();
        let out = EditFile
            .execute(
                serde_json::json!({
                    "path": "src/anchored.txt",
                    "old_string": "fn target() { handle() { } }",
                    "new_string": "fn target() { FIXED() { } }",
                    "anchor_line": 5
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("unique edit"), "got: {}", out.content);
        let patches = ctx.proposed_patches.lock().unwrap();
        assert_eq!(patches.len(), 1);
        assert!(
            patches[0]
                .proposed_content
                .contains("fn target() { FIXED()"),
            "got: {}",
            patches[0].proposed_content
        );
    }

    #[tokio::test]
    async fn edit_file_without_anchor_still_rejects_ambiguous() {
        let (_tmp, ctx) = fixture();
        let root = ctx.project.as_ref().unwrap().canonical_root.clone();
        std::fs::write(root.join("src/amb.txt"), "x\nx\n").unwrap();
        let err = EditFile
            .execute(
                serde_json::json!({"path": "src/amb.txt", "old_string": "x", "new_string": "y"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not unique"));
    }

    #[tokio::test]
    async fn grep_reuses_walk_cache_then_sees_new_file_after_ttl() {
        // N2.3: consecutive greps reuse a single traversal; a newly-written file
        // becomes visible once the short TTL expires (or after a write).
        let (_tmp, ctx) = fixture();
        let root = ctx.project.as_ref().unwrap().canonical_root.clone();
        std::fs::write(root.join("src/a.txt"), "needle-x\n").unwrap();
        let _ = super::cached_walk(&root);
        // Simulate a write landing right after the first walk populated the cache.
        std::fs::write(root.join("src/b.txt"), "needle-y\n").unwrap();
        // With the cache active (same instant), b.txt may be missing from the
        // list until the TTL lapses — but re-running the real Grep after letting
        // time pass must find it. Sleep past the TTL to force a fresh walk.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let out = Grep
            .execute(
                serde_json::json!({"pattern": "needle-y", "path": "src"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            out.content.contains("needle-y"),
            "new file must be found after TTL, got: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn grep_finds_function() {
        let (_tmp, ctx) = fixture();
        let out = Grep
            .execute(serde_json::json!({"pattern": "fn authenticate"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("src/lib.rs:1:"));
    }

    #[tokio::test]
    async fn glob_finds_rust_files() {
        let (_tmp, ctx) = fixture();
        let out = Glob
            .execute(serde_json::json!({"pattern": "**/*.rs"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.replace('\\', "/").contains("src/lib.rs"));
    }

    #[tokio::test]
    async fn rejects_path_outside_root() {
        let (_tmp, ctx) = fixture();
        let err = ReadFile
            .execute(serde_json::json!({"path": "../../etc/passwd"}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("escapes") || err.to_string().contains("path"));
    }

    #[tokio::test]
    async fn sensitive_read_requires_confirmation() {
        let (_tmp, ctx) = fixture();
        let err = ReadFile
            .execute(serde_json::json!({"path": ".env"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NeedsConfirmation(_)));
    }

    #[tokio::test]
    async fn sensitive_read_allowed_when_flag_set() {
        let (_tmp, mut ctx) = fixture();
        ctx.allow_sensitive = true;
        let out = ReadFile
            .execute(serde_json::json!({"path": ".env"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("SECRET=1"));
    }

    #[tokio::test]
    async fn write_file_does_not_touch_disk() {
        let (_tmp, ctx) = fixture();
        let dest = ctx
            .project
            .as_ref()
            .unwrap()
            .canonical_root
            .join("src/new.rs");
        WriteFile
            .execute(
                serde_json::json!({"path": "src/new.rs", "content": "fn x() {}"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!dest.exists());
        assert_eq!(ctx.proposed_patches.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn edit_file_rejects_ambiguous_snippet() {
        let (_tmp, ctx) = fixture();
        std::fs::write(
            ctx.project
                .as_ref()
                .unwrap()
                .canonical_root
                .join("src/lib.rs"),
            "aa\naa\n",
        )
        .unwrap();
        let err = EditFile
            .execute(
                serde_json::json!({
                    "path": "src/lib.rs",
                    "old_string": "aa",
                    "new_string": "bb"
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not unique"));
    }

    #[tokio::test]
    async fn edit_file_proposes_unique_replacement() {
        let (_tmp, ctx) = fixture();
        EditFile
            .execute(
                serde_json::json!({
                    "path": "src/lib.rs",
                    "old_string": "fn authenticate() {}",
                    "new_string": "fn login() {}"
                }),
                &ctx,
            )
            .await
            .unwrap();
        let patches = ctx.proposed_patches.lock().unwrap();
        assert!(patches[0].proposed_content.contains("fn login()"));
        assert!(
            std::fs::read_to_string(
                ctx.project
                    .as_ref()
                    .unwrap()
                    .canonical_root
                    .join("src/lib.rs")
            )
            .unwrap()
            .contains("fn authenticate()")
        );
    }
}
