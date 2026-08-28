//! Proves Music state (library roots, tracks, playlists, favorites,
//! recently-played, queue) survives a `clouddeskd` restart -- not just
//! surviving within one process's lifetime.
//!
//! `clouddeskd` is stateless except through its `SQLite` database (there
//! is no separate in-memory cache for any of `LibraryStore`'s tables --
//! `crates/library/src/store.rs` is pure `SQLite` reads/writes). A real
//! process restart is therefore equivalent, for this state, to closing
//! one connection pool over a database file and opening a brand new one
//! over the same file: the file on disk, not the process's memory, is
//! what must survive. This test builds two fully independent `Router`s
//! (independent `AuthService`, `MediaService`, `LibraryStore`, `SqlitePool`
//! instances -- nothing is shared in memory between them) against the
//! same on-disk database file, simulating a genuine restart.

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

/// Builds a fresh `Router` (fresh `AuthService`/`MediaService`/
/// `LibraryStore`/pool, nothing shared with any previous call) against
/// the `SQLite` file at `db_path`. Two calls with the same `db_path` and
/// the same `home_dir` simulate two lifetimes of the same `clouddeskd`
/// installation: a fresh process, the same persisted data.
async fn application_against_db_file(
    db_path: &std::path::Path,
    home_dir: &std::path::Path,
) -> Router {
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = clouddesk_db::connect(&url, 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    sqlx::query(
        "UPDATE system_settings SET value_json = 'true' WHERE key = 'runtime.media.enabled'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let availability = clouddesk_media::ffmpeg::detect(true).await;
    let cache_dir = home_dir.join(".music-cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    let media = clouddesk_media::MediaService::new(availability, pool.clone(), cache_dir);
    let library = clouddesk_library::LibraryStore::new(pool.clone());
    let auth = AuthService::new(
        pool,
        SecretCipher::new(&[9_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let secret_path = home_dir.join("bootstrap.secret");
    if !secret_path.exists() {
        fs::write(&secret_path, "one-time-test-secret\n").unwrap();
    }
    clouddeskd::application_router_and_media_and_library_configured(
        home_dir.to_owned(),
        auth,
        secret_path,
        true,
        Some(media),
        Some(library),
    )
}

fn request(method: Method, uri: &str, body: Body, cookie: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::ORIGIN, "https://localhost")
        .header("sec-fetch-site", "same-origin");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, format!("clouddesk_session={cookie}"));
    }
    request
        .header("x-forwarded-for", "127.0.0.1")
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
        .body(body)
        .unwrap()
}

fn json_request(method: Method, uri: &str, body: &Value, cookie: Option<&str>) -> Request<Body> {
    let mut req = request(
        method,
        uri,
        Body::from(serde_json::to_vec(body).unwrap()),
        cookie,
    );
    req.headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    req
}

fn current_process_linux_username() -> Option<String> {
    let uid = rustix::process::getuid();
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _password = fields.next();
        let entry_uid: u32 = fields.next()?.parse().ok()?;
        if entry_uid == uid.as_raw() {
            return Some(name.to_owned());
        }
    }
    None
}

async fn bootstrap_and_login(app: &Router, username: &str, password: &str) -> Option<String> {
    let real_username = current_process_linux_username()?;
    if real_username != username {
        return None;
    }
    let bootstrap = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/bootstrap",
            &json!({
                "secret": "one-time-test-secret",
                "username": username,
                "password": password,
            }),
            None,
        ))
        .await
        .unwrap();
    if bootstrap.status() != StatusCode::OK {
        return None;
    }
    login(app, username, password).await
}

async fn login(app: &Router, username: &str, password: &str) -> Option<String> {
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/auth/login",
            &json!({ "username": username, "password": password }),
            None,
        ))
        .await
        .unwrap();
    if response.status() != StatusCode::OK {
        return None;
    }
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.strip_prefix("clouddesk_session=")?
                .split(';')
                .next()
                .map(str::to_owned)
        })
}

