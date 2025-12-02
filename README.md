# Unicity Orchestrator

A knowledge graph-based orchestrator for Model Context Protocol (MCP) services with advanced tool discovery and symbolic reasoning capabilities.

## Overview

Unicity Orchestrator is a sophisticated system that manages and discovers MCP tools through a combination of:

- **Knowledge Graph**: Typed relationships between tools, services, and data types
- **Vector Embeddings**: Semantic similarity search for tool discovery
- **Symbolic Reasoning**: Rule-based inference for intelligent tool selection
- **Registry Integration**: Support for multiple MCP manifest registries
- **Async Execution**: Parallel tool execution with dependency management

### LLM Interaction Model

Unicity Orchestrator is designed to operate **with minimal reliance on large language models** during planning and execution. The LLM is only involved at two points:

1. **Initial Intent Description** – The LLM (or user) provides a natural-language description of the task and optional high-level steps. The orchestrator does *not* require the LLM to name tools or manipulate schemas.
2. **Fallback Assistance** – The orchestrator asks the LLM for clarification or reformulation only if:
   - no semantically relevant tools are found,
   - no valid type-safe tool chain exists,
   - or a runtime error cannot be auto-resolved.

All core decision-making is handled internally by:
- **Semantic Retrieval** via vector embeddings
- **Type Normalization** from JSON Schemas
- **Knowledge-Graph Traversal** for compatibility and chaining
- **Symbolic Reasoning** for rule-based inference

This design ensures fast, predictable, and cost-efficient orchestration while keeping LLM interactions intentional and minimal.

## Features

### Core Capabilities
- **Multi-Registry Support**: GitHub, npm, and custom MCP registries
- **Tool Discovery**: Automatic discovery and indexing of MCP services
- **Semantic Search**: Find tools by meaning, not just keywords
- **Type-Safe Graph**: Enforced compatibility between tool inputs/outputs
- **Symbolic Rules**: Define custom reasoning rules for tool selection
- **Async Planning**: Plan and execute complex tool workflows

### Knowledge Graph Features
- **Typed Edges**: DataFlow, SemanticSimilarity, Sequential, etc.
- **Graph Traversal**: Find optimal tool chains for data transformations
- **Usage Patterns**: Learn from historical tool usage
- **Alternative Suggestions**: Find equivalent tools for the same task

### API & CLI
- **REST API**: HTTP endpoints for all orchestration functions
- **CLI Tool**: Command-line interface for management and queries
- **Real-time Updates**: Async updates to tool status and availability

## Quick Start

## Docker Deployment

Unicity Orchestrator can be run fully containerized together with SurrealDB.

### Using Docker Compose

The repository includes a `docker-compose.yml` file that launches:

- **SurrealDB** (persistent database)
- **Unicity Orchestrator** running the **MCP HTTP server** on port `3942`

Example compose file:

```yaml
version: "3.9"

services:
  surrealdb:
    image: surrealdb/surrealdb:latest
    command: >
      start
      --log info
      --user ${SURREALDB_USERNAME:-root}
      --pass ${SURREALDB_PASSWORD:-root}
      file:/data/surreal.db
    ports:
      - "8000:8000"
    volumes:
      - surreal-data:/data

  orchestrator:
    build: .
    depends_on:
      - surrealdb
    environment:
      - SURREALDB_URL=ws://surrealdb:8000
      - SURREALDB_NAMESPACE=unicity
      - SURREALDB_DATABASE=orchestrator
      - SURREALDB_USERNAME=${SURREALDB_USERNAME:-root}
      - SURREALDB_PASSWORD=${SURREALDB_PASSWORD:-root}
      - MCP_BIND=0.0.0.0:3942
      - RUST_LOG=info,unicity_orchestrator=info
    ports:
      - "3942:3942"
    command: >
      sh -c "unicity-orchestrator mcp-http
             --bind ${MCP_BIND}
             --db-url ${SURREALDB_URL}"

volumes:
  surreal-data:
```

Start the system with:

```bash
docker compose up --build
```

The MCP server will be accessible at:

```
http://localhost:3942/mcp
```

### Using the Dockerfile directly

A multi-stage `Dockerfile` is included. To build manually:

```bash
docker build -t unicity-orchestrator .
```

Run with environment variables:

```bash
docker run --rm \
  -e SURREALDB_URL=ws://host.docker.internal:8000 \
  -e SURREALDB_NAMESPACE=unicity \
  -e SURREALDB_DATABASE=orchestrator \
  -e SURREALDB_USERNAME=root \
  -e SURREALDB_PASSWORD=root \
  -p 3942:3942 \
  unicity-orchestrator
```

