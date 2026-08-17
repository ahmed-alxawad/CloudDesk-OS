use std::path::PathBuf;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn versioned_health_endpoint_is_available_without_authentication() {
    let response = clouddeskd::router(PathBuf::from("apps/web/dist"))
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["service"], "cloudeskd");
}

#[tokio::test]
async fn cross_site_mutations_are_rejected_before_routing() {
    let response = clouddeskd::router(PathBuf::from("apps/web/dist"))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/setup/bootstrap")
                .header(header::HOST, "cloud.example")
                .header(header::ORIGIN, "https://evil.example")
                .header("sec-fetch-site", "cross-site")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let websocket = clouddeskd::router(PathBuf::from("apps/web/dist"))
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/terminal/ws")
                .header(header::HOST, "cloud.example")
                .header(header::ORIGIN, "https://evil.example")
                .header(header::UPGRADE, "websocket")
                .header("sec-fetch-site", "cross-site")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(websocket.status(), StatusCode::FORBIDDEN);
}

#[test]
fn root_is_rejected_before_the_service_starts() {
    assert!(clouddeskd::security::require_unprivileged(0).is_err());
    assert!(clouddeskd::security::require_unprivileged(1_000).is_ok());
}
