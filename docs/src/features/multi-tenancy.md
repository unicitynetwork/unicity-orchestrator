# Multi-Tenancy

Unicity Orchestrator supports per-user customization of tool access, service trust, and preferences. Each authenticated user gets their own view of the tool ecosystem.

## User Context

When a user authenticates (via JWT, API key, or anonymous access), a `UserContext` is created that tracks:

- User ID (internal)
- External ID (from identity provider)
- Identity provider name
- Email and display name
- Client IP address and user agent

## Per-User Filtering

The `UserToolFilter` applies per-user service preferences during tool queries:

### Blocked Services

Users can block services they don't want tools from. Blocked services are excluded from:
- Tool query results
- Tool selections
- Plan steps

### Trusted Services

Users can mark services as trusted. Trusted services receive a **confidence boost** in query results, making their tools rank higher.

### Filter Pipeline

```text
Semantic search results
       │
       ▼
Remove tools from blocked services
       │
       ▼
Boost confidence for trusted services
       │
       ▼
Final ranked results
```

## User Preferences

Each user has a preferences record in the `user_preferences` table:

| Preference | Default | Description |
|------------|---------|-------------|
| `default_approval_mode` | `prompt` | How to handle tool approval (`prompt`, etc.) |
| `trusted_services` | `[]` | List of trusted service names |
| `blocked_services` | `[]` | List of blocked service names |
| `elicitation_timeout_seconds` | `300` | Timeout for approval prompts |
| `remember_decisions` | `true` | Remember approval decisions |
| `notify_on_tool_execution` | `false` | Notify on tool execution |
| `notify_on_permission_grant` | `true` | Notify when permissions are granted |

## User Management

The `UserStore` provides user lifecycle operations:

- **Get or create** — Users are created on first authentication
- **Deactivate / reactivate** — Disable user accounts
- **Preferences** — Read and update user preferences
- **Service trust** — Check, block, and unblock services
- **Audit log** — Record user actions

## Audit Logging

All significant user actions are logged to the `audit_log` table:

| Action | Description |
|--------|-------------|
| `Login` | User authenticated |
| `ToolExecuted` | A tool was executed |
| `PermissionGranted` | Permission was granted for a tool |
| `PermissionDenied` | Permission was denied |
| `PermissionRevoked` | A permission was revoked |
| `ElicitationRequested` | An elicitation was sent to the user |
| `ElicitationCompleted` | An elicitation was completed |
| `OAuthStarted` | OAuth flow initiated |
| `OAuthCompleted` | OAuth flow completed |
| `PreferencesUpdated` | User preferences were changed |

Each audit entry includes the user ID, action type, resource details, IP address, and user agent.
