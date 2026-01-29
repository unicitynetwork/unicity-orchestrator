//! URL mode elicitation handler for OAuth and other out-of-band interactions.
//!
//! URL mode allows servers to direct users to external URLs for sensitive
//! interactions that must not pass through the MCP client, such as:
//! - OAuth authorization flows
//! - API key entry
//! - Payment processing
//!
//! ## Extension Status
//!
//! **URL mode is an extension to MCP elicitation**, not part of the core spec:
//! - MCP 2025-06-18 only defines form-mode elicitation via `elicitation/create`
//! - MCP 2025-11-25 (SEP-1036) adds URL-mode support via `-32042` error code
//! - rmcp 0.12.0 doesn't implement URL mode natively yet
//! - We implement it as an extension using `UrlElicitationRequest`
//!
//! ## Error Code
//!
//! Servers that need URL-mode elicitation should return error code `-32042`
//! (`URL_ELICITATION_REQUIRED_ERROR_CODE`) with the URL in the error data.
//! See `ElicitationError::url_elicitation_required()` for a convenient builder.

use crate::elicitation::{ElicitationError, ElicitationResult, UrlElicitationRequest};
use crate::elicitation::store::{PermissionStore, OAuthState};
use std::sync::Arc;
use uuid::Uuid;

/// URL mode elicitation handler.
#[derive(Clone)]
pub struct UrlHandler {
    store: Arc<PermissionStore>,
    /// Base URL for the orchestrator's OAuth callback endpoint
    callback_base_url: String,
}

impl UrlHandler {
    /// Create a new URL handler.
    pub fn new(store: Arc<PermissionStore>) -> ElicitationResult<Self> {
        Ok(Self {
            store,
            callback_base_url: "http://localhost:3942".to_string(), // TODO: Configurable
        })
    }

    /// Set the callback base URL.
    pub fn with_callback_url(mut self, url: String) -> Self {
        self.callback_base_url = url;
        self
    }

    /// Validate a URL elicitation request.
    pub fn validate_request(&self, request: &UrlElicitationRequest) -> ElicitationResult<()> {
        // Ensure message is present
        if request.message.is_empty() {
            return Err(ElicitationError::MissingField("message".to_string()));
        }

        // Validate URL format
        let parsed = ::url::Url::parse(request.url.as_str())
            .map_err(|_| ElicitationError::InvalidSchema("Invalid URL format".to_string()))?;

        // Ensure HTTPS for production (allow HTTP for development)
        if !["http", "https"].contains(&parsed.scheme()) {
            return Err(ElicitationError::InvalidSchema("URL must use HTTP or HTTPS".to_string()));
        }

        // Warn about HTTP in production
        if parsed.scheme() == "http" && !parsed.host_str().map_or(false, |h| h.starts_with("localhost") || h.starts_with("127.0.0.1")) {
            tracing::warn!("URL mode elicitation using HTTP (non-localhost): {}", request.url);
        }

        // Ensure elicitation_id is present
        if request.elicitation_id.is_empty() {
            return Err(ElicitationError::MissingField("elicitation_id".to_string()));
        }

        Ok(())
    }

    /// Generate a unique elicitation ID for URL mode.
    pub fn generate_elicitation_id(&self) -> String {
        format!("elicitation-{}", Uuid::new_v4())
    }

    /// Create an OAuth state entry for a URL elicitation.
    ///
    /// This binds the elicitation to the user's identity for security.
    pub async fn create_oauth_state(
        &self,
        user_id: &str,
        provider: &str,
        redirect_uri: &str,
        ttl_seconds: u64,
    ) -> ElicitationResult<String> {
        let elicitation_id = self.generate_elicitation_id();
        let state_token = format!("state-{}", Uuid::new_v4());

        let state = OAuthState {
            elicitation_id: elicitation_id.clone(),
            user_id: crate::types::ExternalUserId::new(user_id),
            provider: crate::types::IdentityProvider::new(provider),
            state_token: state_token.clone(),
            redirect_uri: crate::types::RedirectUri::new(redirect_uri),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(ttl_seconds as i64),
        };

        self.store.store_oauth_state(state).await?;

        Ok(elicitation_id)
    }

