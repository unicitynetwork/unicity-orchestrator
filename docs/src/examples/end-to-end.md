# End-to-End Walkthrough

This example demonstrates the full lifecycle of task orchestration — from a natural-language request to tool discovery, semantic matching, planning, and execution.

## Scenario

An LLM needs to summarize open GitHub issues from a repository and group them by severity. The LLM does **not** need to know which tools exist — the orchestrator handles everything.

## Step 1: User / LLM Request

The LLM sends a query:

```json
{
  "query": "Summarize open GitHub issues from this repository and group them by severity.",
  "context": {
    "repo_url": "https://github.com/example/project"
  }
}
```

## Step 2: Semantic Retrieval

The orchestrator generates an embedding for the query and performs a vector search. Candidate tools are ranked by cosine similarity:

| Tool | Similarity |
|------|-----------|
| `github.list_issues` | 0.91 |
| `github.get_repo` | 0.78 |
| `json.structure_data` | 0.72 |
| `text.summarize` | 0.68 |

Irrelevant tools are discarded (below the 0.25 threshold).

## Step 3: Type-Aware Graph Filtering

The orchestrator inspects each candidate tool's `input_ty` and `output_ty`:

- `github.list_issues` → produces `Issue[]`
- `json.structure_data` → accepts `Issue[]`, produces structured JSON
- `text.summarize` → accepts text or JSON

Incompatible paths are eliminated. The valid data flow chain emerges:

```text
github.list_issues → json.structure_data → text.summarize
```

## Step 4: Symbolic Reasoning

Symbolic rules reinforce the chain:

- "Listing issues" is commonly followed by "filtering" or "processing" tools
- Rules recommend grouping, summarizing, or transforming steps

The planner generates the final sequence:

1. `github.list_issues`
2. `json.structure_data`
3. `text.summarize`

## Step 5: Execution

The orchestrator executes each step through its rmcp client:

1. Calls GitHub MCP service → retrieves raw issues
2. Calls JSON transformer → groups by severity
3. Calls summarizer → produces a clean summary

Arguments are derived from the initial context and prior tool outputs.

## Step 6: Final Response

```json
{
  "summary": "12 open issues found. Critical: 2, High: 4, Medium: 3, Low: 3.",
  "grouped_issues": {
    "critical": ["..."],
    "high": ["..."],
    "medium": ["..."],
    "low": ["..."]
  }
}
```

The LLM can present or refine the result, but the entire orchestration flow — discovery, reasoning, planning, and execution — was automated.

## What the LLM Did vs. What the Orchestrator Did

| Actor | Actions |
|-------|---------|
| **LLM** | Described the task in natural language, presented the final result |
| **Orchestrator** | Embedded the query, searched tools, checked type compatibility, applied symbolic rules, planned the chain, executed each step |

The LLM never named a tool, constructed a schema, or managed execution order. This is the core design principle: **minimal LLM involvement, maximum automated reasoning**.