async fn ffmpeg_available() -> bool {
    clouddesk_media::ffmpeg::detect(true).await.is_available()
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

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

/// Task 41/42: library roots, tracks, playlists, favorites,
/// recently-played, and the queue all survive a `clouddeskd` restart
/// (independent process lifetime, same persisted database); a rescan
/// after "restart" produces no duplicate tracks.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn music_state_survives_a_restart_with_no_duplicate_reindex() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "music_state_survives_a_restart_with_no_duplicate_reindex",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("clouddesk.db");

    let music_home = std::env::var("HOME").unwrap();
    let target_dir = tempfile::tempdir_in(&music_home).unwrap();
    let dir_name = target_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    generate_track(target_dir.path(), "one.mp3", "First", "Artist").await;

    // -- "process lifetime 1": add root, scan, favorite, playlist, queue, recent.
    let app_a = application_against_db_file(&db_path, home.path()).await;
    let Some(cookie_a) = bootstrap_and_login(&app_a, "admin", "correct horse battery staple").await
    else {
        clouddesk_test_support::blocked_by_environment(
            "music_state_survives_a_restart_with_no_duplicate_reindex",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    };

    let add_root = app_a
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/roots",
            &json!({ "path": format!("/{dir_name}") }),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    assert_eq!(add_root.status(), StatusCode::OK);
    let root_id = body_json(add_root).await["id"].as_str().unwrap().to_owned();

    let scan = app_a
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(scan).await["added"], 1);

    let tracks = app_a
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    let track_id = body_json(tracks).await["tracks"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let playlist = app_a
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/playlists",
            &json!({ "name": "Survives Restart" }),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    let playlist_id = body_json(playlist).await["id"].as_str().unwrap().to_owned();
    let add_entry = app_a
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/music/playlists/{playlist_id}/entries"),
            &json!({ "track_id": track_id }),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    assert_eq!(add_entry.status(), StatusCode::NO_CONTENT);

    let favorite = app_a
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/music/favorites/{track_id}"),
            Body::empty(),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    assert_eq!(favorite.status(), StatusCode::NO_CONTENT);

    let record = app_a
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/recent",
            &json!({ "track_id": track_id }),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    assert_eq!(record.status(), StatusCode::NO_CONTENT);

    let set_queue = app_a
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/music/queue",
            &json!({ "track_ids": [track_id.clone()] }),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    assert_eq!(set_queue.status(), StatusCode::NO_CONTENT);

    // -- simulated restart: brand new pool/router/auth/library/media,
    // same db file, same home dir. The old app/pool go out of scope here.
    let app_b = application_against_db_file(&db_path, home.path()).await;
    let cookie_b = login(&app_b, "admin", "correct horse battery staple")
        .await
        .expect("session must be persisted across a restart");

    let tracks_b = app_b
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(tracks_b).await["total"], 1);

    let playlist_entries_b = app_b
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(playlist_entries_b)
            .await
            .as_array()
            .unwrap()
            .len(),
        1,
        "playlist entry must survive a restart"
    );

    let favorites_b = app_b
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/favorites",
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(favorites_b).await.as_array().unwrap().len(),
        1,
        "favorite must survive a restart"
    );

    let recent_b = app_b
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/recent",
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(recent_b).await.as_array().unwrap().len(),
        1,
        "recently-played must survive a restart"
    );

    let queue_b = app_b
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/queue",
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(queue_b).await["track_ids"],
        json!([track_id]),
        "queue must survive a restart"
    );

    // Rescan after "restart" must not duplicate the already-indexed track.
    let rescan_b = app_b
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    let summary = body_json(rescan_b).await;
    assert_eq!(summary["added"], 0);
    assert_eq!(summary["unchanged"], 1);

    let tracks_after_rescan = app_b
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(tracks_after_rescan).await["total"],
        1,
        "no duplicate track rows after a post-restart rescan"
    );
}
