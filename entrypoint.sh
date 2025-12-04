#!/bin/sh

set -e

# Dev-friendly defaults: if no SURREALDB_URL is provided, assume in-memory SurrealDB
if [ -z "${SURREALDB_URL:-}" ]; then
  echo "INFO: SURREALDB_URL not set; defaulting to in-memory SurrealDB for development."
  SURREALDB_URL="memory"
  : "${SURREALDB_NAMESPACE:=unicity}"
  : "${SURREALDB_DATABASE:=orchestrator}"
  : "${SURREALDB_USERNAME:=root}"
  : "${SURREALDB_PASSWORD:=root}"
  : "${MCP_BIND:=0.0.0.0:3942}"
fi

required_vars="
SURREALDB_URL
SURREALDB_NAMESPACE
SURREALDB_DATABASE
SURREALDB_USERNAME
SURREALDB_PASSWORD
MCP_BIND
"

missing=""

for var in $required_vars; do
  value=$(eval echo \$$var)
  if [ -z "$value" ]; then
    missing="$missing $var"
  fi
done

if [ -n "$missing" ]; then
  echo ""
  echo "============================================================"
  echo "  ❌ Missing required environment variables"
  echo "  The following variables must be provided:"
  for v in $missing; do
    echo "   - $v"
  done
  echo ""
  echo "  Example (docker run):"
  echo "    docker run -e SURREALDB_URL=... -e SURREALDB_USERNAME=... yourimage"
  echo "============================================================"
  echo ""
  exit 1
fi

# All variables present → run orchestrator
exec unicity-orchestrator mcp-http \
  --bind "${MCP_BIND}" \
  --db-url "${SURREALDB_URL}"
