use anyhow::Result;
use clap::{Parser, Subcommand};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{Level, info};
use tracing_subscriber::EnvFilter;
use unicity_orchestrator::{AuthConfig, DatabaseConfig, Orchestrator, create_server};

// rmcp imports for MCP stdio server mode
use rmcp::service::ServiceExt;
use rmcp::transport::stdio;

#[derive(Parser)]
#[command(name = "unicity-orchestrator")]
#[command(about = "MCP Knowledge Graph Orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the orchestrator REST server (Axum HTTP API, not MCP)
    Server {
        #[arg(short, long, default_value = "8080")]
        port: u16,
        /// Bind address for the admin API (internal / trusted only)
        #[arg(long, default_value = "127.0.0.1:8081")]
        admin_bind: String,
        #[arg(long, default_value = "memory")]
        db_url: String,
    },
    /// Discover tools from configured MCP services
    DiscoverTools,
    /// Query for tools
    Query {
        query: String,
        #[arg(short, long)]
        context: Option<String>,
    },
    /// Run as an MCP stdio server (for use in mcp.json)
    McpStdio {
        #[arg(long, default_value = "memory")]
        db_url: String,
    },
    /// Run as an MCP HTTP server
    McpHttp {
        /// Bind address, e.g. 0.0.0.0:8081
        #[arg(long, default_value = "0.0.0.0:3942")]
        bind: String,
        #[arg(long, default_value = "memory")]
        db_url: String,
        /// Allow anonymous access (default: true for backwards compatibility)
        #[arg(long, default_value_t = true)]
        allow_anonymous: bool,
        /// API key for authentication (enables API key auth if provided)
        #[arg(long, env = "ORCHESTRATOR_API_KEY")]
        api_key: Option<String>,
        /// JWKS endpoint URL for JWT RS256 signature verification
        #[arg(long, env = "ORCHESTRATOR_JWKS_URL")]
        jwks_url: Option<String>,
        /// JWT issuer for validation (required when using JWKS)
        #[arg(long, env = "ORCHESTRATOR_JWT_ISSUER")]
        jwt_issuer: Option<String>,
        /// JWT audience for validation
        #[arg(long, env = "ORCHESTRATOR_JWT_AUDIENCE")]
        jwt_audience: Option<String>,
        /// Enable database-backed API key lookup
        #[arg(long, default_value_t = false)]
        enable_db_api_keys: bool,
    },
    /// Initialize the database
    Init {
        #[arg(long, default_value = "memory")]
        db_url: String,
    },
    /// Create a new API key
    CreateApiKey {
        /// Human-readable name for this key
        #[arg(long)]
        name: Option<String>,
        /// Number of days until the key expires (omit for no expiration)
        #[arg(long)]
        expires_days: Option<u32>,
        /// Comma-separated list of scopes for this key
        #[arg(long)]
        scopes: Option<String>,
        #[arg(long, default_value = "memory")]
        db_url: String,
    },
    /// List all API keys
    ListApiKeys {
        #[arg(long, default_value = "memory")]
        db_url: String,
        /// Show only active keys
        #[arg(long, default_value_t = false)]
        active_only: bool,
    },
    /// Revoke an API key by its prefix
    RevokeApiKey {
        /// The key prefix to revoke (e.g., "uo_abc12345")
        key_prefix: String,
        #[arg(long, default_value = "memory")]
        db_url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("unicity_orchestrator=info".parse()?)
                .add_directive("rmcp=warn".parse()?),
        )
        .with_max_level(Level::INFO)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Server {
            port,
            admin_bind,
            db_url,
        } => {
            info!("Starting orchestrator server on port {}", port);
            info!("Starting admin API on {}", admin_bind);

            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };
            info!("Using database url for REST server: {}", db_config.url);

            let mut orchestrator = Orchestrator::new(db_config).await?;
            orchestrator.warmup().await?;

            // Shared orchestrator state for both public and admin routers.
            let shared = Arc::new(Mutex::new(orchestrator));

            let public_app = unicity_orchestrator::api::create_public_router(shared.clone());
            let admin_app = unicity_orchestrator::api::create_admin_router(shared.clone());

            let public_listener =
                tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
            let admin_listener = tokio::net::TcpListener::bind(&admin_bind).await?;

            info!("Public server listening on http://0.0.0.0:{}", port);
            info!("Admin server listening on http://{}", admin_bind);

            tokio::try_join!(
                axum::serve(public_listener, public_app),
                axum::serve(admin_listener, admin_app),
            )?;
        }
        Commands::DiscoverTools => {
            info!("Discovering tools using default database configuration");
            let mut orchestrator = Orchestrator::new(DatabaseConfig::default()).await?;
            let count = orchestrator.discover_tools().await?;
            println!("Discovered {} services and {} tools", count.0, count.1);
        }
        Commands::Query { query, context } => {
            info!(
                "Running query command. query='{}', context_present={}",
                query,
                context.is_some()
            );

            let orchestrator = Orchestrator::new(DatabaseConfig::default()).await?;

            let context_value = context.and_then(|c| serde_json::from_str(&c).ok());

            // CLI query runs without user context (anonymous mode)
            let selections = orchestrator
                .query_tools(&query, context_value, None)
                .await?;

            println!("Query: {}", query);
            println!("Found {} tool selections:", selections.len());

            for selection in selections {
                println!("  Tool: {}", selection.tool_id);
                println!("    Confidence: {:.2}", selection.confidence);
                println!("    Reasoning: {}", selection.reasoning);
                if !selection.dependencies.is_empty() {
                    println!("    Dependencies: {}", selection.dependencies.join(", "));
                }
                println!();
            }
        }
        Commands::McpStdio { db_url } => {
            info!("Starting MCP stdio server (rmcp) with db_url={}", db_url);

            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };
            info!("Using database url for MCP stdio server: {}", db_config.url);

            // Create the full server with tools
            let server = create_server(db_config).await?;

            // Run as an MCP stdio server. McpServer implements ServerHandler.
            let service = server
                .as_ref()
                .clone()
                .serve(stdio())
                .await
                .inspect_err(|e| tracing::error!("serving error: {:?}", e))?;

            // Block until the MCP session ends.
            service.waiting().await?;
            info!("MCP stdio server session ended");
        }
        Commands::McpHttp {
            bind,
            db_url,
            allow_anonymous,
            api_key,
            jwks_url,
            jwt_issuer,
            jwt_audience,
            enable_db_api_keys,
        } => {
            info!(
                "Starting MCP HTTP server (rmcp) on {} with db_url={}",
                bind, db_url
            );

            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };
            info!("Using database url for MCP HTTP server: {}", db_config.url);

            let server = create_server(db_config).await?;

            // Build auth config based on CLI args
            let auth_config = build_auth_config(
                allow_anonymous,
                api_key,
                jwks_url,
                jwt_issuer,
                jwt_audience,
                enable_db_api_keys,
            );

            unicity_orchestrator::server::start_mcp_http(server, &bind, auth_config).await?;
        }
        Commands::Init { db_url } => {
            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };
            info!("Using database url for initialization: {}", db_config.url);

            info!("Initializing database...");
            let db = unicity_orchestrator::create_connection(db_config).await?;
            unicity_orchestrator::ensure_schema(&db).await?;
            info!("Database initialized successfully");
        }
        Commands::CreateApiKey {
            name,
            expires_days,
            scopes,
            db_url,
        } => {
            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };
            let db = unicity_orchestrator::create_connection(db_config).await?;
            unicity_orchestrator::ensure_schema(&db).await?;

            // Generate a new API key
            let (full_key, prefix, key_hash) = unicity_orchestrator::generate_api_key();

            // Calculate expiration if specified
            let expires_at = expires_days.map(|days| {
                let duration = chrono::Duration::days(days as i64);
                chrono::Utc::now() + duration
            });

            // Parse scopes
            let scopes_vec = scopes.map(|s| {
                s.split(',')
                    .map(|scope| scope.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            });

            // Create the API key record
            let api_key_create = unicity_orchestrator::db::ApiKeyCreate {
                key_hash,
                key_prefix: prefix.clone(),
                user_id: None,
                name: name.clone(),
                expires_at: expires_at.map(surrealdb::sql::Datetime::from),
                scopes: scopes_vec,
            };

            unicity_orchestrator::db::QueryBuilder::create_api_key(&db, &api_key_create).await?;

            println!("API Key created successfully!");
            println!();
            println!("  Key:     {}", full_key);
            println!("  Prefix:  {}", prefix);
            if let Some(n) = &name {
                println!("  Name:    {}", n);
            }
            if let Some(exp) = expires_at {
                println!("  Expires: {}", exp.format("%Y-%m-%d %H:%M:%S UTC"));
            } else {
                println!("  Expires: Never");
            }
            println!();
            println!("IMPORTANT: Save this key now. It cannot be retrieved later.");
            println!("Use with: -H 'X-API-Key: {}'", full_key);
        }
        Commands::ListApiKeys {
            db_url,
            active_only,
        } => {
            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };
            let db = unicity_orchestrator::create_connection(db_config).await?;
            unicity_orchestrator::ensure_schema(&db).await?;

            let api_keys = if active_only {
                unicity_orchestrator::db::QueryBuilder::list_active_api_keys(&db).await?
            } else {
                unicity_orchestrator::db::QueryBuilder::list_api_keys(&db).await?
            };

            if api_keys.is_empty() {
                println!("No API keys found.");
                return Ok(());
            }

            println!(
                "{:<20} {:<20} {:<10} {:<25} {:<25}",
                "PREFIX", "NAME", "STATUS", "CREATED", "LAST USED"
            );
            println!("{}", "-".repeat(100));

            for key in api_keys {
                let status = if key.is_active { "Active" } else { "Revoked" };
                let name = key.name.unwrap_or_else(|| "-".to_string());
                let created = key
                    .created_at
                    .map(|dt| dt.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let last_used = key
                    .last_used_at
                    .map(|dt| dt.to_string())
                    .unwrap_or_else(|| "Never".to_string());

                println!(
                    "{:<20} {:<20} {:<10} {:<25} {:<25}",
                    key.key_prefix, name, status, created, last_used
                );
            }
        }
        Commands::RevokeApiKey { key_prefix, db_url } => {
            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };
            let db = unicity_orchestrator::create_connection(db_config).await?;
            unicity_orchestrator::ensure_schema(&db).await?;

            let revoked = unicity_orchestrator::db::QueryBuilder::deactivate_api_key_by_prefix(
                &db,
                &key_prefix,
            )
            .await?;

            if revoked {
                println!("API key '{}' has been revoked.", key_prefix);
            } else {
                println!("No API key found with prefix '{}'.", key_prefix);
            }
        }
    }

    Ok(())
}

