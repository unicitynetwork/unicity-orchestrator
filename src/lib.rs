pub mod model;
pub mod config;
pub mod mcp_client;

// Core modules
pub mod db;
pub mod knowledge_graph;
pub mod mcp;
pub mod core;
pub mod api;
pub mod utils;

// Re-export key types and functions
pub use db::{DatabaseConfig, create_connection, ensure_schema, ToolRecord};
pub use knowledge_graph::{KnowledgeGraph, EmbeddingManager};
pub use mcp::registry::{McpRegistryManager, RegistryConfig};
pub use config::{load_mcp_services, McpServiceConfig};

use anyhow::Result;
use serde_json;

use crate::db::queries::QueryBuilder;
use crate::db::schema::{ServiceCreate, CreateToolRecord, ServiceOrigin as DbServiceOrigin, TypedSchema};
use serde_json::Value;

use rmcp::{
    ServerHandler,
    model::{ServerInfo, ServerCapabilities, ProtocolVersion, Implementation},
};
use crate::knowledge_graph::{SymbolicReasoner, ToolSelection};

pub struct UnicityOrchestrator {
    db: surrealdb::Surreal<surrealdb::engine::any::Any>,
    knowledge_graph: KnowledgeGraph,
    embedding_manager: EmbeddingManager,
    symbolic_reasoner: SymbolicReasoner,
    registry_manager: McpRegistryManager,
}

impl UnicityOrchestrator {
    pub async fn new(config: DatabaseConfig) -> Result<Self> {
        // Initialize database connection
        let db = create_connection(config).await?;
        ensure_schema(&db).await?;

        // Initialize components
        let knowledge_graph = KnowledgeGraph::new();
        let embedding_manager = EmbeddingManager::new(
            db.clone(),
            knowledge_graph::embedding::EmbeddingConfig::default(),
        ).await?;
        let symbolic_reasoner = SymbolicReasoner::new(db.clone());
        let registry_manager = McpRegistryManager::new(db.clone());

        Ok(Self {
            db,
            knowledge_graph,
            embedding_manager,
            symbolic_reasoner,
            registry_manager,
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
        self.embedding_manager.update_tool_embeddings().await?;

        // Rebuild the knowledge graph from the current database state and
        // load symbolic rules so the orchestrator can answer queries.
        self.knowledge_graph = KnowledgeGraph::build_from_database(&self.db).await?;
        self.symbolic_reasoner.load_rules().await?;

        Ok(())
    }

    pub async fn sync_registries(&mut self) -> Result<mcp::registry::SyncResult> {
        self.registry_manager.sync_all_registries().await
    }

    // TODO, result should be stats about discovered services/tools
    pub async fn discover_tools(&mut self) -> Result<(usize, usize)> {
        // Start MCP services and discover tools
        let services = load_mcp_services()?;
        let mut discovered_servers = 0;
        let mut discovered_tools = 0;

        for service_config in services {
            match mcp_client::start_stdio_service(&service_config).await {
                Ok(Some(running_service)) => {
                    match mcp_client::inspect_service(&running_service).await {
                        Ok((server_info, tools)) => {
                            // Persist the service in the database. For now we
                            // treat all discovered services as coming from
                            // static configuration; registry/broadcast origins
                            // can override this when those flows are wired in.
                            let server_info = server_info.server_info;
                            let service = QueryBuilder::upsert_service(
                                &self.db,
                                &ServiceCreate {
                                    name: server_info.name.clone(),
                                    title: server_info.title.clone(),
                                    version: server_info.version.clone(),
                                    // icons: server_info.icons.clone(),
                                    website_url: server_info.website_url.clone(),
                                    origin: DbServiceOrigin::StaticConfig,
                                    registry_id: None,
                                },
                            )
                            .await?;

                            discovered_servers += 1;

                            for tool in tools {
                                let input_schema = Value::Object((*tool.input_schema).clone());
                                let output_schema = tool
                                    .output_schema
                                    .as_ref()
                                    .map(|schema| Value::Object((**schema).clone()));

                                let create_tool = CreateToolRecord {
                                    service_id: service.id.clone(),
                                    name: tool.name.to_string(),
                                    description: tool
                                        .description
                                        .as_ref()
                                        .map(|d| d.to_string()),
                                    input_schema: Some(input_schema),
                                    output_schema,
                                    embedding_id: None,
                                    input_ty: None,
                                    output_ty: None,
                                };

                                let _tool_record = QueryBuilder::upsert_tool(&self.db, &create_tool).await?;
                                discovered_tools += 1;
                            }
                        }
                        Err(e) => tracing::error!("Failed to inspect service: {}", e),
                    }
                }
                Ok(None) => continue, // Service disabled or not stdio
                Err(e) => tracing::error!("Failed to start service: {}", e),
            }
        }

        Ok((discovered_servers, discovered_tools))
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
            let input_ty = tool
                .input_schema
                .as_ref()
                .map(|schema| TypedSchema::from_json_schema(schema));

            let output_ty = tool
                .output_schema
                .as_ref()
                .map(|schema| TypedSchema::from_json_schema(schema));

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
        &mut self,
        query: &str,
        context: Option<Value>,
    ) -> Result<Vec<ToolSelection>> {
        // First, use semantic search to find the most relevant tools for this
        // query. This keeps the symbolic reasoning step focused on a small,
        // semantically coherent subset.
        let semantic_hits = self
            .embedding_manager
            .search_tools_by_embedding(query, 32, 0.25)
            .await?;

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

        self.symbolic_reasoner
            .infer_tool_selection(query, &tools, &context_map)
            .await
    }

    /// High-level convenience API: given a natural-language query and optional
    /// context, run the full selection pipeline (semantic search + symbolic
    /// reasoning) and return the single best tool selection, if any.
    ///
    /// This does not execute the tool; it only chooses which tool (and
    /// associated reasoning) should be used. Execution is handled by higher
    /// layers (e.g. an executor that calls into rmcp clients).
    pub async fn orchestrate_tool(
        &mut self,
        query: &str,
        context: Option<Value>,
    ) -> Result<Option<ToolSelection>> {
        let selections = self.query_tools(query, context).await?;
        // For now, we assume the symbolic reasoner returns selections ordered
        // by descending relevance/score, so we simply take the first one.
        Ok(selections.into_iter().next())
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
}
