use crate::types::ServiceConfigId;
use serde::Deserialize;
use std::{collections::BTreeMap, env, fs, path::PathBuf};

#[derive(Debug, Deserialize)]
pub struct McpJsonConfig {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct McpServerConfig {
    // STDIO server
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,

    // HTTP server
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,

    // Flags
    #[serde(default)]
    pub disabled: bool,
    #[serde(default, rename = "autoApprove")]
    pub auto_approve: Vec<String>,
    #[serde(default, rename = "disabled_tools")]
    pub disabled_tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum McpServiceConfig {
    Stdio {
        id: ServiceConfigId,
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        disabled: bool,
        auto_approve: Vec<String>,
        disabled_tools: Vec<String>,
    },
    Http {
        id: ServiceConfigId,
        url: String,
        headers: BTreeMap<String, String>,
        disabled: bool,
        auto_approve: Vec<String>,
        disabled_tools: Vec<String>,
    },
}

impl McpServiceConfig {
    pub fn from_json(id: String, cfg: McpServerConfig) -> anyhow::Result<Self> {
        let service_id = ServiceConfigId::new(&id);
        if let Some(cmd) = cfg.command {
            return Ok(McpServiceConfig::Stdio {
                id: service_id,
                command: cmd,
                args: cfg.args,
                env: cfg.env,
                disabled: cfg.disabled,
                auto_approve: cfg.auto_approve,
                disabled_tools: cfg.disabled_tools,
            });
        }

        if let Some(url) = cfg.url {
            return Ok(McpServiceConfig::Http {
                id: service_id,
                url,
                headers: cfg.headers,
                disabled: cfg.disabled,
                auto_approve: cfg.auto_approve,
                disabled_tools: cfg.disabled_tools,
            });
        }

        Err(anyhow::anyhow!(
            "Server `{}` must have either `command` or `url`",
            id
        ))
    }
}

pub fn resolve_mcp_json_path() -> anyhow::Result<PathBuf> {
    if let Ok(p) = env::var("MCP_CONFIG") {
        return Ok(PathBuf::from(p));
    }

    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        let candidate = PathBuf::from(xdg).join("mcp").join("mcp.json");
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    let candidate = PathBuf::from("mcp.json");
    if candidate.exists() {
        return Ok(candidate);
    }

    let default_path = PathBuf::from("mcp.json");
    let default_contents = r#"{ "mcpServers": {} }"#;
    if let Err(e) = fs::write(&default_path, default_contents) {
        return Err(anyhow::anyhow!("Failed to create default mcp.json: {e}"));
    }
    tracing::info!("Created default mcp.json at {:?}", default_path);
    Ok(default_path)
}

fn expand_env_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    #[allow(clippy::while_let_on_iterator)]
    while let Some(ch) = chars.next() {
        if ch == '$' && matches!(chars.peek(), Some('{')) {
            chars.next(); // consume '{'
            let mut name = String::new();
            while let Some(c) = chars.next() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
            if let Ok(val) = env::var(&name) {
                out.push_str(&val);
            } else {
                out.push_str("${");
                out.push_str(&name);
                out.push('}');
            }
        } else {
            out.push(ch);
        }
    }

    out
}

fn expand_server(cfg: McpServerConfig) -> McpServerConfig {
    let mut cfg = cfg;

    for val in cfg.env.values_mut() {
        *val = expand_env_vars(val);
    }
    for val in cfg.headers.values_mut() {
        *val = expand_env_vars(val);
    }
    if let Some(cmd) = cfg.command.as_mut() {
        *cmd = expand_env_vars(cmd);
    }
    cfg.args = cfg.args.into_iter().map(|a| expand_env_vars(&a)).collect();
    if let Some(url) = cfg.url.as_mut() {
        *url = expand_env_vars(url);
    }

    cfg
}

pub struct McpConfigs(pub Vec<McpServiceConfig>);

impl McpConfigs {
    pub fn load() -> anyhow::Result<Self> {
        let path = resolve_mcp_json_path()?;
        Self::load_from_path(path)
    }

    pub fn load_from_path(path: PathBuf) -> anyhow::Result<Self> {
        let config: McpJsonConfig = serde_json::from_str(&fs::read_to_string(&path)?)?;
        Self::load_from_config(config)
    }

