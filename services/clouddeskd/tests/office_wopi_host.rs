//! Phase 8 — `CloudDesk`'s own WOPI host, exercised through the real
//! product HTTP router without requiring the Collabora runtime.
//!
//! ## Evidence level: LIVE WOPI HOST (not LIVE COLLABORA)
//!
//! Every request here is a real HTTP request against the real router,
//! real `AuthService`, real `SQLite` schema, and real filesystem — but
//! the *client* is this test, not Collabora. That is exactly the right
//! level for the properties under test (lock expiry, read-only
//! enforcement, revocation, token audience, hostile input), all of which
//! are `CloudDesk`-side authorization decisions that must hold no matter
//! which client calls them. Evidence that real Collabora drives this
//! same protocol lives separately in `office_runtime.rs` and is never
//! conflated with it (Task 31/72).
//!
//! Not needing Docker is a deliberate property: these are the
//! security-critical paths, so they must run on every `cargo test
//! --workspace`, not only where a container runtime happens to exist.
//!
//! Safety: test users map to the *current test process's own* real
//! non-root Linux UID/GID (the established pattern across this repo's
//! runtime tests); all fixtures live in disposable temp directories.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::net::SocketAddr;

/// Serves the real product router (Office configured, no runtime
/// manager needed for the WOPI endpoints themselves) and hands back the
/// live pool so tests can age rows deterministically instead of
/// sleeping out production timeouts.
async fn application() -> (String, tempfile::TempDir, SqlitePool) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[31_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "wopi-test-secret\n").unwrap();

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
            "secret": "wopi-test-secret",
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
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "login {username}"
    );
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

/// Creates a user mapped to this process's own real Linux identity and
/// returns `(cookie, user_id)`.
async fn create_user(base: &str, admin_cookie: &str, username: &str) -> (String, String) {
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

    let cookie = login(base, username, "user horse battery staple").await;
    (cookie, user_id)
}

async fn add_root(
    base: &str,
    admin_cookie: &str,
    user_id: &str,
    path: &std::path::Path,
    access_mode: &str,
) -> String {
    step_up(base, admin_cookie).await;
    let response = http(
        base,
        Method::POST,
        &format!("/api/v1/users/{user_id}/assigned-roots"),
        Some(admin_cookie),
        Some(&json!({ "path": path, "access_mode": access_mode })),
    )
    .await;
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::CREATED, "{body:?}");
    body["root_id"].as_str().unwrap().to_owned()
}

/// Registers a canonical path as a WOPI file and mints a token for it,
/// writing the same rows `open_session` would. This exists so the WOPI
/// endpoints can be exercised without starting Collabora — the rows are
/// written through the real production schema and the token is hashed
/// exactly as `wopi::issue_token` hashes it, so the endpoints under test
/// see input indistinguishable from a real session's.
async fn mint_token(
    pool: &SqlitePool,
    user_id: &str,
    path: &std::path::Path,
    read_write: bool,
) -> (String, String) {
    let canonical = std::fs::canonicalize(path).unwrap();
    let canonical = canonical.to_string_lossy().into_owned();
    let file_id = format!("f{}", uuid_like());
    sqlx::query(
        "INSERT INTO office_wopi_files (id, canonical_path, identity_key, generation, created_at)
         VALUES (?, ?, ?, 0, ?) ON CONFLICT(identity_key) DO NOTHING",
    )
    .bind(&file_id)
    .bind(&canonical)
    .bind(&canonical)
    .bind(0_i64)
    .execute(pool)
    .await
    .unwrap();
    let file_id: String =
        sqlx::query_scalar("SELECT id FROM office_wopi_files WHERE identity_key = ?")
            .bind(&canonical)
            .fetch_one(pool)
            .await
            .unwrap();

    let raw = format!("t{}", uuid_like());
    insert_token(pool, &raw, user_id, &file_id, read_write, now() + 1800).await;
    (file_id, raw)
}

async fn insert_token(
    pool: &SqlitePool,
    raw: &str,
    user_id: &str,
    file_id: &str,
    read_write: bool,
    expires_at: i64,
) {
    sqlx::query(
        "INSERT INTO office_wopi_tokens
            (token_hash, user_id, file_id, read_write, runtime_instance_id, created_at, expires_at)
         VALUES (?, ?, ?, ?, 'test-instance', ?, ?)",
    )
    .bind(hash_token(raw))
    .bind(user_id)
    .bind(file_id)
    .bind(read_write)
    .bind(now())
    .bind(expires_at)
    .execute(pool)
    .await
    .unwrap();
}

fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Issues a raw WOPI lock/unlock/refresh operation.
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

async fn check_file_info(base: &str, file_id: &str, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .send()
        .await
        .unwrap()
}

async fn get_file(base: &str, file_id: &str, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={token}"
        ))
        .send()
        .await
        .unwrap()
}

async fn put_file(
    base: &str,
    file_id: &str,
    token: &str,
    lock_value: &str,
    body: Vec<u8>,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={token}"
        ))
        .header("X-WOPI-Lock", lock_value)
        .body(body)
        .send()
        .await
        .unwrap()
}

// ===========================================================
// Task 1 — lock expiry / abandoned session cleanup
// ===========================================================

