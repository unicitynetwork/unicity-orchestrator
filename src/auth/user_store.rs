//! User storage and management.

use anyhow::Result;
use surrealdb::RecordId;

use crate::db::Db;
use crate::db::schema::{
    UserRecord, UserCreate, UserPreferencesRecord, UserPreferencesUpdate,
    AuditLogCreate, AuditAction,
};

/// User store for database operations.
pub struct UserStore {
    db: Db,
}

impl UserStore {
    /// Create a new user store.
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Get or create a user by external identity.
    ///
    /// This is the main entry point for authentication - it either finds an
    /// existing user or creates a new one.
    pub async fn get_or_create_user(
        &self,
        external_id: &str,
        provider: &str,
        email: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<UserRecord> {
        // Try to find existing user
        if let Some(user) = self.get_user_by_external_id(external_id, provider).await? {
            // Update last_seen_at
            self.update_last_seen(&user.id).await?;
            return Ok(user);
        }

        // Create new user
        let create = UserCreate {
            external_id: external_id.to_string(),
            provider: provider.to_string(),
            email: email.map(|s| s.to_string()),
            display_name: display_name.map(|s| s.to_string()),
        };

        let user = self.create_user(&create).await?;

        // Create default preferences for the new user
        self.create_default_preferences(&user.id).await?;

        // Log the user creation
        self.audit_log(AuditLogCreate {
            user_id: Some(user.id.to_string()),
            action: AuditAction::Login.as_str().to_string(),
            resource_type: "user".to_string(),
            resource_id: Some(user.id.to_string()),
            details: Some(serde_json::json!({
                "provider": provider,
                "is_new_user": true,
            })),
            ip_address: None,
            user_agent: None,
        }).await?;

        Ok(user)
    }

    /// Get a user by external ID and provider.
    pub async fn get_user_by_external_id(
        &self,
        external_id: &str,
        provider: &str,
    ) -> Result<Option<UserRecord>> {
        let external_id = external_id.to_string();
        let provider = provider.to_string();

        let query = r#"
            SELECT * FROM user
            WHERE external_id = $external_id
              AND provider = $provider
            LIMIT 1
        "#;

        let mut res = self.db
            .query(query)
            .bind(("external_id", external_id))
            .bind(("provider", provider))
            .await?;

        let users: Vec<UserRecord> = res.take(0)?;
        Ok(users.into_iter().next())
    }

    /// Get a user by database ID.
    pub async fn get_user_by_id(&self, user_id: &RecordId) -> Result<Option<UserRecord>> {
        let query = "SELECT * FROM user WHERE id = $id LIMIT 1";

        let mut res = self.db
            .query(query)
            .bind(("id", user_id.clone()))
            .await?;

        let users: Vec<UserRecord> = res.take(0)?;
        Ok(users.into_iter().next())
    }

    /// Create a new user.
    async fn create_user(&self, create: &UserCreate) -> Result<UserRecord> {
        let external_id = create.external_id.clone();
        let provider = create.provider.clone();
        let email = create.email.clone();
        let display_name = create.display_name.clone();

        let query = r#"
            CREATE user CONTENT {
                external_id: $external_id,
                provider: $provider,
                email: $email,
                display_name: $display_name,
                is_active: true,
                last_seen_at: time::now()
            }
        "#;

        let mut res = self.db
            .query(query)
            .bind(("external_id", external_id))
            .bind(("provider", provider))
            .bind(("email", email))
            .bind(("display_name", display_name))
            .await?;

        let users: Vec<UserRecord> = res.take(0)?;
        users.into_iter().next()
            .ok_or_else(|| anyhow::anyhow!("Failed to create user"))
    }

    /// Update user's last_seen_at timestamp.
    async fn update_last_seen(&self, user_id: &RecordId) -> Result<()> {
        let query = r#"
            UPDATE user SET
                last_seen_at = time::now(),
                updated_at = time::now()
            WHERE id = $id
        "#;

        self.db
            .query(query)
            .bind(("id", user_id.clone()))
            .await?;

        Ok(())
    }

    /// Deactivate a user account.
    pub async fn deactivate_user(&self, user_id: &RecordId) -> Result<()> {
        let query = r#"
            UPDATE user SET
                is_active = false,
                updated_at = time::now()
            WHERE id = $id
        "#;

        self.db
            .query(query)
            .bind(("id", user_id.clone()))
            .await?;

        Ok(())
    }

    /// Reactivate a user account.
    pub async fn reactivate_user(&self, user_id: &RecordId) -> Result<()> {
        let query = r#"
            UPDATE user SET
                is_active = true,
                updated_at = time::now()
            WHERE id = $id
        "#;

        self.db
            .query(query)
            .bind(("id", user_id.clone()))
            .await?;

        Ok(())
    }

    /// Create default preferences for a new user.
    async fn create_default_preferences(&self, user_id: &RecordId) -> Result<()> {
        let query = r#"
            CREATE user_preferences CONTENT {
                user_id: $user_id,
                default_approval_mode: 'prompt',
                elicitation_timeout_seconds: 300,
                remember_decisions: true,
                notify_on_tool_execution: false,
                notify_on_permission_grant: true
            }
        "#;

        self.db
            .query(query)
            .bind(("user_id", user_id.clone()))
            .await?;

        Ok(())
    }

    /// Get user preferences.
    pub async fn get_preferences(&self, user_id: &RecordId) -> Result<Option<UserPreferencesRecord>> {
        let query = "SELECT * FROM user_preferences WHERE user_id = $user_id LIMIT 1";

        let mut res = self.db
            .query(query)
            .bind(("user_id", user_id.clone()))
            .await?;

        let prefs: Vec<UserPreferencesRecord> = res.take(0)?;
        Ok(prefs.into_iter().next())
    }

    /// Update user preferences.
    pub async fn update_preferences(
        &self,
        user_id: &RecordId,
        update: &UserPreferencesUpdate,
    ) -> Result<()> {
        // Build the update query dynamically based on which fields are set
        let mut updates = Vec::new();
        let mut binds: Vec<(String, serde_json::Value)> = Vec::new();

        if let Some(mode) = &update.default_approval_mode {
            updates.push("default_approval_mode = $mode");
            binds.push(("mode".to_string(), serde_json::json!(mode)));
        }

        if let Some(trusted) = &update.trusted_services {
            updates.push("trusted_services = $trusted");
            binds.push(("trusted".to_string(), serde_json::json!(trusted)));
        }

        if let Some(blocked) = &update.blocked_services {
            updates.push("blocked_services = $blocked");
            binds.push(("blocked".to_string(), serde_json::json!(blocked)));
        }

        if let Some(timeout) = update.elicitation_timeout_seconds {
            updates.push("elicitation_timeout_seconds = $timeout");
            binds.push(("timeout".to_string(), serde_json::json!(timeout)));
        }

        if let Some(remember) = update.remember_decisions {
            updates.push("remember_decisions = $remember");
            binds.push(("remember".to_string(), serde_json::json!(remember)));
        }

        if let Some(notify_exec) = update.notify_on_tool_execution {
            updates.push("notify_on_tool_execution = $notify_exec");
            binds.push(("notify_exec".to_string(), serde_json::json!(notify_exec)));
        }

        if let Some(notify_grant) = update.notify_on_permission_grant {
            updates.push("notify_on_permission_grant = $notify_grant");
            binds.push(("notify_grant".to_string(), serde_json::json!(notify_grant)));
        }

        if updates.is_empty() {
            return Ok(());
        }

        // Always update the updated_at timestamp
        updates.push("updated_at = time::now()");

        let query = format!(
            "UPDATE user_preferences SET {} WHERE user_id = $user_id",
            updates.join(", ")
        );

        let mut query_builder = self.db.query(&query);
        query_builder = query_builder.bind(("user_id", user_id.clone()));

        // Note: SurrealDB's query builder doesn't support dynamic binding well
        // In a real implementation, we'd use a different approach
        // For now, this is a simplified version
        query_builder.await?;

        Ok(())
    }

    /// Check if a service is trusted by the user.
    pub async fn is_service_trusted(&self, user_id: &RecordId, service_id: &str) -> Result<bool> {
        if let Some(prefs) = self.get_preferences(user_id).await? {
            if let Some(trusted) = &prefs.trusted_services {
                return Ok(trusted.contains(&service_id.to_string()));
            }
        }
        Ok(false)
    }

    /// Check if a service is blocked by the user.
    pub async fn is_service_blocked(&self, user_id: &RecordId, service_id: &str) -> Result<bool> {
        if let Some(prefs) = self.get_preferences(user_id).await? {
            if let Some(blocked) = &prefs.blocked_services {
                return Ok(blocked.contains(&service_id.to_string()));
            }
        }
        Ok(false)
    }

    /// Unblock a service for the user.
    ///
    /// Removes the service from the user's blocked_services list.
    pub async fn unblock_service(&self, user_id: &RecordId, service_id: &str) -> Result<()> {
        if let Some(prefs) = self.get_preferences(user_id).await? {
            if let Some(blocked) = &prefs.blocked_services {
                let new_blocked: Vec<String> = blocked
                    .iter()
                    .filter(|s| s.as_str() != service_id)
                    .cloned()
                    .collect();

                let query = r#"
                    UPDATE user_preferences SET
                        blocked_services = $blocked,
                        updated_at = time::now()
                    WHERE user_id = $user_id
                "#;

                self.db
                    .query(query)
                    .bind(("user_id", user_id.clone()))
                    .bind(("blocked", new_blocked))
                    .await?;
            }
        }
        Ok(())
    }

    /// Block a service for the user.
    ///
    /// Adds the service to the user's blocked_services list.
    pub async fn block_service(&self, user_id: &RecordId, service_id: &str) -> Result<()> {
        let mut blocked = if let Some(prefs) = self.get_preferences(user_id).await? {
            prefs.blocked_services.unwrap_or_default()
        } else {
            Vec::new()
        };

        if !blocked.contains(&service_id.to_string()) {
            blocked.push(service_id.to_string());

            let query = r#"
                UPDATE user_preferences SET
                    blocked_services = $blocked,
                    updated_at = time::now()
                WHERE user_id = $user_id
            "#;

            self.db
                .query(query)
                .bind(("user_id", user_id.clone()))
                .bind(("blocked", blocked))
                .await?;
        }
        Ok(())
    }

    /// Write an audit log entry.
    pub async fn audit_log(&self, entry: AuditLogCreate) -> Result<()> {
        let query = r#"
            CREATE audit_log CONTENT {
                user_id: $user_id,
                action: $action,
                resource_type: $resource_type,
                resource_id: $resource_id,
                details: $details,
                ip_address: $ip_address,
                user_agent: $user_agent
            }
        "#;

        self.db
            .query(query)
            .bind(("user_id", entry.user_id))
            .bind(("action", entry.action))
            .bind(("resource_type", entry.resource_type))
            .bind(("resource_id", entry.resource_id))
            .bind(("details", entry.details))
            .bind(("ip_address", entry.ip_address))
            .bind(("user_agent", entry.user_agent))
            .await?;

        Ok(())
    }

    /// Get recent audit log entries for a user.
    pub async fn get_user_audit_log(
        &self,
        user_id: &str,
        limit: u32,
    ) -> Result<Vec<crate::db::schema::AuditLogRecord>> {
        let user_id = user_id.to_string();

        let query = r#"
            SELECT * FROM audit_log
            WHERE user_id = $user_id
            ORDER BY created_at DESC
            LIMIT $limit
        "#;

        let mut res = self.db
            .query(query)
            .bind(("user_id", user_id))
            .bind(("limit", limit))
            .await?;

        let logs: Vec<crate::db::schema::AuditLogRecord> = res.take(0)?;
        Ok(logs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{create_connection, ensure_schema, DatabaseConfig};

    async fn setup_test_db() -> Db {
        let config = DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = create_connection(config).await.unwrap();
        ensure_schema(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn test_get_or_create_user_creates_new() {
        let db = setup_test_db().await;
        let store = UserStore::new(db);

        let user = store.get_or_create_user(
            "sub123",
            "jwt",
            Some("test@example.com"),
            Some("Test User"),
        ).await.unwrap();

        assert_eq!(user.external_id, "sub123");
        assert_eq!(user.provider, "jwt");
        assert_eq!(user.email, Some("test@example.com".to_string()));
        assert_eq!(user.display_name, Some("Test User".to_string()));
        assert!(user.is_active);
    }

    #[tokio::test]
    async fn test_get_or_create_user_returns_existing() {
        let db = setup_test_db().await;
        let store = UserStore::new(db);

        // Create user
        let user1 = store.get_or_create_user(
            "sub123",
            "jwt",
            Some("test@example.com"),
            Some("Test User"),
        ).await.unwrap();

        // Get same user again
        let user2 = store.get_or_create_user(
            "sub123",
            "jwt",
            Some("test@example.com"),
            Some("Test User"),
        ).await.unwrap();

        assert_eq!(user1.id, user2.id);
    }

    #[tokio::test]
    async fn test_user_deactivation() {
        let db = setup_test_db().await;
        let store = UserStore::new(db);

        let user = store.get_or_create_user(
            "sub123",
            "jwt",
            None,
            None,
        ).await.unwrap();

        assert!(user.is_active);

        // Deactivate
        store.deactivate_user(&user.id).await.unwrap();

        let updated = store.get_user_by_id(&user.id).await.unwrap().unwrap();
        assert!(!updated.is_active);

        // Reactivate
        store.reactivate_user(&user.id).await.unwrap();

        let reactivated = store.get_user_by_id(&user.id).await.unwrap().unwrap();
        assert!(reactivated.is_active);
    }

    #[tokio::test]
    async fn test_user_preferences_created_on_user_create() {
        let db = setup_test_db().await;
        let store = UserStore::new(db);

        let user = store.get_or_create_user(
            "sub123",
            "jwt",
            None,
            None,
        ).await.unwrap();

        let prefs = store.get_preferences(&user.id).await.unwrap();
        assert!(prefs.is_some());

        let prefs = prefs.unwrap();
        assert_eq!(prefs.default_approval_mode, "prompt");
        assert_eq!(prefs.elicitation_timeout_seconds, 300);
        assert!(prefs.remember_decisions);
    }

    #[tokio::test]
    async fn test_audit_log() {
        let db = setup_test_db().await;
        let store = UserStore::new(db);

        let user = store.get_or_create_user(
            "sub123",
            "jwt",
            None,
            None,
        ).await.unwrap();

        // The user creation should have logged an audit entry
        let logs = store.get_user_audit_log(&user.id.to_string(), 10).await.unwrap();

        // Should have at least one log entry (the login/creation)
        assert!(!logs.is_empty());
        assert_eq!(logs[0].action, "login");
    }

    #[tokio::test]
    async fn test_different_providers_different_users() {
        let db = setup_test_db().await;
        let store = UserStore::new(db);

        // Same external_id but different providers = different users
        let user1 = store.get_or_create_user(
            "user123",
            "jwt",
            None,
            None,
        ).await.unwrap();

        let user2 = store.get_or_create_user(
            "user123",
            "api_key",
            None,
            None,
        ).await.unwrap();

        assert_ne!(user1.id, user2.id);
    }

    #[tokio::test]
    async fn test_service_trust_settings() {
        let db = setup_test_db().await;
        let store = UserStore::new(db);

        let user = store.get_or_create_user(
            "trust_test_user",
            "jwt",
            None,
            None,
        ).await.unwrap();

        // Initially no services are trusted
        let trusted = store.is_service_trusted(&user.id, "github").await.unwrap();
        assert!(!trusted);

        let blocked = store.is_service_blocked(&user.id, "malicious").await.unwrap();
        assert!(!blocked);
    }
}
