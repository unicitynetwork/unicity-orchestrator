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

use crate::UnicityOrchestrator;

/// Wrapper around a shared `UnicityOrchestrator` so it can be used as an
/// rmcp `Service<RoleServer>` via the `ServerHandler` blanket impl.
#[derive(Clone)]
struct SharedOrchestrator {
    inner: Arc<UnicityOrchestrator>,
}

impl SharedOrchestrator {
    fn new(inner: Arc<UnicityOrchestrator>) -> Self {
        Self { inner }
    }
}

impl ServerHandler for SharedOrchestrator {
    fn ping(
        &self,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        self.inner.ping(context)
    }

    fn initialize(
        &self,
        request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<InitializeResult, McpError>> + Send + '_ {
        self.inner.initialize(request, context)
    }

    fn complete(
        &self,
        request: CompleteRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CompleteResult, McpError>> + Send + '_ {
        self.inner.complete(request, context)
    }

    fn set_level(
        &self,
        request: SetLevelRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        self.inner.set_level(request, context)
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
        self.inner.get_prompt(request, context)
    }

    fn list_prompts(
        &self,
        request: Option<PaginatedRequestParam>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        self.inner.list_prompts(request, context)
    }

    fn list_resources(
        &self,
        request: Option<PaginatedRequestParam>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        self.inner.list_resources(request, context)
    }

    fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParam>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
        self.inner.list_resource_templates(request, context)
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        self.inner.read_resource(request, context)
    }

    fn subscribe(
        &self,
        request: SubscribeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        self.inner.subscribe(request, context)
    }

    fn unsubscribe(
        &self,
        request: UnsubscribeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        self.inner.unsubscribe(request, context)
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        self.inner.call_tool(request, context)
    }

    fn list_tools(
        &self,
        request: Option<PaginatedRequestParam>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        self.inner.list_tools(request, context)
    }

    fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.inner.on_cancelled(notification, context)
    }

    fn on_progress(
        &self,
        notification: ProgressNotificationParam,
        context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.inner.on_progress(notification, context)
    }

    fn on_initialized(
        &self,
        context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.inner.on_initialized(context)
    }

    fn on_roots_list_changed(
        &self,
        context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.inner.on_roots_list_changed(context)
    }

    fn get_info(&self) -> ServerInfo {
        self.inner.get_info()
    }
}

/// Start the orchestrator as an MCP Streamable HTTP server.
///
/// This exposes the MCP endpoint at `/mcp` on the given bind address,
/// e.g. `127.0.0.1:3942` or `0.0.0.0:3942`.
pub async fn start_mcp_http(
    orchestrator: Arc<UnicityOrchestrator>,
    bind: &str,
) -> Result<()> {
    // Create a Streamable HTTP MCP service that reuses the same orchestrator
    // instance across all sessions via `Arc`.
    let handler = SharedOrchestrator::new(orchestrator.clone());

    let service = StreamableHttpService::new(
        {
            let handler = handler.clone();
            move || Ok(handler.clone())
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