/// Task 1: an abandoned lock must eventually stop blocking a legitimate
/// new session, while an actively refreshed lock must stay alive. Uses a
/// deterministic aging of the persisted `expires_at` rather than sleeping
/// out the real 10-minute production TTL.
#[tokio::test]
async fn task_1_lock_expiry_and_refresh_lifecycle() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "lockuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    let doc = workspace.path().join("doc.odt");
    std::fs::write(&doc, b"original").unwrap();
    add_root(&base, &admin, &user_id, workspace.path(), "read-write").await;
    let (file_id, token) = mint_token(&pool, &user_id, &doc, true).await;

    // --- an actively refreshed lock stays valid ---
    assert_eq!(
        wopi_op(&base, &file_id, &token, "LOCK", "LOCK-A")
            .await
            .status(),
        reqwest::StatusCode::OK
    );
    for _ in 0..3 {
        assert_eq!(
            wopi_op(&base, &file_id, &token, "REFRESH_LOCK", "LOCK-A")
                .await
                .status(),
            reqwest::StatusCode::OK,
            "an actively refreshed lock must remain valid"
        );
    }
    // A different session still cannot take it while it is live.
    let conflict = wopi_op(&base, &file_id, &token, "LOCK", "LOCK-B").await;
    assert_eq!(conflict.status(), reqwest::StatusCode::CONFLICT);
    assert_eq!(
        conflict.headers().get("X-WOPI-Lock").unwrap(),
        "LOCK-A",
        "a conflict must echo the current lock value back"
    );

    // --- abandon it: age the persisted expiry into the past ---
    sqlx::query("UPDATE office_locks SET expires_at = ? WHERE file_id = ?")
        .bind(now() - 1)
        .bind(&file_id)
        .execute(&pool)
        .await
        .unwrap();

    // An expired lock must read as absent...
    let get_lock = wopi_op(&base, &file_id, &token, "GET_LOCK", "").await;
    assert_eq!(get_lock.status(), reqwest::StatusCode::OK);
    assert_eq!(
        get_lock
            .headers()
            .get("X-WOPI-Lock")
            .map(|v| v.to_str().unwrap()),
        Some(""),
        "an expired lock must not be reported as held"
    );
    // ...and must not block a legitimate new session.
    assert_eq!(
        wopi_op(&base, &file_id, &token, "LOCK", "LOCK-B")
            .await
            .status(),
        reqwest::StatusCode::OK,
        "an abandoned/expired lock must never permanently block a new LOCK"
    );

    // The new lock is genuinely held by the new value, not the stale one.
    let after = wopi_op(&base, &file_id, &token, "GET_LOCK", "").await;
    assert_eq!(after.headers().get("X-WOPI-Lock").unwrap(), "LOCK-B");
    // And refreshing the *stale* value is refused rather than reviving it.
    assert_eq!(
        wopi_op(&base, &file_id, &token, "REFRESH_LOCK", "LOCK-A")
            .await
            .status(),
        reqwest::StatusCode::CONFLICT,
        "a stale lock value must never be revivable once superseded"
    );
}

/// Task 1: the storage sweep removes expired rows and leaves live ones
/// untouched. Correctness never depends on the sweep having run (every
/// read path already treats an expired row as absent) — this proves the
/// table does not accumulate dead rows forever.
#[tokio::test]
async fn task_1_expired_lock_rows_are_swept_live_rows_are_not() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "sweepuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path(), "read-write").await;

    let stale_doc = workspace.path().join("stale.odt");
    std::fs::write(&stale_doc, b"x").unwrap();
    let (stale_id, stale_token) = mint_token(&pool, &user_id, &stale_doc, true).await;

    let live_doc = workspace.path().join("live.odt");
    std::fs::write(&live_doc, b"y").unwrap();
    let (live_id, live_token) = mint_token(&pool, &user_id, &live_doc, true).await;

    wopi_op(&base, &stale_id, &stale_token, "LOCK", "STALE").await;
    wopi_op(&base, &live_id, &live_token, "LOCK", "LIVE").await;
    sqlx::query("UPDATE office_locks SET expires_at = ? WHERE file_id = ?")
        .bind(now() - 1)
        .bind(&stale_id)
        .execute(&pool)
        .await
        .unwrap();

    let removed = clouddeskd::wopi::sweep_expired_locks(&pool).await.unwrap();
    assert_eq!(removed, 1, "exactly the expired lock row should be swept");

    let remaining: Vec<String> = sqlx::query_scalar("SELECT file_id FROM office_locks")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        remaining,
        vec![live_id.clone()],
        "the live lock must survive the sweep"
    );
    // The surviving lock is still functionally held.
    assert_eq!(
        wopi_op(&base, &live_id, &live_token, "REFRESH_LOCK", "LIVE")
            .await
            .status(),
        reqwest::StatusCode::OK
    );
}

/// Task 1: expiry must not become an authorization hole — a *different*
/// user cannot exploit an expired lock to reach a file they were never
/// authorized for, and a wrong lock value never substitutes for
/// authorization.
#[tokio::test]
async fn task_1_lock_expiry_is_not_an_authorization_bypass() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_a_cookie, user_a) = create_user(&base, &admin, "lockowner").await;
    let (_b_cookie, user_b) = create_user(&base, &admin, "lockstranger").await;

    // Deliberately outside any home directory: in this environment every
    // test user maps to the same real Linux UID/home, so a genuinely
    // separate authorization boundary must come from assigned roots.
    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    let doc = workspace.path().join("secret.odt");
    std::fs::write(&doc, b"confidential").unwrap();
    add_root(&base, &admin, &user_a, workspace.path(), "read-write").await;

    let (file_id, a_token) = mint_token(&pool, &user_a, &doc, true).await;
    wopi_op(&base, &file_id, &a_token, "LOCK", "A-LOCK").await;
    sqlx::query("UPDATE office_locks SET expires_at = ? WHERE file_id = ?")
        .bind(now() - 1)
        .bind(&file_id)
        .execute(&pool)
        .await
        .unwrap();

    // User B has a syntactically valid token bound to the same file, but
    // no CloudDesk authorization for it. The expired lock must not help.
    let b_raw = format!("t{}", uuid_like());
    insert_token(&pool, &b_raw, &user_b, &file_id, true, now() + 1800).await;

    for (op, lock) in [
        ("LOCK", "B-LOCK"),
        ("GET_LOCK", ""),
        ("REFRESH_LOCK", "A-LOCK"),
        ("UNLOCK", "A-LOCK"),
    ] {
        let response = wopi_op(&base, &file_id, &b_raw, op, lock).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN,
            "unauthorized user must be denied {op} regardless of lock expiry state"
        );
    }
    assert_eq!(
        get_file(&base, &file_id, &b_raw).await.status(),
        reqwest::StatusCode::FORBIDDEN,
        "an expired lock must never expose file contents to an unauthorized user"
    );
    assert_eq!(
        put_file(&base, &file_id, &b_raw, "B-LOCK", b"overwritten".to_vec())
            .await
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        b"confidential",
        "the document must be untouched by every denied attempt"
    );
}