    /// Validate and retrieve OAuth state.
    pub async fn validate_oauth_state(&self, elicitation_id: &str) -> ElicitationResult<OAuthState> {
        self.store.get_oauth_state(elicitation_id).await?
            .ok_or_else(|| ElicitationError::NotFound(elicitation_id.to_string()))
    }

    /// Complete an OAuth flow and consume the state.
    pub async fn complete_oauth_flow(&self, elicitation_id: &str) -> ElicitationResult<()> {
        self.store.consume_oauth_state(elicitation_id).await
    }

    /// Build a "connect URL" for OAuth flows.
    ///
    /// This pattern prevents phishing attacks by ensuring the user who opens
    /// the URL is the same user who requested the elicitation.
    ///
    /// The connect URL should verify the user's session before redirecting
    /// to the actual OAuth provider.
    pub fn build_connect_url(&self, provider: &str, elicitation_id: &str) -> String {
        format!(
            "{}/oauth/connect/{}?elicitation_id={}",
            self.callback_base_url, provider, elicitation_id
        )
    }

    /// Create a URL mode elicitation request for OAuth.
    pub fn create_oauth_elicitation(
        &self,
        _user_id: &str,
        provider: &str,
        message: &str,
    ) -> ElicitationResult<(UrlElicitationRequest, String)> {
        let elicitation_id = self.generate_elicitation_id();
        let connect_url = self.build_connect_url(provider, &elicitation_id);

        let request = UrlElicitationRequest {
            message: message.to_string(),
            url: crate::types::OAuthUrl::new(connect_url.clone()),
            elicitation_id: elicitation_id.clone(),
            service_name: Some(crate::types::ServiceName::new(provider)),
        };

        Ok((request, elicitation_id))
    }

    /// Clean up expired OAuth state entries.
    pub async fn cleanup_expired(&self) -> ElicitationResult<usize> {
        self.store.cleanup_expired_oauth_state().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_generate_elicitation_id() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        let store = PermissionStore::new(db);
        let handler = UrlHandler::new(Arc::new(store)).unwrap();

        let id1 = handler.generate_elicitation_id();
        let id2 = handler.generate_elicitation_id();

        assert_ne!(id1, id2);
        assert!(id1.starts_with("elicitation-"));
    }

    #[tokio::test]
    async fn test_validate_url_request() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        let store = PermissionStore::new(db);
        let handler = UrlHandler::new(Arc::new(store)).unwrap();

        let valid_request = UrlElicitationRequest {
            message: "Please authorize".to_string(),
            url: crate::types::OAuthUrl::new("https://example.com/auth"),
            elicitation_id: "test-id".to_string(),
            service_name: None,
        };

        assert!(handler.validate_request(&valid_request).is_ok());

        // Empty URL
        let invalid_request = UrlElicitationRequest {
            message: "Please authorize".to_string(),
            url: crate::types::OAuthUrl::new(""),
            elicitation_id: "test-id".to_string(),
            service_name: None,
        };

        assert!(handler.validate_request(&invalid_request).is_err());
    }

    #[tokio::test]
    async fn test_build_connect_url() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        let store = PermissionStore::new(db);
        let handler = UrlHandler::new(Arc::new(store)).unwrap();

        let url = handler.build_connect_url("github", "test-elicitation-123");
        assert!(url.contains("/oauth/connect/github"));
        assert!(url.contains("elicitation_id=test-elicitation-123"));
    }

    #[tokio::test]
    async fn test_create_oauth_elicitation() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        let store = PermissionStore::new(db);
        let handler = UrlHandler::new(Arc::new(store)).unwrap();

        let (request, id) = handler.create_oauth_elicitation(
            "user123",
            "github",
            "Please authorize with GitHub"
        ).unwrap();

        assert!(request.url.as_str().contains("/oauth/connect/github"));
        assert_eq!(request.service_name.map(|s| s.to_string()), Some("github".to_string()));
        assert!(id.starts_with("elicitation-"));
    }
}
