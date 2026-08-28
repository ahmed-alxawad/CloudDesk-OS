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
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tower::ServiceExt;

async fn application_with_media() -> (Router, tempfile::TempDir, tempfile::TempDir) {
    application_with_media_limits(clouddesk_media::exec::MediaLimits::default()).await
}

/// Same real product/API harness, with a reduced `MediaLimits` injected
/// into the real `MediaService` -- Phase 3 residual closure, Part 8: at
/// least timeout and quota must be proven through the same media-job
/// entry point the real HTTP API uses, not only through internal Rust
/// functions. No HTTP route accepts a limit override; this is purely a
/// test-harness construction choice, exactly like every other
/// test-configurable-limit path in this codebase.
async fn application_with_media_limits(
    limits: clouddesk_media::exec::MediaLimits,
) -> (Router, tempfile::TempDir, tempfile::TempDir) {
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
        clouddesk_media::MediaService::new(availability, pool.clone(), cache_dir.path().to_owned())
            .with_limits(limits);

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
        clouddesk_test_support::blocked_by_environment(
            "probe_classifies_a_real_fixture_and_requires_authorization",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_media().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "probe_classifies_a_real_fixture_and_requires_authorization",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
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
        clouddesk_test_support::blocked_by_environment(
            "remux_job_completes_and_output_is_downloadable",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_media().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "remux_job_completes_and_output_is_downloadable",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
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
        clouddesk_test_support::blocked_by_environment(
            "a_users_media_job_is_invisible_and_uncontrollable_by_another_user",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_media().await;
    let Some(admin_cookie) =
        bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "a_users_media_job_is_invisible_and_uncontrollable_by_another_user",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
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
        clouddesk_test_support::blocked_by_environment(
            "media_endpoints_are_unavailable_when_ffmpeg_support_is_disabled",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
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
        clouddesk_test_support::blocked_by_environment(
            "malformed_range_requests_against_media_stream_are_handled_safely",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
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

/// Generates an MKV with one video, one audio, and one subtitle stream
/// (a real one, containing real text) directly inside the mapped user's
/// `$HOME` tempdir.
async fn generate_subtitled_fixture() -> (tempfile::TempDir, String) {
    let home = std::env::var("HOME").unwrap();
    let target_dir = tempfile::tempdir_in(&home).unwrap();
    let dir_name = target_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let file_path = target_dir.path().join("subtitled.mkv");
    let mut child = Command::new("ffmpeg")
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
            "-f",
            "srt",
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-c:s",
            "srt",
            "-shortest",
        ])
        .arg(&file_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"1\n00:00:00,000 --> 00:00:00,900\nHello from a test\n\n")
        .await
        .unwrap();
    assert!(child.wait().await.unwrap().success());
    (target_dir, format!("{dir_name}/subtitled.mkv"))
}

#[tokio::test]
async fn subtitle_track_is_detected_extracted_and_rejects_a_bogus_index() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "subtitle_track_is_detected_extracted_and_rejects_a_bogus_index",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_media().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "subtitle_track_is_detected_extracted_and_rejects_a_bogus_index",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let (_fixture_dir, virtual_path) = generate_subtitled_fixture().await;

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
    assert_eq!(probe.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&probe.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let streams = body["probe"]["streams"].as_array().unwrap();
    let subtitle_index = streams
        .iter()
        .find(|s| s["codec_type"] == "subtitle")
        .expect("fixture must have a subtitle stream")["index"]
        .as_u64()
        .unwrap();

    let extract = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/subtitles",
            &json!({ "path": virtual_path, "stream_index": subtitle_index }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(extract.status(), StatusCode::OK);
    let vtt = extract.into_body().collect().await.unwrap().to_bytes();
    let vtt_text = String::from_utf8_lossy(&vtt);
    assert!(vtt_text.starts_with("WEBVTT"));
    assert!(vtt_text.contains("Hello from a test"));

    // A stream_index that isn't actually a subtitle stream on this file
    // (here: the video stream's own index) is rejected before ever
    // reaching ffmpeg.
    let video_index = streams.iter().find(|s| s["codec_type"] == "video").unwrap()["index"]
        .as_u64()
        .unwrap();
    let bogus = app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/subtitles",
            &json!({ "path": virtual_path, "stream_index": video_index }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(bogus.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn audio_track_ordinal_is_threaded_through_to_the_remux_job() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "audio_track_ordinal_is_threaded_through_to_the_remux_job",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_media().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "audio_track_ordinal_is_threaded_through_to_the_remux_job",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let (_fixture_dir, virtual_path, _) = generate_mkv_fixture().await;

    // audio_track_ordinal is accepted and a job still completes even
    // though this fixture only has one audio track (ordinal 0) --
    // multi-track selection itself is covered live in
    // crates/media/tests/live_ffmpeg.rs; this test is the HTTP-surface
    // contract (the field round-trips without breaking the job).
    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/jobs",
            &json!({ "path": virtual_path, "operation": "remux", "audio_track_ordinal": 0 }),
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
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn resume_position_round_trips_and_is_isolated_per_user() {
    let (app, _dir, _cache) = application_with_media().await;
    let Some(admin_cookie) =
        bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "resume_position_round_trips_and_is_isolated_per_user",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };

    // No resume state yet -> null, not an error.
    let missing = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/media/resume?path=%2Fsome%2Fmovie.mp4",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&missing.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body.is_null());

    let put = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/media/resume",
            &json!({ "path": "/some/movie.mp4", "position_seconds": 123.5 }),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let get = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/media/resume?path=%2Fsome%2Fmovie.mp4",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&get.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["position_seconds"], 123.5);

    // A negative position is rejected rather than silently stored.
    let invalid = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/media/resume",
            &json!({ "path": "/some/movie.mp4", "position_seconds": -5.0 }),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    // A different user's identically-pathed resume state is fully
    // independent (never leaked, never overwritten by another user's
    // write) -- same-named file in a different user's own VFS root.
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

    let user_get = app
        .oneshot(request(
            Method::GET,
            "/api/v1/media/resume?path=%2Fsome%2Fmovie.mp4",
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&user_get.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(
        body.is_null(),
        "user1 must not see admin's resume position for the same virtual path"
    );
}

/// Like `generate_mkv_fixture`, but with a caller-chosen duration/size so
/// the real production transcode of it reliably takes several seconds --
/// needed by the timeout/quota product-API tests below, which must
/// exercise a real multi-second `ffmpeg` run, not a near-instant one.
async fn generate_heavy_fixture(duration_secs: u32, size: &str) -> (tempfile::TempDir, String) {
    let home = std::env::var("HOME").unwrap();
    let target_dir = tempfile::tempdir_in(&home).unwrap();
    let dir_name = target_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let file_path = target_dir.path().join("heavy.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=duration={duration_secs}:size={size}:rate=30"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=440:duration={duration_secs}"),
            "-c:v",
            "mpeg2video",
            "-c:a",
            "mp2",
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
    let virtual_path = format!("{dir_name}/heavy.mkv");
    (target_dir, virtual_path)
}

async fn create_transcode_job(app: &Router, cookie: &str, virtual_path: &str) -> String {
    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/jobs",
            &json!({ "path": virtual_path, "operation": "transcode" }),
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&create.into_body().collect().await.unwrap().to_bytes()).unwrap();
    body["job_id"].as_str().unwrap().to_owned()
}

async fn poll_job_terminal(app: &Router, cookie: &str, job_id: &str) -> Value {
    for _ in 0..150 {
        let status = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("/api/v1/media/jobs/{job_id}"),
                Body::empty(),
                Some(cookie),
            ))
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&status.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let state = body["state"].as_str().unwrap_or_default();
        if matches!(state, "completed" | "failed" | "cancelled" | "expired") {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("media job did not reach a terminal state in time");
}

/// Phase 3 residual closure, Part 8: the real timeout-enforcement code
/// path (`exec::run_ffmpeg`'s `tokio::time::timeout`), exercised through
/// the actual `POST /api/v1/media/jobs` -> poll `GET
/// /api/v1/media/jobs/{id}` product/API route -- not only the internal
/// `MediaService::start_job` call directly (that evidence already exists
/// in `crates/media/tests/limits_boundary.rs`).
#[tokio::test]
async fn live_timeout_boundary_through_real_http_api() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "live_timeout_boundary_through_real_http_api",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_media_limits(clouddesk_media::exec::MediaLimits {
        job_timeout: std::time::Duration::from_secs(2),
        ..clouddesk_media::exec::MediaLimits::default()
    })
    .await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "live_timeout_boundary_through_real_http_api",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let (_fixture_dir, virtual_path) = generate_heavy_fixture(30, "1920x1080").await;

    let job_id = create_transcode_job(&app, &cookie, &virtual_path).await;
    let final_body = poll_job_terminal(&app, &cookie, &job_id).await;

    assert_eq!(
        final_body["state"], "failed",
        "a real timeout must never surface as completed/cancelled through the API: {final_body}"
    );
    assert_eq!(final_body["error_class"], "timeout");

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
    assert_ne!(
        output.status(),
        StatusCode::OK,
        "a timed-out job's output must never be exposed as a successful media result"
    );
}

/// Phase 3 residual closure, Part 8: the real output-quota-enforcement
/// code path (`exec::run_ffmpeg`'s size watcher), exercised through the
/// actual product/API route.
#[tokio::test]
async fn live_quota_boundary_through_real_http_api() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "live_quota_boundary_through_real_http_api",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_media_limits(clouddesk_media::exec::MediaLimits {
        max_output_bytes: 64 * 1024,
        ..clouddesk_media::exec::MediaLimits::default()
    })
    .await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "live_quota_boundary_through_real_http_api",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };
    let (_fixture_dir, virtual_path) = generate_heavy_fixture(30, "1280x720").await;

    let job_id = create_transcode_job(&app, &cookie, &virtual_path).await;
    let final_body = poll_job_terminal(&app, &cookie, &job_id).await;

    assert_eq!(final_body["state"], "failed", "a real quota breach must never surface as completed/cancelled through the API: {final_body}");
    assert_eq!(final_body["error_class"], "output_too_large");

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
    assert_ne!(
        output.status(),
        StatusCode::OK,
        "an over-quota job's output must never be exposed as a successful media result"
    );
}
