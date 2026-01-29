//! Handler for the `unicity.debug.list_tools` tool.
//!
//! Debug tool for listing all discovered MCP service tools.
//! Not intended for LLM use - use `unicity.select_tool` for semantic search instead.

use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use rmcp::model::{CallToolResult, Content, JsonObject};
use serde_json::json;
use crate::db::ToolRecord;
use crate::orchestrator::Orchestrator;
use crate::orchestrator::user_filter::UserToolFilter;
use crate::tools::{ToolHandler, ToolContext};

/// Handler for the `unicity.debug.list_tools` debug tool.
pub struct ListDiscoveredToolsHandler {
    orchestrator: Arc<Orchestrator>,
}

impl ListDiscoveredToolsHandler {
    /// Create a new list discovered tools handler.
    pub fn new(orchestrator: Arc<Orchestrator>) -> Self {
        Self { orchestrator }
    }

    /// Build the input schema for this tool.
    fn input_schema(&self) -> JsonObject {
        let mut schema = JsonObject::new();
        schema.insert("type".to_string(), json!("object"));

        let mut properties = serde_json::Map::new();
        properties.insert(
            "service_filter".to_string(),
            json!({
                "type": "string",
                "description": "Optional service ID or name to filter by."
            }),
        );
        properties.insert(
            "include_blocked".to_string(),
            json!({
                "type": "boolean",
                "description": "Whether to include blocked services (default: true for management).",
                "default": true
            }),
        );
        properties.insert(
            "limit".to_string(),
            json!({
                "type": "integer",
                "description": "Maximum number of tools to return (default: 100).",
                "default": 100
            }),
        );
        properties.insert(
            "offset".to_string(),
            json!({
                "type": "integer",
                "description": "Offset for pagination (default: 0).",
                "default": 0
            }),
        );

        schema.insert("properties".to_string(), json!(properties));
        schema.insert("required".to_string(), json!([]));
        schema
    }

    /// Build the output schema for this tool.
    fn output_schema(&self) -> JsonObject {
        let mut schema = JsonObject::new();
        schema.insert("type".to_string(), json!("object"));
        schema.insert(
            "description".to_string(),
            json!("List of discovered tools with blocked status."),
        );
        schema
    }
}

impl ToolHandler for ListDiscoveredToolsHandler {
    fn name(&self) -> &str {
        "unicity.debug.list_tools"
    }

    fn title(&self) -> Option<&str> {
        Some("Debug: List All Discovered Tools")
    }

    fn description(&self) -> &str {
        "[DEBUG] List all discovered MCP service tools with their blocked/trusted status. \
         This is a debug/admin tool - do NOT use for normal tool discovery. \
         Use `unicity.select_tool` with a natural language query instead."
    }

    fn input_schema(&self) -> JsonObject {
        self.input_schema()
    }

    fn output_schema(&self) -> Option<JsonObject> {
        Some(self.output_schema())
    }

    fn execute(
        &self,
        args: JsonObject,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CallToolResult>> + Send + '_>> {
        let orchestrator = self.orchestrator.clone();
        let user_context = ctx.user_context.clone();

        Box::pin(async move {
            let service_filter = args.get("service_filter").and_then(|v| v.as_str());
            let include_blocked = args
                .get("include_blocked")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as usize;
            let offset = args
                .get("offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            // Load user's filter to determine blocked/trusted status
            let filter = match &user_context {
                Some(ctx) => {
                    UserToolFilter::from_user_context(orchestrator.db(), ctx).await
                        .unwrap_or_else(|_| UserToolFilter::allow_all())
                }
                None => UserToolFilter::allow_all(),
            };

            // Query all tools from database
            let query = if let Some(svc) = service_filter {
                format!(
                    "SELECT * FROM tool WHERE service_id CONTAINS '{}' OR service_id = type::thing('service', '{}') LIMIT {} START {}",
                    svc, svc, limit, offset
                )
            } else {
                format!("SELECT * FROM tool LIMIT {} START {}", limit, offset)
            };

            let tools: Vec<ToolRecord> = match orchestrator.db().query(&query).await {
                Ok(mut res) => res.take(0).unwrap_or_default(),
                Err(e) => {
                    let payload = json!({
                        "status": "error",
                        "reason": format!("Failed to query tools: {}", e)
                    });
                    let text = serde_json::to_string(&payload)
                        .unwrap_or_else(|_| "internal serialization error".to_string());
                    return Ok(CallToolResult {
                        content: vec![Content::text(text)],
                        structured_content: None,
                        is_error: Some(true),
                        meta: None,
                    });
                }
            };

            // Build response with blocked/trusted status
            let mut tool_list = Vec::new();
            let mut blocked_count = 0;
            let mut trusted_count = 0;

            for tool in tools {
                let is_blocked = !filter.is_tool_allowed(&tool);
                let is_trusted = filter.is_service_trusted(&tool.service_id);

                if is_blocked {
                    blocked_count += 1;
                    if !include_blocked {
                        continue;
                    }
                }
                if is_trusted {
                    trusted_count += 1;
                }

                tool_list.push(json!({
                    "toolId": tool.id.to_string(),
                    "toolName": tool.name,
                    "serviceId": tool.service_id.to_string(),
                    "description": tool.description,
                    "blocked": is_blocked,
                    "trusted": is_trusted,
                    "inputSchema": tool.input_schema,
                    "outputSchema": tool.output_schema,
                }));
            }

            let payload = json!({
                "status": "ok",
                "tools": tool_list,
                "count": tool_list.len(),
                "blockedCount": blocked_count,
                "trustedCount": trusted_count,
                "pagination": {
                    "limit": limit,
                    "offset": offset,
                }
            });

            let text = serde_json::to_string(&payload)
                .unwrap_or_else(|_| "internal serialization error".to_string());

            Ok(CallToolResult {
                content: vec![Content::text(text)],
                structured_content: None,
                is_error: Some(false),
                meta: None,
            })
        })
    }
}
