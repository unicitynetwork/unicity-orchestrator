use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::env;
use surrealdb::engine::any::Any;
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;

pub type Db = Surreal<Any>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub namespace: String,
    pub database: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: env::var("SURREALDB_URL")
                .unwrap_or_else(|_| "memory".to_string()),
            namespace: env::var("SURREALDB_NAMESPACE")
                .unwrap_or_else(|_| "unicity".to_string()),
            database: env::var("SURREALDB_DATABASE")
                .unwrap_or_else(|_| "orchestrator".to_string()),
            username: env::var("SURREALDB_USERNAME").ok(),
            password: env::var("SURREALDB_PASSWORD").ok(),
        }
    }
}

pub async fn create_connection(config: DatabaseConfig) -> Result<Db> {
    let db = surrealdb::engine::any::connect(config.url).await?;

    // Sign in if credentials are provided
    if let (Some(username), Some(password)) = (config.username, config.password) {
        db.signin(Root {
            username: &username,
            password: &password,
        })
        .await?;
    }

    // Use the specified namespace and database
    db.use_ns(config.namespace).use_db(config.database).await?;

    Ok(db)
}

pub async fn ensure_schema(db: &Db) -> Result<()> {
    // Define schema for each table
    let schema_queries = vec![
        // Service table
        "DEFINE TABLE registry SCHEMAFULL;
         DEFINE FIELD name ON TABLE service TYPE string;
         DEFINE FIELD title ON TABLE service TYPE option<string>;
         DEFINE FIELD version ON TABLE service TYPE string;
         DEFINE FIELD website_url ON TABLE service TYPE option<string>;
         DEFINE FIELD origin ON TABLE service TYPE string;
         DEFINE FIELD registry_id ON TABLE service TYPE option<record<registry>>;
         DEFINE FIELD created_at ON TABLE service VALUE time::now();
         DEFINE FIELD updated_at ON TABLE service VALUE time::now();",

        // Tool table
        "DEFINE TABLE tool SCHEMALESS;
         DEFINE FIELD service_id ON TABLE tool TYPE record<service>;
         DEFINE FIELD name ON TABLE tool TYPE string;
         DEFINE FIELD description ON TABLE tool TYPE option<string>;
         DEFINE FIELD input_schema ON TABLE tool TYPE object;
         DEFINE FIELD output_schema ON TABLE tool TYPE option<object>;
         DEFINE FIELD embedding_id ON TABLE tool TYPE option<record<embedding>>;
         DEFINE FIELD input_ty ON TABLE tool TYPE option<object>;
         DEFINE FIELD output_ty ON TABLE tool TYPE option<object>;
         DEFINE FIELD usage_count ON TABLE tool TYPE number DEFAULT 0;
         DEFINE FIELD created_at ON TABLE tool VALUE time::now();
         DEFINE FIELD updated_at ON TABLE tool VALUE time::now();",

        // Embedding table
        "DEFINE TABLE embedding SCHEMAFULL;
         DEFINE FIELD vector ON TABLE embedding TYPE array<float>;
         DEFINE FIELD model ON TABLE embedding TYPE string;
         DEFINE FIELD content_type ON TABLE embedding TYPE string;
         DEFINE FIELD content_hash ON TABLE embedding TYPE string;
         DEFINE FIELD created_at ON TABLE embedding VALUE time::now();",

        // Typed relationship between tools
        "DEFINE TABLE tool_compatibility SCHEMAFULL;
         DEFINE FIELD in ON TABLE tool_compatibility TYPE record<tool>;
         DEFINE FIELD out ON TABLE tool_compatibility TYPE record<tool>;
         DEFINE FIELD compatibility_type ON TABLE tool_compatibility TYPE string;
         DEFINE FIELD confidence ON TABLE tool_compatibility TYPE float DEFAULT 1.0;
         DEFINE FIELD reasoning ON TABLE tool_compatibility TYPE option<string>;
         DEFINE FIELD created_at ON TABLE tool_compatibility VALUE time::now();",

        // Tool usage patterns
        "DEFINE TABLE tool_sequence SCHEMAFULL;
         DEFINE FIELD in ON TABLE tool_sequence TYPE record<tool>;
         DEFINE FIELD out ON TABLE tool_sequence TYPE record<tool>;
         DEFINE FIELD sequence_type ON TABLE tool_sequence TYPE string;
         DEFINE FIELD frequency ON TABLE tool_sequence TYPE number DEFAULT 1;
         DEFINE FIELD success_rate ON TABLE tool_sequence TYPE float DEFAULT 1.0;
         DEFINE FIELD created_at ON TABLE tool_sequence VALUE time::now();",

        // Registry information
        "DEFINE TABLE registry SCHEMAFULL;
         DEFINE FIELD url ON TABLE registry TYPE string;
         DEFINE FIELD name ON TABLE registry TYPE string;
         DEFINE FIELD description ON TABLE registry TYPE option<string>;
         DEFINE FIELD is_active ON TABLE registry TYPE bool DEFAULT true;
         DEFINE FIELD last_sync ON TABLE registry TYPE option<datetime>;
         DEFINE FIELD created_at ON TABLE registry VALUE time::now();",

        // MCP manifests from registries
        "DEFINE TABLE manifest SCHEMAFULL;
         DEFINE FIELD registry_id ON TABLE manifest TYPE record<registry>;
         DEFINE FIELD name ON TABLE manifest TYPE string;
         DEFINE FIELD version ON TABLE manifest TYPE string;
         DEFINE FIELD content ON TABLE manifest TYPE object;
         DEFINE FIELD hash ON TABLE manifest TYPE string;
         DEFINE FIELD is_active ON TABLE manifest TYPE bool DEFAULT true;
         DEFINE FIELD created_at ON TABLE manifest VALUE time::now();",

        // Indexes for performance
        "DEFINE INDEX tool_service_id ON TABLE tool COLUMNS service_id;
         DEFINE INDEX tool_name ON TABLE tool COLUMNS name;
         DEFINE INDEX embedding_model ON TABLE embedding COLUMNS model;
         DEFINE INDEX embedding_hash ON TABLE embedding COLUMNS content_hash;
         DEFINE INDEX embedding_vector ON TABLE embedding COLUMNS vector;
         DEFINE INDEX manifest_registry_version ON TABLE manifest COLUMNS registry_id, version;",

        // Symbolic rule table
        "DEFINE TABLE symbolic_rule SCHEMAFULL;
         DEFINE FIELD name ON TABLE symbolic_rule TYPE string;
         DEFINE FIELD description ON TABLE symbolic_rule TYPE option<string>;
         DEFINE FIELD antecedents ON TABLE symbolic_rule TYPE array;
         DEFINE FIELD consequents ON TABLE symbolic_rule TYPE array;
         DEFINE FIELD confidence ON TABLE symbolic_rule TYPE float;
         DEFINE FIELD priority ON TABLE symbolic_rule TYPE int;
         DEFINE FIELD is_active ON TABLE symbolic_rule TYPE bool DEFAULT true;
         DEFINE FIELD created_at ON TABLE symbolic_rule VALUE time::now();",
    ];

    for query in schema_queries {
        db.query(query).await?;
    }

    // Seed fallback symbolic rule if table is empty
    let seed_check = db.query("SELECT * FROM symbolic_rule LIMIT 1").await?.take::<Option<serde_json::Value>>(0)?;
    if seed_check.is_none() {
        let seed_rule = r#"
        CREATE symbolic_rule CONTENT {
            name: "Fallback tool selection",
            description: "Select any available tool with low confidence as a fallback.",
            antecedents: [
                {
                    "Fact": {
                        "predicate": "tool_exists",
                        "arguments": [
                            { "Variable": "tool" }
                        ],
                        "confidence": null
                    }
                }
            ],
            consequents: [
                {
                    "Fact": {
                        "predicate": "tool_selected",
                        "arguments": [
                            { "Variable": "tool" },
                            { "Literal": { "type": "Number", "Number": 0.1 } },
                            { "Literal": { "type": "String", "String": "fallback selection" } }
                        ],
                        "confidence": null
                    }
                }
            ],
            confidence: 0.1,
            priority: 0,
            is_active: true
        };
        "#;
        db.query(seed_rule).await?;
    }

    Ok(())
}
