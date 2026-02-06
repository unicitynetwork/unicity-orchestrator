use crate::db::queries::QueryBuilder;
use crate::knowledge_graph::ToolSelection;
use crate::mcp_client::RunningService;
use anyhow::{Result, anyhow};
use rmcp::model::{Content, JsonObject};
use std::collections::HashMap;
use std::sync::Arc;
use surrealdb::RecordId;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

/// Execute a single selected tool by:
/// 1. Looking up the tool row by `tool_id`.
/// 2. Using the tool's `service_id` to find an already-running rmcp client.
/// 3. Calling the underlying MCP tool via `mcp_client::call_tool`.
///
/// This does **not** perform any planning or selection; it only executes the
/// given selection.
#[allow(clippy::mutable_key_type)]
pub async fn execute_selection(
    db: &Surreal<Any>,
    running_services: &HashMap<RecordId, Arc<RunningService>>,
    selection: &ToolSelection,
    args: JsonObject,
) -> Result<Vec<Content>> {
    // 1) Load the selected tool from the database using its RecordId.
    let tool = QueryBuilder::find_tool_by_id(db, selection.tool_id.clone())
        .await?
        .ok_or_else(|| anyhow!("Tool not found for id {}", selection.tool_id))?;

    // 2) Find the running service client for this tool's service_id.
    let svc = running_services.get(&tool.service_id).ok_or_else(|| {
        anyhow!(
            "No running service client for service_id {}",
            tool.service_id
        )
    })?;

    // 3) Call the underlying MCP tool via rmcp. The actual call is delegated
    // to `mcp_client::call_tool`, which should wrap the rmcp client API.
    let result = crate::mcp_client::call_tool(svc, &tool.name, args).await?;

    Ok(result)
}