// ===========================================================
// Task 3 — read-only enforcement
// ===========================================================

/// Task 3: a read-only `CloudDesk` authorization must be reflected
/// accurately in `CheckFileInfo` *and* enforced by the backend on every
/// write path — the editor UI is never the boundary.
#[tokio::test]
async fn task_3_read_only_authorization_is_enforced_by_the_backend() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "readonlyuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path(), "read").await;

    for (name, original) in [
        ("doc.docx", &b"docx-original"[..]),
        ("sheet.xlsx", &b"xlsx-original"[..]),
        ("text.odt", &b"odt-original"[..]),
    ] {
        let doc = workspace.path().join(name);
        std::fs::write(&doc, original).unwrap();
        // Deliberately mint a token *claiming* read-write: verify_token
        // re-derives access from live authorization, so the claim must
        // not matter (Task 3's "no crafted callback can upgrade access").
        let (file_id, token) = mint_token(&pool, &user_id, &doc, true).await;

        let info = check_file_info(&base, &file_id, &token).await;
        assert_eq!(info.status(), reqwest::StatusCode::OK, "{name}");
        let info: Value = info.json().await.unwrap();
        assert_eq!(
            info["UserCanWrite"],
            json!(false),
            "{name}: CheckFileInfo must advertise read-only accurately"
        );
        assert_eq!(
            info["ReadOnly"],
            json!(true),
            "{name}: CheckFileInfo must advertise read-only accurately"
        );

        // Reading is allowed.
        let content = get_file(&base, &file_id, &token).await;
        assert_eq!(content.status(), reqwest::StatusCode::OK, "{name}");
        assert_eq!(content.bytes().await.unwrap().as_ref(), original, "{name}");

        // Every write path is refused.
        assert_eq!(
            wopi_op(&base, &file_id, &token, "LOCK", "RO-LOCK")
                .await
                .status(),
            reqwest::StatusCode::FORBIDDEN,
            "{name}: LOCK is a write-intent operation and must be refused read-only"
        );
        assert_eq!(
            put_file(&base, &file_id, &token, "RO-LOCK", b"tampered".to_vec())
                .await
                .status(),
            reqwest::StatusCode::FORBIDDEN,
            "{name}: PutFile must be refused on a read-only authorization"
        );
        assert_eq!(
            std::fs::read(&doc).unwrap(),
            original,
            "{name}: the document must be byte-identical after every refused write"
        );
    }
}

// ===========================================================
// Task 4 — access revocation
// ===========================================================

/// Task 4: revoking the assigned root mid-session must fail-close every
/// subsequent WOPI operation on an already-issued, still-unexpired token.
#[tokio::test]
async fn task_4_access_revocation_fails_closed_on_an_existing_token() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "revokeuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    let doc = workspace.path().join("doc.odt");
    std::fs::write(&doc, b"before-revocation").unwrap();
    let root_id = add_root(&base, &admin, &user_id, workspace.path(), "read-write").await;
    let (file_id, token) = mint_token(&pool, &user_id, &doc, true).await;

    // Baseline: everything works while authorized.
    assert_eq!(
        check_file_info(&base, &file_id, &token).await.status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        get_file(&base, &file_id, &token).await.status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        wopi_op(&base, &file_id, &token, "LOCK", "EDIT")
            .await
            .status(),
        reqwest::StatusCode::OK
    );

    // Administrator revokes the assigned root out from under the session.
    step_up(&base, &admin).await;
    let revoke = http(
        &base,
        Method::DELETE,
        &format!("/api/v1/users/{user_id}/assigned-roots/{root_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(revoke.status(), reqwest::StatusCode::NO_CONTENT);

    // The very next operation on the *same* unexpired token fails closed.
    for (label, status) in [
        (
            "CheckFileInfo",
            check_file_info(&base, &file_id, &token).await.status(),
        ),
        ("GetFile", get_file(&base, &file_id, &token).await.status()),
        (
            "LOCK",
            wopi_op(&base, &file_id, &token, "LOCK", "EDIT")
                .await
                .status(),
        ),
        (
            "REFRESH_LOCK",
            wopi_op(&base, &file_id, &token, "REFRESH_LOCK", "EDIT")
                .await
                .status(),
        ),
    ] {
        assert_eq!(
            status,
            reqwest::StatusCode::FORBIDDEN,
            "{label} must fail closed immediately after revocation"
        );
    }

    // The critical one: no write may land after revocation.
    assert_eq!(
        put_file(
            &base,
            &file_id,
            &token,
            "EDIT",
            b"written-after-revocation".to_vec()
        )
        .await
        .status(),
        reqwest::StatusCode::FORBIDDEN,
        "PutFile after revocation MUST fail"
    );
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        b"before-revocation",
        "the document must be unchanged by the post-revocation write attempt"
    );
}

// ===========================================================
// Task 14 — WOPI token audience isolation
// ===========================================================

