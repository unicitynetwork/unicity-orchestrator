mod config;
mod mcp_client;
mod db;
mod knowledge_graph;
pub mod api;
mod executor;
pub mod server;

// Re-export key types and functions
pub use db::{DatabaseConfig, create_connection, ensure_schema, ToolRecord};
pub use knowledge_graph::{KnowledgeGraph, EmbeddingManager};
pub use config::McpServiceConfig;
use anyhow::Result;
use crate::db::queries::QueryBuilder;
use crate::db::schema::{ServiceCreate, CreateToolRecord, ServiceOrigin as DbServiceOrigin, TypedSchema};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use rmcp::{ServerHandler, model::{
    ServerInfo,
    ServerCapabilities,
    ProtocolVersion,
    Implementation,
    Tool as McpTool,
    JsonObject,
    ListToolsResult,
    Content,
    PaginatedRequestParam,
}, RoleServer};
use rmcp::model::{CallToolRequestMethod, CallToolRequestParam, CallToolResult};
use rmcp::service::RequestContext;
use crate::config::McpConfigs;
use crate::knowledge_graph::{SymbolicReasoner, ToolSelection};
use crate::mcp_client::RunningService;

pub struct UnicityOrchestrator {
    db: surrealdb::Surreal<surrealdb::engine::any::Any>,
    knowledge_graph: KnowledgeGraph,
    /// Embedding manager is wrapped in a Mutex so we can safely call async
    /// methods that require mutable access (e.g. updating embeddings) from
    /// methods that only have `&self` (such as MCP handlers).
    embedding_manager: Mutex<EmbeddingManager>,
    /// Symbolic reasoner is also wrapped in a Mutex to allow interior
    /// mutability for rule loading and inference without requiring `&mut self`
    /// on the orchestrator.
    symbolic_reasoner: Mutex<SymbolicReasoner>,
    // registry_manager: McpRegistryManager, // TODO
    running_services: HashMap<surrealdb::RecordId, Arc<RunningService>>,
}

/// A single step in a proposed multi-tool plan.
pub struct PlanStep {
    pub description: String,
    pub service_id: surrealdb::RecordId,
    pub tool_name: String,
    pub inputs: Vec<String>,
}

/// Result of planning: a sequence of steps plus overall confidence and reasoning.
pub struct PlanResult {
    pub steps: Vec<PlanStep>,
    pub confidence: f32,
    pub reasoning: String,
}

impl UnicityOrchestrator {
    pub async fn new(config: DatabaseConfig) -> Result<Self> {
        // Initialize database connection
        let db = create_connection(config).await?;
        ensure_schema(&db).await?;

        // Initialize components
        let knowledge_graph = KnowledgeGraph::new();
        let embedding_manager_inner = EmbeddingManager::new(
            db.clone(),
            knowledge_graph::embedding::EmbeddingConfig::default(),
        ).await?;
        let symbolic_reasoner_inner = SymbolicReasoner::new(db.clone());
        // let registry_manager = McpRegistryManager::new(db.clone()); // TODO

        Ok(Self {
            db,
            knowledge_graph,
            embedding_manager: Mutex::new(embedding_manager_inner),
            symbolic_reasoner: Mutex::new(symbolic_reasoner_inner),
            // registry_manager, // TODO
            running_services: HashMap::new(),
        })
    }

    /// High-level initialization entrypoint used by the binary. This is kept
    /// separate from the MCP `initialize` request handler (which is part of
    /// the `ServerHandler` trait) and simply runs the warmup pipeline.
    pub async fn initialize(&mut self) -> Result<()> {
        self.warmup().await
    }

    pub async fn warmup(&mut self) -> Result<()> {
        // In the future this can be made config-driven (e.g. whether to sync
        // registries on startup), but for now we perform a simple discovery
        // pass and then build the in-memory graph + load rules.

        // Discover services and tools from local MCP config and persist them.
        let _ = self.discover_tools().await?;

        // Normalize tool schemas into typed representations before building
        // the knowledge graph.
        self.normalize_tool_types().await?;

        // Run the embedding update pass so all tools get embeddings before
        // we build the knowledge graph and start serving queries.
        {
            let mut embedding_manager = self.embedding_manager.lock().await;
            embedding_manager.update_tool_embeddings().await?;
        }

        // Rebuild the knowledge graph from the current database state and
        // load symbolic rules so the orchestrator can answer queries.
        self.knowledge_graph = KnowledgeGraph::build_from_database(&self.db).await?;
        {
            let mut symbolic_reasoner = self.symbolic_reasoner.lock().await;
            symbolic_reasoner.load_rules().await?;
        }

        Ok(())
    }

