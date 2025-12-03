use serde::{Deserialize, Serialize};
use surrealdb::{RecordId, sql::Datetime};
use std::collections::HashMap;
use rmcp::model::{Icon, JsonObject};
use rmcp::schemars;
use serde_json::{from_value, Value};

/// Persisted representation of an MCP service in SurrealDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRecord {
    /// Stable database identifier for this service (table: `service`).
    pub id: RecordId,
    /// Human-friendly name shown to users, if available from the MCP manifest.
    pub name: Option<String>,
    /// Longer description of the service and its capabilities.
    pub title: Option<String>,
    /// Version string from the MCP manifest.
    pub version: String,
    /// Optional icons associated with this service.
    pub icons: Option<Vec<Icon>>,
    /// Optional website URL for this service.
    pub website_url: Option<String>,
    /// How this service was discovered (static config, registry, broadcast).
    pub origin: ServiceOrigin,
    /// Optional reference to the registry this service came from.
    pub registry_id: Option<RecordId>,
    /// When this record was first created.
    pub created_at: Option<Datetime>,
    /// When this record was last updated.
    pub updated_at: Option<Datetime>,
}

/// Payload used when inserting a new service into the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCreate {
    /// Human-friendly name shown to users, if available.
    pub name: String,
    /// Longer description of the service and its capabilities.
    pub title: Option<String>,
    /// Version string from the MCP manifest.
    pub version: String,
    // /// Optional icons associated with this service.
    // pub icons: Option<Vec<Icon>>, // TODO
    /// Optional website URL for this service.
    pub website_url: Option<String>,
    /// How this service was discovered.
    pub origin: ServiceOrigin,
    /// Optional registry that this service belongs to.
    pub registry_id: Option<RecordId>,
}

/// High-level origin of a service in the orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceOrigin {
    #[serde(rename = "StaticConfig")]
    StaticConfig,
    #[serde(rename = "Registry")]
    Registry,
    #[serde(rename = "Broadcast")]
    Broadcast,
}

/// Persisted representation of a single MCP tool plus analysis metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRecord {
    /// Stable database identifier for this tool (table: `tool`).
    pub id: RecordId,
    /// Foreign key pointing to the owning service.
    pub service_id: RecordId,
    /// Tool name as defined in the MCP manifest.
    pub name: String,
    /// Optional human-readable description from the manifest.
    pub description: Option<String>,
    /// Raw JSON schema describing the tool input.
    pub input_schema: JsonObject,
    /// Raw JSON schema describing the tool output.
    pub output_schema: Option<JsonObject>,
    /// Optional reference to the stored embedding for this tool.
    pub embedding_id: Option<RecordId>,
    /// Normalized, typed representation of the input schema.
    pub input_ty: Option<TypedSchema>,
    /// Normalized, typed representation of the output schema.
    pub output_ty: Option<TypedSchema>,
    /// Number of times this tool has been executed.
    pub usage_count: u64,
    /// When this record was first created.
    pub created_at: Option<Datetime>,
    /// When this record was last updated.
    pub updated_at: Option<Datetime>,
}

/// Payload used when inserting a new tool into the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateToolRecord {
    /// Owning service for this tool.
    pub service_id: RecordId,
    /// Tool name as defined in the MCP manifest.
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Raw JSON schema describing the tool input.
    pub input_schema: JsonObject,
    /// Raw JSON schema describing the tool output.
    pub output_schema: Option<JsonObject>,
    /// Optional reference to the stored embedding for this tool.
    pub embedding_id: Option<RecordId>,
    /// Normalized input schema.
    pub input_ty: Option<TypedSchema>,
    /// Normalized output schema.
    pub output_ty: Option<TypedSchema>,
}

/// Simplified, normalized representation of a JSON schema used for type reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypedSchema {
    /// JSON Schema `type` field (e.g. "string", "object", "array").
    /// Defaults to `"any"` when not present in the source JSON.
    #[serde(rename = "type", default = "default_schema_type")]
    pub schema_type: String,
    /// Properties for object types, if any.
    pub properties: Option<HashMap<String, Box<TypedSchema>>>,
    /// Item type for array types, if any.
    pub items: Option<Box<TypedSchema>>,
    /// List of required property names for object types.
    pub required: Option<Vec<String>>,
    /// Optional enum value set for constrained types.
    pub enum_values: Option<Vec<Value>>,
}

fn default_schema_type() -> String {
    "any".to_string()
}

impl TypedSchema {

