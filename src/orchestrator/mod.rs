//! Core orchestrator logic - the "brain" that handles tool selection,
//! planning, and execution using semantic search and symbolic reasoning.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use surrealdb::engine::any::Any;
use surrealdb::{RecordId, Surreal};
use anyhow::Result;
use serde_json::Value;

use crate::db::{DatabaseConfig, create_connection, ensure_schema, ToolRecord};
use crate::knowledge_graph::{KnowledgeGraph, EmbeddingManager, SymbolicReasoner, ToolSelection};
use crate::config::McpConfigs;
use crate::mcp_client::RunningService;
use rmcp::model::JsonObject;

/// A single step in a proposed multi-tool plan.
#[derive(Debug, Clone)]
pub struct PlanStep {
    pub description: String,
    pub service_id: RecordId,
    pub tool_name: String,
    pub inputs: Vec<String>,
}

/// Result of planning: a sequence of steps plus overall confidence and reasoning.
#[derive(Debug, Clone)]
pub struct PlanResult {
    pub steps: Vec<PlanStep>,
    pub confidence: f32,
    pub reasoning: String,
}

/// The core orchestrator - uses embeddings + symbolic reasoning to select and chain tools.
pub struct Orchestrator {
    db: Surreal<Any>,
    knowledge_graph: KnowledgeGraph,
    embedding_manager: Mutex<EmbeddingManager>,
    symbolic_reasoner: Mutex<SymbolicReasoner>,
    running_services: HashMap<RecordId, Arc<RunningService>>,
}

impl Orchestrator {
    /// Create a new orchestrator with the given database configuration.
    pub async fn new(config: DatabaseConfig) -> Result<Self> {
        let db = create_connection(config).await?;
        ensure_schema(&db).await?;

        let knowledge_graph = KnowledgeGraph::new();
        let embedding_manager_inner = EmbeddingManager::new(
            db.clone(),
            crate::knowledge_graph::embedding::EmbeddingConfig::default(),
        ).await?;
        let symbolic_reasoner_inner = SymbolicReasoner::new(db.clone());

        Ok(Self {
            db,
            knowledge_graph,
            embedding_manager: Mutex::new(embedding_manager_inner),
            symbolic_reasoner: Mutex::new(symbolic_reasoner_inner),
            running_services: HashMap::new(),
        })
    }

    /// Initialize the orchestrator - run warmup pipeline.
    pub async fn initialize(&mut self) -> Result<()> {
        self.warmup().await
    }

    /// Warmup pipeline: discover tools, normalize types, update embeddings, build graph.
    pub async fn warmup(&mut self) -> Result<()> {
        // Discover services and tools from local MCP config
        let _ = self.discover_tools().await?;

        // Normalize tool schemas into typed representations
        self.normalize_tool_types().await?;

        // Update embeddings for all tools
        {
            let mut embedding_manager = self.embedding_manager.lock().await;
            embedding_manager.update_tool_embeddings().await?;
        }

        // Rebuild knowledge graph and load symbolic rules
        self.knowledge_graph = KnowledgeGraph::build_from_database(&self.db).await?;
        {
            let mut symbolic_reasoner = self.symbolic_reasoner.lock().await;
            symbolic_reasoner.load_rules().await?;
        }

        Ok(())
    }

    /// Discover MCP services and tools from local config.
    pub async fn discover_tools(&mut self) -> Result<(usize, usize)> {
        let services = McpConfigs::load()?;
        let mut discovered_servers = 0;
        let mut discovered_tools = 0;

        for service_config in services {
            match crate::mcp_client::start_service(&service_config).await {
                Ok(Some(running_service)) => {
                    match crate::mcp_client::inspect_service(&running_service).await {
                        Ok((server_info, tools)) => {
                            let server_info = server_info.server_info;
                            let service = crate::db::queries::QueryBuilder::upsert_service(
                                &self.db,
                                &crate::db::schema::ServiceCreate {
                                    name: server_info.name.clone(),
                                    title: server_info.title.clone(),
                                    version: server_info.version.clone(),
                                    icons: server_info.icons.clone(),
                                    website_url: server_info.website_url.clone(),
                                    origin: crate::db::schema::ServiceOrigin::StaticConfig,
                                    registry_id: None,
                                },
                            ).await?;

                            let service_id = service.id.clone();
                            let rc = Arc::new(running_service);
                            self.running_services.insert(service_id.clone(), rc.clone());
                            discovered_servers += 1;

                            for tool in tools {
                                let input_schema = (*tool.input_schema).clone();
                                let output_schema = tool
                                    .output_schema
                                    .as_ref()
                                    .map(|schema| (**schema).clone());

                                let create_tool = crate::db::schema::CreateToolRecord {
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
                                    crate::db::queries::QueryBuilder::upsert_tool(&self.db, &create_tool).await?;
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

    /// Normalize tool input/output schemas into `TypedSchema` and persist them.
    pub async fn normalize_tool_types(&self) -> Result<()> {
        let mut res = self.db.query("SELECT * FROM tool").await?;
        let tools: Vec<ToolRecord> = res.take(0)?;

        for tool in tools {
            let input_ty = crate::db::schema::TypedSchema::from_json_schema(&tool.input_schema);
            let output_ty = tool
                .output_schema
                .as_ref()
                .map(|schema| {
                    crate::db::schema::TypedSchema::from_json_schema(schema)
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

    /// Query tools using semantic search + symbolic reasoning.
    pub async fn query_tools(
        &self,
        query: &str,
        context: Option<Value>,
    ) -> Result<Vec<ToolSelection>> {
        // Semantic search first
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

        let context_map = context
            .map(|c| serde_json::from_value(c).unwrap_or_default())
            .unwrap_or_default();

        let selections = {
            let mut symbolic_reasoner = self.symbolic_reasoner.lock().await;
            symbolic_reasoner
                .infer_tool_selection(query, &tools, &context_map)
                .await?
        };

        // Fallback to raw embedding hits if symbolic reasoning produced nothing
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

    /// Get the single best tool for a query.
    pub async fn orchestrate_tool(
        &self,
        query: &str,
        context: Option<Value>,
    ) -> Result<Option<ToolSelection>> {
        let selections = self.query_tools(query, context).await?;
        Ok(selections.into_iter().next())
    }

    /// Plan a multi-step tool sequence for a query.
    pub async fn plan_tools_for_query(
        &self,
        query: &str,
        _context: Option<Value>,
    ) -> Result<Option<PlanResult>> {
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

        let mut tool_map: HashMap<RecordId, ToolRecord> = HashMap::new();
        for tool in &tools {
            tool_map.insert(tool.id.clone(), tool.clone());
        }

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

        let mut steps = Vec::new();
        for step in plan.steps {
            if let Some(tool) = tool_map.get(&step.tool_id) {
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

    /// Execute a selected tool.
    pub async fn execute_selected_tool(
        &self,
        selection: &ToolSelection,
        args: JsonObject,
    ) -> Result<Vec<rmcp::model::Content>> {
        crate::executor::execute_selection(
            &self.db,
            &self.running_services,
            selection,
            args,
        ).await
    }

    /// Get reference to the database.
    pub fn db(&self) -> &Surreal<Any> {
        &self.db
    }

    /// Get reference to the knowledge graph.
    pub fn knowledge_graph(&self) -> &KnowledgeGraph {
        &self.knowledge_graph
    }

    /// Get reference to running services map.
    pub fn running_services(&self) -> &HashMap<RecordId, Arc<RunningService>> {
        &self.running_services
    }
}
