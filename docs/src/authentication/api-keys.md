# API Key Authentication

API keys provide a simple authentication mechanism using the `X-API-Key` header.

## Static API Key

The simplest approach — a single shared key configured at startup:

```bash
unicity-orchestrator mcp-http --api-key my-secret-key --db-url memory

# Or via environment variable
ORCHESTRATOR_API_KEY=my-secret-key unicity-orchestrator mcp-http --db-url memory
```

Clients include the key in requests:

```
X-API-Key: my-secret-key
```

## Database-Backed API Keys

For multi-user deployments, enable database-backed API keys:

```bash
unicity-orchestrator mcp-http --enable-db-api-keys --db-url ws://localhost:8000/rpc
```

### Creating Keys

```bash
unicity-orchestrator create-api-key --name "My App" --db-url ws://localhost:8000/rpc
```

This generates a key in the format:

```
uo_{8-char-uuid}_{32-char-uuid}
```

The key is displayed once. Only the **prefix** (`uo_{8-char}`) and a **SHA-256 hash** of the full key are stored in the database.

### Listing Keys

```bash
unicity-orchestrator list-api-keys --db-url ws://localhost:8000/rpc
```

Shows key prefixes, names, creation dates, and status.

### Revoking Keys

```bash
unicity-orchestrator revoke-api-key uo_abc12345 --db-url ws://localhost:8000/rpc
```

Revoked keys immediately stop working. The revocation is permanent.

## Key Storage

API keys are stored in the `api_key` table:

| Field | Description |
|-------|-------------|
| `key_hash` | SHA-256 hash of the full key |
| `key_prefix` | Display prefix (`uo_{8-char}`) |
| `user_id` | Associated user (optional) |
| `name` | Human-readable name |
| `is_active` | Whether the key is active |
| `expires_at` | Optional expiration time |
| `scopes` | Optional permission scopes |

## Security

- Full keys are **never stored** — only SHA-256 hashes
- Key prefixes allow identification without exposing the key
- Keys can be associated with specific users for audit tracking
- Expired and revoked keys are rejected immediately

## Combining with Other Methods

API key auth can be combined with JWT and anonymous access. The auth extractor checks methods in order:

1. Bearer token (JWT) — if enabled
2. `X-API-Key` header — if present
3. Anonymous — if allowed
