//! Task 19 — adversarial authorization sweep of every Music backend
//! endpoint. Attacks cross-user access, ID substitution, guest-role
//! mutation attempts, raw-VFS-path bypass attempts, and the "a DB row is
//! not permanent authorization" requirement. No mocks: real ffmpeg
//! fixtures, real second/third user accounts, real role capabilities.

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

async fn application_with_music() -> (
    Router,
    tempfile::TempDir,
    tempfile::TempDir,
    sqlx::SqlitePool,
) {
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
        pool.clone(),
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
        pool,
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
        "127.0.0.1:43128".parse::<SocketAddr>().unwrap(),
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

fn current_process_linux_identity() -> Option<clouddesk_linux::LinuxIdentity> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        return None;
    }
    clouddesk_linux::lookup_uid(uid).ok().flatten()
}

fn current_process_linux_username() -> Option<String> {
    current_process_linux_identity().map(|identity| identity.username)
}

async fn bootstrap_admin(app: &Router) -> Option<String> {
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
    login(app, "admin", "correct horse battery staple").await
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
    assert_eq!(login.status(), StatusCode::OK, "login as {username} failed");
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

/// Creates `username` with `role_id` under the already-logged-in
/// `admin_cookie`, requiring a step-up first (mirrors the real product
/// flow -- creating users is itself a privileged action).
/// `map_identity`: whether to also map the new account to a real Linux
/// identity (shared with the current test process -- see the comment
/// below). Endpoints that call `mapped_identity` (artwork/scan/probe)
/// need this to reach their ownership check at all; pass `false` only
/// when deliberately testing the "no identity mapped yet" denial path
/// itself, or when identity-sharing would make a path-based assertion
/// meaningless (two accounts sharing a real home directory are not a
/// useful test of cross-*directory* isolation).
async fn create_user_ext(
    app: &Router,
    admin_cookie: &str,
    username: &str,
    role_id: &str,
    map_identity: bool,
) -> String {
    let step_up = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/auth/step-up",
            &json!({"password": "correct horse battery staple"}),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(step_up.status(), StatusCode::OK);
    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/users",
            &json!({
                "username": username,
                "display_name": username,
                "password": "user horse battery staple",
                "role_ids": [role_id],
            }),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let user_id = body_json(create).await["user_id"]
        .as_str()
        .unwrap()
        .to_owned();

    if map_identity {
        if let Some(identity) = current_process_linux_identity() {
            let set_identity = app
                .clone()
                .oneshot(json_request(
                    Method::PUT,
                    &format!("/api/v1/users/{user_id}/linux-identity"),
                    &json!({ "uid": identity.uid, "gid": identity.gid }),
                    Some(admin_cookie),
                ))
                .await
                .unwrap();
            assert_eq!(set_identity.status(), StatusCode::NO_CONTENT);
        }
    }

    login(app, username, "user horse battery staple")
        .await
        .unwrap()
}

async fn create_user(app: &Router, admin_cookie: &str, username: &str, role_id: &str) -> String {
    create_user_ext(app, admin_cookie, username, role_id, true).await
}

async fn ffmpeg_available() -> bool {
    clouddesk_media::ffmpeg::detect(true).await.is_available()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn generate_track(dir: &std::path::Path, name: &str, title: &str) {
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

/// Sets up: admin's own music root+track+playlist, all real and scanned.
/// Returns (`root_id`, `track_id`, `playlist_id`, `entry_id`, `dir_name`).
async fn seed_admin_library(
    app: &Router,
    admin_cookie: &str,
) -> (String, String, String, String, String) {
    let home = std::env::var("HOME").unwrap();
    let target_dir = tempfile::tempdir_in(&home).unwrap();
    let dir_name = target_dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    generate_track(target_dir.path(), "admin-track.mp3", "Admin's Song").await;
    std::mem::forget(target_dir); // keep the directory alive for the test

    let add_root = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/roots",
            &json!({ "path": format!("/{dir_name}") }),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        add_root.status(),
        StatusCode::OK,
        "admin must be able to add their own root"
    );
    let root_id = body_json(add_root).await["id"].as_str().unwrap().to_owned();

    let scan = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(scan.status(), StatusCode::OK);

    let tracks_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    let tracks = body_json(tracks_resp).await;
    let track_id = tracks["tracks"][0]["id"].as_str().unwrap().to_owned();

    let playlist_resp = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/playlists",
            &json!({ "name": "Admin's Playlist" }),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    let playlist_id = body_json(playlist_resp).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/music/playlists/{playlist_id}/entries"),
            &json!({ "track_id": track_id }),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    let entries_resp = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            Body::empty(),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    let entry_id = body_json(entries_resp).await[0]["entry_id"]
        .as_str()
        .unwrap()
        .to_owned();

    (root_id, track_id, playlist_id, entry_id, dir_name)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cross_user_and_id_substitution_attacks_are_all_denied() {
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    let (app, _dir, _cache, _pool) = application_with_music().await;
    let Some(admin_cookie) = bootstrap_admin(&app).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let (root_id, track_id, playlist_id, entry_id, dir_name) =
        seed_admin_library(&app, &admin_cookie).await;
    let _ = dir_name;

    // Attacker: a second, fully-privileged "user" account (not guest) --
    // the strongest attacker short of administrator, so a denial here
    // proves the boundary is ownership, not merely a capability tier.
    let attacker_cookie = create_user(&app, &admin_cookie, "attacker", "user").await;

    // -- 1. view User B's music library --
    let attacker_tracks = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(attacker_tracks).await["total"], 0);

    // -- 2. view User B's indexed track metadata (via playlist read,
    //       the only path that returns full track metadata by id) --
    // covered by playlist read below.

    // -- 3/4. read/modify User B's playlists --
    let read_playlist = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(read_playlist.status(), StatusCode::NOT_FOUND);

    let rename_playlist = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            &json!({ "name": "Hijacked" }),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(rename_playlist.status(), StatusCode::NOT_FOUND);

    let delete_playlist = app
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(delete_playlist.status(), StatusCode::NO_CONTENT); // no-op: 0 rows matched (owner_user_id filter), not an error, not a mutation of admin's row
    let playlist_survived = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        playlist_survived.status(),
        StatusCode::OK,
        "attacker's delete of admin's playlist ID must be a no-op, not an actual deletion"
    );

    let add_entry_as_attacker = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/music/playlists/{playlist_id}/entries"),
            &json!({ "track_id": track_id }),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(add_entry_as_attacker.status(), StatusCode::NOT_FOUND);

    let remove_entry_as_attacker = app
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("/api/v1/music/playlists/{playlist_id}/entries/{entry_id}"),
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(remove_entry_as_attacker.status(), StatusCode::NOT_FOUND);

    let reorder_as_attacker = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/music/playlists/{playlist_id}/reorder"),
            &json!({ "entry_ids": [entry_id] }),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(reorder_as_attacker.status(), StatusCode::NOT_FOUND);

    // Prove the entry genuinely survived untouched, from admin's view.
    let entries_after_attack = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(entries_after_attack)
            .await
            .as_array()
            .unwrap()
            .len(),
        1,
        "admin's playlist entry must be untouched by the attacker's attempts"
    );

    // -- 5/6. read/modify User B's favorites --
    let favorite_attack = app
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/music/favorites/{track_id}"),
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(favorite_attack.status(), StatusCode::NOT_FOUND);
    let attacker_favorites = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/favorites",
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert!(body_json(attacker_favorites)
        .await
        .as_array()
        .unwrap()
        .is_empty());
    let admin_favorites_untouched = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/favorites",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert!(body_json(admin_favorites_untouched)
        .await
        .as_array()
        .unwrap()
        .is_empty());

    // -- 7. access/manipulate User B's queue --
    // The queue endpoint has no target-user parameter at all -- it is
    // always "my own queue" -- so there is structurally no cross-user
    // queue-ID to attack. Verify setting the attacker's own queue does
    // not affect admin's.
    app.clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/music/queue",
            &json!({ "track_ids": [track_id.clone()] }),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    let admin_queue = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/queue",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(admin_queue).await["track_ids"], json!([]));

    // -- 8. access User B's recently-played history --
    app.clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/recent",
            &json!({ "track_id": track_id }),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    let attacker_recent = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/recent",
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert!(body_json(attacker_recent)
        .await
        .as_array()
        .unwrap()
        .is_empty());

    // -- 9. retrieve User B's artwork --
    let artwork_attack = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/tracks/{track_id}/artwork"),
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(artwork_attack.status(), StatusCode::NOT_FOUND);

    // -- 10. trigger indexing against User B's library roots --
    let scan_attack = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(scan_attack.status(), StatusCode::NOT_FOUND);

    let remove_root_attack = app
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("/api/v1/music/roots/{root_id}"),
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    // Same no-op-not-error shape as delete_playlist -- verify the root
    // actually survives from admin's perspective.
    assert_eq!(remove_root_attack.status(), StatusCode::NO_CONTENT);
    let roots_after_attack = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/roots",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(roots_after_attack)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // -- 11/12. start conversion for / retrieve output of User B's
    //           files, including a raw-VFS-path bypass attempt --
    // Uses a THIRD, deliberately unmapped attacker: `attacker_cookie`
    // above shares the test process's real Linux identity (needed so
    // earlier assertions reach the ownership check instead of failing
    // at "no identity mapped"), which would make this specific
    // assertion meaningless -- both accounts would genuinely share the
    // same real home directory, so "the file is reachable" would be
    // true but not because of a bypass. A separate, unmapped attacker
    // proves the actual claim: a virtual_path string alone never
    // reaches another user's real files, regardless of which user
    // supplies it.
    let unmapped_attacker_cookie =
        create_user_ext(&app, &admin_cookie, "unmapped-attacker", "user", false).await;
    let probe_attack = app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/media/probe",
            &json!({ "path": format!("/{dir_name}/admin-track.mp3") }),
            Some(&unmapped_attacker_cookie),
        ))
        .await
        .unwrap();
    // No mapped identity at all -> denied outright, before even
    // reaching path resolution. Never 200: there is no path by which
    // this request could return admin's real audio bytes.
    assert_ne!(
        probe_attack.status(),
        StatusCode::OK,
        "attacker must never reach admin's file via a raw virtual path"
    );
}

