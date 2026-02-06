//! Elicitation module for the Unicity Orchestrator.
//!
//! This module implements MCP elicitation (spec 2025-06-18), allowing the orchestrator
//! to request additional information from users through the MCP client. It supports:
//!
//! - **Form mode**: Structured data collection with JSON schema validation (MCP standard)
//! - **URL mode**: Out-of-band interactions for OAuth flows (extension, see below)
//! - **Tool approval**: Pre-flight permission checks for tool execution
//! - **Provenance wrapping**: Forward downstream service elicitations with service context
//!
//! ## MCP Spec Compliance
//!
//! This implementation follows the MCP 2025-06-18 / 2025-11-25 elicitation specification:
//!
//! ### Form Mode (Standard MCP)
//! - ✅ `elicitation/create` request/response via rmcp
//! - ✅ Schema types: String, Number, Integer, Boolean, Enum
//! - ✅ String formats: email, uri, date, date-time
//! - ✅ Actions: Accept, Decline, Cancel
//! - ✅ Client capability check (`capabilities.elicitation`)
//!
//! ### URL Mode (Extension)
//! - ⚠️ **URL mode is an extension** for OAuth flows, not part of standard MCP elicitation
//! - The MCP spec (SEP-1036, 2025-11-25) adds URL-mode via `-32042` error code
//! - rmcp 0.12.0 doesn't yet implement URL mode natively
//! - We implement URL mode as an extension using `UrlElicitationRequest`
//! - Error code `-32042` (`URL_ELICITATION_REQUIRED_ERROR_CODE`) for OAuth required
//!
//! ## Architecture
//!
//! The elicitation system is organized into several components:
//!
//! - `approval`: Tool approval manager with "allow once" / "always allow" permissions
//! - `form`: Form mode elicitation handler (uses rmcp's ElicitationSchema)
//! - `url`: URL mode elicitation handler for OAuth flows (extension)
//! - `store`: Permission storage in SurrealDB
//! - `provenance`: Wrapping downstream service elicitations with service context
//! - `error`: Error types including `URL_ELICITATION_REQUIRED_ERROR_CODE` (-32042)
//!
//! ## Security Considerations
//!
//! - Elicitation requests are bound to user identity (from auth), not session ID
//! - Sensitive data (API keys, passwords) MUST use URL mode, not form mode
//! - Tool approval permissions are stored per-user and can expire
//! - OAuth tokens are stored securely and never exposed to clients
//! - Provenance wrapping ensures users know which service is requesting information
//!
//! ## rmcp Integration
//!
//! This module uses rmcp's type-safe elicitation types:
//! - `ElicitationSchema` - type-safe schema builder
//! - `CreateElicitationRequestParam` - request parameters
//! - `CreateElicitationResult` - response with action and content
//! - `ElicitationAction` - Accept, Decline, Cancel
//!
//! ## Error Codes
//!
//! - `-32042`: URL elicitation required (server needs OAuth/external auth before proceeding)

mod approval;
mod error;
mod form;
#[cfg(test)]
mod integration_tests;
mod provenance;
mod store;
mod url;

pub use approval::{
    ApprovalAction, ApprovalManager, ApprovalRequest, PermissionStatus, ToolPermission,
};
pub use error::{ElicitationError, ElicitationResult};
pub use form::FormHandler;
pub use provenance::{wrap_url_with_provenance, wrap_with_provenance};
pub use store::PermissionStore;
pub use url::UrlHandler;

// Re-export rmcp elicitation types for external use
pub use rmcp::model::{
    CreateElicitationRequestParams, CreateElicitationResult, ElicitationAction, ElicitationSchema,
    PrimitiveSchema, StringFormat,
};

use crate::types::{OAuthUrl, ServiceName};
use anyhow::Result;
use rmcp::model::ClientCapabilities;
use rmcp::service::{Peer, RoleServer};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Policy for handling tool execution when the client doesn't support elicitation.
///
/// This is a security-critical setting that operators should configure based on
/// their deployment requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationFallbackPolicy {
    /// Deny tool execution if the client doesn't support elicitation.
    /// This is the secure default - tools cannot be executed without user approval.
    #[default]
    Deny,
    /// Allow tool execution if the client doesn't support elicitation.
    /// Use this for backwards compatibility with older clients, but be aware
    /// this bypasses the approval system entirely for those clients.
    Allow,
}

