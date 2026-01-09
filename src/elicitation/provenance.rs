//! Provenance wrapper for downstream service elicitation requests.
//!
//! When MCP services send elicitation requests, the orchestrator wraps them
//! with service context so users know which service is requesting information.
//!
//! This is important for security and UX - users should always know which
//! service is asking for information before approving.

use crate::elicitation::UrlElicitationRequest;

/// Wrap an elicitation request with service provenance.
///
/// This adds context about which service is making the request so users
/// can make informed decisions about whether to approve.
pub struct ProvenanceWrapper;

impl ProvenanceWrapper {
    /// Wrap a message with service context for form-mode elicitation.
    ///
    /// This prepends service information to the message that will be shown
    /// to the user in the MCP client.
    ///
    /// # Arguments
    /// * `message` - The original message from a downstream service
    /// * `service_name` - The human-readable name of the service
    ///
    /// # Returns
    /// A new message with added provenance information
    pub fn wrap_message(&self, message: &str, service_name: &str) -> String {
        format!("[{}] {}", service_name, message)
    }

    /// Wrap a URL elicitation request with service context.
    ///
    /// # Arguments
    /// * `request` - The original URL elicitation request from a downstream service
    /// * `service_name` - The human-readable name of the service
    /// * `service_id` - The unique identifier of the service
    ///
    /// # Returns
    /// A new URL elicitation request with added provenance information
    pub fn wrap_url_request(
        &self,
        request: UrlElicitationRequest,
        service_name: &str,
        service_id: &str,
    ) -> UrlElicitationRequest {
        let wrapped_message = self.wrap_message(&request.message, service_name);

        tracing::info!(
            "URL mode elicitation from service '{}' ({}) to {}",
            service_name, service_id, request.url
        );

        UrlElicitationRequest {
            message: wrapped_message,
            url: request.url,
            elicitation_id: request.elicitation_id,
            service_name: Some(service_name.to_string()),
        }
    }
}

/// Convenience function to wrap a message with provenance.
pub fn wrap_with_provenance(message: &str, service_name: &str) -> String {
    ProvenanceWrapper.wrap_message(message, service_name)
}

/// Convenience function to wrap a URL elicitation request with provenance.
pub fn wrap_url_with_provenance(
    request: UrlElicitationRequest,
    service_name: &str,
    service_id: &str,
) -> UrlElicitationRequest {
    ProvenanceWrapper.wrap_url_request(request, service_name, service_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_message() {
        let wrapper = ProvenanceWrapper;

        let original = "Please enter your API key";
        let wrapped = wrapper.wrap_message(original, "github");

        assert!(wrapped.starts_with("[github]"));
        assert!(wrapped.contains("Please enter your API key"));
    }

    #[test]
    fn test_wrap_url_elicitation() {
        let wrapper = ProvenanceWrapper;

        let original = UrlElicitationRequest {
            message: "Authorization required".to_string(),
            url: "https://github.com/login/oauth".to_string(),
            elicitation_id: "elicitation-123".to_string(),
            service_name: None,
        };

        let wrapped = wrapper.wrap_url_request(original, "github", "service:github");

        assert!(wrapped.message.starts_with("[github]"));
        assert_eq!(wrapped.url, "https://github.com/login/oauth");
        assert_eq!(wrapped.service_name, Some("github".to_string()));
    }

    #[test]
    fn test_wrap_with_provenance_helper() {
        let message = "Enter your username";
        let wrapped = wrap_with_provenance(message, "filesystem");

        assert!(wrapped.starts_with("[filesystem]"));
        assert!(wrapped.contains("Enter your username"));
    }

    #[test]
    fn test_wrap_url_with_provenance_helper() {
        let request = UrlElicitationRequest {
            message: "Please authorize".to_string(),
            url: "https://example.com/auth".to_string(),
            elicitation_id: "test-id".to_string(),
            service_name: None,
        };

        let wrapped = wrap_url_with_provenance(request, "github", "service:github");

        assert!(wrapped.message.starts_with("[github]"));
        assert_eq!(wrapped.service_name, Some("github".to_string()));
    }
}
