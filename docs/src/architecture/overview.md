# Architecture Overview

Unicity Orchestrator is built around a pipeline that transforms natural-language queries into tool executions through semantic search, graph reasoning, and symbolic inference.

## Core Flow

```text
┌──────────┐     ┌──────────────┐     ┌─────────────┐     ┌───────────┐
│  Warmup  │────▶│  Query Tools │────▶│  Plan Tools │────▶│  Execute  │
└──────────┘     └──────────────┘     └─────────────┘     └───────────┘
```

### 1. Warmup (`Orchestrator::warmup`)

At startup, the orchestrator runs a warmup pipeline:

1. **Discover tools** — Starts child MCP services from `mcp.json`, lists tools from each
2. **Normalize types** — Converts JSON Schemas to internal `TypedSchema` representation
3. **Generate embeddings** — Creates vector embeddings for all tools using `embed_anything`
4. **Build knowledge graph** — Constructs the graph with services, tools, and edges
5. **Load symbolic rules** — Loads inference rules from the `symbolic_rule` database table
6. **Discover prompts** — Aggregates prompts from child services
7. **Discover resources** — Aggregates resources from child services

### 2. Query (`Orchestrator::query_tools`)

When a query arrives:

1. **Semantic search** — Top 32 results with similarity threshold of 0.25
2. **Symbolic reasoning** — Forward chaining with the working memory
3. **User filtering** — Applies per-user blocked/trusted service lists
4. **Trust boost** — Increases confidence for trusted services
5. **Fallback** — Uses raw embedding results if symbolic reasoning yields nothing

### 3. Execute (`Orchestrator::execute_selected_tool`)

Selected tools are executed through the rmcp client, forwarding calls to the appropriate child MCP service.

## Module Map

```text
src/
├── bin/main.rs          # CLI entry point (clap)
├── lib.rs               # Library root, server creation
├── server.rs            # MCP HTTP/stdio server (rmcp)
├── mcp_client.rs        # Child MCP service communication
├── executor.rs          # Tool execution dispatcher
├── config.rs            # mcp.json parsing and resolution
├── types.rs             # Newtype wrappers (ToolId, ServiceId, etc.)
├── registry.rs          # External registry management
│
├── orchestrator/
│   ├── mod.rs           # Central coordinator
│   └── user_filter.rs   # Per-user tool filtering
│
├── knowledge_graph/
│   ├── graph.rs         # Graph structures (nodes, edges, types)
│   ├── embedding.rs     # Vector embedding manager
│   ├── symbolic.rs      # Rule engine and working memory
│   └── traversal.rs     # Graph traversal algorithms
│
├── db/
│   ├── connection.rs    # SurrealDB connection and schema
│   ├── schema.rs        # Record types and typed schemas
│   └── queries.rs       # Query builder
│
├── tools/
│   ├── registry.rs      # ToolHandler trait and ToolRegistry
│   ├── select_tool.rs   # unicity.select_tool handler
│   ├── plan_tools.rs    # unicity.plan_tools handler
│   ├── execute_tool.rs  # unicity.execute_tool handler
│   └── list_discovered_tools.rs  # unicity.debug.list_tools handler
│
├── auth/
│   ├── context.rs       # UserContext
│   ├── extractor.rs     # AuthExtractor, AuthConfig
│   ├── jwks.rs          # JWKS key cache for JWT
│   └── user_store.rs    # User persistence and preferences
│
├── elicitation/
│   ├── mod.rs           # ElicitationCoordinator
│   ├── approval.rs      # Tool approval manager
│   ├── form.rs          # Form-based elicitation
│   ├── url.rs           # URL/OAuth elicitation
│   ├── store.rs         # Permission storage
│   ├── provenance.rs    # Service attribution
│   └── error.rs         # Error types
│
├── prompts/mod.rs       # Prompt forwarding registry
├── resources/mod.rs     # Resource forwarding registry
└── api/mod.rs           # REST API (axum)
```

## Key Components

### Orchestrator

The central coordinator (`src/orchestrator/mod.rs`) owns:

- Database connection
- Knowledge graph
- Embedding manager
- Symbolic reasoner
- Running service clients

All query and execution operations flow through the orchestrator.

### MCP Server

The `McpServer` (`src/server.rs`) implements the MCP protocol via rmcp. It handles:

- Tool listing and execution
- Prompt and resource forwarding
- Client capability negotiation
- Authentication extraction
- Subscription management

### Tool Registry

The `ToolRegistry` (`src/tools/registry.rs`) manages the orchestrator's own MCP tools:

- `unicity.select_tool` — Semantic tool search
- `unicity.plan_tools` — Multi-step plan generation
- `unicity.execute_tool` — Tool execution
- `unicity.debug.list_tools` — Debug listing of all discovered tools

## Database

SurrealDB stores 13 tables:

| Table | Purpose |
|-------|---------|
| `service` | Registered MCP services |
| `tool` | Discovered tools with schemas |
| `embedding` | Vector embeddings |
| `tool_compatibility` | Type-compatible tool edges |
| `tool_sequence` | Historical tool sequences |
| `registry` | External registry configs |
| `manifest` | Registry manifests |
| `symbolic_rule` | Inference rules |
| `permission` | User tool permissions |
| `user` | User accounts |
| `user_preferences` | Per-user settings |
| `audit_log` | Action audit trail |
| `api_key` | API key records |

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `rmcp` | Rust MCP client/server implementation |
| `surrealdb` | Multi-model database |
| `embed_anything` | Vector embedding generation |
| `axum` | HTTP server framework |
| `clap` | CLI argument parsing |
| `jsonwebtoken` | JWT validation |
| `tokio` | Async runtime |