/// Elicitation modes supported by the orchestrator.
///
/// Note: The MCP spec uses form mode for elicitation/create.
/// URL mode is an extension for out-of-band OAuth flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationMode {
    /// Form mode: in-band structured data collection (standard MCP elicitation)
    Form,
    /// URL mode: out-of-band interaction via URL navigation (OAuth, etc.)
    Url,
}

impl ElicitationMode {
    /// Convert to MCP protocol string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Form => "form",
            Self::Url => "url",
        }
    }

    /// Parse from MCP protocol string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "form" => Some(Self::Form),
            "url" => Some(Self::Url),
            _ => None,
        }
    }
}

/// URL mode elicitation request (for OAuth and out-of-band flows).
///
/// This is NOT part of the standard MCP spec but extends it for OAuth support.
/// Standard form-mode elicitation uses rmcp's `CreateElicitationRequestParam`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlElicitationRequest {
    /// Human-readable message explaining why the interaction is needed
    pub message: String,

    /// The URL the user should navigate to
    pub url: OAuthUrl,

    /// Unique identifier for the elicitation (for callback matching)
    pub elicitation_id: String,

    /// Optional service name for provenance tracking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<ServiceName>,
}

/// Notification sent when URL mode elicitation completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationCompleteNotification {
    /// The elicitation ID from the original request
    pub elicitation_id: String,
}

/// Main elicitation coordinator.
///
/// The coordinator manages all elicitation operations and routes requests
/// to the appropriate handlers. It can send elicitation requests to connected
/// clients via the stored peer reference.
#[derive(Clone)]
pub struct ElicitationCoordinator {
    /// Client capabilities (which elicitation modes are supported)
    client_capabilities: Arc<RwLock<Option<ClientCapabilities>>>,

    /// Peer reference for sending elicitation requests to the client
    peer: Arc<RwLock<Option<Peer<RoleServer>>>>,

    /// Form mode handler
    form_handler: Arc<FormHandler>,

    /// URL mode handler
    url_handler: Arc<UrlHandler>,

    /// Tool approval manager
    approval_manager: Arc<ApprovalManager>,

    /// Permission store
    store: Arc<PermissionStore>,

    /// Policy for handling clients that don't support elicitation
    fallback_policy: Arc<RwLock<ElicitationFallbackPolicy>>,
}

impl ElicitationCoordinator {
    /// Create a new elicitation coordinator.
    pub fn new(db: surrealdb::Surreal<surrealdb::engine::any::Any>) -> Result<Self> {
        let store = Arc::new(PermissionStore::new(db));
        let approval_manager = Arc::new(ApprovalManager::new(store.clone()));
        let form_handler = Arc::new(FormHandler::new());
        let url_handler = Arc::new(UrlHandler::new(store.clone())?);

        Ok(Self {
            client_capabilities: Arc::new(RwLock::new(None)),
            peer: Arc::new(RwLock::new(None)),
            form_handler,
            url_handler,
            approval_manager,
            store,
            fallback_policy: Arc::new(RwLock::new(ElicitationFallbackPolicy::default())),
        })
    }

    /// Create a new elicitation coordinator with a specific fallback policy.
    pub fn new_with_policy(
        db: surrealdb::Surreal<surrealdb::engine::any::Any>,
        fallback_policy: ElicitationFallbackPolicy,
    ) -> Result<Self> {
        let store = Arc::new(PermissionStore::new(db));
        let approval_manager = Arc::new(ApprovalManager::new(store.clone()));
        let form_handler = Arc::new(FormHandler::new());
        let url_handler = Arc::new(UrlHandler::new(store.clone())?);

        Ok(Self {
            client_capabilities: Arc::new(RwLock::new(None)),
            peer: Arc::new(RwLock::new(None)),
            form_handler,
            url_handler,
            approval_manager,
            store,
            fallback_policy: Arc::new(RwLock::new(fallback_policy)),
        })
    }

