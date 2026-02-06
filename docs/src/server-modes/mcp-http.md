# MCP HTTP Server

The MCP HTTP server is the primary interface for LLM-based workflows and tool execution. It implements the full Model Context Protocol over HTTP.

## Starting the Server

```bash
unicity-orchestrator mcp-http --bind 0.0.0.0:3942 --db-url memory
```

The MCP endpoint is available at:

```
http://localhost:3942/mcp
```

## CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--bind` | `0.0.0.0:3942` | Address and port to bind |
| `--db-url` | env or `memory` | Database connection URL |
| `--allow-anonymous` | `false` | Allow unauthenticated access |
| `--api-key` | — | Require a static API key |
| `--enable-db-api-keys` | `false` | Enable database-backed API keys |
| `--jwks-url` | — | JWKS endpoint for JWT validation |
| `--jwt-issuer` | — | Expected JWT issuer claim |
| `--jwt-audience` | — | Expected JWT audience claim |

## Protocol Details

- **Protocol version**: `2025-06-18`
- **Transport**: Streamable HTTP with local session management

### Server Capabilities

The server advertises the following capabilities:

| Capability | Description |
|------------|-------------|
| Tools | List and call tools |
| Prompts | List and get prompts (with list-changed notifications) |
| Resources | List, read, and subscribe to resources (with list-changed notifications) |

### Exposed Tools

| Tool | Description |
|------|-------------|
| `unicity.select_tool` | Semantic search for the best matching tool |
| `unicity.plan_tools` | Generate a multi-step execution plan |
| `unicity.execute_tool` | Execute a tool by ID |
| `unicity.debug.list_tools` | List all discovered tools with filters |

### Tool Details

#### `unicity.select_tool`

Input:
- `query` (string, required) — Natural-language description of what you need
- `context` (object, optional) — Additional context for the query

Returns an array of tool selections with `toolId`, `toolName`, `serviceId`, `confidence`, `reasoning`, `dependencies`, `estimatedCost`, `inputSchema`, and `outputSchema`.

#### `unicity.plan_tools`

Input:
- `query` (string, required) — Description of the goal
- `context` (object, optional) — Additional context

Returns a plan with `steps`, `confidence`, and `reasoning`.

#### `unicity.execute_tool`

Input:
- `toolId` (string, required) — The tool ID from a previous selection
- `args` (object, required) — Arguments to pass to the tool

If the tool belongs to a blocked service, the orchestrator uses elicitation to ask the user for approval.

#### `unicity.debug.list_tools`

Input:
- `service_filter` (string, optional) — Filter by service name
- `include_blocked` (boolean, optional) — Include blocked tools
- `limit` (integer, optional, default: 100) — Maximum results
- `offset` (integer, optional, default: 0) — Pagination offset

## Authentication

Without any auth flags, the MCP HTTP server runs in local mode (anonymous access). See [Authentication Overview](../authentication/overview.md) for configuring JWT, API keys, or anonymous access.

## Planning Constraints

The `plan_tools` endpoint respects constraints:

| Constraint | Default | Description |
|------------|---------|-------------|
| Max steps | 8 | Maximum number of plan steps |
| Timeout | 30s | Planning timeout |
| Allowed tools | all | Restrict to specific tools |
| Forbidden tools | none | Exclude specific tools |
| Max cost | unlimited | Cost budget |
