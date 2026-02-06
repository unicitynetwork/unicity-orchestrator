# Unicity Orchestrator

A knowledge graph-based orchestrator for [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) services with advanced tool discovery and symbolic reasoning capabilities.

## What is Unicity Orchestrator?

Unicity Orchestrator is a system that manages and discovers MCP tools through a combination of:

- **Knowledge Graph** — Typed relationships between tools, services, and data types
- **Vector Embeddings** — Semantic similarity search for tool discovery
- **Symbolic Reasoning** — Rule-based inference for intelligent tool selection
- **Registry Integration** — Support for multiple MCP manifest registries
- **Async Execution** — Parallel tool execution with dependency management

It acts as a meta-MCP server: it connects to multiple child MCP services, indexes their tools, and exposes a unified interface that LLMs and other MCP clients can query.

## LLM Interaction Model

Unicity Orchestrator is designed to operate **with minimal reliance on large language models** during planning and execution. The LLM is only involved at two points:

1. **Initial Intent Description** — The LLM (or user) provides a natural-language description of the task and optional high-level steps. The orchestrator does *not* require the LLM to name tools or manipulate schemas.
2. **Fallback Assistance** — The orchestrator asks the LLM for clarification or reformulation only if no semantically relevant tools are found, no valid type-safe tool chain exists, or a runtime error cannot be auto-resolved.

All core decision-making is handled internally by:

- **Semantic Retrieval** via vector embeddings
- **Type Normalization** from JSON Schemas
- **Knowledge-Graph Traversal** for compatibility and chaining
- **Symbolic Reasoning** for rule-based inference

This design ensures fast, predictable, and cost-efficient orchestration while keeping LLM interactions intentional and minimal.

## Core Capabilities

| Feature | Description |
|---------|-------------|
| Multi-Registry Support | GitHub, npm, and custom MCP registries |
| Tool Discovery | Automatic discovery and indexing of MCP services |
| Prompt Forwarding | Aggregate prompts from all MCP services with conflict resolution |
| Resource Forwarding | Aggregate resources with automatic discovery |
| Semantic Search | Find tools by meaning, not just keywords |
| Type-Safe Graph | Enforced compatibility between tool inputs/outputs |
| Symbolic Rules | Custom reasoning rules for tool selection |
| Async Planning | Plan and execute complex tool workflows |
| Multi-Tenancy | Per-user service trust, blocking, and preferences |
| Authentication | JWT, API keys, and anonymous access modes |
| Elicitation | Interactive approval flows for tool execution |

## How It Works

```text
                    ┌─────────────┐
                    │  LLM / User │
                    └──────┬──────┘
                           │ natural-language query
                           ▼
              ┌────────────────────────┐
              │   Unicity Orchestrator │
              │                        │
              │  1. Embed query        │
              │  2. Semantic search    │
              │  3. Graph filtering    │
              │  4. Symbolic reasoning │
              │  5. Plan & execute     │
              └───┬────────┬────────┬──┘
                  │        │        │
                  ▼        ▼        ▼
            ┌─────────┐ ┌──────┐ ┌──────────┐
            │ GitHub  │ │ File │ │ Custom   │
            │ MCP     │ │ MCP  │ │ MCP      │
            └─────────┘ └──────┘ └──────────┘
```

## Next Steps

- [Installation](getting-started/installation.md) — Get the orchestrator running
- [Quick Start](getting-started/quick-start.md) — Your first query in under 5 minutes
- [Architecture Overview](architecture/overview.md) — Understand the internals
