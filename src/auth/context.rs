//! User context for request-scoped identity.

use crate::types::{ExternalUserId, IdentityProvider};
use serde::{Deserialize, Serialize};
use surrealdb::RecordId;

/// User context extracted from the HTTP request.
///
/// This struct is passed through the request handling chain to provide
/// user identity for all operations. It is immutable once created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserContext {
    /// Database record ID for this user
    user_id: RecordId,
    /// External identity (e.g., JWT sub claim, API key hash)
    external_id: ExternalUserId,
    /// Identity provider that authenticated this user
    provider: IdentityProvider,
    /// Optional email for display
    email: Option<String>,
    /// Optional display name
    display_name: Option<String>,
    /// Whether this is an anonymous/local user
    is_anonymous: bool,
    /// Client IP address (for audit logging)
    ip_address: Option<String>,
    /// Client user agent (for audit logging)
    user_agent: Option<String>,
}

impl UserContext {
    /// Create a new user context.
    pub fn new(
        user_id: RecordId,
        external_id: ExternalUserId,
        provider: IdentityProvider,
        email: Option<String>,
        display_name: Option<String>,
    ) -> Self {
        let is_anonymous = provider.as_str() == "anonymous";
        Self {
            user_id,
            external_id,
            provider,
            email,
            display_name,
            is_anonymous,
            ip_address: None,
            user_agent: None,
        }
    }

    /// Create an anonymous user context for local/single-user mode.
    pub fn anonymous(user_id: RecordId) -> Self {
        Self {
            user_id,
            external_id: ExternalUserId::new("anonymous"),
            provider: IdentityProvider::new("anonymous"),
            email: None,
            display_name: Some("Local User".to_string()),
            is_anonymous: true,
            ip_address: None,
            user_agent: None,
        }
    }

    /// Set client metadata for audit logging.
    pub fn with_client_info(
        mut self,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        self.ip_address = ip_address;
        self.user_agent = user_agent;
        self
    }

    /// Get the database user ID.
    pub fn user_id(&self) -> &RecordId {
        &self.user_id
    }

    /// Get the user ID as a string for use in queries.
    pub fn user_id_string(&self) -> String {
        self.user_id.to_string()
    }

    /// Get the external identity.
    pub fn external_id(&self) -> &ExternalUserId {
        &self.external_id
    }

    /// Get the identity provider.
    pub fn provider(&self) -> &IdentityProvider {
        &self.provider
    }

    /// Get the email if available.
    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    /// Get the display name.
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// Check if this is an anonymous user.
    pub fn is_anonymous(&self) -> bool {
        self.is_anonymous
    }

    /// Get the client IP address for audit logging.
    pub fn ip_address(&self) -> Option<&str> {
        self.ip_address.as_deref()
    }

    /// Get the client user agent for audit logging.
    pub fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }

    /// Get a display-friendly name for this user.
    pub fn display(&self) -> String {
        if let Some(name) = &self.display_name {
            name.clone()
        } else if let Some(email) = &self.email {
            email.clone()
        } else if self.is_anonymous {
            "Anonymous".to_string()
        } else {
            self.external_id.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_user_id() -> RecordId {
        RecordId::from_table_key("user", "test123")
    }

    #[test]
    fn test_user_context_new() {
        let ctx = UserContext::new(
            test_user_id(),
            ExternalUserId::new("sub123"),
            IdentityProvider::new("jwt"),
            Some("user@example.com".to_string()),
            Some("Test User".to_string()),
        );

        assert_eq!(ctx.external_id().as_str(), "sub123");
        assert_eq!(ctx.provider().as_str(), "jwt");
        assert_eq!(ctx.email(), Some("user@example.com"));
        assert_eq!(ctx.display_name(), Some("Test User"));
        assert!(!ctx.is_anonymous());
    }

    #[test]
    fn test_user_context_anonymous() {
        let ctx = UserContext::anonymous(test_user_id());

        assert_eq!(ctx.external_id().as_str(), "anonymous");
        assert_eq!(ctx.provider().as_str(), "anonymous");
        assert!(ctx.is_anonymous());
        assert_eq!(ctx.display(), "Local User");
    }

    #[test]
    fn test_user_context_with_client_info() {
        let ctx = UserContext::anonymous(test_user_id()).with_client_info(
            Some("192.168.1.1".to_string()),
            Some("Mozilla/5.0".to_string()),
        );

        assert_eq!(ctx.ip_address(), Some("192.168.1.1"));
        assert_eq!(ctx.user_agent(), Some("Mozilla/5.0"));
    }

    #[test]
    fn test_user_context_display() {
        // With display name
        let ctx1 = UserContext::new(
            test_user_id(),
            ExternalUserId::new("sub123"),
            IdentityProvider::new("jwt"),
            Some("user@example.com".to_string()),
            Some("Test User".to_string()),
        );
        assert_eq!(ctx1.display(), "Test User");

        // With email only
        let ctx2 = UserContext::new(
            test_user_id(),
            ExternalUserId::new("sub123"),
            IdentityProvider::new("jwt"),
            Some("user@example.com".to_string()),
            None,
        );
        assert_eq!(ctx2.display(), "user@example.com");

        // With external_id only
        let ctx3 = UserContext::new(
            test_user_id(),
            ExternalUserId::new("sub123"),
            IdentityProvider::new("jwt"),
            None,
            None,
        );
        assert_eq!(ctx3.display(), "sub123");
    }

    #[test]
    fn test_user_id_string() {
        let ctx = UserContext::anonymous(test_user_id());
        let id_str = ctx.user_id_string();
        assert!(id_str.contains("user"));
        assert!(id_str.contains("test123"));
    }
}