    /// Construct a `TypedSchema` from a JSON Schema-like value.
    ///
    /// This version expects a JsonObject (serde_json::Map<String, Value>)
    /// at the top level, and recurses into nested `Value`s.
    pub fn from_json_schema(schema: &JsonObject) -> Self {
        // 1) explicit `type`
        if let Some(type_value) = schema.get("type") {
            match type_value {
                Value::String(s) => match s.as_str() {
                    "object" => {
                        // Object with properties and required fields.
                        let mut props = HashMap::new();
                        if let Some(Value::Object(map)) = schema.get("properties") {
                            for (name, prop_schema) in map {
                                let child = Self::from_value(prop_schema);
                                props.insert(name.clone(), Box::new(child));
                            }
                        }

                        let required = schema
                            .get("required")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect::<Vec<_>>()
                            });

                        TypedSchema {
                            schema_type: "object".to_string(),
                            properties: if props.is_empty() { None } else { Some(props) },
                            items: None,
                            required,
                            enum_values: None,
                        }
                    }
                    "array" => {
                        let item_schema = schema
                            .get("items")
                            .map(Self::from_value)
                            .map(Box::new);

                        TypedSchema {
                            schema_type: "array".to_string(),
                            properties: None,
                            items: item_schema,
                            required: None,
                            enum_values: None,
                        }
                    }
                    other => Self::simple(other),
                },
                Value::Array(arr) => {
                    // type: ["string", "null"] etc. -> mark overall type as "union"
                    // and store raw type strings in enum_values.
                    let mut types = Vec::new();
                    for v in arr {
                        if let Value::String(s) = v {
                            types.push(Value::String(s.clone()));
                        }
                    }

                    TypedSchema {
                        schema_type: "union".to_string(),
                        properties: None,
                        items: None,
                        required: None,
                        enum_values: if types.is_empty() { None } else { Some(types) },
                    }
                }
                _ => Self::simple("any"),
            }
        // 2) anyOf / oneOf
        } else if let Some(variants) = schema.get("anyOf").or_else(|| schema.get("oneOf")) {
            if let Value::Array(arr) = variants {
                let mut variants_json = Vec::new();
                for v in arr {
                    variants_json.push(v.clone());
                }

                TypedSchema {
                    schema_type: "union".to_string(),
                    properties: None,
                    items: None,
                    required: None,
                    enum_values: if variants_json.is_empty() {
                        None
                    } else {
                        Some(variants_json)
                    },
                }
            } else {
                Self::simple("any")
            }
        // 3) infer `object` from properties even without `type`
        } else if schema.get("properties").is_some() {
            let mut props = HashMap::new();
            if let Some(Value::Object(map)) = schema.get("properties") {
                for (name, prop_schema) in map {
                    let child = Self::from_value(prop_schema);
                    props.insert(name.clone(), Box::new(child));
                }
            }

            let required = schema
                .get("required")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                });

            TypedSchema {
                schema_type: "object".to_string(),
                properties: if props.is_empty() { None } else { Some(props) },
                items: None,
                required,
                enum_values: None,
            }
        // 4) infer `array` from items
        } else if schema.get("items").is_some() {
            let item_schema = schema
                .get("items")
                .map(Self::from_value)
                .map(Box::new);

            TypedSchema {
                schema_type: "array".to_string(),
                properties: None,
                items: item_schema,
                required: None,
                enum_values: None,
            }
        // 5) truly unknown
        } else {
            Self::simple("any")
        }
    }

    // small helper for primitive types
    fn simple(schema_type: &str) -> TypedSchema {
        TypedSchema {
            schema_type: schema_type.to_string(),
            properties: None,
            items: None,
            required: None,
            enum_values: None,
        }
    }

    // helper that can handle any Value
    fn from_value(value: &Value) -> TypedSchema {
        match value {
            Value::Object(map) => Self::from_json_schema(map),
            _ => Self::simple("any"),
        }
    }
}

/// Persisted embedding vector used for semantic search and similarity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRecord {
    pub id: RecordId,
    pub vector: Vec<f32>,
    pub model: String,
    pub content_type: String,
    pub content_hash: String,
    pub created_at: Option<Datetime>,
}

/// Typed, directional compatibility edge between two tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCompatibility {
    /// Stable database identifier for this compatibility edge.
    pub id: RecordId,
    /// Source tool whose output is being consumed.
    pub r#in: RecordId,
    /// Target tool whose input is being satisfied.
    pub out: RecordId,
    /// Kind of compatibility relationship between tools.
    pub compatibility_type: CompatibilityType,
    /// Confidence score in the compatibility.
    pub confidence: f32,
    /// Optional human-readable explanation of the compatibility.
    pub reasoning: Option<String>,
    /// When this compatibility edge was created.
    pub created_at: Option<Datetime>,
}

