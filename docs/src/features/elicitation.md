# Elicitation

Elicitation provides interactive approval flows for tool execution. When a tool requires user consent, the orchestrator can present approval dialogs, forms, or OAuth flows to the connected MCP client.

## Overview

The `ElicitationCoordinator` manages all elicitation flows:

- **Form elicitation** — Standard MCP elicitation with structured schemas
- **URL elicitation** — OAuth/redirect-based authorization flows
- **Tool approval** — Permission checks before tool execution

## Fallback Policy

When a client does not support elicitation, the fallback policy determines behavior:

| Policy | Description |
|--------|-------------|
| `Deny` (default) | Reject operations that require elicitation — secure by default |
| `Allow` | Allow operations without elicitation — backwards compatible |

## Tool Approval

When `unicity.execute_tool` is called for a tool on a blocked service, the approval manager:

1. **Checks permission** — Looks up existing permissions in the `permission` table
2. **Creates elicitation** — If permission is required, builds an approval form with three options:
   - `allow_once` — Permit this single execution
   - `always_allow` — Unblock the service permanently
   - `deny` — Deny execution
3. **Processes response** — Grants or denies permission based on the user's choice
4. **Consumes one-time permissions** — `allow_once` permissions are consumed after use

### Permission Status

| Status | Description |
|--------|-------------|
| `Granted` | Tool is approved for execution |
| `Denied` | Tool execution is denied |
| `Required` | User approval is needed |
| `Expired` | Previous permission has expired |

## Form Elicitation

The form handler validates elicitation responses against schemas. Supported property types:

| Type | Validations |
|------|-------------|
| String | min/max length, format (email, uri, date, date-time) |
| Number | min/max value |
| Integer | min/max value |
| Boolean | type check |
| Enum | value must be in allowed list |

Required fields and constraint violations produce structured error messages.

## URL Elicitation (OAuth Extension)

For services that require OAuth authentication, the orchestrator supports a URL-based elicitation mode:

1. **Create OAuth state** — Binds the elicitation to the user's identity
2. **Build connect URL** — Generates a redirect URL: `{base}/oauth/connect/{provider}?elicitation_id={id}`
3. **Validate state** — Verifies the OAuth callback matches the original request
4. **Complete flow** — Consumes the OAuth state and grants access

The URL elicitation uses a custom MCP error code (`-32042`) to signal that the client should redirect the user to an authorization URL.

OAuth state is stored **in-memory** (not in the database) for security, as it contains sensitive session data.

## Provenance

All elicitation messages are wrapped with service attribution. For example, a message from the GitHub service will be prefixed with `[github]`, so users can identify which service is requesting approval.

## Error Handling

| Error | MCP Code | Description |
|-------|----------|-------------|
| Declined | -32001 | User declined the elicitation |
| Canceled | -32001 | Elicitation was canceled |
| Expired | -32001 | Elicitation timed out |
| Not found | -32002 | Referenced elicitation not found |
| URL required | -32042 | Client must redirect to an auth URL |
