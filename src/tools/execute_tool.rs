//! Handler for the `unicity.execute_tool` tool.
//!
//! Execute a previously selected underlying MCP tool by toolId with the given arguments.

use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use rmcp::model::{CallToolResult, Content, JsonObject};
use serde_json::json;
use crate::auth::UserStore;
use crate::db::ToolRecord;
use crate::elicitation::ElicitationSchema;
use crate::knowledge_graph::ToolSelection;
use crate::orchestrator::Orchestrator;
use crate::orchestrator::user_filter::UserToolFilter;
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
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CallToolResult>> + Send + '_>> {
        let orchestrator = self.orchestrator.clone();
        // Clone user context for use in async block
        let user_context = ctx.user_context.clone();

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

            // Check if the tool's service is blocked by the user
            if let Some(ref ctx) = user_context {
                let filter = UserToolFilter::from_user_context(orchestrator.db(), ctx)
                    .await
                    .unwrap_or_else(|_| UserToolFilter::allow_all());

                if !filter.is_tool_allowed(&tool) {
                    // Service is blocked - trigger elicitation to ask user what to do
                    let service_id_str = tool.service_id.to_string();

                    let message = format!(
                        "The tool '{}' is from a blocked service '{}'.\n\n\
                         Choose how to proceed:\n\
                         - allow_once: Execute this tool just this once\n\
                         - unblock_service: Remove the block and execute\n\
                         - keep_blocked: Don't execute, keep the service blocked",
                        tool.name,
                        service_id_str
                    );

                    let schema = ElicitationSchema::builder()
                        .required_enum(
                            "action",
                            vec![
                                "allow_once".to_string(),
                                "unblock_service".to_string(),
                                "keep_blocked".to_string(),
                            ],
                        )
                        .description("Blocked service action")
                        .build()
                        .expect("Invalid blocked service schema");

                    let coordinator = orchestrator.elicitation_coordinator();
                    match coordinator.create_elicitation(&message, schema).await {
                        Ok(response) => {
                            use crate::elicitation::ElicitationAction;

                            match response.action {
                                ElicitationAction::Accept => {
                                    let action_str = response.content
                                        .as_ref()
                                        .and_then(|c| c.get("action"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("keep_blocked");

                                    match action_str {
                                        "allow_once" => {
                                            // Continue with execution (fall through)
                                        }
                                        "unblock_service" => {
                                            // Unblock the service and continue
                                            let user_store = UserStore::new(orchestrator.db().clone());
                                            if let Err(e) = user_store.unblock_service(ctx.user_id(), &service_id_str).await {
                                                tracing::warn!("Failed to unblock service: {}", e);
                                            } else {
                                                tracing::info!(
                                                    user_id = %ctx.user_id_string(),
                                                    service_id = %service_id_str,
                                                    "User unblocked service via elicitation"
                                                );
                                            }
                                            // Continue with execution (fall through)
                                        }
                                        "keep_blocked" | _ => {
                                            let payload = json!({
                                                "status": "blocked",
                                                "reason": format!("Tool execution blocked: service '{}' is blocked", service_id_str),
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
                                    }
                                }
                                ElicitationAction::Decline | ElicitationAction::Cancel => {
                                    let payload = json!({
                                        "status": "blocked",
                                        "reason": format!("Tool execution blocked: service '{}' is blocked", service_id_str),
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
                            }
                        }
                        Err(e) => {
                            // Elicitation failed - block execution
                            tracing::warn!("Blocked service elicitation failed: {}", e);
                            let payload = json!({
                                "status": "blocked",
                                "reason": format!("Tool execution blocked: service '{}' is blocked (elicitation unavailable)", service_id_str),
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
                    }
                }
            }

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
                .execute_selected_tool_with_approval(&selection, tool_args, user_context.as_ref())
                .await
            {
                Ok(contents) => (contents, false),
                Err(e) => {
                    let error_msg = e.to_string();
                    let (status, reason) = if error_msg.contains("denied by user") {
                        ("denied", error_msg)
                    } else if error_msg.contains("cancelled") {
                        ("cancelled", error_msg)
                    } else {
                        ("error", format!("Tool execution failed: {}", e))
                    };
                    let payload = json!({
                        "status": status,
                        "reason": reason,
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
