//! Procedural skills stored as `.orbit/skills/<slug>/SKILL.md`.
//!
//! Hand-edited files must never crash the app: a missing or malformed
//! frontmatter produces a warning and the skill is skipped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const SKILLS_DIR: &str = "skills";
pub const SKILL_FILE: &str = "SKILL.md";
pub const SLUG_MAX_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: PathBuf,
}

impl Skill {
    pub fn relative_path(&self) -> PathBuf {
        PathBuf::from(format!(".orbit/{SKILLS_DIR}/{}/{SKILL_FILE}", self.slug))
    }
}

pub fn is_valid_slug(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= SLUG_MAX_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Scan `.orbit/skills/*/SKILL.md`. Corrupt or incomplete files become
/// warnings; they never panic.
pub fn load_all(dir: &Path) -> (Vec<Skill>, Vec<String>) {
    let mut skills = Vec::new();
    let mut warnings = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (skills, warnings);
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for skill_dir in dirs {
        let slug = skill_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if slug.is_empty() || slug.starts_with('.') {
            continue;
        }
        let path = skill_dir.join(SKILL_FILE);
        if !path.is_file() {
            warnings.push(format!("skill `{slug}` has no {SKILL_FILE}; ignored"));
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("could not read skill `{slug}`: {e}"));
                continue;
            }
        };
        match parse_skill(&slug, &path, &text) {
            Ok(skill) => skills.push(skill),
            Err(w) => warnings.push(w),
        }
    }
    (skills, warnings)
}

pub fn render_skill_file(name: &str, description: &str, body: &str) -> String {
    let desc = yaml_escape(&description.replace(['\r', '\n'], " "));
    let name = yaml_escape(name);
    format!(
        "---\nname: {name}\ndescription: {desc}\n---\n\n{}\n",
        body.trim()
    )
}

fn parse_skill(slug: &str, path: &Path, text: &str) -> Result<Skill, String> {
    let (front, body) = split_frontmatter(text)
        .ok_or_else(|| format!("skill `{slug}` is missing YAML frontmatter; ignored"))?;
    let fields = parse_simple_yaml(front)
        .map_err(|e| format!("skill `{slug}` has malformed frontmatter ({e}); ignored"))?;
    let name = fields
        .get("name")
        .cloned()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("skill `{slug}` is missing `name`; ignored"))?;
    let description = fields
        .get("description")
        .cloned()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("skill `{slug}` is missing `description`; ignored"))?;
    Ok(Skill {
        slug: slug.to_string(),
        name,
        description,
        body: body.trim().to_string(),
        path: path.to_path_buf(),
    })
}

fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))?;
    let mut pos = 0;
    while pos <= rest.len() {
        let next_nl = rest[pos..].find('\n').map(|i| pos + i);
        let line_end = next_nl.unwrap_or(rest.len());
        let line = rest[pos..line_end].trim_end_matches('\r');
        if line == "---" {
            let body_start = next_nl.map(|i| i + 1).unwrap_or(rest.len());
            return Some((&rest[..pos], &rest[body_start..]));
        }
        match next_nl {
            Some(i) => pos = i + 1,
            None => break,
        }
    }
    None
}

fn parse_simple_yaml(front: &str) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    for (i, raw) in front.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("line {}: expected key: value", i + 1));
        };
        let key = key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err(format!("line {}: invalid key", i + 1));
        }
        map.insert(key.to_string(), unquote(value.trim()));
    }
    Ok(map)
}

fn unquote(s: &str) -> String {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn yaml_escape(value: &str) -> String {
    if value.is_empty()
        || value
            .chars()
            .any(|c| matches!(c, ':' | '#' | '"' | '\'' | '{' | '}' | '[' | ']'))
        || value.starts_with([' ', '*', '&', '!', '%', '@', '`'])
    {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_skill(root: &Path, slug: &str, contents: &str) {
        let dir = root.join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE), contents).unwrap();
    }

    #[test]
    fn load_all_reads_valid_skill() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "rodar-testes-de-integracao",
            "---\n\
             name: rodar-testes-de-integracao\n\
             description: How to run the integration suite.\n\
             ---\n\
             \n\
             Use the ephemeral database.\n",
        );
        let (skills, warnings) = load_all(tmp.path());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].slug, "rodar-testes-de-integracao");
        assert_eq!(skills[0].name, "rodar-testes-de-integracao");
        assert_eq!(skills[0].description, "How to run the integration suite.");
        assert_eq!(skills[0].body, "Use the ephemeral database.");
    }

    #[test]
    fn missing_frontmatter_is_a_warning() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "broken", "just a markdown file\n");
        let (skills, warnings) = load_all(tmp.path());
        assert!(skills.is_empty());
        assert!(
            warnings.iter().any(|w| w.contains("frontmatter")),
            "{warnings:?}"
        );
    }

    #[test]
    fn malformed_frontmatter_is_a_warning() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "broken",
            "---\nthis is not yaml at all\n---\nbody\n",
        );
        let (skills, warnings) = load_all(tmp.path());
        assert!(skills.is_empty());
        assert!(
            warnings.iter().any(|w| w.contains("malformed")),
            "{warnings:?}"
        );
    }

    #[test]
    fn quoted_fields_round_trip() {
        let rendered = render_skill_file(
            "deploy",
            "Ship to staging: the canary box",
            "Run `just ship`.",
        );
        let parsed = parse_skill("deploy", Path::new("SKILL.md"), &rendered).unwrap();
        assert_eq!(parsed.name, "deploy");
        assert_eq!(parsed.description, "Ship to staging: the canary box");
        assert_eq!(parsed.body, "Run `just ship`.");
    }

    #[test]
    fn slug_rejects_uppercase_and_oversize() {
        assert!(is_valid_slug("run-tests"));
        assert!(is_valid_slug("a"));
        assert!(!is_valid_slug("Run-Tests"));
        assert!(!is_valid_slug("run_tests"));
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug(&"a".repeat(SLUG_MAX_LEN + 1)));
        assert!(is_valid_slug(&"a".repeat(SLUG_MAX_LEN)));
    }
}
