//! User-based tool filtering for multi-tenant isolation.
//!
//! This module provides filtering of tools based on user preferences,
//! including blocked services, trusted services, and trust boosting.

use std::collections::HashSet;
use anyhow::Result;
use surrealdb::engine::any::Any;
use surrealdb::{RecordId, Surreal};

use crate::auth::UserContext;
use crate::db::{ToolRecord, UserPreferencesRecord};
use crate::knowledge_graph::ToolSelection;

/// Filter for tools based on user preferences.
///
/// This struct is created from a user's preferences and can be used to:
/// - Filter out tools from blocked services
/// - Apply confidence boosts to trusted services
pub struct UserToolFilter {
    /// Service IDs that are blocked (tools from these services are excluded)
    blocked_services: HashSet<String>,
    /// Service IDs that are trusted (tools from these services get a confidence boost)
    trusted_services: HashSet<String>,
}

impl UserToolFilter {
    /// Create a filter that allows all tools (no filtering).
    ///
    /// Use this for anonymous users or when multi-tenancy is disabled.
    pub fn allow_all() -> Self {
        Self {
            blocked_services: HashSet::new(),
            trusted_services: HashSet::new(),
        }
    }

    /// Create a filter from user context by loading their preferences.
    ///
    /// If the user has no preferences, returns a filter that allows all tools.
    pub async fn from_user_context(
        db: &Surreal<Any>,
        ctx: &UserContext,
    ) -> Result<Self> {
        // Query user preferences
        let user_id = ctx.user_id();

        let mut result = db
            .query("SELECT * FROM user_preferences WHERE user_id = $user_id LIMIT 1")
            .bind(("user_id", user_id.clone()))
            .await?;

        let prefs: Option<UserPreferencesRecord> = result.take(0)?;

        match prefs {
            Some(p) => {
                let blocked_services = p.blocked_services
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                let trusted_services = p.trusted_services
                    .unwrap_or_default()
                    .into_iter()
                    .collect();

                Ok(Self {
                    blocked_services,
                    trusted_services,
                })
            }
            None => {
                // No preferences - allow all
                Ok(Self::allow_all())
            }
        }
    }

    /// Check if a tool is allowed based on its service.
    pub fn is_tool_allowed(&self, tool: &ToolRecord) -> bool {
        let service_id_str = tool.service_id.to_string();
        !self.blocked_services.contains(&service_id_str)
    }

    /// Check if a service is trusted.
    pub fn is_service_trusted(&self, service_id: &RecordId) -> bool {
        let service_id_str = service_id.to_string();
        self.trusted_services.contains(&service_id_str)
    }

    /// Filter a list of tools, removing those from blocked services.
    pub fn filter_tools(&self, tools: Vec<ToolRecord>) -> Vec<ToolRecord> {
        if self.blocked_services.is_empty() {
            return tools;
        }

        tools
            .into_iter()
            .filter(|tool| self.is_tool_allowed(tool))
            .collect()
    }

    /// Filter a list of tool selections, removing those from blocked services.
    pub fn filter_selections(&self, selections: Vec<ToolSelection>) -> Vec<ToolSelection> {
        if self.blocked_services.is_empty() {
            return selections;
        }

        selections
            .into_iter()
            .filter(|sel| {
                let service_id_str = sel.service_id.to_string();
                !self.blocked_services.contains(&service_id_str)
            })
            .collect()
    }

    /// Apply a confidence boost to tools from trusted services.
    ///
    /// This modifies the selections in place, increasing confidence for
    /// tools from trusted services by the given boost factor.
    pub fn apply_trust_boost(&self, selections: &mut Vec<ToolSelection>, boost: f32) {
        if self.trusted_services.is_empty() {
            return;
        }

        for selection in selections.iter_mut() {
            if self.is_service_trusted(&selection.service_id) {
                // Boost confidence, capping at 1.0
                selection.confidence = (selection.confidence + boost).min(1.0);
            }
        }
    }

    /// Check if any services are blocked.
    pub fn has_blocked_services(&self) -> bool {
        !self.blocked_services.is_empty()
    }

    /// Check if any services are trusted.
    pub fn has_trusted_services(&self) -> bool {
        !self.trusted_services.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::RecordId;

    fn make_tool_record(service_id: &str, name: &str) -> ToolRecord {
        ToolRecord {
            id: RecordId::from_table_key("tool", name),
            service_id: RecordId::from_table_key("service", service_id),
            name: name.to_string(),
            description: None,
            input_schema: serde_json::Map::new(),
            output_schema: None,
            embedding_id: None,
            input_ty: None,
            output_ty: None,
            usage_count: 0,
            created_at: None,
            updated_at: None,
        }
    }

    fn make_tool_selection(service_id: &str, name: &str, confidence: f32) -> ToolSelection {
        ToolSelection {
            tool_id: RecordId::from_table_key("tool", name),
            tool_name: name.to_string(),
            service_id: RecordId::from_table_key("service", service_id),
            confidence,
            reasoning: "test".to_string(),
            dependencies: vec![],
            estimated_cost: None,
        }
    }

    #[test]
    fn test_allow_all_filter() {
        let filter = UserToolFilter::allow_all();

        let tools = vec![
            make_tool_record("service1", "tool1"),
            make_tool_record("service2", "tool2"),
        ];

        let filtered = filter.filter_tools(tools.clone());
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_blocked_services() {
        let filter = UserToolFilter {
            blocked_services: vec!["service:service1".to_string()].into_iter().collect(),
            trusted_services: HashSet::new(),
        };

        let tools = vec![
            make_tool_record("service1", "tool1"),
            make_tool_record("service2", "tool2"),
        ];

        let filtered = filter.filter_tools(tools);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "tool2");
    }

    #[test]
    fn test_trust_boost() {
        let filter = UserToolFilter {
            blocked_services: HashSet::new(),
            trusted_services: vec!["service:service1".to_string()].into_iter().collect(),
        };

        let mut selections = vec![
            make_tool_selection("service1", "tool1", 0.5),
            make_tool_selection("service2", "tool2", 0.5),
        ];

        filter.apply_trust_boost(&mut selections, 0.1);

        // service1's tool should be boosted
        assert!((selections[0].confidence - 0.6).abs() < 0.001);
        // service2's tool should not be boosted
        assert!((selections[1].confidence - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_trust_boost_caps_at_one() {
        let filter = UserToolFilter {
            blocked_services: HashSet::new(),
            trusted_services: vec!["service:service1".to_string()].into_iter().collect(),
        };

        let mut selections = vec![
            make_tool_selection("service1", "tool1", 0.95),
        ];

        filter.apply_trust_boost(&mut selections, 0.1);

        // Should cap at 1.0
        assert!((selections[0].confidence - 1.0).abs() < 0.001);
    }
}