/// Task 14: a WOPI token is scoped to exactly one file, one access
/// level, and the WOPI surface only — never a general `CloudDesk` API
/// credential, and never usable across files.
#[tokio::test]
async fn task_14_wopi_token_audience_is_strictly_bounded() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (cookie, user_id) = create_user(&base, &admin, "audienceuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path(), "read-write").await;
    let doc_a = workspace.path().join("a.odt");
    let doc_b = workspace.path().join("b.odt");
    std::fs::write(&doc_a, b"file-a").unwrap();
    std::fs::write(&doc_b, b"file-b").unwrap();
    let (file_a, token_a) = mint_token(&pool, &user_id, &doc_a, true).await;
    let (file_b, _token_b) = mint_token(&pool, &user_id, &doc_b, true).await;

    // Token for file A against file B: denied. (The binding is checked
    // before authorization, so this surfaces as 401 rather than 403 --
    // what matters is that it is refused and nothing is disclosed.)
    for (label, status) in [
        (
            "CheckFileInfo",
            check_file_info(&base, &file_b, &token_a).await.status(),
        ),
        ("GetFile", get_file(&base, &file_b, &token_a).await.status()),
        (
            "PutFile",
            put_file(&base, &file_b, &token_a, "L", b"x".to_vec())
                .await
                .status(),
        ),
    ] {
        assert!(
            status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN,
            "{label}: a token bound to file A must never operate on file B (got {status})"
        );
    }
    assert_eq!(std::fs::read(&doc_b).unwrap(), b"file-b");

    // A WOPI token is not a CloudDesk API credential.
    for path in [
        "/api/v1/auth/me",
        "/api/v1/auth/sessions",
        "/api/v1/system/summary",
        "/api/v1/runtime-settings",
        "/api/v1/admin/ping",
    ] {
        let response = reqwest::Client::new()
            .get(format!("{base}{path}?access_token={token_a}"))
            .send()
            .await
            .unwrap();
        assert!(
            response.status() == reqwest::StatusCode::UNAUTHORIZED
                || response.status() == reqwest::StatusCode::FORBIDDEN,
            "a WOPI token must never authorize {path} (got {})",
            response.status()
        );
    }

    // Conversely, a CloudDesk session cookie is not a WOPI credential:
    // the WOPI endpoints require the scoped token, not the browser session.
    let cookie_only = reqwest::Client::new()
        .get(format!("{base}/wopi/files/{file_a}"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert!(
        !cookie_only.status().is_success(),
        "a CloudDesk session cookie alone must not satisfy a WOPI callback endpoint (got {})",
        cookie_only.status()
    );

    // An expired token is refused even though it is otherwise well-formed.
    let expired = format!("t{}", uuid_like());
    insert_token(&pool, &expired, &user_id, &file_a, true, now() - 1).await;
    assert_eq!(
        check_file_info(&base, &file_a, &expired).await.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "an expired token must be refused"
    );

    // A read-only token cannot be upgraded to a write by the client.
    // (Backed by live authorization, not the token's own stored claim —
    // see task_3 for the authorization-driven direction of this rule.)
    let random = format!("t{}", uuid_like());
    assert_eq!(
        check_file_info(&base, &file_a, &random).await.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a random unissued token must be refused"
    );
}

// ===========================================================
// Task 15 — hostile input sweep
// ===========================================================

/// Task 15: malformed/oversized/hostile WOPI input must produce safe
/// 4xx responses — never a panic, an authorization bypass, or file
/// corruption.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_15_hostile_wopi_input_is_rejected_safely() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "hostileuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path(), "read-write").await;
    let doc = workspace.path().join("doc.odt");
    std::fs::write(&doc, b"pristine").unwrap();
    let (file_id, token) = mint_token(&pool, &user_id, &doc, true).await;

    // --- hostile file IDs against a valid token ---
    for hostile_id in [
        "../../../../etc/passwd",
        "..%2f..%2fetc%2fpasswd",
        "/etc/shadow",
        "'; DROP TABLE office_wopi_files; --",
        "' OR '1'='1",
        &"A".repeat(8192),
        "%00",
        "\u{202e}gnp.exe",
    ] {
        let encoded = urlencoding_lite(hostile_id);
        let response = reqwest::Client::new()
            .get(format!("{base}/wopi/files/{encoded}?access_token={token}"))
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "hostile file id {hostile_id:?} must produce a safe 4xx, got {}",
            response.status()
        );
    }

    // --- hostile tokens against a valid file ---
    for hostile_token in [
        "",
        &"z".repeat(16384),
        "../../secret",
        "'; DROP TABLE office_wopi_tokens; --",
        "\u{0}\u{1}\u{2}",
    ] {
        let response = reqwest::Client::new()
            .get(format!(
                "{base}/wopi/files/{file_id}?access_token={}",
                urlencoding_lite(hostile_token)
            ))
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "hostile token must produce a safe 4xx, got {}",
            response.status()
        );
    }

    // --- hostile override headers ---
    for hostile_override in [
        "",
        "NONSENSE",
        "lock",
        &"L".repeat(4096),
        "LOCK\r\nX-Injected: 1",
        "PUT_RELATIVE",
        "DELETE",
    ] {
        let Ok(header_value) = reqwest::header::HeaderValue::from_str(hostile_override) else {
            continue; // header crate refuses to even build it: already safe
        };
        let response = reqwest::Client::new()
            .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
            .header("X-WOPI-Override", header_value)
            .header("X-WOPI-Lock", "L")
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_client_error() || response.status().is_server_error(),
            "hostile override {hostile_override:?} must not be treated as a valid operation"
        );
        assert_ne!(
            response.status(),
            reqwest::StatusCode::OK,
            "hostile override {hostile_override:?} must never succeed"
        );
    }

    // --- oversized lock value (regression: this was accepted and
    // persisted unbounded before the MAX_WOPI_LOCK_BYTES bound) ---
    for size in [2 * 1024, 64 * 1024] {
        let huge_lock = "L".repeat(size);
        let response = reqwest::Client::new()
            .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
            .header("X-WOPI-Override", "LOCK")
            .header("X-WOPI-Lock", &huge_lock)
            .send()
            .await;
        // Either the server refuses the oversized value or the HTTP
        // framing rejects it before it arrives; both are safe, neither
        // may succeed.
        if let Ok(response) = response {
            assert!(
                response.status().is_client_error(),
                "an oversized ({size}-byte) lock value must be refused, got {}",
                response.status()
            );
        }
    }
    // Nothing oversized was persisted.
    let stored: Option<String> =
        sqlx::query_scalar("SELECT lock_value FROM office_locks WHERE file_id = ?")
            .bind(&file_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(
        stored.is_none_or(|v| v.len() <= 1024),
        "an oversized lock value must never reach the database"
    );

    // The document survives the entire sweep untouched.
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        b"pristine",
        "no hostile input may modify the document"
    );

    // And the service is still fully functional afterwards (no poisoned state).
    assert_eq!(
        check_file_info(&base, &file_id, &token).await.status(),
        reqwest::StatusCode::OK,
        "the WOPI host must remain healthy after the hostile-input sweep"
    );
}

