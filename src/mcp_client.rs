// MCP client implementation backed by rmcp

use std::borrow::Cow;
use crate::config::McpServiceConfig;
use anyhow::Result;
use rmcp::{
    model::{Tool as McpTool, ServerInfo},
    service::{RunningService as RmcpRunningService, RoleClient},
    ServiceExt,
    transport::{TokioChildProcess, ConfigureCommandExt},
};
use rmcp::model::{CallToolRequestParam, Content, JsonObject};
use surrealdb::RecordId;
use tokio::process::Command;
use tracing::{info, warn};

/// Wrapper for a running MCP service client.
///
/// This holds the SurrealDB id for the service as well as the rmcp `RunningService`
/// handle used to talk MCP (initialize, list_tools, call_tool, etc.).
pub struct RunningService {
    pub service_id: RecordId,
    pub client: RmcpRunningService<RoleClient, ()>,
}

pub async fn start_stdio_service(cfg: &McpServiceConfig) -> Result<Option<RunningService>> {
    if let McpServiceConfig::Stdio { id, disabled, .. } = cfg {
        if *disabled {
            info!("Skipping disabled MCP service `{id}`");
            return Ok(None);
        }

        info!("Starting MCP stdio service `{id}` via rmcp");

        // NOTE: For now we assume that `id` is the command to run. If your
        // McpServiceConfig has an explicit command / args field, wire that
        // in here instead of using `id` directly.
        let child = TokioChildProcess::new(Command::new(id).configure(|_cmd| {
            // TODO: attach additional args / env from McpServiceConfig when available.
        }))?;

        // Start the rmcp client service over stdio and complete MCP initialization.
        // This returns rmcp's `RunningService<RoleClient, ()>` which we store.
        let client = ().serve(child).await?;

        // Use a typed SurrealDB RecordId for this service.
        let service_id = RecordId::from(("service", id.clone()));

        Ok(Some(RunningService { service_id, client }))
    } else {
        Ok(None)
    }
}

/// Fetch service info + tools and normalize them using rmcp.
pub async fn inspect_service(
    running: &RunningService,
) -> Result<(ServerInfo, Vec<McpTool>)> {
    // Basic metadata about the server from the MCP `initialize` handshake.
    // `peer_info` returns an Option<&ServerInfo>, so fall back to a minimal
    // placeholder if for some reason it is not set.
    let server_info = running
        .client
        .peer_info()
        .cloned()
        .unwrap_or_else(|| ServerInfo::default());

    // List tools via rmcp. The exact return type is `ListToolsResult`, which
    // contains a `tools: Vec<Tool>` field.
    let tools = running.client.list_tools(Default::default()).await?.tools;

    if tools.is_empty() {
        warn!("Service `{}` reported no tools", server_info.server_info.name);
    }

    Ok((server_info, tools))
}

pub async fn call_tool(
    running: &RunningService,
    tool_name: &str,
    args: JsonObject,
) -> Result<Vec<Content>> {
    let request = CallToolRequestParam {
        name: Cow::from(tool_name.to_string()),
        arguments: Some(args),
    };

    let resp = running.client.call_tool(request).await?;
    Ok(resp.content)
}
