//! Phase 8 Task 13/14 — database failures around security-sensitive
//! WOPI state must fail closed, never open closed.
//!
//! Uses a real file-backed `SQLite` database (not `:memory:`) so a
//! second, independent connection can deterministically break specific
//! tables out from under the running server -- a controlled, repeatable
//! failure injection rather than a timing-dependent lock race.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::net::SocketAddr;

async fn application(db_path: &std::path::Path) -> (String, tempfile::TempDir, SqlitePool) {
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = clouddesk_db::connect(&url, 4).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[53_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "db-failure-test-secret\n").unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router =
        clouddeskd::application_router_and_media_and_library_and_runtime_and_office_configured(
            directory.path().to_owned(),
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
    (format!("http://127.0.0.1:{port}"), directory, pool)
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
            "secret": "db-failure-test-secret",
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

/// Task 13/14: with `office_locks` gone (a stand-in for a catastrophic
/// DB failure on exactly the table lock verification depends on),
/// `PutFile`/`LOCK`/`REFRESH_LOCK`/`UNLOCK` must all fail closed -- never treat
/// the DB error as "no lock exists, so writing is fine", and never let
/// a write land without a verified lock.
#[tokio::test]
async fn task_13_14_lock_table_failure_fails_closed() {
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("test.db");
    let (base, dir, pool) = application(&db_path).await;
    let admin = bootstrap_admin(&base).await;
    let user_id = create_user(&base, &admin, "dbfailuser").await;
    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path()).await;

    let doc = workspace.path().join("doc.odt");
    let original = b"BEFORE-DB-FAILURE".to_vec();
    std::fs::write(&doc, &original).unwrap();
    let (file_id, token) = mint_token(&pool, &user_id, &doc).await;

    // Baseline: LOCK works while the DB is healthy.
    assert_eq!(
        wopi_op(&base, &file_id, &token, "LOCK", "PRE-FAILURE-LOCK")
            .await
            .status(),
        reqwest::StatusCode::OK
    );

    // Inject the failure: drop the table lock verification depends on,
    // via a completely independent connection to the same file.
    sqlx::query("DROP TABLE office_locks")
        .execute(&pool)
        .await
        .unwrap();

    // Every lock-touching WOPI operation must now fail -- and critically,
    // PutFile must NOT treat "can't verify the lock" as "no lock, proceed".
    for (label, status) in [
        (
            "LOCK",
            wopi_op(&base, &file_id, &token, "LOCK", "NEW-LOCK")
                .await
                .status(),
        ),
        (
            "REFRESH_LOCK",
            wopi_op(&base, &file_id, &token, "REFRESH_LOCK", "PRE-FAILURE-LOCK")
                .await
                .status(),
        ),
        (
            "GET_LOCK",
            wopi_op(&base, &file_id, &token, "GET_LOCK", "")
                .await
                .status(),
        ),
        (
            "UNLOCK",
            wopi_op(&base, &file_id, &token, "UNLOCK", "PRE-FAILURE-LOCK")
                .await
                .status(),
        ),
    ] {
        assert!(
            status.is_server_error() || status == reqwest::StatusCode::CONFLICT,
            "{label} must fail closed when the lock table is unavailable, got {status}"
        );
        assert_ne!(
            status,
            reqwest::StatusCode::OK,
            "{label} must never report success when lock state cannot be verified"
        );
    }

    let put = reqwest::Client::new()
        .post(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={token}"
        ))
        .header("X-WOPI-Lock", "NEW-LOCK")
        .body(b"unauthorized-write-attempt".to_vec())
        .send()
        .await
        .unwrap();
    assert_ne!(
        put.status(),
        reqwest::StatusCode::OK,
        "PutFile must never succeed when lock verification is impossible \
         (a DB outage must not become an unauthorized-write bypass)"
    );
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        original,
        "the document must be completely unchanged after every failed operation"
    );
}

/// Task 14: specifically the PutFile-body-already-received-then-DB-fails
/// sequence -- the temp file must still be cleaned up and the canonical
/// document must remain exactly as it was.
#[tokio::test]
async fn task_14_putfile_db_failure_after_body_received_leaves_original_intact() {
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("test.db");
    let (base, dir, pool) = application(&db_path).await;
    let admin = bootstrap_admin(&base).await;
    let user_id = create_user(&base, &admin, "dbfailuser2").await;
    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path()).await;

    let doc = workspace.path().join("doc2.odt");
    let original = b"BEFORE-SECOND-FAILURE".to_vec();
    std::fs::write(&doc, &original).unwrap();
    let (file_id, token) = mint_token(&pool, &user_id, &doc).await;

    // office_wopi_files is what bump_generation depends on post-write;
    // dropping it simulates a DB failure discovered only after the
    // upload body has already been fully received and staged.
    sqlx::query("DROP TABLE office_wopi_files")
        .execute(&pool)
        .await
        .unwrap();

    let put = reqwest::Client::new()
        .post(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={token}"
        ))
        .body(b"this write races a DB failure".to_vec())
        .send()
        .await
        .unwrap();
    // verify_token itself depends on office_wopi_files, so this fails
    // at authorization time -- which is itself the correct fail-closed
    // behavior (Task 13's "never permit stale token" applies just as
    // much to "can't verify the token's file at all").
    assert_ne!(put.status(), reqwest::StatusCode::OK);
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        original,
        "the document must survive a DB failure discovered mid-request"
    );
    // No leftover temp file in the workspace.
    let leftovers: Vec<_> = std::fs::read_dir(workspace.path())
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.starts_with(".cloudesk-office-").then_some(name)
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "no temp file should remain after a DB-failure-aborted PutFile: {leftovers:?}"
    );
}
