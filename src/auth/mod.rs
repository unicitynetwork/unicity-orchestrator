//! Authentication and user context module.
//!
//! This module handles user identity extraction and management for multi-tenant
//! operation. It supports multiple authentication methods:
//!
//! - **JWT**: Extract user from Bearer token (for hosted deployments)
//! - **API Key**: Extract user from X-API-Key header
//! - **Anonymous**: Single-user mode for local deployments
//!
//! ## Security Model
//!
//! - User identity is extracted at the HTTP layer before MCP processing
//! - All database operations are scoped by user_id
//! - Audit logs track all security-sensitive operations
//! - OAuth state and permissions are per-user
//!
//! ## Usage
//!
//! ```ignore
//! // Extract user from HTTP request
//! let user_context = UserContext::from_request(&headers, &db).await?;
//!
//! // Use in elicitation
//! let result = elicitation_coordinator.create_elicitation_for_user(
//!     &user_context,
//!     message,
//!     schema,
//! ).await?;
//! ```

mod context;
mod extractor;
pub mod jwks;
mod user_store;

pub use context::UserContext;
pub use extractor::{AuthConfig, AuthError, AuthExtractor, generate_api_key, hash_api_key};
pub use jwks::{DEFAULT_CACHE_TTL_SECONDS, JwksCache, JwksCacheError};
pub use user_store::UserStore;