    /// Set the fallback policy for clients that don't support elicitation.
    pub async fn set_fallback_policy(&self, policy: ElicitationFallbackPolicy) {
        *self.fallback_policy.write().await = policy;
    }

    /// Get the current fallback policy.
    pub async fn fallback_policy(&self) -> ElicitationFallbackPolicy {
        *self.fallback_policy.read().await
    }

    /// Store the peer reference for sending elicitation requests.
    ///
    /// This should be called during `initialize` when we receive the peer from context.
    pub async fn set_peer(&self, peer: Peer<RoleServer>) {
        *self.peer.write().await = Some(peer);
    }

    /// Update client capabilities from initialize request.
    pub async fn set_client_capabilities(&self, capabilities: &ClientCapabilities) {
        *self.client_capabilities.write().await = Some(capabilities.clone());
    }

    /// Get the approval manager.
    pub fn approval_manager(&self) -> &Arc<ApprovalManager> {
        &self.approval_manager
    }

    /// Get the permission store.
    pub fn store(&self) -> &Arc<PermissionStore> {
        &self.store
    }

    /// Get the form handler.
    pub fn form_handler(&self) -> &Arc<FormHandler> {
        &self.form_handler
    }

    /// Get the URL handler.
    pub fn url_handler(&self) -> &Arc<UrlHandler> {
        &self.url_handler
    }

    /// Check if the client supports elicitation at all.
    ///
    /// Returns false if:
    /// - Client capabilities haven't been received yet
    /// - Client didn't declare elicitation capability
    pub async fn client_supports_elicitation(&self) -> bool {
        if let Some(capabilities) = self.client_capabilities.read().await.as_ref() {
            capabilities.elicitation.as_ref().is_some()
        } else {
            // If we don't know the client's capabilities yet, assume no support
            false
        }
    }

    /// Check if the client supports the given elicitation mode.
    ///
    /// Note: MCP spec only has form mode for elicitation/create.
    /// URL mode is handled separately for OAuth flows.
    pub async fn client_supports_mode(&self, _mode: ElicitationMode) -> bool {
        self.client_supports_elicitation().await
    }

    /// Send an elicitation request to the connected client.
    ///
    /// This uses rmcp's `create_elicitation` to send a request via the MCP protocol.
    ///
    /// # Arguments
    /// * `message` - Human-readable message explaining what input is needed
    /// * `schema` - Type-safe schema defining the expected response structure
    ///
    /// # Returns
    /// * `Ok(CreateElicitationResult)` - The client's response (action + optional content)
    /// * `Err` - If no peer is connected or the request failed
    ///
    /// # Example
    /// ```ignore
    /// let schema = ElicitationSchema::builder()
    ///     .required_enum("action", vec!["allow_once".into(), "always_allow".into(), "deny".into()])
    ///     .build()?;
    ///
    /// let result = coordinator.create_elicitation(
    ///     "Allow tool execution?",
    ///     schema,
    /// ).await?;
    ///
    /// match result.action {
    ///     ElicitationAction::Accept => { /* handle content */ }
    ///     ElicitationAction::Decline => { /* user declined */ }
    ///     ElicitationAction::Cancel => { /* user cancelled */ }
    /// }
    /// ```
    pub async fn create_elicitation(
        &self,
        message: impl Into<String>,
        schema: ElicitationSchema,
    ) -> ElicitationResult<CreateElicitationResult> {
        self.create_elicitation_internal(message.into(), schema)
            .await
    }

    /// Forward a form-mode elicitation request from a downstream MCP service to the client.
    ///
    /// This wraps the message with provenance information so users know which
    /// service is requesting the information. This is critical for security -
    /// users should always know the source of an elicitation request.
    ///
    /// # Arguments
    /// * `message` - The original message from the downstream service
    /// * `schema` - Type-safe schema defining the expected response structure
    /// * `service_name` - Human-readable name of the service (shown to user)
    ///
    /// # Returns
    /// * `Ok(CreateElicitationResult)` - The client's response
    /// * `Err` - If no peer is connected or the request failed
    pub async fn forward_elicitation(
        &self,
        message: &str,
        schema: ElicitationSchema,
        service_name: &str,
    ) -> ElicitationResult<CreateElicitationResult> {
        let wrapped_message = wrap_with_provenance(message, service_name);
        self.create_elicitation_internal(wrapped_message, schema)
            .await
    }

