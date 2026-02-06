# Anonymous Access

Anonymous access allows clients to use the orchestrator without providing any credentials.

## Configuration

Enable anonymous access with the `--allow-anonymous` flag:

```bash
unicity-orchestrator mcp-http --allow-anonymous --db-url memory
```

## How It Works

When anonymous access is enabled and no credentials are provided:

1. The auth extractor skips JWT and API key checks
2. A `UserContext` is created with `is_anonymous: true`
3. The user gets a generated anonymous identity

Anonymous users can:
- Query and select tools
- Execute tools (subject to elicitation)
- List prompts and resources

## MCP Stdio

The MCP stdio server always runs in anonymous mode — there is no authentication layer for stdio transport.

## Local Mode

When the MCP HTTP server starts **without any auth flags**, it runs in local mode, which is equivalent to anonymous access. This is the default for development:

```bash
# These are equivalent:
unicity-orchestrator mcp-http --db-url memory
unicity-orchestrator mcp-http --allow-anonymous --db-url memory
```

## Combining with Authenticated Methods

Anonymous access can serve as a fallback when combined with JWT or API keys:

```bash
unicity-orchestrator mcp-http \
  --jwks-url https://auth.example.com/.well-known/jwks.json \
  --allow-anonymous \
  --db-url memory
```

In this configuration:
- Clients with valid JWTs get full user identity and preferences
- Clients without credentials get anonymous access with default preferences

## Limitations

Anonymous users:
- Do not have persistent preferences across sessions
- Have limited audit trail information
- Cannot manage API keys or user-specific settings
