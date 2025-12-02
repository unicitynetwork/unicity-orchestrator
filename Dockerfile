# === Builder stage ===
FROM rust:1.82 as builder

WORKDIR /app

# Copy manifest(s) first for better caching
COPY Cargo.toml Cargo.lock ./
# If you're in a workspace, copy the workspace-level Cargo.toml and relevant members.

# Build deps with a dummy main to leverage cache
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src

# Now copy the real source
COPY src ./src

# Build the actual binary
RUN cargo build --release

# === Runtime stage ===
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Bring entire build context root so we can pick up an mcp.json if present
COPY . /tmp/root

# If the user has an mcp.json at project root, copy it into the image.
# If not present, this does nothing — the app will generate a default one at runtime.
RUN if [ -f /tmp/root/mcp.json ]; then \
      cp /tmp/root/mcp.json /app/mcp.json; \
      echo "Using provided mcp.json from project root."; \
    else \
      echo "No mcp.json provided in project root; runtime will auto-generate one."; \
    fi

# Copy the built binary
COPY --from=builder /app/target/release/unicity-orchestrator /usr/local/bin/unicity-orchestrator

# Defaults – can be overridden in docker-compose / kubernetes
ENV RUST_LOG=info,unicity_orchestrator=info

# SurrealDB connection config
ENV SURREALDB_URL=memory
ENV SURREALDB_NAMESPACE=unicity
ENV SURREALDB_DATABASE=orchestrator
ENV SURREALDB_USERNAME=root
ENV SURREALDB_PASSWORD=root

# MCP HTTP bind address
ENV MCP_BIND=0.0.0.0:3942

# Default: run MCP HTTP server and point it at SURREALDB_URL
CMD ["sh", "-c", "unicity-orchestrator mcp-http --bind ${MCP_BIND} --db-url ${SURREALDB_URL}"]