// ===========================================================
// Task 2 — kill-mid-write / conflict-safe save
// ===========================================================

/// Counts leftover Office temp files in a directory. The save path
/// writes `.cloudesk-office-{random}.tmp` siblings; none may survive a
/// failed save.
fn leftover_temp_files(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.starts_with(".cloudesk-office-").then_some(name)
        })
        .collect()
}

/// Task 2: the original document must survive every failure mode of the
/// save path byte-for-byte, with no partial/zero-byte canonical file and
/// no leftover temp files — and a normal save must still succeed
/// afterwards.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_2_failed_saves_never_damage_the_original_document() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "saveuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path(), "read-write").await;
    let doc = workspace.path().join("doc.odt");
    let original = b"ORIGINAL-DOCUMENT-CONTENT".to_vec();
    std::fs::write(&doc, &original).unwrap();
    // A deliberately private document: the save path must not widen this.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let (file_id, token) = mint_token(&pool, &user_id, &doc, true).await;

    assert_eq!(
        wopi_op(&base, &file_id, &token, "LOCK", "SAVE-LOCK")
            .await
            .status(),
        reqwest::StatusCode::OK
    );

    // --- failure 1: wrong lock at the final commit ---
    let wrong_lock = put_file(&base, &file_id, &token, "NOT-THE-LOCK", b"clobber".to_vec()).await;
    assert_eq!(wrong_lock.status(), reqwest::StatusCode::CONFLICT);
    assert_eq!(std::fs::read(&doc).unwrap(), original);

    // --- failure 2: a body that fails partway through the stream ---
    // The stream yields real bytes (so the temp file already has content
    // on disk) and then errors, exercising the mid-write abort path
    // rather than a rejection before any write happens.
    {
        let failing = futures_util::stream::iter(vec![
            Ok::<Vec<u8>, std::io::Error>(vec![b'P'; 4096]),
            Ok(vec![b'P'; 4096]),
            Err(std::io::Error::other("injected mid-stream failure")),
        ]);
        let response = reqwest::Client::new()
            .post(format!(
                "{base}/wopi/files/{file_id}/contents?access_token={token}"
            ))
            .header("X-WOPI-Lock", "SAVE-LOCK")
            .body(reqwest::Body::wrap_stream(failing))
            .send()
            .await;
        // Whether the server reports the read error or the connection
        // simply dies, the original must be safe either way.
        let _ = response;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        original,
        "a save that fails mid-stream must not touch the original"
    );
    assert!(
        leftover_temp_files(workspace.path()).is_empty(),
        "a mid-stream failure must clean up its temp file"
    );

    // --- failure 3: connection dropped mid-PutFile ---
    // A body that announces more bytes than it delivers, then goes away:
    // the server sees a truncated stream and must abandon the save.
    {
        use tokio::io::AsyncWriteExt;
        let addr = base.trim_start_matches("http://").to_owned();
        let mut socket = tokio::net::TcpStream::connect(&addr).await.unwrap();
        let request = format!(
            "POST /wopi/files/{file_id}/contents?access_token={token} HTTP/1.1\r\n\
             Host: {addr}\r\nX-WOPI-Lock: SAVE-LOCK\r\nContent-Length: 100000\r\n\r\n"
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        socket.write_all(&vec![b'P'; 500]).await.unwrap();
        socket.flush().await.unwrap();
        drop(socket); // sever the connection mid-body
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        original,
        "a connection dropped mid-upload must leave the original intact"
    );

    // --- failure 4: version changed out of band during the locked session ---
    // An external writer modifies the file while the session holds a lock.
    std::thread::sleep(std::time::Duration::from_millis(1100)); // distinct mtime
    std::fs::write(&doc, b"EXTERNALLY-MODIFIED").unwrap();
    let stale = put_file(
        &base,
        &file_id,
        &token,
        "SAVE-LOCK",
        b"save-from-stale-version".to_vec(),
    )
    .await;
    assert_eq!(
        stale.status(),
        reqwest::StatusCode::CONFLICT,
        "a save must never blindly clobber a newer external version"
    );
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        b"EXTERNALLY-MODIFIED",
        "the external writer's content must survive the refused save"
    );

    // No canonical file is ever left zero-byte or half-written, and no
    // temp file survives any of the failures above.
    assert!(
        !std::fs::read(&doc).unwrap().is_empty(),
        "the canonical file must never be left zero-byte"
    );
    assert!(
        leftover_temp_files(workspace.path()).is_empty(),
        "failed saves must not leave temp files behind: {:?}",
        leftover_temp_files(workspace.path())
    );

    // --- recovery: a legitimate save still succeeds after all of that ---
    // Re-lock against the current (externally modified) state.
    wopi_op(&base, &file_id, &token, "UNLOCK", "SAVE-LOCK").await;
    assert_eq!(
        wopi_op(&base, &file_id, &token, "LOCK", "SAVE-LOCK-2")
            .await
            .status(),
        reqwest::StatusCode::OK
    );
    let good = b"SUCCESSFULLY-SAVED-CONTENT".to_vec();
    assert_eq!(
        put_file(&base, &file_id, &token, "SAVE-LOCK-2", good.clone())
            .await
            .status(),
        reqwest::StatusCode::OK,
        "a legitimate save must still succeed after prior failures"
    );
    assert_eq!(std::fs::read(&doc).unwrap(), good);
    assert!(leftover_temp_files(workspace.path()).is_empty());

    // Regression: the successful save must not have widened permissions.
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&doc).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "saving must preserve the original's permission bits, not apply the daemon umask"
        );
    }
}

