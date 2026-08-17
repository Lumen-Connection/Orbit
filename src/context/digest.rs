//! Build the Project Context block injected into the system prompt.

use super::store::{Decision, Finding, OrbitStore, SessionRecord, TaskStatus};
use crate::session::SessionId;
use chrono::{DateTime, Utc};

pub struct Digest {
    pub text: String,
    pub token_estimate: usize,
}

pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

pub fn build_digest(store: &OrbitStore, session: &SessionId, project_name: &str) -> Digest {
    let settings = &store.settings;
    let mine = store.session(session);
    let last_active = mine
        .and_then(|s| s.last_active_at.as_deref())
        .and_then(parse_ts);

    let mut decisions: Vec<&Decision> = store.decisions.iter().collect();
    decisions.sort_by_key(|d| d.at);
    let pinned: Vec<&Decision> = decisions.iter().copied().filter(|d| d.pinned).collect();
    let recent: Vec<&Decision> = decisions
        .iter()
        .copied()
        .rev()
        .take(settings.recent_decisions)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let mut shown: Vec<&Decision> = Vec::new();
    for d in pinned.into_iter().chain(recent) {
        if !shown.iter().any(|s| std::ptr::eq(*s, d)) {
            shown.push(d);
        }
    }

    let open_tasks: Vec<_> = store
        .tasks
        .iter()
        .filter(|t| t.status != TaskStatus::Done)
        .collect();

    let mut findings: Vec<&Finding> = store.findings.iter().collect();
    findings.sort_by_key(|f| f.at);
    let findings: Vec<&Finding> = findings
        .into_iter()
        .rev()
        .take(settings.recent_decisions)
        .collect();

    let changes = foreign_changes(store, session, last_active);
    let foreign_section = format_foreign_section(&changes);

    let mut body = String::new();
    body.push_str("=== PROJECT CONTEXT ===\n");
    body.push_str(&format!("Project: {project_name}\n"));
    let context = store.context_md.trim();
    if !context.is_empty() {
        body.push('\n');
        body.push_str(context);
        body.push('\n');
    }

    body.push_str(&format!(
        "\nCurrent decisions ({} of {}):\n",
        shown.len(),
        store.decisions.len()
    ));
    if shown.is_empty() {
        body.push_str("- (none yet)\n");
    } else {
        for d in &shown {
            body.push_str(&format!(
                "- [{model}, {when}] {text}\n",
                model = d.model,
                when = short_when(d.at),
                text = d.decision
            ));
        }
    }

    body.push_str(&format!("\nOpen tasks ({}):\n", open_tasks.len()));
    if open_tasks.is_empty() {
        body.push_str("- (none)\n");
    } else {
        for t in &open_tasks {
            body.push_str(&format!(
                "- [{status}] {desc}\n",
                status = t.status.as_str(),
                desc = t.description
            ));
        }
    }

    let contract =
        crate::pipeline::contract::ContractStore::open(store.dir.parent().unwrap_or(&store.dir));
    if let Some(plan) = contract.planner().ok().flatten() {
        body.push_str("\nAcceptance criteria:\n");
        if plan.acceptance_criteria.is_empty() {
            body.push_str("- (none)\n");
        } else {
            for ac in &plan.acceptance_criteria {
                body.push_str(&format!("- [{id}] {text}\n", id = ac.id, text = ac.text));
            }
        }
        if !plan.scope.is_empty() {
            body.push_str(&format!("Scope: {}\n", plan.scope));
        }
        if !plan.non_goals.is_empty() {
            body.push_str(&format!("Non-goals: {}\n", plan.non_goals));
        }
    }
    if let Some(coder) = contract.coder().ok().flatten()
        && (!coder.lint_results.is_empty() || !coder.test_results.is_empty())
    {
        body.push_str("\nVerification results:\n");
        if !coder.lint_results.is_empty() {
            body.push_str(&format!("{}\n", coder.lint_results));
        }
        if !coder.test_results.is_empty() {
            body.push_str(&format!("{}\n", coder.test_results));
        }
    }

    body.push_str(&format!("\nRecent findings ({}):\n", findings.len()));
    if findings.is_empty() {
        body.push_str("- (none)\n");
    } else {
        for f in &findings {
            body.push_str(&format!(
                "- [{model}, {when}] {text}\n",
                model = f.model,
                when = short_when(f.at),
                text = f.description
            ));
        }
    }

    append_skills_section(&mut body, store);

    body.push('\n');
    body.push_str(&foreign_section);
    body.push_str("=== END OF CONTEXT ===\n");

    let mut text = body;
    trim_to_cap(&mut text, settings.token_cap);
    let token_estimate = estimate_tokens(&text);
    Digest {
        text,
        token_estimate,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffSummary {
    pub new_decisions: usize,
    pub files: Vec<(String, String)>,
    pub banner: String,
    pub digest_section: String,
}

impl HandoffSummary {
    pub fn is_interesting(&self) -> bool {
        self.new_decisions > 0 || !self.files.is_empty()
    }
}

pub fn build_handoff(store: &OrbitStore, session: &SessionId) -> HandoffSummary {
    let last_active = store
        .session(session)
        .and_then(|s| s.last_active_at.as_deref())
        .and_then(parse_ts);
    let changes = foreign_changes(store, session, last_active);
    let new_decisions = store
        .decisions
        .iter()
        .filter(|d| match last_active {
            None => true,
            Some(mine) => d.at > mine,
        })
        .count();
    let files: Vec<(String, String)> = changes
        .iter()
        .map(|(path, rec)| ((*path).to_string(), rec.label.clone()))
        .collect();
    let mut authors: Vec<String> = Vec::new();
    for (_, label) in &files {
        if !authors.iter().any(|a| a == label) {
            authors.push(label.clone());
        }
    }
    let by = if authors.is_empty() {
        String::new()
    } else {
        format!(" by `{}`", authors.join("`, `"))
    };
    let banner = format!(
        "Since last time: {new_decisions} decisions, {n} files changed{by}",
        n = files.len()
    );
    HandoffSummary {
        new_decisions,
        files,
        banner,
        digest_section: format_foreign_section(&changes),
    }
}

fn append_skills_section(body: &mut String, store: &OrbitStore) {
    let total = store.skills.len();
    let cap = store.settings.max_skills.max(1);
    let shown = total.min(cap);
    if total > shown {
        body.push_str(&format!("\nAvailable skills ({total}, showing {shown}):\n"));
    } else {
        body.push_str(&format!("\nAvailable skills ({total}):\n"));
    }
    if total == 0 {
        body.push_str("- (none)\n");
        return;
    }
    for skill in store.skills.iter().take(shown) {
        body.push_str(&format!("- {}: {}\n", skill.name, skill.description));
    }
    if total > shown {
        body.push_str(&format!("- ({} more skills not listed)\n", total - shown));
    }
}

fn format_foreign_section(changes: &[(&str, &SessionRecord)]) -> String {
    let mut body = String::from("Since your last run, other sessions changed:\n");
    if changes.is_empty() {
        body.push_str("- (no files changed by other sessions)\n");
    } else {
        for (path, rec) in changes {
            body.push_str(&format!(
                "- {path}  (by {model}, session \"{label}\")\n",
                model = rec.model,
                label = rec.label
            ));
        }
    }
    body
}

fn foreign_changes<'a>(
    store: &'a OrbitStore,
    session: &SessionId,
    last_active: Option<DateTime<Utc>>,
) -> Vec<(&'a str, &'a SessionRecord)> {
    let mut out = Vec::new();
    for rec in &store.sessions {
        if rec.id == session.as_str() {
            continue;
        }
        for touch in &rec.touched {
            let at = parse_ts(&touch.at);
            let include = match (last_active, at) {
                (None, _) => true,
                (Some(mine), Some(theirs)) => theirs > mine,
                (Some(_), None) => true,
            };
            if include {
                out.push((touch.path.as_str(), rec));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(b.0));
    out
}

fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn short_when(at: DateTime<Utc>) -> String {
    at.format("%d/%m %H:%M").to_string()
}

fn trim_to_cap(text: &mut String, cap: usize) {
    if estimate_tokens(text) <= cap {
        return;
    }
    let max_chars = cap.saturating_mul(4);
    let cutoff = max_chars.saturating_sub(80);
    if text.chars().count() <= max_chars {
        return;
    }
    // Cut on the last newline before the cutoff so we never split a decision or
    // finding mid-entry: the kept text always ends at a complete line. Fall back
    // to the raw character cutoff if there is no earlier newline.
    let kept_upto: String = text.chars().take(cutoff).collect();
    let end_line = kept_upto.rfind('\n').map(|i| i + 1).unwrap_or(cutoff);
    let kept: String = text.chars().take(end_line).collect();
    *text =
        format!("{kept}\n[digest truncated to stay under {cap} tokens]\n=== END OF CONTEXT ===\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::store::{Decision, OrbitStore, SessionRecord, TaskStatus, TouchedFile};
    use crate::session::SessionId;
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    fn root() -> (TempDir, OrbitStore) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let store = OrbitStore::open(&root);
        (tmp, store)
    }

    #[test]
    fn handoff_matches_the_injected_digest_section() {
        let (_tmp, mut store) = root();
        store.sessions = vec![
            SessionRecord {
                id: "aaa".into(),
                label: "implementation".into(),
                model: "gpt-5.6-sol".into(),
                last_active_at: Some("2026-08-12T15:00:00Z".into()),
                touched: vec![TouchedFile {
                    path: "src/auth/token.rs".into(),
                    at: "2026-08-12T14:50:00Z".into(),
                }],
            },
            SessionRecord {
                id: "bbb".into(),
                label: "review".into(),
                model: "claude-opus-5".into(),
                last_active_at: Some("2026-08-12T14:00:00Z".into()),
                touched: vec![],
            },
        ];
        store.decisions.push(Decision {
            at: Utc.with_ymd_and_hms(2026, 8, 12, 14, 40, 0).unwrap(),
            model: "gpt-5.6-sol".into(),
            session: "implementation".into(),
            role: "Coder".into(),
            decision: "Use rusqlite.".into(),
            rationale: String::new(),
            files: Vec::new(),
            pinned: false,
        });
        let digest = build_digest(&store, &SessionId::new("bbb"), "Orbit");
        let handoff = build_handoff(&store, &SessionId::new("bbb"));
        assert!(handoff.is_interesting());
        assert!(digest.text.contains(handoff.digest_section.trim()));
        assert_eq!(handoff.new_decisions, 1);
        assert!(handoff.banner.contains("implementation"));
        assert!(handoff.digest_section.contains("src/auth/token.rs"));
    }

    #[test]
    fn other_session_changes_appear_for_idle_session() {
        let (_tmp, mut store) = root();
        store.sessions = vec![
            SessionRecord {
                id: "aaa".into(),
                label: "implementation".into(),
                model: "gpt-5.6-sol".into(),
                last_active_at: Some("2026-08-12T15:00:00Z".into()),
                touched: vec![TouchedFile {
                    path: "src/auth/token.rs".into(),
                    at: "2026-08-12T14:50:00Z".into(),
                }],
            },
            SessionRecord {
                id: "bbb".into(),
                label: "review".into(),
                model: "claude-opus-5".into(),
                last_active_at: Some("2026-08-12T14:00:00Z".into()),
                touched: vec![],
            },
        ];
        let digest = build_digest(&store, &SessionId::new("bbb"), "Orbit");
        assert!(digest.text.contains("src/auth/token.rs"), "{}", digest.text);
        assert!(digest.text.contains("implementation"));
        assert!(digest.text.contains("gpt-5.6-sol"));
    }

    #[test]
    fn own_touches_are_not_listed_as_foreign() {
        let (_tmp, mut store) = root();
        store.sessions = vec![SessionRecord {
            id: "bbb".into(),
            label: "review".into(),
            model: "claude-opus-5".into(),
            last_active_at: None,
            touched: vec![TouchedFile {
                path: "src/mine.rs".into(),
                at: "2026-08-12T14:50:00Z".into(),
            }],
        }];
        let digest = build_digest(&store, &SessionId::new("bbb"), "Orbit");
        assert!(!digest.text.contains("src/mine.rs"));
    }

    #[test]
    fn hand_edited_decisions_show_after_reload() {
        let (tmp, mut store) = root();
        let path = store.dir.join("decisions.md");
        std::fs::write(
            &path,
            "## 2026-08-12T13:10:00Z — gpt-5.6-sol (session \"implementation\")\n\
             **Decision:** SQLite via rusqlite, not sqlx.\n",
        )
        .unwrap();
        store.reload();
        let digest = build_digest(&store, &SessionId::new("bbb"), "Orbit");
        assert!(digest.text.contains("SQLite via rusqlite"));
        let _ = tmp;
    }

    #[test]
    fn digest_respects_token_cap() {
        let (_tmp, mut store) = root();
        store.settings.token_cap = 200;
        store.context_md = "word ".repeat(4000);
        store.decisions = (0..20)
            .map(|i| Decision {
                at: Utc.with_ymd_and_hms(2026, 8, 12, 10, i, 0).unwrap(),
                model: "m".into(),
                session: "s".into(),
                role: "Coder".into(),
                decision: "x".repeat(200),
                rationale: String::new(),
                files: Vec::new(),
                pinned: false,
            })
            .collect();
        let digest = build_digest(&store, &SessionId::new("bbb"), "Orbit");
        assert!(
            digest.token_estimate <= 200 + 20,
            "{}",
            digest.token_estimate
        );
        assert!(digest.text.contains("truncated") || digest.token_estimate <= 200);
    }

    #[test]
    fn trim_never_ends_mid_line() {
        // A trimmed digest must not end with a partial entry line: the text
        // always stops at a newline before the truncation marker.
        let mut text = String::new();
        for i in 0..40 {
            text.push_str(&format!("line {i}: {}\n", "word ".repeat(30)));
        }
        trim_to_cap(&mut text, 60);
        let before_marker = text
            .split("[digest truncated")
            .next()
            .unwrap_or_default()
            .to_string();
        // The last line before the marker should be a complete entry ending in \n
        assert!(
            before_marker.ends_with('\n'),
            "trimmed digest must end with a complete line, got: …{before_marker:?}"
        );
        assert!(
            !before_marker.ends_with("word"),
            "partial line leaked through"
        );
    }

    #[test]
    fn coder_digest_includes_planner_tasks_decision_and_acs() {
        let (tmp, mut store) = root();
        let root = tmp.path().join("proj");
        crate::pipeline::contract::ContractStore::open(&root)
            .write_planner(&crate::pipeline::contract::PlannerOutput {
                tasks: vec!["index chats".into()],
                decision: "Reuse the FTS table.".into(),
                acceptance_criteria: vec![crate::pipeline::contract::AcceptanceCriterion {
                    id: "AC1".into(),
                    text: "Ctrl+K focuses search".into(),
                }],
                scope: "sidebar".into(),
                non_goals: "folders".into(),
            })
            .unwrap();
        store
            .append_decision(Decision {
                at: Utc::now(),
                model: "planner".into(),
                session: "plan".into(),
                role: "Architect".into(),
                decision: "Reuse the FTS table.".into(),
                rationale: String::new(),
                files: Vec::new(),
                pinned: true,
            })
            .unwrap();
        store
            .upsert_task(None, TaskStatus::Open, "index chats".into())
            .unwrap();
        let digest = build_digest(&store, &SessionId::new("coder"), "orbit");
        assert!(digest.text.contains("Reuse the FTS table."));
        assert!(digest.text.contains("index chats"));
        assert!(digest.text.contains("[AC1] Ctrl+K focuses search"));
    }

    fn push_skill(store: &mut OrbitStore, slug: &str, description: &str, body: &str) {
        store.skills.push(crate::context::Skill {
            slug: slug.into(),
            name: slug.into(),
            description: description.into(),
            body: body.into(),
            path: store.dir.join("skills").join(slug).join("SKILL.md"),
        });
    }

    #[test]
    fn digest_lists_skill_names_not_bodies() {
        let (_tmp, mut store) = root();
        push_skill(
            &mut store,
            "run-tests",
            "How to run the suite.",
            "SECRET BODY cargo test --all-targets",
        );
        let digest = build_digest(&store, &SessionId::new("bbb"), "Orbit");
        assert!(digest.text.contains("Available skills (1):"));
        assert!(digest.text.contains("- run-tests: How to run the suite."));
        assert!(!digest.text.contains("SECRET BODY"));
    }

    #[test]
    fn twenty_skills_add_about_one_line_each() {
        let (_tmp, mut store) = root();
        let before = build_digest(&store, &SessionId::new("bbb"), "Orbit");
        let before_lines = before.text.lines().count();
        for i in 0..20 {
            push_skill(
                &mut store,
                &format!("skill-{i:02}"),
                &format!("Description of skill {i}."),
                &format!("BODY of skill {i} that must not appear in the digest."),
            );
        }
        let after = build_digest(&store, &SessionId::new("bbb"), "Orbit");
        let added = after.text.lines().count().saturating_sub(before_lines);
        let skill_lines = after
            .text
            .lines()
            .filter(|l| l.starts_with("- skill-"))
            .count();
        assert_eq!(skill_lines, 20);
        assert!(
            (18..=22).contains(&added),
            "expected ~1 line per skill, grew by {added} lines"
        );
        assert!(after.text.contains("Available skills (20):"));
        assert!(!after.text.contains("BODY of skill"));
    }

    #[test]
    fn digest_caps_listed_skills() {
        let (_tmp, mut store) = root();
        store.settings.max_skills = 3;
        for i in 0..5 {
            push_skill(
                &mut store,
                &format!("skill-{i}"),
                "desc",
                "body that stays out",
            );
        }
        let digest = build_digest(&store, &SessionId::new("bbb"), "Orbit");
        assert!(digest.text.contains("Available skills (5, showing 3):"));
        assert!(digest.text.contains("2 more skills not listed"));
        assert!(!digest.text.contains("body that stays out"));
    }
}
