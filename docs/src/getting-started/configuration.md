# Configuration

## MCP Configuration File

The orchestrator reads child MCP services from an `mcp.json` file. The file is resolved in this order:

1. `MCP_CONFIG` environment variable (path to a custom config file)
2. `$XDG_CONFIG_HOME/mcp/mcp.json`
3. `./mcp.json` (current working directory)
4. If none found, a default empty config is created

### Format

```json
{
  "mcpServers": {
    "service-name": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
      "env": {
        "SOME_VAR": "${MY_ENV_VAR}"
      }
    }
  }
}
```

### Stdio Services

Most MCP services use stdio transport. Configure them with `command`, `args`, and `env`:

```json
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"
      }
    },
    "fetch": {
      "command": "uvx",
      "args": ["mcp-server-fetch"]
    }
  }
}
```

### HTTP Services

For remote MCP services, use the `url` field instead:

```json
{
  "mcpServers": {
    "remote-service": {
      "url": "https://mcp.example.com/mcp",
      "headers": {
        "Authorization": "Bearer ${API_TOKEN}"
      }
    }
  }
}
```

### Additional Options

| Field | Type | Description |
|-------|------|-------------|
| `command` | string | Executable to run (stdio mode) |
| `args` | string[] | Command-line arguments |
| `env` | object | Environment variables (supports `${VAR}` expansion) |
| `url` | string | Remote MCP endpoint URL (HTTP mode) |
| `headers` | object | HTTP headers for remote services |
| `disabled` | bool | Disable this service without removing it |
| `autoApprove` | string[] | Tools to auto-approve without elicitation |
| `disabledTools` | string[] | Tools to exclude from this service |

### Environment Variable Expansion

Config values support `${VAR_NAME}` syntax. Variables are expanded from the process environment at startup:

```json
{
  "env": {
    "API_KEY": "${MY_SECRET_KEY}",
    "BASE_URL": "${SERVICE_URL}"
  }
}
```

## Database Configuration

Configure the SurrealDB connection via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `SURREALDB_URL` | `memory` | Database URL (`memory` for in-memory, or `ws://host:port/rpc`) |
| `SURREALDB_NAMESPACE` | `unicity` | Database namespace |
| `SURREALDB_DATABASE` | `orchestrator` | Database name |
| `SURREALDB_USERNAME` | `root` | Database username |
| `SURREALDB_PASSWORD` | `root` | Database password |

All subcommands also accept `--db-url` to override the URL:

```bash
unicity-orchestrator mcp-http --db-url memory
unicity-orchestrator mcp-http --db-url ws://localhost:8000/rpc
```

## Authentication Configuration

Authentication is configured via CLI flags on the `mcp-http` subcommand. See [Authentication Overview](../authentication/overview.md) for details.

| Flag | Env Var | Description |
|------|---------|-------------|
| `--allow-anonymous` | — | Allow unauthenticated access |
| `--api-key` | `ORCHESTRATOR_API_KEY` | Require a static API key |
| `--enable-db-api-keys` | — | Enable database-backed API keys |
| `--jwks-url` | — | JWKS endpoint URL for JWT validation |
| `--jwt-issuer` | — | Expected JWT issuer |
| `--jwt-audience` | — | Expected JWT audience |

## Logging

The orchestrator uses the `tracing` framework. Control log levels via the `RUST_LOG` environment variable:

```bash
# Info level for orchestrator, warn for everything else
RUST_LOG=warn,unicity_orchestrator=info

# Debug level for detailed output
RUST_LOG=debug

# Trace level for maximum verbosity
RUST_LOG=trace
```
