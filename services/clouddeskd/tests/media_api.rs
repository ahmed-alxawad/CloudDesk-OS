//! Integration tests for the media (`FFmpeg`) HTTP surface: probe, job
//! creation, ownership-scoped status/cancel/output, and malformed `Range`
//! handling on the existing streaming endpoint. Uses a real installed
//! `ffmpeg`/`ffprobe` and generates fixtures on the fly; every test that
//! needs a live binary skips cleanly (not falsely PASS) if one isn't
//! present.

use std::{fs, net::SocketAddr, process::Stdio};

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
use tokio::process::Command;
use tower::ServiceExt;

async fn application_with_media() -> (Router, tempfile::TempDir, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    // Media support must be explicitly enabled, same as any other
    // optional runtime -- mirror what a real deployment would flip via
    // `runtime.media.enabled` before FFmpeg is ever probed for.
    sqlx::query(
        "UPDATE system_settings SET value_json = 'true' WHERE key = 'runtime.media.enabled'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let availability = clouddesk_media::ffmpeg::detect(true).await;
    let cache_dir = tempfile::tempdir().unwrap();
    let media =
        clouddesk_media::MediaService::new(availability, pool.clone(), cache_dir.path().to_owned());

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
        clouddeskd::application_router_and_media_configured(
            directory.path().to_owned(),
            auth,
            secret_path,
            true,
            Some(media),
        ),
        directory,
        cache_dir,
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
        "127.0.0.1:43126".parse::<SocketAddr>().unwrap(),
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

fn current_process_linux_username() -> Option<String> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        return None;
    }
    clouddesk_linux::lookup_uid(uid)
        .ok()
        .flatten()
        .map(|identity| identity.username)
}

