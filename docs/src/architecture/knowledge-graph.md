# Knowledge Graph

The knowledge graph is the central data structure that maps relationships between services, tools, types, and concepts. It enables type-aware tool chaining and compatibility analysis.

## Structure

### Nodes

Each node in the graph represents an entity:

```rust
pub struct GraphNode {
    pub id: RecordId,
    pub node_type: NodeType,
    pub data: serde_json::Value,
    pub embeddings: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
}
```

**Node types:**

| Type | Description |
|------|-------------|
| `Service` | An MCP service (e.g., "github", "filesystem") |
| `Tool` | A specific tool exposed by a service |
| `Type` | A data type used in tool inputs/outputs |
| `Concept` | An abstract concept for semantic grouping |
| `Registry` | An external MCP registry |

### Edges

Edges encode typed relationships between nodes:

```rust
pub struct GraphEdge {
    pub from: RecordId,
    pub to: RecordId,
    pub edge_type: EdgeType,
    pub weight: f64,
    pub metadata: HashMap<String, serde_json::Value>,
}
```

**Edge types:**

| Type | Description |
|------|-------------|
| `DataFlow` | Data flows from one tool's output to another's input |
| `SemanticSimilarity` | Tools are semantically related |
| `Sequential` | Tools are commonly used in sequence |
| `Parallel` | Tools can execute in parallel |
| `Conditional` | Conditional execution relationship |
| `Transform` | One tool transforms data for another |
| `BelongsTo` | Tool belongs to a service |
| `TypeRelation` | Type inheritance or compatibility |
| `ConceptRelation` | Conceptual relationship |

## Type System

The knowledge graph includes a type system that tracks compatibility between tool inputs and outputs:

```rust
pub struct TypeSystem {
    types: HashMap<String, TypeNode>,
    compatibility_rules: Vec<CompatibilityRule>,
}
```

Type compatibility is determined by:

- **Direct match** — Types are identical
- **Inheritance** — A child type is compatible with its parent (0.8 confidence)
- **Compatibility rules** — Custom rules defining type conversions

## Graph Building

The graph is constructed during warmup from database records:

1. Load all services → create `Service` nodes
2. Load all tools → create `Tool` nodes with `BelongsTo` edges to their service
3. Load `tool_compatibility` edges → create `DataFlow` edges between compatible tools
4. Assign embeddings to nodes from the embedding table

## Traversal

The graph supports pathfinding with configurable constraints:

- **BFS traversal** with maximum depth limit
- **Edge type filtering** — Only follow specific edge types
- **Similarity computation** — `1.0 / (1.0 + distance)` based on path length
- **Path finding** — Find routes between any two nodes through compatible edges

This enables the orchestrator to discover multi-step tool chains where one tool's output type matches the next tool's input type.
