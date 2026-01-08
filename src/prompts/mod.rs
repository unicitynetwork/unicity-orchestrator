//! Prompt forwarding for MCP servers.
//!
//! This module handles discovering and forwarding prompts from configured MCP services.
//! When multiple services define prompts with the same name, the orchestrator creates
//! namespaced aliases to avoid conflicts (e.g., `github-commit`, `gitlab-commit`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use rmcp::model::{
    Prompt as McpPrompt, PromptArgument as McpPromptArgument,
    GetPromptResult, GetPromptRequestParam, ListPromptsResult, JsonObject,
    Icon,
};
use anyhow::Result;

/// Error types for prompt operations.
#[derive(Debug, Clone)]
pub enum PromptError {
    /// Prompt name not found.
    NotFound(String),
    /// Invalid prompt name (contains unsafe characters or fails validation).
    InvalidName(String),
    /// Invalid arguments (missing required or contains unsafe data).
    InvalidArguments(String),
    /// Internal error during prompt operations.
    Internal(String),
}

impl std::fmt::Display for PromptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PromptError::NotFound(name) => write!(f, "Prompt not found: {}", name),
            PromptError::InvalidName(name) => write!(f, "Invalid prompt name: {}", name),
            PromptError::InvalidArguments(msg) => write!(f, "Invalid arguments: {}", msg),
            PromptError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for PromptError {}

/// Validate a prompt name to prevent injection attacks.
/// Returns true if the name contains only safe characters.
fn is_valid_prompt_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 256 {
        return false;
    }
    // Allow alphanumeric, hyphens, underscores, and colons (for service:prompt pattern)
    name.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ':')
}

/// Validate prompt arguments to prevent injection attacks.
/// Ensures argument names are safe and values are reasonable.
fn is_valid_arguments(arguments: &Option<JsonObject>) -> bool {
    match arguments {
        None => true,
        Some(args) => {
            if args.len() > 100 {
                return false; // Too many arguments
            }
            // Check each argument name is safe
            args.keys().all(|key| {
                !key.is_empty()
                    && key.len() <= 128
                    && key.chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            })
        }
    }
}

/// Sanitize a string for use as a prompt identifier.
/// Replaces spaces and special characters with hyphens, ensuring URL-safe output.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c.is_whitespace() {
                '-'
            } else {
                '-' // Replace other special chars with hyphens
            }
        })
        .collect::<String>()
        // Collapse multiple consecutive hyphens into one
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<&str>>()
        .join("-")
}

/// A discovered prompt from an MCP service.
#[derive(Clone, Debug)]
pub struct DiscoveredPrompt {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub arguments: Option<Vec<McpPromptArgument>>,
    pub icons: Option<Vec<Icon>>,
    pub service_id: String,
    pub service_name: String,  // Human-readable service name for display
}

/// An entry in the prompt registry with alias information.
#[derive(Clone, Debug)]
struct PromptEntry {
    prompt: DiscoveredPrompt,
    is_conflict: bool,          // Whether this prompt name has conflicts
    namespaced_name: String,    // e.g., "github-commit"
}

/// Registry for managing discovered prompts from MCP services.
#[derive(Clone)]
pub struct PromptRegistry {
    prompts: HashMap<String, PromptEntry>,  // Key: namespaced_name or prompt_name
    prompt_to_services: HashMap<String, Vec<String>>,  // prompt_name -> [service_ids]
    aliases: HashMap<String, String>,  // alias -> namespaced_name
}

