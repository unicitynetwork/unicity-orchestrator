// Core modules
mod config;
mod mcp_client;
mod db;
mod knowledge_graph;
pub mod api;
mod executor;

// New modular structure
mod orchestrator;
mod tools;
mod prompts;
mod resources;
pub mod server;

// Re-export key types and functions
pub use db::{DatabaseConfig, create_connection, ensure_schema, ToolRecord};
pub use knowledge_graph::{KnowledgeGraph, EmbeddingManager};
pub use config::McpServiceConfig;

// Re-export from new modular structure
pub use orchestrator::{Orchestrator, PlanStep, PlanResult};
pub use tools::{ToolHandler, ToolRegistry};
pub use server::McpServer;

use std::sync::Arc;
use anyhow::Result;
use tools::{SelectToolHandler, PlanToolsHandler, ExecuteToolHandler};

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
        .register_handler(ExecuteToolHandler::new(orchestrator.clone()));

    let tool_registry = Arc::new(tool_registry);

    // Create the server
    let server = McpServer::new(orchestrator, tool_registry);

    Ok(Arc::new(server))
}
