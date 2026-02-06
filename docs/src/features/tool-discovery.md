# Tool Discovery

Tool discovery is the process by which the orchestrator finds, indexes, and makes available tools from child MCP services and external registries.

## Discovery Pipeline

During warmup (`Orchestrator::warmup`), the orchestrator runs the full discovery pipeline:

```text
mcp.json → Start Services → List Tools → Normalize Schemas
         → Generate Embeddings → Build Knowledge Graph → Load Rules
         → Discover Prompts → Discover Resources
```

### 1. Start Services

The orchestrator reads `mcp.json` and starts each configured service:

- **Stdio services** — Spawned as child processes using `TokioChildProcess`
- **HTTP services** — Connected via `StreamableHttpClientTransport`

Disabled services (with `"disabled": true`) are skipped.

### 2. List Tools

Each running service is queried via the MCP `tools/list` method. The orchestrator collects all tools along with their:

- Name and description
- Input schema (JSON Schema)
- Output schema (if available)
- Type URIs (`input_ty`, `output_ty`)

### 3. Normalize Schemas

Raw JSON Schemas are converted to the internal `TypedSchema` format, supporting objects, arrays, unions, primitives, and enums.

### 4. Generate Embeddings

Each tool's content (name, description, schema text, type URIs) is combined and embedded using the Qwen3 model. Embeddings are cached by content hash — unchanged tools are not re-embedded.

### 5. Build Knowledge Graph

The graph is constructed with:
- Service nodes
- Tool nodes with `BelongsTo` edges to their service
- `DataFlow` edges between type-compatible tools
- Embeddings assigned to tool nodes

### 6. Load Symbolic Rules

Inference rules from the `symbolic_rule` database table are loaded into the rule engine.

## Re-Discovery

Tools can be re-discovered at runtime via:

- **Admin REST API**: `POST /discover` endpoint
- **CLI**: `cargo run -- discover-tools`

Re-discovery restarts child services, re-indexes tools, and rebuilds the graph.

## Registry Integration

> **Note:** External registry integration is planned but not yet implemented.

The orchestrator supports external registries for discovering additional MCP services:

| Registry Type | Description |
|---------------|-------------|
| HTTP | Generic HTTP-based MCP registry |
| GitHub | GitHub-hosted registry |
| npm | npm registry search |

Registries provide `RegistryManifest` entries that describe available MCP services, their versions, download URLs, and metadata.

## Querying Discovered Tools

After discovery, tools are available through:

1. **Semantic search** (`unicity.select_tool`) — Natural-language queries matched against embeddings
2. **Planning** (`unicity.plan_tools`) — Multi-step goal decomposition
3. **Direct execution** (`unicity.execute_tool`) — Execute by tool ID
4. **Listing** (`unicity.debug.list_tools`) — Paginated listing with filters

### Query Pipeline

```text
Natural language query
       │
       ▼
Embed query (1024-dim vector)
       │
       ▼
Cosine similarity search (top 32, threshold 0.25)
       │
       ▼
Symbolic reasoning (forward chaining)
       │
       ▼
User filtering (blocked/trusted services)
       │
       ▼
Trust boost (confidence increase for trusted services)
       │
       ▼
Ranked tool selections
```
