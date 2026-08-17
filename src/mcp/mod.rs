//! MCP client: stdio + JSON-RPC 2.0, project-scoped servers.

pub mod client;
pub mod manager;
pub mod tool;
pub mod trust;

pub use manager::{McpManager, ServerStatus};