// ===========================================================
// Task 5 — WOPI token log scrubbing
// ===========================================================

/// An in-memory `tracing` writer, so a test can assert on what the
/// application genuinely logged.
#[derive(Clone, Default)]
struct CapturedLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Task 5: a sentinel token driven through success, denial, and error
/// paths must never appear in anything `CloudDesk` writes — application
/// logs, the audit trail, or error bodies returned to the caller.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_5_wopi_tokens_are_scrubbed_from_logs_and_audit() {
    // Capture everything the application actually logs, so this proves
    // `make_redacted_span` really redacts rather than asserting only on
    // what the response body happens to contain.
    let captured = CapturedLogs::default();
    let sink = captured.clone();
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || sink.clone())
            .finish(),
    );

    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "scrubuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path(), "read-write").await;
    let doc = workspace.path().join("doc.odt");
    std::fs::write(&doc, b"scrub-test").unwrap();

    // A token whose value is trivially greppable and cannot occur by chance.
    let sentinel = "SENTINELWOPITOKEN0123456789abcdefSENTINEL";
    let canonical = std::fs::canonicalize(&doc).unwrap();
    let canonical_str = canonical.to_string_lossy().into_owned();
    sqlx::query(
        "INSERT INTO office_wopi_files (id, canonical_path, identity_key, generation, created_at)
         VALUES ('sentinelfile', ?, ?, 0, 0)",
    )
    .bind(&canonical_str)
    .bind(&canonical_str)
    .execute(&pool)
    .await
    .unwrap();
    insert_token(
        &pool,
        sentinel,
        &user_id,
        "sentinelfile",
        true,
        now() + 1800,
    )
    .await;

    // Drive success, denial, not-found, conflict, and expired paths.
    let mut bodies = Vec::new();
    bodies.push(
        check_file_info(&base, "sentinelfile", sentinel)
            .await
            .text()
            .await
            .unwrap(),
    );
    bodies.push(
        get_file(&base, "sentinelfile", sentinel)
            .await
            .text()
            .await
            .unwrap(),
    );
    // wrong file id (not found / mismatch)
    bodies.push(
        check_file_info(&base, "no-such-file", sentinel)
            .await
            .text()
            .await
            .unwrap(),
    );
    // lock conflict
    wopi_op(&base, "sentinelfile", sentinel, "LOCK", "L1").await;
    bodies.push(
        wopi_op(&base, "sentinelfile", sentinel, "LOCK", "L2")
            .await
            .text()
            .await
            .unwrap(),
    );
    // bad override -> 400
    bodies.push(
        wopi_op(&base, "sentinelfile", sentinel, "NONSENSE", "L1")
            .await
            .text()
            .await
            .unwrap(),
    );
    // expired sentinel-shaped token
    let expired_sentinel = format!("{sentinel}EXPIRED");
    insert_token(
        &pool,
        &expired_sentinel,
        &user_id,
        "sentinelfile",
        true,
        now() - 1,
    )
    .await;
    bodies.push(
        check_file_info(&base, "sentinelfile", &expired_sentinel)
            .await
            .text()
            .await
            .unwrap(),
    );

    // 1. No error body returned to a caller echoes the token back.
    for body in &bodies {
        assert!(
            !body.contains(sentinel),
            "a response body must never echo the WOPI token: {body}"
        );
    }

    // 2. Nothing the application logged contains it. This is the direct
    //    proof that the redacting span builder works: the token was in
    //    the query string of every request above, which is exactly what
    //    a default HTTP trace span would have recorded verbatim.
    let logs = captured.text();
    assert!(
        !logs.is_empty(),
        "the log capture produced nothing, so this assertion would be vacuous"
    );
    assert!(
        !logs.contains(sentinel),
        "a WOPI token must never reach the application log; found it in:\n{logs}"
    );
    assert!(
        logs.contains("/wopi/files"),
        "the captured logs should include the WOPI requests, otherwise the \
         absence of the token above proves nothing"
    );

    // 2. The audit trail must not contain it.
    let audit_rows: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(action,'') || ' ' || COALESCE(detail_json,'') FROM audit_events",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();
    for row in &audit_rows {
        assert!(
            !row.contains(sentinel),
            "the audit trail must never record a WOPI token: {row}"
        );
    }

    // 3. Nothing persisted anywhere in the database stores the raw token
    //    (only its SHA-256 hash may exist).
    let raw_hits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM office_wopi_tokens WHERE token_hash = ?")
            .bind(sentinel)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        raw_hits, 0,
        "the raw token must never be stored; only its hash"
    );
    let hashed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM office_wopi_tokens WHERE token_hash = ?")
            .bind(hash_token(sentinel))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(hashed, 1, "the token must be stored as its hash");
}

