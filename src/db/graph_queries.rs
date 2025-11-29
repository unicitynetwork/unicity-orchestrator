// SurrealDB Graph Query Implementations
// Based on https://surrealdb.com/docs/surrealdb/models/graph

use crate::db::schema::*;
use anyhow::Result;
use surrealdb::sql::Thing;
use surrealdb::{RecordId, Surreal};
use std::collections::HashMap;

/// Graph traversal utilities for SurrealDB
pub struct GraphQueries;

impl GraphQueries {
    /// Find tools that can transform input_type to output_type through graph traversal
    pub async fn find_transformation_path(
        db: &Surreal<surrealdb::engine::any::Any>,
        start_tool_id: &RecordId,
        target_output_type: &str,
        max_depth: u8,
    ) -> Result<Vec<Vec<String>>> {
        // Use SurrealDB's graph traversal syntax
        let mut result = db.query(r#"
            -- Find paths through the compatibility graph
            SELECT array::flatten((
                SELECT $start_tool_id->tool_compatibility[WHERE compatibility_type = 'DataFlow']{{0..$max_depth}}->out.id
                AS path
            )) AS paths
            FROM (
                SELECT id
                FROM tool
                WHERE id = type::thing('tool', $start_tool_id)
            ) LIMIT 1
        "#)
            .bind(("start_tool_id", start_tool_id.to_owned()))
            .bind(("max_depth", max_depth))
            .await?;

        let paths: Vec<Vec<Thing>> = result.take("paths")?;

        // Filter paths that end with tools producing the target output type
        let mut valid_paths = Vec::new();
        for path in paths {
            // Check if the last tool in the path produces the target output
            if let Some(last_tool) = path.last() {
                // Check the tool's output type (simplified)
                // In a real implementation, this would query the tool's schema
                let query = format!(
                    "SELECT output_ty FROM ONLY {} WHERE id = {}",
                    last_tool, last_tool
                );
                let mut check_result = db.query(&query).await?;
                let output_ty: Option<String> = check_result.take("output_ty")?;

                if let Some(output) = output_ty && output == target_output_type {
                    valid_paths.push(
                        path.iter().map(|t| t.id.to_string()).collect()
                    );
                }
            }
        }

        Ok(valid_paths)
    }

    /// Find tools similar to a given tool using semantic similarity edges
    pub async fn find_similar_tools(
        db: &Surreal<surrealdb::engine::any::Any>,
        tool_id: &str,
        min_similarity: f64,
        limit: u32,
    ) -> Result<Vec<(String, f64)>> {
        let mut result = db.query(r#"
            -- Find tools connected through semantic similarity
            SELECT
                id as similar_tool_id,
                similarity as score
            FROM tool
            WHERE ->tool_compatibility[WHERE compatibility_type = 'SemanticSimilarity']->{
                ->(tool WHERE id = type::thing('tool', $tool_id))
            } AS similar_tools
            WHERE similarity.score >= $min_similarity
            ORDER BY similarity.score DESC
            LIMIT $limit
        "#)
            .bind(("tool_id", tool_id.to_owned()))
            .bind(("min_similarity", min_similarity))
            .bind(("limit", limit))
            .await?;

        let tool_ids: Vec<String> = result.take("similar_tool_id")?;
        let scores: Vec<f64> = result.take("score")?;
        let tools: Vec<(String, f64)> = tool_ids.into_iter().zip(scores.into_iter()).collect();

        Ok(tools)
    }

    /// Get tool usage patterns and sequences
    pub async fn get_tool_sequences(
        db: &Surreal<surrealdb::engine::any::Any>,
        tool_id: &str,
        limit: u32,
    ) -> Result<Vec<ToolSequence>> {
        let mut result = db.query(r#"
            -- Get tool sequences where this tool is involved
            SELECT *,
                ->tool_sequence[WHERE in = type::thing('tool', $tool_id)] AS outgoing,
                <-tool_sequence[WHERE out = type::thing('tool', $tool_id)] AS incoming
            FROM tool_sequence
            WHERE in = type::thing('tool', $tool_id)
               OR out = type::thing('tool', $tool_id)
            ORDER BY frequency DESC
            LIMIT $limit
        "#)
            .bind(("tool_id", tool_id.to_owned()))
            .bind(("limit", limit))
            .await?;
        let sequences: Vec<ToolSequence> = result.take(0)?;

        Ok(sequences)
    }

    /// Create a compatibility edge between two tools
    pub async fn create_compatibility_edge(
        db: &Surreal<surrealdb::engine::any::Any>,
        from_tool: RecordId,
        to_tool: RecordId,
        compatibility_type: &str,
        confidence: f32,
        reasoning: Option<String>,
    ) -> Result<String> {
        let mut result = db.query(r#"
            RELATE type::thing('tool', $from_tool)
                ->tool_compatibility[{
                    compatibility_type: $compatibility_type,
                    confidence: $confidence,
                    reasoning: $reasoning,
                    created_at: time::now()
                }]
                ->type::thing('tool', $to_tool)
            RETURN id
        "#)
            .bind(("from_tool", from_tool.to_owned()))
            .bind(("to_tool", to_tool.to_owned()))
            .bind(("compatibility_type", compatibility_type.to_owned()))
            .bind(("confidence", confidence))
            .bind(("reasoning", reasoning))
            .await?;
        let edge_id: Option<String> = result.take("id")?;

        edge_id.ok_or_else(|| anyhow::anyhow!("Failed to create compatibility edge"))
    }

    /// Find all tools compatible with a given input type
    pub async fn find_tools_by_input_type(
        db: &Surreal<surrealdb::engine::any::Any>,
        input_type: &str,
    ) -> Result<Vec<ToolRecord>> {
        let mut result = db.query(r#"
            SELECT * FROM tool
            WHERE input_ty.type = $input_type
            ORDER BY usage_count DESC
        "#)
            .bind(("input_type", input_type.to_owned()))
            .await?;
        let tools: Vec<ToolRecord> = result.take(0)?;

        Ok(tools)
    }

    /// Find all tools that produce a given output type
    pub async fn find_tools_by_output_type(
        db: &Surreal<surrealdb::engine::any::Any>,
        output_type: &str,
    ) -> Result<Vec<ToolRecord>> {
        let mut result = db.query(r#"
            SELECT * FROM tool
            WHERE output_ty.type = $output_type
            ORDER BY usage_count DESC
        "#)
            .bind(("output_type", output_type.to_owned()))
            .await?;
        let tools: Vec<ToolRecord> = result.take(0)?;

        Ok(tools)
    }

    /// Get the complete knowledge graph structure
    pub async fn get_graph_structure(
        db: &Surreal<surrealdb::engine::any::Any>,
    ) -> Result<GraphStructure> {
        // Get all tools
        let tools_query = "SELECT * FROM tool";
        let mut result = db.query(tools_query).await?;
        let tools: Vec<ToolRecord> = result.take(0)?;

        // Get all services
        let services_query = "SELECT * FROM service";
        let mut result = db.query(services_query).await?;
        let services: Vec<ServiceRecord> = result.take(0)?;

        // Get compatibility edges
        let edges_query = "SELECT *, in, out FROM tool_compatibility";
        let mut result = db.query(edges_query).await?;
        // Get all compatibility edges
        // Note: This is a simplified query - in practice you'd need to join or handle the relation structure
        let edges: Vec<ToolCompatibility> = result.take(0)?;

        Ok(GraphStructure {
            tools,
            services,
            compatibility_edges: edges,
        })
    }

    /// Update tool usage statistics
    pub async fn increment_tool_usage(
        db: &Surreal<surrealdb::engine::any::Any>,
        tool_id: &str,
        success: bool,
    ) -> Result<()> {
        let result = db.query(r#"
            UPDATE type::thing('tool', $tool_id) SET
                usage_count += 1,
                updated_at = time::now()
        "#)
            .bind(("tool_id", tool_id.to_owned()))
            .await?;

        // If successful, also update success rate in sequences
        if success {
            // This could trigger background jobs to update tool sequences
            tracing::info!("Tool {} used successfully", tool_id);
        }

        Ok(())
    }

    /// Analyze tool usage patterns to suggest sequences
    pub async fn analyze_usage_patterns(
        db: &Surreal<surrealdb::engine::any::Any>,
        time_window_hours: u64,
    ) -> Result<Vec<PatternAnalysis>> {
        let mut result = db.query(r#"
            -- Analyze recent tool sequences
            WITH time_window = time::now() - ${time_window_hours}h

            SELECT
                in_id::string || ' -> ' || out_id::string as sequence,
                COUNT() as frequency,
                AVG(CASE WHEN success = true THEN 1.0 ELSE 0.0 END) as avg_success
            FROM tool_sequence
            WHERE created_at > time_window
            GROUP BY sequence
            ORDER BY frequency DESC
            LIMIT 20
        "#)
            .bind(("time_window_hours", time_window_hours))
            .await?;
        let sequences: Vec<String> = result.take("sequence")?;
        let frequencies: Vec<u64> = result.take("frequency")?;
        let avg_successes: Vec<f64> = result.take("avg_success")?;
        let patterns: Vec<(String, u64, f64)> = sequences.into_iter()
            .zip(frequencies.into_iter())
            .zip(avg_successes.into_iter())
            .map(|((seq, freq), success)| (seq, freq, success))
            .collect();

        let analyses = patterns
            .into_iter()
            .enumerate()
            .map(|(i, (sequence, freq, success))| PatternAnalysis {
                pattern_id: i.to_string(),
                sequence,
                frequency: freq,
                avg_success_rate: success,
                confidence: (freq as f64 / 100.0).min(1.0), // Simple confidence calculation
            })
            .collect();

        Ok(analyses)
    }
}

/// Structure representing the complete graph
#[derive(Debug, Clone)]
pub struct GraphStructure {
    pub tools: Vec<ToolRecord>,
    pub services: Vec<ServiceRecord>,
    pub compatibility_edges: Vec<ToolCompatibility>,
}

/// Pattern analysis result
#[derive(Debug, Clone)]
pub struct PatternAnalysis {
    pub pattern_id: String,
    pub sequence: String,
    pub frequency: u64,
    pub avg_success_rate: f64,
    pub confidence: f64,
}
