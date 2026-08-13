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
const DENY_NAMES: &[&str] = &["target", "node_modules", ".git", "dist", "build"];

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
        let text = std::fs::read_to_string(&path).map_err(|e| ToolError::Message(e.to_string()))?;
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
                "path": { "type": "string" }
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
        let walk = WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(true)
            .filter_entry(|e| !DENY_NAMES.iter().any(|d| e.file_name() == *d))
            .build();
        for entry in walk.flatten() {
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::Message("cancelled".into()));
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let rel = PathBuf::from(relative_of(project, path));
            if is_sensitive(&rel) && !ctx.allow_sensitive {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            for (idx, line) in text.lines().enumerate() {
                if regex.is_match(line) {
                    hits.push(format!("{}:{}:{line}", rel.display(), idx + 1));
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
        let walk = WalkBuilder::new(&project.canonical_root)
            .hidden(false)
            .git_ignore(true)
            .filter_entry(|e| !DENY_NAMES.iter().any(|d| e.file_name() == *d))
            .build();
        for entry in walk.flatten() {
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::Message("cancelled".into()));
            }
            let path = entry.path();
            if path == project.canonical_root {
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
                "new_string": { "type": "string" }
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
        let matches = original.matches(old).count();
        if matches == 0 {
            return Err(ToolError::Message(format!("snippet not found in {rel}")));
        }
        if matches > 1 {
            return Err(ToolError::Message(format!(
                "snippet is not unique in {rel} ({matches} occurrences); refuse to guess"
            )));
        }
        let proposed = original.replacen(old, new, 1);
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
            runner: None,
            run_configs: None,
            run_starts: None,
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
    async fn list_dir_lists_src() {
        let (_tmp, ctx) = fixture();
        let out = ListDir
            .execute(serde_json::json!({"path": "src"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("lib.rs"));
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
