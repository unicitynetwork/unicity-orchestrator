//! Handler for the `unicity.execute_tool` tool.
//!
//! Execute a previously selected underlying MCP tool by toolId with the given arguments.

use std::pin::Pin;
use std::sync::Arc;
use rmcp::model::{CallToolResult, Content, JsonObject};
use serde_json::json;
use crate::db::ToolRecord;
use crate::knowledge_graph::ToolSelection;
use crate::orchestrator::Orchestrator;
use crate::tools::{ToolHandler, ToolContext};

/// Handler for the `unicity.execute_tool` tool.
pub struct ExecuteToolHandler {
    orchestrator: Arc<Orchestrator>,
}

impl ExecuteToolHandler {
    /// Create a new execute tool handler.
    pub fn new(orchestrator: Arc<Orchestrator>) -> Self {
        Self { orchestrator }
    }

    /// Build the input schema for this tool.
    fn input_schema(&self) -> JsonObject {
        let mut schema = JsonObject::new();
        schema.insert(
            "type".to_string(),
            json!("object"),
        );

        let mut properties = serde_json::Map::new();
        properties.insert(
            "toolId".to_string(),
            json!({
                "type": "string",
                "description": "The orchestrator toolId of the tool to execute (e.g. 'tool:abc123')."
            }),
        );
        properties.insert(
            "args".to_string(),
            json!({
                "type": "object",
                "description": "Arguments to pass to the underlying MCP tool, shaped according to its inputSchema.",
                "additionalProperties": true
            }),
        );

        schema.insert("properties".to_string(), json!(properties));
        schema.insert("required".to_string(), json!(["toolId", "args"]));
        schema
    }

    /// Look up a tool by its ID string.
    async fn lookup_tool(orchestrator: &Orchestrator, tool_id_str: String) -> Result<Option<ToolRecord>, String> {
        let db_res = orchestrator
            .db()
            .query("SELECT * FROM type::thing($id)")
            .bind(("id", tool_id_str))
            .await;

        match db_res {
            Ok(mut res) => {
                match res.take(0) {
                    Ok(tool) => Ok(tool),
                    Err(e) => Err(format!("Failed to decode ToolRecord: {}", e)),
                }
            }
            Err(e) => Err(format!("Database error while loading tool: {}", e)),
        }
    }
}

impl ToolHandler for ExecuteToolHandler {
    fn name(&self) -> &str {
        "unicity.execute_tool"
    }

    fn title(&self) -> Option<&str> {
        Some("Unicity Orchestrator: Execute Tool")
    }

    fn description(&self) -> &str {
        "Execute a previously selected underlying MCP tool by toolId with the given arguments."
    }

    fn input_schema(&self) -> JsonObject {
        self.input_schema()
    }

    fn execute(
        &self,
        args: JsonObject,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<CallToolResult>> + Send + '_>> {
        let orchestrator = self.orchestrator.clone();

        Box::pin(async move {
            let tool_id_str = match args.get("toolId").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => {
                    let payload = json!({
                        "status": "error",
                        "reason": "unicity.execute_tool requires a `toolId` string"
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

            let tool_id_str_clone = tool_id_str.clone();

            let tool_args: JsonObject = args
                .get("args")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();

            // Look up the tool
            let tool = match Self::lookup_tool(&orchestrator, tool_id_str).await {
                Ok(Some(t)) => t,
                Ok(None) => {
                    let payload = json!({
                        "status": "error",
                        "reason": format!("No tool found with id {}", tool_id_str_clone),
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
                Err(e) => {
                    let payload = json!({
                        "status": "error",
                        "reason": e,
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

            let selection = ToolSelection {
                tool_id: tool.id.clone(),
                tool_name: tool.name.clone(),
                service_id: tool.service_id.clone(),
                confidence: 1.0,
                reasoning: "Direct execution via unicity.execute_tool".to_string(),
                dependencies: Vec::new(),
                estimated_cost: None,
            };

            let (content, is_error) = match orchestrator
                .execute_selected_tool(&selection, tool_args)
                .await
            {
                Ok(contents) => (contents, false),
                Err(e) => {
                    let payload = json!({
                        "status": "error",
                        "reason": format!("Tool execution failed: {}", e),
                    });
                    let text = serde_json::to_string(&payload)
                        .unwrap_or_else(|_| "internal serialization error".to_string());
                    (vec![Content::text(text)], true)
                }
            };

            Ok(CallToolResult {
                content,
                structured_content: None,
                is_error: Some(is_error),
                meta: None,
            })
        })
    }
}