/// Build authentication configuration from CLI arguments.
fn build_auth_config(
    allow_anonymous: bool,
    api_key: Option<String>,
    jwks_url: Option<String>,
    jwt_issuer: Option<String>,
    jwt_audience: Option<String>,
    enable_db_api_keys: bool,
) -> Option<AuthConfig> {
    // Check if any authentication method is configured
    let has_api_key = api_key.is_some();
    let has_jwt = jwks_url.is_some() && jwt_issuer.is_some();

    if !has_api_key && !has_jwt && allow_anonymous && !enable_db_api_keys {
        // Default: anonymous mode
        return None;
    }

    let mut config = AuthConfig {
        allow_anonymous,
        ..Default::default()
    };

    // Configure static API key if provided
    if let Some(key) = api_key {
        info!("Static API key authentication enabled");
        config.api_key = Some(key);
    }

    // Configure database-backed API keys
    if enable_db_api_keys {
        info!("Database-backed API key authentication enabled");
        config.db_api_keys_enabled = true;
    }

    // Configure JWT with JWKS (requires both URL and issuer)
    if let (Some(url), Some(issuer)) = (jwks_url, jwt_issuer) {
        info!("JWT authentication enabled (RS256 with JWKS)");
        config.jwt_enabled = true;
        config.jwks_url = Some(url);
        config.jwt_issuer = Some(issuer);
        config.jwt_audience = jwt_audience;
    }

    // Warn if no auth methods configured but anonymous is disabled
    if !has_api_key && !has_jwt && !allow_anonymous && !enable_db_api_keys {
        tracing::warn!(
            "No authentication method configured and anonymous access disabled - all requests will be rejected"
        );
    }

    Some(config)
}
