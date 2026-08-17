//! Dynamic MCP tools exposed to the model as `mcp__<server>__<tool>`.

use super::client::McpClient;
use crate::tools::{Tool, ToolContext, ToolError, ToolOutcome, ToolRisk, truncate_output};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub fn qualified_name(server: &str, remote: &str) -> String {
    format!("mcp__{server}__{remote}")
}

pub struct McpTool {
    pub qualified: String,
    pub remote_name: String,
    pub description: String,
    pub schema: Value,
    pub risk: ToolRisk,
    client: Arc<McpClient>,
}

impl McpTool {
    pub fn new(
        server: &str,
        remote: super::client::RemoteTool,
        risk: ToolRisk,
        client: Arc<McpClient>,
    ) -> Self {
        Self {
            qualified: qualified_name(server, &remote.name),
            remote_name: remote.name,
            description: if remote.description.is_empty() {
                format!("MCP tool from `{server}`")
            } else {
                remote.description
            },
            schema: if remote.schema.is_null() {
                serde_json::json!({"type":"object","properties":{}})
            } else {
                remote.schema
            },
            risk,
            client,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.qualified
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn risk(&self) -> ToolRisk {
        self.risk
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutcome, ToolError> {
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::Message("cancelled".into()));
        }
        if self.client.is_dead() {
            return Err(ToolError::Message(format!(
                "MCP server `{}` is not running",
                self.client.name
            )));
        }
        match self.client.call_tool(&self.remote_name, args).await {
            Ok(text) => Ok(truncate_output(text)),
            Err(e) => Err(ToolError::Message(e.to_string())),
        }
    }
}
