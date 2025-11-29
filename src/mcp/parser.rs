// MCP protocol parsing and type extraction

use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

pub struct McpParser;

impl McpParser {
    /// Extract type information from JSON schema
    pub fn extract_type_from_schema(schema: &Value) -> Result<crate::db::schema::TypedSchema> {
        let schema_type = schema.get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("any")
            .to_string();

        let mut properties = None;
        let mut items = None;
        let mut required = None;
        let mut enum_values = None;

        match schema_type.as_str() {
            "object" => {
                if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                    let mut typed_props = HashMap::new();
                    for (name, prop_schema) in props {
                        let prop_type = Self::extract_type_from_schema(prop_schema)?;
                        typed_props.insert(name.clone(), Box::new(prop_type));
                    }
                    properties = Some(typed_props);
                }

                if let Some(req) = schema.get("required").and_then(|r| r.as_array()) {
                    required = Some(req.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_string())
                        .collect());
                }
            }
            "array" => {
                if let Some(items_schema) = schema.get("items") {
                    items = Some(Box::new(Self::extract_type_from_schema(items_schema)?));
                }
            }
            _ => {}
        }

        if let Some(enum_vals) = schema.get("enum").and_then(|e| e.as_array()) {
            enum_values = Some(enum_vals.to_vec());
        }

        Ok(crate::db::schema::TypedSchema {
            schema_type,
            properties,
            items,
            required,
            enum_values,
        })
    }

    /// Check if two types are compatible
    pub fn are_types_compatible(
        from: &crate::db::schema::TypedSchema,
        to: &crate::db::schema::TypedSchema,
    ) -> f32 {
        // Exact match
        if from.schema_type == to.schema_type {
            return 1.0;
        }

        // Any type compatibility
        if from.schema_type == "any" || to.schema_type == "any" {
            return 0.7;
        }

        // Number/int compatibility
        if (from.schema_type == "number" && to.schema_type == "integer") ||
           (from.schema_type == "integer" && to.schema_type == "number") {
            return 0.9;
        }

        // String compatibility with various string-like types
        let string_compatible = ["string", "uri", "email", "date", "time", "datetime"];
        if string_compatible.contains(&from.schema_type.as_str()) &&
           string_compatible.contains(&to.schema_type.as_str()) {
            return 0.8;
        }

        // Array compatibility (check element types)
        if from.schema_type == "array" && to.schema_type == "array" {
            if let (Some(from_items), Some(to_items)) = (&from.items, &to.items) {
                return Self::are_types_compatible(from_items, to_items);
            }
        }

        // Object compatibility (check structural similarity)
        if from.schema_type == "object" && to.schema_type == "object" {
            return Self::object_compatibility(from, to);
        }

        // No compatibility
        0.0
    }

    /// Calculate structural compatibility between objects
    fn object_compatibility(
        from: &crate::db::schema::TypedSchema,
        to: &crate::db::schema::TypedSchema,
    ) -> f32 {
        if let (Some(from_props), Some(to_props)) = (&from.properties, &to.properties) {
            let mut total_similarity = 0.0;
            let mut common_fields = 0;

            for (field_name, from_field) in from_props {
                if let Some(to_field) = to_props.get(field_name) {
                    let similarity = Self::are_types_compatible(from_field, to_field);
                    total_similarity += similarity;
                    common_fields += 1;
                }
            }

            if common_fields > 0 {
                return total_similarity / common_fields as f32;
            }
        }

        0.0
    }

    /// Parse tool description for key concepts
    pub fn extract_concepts(description: &str) -> Vec<String> {
        let mut concepts = Vec::new();

        // Simple keyword extraction
        let keywords = [
            "file", "directory", "path", "read", "write", "create", "delete",
            "list", "search", "find", "filter", "parse", "validate", "transform",
            "convert", "encode", "decode", "compress", "extract", "merge",
            "split", "sort", "count", "compare", "diff", "patch", "apply",
            "database", "query", "insert", "update", "select", "join",
            "api", "http", "request", "response", "get", "post", "put", "delete",
            "json", "yaml", "xml", "csv", "markdown", "pdf", "image", "video",
            "git", "commit", "push", "pull", "branch", "merge", "clone",
            "docker", "container", "image", "build", "run", "deploy",
            "aws", "azure", "gcp", "cloud", "storage", "bucket", "queue",
            "slack", "email", "notification", "message", "send", "receive",
        ];

        let lower_desc = description.to_lowercase();
        for keyword in keywords {
            if lower_desc.contains(keyword) {
                concepts.push(keyword.to_string());
            }
        }

        concepts
    }

    /// Infer the primary purpose of a tool
    pub fn infer_tool_purpose(
        name: &str,
        description: &Option<String>,
        input_schema: &Value,
    ) -> ToolPurpose {
        let combined = format!("{} {}", name, description.as_deref().unwrap_or("")).to_lowercase();

        // Action-based inference
        if combined.contains("create") || combined.contains("make") || combined.contains("generate") {
            return ToolPurpose::Creation;
        }

        if combined.contains("read") || combined.contains("get") || combined.contains("fetch") ||
           combined.contains("list") || combined.contains("search") {
            return ToolPurpose::Retrieval;
        }

        if combined.contains("update") || combined.contains("modify") || combined.contains("edit") ||
           combined.contains("change") || combined.contains("transform") {
            return ToolPurpose::Modification;
        }

        if combined.contains("delete") || combined.contains("remove") || combined.contains("destroy") {
            return ToolPurpose::Deletion;
        }

        if combined.contains("validate") || combined.contains("check") || combined.contains("verify") {
            return ToolPurpose::Validation;
        }

        if combined.contains("parse") || combined.contains("convert") || combined.contains("format") {
            return ToolPurpose::Transformation;
        }

        // Default based on schema
        if input_schema.get("properties").is_some() {
            ToolPurpose::Processing
        } else {
            ToolPurpose::Utility
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolPurpose {
    Creation,
    Retrieval,
    Modification,
    Deletion,
    Validation,
    Transformation,
    Processing,
    Utility,
}