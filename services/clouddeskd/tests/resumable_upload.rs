//! Integration tests for resumable local-file uploads (`GOAL.md` G3).
//!
//! These exercise the full HTTP surface (create session -> chunked PUT,
//! simulating a client that resumes after a dropped connection -> status
//! check -> finalize) plus authorization/cross-user isolation and checksum
//! verification. Uses the bootstrap administrator mapped to whichever Linux
//! account is actually running the test process, so the write target is
//! guaranteed to be writable without root and without hardcoding a
//! CI-specific username.

use std::{fs, net::SocketAddr};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, Method, Request, StatusCode},
    Router,
};
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn application() -> (Router, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool,
        SecretCipher::new(&[9_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    fs::write(&secret_path, "one-time-test-secret\n").unwrap();
    (
        clouddeskd::application_router(directory.path().to_owned(), auth, secret_path),
        directory,
    )
}

fn request(method: Method, uri: &str, body: Body, cookie: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, "integration-test")
        .body(body)
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:43125".parse::<SocketAddr>().unwrap(),
    ));
    if let Some(cookie) = cookie {
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
    }
    request
}

fn json_request(method: Method, uri: &str, body: &Value, cookie: Option<&str>) -> Request<Body> {
    let mut req = request(method, uri, Body::from(body.to_string()), cookie);
    req.headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    req
}

/// A Linux identity the current (unprivileged) test process can actually
/// write files as — whoever is running `cargo test`, not a hardcoded name.
fn current_process_linux_username() -> Option<String> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        // Bootstrap explicitly rejects mapping the administrator to root;
        // skip the live filesystem assertions when running as root (e.g.
        // some CI containers) rather than fail for an unrelated reason.
        return None;
    }
    clouddesk_linux::lookup_uid(uid)
        .ok()
        .flatten()
        .map(|identity| identity.username)
}

async fn bootstrap_and_login(app: &Router) -> Option<(String, String)> {
    let linux_username = current_process_linux_username()?;

    let bootstrap = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            &json!({
                "secret": "one-time-test-secret",
                "username": "admin",
                "display_name": "Admin",
                "password": "correct horse battery staple",
                "linux_username": linux_username,
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::CREATED);

    let login = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/auth/login",
            &json!({"username": "admin", "password": "correct horse battery staple"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Some((linux_username, cookie))
}

#[tokio::test]
async fn resumable_upload_round_trips_across_multiple_chunks() {
    let (app, _directory) = application().await;
    let Some((_user, cookie)) = bootstrap_and_login(&app).await else {
        eprintln!("skipping: test process runs as root, cannot map a non-root Linux identity");
        return;
    };

    let target_dir = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
    let file_name = target_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let virtual_path = format!("{file_name}/resumable.bin");

    let full_content: Vec<u8> = (0..300_000_u32)
        .map(|i| u8::try_from(i % 256).unwrap_or(0))
        .collect();
    let checksum = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(&full_content))
    };

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/files/local/uploads",
            &json!({
                "path": virtual_path,
                "total_size": full_content.len(),
                "sha256": checksum,
            }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let create_status = create.status();
    let create_bytes = create.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        create_status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&create_bytes)
    );
    let create_body: Value = serde_json::from_slice(&create_bytes).unwrap();
    let upload_id = create_body["upload_id"].as_str().unwrap().to_owned();
    assert_eq!(create_body["bytes_received"], 0);

    // Send the first two-thirds of the file, then simulate a dropped
    // connection by stopping short of the total size.
    let first_chunk = &full_content[..200_000];
    let chunk_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/files/local/uploads/{upload_id}"),
            Body::from(first_chunk.to_vec()),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(chunk_response.status(), StatusCode::OK);

    // Resume: query status, confirm it reflects exactly what was received,
    // then send the remainder starting from that offset.
    let status = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/files/local/uploads/{upload_id}"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let status_body: Value =
        serde_json::from_slice(&status.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(status_body["bytes_received"], 200_000);

    let remainder = &full_content[200_000..];
    let chunk_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/files/local/uploads/{upload_id}"),
            Body::from(remainder.to_vec()),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(chunk_response.status(), StatusCode::OK);

    let finalize = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/files/local/uploads/{upload_id}/complete"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(finalize.status(), StatusCode::OK);

    let final_path = target_dir.path().join("resumable.bin");
    let written = fs::read(&final_path).unwrap();
    assert_eq!(
        written, full_content,
        "reassembled file must match byte-for-byte"
    );
}

#[tokio::test]
async fn resumable_upload_rejects_checksum_mismatch() {
    let (app, _directory) = application().await;
    let Some((_user, cookie)) = bootstrap_and_login(&app).await else {
        eprintln!("skipping: test process runs as root, cannot map a non-root Linux identity");
        return;
    };

    let target_dir = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
    let file_name = target_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let virtual_path = format!("{file_name}/bad-checksum.bin");
    let content = b"some bytes".to_vec();

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/files/local/uploads",
            &json!({
                "path": virtual_path,
                "total_size": content.len(),
                "sha256": "0".repeat(64),
            }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let create_body: Value =
        serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let upload_id = create_body["upload_id"].as_str().unwrap().to_owned();

    app.clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/files/local/uploads/{upload_id}"),
            Body::from(content),
            Some(&cookie),
        ))
        .await
        .unwrap();

    let finalize = app
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/files/local/uploads/{upload_id}/complete"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(finalize.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn resumable_upload_session_is_isolated_per_user() {
    let (app, _directory) = application().await;
    let Some((linux_username, admin_cookie)) = bootstrap_and_login(&app).await else {
        eprintln!("skipping: test process runs as root, cannot map a non-root Linux identity");
        return;
    };

    let step_up = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/auth/step-up",
            &json!({"password": "correct horse battery staple"}),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(step_up.status(), StatusCode::OK);

    let create_user = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/users",
            &json!({
                "username": "user1",
                "display_name": "User One",
                "password": "user horse battery staple",
                "role_ids": ["user"],
            }),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create_user.status(), StatusCode::CREATED);

    let user_login = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/auth/login",
            &json!({"username": "user1", "password": "user horse battery staple"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(user_login.status(), StatusCode::OK);
    let user_cookie = user_login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let _ = linux_username;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/files/local/uploads",
            &json!({"path": "admin-owned.bin", "total_size": 10}),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    let create_body: Value =
        serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let upload_id = create_body["upload_id"].as_str().unwrap().to_owned();

    let cross_user_status = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/files/local/uploads/{upload_id}"),
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        cross_user_status.status(),
        StatusCode::FORBIDDEN,
        "a session belonging to a different user must not be readable"
    );

    let cross_user_chunk = app
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/files/local/uploads/{upload_id}"),
            Body::from(vec![0_u8; 10]),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        cross_user_chunk.status(),
        StatusCode::FORBIDDEN,
        "a session belonging to a different user must not be writable"
    );
}