    /// Forward a URL-mode elicitation request from a downstream MCP service.
    ///
    /// URL mode is used for OAuth flows and other sensitive interactions that
    /// require redirecting the user to an external URL. This wraps the request
    /// with provenance so users know which service is requesting authorization.
    ///
    /// # Arguments
    /// * `request` - The URL elicitation request from the downstream service
    /// * `service_name` - Human-readable name of the service (shown to user)
    /// * `service_id` - Unique identifier of the service
    ///
    /// # Returns
    /// The wrapped `UrlElicitationRequest` with provenance information added.
    /// The caller is responsible for delivering this to the client (URL mode
    /// is a custom extension, not part of the standard MCP elicitation spec).
    pub fn forward_url_elicitation(
        &self,
        request: UrlElicitationRequest,
        service_name: &str,
        service_id: &str,
    ) -> UrlElicitationRequest {
        wrap_url_with_provenance(request, service_name, service_id)
    }

    /// Complete a URL-mode elicitation (e.g., after OAuth callback).
    ///
    /// This should be called when the OAuth callback is received, indicating
    /// the user has completed the URL-mode authorization flow. It:
    /// 1. Consumes the OAuth state (preventing replay)
    /// 2. Logs the completion for audit purposes
    ///
    /// Note: URL mode doesn't use the standard MCP elicitation/create flow,
    /// so there's no response to send back. The service should instead check
    /// for the OAuth token/credentials that were stored during the callback.
    ///
    /// # Arguments
    /// * `elicitation_id` - The ID of the URL-mode elicitation that completed
    ///
    /// # Returns
    /// * `Ok(())` - If the elicitation was found and marked complete
    /// * `Err` - If the elicitation was not found or already consumed
    pub async fn complete_url_elicitation(&self, elicitation_id: &str) -> ElicitationResult<()> {
        tracing::info!(
            elicitation_id = elicitation_id,
            "URL-mode elicitation completed"
        );

        // Consume the OAuth state (marks it as used, prevents replay)
        self.url_handler.complete_oauth_flow(elicitation_id).await?;

        Ok(())
    }

    /// Handle an elicitation complete notification from a downstream service.
    ///
    /// When a downstream MCP service sends `notifications/elicitation/complete`,
    /// this method processes it and cleans up any associated state.
    ///
    /// # Arguments
    /// * `notification` - The complete notification with elicitation_id
    pub async fn handle_elicitation_complete(
        &self,
        notification: ElicitationCompleteNotification,
    ) -> ElicitationResult<()> {
        tracing::debug!(
            elicitation_id = notification.elicitation_id,
            "Received elicitation complete notification"
        );

        // Clean up the OAuth state if this was a URL-mode elicitation
        if let Err(e) = self
            .url_handler
            .complete_oauth_flow(&notification.elicitation_id)
            .await
        {
            // Log but don't fail - the elicitation may have already been consumed
            // or may have been a form-mode elicitation
            tracing::debug!(
                elicitation_id = notification.elicitation_id,
                error = %e,
                "Could not clean up elicitation state (may have been form-mode or already consumed)"
            );
        }

        Ok(())
    }

    /// Internal implementation for sending elicitation requests.
    async fn create_elicitation_internal(
        &self,
        message: impl Into<String>,
        schema: ElicitationSchema,
    ) -> ElicitationResult<CreateElicitationResult> {
        // Check if client supports elicitation
        if !self.client_supports_elicitation().await {
            return Err(ElicitationError::UnsupportedMode(
                "Client does not support elicitation".to_string(),
            ));
        }

        // Get the peer
        let peer_guard = self.peer.read().await;
        let peer = peer_guard
            .as_ref()
            .ok_or_else(|| ElicitationError::Internal("No peer connected".to_string()))?;

        // Create the request parameters
        let params = CreateElicitationRequestParams {
            message: message.into(),
            requested_schema: schema,
            meta: None,
        };

        // Send the elicitation request
        let result = peer.create_elicitation(params).await.map_err(|e| {
            ElicitationError::Internal(format!("Elicitation request failed: {:?}", e))
        })?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create an in-memory database for testing.
    async fn setup_test_db() -> surrealdb::Surreal<surrealdb::engine::any::Any> {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            ..Default::default()
        };
        crate::db::create_connection(db_config).await.unwrap()
    }