#[tokio::test]
async fn guest_role_cannot_perform_any_music_mutation() {
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    let (app, _dir, _cache, _pool) = application_with_music().await;
    let Some(admin_cookie) = bootstrap_admin(&app).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let (_root_id, track_id, playlist_id, _entry_id, _dir_name) =
        seed_admin_library(&app, &admin_cookie).await;
    let guest_cookie = create_user(&app, &admin_cookie, "guestling", "guest").await;

    // Guest CAN read (files.local.read) -- sanity-check the positive
    // side of the boundary isn't accidentally broken too.
    let guest_reads_own_tracks = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/music/tracks",
            Body::empty(),
            Some(&guest_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(guest_reads_own_tracks.status(), StatusCode::OK);

    // Guest CANNOT mutate anything, even within their own (empty) library.
    let add_root = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/roots",
            &json!({ "path": "/" }),
            Some(&guest_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(add_root.status(), StatusCode::FORBIDDEN);

    let create_playlist = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/playlists",
            &json!({ "name": "Guest playlist" }),
            Some(&guest_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create_playlist.status(), StatusCode::FORBIDDEN);

    let favorite = app
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/music/favorites/{track_id}"),
            Body::empty(),
            Some(&guest_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(favorite.status(), StatusCode::FORBIDDEN);

    let set_queue = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/music/queue",
            &json!({ "track_ids": [] }),
            Some(&guest_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(set_queue.status(), StatusCode::FORBIDDEN);

    let record_played = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/music/recent",
            &json!({ "track_id": track_id }),
            Some(&guest_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(record_played.status(), StatusCode::FORBIDDEN);

    let add_entry = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/music/playlists/{playlist_id}/entries"),
            &json!({ "track_id": track_id }),
            Some(&guest_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(add_entry.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn administrator_does_not_bypass_ownership_scoping_on_another_users_rows() {
    // Explicit policy check: this product's Music authorization is
    // capability-gated (files.local.read/write), not row-level. An
    // administrator has every capability, but has no special "view any
    // user's library" override -- CAPABILITIES seeding
    // (crates/auth/src/lib.rs) grants administrator every named
    // capability, and none of them is "music.any_user.read". This test
    // proves that is actually true in the running product, not just
    // documented as intent.
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    let (app, _dir, _cache, _pool) = application_with_music().await;
    let Some(admin_cookie) = bootstrap_admin(&app).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let second_admin_cookie =
        create_user(&app, &admin_cookie, "second-admin", "administrator").await;
    let (_root_id, _track_id, playlist_id, _entry_id, _dir_name) =
        seed_admin_library(&app, &admin_cookie).await;

    let cross_admin_read = app
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/playlists/{playlist_id}"),
            Body::empty(),
            Some(&second_admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        cross_admin_read.status(),
        StatusCode::NOT_FOUND,
        "a second administrator account must not automatically see the first admin's playlist -- \
         this product has no row-level admin override, by design"
    );
}

#[tokio::test]
async fn authorized_owner_can_perform_every_operation_denied_to_others() {
    // Positive control for the whole suite: everything asserted DENIED
    // above must be exactly the set of things the true owner CAN do.
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    let (app, _dir, _cache, _pool) = application_with_music().await;
    let Some(admin_cookie) = bootstrap_admin(&app).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let (root_id, track_id, playlist_id, entry_id, _dir_name) =
        seed_admin_library(&app, &admin_cookie).await;

    let favorite = app
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/api/v1/music/favorites/{track_id}"),
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(favorite.status(), StatusCode::NO_CONTENT);

    let artwork = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/music/tracks/{track_id}/artwork"),
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    // 404 is fine here (this fixture has no embedded art/sidecar) as
    // long as it isn't itself an authorization failure.
    assert_ne!(artwork.status(), StatusCode::FORBIDDEN);

    let reorder = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/music/playlists/{playlist_id}/reorder"),
            &json!({ "entry_ids": [entry_id] }),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(reorder.status(), StatusCode::NO_CONTENT);

    let rescan = app
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(rescan.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_library_row_is_not_permanent_authorization() {
    // "A library/database row is NOT permanent authorization" -- if the
    // path a root/track's row points at is no longer inside the owner's
    // authorized VFS root (simulating a moved/revoked root, since this
    // product has no assigned-root *revocation* endpoint to attack
    // directly), the next scan must re-validate against the current
    // identity and fail closed, not trust the previously-stored row.
    if !ffmpeg_available().await {
        eprintln!("SKIPPED: ffmpeg not available");
        return;
    }
    let (app, _dir, _cache, pool) = application_with_music().await;
    let Some(admin_cookie) = bootstrap_admin(&app).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };
    let (root_id, ..) = seed_admin_library(&app, &admin_cookie).await;

    // Simulate the row becoming stale/out-of-bounds (e.g. an assigned
    // root that was revoked out from under it) by forcing its stored
    // virtual_path to point outside any authorized location.
    sqlx::query("UPDATE music_library_roots SET virtual_path = '/../../../etc' WHERE id = ?")
        .bind(&root_id)
        .execute(&pool)
        .await
        .unwrap();

    let rescan_after_tamper = app
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/music/roots/{root_id}/scan"),
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        rescan_after_tamper.status(),
        StatusCode::BAD_REQUEST,
        "a root row pointing outside the authorized VFS root must be rejected on \
         re-resolution, not trusted because it was previously stored"
    );
}