    // TODO
    // pub async fn sync_registries(&mut self) -> Result<mcp::registry::SyncResult> {
    //     self.registry_manager.sync_all_registries().await
    // }

    // TODO, result should be stats about discovered services/tools
    pub async fn discover_tools(&mut self) -> Result<(usize, usize)> {
        // Start MCP services and discover tools
        let services = McpConfigs::load()?;
        let mut discovered_servers = 0;
        let mut discovered_tools = 0;

        for service_config in services {
            match mcp_client::start_service(&service_config).await {
                Ok(Some(running_service)) => {
                    match mcp_client::inspect_service(&running_service).await {
                        Ok((server_info, tools)) => {
                            let server_info = server_info.server_info;
                            let service = QueryBuilder::upsert_service(
                                &self.db,
                                &ServiceCreate {
                                    name: server_info.name.clone(),
                                    title: server_info.title.clone(),
                                    version: server_info.version.clone(),
                                    icons: server_info.icons.clone(),
                                    website_url: server_info.website_url.clone(),
                                    origin: DbServiceOrigin::StaticConfig,
                                    registry_id: None,
                                },
                            )
                                .await?;

                            // 🔹 keep this rmcp client alive, keyed by the Surreal service id
                            let service_id = service.id.clone();
                            let rc = Arc::new(running_service);
                            self.running_services.insert(service_id, rc.clone());

                            discovered_servers += 1;

                            for tool in tools {
                                let input_schema = (*tool.input_schema).clone();
                                let output_schema = tool
                                    .output_schema
                                    .as_ref()
                                    .map(|schema| (**schema).clone());

                                let create_tool = CreateToolRecord {
                                    service_id: service.id.clone(),
                                    name: tool.name.to_string(),
                                    description: tool
                                        .description
                                        .as_ref()
                                        .map(|d| d.to_string()),
                                    input_schema,
                                    output_schema,
                                    embedding_id: None,
                                    input_ty: None,
                                    output_ty: None,
                                };

                                let _tool_record =
                                    QueryBuilder::upsert_tool(&self.db, &create_tool).await?;
                                discovered_tools += 1;
                            }
                        }
                        Err(e) => tracing::error!("Failed to inspect service: {}", e),
                    }
                }
                Ok(None) => continue,
                Err(e) => tracing::error!("Failed to start service: {}", e),
            }
        }

        Ok((discovered_servers, discovered_tools))
    }

    pub async fn execute_selected_tool(
        &self,
        selection: &ToolSelection,
        args: JsonObject,
    ) -> Result<Vec<Content>> {
        executor::execute_selection(
            &self.db,
            &self.running_services,
            selection,
            args,
        ).await
    }

    /// Normalize tool input/output schemas into `TypedSchema` and persist them.
    ///
    /// This reads all tools from the database, derives a conservative structural
    /// type representation from their JSON Schemas, and writes `input_ty` /
    /// `output_ty` back to the `tool` table. It is intended to be called during
    /// startup before rebuilding the knowledge graph.
    pub async fn normalize_tool_types(&self) -> Result<()> {
        // Load all tools from the database. For now we normalize everything
        // eagerly; later this can be optimized or made incremental.
        let mut res = self.db.query("SELECT * FROM tool").await?;
        let tools: Vec<ToolRecord> = res.take(0)?;

        for tool in tools {
            let input_ty = TypedSchema::from_json_schema(&tool.input_schema);

            let output_ty = tool
                .output_schema
                .as_ref()
                .map(|schema| {
                    TypedSchema::from_json_schema(schema)
                });

            self.db
                .query(
                    r#"
                    UPDATE tool
                    SET input_ty = $input_ty,
                        output_ty = $output_ty,
                        updated_at = time::now()
                    WHERE id = $id
                    "#,
                )
                .bind(("id", tool.id.clone()))
                .bind(("input_ty", input_ty))
                .bind(("output_ty", output_ty))
                .await?;
        }

        Ok(())
    }