    pub fn load_from_config(cfg: McpJsonConfig) -> anyhow::Result<Self> {
        let mut services = Vec::new();
        for (id, server_cfg) in cfg.mcp_servers {
            let expanded = expand_server(server_cfg);
            services.push(McpServiceConfig::from_json(id, expanded)?);
        }

        Ok(Self(services))
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Iterator for McpConfigs {
    type Item = McpServiceConfig;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            None
        } else {
            Some(self.0.remove(0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    #[test]
    fn test_mcp_service_config_from_json_stdio() {
        let id = "test-server".to_string();
        let cfg = McpServerConfig {
            command: Some("node".to_string()),
            args: vec!["server.js".to_string()],
            env: BTreeMap::from([("PATH".to_string(), "/usr/bin".to_string())]),
            url: None,
            headers: BTreeMap::new(),
            disabled: false,
            auto_approve: vec!["tool1".to_string()],
            disabled_tools: vec!["tool2".to_string()],
        };

        let result = McpServiceConfig::from_json(id, cfg).unwrap();

        match result {
            McpServiceConfig::Stdio {
                id,
                command,
                args,
                env,
                disabled,
                auto_approve,
                disabled_tools,
            } => {
                assert_eq!(id.as_str(), "test-server");
                assert_eq!(command, "node");
                assert_eq!(*args, vec!["server.js".to_string()]);
                assert_eq!(env.get("PATH"), Some(&"/usr/bin".to_string()));
                assert!(!disabled);
                assert_eq!(auto_approve, vec!["tool1"]);
                assert_eq!(disabled_tools, vec!["tool2"]);
            }
            _ => panic!("Expected Stdio variant"),
        }
    }

    #[test]
    fn test_mcp_service_config_from_json_http() {
        let id = "test-server".to_string();
        let cfg = McpServerConfig {
            command: None,
            args: vec![],
            env: BTreeMap::new(),
            url: Some("http://localhost:3000".to_string()),
            headers: BTreeMap::from([("Authorization".to_string(), "Bearer token".to_string())]),
            disabled: true,
            auto_approve: vec![],
            disabled_tools: vec!["tool3".to_string()],
        };

        let result = McpServiceConfig::from_json(id, cfg).unwrap();

        match result {
            McpServiceConfig::Http {
                id,
                url,
                headers,
                disabled,
                auto_approve,
                disabled_tools,
            } => {
                assert_eq!(id.as_str(), "test-server");
                assert_eq!(url, "http://localhost:3000");
                assert_eq!(
                    headers.get("Authorization"),
                    Some(&"Bearer token".to_string())
                );
                assert!(disabled);
                assert_eq!(auto_approve, Vec::<String>::new());
                assert_eq!(disabled_tools, vec!["tool3"]);
            }
            _ => panic!("Expected Http variant"),
        }
    }

    #[test]
    fn test_mcp_service_config_from_json_error() {
        let id = "test-server".to_string();
        let cfg = McpServerConfig {
            command: None,
            args: vec![],
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
            disabled: false,
            auto_approve: vec![],
            disabled_tools: vec![],
        };

        let result = McpServiceConfig::from_json(id, cfg);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must have either `command` or `url`")
        );
    }

    #[test]
    fn test_expand_env_vars() {
        // Test with existing environment variable
        unsafe {
            env::set_var("TEST_VAR", "test_value");
        }

        let input = "prefix ${TEST_VAR} suffix";
        let result = expand_env_vars(input);
        assert_eq!(result, "prefix test_value suffix");

        // Test with non-existing environment variable
        let input = "prefix ${NON_EXISTENT} suffix";
        let result = expand_env_vars(input);
        assert_eq!(result, "prefix ${NON_EXISTENT} suffix");

        // Test with no variables
        let input = "plain string";
        let result = expand_env_vars(input);
        assert_eq!(result, "plain string");

        // Test with multiple variables
        unsafe {
            env::set_var("VAR1", "value1");
            env::set_var("VAR2", "value2");
        }
        let input = "${VAR1}-${VAR2}";
        let result = expand_env_vars(input);
        assert_eq!(result, "value1-value2");

        // Clean up
        unsafe {
            env::remove_var("TEST_VAR");
            env::remove_var("VAR1");
            env::remove_var("VAR2");
        }
    }

    #[test]
    fn test_expand_server() {
        unsafe {
            env::set_var("HOME", "/home/user");
        }

        let mut env = BTreeMap::new();
        env.insert("HOME_DIR".to_string(), "${HOME}".to_string());

        let mut headers = BTreeMap::new();
        headers.insert("Auth".to_string(), "Bearer ${HOME}/token".to_string());

        let cfg = McpServerConfig {
            command: Some("${HOME}/bin/server".to_string()),
            args: vec!["--config".to_string(), "${HOME}/config.json".to_string()],
            env,
            url: Some("http://${HOME}:3000".to_string()),
            headers,
            disabled: false,
            auto_approve: vec![],
            disabled_tools: vec![],
        };

        let result = expand_server(cfg);

        assert_eq!(result.command.unwrap(), "/home/user/bin/server");
        assert_eq!(result.args, vec!["--config", "/home/user/config.json"]);
        assert_eq!(result.env.get("HOME_DIR"), Some(&"/home/user".to_string()));
        assert_eq!(result.url.unwrap(), "http:///home/user:3000");
        assert_eq!(
            result.headers.get("Auth"),
            Some(&"Bearer /home/user/token".to_string())
        );

        unsafe {
            env::remove_var("HOME");
        }
    }

    #[test]
    fn test_resolve_mcp_json_path_with_env_var() {
        // Store original values and clean environment
        let original_mcp_config = env::var("MCP_CONFIG").ok();
        let original_xdg_config = env::var("XDG_CONFIG_HOME").ok();

        unsafe {
            env::remove_var("MCP_CONFIG");
            env::remove_var("XDG_CONFIG_HOME");
            env::set_var("MCP_CONFIG", "/custom/path/mcp.json");
        }

        let result = resolve_mcp_json_path().unwrap();
        assert_eq!(result.to_string_lossy(), "/custom/path/mcp.json");

        // Restore original values
        unsafe {
            env::remove_var("MCP_CONFIG");
            match original_mcp_config {
                Some(val) => env::set_var("MCP_CONFIG", val),
                None => env::remove_var("MCP_CONFIG"),
            }
            match original_xdg_config {
                Some(val) => env::set_var("XDG_CONFIG_HOME", val),
                None => env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    #[test]
    fn test_resolve_mcp_json_path_creates_default() {
        let temp_dir = TempDir::new().unwrap();
        env::set_current_dir(&temp_dir).unwrap();

        // Clear any existing MCP_CONFIG environment variable
        unsafe {
            env::remove_var("MCP_CONFIG");
        }
        unsafe {
            env::remove_var("XDG_CONFIG_HOME");
        }

        // Ensure no mcp.json exists
        assert!(!PathBuf::from("mcp.json").exists());

        let result = resolve_mcp_json_path().unwrap();
        assert_eq!(result.file_name().unwrap(), "mcp.json");
        assert!(result.exists());

        // Check that default content was written
        let content = fs::read_to_string(&result).unwrap();
        assert!(content.contains("mcpServers"));
        // The default creates an empty object for mcpServers
        assert!(content.contains("{}"));
    }

    #[test]
    fn test_load_mcp_services() {
        let mcp_json_content = serde_json::json!({
            "mcpServers": {
                "stdio-server": {
                    "command": "node",
                    "args": ["server.js"],
                    "env": {
                        "NODE_ENV": "production"
                    }
                },
                "http-server": {
                    "url": "http://localhost:3000",
                    "headers": {
                        "Authorization": "Bearer token123"
                    },
                    "disabled": true
                }
            }
        });

        let config: McpJsonConfig = serde_json::from_value(mcp_json_content).unwrap();
        let result = McpConfigs::load_from_config(config).unwrap();
        assert_eq!(result.len(), 2);

        // Find stdio and http servers regardless of order
        let stdio_server = result
            .0
            .iter()
            .find(|s| matches!(s, McpServiceConfig::Stdio { .. }));
        let http_server = result
            .0
            .iter()
            .find(|s| matches!(s, McpServiceConfig::Http { .. }));

        // Check stdio server
        match stdio_server {
            Some(McpServiceConfig::Stdio {
                id,
                command,
                args,
                env,
                disabled,
                ..
            }) => {
                assert_eq!(id.as_str(), "stdio-server");
                assert_eq!(command, "node");
                assert_eq!(*args, vec!["server.js".to_string()]);
                assert_eq!(env.get("NODE_ENV"), Some(&"production".to_string()));
                assert!(!disabled);
            }
            _ => panic!("Expected Stdio variant"),
        }

        // Check http server
        match http_server {
            Some(McpServiceConfig::Http {
                id,
                url,
                headers,
                disabled,
                ..
            }) => {
                assert_eq!(id.as_str(), "http-server");
                assert_eq!(url, "http://localhost:3000");
                assert_eq!(
                    headers.get("Authorization"),
                    Some(&"Bearer token123".to_string())
                );
                assert!(disabled);
            }
            _ => panic!("Expected Http variant"),
        }
    }
}
