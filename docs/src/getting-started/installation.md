# Installation

## Prerequisites

- **Rust** 1.91+ (edition 2024)
- **Cargo** (included with Rust)

For running child MCP services, you may also need:

- **Node.js** and **npm** — for npx-based MCP servers
- **Python 3** and **uv/uvx** — for Python-based MCP servers

## Building from Source

```bash
# Clone the repository
git clone https://github.com/joshuajbouw/unicity-orchestrator.git
cd unicity-orchestrator

# Build in release mode
cargo build --release
```

The binary will be at `target/release/unicity-orchestrator`.

## Docker

If you prefer Docker, see the [Docker deployment guide](../docker.md). A multi-stage `Dockerfile` and `docker-compose.yml` are included in the repository.

```bash
# Build and run with Docker Compose
docker compose up --build
```

## Database

Unicity Orchestrator uses [SurrealDB](https://surrealdb.com/) as its backing database. Two modes are supported:

### In-Memory Mode (Development)

No external database needed. Pass `--db-url memory` to any subcommand:

```bash
unicity-orchestrator init --db-url memory
```

### External SurrealDB (Production)

Install and start SurrealDB, then configure via environment variables:

```bash
export SURREALDB_URL=ws://localhost:8000/rpc
export SURREALDB_NAMESPACE=unicity
export SURREALDB_DATABASE=orchestrator
export SURREALDB_USERNAME=root
export SURREALDB_PASSWORD=root
```

## Verifying the Installation

```bash
# Initialize the database (in-memory)
unicity-orchestrator init --db-url memory

# Check that tools can be discovered
unicity-orchestrator discover-tools --db-url memory
```

If no `mcp.json` is present, the orchestrator will create a minimal default config and start cleanly.
