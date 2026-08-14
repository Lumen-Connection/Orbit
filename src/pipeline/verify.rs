//! Deterministic verification between Coder and Reviewer (N3.8).

use crate::workspace::run_config::RunConfig;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyCommand {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyStep {
    pub name: String,
    pub passed: bool,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerifyReport {
    pub steps: Vec<VerifyStep>,
}

impl VerifyReport {
    pub fn passed(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|s| s.passed)
    }

    pub fn summary(&self) -> String {
        if self.steps.is_empty() {
            return "No verification commands planned.".into();
        }
        self.steps
            .iter()
            .map(|s| {
                let mark = if s.passed { "PASS" } else { "FAIL" };
                format!("[{mark}] {}: {}", s.name, truncate(&s.output, 400))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn plan_verify_commands(root: &Path, configs: &[RunConfig]) -> Vec<VerifyCommand> {
    if root.join("Cargo.toml").exists() {
        return vec![
            VerifyCommand {
                name: "fmt".into(),
                program: "cargo".into(),
                args: vec!["fmt".into(), "--all".into(), "--check".into()],
            },
            VerifyCommand {
                name: "clippy".into(),
                program: "cargo".into(),
                args: vec![
                    "clippy".into(),
                    "--all-targets".into(),
                    "--".into(),
                    "-D".into(),
                    "warnings".into(),
                ],
            },
            VerifyCommand {
                name: "test".into(),
                program: "cargo".into(),
                args: vec!["test".into(), "--all-targets".into()],
            },
        ];
    }
    let mut out = Vec::new();
    for cfg in configs {
        let key = format!("{} {}", cfg.name, cfg.program).to_ascii_lowercase();
        let name = if key.contains("fmt") || key.contains("format") {
            "fmt"
        } else if key.contains("lint") || key.contains("clippy") || key.contains("vet") {
            "lint"
        } else if key.contains("test") {
            "test"
        } else {
            continue;
        };
        let mut args = cfg.args.clone();
        if name == "fmt" && cfg.program == "cargo" && !args.iter().any(|a| a == "--check") {
            args.push("--check".into());
        }
        out.push(VerifyCommand {
            name: name.into(),
            program: cfg.program.clone(),
            args,
        });
    }
    out
}

pub trait CommandRunner {
    fn run(&self, program: &str, args: &[String], cwd: &Path) -> VerifyStep;
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[String], cwd: &Path) -> VerifyStep {
        let output = Command::new(program).args(args).current_dir(cwd).output();
        match output {
            Ok(out) => {
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                if !out.stderr.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&String::from_utf8_lossy(&out.stderr));
                }
                VerifyStep {
                    name: program.to_string(),
                    passed: out.status.success(),
                    output: text,
                }
            }
            Err(e) => VerifyStep {
                name: program.to_string(),
                passed: false,
                output: format!("failed to start {program}: {e}"),
            },
        }
    }
}

pub fn run_verify(
    commands: &[VerifyCommand],
    runner: &dyn CommandRunner,
    cwd: &Path,
) -> VerifyReport {
    let mut report = VerifyReport::default();
    for cmd in commands {
        let mut step = runner.run(&cmd.program, &cmd.args, cwd);
        step.name = cmd.name.clone();
        report.steps.push(step);
    }
    report
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.trim().to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::run_config::{RunConfig, RunKind};
    use std::collections::HashMap;

    struct Scripted(HashMap<String, VerifyStep>);

    impl CommandRunner for Scripted {
        fn run(&self, program: &str, args: &[String], _cwd: &Path) -> VerifyStep {
            let key = format!("{program} {}", args.join(" "));
            self.0.get(&key).cloned().unwrap_or(VerifyStep {
                name: program.into(),
                passed: false,
                output: format!("unexpected {key}"),
            })
        }
    }

    #[test]
    fn rust_project_plans_the_triad() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let cmds = plan_verify_commands(tmp.path(), &[]);
        assert_eq!(
            cmds.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["fmt", "clippy", "test"]
        );
        assert!(cmds[1].args.contains(&"-D".into()));
    }

    #[test]
    fn clippy_failure_is_visible_without_running_the_reviewer() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let cmds = plan_verify_commands(tmp.path(), &[]);
        let mut map = HashMap::new();
        map.insert(
            "cargo fmt --all --check".into(),
            VerifyStep {
                name: "fmt".into(),
                passed: true,
                output: String::new(),
            },
        );
        map.insert(
            "cargo clippy --all-targets -- -D warnings".into(),
            VerifyStep {
                name: "clippy".into(),
                passed: false,
                output: "error: unused variable `x`".into(),
            },
        );
        map.insert(
            "cargo test --all-targets".into(),
            VerifyStep {
                name: "test".into(),
                passed: true,
                output: "ok".into(),
            },
        );
        let report = run_verify(&cmds, &Scripted(map), tmp.path());
        assert!(!report.passed());
        let summary = report.summary();
        assert!(summary.contains("[FAIL] clippy"));
        assert!(summary.contains("unused variable"));
    }

    #[test]
    fn non_rust_uses_matching_run_configs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let configs = vec![RunConfig {
            id: "t".into(),
            name: "test".into(),
            program: "npm".into(),
            args: vec!["test".into()],
            env: Vec::new(),
            cwd: None,
            kind: RunKind::OneShot,
        }];
        let cmds = plan_verify_commands(tmp.path(), &configs);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].program, "npm");
    }
}
