//! On-disk Project Context under `.orbit/`.
//!
//! Hand-edited files must never crash the app. Decisions and findings are
//! append-only; tasks and the session index are rewritten atomically.

use crate::session::SessionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONTEXT_MD: &str = "context.md";
pub const DECISIONS_MD: &str = "decisions.md";
pub const FINDINGS_MD: &str = "findings.md";
pub const TASKS_MD: &str = "tasks.md";
pub const SESSIONS_JSON: &str = "sessions.json";
pub const CONFIG_TOML: &str = "config.toml";

const DEFAULT_CONTEXT: &str = "\
# Project context

Describe the goal, architecture, constraints and conventions of this project.
This file is shared by every agent session.
";

const DEFAULT_DECISIONS: &str = "\
# Decisions

Append-only log. Agents and humans can edit this file.
";

const DEFAULT_FINDINGS: &str = "\
# Findings

Append-only log of problems and observations.
";

const DEFAULT_TASKS: &str = "\
# Tasks

Checkbox items: open [ ], in progress [/], done [x]. Quote the id after the box.
";

const DEFAULT_CONFIG: &str = "\
[commands]
allowed = []

[context]
recent_decisions = 10
token_cap = 4000
max_skills = 50

[budget]
session_usd = 2.0
subagent_fraction = 0.25
";

#[derive(Debug, Clone)]
pub struct DigestSettings {
    pub recent_decisions: usize,
    pub token_cap: usize,
    pub session_budget_usd: f64,
    /// Optional cheaper model for context-window summarization. If empty, the
    /// session's own model is used (unchanged behavior).
    pub summary_model: Option<String>,
    /// Cap on skill names listed in the digest. Bodies are never injected.
    pub max_skills: usize,
    /// Fraction of the remaining session budget given to a spawned subagent.
    pub subagent_budget_fraction: f64,
}

