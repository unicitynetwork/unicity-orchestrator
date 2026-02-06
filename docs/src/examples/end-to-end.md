# End-to-End Walkthrough

This example demonstrates the current lifecycle of tool orchestration — from a natural-language request to tool discovery, semantic matching, symbolic reasoning, and execution.

## Scenario

An LLM needs to find tools that can list GitHub issues from a repository. The LLM does **not** need to know which tools exist — the orchestrator discovers and ranks them.

## Step 1: User / LLM Request

The LLM sends a query via the REST API or MCP protocol:

```json
{
  "query": "List open GitHub issues from this repository and group them by severity.",
  "context": {
    "repo_url": "https://github.com/example/project"
  }
}
```

The `query` field is required. The `context` field is optional and provides additional information that the orchestrator can use during tool selection.

## Step 2: Semantic Retrieval

The orchestrator generates an embedding for the query using `embed_anything` and performs a cosine similarity search against all indexed tool embeddings in SurrealDB. Up to 32 candidate tools are retrieved, filtered by a **0.25 similarity threshold**:

| Tool | Similarity |
|------|-----------|
| `github.list_issues` | 0.91 |
| `github.get_repo` | 0.78 |
| `json.structure_data` | 0.72 |
| `text.summarize` | 0.68 |

Tools below the 0.25 threshold are discarded.

## Step 3: Symbolic Reasoning

The symbolic reasoning engine loads the candidate tools and query context into working memory as facts (e.g., `tool_exists`, `tool_input_type`, `tool_output_type`). It then applies forward and backward chaining over loaded rules to adjust confidence scores and rank the selections.

For example, a rule might boost a tool's score if its description matches the query intent, or if its type signature is compatible with other selected tools.

The reasoner outputs a ranked list of `ToolSelection` results, each with a tool name, service, confidence score, and a reasoning trace explaining why it was selected.

## Step 4: Tool Selection Response

The orchestrator returns ranked tool selections to the caller:

```json
[
  {
    "tool_name": "github.list_issues",
    "service_name": "github-mcp",
    "confidence": 0.91,
    "reasoning": "High semantic similarity to query; tool lists GitHub issues."
  },
  {
    "tool_name": "json.structure_data",
    "service_name": "json-tools-mcp",
    "confidence": 0.72,
    "reasoning": "Can structure and group JSON data."
  }
]
```

## Step 5: Execution

The LLM (or calling client) selects a tool from the results and asks the orchestrator to execute it. The orchestrator routes the call to the correct child MCP service via its rmcp client:

```json
{
  "tool_name": "github.list_issues",
  "service_name": "github-mcp",
  "arguments": {
    "repo": "example/project",
    "state": "open"
  }
}
```

Execution is currently **single-tool per call**. The LLM is responsible for sequencing multiple tool calls — inspecting each result, deciding the next step, and passing arguments between tools.

> **Note:** Automatic multi-step chain execution (where the orchestrator executes a planned sequence of tools and pipes outputs between them) is a planned feature being developed alongside rmcp 0.14+. Today, the `plan_tools` endpoint can suggest a multi-step plan, but execution of each step is driven by the caller.

## Step 6: Final Response

The executed tool returns its result through the orchestrator:

```json
{
  "content": [
    {
      "type": "text",
      "text": "[{\"number\": 42, \"title\": \"Critical bug in auth\", \"labels\": [\"critical\"]}, ...]"
    }
  ]
}
```

The LLM can then call additional tools (e.g., a summarizer or formatter) using the same select-then-execute flow, or process the result directly.

## What the LLM Does vs. What the Orchestrator Does

| Actor | Actions |
|-------|---------|
| **LLM** | Describes the task in natural language, chooses which selected tools to execute, sequences multi-step workflows, presents results |
| **Orchestrator** | Embeds the query, searches tools by similarity, applies symbolic reasoning, ranks candidates, executes individual tool calls via child MCP services |

The LLM never needs to know which MCP services are available or how to reach them. The orchestrator handles discovery, ranking, and routing. The LLM focuses on **what** to do; the orchestrator handles **which tool** and **where**.
