//! NewType wrappers for strong typing throughout the orchestrator.
//!
//! These types prevent accidental mixing of semantically different strings
//! (e.g., passing a tool name where a tool ID is expected).

use serde::{Deserialize, Serialize};
use std::fmt;

/// Macro to generate a NewType wrapper with standard trait implementations.
macro_rules! newtype_string {
    (
        $(#[$meta:meta])*
        $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Create a new instance.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Get the inner value as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume and return the inner String.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

newtype_string!(
    /// Database identifier for a tool record (e.g., "tool:abc123").
    ///
    /// This is the stable ID used to reference tools in the database and
    /// in API responses. It is distinct from `ToolName` which is the
    /// human-readable name from the MCP manifest.
    ToolId
);

newtype_string!(
    /// Tool name as defined in the MCP manifest.
    ///
    /// This is the human-readable identifier that users and LLMs interact with.
    /// Multiple tools may have the same name across different services, but
    /// each is uniquely identified by its `ToolId`.
    ToolName
);

newtype_string!(
    /// Database identifier for a service record (e.g., "service:xyz789").
    ///
    /// This is the stable ID used to reference services in the database.
    /// It is distinct from `ServiceName` which is human-readable.
    ServiceId
);

newtype_string!(
    /// External user identifier from the authentication provider.
    ///
    /// This might be a JWT `sub` claim, an API key hash, or "anonymous"
    /// for local single-user mode. It is used to scope permissions and
    /// audit logs to specific users.
    ExternalUserId
);

newtype_string!(
    /// Identity provider that authenticated the user.
    ///
    /// Common values: "jwt", "api_key", "anonymous".
    /// Used to determine how to validate and refresh credentials.
    IdentityProvider
);

newtype_string!(
    /// Service ID from the mcp.json configuration file.
    ///
    /// This is the key used in the `mcpServers` object, e.g., "github",
    /// "filesystem". It may differ from the `ServiceId` which is the
    /// database record ID.
    ServiceConfigId
);

newtype_string!(
    /// A valid resource URI with scheme (e.g., "file:///path/to/file").
    ///
    /// URIs must contain a scheme like "file://", "https://", etc.
    /// They are validated to prevent path traversal and injection attacks.
    ResourceUri
);

newtype_string!(
    /// Prompt identifier for MCP prompts.
    ///
    /// This is the name used to reference prompts, which may be namespaced
    /// with the service name (e.g., "github-commit") to avoid conflicts.
    PromptName
);

newtype_string!(
    /// Human-readable service name for display purposes.
    ///
    /// This is the friendly name shown to users, e.g., "GitHub", "Filesystem".
    /// It may differ from the `ServiceConfigId` and `ServiceId`.
    ServiceName
);

newtype_string!(
    /// OAuth endpoint URL for authentication flows.
    ///
    /// Used for initiating OAuth authorization, typically pointing to
    /// a provider's authorization endpoint.
    OAuthUrl
);

newtype_string!(
    /// OAuth redirect URI for callback handling.
    ///
    /// The URI where the OAuth provider redirects after authorization.
    /// Must match what's configured with the OAuth provider.
    RedirectUri
);

newtype_string!(
    /// SHA-256 hash of an API key for secure storage and lookup.
    ///
    /// API keys are never stored in plain text. Instead, they are hashed
    /// using SHA-256 and stored/compared using this hash. The hash is
    /// computed once when the key is created or received.
    ApiKeyHash
);

newtype_string!(
    /// Display prefix of an API key (e.g., "uo_abc12345").
    ///
    /// The prefix is the first part of an API key that can be safely
    /// displayed to users for identification purposes. It does not
    /// reveal the full key and cannot be used for authentication.
    ApiKeyPrefix
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_id_creation() {
        let id = ToolId::new("tool:abc123");
        assert_eq!(id.as_str(), "tool:abc123");
        assert_eq!(id.to_string(), "tool:abc123");
    }

    #[test]
    fn test_tool_id_from_string() {
        let id: ToolId = "tool:abc123".into();
        assert_eq!(id.as_str(), "tool:abc123");

        let id: ToolId = String::from("tool:xyz789").into();
        assert_eq!(id.as_str(), "tool:xyz789");
    }

    #[test]
    fn test_tool_id_into_inner() {
        let id = ToolId::new("tool:abc123");
        let inner: String = id.into_inner();
        assert_eq!(inner, "tool:abc123");
    }

    #[test]
    fn test_tool_id_serde() {
        let id = ToolId::new("tool:abc123");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"tool:abc123\"");

        let parsed: ToolId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_tool_name_creation() {
        let name = ToolName::new("read_file");
        assert_eq!(name.as_str(), "read_file");
    }

    #[test]
    fn test_service_id_creation() {
        let id = ServiceId::new("service:github");
        assert_eq!(id.as_str(), "service:github");
    }

    #[test]
    fn test_external_user_id_creation() {
        let id = ExternalUserId::new("user|12345");
        assert_eq!(id.as_str(), "user|12345");
    }

    #[test]
    fn test_identity_provider_creation() {
        let provider = IdentityProvider::new("jwt");
        assert_eq!(provider.as_str(), "jwt");
    }

    #[test]
    fn test_service_config_id_creation() {
        let id = ServiceConfigId::new("filesystem");
        assert_eq!(id.as_str(), "filesystem");
    }

    #[test]
    fn test_resource_uri_creation() {
        let uri = ResourceUri::new("file:///path/to/file.txt");
        assert_eq!(uri.as_str(), "file:///path/to/file.txt");
    }

    #[test]
    fn test_prompt_name_creation() {
        let name = PromptName::new("github-commit");
        assert_eq!(name.as_str(), "github-commit");
    }

    #[test]
    fn test_service_name_creation() {
        let name = ServiceName::new("GitHub");
        assert_eq!(name.as_str(), "GitHub");
    }

    #[test]
    fn test_oauth_url_creation() {
        let url = OAuthUrl::new("https://github.com/login/oauth/authorize");
        assert_eq!(url.as_str(), "https://github.com/login/oauth/authorize");
    }

    #[test]
    fn test_redirect_uri_creation() {
        let uri = RedirectUri::new("http://localhost:8080/oauth/callback");
        assert_eq!(uri.as_str(), "http://localhost:8080/oauth/callback");
    }

    #[test]
    fn test_type_equality() {
        let id1 = ToolId::new("tool:abc");
        let id2 = ToolId::new("tool:abc");
        let id3 = ToolId::new("tool:xyz");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_type_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(ToolId::new("tool:abc"));
        set.insert(ToolId::new("tool:xyz"));

        assert!(set.contains(&ToolId::new("tool:abc")));
        assert!(!set.contains(&ToolId::new("tool:123")));
    }

    #[test]
    fn test_as_ref() {
        let id = ToolId::new("tool:abc");
        let s: &str = id.as_ref();
        assert_eq!(s, "tool:abc");
    }

    #[test]
    fn test_borrow() {
        use std::borrow::Borrow;
        let id = ToolId::new("tool:abc");
        let s: &str = id.borrow();
        assert_eq!(s, "tool:abc");
    }

    #[test]
    fn test_api_key_hash_creation() {
        let hash = ApiKeyHash::new("a1b2c3d4e5f6...");
        assert_eq!(hash.as_str(), "a1b2c3d4e5f6...");
    }

    #[test]
    fn test_api_key_prefix_creation() {
        let prefix = ApiKeyPrefix::new("uo_abc12345");
        assert_eq!(prefix.as_str(), "uo_abc12345");
    }
}