impl PromptRegistry {
    /// Create a new empty prompt registry.
    pub fn new() -> Self {
        Self {
            prompts: HashMap::new(),
            prompt_to_services: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    /// Register a discovered prompt.
    pub fn register(&mut self, prompt: DiscoveredPrompt) {
        let service_id = prompt.service_id.clone();
        let prompt_name = prompt.name.clone();

        // Track which services have this prompt
        self.prompt_to_services
            .entry(prompt_name.clone())
            .or_insert_with(Vec::new)
            .push(service_id.clone());

        // Create namespaced name with sanitized service and prompt names
        let sanitized_service = sanitize_name(&prompt.service_name);
        let sanitized_prompt = sanitize_name(&prompt_name);
        let namespaced_name = format!("{}-{}", sanitized_service, sanitized_prompt);

        // Register the namespaced variant
        self.prompts.insert(namespaced_name.clone(), PromptEntry {
            prompt: prompt.clone(),
            is_conflict: false,
            namespaced_name: namespaced_name.clone(),
        });

        // Create alias from prompt_name to namespaced_name
        // This will be resolved during lookup
        self.aliases.insert(prompt_name, namespaced_name);
    }

    /// Mark prompt names as conflicting after all discovery is done.
    /// Uses case-insensitive comparison to detect conflicts (e.g., "commit" and "Commit" are conflicts).
    pub fn mark_conflicts(&mut self) {
        let mut case_insensitive_counts: HashMap<String, Vec<String>> = HashMap::new();

        // Count prompts by lowercased name
        for (prompt_name, services) in &self.prompt_to_services {
            let lower_name = prompt_name.to_lowercase();
            case_insensitive_counts
                .entry(lower_name)
                .or_insert_with(Vec::new)
                .extend(services.iter().cloned());
        }

        // Mark as conflicting if the case-insensitive name has multiple services
        let mut conflicting: HashSet<String> = HashSet::new();
        for (prompt_name, _) in &self.prompt_to_services {
            let lower_name = prompt_name.to_lowercase();
            if let Some(services) = case_insensitive_counts.get(&lower_name) {
                if services.len() > 1 {
                    conflicting.insert(prompt_name.clone());
                }
            }
        }

        // Update is_conflict flag for affected prompts
        for entry in self.prompts.values_mut() {
            if conflicting.contains(&entry.prompt.name) {
                entry.is_conflict = true;
            }
        }
    }

    /// List all registered prompts as MCP Prompt objects.
    pub fn list_prompts(&self) -> Vec<DiscoveredPrompt> {
        self.prompts.values().map(|entry| {
            let mut p = entry.prompt.clone();

            // For conflicts, update description to note the conflict
            if entry.is_conflict {
                let desc = p.description.as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Prompt");

                // Count arguments for additional info
                let arg_count = p.arguments.as_ref()
                    .map(|args| args.len())
                    .unwrap_or(0);
                let arg_info = if arg_count > 0 {
                    format!(" ({} argument{})", arg_count, if arg_count == 1 { "" } else { "s" })
                } else {
                    String::new()
                };

                p.description = Some(format!(
                    "{} (from {}){}\n\nNote: This prompt name is used by multiple services. \
                     Use the namespaced variant (e.g. {}-{}) to be specific.",
                    desc, entry.prompt.service_name, arg_info,
                    sanitize_name(&entry.prompt.service_name), sanitize_name(&entry.prompt.name)
                ));
            } else if p.description.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                // Provide a sensible default for empty or missing descriptions
                p.description = Some(format!("Prompt from {}", entry.prompt.service_name));
            }

            // Use namespaced name for display
            p.name = entry.namespaced_name.clone();
            p
        }).collect()
    }

    /// Resolve a prompt name to its entry.
    /// Returns the namespaced name and the service ID.
    ///
    /// Resolution order:
    /// 1. Direct match (namespaced name)
    /// 2. Alias lookup (original prompt name)
    /// 3. Service-prompt pattern (e.g., "my-service:commit" — uses sanitized names)
    /// 4. Case-insensitive fallback for the above patterns
    pub fn resolve(&self, name: &str) -> Option<(String, String)> {
        // First, check if it's a direct match (namespaced name)
        if let Some(entry) = self.prompts.get(name) {
            return Some((entry.prompt.service_id.clone(), entry.prompt.name.clone()));
        }

        // Check if it's an alias (original prompt name)
        if let Some(namespaced) = self.aliases.get(name) {
            if let Some(entry) = self.prompts.get(namespaced) {
                return Some((entry.prompt.service_id.clone(), entry.prompt.name.clone()));
            }
        }

        // Check if it's a service-prompt pattern (for direct addressing)
        // Both service and prompt names are sanitized for consistency
        if let Some((service_part, prompt_name)) = name.split_once(':') {
            let sanitized_service = sanitize_name(service_part);
            let sanitized_prompt = sanitize_name(prompt_name);

            // Try exact match first (with sanitized names)
            for entry in self.prompts.values() {
                if sanitize_name(&entry.prompt.service_name) == sanitized_service
                    && sanitize_name(&entry.prompt.name) == sanitized_prompt {
                    return Some((entry.prompt.service_id.clone(), entry.prompt.name.clone()));
                }
            }

            // Try case-insensitive match
            let service_lower = sanitized_service.to_lowercase();
            let prompt_lower = sanitized_prompt.to_lowercase();
            for entry in self.prompts.values() {
                if sanitize_name(&entry.prompt.service_name).to_lowercase() == service_lower
                    && sanitize_name(&entry.prompt.name).to_lowercase() == prompt_lower {
                    return Some((entry.prompt.service_id.clone(), entry.prompt.name.clone()));
                }
            }
        }

        // Case-insensitive fallback: try lowercasing the input
        let name_lower = name.to_lowercase();

        // Try case-insensitive namespaced match
        for (key, entry) in &self.prompts {
            if key.to_lowercase() == name_lower {
                return Some((entry.prompt.service_id.clone(), entry.prompt.name.clone()));
            }
        }

        // Try case-insensitive alias match
        for (alias, namespaced) in &self.aliases {
            if alias.to_lowercase() == name_lower {
                if let Some(entry) = self.prompts.get(namespaced) {
                    return Some((entry.prompt.service_id.clone(), entry.prompt.name.clone()));
                }
            }
        }

        None
    }

    /// Return the number of registered prompts.
    pub fn len(&self) -> usize {
        self.prompts.len()
    }

    /// Return `true` if no prompts are registered.
    pub fn is_empty(&self) -> bool {
        self.prompts.is_empty()
    }

    /// Clear all registered prompts.
    /// Useful for re-discovery to avoid duplicate entries.
    pub fn clear(&mut self) {
        self.prompts.clear();
        self.prompt_to_services.clear();
        self.aliases.clear();
    }
}

impl Default for PromptRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Pagination constants for prompts.
const DEFAULT_PAGE_SIZE: usize = 100;

/// Handles prompt forwarding to discovered MCP services.
pub struct PromptForwarder {
    pub(crate) registry: Arc<Mutex<PromptRegistry>>,
    pub(crate) running_services: Arc<Mutex<HashMap<String, Arc<crate::mcp_client::RunningService>>>>,
    /// Database reference for querying service metadata.
    db: surrealdb::Surreal<surrealdb::engine::any::Any>,
}

impl PromptForwarder {
    /// Create a new prompt forwarder.
    pub fn new(
        registry: Arc<Mutex<PromptRegistry>>,
        running_services: Arc<Mutex<HashMap<String, Arc<crate::mcp_client::RunningService>>>>,
        db: surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> Self {
        Self {
            registry,
            running_services,
            db,
        }
    }

    /// List available prompts from discovered services with pagination.
    ///
    /// # Arguments
    /// * `cursor` - Optional pagination cursor as a stringified offset (e.g., "0", "100")
    pub async fn list_prompts(&self, cursor: Option<&str>) -> Result<ListPromptsResult> {
        let mut registry = self.registry.lock().await;

        // Mark conflicts before listing
        registry.mark_conflicts();

        let prompts = registry.list_prompts();

        // Parse cursor to get offset
        let offset = cursor
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);

        let total = prompts.len();
        let next_offset = offset + DEFAULT_PAGE_SIZE;

        // Paginate the prompts
        let page: Vec<DiscoveredPrompt> = prompts
            .into_iter()
            .skip(offset)
            .take(DEFAULT_PAGE_SIZE)
            .collect();

        let mcp_prompts: Vec<McpPrompt> = page
            .into_iter()
            .map(|p| {
                // Use original title if available, otherwise fall back to namespaced name
                let title = p.title.unwrap_or_else(|| p.name.clone());

                McpPrompt {
                    name: p.name.into(),
                    title: Some(title.into()),
                    description: p.description.map(Into::into),
                    arguments: p.arguments,
                    icons: p.icons,
                    meta: None,
                }
            })
            .collect();

        let next_cursor = if next_offset < total {
            Some(next_offset.to_string().into())
        } else {
            None
        };

        Ok(ListPromptsResult {
            meta: None,
            prompts: mcp_prompts,
            next_cursor,
        })
    }

