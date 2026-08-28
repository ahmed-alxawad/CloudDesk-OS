//! Integration tests for the Music (library) HTTP surface: indexing,
//! browsing, search, playlists, favorites, queue, recently-played,
//! artwork, and cross-user isolation. Uses real `ffmpeg`-generated audio
//! fixtures with real tags; skips cleanly if `ffmpeg` isn't installed.

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

async fn application_with_music() -> (Router, tempfile::TempDir, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
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
    let library = clouddesk_library::LibraryStore::new(pool.clone());

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
        clouddeskd::application_router_and_media_and_library_configured(
            directory.path().to_owned(),
            auth,
            secret_path,
            true,
            Some(media),
            Some(library),
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
        "127.0.0.1:43127".parse::<SocketAddr>().unwrap(),
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

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

/// Generates a small real MP3 with real tags directly inside the mapped
/// user's real `$HOME` tempdir (matching every other test in this suite).
async fn generate_track(dir: &std::path::Path, name: &str, title: &str, artist: &str) {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "libmp3lame",
            "-metadata",
            &format!("title={title}"),
            "-metadata",
            &format!("artist={artist}"),
            "-metadata",
            "album=Integration Test Album",
        ])
        .arg(dir.join(name))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn full_library_lifecycle() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "full_library_lifecycle",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_music().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "full_library_lifecycle",
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
    generate_track(target_dir.path(), "one.mp3", "First Song", "Test Artist").await;
    generate_track(target_dir.path(), "two.mp3", "Second Song", "Test Artist").await;

    // 1. add root
    let add_root = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/roots",
            &json!({ "path": format!("/{dir_name}") }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(add_root.status(), StatusCode::OK);
    let root_body = body_json(add_root).await;
    let root_id = root_body["id"].as_str().unwrap().to_owned();

    // 2. scan
    let scan = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(scan.status(), StatusCode::OK);
    let summary = body_json(scan).await;
    assert_eq!(summary["added"], 2);

    // 3. retrieve tracks
    let tracks_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let tracks_body = body_json(tracks_resp).await;
    assert_eq!(tracks_body["total"], 2);
    let tracks = tracks_body["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 2);
    let track_one_id = tracks.iter().find(|t| t["title"] == "First Song").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // 4. artists
    let artists_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/artists",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(artists_resp).await, json!(["Test Artist"]));

    // 5. albums
    let albums_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/albums",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let albums = body_json(albums_resp).await;
    assert_eq!(albums[0]["album"], "Integration Test Album");

    // 6. search
    let search_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/search?q=First",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let results = body_json(search_resp).await;
    assert_eq!(results.as_array().unwrap().len(), 1);

    // 7. create playlist, 8. add/reorder/remove tracks
    let playlist_resp = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/playlists",
            &json!({ "name": "My Playlist" }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(playlist_resp.status(), StatusCode::OK);
    let playlist_id = body_json(playlist_resp).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let add_entry = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/music/playlists/{playlist_id}/entries"),
            &json!({ "track_id": track_one_id }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(add_entry.status(), StatusCode::NO_CONTENT);

    let entries_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let entries = body_json(entries_resp).await;
    assert_eq!(entries.as_array().unwrap().len(), 1);
    let entry_id = entries[0]["entry_id"].as_str().unwrap().to_owned();

    let remove_entry = app
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("/api/v1/music/playlists/{playlist_id}/entries/{entry_id}"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(remove_entry.status(), StatusCode::NO_CONTENT);

    // 9. favorite/unfavorite
    let favorite = app
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/music/favorites/{track_one_id}"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(favorite.status(), StatusCode::NO_CONTENT);
    let favorites_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/favorites",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(favorites_resp).await.as_array().unwrap().len(), 1);

    // 10/11. DIRECT playback classification (reuses Phase 3's /media/probe)
    let probe = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/probe",
            &json!({ "path": format!("/{dir_name}/one.mp3") }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let probe_body = body_json(probe).await;
    assert_eq!(probe_body["plan"], "direct");

    // 12. queue operations
    let set_queue = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/music/queue",
            &json!({ "track_ids": [track_one_id.clone()] }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(set_queue.status(), StatusCode::NO_CONTENT);
    let get_queue = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/queue",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(get_queue).await["track_ids"],
        json!([track_one_id.clone()])
    );

    // 13. recently played
    let record = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/recent",
            &json!({ "track_id": track_one_id }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(record.status(), StatusCode::NO_CONTENT);
    let recent_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/recent",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(recent_resp).await.as_array().unwrap().len(), 1);

    // 14. remove file + incremental rescan
    std::fs::remove_file(target_dir.path().join("one.mp3")).unwrap();
    let rescan = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let rescan_summary = body_json(rescan).await;
    assert_eq!(rescan_summary["removed"], 1);
    let after_removal = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(after_removal).await["total"], 1);

    // 15. cross-user isolation
    let step_up = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/auth/step-up",
            &json!({"password": "correct horse battery staple"}),
            Some(&cookie),
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
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create_user.status(), StatusCode::CREATED);
    let user_cookie = login(&app, "user1", "user horse battery staple")
        .await
        .unwrap();

    let user_tracks = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(user_tracks).await["total"], 0);

    let user_favorites = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/favorites",
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert!(body_json(user_favorites)
        .await
        .as_array()
        .unwrap()
        .is_empty());

    let user_playlists = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/playlists",
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert!(body_json(user_playlists)
        .await
        .as_array()
        .unwrap()
        .is_empty());

    // user1 cannot read admin's playlist by ID, cannot add to it, cannot
    // favorite admin's track, cannot see admin's queue.
    let user_reads_admin_playlist = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(user_reads_admin_playlist.status(), StatusCode::NOT_FOUND);

    let user_favorites_admin_track = app
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/music/favorites/{track_one_id}"),
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(user_favorites_admin_track.status(), StatusCode::NOT_FOUND);

    let user_queue = app
        .oneshot(request(
            Method::GET,
            "/api/v1/music/queue",
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(user_queue).await["track_ids"], json!([]));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn artwork_extraction_and_sidecar_fallback() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "artwork_extraction_and_sidecar_fallback",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_music().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "artwork_extraction_and_sidecar_fallback",
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

    // Track with embedded artwork.
    let cover = target_dir.path().join("cover-src.jpg");
    Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=32x32:d=1",
            "-frames:v",
            "1",
        ])
        .arg(&cover)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    let plain = target_dir.path().join("plain.mp3");
    Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "libmp3lame",
        ])
        .arg(&plain)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    let with_art = target_dir.path().join("embedded.mp3");
    Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(&plain)
        .arg("-i")
        .arg(&cover)
        .args([
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c:a",
            "copy",
            "-c:v",
            "mjpeg",
            "-disposition:v:0",
            "attached_pic",
        ])
        .arg(&with_art)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();

    // Second track directory relying on a sidecar cover.jpg instead.
    let sidecar_dir = target_dir.path().join("sidecar-album");
    std::fs::create_dir_all(&sidecar_dir).unwrap();
    let sidecar_track = sidecar_dir.join("track.mp3");
    Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "libmp3lame",
        ])
        .arg(&sidecar_track)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    std::fs::copy(&cover, sidecar_dir.join("cover.jpg")).unwrap();

    // Third track with no artwork anywhere.
    let no_art_dir = target_dir.path().join("no-art-album");
    std::fs::create_dir_all(&no_art_dir).unwrap();
    Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "libmp3lame",
        ])
        .arg(no_art_dir.join("track.mp3"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();

    let add_root = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/roots",
            &json!({ "path": format!("/{dir_name}") }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let root_id = body_json(add_root).await["id"].as_str().unwrap().to_owned();
    let scan = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let summary = body_json(scan).await;
    // plain.mp3 (the source used to build embedded.mp3, but itself a
    // real independent track too), embedded.mp3, sidecar-album/track.mp3,
    // no-art-album/track.mp3 -- 4 real audio files (cover-src.jpg is
    // filtered out by extension).
    assert_eq!(summary["added"], 4);

    let tracks_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let tracks = body_json(tracks_resp).await;
    let find_id = |path_suffix: &str| -> String {
        tracks["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["virtual_path"].as_str().unwrap().ends_with(path_suffix))
            .unwrap_or_else(|| panic!("no track ending with {path_suffix}"))["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let embedded_id = find_id("embedded.mp3");
    let sidecar_id = find_id("sidecar-album/track.mp3");
    let no_art_id = find_id("no-art-album/track.mp3");

    let embedded_art = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/tracks/{embedded_id}/artwork"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(embedded_art.status(), StatusCode::OK);
    let embedded_bytes = embedded_art.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&embedded_bytes[0..2], &[0xFF, 0xD8]);

    let sidecar_art = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/tracks/{sidecar_id}/artwork"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(sidecar_art.status(), StatusCode::OK);

    let no_art = app
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/tracks/{no_art_id}/artwork"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(no_art.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn malicious_metadata_is_stored_and_returned_faithfully_as_plain_text() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "malicious_metadata_is_stored_and_returned_faithfully_as_plain_text",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let (app, _dir, _cache) = application_with_music().await;
    let Some(cookie) = bootstrap_and_login(&app, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "malicious_metadata_is_stored_and_returned_faithfully_as_plain_text",
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
    let hostile_title = "<script>alert(1)</script>&\"'unicode\u{202e}control";
    generate_track(
        target_dir.path(),
        "hostile.mp3",
        hostile_title,
        "Normal Artist",
    )
    .await;

    let add_root = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/roots",
            &json!({ "path": format!("/{dir_name}") }),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let root_id = body_json(add_root).await["id"].as_str().unwrap().to_owned();
    app.clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();

    let tracks_resp = app
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let tracks = body_json(tracks_resp).await;
    let title = tracks["tracks"][0]["title"].as_str().unwrap();
    // The API returns the tag verbatim as a JSON string value -- never
    // interpreted as markup server-side. Frontend rendering safety is
    // covered separately (music.test.ts / no {@html} usage).
    assert_eq!(title, hostile_title);
}

#[tokio::test]
async fn large_library_is_paginated_and_bounded_without_ffmpeg() {
    // This test exercises DB-level pagination/search bounds directly
    // against the store, independent of ffmpeg -- proving the API layer
    // never returns an unbounded payload regardless of library size.
    // Real ffmpeg-backed indexing correctness is covered by the smaller,
    // realistic fixtures in the other tests and in
    // crates/library/tests/scan_live.rs.
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
         VALUES ('bulk-user', 'bulk', 'Bulk', 'x', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let library = clouddesk_library::LibraryStore::new(pool.clone());
    let root = library.add_root("bulk-user", "/music").await.unwrap();

    for i in 0..1200 {
        library
            .upsert_track(
                "bulk-user",
                &root.id,
                &format!("/music/track-{i:05}.mp3"),
                &clouddesk_library::TrackMetadata {
                    title: Some(format!("Track {i}")),
                    artist: Some("Bulk Artist".to_owned()),
                    ..Default::default()
                },
                "0:0",
            )
            .await
            .unwrap();
    }

    let total = library.count_tracks("bulk-user").await.unwrap();
    assert_eq!(total, 1200);

    // A default-ish page is bounded, not the whole library.
    let page = library.list_tracks("bulk-user", 200, 0).await.unwrap();
    assert_eq!(page.len(), 200);

    // A caller asking for an absurd page size is clamped server-side.
    let clamped = library.list_tracks("bulk-user", 999_999, 0).await.unwrap();
    assert!(
        clamped.len() <= 500,
        "list_tracks must clamp an oversized limit"
    );

    // Search is index-backed and still bounded.
    let search_results = library.search("bulk-user", "Track", 50).await.unwrap();
    assert_eq!(search_results.len(), 50);
}
