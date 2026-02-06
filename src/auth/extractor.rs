//! Authentication extractor for HTTP requests.

use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::auth::context::UserContext;
use crate::auth::jwks::{DEFAULT_CACHE_TTL_SECONDS, JwksCache};
use crate::auth::user_store::UserStore;
use crate::db::Db;
use crate::types::{ApiKeyHash, ApiKeyPrefix, ExternalUserId, IdentityProvider};
use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::debug;

/// Authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Whether to allow anonymous access (single-user local mode)
    pub allow_anonymous: bool,
    /// Header name for API key authentication
    pub api_key_header: String,
    /// Expected API key value (for simple deployments)
    /// In production, use JWT or a proper API key store
    pub api_key: Option<String>,
    /// Whether to validate JWT tokens
    pub jwt_enabled: bool,
    /// JWT issuer for validation
    pub jwt_issuer: Option<String>,
    /// JWT audience for validation
    pub jwt_audience: Option<String>,
    /// JWKS endpoint URL for key fetching
    #[serde(default)]
    pub jwks_url: Option<String>,
    /// JWKS cache TTL in seconds (default: 3600)
    #[serde(default = "default_jwks_cache_seconds")]
    pub jwks_cache_seconds: u64,
    /// Whether to allow stale JWKS cache on fetch failure
    #[serde(default = "default_allow_stale_jwks")]
    pub allow_stale_jwks: bool,
    /// Whether to enable database-backed API key lookup
    #[serde(default)]
    pub db_api_keys_enabled: bool,
}

fn default_jwks_cache_seconds() -> u64 {
    DEFAULT_CACHE_TTL_SECONDS
}

fn default_allow_stale_jwks() -> bool {
    true
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            // Default to anonymous for local development
            allow_anonymous: true,
            api_key_header: "X-API-Key".to_string(),
            api_key: None,
            jwt_enabled: false,
            jwt_issuer: None,
            jwt_audience: None,
            jwks_url: None,
            jwks_cache_seconds: DEFAULT_CACHE_TTL_SECONDS,
            allow_stale_jwks: true,
            db_api_keys_enabled: false,
        }
    }
}

impl AuthConfig {
    /// Create a config for local single-user mode.
    pub fn local() -> Self {
        Self {
            allow_anonymous: true,
            ..Default::default()
        }
    }

    /// Create a config for static API key authentication.
    pub fn with_api_key(api_key: String) -> Self {
        Self {
            allow_anonymous: false,
            api_key: Some(api_key),
            ..Default::default()
        }
    }

    /// Create a config for database-backed API key authentication.
    pub fn with_db_api_keys() -> Self {
        Self {
            allow_anonymous: false,
            db_api_keys_enabled: true,
            ..Default::default()
        }
    }

    /// Create a config for JWT authentication with JWKS for RS256 signature verification.
    pub fn with_jwt(issuer: String, jwks_url: String, audience: Option<String>) -> Self {
        Self {
            allow_anonymous: false,
            jwt_enabled: true,
            jwt_issuer: Some(issuer),
            jwt_audience: audience,
            jwks_url: Some(jwks_url),
            jwks_cache_seconds: DEFAULT_CACHE_TTL_SECONDS,
            allow_stale_jwks: true,
            ..Default::default()
        }
    }
}

/// Authentication errors.
#[derive(Debug, Clone)]
pub enum AuthError {
    /// No authentication provided and anonymous not allowed
    Unauthenticated,
    /// Invalid API key
    InvalidApiKey,
    /// API key is expired
    ApiKeyExpired,
    /// API key is inactive/revoked
    ApiKeyRevoked,
    /// Invalid or expired JWT
    InvalidToken(String),
    /// User is deactivated
    UserDeactivated,
    /// Database error
    DatabaseError(String),
    /// JWKS error
    JwksError(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthenticated => write!(f, "Authentication required"),
            Self::InvalidApiKey => write!(f, "Invalid API key"),
            Self::ApiKeyExpired => write!(f, "API key has expired"),
            Self::ApiKeyRevoked => write!(f, "API key has been revoked"),
            Self::InvalidToken(msg) => write!(f, "Invalid token: {}", msg),
            Self::UserDeactivated => write!(f, "User account is deactivated"),
            Self::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            Self::JwksError(msg) => write!(f, "JWKS error: {}", msg),
        }
    }
}

impl std::error::Error for AuthError {}

/// Authentication extractor for HTTP requests.
pub struct AuthExtractor {
    config: AuthConfig,
    user_store: Arc<UserStore>,
    jwks_cache: Option<Arc<JwksCache>>,
    db: Db,
}

