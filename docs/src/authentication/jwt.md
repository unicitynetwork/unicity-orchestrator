# JWT Authentication

JWT (JSON Web Token) authentication allows the orchestrator to validate tokens issued by external identity providers using JWKS (JSON Web Key Set) endpoints.

## Configuration

Enable JWT by providing a JWKS URL:

```bash
unicity-orchestrator mcp-http \
  --jwks-url https://auth.example.com/.well-known/jwks.json \
  --jwt-issuer https://auth.example.com \
  --jwt-audience my-app \
  --db-url memory
```

| Flag | Required | Description |
|------|----------|-------------|
| `--jwks-url` | Yes | URL to the JWKS endpoint |
| `--jwt-issuer` | No | Expected `iss` claim |
| `--jwt-audience` | No | Expected `aud` claim |

## How It Works

1. Client sends `Authorization: Bearer <token>` header
2. Orchestrator decodes the JWT header to find the `kid` (key ID)
3. The matching public key is fetched from the JWKS endpoint
4. Token signature is verified, and claims are extracted
5. A `UserContext` is created from the token claims

## JWT Claims

The orchestrator extracts these claims:

| Claim | Required | Description |
|-------|----------|-------------|
| `sub` | Yes | Subject — used as the external user ID |
| `email` | No | User's email address |
| `name` | No | User's display name |
| `exp` | No | Token expiration time |

## JWKS Caching

The `JwksCache` manages public keys from the JWKS endpoint:

| Setting | Value |
|---------|-------|
| Cache TTL | 1 hour (3600 seconds) |
| Max stale cache | 24 hours (86400 seconds) |
| Allow stale | Enabled by default |
| HTTP timeout | 10 seconds |

- Keys are fetched on first use and cached
- After the TTL expires, a refresh is attempted
- If the refresh fails and `allow_stale` is enabled, the stale key is used (up to 24 hours)
- This prevents authentication outages due to temporary JWKS endpoint unavailability

## Supported Key Types

The JWKS cache supports **RSA** keys only:

- Keys with `kty: RSA` and `n`/`e` components
- Keys with `x5c` (X.509 certificate chain) — the first certificate is used
- Encryption-only keys (`use: enc`) are skipped
- Non-RSA key types are skipped

## User Creation

On successful JWT validation, the orchestrator calls `UserStore::get_or_create_user` with:

- `external_id` = JWT `sub` claim
- `provider` = "jwt"
- `email` and `display_name` from token claims

If the user doesn't exist, they are created with default preferences.
