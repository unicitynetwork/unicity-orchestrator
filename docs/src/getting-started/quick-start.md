# Quick Start

This guide walks you through getting Unicity Orchestrator running and executing your first tool query.

## 1. Configure MCP Services

Create an `mcp.json` file in your project root:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
      "env": {}
    }
  }
}
```

This configures a single filesystem MCP server. You can add more services later.

## 2. Initialize and Discover

```bash
# Initialize the database (in-memory for quick testing)
cargo run -- init --db-url memory

# Discover tools from configured MCP services
cargo run -- discover-tools
```

During discovery, the orchestrator will:
1. Spawn each configured MCP service
2. List available tools from each service
3. Normalize tool schemas
4. Generate vector embeddings
5. Build the knowledge graph

## 3. Query for Tools

```bash
cargo run -- query "read a file from the filesystem" --db-url memory
```

The orchestrator performs semantic search against the indexed tools and returns ranked matches with confidence scores.

## 4. Start the MCP Server

To expose the orchestrator as an MCP server that LLMs can connect to:

```bash
# MCP HTTP server (recommended for remote clients)
cargo run -- mcp-http --bind 0.0.0.0:3942 --db-url memory

# Or MCP stdio server (for local client integration)
cargo run -- mcp-stdio --db-url memory
```

The MCP HTTP endpoint will be available at `http://localhost:3942/mcp`.

## 5. Connect an LLM

Point your MCP-compatible LLM client at the orchestrator. The orchestrator exposes four tools:

| Tool | Description |
|------|-------------|
| `unicity.select_tool` | Semantic search for the best matching tool |
| `unicity.plan_tools` | Generate a multi-step tool execution plan |
| `unicity.execute_tool` | Execute a specific tool by ID |
| `unicity.debug.list_tools` | List all discovered tools |

A typical LLM workflow:

1. Call `unicity.select_tool` with a natural-language query
2. Review the returned tool selection (confidence, reasoning)
3. Call `unicity.execute_tool` with the selected tool ID and arguments

## Next Steps

- [Configuration](configuration.md) — Customize MCP services, environment variables, and auth
- [Server Modes](../server-modes/overview.md) — Understand the different server interfaces
- [Architecture](../architecture/overview.md) — Learn how the system works internally
