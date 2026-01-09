//! Permission storage for elicitation and tool approval.
//!
//! This module handles persistent storage of:
//! - Tool approval permissions
//! - OAuth state for URL mode elicitations
//! - User preferences for elicitation

use crate::elicitation::{ElicitationError, ElicitationResult, ToolPermission};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Permission store for persistent storage of elicitation-related data.
#[derive(Clone)]
pub struct PermissionStore {
    db: Surreal<Any>,
    /// In-memory cache for OAuth state (for security - don't persist to DB)
    oauth_state: Arc<Mutex<std::collections::HashMap<String, OAuthEntry>>>,
}

/// OAuth state entry for URL mode elicitation.
#[derive(Clone, Debug)]
struct OAuthEntry {
    user_id: String,
    provider: String,
    state_token: String,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
    redirect_uri: String,
}

impl PermissionStore {
    /// Create a new permission store.
    pub fn new(db: Surreal<Any>) -> Self {
        Self {
            db,
            oauth_state: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Save a tool permission.
    pub async fn save_permission(&self, permission: &ToolPermission) -> ElicitationResult<ToolPermission> {
        let query = r#"
            CREATE permission CONTENT {
                tool_id: $tool_id,
                service_id: $service_id,
                user_id: $user_id,
                action: $action,
                created_at: $created_at,
                expires_at: $expires_at
            }
        "#;

        let mut res = self.db
            .query(query)
            .bind(("tool_id", permission.tool_id.clone()))
            .bind(("service_id", permission.service_id.clone()))
            .bind(("user_id", permission.user_id.clone()))
            .bind(("action", serde_json::to_string(&permission.action).unwrap()))
            .bind(("created_at", permission.created_at.clone()))
            .bind(("expires_at", permission.expires_at.clone()))
            .await
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        #[derive(serde::Deserialize)]
        struct Created {
            id: Option<String>,
        }

        let created: Vec<Created> = res.take(0)
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        let mut result = permission.clone();
        if let Some(record) = created.first() {
            result.id = record.id.clone();
        }

        Ok(result)
    }

    /// Get a permission for a specific tool, service, and user.
    pub async fn get_permission(
        &self,
        tool_id: &str,
        service_id: &str,
        user_id: &str,
    ) -> ElicitationResult<Option<ToolPermission>> {
        let tool_id = tool_id.to_string();
        let service_id = service_id.to_string();
        let user_id = user_id.to_string();

        let query = r#"
            SELECT * FROM permission
            WHERE tool_id = $tool_id
              AND service_id = $service_id
              AND user_id = $user_id
            ORDER BY created_at DESC
            LIMIT 1
        "#;

        let mut res = self.db
            .query(query)
            .bind(("tool_id", tool_id))
            .bind(("service_id", service_id))
            .bind(("user_id", user_id))
            .await
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        let result: Vec<ToolPermission> = res.take(0)
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        Ok(result.into_iter().next())
    }

    /// Delete a specific permission.
    pub async fn delete_permission(
        &self,
        tool_id: &str,
        service_id: &str,
        user_id: &str,
    ) -> ElicitationResult<()> {
        let tool_id = tool_id.to_string();
        let service_id = service_id.to_string();
        let user_id = user_id.to_string();

        let query = r#"
            DELETE permission
            WHERE tool_id = $tool_id
              AND service_id = $service_id
              AND user_id = $user_id
        "#;

        self.db
            .query(query)
            .bind(("tool_id", tool_id))
            .bind(("service_id", service_id))
            .bind(("user_id", user_id))
            .await
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        Ok(())
    }

    /// Delete all permissions for a specific tool and user.
    pub async fn delete_tool_permissions(
        &self,
        tool_id: &str,
        user_id: &str,
    ) -> ElicitationResult<()> {
        let tool_id = tool_id.to_string();
        let user_id = user_id.to_string();

        let query = r#"
            DELETE permission
            WHERE tool_id = $tool_id
              AND user_id = $user_id
        "#;

        self.db
            .query(query)
            .bind(("tool_id", tool_id))
            .bind(("user_id", user_id))
            .await
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        Ok(())
    }

    /// Delete all permissions for a specific service and user.
    pub async fn delete_service_permissions(
        &self,
        service_id: &str,
        user_id: &str,
    ) -> ElicitationResult<()> {
        let service_id = service_id.to_string();
        let user_id = user_id.to_string();

        let query = r#"
            DELETE permission
            WHERE service_id = $service_id
              AND user_id = $user_id
        "#;

        self.db
            .query(query)
            .bind(("service_id", service_id))
            .bind(("user_id", user_id))
            .await
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        Ok(())
    }

    /// List all permissions for a user.
    pub async fn list_user_permissions(
        &self,
        user_id: &str,
    ) -> ElicitationResult<Vec<ToolPermission>> {
        let user_id = user_id.to_string();

        let query = r#"
            SELECT * FROM permission
            WHERE user_id = $user_id
            ORDER BY created_at DESC
        "#;

        let mut res = self.db
            .query(query)
            .bind(("user_id", user_id))
            .await
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        let permissions: Vec<ToolPermission> = res.take(0)
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        Ok(permissions)
    }

    /// Clean up expired permissions.
    pub async fn cleanup_expired_permissions(&self) -> ElicitationResult<usize> {
        let query = r#"
            DELETE permission
            WHERE expires_at IS NOT NULL
              AND expires_at < time::now()
        "#;

        let mut res = self.db
            .query(query)
            .await
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        // The DELETE statement returns the number of deleted records
        let count: Vec<serde_json::Value> = res.take(0)
            .map_err(|e| ElicitationError::Database(e.to_string()))?;

        Ok(count.len())
    }
}

// ============================================================================
// OAuth State Management (in-memory only for security)
// ============================================================================

/// OAuth state for URL mode elicitation.
#[derive(Clone, Debug)]
pub struct OAuthState {
    /// Unique elicitation ID
    pub elicitation_id: String,
    /// User ID
    pub user_id: String,
    /// OAuth provider (e.g., "github", "google")
    pub provider: String,
    /// State token for CSRF protection
    pub state_token: String,
    /// Redirect URI after OAuth completes
    pub redirect_uri: String,
    /// When this state expires
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl PermissionStore {
    /// Store OAuth state for a URL mode elicitation.
    pub async fn store_oauth_state(&self, state: OAuthState) -> ElicitationResult<()> {
        let entry = OAuthEntry {
            user_id: state.user_id.clone(),
            provider: state.provider.clone(),
            state_token: state.state_token.clone(),
            created_at: chrono::Utc::now(),
            expires_at: state.expires_at,
            redirect_uri: state.redirect_uri,
        };

        let mut state_map = self.oauth_state.lock().await;
        state_map.insert(state.elicitation_id.clone(), entry);

        // Schedule cleanup of expired entries (simplified - in production would use a proper scheduler)
        // For now, we'll clean up when retrieving

        Ok(())
    }

    /// Retrieve and validate OAuth state.
    pub async fn get_oauth_state(&self, elicitation_id: &str) -> ElicitationResult<Option<OAuthState>> {
        let mut state_map = self.oauth_state.lock().await;

        if let Some(entry) = state_map.get(elicitation_id) {
            // Check expiration
            if chrono::Utc::now() > entry.expires_at {
                state_map.remove(elicitation_id);
                return Ok(None);
            }

            return Ok(Some(OAuthState {
                elicitation_id: elicitation_id.to_string(),
                user_id: entry.user_id.clone(),
                provider: entry.provider.clone(),
                state_token: entry.state_token.clone(),
                redirect_uri: entry.redirect_uri.clone(),
                expires_at: entry.expires_at,
            }));
        }

        Ok(None)
    }

    /// Consume OAuth state after use.
    pub async fn consume_oauth_state(&self, elicitation_id: &str) -> ElicitationResult<()> {
        let mut state_map = self.oauth_state.lock().await;
        state_map.remove(elicitation_id);
        Ok(())
    }

    /// Clean up expired OAuth state entries.
    pub async fn cleanup_expired_oauth_state(&self) -> ElicitationResult<usize> {
        let mut state_map = self.oauth_state.lock().await;
        let now = chrono::Utc::now();
        let initial_len = state_map.len();

        state_map.retain(|_, entry| entry.expires_at > now);

        Ok(initial_len - state_map.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_state_in_memory() {
        // This would need a database for full testing
        // For now, just verify the struct compiles
        let state = OAuthState {
            elicitation_id: "test-id".to_string(),
            user_id: "user123".to_string(),
            provider: "github".to_string(),
            state_token: "random-token".to_string(),
            redirect_uri: "http://localhost/callback".to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        };

        assert_eq!(state.elicitation_id, "test-id");
        assert_eq!(state.provider, "github");
    }
}
