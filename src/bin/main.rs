use clap::{Parser, Subcommand};
use anyhow::Result;
use tracing::{info, Level};
use tracing_subscriber::EnvFilter;
use unicity_orchestrator::{DatabaseConfig, UnicityOrchestrator};

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
        #[arg(long, default_value = "memory")]
        db_url: String,
    },
    /// Sync MCP registries
    SyncRegistries,
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
    /// Run as an MCP HTTP server (placeholder for rmcp HTTP/SSE transport)
    McpHttp {
        /// Bind address, e.g. 0.0.0.0:8081
        #[arg(long, default_value = "0.0.0.0:8081")]
        bind: String,
        #[arg(long, default_value = "memory")]
        db_url: String,
    },
    /// Run as an MCP SSE server (placeholder for rmcp SSE transport)
    McpSse {
        /// Bind address, e.g. 0.0.0.0:8082
        #[arg(long, default_value = "0.0.0.0:8082")]
        bind: String,
        #[arg(long, default_value = "memory")]
        db_url: String,
    },
    /// Initialize the database
    Init {
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
        Commands::Server { port, db_url } => {
            info!("Starting orchestrator server on port {}", port);

            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };

            let mut orchestrator = UnicityOrchestrator::new(db_config).await?;
            orchestrator.warmup().await?;

            // Create and run the web server
            let app = unicity_orchestrator::api::create_router(orchestrator);

            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
            info!("Server listening on http://0.0.0.0:{}", port);

            axum::serve(listener, app).await?;
        }
        Commands::SyncRegistries => {
            let mut orchestrator = UnicityOrchestrator::new(DatabaseConfig::default()).await?;
            let result = orchestrator.sync_registries().await?;

            println!("Sync complete:");
            println!("  Total manifests: {}", result.total_manifests);
            println!("  New manifests: {}", result.new_manifests);
            println!("  Updated manifests: {}", result.updated_manifests);
            if !result.errors.is_empty() {
                println!("  Errors: {}", result.errors.len());
            }
        }
        Commands::DiscoverTools => {
            let mut orchestrator = UnicityOrchestrator::new(DatabaseConfig::default()).await?;
            let count = orchestrator.discover_tools().await?;
            println!("Discovered {} services and {} tools", count.0, count.1);
        }
        Commands::Query { query, context } => {
            let mut orchestrator = UnicityOrchestrator::new(DatabaseConfig::default()).await?;

            let context_value = context
                .map(|c| serde_json::from_str(&c).ok())
                .flatten();

            let selections = orchestrator.query_tools(&query, context_value).await?;

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

            // Initialize orchestrator state and schema
            let mut orchestrator = UnicityOrchestrator::new(db_config).await?;
            orchestrator.warmup().await?;

            // Run as an MCP stdio server. This assumes `UnicityOrchestrator`
            // implements `ServerHandler` from rmcp.
            let service = orchestrator
                .serve(stdio())
                .await
                .inspect_err(|e| tracing::error!("serving error: {:?}", e))?;

            // Block until the MCP session ends.
            service.waiting().await?;
        }
        Commands::McpHttp { bind, db_url } => {
            info!("Starting MCP HTTP server (rmcp) on {} with db_url={}", bind, db_url);

            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };

            let mut orchestrator = UnicityOrchestrator::new(db_config).await?;
            orchestrator.warmup().await?;

            // TODO: Integrate rmcp's HTTP/SSE transport here. For now, this is a
            // placeholder so the CLI surface is in place. See rmcp documentation
            // for creating an HTTP/SSE transport and pass it to `orchestrator.serve(...)`.
            eprintln!(
                "McpHttp mode is not yet implemented. Please plug in rmcp's HTTP/SSE transport here."
            );
        }
        Commands::McpSse { bind, db_url } => {
            info!("Starting MCP SSE server (rmcp) on {} with db_url={}", bind, db_url);

            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };

            let mut orchestrator = UnicityOrchestrator::new(db_config).await?;
            orchestrator.warmup().await?;

            // TODO: Integrate rmcp's SSE transport here. For now, this is a placeholder
            // so the CLI surface is in place.
            eprintln!(
                "McpSse mode is not yet implemented. Please plug in rmcp's SSE transport here."
            );
        }
        Commands::Init { db_url } => {
            let db_config = DatabaseConfig {
                url: db_url,
                ..Default::default()
            };

            info!("Initializing database...");
            let db = unicity_orchestrator::create_connection(db_config).await?;
            unicity_orchestrator::ensure_schema(&db).await?;
            info!("Database initialized successfully");
        }
    }

    Ok(())
}