impl Default for DigestSettings {
    fn default() -> Self {
        Self {
            recent_decisions: 10,
            token_cap: 4000,
            session_budget_usd: 2.0,
            summary_model: None,
            max_skills: 50,
            subagent_budget_fraction: 0.25,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub at: DateTime<Utc>,
    pub model: String,
    pub session: String,
    pub role: String,
    pub decision: String,
    pub rationale: String,
    pub files: Vec<String>,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub at: DateTime<Utc>,
    pub model: String,
    pub session: String,
    pub role: String,
    pub description: String,
    pub severity: String,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Open,
    InProgress,
    Done,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "open" | "todo" | "aberta" => Some(Self::Open),
            "in_progress" | "in-progress" | "doing" | "em andamento" => Some(Self::InProgress),
            "done" | "closed" | "feita" => Some(Self::Done),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskItem {
    pub id: String,
    pub status: TaskStatus,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TouchedFile {
    pub path: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionRecord {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub last_active_at: Option<String>,
    #[serde(default)]
    pub touched: Vec<TouchedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SessionsFile {
    #[serde(default)]
    sessions: Vec<SessionRecord>,
}

#[derive(Debug, Clone)]
pub struct OrbitStore {
    pub dir: PathBuf,
    pub context_md: String,
    pub decisions: Vec<Decision>,
    pub findings: Vec<Finding>,
    pub tasks: Vec<TaskItem>,
    pub sessions: Vec<SessionRecord>,
    pub skills: Vec<crate::context::Skill>,
    pub warnings: Vec<String>,
    pub settings: DigestSettings,
}

impl OrbitStore {
    pub fn dir_for(project_root: &Path) -> PathBuf {
        project_root.join(".orbit")
    }

    /// Create missing files, then load. Never panics on corrupt input.
    pub fn open(project_root: impl AsRef<Path>) -> Self {
        let project_root = project_root.as_ref();
        let dir = Self::dir_for(project_root);
        let mut warnings = Vec::new();
        if let Err(e) = ensure_layout(&dir) {
            warnings.push(format!("could not create .orbit/: {e}"));
        }
        let mut store = Self {
            dir,
            context_md: String::new(),
            decisions: Vec::new(),
            findings: Vec::new(),
            tasks: Vec::new(),
            sessions: Vec::new(),
            skills: Vec::new(),
            warnings,
            settings: DigestSettings::default(),
        };
        store.reload();
        store
    }

    pub fn reload(&mut self) {
        let mut warnings = std::mem::take(&mut self.warnings);
        self.context_md = read_or_empty(&self.dir.join(CONTEXT_MD));
        let (decisions, w) = parse_decisions(&read_or_empty(&self.dir.join(DECISIONS_MD)));
        warnings.extend(w);
        self.decisions = decisions;
        let (findings, w) = parse_findings(&read_or_empty(&self.dir.join(FINDINGS_MD)));
        warnings.extend(w);
        self.findings = findings;
        let (tasks, w) = parse_tasks(&read_or_empty(&self.dir.join(TASKS_MD)));
        warnings.extend(w);
        self.tasks = tasks;
        match load_sessions(&self.dir.join(SESSIONS_JSON)) {
            Ok(sessions) => self.sessions = sessions,
            Err(w) => {
                warnings.push(w);
                self.sessions = Vec::new();
            }
        }
        self.settings = load_digest_settings(&self.dir.join(CONFIG_TOML));
        let (skills, w) =
            crate::context::skills::load_all(&self.dir.join(crate::context::skills::SKILLS_DIR));
        warnings.extend(w);
        self.skills = skills;
        self.warnings = warnings;
    }

    pub fn append_decision(&mut self, decision: Decision) -> Result<(), String> {
        let block = format_decision(&decision);
        append_file(&self.dir.join(DECISIONS_MD), &block)?;
        self.decisions.push(decision);
        Ok(())
    }

    pub fn append_finding(&mut self, finding: Finding) -> Result<(), String> {
        let block = format_finding(&finding);
        append_file(&self.dir.join(FINDINGS_MD), &block)?;
        self.findings.push(finding);
        Ok(())
    }

    pub fn upsert_task(
        &mut self,
        id: Option<String>,
        status: TaskStatus,
        description: String,
    ) -> Result<TaskItem, String> {
        let item = if let Some(id) = id.filter(|s| !s.is_empty() && s != "new") {
            if let Some(existing) = self.tasks.iter_mut().find(|t| t.id == id) {
                existing.status = status;
                if !description.is_empty() {
                    existing.description = description;
                }
                existing.clone()
            } else {
                let created = TaskItem {
                    id,
                    status,
                    description: if description.is_empty() {
                        "(no description)".into()
                    } else {
                        description
                    },
                };
                self.tasks.push(created.clone());
                created
            }
        } else {
            let created = TaskItem {
                id: next_task_id(&self.tasks),
                status,
                description: if description.is_empty() {
                    "(no description)".into()
                } else {
                    description
                },
            };
            self.tasks.push(created.clone());
            created
        };
        self.write_tasks()?;
        Ok(item)
    }

    pub fn upsert_session(&mut self, record: SessionRecord) -> Result<(), String> {
        if let Some(existing) = self.sessions.iter_mut().find(|s| s.id == record.id) {
            existing.label = record.label;
            if !record.model.is_empty() {
                existing.model = record.model;
            }
            if record.last_active_at.is_some() {
                existing.last_active_at = record.last_active_at;
            }
        } else {
            self.sessions.push(record);
        }
        self.write_sessions()
    }

    pub fn mark_active(&mut self, id: &SessionId, label: &str, model: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        if let Some(existing) = self.sessions.iter_mut().find(|s| s.id == id.as_str()) {
            existing.last_active_at = Some(now);
            existing.label = label.to_string();
            if !model.is_empty() {
                existing.model = model.to_string();
            }
        } else {
            self.sessions.push(SessionRecord {
                id: id.as_str().to_string(),
                label: label.to_string(),
                model: model.to_string(),
                last_active_at: Some(now),
                touched: Vec::new(),
            });
        }
        self.write_sessions()
    }

    pub fn record_touch(
        &mut self,
        id: &SessionId,
        label: &str,
        model: &str,
        relative: &Path,
    ) -> Result<(), String> {
        let path = relative.display().to_string().replace('\\', "/");
        let at = Utc::now().to_rfc3339();
        let session = if let Some(existing) = self.sessions.iter_mut().find(|s| s.id == id.as_str())
        {
            existing
        } else {
            self.sessions.push(SessionRecord {
                id: id.as_str().to_string(),
                label: label.to_string(),
                model: model.to_string(),
                last_active_at: None,
                touched: Vec::new(),
            });
            self.sessions.last_mut().expect("just pushed")
        };
        session.label = label.to_string();
        if !model.is_empty() {
            session.model = model.to_string();
        }
        if let Some(hit) = session.touched.iter_mut().find(|t| t.path == path) {
            hit.at = at;
        } else {
            session.touched.push(TouchedFile { path, at });
        }
        self.write_sessions()
    }

    pub fn session(&self, id: &SessionId) -> Option<&SessionRecord> {
        self.sessions.iter().find(|s| s.id == id.as_str())
    }

    fn write_tasks(&self) -> Result<(), String> {
        let mut body = String::from("# Tasks\n\n");
        for task in &self.tasks {
            let mark = match task.status {
                TaskStatus::Open => " ",
                TaskStatus::InProgress => "/",
                TaskStatus::Done => "x",
            };
            body.push_str(&format!(
                "- [{mark}] `{id}` {desc}\n",
                id = task.id,
                desc = task.description
            ));
        }
        atomic_write(&self.dir.join(TASKS_MD), &body)
    }

    fn write_sessions(&self) -> Result<(), String> {
        let file = SessionsFile {
            sessions: self.sessions.clone(),
        };
        let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        atomic_write(&self.dir.join(SESSIONS_JSON), &json)
    }
}

fn ensure_layout(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    write_if_missing(&dir.join(CONTEXT_MD), DEFAULT_CONTEXT)?;
    write_if_missing(&dir.join(DECISIONS_MD), DEFAULT_DECISIONS)?;
    write_if_missing(&dir.join(FINDINGS_MD), DEFAULT_FINDINGS)?;
    write_if_missing(&dir.join(TASKS_MD), DEFAULT_TASKS)?;
    write_if_missing(&dir.join(SESSIONS_JSON), "{\n  \"sessions\": []\n}\n")?;
    write_if_missing(&dir.join(CONFIG_TOML), DEFAULT_CONFIG)?;
    std::fs::create_dir_all(dir.join(crate::context::skills::SKILLS_DIR))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn write_if_missing(path: &Path, contents: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    atomic_write(path, contents)
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("orbit.tmp");
    std::fs::write(&tmp, contents).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

fn append_file(path: &Path, block: &str) -> Result<(), String> {
    let mut current = read_or_empty(path);
    if !current.is_empty() && !current.ends_with('\n') {
        current.push('\n');
    }
    if !current.ends_with("\n\n") && !current.is_empty() {
        current.push('\n');
    }
    current.push_str(block);
    if !current.ends_with('\n') {
        current.push('\n');
    }
    atomic_write(path, &current)
}

fn load_sessions(path: &Path) -> Result<Vec<SessionRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("sessions.json: {e}"))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<SessionsFile>(&text) {
        Ok(file) => Ok(file.sessions),
        Err(e) => Err(format!(
            "sessions.json is not valid JSON ({e}); left untouched"
        )),
    }
}

fn load_digest_settings(path: &Path) -> DigestSettings {
    let Ok(text) = std::fs::read_to_string(path) else {
        return DigestSettings::default();
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return DigestSettings::default();
    };
    let ctx = value.get("context");
    let budget = value.get("budget");
    DigestSettings {
        recent_decisions: ctx
            .and_then(|c| c.get("recent_decisions"))
            .and_then(|v| v.as_integer())
            .map(|n| n.clamp(1, 100) as usize)
            .unwrap_or(10),
        token_cap: ctx
            .and_then(|c| c.get("token_cap"))
            .and_then(|v| v.as_integer())
            .map(|n| n.clamp(200, 32_000) as usize)
            .unwrap_or(4000),
        session_budget_usd: budget
            .and_then(|b| b.get("session_usd"))
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|n| n as f64)))
            .map(|n| n.clamp(0.01, 10_000.0))
            .unwrap_or(2.0),
        summary_model: ctx
            .and_then(|c| c.get("summary_model"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        max_skills: ctx
            .and_then(|c| c.get("max_skills"))
            .and_then(|v| v.as_integer())
            .map(|n| n.clamp(1, 500) as usize)
            .unwrap_or(50),
        subagent_budget_fraction: budget
            .and_then(|b| b.get("subagent_fraction"))
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|n| n as f64)))
            .map(|n| n.clamp(0.05, 0.5))
            .unwrap_or(0.25),
    }
}

fn next_task_id(tasks: &[TaskItem]) -> String {
    let mut n = tasks.len() + 1;
    loop {
        let id = format!("t{n}");
        if !tasks.iter().any(|t| t.id == id) {
            return id;
        }
        n += 1;
    }
}

pub fn parse_decisions(text: &str) -> (Vec<Decision>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut out = Vec::new();
    for (heading, body) in markdown_blocks(text) {
        match parse_decision_block(&heading, &body) {
            Ok(item) => out.push(item),
            Err(w) => warnings.push(w),
        }
    }
    (out, warnings)
}

pub fn parse_findings(text: &str) -> (Vec<Finding>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut out = Vec::new();
    for (heading, body) in markdown_blocks(text) {
        match parse_finding_block(&heading, &body) {
            Ok(item) => out.push(item),
            Err(w) => warnings.push(w),
        }
    }
    (out, warnings)
}

pub fn parse_tasks(text: &str) -> (Vec<TaskItem>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('-') {
            continue;
        }
        match parse_task_line(trimmed) {
            Ok(Some(task)) => out.push(task),
            Ok(None) => {}
            Err(w) => warnings.push(w),
        }
    }
    (out, warnings)
}

fn markdown_blocks(text: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
            current = Some((rest.to_string(), String::new()));
        } else if let Some((_, body)) = &mut current {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(block) = current {
        blocks.push(block);
    }
    blocks
}

fn parse_heading(heading: &str) -> Result<(DateTime<Utc>, String, String, bool), String> {
    let pinned = heading.to_ascii_lowercase().contains("[pinned]");
    let heading = heading.replace("[pinned]", "");
    let heading = heading.trim();
    let (when_raw, rest) = split_heading(heading)
        .ok_or_else(|| format!("could not parse decision/finding heading: {heading}"))?;
    let at = DateTime::parse_from_rfc3339(when_raw.trim())
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            DateTime::parse_from_rfc3339(&format!("{}Z", when_raw.trim()))
                .map(|d| d.with_timezone(&Utc))
        })
        .map_err(|_| format!("invalid timestamp in heading: {when_raw}"))?;
    let rest = rest.trim();
    let (model, session) = parse_model_session(rest);
    Ok((at, model, session, pinned))
}

