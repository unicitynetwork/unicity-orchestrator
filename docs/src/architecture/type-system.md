# Type System

The type system ensures that tool chains are valid by tracking input and output types as URI-like identifiers and checking compatibility through the knowledge graph.

## Typed Inputs and Outputs

Each tool can declare typed inputs and outputs:

| Field | Description |
|-------|-------------|
| `input_ty` | URI describing the type this tool accepts |
| `output_ty` | URI describing the type this tool produces |

For example, a GitHub issue lister might have:

- `input_ty`: `github://repo/url`
- `output_ty`: `github://issues/list`

A JSON processor might accept `github://issues/list` as its `input_ty`, creating a valid data flow edge in the knowledge graph.

## Schema Normalization

Tool input schemas from MCP services (JSON Schema format) are normalized into an internal representation:

```rust
pub struct TypedSchema {
    pub schema_type: String,
    pub properties: Option<HashMap<String, TypedSchema>>,
    pub items: Option<Box<TypedSchema>>,
    pub required: Option<Vec<String>>,
    pub enum_values: Option<Vec<String>>,
}
```

The normalizer handles:

- **Object types** — Properties with nested schemas and required fields
- **Array types** — Item type tracking
- **Union types** — `anyOf`/`oneOf` flattening
- **Primitive types** — String, number, integer, boolean
- **Enum types** — Enumerated string values

## Type Compatibility

The knowledge graph's `TypeSystem` checks compatibility between types:

```rust
pub struct TypeSystem {
    types: HashMap<String, TypeNode>,
    compatibility_rules: Vec<CompatibilityRule>,
}
```

Compatibility is resolved through:

1. **Direct match** — Types are identical (confidence: 1.0)
2. **Inheritance** — Child type is compatible with parent type (confidence: 0.8)
3. **Compatibility rules** — Custom rules defining type conversions

## Newtype Wrappers

The codebase uses strongly-typed wrappers for identifiers to prevent mixing:

| Type | Description |
|------|-------------|
| `ToolId` | Tool identifier |
| `ToolName` | Tool name |
| `ServiceId` | Service identifier |
| `ServiceName` | Service name |
| `ResourceUri` | Resource URI |
| `PromptName` | Prompt name |
| `ApiKeyHash` | Hashed API key |
| `ApiKeyPrefix` | API key prefix for display |
| `ExternalUserId` | External identity provider user ID |
| `IdentityProvider` | Identity provider name |

All are generated via a `newtype_string!` macro and implement `Display`, `From<String>`, `AsRef<str>`, `Hash`, `Serialize`, and `Deserialize`.

## Tool Compatibility Edges

During knowledge graph construction, `tool_compatibility` records from the database are loaded as `DataFlow` edges. These edges connect tools where one tool's `output_ty` matches another tool's `input_ty`, enabling the graph traversal to discover valid multi-step tool chains.
