use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tower_http::cors::CorsLayer;

use crate::manager::Manager;

pub fn router(mgr: Arc<Manager>) -> Router {
    Router::new()
        .route("/api/streams", get(list_streams))
        .route("/stream/hls/skyfire/{slug}/index.m3u8", get(playlist))
        .route("/stream/hls/skyfire/{slug}/{segment}", get(segment))
        .layer(CorsLayer::permissive())
        .with_state(mgr)
}

async fn list_streams(State(mgr): State<Arc<Manager>>) -> Json<Vec<String>> {
    Json(mgr.slugs())
}

async fn playlist(State(mgr): State<Arc<Manager>>, Path(slug): Path<String>) -> Response {
    match mgr.playlist(&slug) {
        Some(pl) if mgr.is_ready(&slug) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
                (header::CACHE_CONTROL, "no-cache, no-store"),
            ],
            pl,
        )
            .into_response(),
        Some(_) => (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response(),
        None => (StatusCode::NOT_FOUND, "unknown stream").into_response(),
    }
}

async fn segment(
    State(mgr): State<Arc<Manager>>,
    Path((slug, segment)): Path<(String, String)>,
) -> Response {
    // Path-traversal guard.
    if segment.contains('/') || segment.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad segment name").into_response();
    }
    match mgr.segment(&slug, &segment) {
        Some(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "video/mp2t"),
                (header::CACHE_CONTROL, "max-age=30"),
            ],
            (*bytes).clone(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no such segment").into_response(),
    }
}
