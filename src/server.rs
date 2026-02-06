//! MCP server implementation using rmcp.
//!
//! Provides HTTP-based MCP server functionality for the orchestrator.

use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use anyhow::Result;
use axum::Router;
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{
    ErrorData as McpError,
    handler::server::ServerHandler,
    model::*,
    service::{NotificationContext, Peer, RequestContext, RoleServer},
};

use crate::auth::{AuthConfig, AuthError, AuthExtractor, UserContext};
use crate::orchestrator::Orchestrator;
use crate::resources::ResourceError;
use crate::tools::ToolRegistry;

/// Type alias for HTTP request parts stored in rmcp extensions.
type HttpParts = http::request::Parts;

/// MCP server that handles protocol requests and delegates to tool handlers.
#[derive(Clone)]
pub struct McpServer {
    orchestrator: Arc<Orchestrator>,
    tool_registry: Arc<ToolRegistry>,
    /// Stored peer for sending notifications to the client.
    peer: Arc<RwLock<Option<Peer<RoleServer>>>>,
    /// Set of resource URIs that the client has subscribed to.
    subscriptions: Arc<RwLock<HashSet<String>>>,
    /// User context for multi-tenant isolation (None for anonymous/stdio mode).
    /// Uses interior mutability so it can be set during initialize().
    user_context: Arc<RwLock<Option<UserContext>>>,
    /// Optional auth extractor for HTTP mode.
    auth_extractor: Option<Arc<AuthExtractor>>,
}

