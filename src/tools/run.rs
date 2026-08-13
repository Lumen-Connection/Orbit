//! Agent tools for run configs and live process output.

use super::{Tool, ToolContext, ToolError, ToolOutcome, ToolRisk};
use crate::workspace::run_config::RunConfig;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

pub const STARTS_PER_TURN: u32 = 3;

pub fn record_start(starts: &Mutex<HashMap<String, u32>>, id: &str) -> Result<(), String> {
    let mut map = starts
        .lock()
        .map_err(|_| "run-start lock poisoned".to_string())?;
    let count = map.entry(id.to_string()).or_insert(0);
    *count += 1;
    if *count > STARTS_PER_TURN {
        return Err(format!(
            "Refusing to start `{id}` again: {STARTS_PER_TURN} start/restart limit for this turn."
        ));
    }
    Ok(())
}

pub struct ListRunConfigs;

#[async_trait]
impl Tool for ListRunConfigs {
    fn name(&self) -> &'static str {
        "list_run_configs"
    }

    fn description(&self) -> &'static str {
        "List saved and suggested run configs for this project."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {}, "required": [] })
    }

    fn risk(&self) -> ToolRisk {
        ToolRisk::ReadOnly
    }

    async fn execute(
        &self,
        _args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let Some(configs) = &ctx.run_configs else {
            return Err(ToolError::Message("no run configs available".into()));
        };
        if configs.is_empty() {
            return Ok(ToolOutcome {
                content: "No run configs.".into(),
                truncated: false,
            });
        }
        let mut body = String::new();
        for cfg in configs {
            body.push_str(&format!(
                "- {} [{}] {} ({})\n",
                cfg.id,
                match cfg.kind {
                    crate::workspace::run_config::RunKind::OneShot => "one-shot",
                    crate::workspace::run_config::RunKind::LongRunning => "long-running",
                },
                cfg.name,
                cfg.display()
            ));
        }
        Ok(super::truncate_output(body))
    }
}

pub struct ReadRunOutput;

#[async_trait]
impl Tool for ReadRunOutput {
    fn name(&self) -> &'static str {
        "read_run_output"
    }

    fn description(&self) -> &'static str {
        "Read the last N lines of a long-running process buffer (default 200, max 1000)."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "config_id": { "type": "string" },
                "lines": { "type": "integer", "minimum": 1, "maximum": 1000 }
            },
            "required": ["config_id"]
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
        let id = args
            .get("config_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing `config_id`".into()))?;
        let n = args
            .get("lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(200)
            .clamp(1, 1000) as usize;
        let Some(runner) = &ctx.runner else {
            return Err(ToolError::Message("no process registry".into()));
        };
        let runner = runner
            .lock()
            .map_err(|_| ToolError::Message("runner lock poisoned".into()))?;
        let Some((lines, truncated)) = runner.last_lines(id, n) else {
            return Ok(ToolOutcome {
                content: format!("No process output for `{id}`."),
                truncated: false,
            });
        };
        let mut body = lines.join("\n");
        if truncated {
            body.push_str("\n[truncated]");
        }
        Ok(super::truncate_output(body))
    }
}

pub struct StartRun;

#[async_trait]
impl Tool for StartRun {
    fn name(&self) -> &'static str {
        "start_run"
    }

    fn description(&self) -> &'static str {
        "Start a named run config. Long-running configs stay up until stop_run."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "config_id": { "type": "string" } },
            "required": ["config_id"]
        })
    }

    fn risk(&self) -> ToolRisk {
        ToolRisk::Executing
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        if !ctx.allow_execute {
            return Err(ToolError::Message("start_run was not approved".into()));
        }
        let id = args
            .get("config_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing `config_id`".into()))?;
        if let Some(starts) = &ctx.run_starts
            && let Err(msg) = record_start(starts, id)
        {
            return Err(ToolError::Message(msg));
        }
        let config = lookup_config(ctx, id)?;
        let project = ctx
            .project
            .as_ref()
            .ok_or_else(|| ToolError::Message("no project is open".into()))?;
        let runner = ctx
            .runner
            .as_ref()
            .ok_or_else(|| ToolError::Message("no process registry".into()))?;
        let mut runner = runner
            .lock()
            .map_err(|_| ToolError::Message("runner lock poisoned".into()))?;
        if runner.is_running(&config.id) {
            runner.request_restart(config.clone(), project.canonical_root.clone(), None);
            return Ok(ToolOutcome {
                content: format!("Restarting `{}`.", config.display()),
                truncated: false,
            });
        }
        runner
            .start(config.clone(), project.canonical_root.clone(), None)
            .map_err(|e| ToolError::Message(e.to_string()))?;
        Ok(ToolOutcome {
            content: format!("Started `{}`.", config.display()),
            truncated: false,
        })
    }
}

pub struct StopRun;

#[async_trait]
impl Tool for StopRun {
    fn name(&self) -> &'static str {
        "stop_run"
    }

    fn description(&self) -> &'static str {
        "Stop a running process started from a run config."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "config_id": { "type": "string" } },
            "required": ["config_id"]
        })
    }

    fn risk(&self) -> ToolRisk {
        ToolRisk::Executing
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        if !ctx.allow_execute {
            return Err(ToolError::Message("stop_run was not approved".into()));
        }
        let id = args
            .get("config_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing `config_id`".into()))?;
        let runner = ctx
            .runner
            .as_ref()
            .ok_or_else(|| ToolError::Message("no process registry".into()))?;
        runner
            .lock()
            .map_err(|_| ToolError::Message("runner lock poisoned".into()))?
            .stop(id);
        Ok(ToolOutcome {
            content: format!("Stop requested for `{id}`."),
            truncated: false,
        })
    }
}

fn lookup_config(ctx: &ToolContext, id: &str) -> Result<RunConfig, ToolError> {
    ctx.run_configs
        .as_ref()
        .and_then(|cfgs| cfgs.iter().find(|c| c.id == id || c.name == id).cloned())
        .ok_or_else(|| ToolError::Message(format!("unknown run config `{id}`")))
}

#[cfg(test)]
mod tests {
    use super::{STARTS_PER_TURN, record_start};
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[test]
    fn anti_loop_trips_on_the_fourth_start() {
        let starts = Mutex::new(HashMap::new());
        for _ in 0..STARTS_PER_TURN {
            assert!(record_start(&starts, "dev").is_ok());
        }
        let err = record_start(&starts, "dev").unwrap_err();
        assert!(err.contains("limit"));
    }
}
