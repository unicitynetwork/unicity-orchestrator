//! Handler for the `unicity.plan_tools` tool.
//!
//! Given a higher-level goal, proposes a multi-step plan using underlying
//! MCP tools, without executing them.

use std::pin::Pin;
use std::sync::Arc;
use rmcp::model::{CallToolResult, Content, JsonObject};
use serde_json::json;
use crate::orchestrator::Orchestrator;
use crate::tools::{ToolHandler, ToolContext};

/// Handler for the `unicity.plan_tools` tool.
pub struct PlanToolsHandler {
    orchestrator: Arc<Orchestrator>,
}

impl PlanToolsHandler {
    /// Create a new plan tools handler.
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
                "description": "Optional JSON context to guide tool planning.",
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

        let mut properties = serde_json::Map::new();
        properties.insert(
            "steps".to_string(),
            json!({
                "type": "array",
                "description": "Proposed sequence of tool invocations to achieve the goal.",
                "items": {
                    "type": "object",
                    "properties": {
                        "description": { "type": "string" },
                        "serviceId":  { "type": "string" },
                        "toolName":   { "type": "string" },
                        "inputs": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["description", "serviceId", "toolName"]
                }
            }),
        );
        properties.insert(
            "confidence".to_string(),
            json!({
                "type": "number",
                "description": "Overall confidence score for the proposed plan."
            }),
        );
        properties.insert(
            "reasoning".to_string(),
            json!({
                "type": "string",
                "description": "High-level explanation of why this plan was proposed."
            }),
        );

        schema.insert("properties".to_string(), json!(properties));
        schema.insert("required".to_string(), json!(["steps"]));
        schema
    }
}

impl ToolHandler for PlanToolsHandler {
    fn name(&self) -> &str {
        "unicity.plan_tools"
    }

    fn title(&self) -> Option<&str> {
        Some("Unicity Orchestrator: Plan Tools")
    }

    fn description(&self) -> &str {
        "Given a higher-level goal, propose a multi-step plan using underlying MCP tools, without executing them."
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
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<CallToolResult>> + Send + '_>> {
        let orchestrator = self.orchestrator.clone();

        Box::pin(async move {
            let query = match args.get("query").and_then(|v| v.as_str()) {
                Some(q) => q.to_string(),
                None => {
                    let payload = json!({
                        "status": "error",
                        "reason": "unicity.plan_tools requires a `query` string argument"
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

            let plan_result = orchestrator
                .plan_tools_for_query(&query, context_value)
                .await;

            let mut is_error = false;
            let payload = match plan_result {
                Ok(Some(plan)) => {
                    let steps_json: Vec<_> = plan
                        .steps
                        .into_iter()
                        .map(|step| {
                            json!({
                                "description": step.description,
                                "serviceId": step.service_id.to_string(),
                                "toolName": step.tool_name,
                                "inputs": step.inputs,
                            })
                        })
                        .collect();

                    json!({
                        "status": "ok",
                        "steps": steps_json,
                        "confidence": plan.confidence,
                        "reasoning": plan.reasoning,
                    })
                }
                Ok(None) => {
                    is_error = true;
                    json!({
                        "status": "no_match",
                        "reason": "No suitable tools were found to construct a plan for this query"
                    })
                }
                Err(e) => {
                    is_error = true;
                    json!({
                        "status": "error",
                        "reason": format!("Tool planning failed: {}", e),
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
