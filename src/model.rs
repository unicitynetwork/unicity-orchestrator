use rmcp::model::{ServerInfo, Tool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::RecordId;
use uuid::Uuid;

pub type McpServiceId = RecordId;

// A stable representation of a service as stored in Surreal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRecord {
    pub id: McpServiceId,
    pub server_info: ServerInfo,
}

// High‑level origin information (useful for debugging + registry integration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceOrigin {
    StaticConfig,
    Registry { registry_id: String },
    Broadcast { host: String },
}

// Internal typed schema IR for reasoning about tool I/O compatibility.
//
// This is intentionally conservative: it captures only the structural aspects
// that matter for planning and type compatibility. If a feature of JSON Schema
// is not recognized, we fall back to `Ty::Any`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Ty {
    /// Unknown or too complex type.
    Any,
    /// Explicit JSON `null`.
    Null,
    String,
    Integer,
    Number,
    Boolean,
    Array(Box<Ty>),
    Object {
        /// Known object properties and their types.
        properties: std::collections::BTreeMap<String, Ty>,
        /// Whether additional, unknown properties are allowed.
        additional: bool,
    },
    /// A union of multiple possible types (e.g. anyOf/oneOf or ["string", "null"]).
    Union(Vec<Ty>),
}

impl Ty {
    /// Construct a `Ty` from a JSON Schema-like value.
    ///
    /// This is intentionally conservative and only understands a small subset
    /// of JSON Schema:
    /// - `type`: "string" | "integer" | "number" | "boolean" | "null"
    /// - `type`: "array" with `items`
    /// - `type`: "object" with `properties` and optional `additionalProperties`
    /// - `type`: [ ... ] where elements are primitive type strings (treated as `Union`)
    /// - `anyOf` / `oneOf` (treated as `Union`)
    ///
    /// Anything else is mapped to `Ty::Any`. This is suitable for building a
    /// symbolic graph where we care mainly about structural compatibility
    /// between tool inputs and outputs, not full JSON Schema fidelity.
    pub fn from_json_schema(schema: &Value) -> Ty {
        // Helper: map a simple type string to Ty.
        fn simple_type(s: &str) -> Ty {
            match s {
                "string" => Ty::String,
                "integer" => Ty::Integer,
                "number" => Ty::Number,
                "boolean" => Ty::Boolean,
                "null" => Ty::Null,
                "array" => Ty::Array(Box::new(Ty::Any)),
                "object" => Ty::Object {
                    properties: std::collections::BTreeMap::new(),
                    additional: true,
                },
                _ => Ty::Any,
            }
        }

        // If there is a `type` field, handle that first.
        if let Some(type_value) = schema.get("type") {
            match type_value {
                Value::String(s) => {
                    match s.as_str() {
                        // Primitive types map directly.
                        "string" | "integer" | "number" | "boolean" | "null" => simple_type(s),
                        "array" => {
                            // Look for `items` schema.
                            let item_ty = schema
                                .get("items")
                                .map(Ty::from_json_schema)
                                .unwrap_or(Ty::Any);
                            Ty::Array(Box::new(item_ty))
                        }
                        "object" => {
                            let mut props = std::collections::BTreeMap::new();
                            if let Some(Value::Object(map)) = schema.get("properties") {
                                for (name, prop_schema) in map {
                                    props.insert(name.clone(), Ty::from_json_schema(prop_schema));
                                }
                            }

                            // `additionalProperties` can be a bool or a schema; for now we only
                            // distinguish between allowed vs disallowed.
                            let additional = schema
                                .get("additionalProperties")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);

                            Ty::Object {
                                properties: props,
                                additional,
                            }
                        }
                        _ => Ty::Any,
                    }
                }
                Value::Array(arr) => {
                    // type: ["string", "null"] etc. -> Union of simple types.
                    let mut tys = Vec::new();
                    for v in arr {
                        if let Value::String(s) = v {
                            tys.push(simple_type(s));
                        }
                    }
                    if tys.is_empty() {
                        Ty::Any
                    } else if tys.len() == 1 {
                        tys.into_iter().next().unwrap()
                    } else {
                        Ty::Union(tys)
                    }
                }
                _ => Ty::Any,
            }
        } else if let Some(variants) = schema.get("anyOf").or_else(|| schema.get("oneOf")) {
            // anyOf/oneOf: treat as Union of subschemas.
            if let Value::Array(arr) = variants {
                let mut tys = Vec::new();
                for v in arr {
                    tys.push(Ty::from_json_schema(v));
                }
                if tys.is_empty() {
                    Ty::Any
                } else if tys.len() == 1 {
                    tys.into_iter().next().unwrap()
                } else {
                    Ty::Union(tys)
                }
            } else {
                Ty::Any
            }
        } else {
            // Fallback: we don't understand this schema; treat as unknown.
            Ty::Any
        }
    }
}

// Wrapper around rmcp::model::Tool plus metadata we attach.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedTool {
    pub id: RecordId,
    pub service_id: McpServiceId,

    #[serde(flatten)]
    pub raw: Tool,

    // Metadata for embeddings + planning:
    pub embedding_id: Option<RecordId>,
    pub input_ty: Option<Ty>,
    pub output_ty: Option<Ty>,
    pub usage_count: u64,
}

impl IndexedTool {
    /// Construct an `IndexedTool` from an MCP `Tool` and its parent service id.
    ///
    /// This assigns a fresh SurrealDB record id for the tool and leaves typed
    /// schemas unset for now. Typed schemas (`input_ty` / `output_ty`) can be
    /// populated later by a separate normalization pass that inspects
    /// `raw.input_schema` / `raw.output_schema`.
    pub fn from_mcp(service_id: McpServiceId, raw: Tool) -> Self {
        let id = RecordId::from(("tool", Uuid::new_v4().to_string()));

        IndexedTool {
            id,
            service_id,
            raw,
            embedding_id: None,
            input_ty: None,
            output_ty: None,
            usage_count: 0,
        }
    }
}
