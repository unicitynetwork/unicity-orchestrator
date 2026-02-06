# Unicity Orchestrator

A knowledge graph-based orchestrator for Model Context Protocol (MCP) services with advanced tool discovery and symbolic reasoning capabilities.

## Overview

Unicity Orchestrator is a sophisticated system that manages and discovers MCP tools through a combination of:

- **Knowledge Graph**: Typed relationships between tools, services, and data types
- **Vector Embeddings**: Semantic similarity search for tool discovery
- **Symbolic Reasoning**: Rule-based inference for intelligent tool selection
- **Registry Integration**: Support for multiple MCP manifest registries *(in development)*
- **Chain Execution**: Multi-step tool execution with dependency management *(in development — rmcp 0.14+)*

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
- **Multi-Registry Support**: GitHub, npm, and custom MCP registries *(in development)*
- **Tool Discovery**: Automatic discovery and indexing of MCP services
- **Prompt Forwarding**: Aggregate prompts from all MCP services with intelligent conflict resolution
- **Resource Forwarding**: Aggregate resources from all MCP services with automatic discovery
- **Semantic Search**: Find tools by meaning, not just keywords
- **Type-Safe Graph**: Enforced compatibility between tool inputs/outputs
- **Symbolic Rules**: Define custom reasoning rules for tool selection
- **Planning**: Plan complex tool workflows; the LLM drives execution of each step

### Knowledge Graph Features
- **Typed Edges**: DataFlow, SemanticSimilarity, Sequential, etc.
- **Graph Traversal**: Find optimal tool chains for data transformations
- **Usage Patterns**: Learn from historical tool usage
- **Alternative Suggestions**: Find equivalent tools for the same task

### Prompt Forwarding
The orchestrator aggregates prompts from all configured MCP services and presents them through a unified interface:

- **Automatic Discovery**: Prompts are discovered during service initialization
- **Conflict Resolution**: When multiple services define prompts with the same name, the orchestrator creates namespaced aliases (e.g., `github-commit`, `gitlab-commit`)
- **Flexible Resolution**: Prompts can be accessed via:
  - Namespaced names: `github-commit`
  - Original prompt names: `commit` (resolves to the first match)
  - Service-prompt pattern: `github:commit` or `my-service:commit` (sanitized)
- **Case-Insensitive Matching**: All resolution patterns work regardless of case
- **Argument Validation**: Prompt names and arguments are validated to prevent injection attacks

### Resource Forwarding
The orchestrator aggregates resources from all configured MCP services and presents them through a unified interface:

- **Automatic Discovery**: Resources are discovered during service initialization
- **URI-Based Resolution**: Resources are accessed by their unique URI (e.g., `file:///path/to/file.txt`, `git://github.com/user/repo`)
- **Conflict Resolution**: When multiple services define resources with the same URI, the first service's version is used
- **Security Validation**: Resource URIs are validated to prevent path traversal and injection attacks
- **Resource Templates**: Support for parameterized resource templates (e.g., `git://{repo}/file/{path}`)

### API & CLI
- **REST API**: HTTP endpoints for tool queries and management
- **CLI Tool**: Command-line interface for management and queries
- **MCP Protocol**: Full MCP server via HTTP or stdio transport

## Quick Start

## Docker Deployment

Unicity Orchestrator can be run fully containerized together with SurrealDB.

### Using Docker Compose

The repository includes a `docker-compose.yml` file that launches the orchestrator in development mode:

- **In-Memory Mode (default)** — If no `SURREALDB_URL` is provided, the orchestrator automatically uses an in-memory SurrealDB instance.
- **External SurrealDB (optional)** — To connect to a real SurrealDB deployment, set `SURREALDB_URL` and related env variables.

```yaml
version: "3.9"

services:
  orchestrator:
    build: .
    environment:
      # Optional: If omitted, orchestrator runs with in-memory DB for development
      - SURREALDB_NAMESPACE=unicity
      - SURREALDB_DATABASE=orchestrator
      - SURREALDB_USERNAME=${SURREALDB_USERNAME:-root}
      - SURREALDB_PASSWORD=${SURREALDB_PASSWORD:-root}
      - MCP_BIND=0.0.0.0:3942
      - RUST_LOG=info,unicity_orchestrator=info
    ports:
      - "3942:3942"
```

Start the system with:

```bash
docker compose up --build
```

By default this runs with an in-memory database. To point the orchestrator at a SurrealDB instance, set `SURREALDB_URL`:

