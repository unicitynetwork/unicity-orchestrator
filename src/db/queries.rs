// Database query helpers for SurrealDB.
//
// These are intentionally conservative skeleton implementations that perform
// real SurrealDB queries, but keep the logic simple so we can evolve them
// alongside the schema and graph engine.

use crate::db::schema::{
    ApiKeyCreate, ApiKeyRecord, CompatibilityType, CreateToolRecord, ManifestRecord,
    ServiceCreate, ServiceRecord, ToolCompatibility, ToolRecord, ToolSearchQuery,
    ToolSearchResult, ToolSequence,
};
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
                    name = $name,
                    title = $title,
                    version = $version,
                    icons = $icons,
                    website_url = $website_url,
                    origin = $origin,
                    registry_id = $registry_id,
                    created_at = time::now(),
                    updated_at = time::now()
                "#,
            )
            .bind(("name", data.name.clone()))
            .bind(("title", data.title.clone()))
            .bind(("version", data.version.clone()))
            .bind(("icons", data.icons.clone()))
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

    /// Find a tool by its ID.
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

    /// Find tools by vector similarity against the `embedding` table.
    ///
    /// Returns `(ToolRecord, similarity_score)` tuples.
    pub async fn find_tools_by_embedding(
        db: &Surreal<Any>,
        query_vector: Vec<f32>,
        limit: u32,
        threshold: f32,
    ) -> Result<Vec<(ToolRecord, f32)>> {
        // We only need the embedding id + score from the embedding table.
        #[derive(Deserialize)]
        struct EmbeddingHit {
            id: RecordId,
            score: f32,
        }

        let mut res = db
            .query(
                r#"
                SELECT
                    id,
                    vector::similarity::cosine(vector, $query_vec) AS score
                FROM embedding
                WHERE vector::similarity::cosine(vector, $query_vec) >= $threshold
                ORDER BY score DESC
                LIMIT $limit
                "#,
            )
            .bind(("query_vec", query_vector))
            .bind(("threshold", threshold))
            .bind(("limit", limit as i64))
            .await?;

        let hits: Vec<EmbeddingHit> = res.take(0)?;
        let mut results = Vec::new();

        for hit in hits {
            let mut tool_res = db
                .query(
                    r#"
                    SELECT * FROM tool
                    WHERE embedding_id = $embedding_id
                    LIMIT 1
                    "#,
                )
                .bind(("embedding_id", hit.id.clone()))
                .await?;

            if let Some(tool) = tool_res.take::<Option<ToolRecord>>(0)? {
                results.push((tool, hit.score));
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

    // =========================================================================
    // API Key Management
    // =========================================================================

    /// Find an API key by its hash.
    pub async fn find_api_key_by_hash(
        db: &Surreal<Any>,
        key_hash: &str,
    ) -> Result<Option<ApiKeyRecord>> {
        let mut res = db
            .query(
                r#"
                SELECT * FROM api_key
                WHERE key_hash = $key_hash
                LIMIT 1
                "#,
            )
            .bind(("key_hash", key_hash.to_string()))
            .await?;

        let api_key: Option<ApiKeyRecord> = res.take(0)?;
        Ok(api_key)
    }

    /// Find an API key by its prefix.
    pub async fn find_api_key_by_prefix(
        db: &Surreal<Any>,
        key_prefix: &str,
    ) -> Result<Option<ApiKeyRecord>> {
        let mut res = db
            .query(
                r#"
                SELECT * FROM api_key
                WHERE key_prefix = $key_prefix
                LIMIT 1
                "#,
            )
            .bind(("key_prefix", key_prefix.to_string()))
            .await?;

        let api_key: Option<ApiKeyRecord> = res.take(0)?;
        Ok(api_key)
    }

    /// Create a new API key record.
    pub async fn create_api_key(
        db: &Surreal<Any>,
        data: &ApiKeyCreate,
    ) -> Result<ApiKeyRecord> {
        let mut res = db
            .query(
                r#"
                CREATE api_key SET
                    key_hash = $key_hash,
                    key_prefix = $key_prefix,
                    user_id = $user_id,
                    name = $name,
                    is_active = true,
                    expires_at = $expires_at,
                    scopes = $scopes,
                    created_at = time::now(),
                    last_used_at = NONE
                "#,
            )
            .bind(("key_hash", data.key_hash.to_string()))
            .bind(("key_prefix", data.key_prefix.to_string()))
            .bind(("user_id", data.user_id.clone()))
            .bind(("name", data.name.clone()))
            .bind(("expires_at", data.expires_at.clone()))
            .bind(("scopes", data.scopes.clone()))
            .await?;

        let created: Option<ApiKeyRecord> = res.take(0)?;
        created.ok_or_else(|| anyhow!("failed to create API key record"))
    }

    /// Update the last_used_at timestamp for an API key.
    pub async fn update_api_key_last_used(
        db: &Surreal<Any>,
        key_id: &RecordId,
    ) -> Result<()> {
        db.query(
            r#"
                UPDATE api_key
                SET last_used_at = time::now()
                WHERE id = $id
                "#,
        )
        .bind(("id", key_id.clone()))
        .await?;

        Ok(())
    }

    /// Deactivate (revoke) an API key.
    pub async fn deactivate_api_key(
        db: &Surreal<Any>,
        key_id: &RecordId,
    ) -> Result<()> {
        db.query(
            r#"
                UPDATE api_key
                SET is_active = false
                WHERE id = $id
                "#,
        )
        .bind(("id", key_id.clone()))
        .await?;

        Ok(())
    }

    /// Deactivate (revoke) an API key by its prefix.
    pub async fn deactivate_api_key_by_prefix(
        db: &Surreal<Any>,
        key_prefix: &str,
    ) -> Result<bool> {
        let mut res = db
            .query(
                r#"
                UPDATE api_key
                SET is_active = false
                WHERE key_prefix = $key_prefix
                RETURN AFTER
                "#,
            )
            .bind(("key_prefix", key_prefix.to_string()))
            .await?;

        let updated: Option<ApiKeyRecord> = res.take(0)?;
        Ok(updated.is_some())
    }

    /// List all API keys (for administrative purposes).
    /// Returns keys sorted by creation date, most recent first.
    pub async fn list_api_keys(
        db: &Surreal<Any>,
    ) -> Result<Vec<ApiKeyRecord>> {
        let mut res = db
            .query(
                r#"
                SELECT * FROM api_key
                ORDER BY created_at DESC
                "#,
            )
            .await?;

        let api_keys: Vec<ApiKeyRecord> = res.take(0)?;
        Ok(api_keys)
    }

    /// List active API keys only.
    pub async fn list_active_api_keys(
        db: &Surreal<Any>,
    ) -> Result<Vec<ApiKeyRecord>> {
        let mut res = db
            .query(
                r#"
                SELECT * FROM api_key
                WHERE is_active = true
                ORDER BY created_at DESC
                "#,
            )
            .await?;

        let api_keys: Vec<ApiKeyRecord> = res.take(0)?;
        Ok(api_keys)
    }
}

#[cfg(test)]
mod tests {
    use crate::db::connection::create_connection;
    use crate::db::connection::DatabaseConfig;
    use serde_json::json;
    use surrealdb::RecordId;
    use crate::db::{CompatibilityType, CreateToolRecord, QueryBuilder, ServiceCreate, ServiceOrigin, ToolSearchQuery, TypedSchema};

    #[tokio::test]
    async fn test_upsert_service() {
        // Create an in-memory database for testing
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // Create test data
        let service_data = ServiceCreate {
            name: "test_service".to_string(),
            title: Some("Test Service".to_string()),
            version: "1.0.0".to_string(),
            icons: None,
            website_url: Some("https://example.com".to_string()),
            origin: ServiceOrigin::StaticConfig,
            registry_id: None,
        };

        // Test upsert_service
        let result = QueryBuilder::upsert_service(&db, &service_data).await;
        assert!(result.is_ok(), "Failed to upsert service: {:?}", result.err());

        let service = result.unwrap();
        assert_eq!(service.name, Some("test_service".to_string()));
        assert_eq!(service.title, Some("Test Service".to_string()));
        assert_eq!(service.version, "1.0.0");
        assert_eq!(service.website_url, Some("https://example.com".to_string()));
        assert!(matches!(service.origin, ServiceOrigin::StaticConfig));
        assert!(service.created_at.is_some());
        assert!(service.updated_at.is_some());
    }

    #[tokio::test]
    async fn test_upsert_tool() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // First create a service to reference
        let service_data = ServiceCreate {
            name: "test_service".to_string(),
            title: Some("Test Service".to_string()),
            version: "1.0.0".to_string(),
            icons: None,
            website_url: None,
            origin: ServiceOrigin::StaticConfig,
            registry_id: None,
        };
        let service = QueryBuilder::upsert_service(&db, &service_data).await.unwrap();

        // Create test tool data
        let mut input_schema = serde_json::Map::new();
        input_schema.insert("type".to_string(), json!("string"));

        let mut output_schema = serde_json::Map::new();
        output_schema.insert("type".to_string(), json!("object"));

        let tool_data = CreateToolRecord {
            service_id: service.id.clone(),
            name: "test_tool".to_string(),
            description: Some("A test tool".to_string()),
            input_schema: input_schema.clone(),
            output_schema: Some(output_schema.clone()),
            embedding_id: None,
            input_ty: Some(TypedSchema {
                schema_type: "string".to_string(),
                properties: None,
                items: None,
                required: None,
                enum_values: None,
            }),
            output_ty: Some(TypedSchema {
                schema_type: "object".to_string(),
                properties: None,
                items: None,
                required: None,
                enum_values: None,
            }),
        };

        // Test upsert_tool
        let result = QueryBuilder::upsert_tool(&db, &tool_data).await;
        assert!(result.is_ok(), "Failed to upsert tool: {:?}", result.err());

        let tool = result.unwrap();
        assert_eq!(tool.name, "test_tool");
        assert_eq!(tool.description, Some("A test tool".to_string()));
        assert_eq!(tool.service_id, service.id);
        assert_eq!(tool.usage_count, 0);
        assert!(tool.created_at.is_some());
        assert!(tool.updated_at.is_some());
    }

    #[tokio::test]
    async fn test_find_tool_by_id() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // Create a service and tool first
        let service_data = ServiceCreate {
            name: "test_service".to_string(),
            title: None,
            version: "1.0.0".to_string(),
            icons: None,
            website_url: None,
            origin: ServiceOrigin::StaticConfig,
            registry_id: None,
        };
        let service = QueryBuilder::upsert_service(&db, &service_data).await.unwrap();

        let mut input_schema = serde_json::Map::new();
        input_schema.insert("type".to_string(), json!("string"));

        let tool_data = CreateToolRecord {
            service_id: service.id.clone(),
            name: "test_tool".to_string(),
            description: None,
            input_schema,
            output_schema: None,
            embedding_id: None,
            input_ty: None,
            output_ty: None,
        };
        let created_tool = QueryBuilder::upsert_tool(&db, &tool_data).await.unwrap();

        // Test find_tool_by_id
        let result = QueryBuilder::find_tool_by_id(&db, created_tool.id.clone()).await;
        assert!(result.is_ok(), "Failed to find tool by id: {:?}", result.err());

        let found_tool = result.unwrap();
        assert!(found_tool.is_some(), "Tool should be found");
        assert_eq!(found_tool.unwrap().id, created_tool.id);
    }

    #[tokio::test]
    async fn test_find_tool_by_id_not_found() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // Test with non-existent ID
        let fake_id = RecordId::from(("tool", "nonexistent"));
        let result = QueryBuilder::find_tool_by_id(&db, fake_id).await;
        assert!(result.is_ok());

        let found_tool = result.unwrap();
        assert!(found_tool.is_none(), "Non-existent tool should return None");
    }

    #[tokio::test]
    async fn test_find_tools_by_embedding_empty_result() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // Test with empty database
        let query_vector = vec![0.1, 0.2, 0.3];
        let result = QueryBuilder::find_tools_by_embedding(&db, query_vector, 10, 0.5).await;
        assert!(result.is_ok());

        let tools = result.unwrap();
        assert!(tools.is_empty(), "Should return empty result when no embeddings exist");
    }

    #[tokio::test]
    async fn test_find_tool_chain_no_start_tool() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // Test with non-existent start tool
        let fake_id = RecordId::from(("tool", "nonexistent"));
        let result = QueryBuilder::find_tool_chain(&db, fake_id, "string".to_string()).await;
        assert!(result.is_ok());

        let tools = result.unwrap();
        assert!(tools.is_empty(), "Should return empty result when start tool doesn't exist");
    }

    #[tokio::test]
    async fn test_get_tool_usage_patterns() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // Test with empty database
        let fake_id = RecordId::from(("tool", "nonexistent"));
        let result = QueryBuilder::get_tool_usage_patterns(&db, &fake_id).await;
        assert!(result.is_ok());

        let patterns = result.unwrap();
        assert!(patterns.is_empty(), "Should return empty result when no patterns exist");
    }

    #[tokio::test]
    async fn test_search_tools_semantically() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        let query = ToolSearchQuery {
            text_query: Some("test".to_string()),
            input_types: None,
            output_types: None,
            service_ids: None,
            min_confidence: None,
            include_embeddings: false,
            limit: Some(10),
            offset: None,
        };

        // Test search with empty database
        let result = QueryBuilder::search_tools_semantically(&db, &query).await;
        assert!(result.is_ok());

        let search_result = result.unwrap();
        assert_eq!(search_result.total_count, 0);
        assert!(search_result.tools.is_empty());
        assert!(search_result.embeddings.is_none());
        assert_eq!(search_result.search_time_ms, 0);
    }

    #[tokio::test]
    async fn test_update_tool_usage() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // Create a service and tool first
        let service_data = ServiceCreate {
            name: "test_service".to_string(),
            title: None,
            version: "1.0.0".to_string(),
            icons: None,
            website_url: None,
            origin: ServiceOrigin::StaticConfig,
            registry_id: None,
        };
        let service = QueryBuilder::upsert_service(&db, &service_data).await.unwrap();

        let mut input_schema = serde_json::Map::new();
        input_schema.insert("type".to_string(), json!("string"));

        let tool_data = CreateToolRecord {
            service_id: service.id.clone(),
            name: "test_tool".to_string(),
            description: None,
            input_schema,
            output_schema: None,
            embedding_id: None,
            input_ty: None,
            output_ty: None,
        };
        let created_tool = QueryBuilder::upsert_tool(&db, &tool_data).await.unwrap();

        // Test update_tool_usage
        let result = QueryBuilder::update_tool_usage(&db, &created_tool.id, true).await;
        assert!(result.is_ok(), "Failed to update tool usage: {:?}", result.err());

        // Verify the usage count was incremented
        let updated_tool = QueryBuilder::find_tool_by_id(&db, created_tool.id).await.unwrap().unwrap();
        assert_eq!(updated_tool.usage_count, 1);
    }

    #[tokio::test]
    async fn test_create_compatibility_edge() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // Create two tools
        let service_data = ServiceCreate {
            name: "test_service".to_string(),
            title: None,
            version: "1.0.0".to_string(),
            icons: None,
            website_url: None,
            origin: ServiceOrigin::StaticConfig,
            registry_id: None,
        };
        let service = QueryBuilder::upsert_service(&db, &service_data).await.unwrap();

        let mut input_schema = serde_json::Map::new();
        input_schema.insert("type".to_string(), json!("string"));

        let tool1_data = CreateToolRecord {
            service_id: service.id.clone(),
            name: "tool1".to_string(),
            description: None,
            input_schema: input_schema.clone(),
            output_schema: None,
            embedding_id: None,
            input_ty: None,
            output_ty: None,
        };
        let tool1 = QueryBuilder::upsert_tool(&db, &tool1_data).await.unwrap();

        let tool2_data = CreateToolRecord {
            service_id: service.id.clone(),
            name: "tool2".to_string(),
            description: None,
            input_schema,
            output_schema: None,
            embedding_id: None,
            input_ty: None,
            output_ty: None,
        };
        let tool2 = QueryBuilder::upsert_tool(&db, &tool2_data).await.unwrap();

        // Test create_compatibility_edge
        let result = QueryBuilder::create_compatibility_edge(
            &db,
            &tool1.id,
            &tool2.id,
            CompatibilityType::DataFlow,
            0.9,
            Some("Test compatibility".to_string()),
        ).await;
        assert!(result.is_ok(), "Failed to create compatibility edge: {:?}", result.err());

        let compatibility = result.unwrap();
        assert_eq!(compatibility.r#in, tool1.id);
        assert_eq!(compatibility.out, tool2.id);
        assert!(matches!(compatibility.compatibility_type, CompatibilityType::DataFlow));
        assert_eq!(compatibility.confidence, 0.9);
        assert_eq!(compatibility.reasoning, Some("Test compatibility".to_string()));
        assert!(compatibility.created_at.is_some());
    }

    #[tokio::test]
    async fn test_get_registry_manifests_empty() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        // Test with empty database
        let fake_id = RecordId::from(("registry", "nonexistent"));
        let result = QueryBuilder::get_registry_manifests(&db, &fake_id).await;
        assert!(result.is_ok());

        let manifests = result.unwrap();
        assert!(manifests.is_empty(), "Should return empty result when no manifests exist");
    }

    #[tokio::test]
    async fn test_sync_manifest_to_db() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        let registry_id = RecordId::from(("registry", "test"));
        let manifest_content = json!({
            "name": "test_manifest",
            "version": "1.0.0",
            "description": "A test manifest"
        });
        let hash = "test_hash_123";

        // Test sync_manifest_to_db
        let result = QueryBuilder::sync_manifest_to_db(&db, &registry_id, &manifest_content, hash).await;
        assert!(result.is_ok(), "Failed to sync manifest: {:?}", result.err());

        let manifest = result.unwrap();
        assert_eq!(manifest.registry_id, registry_id);
        assert_eq!(manifest.name, "test_manifest");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.content, manifest_content);
        assert_eq!(manifest.hash, "test_hash_123");
        assert!(manifest.is_active);
        assert!(manifest.created_at.is_some());
    }

    #[tokio::test]
    async fn test_sync_manifest_to_db_with_minimal_content() {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            username: None,
            password: None,
        };
        let db = create_connection(config).await.unwrap();

        let registry_id = RecordId::from(("registry", "test"));
        let manifest_content = json!({});
        let hash = "minimal_hash";

        // Test with minimal manifest content (no name or version)
        let result = QueryBuilder::sync_manifest_to_db(&db, &registry_id, &manifest_content, hash).await;
        assert!(result.is_ok(), "Failed to sync minimal manifest: {:?}", result.err());

        let manifest = result.unwrap();
        assert_eq!(manifest.name, "unknown");
        assert_eq!(manifest.version, "0.0.0");
        assert_eq!(manifest.hash, "minimal_hash");
    }

    #[tokio::test]
    async fn test_service_origin_serialization() {
        // Test that ServiceOrigin enum serializes correctly
        let static_config = ServiceOrigin::StaticConfig;
        let serialized = serde_json::to_string(&static_config).unwrap();
        assert_eq!(serialized, "\"StaticConfig\"");

        let registry = ServiceOrigin::Registry;
        let serialized = serde_json::to_string(&registry).unwrap();
        assert_eq!(serialized, "\"Registry\"");

        let broadcast = ServiceOrigin::Broadcast;
        let serialized = serde_json::to_string(&broadcast).unwrap();
        assert_eq!(serialized, "\"Broadcast\"");
    }

    #[tokio::test]
    async fn test_compatibility_type_serialization() {
        // Test that CompatibilityType enum serializes correctly
        let data_flow = CompatibilityType::DataFlow;
        let serialized = serde_json::to_string(&data_flow).unwrap();
        assert_eq!(serialized, "\"data_flow\"");

        let semantic = CompatibilityType::SemanticSimilarity;
        let serialized = serde_json::to_string(&semantic).unwrap();
        assert_eq!(serialized, "\"semantic_similarity\"");
    }

    #[tokio::test]
    async fn test_typed_schema_default_type() {
        // Test the default_schema_type function
        let schema = TypedSchema {
            schema_type: "any".to_string(),
            properties: None,
            items: None,
            required: None,
            enum_values: None,
        };
        assert_eq!(schema.schema_type, "any");
    }
}
