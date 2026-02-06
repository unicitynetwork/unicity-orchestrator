# Prompt Forwarding

The orchestrator aggregates prompts from all configured MCP services and presents them through a unified interface. This allows MCP clients to access prompts from multiple services without connecting to each one individually.

## How It Works

### Discovery

During warmup, the orchestrator iterates through all running child services and calls `prompts/list` on each. Discovered prompts are registered in the `PromptRegistry`.

### Conflict Resolution

When multiple services define prompts with the same name, the orchestrator handles conflicts automatically:

1. **First service wins** — The first-discovered prompt keeps the original name
2. **Aliases created** — Subsequent conflicts create namespaced aliases in the format `service:prompt_name`
3. **Case-insensitive** — Conflict detection is case-insensitive to prevent subtle collisions

For example, if both `github` and `gitlab` services define a `commit` prompt:
- `commit` → resolves to the first-discovered version
- `github:commit` → explicitly targets the GitHub version
- `gitlab:commit` → explicitly targets the GitLab version

### Resolution Order

When a client requests a prompt by name, the registry resolves it in order:

1. **Direct match** — Exact name in the registry
2. **Alias lookup** — Check the aliases table
3. **Service:prompt pattern** — Parse `service:prompt_name` format
4. **Case-insensitive fallback** — Try lowercase matching

## Listing Prompts

The MCP `prompts/list` method returns all registered prompts with pagination support. The server also supports `prompts/listChanged` notifications when the prompt list is modified.

## Getting a Prompt

The MCP `prompts/get` method resolves the prompt name and forwards the request to the source service. Arguments are validated before forwarding.

## Security

- **Name validation** — Prompt names must be alphanumeric with hyphens, underscores, and colons, maximum 256 characters
- **Argument validation** — Maximum 100 arguments per request, with safe key name enforcement
- **Provenance** — Responses are wrapped with service attribution (e.g., `[github]` prefix)
