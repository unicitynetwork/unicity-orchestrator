# MCP Stdio Server

The MCP stdio server runs the orchestrator as a local process communicating over standard input/output. This is the standard transport for MCP client integrations that spawn child processes.

## Starting the Server

```bash
unicity-orchestrator mcp-stdio --db-url memory
```

## CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--db-url` | env or `memory` | Database connection URL |

## Usage

The stdio server is designed for MCP clients that manage child processes. It always runs in anonymous mode — there is no authentication layer.

### With In-Memory Database

For quick local testing:

```bash
unicity-orchestrator mcp-stdio --db-url memory
```

### With External SurrealDB

For connecting to a shared database:

```bash
SURREALDB_URL=ws://localhost:8000/rpc \
SURREALDB_NAMESPACE=unicity \
SURREALDB_DATABASE=orchestrator \
SURREALDB_USERNAME=root \
SURREALDB_PASSWORD=root \
unicity-orchestrator mcp-stdio --db-url ws://localhost:8000/rpc
```

## Client Configuration

To use the orchestrator as a child MCP service in another tool's `mcp.json`:

```json
{
  "mcpServers": {
    "unicity": {
      "command": "unicity-orchestrator",
      "args": ["mcp-stdio", "--db-url", "memory"]
    }
  }
}
```

## Capabilities

The stdio server exposes the same capabilities as the [MCP HTTP server](mcp-http.md):

- Tools (select, plan, execute, list)
- Prompts (with list-changed notifications)
- Resources (with list-changed and subscribe support)

The only difference is the transport layer and the lack of authentication.
