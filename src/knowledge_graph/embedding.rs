use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use surrealdb::engine::any::Any;
use surrealdb::{RecordId, Surreal};
use crate::db::queries::QueryBuilder;

use embed_anything::{
    config::TextEmbedConfig,
    embeddings::embed::{Embedder, EmbedderBuilder},
    embed_query,
};
use rmcp::model::JsonObject;

pub struct EmbeddingManager {
    db: Surreal<Any>,
    cache: HashMap<String, Vec<f32>>,
    embedder: Embedder,
    text_config: TextEmbedConfig,
    /// The model id used for embeddings (e.g. Hugging Face model id).
    model_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// The Hugging Face model ID used for embeddings.
    pub model_name: String,
    /// The model architecture for embed_anything (e.g. "jina", "qwen").
    /// If left empty, the architecture will be inferred from the model name
    /// ("qwen" in the name -> "qwen", otherwise "jina").
    pub model_architecture: String,
    pub dimension: usize,
    pub batch_size: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            // A reasonable default sentence embedding model. To use Qwen
            // embeddings instead, set `model_architecture = "qwen"` and
            // provide the appropriate Qwen embedding model id via
            // configuration.
            model_name: "Qwen/QWen3-Embedding-0.6B".to_string(),
            model_architecture: "Qwen3".to_string(),
            dimension: 1024,
            batch_size: 32,
        }
    }
}

impl EmbeddingManager {
    pub async fn new(db: Surreal<Any>, config: EmbeddingConfig) -> Result<Self> {
        // Determine the architecture. If explicitly provided, use it; otherwise
        // infer from the model name ("qwen" -> "qwen", else default to "jina").
        let arch = if !config.model_architecture.is_empty() {
            config.model_architecture.clone()
        } else if config.model_name.to_lowercase().contains("qwen3") {
            "qwen3".to_string()
        } else {
            "jina".to_string()
        };

        let embedder = EmbedderBuilder::new()
            .model_architecture(&arch)
            .model_id(Some(&config.model_name))
            .from_pretrained_hf()?;

        let text_config = TextEmbedConfig::default();

        Ok(Self {
            db,
            cache: HashMap::new(),
            embedder,
            text_config,
            model_name: config.model_name,
        })
    }

    pub async fn embed_text(&mut self, text: &str) -> Result<Vec<f32>> {
        // Check cache first
        let hash = self.hash_content(text);
        if let Some(cached) = self.cache.get(&hash) {
            return Ok(cached.clone());
        }

        // Use embed_anything to generate a real embedding
        let queries = vec![text];
        let results = embed_query(&queries, &self.embedder, Some(&self.text_config)).await?;

        // Take the first embedding, or return an error if nothing was produced
        let embedding = results
            .get(0)
            .map(|d| d.embedding.clone())
            .ok_or_else(|| anyhow::anyhow!("embed_anything returned no embeddings for query"))?;

        let dense = embedding.to_dense()?;

        // Cache the result
        self.cache.insert(hash, dense.clone());

        Ok(dense)
    }

    pub async fn embed_tool(&mut self, tool: &crate::db::schema::ToolRecord) -> Result<Vec<f32>> {
        // Combine tool name, description, and schema for embedding
        let mut text_parts = Vec::new();
        text_parts.push(format!("Tool: {}", tool.name));

        if let Some(description) = &tool.description {
            text_parts.push(format!("Description: {}", description));
        }

        // Include input schema information
        if let Ok(schema_text) = self.schema_to_text(&tool.input_schema) {
            text_parts.push(format!("Input: {}", schema_text));
        }

        // Include type information
        if let Some(input_ty) = &tool.input_ty {
            text_parts.push(format!("Input Type: {}", self.typed_schema_to_text(input_ty)));
        }

        if let Some(output_ty) = &tool.output_ty {
            text_parts.push(format!("Output Type: {}", self.typed_schema_to_text(output_ty)));
        }

        let combined_text = text_parts.join("\n");
        self.embed_text(&combined_text).await
    }

    pub async fn embed_batch(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // Check cache for each text
        let mut uncached_texts = Vec::new();
        let mut uncached_indices = Vec::new();
        let mut results = vec![vec![]; texts.len()];

        for (i, text) in texts.iter().enumerate() {
            let hash = self.hash_content(text);
            if let Some(cached) = self.cache.get(&hash) {
                results[i] = cached.clone();
            } else {
                uncached_texts.push(text.as_str());
                uncached_indices.push(i);
            }
        }

        // Embed uncached texts using embed_anything
        if !uncached_texts.is_empty() {
            let batch_results =
                embed_query(&uncached_texts, &self.embedder, Some(&self.text_config)).await?;

            for (j, embed_data) in batch_results.into_iter().enumerate() {
                let original_index = uncached_indices[j];
                let embedding = embed_data.embedding;
                let dense = embedding.to_dense()?;
                results[original_index] = dense.clone();

                // Cache the result
                let hash = self.hash_content(&texts[original_index]);
                self.cache.insert(hash, dense);
            }
        }

        Ok(results)
    }

