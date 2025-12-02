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