impl AuthExtractor {
    /// Create a new auth extractor.
    pub fn new(config: AuthConfig, db: Db) -> Self {
        // Initialize JWKS cache if URL is provided
        let jwks_cache = config.jwks_url.as_ref().map(|url| {
            Arc::new(JwksCache::new(
                url.clone(),
                config.jwks_cache_seconds,
                config.allow_stale_jwks,
            ))
        });

        Self {
            config,
            user_store: Arc::new(UserStore::new(db.clone())),
            jwks_cache,
            db,
        }
    }

    /// Get reference to the user store.
    pub fn user_store(&self) -> &Arc<UserStore> {
        &self.user_store
    }

    /// Get reference to the database.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Extract user context from HTTP headers.
    ///
    /// This checks authentication in order:
    /// 1. Bearer token (JWT) if enabled
    /// 2. API key header
    /// 3. Anonymous if allowed
    pub async fn extract_user(
        &self,
        authorization: Option<&str>,
        api_key: Option<&str>,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Result<UserContext, AuthError> {
        // Try Bearer token first
        if let Some(auth_header) = authorization
            && let Some(token) = auth_header.strip_prefix("Bearer ")
        {
            return self.extract_from_jwt(token, ip_address, user_agent).await;
        }

        // Try API key
        if let Some(key) = api_key {
            return self.extract_from_api_key(key, ip_address, user_agent).await;
        }

        // Fall back to anonymous if allowed
        if self.config.allow_anonymous {
            return self.extract_anonymous(ip_address, user_agent).await;
        }

        Err(AuthError::Unauthenticated)
    }

    /// Extract user from JWT token with RS256 signature verification.
    async fn extract_from_jwt(
        &self,
        token: &str,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Result<UserContext, AuthError> {
        if !self.config.jwt_enabled {
            return Err(AuthError::InvalidToken(
                "JWT authentication not enabled".to_string(),
            ));
        }

        let jwks_cache = self.jwks_cache.as_ref().ok_or_else(|| {
            AuthError::InvalidToken("JWT enabled but JWKS URL not configured".to_string())
        })?;

        // Parse the JWT header to get the key ID (kid)
        let header = decode_header(token)
            .map_err(|e| AuthError::InvalidToken(format!("Invalid JWT header: {}", e)))?;

        // Get the decoding key from JWKS cache
        let decoding_key = jwks_cache
            .get_key(header.kid.as_deref())
            .await
            .map_err(|e| AuthError::JwksError(e.to_string()))?;

        // Set up validation
        let mut validation = Validation::new(Algorithm::RS256);

        // Configure issuer validation
        if let Some(issuer) = &self.config.jwt_issuer {
            validation.set_issuer(&[issuer]);
        }

        // Configure audience validation
        if let Some(audience) = &self.config.jwt_audience {
            validation.set_audience(&[audience]);
        }

        // Decode and validate the token
        let token_data = decode::<JwtClaims>(token, &decoding_key, &validation).map_err(|e| {
            AuthError::InvalidToken(format!("Signature verification failed: {}", e))
        })?;

        let claims = token_data.claims;

        // Additional expiration check (jsonwebtoken does this, but be explicit)
        if let Some(exp) = claims.exp {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            if exp < now {
                return Err(AuthError::InvalidToken("Token expired".to_string()));
            }
        }

        debug!("JWT verified successfully for subject: {}", claims.sub);

        // Get or create user
        let user = self
            .user_store
            .get_or_create_user(
                &claims.sub,
                "jwt",
                claims.email.as_deref(),
                claims.name.as_deref(),
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;

        if !user.is_active {
            return Err(AuthError::UserDeactivated);
        }

        let ctx = UserContext::new(
            user.id,
            ExternalUserId::new(claims.sub),
            IdentityProvider::new("jwt"),
            claims.email,
            claims.name,
        )
        .with_client_info(ip_address, user_agent);

        Ok(ctx)
    }

    /// Extract user from API key.
    pub async fn extract_from_api_key(
        &self,
        key: &str,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Result<UserContext, AuthError> {
        // Database-backed API key lookup
        if self.config.db_api_keys_enabled {
            return self
                .extract_from_db_api_key(key, ip_address, user_agent)
                .await;
        }

        // Static API key validation
        if let Some(expected_key) = &self.config.api_key {
            if key != expected_key {
                return Err(AuthError::InvalidApiKey);
            }
        } else {
            return Err(AuthError::InvalidApiKey);
        }

        // Hash the key for user identity
        let key_hash = hash_api_key(key);

        // Get or create user for this API key
        let user = self
            .user_store
            .get_or_create_user(key_hash.as_str(), "api_key", None, Some("API User"))
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;

        if !user.is_active {
            return Err(AuthError::UserDeactivated);
        }

        let ctx = UserContext::new(
            user.id,
            ExternalUserId::new(key_hash.into_inner()),
            IdentityProvider::new("api_key"),
            None,
            Some("API User".to_string()),
        )
        .with_client_info(ip_address, user_agent);

        Ok(ctx)
    }

    /// Extract user from database-backed API key.
    async fn extract_from_db_api_key(
        &self,
        key: &str,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Result<UserContext, AuthError> {
        use crate::db::QueryBuilder;

        let key_hash = hash_api_key(key);

        // Look up the API key
        let api_key = QueryBuilder::find_api_key_by_hash(&self.db, key_hash.as_str())
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?
            .ok_or(AuthError::InvalidApiKey)?;

        // Check if the key is active
        if !api_key.is_active {
            return Err(AuthError::ApiKeyRevoked);
        }

        // Check expiration
        if let Some(expires_at) = &api_key.expires_at {
            let now = chrono::Utc::now();
            let expires = chrono::DateTime::parse_from_rfc3339(&expires_at.to_string())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or(now);
            if expires < now {
                return Err(AuthError::ApiKeyExpired);
            }
        }

        // Update last_used_at
        let _ = QueryBuilder::update_api_key_last_used(&self.db, &api_key.id).await;

        // Get the user associated with this API key, or create one
        let user_id = if let Some(ref user_record_id) = api_key.user_id {
            user_record_id.clone()
        } else {
            let user = self
                .user_store
                .get_or_create_user(
                    &format!("api_key:{}", api_key.key_prefix),
                    "api_key",
                    None,
                    api_key.name.as_deref(),
                )
                .await
                .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
            user.id
        };

        let display_name = api_key
            .name
            .clone()
            .unwrap_or_else(|| format!("API Key {}", api_key.key_prefix));

        let ctx = UserContext::new(
            user_id,
            ExternalUserId::new(format!("api_key:{}", api_key.key_prefix)),
            IdentityProvider::new("api_key"),
            None,
            Some(display_name),
        )
        .with_client_info(ip_address, user_agent);

        Ok(ctx)
    }

    /// Extract anonymous user for local mode.
    async fn extract_anonymous(
        &self,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Result<UserContext, AuthError> {
        // Get or create the anonymous user
        let user = self
            .user_store
            .get_or_create_user("anonymous", "anonymous", None, Some("Local User"))
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;

        if !user.is_active {
            return Err(AuthError::UserDeactivated);
        }

        let ctx = UserContext::anonymous(user.id).with_client_info(ip_address, user_agent);

        Ok(ctx)
    }
}

/// JWT claims structure.
#[derive(Debug, Deserialize)]
pub struct JwtClaims {
    /// Subject (user ID)
    pub sub: String,
    /// Email
    pub email: Option<String>,
    /// Name
    pub name: Option<String>,
    /// Expiration time (Unix timestamp)
    pub exp: Option<u64>,
}

/// Hash an API key for storage and lookup (don't store raw keys).
pub fn hash_api_key(key: &str) -> ApiKeyHash {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let result = hasher.finalize();
    ApiKeyHash::new(format!("{:x}", result))
}

/// Generate a new API key with the format: prefix_randompart
/// Returns (full_key, prefix, hash)
pub fn generate_api_key() -> (String, ApiKeyPrefix, ApiKeyHash) {
    use uuid::Uuid;

    let prefix = ApiKeyPrefix::new(format!("uo_{}", &Uuid::new_v4().to_string()[..8]));
    let secret = Uuid::new_v4().to_string().replace("-", "");
    let full_key = format!("{}_{}", prefix, secret);
    let key_hash = hash_api_key(&full_key);

    (full_key, prefix, key_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{DatabaseConfig, create_connection, ensure_schema};

    async fn setup_test_db() -> crate::db::Db {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = create_connection(config).await.unwrap();
        ensure_schema(&db).await.unwrap();
        db
    }

    #[test]
    fn test_auth_config_default() {
        let config = AuthConfig::default();
        assert!(config.allow_anonymous);
        assert_eq!(config.api_key_header, "X-API-Key");
        assert!(!config.jwt_enabled);
        assert!(config.jwks_url.is_none());
        assert_eq!(config.jwks_cache_seconds, DEFAULT_CACHE_TTL_SECONDS);
        assert!(config.allow_stale_jwks);
    }

    #[test]
    fn test_auth_config_local() {
        let config = AuthConfig::local();
        assert!(config.allow_anonymous);
    }

    #[test]
    fn test_auth_config_with_api_key() {
        let config = AuthConfig::with_api_key("secret123".to_string());
        assert!(!config.allow_anonymous);
        assert_eq!(config.api_key, Some("secret123".to_string()));
    }

    #[test]
    fn test_auth_config_with_jwt() {
        let config = AuthConfig::with_jwt(
            "https://issuer.example.com".to_string(),
            "https://issuer.example.com/.well-known/jwks.json".to_string(),
            Some("my-api".to_string()),
        );
        assert!(!config.allow_anonymous);
        assert!(config.jwt_enabled);
        assert_eq!(
            config.jwt_issuer,
            Some("https://issuer.example.com".to_string())
        );
        assert_eq!(config.jwt_audience, Some("my-api".to_string()));
        assert_eq!(
            config.jwks_url,
            Some("https://issuer.example.com/.well-known/jwks.json".to_string())
        );
    }

    #[test]
    fn test_auth_config_with_db_api_keys() {
        let config = AuthConfig::with_db_api_keys();
        assert!(!config.allow_anonymous);
        assert!(config.db_api_keys_enabled);
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_hash_api_key() {
        let hash1 = hash_api_key("secret123");
        let hash2 = hash_api_key("secret123");
        let hash3 = hash_api_key("different");

        // Should be a hex string
        assert!(hash1.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(hash1, hash2); // Same input = same output
        assert_ne!(hash1, hash3); // Different input = different output
    }

    #[test]
    fn test_generate_api_key() {
        let (full_key, prefix, hash) = generate_api_key();

        assert!(full_key.starts_with("uo_"));
        assert!(prefix.as_str().starts_with("uo_"));
        assert!(full_key.contains(prefix.as_str()));
        assert!(hash.as_str().chars().all(|c| c.is_ascii_hexdigit()));

        // Verify hash matches
        assert_eq!(hash, hash_api_key(&full_key));
    }

    #[test]
    fn test_auth_error_display() {
        assert_eq!(
            AuthError::Unauthenticated.to_string(),
            "Authentication required"
        );
        assert_eq!(AuthError::InvalidApiKey.to_string(), "Invalid API key");
        assert_eq!(AuthError::ApiKeyExpired.to_string(), "API key has expired");
        assert_eq!(
            AuthError::ApiKeyRevoked.to_string(),
            "API key has been revoked"
        );
        assert_eq!(
            AuthError::UserDeactivated.to_string(),
            "User account is deactivated"
        );
    }

    #[tokio::test]
    async fn test_auth_extractor_anonymous_mode() {
        let db = setup_test_db().await;
        let config = AuthConfig::local();
        let extractor = AuthExtractor::new(config, db);

        let result = extractor
            .extract_user(
                None,
                None,
                Some("127.0.0.1".to_string()),
                Some("Test Agent".to_string()),
            )
            .await;

        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert!(ctx.is_anonymous());
        assert_eq!(ctx.ip_address(), Some("127.0.0.1"));
        assert_eq!(ctx.user_agent(), Some("Test Agent"));
    }

    #[tokio::test]
    async fn test_auth_extractor_api_key_valid() {
        let db = setup_test_db().await;
        let config = AuthConfig::with_api_key("secret123".to_string());
        let extractor = AuthExtractor::new(config, db);

        let result = extractor
            .extract_user(None, Some("secret123"), None, None)
            .await;

        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert!(!ctx.is_anonymous());
        assert_eq!(ctx.provider().as_str(), "api_key");
    }

    #[tokio::test]
    async fn test_auth_extractor_api_key_invalid() {
        let db = setup_test_db().await;
        let config = AuthConfig::with_api_key("secret123".to_string());
        let extractor = AuthExtractor::new(config, db);

        let result = extractor
            .extract_user(None, Some("wrong_key"), None, None)
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AuthError::InvalidApiKey));
    }

    #[tokio::test]
    async fn test_auth_extractor_no_auth_required() {
        let db = setup_test_db().await;
        let config = AuthConfig::with_api_key("secret123".to_string());
        let extractor = AuthExtractor::new(config, db);

        let result = extractor.extract_user(None, None, None, None).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AuthError::Unauthenticated));
    }

    #[tokio::test]
    async fn test_user_deactivation_blocks_access() {
        let db = setup_test_db().await;
        let config = AuthConfig::local();
        let extractor = AuthExtractor::new(config, db.clone());

        let ctx = extractor
            .extract_user(None, None, None, None)
            .await
            .unwrap();

        extractor
            .user_store()
            .deactivate_user(ctx.user_id())
            .await
            .unwrap();

        let result = extractor.extract_user(None, None, None, None).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AuthError::UserDeactivated));
    }

    #[test]
    fn test_jwt_claims_deserialization() {
        let json = r#"{
            "sub": "user123",
            "email": "user@example.com",
            "name": "Test User",
            "exp": 1735689600
        }"#;

        let claims: JwtClaims = serde_json::from_str(json).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.email, Some("user@example.com".to_string()));
        assert_eq!(claims.name, Some("Test User".to_string()));
        assert_eq!(claims.exp, Some(1735689600));
    }
}
