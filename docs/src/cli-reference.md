# CLI Reference

The `unicity-orchestrator` binary provides all management and server operations through subcommands.

## Global Usage

```bash
unicity-orchestrator <COMMAND> [OPTIONS]
```

## Commands

### `init`

Initialize the database schema.

```bash
unicity-orchestrator init --db-url memory
```

| Flag | Default | Description |
|------|---------|-------------|
| `--db-url` | `memory` | Database URL |

Creates all required tables (`service`, `tool`, `embedding`, etc.) and seeds default data.

---

### `discover-tools`

Discover tools from configured MCP services.

```bash
unicity-orchestrator discover-tools --db-url memory
```

| Flag | Default | Description |
|------|---------|-------------|
| `--db-url` | env or `memory` | Database URL |

Runs the full warmup pipeline: starts services, lists tools, generates embeddings, builds the knowledge graph.

---

### `query`

Query for tools using natural language.

```bash
unicity-orchestrator query "read a file from the filesystem" --limit 5 --db-url memory
```

| Flag | Default | Description |
|------|---------|-------------|
| `<query>` | — | Natural-language query (positional argument) |
| `--limit` | `10` | Maximum number of results |
| `--db-url` | env or `memory` | Database URL |

---

### `server`

Start the REST API server.

```bash
unicity-orchestrator server --port 8080 --admin-port 8081 --db-url memory
```

| Flag | Default | Description |
|------|---------|-------------|
| `--port` | `8080` | Public API port |
| `--admin-port` | `8081` | Admin API port |
| `--db-url` | env or `memory` | Database URL |

---

### `mcp-http`

Start the MCP HTTP server.

```bash
unicity-orchestrator mcp-http --bind 0.0.0.0:3942 --db-url memory
```

| Flag | Default | Description |
|------|---------|-------------|
| `--bind` | `0.0.0.0:3942` | Bind address |
| `--db-url` | env or `memory` | Database URL |
| `--allow-anonymous` | `false` | Allow unauthenticated access |
| `--api-key` | — | Static API key (also: `ORCHESTRATOR_API_KEY` env) |
| `--enable-db-api-keys` | `false` | Enable database-backed API keys |
| `--jwks-url` | — | JWKS endpoint URL |
| `--jwt-issuer` | — | Expected JWT issuer |
| `--jwt-audience` | — | Expected JWT audience |

---

### `mcp-stdio`

Start the MCP stdio server.

```bash
unicity-orchestrator mcp-stdio --db-url memory
```

| Flag | Default | Description |
|------|---------|-------------|
| `--db-url` | env or `memory` | Database URL |

---

### `create-api-key`

Generate a new database-backed API key.

```bash
unicity-orchestrator create-api-key --name "My App" --db-url ws://localhost:8000/rpc
```

| Flag | Default | Description |
|------|---------|-------------|
| `--name` | — | Human-readable key name |
| `--db-url` | env or `memory` | Database URL |

Prints the generated key to stdout. The key is shown only once.

---

### `list-api-keys`

List all API keys in the database.

```bash
unicity-orchestrator list-api-keys --db-url ws://localhost:8000/rpc
```

| Flag | Default | Description |
|------|---------|-------------|
| `--db-url` | env or `memory` | Database URL |

---

### `revoke-api-key`

Revoke an API key by its prefix.

```bash
unicity-orchestrator revoke-api-key uo_abc12345 --db-url ws://localhost:8000/rpc
```

| Flag | Default | Description |
|------|---------|-------------|
| `<prefix>` | — | Key prefix to revoke (positional argument) |
| `--db-url` | env or `memory` | Database URL |

## Environment Variables

All subcommands respect these database environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `SURREALDB_URL` | `memory` | Database URL |
| `SURREALDB_NAMESPACE` | `unicity` | Database namespace |
| `SURREALDB_DATABASE` | `orchestrator` | Database name |
| `SURREALDB_USERNAME` | `root` | Database username |
| `SURREALDB_PASSWORD` | `root` | Database password |
| `ORCHESTRATOR_API_KEY` | — | Static API key for `mcp-http` |
| `MCP_CONFIG` | — | Path to custom `mcp.json` |
| `RUST_LOG` | — | Log level filter |