    pub async fn query_tools(
        &self,
        query: &str,
        context: Option<Value>,
    ) -> Result<Vec<ToolSelection>> {
        // First, use semantic search to find the most relevant tools for this
        // query. This keeps the symbolic reasoning step focused on a small,
        // semantically coherent subset.
        let semantic_hits = {
            let mut embedding_manager = self.embedding_manager.lock().await;
            embedding_manager
                .search_tools_by_embedding(query, 32, 0.25)
                .await?
        };

        let tools: Vec<ToolRecord> = if !semantic_hits.is_empty() {
            // Load only the tools that were returned by semantic search.
            let ids: Vec<_> = semantic_hits
                .iter()
                .map(|hit| hit.tool_id.clone())
                .collect();

            self.db
                .query("SELECT * FROM tool WHERE id IN $ids")
                .bind(("ids", ids))
                .await?
                .take(0)?
        } else {
            // Fallback: no semantic hits (or embeddings not yet available),
            // so load all tools and let the symbolic reasoner handle it.
            self.db
                .query("SELECT * FROM tool")
                .await?
                .take(0)?
        };

        let context_map = context
            .map(|c| serde_json::from_value(c).unwrap_or_default())
            .unwrap_or_default();

        let selections = {
            let mut symbolic_reasoner = self.symbolic_reasoner.lock().await;
            symbolic_reasoner
                .infer_tool_selection(query, &tools, &context_map)
                .await?
        };

        // If symbolic reasoning produced no selections, fall back to raw embedding hits.
        if selections.is_empty() && !semantic_hits.is_empty() {
            let mut fallback = Vec::new();

            for hit in &semantic_hits {
                if let Some(tool) = &hit.tool {
                    fallback.push(ToolSelection {
                        tool_id: tool.id.clone(),
                        tool_name: tool.name.clone(),
                        service_id: tool.service_id.clone(),
                        confidence: hit.similarity,
                        reasoning: format!(
                            "Selected by cosine similarity {:.3} to query embedding",
                            hit.similarity
                        ),
                        dependencies: Vec::new(),
                        estimated_cost: None,
                    });
                }
            }

            return Ok(fallback);
        }

        Ok(selections)
    }

    /// High-level convenience API: given a natural-language query and optional
    /// context, run the full selection pipeline (semantic search + symbolic
    /// reasoning) and return the single best tool selection, if any.
    ///
    /// This does not execute the tool; it only chooses which tool (and
    /// associated reasoning) should be used. Execution is handled by higher
    /// layers (e.g. an executor that calls into rmcp clients).
    pub async fn orchestrate_tool(
        &self,
        query: &str,
        context: Option<Value>,
    ) -> Result<Option<ToolSelection>> {
        let selections = self.query_tools(query, context).await?;
        // For now, we assume the symbolic reasoner returns selections ordered
        // by descending relevance/score, so we simply take the first one.
        Ok(selections.into_iter().next())
    }

