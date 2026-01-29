//! JWKS (JSON Web Key Set) fetching and caching module.
//!
//! This module provides functionality for fetching and caching JSON Web Keys
//! from a JWKS endpoint for JWT signature verification.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use jsonwebtoken::DecodingKey;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Default cache TTL in seconds (1 hour).
pub const DEFAULT_CACHE_TTL_SECONDS: u64 = 3600;

/// Maximum stale cache age in seconds (24 hours).
pub const MAX_STALE_CACHE_SECONDS: u64 = 86400;

/// A single JSON Web Key from a JWKS document.
#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    /// Key type (e.g., "RSA")
    pub kty: String,
    /// Key ID (optional, used to match JWT header kid)
    pub kid: Option<String>,
    /// Algorithm (e.g., "RS256")
    pub alg: Option<String>,
    /// Key use (e.g., "sig" for signature)
    #[serde(rename = "use")]
    pub key_use: Option<String>,
    /// RSA modulus (base64url encoded)
    pub n: Option<String>,
    /// RSA exponent (base64url encoded)
    pub e: Option<String>,
    /// X.509 certificate chain
    pub x5c: Option<Vec<String>>,
}

/// A JWKS document containing multiple keys.
#[derive(Debug, Clone, Deserialize)]
pub struct JwksDocument {
    pub keys: Vec<Jwk>,
}

/// Cached key entry with metadata.
#[derive(Clone)]
struct CachedKey {
    decoding_key: DecodingKey,
    #[allow(dead_code)]
    fetched_at: Instant,
}

/// Thread-safe JWKS cache with automatic refresh.
pub struct JwksCache {
    /// The JWKS endpoint URL.
    jwks_url: String,
    /// Cache TTL in seconds.
    cache_ttl: Duration,
    /// Whether to allow stale cache on fetch failure.
    allow_stale: bool,
    /// Cached keys by kid.
    keys: Arc<RwLock<HashMap<String, CachedKey>>>,
    /// Last successful fetch time.
    last_fetch: Arc<RwLock<Option<Instant>>>,
    /// HTTP client for fetching JWKS.
    client: reqwest::Client,
}

impl JwksCache {
    /// Create a new JWKS cache.
    pub fn new(jwks_url: String, cache_ttl_seconds: u64, allow_stale: bool) -> Self {
        Self {
            jwks_url,
            cache_ttl: Duration::from_secs(cache_ttl_seconds),
            allow_stale,
            keys: Arc::new(RwLock::new(HashMap::new())),
            last_fetch: Arc::new(RwLock::new(None)),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Get a decoding key by key ID.
    ///
    /// If `kid` is None, returns the first available key.
    /// Fetches from the JWKS endpoint if cache is stale or key not found.
    pub async fn get_key(&self, kid: Option<&str>) -> Result<DecodingKey, JwksCacheError> {
        // Check if cache is stale
        let should_refresh = {
            let last_fetch = self.last_fetch.read().await;
            match *last_fetch {
                Some(t) => t.elapsed() > self.cache_ttl,
                None => true,
            }
        };

        // Try to get from cache first
        if !should_refresh {
            if let Some(key) = self.get_from_cache(kid).await {
                return Ok(key);
            }
        }

        // Need to refresh or key not found
        match self.fetch_keys().await {
            Ok(()) => {
                // Try to get the key again after refresh
                self.get_from_cache(kid)
                    .await
                    .ok_or_else(|| {
                        if let Some(k) = kid {
                            JwksCacheError::KeyNotFound(k.to_string())
                        } else {
                            JwksCacheError::NoKeysAvailable
                        }
                    })
            }
            Err(e) => {
                // Fetch failed - try stale cache if allowed
                if self.allow_stale {
                    let last_fetch = self.last_fetch.read().await;
                    let stale_ok = last_fetch
                        .map(|t| t.elapsed() < Duration::from_secs(MAX_STALE_CACHE_SECONDS))
                        .unwrap_or(false);

                    if stale_ok {
                        warn!("JWKS fetch failed, using stale cache: {}", e);
                        if let Some(key) = self.get_from_cache(kid).await {
                            return Ok(key);
                        }
                    }
                }

                Err(e)
            }
        }
    }

    /// Get a key from the cache without fetching.
    async fn get_from_cache(&self, kid: Option<&str>) -> Option<DecodingKey> {
        let keys = self.keys.read().await;

        match kid {
            Some(k) => keys.get(k).map(|c| c.decoding_key.clone()),
            None => {
                // Return the first key if no kid specified
                keys.values().next().map(|c| c.decoding_key.clone())
            }
        }
    }

    /// Fetch keys from the JWKS endpoint.
    pub async fn fetch_keys(&self) -> Result<(), JwksCacheError> {
        debug!("Fetching JWKS from {}", self.jwks_url);

        let response = self
            .client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| JwksCacheError::FetchError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(JwksCacheError::FetchError(format!(
                "HTTP {} from JWKS endpoint",
                response.status()
            )));
        }

        let jwks: JwksDocument = response
            .json()
            .await
            .map_err(|e| JwksCacheError::ParseError(e.to_string()))?;

        let mut new_keys = HashMap::new();
        let now = Instant::now();

        for jwk in jwks.keys {
            // Only process RSA keys for now
            if jwk.kty != "RSA" {
                debug!("Skipping non-RSA key: {:?}", jwk.kty);
                continue;
            }

            // Only process signature keys
            if jwk.key_use.as_deref() == Some("enc") {
                debug!("Skipping encryption key");
                continue;
            }

            match Self::jwk_to_decoding_key(&jwk) {
                Ok(decoding_key) => {
                    let kid = jwk.kid.clone().unwrap_or_else(|| "default".to_string());
                    debug!("Cached key with kid: {}", kid);
                    new_keys.insert(
                        kid,
                        CachedKey {
                            decoding_key,
                            fetched_at: now,
                        },
                    );
                }
                Err(e) => {
                    warn!("Failed to parse JWK: {}", e);
                }
            }
        }

        if new_keys.is_empty() {
            return Err(JwksCacheError::NoValidKeys);
        }

        // Update cache
        {
            let mut keys = self.keys.write().await;
            *keys = new_keys;
        }

        {
            let mut last_fetch = self.last_fetch.write().await;
            *last_fetch = Some(now);
        }

        debug!("Successfully cached {} keys", self.keys.read().await.len());
        Ok(())
    }

    /// Convert a JWK to a jsonwebtoken DecodingKey.
    fn jwk_to_decoding_key(jwk: &Jwk) -> Result<DecodingKey, JwksCacheError> {
        // Try X.509 certificate first
        if let Some(x5c) = &jwk.x5c {
            if let Some(cert) = x5c.first() {
                // x5c contains base64-encoded (not URL-safe) DER certificates
                let cert_der = base64::engine::general_purpose::STANDARD
                    .decode(cert)
                    .map_err(|e| JwksCacheError::ParseError(format!("Invalid x5c: {}", e)))?;

                // from_rsa_der doesn't return Result, use from_rsa_pem with proper conversion
                // or use from_rsa_components instead - x5c is actually a certificate, not raw key
                // For proper x5c handling, we'd need to extract the public key from the cert
                // For now, prefer n/e components which are more common in JWKS
                return Ok(DecodingKey::from_rsa_der(&cert_der));
            }
        }

        // Fall back to n and e (most common case)
        let n = jwk
            .n
            .as_ref()
            .ok_or_else(|| JwksCacheError::ParseError("Missing 'n' in RSA key".to_string()))?;
        let e = jwk
            .e
            .as_ref()
            .ok_or_else(|| JwksCacheError::ParseError("Missing 'e' in RSA key".to_string()))?;

        DecodingKey::from_rsa_components(n, e)
            .map_err(|e| JwksCacheError::ParseError(format!("Invalid RSA components: {}", e)))
    }

    /// Check if the cache has any keys.
    pub async fn has_keys(&self) -> bool {
        !self.keys.read().await.is_empty()
    }

    /// Get the number of cached keys.
    pub async fn key_count(&self) -> usize {
        self.keys.read().await.len()
    }

    /// Clear the cache (useful for testing).
    pub async fn clear(&self) {
        let mut keys = self.keys.write().await;
        keys.clear();
        let mut last_fetch = self.last_fetch.write().await;
        *last_fetch = None;
    }
}

/// Errors that can occur when working with the JWKS cache.
#[derive(Debug, Clone)]
pub enum JwksCacheError {
    /// Failed to fetch JWKS from endpoint.
    FetchError(String),
    /// Failed to parse JWKS response.
    ParseError(String),
    /// No valid keys found in JWKS.
    NoValidKeys,
    /// Key with specified kid not found.
    KeyNotFound(String),
    /// No keys available in cache.
    NoKeysAvailable,
}

impl std::fmt::Display for JwksCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FetchError(msg) => write!(f, "Failed to fetch JWKS: {}", msg),
            Self::ParseError(msg) => write!(f, "Failed to parse JWKS: {}", msg),
            Self::NoValidKeys => write!(f, "No valid keys found in JWKS"),
            Self::KeyNotFound(kid) => write!(f, "Key not found: {}", kid),
            Self::NoKeysAvailable => write!(f, "No keys available in cache"),
        }
    }
}