impl McpServer {
    /// Create a new MCP server with the given orchestrator and tool registry.
    ///
    /// This creates a server in anonymous/stdio mode without auth extraction.
    pub fn new(orchestrator: Arc<Orchestrator>, tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            orchestrator,
            tool_registry,
            peer: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashSet::new())),
            user_context: Arc::new(RwLock::new(None)), // Anonymous/stdio mode
            auth_extractor: None,
        }
    }

    /// Create a new MCP server with user context for multi-tenant isolation.
    ///
    /// This is used when the user context is already known (e.g., pre-extracted).
    pub fn new_with_user(
        orchestrator: Arc<Orchestrator>,
        tool_registry: Arc<ToolRegistry>,
        user_context: Option<UserContext>,
    ) -> Self {
        Self {
            orchestrator,
            tool_registry,
            peer: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashSet::new())),
            user_context: Arc::new(RwLock::new(user_context)),
            auth_extractor: None,
        }
    }

    /// Create a new MCP server with auth extractor for HTTP mode.
    ///
    /// The auth extractor will be called during initialize() to extract
    /// user context from HTTP request headers.
    pub fn new_with_auth(
        orchestrator: Arc<Orchestrator>,
        tool_registry: Arc<ToolRegistry>,
        auth_extractor: Arc<AuthExtractor>,
    ) -> Self {
        Self {
            orchestrator,
            tool_registry,
            peer: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashSet::new())),
            user_context: Arc::new(RwLock::new(None)),
            auth_extractor: Some(auth_extractor),
        }
    }

    /// Get a clone of the current user context.
    pub async fn user_context(&self) -> Option<UserContext> {
        self.user_context.read().await.clone()
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
                uri: uri.to_string(),
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

    /// Rediscover resources from all MCP services and notify connected clients.
    ///
    /// This method:
    /// 1. Clears existing resource registrations
    /// 2. Discovers resources from all running services
    /// 3. Sends a `notifications/resources/list_changed` notification to the client
    pub async fn rediscover_resources(&self) -> Result<usize, anyhow::Error> {
        let count = self.orchestrator.discover_resources().await?;

        // Notify the client that the resource list has changed
        if let Err(e) = self.notify_resource_list_changed().await {
            tracing::warn!("Failed to send resource list changed notification: {}", e);
        }

        Ok(count)
    }

    /// Rediscover prompts from all MCP services and notify connected clients.
    ///
    /// This method:
    /// 1. Clears existing prompt registrations
    /// 2. Discovers prompts from all running services
    /// 3. Sends a `notifications/prompts/list_changed` notification to the client
    pub async fn rediscover_prompts(&self) -> Result<usize, anyhow::Error> {
        let count = self.orchestrator.discover_prompts().await?;

        // Notify the client that the prompt list has changed
        if let Err(e) = self.notify_prompt_list_changed().await {
            tracing::warn!("Failed to send prompt list changed notification: {}", e);
        }

        Ok(count)
    }

    /// Rediscover all resources and prompts, notifying clients of changes.
    ///
    /// This is a convenience method that calls both `rediscover_resources()`
    /// and `rediscover_prompts()`.
    pub async fn rediscover_all(&self) -> Result<(usize, usize), anyhow::Error> {
        let resources = self.rediscover_resources().await?;
        let prompts = self.rediscover_prompts().await?;
        Ok((resources, prompts))
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
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<InitializeResult, McpError>> + Send + '_ {
        // Update client capabilities in elicitation coordinator
        let capabilities = request.capabilities.clone();
        let coordinator = self.orchestrator.elicitation_coordinator().clone();
        let peer_storage = self.peer.clone();
        let peer = context.peer.clone();
        let user_context_storage = self.user_context.clone();
        let auth_extractor = self.auth_extractor.clone();

        // Try to extract HTTP request parts from rmcp extensions for auth
        // The rmcp library stores http::request::Parts in extensions when using HTTP transport
        let extensions = context.extensions.clone();

        async move {
            // Store the peer for sending notifications later
            *peer_storage.write().await = Some(peer.clone());

            // Store client capabilities for elicitation
            coordinator.set_client_capabilities(&capabilities).await;

            // Store peer in coordinator for sending elicitation requests
            coordinator.set_peer(peer).await;

            // Extract user context from HTTP headers if auth extractor is configured
            if let Some(extractor) = auth_extractor {
                // Try to get HTTP request parts from extensions
                // rmcp stores http::request::Parts in extensions for HTTP transport
                let (authorization, api_key, ip_address, user_agent) =
                    if let Some(parts) = extensions.get::<HttpParts>() {
                        let auth = parts
                            .headers
                            .get(http::header::AUTHORIZATION)
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let api_key = parts
                            .headers
                            .get("X-API-Key")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let ip = parts
                            .headers
                            .get("X-Forwarded-For")
                            .or_else(|| parts.headers.get("X-Real-IP"))
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let ua = parts
                            .headers
                            .get(http::header::USER_AGENT)
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        (auth, api_key, ip, ua)
                    } else {
                        (None, None, None, None)
                    };

                match extractor
                    .extract_user(
                        authorization.as_deref(),
                        api_key.as_deref(),
                        ip_address,
                        user_agent,
                    )
                    .await
                {
                    Ok(ctx) => {
                        tracing::info!(
                            user_id = %ctx.user_id_string(),
                            provider = %ctx.provider().as_str(),
                            "User authenticated for MCP session"
                        );
                        *user_context_storage.write().await = Some(ctx);
                    }
                    Err(AuthError::Unauthenticated) => {
                        tracing::warn!("MCP session rejected: authentication required");
                        return Err(McpError::new(
                            ErrorCode(-32001),
                            "Authentication required".to_string(),
                            None,
                        ));
                    }
                    Err(AuthError::InvalidApiKey) => {
                        tracing::warn!("MCP session rejected: invalid API key");
                        return Err(McpError::new(
                            ErrorCode(-32001),
                            "Invalid API key".to_string(),
                            None,
                        ));
                    }
                    Err(AuthError::InvalidToken(msg)) => {
                        tracing::warn!("MCP session rejected: invalid token - {}", msg);
                        return Err(McpError::new(
                            ErrorCode(-32001),
                            format!("Invalid token: {}", msg),
                            None,
                        ));
                    }
                    Err(AuthError::UserDeactivated) => {
                        tracing::warn!("MCP session rejected: user deactivated");
                        return Err(McpError::new(
                            ErrorCode(-32001),
                            "User account is deactivated".to_string(),
                            None,
                        ));
                    }
                    Err(AuthError::DatabaseError(msg)) => {
                        tracing::error!("MCP session auth failed: database error - {}", msg);
                        return Err(McpError::internal_error(
                            format!("Authentication failed: {}", msg),
                            None,
                        ));
                    }
                    Err(AuthError::ApiKeyExpired) => {
                        tracing::warn!("MCP session rejected: API key expired");
                        return Err(McpError::new(
                            ErrorCode(-32001),
                            "API key has expired".to_string(),
                            None,
                        ));
                    }
                    Err(AuthError::ApiKeyRevoked) => {
                        tracing::warn!("MCP session rejected: API key revoked");
                        return Err(McpError::new(
                            ErrorCode(-32001),
                            "API key has been revoked".to_string(),
                            None,
                        ));
                    }
                    Err(AuthError::JwksError(msg)) => {
                        tracing::error!("MCP session auth failed: JWKS error - {}", msg);
                        return Err(McpError::internal_error(
                            format!("JWKS error: {}", msg),
                            None,
                        ));
                    }
                }
            }

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
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let cursor = request.as_ref().and_then(|r| r.cursor.as_deref());
        let (tools, next_cursor) = self.tool_registry.list_tools(cursor);
        let result = ListToolsResult {
            tools,
            next_cursor,
            ..Default::default()
        };
        std::future::ready(Ok(result))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let tool_name = request.name.to_string();
        let args = request.arguments.unwrap_or_default();
        let registry = self.tool_registry.clone();
        let user_context_storage = self.user_context.clone();

        async move {
            // Read user context from the lock (set during initialize())
            let user_context = user_context_storage.read().await.clone();
            let ctx = crate::tools::ToolContext {
                request_context: context,
                user_context,
            };

            match registry.call_tool(&tool_name, args, &ctx).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    // Convert anyhow error to McpError
                    Err(McpError::internal_error(
                        format!("Tool execution failed: {}", e),
                        None,
                    ))
                }
            }
        }
    }

    // Default implementations for unsupported features

    fn complete(
        &self,
        _request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CompleteResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<CompleteRequestMethod>()))
    }

    fn set_level(
        &self,
        _request: SetLevelRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<SetLevelRequestMethod>()))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
        use crate::prompts::PromptError;

        let forwarder = self.orchestrator.prompt_forwarder().clone();
        let name = request.name.to_string();
        let arguments = request
            .arguments
            .map(|a| a.into_iter().map(|(k, v)| (k.to_string(), v)).collect());

        async move {
            match forwarder.get_prompt(&name, arguments).await {
                Ok(result) => Ok(result),
                Err(PromptError::NotFound(name)) => {
                    // -32602: Invalid params (per MCP spec for invalid prompt name)
                    Err(McpError::invalid_params(
                        format!("Prompt not found: {}", name),
                        None,
                    ))
                }
                Err(PromptError::InvalidName(name)) => {
                    // -32602: Invalid params (per MCP spec for invalid prompt name)
                    Err(McpError::invalid_params(
                        format!("Invalid prompt name: {}", name),
                        None,
                    ))
                }
                Err(PromptError::InvalidArguments(msg)) => {
                    // -32602: Invalid params (per MCP spec for missing required arguments)
                    Err(McpError::invalid_params(
                        format!("Invalid arguments: {}", msg),
                        None,
                    ))
                }
                Err(PromptError::Internal(msg)) => {
                    // -32603: Internal error
                    Err(McpError::internal_error(
                        format!("Failed to get prompt: {}", msg),
                        None,
                    ))
                }
            }
        }
    }

    fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        let forwarder = self.orchestrator.prompt_forwarder().clone();
        let cursor = request
            .as_ref()
            .and_then(|r| r.cursor.as_ref().map(|c| c.to_string()));

        async move {
            match forwarder.list_prompts(cursor.as_deref()).await {
                Ok(result) => Ok(result),
                Err(e) => Err(McpError::internal_error(
                    format!("Failed to list prompts: {}", e),
                    None,
                )),
            }
        }
    }

    fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        let forwarder = self.orchestrator.resource_forwarder().clone();
        let cursor = request
            .as_ref()
            .and_then(|r| r.cursor.as_ref().map(|c| c.to_string()));

        async move {
            match forwarder.list_resources(None, cursor.as_deref()).await {
                Ok(result) => Ok(result),
                Err(e) => Err(McpError::internal_error(
                    format!("Failed to list resources: {}", e),
                    None,
                )),
            }
        }
    }

    fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
        let forwarder = self.orchestrator.resource_forwarder().clone();
        let cursor = request
            .as_ref()
            .and_then(|r| r.cursor.as_ref().map(|c| c.to_string()));

        async move {
            match forwarder.list_templates(None, cursor.as_deref()).await {
                Ok(result) => Ok(result),
                Err(e) => Err(McpError::internal_error(
                    format!("Failed to list resource templates: {}", e),
                    None,
                )),
            }
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
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
                        ErrorCode(-32002),
                        format!("Resource not found: {}", uri),
                        None,
                    ))
                }
                Err(ResourceError::InvalidUri(uri)) => {
                    // -32602: Invalid params (per MCP spec for invalid URI)
                    Err(McpError::invalid_params(
                        format!("Invalid URI: {}", uri),
                        None,
                    ))
                }
                Err(ResourceError::Internal(msg)) => {
                    // -32603: Internal error
                    Err(McpError::internal_error(
                        format!("Failed to read resource: {}", msg),
                        None,
                    ))
                }
            }
        }
    }

    fn subscribe(
        &self,
        request: SubscribeRequestParams,
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
        request: UnsubscribeRequestParams,
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
///
/// # Arguments
/// * `server` - The MCP server instance (used to get orchestrator and tool_registry)
/// * `bind` - The address to bind to (e.g., "0.0.0.0:3942")
/// * `auth_config` - Optional authentication configuration. If provided, auth extraction
///   will be enabled for HTTP sessions.
pub async fn start_mcp_http(
    server: Arc<McpServer>,
    bind: &str,
    auth_config: Option<AuthConfig>,
) -> Result<()> {
    let orchestrator = server.orchestrator().clone();
    let tool_registry = server.tool_registry().clone();
    let db = orchestrator.db().clone();

    // Create auth extractor if config provided
    let auth_extractor = auth_config.map(|config| Arc::new(AuthExtractor::new(config, db)));

    let service = StreamableHttpService::new(
        {
            let orchestrator = orchestrator.clone();
            let tool_registry = tool_registry.clone();
            let auth_extractor = auth_extractor.clone();
            move || {
                // Create server with auth extractor if configured
                let server = if let Some(ref extractor) = auth_extractor {
                    McpServer::new_with_auth(
                        orchestrator.clone(),
                        tool_registry.clone(),
                        extractor.clone(),
                    )
                } else {
                    McpServer::new(orchestrator.clone(), tool_registry.clone())
                };
                Ok(server)
            }
        },
        LocalSessionManager::default().into(),
        Default::default(),
    );

    let router = Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(bind).await?;

    if auth_extractor.is_some() {
        tracing::info!(
            "MCP HTTP server listening on http://{} (auth enabled)",
            bind
        );
    } else {
        tracing::info!(
            "MCP HTTP server listening on http://{} (anonymous mode)",
            bind
        );
    }

    axum::serve(listener, router).await?;

    Ok(())
}
