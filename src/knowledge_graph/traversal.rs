// Graph traversal using SurrealDB's native graph capabilities
// Based on https://surrealdb.com/docs/surrealdb/models/graph

use crate::db::GraphQueries;
use crate::db::schema::ToolSequence;
use crate::db::graph_queries::GraphStructure;
use crate::knowledge_graph::graph::EdgeType;
use anyhow::Result;
use surrealdb::{RecordId, Surreal};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct TraversalEngine {
    db: Surreal<surrealdb::engine::any::Any>,
}

impl TraversalEngine {
    pub fn new(db: Surreal<surrealdb::engine::any::Any>) -> Self {
        Self { db }
    }

    /// Find tools that can transform input_type to output_type using SurrealDB graph traversal
    pub async fn find_transformation_path(
        &self,
        input_type: &str,
        output_type: &str,
        max_depth: u8,
    ) -> Result<Vec<TransformationPath>> {
        // Use SurrealDB's built-in graph traversal
        let start_tool_id = self.find_sample_tool_by_input_type(input_type).await?;

        let paths = GraphQueries::find_transformation_path(
            &self.db,
            &start_tool_id,
            output_type,
            max_depth,
        ).await?;

        // Convert paths to TransformationPath structure
        let transformation_paths = paths
            .into_iter()
            .map(|tool_ids| {
                let cost = tool_ids.len() as f32 * 1.0; // Simple cost model
                TransformationPath {
                    tools: tool_ids,
                    total_confidence: 0.8, // Default confidence, could be based on usage stats
                    estimated_cost: cost,
                }
            })
            .collect();

        Ok(transformation_paths)
    }

    /// Helper: Find a sample tool with the given input type
    async fn find_sample_tool_by_input_type(&self, input_type: &str) -> Result<RecordId> {
        let tools = GraphQueries::find_tools_by_input_type(&self.db, input_type).await?;
        Ok(tools
            .into_iter()
            .next()
            .map(|tool| tool.id)
            .ok_or_else(|| anyhow::anyhow!("No tool found with input type: {}", input_type))?)
    }

    /// Find tools that are semantically similar to the query using graph similarity
    pub async fn find_semantically_similar_tools(
        &self,
        _tool_id: &str,
        _threshold: f32,
        _limit: usize,
    ) -> Result<Vec<SemanticMatch>> {
        // Use SurrealDB to find tools connected through semantic similarity edges
        // For now, return empty list as semantic matching needs embeddings
        Ok(vec![])
    }

    /// Find tool sequences based on usage patterns stored in the database
    pub async fn find_tool_sequences(
        &self,
        tool_id: &str,
        _max_length: usize,
    ) -> Result<Vec<ToolSequence>> {
        // Get tool sequences from database
        GraphQueries::get_tool_sequences(&self.db, tool_id, 10).await
    }

    /// Find alternative tools for the same task
    pub async fn find_alternative_tools(
        &self,
        tool_id: &str,
        _max_alternatives: usize,
    ) -> Result<Vec<AlternativeTool>> {
        // Find tools with similar input/output signatures
        let alternatives = GraphQueries::find_similar_tools(&self.db, tool_id, 0.7, 10).await?;

        let alt_tools = alternatives
            .into_iter()
            .map(|(alt_id, similarity)| AlternativeTool {
                tool_id: alt_id,
                similarity: similarity as f32,
                confidence: 0.8,
            })
            .collect();

        Ok(alt_tools)
    }

    /// Recommend tools based on similar users' behavior (collaborative filtering)
    pub async fn recommend_tools_collaborative(
        &self,
        _user_id: &str,
        _context: &HashMap<String, serde_json::Value>,
        _limit: usize,
    ) -> Result<Vec<CollaborativeRecommendation>> {
        // TODO: Implement collaborative filtering
        // This would require tracking user interactions and building a recommendation model
        Ok(vec![])
    }

    /// Build the knowledge graph from database data
    pub async fn build_from_database(&self) -> Result<GraphStructure> {
        GraphQueries::get_graph_structure(&self.db).await
    }

    /// Calculate similarity between two tool types
    pub fn calculate_type_similarity(
        &self,
        _from_type: &crate::db::schema::TypedSchema,
        _to_type: &crate::db::schema::TypedSchema,
    ) -> f32 {
        // Implement type compatibility checking
        // This would use the TypeSystem for strict type checking
        0.0
    }

    /// Check if two tools can be connected in the graph
    pub async fn can_connect(
        &self,
        from_tool: &str,
        to_tool: &str,
        required_edge_type: EdgeType,
    ) -> Result<bool> {
        // Query the database to check if there's a valid edge
        let mut result = self.db.query(r#"
            SELECT count() FROM tool_compatibility
            WHERE
                in = type::thing('tool', $from_tool)
                AND out = type::thing('tool', $to_tool)
                AND compatibility_type = $edge_type
        "#)
            .bind(("from_tool", from_tool.to_owned()))
            .bind(("to_tool", to_tool.to_owned()))
            .bind(("edge_type", format!("{:?}", required_edge_type)))
            .await?;
        let count: Option<i64> = result.take(0)?;

        Ok(count.unwrap_or(0) > 0)
    }
}

#[derive(Debug, Clone)]
pub struct PathState {
    pub current_tool: String,
    pub path: Vec<String>,
    pub current_type: String,
    pub depth: usize,
}

#[derive(Debug, Clone)]
pub struct TransformationPath {
    pub tools: Vec<String>,
    pub total_confidence: f32,
    pub estimated_cost: f32,
}

#[derive(Debug, Clone)]
pub struct SemanticMatch {
    pub tool_id: String,
    pub similarity: f32,
    pub embedding_model: String,
}


#[derive(Debug, Clone)]
pub struct AlternativeTool {
    pub tool_id: String,
    pub similarity: f32,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct CollaborativeRecommendation {
    pub tool_id: String,
    pub score: f32,
    pub similar_users: Vec<String>,
    pub reasoning: String,
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot_product / (norm_a * norm_b)
    }
}