    /// Construct a simple multi-step plan for a given query by:
    /// 1. Running the selection pipeline to get ranked tool candidates.
    /// 2. Taking the top N candidates.
    /// 3. Loading their tool records from the database.
    /// 4. Deriving an ordered list of steps with inferred input parameter names.
    ///
    /// This is intentionally conservative and can be extended later to use the
    /// full symbolic planning engine and knowledge graph for type-based
    /// chaining. For now it gives the LLM a structured set of candidate steps.
    pub async fn plan_tools_for_query(
        &self,
        query: &str,
        context: Option<Value>, // TODO
    ) -> Result<Option<PlanResult>> {
        // 1) Choose candidate tools using the same semantic filter as selection.
        let semantic_hits = {
            let mut embedding_manager = self.embedding_manager.lock().await;
            embedding_manager
                .search_tools_by_embedding(query, 32, 0.25)
                .await?
        };

        let tools: Vec<ToolRecord> = if !semantic_hits.is_empty() {
            let ids: Vec<_> = semantic_hits
                .iter()
                .map(|hit| hit.tool_id.clone())
                .collect();

            self.db
                .query("SELECT * FROM tool WHERE id IN $ids")
                .bind(("ids", ids))
                .await?
                .take(0)?
        } else {
            self.db
                .query("SELECT * FROM tool")
                .await?
                .take(0)?
        };

        if tools.is_empty() {
            return Ok(None);
        }

        // Build a lookup map so we can enrich plan steps with tool metadata.
        let mut tool_map: HashMap<surrealdb::RecordId, ToolRecord> = HashMap::new();
        for tool in &tools {
            tool_map.insert(tool.id.clone(), tool.clone());
        }

        // 2) Ask the symbolic reasoner to plan using its backward-chaining engine.
        let plan_opt = {
            let mut symbolic_reasoner = self.symbolic_reasoner.lock().await;
            symbolic_reasoner
                .plan_tools_for_goal(query, &tools, None)
                .await?
        };

        let plan = match plan_opt {
            Some(p) => p,
            None => return Ok(None),
        };

        if plan.steps.is_empty() {
            return Ok(None);
        }

        // 3) Convert symbolic ToolPlan steps into our MCP-facing PlanResult.
        let mut steps = Vec::new();
        for step in plan.steps {
            if let Some(tool) = tool_map.get(&step.tool_id) {
                // Prefer explicit inputs from the symbolic plan; fall back to schema if empty.
                let mut inputs: Vec<String> = step.inputs.keys().cloned().collect();

                if let Some(props) = tool.input_schema
                    .get("properties")
                    .and_then(|v| v.as_object())
                {
                    inputs.extend(props.keys().cloned());
                }

                let description = tool
                    .description
                    .clone()
                    .unwrap_or_else(|| {
                        format!("Step {}: call {}", step.step_number, tool.name)
                    });

                steps.push(PlanStep {
                    description,
                    service_id: tool.service_id.clone(),
                    tool_name: tool.name.clone(),
                    inputs,
                });
            }
        }

        if steps.is_empty() {
            return Ok(None);
        }

        let reasoning = format!(
            "Plan constructed by symbolic planner for goal \"{}\" using {} steps.",
            plan.goal,
            steps.len()
        );

        Ok(Some(PlanResult {
            steps,
            confidence: plan.confidence,
            reasoning,
        }))
    }
}