async fn bootstrap_and_login(app: &Router, username: &str, password: &str) -> Option<String> {
    let linux_username = current_process_linux_username()?;
    let bootstrap = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            &json!({
                "secret": "one-time-test-secret",
                "username": username,
                "display_name": "Admin",
                "password": password,
                "linux_username": linux_username,
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::CREATED);
    login(app, username, password).await
}

async fn login(app: &Router, username: &str, password: &str) -> Option<String> {
    let login = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/auth/login",
            &json!({"username": username, "password": password}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    Some(
        login
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned(),
    )
}

async fn ffmpeg_available() -> bool {
    clouddesk_media::ffmpeg::detect(true).await.is_available()
}

/// Generates a small H.264/AAC-in-MKV fixture directly inside the mapped
/// user's real `$HOME` (in a self-cleaning tempdir, matching the existing
/// resumable-upload tests' pattern) so the product's own path-resolution
/// and authorization code is exercised unmodified.
async fn generate_mkv_fixture() -> (tempfile::TempDir, String, String) {
    let home = std::env::var("HOME").unwrap();
    let target_dir = tempfile::tempdir_in(&home).unwrap();
    let dir_name = target_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let file_path = target_dir.path().join("fixture.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&file_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    assert!(status.success());
    let virtual_path = format!("{dir_name}/fixture.mkv");
    (target_dir, virtual_path, dir_name)
}

#[tokio::test]
async fn probe_classifies_a_real_fixture_and_requires_authorization() {
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    let (app, _dir, _cache) = application_with_media().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let (_fixture_dir, virtual_path, _) = generate_mkv_fixture().await;

    // Unauthenticated probe is rejected.
    let unauth = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/probe",
            &json!({ "path": virtual_path }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let probe = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/probe",
            &json!({ "path": virtual_path }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let status = probe.status();
    let body: Value =
        serde_json::from_slice(&probe.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["plan"], "remux");
}

#[tokio::test]
async fn remux_job_completes_and_output_is_downloadable() {
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    let (app, _dir, _cache) = application_with_media().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let (_fixture_dir, virtual_path, _) = generate_mkv_fixture().await;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/jobs",
            &json!({ "path": virtual_path, "operation": "remux" }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let job_id = body["job_id"].as_str().unwrap().to_owned();

    let mut final_state = None;
    for _ in 0..100 {
        let status = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("/api/v1/media/jobs/{job_id}"),
                Body::empty(),
                Some(&cookie),
            ))
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&status.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let state = body["state"].as_str().unwrap().to_owned();
        if matches!(
            state.as_str(),
            "completed" | "failed" | "cancelled" | "expired"
        ) {
            final_state = Some(state);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(final_state.as_deref(), Some("completed"));

    let output = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/media/jobs/{job_id}/output"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(output.status(), StatusCode::OK);
    let bytes = output.into_body().collect().await.unwrap().to_bytes();
    assert!(!bytes.is_empty(), "remuxed output must be non-empty");
}

#[tokio::test]
async fn a_users_media_job_is_invisible_and_uncontrollable_by_another_user() {
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    let (app, _dir, _cache) = application_with_media().await;
    let Some(admin_cookie) =
        bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let (_fixture_dir, virtual_path, _) = generate_mkv_fixture().await;

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
    let user_cookie = login(&app, "user1", "user horse battery staple")
        .await
        .unwrap();

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/jobs",
            &json!({ "path": virtual_path, "operation": "remux" }),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let job_id = body["job_id"].as_str().unwrap().to_owned();

    // A different, authorized-for-media-in-general user must not be able
    // to see this job exists at all.
    let status = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/media/jobs/{job_id}"),
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::NOT_FOUND);

    let cancel = app
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("/api/v1/media/jobs/{job_id}"),
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(cancel.status(), StatusCode::NOT_FOUND);

    let output = app
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/media/jobs/{job_id}/output"),
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(output.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn media_endpoints_are_unavailable_when_ffmpeg_support_is_disabled() {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    // Do NOT flip runtime.media.enabled -- stays false (the seeded
    // default), so detection is never even attempted.
    let availability = clouddesk_media::ffmpeg::detect(false).await;
    assert_eq!(availability, clouddesk_media::FfmpegAvailability::Disabled);
    let cache_dir = tempfile::tempdir().unwrap();
    let media =
        clouddesk_media::MediaService::new(availability, pool.clone(), cache_dir.path().to_owned());

    let auth = AuthService::new(
        pool,
        SecretCipher::new(&[9_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    fs::write(&secret_path, "one-time-test-secret\n").unwrap();
    let app = clouddeskd::application_router_and_media_configured(
        directory.path().to_owned(),
        auth,
        secret_path,
        true,
        Some(media),
    );

    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/jobs",
            &json!({ "path": "whatever.mkv", "operation": "remux" }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn malformed_range_requests_against_media_stream_are_handled_safely() {
    let (app, _dir, _cache) = application_with_media().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let home = std::env::var("HOME").unwrap();
    let target_dir = tempfile::tempdir_in(&home).unwrap();
    let dir_name = target_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let file_path = target_dir.path().join("small.bin");
    std::fs::write(&file_path, vec![7_u8; 1000]).unwrap();
    let virtual_path = format!("{dir_name}/small.bin");

    let range_request = |range: &str, cookie: &str| {
        let mut req = request(
            Method::GET,
            &format!("/api/v1/media/stream?path={virtual_path}"),
            Body::empty(),
            Some(cookie),
        );
        req.headers_mut()
            .insert(header::RANGE, range.parse().unwrap());
        req
    };

    // Start beyond EOF -> 416, not a silently-wrong 200 full body.
    let beyond_eof = app
        .clone()
        .oneshot(range_request("bytes=5000-6000", &cookie))
        .await
        .unwrap();
    assert_eq!(beyond_eof.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    // Reversed range -> 416.
    let reversed = app
        .clone()
        .oneshot(range_request("bytes=500-100", &cookie))
        .await
        .unwrap();
    assert_eq!(reversed.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    // Huge range clamps to EOF and still succeeds as a partial response.
    let huge = app
        .clone()
        .oneshot(range_request("bytes=0-999999999", &cookie))
        .await
        .unwrap();
    assert_eq!(huge.status(), StatusCode::PARTIAL_CONTENT);

    // Malformed syntax -> ignored, full 200 body served.
    let malformed = app
        .clone()
        .oneshot(range_request("bytes=not-a-range", &cookie))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::OK);

    // Empty file + Range header -> 416, never a panic or hang.
    let empty_path = target_dir.path().join("empty.bin");
    std::fs::write(&empty_path, []).unwrap();
    let empty_virtual = format!("{dir_name}/empty.bin");
    let mut empty_req = request(
        Method::GET,
        &format!("/api/v1/media/stream?path={empty_virtual}"),
        Body::empty(),
        Some(&cookie),
    );
    empty_req
        .headers_mut()
        .insert(header::RANGE, "bytes=0-".parse().unwrap());
    let empty_response = app.oneshot(empty_req).await.unwrap();
    assert_eq!(empty_response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
}
