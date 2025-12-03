// REST API endpoints for the orchestrator

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde_json::Value;
use tokio::sync::Mutex;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::UnicityOrchestrator;

pub type AppState = Arc<Mutex<UnicityOrchestrator>>;

pub fn create_public_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/query", post(query_tools))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

pub fn create_admin_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        // .route("/sync", post(sync_registries)) // TODO
        .route("/discover", post(discover_tools))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

async fn health_check() -> Result<Json<Value>, StatusCode> {
    Ok(Json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

async fn query_tools(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let query = payload
        .get("query")
        .and_then(|q| q.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_string();

    let context = payload.get("context").cloned();

    // Read-only operation: we only need an immutable borrow of the orchestrator,
    // but we go through the mutex so we share the same instance with mutating ops.
    let orchestrator = state
        .lock()
        .await;

    let selections = orchestrator
        .query_tools(&query, context)
        .await
        .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "selections": selections,
        "count": selections.len()
    })))
}

// TODO
// async fn sync_registries(
//     State(state): State<AppState>,
// ) -> Result<Json<Value>, StatusCode> {
//     // Mutating operation: sync registry manifests from configured sources.
//     let mut orchestrator = state
//         .lock()
//         .await;
//
//     let result = orchestrator
//         .sync_registries()
//         .await
//         .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;
//
//     Ok(Json(serde_json::json!({
//         "status": "ok",
//         "total_manifests": result.total_manifests,
//         "new_manifests": result.new_manifests,
//         "updated_manifests": result.updated_manifests,
//         "errors": result
//             .errors
//             .iter()
//             .map(|e| (e.0.to_string(), e.1.to_string()))
//             .collect::<Vec<_>>(),
//     })))
// }

async fn discover_tools(
    State(state): State<AppState>,
) -> Result<Json<Value>, StatusCode> {
    // Mutating operation: (re)discover tools from all known MCP services.
    let mut orchestrator = state
        .lock()
        .await;

    let (services, tools) = orchestrator
        .discover_tools()
        .await
        .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "services_discovered": services,
        "tools_discovered": tools,
    })))
}
