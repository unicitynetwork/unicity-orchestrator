# Resource Forwarding

The orchestrator aggregates resources from all configured MCP services and presents them through a unified interface. Resources are identified by URI and forwarded to the appropriate child service on access.

## How It Works

### Discovery

During warmup, the orchestrator calls `resources/list` on each running child service. Discovered resources are registered in the `ResourceRegistry` along with any resource templates.

### Conflict Resolution

When multiple services expose resources with the same URI, **the first service wins**. Unlike prompts, resources use URIs as unique identifiers, so conflicts are less common.

Resources are also accessible via namespaced names in the format `service:resource_name`, and lookups are case-insensitive.

## MCP Operations

### List Resources

The MCP `resources/list` method returns all registered resources. Supports:
- **Pagination** — Cursor-based pagination
- **Service filtering** — Filter by source service
- **List-changed notifications** — Clients are notified when the resource list changes

### Read Resource

The MCP `resources/read` method resolves the URI and forwards the read request to the source service.

### Resource Templates

The MCP `resources/templates/list` method returns parameterized resource templates that clients can use to construct resource URIs. For example:

```
git://{repo}/file/{path}
```

### Subscriptions

Clients can subscribe to specific resources via `resources/subscribe` and unsubscribe with `resources/unsubscribe`. The server tracks active subscriptions per session.

## URI Security

Resource URIs are validated before processing:

| Check | Description |
|-------|-------------|
| Protocol required | Must contain `://` |
| No path traversal | Rejects URIs containing `../` |
| No null bytes | Rejects URIs containing null characters |
| Length limit | Maximum 4096 characters |

These checks prevent injection and traversal attacks through crafted URIs.
