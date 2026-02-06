# Server Modes

Unicity Orchestrator provides multiple server interfaces, each designed for different use cases.

## Summary

| Mode | Default Address | Protocol | Use Case |
|------|----------------|----------|----------|
| [MCP HTTP](mcp-http.md) | `0.0.0.0:3942` | MCP over HTTP | Primary interface for LLMs and remote MCP clients |
| [MCP Stdio](mcp-stdio.md) | stdin/stdout | MCP over stdio | Local MCP client integration |
| [REST API](rest-api.md) | `0.0.0.0:8080` / `127.0.0.1:8081` | HTTP/JSON | Web applications and admin operations |

## Choosing a Mode

### MCP HTTP

Use this when:
- LLMs or MCP clients connect over the network
- You need authentication (JWT, API keys)
- Running in production or Docker

```bash
unicity-orchestrator mcp-http --bind 0.0.0.0:3942 --db-url memory
```

### MCP Stdio

Use this when:
- Integrating with local tools that expect stdio MCP transport
- Running as a child process of an LLM client
- Development and testing

```bash
unicity-orchestrator mcp-stdio --db-url memory
```

### REST API

Use this when:
- Building a web application that queries tools
- You need admin operations (sync, discover)
- You want a simple HTTP/JSON interface

```bash
unicity-orchestrator server --port 8080 --db-url memory
```

## Running Multiple Modes

The MCP HTTP and REST API servers are independent processes. You can run both simultaneously for different audiences:

```bash
# Terminal 1: MCP HTTP for LLMs
unicity-orchestrator mcp-http --bind 0.0.0.0:3942 --db-url ws://localhost:8000/rpc

# Terminal 2: REST API for web apps
unicity-orchestrator server --port 8080 --db-url ws://localhost:8000/rpc
```

Both will share the same SurrealDB instance when pointed at the same database URL.
