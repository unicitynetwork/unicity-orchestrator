//! Tool registry for managing MCP tool handlers.
//!
//! Provides a `ToolHandler` trait for implementing tools and a `ToolRegistry`
//! for registering and invoking them.

use std::collections::HashMap;
use std::sync::Arc;
use std::pin::Pin;
use std::future::Future;
use rmcp::model::{Tool as McpTool, JsonObject, CallToolResult};
use rmcp::service::RequestContext;
use rmcp::RoleServer;
use anyhow::Result;

/// Context passed to tool handlers during execution.
#[derive(Clone)]
pub struct ToolContext {
    /// Request context from rmcp (for session info, etc.)
    pub request_context: RequestContext<RoleServer>,
}

/// Trait for handling MCP tool invocations.
///
/// Each tool implements this trait to define its schema and execution logic.
pub trait ToolHandler: Send + Sync {
    /// Returns the tool's name (e.g., "unicity.select_tool").
    fn name(&self) -> &str;

    /// Returns the tool's human-readable title.
    fn title(&self) -> Option<&str> {
        None
    }

    /// Returns the tool's description.
    fn description(&self) -> &str;

    /// Returns the input schema for this tool.
    fn input_schema(&self) -> JsonObject;

    /// Returns the output schema for this tool (optional).
    fn output_schema(&self) -> Option<JsonObject> {
        None
    }

    /// Executes the tool with the given arguments.
    fn execute(
        &self,
        args: JsonObject,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<CallToolResult>> + Send + '_>>;

    /// Converts this handler to an `McpTool` for use in `list_tools`.
    fn to_mcp_tool(&self) -> McpTool {
        use std::borrow::Cow;
        use std::sync::Arc;

        McpTool {
            name: Cow::Owned(self.name().to_string()),
            title: self.title().map(|s| s.to_string()),
            description: Some(Cow::Owned(self.description().to_string())),
            input_schema: Arc::new(self.input_schema()),
            output_schema: self.output_schema().map(Arc::new),
            annotations: None,
            icons: None,
            meta: None,
        }
    }
}

/// Registry for managing tool handlers.
#[derive(Clone)]
pub struct ToolRegistry {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    /// Create a new empty tool registry.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a tool handler.
    pub fn register(mut self, handler: Arc<dyn ToolHandler>) -> Self {
        self.handlers.insert(handler.name().to_string(), handler);
        self
    }

    /// Register a tool handler from a type that implements `ToolHandler`.
    pub fn register_handler<T: ToolHandler + 'static>(mut self, handler: T) -> Self {
        self.handlers.insert(handler.name().to_string(), Arc::new(handler));
        self
    }

    /// Get a tool handler by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.handlers.get(name).cloned()
    }

    /// List all registered tool names.
    pub fn list_names(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Get all registered tools as `McpTool` instances for `list_tools`.
    pub fn list_tools(&self) -> Vec<McpTool> {
        self.handlers
            .values()
            .map(|handler| handler.to_mcp_tool())
            .collect()
    }

    /// Execute a tool by name with the given arguments.
    pub async fn call_tool(
        &self,
        name: &str,
        args: JsonObject,
        ctx: &ToolContext,
    ) -> Result<CallToolResult> {
        let handler = self
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))?;
        handler.execute(args, ctx).await
    }

    /// Check if a tool with the given name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Return the number of registered tools.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Return `true` if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
