use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use surrealdb::engine::any::Any;
use surrealdb::Surreal;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    pub id: String,
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    pub auth_token: Option<String>,
    pub sync_interval: Duration,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryManifest {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub mcp_version: String,
    pub schema_version: String,
    pub manifest_url: String,
    pub download_url: String,
    pub checksum: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<AuthorInfo>,
    pub license: Option<String>,
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorInfo {
    pub name: String,
    pub email: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub optional: bool,
}

#[async_trait]
pub trait RegistryProvider: Send + Sync {
    async fn list_manifests(&self) -> Result<Vec<RegistryManifest>>;
    async fn get_manifest(&self, name: &str, version: &str) -> Result<Option<RegistryManifest>>;
    async fn download_manifest(&self, manifest: &RegistryManifest) -> Result<serde_json::Value>;
    async fn verify_manifest(&self, manifest: &RegistryManifest, content: &[u8]) -> Result<bool>;
}

pub struct McpRegistryManager {
    db: Surreal<Any>,
    registries: HashMap<String, Box<dyn RegistryProvider>>,
    client: Client,
}

impl McpRegistryManager {
    pub fn new(db: Surreal<Any>) -> Self {
        Self {
            db,
            registries: HashMap::new(),
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("unicity-orchestrator/0.1.0")
                .build()
                .unwrap(),
        }
    }

    pub async fn add_registry(&mut self, config: RegistryConfig) -> Result<()> {
        info!("Adding registry: {}", config.name);

        // Store in database
        let registry_id = surrealdb::sql::Id::from(config.id.clone());
        let query = r#"
        UPSERT registry SET
            id = $id,
            url = $url,
            name = $name,
            description = $description,
            is_active = $active,
            created_at = time::now(),
            updated_at = time::now()
        RETURN id
        "#;

        let mut result = self.db
            .query(query)
            .bind(("id", registry_id))
            .bind(("url", config.url.clone()))
            .bind(("name", config.name.clone()))
            .bind(("description", config.description.clone()))
            .bind(("active", config.is_active))
            .await?;

        let _registry_id: Option<String> = result.take("id")?;

        // Create provider based on URL pattern
        let provider: Box<dyn RegistryProvider> = if config.url.contains("github.com") {
            Box::new(GitHubRegistryProvider::new(config.clone(), self.client.clone()))
        } else if config.url.contains("npm") {
            Box::new(NpmRegistryProvider::new(config.clone(), self.client.clone()))
        } else {
            Box::new(HttpRegistryProvider::new(config.clone(), self.client.clone()))
        };

        self.registries.insert(config.id, provider);
        Ok(())
    }

    pub async fn remove_registry(&mut self, registry_id: String) -> Result<()> {
        info!("Removing registry: {}", registry_id);

        // Remove from memory
        self.registries.remove(&registry_id);
        Ok(())
    }

    pub async fn sync_all_registries(&mut self) -> Result<SyncResult> {
        info!("Starting sync of all registries");
        let mut total_manifests = 0;
        let mut new_manifests = 0;
        let mut updated_manifests = 0;
        let mut errors = Vec::new();

        let registry_ids: Vec<String> = self.registries.keys().cloned().collect();

        for registry_id in registry_ids {
            match self.sync_registry(&registry_id).await {
                Ok(result) => {
                    total_manifests += result.total_manifests;
                    new_manifests += result.new_manifests;
                    updated_manifests += result.updated_manifests;
                    info!("Registry {} synced: {} manifests", registry_id, result.total_manifests);
                }
                Err(e) => {
                    error!("Failed to sync registry {}: {}", registry_id, e);
                    errors.push((registry_id, e.to_string()));
                }
            }
        }

        info!("Sync complete: {} total, {} new, {} updated, {} errors",
              total_manifests, new_manifests, updated_manifests, errors.len());

        Ok(SyncResult {
            total_manifests,
            new_manifests,
            updated_manifests,
            errors,
        })
    }

    pub async fn sync_registry(&mut self, registry_id: &str) -> Result<RegistrySyncResult> {
        let provider = self.registries.get(registry_id)
            .ok_or_else(|| anyhow::anyhow!("Registry not found: {}", registry_id))?;

        debug!("Syncing registry: {}", registry_id);

        // Get manifests from registry
        let manifests = provider.list_manifests().await?;
        let mut total_manifests = 0;
        let mut new_manifests = 0;
        let mut updated_manifests = 0;

        for manifest in manifests {
            total_manifests += 1;

            // Check if manifest already exists
            let existing = self.get_manifest(manifest.name.clone(), manifest.version.clone()).await?;

            if existing.is_none() {
                // Download and store new manifest
                match provider.download_manifest(&manifest).await {
                    Ok(content) => {
                        if let Err(e) = self.store_manifest(registry_id.to_string(), manifest.clone(), content).await {
                            error!("Failed to store manifest {} {}: {}",
                                   manifest.name, manifest.version, e);
                        } else {
                            new_manifests += 1;
                        }
                    }
                    Err(e) => {
                        error!("Failed to download manifest {}: {}",
                               manifest.name, e);
                    }
                }
            } else {
                // Check for updates
                if self.should_update_manifest(manifest.clone()).await? {
                    match provider.download_manifest(&manifest).await {
                        Ok(content) => {
                            if let Err(e) = self.update_manifest(registry_id.to_string(), manifest.clone(), content).await {
                                error!("Failed to update manifest {} {}: {}",
                                       &manifest.name, &manifest.version, e);
                            } else {
                                updated_manifests += 1;
                            }
                        }
                        Err(e) => {
                            error!("Failed to download updated manifest {} {}: {}",
                                   manifest.name, manifest.version, e);
                        }
                    }
                }
            }
        }

        // Update last sync time
        self.update_registry_sync_time(registry_id.to_string()).await?;

        Ok(RegistrySyncResult {
            total_manifests,
            new_manifests,
            updated_manifests,
        })
    }

    pub async fn search_manifests(
        &self,
        query: &str,
        registry_id: Option<&str>,
        tags: Option<Vec<String>>,
    ) -> Result<Vec<RegistryManifest>> {
        let mut conditions = Vec::new();

        if let Some(rid) = registry_id {
            conditions.push(format!("manifest.registry_id = record('registry', '{}')", rid));
        }

        if !query.is_empty() {
            conditions.push(format!("manifest.name ~ ${} OR manifest.description ~ ${}",
                                  query, query));
        }

        if let Some(tag_list) = tags {
            for tag in tag_list {
                conditions.push(format!("manifest.tags CONTAINS '{}'", tag));
            }
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let query_str = format!(
            r#"
            SELECT manifest.*
            FROM manifest
            {}
            ORDER BY manifest.name ASC
            LIMIT 100
            "#,
            where_clause
        );

        let mut result = self.db.query(&query_str).await?;
        let manifests: Vec<RegistryManifest> = result.take("manifest")?;

        Ok(manifests)
    }

    async fn get_manifest(&self, name: String, version: String) -> Result<Option<RegistryManifest>> {
        let query = r#"
        SELECT * FROM manifest
        WHERE name = $name AND version = $version
        LIMIT 1
        "#;

        let mut result = self.db
            .query(query)
            .bind(("name", name))
            .bind(("version", version))
            .await?;

        let manifest: Option<RegistryManifest> = result.take(0)?;
        Ok(manifest)
    }

    async fn should_update_manifest(&self, manifest: RegistryManifest) -> Result<bool> {
        // Implement update logic based on checksum, version, etc.
        // For now, always update if checksum is provided and different
        if let Some(checksum) = &manifest.checksum {
            let query = r#"
            SELECT checksum FROM manifest
            WHERE name = $name AND version = $version
            LIMIT 1
            "#;

            let mut result = self.db
                .query(query)
                .bind(("name", manifest.name))
                .bind(("version", manifest.version))
                .await?;

            let stored: Option<String> = result.take("checksum")?;
            if let Some(stored_value) = stored {
                return Ok(stored_value != *checksum);
            }
        }
        Ok(false)
    }

    async fn store_manifest(
        &self,
        registry_id: String,
        manifest: RegistryManifest,
        content: serde_json::Value,
    ) -> Result<()> {
        let registry_thing = surrealdb::sql::Thing::from((String::from("registry"), registry_id));

        let query = r#"
        CREATE manifest SET
            registry_id = $registry_id,
            name = $name,
            version = $version,
            description = $description,
            content = $content,
            hash = $hash,
            checksum = $checksum,
            tags = $tags,
            is_active = true,
            created_at = time::now()
        "#;

        let hash = self.calculate_hash(&content);

        self.db
            .query(query)
            .bind(("registry_id", registry_thing))
            .bind(("name", manifest.name.clone()))
            .bind(("version", manifest.version.clone()))
            .bind(("description", manifest.description.clone()))
            .bind(("content", content))
            .bind(("hash", hash.clone()))
            .bind(("checksum", manifest.checksum.clone()))
            .bind(("tags", manifest.tags.clone()))
            .await?;

        Ok(())
    }

    async fn update_manifest(
        &self,
        registry_id: String,
        manifest: RegistryManifest,
        content: serde_json::Value,
    ) -> Result<()> {
        let query = r#"
        UPDATE manifest SET
            content = $content,
            hash = $hash,
            checksum = $checksum,
            tags = $tags,
            updated_at = time::now()
        WHERE name = $name AND version = $version
        "#;

        let hash = self.calculate_hash(&content);

        self.db
            .query(query)
            .bind(("content", content))
            .bind(("hash", hash))
            .bind(("checksum", manifest.checksum))
            .bind(("tags", manifest.tags))
            .bind(("name", manifest.name))
            .bind(("version", manifest.version))
            .await?;

        Ok(())
    }

    async fn update_registry_sync_time(&self, registry_id: String) -> Result<()> {
        let query = r#"
        UPDATE registry SET
            last_sync = time::now()
        WHERE id = $registry_id
        "#;

        self.db
            .query(query)
            .bind(("registry_id", registry_id))
            .await?;

        Ok(())
    }

    fn calculate_hash(&self, content: &serde_json::Value) -> String {
        use sha2::{Digest, Sha256};
        let content_str = serde_json::to_string(content).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(content_str.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub total_manifests: usize,
    pub new_manifests: usize,
    pub updated_manifests: usize,
    pub errors: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct RegistrySyncResult {
    pub total_manifests: usize,
    pub new_manifests: usize,
    pub updated_manifests: usize,
}

// Registry Provider Implementations

pub struct HttpRegistryProvider {
    config: RegistryConfig,
    client: Client,
}

impl HttpRegistryProvider {
    pub fn new(config: RegistryConfig, client: Client) -> Self {
        Self { config, client }
    }
}

#[async_trait]
impl RegistryProvider for HttpRegistryProvider {
    async fn list_manifests(&self) -> Result<Vec<RegistryManifest>> {
        let url = format!("{}/manifests", self.config.url);
        let response = self.client.get(&url).send().await?;

        if response.status().is_success() {
            let manifests: Vec<RegistryManifest> = response.json().await?;
            Ok(manifests)
        } else {
            Err(anyhow::anyhow!("Failed to list manifests: {}", response.status()))
        }
    }

    async fn get_manifest(&self, name: &str, version: &str) -> Result<Option<RegistryManifest>> {
        let url = format!("{}/manifests/{}/{}", self.config.url, name, version);

        match self.client.get(&url).send().await {
            Ok(response) if response.status().is_success() => {
                let manifest: RegistryManifest = response.json().await?;
                Ok(Some(manifest))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn download_manifest(&self, manifest: &RegistryManifest) -> Result<serde_json::Value> {
        let response = self.client.get(&manifest.manifest_url).send().await?;

        if response.status().is_success() {
            let content: serde_json::Value = response.json().await?;
            Ok(content)
        } else {
            Err(anyhow::anyhow!("Failed to download manifest: {}", response.status()))
        }
    }

    async fn verify_manifest(&self, manifest: &RegistryManifest, content: &[u8]) -> Result<bool> {
        if let Some(checksum) = &manifest.checksum {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(content);
            let calculated = format!("{:x}", hasher.finalize());
            Ok(calculated == *checksum)
        } else {
            Ok(true) // No checksum to verify against
        }
    }
}

pub struct GitHubRegistryProvider {
    config: RegistryConfig,
    client: Client,
}

impl GitHubRegistryProvider {
    pub fn new(config: RegistryConfig, client: Client) -> Self {
        Self { config, client }
    }
}

#[async_trait]
impl RegistryProvider for GitHubRegistryProvider {
    async fn list_manifests(&self) -> Result<Vec<RegistryManifest>> {
        // Parse GitHub URL and search for mcp.json files
        let url = if self.config.url.ends_with(".json") {
            self.config.url.clone()
        } else {
            format!("{}/tree/main", self.config.url)
        };

        // For now, implement a basic search
        // In a real implementation, you'd use GitHub API to search for repositories with mcp.json
        let manifests: Vec<RegistryManifest> = vec![];
        Ok(manifests)
    }

    async fn get_manifest(&self, name: &str, version: &str) -> Result<Option<RegistryManifest>> {
        // Implementation for GitHub
        Ok(None)
    }

    async fn download_manifest(&self, manifest: &RegistryManifest) -> Result<serde_json::Value> {
        let response = self.client.get(&manifest.manifest_url).send().await?;

        if response.status().is_success() {
            let content: serde_json::Value = response.json().await?;
            Ok(content)
        } else {
            Err(anyhow::anyhow!("Failed to download manifest from GitHub: {}", response.status()))
        }
    }

    async fn verify_manifest(&self, manifest: &RegistryManifest, content: &[u8]) -> Result<bool> {
        if let Some(checksum) = &manifest.checksum {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(content);
            let calculated = format!("{:x}", hasher.finalize());
            Ok(calculated == *checksum)
        } else {
            Ok(true)
        }
    }
}

pub struct NpmRegistryProvider {
    config: RegistryConfig,
    client: Client,
}

impl NpmRegistryProvider {
    pub fn new(config: RegistryConfig, client: Client) -> Self {
        Self { config, client }
    }
}

#[async_trait]
impl RegistryProvider for NpmRegistryProvider {
    async fn list_manifests(&self) -> Result<Vec<RegistryManifest>> {
        // Search npm for MCP packages
        let url = format!("{}/-/v1/search", self.config.url);

        let mut params = HashMap::new();
        params.insert("text", "mcp");
        params.insert("size", "100");

        let response = self.client.get(&url).query(&params).send().await?;

        if response.status().is_success() {
            let search_result: serde_json::Value = response.json().await?;
            let manifests = self.parse_npm_search_result(search_result)?;
            Ok(manifests)
        } else {
            Err(anyhow::anyhow!("Failed to search npm registry: {}", response.status()))
        }
    }

    async fn get_manifest(&self, name: &str, version: &str) -> Result<Option<RegistryManifest>> {
        let url = format!("{}/{}", self.config.url, name);

        match self.client.get(&url).send().await {
            Ok(response) if response.status().is_success() => {
                let package: serde_json::Value = response.json().await?;
                let manifest = self.parse_npm_package(name, version, package)?;
                Ok(Some(manifest))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn download_manifest(&self, manifest: &RegistryManifest) -> Result<serde_json::Value> {
        let response = self.client.get(&manifest.manifest_url).send().await?;

        if response.status().is_success() {
            let content: serde_json::Value = response.json().await?;
            Ok(content)
        } else {
            Err(anyhow::anyhow!("Failed to download manifest from npm: {}", response.status()))
        }
    }

    async fn verify_manifest(&self, manifest: &RegistryManifest, content: &[u8]) -> Result<bool> {
        if let Some(checksum) = &manifest.checksum {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(content);
            let calculated = format!("{:x}", hasher.finalize());
            Ok(calculated == *checksum)
        } else {
            Ok(true)
        }
    }
}

impl NpmRegistryProvider {
    fn parse_npm_search_result(&self, result: serde_json::Value) -> Result<Vec<RegistryManifest>> {
        let mut manifests = Vec::new();

        if let Some(objects) = result.get("objects").and_then(|o| o.as_array()) {
            for obj in objects {
                if let Some(package) = obj.get("package") {
                    if let Some(name) = package.get("name").and_then(|n| n.as_str()) {
                        if name.contains("mcp") || name.contains("model-context-protocol") {
                            if let Some(manifest) = self.parse_npm_package(
                                name,
                                "latest",
                                package.clone()
                            ).ok() {
                                manifests.push(manifest);
                            }
                        }
                    }
                }
            }
        }

        Ok(manifests)
    }

    fn parse_npm_package(
        &self,
        name: &str,
        version: &str,
        package: serde_json::Value,
    ) -> Result<RegistryManifest> {
        let description = package.get("description")
            .and_then(|d| d.as_str())
            .map(|s| s.to_string());

        let latest_version = package.get("dist-tags")
            .and_then(|tags| tags.get("latest"))
            .and_then(|v| v.as_str())
            .unwrap_or(version);

        let dist = package.get("versions")
            .and_then(|v| v.get(latest_version))
            .and_then(|ver| ver.get("dist"))
            .ok_or_else(|| anyhow::anyhow!("No distribution info found"))?;

        let tarball_url = dist.get("tarball")
            .and_then(|url| url.as_str())
            .ok_or_else(|| anyhow::anyhow!("No tarball URL found"))?;

        let checksum = dist.get("shasum")
            .and_then(|sum| sum.as_str())
            .map(|s| s.to_string());

        Ok(RegistryManifest {
            name: name.to_string(),
            version: latest_version.to_string(),
            description,
            mcp_version: "2025-11-25".to_string(), // Assume latest
            schema_version: "1.0.0".to_string(),
            manifest_url: tarball_url.to_string(),
            download_url: tarball_url.to_string(),
            checksum,
            tags: vec!["npm".to_string()],
            author: None,
            license: package.get("license")
                .and_then(|l| l.as_str())
                .map(|s| s.to_string()),
            dependencies: vec![], // Would need to parse from package.json
        })
    }
}