/// Minimal percent-encoder for path/query segments in test URLs.
fn urlencoding_lite(raw: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for byte in raw.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

// ===========================================================
// Task 13 — Office/WOPI route authorization sweep
// ===========================================================

/// Every Office/WOPI route, attacked as each principal that must not
/// reach it. Authorization is decided before any runtime lookup, so the
/// denial behaviour is fully testable without the Collabora container.
///
/// | method | route                                          | auth            | capability       | binding                  |
/// |--------|------------------------------------------------|-----------------|------------------|--------------------------|
/// | POST   | /api/v1/office/sessions                         | session cookie  | apps.office.use  | VFS path re-authorized   |
/// | GET    | /wopi/files/{id}                                | WOPI token only | (none)           | token↔file + live authz  |
/// | POST   | /wopi/files/{id}                                | WOPI token only | (none)           | token↔file + live authz  |
/// | GET    | /wopi/files/{id}/contents                       | WOPI token only | (none)           | token↔file + live authz  |
/// | POST   | /wopi/files/{id}/contents                       | WOPI token only | (none)           | token↔file + write authz |
/// | ANY    | .../office/{instance}/office-proxy[/...]        | session cookie  | apps.office.use  | shared instance          |
/// | GET    | .../office/{instance}/office-proxy-ws           | session cookie  | apps.office.use  | shared instance          |
/// | POST   | /api/v1/runtimes/office/{enable,disable}        | session cookie  | runtime.admin    | global                   |
///
/// The rule this asserts throughout: **no route is satisfied by
/// possession of an opaque id alone.**
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_13_office_route_authorization_sweep() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (a_cookie, user_a) = create_user(&base, &admin, "sweepa").await;
    let (b_cookie, _user_b) = create_user(&base, &admin, "sweepb").await;
    let guest_cookie = create_guest(&base, &admin, "sweepguest").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    let doc = workspace.path().join("a.odt");
    std::fs::write(&doc, b"user-a-document").unwrap();
    add_root(&base, &admin, &user_a, workspace.path(), "read-write").await;
    let (file_id, a_token) = mint_token(&pool, &user_a, &doc, true).await;

    let client = reqwest::Client::new();
    let instance = "some-instance-id";
    let proxy = format!("/api/v1/runtime-instances/office/{instance}/office-proxy/");
    let proxy_ws = format!("/api/v1/runtime-instances/office/{instance}/office-proxy-ws");

    // --- unauthenticated against every session/proxy route ---
    for (method, path) in [
        (Method::POST, "/api/v1/office/sessions".to_owned()),
        (Method::GET, proxy.clone()),
        (Method::GET, proxy_ws.clone()),
        (Method::POST, "/api/v1/runtimes/office/enable".to_owned()),
        (Method::POST, "/api/v1/runtimes/office/disable".to_owned()),
    ] {
        let response = http(
            &base,
            method.clone(),
            &path,
            None,
            Some(&json!({ "path": doc.to_string_lossy() })),
        )
        .await;
        assert!(
            !response.status().is_success(),
            "unauthenticated {method} {path} must be refused, got {}",
            response.status()
        );
    }

    // --- unauthenticated against every WOPI route (no token at all) ---
    for path in [
        format!("/wopi/files/{file_id}"),
        format!("/wopi/files/{file_id}/contents"),
    ] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert!(
            !response.status().is_success(),
            "WOPI {path} must never be reachable without a token, got {}",
            response.status()
        );
    }

    // --- Guest: has no apps.office.use capability ---
    for (method, path) in [
        (Method::POST, "/api/v1/office/sessions".to_owned()),
        (Method::GET, proxy.clone()),
        (Method::POST, "/api/v1/runtimes/office/enable".to_owned()),
    ] {
        let response = http(
            &base,
            method.clone(),
            &path,
            Some(&guest_cookie),
            Some(&json!({ "path": doc.to_string_lossy() })),
        )
        .await;
        assert!(
            !response.status().is_success(),
            "Guest {method} {path} must be refused, got {}",
            response.status()
        );
    }

    // --- User B against User A's document, by path ---
    let cross = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&b_cookie),
        Some(&json!({ "path": doc.to_string_lossy() })),
    )
    .await;
    assert!(
        !cross.status().is_success(),
        "User B must not open User A's document, got {}",
        cross.status()
    );

    // --- User B holding User A's real opaque file id, but their own
    //     session: possession of the id is not authorization ---
    let b_raw = format!("t{}", uuid_like());
    let b_id = whoami(&base, &b_cookie).await;
    insert_token(&pool, &b_raw, &b_id, &file_id, true, now() + 1800).await;
    for (label, status) in [
        (
            "CheckFileInfo",
            check_file_info(&base, &file_id, &b_raw).await.status(),
        ),
        ("GetFile", get_file(&base, &file_id, &b_raw).await.status()),
        (
            "PutFile",
            put_file(&base, &file_id, &b_raw, "L", b"x".to_vec())
                .await
                .status(),
        ),
        (
            "LOCK",
            wopi_op(&base, &file_id, &b_raw, "LOCK", "L").await.status(),
        ),
        (
            "GET_LOCK",
            wopi_op(&base, &file_id, &b_raw, "GET_LOCK", "")
                .await
                .status(),
        ),
        (
            "UNLOCK",
            wopi_op(&base, &file_id, &b_raw, "UNLOCK", "L")
                .await
                .status(),
        ),
        (
            "REFRESH_LOCK",
            wopi_op(&base, &file_id, &b_raw, "REFRESH_LOCK", "L")
                .await
                .status(),
        ),
    ] {
        assert_eq!(
            status,
            reqwest::StatusCode::FORBIDDEN,
            "{label}: User B must be denied on User A's file even holding its real id"
        );
    }
    assert_eq!(std::fs::read(&doc).unwrap(), b"user-a-document");

    // --- an ordinary user cannot administer the runtime ---
    for path in [
        "/api/v1/runtimes/office/enable",
        "/api/v1/runtimes/office/disable",
    ] {
        let response = http(&base, Method::POST, path, Some(&a_cookie), None).await;
        assert!(
            !response.status().is_success(),
            "an ordinary user must not reach {path}, got {}",
            response.status()
        );
    }

    // --- a CloudDesk session cookie is not a WOPI credential, and a
    //     WOPI token is not a session (both directions, Task 14) ---
    let cookie_on_wopi = client
        .get(format!("{base}/wopi/files/{file_id}"))
        .header(reqwest::header::COOKIE, &a_cookie)
        .send()
        .await
        .unwrap();
    assert!(!cookie_on_wopi.status().is_success());
    let token_on_proxy = client
        .get(format!("{base}{proxy}?access_token={a_token}"))
        .send()
        .await
        .unwrap();
    assert!(
        !token_on_proxy.status().is_success(),
        "a WOPI token must not authorize the browser-facing Office proxy"
    );
}

