//! Resource forwarding for MCP servers.
//!
//! This module handles discovering and forwarding resources from configured MCP services.
//! When multiple services define resources with conflicting URIs, the orchestrator provides
//! clear resolution options.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use rmcp::model::{
    RawResource, ReadResourceRequestParams,
    ListResourcesResult, ReadResourceResult,
    ListResourceTemplatesResult, RawResourceTemplate,
    AnnotateAble, Annotations, Icon,
};
use anyhow::Result;
use crate::db::Db;
use crate::types::{ResourceUri, ServiceId, ServiceName};

/// Default page size for paginated resource listings.
const DEFAULT_PAGE_SIZE: usize = 100;

/// Maximum URI length to prevent abuse.
const MAX_URI_LENGTH: usize = 4096;

/// A discovered resource from an MCP service.
#[derive(Clone, Debug)]
pub struct DiscoveredResource {
    pub uri: ResourceUri,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub mime_type: Option<String>,
    pub size: Option<u32>,
    pub icons: Option<Vec<Icon>>,
    pub annotations: Option<Annotations>,
    pub service_id: ServiceId,
    pub service_name: ServiceName,
}

/// A discovered resource template from an MCP service.
#[derive(Clone, Debug)]
pub struct DiscoveredResourceTemplate {
    pub uri_template: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub mime_type: Option<String>,
    pub annotations: Option<Annotations>,
    pub service_id: ServiceId,
    pub service_name: ServiceName,
}

/// Error types for resource operations.
#[derive(Debug, Clone)]
pub enum ResourceError {
    /// Resource URI not found.
    NotFound(String),
    /// Invalid URI (contains unsafe characters or fails validation).
    InvalidUri(String),
    /// Internal error during resource operations.
    Internal(String),
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceError::NotFound(uri) => write!(f, "Resource not found: {}", uri),
            ResourceError::InvalidUri(uri) => write!(f, "Invalid URI: {}", uri),
            ResourceError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for ResourceError {}

/// Validate a resource URI to prevent injection attacks and path traversal.
/// Returns true if the URI is safe.
fn is_valid_uri(uri: &str) -> bool {
    if uri.is_empty() || uri.len() > MAX_URI_LENGTH {
        return false;
    }

    // Basic URI validation - must contain a scheme like "file://", "https://", etc.
    if !uri.contains("://") {
        return false;
    }

    // Block obvious path traversal attempts
    if uri.contains("../") || uri.contains("..\\") {
        return false;
    }

    // Block null bytes
    if uri.contains('\0') {
        return false;
    }

    true
}

/// Registry for managing discovered resources from MCP services.
#[derive(Clone)]
pub struct ResourceRegistry {
    /// Key: URI string, Value: (service_id, original resource data)
    resources: HashMap<String, (ServiceId, DiscoveredResource)>,
    /// Track which services have each resource URI
    resource_to_services: HashMap<String, Vec<ServiceId>>,
    /// Store discovered resource templates
    templates: Vec<DiscoveredResourceTemplate>,
}

impl ResourceRegistry {
    /// Create a new empty resource registry.
    pub fn new() -> Self {
        Self {
            resources: HashMap::new(),
            resource_to_services: HashMap::new(),
            templates: Vec::new(),
        }
    }

    /// Register a discovered resource.
    pub fn register(&mut self, resource: DiscoveredResource) {
        let service_id = resource.service_id.clone();
        let uri = resource.uri.to_string();

        // Track which services have this resource
        self.resource_to_services
            .entry(uri.clone())
            .or_insert_with(Vec::new)
            .push(service_id.clone());

        // Store the resource (using the first service's version)
        self.resources.entry(uri)
            .or_insert_with(|| (service_id, resource));
    }

    /// Register a discovered resource template.
    pub fn register_template(&mut self, template: DiscoveredResourceTemplate) {
        self.templates.push(template);
    }

    /// List all registered resources.
    pub fn list_resources(&self) -> Vec<DiscoveredResource> {
        self.resources
            .values()
            .map(|(_, resource)| resource.clone())
            .collect()
    }

    /// List all registered resource templates.
    pub fn list_templates(&self) -> Vec<DiscoveredResourceTemplate> {
        self.templates.clone()
    }

