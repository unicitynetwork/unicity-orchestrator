// REST API endpoints for the orchestrator

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::UnicityOrchestrator;

pub type AppState = Arc<UnicityOrchestrator>;

pub fn create_router(orchestrator: UnicityOrchestrator) -> Router {
    let state = Arc::new(orchestrator);

    Router::new()
        .route("/health", get(health_check))
        .route("/query", post(query_tools))
        .route("/sync", post(sync_registries))
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
    let query = payload.get("query")
        .and_then(|q| q.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let context = payload.get("context").cloned();

    // Placeholder implementation - API needs mutable access to orchestrator
    Ok(Json(serde_json::json!({
        "selections": [],
        "count": 0
    })))
}

async fn sync_registries(
    State(state): State<AppState>,
) -> Result<Json<Value>, StatusCode> {
    // This would need to be mutable in a real implementation
    // For now, return a placeholder response
    Ok(Json(serde_json::json!({
        "message": "Registry sync initiated",
        "status": "pending"
    })))
}

async fn discover_tools(
    State(state): State<AppState>,
) -> Result<Json<Value>, StatusCode> {
    // This would need to be mutable in a real implementation
    // For now, return a placeholder response
    Ok(Json(serde_json::json!({
        "message": "Tool discovery initiated",
        "status": "pending"
    })))
}