    /// Get a specific prompt by name.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<GetPromptResult, PromptError> {
        // Validate prompt name for security (prevent injection attacks)
        if !is_valid_prompt_name(name) {
            return Err(PromptError::InvalidName(name.to_string()));
        }

        // Validate arguments for security
        if !is_valid_arguments(&arguments) {
            return Err(PromptError::InvalidArguments("Arguments validation failed".to_string()));
        }

        let registry = self.registry.lock().await;

        // Resolve the prompt name to service_id and original prompt name
        let (service_id, prompt_name) = registry.resolve(name)
            .ok_or_else(|| PromptError::NotFound(name.to_string()))?;

        // Drop the registry lock before making the async call
        drop(registry);

        // Forward the request to the appropriate service
        let services = self.running_services.lock().await;
        let service = services.get(&service_id)
            .ok_or_else(|| PromptError::Internal(format!("Service not found: {}", service_id)))?;

        // Call the service's prompts/get method via rmcp
        let result = service
            .client
            .get_prompt(GetPromptRequestParam {
                name: prompt_name,
                arguments: arguments.map(|a| {
                    a.into_iter().map(|(k, v)| (k.into(), v)).collect()
                }),
            })
            .await
            .map_err(|e| PromptError::Internal(format!("Failed to get prompt: {}", e)))?;

