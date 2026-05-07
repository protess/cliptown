use axum::body::to_bytes;
use tower::ServiceExt;

#[tokio::test]
async fn health_returns_ok_json() {
    let app = cliptown_world::http::router_minimal();
    let req = axum::http::Request::builder()
        .uri("/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], br#"{"ok":true}"#);
}
