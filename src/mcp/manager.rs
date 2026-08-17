//! Start, stop and expose MCP servers for a project.

use super::client::{McpClient, RemoteTool};
use super::tool::McpTool;
use super::trust::{self, McpServerConfig};
use crate::session::roles::AgentRole;
use crate::tools::{ToolRegistry, ToolRisk};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    Stopped,
    NeedsTrust,
    Denied,
    Running,
    Failed(String),
}

pub struct ServerState {
    pub config: McpServerConfig,
    pub status: ServerStatus,
    pub tools: Vec<RemoteTool>,
    client: Option<Arc<McpClient>>,
}

impl ServerState {
    pub fn tool_risk(&self, remote: &str) -> ToolRisk {
        let qualified = super::tool::qualified_name(&self.config.name, remote);
        trust::risk_override(&qualified).unwrap_or(ToolRisk::Executing)
    }
}

#[derive(Default)]
pub struct McpManager {
    pub servers: Vec<ServerState>,
}

impl McpManager {
    pub fn load(root: &Path) -> Self {
        let servers = trust::load_servers(root)
            .into_iter()
            .map(|config| {
                let status = if config.is_denied() {
                    ServerStatus::Denied
                } else if !config.enabled {
                    ServerStatus::Stopped
                } else if !trust::is_trusted(&config) {
                    ServerStatus::NeedsTrust
                } else {
                    ServerStatus::Stopped
                };
                ServerState {
                    config,
                    status,
                    tools: Vec::new(),
                    client: None,
                }
            })
            .collect();
        Self { servers }
    }

    pub async fn start_trusted(&mut self) {
        for server in &mut self.servers {
            if server.status == ServerStatus::Stopped
                && server.config.enabled
                && !server.config.is_denied()
                && trust::is_trusted(&server.config)
            {
                start_server(server).await;
            }
        }
    }

    pub async fn trust_and_start(&mut self, name: &str) -> Result<(), String> {
        let server = self
            .servers
            .iter_mut()
            .find(|s| s.config.name == name)
            .ok_or_else(|| format!("unknown MCP server `{name}`"))?;
        if server.config.is_denied() {
            return Err("blocked by the command denylist".into());
        }
        trust::trust_on_this_machine(&server.config)?;
        server.config.enabled = true;
        start_server(server).await;
        match &server.status {
            ServerStatus::Running => Ok(()),
            ServerStatus::Failed(e) => Err(e.clone()),
            other => Err(format!("server stayed {other:?}")),
        }
    }

    pub async fn shutdown(&mut self) {
        for server in &mut self.servers {
            if let Some(client) = server.client.take() {
                client.shutdown().await;
            }
            if !matches!(
                server.status,
                ServerStatus::Denied | ServerStatus::NeedsTrust
            ) {
                server.status = ServerStatus::Stopped;
            }
            server.tools.clear();
        }
    }

    pub fn attach(&self, registry: &mut ToolRegistry, role: AgentRole) {
        for server in &self.servers {
            let Some(client) = &server.client else {
                continue;
            };
            if server.status != ServerStatus::Running {
                continue;
            }
            for remote in &server.tools {
                let risk = server.tool_risk(&remote.name);
                if !role.allows_risk(risk) {
                    continue;
                }
                registry.register(Arc::new(McpTool::new(
                    &server.config.name,
                    remote.clone(),
                    risk,
                    client.clone(),
                )));
            }
        }
    }
}

async fn start_server(server: &mut ServerState) {
    match McpClient::spawn(&server.config).await {
        Ok(client) => {
            if let Err(e) = client.initialize().await {
                client.shutdown().await;
                server.status = ServerStatus::Failed(e.to_string());
                server.client = None;
                return;
            }
            match client.list_tools().await {
                Ok(tools) => {
                    server.tools = tools;
                    server.client = Some(client);
                    server.status = ServerStatus::Running;
                }
                Err(e) => {
                    client.shutdown().await;
                    server.status = ServerStatus::Failed(e.to_string());
                    server.client = None;
                }
            }
        }
        Err(e) => {
            server.status = ServerStatus::Failed(e.to_string());
            server.client = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::roles::AgentRole;
    use crate::tools::ToolRisk;

    #[test]
    fn architect_does_not_receive_executing_mcp_tools() {
        let registry = ToolRegistry::new();
        let remote = RemoteTool {
            name: "write_something".into(),
            description: "writes".into(),
            schema: serde_json::json!({"type":"object"}),
        };
        // No live client: attach skips servers without a client. The risk
        // filter is what we check here via allows_risk.
        assert!(!AgentRole::Architect.allows_risk(ToolRisk::Executing));
        assert!(AgentRole::Coder.allows_risk(ToolRisk::Executing));
        let _ = (registry, remote);
    }
}