    #[test]
    fn test_elicitation_mode_as_str() {
        assert_eq!(ElicitationMode::Form.as_str(), "form");
        assert_eq!(ElicitationMode::Url.as_str(), "url");
    }

    #[test]
    fn test_elicitation_mode_from_str_valid() {
        assert_eq!(
            ElicitationMode::from_str("form"),
            Some(ElicitationMode::Form)
        );
        assert_eq!(ElicitationMode::from_str("url"), Some(ElicitationMode::Url));
    }

    #[test]
    fn test_elicitation_mode_from_str_invalid() {
        assert_eq!(ElicitationMode::from_str("invalid"), None);
        assert_eq!(ElicitationMode::from_str("FORM"), None); // case sensitive
        assert_eq!(ElicitationMode::from_str(""), None);
    }

    #[test]
    fn test_elicitation_mode_roundtrip() {
        // Ensure from_str(as_str()) returns the original value
        assert_eq!(
            ElicitationMode::from_str(ElicitationMode::Form.as_str()),
            Some(ElicitationMode::Form)
        );
        assert_eq!(
            ElicitationMode::from_str(ElicitationMode::Url.as_str()),
            Some(ElicitationMode::Url)
        );
    }

    #[test]
    fn test_fallback_policy_default_is_deny() {
        let policy = ElicitationFallbackPolicy::default();
        assert_eq!(policy, ElicitationFallbackPolicy::Deny);
    }

