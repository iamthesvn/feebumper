use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::bumper::FeeBumper;
use crate::error::Error;
use crate::types::*;

pub fn router(bumper: Arc<FeeBumper>) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/estimate", post(estimate))
        .route("/api/v1/bumps", post(create_bump))
        .route("/api/v1/bumps/{id}", get(get_bump))
        .with_state(bumper)
        .layer(TraceLayer::new_for_http())
}

async fn health() -> &'static str {
    "ok"
}

async fn estimate(
    State(bumper): State<Arc<FeeBumper>>,
    Json(req): Json<EstimateRequest>,
) -> Result<Json<EstimateResponse>, Error> {
    let resp = bumper.estimate(&req).await?;
    Ok(Json(resp))
}

async fn create_bump(
    State(bumper): State<Arc<FeeBumper>>,
    Json(req): Json<BumpCreateRequest>,
) -> Result<Json<BumpCreateResponse>, Error> {
    let resp = bumper.create_bump(&req).await?;
    Ok(Json(resp))
}

async fn get_bump(
    State(bumper): State<Arc<FeeBumper>>,
    Path(id): Path<Uuid>,
) -> Result<Json<BumpStatusResponse>, Error> {
    let resp = bumper.get_bump(id)?;
    Ok(Json(resp))
}
