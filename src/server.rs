//! MCP server implementation using rmcp.
//!
//! Provides HTTP-based MCP server functionality for the orchestrator.

use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use anyhow::Result;
use axum::Router;
use rmcp::{
    ErrorData as McpError,
    handler::server::ServerHandler,
    model::*,
    service::{NotificationContext, Peer, RequestContext, RoleServer},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpService,
    session::local::LocalSessionManager,
};

use crate::tools::ToolRegistry;
use crate::orchestrator::Orchestrator;
use crate::resources::ResourceError;

/// MCP server that handles protocol requests and delegates to tool handlers.
#[derive(Clone)]
pub struct McpServer {
    orchestrator: Arc<Orchestrator>,
    tool_registry: Arc<ToolRegistry>,
    /// Stored peer for sending notifications to the client.
    peer: Arc<RwLock<Option<Peer<RoleServer>>>>,
    /// Set of resource URIs that the client has subscribed to.
    subscriptions: Arc<RwLock<HashSet<String>>>,
}

impl McpServer {
    /// Create a new MCP server with the given orchestrator and tool registry.
    pub fn new(orchestrator: Arc<Orchestrator>, tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            orchestrator,
            tool_registry,
            peer: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashSet::new())),
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

    /// Send a resource list changed notification to the client.
    pub async fn notify_resource_list_changed(&self) -> Result<(), anyhow::Error> {
        if let Some(peer) = self.peer.read().await.as_ref() {
            peer.notify_resource_list_changed()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send notification: {:?}", e))?;
        }
        Ok(())
    }

    /// Send a prompt list changed notification to the client.
    pub async fn notify_prompt_list_changed(&self) -> Result<(), anyhow::Error> {
        if let Some(peer) = self.peer.read().await.as_ref() {
            peer.notify_prompt_list_changed()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send notification: {:?}", e))?;
        }
        Ok(())
    }

    /// Send a resource updated notification for a specific URI.
    pub async fn notify_resource_updated(&self, uri: &str) -> Result<(), anyhow::Error> {
        // Only notify if the client is subscribed to this resource
        let subscriptions = self.subscriptions.read().await;
        if !subscriptions.contains(uri) {
            return Ok(());
        }
        drop(subscriptions);

        if let Some(peer) = self.peer.read().await.as_ref() {
            peer.notify_resource_updated(ResourceUpdatedNotificationParam {
                uri: uri.to_string().into(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send notification: {:?}", e))?;
        }
        Ok(())
    }

    /// Check if the client is subscribed to a specific resource.
    pub async fn is_subscribed(&self, uri: &str) -> bool {
        self.subscriptions.read().await.contains(uri)
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
        request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<InitializeResult, McpError>> + Send + '_ {
        // Update client capabilities in elicitation coordinator
        let capabilities = request.capabilities.clone();
        let coordinator = self.orchestrator.elicitation_coordinator().clone();
        let peer_storage = self.peer.clone();
        let peer = context.peer.clone();

        async move {
            // Store the peer for sending notifications later
            *peer_storage.write().await = Some(peer);

            // Store client capabilities for elicitation
            coordinator.set_client_capabilities(&capabilities).await;

            Ok(InitializeResult {
                protocol_version: ProtocolVersion::V_2025_06_18,
                capabilities: ServerCapabilities::builder()
                    .enable_tools()
                    .enable_prompts()
                    .enable_prompts_list_changed()
                    .enable_resources()
                    .enable_resources_subscribe()
                    .enable_resources_list_changed()
                    .build(),
                server_info: Implementation::from_build_env(),
                instructions: Some(
                    "Unicity orchestrator that discovers MCP services, builds a SurrealDB-backed knowledge graph, \
                     and uses embeddings + symbolic reasoning to select and chain tools.\n\n\
                     This server supports MCP elicitation for tool approval and OAuth flows."
                        .to_string(),
                ),
            })
        }
    }

    fn list_tools(
        &self,
        request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let cursor = request.as_ref().and_then(|r| r.cursor.as_ref().map(|c| c.as_str()));
        let (tools, next_cursor) = self.tool_registry.list_tools(cursor);
        let mut result = ListToolsResult::default();
        result.tools = tools;
        result.next_cursor = next_cursor.map(|c| c.into());
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
        request: GetPromptRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
        use crate::prompts::PromptError;

        let forwarder = self.orchestrator.prompt_forwarder().clone();
        let name = request.name.to_string();
        let arguments = request.arguments.map(|a| {
            a.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
        });

        async move {
            match forwarder.get_prompt(&name, arguments).await {
                Ok(result) => Ok(result),
                Err(PromptError::NotFound(name)) => {
                    // -32602: Invalid params (per MCP spec for invalid prompt name)
                    Err(McpError::invalid_params(format!("Prompt not found: {}", name), None))
                }
                Err(PromptError::InvalidName(name)) => {
                    // -32602: Invalid params (per MCP spec for invalid prompt name)
                    Err(McpError::invalid_params(format!("Invalid prompt name: {}", name), None))
                }
                Err(PromptError::InvalidArguments(msg)) => {
                    // -32602: Invalid params (per MCP spec for missing required arguments)
                    Err(McpError::invalid_params(format!("Invalid arguments: {}", msg), None))
                }
                Err(PromptError::Internal(msg)) => {
                    // -32603: Internal error
                    Err(McpError::internal_error(format!("Failed to get prompt: {}", msg), None))
                }
            }
        }
    }

    fn list_prompts(
        &self,
        request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        let forwarder = self.orchestrator.prompt_forwarder().clone();
        let cursor = request.as_ref().and_then(|r| r.cursor.as_ref().map(|c| c.to_string()));

        async move {
            match forwarder.list_prompts(cursor.as_deref()).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    Err(McpError::internal_error(format!("Failed to list prompts: {}", e), None))
                }
            }
        }
    }

    fn list_resources(
        &self,
        request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        let forwarder = self.orchestrator.resource_forwarder().clone();
        let cursor = request.as_ref().and_then(|r| r.cursor.as_ref().map(|c| c.to_string()));

        async move {
            match forwarder.list_resources(None, cursor.as_deref()).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    Err(McpError::internal_error(format!("Failed to list resources: {}", e), None))
                }
            }
        }
    }

    fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
        let forwarder = self.orchestrator.resource_forwarder().clone();
        let cursor = request.as_ref().and_then(|r| r.cursor.as_ref().map(|c| c.to_string()));

        async move {
            match forwarder.list_templates(None, cursor.as_deref()).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    Err(McpError::internal_error(format!("Failed to list resource templates: {}", e), None))
                }
            }
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        let forwarder = self.orchestrator.resource_forwarder().clone();
        let uri = request.uri.to_string();

        async move {
            match forwarder.read_resource(&uri).await {
                Ok(contents) => Ok(contents),
                Err(ResourceError::NotFound(uri)) => {
                    // -32002: Resource not found (custom error code per MCP spec)
                    Err(McpError::new(
                        rmcp::model::ErrorCode(-32002),
                        format!("Resource not found: {}", uri),
                        None,
                    ))
                }
                Err(ResourceError::InvalidUri(uri)) => {
                    // -32602: Invalid params (per MCP spec for invalid URI)
                    Err(McpError::invalid_params(format!("Invalid URI: {}", uri), None))
                }
                Err(ResourceError::Internal(msg)) => {
                    // -32603: Internal error
                    Err(McpError::internal_error(format!("Failed to read resource: {}", msg), None))
                }
            }
        }
    }

    fn subscribe(
        &self,
        request: SubscribeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        let subscriptions = self.subscriptions.clone();
        let uri = request.uri.to_string();

        async move {
            // Add the URI to the set of subscribed resources
            subscriptions.write().await.insert(uri);
            Ok(())
        }
    }

    fn unsubscribe(
        &self,
        request: UnsubscribeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        let subscriptions = self.subscriptions.clone();
        let uri = request.uri.to_string();

        async move {
            // Remove the URI from the set of subscribed resources
            subscriptions.write().await.remove(&uri);
            Ok(())
        }
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
                .enable_prompts()
                .enable_prompts_list_changed()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_resources_list_changed()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Unicity orchestrator that discovers MCP services, builds a symbolic graph, \
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
