use serde::Deserialize;
use std::{collections::BTreeMap, env, fs, path::PathBuf};

#[derive(Debug, Deserialize)]
pub struct McpJsonConfig {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct McpServerConfig {
    // stdio server
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,

    // http server
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,

    // flags
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
        id: String,
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        disabled: bool,
        auto_approve: Vec<String>,
        disabled_tools: Vec<String>,
    },
    Http {
        id: String,
        url: String,
        headers: BTreeMap<String, String>,
        disabled: bool,
        auto_approve: Vec<String>,
        disabled_tools: Vec<String>,
    },
}

impl McpServiceConfig {
    pub fn from_json(id: String, cfg: McpServerConfig) -> anyhow::Result<Self> {
        if let Some(cmd) = cfg.command {
            return Ok(McpServiceConfig::Stdio {
                id,
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
                id,
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

    Err(anyhow::anyhow!(
        "Could not find mcp.json (set MCP_CONFIG or create ./mcp.json)"
    ))
}

fn expand_env_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

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

pub fn load_mcp_services() -> anyhow::Result<Vec<McpServiceConfig>> {
    let path = resolve_mcp_json_path()?;
    let raw = fs::read_to_string(&path)?;
    let cfg: McpJsonConfig = serde_json::from_str(&raw)?;

    let mut services = Vec::new();
    for (id, server_cfg) in cfg.mcp_servers {
        let expanded = expand_server(server_cfg);
        services.push(McpServiceConfig::from_json(id, expanded)?);
    }

    Ok(services)
}
