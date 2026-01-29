//! Handler for the `unicity.select_tool` tool.
//!
//! Given a natural-language query and optional context, selects the most
//! appropriate tool from discovered MCP services using semantic search
//! and symbolic reasoning.

use std::pin::Pin;
use std::sync::Arc;
use rmcp::model::{CallToolResult, Content, JsonObject};
use serde_json::json;
use crate::orchestrator::Orchestrator;
use crate::tools::{ToolHandler, ToolContext};

/// Handler for the `unicity.select_tool` tool.
pub struct SelectToolHandler {
    orchestrator: Arc<Orchestrator>,
}

impl SelectToolHandler {
    /// Create a new select tool handler.
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
            "query".to_string(),
            json!({
                "type": "string",
                "description": "Natural-language query describing the user's goal.",
            }),
        );
        properties.insert(
            "context".to_string(),
            json!({
                "type": "object",
                "description": "Optional JSON context to guide tool selection.",
                "additionalProperties": true,
            }),
        );

        schema.insert("properties".to_string(), json!(properties));
        schema.insert("required".to_string(), json!(["query"]));
        schema
    }

    /// Build the output schema for this tool.
    fn output_schema(&self) -> JsonObject {
        let mut schema = JsonObject::new();
        schema.insert(
            "type".to_string(),
            json!("object"),
        );
        schema.insert(
            "description".to_string(),
            json!("Selection result describing which underlying MCP tool to use and why."),
        );
        schema
    }
}

impl ToolHandler for SelectToolHandler {
    fn name(&self) -> &str {
        "unicity.select_tool"
    }

    fn title(&self) -> Option<&str> {
        Some("Unicity Orchestrator: Select Tool")
    }

    fn description(&self) -> &str {
        "Given a natural-language instruction and optional execution context, \
         infer whether an underlying MCP tool likely exists, reconstruct its \
         probable interface, and return a structured prediction of the most \
         suitable tool and its expected arguments—without executing anything. \
         \
         The orchestrator performs three core operations: \
         • Semantic Intent Analysis: Extracts the operational goal from user text \
           and maps it to known or hypothesized tool capabilities. \
         • Predictive Tool Inference: When no explicit tool is registered, it \
           synthesizes a plausible tool candidate—including name, purpose, \
           argument schema, and invocation format—based on observed patterns. \
         • Schema Alignment: Produces a machine-readable description of the \
           selected or inferred tool, including argument types, required fields, \
           and justification for why it fits the request. \
         \
         This tool does not execute anything; it exists to enable intelligent \
         routing, adaptive interface negotiation, and resilient orchestration \
         even when APIs are incomplete or missing."
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
            let query = match args.get("query").and_then(|v| v.as_str()) {
                Some(q) => q.to_string(),
                None => {
                    let payload = json!({
                        "status": "error",
                        "reason": "unicity.select_tool requires a `query` string argument"
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

            let context_value = args.get("context").cloned();

            let selection_result = orchestrator
                .orchestrate_tool(&query, context_value, user_context.as_ref())
                .await;

            let mut is_error = false;
            let payload = match selection_result {
                Ok(Some(sel)) => {
                    let db_res = orchestrator
                        .db()
                        .query("SELECT * FROM tool WHERE id = $id")
                        .bind(("id", sel.tool_id.clone()))
                        .await;

                    match db_res {
                        Ok(mut res) => {
                            let tool_res = res.take::<Option<crate::db::ToolRecord>>(0);
                            match tool_res {
                                Ok(Some(tool)) => {
                                    json!({
                                        "status": "ok",
                                        "selection": {
                                            "toolId": tool.id.to_string(),
                                            "toolName": tool.name,
                                            "serviceId": tool.service_id.to_string(),
                                            "confidence": sel.confidence,
                                            "reasoning": sel.reasoning,
                                            "dependencies": sel.dependencies,
                                            "estimatedCost": sel.estimated_cost,
                                            "inputSchema": tool.input_schema,
                                            "outputSchema": tool.output_schema,
                                        }
                                    })
                                }
                                Ok(None) => {
                                    is_error = true;
                                    json!({
                                        "status": "error",
                                        "reason": "Selected tool not found in database"
                                    })
                                }
                                Err(e) => {
                                    is_error = true;
                                    json!({
                                        "status": "error",
                                        "reason": format!("Failed to decode ToolRecord: {}", e),
                                    })
                                }
                            }
                        }
                        Err(e) => {
                            is_error = true;
                            json!({
                                "status": "error",
                                "reason": format!("Database error while loading tool: {}", e),
                            })
                        }
                    }
                }
                Ok(None) => {
                    is_error = true;
                    json!({
                        "status": "no_match",
                        "reason": "No suitable tool was found for this query"
                    })
                }
                Err(e) => {
                    is_error = true;
                    json!({
                        "status": "error",
                        "reason": format!("Tool selection failed: {}", e),
                    })
                }
            };

            let text = serde_json::to_string(&payload)
                .unwrap_or_else(|_| "internal serialization error".to_string());
            Ok(CallToolResult {
                content: vec![Content::text(text)],
                structured_content: None,
                is_error: Some(is_error),
                meta: None,
            })
        })
    }
}
