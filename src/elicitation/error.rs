//! Error types for elicitation operations.
//!
//! Includes MCP-spec error codes:
//! - `-32042`: URL elicitation required (server needs OAuth/external auth)

use std::fmt;

/// MCP error code for URL elicitation required.
///
/// Per MCP spec 2025-11-25, servers return this error when they need the client
/// to complete a URL-mode elicitation (e.g., OAuth authorization flow) before
/// they can proceed with the request.
///
/// The error data should include the URL the user needs to visit.
pub const URL_ELICITATION_REQUIRED_ERROR_CODE: i32 = -32042;

/// Errors that can occur during elicitation operations.
#[derive(Debug, Clone)]
pub enum ElicitationError {
    /// The requested elicitation mode is not supported by the client.
    UnsupportedMode(String),

    /// The elicitation request has an invalid schema.
    InvalidSchema(String),

    /// The elicitation request is missing required fields.
    MissingField(String),

    /// The user declined the elicitation request.
    Declined,

    /// The user canceled the elicitation request.
    Canceled,

    /// The elicitation has expired.
    Expired,

    /// The elicitation ID was not found.
    NotFound(String),

    /// URL elicitation is required before proceeding.
    ///
    /// This maps to MCP error code -32042. The server needs the user
    /// to complete an OAuth or external authorization flow.
    UrlElicitationRequired {
        /// Human-readable message explaining what authorization is needed
        message: String,
        /// The URL the user should visit to complete authorization
        url: String,
        /// Provider name (e.g., "github", "google")
        provider: String,
    },

    /// Database error occurred.
    Database(String),

    /// Internal error occurred.
    Internal(String),
}

impl fmt::Display for ElicitationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMode(mode) => write!(f, "Unsupported elicitation mode: {}", mode),
            Self::InvalidSchema(msg) => write!(f, "Invalid schema: {}", msg),
            Self::MissingField(field) => write!(f, "Missing required field: {}", field),
            Self::Declined => write!(f, "User declined the elicitation request"),
            Self::Canceled => write!(f, "User canceled the elicitation request"),
            Self::Expired => write!(f, "The elicitation has expired"),
            Self::NotFound(id) => write!(f, "Elicitation not found: {}", id),
            Self::UrlElicitationRequired {
                message, provider, ..
            } => {
                write!(f, "URL elicitation required for {}: {}", provider, message)
            }
            Self::Database(msg) => write!(f, "Database error: {}", msg),
            Self::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for ElicitationError {}

/// Result type for elicitation operations.
pub type ElicitationResult<T> = Result<T, ElicitationError>;

impl From<anyhow::Error> for ElicitationError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

impl ElicitationError {
    /// Convert this error to an MCP ErrorData for protocol responses.
    ///
    /// This is particularly useful for the `UrlElicitationRequired` variant,
    /// which uses the special `-32042` error code defined in the MCP spec.
    pub fn to_mcp_error(&self) -> rmcp::ErrorData {
        use rmcp::model::ErrorCode;

        match self {
            Self::UrlElicitationRequired {
                message,
                url,
                provider,
            } => {
                // Create error data with the URL for the client
                let data = serde_json::json!({
                    "url": url,
                    "provider": provider,
                });
                rmcp::ErrorData::new(
                    ErrorCode(URL_ELICITATION_REQUIRED_ERROR_CODE),
                    message.clone(),
                    Some(data),
                )
            }
            Self::UnsupportedMode(msg) => rmcp::ErrorData::invalid_params(msg.clone(), None),
            Self::InvalidSchema(msg) => {
                rmcp::ErrorData::invalid_params(format!("Invalid schema: {}", msg), None)
            }
            Self::MissingField(field) => {
                rmcp::ErrorData::invalid_params(format!("Missing field: {}", field), None)
            }
            Self::Declined => rmcp::ErrorData::new(
                ErrorCode(-32001),
                "User declined the request".to_string(),
                None,
            ),
            Self::Canceled => rmcp::ErrorData::new(
                ErrorCode(-32001),
                "User canceled the request".to_string(),
                None,
            ),
            Self::Expired => {
                rmcp::ErrorData::new(ErrorCode(-32001), "Elicitation expired".to_string(), None)
            }
            Self::NotFound(id) => rmcp::ErrorData::new(
                ErrorCode(-32002),
                format!("Elicitation not found: {}", id),
                None,
            ),
            Self::Database(msg) | Self::Internal(msg) => {
                rmcp::ErrorData::internal_error(msg.clone(), None)
            }
        }
    }

    /// Create a URL elicitation required error.
    ///
    /// Use this when a tool or operation requires OAuth authorization
    /// before it can proceed.
    pub fn url_elicitation_required(
        message: impl Into<String>,
        url: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self::UrlElicitationRequired {
            message: message.into(),
            url: url.into(),
            provider: provider.into(),
        }
    }
}
