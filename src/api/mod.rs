// REST API endpoints for the orchestrator

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::orchestrator::Orchestrator;

pub type AppState = Arc<Mutex<Orchestrator>>;

pub fn create_public_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/query", post(query_tools))
        .route("/services", get(list_services))
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
    let orchestrator = state.lock().await;

    // Note: REST API currently doesn't support authentication, so we pass None
    // for user_context. To add auth, extract user from request headers here.
    let selections = orchestrator
        .query_tools(&query, context, None)
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

async fn discover_tools(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    // Mutating operation: (re)discover tools from all known MCP services.
    let mut orchestrator = state.lock().await;

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

/// List all discovered MCP services.
///
/// This is a read-only endpoint that returns information about all services
/// that have been discovered and registered in the orchestrator.
async fn list_services(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    // Read-only operation: query services from the database
    let orchestrator = state.lock().await;
    let db = orchestrator.db();

    // Query all services from the database
    let mut res = db
        .query("SELECT * FROM service ORDER BY updated_at DESC")
        .await
        .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

    let services: Vec<crate::db::schema::ServiceRecord> = res
        .take(0)
        .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "services": services,
        "count": services.len(),
    })))
}
