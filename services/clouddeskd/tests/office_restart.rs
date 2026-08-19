//! Phase 8 Task 15/16 — `clouddeskd` restart with a live WOPI lock.
//!
//! Simulates a real process restart by starting two fully independent
//! `axum::serve` instances (separate router, separate in-process
//! server, separate `AppState`) against the *same* file-backed `SQLite`
//! database -- exactly what a real process restart looks like from the
//! database's point of view: the old process's in-memory state is gone,
//! only what was persisted (the `office_locks` row) survives.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::net::SocketAddr;

async fn application(db_path: &std::path::Path) -> (String, SqlitePool) {
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = clouddesk_db::connect(&url, 4).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[59_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    // A stable secret file outside the per-call temp dir, so the bootstrap
    // secret is identical before and after the "restart".
    let secret_path = db_path.with_file_name("bootstrap.secret");
    if !secret_path.exists() {
        std::fs::write(&secret_path, "restart-test-secret\n").unwrap();
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router =
        clouddeskd::application_router_and_media_and_library_and_runtime_and_office_configured(
            db_path.parent().unwrap().to_owned(),
            auth,
            secret_path,
            true,
            None,
            None,
            None,
            Some(format!("http://127.0.0.1:{port}")),
        );
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://127.0.0.1:{port}"), pool)
}

fn current_process_linux_identity() -> Option<clouddesk_linux::LinuxIdentity> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        return None;
    }
    clouddesk_linux::lookup_uid(uid).ok().flatten()
}

async fn http(
    base: &str,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    body: Option<&Value>,
) -> reqwest::Response {
    let mut builder = reqwest::Client::new().request(
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
        format!("{base}{path}"),
    );
    if let Some(cookie) = cookie {
        builder = builder.header(reqwest::header::COOKIE, cookie);
    }
    if let Some(body) = body {
        builder = builder
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
    }
    builder.send().await.unwrap()
}