By default the container runs:

```
unicity-orchestrator mcp-http --bind 0.0.0.0:3942 --db-url $SURREALDB_URL
```

## Server Modes

Unicity Orchestrator provides multiple server interfaces, each designed for different use cases:

### Public REST API
Runs on a public-facing port (default: `0.0.0.0:8080`) and exposes only **read‑only** endpoints:

- `GET /health`
- `POST /query` — semantic tool retrieval with user‑supplied context

This API is safe to expose externally and is intended for user‑facing applications.

### Admin REST API
Runs on a restricted/admin port (default: `127.0.0.1:8081`) and exposes **mutating** endpoints:

- `POST /sync` — sync all MCP registries
- `POST /discover` — rediscover & index tools from configured MCP services

These endpoints modify orchestrator state and should **not** be exposed publicly.  
Use firewall rules, Docker port‑mapping, or private network bindings to restrict access.

### MCP HTTP Server
The orchestrator also runs a full **MCP‑compatible server** on its own port (default: `3942`).  
This endpoint exposes the Model Context Protocol for agentic LLMs and MCP clients:

```
http://localhost:3942/mcp
```

This server is the primary interface for LLM‑based workflows and tool execution.

### MCP Stdio Server
For local development or integration with tools that expect an `stdio` MCP transport:

```bash
unicity-orchestrator mcp-stdio --db-url memory
```

When using SurrealDB instead of the in-memory database, you can provide full database configuration to the stdio server through environment variables or CLI flags. For example:

```bash
SURREALDB_URL=ws://localhost:8000 \
SURREALDB_NAMESPACE=unicity \
SURREALDB_DATABASE=orchestrator \
SURREALDB_USERNAME=root \
SURREALDB_PASSWORD=root \
unicity-orchestrator mcp-stdio --db-url ws://localhost:8000
```

This mode is typically used with local MCP client tooling rather than production deployments.


## End-to-End Example

This example demonstrates the full lifecycle of task orchestration — from a natural-language request to tool discovery, semantic matching, planning, and execution. It shows how Unicity Orchestrator handles tasks **without requiring the LLM to know any tools**.

### 1. User / LLM Request
The LLM (or user) sends an orchestration request to the `/query` endpoint:

```json
{
  "query": "Summarize open GitHub issues from this repository and group them by severity.",
  "context": {
    "repo_url": "https://github.com/example/project"
  }
}
```

### 2. Semantic Retrieval
Unicity generates an embedding for the query and performs a vector search to find relevant tools:

- `github.list_issues`
- `github.get_repo`
- `json.structure_data`
- `text.summarize`

These are ranked by semantic similarity; irrelevant tools are discarded.

### 3. Type-Aware Graph Filtering
The orchestrator inspects each candidate tool:

- It loads the normalized `input_ty` / `output_ty` from the database.
- It identifies type-compatible sequences:
  - `github.list_issues` → produces `Issue[]`
  - `json.structure_data` → accepts `Issue[]` and produces structured JSON
  - `text.summarize` → accepts text or JSON

It eliminates incompatible paths and keeps only the valid dataflows.

### 4. Symbolic Reasoning
Symbolic rules reinforce expected tool chains:

- "Listing issues" is commonly followed by "filtering" or "processing" tools.
- Rules may recommend grouping, summarizing, or transforming steps.

The planner generates the final sequence:

1. `github.list_issues`
2. `json.structure_data`
3. `text.summarize`

### 5. Execution
The orchestrator executes each step through its rmcp client:

- Calls GitHub MCP service → retrieves raw issues
- Calls JSON transformer → groups by severity
- Calls summarizer → produces a clean summary

All arguments are derived from context + prior tool outputs.

### 6. Final Response
The orchestrator returns the final structured output:

```json
{
  "summary": "12 open issues found. Critical: 2, High: 4, Medium: 3, Low: 3.",
  "grouped_issues": {
    "critical": [...],
    "high": [...],
    "medium": [...],
    "low": [...]
  }
}
```

The LLM can present or further refine the result, but the entire orchestration flow — discovery, reasoning, planning, and execution — is automated.

### Installation

```bash
# Clone the repository
git clone <repository-url>
cd unicity-orchestrator

# Build the project
cargo build --release
```

### Configuration

### MCP Configuration File (mcp.json)