```bash
SURREALDB_URL=ws://localhost:8000/rpc docker compose up
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

By default the container starts through an entrypoint that:

- Uses **in-memory SurrealDB** if `SURREALDB_URL` is not set.
- Validates required database variables when `SURREALDB_URL` *is* set.

## Server Modes

Unicity Orchestrator provides multiple server interfaces, each designed for different use cases:

### Public REST API
Runs on a public-facing port (default: `0.0.0.0:8080`) and exposes only **read-only** endpoints:

- `GET /health`
- `POST /query` — semantic tool retrieval with user-supplied context

This API is safe to expose externally and is intended for user-facing applications.

### Admin REST API
Runs on a restricted/admin port (default: `127.0.0.1:8081`) and exposes **mutating** endpoints:

- `POST /discover` — rediscover & index tools from configured MCP services

These endpoints modify orchestrator state and should **not** be exposed publicly.
Use firewall rules, Docker port-mapping, or private network bindings to restrict access.

### MCP HTTP Server
The orchestrator also runs a full **MCP-compatible server** on its own port (default: `3942`).
This endpoint exposes the Model Context Protocol for agentic LLMs and MCP clients:

```
http://localhost:3942/mcp
```

This server is the primary interface for LLM-based workflows and tool execution.

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

This example demonstrates the current lifecycle of tool orchestration — from a natural-language request to tool discovery, semantic matching, symbolic reasoning, and execution.

### 1. User / LLM Request
The LLM (or user) sends a query via the REST API or MCP protocol:

```json
{
  "query": "List open GitHub issues from this repository and group them by severity.",
  "context": {
    "repo_url": "https://github.com/example/project"
  }
}
```

### 2. Semantic Retrieval
The orchestrator generates an embedding for the query and performs a cosine similarity search (top 32 results, 0.25 threshold). Candidate tools are ranked:

- `github.list_issues` — 0.91
- `github.get_repo` — 0.78
- `json.structure_data` — 0.72
- `text.summarize` — 0.68

Tools below the threshold are discarded.

### 3. Symbolic Reasoning
The symbolic reasoning engine loads candidates into working memory and applies forward/backward chaining over rules to adjust confidence scores and rank selections.

The orchestrator returns ranked tool selections to the caller:

```json
[
  {
    "tool_name": "github.list_issues",
    "service_name": "github-mcp",
    "confidence": 0.91,
    "reasoning": "High semantic similarity to query; tool lists GitHub issues."
  }
]
```

### 4. Execution
The LLM (or calling client) selects a tool and asks the orchestrator to execute it. The orchestrator routes the call to the correct child MCP service via its rmcp client:

```json
{
  "tool_name": "github.list_issues",
  "service_name": "github-mcp",
  "arguments": {
    "repo": "example/project",
    "state": "open"
  }
}
```

Execution is currently **single-tool per call**. The LLM is responsible for sequencing multiple tool calls — inspecting each result, deciding the next step, and passing arguments between tools.

> **Note:** Automatic multi-step chain execution (where the orchestrator executes a planned sequence of tools and pipes outputs between them) is a planned feature being developed alongside rmcp 0.14+. Today, the `plan_tools` endpoint can suggest a multi-step plan, but execution of each step is driven by the caller.

### 5. Final Response
The executed tool returns its result through the orchestrator:

```json
{
  "content": [
    {
      "type": "text",
      "text": "[{\"number\": 42, \"title\": \"Critical bug in auth\", \"labels\": [\"critical\"]}, ...]"
    }
  ]
}
```

The LLM can then call additional tools (e.g., a summarizer or formatter) using the same select-then-execute flow, or process the result directly.

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
cargo run -- init --db-url memory

# Discover tools from configured services
cargo run -- discover-tools

# Start the API server
cargo run -- server --port 8080

# Query for tools
cargo run -- query "read a file from filesystem"
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
- **Model**: Qwen3 0.6B (1024-dim, local inference via embed_anything)
- **Caching**: In-memory caching with content-hash deduplication
- **Similarity**: Cosine similarity with 0.25 threshold

### Symbolic Reasoner
- **Rules**: Forward and backward chaining inference
- **Facts**: Working memory with typed predicates
- **Planning**: Backward chaining for goal achievement

## API Endpoints

### Health Check
```
GET /health
```

Returns:
```json
{"status": "healthy", "timestamp": "2025-01-01T00:00:00Z"}
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

## Development

### Project Structure

TODO

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
