# Docker Deployment

Unicity Orchestrator includes a multi-stage `Dockerfile` and `docker-compose.yml` for containerized deployment.

## Quick Start with Docker Compose

```bash
docker compose up --build
```

The MCP server will be available at `http://localhost:3942/mcp`.

By default, Docker Compose runs with an **in-memory database** — no external SurrealDB instance is needed.

## Docker Compose Configuration

```yaml
services:
  orchestrator:
    build: .
    ports:
      - "3942:3942"
    environment:
      - SURREALDB_NAMESPACE=unicity
      - SURREALDB_DATABASE=orchestrator
      - SURREALDB_USERNAME=${SURREALDB_USERNAME:-root}
      - SURREALDB_PASSWORD=${SURREALDB_PASSWORD:-root}
      - MCP_BIND=0.0.0.0:3942
      - RUST_LOG=info,unicity_orchestrator=info
```

### Using External SurrealDB

To connect to a real SurrealDB deployment:

```bash
SURREALDB_URL=ws://localhost:8000/rpc docker compose up
```

## Building Manually

```bash
docker build -t unicity-orchestrator .
```

### Running

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

## Dockerfile Details

The Dockerfile uses a multi-stage build:

### Builder Stage
- Base image: `rust:1.91`
- Compiles the project in release mode

### Runtime Stage
- Base image: `ubuntu:24.04`
- Installs runtime dependencies:
  - `ca-certificates`, `curl`
  - `python3`, `python3-venv`
  - `nodejs`, `npm`
  - `uv` / `uvx` (Python package manager)
- Pre-caches common MCP servers:
  - `mcp-server-fetch`
  - `kagimcp`
  - `mcp-server-time`

## Entrypoint

The container uses `entrypoint.sh` which:

1. Defaults to **in-memory SurrealDB** if `SURREALDB_URL` is not set
2. Validates required database variables when `SURREALDB_URL` is set
3. Starts the MCP HTTP server: `unicity-orchestrator mcp-http --bind "${MCP_BIND}" --db-url "${SURREALDB_URL}"`

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SURREALDB_URL` | (in-memory) | Database URL |
| `SURREALDB_NAMESPACE` | `unicity` | Database namespace |
| `SURREALDB_DATABASE` | `orchestrator` | Database name |
| `SURREALDB_USERNAME` | `root` | Database username |
| `SURREALDB_PASSWORD` | `root` | Database password |
| `MCP_BIND` | `0.0.0.0:3942` | Server bind address |
| `RUST_LOG` | `info` | Log level |

## MCP Configuration in Docker

If an `mcp.json` file exists in the project root during Docker build, it will be included in the image automatically. Otherwise, the orchestrator creates a minimal default at startup:

```json
{ "mcpServers": {} }
```

You can mount a custom `mcp.json` at runtime:

```bash
docker run --rm \
  -v $(pwd)/mcp.json:/app/mcp.json \
  -p 3942:3942 \
  unicity-orchestrator
```
