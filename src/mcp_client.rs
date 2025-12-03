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
use rmcp::transport::StreamableHttpClientTransport;
use tokio::process::Command;
use tracing::{info, warn};

/// Wrapper for a running MCP service client.
///
/// This holds the SurrealDB id for the service as well as the rmcp `RunningService`
/// handle used to talk MCP (initialize, list_tools, call_tool, etc.).
pub struct RunningService {
    pub client: RmcpRunningService<RoleClient, ()>,
}

pub async fn start_stdio_service(cfg: &McpServiceConfig) -> Result<Option<RunningService>> {
    if let McpServiceConfig::Stdio {
        id,
        command,
        args,
        env,
        disabled,
        ..
    } = cfg
    {
        if *disabled {
            info!("Skipping disabled MCP stdio service `{id}`");
            return Ok(None);
        }

        info!("Starting MCP stdio service `{id}` via rmcp");

        let mut cmd = Command::new(command);
        if !args.is_empty() {
            cmd.args(args.iter().cloned());
        }
        if !env.is_empty() {
            cmd.envs(env.iter().map(|(k, v)| (k, v)));
        }

        let child = TokioChildProcess::new(cmd.configure(|_cmd| {
            // extra configuration if needed
        }))?;

        let client = ().serve(child).await?;

        Ok(Some(RunningService { client }))
    } else {
        Ok(None)
    }
}

pub async fn start_http_service(cfg: &McpServiceConfig) -> Result<Option<RunningService>> {
    if let McpServiceConfig::Http {
        id,
        url,
        headers: _,
        disabled,
        ..
    } = cfg
    {
        if *disabled {
            info!("Skipping disabled MCP HTTP service `{id}`");
            return Ok(None);
        }

        info!("Starting MCP HTTP service `{id}` at `{url}` via rmcp streamable HTTP");

        // Build HTTP transport (Result -> WorkerTransport)
        let transport = StreamableHttpClientTransport::from_uri(url.as_str());

        // Keep the same client type as stdio: RmcpRunningService<RoleClient, ()>
        let client = ().serve(transport).await?;

        Ok(Some(RunningService { client }))
    } else {
        Ok(None)
    }
}

pub async fn start_service(cfg: &McpServiceConfig) -> Result<Option<RunningService>> {
    match cfg {
        McpServiceConfig::Stdio { .. } => start_stdio_service(cfg).await,
        McpServiceConfig::Http { .. } => start_http_service(cfg).await,
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
