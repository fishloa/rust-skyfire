use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // oneshot

fn app() -> axum::Router {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    skyfire_server::routes::router(Arc::new(skyfire_server::manager::Manager::new(dir, vec![])))
}

async fn get(app: axum::Router, uri: &str) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, body, headers)
}

#[tokio::test]
async fn playlist_and_segment_and_404_and_cors() {
    let (st, body, hdr) = get(app(), "/stream/hls/skyfire/france2-8s/index.m3u8").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(hdr.get("access-control-allow-origin").unwrap(), "*");
    let pl = String::from_utf8(body).unwrap();
    assert!(pl.contains("#EXTM3U") && pl.contains("#EXT-X-ENDLIST"));
    let seg = pl.lines().find(|l| l.ends_with(".ts")).unwrap();

    let (st, body, hdr) = get(app(), &format!("/stream/hls/skyfire/france2-8s/{seg}")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(hdr.get("content-type").unwrap(), "video/mp2t");
    assert_eq!(body[0], 0x47);

    let (st, _, _) = get(app(), "/stream/hls/skyfire/france2-8s/nope.ts").await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Path traversal is rejected.
    let (st, _, _) = get(app(), "/stream/hls/skyfire/france2-8s/..%2f..%2fCargo.toml").await;
    assert!(st == StatusCode::NOT_FOUND || st == StatusCode::BAD_REQUEST);

    let (st, _, _) = get(app(), "/stream/hls/skyfire/does-not-exist/index.m3u8").await;
    assert!(st == StatusCode::NOT_FOUND || st == StatusCode::SERVICE_UNAVAILABLE);
}