    /// Resolve a resource URI to its entry.
    /// Returns the service_id and the original URI.
    pub fn resolve(&self, uri: &str) -> Option<(ServiceId, String)> {
        if let Some((service_id, _)) = self.resources.get(uri) {
            return Some((service_id.clone(), uri.to_string()));
        }
        None
    }

    /// Return the number of registered resources.
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    /// Return `true` if no resources are registered.
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Clear all registered resources.
    pub fn clear(&mut self) {
        self.resources.clear();
        self.resource_to_services.clear();
        self.templates.clear();
    }
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Handles resource forwarding to discovered MCP services.
pub struct ResourceForwarder {
    pub(crate) registry: Arc<Mutex<ResourceRegistry>>,
    pub(crate) running_services: Arc<Mutex<HashMap<String, Arc<crate::mcp_client::RunningService>>>>,
    /// Database reference for querying service metadata.
    db: Db,
}

impl ResourceForwarder {
    /// Create a new resource forwarder.
    pub fn new(
        registry: Arc<Mutex<ResourceRegistry>>,
        running_services: Arc<Mutex<HashMap<String, Arc<crate::mcp_client::RunningService>>>>,
        db: Db,
    ) -> Self {
        Self {
            registry,
            running_services,
            db,
        }
    }

    /// List resources from discovered services.
    /// Resource names are namespaced with their service name for provenance (e.g., "filesystem:config").
    /// Accepts an optional service filter to return only resources from a specific service (case-insensitive).
    ///
    /// Pagination is done via cursor. Provide a cursor string to get the next page.
    /// Cursor format is simply the offset as a string (e.g., "0", "100", "200").
    pub async fn list_resources(
        &self,
        service_filter: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<ListResourcesResult> {
        let registry = self.registry.lock().await;
        let resources = registry.list_resources();

        let filter_lower = service_filter.map(|s| s.to_lowercase());

        // Parse cursor to get offset
        let offset = cursor
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);

        // Filter and collect into a vec for pagination
        let filtered: Vec<_> = resources
            .into_iter()
            .filter(|r| {
                if let Some(ref filter) = filter_lower {
                    r.service_name.as_str().to_lowercase() == *filter
                } else {
                    true
                }
            })
            .collect();

        let total = filtered.len();
        let page: Vec<_> = filtered
            .into_iter()
            .skip(offset)
            .take(DEFAULT_PAGE_SIZE)
            .map(|r| {
                let namespaced_name = format!("{}:{}", r.service_name.as_str(), r.name);
                let description = r.description.clone().unwrap_or_else(|| {
                    format!("Resource from {}", r.service_name.as_str())
                });
                let description_with_provenance = format!("{} [from {}]", description, r.service_name.as_str());

                RawResource {
                    uri: r.uri.to_string(),
                    name: namespaced_name,
                    title: r.title,
                    description: Some(description_with_provenance),
                    mime_type: r.mime_type,
                    size: r.size,
                    icons: r.icons,
                    meta: None,
                }.optional_annotate(r.annotations)
            })
            .collect();

        // Calculate next cursor
        let next_offset = offset + page.len();
        let next_cursor = if next_offset < total {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(ListResourcesResult {
            meta: None,
            resources: page,
            next_cursor,
        })
    }

    /// Read a specific resource by URI or namespaced name.
    ///
    /// Accepts either:
    /// - Raw URI: `file:///config.json`
    /// - Namespaced name: `filesystem:config` (parses service and looks up by original resource name)
    ///
    /// Namespaced lookups are case-insensitive.
    pub async fn read_resource(
        &self,
        uri_or_name: &str,
    ) -> Result<ReadResourceResult, ResourceError> {
        // Check if this is a namespaced name (service:resource)
        let uri = if uri_or_name.contains(':') && !uri_or_name.contains("://") {
            // This is a namespaced name - look up the actual URI
            let parts: Vec<&str> = uri_or_name.splitn(2, ':').collect();
            if parts.len() != 2 {
                return Err(ResourceError::InvalidUri(uri_or_name.to_string()));
            }
            let service_name = parts[0].to_lowercase();
            let resource_name = parts[1].to_lowercase();

            let registry = self.registry.lock().await;
            let found = registry.list_resources()
                .into_iter()
                .find(|r| r.service_name.as_str().to_lowercase() == service_name && r.name.to_lowercase() == resource_name)
                .map(|r| r.uri.to_string());
            drop(registry);

            match found {
                Some(uri) => uri,
                None => return Err(ResourceError::NotFound(uri_or_name.to_string())),
            }
        } else {
            // This is a raw URI - use as-is
            uri_or_name.to_string()
        };

        // Validate URI for security
        if !is_valid_uri(&uri) {
            return Err(ResourceError::InvalidUri(uri));
        }

        let registry = self.registry.lock().await;

        // Resolve the URI to service_id
        let (service_id, _) = registry.resolve(&uri)
            .ok_or_else(|| ResourceError::NotFound(uri.clone()))?;

        // Drop the registry lock before making the async call
        drop(registry);

        // Forward the request to the appropriate service
        let services = self.running_services.lock().await;
        let service = services.get(service_id.as_str())
            .ok_or_else(|| ResourceError::Internal(format!("Service not found: {}", service_id)))?;

        // Read the actual resource contents
        let request = ReadResourceRequestParams {
            uri: uri.clone(),
            meta: None,
        };

        service
            .client
            .read_resource(request)
            .await
            .map_err(|e| ResourceError::Internal(format!("Failed to read resource: {}", e)))
    }

    /// List resource templates from discovered services.
    /// Template names are namespaced with their service name for provenance (e.g., "github:git-file").
    /// Accepts an optional service filter to return only templates from a specific service (case-insensitive).
    ///
    /// Pagination is done via cursor. Provide a cursor string to get the next page.
    /// Cursor format is simply the offset as a string (e.g., "0", "100", "200").
    pub async fn list_templates(
        &self,
        service_filter: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<ListResourceTemplatesResult> {
        let registry = self.registry.lock().await;
        let templates = registry.list_templates();

        let filter_lower = service_filter.map(|s| s.to_lowercase());

        // Parse cursor to get offset
        let offset = cursor
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);

        // Filter and collect into a vec for pagination
        let filtered: Vec<_> = templates
            .into_iter()
            .filter(|t| {
                if let Some(ref filter) = filter_lower {
                    t.service_name.as_str().to_lowercase() == *filter
                } else {
                    true
                }
            })
            .collect();

        let total = filtered.len();
        let page: Vec<_> = filtered
            .into_iter()
            .skip(offset)
            .take(DEFAULT_PAGE_SIZE)
            .map(|t| {
                let namespaced_name = format!("{}:{}", t.service_name.as_str(), t.name);
                let description = t.description.clone().unwrap_or_else(|| {
                    format!("Resource template from {}", t.service_name.as_str())
                });
                let description_with_provenance = format!("{} [from {}]", description, t.service_name.as_str());

                RawResourceTemplate {
                    uri_template: t.uri_template,
                    name: namespaced_name,
                    title: t.title,
                    description: Some(description_with_provenance),
                    mime_type: t.mime_type,
                    icons: None,
                }.optional_annotate(t.annotations)
            })
            .collect();

        // Calculate next cursor
        let next_offset = offset + page.len();
        let next_cursor = if next_offset < total {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(ListResourceTemplatesResult {
            meta: None,
            resource_templates: page,
            next_cursor,
        })
    }

    /// Discover resources from all running services.
    pub async fn discover_resources(&self) -> Result<usize> {
        // Clear any existing resources to avoid duplicates on re-discovery
        {
            let mut registry = self.registry.lock().await;
            registry.clear();
        }

        let services = self.running_services.lock().await;
        let mut count = 0;

        for (service_id, service) in services.iter() {
            match self.discover_service_resources(service_id, service).await {
                Ok(n) => count += n,
                Err(e) => {
                    tracing::warn!("Failed to discover resources from service {}: {}", service_id, e);
                }
            }
        }

        Ok(count)
    }

    /// Discover resources from a specific service.
    async fn discover_service_resources(
        &self,
        service_id: &str,
        service: &Arc<crate::mcp_client::RunningService>,
    ) -> Result<usize> {
        let list_result = service
            .client
            .list_resources(None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list resources: {}", e))?;

        // Get service name from the database
        let service_name = self.get_service_name(service_id).await.unwrap_or_else(|| {
            service_id.split(':')
                .next()
                .unwrap_or(service_id)
                .to_string()
        });

        let mut registry = self.registry.lock().await;
        let mut count = 0;

        for resource in list_result.resources {
            // resource is Annotated<RawResource>, deref gives RawResource fields
            registry.register(DiscoveredResource {
                uri: ResourceUri::new(resource.uri.clone()),
                name: resource.name.clone(),
                title: resource.title.clone(),
                description: resource.description.clone(),
                mime_type: resource.mime_type.clone(),
                size: resource.size,
                icons: resource.icons.clone(),
                annotations: resource.annotations.clone(),
                service_id: ServiceId::new(service_id),
                service_name: ServiceName::new(service_name.clone()),
            });
            count += 1;
        }

        // Also discover resource templates
        if let Ok(templates_result) = service.client.list_resource_templates(None).await {
            for template in templates_result.resource_templates {
                // template is Annotated<RawResourceTemplate>
                registry.register_template(DiscoveredResourceTemplate {
                    uri_template: template.uri_template.clone(),
                    name: template.name.clone(),
                    title: template.title.clone(),
                    description: template.description.clone(),
                    mime_type: template.mime_type.clone(),
                    annotations: template.annotations.clone(),
                    service_id: ServiceId::new(service_id),
                    service_name: ServiceName::new(service_name.clone()),
                });
            }
        }

        Ok(count)
    }

    /// Query the service name from the database by service_id.
    async fn get_service_name(&self, service_id: &str) -> Option<String> {
        let parts: Vec<&str> = service_id.split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        let record_id = surrealdb::RecordId::from_table_key(parts[0], parts[1]);

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

    /// Create a mock DiscoveredResource for testing.
    fn mock_resource(service_name: &str, uri: &str, name: &str) -> DiscoveredResource {
        DiscoveredResource {
            uri: ResourceUri::new(uri),
            name: name.to_string(),
            title: Some(format!("{} Resource", name)),
            description: Some(format!("A test resource: {}", name)),
            mime_type: Some("text/plain".to_string()),
            size: None,
            icons: None,
            annotations: None,
            service_id: ServiceId::new(format!("service:{}", service_name)),
            service_name: ServiceName::new(service_name),
        }
    }

    // === Security validation tests ===

    #[test]
    fn test_is_valid_uri_valid() {
        // Valid URIs
        assert!(is_valid_uri("file:///path/to/file.txt"));
        assert!(is_valid_uri("https://example.com/resource"));
        assert!(is_valid_uri("git://github.com/user/repo"));
        assert!(is_valid_uri("custom://my-resource"));
    }

    #[test]
    fn test_is_valid_uri_invalid() {
        // Empty
        assert!(!is_valid_uri(""));

        // No scheme
        assert!(!is_valid_uri("/path/to/file.txt"));

        // Too long (exceeds MAX_URI_LENGTH of 4096) - 5000 chars with scheme
        assert!(!is_valid_uri(&format!("file://{}", "a".repeat(5000))));

        // Path traversal
        assert!(!is_valid_uri("file:///etc/passwd/../shadow"));
        assert!(!is_valid_uri("file://../secret"));

        // Null bytes
        assert!(!is_valid_uri("file:///\0etc/passwd"));
    }

    // === ResourceRegistry tests ===

    #[test]
    fn test_resource_registry_empty() {
        let registry = ResourceRegistry::new();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_resource_registry_register() {
        let mut registry = ResourceRegistry::new();
        registry.register(mock_resource("github", "file:///main.rs", "main.rs"));

        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_resource_registry_resolve() {
        let mut registry = ResourceRegistry::new();
        registry.register(mock_resource("github", "file:///main.rs", "main.rs"));

        let result = registry.resolve("file:///main.rs");
        assert!(result.is_some());
        assert_eq!(result.unwrap().0.as_str(), "service:github");
    }

    #[test]
    fn test_resource_registry_resolve_not_found() {
        let registry = ResourceRegistry::new();
        assert!(registry.resolve("file:///nonexistent").is_none());
    }

    #[test]
    fn test_resource_registry_clear() {
        let mut registry = ResourceRegistry::new();
        registry.register(mock_resource("github", "file:///main.rs", "main.rs"));
        registry.register(mock_resource("gitlab", "file:///README.md", "README.md"));

        assert_eq!(registry.len(), 2);

        registry.clear();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_resource_registry_multiple_services_same_uri() {
        let mut registry = ResourceRegistry::new();
        // Both services define a resource with the same URI
        registry.register(mock_resource("github", "file:///config.json", "config"));
        registry.register(mock_resource("gitlab", "file:///config.json", "config"));

        // Should have 1 entry (first one wins)
        assert_eq!(registry.len(), 1);

        // Resolve should find the resource
        let result = registry.resolve("file:///config.json");
        assert!(result.is_some());
    }

    #[test]
    fn test_resource_registry_templates() {
        let mut registry = ResourceRegistry::new();
        let template = DiscoveredResourceTemplate {
            uri_template: "git://{repo}/file/{path}".to_string(),
            name: "git-file".to_string(),
            title: Some("Git File".to_string()),
            description: Some("A file from a git repo".to_string()),
            mime_type: Some("text/plain".to_string()),
            annotations: None,
            service_id: ServiceId::new("service:github"),
            service_name: ServiceName::new("github"),
        };

        registry.register_template(template.clone());
        let templates = registry.list_templates();

        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].uri_template, template.uri_template);
    }

    // === ResourceForwarder tests ===

    #[tokio::test]
    async fn test_resource_forwarder_rediscovery_clears_duplicates() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(ResourceRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = ResourceForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Manually add a resource to test clearing
        {
            let mut reg = registry.lock().await;
            reg.register(mock_resource("github", "file:///test.rs", "test"));
            assert_eq!(reg.len(), 1);
        }

        // discover_resources should clear the registry
        let result = forwarder.discover_resources().await.unwrap();
        assert_eq!(result, 0); // No running services, so 0 discovered

        // Registry should be empty after re-discovery
        {
            let reg = registry.lock().await;
            assert_eq!(reg.len(), 0);
        }
    }

    #[tokio::test]
    async fn test_read_resource_validates_uri() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(ResourceRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = ResourceForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Invalid URI with path traversal
        let result = forwarder.read_resource("file:///../../../etc/passwd").await;
        assert!(matches!(result, Err(ResourceError::InvalidUri(_))));

        // Invalid URI without scheme
        let result = forwarder.read_resource("/etc/passwd").await;
        assert!(matches!(result, Err(ResourceError::InvalidUri(_))));

        // Valid URI but doesn't exist
        let result = forwarder.read_resource("file:///nonexistent.txt").await;
        assert!(matches!(result, Err(ResourceError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_list_resources_namespaces_with_service() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(ResourceRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = ResourceForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Add some resources to the registry
        {
            let mut reg = registry.lock().await;
            reg.register(mock_resource("filesystem", "file:///config.json", "config"));
            reg.register(mock_resource("github", "file:///README.md", "readme"));
        }

        let result = forwarder.list_resources(None, None).await.unwrap();
        assert_eq!(result.resources.len(), 2);

        // Check namespacing
        let filesystem_resource = result.resources.iter()
            .find(|r| r.name.to_string().contains("filesystem"))
            .unwrap();
        assert_eq!(filesystem_resource.name.to_string(), "filesystem:config");
        assert!(filesystem_resource.description.as_ref().map(|d| d.as_str()).unwrap_or("").contains("[from filesystem]"));

        let github_resource = result.resources.iter()
            .find(|r| r.name.to_string().contains("github"))
            .unwrap();
        assert_eq!(github_resource.name.to_string(), "github:readme");
        assert!(github_resource.description.as_ref().map(|d| d.as_str()).unwrap_or("").contains("[from github]"));
    }

    #[tokio::test]
    async fn test_list_resources_filters_by_service() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(ResourceRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = ResourceForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Add resources from multiple services
        {
            let mut reg = registry.lock().await;
            reg.register(mock_resource("filesystem", "file:///config.json", "config"));
            reg.register(mock_resource("filesystem", "file:///data.json", "data"));
            reg.register(mock_resource("github", "file:///README.md", "readme"));
        }

        // Filter by filesystem service
        let result = forwarder.list_resources(Some("filesystem"), None).await.unwrap();
        assert_eq!(result.resources.len(), 2);
        for resource in &result.resources {
            assert!(resource.name.to_string().starts_with("filesystem:"));
        }

        // Filter by github service
        let result = forwarder.list_resources(Some("github"), None).await.unwrap();
        assert_eq!(result.resources.len(), 1);
        assert_eq!(result.resources[0].name.to_string(), "github:readme");

        // Filter by non-existent service
        let result = forwarder.list_resources(Some("nonexistent"), None).await.unwrap();
        assert_eq!(result.resources.len(), 0);
    }

    #[tokio::test]
    async fn test_list_templates_namespaces_with_service() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(ResourceRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = ResourceForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Add some templates to the registry
        {
            let mut reg = registry.lock().await;
            reg.register_template(DiscoveredResourceTemplate {
                uri_template: "file:///{path}".to_string(),
                name: "file-template".to_string(),
                title: Some("File Template".to_string()),
                description: Some("A file template".to_string()),
                mime_type: Some("text/plain".to_string()),
                annotations: None,
                service_id: ServiceId::new("service:filesystem"),
                service_name: ServiceName::new("filesystem"),
            });
            reg.register_template(DiscoveredResourceTemplate {
                uri_template: "git://{repo}/file/{path}".to_string(),
                name: "git-file".to_string(),
                title: Some("Git File".to_string()),
                description: Some("A file from git".to_string()),
                mime_type: Some("text/plain".to_string()),
                annotations: None,
                service_id: ServiceId::new("service:github"),
                service_name: ServiceName::new("github"),
            });
        }

        let result = forwarder.list_templates(None, None).await.unwrap();
        assert_eq!(result.resource_templates.len(), 2);

        // Check namespacing
        let filesystem_template = result.resource_templates.iter()
            .find(|t| t.name.to_string().contains("filesystem"))
            .unwrap();
        assert_eq!(filesystem_template.name.to_string(), "filesystem:file-template");
        assert!(filesystem_template.description.as_ref().map(|d| d.as_str()).unwrap_or("").contains("[from filesystem]"));

        let github_template = result.resource_templates.iter()
            .find(|t| t.name.to_string().contains("github"))
            .unwrap();
        assert_eq!(github_template.name.to_string(), "github:git-file");
        assert!(github_template.description.as_ref().map(|d| d.as_str()).unwrap_or("").contains("[from github]"));
    }

    #[tokio::test]
    async fn test_read_resource_by_namespaced_name() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(ResourceRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = ResourceForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Add a resource to the registry
        {
            let mut reg = registry.lock().await;
            reg.register(mock_resource("filesystem", "file:///config.json", "config"));
        }

        // Reading by namespaced name should work (but will fail at service call since no service running)
        let result = forwarder.read_resource("filesystem:config").await;
        // Should fail with Internal error (service not running) not NotFound
        assert!(matches!(result, Err(ResourceError::Internal(_))));
    }

    #[tokio::test]
    async fn test_read_resource_case_insensitive() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(ResourceRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = ResourceForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Add a resource to the registry
        {
            let mut reg = registry.lock().await;
            reg.register(mock_resource("FileSystem", "file:///config.json", "config"));
        }

        // Case-insensitive lookup should work
        let result = forwarder.read_resource("filesystem:config").await;
        assert!(matches!(result, Err(ResourceError::Internal(_)))); // Service not running, but name was found

        let result = forwarder.read_resource("FILESYSTEM:CONFIG").await;
        assert!(matches!(result, Err(ResourceError::Internal(_)))); // Service not running, but name was found
    }

    #[tokio::test]
    async fn test_list_resources_case_insensitive_filter() {
        let db_config = crate::db::DatabaseConfig {
            url: "memory".to_string(),
            ..Default::default()
        };
        let db = crate::db::create_connection(db_config).await.unwrap();
        crate::db::ensure_schema(&db).await.unwrap();

        let registry = Arc::new(Mutex::new(ResourceRegistry::new()));
        let running_services = Arc::new(Mutex::new(HashMap::new()));
        let forwarder = ResourceForwarder::new(
            registry.clone(),
            running_services.clone(),
            db,
        );

        // Add resources from services with different casing
        {
            let mut reg = registry.lock().await;
            reg.register(mock_resource("FileSystem", "file:///config.json", "config"));
            reg.register(mock_resource("filesystem", "file:///data.json", "data"));
            reg.register(mock_resource("GitHub", "file:///README.md", "readme"));
        }

        // Case-insensitive filter should match regardless of case
        let result = forwarder.list_resources(Some("filesystem"), None).await.unwrap();
        assert_eq!(result.resources.len(), 2);

        let result = forwarder.list_resources(Some("FILESYSTEM"), None).await.unwrap();
        assert_eq!(result.resources.len(), 2);

        let result = forwarder.list_resources(Some("github"), None).await.unwrap();
        assert_eq!(result.resources.len(), 1);
    }
}
