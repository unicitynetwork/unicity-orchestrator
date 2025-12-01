// Database query helpers for SurrealDB.
//
// These are intentionally conservative skeleton implementations that perform
// real SurrealDB queries, but keep the logic simple so we can evolve them
// alongside the schema and graph engine.

use crate::db::schema::*;
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::Value;
use surrealdb::{engine::any::Any, Surreal};
use surrealdb::RecordId;

pub struct QueryBuilder;

impl QueryBuilder {
    /// Create a new service record in the database.
    ///
    /// This is currently a simple CREATE and does not attempt to deduplicate
    /// based on registry or origin. In the future we can switch this to a
    /// true UPSERT keyed by a stable identifier (e.g. registry + name).
    pub async fn upsert_service(
        db: &Surreal<Any>,
        data: &ServiceCreate,
    ) -> Result<ServiceRecord> {
        let mut res = db
            .query(
                r#"
                CREATE service SET
                    name = $display_name,
                    title = $description,
                    version = $origin,
                    icons = $icons,
                    website_url = $website_url,
                    origin = $origin,
                    registry_id = $registry_id,
                    created_at = time::now(),
                    updated_at = time::now()
                "#,
            )
            .bind(("display_name", data.name.clone()))
            .bind(("title", data.title.clone()))
            .bind(("version", data.version.clone()))
            // .bind(("icons", data.icons.clone()))
            .bind(("website_url", data.website_url.clone()))
            .bind(("origin", data.origin.clone()))
            .bind(("registry_id", data.registry_id.clone()))
            .await?;

        let created: Option<ServiceRecord> = res.take(0)?;
        created.ok_or_else(|| anyhow!("failed to create service record"))
    }

    /// Create a new tool record in the database.
    ///
    /// This is currently a simple CREATE and does not attempt to deduplicate
    /// tools by (service_id, name). In the future this can be upgraded to
    /// a true UPSERT keyed by that pair.
    pub async fn upsert_tool(
        db: &Surreal<Any>,
        data: &CreateToolRecord,
    ) -> Result<ToolRecord> {
        let mut res = db
            .query(
                r#"
                CREATE tool SET
                    service_id = $service_id,
                    name = $name,
                    description = $description,
                    input_schema = $input_schema,
                    output_schema = $output_schema,
                    embedding_id = $embedding_id,
                    input_ty = $input_ty,
                    output_ty = $output_ty,
                    usage_count = 0,
                    created_at = time::now(),
                    updated_at = time::now()
                "#,
            )
            .bind(("service_id", data.service_id.clone()))
            .bind(("name", data.name.clone()))
            .bind(("description", data.description.clone()))
            .bind(("input_schema", Value::Object(data.input_schema.clone())))
            .bind(("output_schema", data.output_schema.clone()))
            .bind(("embedding_id", data.embedding_id.clone()))
            .bind(("input_ty", data.input_ty.clone()))
            .bind(("output_ty", data.output_ty.clone()))
            .await?;

        let created: Option<ToolRecord> = res.take(0)?;
        created.ok_or_else(|| anyhow!("failed to create tool record"))
    }

    pub async fn find_tool_by_id(
        db: &Surreal<Any>,
        tool_id: RecordId,
    ) -> Result<Option<ToolRecord>> {
        let mut res = db
            .query(
                r#"
                SELECT * FROM tool
                WHERE id = $id
                LIMIT 1
                "#,
            )
            .bind(("id", tool_id))
            .await?;

        let tool: Option<ToolRecord> = res.take(0)?;
        Ok(tool)
    }

    /// Find tools whose embeddings are similar to the given query vector.
    ///
    /// This implementation performs a vector search over the `embedding` table
    /// and then resolves each embedding back to its associated tool via
    /// `embedding_id` on the `tool` table. It is deliberately simple and can be
    /// optimized later.
    pub async fn find_tools_by_embedding(
        db: &Surreal<Any>,
        query_vector: Vec<f32>,
        limit: u32,
        threshold: f32,
    ) -> Result<Vec<(ToolRecord, f32)>> {
        // First, search the embedding table by vector similarity.
        // We assume `EmbeddingRecord` has a `vector` field and lives in the
        // `embedding` table.
        #[derive(Deserialize)]
        struct EmbeddingWithScore {
            #[serde(flatten)]
            embedding: EmbeddingRecord,
            score: f32,
        }

        let mut res = db
            .query(
                r#"
                SELECT *, vector::similarity::cosine(vector, $query_vec) AS score
                FROM embedding
                WHERE score >= $threshold
                ORDER BY score DESC
                LIMIT $limit
                "#,
            )
            .bind(("query_vec", query_vector))
            .bind(("threshold", threshold))
            .bind(("limit", limit as i64))
            .await?;

        let rows: Vec<EmbeddingWithScore> = res.take(0)?;
        let mut results = Vec::new();

        for row in rows {
            let mut tool_res = db
                .query(
                    r#"
                    SELECT * FROM tool
                    WHERE embedding_id = $embedding_id
                    LIMIT 1
                    "#,
                )
                .bind(("embedding_id", row.embedding.id.clone()))
                .await?;

            if let Some(tool) = tool_res.take::<Option<ToolRecord>>(0)? {
                results.push((tool, row.score));
            }
        }

        Ok(results)
    }

