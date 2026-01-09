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

pub use approval::{ApprovalManager, ToolPermission};
pub use error::{ElicitationError, ElicitationResult, URL_ELICITATION_REQUIRED_ERROR_CODE};
pub use form::FormHandler;
pub use provenance::{wrap_with_provenance, wrap_url_with_provenance};
pub use store::PermissionStore;
pub use url::UrlHandler;

// Re-export rmcp elicitation types for external use
pub use rmcp::model::{
    ElicitationAction, ElicitationSchema,
    CreateElicitationRequestParam, CreateElicitationResult,
    StringFormat,
    PrimitiveSchema,
};

use rmcp::model::ClientCapabilities;
use rmcp::service::{Peer, RoleServer};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;

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
    pub url: String,

    /// Unique identifier for the elicitation (for callback matching)
    pub elicitation_id: String,

    /// Optional service name for provenance tracking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
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
        })
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
        self.create_elicitation_internal(message.into(), schema).await
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
        self.create_elicitation_internal(wrapped_message, schema).await
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
    pub async fn complete_url_elicitation(
        &self,
        elicitation_id: &str,
    ) -> ElicitationResult<()> {
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
        if let Err(e) = self.url_handler.complete_oauth_flow(&notification.elicitation_id).await {
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
            return Err(ElicitationError::UnsupportedMode("Client does not support elicitation".to_string()));
        }

        // Get the peer
        let peer_guard = self.peer.read().await;
        let peer = peer_guard.as_ref()
            .ok_or_else(|| ElicitationError::Internal("No peer connected".to_string()))?;

        // Create the request parameters
        let params = CreateElicitationRequestParam {
            message: message.into(),
            requested_schema: schema,
        };

        // Send the elicitation request
        let result = peer.create_elicitation(params)
            .await
            .map_err(|e| ElicitationError::Internal(format!("Elicitation request failed: {:?}", e)))?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elicitation_mode_conversion() {
        assert_eq!(ElicitationMode::Form.as_str(), "form");
        assert_eq!(ElicitationMode::Url.as_str(), "url");
        assert_eq!(ElicitationMode::from_str("form"), Some(ElicitationMode::Form));
        assert_eq!(ElicitationMode::from_str("url"), Some(ElicitationMode::Url));
        assert_eq!(ElicitationMode::from_str("invalid"), None);
    }

    #[test]
    fn test_elicitation_action_serialization() {
        // Test that actions serialize to lowercase
        let accept = serde_json::to_string(&ElicitationAction::Accept).unwrap();
        assert_eq!(accept, "\"accept\"");

        let decline = serde_json::to_string(&ElicitationAction::Decline).unwrap();
        assert_eq!(decline, "\"decline\"");

        let cancel = serde_json::to_string(&ElicitationAction::Cancel).unwrap();
        assert_eq!(cancel, "\"cancel\"");
    }

    #[tokio::test]
    async fn test_client_supports_elicitation_when_no_capabilities() {
        // Create coordinator with in-memory database
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        let coordinator = ElicitationCoordinator::new(db).unwrap();

        // Before initialize, client capabilities are unknown
        // Should return false, not true (fail closed)
        assert!(!coordinator.client_supports_elicitation().await);
        assert!(!coordinator.client_supports_mode(ElicitationMode::Form).await);
        assert!(!coordinator.client_supports_mode(ElicitationMode::Url).await);
    }
}
