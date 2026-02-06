// Core modules
pub mod api;
mod config;
pub mod db;
mod executor;
mod knowledge_graph;
mod mcp_client;

// NewType wrappers for strong typing
pub mod types;

// New modular structure
pub mod auth;
mod elicitation;
mod orchestrator;
mod prompts;
mod resources;
pub mod server;
mod tools;

// Re-export key types and functions
pub use auth::{AuthConfig, UserContext, generate_api_key, hash_api_key};
pub use config::McpServiceConfig;
pub use db::{DatabaseConfig, ToolRecord, create_connection, ensure_schema};
pub use knowledge_graph::{EmbeddingManager, KnowledgeGraph};
pub use types::{
    ApiKeyHash, ApiKeyPrefix, ExternalUserId, IdentityProvider, OAuthUrl, PromptName, RedirectUri,
    ResourceUri, ServiceConfigId, ServiceId, ServiceName, ToolId, ToolName,
};

// Re-export from new modular structure
pub use orchestrator::{Orchestrator, PlanResult, PlanStep};
pub use server::McpServer;
pub use tools::{ToolHandler, ToolRegistry};

use anyhow::Result;
use std::sync::Arc;
use tools::{ExecuteToolHandler, ListDiscoveredToolsHandler, PlanToolsHandler, SelectToolHandler};

/// Convenience function to create a fully configured MCP server.
///
/// This creates the Orchestrator, registers the default tools, and returns
/// a McpServer that implements rmcp's ServerHandler.
pub async fn create_server(config: DatabaseConfig) -> Result<Arc<McpServer>> {
    // Create the orchestrator
    let mut orchestrator = Orchestrator::new(config).await?;
    orchestrator.initialize().await?;
    let orchestrator = Arc::new(orchestrator);

    // Create and configure the tool registry
    let tool_registry = ToolRegistry::new()
        .register_handler(SelectToolHandler::new(orchestrator.clone()))
        .register_handler(PlanToolsHandler::new(orchestrator.clone()))
        .register_handler(ExecuteToolHandler::new(orchestrator.clone()))
        .register_handler(ListDiscoveredToolsHandler::new(orchestrator.clone()));

    let tool_registry = Arc::new(tool_registry);

    // Create the server
    let server = McpServer::new(orchestrator, tool_registry);

    Ok(Arc::new(server))
}