        Ok(result)
    }

    /// Discover prompts from all running services.
    pub async fn discover_prompts(&self) -> Result<usize> {
        // Clear any existing prompts to avoid duplicates on re-discovery
        {
            let mut registry = self.registry.lock().await;
            registry.clear();
        }

        let services = self.running_services.lock().await;
        let mut count = 0;

        for (service_id, service) in services.iter() {
            match self.discover_service_prompts(service_id, service).await {
                Ok(n) => count += n,
                Err(e) => {
                    tracing::warn!("Failed to discover prompts from service {}: {}", service_id, e);
                }
            }
        }

        // Mark conflicts after all discovery is done
        {
            let mut registry = self.registry.lock().await;
            registry.mark_conflicts();
        }

        Ok(count)
    }

    /// Discover prompts from a specific service.
    async fn discover_service_prompts(
        &self,
        service_id: &str,
        service: &Arc<crate::mcp_client::RunningService>,
    ) -> Result<usize> {
        let list_result = service
            .client
            .list_prompts(None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list prompts: {}", e))?;

        // Query the actual service name from the database instead of parsing the RecordId.
        // The RecordId format is typically "service:<id>", but we want the human-readable name.
        let service_name = self.get_service_name(service_id).await.unwrap_or_else(|| {
            // Fallback to parsing the RecordId if DB lookup fails
            service_id
                .split(':')
                .next()
                .unwrap_or(service_id)
                .to_string()
        });

        let mut registry = self.registry.lock().await;
        let mut count = 0;

        for prompt in list_result.prompts {
            registry.register(DiscoveredPrompt {
                name: prompt.name.to_string(),
                title: prompt.title.map(|s| s.to_string()),
                description: prompt.description.map(|s| s.to_string()),
                arguments: prompt.arguments.map(|args| {
                    args.into_iter()
                        .map(|arg| McpPromptArgument {
                            name: arg.name.to_string(),
                            title: arg.title.map(|s| s.to_string()),
                            description: arg.description.map(|s| s.to_string()),
                            required: arg.required,
                        })
                        .collect()
                }),
                icons: prompt.icons,
                service_id: service_id.to_string(),
                service_name: service_name.clone(),
            });
            count += 1;
        }

        Ok(count)
    }

    /// Query the service name from the database by service_id.
    /// Returns None if the service cannot be found.
    async fn get_service_name(&self, service_id: &str) -> Option<String> {
        // Parse the service_id as a RecordId
        // The service_id is expected to be in the format "service:xxx" (table:key)
        let parts: Vec<&str> = service_id.split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        let record_id = surrealdb::RecordId::from_table_key(parts[0], parts[1]);

        // Query the service from the database
        #[derive(serde::Deserialize)]
        struct ServiceNameResult {
            name: Option<String>,
        }

        let mut res = self
            .db
            .query("SELECT name FROM service WHERE id = $id LIMIT 1")
            .bind(("id", record_id))
            .await
            .ok()?;

        let result: Option<ServiceNameResult> = res.take(0).ok()?;
        result?.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Create a mock DiscoveredPrompt for testing.
    fn mock_prompt(service_name: &str, name: &str, description: Option<&str>) -> DiscoveredPrompt {
        DiscoveredPrompt {
            name: name.to_string(),
            title: None,
            description: description.map(|s| s.to_string()),
            arguments: None,
            icons: None,
            service_id: format!("service:{}", service_name),
            service_name: service_name.to_string(),
        }
    }

    #[test]
    fn test_sanitize_name_basic() {
        assert_eq!(sanitize_name("commit"), "commit");
        assert_eq!(sanitize_name("github"), "github");
    }

    #[test]
    fn test_sanitize_name_spaces() {
        assert_eq!(sanitize_name("my service"), "my-service");
        assert_eq!(sanitize_name("commit message"), "commit-message");
    }

    #[test]
    fn test_sanitize_name_special_chars() {
        assert_eq!(sanitize_name("my@service"), "my-service");
        assert_eq!(sanitize_name("service:123"), "service-123");
        assert_eq!(sanitize_name("my/service"), "my-service");
    }

    #[test]
    fn test_sanitize_name_collapses_hyphens() {
        assert_eq!(sanitize_name("my  service"), "my-service");
        assert_eq!(sanitize_name("my--service"), "my-service");
        assert_eq!(sanitize_name("my @ service"), "my-service");
    }

    #[test]
    fn test_prompt_registry_clear() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("github", "commit", Some("Create a commit")));
        assert_eq!(registry.len(), 1);

        registry.clear();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_prompt_registry_single() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("github", "commit", Some("Create a commit")));
        registry.mark_conflicts();