fn split_heading(heading: &str) -> Option<(&str, &str)> {
    for sep in [" — ", " – ", " - "] {
        if let Some((a, b)) = heading.split_once(sep) {
            return Some((a, b));
        }
    }
    None
}

fn parse_model_session(rest: &str) -> (String, String) {
    if let Some((model, after)) = rest.split_once("(session \"") {
        let session = after.trim_end_matches(')').trim_end_matches('"');
        return (model.trim().to_string(), session.to_string());
    }
    if let Some((model, after)) = rest.split_once("(sessão \"") {
        let session = after.trim_end_matches(')').trim_end_matches('"');
        return (model.trim().to_string(), session.to_string());
    }
    (rest.to_string(), String::new())
}

fn field(body: &str, keys: &[&str]) -> Option<String> {
    for line in body.lines() {
        let line = line.trim();
        for key in keys {
            let prefix = format!("**{key}:**");
            if let Some(rest) = line
                .strip_prefix(&prefix)
                .or_else(|| line.strip_prefix(&format!("{key}:")))
            {
                let value = rest.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn parse_decision_block(heading: &str, body: &str) -> Result<Decision, String> {
    let (at, model, session, pinned_heading) = parse_heading(heading)?;
    let decision = field(body, &["Decision", "Decisão"]).unwrap_or_else(|| {
        body.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("(unparsed decision)")
            .to_string()
    });
    let rationale = field(body, &["Rationale", "Motivo"]).unwrap_or_default();
    let files = field(body, &["Files", "Arquivos"])
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let pinned = pinned_heading
        || body.to_ascii_lowercase().contains("**pinned:**")
        || body.to_ascii_lowercase().contains("**fixada:**");
    Ok(Decision {
        at,
        model,
        session,
        role: field(body, &["Role", "Papel"]).unwrap_or_else(|| "Coder".to_string()),
        decision,
        rationale,
        files,
        pinned,
    })
}

fn parse_finding_block(heading: &str, body: &str) -> Result<Finding, String> {
    let (at, model, session, _) = parse_heading(heading)?;
    let description = field(body, &["Finding", "Description", "Descrição"]).unwrap_or_else(|| {
        body.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("(unparsed finding)")
            .to_string()
    });
    let severity = field(body, &["Severity", "Severidade"]).unwrap_or_else(|| "info".into());
    let location = field(body, &["Location", "Local"]);
    Ok(Finding {
        at,
        model,
        session,
        role: field(body, &["Role", "Papel"]).unwrap_or_else(|| "Coder".to_string()),
        description,
        severity,
        location,
    })
}

fn parse_task_line(line: &str) -> Result<Option<TaskItem>, String> {
    let rest = line.trim_start_matches('-').trim();
    let (mark, rest) = if let Some(rest) = rest.strip_prefix('[') {
        let Some((mark, after)) = rest.split_once(']') else {
            return Err(format!("malformed task line: {line}"));
        };
        (mark.trim(), after.trim())
    } else {
        return Ok(None);
    };
    let status = match mark {
        "" | " " => TaskStatus::Open,
        "/" | "~" | "in_progress" => TaskStatus::InProgress,
        "x" | "X" => TaskStatus::Done,
        other => TaskStatus::parse(other).unwrap_or(TaskStatus::Open),
    };
    let (id, description) = if let Some(rest) = rest.strip_prefix('`') {
        let Some((id, after)) = rest.split_once('`') else {
            return Err(format!("task is missing a closing id tick: {line}"));
        };
        (id.trim().to_string(), after.trim().to_string())
    } else {
        let desc = rest.trim();
        if desc.is_empty() {
            return Ok(None);
        }
        (slug_id(desc), desc.to_string())
    };
    if id.is_empty() {
        return Ok(None);
    }
    Ok(Some(TaskItem {
        id,
        status,
        description,
    }))
}

fn slug_id(desc: &str) -> String {
    let slug: String = desc
        .chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else if c.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .take(24)
        .collect();
    if slug.is_empty() { "task".into() } else { slug }
}

pub fn format_decision(d: &Decision) -> String {
    let files = if d.files.is_empty() {
        String::new()
    } else {
        format!("**Files:** {}\n", d.files.join(", "))
    };
    format!(
        "## {at} — {model} (session \"{session}\")\n\
         **Role:** {role}\n\
         **Decision:** {decision}\n\
         **Rationale:** {rationale}\n\
         {files}",
        at = d.at.to_rfc3339(),
        model = d.model,
        session = d.session,
        role = d.role,
        decision = d.decision,
        rationale = d.rationale,
    )
}

pub fn format_finding(f: &Finding) -> String {
    let loc = f
        .location
        .as_ref()
        .map(|l| format!("**Location:** {l}\n"))
        .unwrap_or_default();
    format!(
        "## {at} — {model} (session \"{session}\")\n\
         **Role:** {role}\n\
         **Finding:** {description}\n\
         **Severity:** {severity}\n\
         {loc}",
        at = f.at.to_rfc3339(),
        model = f.model,
        session = f.session,
        role = f.role,
        description = f.description,
        severity = f.severity,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_creates_layout() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let store = OrbitStore::open(&root);
        let dir = root.join(".orbit");
        for name in [
            CONTEXT_MD,
            DECISIONS_MD,
            FINDINGS_MD,
            TASKS_MD,
            SESSIONS_JSON,
            CONFIG_TOML,
        ] {
            assert!(dir.join(name).exists(), "{name}");
        }
        assert!(
            dir.join(crate::context::skills::SKILLS_DIR).is_dir(),
            "skills/"
        );
        assert!(store.warnings.is_empty(), "{:?}", store.warnings);
        assert!(store.skills.is_empty());
    }

    #[test]
    fn corrupt_decisions_degrades_with_warning() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join(".orbit")).unwrap();
        std::fs::write(
            root.join(".orbit").join(DECISIONS_MD),
            "## not-a-date — mystery\nthis is junk\n## also bad\n",
        )
        .unwrap();
        let store = OrbitStore::open(&root);
        assert!(store.decisions.is_empty());
        assert!(
            store
                .warnings
                .iter()
                .any(|w| w.contains("timestamp") || w.contains("heading")),
            "{:?}",
            store.warnings
        );
    }

    #[test]
    fn append_decision_round_trips() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let mut store = OrbitStore::open(&root);
        store
            .append_decision(Decision {
                at: DateTime::parse_from_rfc3339("2026-08-12T14:32:11Z")
                    .unwrap()
                    .with_timezone(&Utc),
                model: "claude-opus-5".into(),
                session: "architecture".into(),
                role: "Reviewer".into(),
                decision: "Use JWT with refresh tokens.".into(),
                rationale: "Stateless API sessions.".into(),
                files: vec!["src/auth/token.rs".into()],
                pinned: false,
            })
            .unwrap();
        let text = std::fs::read_to_string(root.join(".orbit").join(DECISIONS_MD)).unwrap();
        assert!(text.contains("claude-opus-5"));
        assert!(text.contains("session \"architecture\""));
        assert!(text.contains("Use JWT with refresh tokens."));
        store.reload();
        assert_eq!(store.decisions.len(), 1);
        assert_eq!(store.decisions[0].session, "architecture");
        assert_eq!(store.decisions[0].role, "Reviewer");
        assert_eq!(store.decisions[0].files[0], "src/auth/token.rs");
    }

    #[test]
    fn legacy_decision_without_role_defaults_to_coder() {
        let text = r#"
## 2026-08-12T14:32:11Z — claude-opus-5 (session "arquitetura")
**Decision:** An old entry, no Role line.
**Rationale:** Legacy format.
"#;
        let (items, warnings) = parse_decisions(text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].role, "Coder");
    }

    #[test]
    fn hand_edited_portuguese_decision_parses() {
        let text = r#"
## 2026-08-12T14:32:11Z — claude-opus-5 (sessão "arquitetura")
**Decisão:** Autenticação usará JWT.
**Motivo:** Sessão stateless.
**Arquivos:** src/auth/token.rs, src/auth/login.rs
"#;
        let (items, warnings) = parse_decisions(text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].decision, "Autenticação usará JWT.");
        assert_eq!(items[0].files.len(), 2);
    }

    #[test]
    fn task_lines_parse_and_rewrite() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let mut store = OrbitStore::open(&root);
        let created = store
            .upsert_task(None, TaskStatus::Open, "Cover auth".into())
            .unwrap();
        store
            .upsert_task(
                Some(created.id.clone()),
                TaskStatus::InProgress,
                String::new(),
            )
            .unwrap();
        store.reload();
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].status, TaskStatus::InProgress);
        assert_eq!(store.tasks[0].description, "Cover auth");
    }

    #[test]
    fn summary_model_is_parsed_from_context_config() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join(".orbit")).unwrap();
        std::fs::write(
            root.join(".orbit").join(CONFIG_TOML),
            "[context]\nrecent_decisions = 10\ntoken_cap = 4000\nsummary_model = \"flash-cheap\"\n",
        )
        .unwrap();
        let store = OrbitStore::open(&root);
        assert_eq!(store.settings.summary_model.as_deref(), Some("flash-cheap"));
    }

    #[test]
    fn summary_model_defaults_to_none_without_config() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let store = OrbitStore::open(&root);
        assert_eq!(store.settings.summary_model, None);
    }

    #[test]
    fn skill_without_frontmatter_warns_and_does_not_crash() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let mut store = OrbitStore::open(&root);
        let dir = store.dir.join("skills").join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "no frontmatter here\n").unwrap();
        store.reload();
        assert!(store.skills.is_empty());
        assert!(
            store.warnings.iter().any(|w| w.contains("frontmatter")),
            "{:?}",
            store.warnings
        );
    }
}