impl ServerHandler for UnicityOrchestrator {
    /// Basic server metadata exposed over MCP.
    ///
    /// This is used during the `initialize` handshake and by MCP clients
    /// (Claude, Cursor, etc.) to understand what this server does.
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            // Always advertise the latest protocol version we target.
            protocol_version: ProtocolVersion::V_2025_06_18,
            // Advertise that we support tools. Other capabilities (resources, prompts, etc.)
            // can be enabled later once they are wired into the orchestrator.
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            // Implementation metadata (name + version) pulled from build env.
            server_info: Implementation::from_build_env(),
            // High-level description for the LLM / client
            instructions: Some(
                "Unicity orchestrator that discovers MCP services, builds a SurrealDB-backed knowledge graph, \
                 and uses embeddings + symbolic reasoning to select and chain tools."
                    .to_string(),
            ),
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ListToolsResult, rmcp::model::ErrorData>> + Send + '_ {
        async move {
            // Shared input schema: { query: string, context?: object }
            let mut input_schema: JsonObject = JsonObject::new();
            input_schema.insert(
                "type".to_string(),
                Value::String("object".to_string()),
            );

            let mut properties = serde_json::Map::new();
            properties.insert(
                "query".to_string(),
                serde_json::json!({
                "type": "string",
                "description": "Natural-language query describing the user's goal.",
            }),
            );
            properties.insert(
                "context".to_string(),
                serde_json::json!({
                "type": "object",
                "description": "Optional JSON context to guide tool selection.",
                "additionalProperties": true,
            }),
            );

            input_schema.insert(
                "properties".to_string(),
                Value::Object(properties),
            );
            input_schema.insert(
                "required".to_string(),
                serde_json::json!(["query"]),
            );

            // === unicity.select_tool: single-step selection ===

            let mut select_output_schema: JsonObject = JsonObject::new();
            select_output_schema.insert(
                "type".to_string(),
                Value::String("object".to_string()),
            );
            select_output_schema.insert(
                "description".to_string(),
                Value::String(
                    "Selection result describing which underlying MCP tool to use and why.".to_string(),
                ),
            );

            let select_tool = McpTool {
                name: Cow::Borrowed("unicity.select_tool"),
                title: Some("Unicity Orchestrator: Select Tool".to_string()),
                description: Some(Cow::Owned(
                    "Given a natural-language query and optional context, return the most appropriate underlying MCP tool to use, without executing it.".to_string(),
                )),
                input_schema: Arc::new(input_schema.clone()),
                output_schema: Some(Arc::new(select_output_schema)),
                annotations: None,
                icons: None,
                meta: None,
            };

            // === unicity.plan_tools: multi-step planning ===

            let mut plan_output_schema: JsonObject = JsonObject::new();
            plan_output_schema.insert(
                "type".to_string(),
                Value::String("object".to_string()),
            );

            let mut plan_properties = serde_json::Map::new();
            plan_properties.insert(
                "steps".to_string(),
                serde_json::json!({
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
            plan_properties.insert(
                "confidence".to_string(),
                serde_json::json!({
                "type": "number",
                "description": "Overall confidence score for the proposed plan."
            }),
            );
            plan_properties.insert(
                "reasoning".to_string(),
                serde_json::json!({
                "type": "string",
                "description": "High-level explanation of why this plan was proposed."
            }),
            );

            plan_output_schema.insert(
                "properties".to_string(),
                Value::Object(plan_properties),
            );
            plan_output_schema.insert(
                "required".to_string(),
                serde_json::json!(["steps"]),
            );

            let plan_tool = McpTool {
                name: Cow::Borrowed("unicity.plan_tools"),
                title: Some("Unicity Orchestrator: Plan Tools".to_string()),
                description: Some(Cow::Owned(
                    "Given a higher-level goal, propose a multi-step plan using underlying MCP tools, without executing them.".to_string(),
                )),
                // Same input schema (query + context), different structured output.
                input_schema: Arc::new(input_schema.clone()),
                output_schema: Some(Arc::new(plan_output_schema)),
                annotations: None,
                icons: None,
                meta: None,
            };

            // === unicity.execute_tool: execute a selected tool by toolId ===

            let mut exec_input_schema: JsonObject = JsonObject::new();
            exec_input_schema.insert(
                "type".to_string(),
                Value::String("object".to_string()),
            );

            let mut exec_props = serde_json::Map::new();
            exec_props.insert(
                "toolId".to_string(),
                serde_json::json!({
                "type": "string",
                "description": "The orchestrator toolId of the tool to execute (e.g. 'tool:abc123')."
            }),
            );
            exec_props.insert(
                "args".to_string(),
                serde_json::json!({
                "type": "object",
                "description": "Arguments to pass to the underlying MCP tool, shaped according to its inputSchema.",
                "additionalProperties": true
            }),
            );

            exec_input_schema.insert(
                "properties".to_string(),
                Value::Object(exec_props),
            );
            exec_input_schema.insert(
                "required".to_string(),
                serde_json::json!(["toolId", "args"]),
            );

            let exec_tool = McpTool {
                name: Cow::Borrowed("unicity.execute_tool"),
                title: Some("Unicity Orchestrator: Execute Tool".to_string()),
                description: Some(Cow::Owned(
                    "Execute a previously selected underlying MCP tool by toolId with the given arguments.".to_string(),
                )),
                input_schema: Arc::new(exec_input_schema),
                output_schema: None, // you can add a structured output later
                annotations: None,
                icons: None,
                meta: None,
            };

            let mut result = ListToolsResult::default();
            result.tools.push(select_tool);
            result.tools.push(plan_tool);
            result.tools.push(exec_tool);

            Ok(result)
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, rmcp::model::ErrorData>> + Send + '_ {
        async move {
            let tool_name = request.name.as_ref();

            // Arguments are a JSON object; we expect { query: string, context?: object } for selection/plan.
            let args: JsonObject = request
                .arguments
                .clone()
                .unwrap_or_default();

            let query = match args.get("query").and_then(|v| v.as_str()) {
                Some(q) => q.to_string(),
                None => {
                    // For select_tool / plan_tools, `query` is required. For execute_tool we handle separately below.
                    if tool_name == "unicity.execute_tool" {
                        // execute_tool has a different schema; don't error here.
                    } else {
                        let payload = serde_json::json!({
                        "status": "error",
                        "reason": "unicity.select_tool / unicity.plan_tools requires a `query` string argument"
                    });
                        let text = serde_json::to_string(&payload)
                            .unwrap_or_else(|_| "internal serialization error".to_string());
                        let content = Content::text(text);

                        return Ok(CallToolResult {
                            content: vec![content],
                            structured_content: None,
                            is_error: Some(true),
                            meta: None,
                        });
                    }
                    String::new()
                }
            };

            let context_value = args.get("context").cloned();

            match tool_name {
                "unicity.select_tool" => {
                    // Single-step selection: use the orchestrator's selection pipeline.
                    let selection_result = self.orchestrate_tool(&query, context_value).await;

                    let mut is_error = false;
                    let payload = match selection_result {
                        Ok(Some(sel)) => {
                            // Load the full ToolRecord so we can include schemas.
                            let db_res = self.db
                                .query("SELECT * FROM $id")
                                .bind(("id", sel.tool_id.clone()))
                                .await;

                            match db_res {
                                Ok(mut res) => {
                                    let tool_res = res.take::<Option<ToolRecord>>(0);
                                    match tool_res {
                                        Ok(Some(tool)) => {
                                            serde_json::json!({
                                            "status": "ok",
                                            "selection": {
                                                "toolId": tool.id.to_string(),
                                                "toolName": tool.name,
                                                "serviceId": tool.service_id.to_string(),
                                                "confidence": sel.confidence,
                                                "reasoning": sel.reasoning,
                                                "dependencies": sel.dependencies,
                                                "estimatedCost": sel.estimated_cost,
                                                // Expose schemas so the LLM can fill args correctly
                                                "inputSchema": tool.input_schema,
                                                "outputSchema": tool.output_schema,
                                            }
                                        })
                                        }
                                        Ok(None) => {
                                            is_error = true;
                                            serde_json::json!({
                                            "status": "error",
                                            "reason": "Selected tool not found in database"
                                        })
                                        }
                                        Err(e) => {
                                            is_error = true;
                                            serde_json::json!({
                                            "status": "error",
                                            "reason": format!("Failed to decode ToolRecord: {}", e),
                                        })
                                        }
                                    }
                                }
                                Err(e) => {
                                    is_error = true;
                                    serde_json::json!({
                                    "status": "error",
                                    "reason": format!("Database error while loading tool: {}", e),
                                })
                                }
                            }
                        }
                        Ok(None) => {
                            is_error = true;
                            serde_json::json!({
                            "status": "no_match",
                            "reason": "No suitable tool was found for this query"
                        })
                        },
                        Err(e) => {
                            is_error = true;
                            serde_json::json!({
                            "status": "error",
                            "reason": format!("Tool selection failed: {}", e),
                        })
                        },
                    };

                    let text = serde_json::to_string(&payload)
                        .unwrap_or_else(|_| "internal serialization error".to_string());
                    let content = Content::text(text);

                    Ok(CallToolResult {
                        content: vec![content],
                        structured_content: None,
                        is_error: Some(is_error),
                        meta: None,
                    })
                }

                "unicity.plan_tools" => {
                    // Multi-step planning: use the orchestrator's planner to propose
                    // a sequence of tool invocations.
                    let plan_result = self.plan_tools_for_query(&query, context_value).await;

                    let mut is_error = false;
                    let payload = match plan_result {
                        Ok(Some(plan)) => {
                            let steps_json: Vec<_> = plan
                                .steps
                                .into_iter()
                                .map(|step| {
                                    serde_json::json!({
                                    "description": step.description,
                                    "serviceId": step.service_id.to_string(),
                                    "toolName": step.tool_name,
                                    "inputs": step.inputs,
                                })
                                })
                                .collect();

                            serde_json::json!({
                            "status": "ok",
                            "steps": steps_json,
                            "confidence": plan.confidence,
                            "reasoning": plan.reasoning,
                        })
                        }
                        Ok(None) => {
                            is_error = true;
                            serde_json::json!({
                            "status": "no_match",
                            "reason": "No suitable tools were found to construct a plan for this query"
                        })
                        }
                        Err(e) => {
                            is_error = true;
                            serde_json::json!({
                            "status": "error",
                            "reason": format!("Tool planning failed: {}", e),
                        })
                        }
                    };

                    let text = serde_json::to_string(&payload)
                        .unwrap_or_else(|_| "internal serialization error".to_string());
                    let content = Content::text(text);

                    Ok(CallToolResult {
                        content: vec![content],
                        structured_content: None,
                        is_error: Some(is_error),
                        meta: None,
                    })
                }

                "unicity.execute_tool" => {
                    // Execute a previously selected tool by toolId with given args.
                    let args_obj: JsonObject = request
                        .arguments
                        .clone()
                        .unwrap_or_default();

                    let tool_id_str = match args_obj.get("toolId").and_then(|v| v.as_str()) {
                        Some(s) => s.to_string(),
                        None => {
                            let payload = serde_json::json!({
                            "status": "error",
                            "reason": "unicity.execute_tool requires a `toolId` string"
                        });
                            let text = serde_json::to_string(&payload)
                                .unwrap_or_else(|_| "internal serialization error".to_string());
                            let content = Content::text(text);

                            return Ok(CallToolResult {
                                content: vec![content],
                                structured_content: None,
                                is_error: Some(true),
                                meta: None,
                            });
                        }
                    };

                    // Arguments to pass to underlying MCP tool.
                    let tool_args: JsonObject = args_obj
                        .get("args")
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_default();

                    // Look up tool by id string using type::thing so we don't parse RecordId in Rust.
                    let db_res = self.db
                        .query("SELECT * FROM type::thing($id)")
                        .bind(("id", tool_id_str.clone()))
                        .await;

                    let mut is_error = false;

                    let tool: Option<ToolRecord> = match db_res {
                        Ok(mut res) => {
                            match res.take(0) {
                                Ok(t) => t,
                                Err(e) => {
                                    is_error = true;
                                    let payload = serde_json::json!({
                                    "status": "error",
                                    "reason": format!("Failed to decode ToolRecord: {}", e),
                                });
                                    let text = serde_json::to_string(&payload)
                                        .unwrap_or_else(|_| "internal serialization error".to_string());
                                    let content = Content::text(text);

                                    return Ok(CallToolResult {
                                        content: vec![content],
                                        structured_content: None,
                                        is_error: Some(true),
                                        meta: None,
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            is_error = true;
                            let payload = serde_json::json!({
                            "status": "error",
                            "reason": format!("Database error while loading tool: {}", e),
                        });
                            let text = serde_json::to_string(&payload)
                                .unwrap_or_else(|_| "internal serialization error".to_string());
                            let content = Content::text(text);

                            return Ok(CallToolResult {
                                content: vec![content],
                                structured_content: None,
                                is_error: Some(true),
                                meta: None,
                            });
                        }
                    };

                    let tool = match tool {
                        Some(t) => t,
                        None => {
                            let payload = serde_json::json!({
                            "status": "error",
                            "reason": format!("No tool found with id {}", tool_id_str),
                        });
                            let text = serde_json::to_string(&payload)
                                .unwrap_or_else(|_| "internal serialization error".to_string());
                            let content = Content::text(text);

                            return Ok(CallToolResult {
                                content: vec![content],
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

                    let exec_res = self.execute_selected_tool(&selection, tool_args).await;

                    match exec_res {
                        Ok(contents) => {
                            // Pass through underlying MCP content directly.
                            Ok(CallToolResult {
                                content: contents,
                                structured_content: None,
                                is_error: Some(false),
                                meta: None,
                            })
                        }
                        Err(e) => {
                            let payload = serde_json::json!({
                            "status": "error",
                            "reason": format!("Tool execution failed: {}", e),
                        });
                            let text = serde_json::to_string(&payload)
                                .unwrap_or_else(|_| "internal serialization error".to_string());
                            let content = Content::text(text);

                            Ok(CallToolResult {
                                content: vec![content],
                                structured_content: None,
                                is_error: Some(true),
                                meta: None,
                            })
                        }
                    }
                }

                _ => Err(rmcp::model::ErrorData::method_not_found::<CallToolRequestMethod>()),
            }
        }
    }
}