/// Payload used when inserting a new tool compatibility edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCompatibilityCreate {
    pub r#in: RecordId,
    pub out: RecordId,
    pub compatibility_type: CompatibilityType,
    pub confidence: f32,
    pub reasoning: Option<String>,
}

/// Categories of compatibility edges used by the planner and graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityType {
    DataFlow,
    SemanticSimilarity,
    Sequential,
    Parallel,
    Conditional,
    Transform,
}

/// Observed or inferred sequential relationship between two tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSequence {
    pub id: RecordId,
    pub r#in: RecordId,
    pub out: RecordId,
    pub sequence_type: String,
    pub frequency: u64,
    pub success_rate: f32,
    pub created_at: Option<Datetime>,
}

/// Payload used when inserting a new tool sequence edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSequenceCreate {
    pub r#in: RecordId,
    pub out: RecordId,
    pub sequence_type: String,
    pub frequency: u64,
    pub success_rate: f32,
}

/// Persisted representation of a manifest registry (GitHub, npm, custom, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryRecord {
    pub id: RecordId,
    pub url: String,
    pub name: String,
    pub description: Option<String>,
    pub is_active: bool,
    pub last_sync: Option<Datetime>,
    pub created_at: Option<Datetime>,
}

/// Payload used when inserting a new registry into the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryCreate {
    pub url: String,
    pub name: String,
    pub description: Option<String>,
    pub is_active: bool,
}

/// Persisted record of a fetched MCP manifest from a registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestRecord {
    pub id: RecordId,
    pub registry_id: RecordId,
    pub name: String,
    pub version: String,
    pub content: Value,
    pub hash: String,
    pub is_active: bool,
    pub created_at: Option<Datetime>,
}

/// Payload used when inserting a new manifest record into the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestCreate {
    pub registry_id: RecordId,
    pub name: String,
    pub version: String,
    pub content: Value,
    pub hash: String,
    pub is_active: bool,
}

/// High-level search query for tools, combining text and type filters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSearchQuery {
    pub text_query: Option<String>,
    pub input_types: Option<Vec<String>>,
    pub output_types: Option<Vec<String>>,
    pub service_ids: Option<Vec<RecordId>>,
    pub min_confidence: Option<f32>,
    pub include_embeddings: bool,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Result set for a tool search, including optional embeddings and timing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSearchResult {
    pub tools: Vec<ToolRecord>,
    pub total_count: u64,
    pub embeddings: Option<Vec<EmbeddingRecord>>,
    pub search_time_ms: u64,
}

/// Single tool suggestion produced by the planner for a user query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProposal {
    pub id: String,
    pub tool_id: RecordId,
    pub confidence: f32,
    pub reasoning: String,
    pub input_requirements: Option<Value>,
    pub expected_output: Option<Value>,
    pub dependencies: Vec<RecordId>,
    pub estimated_cost: Option<f32>,
}

/// Full plan for satisfying a user query using one or more tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPlan {
    pub id: String,
    pub query: String,
    pub proposals: Vec<ToolProposal>,
    pub selected_tools: Vec<RecordId>,
    pub execution_graph: Option<ExecutionGraph>,
    pub steps: Vec<PlanStep>,
    pub created_at: Datetime,
    pub status: PlanStatus,
}

/// Payload used when inserting a new tool plan into the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPlanCreate {
    pub query: String,
    pub proposals: Vec<ToolProposal>,
    pub selected_tools: Vec<RecordId>,
    pub execution_graph: Option<ExecutionGraph>,
    pub steps: Vec<PlanStep>,
}

/// Lifecycle stages for a tool plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Draft,
    Proposed,
    Approved,
    Executing,
    Completed,
    Failed,
}

/// Executable graph of tool invocations with dataflow edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub nodes: Vec<ExecutionNode>,
    pub edges: Vec<ExecutionEdge>,
}

/// A single node in an execution graph, bound to a specific tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: String,
    pub tool_id: RecordId,
    pub inputs: Option<Value>,
    pub outputs: Option<Value>,
    pub status: ExecutionStatus,
}

/// Runtime status of an execution node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

/// Edge between execution nodes indicating how data flows between them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEdge {
    pub from: String,
    pub to: String,
    pub data_path: String,
}

/// Linearized step view of a plan, useful for explanations and UIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub step_number: u32,
    pub tool_id: RecordId,
    pub inputs: Option<Value>,
    pub expected_outputs: Vec<String>,
    pub parallel: bool,
    pub dependencies: Vec<u32>,
}