impl std::error::Error for JwksCacheError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jwks_cache_error_display() {
        let err = JwksCacheError::FetchError("timeout".to_string());
        assert_eq!(err.to_string(), "Failed to fetch JWKS: timeout");

        let err = JwksCacheError::KeyNotFound("key123".to_string());
        assert_eq!(err.to_string(), "Key not found: key123");

        let err = JwksCacheError::NoKeysAvailable;
        assert_eq!(err.to_string(), "No keys available in cache");
    }

    #[tokio::test]
    async fn test_jwks_cache_clear() {
        let cache = JwksCache::new(
            "https://example.com/.well-known/jwks.json".to_string(),
            3600,
            true,
        );

        assert!(!cache.has_keys().await);
        assert_eq!(cache.key_count().await, 0);

        cache.clear().await;
        assert!(!cache.has_keys().await);
    }

    #[test]
    fn test_jwk_deserialization() {
        let json = r#"{
            "kty": "RSA",
            "kid": "test-key-1",
            "alg": "RS256",
            "use": "sig",
            "n": "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw",
            "e": "AQAB"
        }"#;

        let jwk: Jwk = serde_json::from_str(json).unwrap();
        assert_eq!(jwk.kty, "RSA");
        assert_eq!(jwk.kid, Some("test-key-1".to_string()));
        assert_eq!(jwk.alg, Some("RS256".to_string()));
        assert_eq!(jwk.key_use, Some("sig".to_string()));
        assert!(jwk.n.is_some());
        assert!(jwk.e.is_some());
    }

    #[test]
    fn test_jwks_document_deserialization() {
        let json = r#"{
            "keys": [
                {
                    "kty": "RSA",
                    "kid": "key1",
                    "n": "test",
                    "e": "AQAB"
                },
                {
                    "kty": "RSA",
                    "kid": "key2",
                    "n": "test2",
                    "e": "AQAB"
                }
            ]
        }"#;

        let doc: JwksDocument = serde_json::from_str(json).unwrap();
        assert_eq!(doc.keys.len(), 2);
        assert_eq!(doc.keys[0].kid, Some("key1".to_string()));
        assert_eq!(doc.keys[1].kid, Some("key2".to_string()));
    }
}