        let prompts = registry.list_prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "github-commit");
        // No conflict note in description
        assert!(!prompts[0].description.as_ref().unwrap().contains("Note: This prompt name is used by multiple services"));
    }

    #[test]
    fn test_prompt_registry_conflict() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("github", "commit", Some("Create a commit")));
        registry.register(mock_prompt("gitlab", "commit", Some("Create a commit")));
        registry.mark_conflicts();

        let prompts = registry.list_prompts();
        assert_eq!(prompts.len(), 2);

        // Both should have conflict notes
        for prompt in &prompts {
            assert!(prompt.description.as_ref().unwrap().contains("Note: This prompt name is used by multiple services"));
        }
    }

    #[test]
    fn test_prompt_registry_case_insensitive_conflict() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("github", "commit", Some("Create a commit")));
        registry.register(mock_prompt("gitlab", "Commit", Some("Create a commit")));
        registry.mark_conflicts();

        let prompts = registry.list_prompts();
        assert_eq!(prompts.len(), 2);

        // Both should be marked as conflicting despite case difference
        for prompt in &prompts {
            assert!(prompt.description.as_ref().unwrap().contains("Note: This prompt name is used by multiple services"));
        }
    }

    #[test]
    fn test_prompt_registry_resolve_exact_match() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("github", "commit", Some("Create a commit")));
        registry.mark_conflicts();

        // Exact match on namespaced name
        let result = registry.resolve("github-commit");
        assert!(result.is_some());
        assert_eq!(result.unwrap().1, "commit");
    }

    #[test]
    fn test_prompt_registry_resolve_alias() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("github", "commit", Some("Create a commit")));
        registry.mark_conflicts();

        // Alias lookup (original prompt name)
        let result = registry.resolve("commit");
        assert!(result.is_some());
    }

    #[test]
    fn test_prompt_registry_resolve_case_insensitive() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("github", "commit", Some("Create a commit")));
        registry.register(mock_prompt("gitlab", "push", Some("Push changes")));
        registry.mark_conflicts();

        // Case-insensitive lookup should work
        assert!(registry.resolve("github-commit").is_some());
        assert!(registry.resolve("GITHUB-COMMIT").is_some());
        assert!(registry.resolve("GitHub-Commit").is_some());

        assert!(registry.resolve("gitlab-push").is_some());
        assert!(registry.resolve("GITLAB-PUSH").is_some());
    }

    #[test]
    fn test_prompt_registry_resolve_service_prompt_pattern() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("github", "commit", Some("Create a commit")));
        registry.register(mock_prompt("gitlab", "commit", Some("Create a commit")));
        registry.mark_conflicts();

        // Service-prompt pattern with sanitized names
        let result = registry.resolve("github:commit");
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "service:github");

        let result = registry.resolve("gitlab:commit");
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "service:gitlab");
    }

    #[test]
    fn test_prompt_registry_resolve_service_prompt_pattern_sanitized() {
        let mut registry = PromptRegistry::new();
        // Service with spaces and special chars
        registry.register(mock_prompt("my service", "commit", Some("Create a commit")));
        registry.register(mock_prompt("service:prod", "push", Some("Push changes")));
        registry.mark_conflicts();

        // Should work with sanitized service names
        let result = registry.resolve("my-service:commit");
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "service:my service");

        let result = registry.resolve("service-prod:push");
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "service:service:prod");

        // Original (unsanitized) should also still work
        let result = registry.resolve("my service:commit");
        assert!(result.is_some());
    }

    #[test]
    fn test_prompt_registry_resolve_service_prompt_case_insensitive() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("MyService", "commit", Some("Create a commit")));
        registry.mark_conflicts();

        // Case-insensitive service-prompt pattern should work
        assert!(registry.resolve("myservice:commit").is_some());
        assert!(registry.resolve("MYSERVICE:COMMIT").is_some());
        assert!(registry.resolve("MyService:Commit").is_some());
    }

    #[test]
    fn test_prompt_registry_resolve_service_prompt_with_spaces() {
        let mut registry = PromptRegistry::new();
        // Service with spaces in the name
        registry.register(mock_prompt("My Service", "commit msg", Some("Create a commit")));
        registry.mark_conflicts();

        // All of these should resolve to the same prompt
        assert!(registry.resolve("my-service:commit-msg").is_some()); // Fully sanitized
        assert!(registry.resolve("My Service:commit msg").is_some()); // Original
        assert!(registry.resolve("MY-SERVICE:COMMIT-MSG").is_some()); // Uppercase sanitized
        assert!(registry.resolve("my service:commit msg").is_some()); // Lowercase original
    }

    #[test]
    fn test_prompt_registry_empty_description_default() {
        let mut registry = PromptRegistry::new();
        registry.register(mock_prompt("github", "commit", None));
        registry.mark_conflicts();

        let prompts = registry.list_prompts();
        assert_eq!(prompts.len(), 1);
        // Should have default description
        assert!(prompts[0].description.as_ref().unwrap().contains("Prompt from github"));
    }

    #[test]
    fn test_prompt_registry_conflict_with_arguments() {
        let mut registry = PromptRegistry::new();
        let mut p1 = mock_prompt("github", "commit", Some("Create a commit"));
        p1.arguments = Some(vec![
            McpPromptArgument {
                name: "message".to_string(),
                title: None,
                description: Some("Commit message".to_string()),
                required: Some(true),
            },
        ]);

        let mut p2 = mock_prompt("gitlab", "commit", Some("Create a commit"));
        p2.arguments = Some(vec![
            McpPromptArgument {
                name: "message".to_string(),
                title: None,
                description: Some("Commit message".to_string()),
                required: Some(true),
            },
            McpPromptArgument {
                name: "branch".to_string(),
                title: None,
                description: Some("Target branch".to_string()),
                required: Some(false),
            },
        ]);

        registry.register(p1);
        registry.register(p2);
        registry.mark_conflicts();

        let prompts = registry.list_prompts();

        // Find the one with 2 arguments (gitlab)
        let gitlab_prompt = prompts.iter().find(|p| p.name == "gitlab-commit").unwrap();
        assert!(gitlab_prompt.description.as_ref().unwrap().contains("(2 arguments)"));

        // Find the one with 1 argument (github)
        let github_prompt = prompts.iter().find(|p| p.name == "github-commit").unwrap();
        assert!(github_prompt.description.as_ref().unwrap().contains("(1 argument)"));
    }

    #[tokio::test]
    async fn test_prompt_forwarder_rediscovery_clears_duplicates() {
        // This test requires a database; we'll use an in-memory one
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(PromptRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = PromptForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Since we don't have actual MCP services, we'll test the clear() directly
        // by manually adding prompts to the registry
        {
            let mut reg = registry.lock().await;
            reg.register(mock_prompt("github", "commit", Some("Create a commit")));
            assert_eq!(reg.len(), 1);
        }

        // Call discover_prompts which should clear the registry
        // (with no running services, it will just clear and add nothing)
        let result = forwarder.discover_prompts().await.unwrap();
        assert_eq!(result, 0);

        // Registry should be empty after re-discovery
        {
            let reg = registry.lock().await;
            assert_eq!(reg.len(), 0);
        }
    }

    // === Security validation tests ===

    #[test]
    fn test_is_valid_prompt_name_valid() {
        // Valid names
        assert!(is_valid_prompt_name("commit"));
        assert!(is_valid_prompt_name("github-commit"));
        assert!(is_valid_prompt_name("my_prompt"));
        assert!(is_valid_prompt_name("service:prompt"));
        assert!(is_valid_prompt_name("a")); // Single char
    }

    #[test]
    fn test_is_valid_prompt_name_invalid() {
        // Empty
        assert!(!is_valid_prompt_name(""));

        // Too long (>256 chars)
        assert!(!is_valid_prompt_name(&"a".repeat(257)));

        // Invalid characters
        assert!(!is_valid_prompt_name("commit; drop table"));
        assert!(!is_valid_prompt_name("commit && rm -rf"));
        assert!(!is_valid_prompt_name("commit`"));
        assert!(!is_valid_prompt_name("commit$(whoami)"));
        assert!(!is_valid_prompt_name("../etc/passwd"));
        assert!(!is_valid_prompt_name("<script>"));
    }

    #[test]
    fn test_is_valid_arguments_valid() {
        // No arguments is valid
        assert!(is_valid_arguments(&None));

        // Simple valid arguments
        let mut args = JsonObject::new();
        args.insert("message".to_string(), serde_json::json!("hello"));
        assert!(is_valid_arguments(&Some(args)));

        // Multiple valid arguments
        let mut args = JsonObject::new();
        args.insert("arg1".to_string(), serde_json::json!("value1"));
        args.insert("arg_2".to_string(), serde_json::json!("value2"));
        args.insert("arg-3".to_string(), serde_json::json!("value3"));
        assert!(is_valid_arguments(&Some(args)));
    }

    #[test]
    fn test_is_valid_arguments_invalid() {
        // Too many arguments (>100)
        let mut args = JsonObject::new();
        for i in 0..101 {
            args.insert(format!("arg{}", i), serde_json::json!("value"));
        }
        assert!(!is_valid_arguments(&Some(args)));

        // Empty argument name
        let mut args = JsonObject::new();
        args.insert("".to_string(), serde_json::json!("value"));
        assert!(!is_valid_arguments(&Some(args)));

        // Invalid argument name characters
        let mut args = JsonObject::new();
        args.insert("arg;drop".to_string(), serde_json::json!("value"));
        assert!(!is_valid_arguments(&Some(args)));

        // Argument name too long (>128)
        let mut args = JsonObject::new();
        args.insert("a".repeat(129), serde_json::json!("value"));
        assert!(!is_valid_arguments(&Some(args)));
    }

    #[tokio::test]
    async fn test_get_prompt_validates_name() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(PromptRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = PromptForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Invalid prompt name with injection attempt
        let result = forwarder.get_prompt("commit; drop table", None).await;
        assert!(matches!(result, Err(PromptError::InvalidName(_))));

        // Invalid prompt name with path traversal
        let result = forwarder.get_prompt("../../etc/passwd", None).await;
        assert!(matches!(result, Err(PromptError::InvalidName(_))));
    }

    #[tokio::test]
    async fn test_get_prompt_validates_arguments() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(PromptRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = PromptForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Too many arguments
        let mut args = JsonObject::new();
        for i in 0..101 {
            args.insert(format!("arg{}", i), serde_json::json!("value"));
        }
        let result = forwarder.get_prompt("github-commit", Some(args)).await;
        assert!(matches!(result, Err(PromptError::InvalidArguments(_))));

        // Invalid argument name
        let mut args = JsonObject::new();
        args.insert("bad;name".to_string(), serde_json::json!("value"));
        let result = forwarder.get_prompt("github-commit", Some(args)).await;
        assert!(matches!(result, Err(PromptError::InvalidArguments(_))));
    }

    #[tokio::test]
    async fn test_get_prompt_not_found() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(PromptRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = PromptForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Valid name but doesn't exist
        let result = forwarder.get_prompt("nonexistent-prompt", None).await;
        assert!(matches!(result, Err(PromptError::NotFound(_))));
    }
}
