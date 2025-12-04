# === Builder stage ===
FROM rust:1.91 as builder

WORKDIR /app

# Copy manifest(s) first for better caching
COPY Cargo.toml ./
# If you're in a workspace, copy the workspace-level Cargo.toml and relevant members.

# Build deps with a dummy main to leverage cache
RUN mkdir -p src/bin && echo "fn main() {}" > src/bin/main.rs
RUN cargo build --release
RUN rm -rf src

# Now copy the real source
COPY src ./src

# Build the actual binary
RUN cargo build --release

# === Runtime stage ===
FROM ubuntu:24.04

RUN apt-get update \
 && apt-get install -y ca-certificates curl python3 python3-venv nodejs npm \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Install uv (which provides `uvx`)
RUN curl -LsSf https://astral.sh/uv/install.sh | sh \
 && ln -s /root/.local/bin/uvx /usr/local/bin/uvx

# Caches them in the image
RUN uvx mcp-server-fetch --help || true \
 && uvx kagimcp --help || true \
 && uvx mcp-server-time --help || true

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

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]

ENV RUST_LOG=info,unicity_orchestrator=info
