# API Reference

## REST API

### Public Endpoints (default port 8080)

#### `GET /health`

Health check.

**Response:**
```json
{
  "status": "healthy",
  "timestamp": "2025-01-01T00:00:00Z"
}
```

---

#### `POST /query`

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

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | string | Yes | Natural-language query |
| `context` | object | No | Additional context |

**Response:** Array of tool selections.

---

#### `GET /services`

List registered MCP services.

---

### Admin Endpoints (default port 8081)

#### `POST /discover`

Re-discover tools from configured MCP services.

#### `POST /sync`

Sync with external registries. (Under development.)

---

## MCP Protocol

The MCP HTTP server at `/mcp` implements the full Model Context Protocol (version `2025-06-18`).

### Tools

#### `unicity.select_tool`

Find the best matching tool for a natural-language query.

**Input:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | string | Yes | What you want to do |
| `context` | object | No | Additional context |

**Output:**

```json
[
  {
    "toolId": "tool:abc123",
    "toolName": "filesystem.read_file",
    "serviceId": "service:xyz",
    "confidence": 0.92,
    "reasoning": "High semantic match for file reading operations",
    "dependencies": [],
    "estimatedCost": null,
    "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } } },
    "outputSchema": null
  }
]
```

---

#### `unicity.plan_tools`

Generate a multi-step execution plan.

**Input:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | string | Yes | Description of the goal |
| `context` | object | No | Additional context |

**Output:**

```json
{
  "steps": [
    {
      "description": "Fetch open issues from the repository",
      "serviceId": "service:xyz",
      "toolName": "github.list_issues",
      "inputs": []
    },
    {
      "description": "Group issues by severity",
      "serviceId": "service:abc",
      "toolName": "json.structure_data",
      "inputs": ["output_from_step_1"]
    }
  ],
  "confidence": 0.85,
  "reasoning": "Two-step plan to list and group issues"
}
```

---

#### `unicity.execute_tool`

Execute a tool by its ID.

**Input:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `toolId` | string | Yes | Tool ID from a previous selection |
| `args` | object | Yes | Arguments to pass to the tool |

**Output:** The tool's execution result (varies by tool).

If the tool belongs to a blocked service, an elicitation flow is triggered to ask the user for approval.

---

#### `unicity.debug.list_tools`

List all discovered tools with optional filtering.

**Input:**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `service_filter` | string | No | — | Filter by service name |
| `include_blocked` | boolean | No | `true` | Include blocked tools |
| `limit` | integer | No | `100` | Max results |
| `offset` | integer | No | `0` | Pagination offset |

---

### Prompts

Standard MCP prompt operations are supported:

- `prompts/list` — List all aggregated prompts
- `prompts/get` — Get a specific prompt by name

See [Prompt Forwarding](features/prompt-forwarding.md) for resolution rules.

---

### Resources

Standard MCP resource operations are supported:

- `resources/list` — List all aggregated resources
- `resources/read` — Read a resource by URI
- `resources/templates/list` — List resource templates
- `resources/subscribe` — Subscribe to resource changes
- `resources/unsubscribe` — Unsubscribe from resource changes

See [Resource Forwarding](features/resource-forwarding.md) for details.