The orchestrator requires an `mcp.json` file that lists external MCP services to load.  
When running inside Docker or directly from the binary, the orchestrator will attempt to locate this file in the working directory (`./mcp.json`).

**Automatic Behavior:**

- If an `mcp.json` is present in the project root during Docker build, it will be included in the image automatically.
- If no `mcp.json` exists at runtime, the orchestrator will create a minimal default file:

```json
{ "mcpServers": {} }
```

This ensures the system always starts cleanly even without external MCP services configured.  
You may add services to this file at any time and restart the orchestrator.

Create a `mcp.json` file to configure your MCP services:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
      "env": {}
    },
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"
      }
    }
  }
}
```

### Running

```bash
# Initialize the database
cargo run init

# Discover tools from configured services
cargo run discover-tools

# Start the API server
cargo run server --port 8080

# Query for tools
cargo run query "read a file from filesystem"

# Sync with registries
cargo run sync-registries
```

## Architecture

### Database Layer
- **SurrealDB**: Multi-model database with graph capabilities
- **Schema**: Typed tables for services, tools, embeddings, and relationships
- **Queries**: Optimized for graph traversal and similarity search

### Knowledge Graph
- **Nodes**: Services, Tools, Types, Concepts, Registries
- **Edges**: Typed relationships with confidence scores
- **Traversal**: BFS/DFS with type checking and compatibility rules

### Embedding Engine
- **Models**: Support for multiple embedding models
- **Caching**: In-memory caching for frequently accessed embeddings
- **Similarity**: Cosine similarity with configurable thresholds

### Symbolic Reasoner
- **Rules**: Forward and backward chaining inference
- **Facts**: Working memory with typed predicates
- **Planning**: Backward chaining for goal achievement

## API Endpoints

### Health Check
```
GET /health
```

### Query Tools
```
POST /query
{
  "query": "read a json file",
  "context": {
    "file_path": "/path/to/file.json"
  }
}
```

### Sync Registries
```
POST /sync
```

### Discover Tools
```
POST /discover
```

## Configuration

### Database
```env
SURREALDB_URL=ws://localhost:8000/rpc
SURREALDB_NAMESPACE=unicity
SURREALDB_DATABASE=orchestrator
SURREALDB_USERNAME=root
SURREALDB_PASSWORD=password
```

### Registries
```rust
use unicity_orchestrator::mcp::registry::RegistryConfig;

let github_registry = RegistryConfig {
    id: "github".to_string(),
    name: "GitHub MCP Registry".to_string(),
    url: "https://raw.githubusercontent.com/mcp/registry/main".to_string(),
    description: Some("Official MCP registry on GitHub".to_string()),
    auth_token: None,
    sync_interval: Duration::from_secs(3600),
    is_active: true,
};
```

## Development

### Project Structure

TODO

### Adding New Registry Providers

Implement the `RegistryProvider` trait:

```rust
#[async_trait]
impl RegistryProvider for MyRegistryProvider {
    async fn list_manifests(&self) -> Result<Vec<RegistryManifest>>;
    async fn get_manifest(&self, name: &str, version: &str) -> Result<Option<RegistryManifest>>;
    async fn download_manifest(&self, manifest: &RegistryManifest) -> Result<serde_json::Value>;
    async fn verify_manifest(&self, manifest: &RegistryManifest, content: &[u8]) -> Result<bool>;
}
```

### Adding Symbolic Rules

```rust
use unicity_orchestrator::knowledge_graph::symbolic::*;

let rule = SymbolicRule {
    id: "file_operation_chain".to_string(),
    name: "File Operation Chain".to_string(),
    description: "Chain file read with data processing".to_string(),
    antecedents: vec![
        SymbolicExpression::Fact(Fact {
            predicate: "tool_selected".to_string(),
            arguments: vec![
                SymbolicExpression::Variable("tool".to_string()),
                SymbolicExpression::Literal(LiteralValue::String("file_read".to_string()))
            ],
            confidence: Some(0.9),
        })
    ],
    consequents: vec![
        SymbolicExpression::Fact(Fact {
            predicate: "suggest_following_tool".to_string(),
            arguments: vec![
                SymbolicExpression::Variable("following_tool".to_string()),
                SymbolicExpression::Literal(LiteralValue::String("data_parse".to_string()))
            ],
            confidence: Some(0.8),
        })
    ],
    confidence: 0.85,
    priority: 100,
};
```

## License

MIT License - see LICENSE file for details.

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests
5. Submit a pull request

## Support

For issues and questions, please use the GitHub issue tracker.
