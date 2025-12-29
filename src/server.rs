//! MCP server implementation using rmcp.
//!
//! Provides HTTP-based MCP server functionality for the orchestrator.

use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use rmcp::{
    ErrorData as McpError,
    handler::server::ServerHandler,
    model::*,
    service::{NotificationContext, RequestContext, RoleServer},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpService,
    session::local::LocalSessionManager,
};

use crate::tools::ToolRegistry;
use crate::orchestrator::Orchestrator;

/// MCP server that handles protocol requests and delegates to tool handlers.
#[derive(Clone)]
pub struct McpServer {
    orchestrator: Arc<Orchestrator>,
    tool_registry: Arc<ToolRegistry>,
}

impl McpServer {
    /// Create a new MCP server with the given orchestrator and tool registry.
    pub fn new(orchestrator: Arc<Orchestrator>, tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            orchestrator,
            tool_registry,
        }
    }

    /// Get the orchestrator.
    pub fn orchestrator(&self) -> &Arc<Orchestrator> {
        &self.orchestrator
    }

    /// Get the tool registry.
    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tool_registry
    }
}

impl ServerHandler for McpServer {
    fn ping(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        std::future::ready(Ok(()))
    }

    fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<InitializeResult, McpError>> + Send + '_ {
        std::future::ready(Ok(InitializeResult {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Unicity orchestrator that discovers MCP services, builds a SurrealDB-backed knowledge graph, \
                 and uses embeddings + symbolic reasoning to select and chain tools."
                    .to_string(),
            ),
        }))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = self.tool_registry.list_tools();
        let mut result = ListToolsResult::default();
        result.tools = tools;
        std::future::ready(Ok(result))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let tool_name = request.name.to_string();
        let args = request.arguments.unwrap_or_default();
        let registry = self.tool_registry.clone();
        let ctx = crate::tools::ToolContext {
            request_context: context,
        };

        async move {
            match registry.call_tool(&tool_name, args, &ctx).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    // Convert anyhow error to McpError
                    Err(McpError::internal_error(format!("Tool execution failed: {}", e), None))
                }
            }
        }
    }

    // Default implementations for unsupported features

    fn complete(
        &self,
        _request: CompleteRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CompleteResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<CompleteRequestMethod>()))
    }

    fn set_level(
        &self,
        _request: SetLevelRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<SetLevelRequestMethod>()))
    }

    fn get_prompt(
        &self,
        _request: GetPromptRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<GetPromptRequestMethod>()))
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<ListPromptsRequestMethod>()))
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<ListResourcesRequestMethod>()))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<ListResourceTemplatesRequestMethod>()))
    }

    fn read_resource(
        &self,
        _request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<ReadResourceRequestMethod>()))
    }

    fn subscribe(
        &self,
        _request: SubscribeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<SubscribeRequestMethod>()))
    }

    fn unsubscribe(
        &self,
        _request: UnsubscribeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<UnsubscribeRequestMethod>()))
    }

    fn on_cancelled(
        &self,
        _notification: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }

    fn on_progress(
        &self,
        _notification: ProgressNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }

    fn on_initialized(
        &self,
        _context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }

    fn on_roots_list_changed(
        &self,
        _context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        std::future::ready(())
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Unicity orchestrator that discovers MCP services, builds a SurrealDB-backed knowledge graph, \
                 and uses embeddings + symbolic reasoning to select and chain tools."
                    .to_string(),
            ),
        }
    }
}

/// Start the orchestrator as an MCP Streamable HTTP server.
///
/// This exposes the MCP endpoint at `/mcp` on the given bind address,
/// e.g. `127.0.0.1:3942` or `0.0.0.0:3942`.
pub async fn start_mcp_http(
    server: Arc<McpServer>,
    bind: &str,
) -> Result<()> {
    let orchestrator = server.orchestrator().clone();
    let tool_registry = server.tool_registry().clone();

    let service = StreamableHttpService::new(
        {
            let orchestrator = orchestrator.clone();
            let tool_registry = tool_registry.clone();
            move || {
                Ok(McpServer::new(orchestrator.clone(), tool_registry.clone()))
            }
        },
        LocalSessionManager::default().into(),
        Default::default(),
    );

    let router = Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(bind).await?;

    tracing::info!("MCP HTTP server listening on http://{}", bind);

    axum::serve(listener, router).await?;

    Ok(())
}
