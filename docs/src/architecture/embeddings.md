# Embeddings

Vector embeddings power the semantic search that lets users find tools by meaning rather than exact keyword matches.

## Embedding Model

The orchestrator uses [embed_anything](https://github.com/StarlightSearch/EmbedAnything) for local embedding generation:

| Setting | Value |
|---------|-------|
| Model | `Qwen/QWen3-Embedding-0.6B` |
| Architecture | Qwen3 |
| Dimensions | 1024 |
| Batch size | 32 |

Embeddings are generated locally — no external API calls are needed.

## What Gets Embedded

Each tool is embedded as a combined text document that includes:

- Tool name
- Tool description
- Input schema (as text)
- Input type URI (`input_ty`)
- Output type URI (`output_ty`)

This gives the embedding model rich semantic context about what each tool does and what data types it works with.

## Search Pipeline

When a query arrives:

1. **Generate query embedding** — The natural-language query is embedded using the same model
2. **Vector similarity search** — SurrealDB's `vector::similarity::cosine` function finds the closest tool embeddings
3. **Threshold filtering** — Results below similarity 0.25 are discarded
4. **Top-K selection** — The top 32 results are returned for further reasoning

```text
"read a file" → [0.12, -0.45, 0.78, ...] → cosine similarity → ranked tools
```

## Caching and Deduplication

The embedding manager uses two layers of caching:

- **In-memory cache** — `HashMap<String, Vec<f32>>` keyed by content text
- **Content hashing** — SHA-256 hash of the tool content, stored in the database alongside the embedding. Tools are only re-embedded if their content hash changes.

This means that restarting the orchestrator and re-discovering tools will not regenerate embeddings for unchanged tools.

## Storage

Embeddings are stored in the SurrealDB `embedding` table:

| Field | Description |
|-------|-------------|
| `tool_id` | Reference to the tool |
| `vector` | The embedding vector (1024 floats) |
| `model` | Model name used for generation |
| `content_hash` | SHA-256 of the embedded content |
| `dimensions` | Vector dimensionality |