    /// Find tools that could form a simple one-hop chain from a start tool
    /// to some target output type.
    ///
    /// This is a very conservative implementation: it looks at the start
    /// tool's output type and returns tools whose input and output types
    /// match the desired flow. Full multi-hop planning is handled by the
    /// symbolic planner / graph engine.
    pub async fn find_tool_chain(
        db: &Surreal<Any>,
        start_tool: RecordId,
        target_output_type: String,
    ) -> Result<Vec<ToolRecord>> {
        // Load the start tool to inspect its output type.
        let mut res = db
            .query("SELECT * FROM tool WHERE id = $id")
            .bind(("id", start_tool.clone()))
            .await?;

        let start: Option<ToolRecord> = res.take(0)?;
        let start = match start {
            Some(t) => t,
            None => return Ok(Vec::new()),
        };

        let start_out_type = start
            .output_ty
            .as_ref()
            .map(|t| t.schema_type.clone())
            .unwrap_or_default();

        if start_out_type.is_empty() {
            // Without a known output type, we cannot construct a typed chain.
            return Ok(Vec::new());
        }

        // Find tools whose input type matches the start tool's output type
        // and whose output type matches the target type. This is a one-hop
        // approximation; the symbolic planner can do multi-hop later.
        let mut chain_res = db
            .query(
                r#"
                SELECT * FROM tool
                WHERE input_ty.type = $in_type
                  AND output_ty.type = $target_type
                "#,
            )
            .bind(("in_type", start_out_type))
            .bind(("target_type", target_output_type))
            .await?;

        let tools: Vec<ToolRecord> = chain_res.take(0)?;
        Ok(tools)
    }

    /// Get usage patterns for a given tool, based on the `tool_sequence` table.
    pub async fn get_tool_usage_patterns(
        db: &Surreal<Any>,
        tool_id: &RecordId,
    ) -> Result<Vec<ToolSequence>> {
        let mut res = db
            .query(
                r#"
                SELECT * FROM tool_sequence
                WHERE in = $id OR out = $id
                ORDER BY frequency DESC
                "#,
            )
            .bind(("id", tool_id.clone()))
            .await?;

        let patterns: Vec<ToolSequence> = res.take(0)?;
        Ok(patterns)
    }

    /// Perform a simple semantic search over tools.
    ///
    /// For now, this is a very conservative implementation: it just returns
    /// all tools, ordered as stored, and does not yet combine text + vector
    /// search. This can be evolved as `ToolSearchQuery` matures.
    pub async fn search_tools_semantically(
        db: &Surreal<Any>,
        _query: &ToolSearchQuery,
    ) -> Result<ToolSearchResult> {
        let mut res = db.query("SELECT * FROM tool LIMIT 50").await?;
        let tools: Vec<ToolRecord> = res.take(0)?;
        let total_count = tools.len() as u64;

        Ok(ToolSearchResult {
            tools,
            total_count,
            embeddings: None,
            search_time_ms: 0,
        })
    }

    /// Increment usage statistics for a tool.
    pub async fn update_tool_usage(
        db: &Surreal<Any>,
        tool_id: &RecordId,
        _success: bool,
    ) -> Result<()> {
        // For now we simply increment usage_count; we can later track success
        // vs failure separately in a dedicated telemetry table.
        db
            .query(
                r#"
                UPDATE tool
                SET usage_count += 1
                WHERE id = $id
                "#,
            )
            .bind(("id", tool_id.clone()))
            .await?;

        Ok(())
    }

    /// Create a new compatibility edge between two tools.
    pub async fn create_compatibility_edge(
        db: &Surreal<Any>,
        from_tool: &RecordId,
        to_tool: &RecordId,
        compatibility_type: CompatibilityType,
        confidence: f32,
        reasoning: Option<String>,
    ) -> Result<ToolCompatibility> {
        let mut res = db
            .query(
                r#"
                CREATE tool_compatibility SET
                    in = $in,
                    out = $out,
                    compatibility_type = $ctype,
                    confidence = $confidence,
                    reasoning = $reasoning,
                    created_at = time::now()
                "#,
            )
            .bind(("in", from_tool.clone()))
            .bind(("out", to_tool.clone()))
            .bind(("ctype", compatibility_type))
            .bind(("confidence", confidence))
            .bind(("reasoning", reasoning))
            .await?;

        let created: Option<ToolCompatibility> = res.take(0)?;
        created.ok_or_else(|| anyhow!("failed to create compatibility edge"))
    }

    /// Get all manifests for a given registry.
    pub async fn get_registry_manifests(
        db: &Surreal<Any>,
        registry_id: &RecordId,
    ) -> Result<Vec<ManifestRecord>> {
        let mut res = db
            .query(
                r#"
                SELECT * FROM manifest
                WHERE registry_id = $registry_id
                "#,
            )
            .bind(("registry_id", registry_id.clone()))
            .await?;

        let manifests: Vec<ManifestRecord> = res.take(0)?;
        Ok(manifests)
    }

    /// Sync a manifest into the database, creating a new record.
    ///
    /// This is deliberately simple for now: it always creates a new manifest
    /// row. In the future, this can be upgraded to use UPSERT semantics keyed
    /// by (registry_id, hash) or similar.
    pub async fn sync_manifest_to_db(
        db: &Surreal<Any>,
        registry_id: &RecordId,
        manifest_content: &Value,
        hash: &str,
    ) -> Result<ManifestRecord> {
        // Try to extract a name and version from the manifest content if present.
        let name = manifest_content
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let version = manifest_content
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0")
            .to_string();

        let mut res = db
            .query(
                r#"
                CREATE manifest SET
                    registry_id = $registry_id,
                    name = $name,
                    version = $version,
                    content = $content,
                    hash = $hash,
                    is_active = true,
                    created_at = time::now()
                "#,
            )
            .bind(("registry_id", registry_id.clone()))
            .bind(("name", name))
            .bind(("version", version))
            .bind(("content", manifest_content.clone()))
            .bind(("hash", hash.to_string()))
            .await?;

        let created: Option<ManifestRecord> = res.take(0)?;
        created.ok_or_else(|| anyhow!("failed to create manifest record"))
    }
}
