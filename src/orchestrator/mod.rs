//! Core orchestrator logic - the "brain" that handles tool selection,
//! planning, and execution using semantic search and symbolic reasoning.

pub mod user_filter;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use surrealdb::engine::any::Any;
use surrealdb::{RecordId, Surreal};
use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::auth::UserContext;
use crate::db::{DatabaseConfig, create_connection, ensure_schema, ToolRecord, ServiceRecord};
use crate::db::schema::{AuditAction, AuditLogCreate};
use crate::knowledge_graph::{KnowledgeGraph, EmbeddingManager, SymbolicReasoner, ToolSelection};
use crate::config::McpConfigs;
use crate::mcp_client::RunningService;
use crate::prompts::{PromptRegistry, PromptForwarder};
use crate::resources::{ResourceRegistry, ResourceForwarder};
use crate::elicitation::{ElicitationCoordinator, ElicitationFallbackPolicy, ApprovalRequest, PermissionStatus};
use crate::types::{ExternalUserId, ServiceId, ServiceName, ToolId};
use rmcp::model::JsonObject;
use std::sync::Arc as StdArc;
use tokio::sync::Mutex as TokioMutex;

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
    prompt_forwarder: StdArc<PromptForwarder>,
    resource_forwarder: StdArc<ResourceForwarder>,
    elicitation_coordinator: StdArc<ElicitationCoordinator>,
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

        // Initialize prompt registry and forwarder
        let prompt_registry = StdArc::new(TokioMutex::new(PromptRegistry::new()));
        let running_services_arc: StdArc<TokioMutex<HashMap<String, StdArc<RunningService>>>> =
            StdArc::new(TokioMutex::new(HashMap::new()));
        let prompt_forwarder = StdArc::new(PromptForwarder::new(
            prompt_registry,
            running_services_arc.clone(),
            db.clone(),
        ));

        // Initialize resource registry and forwarder
        let resource_registry = StdArc::new(TokioMutex::new(ResourceRegistry::new()));
        let resource_forwarder = StdArc::new(ResourceForwarder::new(
            resource_registry,
            running_services_arc.clone(),
            db.clone(),
        ));

        // Initialize elicitation coordinator
        let elicitation_coordinator = StdArc::new(ElicitationCoordinator::new(db.clone())?);

        Ok(Self {
            db,
            knowledge_graph,
            embedding_manager: Mutex::new(embedding_manager_inner),
            symbolic_reasoner: Mutex::new(symbolic_reasoner_inner),
            running_services: HashMap::new(),
            prompt_forwarder,
            resource_forwarder,
            elicitation_coordinator,
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

        // Discover prompts from all running services
        let _ = self.discover_prompts().await?;

        // Discover resources from all running services
        let _ = self.discover_resources().await?;

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

                            // Also add to prompt_forwarder's running_services map
                            {
                                let mut services_map = self.prompt_forwarder.running_services.lock().await;
                                services_map.insert(service_id.to_string(), rc.clone());
                            }

                            // Also add to resource_forwarder's running_services map
                            {
                                let mut services_map = self.resource_forwarder.running_services.lock().await;
                                services_map.insert(service_id.to_string(), rc.clone());
                            }

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
    ///
    /// # Arguments
    /// * `query` - Natural language query describing the desired tool
    /// * `context` - Optional JSON context to guide tool selection
    /// * `user_context` - Optional user context for multi-tenant filtering
    pub async fn query_tools(
        &self,
        query: &str,
        context: Option<Value>,
        user_context: Option<&UserContext>,
    ) -> Result<Vec<ToolSelection>> {
        // Import user filter for multi-tenant filtering
        use crate::orchestrator::user_filter::UserToolFilter;

        // Create user filter based on user context
        let filter = match user_context {
            Some(ctx) => UserToolFilter::from_user_context(&self.db, ctx).await?,
            None => UserToolFilter::allow_all(),
        };
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

        // Apply user filter to tools (removes blocked services)
        let tools = filter.filter_tools(tools);

        let context_map = context
            .map(|c| serde_json::from_value(c).unwrap_or_default())
            .unwrap_or_default();

        let mut selections = {
            let mut symbolic_reasoner = self.symbolic_reasoner.lock().await;
            symbolic_reasoner
                .infer_tool_selection(query, &tools, &context_map)
                .await?
        };

        // Apply trust boost for trusted services
        filter.apply_trust_boost(&mut selections, 0.1);

        // Fallback to raw embedding hits if symbolic reasoning produced nothing
        if selections.is_empty() && !semantic_hits.is_empty() {
            let mut fallback = Vec::new();

            for hit in &semantic_hits {
                if let Some(tool) = &hit.tool {
                    // Skip blocked tools in fallback
                    if !filter.is_tool_allowed(tool) {
                        continue;
                    }
                    let mut confidence = hit.similarity;
                    // Apply trust boost in fallback
                    if filter.is_service_trusted(&tool.service_id) {
                        confidence = (confidence + 0.1).min(1.0);
                    }
                    fallback.push(ToolSelection {
                        tool_id: tool.id.clone(),
                        tool_name: tool.name.clone(),
                        service_id: tool.service_id.clone(),
                        confidence,
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
    ///
    /// # Arguments
    /// * `query` - Natural language query describing the desired tool
    /// * `context` - Optional JSON context to guide tool selection
    /// * `user_context` - Optional user context for multi-tenant filtering
    pub async fn orchestrate_tool(
        &self,
        query: &str,
        context: Option<Value>,
        user_context: Option<&UserContext>,
    ) -> Result<Option<ToolSelection>> {
        let selections = self.query_tools(query, context, user_context).await?;
        Ok(selections.into_iter().next())
    }

    /// Plan a multi-step tool sequence for a query.
    ///
    /// # Arguments
    /// * `query` - Natural language query describing the goal
    /// * `context` - Optional JSON context to guide tool planning
    /// * `user_context` - Optional user context for multi-tenant filtering
    pub async fn plan_tools_for_query(
        &self,
        query: &str,
        _context: Option<Value>,
        user_context: Option<&UserContext>,
    ) -> Result<Option<PlanResult>> {
        // Import user filter for multi-tenant filtering
        use crate::orchestrator::user_filter::UserToolFilter;

        // Create user filter based on user context
        let filter = match user_context {
            Some(ctx) => UserToolFilter::from_user_context(&self.db, ctx).await?,
            None => UserToolFilter::allow_all(),
        };

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

        // Apply user filter to tools (removes blocked services)
        let tools = filter.filter_tools(tools);

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

    /// Execute a selected tool (without approval checks - for internal use).
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

    /// Execute a selected tool with approval checks.
    ///
    /// This method checks if the user has permission to execute the tool.
    /// If not, it will send an elicitation request to the client asking for
    /// approval (allow once, always allow, or deny).
    ///
    /// # Arguments
    /// * `selection` - The tool selection to execute
    /// * `args` - Arguments to pass to the tool
    /// * `user_context` - Optional user context for permission checks
    ///
    /// # Returns
    /// * `Ok(contents)` - Tool execution results if approved
    /// * `Err` - If denied, cancelled, or execution failed
    pub async fn execute_selected_tool_with_approval(
        &self,
        selection: &ToolSelection,
        args: JsonObject,
        user_context: Option<&UserContext>,
    ) -> Result<Vec<rmcp::model::Content>> {
        // Get user ID - use "anonymous" for stdio/local mode
        let user_id = ExternalUserId::new(
            user_context
                .map(|ctx| ctx.user_id_string())
                .unwrap_or_else(|| "anonymous".to_string())
        );

        // Look up the service to get its name for the approval message
        let service_name = ServiceName::new(
            self.get_service_name(&selection.service_id)
                .await
                .unwrap_or_else(|| selection.service_id.to_string())
        );

        let tool_id = ToolId::new(selection.tool_id.to_string());
        let service_id = ServiceId::new(selection.service_id.to_string());

        // Check existing permission
        let approval_manager = self.elicitation_coordinator.approval_manager();
        let permission_status = approval_manager
            .check_permission(&tool_id, &service_id, &user_id)
            .await
            .map_err(|e| anyhow!("Failed to check permission: {:?}", e))?;

        match permission_status {
            PermissionStatus::Granted => {
                // Permission already granted - execute the tool
                tracing::debug!(
                    tool_id = %tool_id,
                    user_id = %user_id,
                    "Tool execution approved (existing permission)"
                );
                let result = self.execute_selected_tool(selection, args).await;

                // Audit log the execution
                self.audit_log(AuditLogCreate {
                    user_id: Some(user_id.to_string()),
                    action: AuditAction::ToolExecuted.as_str().to_string(),
                    resource_type: "tool".to_string(),
                    resource_id: Some(tool_id.to_string()),
                    details: Some(serde_json::json!({
                        "service_id": service_id.to_string(),
                        "service_name": service_name.to_string(),
                        "success": result.is_ok(),
                        "permission_type": "existing",
                    })),
                    ip_address: user_context.and_then(|ctx| ctx.ip_address().map(|s| s.to_string())),
                    user_agent: user_context.and_then(|ctx| ctx.user_agent().map(|s| s.to_string())),
                }).await;

                result
            }
            PermissionStatus::Denied => {
                // User previously denied this tool
                self.audit_log(AuditLogCreate {
                    user_id: Some(user_id.to_string()),
                    action: AuditAction::PermissionDenied.as_str().to_string(),
                    resource_type: "tool".to_string(),
                    resource_id: Some(tool_id.to_string()),
                    details: Some(serde_json::json!({
                        "service_id": service_id.to_string(),
                        "reason": "previously_denied",
                    })),
                    ip_address: user_context.and_then(|ctx| ctx.ip_address().map(|s| s.to_string())),
                    user_agent: user_context.and_then(|ctx| ctx.user_agent().map(|s| s.to_string())),
                }).await;

                Err(anyhow!("Tool execution denied by user"))
            }
            PermissionStatus::Expired => {
                // Permission expired - need to re-approve
                self.request_tool_approval(
                    selection,
                    args,
                    &tool_id,
                    &service_id,
                    &service_name,
                    &user_id,
                ).await
            }
            PermissionStatus::Required => {
                // No permission yet - need to request approval
                self.request_tool_approval(
                    selection,
                    args,
                    &tool_id,
                    &service_id,
                    &service_name,
                    &user_id,
                ).await
            }
        }
    }

    /// Request approval from the user via elicitation.
    async fn request_tool_approval(
        &self,
        selection: &ToolSelection,
        args: JsonObject,
        tool_id: &ToolId,
        service_id: &ServiceId,
        service_name: &ServiceName,
        user_id: &ExternalUserId,
    ) -> Result<Vec<rmcp::model::Content>> {
        // Check if client supports elicitation
        if !self.elicitation_coordinator.client_supports_elicitation().await {
            // Client doesn't support elicitation - check fallback policy
            let policy = self.elicitation_coordinator.fallback_policy().await;
            match policy {
                ElicitationFallbackPolicy::Allow => {
                    tracing::warn!(
                        tool_id = %tool_id,
                        "Client does not support elicitation, allowing tool execution (fallback policy: allow)"
                    );
                    return self.execute_selected_tool(selection, args).await;
                }
                ElicitationFallbackPolicy::Deny => {
                    tracing::warn!(
                        tool_id = %tool_id,
                        "Client does not support elicitation, denying tool execution (fallback policy: deny)"
                    );
                    return Err(anyhow!(
                        "Tool execution denied: client does not support elicitation and fallback policy is set to deny"
                    ));
                }
            }
        }

        let approval_manager = self.elicitation_coordinator.approval_manager();

        // Create the approval request
        let request = ApprovalRequest {
            tool_id: tool_id.clone(),
            service_id: service_id.clone(),
            service_name: service_name.clone(),
            user_id: user_id.clone(),
            arguments: Some(serde_json::to_value(&args).unwrap_or_default()),
        };

        // Create the elicitation schema and message
        let (message, schema) = approval_manager.create_approval_elicitation(&request);

        tracing::info!(
            tool_id = %tool_id,
            service_name = %service_name,
            user_id = %user_id,
            "Requesting tool approval from user"
        );

        // Send the elicitation request
        let result = self.elicitation_coordinator
            .create_elicitation(&message, schema)
            .await
            .map_err(|e| anyhow!("Failed to send elicitation request: {:?}", e))?;

        // Handle the response
        let permission_status = approval_manager
            .handle_approval_response(&request, &result)
            .await
            .map_err(|e| anyhow!("Failed to handle approval response: {:?}", e))?;

        match permission_status {
            PermissionStatus::Granted => {
                tracing::info!(
                    tool_id = %tool_id,
                    user_id = %user_id,
                    "Tool execution approved by user"
                );

                // Determine permission type from response
                let permission_type = result.content.as_ref()
                    .and_then(|c| c.get("action"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                // Check if this was a one-time approval
                let is_one_time = permission_type == "allow_once";

                // Execute the tool
                let exec_result = self.execute_selected_tool(selection, args).await;

                // Audit log the permission grant and execution
                self.audit_log(AuditLogCreate {
                    user_id: Some(user_id.to_string()),
                    action: AuditAction::PermissionGranted.as_str().to_string(),
                    resource_type: "tool".to_string(),
                    resource_id: Some(tool_id.to_string()),
                    details: Some(serde_json::json!({
                        "service_id": service_id.to_string(),
                        "service_name": service_name.to_string(),
                        "permission_type": permission_type,
                    })),
                    ip_address: None,
                    user_agent: None,
                }).await;

                self.audit_log(AuditLogCreate {
                    user_id: Some(user_id.to_string()),
                    action: AuditAction::ToolExecuted.as_str().to_string(),
                    resource_type: "tool".to_string(),
                    resource_id: Some(tool_id.to_string()),
                    details: Some(serde_json::json!({
                        "service_id": service_id.to_string(),
                        "service_name": service_name.to_string(),
                        "success": exec_result.is_ok(),
                        "permission_type": permission_type,
                    })),
                    ip_address: None,
                    user_agent: None,
                }).await;

                // Consume one-time permission after execution
                if is_one_time {
                    let _ = approval_manager
                        .consume_permission(tool_id, service_id, user_id)
                        .await;
                }

                exec_result
            }
            PermissionStatus::Denied => {
                tracing::info!(
                    tool_id = %tool_id,
                    user_id = %user_id,
                    "Tool execution denied by user"
                );

                // Audit log the denial
                self.audit_log(AuditLogCreate {
                    user_id: Some(user_id.to_string()),
                    action: AuditAction::PermissionDenied.as_str().to_string(),
                    resource_type: "tool".to_string(),
                    resource_id: Some(tool_id.to_string()),
                    details: Some(serde_json::json!({
                        "service_id": service_id.to_string(),
                        "service_name": service_name.to_string(),
                        "reason": "user_denied",
                    })),
                    ip_address: None,
                    user_agent: None,
                }).await;

                Err(anyhow!("Tool execution denied by user"))
            }
            _ => {
                // Cancelled or other status
                Err(anyhow!("Tool approval cancelled"))
            }
        }
    }

    /// Look up the service name by ID.
    async fn get_service_name(&self, service_id: &RecordId) -> Option<String> {
        let query = "SELECT * FROM service WHERE id = $id LIMIT 1";
        let mut res = self.db
            .query(query)
            .bind(("id", service_id.clone()))
            .await
            .ok()?;

        let service: Option<ServiceRecord> = res.take(0).ok()?;
        service.and_then(|s| s.name.or(s.title))
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

    /// Get reference to the prompt forwarder.
    pub fn prompt_forwarder(&self) -> &StdArc<PromptForwarder> {
        &self.prompt_forwarder
    }

    /// Discover prompts from all running services.
    pub async fn discover_prompts(&self) -> Result<usize> {
        self.prompt_forwarder.discover_prompts().await
    }

    /// Get reference to the resource forwarder.
    pub fn resource_forwarder(&self) -> &StdArc<ResourceForwarder> {
        &self.resource_forwarder
    }

    /// Discover resources from all running services.
    pub async fn discover_resources(&self) -> Result<usize> {
        self.resource_forwarder.discover_resources().await
    }

    /// Get running services as a String-keyed map for use by the prompt forwarder.
    pub async fn running_services_as_string_map(
        &self,
    ) -> HashMap<String, StdArc<RunningService>> {
        let mut map = HashMap::new();
        for (id, service) in &self.running_services {
            map.insert(id.to_string(), service.clone());
        }
        map
    }

    /// Get reference to the elicitation coordinator.
    pub fn elicitation_coordinator(&self) -> &StdArc<ElicitationCoordinator> {
        &self.elicitation_coordinator
    }

    /// Write an audit log entry.
    pub async fn audit_log(&self, entry: AuditLogCreate) {
        let result = self.db
            .query(
                r#"
                CREATE audit_log CONTENT {
                    user_id: $user_id,
                    action: $action,
                    resource_type: $resource_type,
                    resource_id: $resource_id,
                    details: $details,
                    ip_address: $ip_address,
                    user_agent: $user_agent
                }
                "#,
            )
            .bind(("user_id", entry.user_id))
            .bind(("action", entry.action))
            .bind(("resource_type", entry.resource_type))
            .bind(("resource_id", entry.resource_id))
            .bind(("details", entry.details))
            .bind(("ip_address", entry.ip_address))
            .bind(("user_agent", entry.user_agent))
            .await;

        if let Err(e) = result {
            tracing::warn!("Failed to write audit log: {}", e);
        }
    }
}