async fn bootstrap_admin(base: &str) -> String {
    let linux_username = current_process_linux_identity().map(|i| i.username);
    let response = http(
        base,
        Method::POST,
        "/api/v1/setup/bootstrap",
        None,
        Some(&json!({
            "secret": "restart-test-secret",
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
            "linux_username": linux_username,
        })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    login(base, "admin", "correct horse battery staple").await
}

async fn login(base: &str, username: &str, password: &str) -> String {
    let response = http(
        base,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(&json!({"username": username, "password": password})),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn step_up(base: &str, admin_cookie: &str) {
    let response = http(
        base,
        Method::POST,
        "/api/v1/auth/step-up",
        Some(admin_cookie),
        Some(&json!({"password": "correct horse battery staple"})),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

async fn create_user(base: &str, admin_cookie: &str, username: &str) -> String {
    let identity = current_process_linux_identity()
        .expect("this test requires running as a real, mapped, non-root Linux user");
    step_up(base, admin_cookie).await;
    let create = http(
        base,
        Method::POST,
        "/api/v1/users",
        Some(admin_cookie),
        Some(&json!({
            "username": username,
            "display_name": username,
            "password": "user horse battery staple",
            "role_ids": ["user"],
        })),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::CREATED);
    let body: Value = create.json().await.unwrap();
    let user_id = body["user_id"].as_str().unwrap().to_owned();
    let set_identity = http(
        base,
        Method::PUT,
        &format!("/api/v1/users/{user_id}/linux-identity"),
        Some(admin_cookie),
        Some(&json!({ "uid": identity.uid, "gid": identity.gid })),
    )
    .await;
    assert_eq!(set_identity.status(), reqwest::StatusCode::NO_CONTENT);
    user_id
}

async fn add_root(base: &str, admin_cookie: &str, user_id: &str, path: &std::path::Path) {
    step_up(base, admin_cookie).await;
    let response = http(
        base,
        Method::POST,
        &format!("/api/v1/users/{user_id}/assigned-roots"),
        Some(admin_cookie),
        Some(&json!({ "path": path, "access_mode": "read-write" })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
}

async fn mint_token(pool: &SqlitePool, user_id: &str, path: &std::path::Path) -> (String, String) {
    let canonical = std::fs::canonicalize(path).unwrap();
    let canonical = canonical.to_string_lossy().into_owned();
    let file_id = format!("f{}", unique());
    sqlx::query(
        "INSERT INTO office_wopi_files (id, canonical_path, identity_key, generation, created_at)
         VALUES (?, ?, ?, 0, 0) ON CONFLICT(identity_key) DO NOTHING",
    )
    .bind(&file_id)
    .bind(&canonical)
    .bind(&canonical)
    .execute(pool)
    .await
    .unwrap();
    let file_id: String =
        sqlx::query_scalar("SELECT id FROM office_wopi_files WHERE identity_key = ?")
            .bind(&canonical)
            .fetch_one(pool)
            .await
            .unwrap();
    let raw = format!("t{}", unique());
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    sqlx::query(
        "INSERT INTO office_wopi_tokens
            (token_hash, user_id, file_id, read_write, runtime_instance_id, created_at, expires_at)
         VALUES (?, ?, ?, 1, 'test-instance', 0, ?)",
    )
    .bind(hex::encode(hasher.finalize()))
    .bind(user_id)
    .bind(&file_id)
    .bind(i64::MAX)
    .execute(pool)
    .await
    .unwrap();
    (file_id, raw)
}

fn unique() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

async fn wopi_op(
    base: &str,
    file_id: &str,
    token: &str,
    override_header: &str,
    lock_value: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .header("X-WOPI-Override", override_header)
        .header("X-WOPI-Lock", lock_value)
        .send()
        .await
        .unwrap()
}

/// Task 15/16: a lock acquired before a `clouddeskd` restart must
/// remain valid, still reject the wrong lock value, and still accept
/// the correct refresh/unlock -- and must not become duplicatable or
/// bypassable purely because the process restarted.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_15_lock_survives_a_clouddeskd_restart() {
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("test.db");

    // --- "before restart": real process instance #1 ---
    let (base_a, pool) = application(&db_path).await;
    let admin = bootstrap_admin(&base_a).await;
    let user_id = create_user(&base_a, &admin, "restartuser").await;
    let workspace = tempfile::tempdir_in(db_dir.path()).unwrap();
    add_root(&base_a, &admin, &user_id, workspace.path()).await;

    let doc = workspace.path().join("doc.odt");
    let original = b"BEFORE-RESTART".to_vec();
    std::fs::write(&doc, &original).unwrap();
    let (file_id, token) = mint_token(&pool, &user_id, &doc).await;

    assert_eq!(
        wopi_op(&base_a, &file_id, &token, "LOCK", "SURVIVING-LOCK")
            .await
            .status(),
        reqwest::StatusCode::OK
    );

    // --- restart: a completely independent server instance, same DB
    //     file, no shared in-memory state whatsoever ---
    let (base_b, _pool_b) = application(&db_path).await;

    // The lock is still there and still enforced correctly.
    let wrong_lock = wopi_op(&base_b, &file_id, &token, "LOCK", "WRONG-AFTER-RESTART").await;
    assert_eq!(
        wrong_lock.status(),
        reqwest::StatusCode::CONFLICT,
        "the pre-restart lock must still be enforced after a restart"
    );
    assert_eq!(
        wrong_lock.headers().get("X-WOPI-Lock").unwrap(),
        "SURVIVING-LOCK",
        "the conflict must echo the real surviving lock value"
    );

    // The correct value still refreshes it.
    assert_eq!(
        wopi_op(&base_b, &file_id, &token, "REFRESH_LOCK", "SURVIVING-LOCK")
            .await
            .status(),
        reqwest::StatusCode::OK,
        "the correct lock value must still refresh successfully after a restart"
    );

    // A real save under the surviving lock still works correctly.
    let put = reqwest::Client::new()
        .post(format!(
            "{base_b}/wopi/files/{file_id}/contents?access_token={token}"
        ))
        .header("X-WOPI-Lock", "SURVIVING-LOCK")
        .body(b"AFTER-RESTART-SAVE".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), reqwest::StatusCode::OK);
    assert_eq!(std::fs::read(&doc).unwrap(), b"AFTER-RESTART-SAVE");

    // Correct unlock still works.
    assert_eq!(
        wopi_op(&base_b, &file_id, &token, "UNLOCK", "SURVIVING-LOCK")
            .await
            .status(),
        reqwest::StatusCode::OK
    );

    // No duplicate/conflicting lock became possible: after the unlock,
    // a fresh LOCK succeeds cleanly (exactly once, not into some
    // duplicated state).
    assert_eq!(
        wopi_op(&base_b, &file_id, &token, "LOCK", "POST-RESTART-FRESH-LOCK")
            .await
            .status(),
        reqwest::StatusCode::OK,
        "a fresh LOCK after the surviving lock is released must succeed exactly once"
    );
    // And GET_LOCK reports exactly that one value -- not two, not stale.
    let get_lock = wopi_op(&base_b, &file_id, &token, "GET_LOCK", "").await;
    assert_eq!(
        get_lock.headers().get("X-WOPI-Lock").unwrap(),
        "POST-RESTART-FRESH-LOCK"
    );
}