    #[test]
    fn test_fallback_policy_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&ElicitationFallbackPolicy::Deny).unwrap(),
            "\"deny\""
        );
        assert_eq!(
            serde_json::to_string(&ElicitationFallbackPolicy::Allow).unwrap(),
            "\"allow\""
        );
    }

    #[test]
    fn test_fallback_policy_deserializes_from_snake_case() {
        assert_eq!(
            serde_json::from_str::<ElicitationFallbackPolicy>("\"deny\"").unwrap(),
            ElicitationFallbackPolicy::Deny
        );
        assert_eq!(
            serde_json::from_str::<ElicitationFallbackPolicy>("\"allow\"").unwrap(),
            ElicitationFallbackPolicy::Allow
        );
    }

    #[test]
    fn test_fallback_policy_deserialize_invalid_fails() {
        let result = serde_json::from_str::<ElicitationFallbackPolicy>("\"invalid\"");
        assert!(result.is_err());
    }

    #[test]
    fn test_elicitation_action_serialization() {
        assert_eq!(
            serde_json::to_string(&ElicitationAction::Accept).unwrap(),
            "\"accept\""
        );
        assert_eq!(
            serde_json::to_string(&ElicitationAction::Decline).unwrap(),
            "\"decline\""
        );
        assert_eq!(
            serde_json::to_string(&ElicitationAction::Cancel).unwrap(),
            "\"cancel\""
        );
    }

    #[tokio::test]
    async fn test_coordinator_new_has_deny_policy_by_default() {
        let db = setup_test_db().await;
        let coordinator = ElicitationCoordinator::new(db).unwrap();

        assert_eq!(
            coordinator.fallback_policy().await,
            ElicitationFallbackPolicy::Deny
        );
    }

    #[tokio::test]
    async fn test_coordinator_new_with_policy_sets_policy() {
        let db = setup_test_db().await;
        let coordinator =
            ElicitationCoordinator::new_with_policy(db, ElicitationFallbackPolicy::Allow).unwrap();

        assert_eq!(
            coordinator.fallback_policy().await,
            ElicitationFallbackPolicy::Allow
        );
    }

    #[tokio::test]
    async fn test_coordinator_set_fallback_policy_changes_policy() {
        let db = setup_test_db().await;
        let coordinator = ElicitationCoordinator::new(db).unwrap();

        // Default is Deny
        assert_eq!(
            coordinator.fallback_policy().await,
            ElicitationFallbackPolicy::Deny
        );

        // Change to Allow
        coordinator
            .set_fallback_policy(ElicitationFallbackPolicy::Allow)
            .await;
        assert_eq!(
            coordinator.fallback_policy().await,
            ElicitationFallbackPolicy::Allow
        );

        // Change back to Deny
        coordinator
            .set_fallback_policy(ElicitationFallbackPolicy::Deny)
            .await;
        assert_eq!(
            coordinator.fallback_policy().await,
            ElicitationFallbackPolicy::Deny
        );
    }

    #[tokio::test]
    async fn test_coordinator_client_supports_elicitation_false_when_no_capabilities() {
        let db = setup_test_db().await;
        let coordinator = ElicitationCoordinator::new(db).unwrap();

        // Before initialize, client capabilities are unknown
        // Should return false (fail closed for security)
        assert!(!coordinator.client_supports_elicitation().await);
    }

    #[tokio::test]
    async fn test_coordinator_client_supports_elicitation_true_when_capability_set() {
        let db = setup_test_db().await;
        let coordinator = ElicitationCoordinator::new(db).unwrap();

        // Set capabilities with elicitation enabled
        let capabilities = ClientCapabilities::builder().enable_elicitation().build();
        coordinator.set_client_capabilities(&capabilities).await;

        assert!(coordinator.client_supports_elicitation().await);
    }

    #[tokio::test]
    async fn test_coordinator_client_supports_elicitation_false_when_capability_not_set() {
        let db = setup_test_db().await;
        let coordinator = ElicitationCoordinator::new(db).unwrap();

        // Set capabilities WITHOUT elicitation
        let capabilities = ClientCapabilities::builder().build();
        coordinator.set_client_capabilities(&capabilities).await;

        assert!(!coordinator.client_supports_elicitation().await);
    }

    #[tokio::test]
    async fn test_coordinator_approval_manager_accessible() {
        let db = setup_test_db().await;
        let coordinator = ElicitationCoordinator::new(db).unwrap();

        // Should be able to get the approval manager
        let manager = coordinator.approval_manager();
        assert!(Arc::strong_count(manager) >= 1);
    }

    #[tokio::test]
    async fn test_coordinator_store_accessible() {
        let db = setup_test_db().await;
        let coordinator = ElicitationCoordinator::new(db).unwrap();

        // Should be able to get the store
        let store = coordinator.store();
        assert!(Arc::strong_count(store) >= 1);
    }

    #[test]
    fn test_url_elicitation_request_serialization() {
        let request = UrlElicitationRequest {
            message: "Please authorize".to_string(),
            url: OAuthUrl::new("https://github.com/oauth/authorize"),
            elicitation_id: "elic-123".to_string(),
            service_name: Some(ServiceName::new("GitHub")),
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["message"], "Please authorize");
        assert_eq!(json["url"], "https://github.com/oauth/authorize");
        assert_eq!(json["elicitation_id"], "elic-123");
        assert_eq!(json["service_name"], "GitHub");
    }

    #[test]
    fn test_url_elicitation_request_service_name_optional() {
        let request = UrlElicitationRequest {
            message: "Please authorize".to_string(),
            url: OAuthUrl::new("https://github.com/oauth/authorize"),
            elicitation_id: "elic-123".to_string(),
            service_name: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        // service_name should be omitted when None (skip_serializing_if)
        assert!(!json.contains("service_name"));
    }

    #[test]
    fn test_elicitation_complete_notification_serialization() {
        let notification = ElicitationCompleteNotification {
            elicitation_id: "elic-456".to_string(),
        };

        let json = serde_json::to_value(&notification).unwrap();
        assert_eq!(json["elicitation_id"], "elic-456");
    }

    #[test]
    fn test_elicitation_complete_notification_deserialization() {
        let json = r#"{"elicitation_id": "elic-789"}"#;
        let notification: ElicitationCompleteNotification = serde_json::from_str(json).unwrap();

        assert_eq!(notification.elicitation_id, "elic-789");
    }
}
