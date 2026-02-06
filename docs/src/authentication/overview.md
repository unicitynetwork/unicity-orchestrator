# Authentication Overview

The MCP HTTP server supports multiple authentication methods. Without any auth flags, it runs in **local mode** (anonymous access for all clients).

## Authentication Methods

| Method | Flag | Description |
|--------|------|-------------|
| [JWT](jwt.md) | `--jwks-url` | Token-based auth via JWKS/OAuth providers |
| [API Keys](api-keys.md) | `--api-key` or `--enable-db-api-keys` | Static or database-backed API keys |
| [Anonymous](anonymous.md) | `--allow-anonymous` | No authentication required |

## Auth Resolution Order

When a request arrives, the `AuthExtractor` checks credentials in this order:

1. **Bearer token** — If JWT is enabled and an `Authorization: Bearer <token>` header is present, validate the JWT
2. **API key** — If an `X-API-Key` header is present, validate against the static key or database
3. **Anonymous** — If anonymous access is allowed, create an anonymous user context
4. **Reject** — If none of the above succeed, return `Unauthenticated`

## Configuration Examples

### Local Development (No Auth)

```bash
unicity-orchestrator mcp-http --bind 0.0.0.0:3942 --db-url memory
```

### Static API Key

```bash
unicity-orchestrator mcp-http --api-key my-secret-key --db-url memory
```

### JWT with Anonymous Fallback

```bash
unicity-orchestrator mcp-http \
  --jwks-url https://auth.example.com/.well-known/jwks.json \
  --jwt-issuer https://auth.example.com \
  --jwt-audience my-app \
  --allow-anonymous \
  --db-url memory
```

### Database API Keys

```bash
unicity-orchestrator mcp-http --enable-db-api-keys --db-url ws://localhost:8000/rpc
```

## User Context

Successful authentication produces a `UserContext` that flows through the entire request lifecycle:

- Tool queries are filtered by user preferences
- Tool executions are audited
- Elicitations are bound to the user
- Permissions are stored per-user

Anonymous users receive a `UserContext` with `is_anonymous: true` and limited tracking.

## Error Responses

| Error | Description |
|-------|-------------|
| `Unauthenticated` | No valid credentials provided |
| `InvalidApiKey` | API key not recognized |
| `ApiKeyExpired` | API key has expired |
| `ApiKeyRevoked` | API key has been revoked |
| `InvalidToken(reason)` | JWT validation failed |
| `UserDeactivated` | User account is deactivated |