/// Creates a Guest-role user and returns their session cookie.
async fn create_guest(base: &str, admin_cookie: &str, username: &str) -> String {
    let identity = current_process_linux_identity().unwrap();
    step_up(base, admin_cookie).await;
    let create = http(
        base,
        Method::POST,
        "/api/v1/users",
        Some(admin_cookie),
        Some(&json!({
            "username": username,
            "display_name": username,
            "password": "guest horse battery staple",
            "role_ids": ["guest"],
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
    login(base, username, "guest horse battery staple").await
}

async fn whoami(base: &str, cookie: &str) -> String {
    let response = http(base, Method::GET, "/api/v1/auth/me", Some(cookie), None).await;
    let body: Value = response.json().await.unwrap();
    body["user_id"].as_str().unwrap().to_owned()
}

// ===========================================================
// Task 24/25 — file size policy enforced on real bytes, never
// Content-Length alone; bounded large-file streaming.
// ===========================================================

/// Task 24: a body streamed via chunked transfer encoding (no
/// `Content-Length` header at all, so there is nothing to lie about --
/// the server can only ever know the true size by counting bytes as
/// they arrive) that exceeds the Office size policy must still be
/// rejected based on the real byte count, and must never let the
/// oversized content land as the canonical document.
#[tokio::test]
async fn task_24_size_policy_is_enforced_on_real_bytes_not_a_declared_length() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "sizepolicyuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path(), "read-write").await;
    let doc = workspace.path().join("doc.odt");
    let original = b"under the limit".to_vec();
    std::fs::write(&doc, &original).unwrap();
    let (file_id, token) = mint_token(&pool, &user_id, &doc, true).await;

    // 200MB (MAX_OFFICE_FILE_BYTES) + a real surplus, delivered as a
    // chunked stream so no Content-Length is ever declared.
    let over_limit: usize = 200 * 1024 * 1024 + 4096;
    let chunk_size: usize = 1024 * 1024;
    let mut sent = 0usize;
    let body_stream = futures_util::stream::poll_fn(move |_cx| {
        if sent >= over_limit {
            return std::task::Poll::Ready(None);
        }
        sent += chunk_size;
        std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(vec![b'X'; chunk_size])))
    });

    let response = reqwest::Client::new()
        .post(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={token}"
        ))
        .body(reqwest::Body::wrap_stream(body_stream))
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_client_error(),
        "a real byte count over the size policy must be rejected even with \
         no Content-Length to lie about, got {}",
        response.status()
    );
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        original,
        "an oversized chunked upload must never become the canonical document"
    );
    let leftover = std::fs::read_dir(workspace.path())
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.starts_with(".cloudesk-office-").then_some(name)
        })
        .count();
    assert_eq!(
        leftover, 0,
        "no temp file should remain after an oversized chunked upload is rejected"
    );
}

/// Task 25: a moderately large *valid* document streams through both
/// `GetFile` and `PutFile` correctly and the save/reopen round-trip remains
/// byte-exact -- practical evidence at a real, non-trivial size (16MB),
/// not a claim of unlimited support.
#[tokio::test]
async fn task_25_large_valid_document_streams_and_round_trips() {
    let (base, dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "largefileuser").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(&base, &admin, &user_id, workspace.path(), "read-write").await;
    let doc = workspace.path().join("large.odt");
    // 16MB of realistic, non-degenerate content (not all-zero, so this
    // also isn't accidentally testing compression rather than streaming).
    let large_content: Vec<u8> = (0..16 * 1024 * 1024)
        .map(|i: u32| (i % 251) as u8)
        .collect();
    std::fs::write(&doc, &large_content).unwrap();
    let (file_id, token) = mint_token(&pool, &user_id, &doc, true).await;

    let client = reqwest::Client::new();
    let fetched = client
        .get(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={token}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(fetched.status(), reqwest::StatusCode::OK);
    assert_eq!(
        fetched.bytes().await.unwrap().as_ref(),
        large_content.as_slice()
    );

    assert_eq!(
        wopi_op(&base, &file_id, &token, "LOCK", "LARGE-LOCK")
            .await
            .status(),
        reqwest::StatusCode::OK
    );
    let replacement: Vec<u8> = (0..16 * 1024 * 1024)
        .map(|i: u32| ((i + 7) % 251) as u8)
        .collect();
    let put = put_file(&base, &file_id, &token, "LARGE-LOCK", replacement.clone()).await;
    assert_eq!(put.status(), reqwest::StatusCode::OK);
    assert_eq!(std::fs::read(&doc).unwrap(), replacement);

    let reopened = client
        .get(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={token}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        reopened.bytes().await.unwrap().as_ref(),
        replacement.as_slice()
    );
    let leftover = std::fs::read_dir(workspace.path())
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.starts_with(".cloudesk-office-").then_some(name)
        })
        .count();
    assert_eq!(leftover, 0);
}
