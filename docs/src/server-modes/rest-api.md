# REST API

The REST API provides a traditional HTTP/JSON interface for web applications and administrative operations. It runs two separate servers: a public API and an admin API.

## Starting the Server

```bash
unicity-orchestrator server --port 8080 --db-url memory
```

## CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--port` | `8080` | Public API port |
| `--admin-port` | `8081` | Admin API port |
| `--db-url` | env or `memory` | Database connection URL |

## Public API

Runs on `0.0.0.0:{port}` (default 8080). Exposes **read-only** endpoints safe for external access.

### `GET /health`

Health check endpoint.

**Response:**
```json
{
  "status": "ok"
}
```

### `POST /query`

Semantic tool search.

**Request:**
```json
{
  "query": "read a json file",
  "context": {
    "file_path": "/path/to/file.json"
  }
}
```

**Response:** Array of tool selections with confidence scores.

### `GET /services`

List registered MCP services.

## Admin API

Runs on `127.0.0.1:{admin-port}` (default 8081). Exposes **mutating** endpoints that modify orchestrator state.

> **Security:** The admin API binds to localhost by default. Do not expose it publicly. Use firewall rules, Docker port mapping, or private network bindings to restrict access.

### `POST /discover`

Trigger tool re-discovery from configured MCP services. This restarts child processes, re-lists tools, regenerates embeddings, and rebuilds the knowledge graph.

### `POST /sync`

Sync with external MCP registries. (Under development.)

## CORS

Both APIs use permissive CORS settings to allow browser-based clients.

## Authentication

The REST API does not include authentication. For authenticated access, use the [MCP HTTP server](mcp-http.md) which supports JWT, API keys, and anonymous access modes.