    pub async fn store_embedding(
        &self,
        vector: Vec<f32>,
        model: String,
        content_type: String,
        content_hash: String,
    ) -> Result<RecordId> {
        // First, check if we already have an embedding for this (model, content_hash).
        #[derive(Deserialize)]
        struct ExistingRow {
            id: RecordId,
        }

        let mut res = self
            .db
            .query(
                r#"
                SELECT id FROM embedding
                WHERE content_hash = $hash AND model = $model
                LIMIT 1
                "#,
            )
            .bind(("hash", content_hash.clone()))
            .bind(("model", model.clone()))
            .await?;

        let res: Option<ExistingRow> = res.take(0)?;
        if let Some(existing) = res {
            return Ok(existing.id);
        }

        // Otherwise create a new embedding record.
        let mut res = self
            .db
            .query(
                r#"
                CREATE embedding SET
                    vector = $vector,
                    model = $model,
                    content_type = $ctype,
                    content_hash = $hash,
                    created_at = time::now()
                "#,
            )
            .bind(("vector", vector))
            .bind(("model", model))
            .bind(("ctype", content_type))
            .bind(("hash", content_hash))
            .await?;

        let created: Option<crate::db::schema::EmbeddingRecord> = res.take(0)?;
        created
            .map(|e| e.id)
            .ok_or_else(|| anyhow::anyhow!("failed to create embedding record"))
    }

    pub async fn get_embedding(
        &self,
        content_hash: String,
        model: String,
    ) -> Result<Option<Vec<f32>>> {
        #[derive(Deserialize)]
        struct Row {
            vector: Vec<f32>,
        }

        let mut res = self
            .db
            .query(
                r#"
                SELECT vector FROM embedding
                WHERE content_hash = $hash AND model = $model
                LIMIT 1
                "#,
            )
            .bind(("hash", content_hash))
            .bind(("model", model))
            .await?;

        let row: Option<Row> = res.take(0)?;
        Ok(row.map(|r| r.vector))
    }

    pub async fn update_tool_embeddings(&mut self) -> Result<usize> {
        // Get all tools without embeddings
        let query = r#"
        SELECT * FROM tool
        WHERE embedding_id = NONE
        "#;

        let mut result = self.db.query(query).await?;
        let tools: Vec<crate::db::schema::ToolRecord> = result.take(0)?;

        let mut updated = 0;

        for tool in tools {
            // Generate embedding
            let embedding = self.embed_tool(&tool).await?;

            // Build a stable content hash for this tool's semantic description.
            let schema_str = serde_json::to_string(&tool.input_schema)?;

            let content_hash = self.hash_content(&format!(
                "{}:{}:{}",
                tool.name,
                tool.description.as_deref().unwrap_or(""),
                schema_str,
            ));

            let embedding_id = self
                .store_embedding(
                    embedding,
                    self.model_name.clone(),
                    "tool".to_string(),
                    content_hash,
                )
                .await?;

            // Update tool record
            let update_query = r#"
            UPDATE $tool_id SET
                embedding_id = $embedding_id,
                updated_at = time::now()
            "#;

            self.db
                .query(update_query)
                .bind(("tool_id", tool.id))
                .bind(("embedding_id", embedding_id))
                .await?;

            updated += 1;
        }

        Ok(updated)
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    fn hash_content(&self, content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn schema_to_text(&self, schema: &JsonObject) -> Result<String> {
        Ok(serde_json::to_string(&schema)?)
    }

    fn typed_schema_to_text(&self, schema: &crate::db::schema::TypedSchema) -> String {
        let mut parts = Vec::new();
        parts.push(schema.schema_type.clone());

        if let Some(properties) = &schema.properties {
            for (name, prop) in properties {
                parts.push(format!("{}: {}", name, prop.schema_type));
            }
        }

        parts.join(", ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingSearchResult {
    pub tool_id: RecordId,
    pub similarity: f32,
    pub tool: Option<crate::db::schema::ToolRecord>,
}

impl EmbeddingManager {
    /// Search for tools by embedding similarity.
    pub async fn search_tools_by_embedding(
        &mut self,
        query: &str,
        limit: u32,
        threshold: f32,
    ) -> Result<Vec<EmbeddingSearchResult>> {
        // Generate query embedding using embed_anything.
        let query_vector = self.embed_text(query).await?;

        // Delegate to the DB query helper to perform the vector search and
        // map embeddings back to tools.
        let matches = QueryBuilder::find_tools_by_embedding(
            &self.db,
            query_vector,
            limit,
            threshold,
        )
            .await?;

        let mut results = Vec::new();

        for (tool, similarity) in matches {
            results.push(EmbeddingSearchResult {
                tool_id: tool.id.clone(),
                similarity,
                tool: Some(tool),
            });
        }

        Ok(results)
    }
}
