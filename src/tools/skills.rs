//! Procedural skill tools. `read_skill` is immediate; `create_skill` is a patch.

use super::{Tool, ToolContext, ToolError, ToolOutcome, ToolRisk, truncate_output};
use crate::context::skills::{self, Skill, is_valid_slug, render_skill_file};
use crate::workspace::FilePatch;
use async_trait::async_trait;
use std::path::PathBuf;

pub struct ReadSkill;
pub struct CreateSkill;

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArgs(format!("missing `{key}`")))
}

fn load_skills(ctx: &ToolContext) -> Result<Vec<Skill>, ToolError> {
    if let Some(store) = &ctx.store {
        let mut store = store
            .lock()
            .map_err(|_| ToolError::Message("context store lock poisoned".into()))?;
        store.reload();
        return Ok(store.skills.clone());
    }
    let project = ctx
        .project
        .as_ref()
        .ok_or_else(|| ToolError::Message("no project is open".into()))?;
    let (skills, _) = skills::load_all(
        &project
            .canonical_root
            .join(".orbit")
            .join(skills::SKILLS_DIR),
    );
    Ok(skills)
}

fn find_skill<'a>(skills: &'a [Skill], name: &str) -> Option<&'a Skill> {
    skills.iter().find(|s| s.slug == name || s.name == name)
}

fn listing(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return "(none)".into();
    }
    skills
        .iter()
        .map(|s| {
            if s.slug == s.name {
                s.slug.clone()
            } else {
                format!("{} ({})", s.name, s.slug)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[async_trait]
impl Tool for ReadSkill {
    fn name(&self) -> &'static str {
        "read_skill"
    }

    fn description(&self) -> &'static str {
        "Read the full body of a project skill by name or slug. \
         Use this after seeing a relevant skill in the Project Context digest."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
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
        let name = arg_str(&args, "name")?;
        let skills = load_skills(ctx)?;
        let Some(skill) = find_skill(&skills, name) else {
            return Err(ToolError::Message(format!(
                "unknown skill `{name}`. Known skills: {}",
                listing(&skills)
            )));
        };
        Ok(truncate_output(format!(
            "# {} ({})\n{}\n\n{}",
            skill.name, skill.slug, skill.description, skill.body
        )))
    }
}

#[async_trait]
impl Tool for CreateSkill {
    fn name(&self) -> &'static str {
        "create_skill"
    }

    fn description(&self) -> &'static str {
        "Create or overwrite a project skill under .orbit/skills/<name>/SKILL.md. \
         The write stays pending until approved. Use this to record how something \
         is done in this project."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "description": { "type": "string" },
                "body": { "type": "string" }
            },
            "required": ["name", "description", "body"]
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
        let name = arg_str(&args, "name")?;
        if !is_valid_slug(name) {
            return Err(ToolError::InvalidArgs(format!(
                "`name` must be a slug ([a-z0-9-], at most {} characters)",
                skills::SLUG_MAX_LEN
            )));
        }
        let description = arg_str(&args, "description")?;
        let body = arg_str(&args, "body")?;
        let project = ctx
            .project
            .as_ref()
            .ok_or_else(|| ToolError::Message("no project is open".into()))?;
        let rel = format!(
            ".orbit/{}/{}/{}",
            skills::SKILLS_DIR,
            name,
            skills::SKILL_FILE
        );
        let dest = project.canonical_root.join(&rel);
        let original = if dest.exists() {
            std::fs::read_to_string(&dest).map_err(|e| ToolError::Message(e.to_string()))?
        } else {
            String::new()
        };
        let content = render_skill_file(name, description, body);
        let patch = FilePatch::new(PathBuf::from(rel.replace('\\', "/")), original, content);
        let summary = format!(
            "Proposed skill `{name}` ({} bytes). Waiting for approval.",
            patch.proposed_content.len()
        );
        ctx.proposed_patches
            .lock()
            .map_err(|_| ToolError::Message("patch lock poisoned".into()))?
            .push(patch);
        Ok(truncate_output(summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::OrbitStore;
    use crate::session::SessionId;
    use crate::tools::ToolContext;
    use crate::workspace::Project;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn fixture() -> (TempDir, ToolContext) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let project = Arc::new(Project::open(&root).unwrap());
        let store = Arc::new(Mutex::new(OrbitStore::open(&root)));
        let ctx = ToolContext {
            session: SessionId::new("sid"),
            cancel: CancellationToken::new(),
            project: Some(project),
            allow_sensitive: false,
            proposed_patches: Arc::new(Mutex::new(Vec::new())),
            allow_execute: false,
            command_timeout: crate::tools::shell::COMMAND_TIMEOUT,
            terminal: None,
            store: Some(store),
            session_label: "architecture".into(),
            session_model: "model-a".into(),
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
    async fn create_skill_proposes_a_patch_and_does_not_write() {
        let (_tmp, ctx) = fixture();
        CreateSkill
            .execute(
                serde_json::json!({
                    "name": "run-tests",
                    "description": "How to run the suite.",
                    "body": "cargo test --all-targets"
                }),
                &ctx,
            )
            .await
            .unwrap();
        let patches = ctx.proposed_patches.lock().unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(
            patches[0]
                .relative_path
                .to_string_lossy()
                .replace('\\', "/"),
            ".orbit/skills/run-tests/SKILL.md"
        );
        assert!(
            patches[0]
                .proposed_content
                .contains("cargo test --all-targets")
        );
        assert!(
            !ctx.project
                .as_ref()
                .unwrap()
                .canonical_root
                .join(".orbit/skills/run-tests/SKILL.md")
                .exists()
        );
    }

    #[tokio::test]
    async fn create_skill_rejects_invalid_slug() {
        let (_tmp, ctx) = fixture();
        let err = CreateSkill
            .execute(
                serde_json::json!({
                    "name": "Run Tests",
                    "description": "x",
                    "body": "y"
                }),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("slug"), "{err}");
    }

    #[tokio::test]
    async fn read_skill_returns_body_and_lists_names_on_miss() {
        let (_tmp, ctx) = fixture();
        let root = ctx.project.as_ref().unwrap().canonical_root.clone();
        let dir = root.join(".orbit/skills/run-tests");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            render_skill_file(
                "run-tests",
                "How to run the suite.",
                "cargo test --all-targets",
            ),
        )
        .unwrap();
        let out = ReadSkill
            .execute(serde_json::json!({"name": "run-tests"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("cargo test --all-targets"));
        assert!(out.content.contains("How to run the suite."));

        let err = ReadSkill
            .execute(serde_json::json!({"name": "missing"}), &ctx)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown skill"), "{msg}");
        assert!(msg.contains("run-tests"), "{msg}");
    }
}
