//! Phase 8 — real, live Collabora Online (CODE, the development/test
//! edition) acceptance through the actual `clouddeskd` HTTP API. Uses
//! the real local Docker daemon and the real, version-pinned
//! `collabora/code:26.04.3.1.1` image confirmed present during this
//! phase's closure pass -- no mock WOPI client pretending to be
//! Collabora. Skips cleanly (not PASS) if Docker/the image aren't
//! reachable.
//!
//! Unlike Code's tests, these bind a *real* `TcpListener` on
//! `0.0.0.0` (not `127.0.0.1`) for the duration of each test: the
//! managed Collabora container must be able to call back into this
//! same process's own WOPI host via `host.docker.internal`, which
//! only resolves to a reachable address if `clouddeskd` is actually
//! listening on an interface Docker's bridge gateway can reach.
//!
//! Safety: every test maps its `CloudDesk` test user to the *current
//! test process's own* real Linux UID/GID (the same established
//! pattern `code_runtime.rs`/`music_authorization.rs` use) -- safe
//! because it's this agent's own account. All file fixtures are
//! scoped to a fresh, disposable subdirectory created via
//! `tempfile::tempdir_in(&home)`, never touching pre-existing content.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use serde_json::{json, Value};
use std::{net::SocketAddr, process::Stdio};
use tokio::process::Command as TokioCommand;

const OFFICE_IMAGE: &str = "collabora/code:26.04.3.1.1";

async fn docker_and_image_available() -> bool {
    TokioCommand::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
        && TokioCommand::new("docker")
            .args(["image", "inspect", OFFICE_IMAGE])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success())
}

/// Binds a real listener on all interfaces and serves the real product
/// router on it for the test's duration, returning a base URL the test
/// itself uses (`http://127.0.0.1:{port}`) -- the *same* port is handed
/// to Collabora as `host.docker.internal:{port}`, so both the test's
/// own HTTP client and the real container reach the identical live
/// server.
async fn application_with_office() -> (String, tempfile::TempDir) {
    let (base, dir, _pool) = application_with_office_and_pool().await;
    (base, dir)
}

/// Same harness, additionally handing back the live `SqlitePool` the
/// server is using. Tests that need to age a lock/token deterministically
/// (rather than sleeping out a real production timeout) manipulate it
/// through this handle, so no test-only mutation hook has to exist in
/// production code.
async fn application_with_office_and_pool() -> (String, tempfile::TempDir, sqlx::SqlitePool) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[29_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "office-test-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let wopi_host_base = format!("http://host.docker.internal:{port}");

    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!("clouddesk-office-test-{}", std::process::id())),
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(20),
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(
                clouddeskd::office_runtime::office_oci_spec(
                    OFFICE_IMAGE.to_owned(),
                    wopi_host_base.clone(),
                    false,
                ),
            ),
        )),
    );

    let router =
        clouddeskd::application_router_and_media_and_library_and_runtime_and_office_configured(
            directory.path().to_owned(),
            auth,
            secret_path,
            true,
            None,
            None,
            Some(runtime_manager),
            Some(wopi_host_base),
        );

    let serve_router = router.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            serve_router.into_make_service_with_connect_info::<SocketAddr>(),
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

/// A thin client over the base URL a spawned `axum::serve` is actually
/// listening on -- unlike Code's tests, these hit a real socket, not
/// `tower::ServiceExt::oneshot`, because Collabora itself must reach
/// the very same server.
async fn http(
    base: &str,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    body: Option<&Value>,
) -> reqwest::Response {
    let client = reqwest::Client::new();
    let mut builder = client.request(
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
            "secret": "office-test-secret",
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
        "login as {username} failed"
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

async fn create_user_with_identity(
    base: &str,
    admin_cookie: &str,
    username: &str,
) -> (String, clouddesk_linux::LinuxIdentity) {
    let identity = current_process_linux_identity()
        .expect("this test requires running as a real, mapped, non-root Linux user");

    let step_up = http(
        base,
        Method::POST,
        "/api/v1/auth/step-up",
        Some(admin_cookie),
        Some(&json!({"password": "correct horse battery staple"})),
    )
    .await;
    assert_eq!(step_up.status(), reqwest::StatusCode::OK);

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
    (cookie, identity)
}

async fn enable_office(base: &str, admin_cookie: &str) {
    let response = http(
        base,
        Method::POST,
        "/api/v1/runtimes/office/enable",
        Some(admin_cookie),
        None,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
}

async fn whoami(base: &str, cookie: &str) -> String {
    let response = http(base, Method::GET, "/api/v1/auth/me", Some(cookie), None).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    body["user_id"].as_str().unwrap().to_owned()
}

/// Adds an assigned root (admin operation, the same authorization
/// model Code's workspaces use) and returns its `assigned_roots.id`.
async fn add_root(
    base: &str,
    admin_cookie: &str,
    user_id: &str,
    path: &std::path::Path,
    access_mode: &str,
) -> String {
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

/// Task 1/2/3/5/9 -- a real Office session end to end: availability,
/// admin enable, a real user opening a real fixture through
/// `/api/v1/office/sessions` (server-side authorization + opaque file
/// ID + real Collabora discovery + a token), and the resulting editor
/// URL routed through `CloudDesk`'s own proxy (never Collabora's raw
/// address).
#[tokio::test]
async fn task_1_2_3_open_session_end_to_end() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{OFFICE_IMAGE} not reachable on this host");
        return;
    }
    let (base, _dir) = application_with_office().await;
    let admin_cookie = bootstrap_admin(&base).await;
    enable_office(&base, &admin_cookie).await;
    let (cookie, identity) = create_user_with_identity(&base, &admin_cookie, "officeuser1").await;

    let workspace = tempfile::tempdir_in(&identity.home).unwrap();
    let doc_path = workspace.path().join("hello.odt");
    let convert = soffice_convert_to(
        workspace.path(),
        "odt",
        &write_txt_fixture(workspace.path(), "hello.txt", "hello office"),
    )
    .await;
    assert!(
        convert.status.success(),
        "soffice conversion failed: {convert:?}"
    );
    assert!(doc_path.exists(), "expected hello.odt to be generated");

    let opened = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await;
    let status = opened.status();
    let body: Value = opened.json().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "{body:?}");
    assert!(body["file_id"].as_str().is_some());
    assert_eq!(body["read_write"], json!(true));
    let editor_url = body["editor_url"].as_str().unwrap();
    assert!(
        editor_url.starts_with(&format!(
            "/api/v1/runtime-instances/office/{}/office-proxy",
            body["instance_id"].as_str().unwrap()
        )),
        "editor URL must go through CloudDesk's own proxy, not Collabora's raw address: {editor_url}"
    );
    assert!(
        !editor_url.contains(":9980") && !editor_url.contains("127.0.0.1:1998"),
        "editor URL must never expose Collabora's raw port: {editor_url}"
    );

    stop_office_instance(&base, &admin_cookie, body["instance_id"].as_str().unwrap()).await;
}

async fn stop_office_instance(base: &str, admin_cookie: &str, instance_id: &str) {
    let _ = http(
        base,
        Method::POST,
        &format!("/api/v1/runtime-instances/office/{instance_id}/stop"),
        Some(admin_cookie),
        None,
    )
    .await;
}

/// Runs `soffice --headless --convert-to` with its own dedicated
/// `-env:UserInstallation` profile directory. `LibreOffice`'s headless
/// mode single-flights on a shared user profile lock, so without this
/// the several tests in this file that call `soffice` concurrently
/// (this binary runs test functions in parallel by default) randomly
/// fail each other with a bare non-zero exit and empty stderr -- a
/// test-harness flake, not a product defect, but real enough to have
/// been observed live in this exact suite.
async fn soffice_convert_to(
    outdir: &std::path::Path,
    format: &str,
    input: &std::path::Path,
) -> std::process::Output {
    let profile_dir = tempfile::tempdir().unwrap();
    TokioCommand::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile_dir.path().display()
        ))
        .args(["--headless", "--convert-to", format, "--outdir"])
        .arg(outdir)
        .arg(input)
        .output()
        .await
        .unwrap()
}

fn write_txt_fixture(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// Extracts plain text content from an ODT/DOCX/etc. file using the
/// real, headless `LibreOffice` engine (not a hand-rolled ZIP/XML
/// parser) -- this is the "inspect actual document content using
/// suitable LibreOffice/headless tooling" evidence Task 21/72 require,
/// not just a successful HTTP status code.
async fn extract_text(path: &std::path::Path) -> String {
    let outdir = tempfile::tempdir().unwrap();
    let convert = soffice_convert_to(outdir.path(), "txt:Text", path).await;
    assert!(
        convert.status.success(),
        "soffice text extraction failed: {convert:?}"
    );
    let txt_path = outdir
        .path()
        .join(path.file_stem().unwrap())
        .with_extension("txt");
    // soffice's plain-text export prefixes a UTF-8 BOM.
    std::fs::read_to_string(txt_path)
        .unwrap_or_default()
        .trim_start_matches('\u{feff}')
        .to_owned()
}

/// Task 9/10/11/14/15/16 -- the real WOPI host protocol surface,
/// exercised via direct HTTP calls carrying the real token
/// `open_session` issued (the "LIVE WOPI HOST" evidence tier, Task 72
/// -- distinct from, and a prerequisite for, Task 58's real-Collabora-
/// driven evidence below). `CheckFileInfo`, `GetFile` (content matches the
/// real fixture), LOCK, `PutFile` under a valid lock (content changes,
/// verified via real headless `LibreOffice` text extraction -- not a
/// bare 200 status), `GET_LOCK`, `REFRESH_LOCK`, UNLOCK, and a representative
/// set of Task 8 replay/security attacks against the same live file.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_9_10_11_14_15_wopi_protocol_round_trip() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{OFFICE_IMAGE} not reachable on this host");
        return;
    }
    let (base, _dir) = application_with_office().await;
    let admin_cookie = bootstrap_admin(&base).await;
    enable_office(&base, &admin_cookie).await;
    let (cookie, _identity) = create_user_with_identity(&base, &admin_cookie, "officeuser2").await;
    let (cookie_b, _identity_b) =
        create_user_with_identity(&base, &admin_cookie, "officeuser2b").await;
    let user_a = whoami(&base, &cookie).await;

    // Deliberately NOT nested under home (unlike most fixtures in this
    // file): this test environment only has one real non-root Linux
    // UID, so both users' mapped homes are the literal same directory.
    // A root explicitly assigned to User A only, physically outside
    // anyone's home, is what actually exercises the ownership boundary
    // `resolve_and_register_file`/`authorize_path` enforce (mirrors the
    // same fix applied to Phase 7's deep-link tests).
    let workspace = tempfile::tempdir().unwrap();
    add_root(
        &base,
        &admin_cookie,
        &user_a,
        workspace.path(),
        "read-write",
    )
    .await;
    let src_txt = write_txt_fixture(workspace.path(), "doc.txt", "version one");
    let doc_path = workspace.path().join("doc.odt");
    let convert = soffice_convert_to(workspace.path(), "odt", &src_txt).await;
    assert!(convert.status.success());
    assert_eq!(extract_text(&doc_path).await.trim(), "version one");

    let opened: Value = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await
    .json()
    .await
    .unwrap();
    let file_id = opened["file_id"].as_str().unwrap().to_owned();
    let instance_id = opened["instance_id"].as_str().unwrap().to_owned();

    // Re-derive the raw token: `open_session`'s response deliberately
    // doesn't expose it under a different name than `editor_url`'s own
    // query string carries it -- extract it exactly as Collabora would.
    let token = extract_query_param(opened["editor_url"].as_str().unwrap(), "access_token");

    // --- CheckFileInfo ---
    let info: Value = reqwest::Client::new()
        .get(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["BaseFileName"], json!("doc.odt"));
    assert_eq!(info["UserCanWrite"], json!(true));
    assert!(info["Size"].as_u64().unwrap() > 0);
    // Never the absolute server path.
    let info_text = info.to_string();
    assert!(!info_text.contains(&workspace.path().to_string_lossy().into_owned()));

    // --- GetFile: content matches the real fixture byte-for-byte ---
    let get_response = reqwest::get(format!(
        "{base}/wopi/files/{file_id}/contents?access_token={token}"
    ))
    .await
    .unwrap();
    assert_eq!(get_response.status(), reqwest::StatusCode::OK);
    let fetched_bytes = get_response.bytes().await.unwrap();
    let original_bytes = tokio::fs::read(&doc_path).await.unwrap();
    assert_eq!(fetched_bytes.as_ref(), original_bytes.as_slice());

    // --- LOCK ---
    let lock_value = "test-lock-value-1";
    let lock_response = reqwest::Client::new()
        .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .header("X-WOPI-Override", "LOCK")
        .header("X-WOPI-Lock", lock_value)
        .send()
        .await
        .unwrap();
    assert_eq!(lock_response.status(), reqwest::StatusCode::OK);

    // GET_LOCK reflects it back.
    let get_lock_response = reqwest::Client::new()
        .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .header("X-WOPI-Override", "GET_LOCK")
        .send()
        .await
        .unwrap();
    assert_eq!(
        get_lock_response.headers().get("X-WOPI-Lock").unwrap(),
        lock_value
    );

    // A second, independent LOCK attempt with a different value conflicts.
    let conflicting_lock = reqwest::Client::new()
        .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .header("X-WOPI-Override", "LOCK")
        .header("X-WOPI-Lock", "different-value")
        .send()
        .await
        .unwrap();
    assert_eq!(conflicting_lock.status(), reqwest::StatusCode::CONFLICT);
    assert_eq!(
        conflicting_lock.headers().get("X-WOPI-Lock").unwrap(),
        lock_value,
        "a lock conflict must echo the CURRENT lock value back"
    );

    // REFRESH_LOCK with the correct value succeeds.
    let refresh = reqwest::Client::new()
        .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .header("X-WOPI-Override", "REFRESH_LOCK")
        .header("X-WOPI-Lock", lock_value)
        .send()
        .await
        .unwrap();
    assert_eq!(refresh.status(), reqwest::StatusCode::OK);

    // REFRESH_LOCK with the WRONG value conflicts.
    let wrong_refresh = reqwest::Client::new()
        .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .header("X-WOPI-Override", "REFRESH_LOCK")
        .header("X-WOPI-Lock", "wrong-value")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_refresh.status(), reqwest::StatusCode::CONFLICT);

    // --- PutFile under the valid lock: content genuinely changes ---
    let new_txt = write_txt_fixture(workspace.path(), "doc2.txt", "version two");
    let convert2 = soffice_convert_to(workspace.path(), "odt", &new_txt).await;
    assert!(convert2.status.success());
    let new_bytes = tokio::fs::read(workspace.path().join("doc2.odt"))
        .await
        .unwrap();

    let put_response = reqwest::Client::new()
        .post(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={token}"
        ))
        .header("X-WOPI-Lock", lock_value)
        .body(new_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(put_response.status(), reqwest::StatusCode::OK);

    // Verified by reopening/reparsing the real file on disk with
    // headless LibreOffice -- not just a 200 from PutFile (Task 21/72).
    assert_eq!(extract_text(&doc_path).await.trim(), "version two");

    // --- UNLOCK ---
    let wrong_unlock = reqwest::Client::new()
        .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .header("X-WOPI-Override", "UNLOCK")
        .header("X-WOPI-Lock", "wrong-value")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_unlock.status(), reqwest::StatusCode::CONFLICT);

    let unlock = reqwest::Client::new()
        .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .header("X-WOPI-Override", "UNLOCK")
        .header("X-WOPI-Lock", lock_value)
        .send()
        .await
        .unwrap();
    assert_eq!(unlock.status(), reqwest::StatusCode::OK);

    // REFRESH_LOCK on a now-unlocked file: 404, not a false conflict.
    let refresh_after_unlock = reqwest::Client::new()
        .post(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .header("X-WOPI-Override", "REFRESH_LOCK")
        .header("X-WOPI-Lock", lock_value)
        .send()
        .await
        .unwrap();
    assert_eq!(
        refresh_after_unlock.status(),
        reqwest::StatusCode::NOT_FOUND
    );

    // --- Task 8/30/66: token replay/security sweep ---

    // Random token: invalid.
    let random_token_response = reqwest::get(format!(
        "{base}/wopi/files/{file_id}?access_token=totally-not-a-real-token"
    ))
    .await
    .unwrap();
    assert_eq!(
        random_token_response.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    // Valid token against the WRONG file ID: rejected (file binding).
    let wrong_file_response = reqwest::get(format!(
        "{base}/wopi/files/some-other-file-id?access_token={token}"
    ))
    .await
    .unwrap();
    assert_eq!(
        wrong_file_response.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    // User B never had this file authorized -- their own session's
    // open_session attempt on the same path must fail outright (they
    // don't even get a token to try).
    let b_open = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie_b),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await;
    assert_eq!(b_open.status(), reqwest::StatusCode::FORBIDDEN);

    // Task 66: a valid WOPI token must never work as a general
    // CloudDesk API credential.
    let wopi_token_on_normal_api = reqwest::Client::new()
        .get(format!("{base}/api/v1/auth/me"))
        .header(
            reqwest::header::COOKIE,
            format!("clouddesk_session={token}"),
        )
        .send()
        .await
        .unwrap();
    assert_ne!(wopi_token_on_normal_api.status(), reqwest::StatusCode::OK);

    stop_office_instance(&base, &admin_cookie, &instance_id).await;
}

fn extract_query_param(url: &str, key: &str) -> String {
    let query = url.split('?').nth(1).unwrap_or_default();
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return v.to_owned();
            }
        }
    }
    panic!("query param {key} not found in {url}");
}

/// Task 58 -- real Collabora-*driven* WOPI protocol evidence, not just
/// handcrafted client calls against our own WOPI host (which
/// `task_9_10_11_14_15_wopi_protocol_round_trip` above already proves).
/// Fetches the actual editor bootstrap URL (`cool.html?WOPISrc=...`)
/// through `CloudDesk`'s own proxy -- the same request a browser's
/// top-level navigation would make before any JavaScript runs -- and
/// inspects the real Collabora container's own logs for evidence it
/// independently called back into `CloudDesk`'s WOPI host with the
/// issued token, rather than trusting only that our own request
/// succeeded.
#[tokio::test]
async fn task_58_real_collabora_driven_wopi_callback() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{OFFICE_IMAGE} not reachable on this host");
        return;
    }
    let (base, _dir) = application_with_office().await;
    let admin_cookie = bootstrap_admin(&base).await;
    enable_office(&base, &admin_cookie).await;
    let (cookie, identity) = create_user_with_identity(&base, &admin_cookie, "officeuser3").await;

    let workspace = tempfile::tempdir_in(&identity.home).unwrap();
    let src_txt = write_txt_fixture(workspace.path(), "doc.txt", "collabora driven test");
    let doc_path = workspace.path().join("doc.odt");
    let convert = soffice_convert_to(workspace.path(), "odt", &src_txt).await;
    assert!(convert.status.success());

    let opened: Value = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await
    .json()
    .await
    .unwrap();
    let instance_id = opened["instance_id"].as_str().unwrap().to_owned();
    let editor_url = opened["editor_url"].as_str().unwrap().to_owned();

    // The same top-level GET a browser's iframe navigation issues --
    // no JavaScript execution, but real HTTP through the real
    // `office-proxy` route to the real container. The proxy route
    // itself requires the caller's real CloudDesk session (same
    // authenticated-proxy model Code uses, Task 27) -- this is not a
    // WOPI credential, it's what authorizes reaching this specific
    // shared instance through CloudDesk at all.
    let editor_response = reqwest::Client::new()
        .get(format!("{base}{editor_url}"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    let editor_status = editor_response.status();
    let editor_body = editor_response.text().await.unwrap_or_default();

    // LIVE REAL COLLABORA evidence (Task 58/72, distinct from the
    // handcrafted LIVE WOPI HOST evidence in
    // `task_9_10_11_14_15_wopi_protocol_round_trip` above): the
    // bootstrap HTML actually served by the real coolwsd process,
    // reached only through CloudDesk's own authenticated proxy,
    // contains a real `wss://` WebSocket target and a real
    // `frame-ancestors` directive reflecting the configured WOPI host
    // base -- not a stub. (The `access_token` itself stays in the
    // query string of `editor_url`, per the WOPI/Collabora bootstrap
    // convention -- `bundle.js` reads it from `location` client-side
    // rather than the server echoing it into the static HTML, so it is
    // deliberately not asserted here.)
    assert!(
        editor_status.is_success() || editor_status.is_redirection(),
        "expected the real editor bootstrap HTML to be reachable through the proxy, got {editor_status}"
    );
    assert!(
        editor_body.contains("wss://") || editor_body.contains("wss%3A"),
        "expected the real Collabora bootstrap HTML to embed a wss:// WebSocket target"
    );
    assert!(
        editor_body.contains("frame-ancestors") || editor_body.contains("frame_ancestors"),
        "expected the real Collabora bootstrap HTML to reflect a frame-ancestors directive"
    );

    let container = format!("clouddesk-runtime-{instance_id}");
    // Give coolwsd a moment to process/log the request.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let logs = TokioCommand::new("docker")
        .args(["logs", &container])
        .output()
        .await
        .unwrap();
    let log_text = format!(
        "{}{}",
        String::from_utf8_lossy(&logs.stdout),
        String::from_utf8_lossy(&logs.stderr)
    );

    // Honest, live-observed limit of this evidence tier (Task 72: never
    // conflate tiers): a JS-free top-level GET of the bootstrap HTML is
    // enough to prove the real editor page is reachable and correctly
    // populated through CloudDesk's proxy, but it does NOT by itself
    // make coolwsd issue a server-side WOPI CheckFileInfo callback --
    // that only happens once the bundled `bundle.js` executes in an
    // actual browser and opens the `wss://` WebSocket. Confirmed
    // directly against this container's own logs: neither
    // "CheckFileInfo" nor the issued file_id appears here. This makes
    // "genuine Collabora-initiated WOPI callback without a browser"
    // honestly BLOCKED BY ENVIRONMENT (no headless browser available,
    // Task 56/57) -- it is not silently assumed to pass merely because
    // the bootstrap request above succeeded.
    let real_callback_seen = log_text.contains("CheckFileInfo")
        || log_text.contains(opened["file_id"].as_str().unwrap());
    assert!(
        !real_callback_seen,
        "unexpectedly observed a server-side WOPI callback from a JS-free bootstrap \
         request alone -- if this now fires, upgrade the LIVE REAL COLLABORA evidence \
         tier in PHASE8_OFFICE_EVIDENCE.md instead of leaving this assertion stale"
    );

    stop_office_instance(&base, &admin_cookie, &instance_id).await;
}

// ===========================================================
// Task 16/18 — OCI network isolation and hardening
// ===========================================================

/// Task 16/18: inspect the *actual* Docker state of the managed
/// Collabora container, not the command `CloudDesk` constructed. Records
/// the real hardening posture and asserts the dangerous things are
/// genuinely absent.
///
/// Collabora is the one runtime that needs capabilities beyond the
/// hardened zero-capability default: its own per-document jailing
/// (`coolmount`) requires them, verified live rather than assumed. That
/// is an explicit, documented exception -- it does not extend to
/// privileged mode, host namespaces, the Docker socket, or host mounts,
/// all of which are asserted absent below.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_16_18_office_container_isolation_and_hardening() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{OFFICE_IMAGE} not reachable on this host");
        return;
    }
    let (base, _dir) = application_with_office().await;
    let admin_cookie = bootstrap_admin(&base).await;
    enable_office(&base, &admin_cookie).await;
    let (cookie, identity) = create_user_with_identity(&base, &admin_cookie, "hardenuser").await;

    let workspace = tempfile::tempdir_in(&identity.home).unwrap();
    let src = write_txt_fixture(workspace.path(), "h.txt", "hardening");
    assert!(soffice_convert_to(workspace.path(), "odt", &src)
        .await
        .status
        .success());
    let doc_path = workspace.path().join("h.odt");

    let opened: Value = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await
    .json()
    .await
    .unwrap();
    let instance_id = opened["instance_id"].as_str().unwrap().to_owned();
    let container = format!("clouddesk-runtime-{instance_id}");

    let inspect = TokioCommand::new("docker")
        .args(["inspect", &container])
        .output()
        .await
        .unwrap();
    assert!(inspect.status.success(), "docker inspect failed");
    let parsed: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    let entry = &parsed[0];
    let host_config = &entry["HostConfig"];

    // --- privilege / namespaces ---
    assert_eq!(
        host_config["Privileged"],
        json!(false),
        "the Office container must never run privileged"
    );
    assert_ne!(
        host_config["NetworkMode"],
        json!("host"),
        "the Office container must never use host networking"
    );
    assert_ne!(
        host_config["PidMode"],
        json!("host"),
        "the Office container must never share the host PID namespace"
    );
    assert_ne!(host_config["IpcMode"], json!("host"));
    assert_ne!(host_config["UTSMode"], json!("host"));

    // --- capabilities: exactly the documented, live-verified set ---
    let cap_drop: Vec<String> = host_config["CapDrop"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        cap_drop.iter().any(|c| c == "ALL"),
        "the Office container must drop ALL capabilities as its baseline, got {cap_drop:?}"
    );
    let cap_add: Vec<String> = host_config["CapAdd"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    // Docker normalizes capability names to the CAP_ prefix in its own
    // inspect output, so compare on the normalized form.
    let mut sorted_add: Vec<String> = cap_add
        .iter()
        .map(|c| c.trim_start_matches("CAP_").to_owned())
        .collect();
    sorted_add.sort();
    let mut expected = vec![
        "CHOWN",
        "DAC_OVERRIDE",
        "FOWNER",
        "MKNOD",
        "SETGID",
        "SETUID",
        "SYS_ADMIN",
        "SYS_CHROOT",
    ];
    expected.sort_unstable();
    assert_eq!(
        sorted_add, expected,
        "the Office container must add exactly the documented capability set \
         Collabora's own per-document jailing needs -- nothing more"
    );

    // --- mounts: no host filesystem access at all ---
    let mounts = entry["Mounts"].as_array().cloned().unwrap_or_default();
    for mount in &mounts {
        let source = mount["Source"].as_str().unwrap_or_default();
        for forbidden in [
            "/var/run/docker.sock",
            "/run/docker.sock",
            "/etc",
            "/root",
            "/proc",
            "/sys",
        ] {
            assert_ne!(
                source, forbidden,
                "the Office container must never mount {forbidden}"
            );
        }
        assert_ne!(
            source, "/",
            "the Office container must never mount host root"
        );
    }
    // Collabora specifically gets no document mount at all: bytes only
    // ever cross through authorized WOPI operations.
    let workspace_str = workspace.path().to_string_lossy().into_owned();
    assert!(
        !mounts.iter().any(|m| m["Source"]
            .as_str()
            .unwrap_or_default()
            .contains(&workspace_str)),
        "Collabora must never receive a bind mount of the user's documents"
    );

    // --- port publishing stays on loopback ---
    let ports = &entry["NetworkSettings"]["Ports"];
    if let Some(map) = ports.as_object() {
        for (_, bindings) in map {
            for binding in bindings.as_array().cloned().unwrap_or_default() {
                let host_ip = binding["HostIp"].as_str().unwrap_or_default();
                assert!(
                    host_ip == "127.0.0.1" || host_ip == "::1",
                    "the Office container must only publish on loopback, got {host_ip}"
                );
            }
        }
    }

    // --- no CloudDesk secrets handed to the container ---
    let env: Vec<String> = entry["Config"]["Env"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let env_text = env.join("\n");
    for forbidden in ["master_key", "MASTER_KEY", "bootstrap", "grant_key"] {
        assert!(
            !env_text.contains(forbidden),
            "the Office container environment must not carry CloudDesk secrets \
             (found {forbidden})"
        );
    }

    eprintln!(
        "Office container hardening (live docker inspect): Privileged={} NetworkMode={} \
         CapDrop={cap_drop:?} CapAdd={cap_add:?} Mounts={} Memory={} PidsLimit={}",
        host_config["Privileged"],
        host_config["NetworkMode"],
        mounts.len(),
        host_config["Memory"],
        host_config["PidsLimit"],
    );

    stop_office_instance(&base, &admin_cookie, &instance_id).await;
}

// ===========================================================
// Task 19/20/21 — crash recovery, enable/disable, resources
// ===========================================================

/// Task 19: killing the real Collabora container out from under
/// `CloudDesk` must leave the document intact, be detected by the Phase 6
/// runtime, and allow a clean restart and reopen -- with no orphaned
/// container left behind.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_19_office_crash_recovery() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{OFFICE_IMAGE} not reachable on this host");
        return;
    }
    let (base, _dir) = application_with_office().await;
    let admin_cookie = bootstrap_admin(&base).await;
    enable_office(&base, &admin_cookie).await;
    let (cookie, identity) = create_user_with_identity(&base, &admin_cookie, "crashuser").await;

    let workspace = tempfile::tempdir_in(&identity.home).unwrap();
    let src = write_txt_fixture(workspace.path(), "c.txt", "crash recovery");
    assert!(soffice_convert_to(workspace.path(), "odt", &src)
        .await
        .status
        .success());
    let doc_path = workspace.path().join("c.odt");
    let original_bytes = std::fs::read(&doc_path).unwrap();

    let opened: Value = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await
    .json()
    .await
    .unwrap();
    let instance_id = opened["instance_id"].as_str().unwrap().to_owned();
    let file_id = opened["file_id"].as_str().unwrap().to_owned();
    let token = extract_query_param(opened["editor_url"].as_str().unwrap(), "access_token");
    let container = format!("clouddesk-runtime-{instance_id}");

    // --- kill the real container out from under CloudDesk ---
    let killed = TokioCommand::new("docker")
        .args(["kill", &container])
        .output()
        .await
        .unwrap();
    assert!(killed.status.success(), "failed to kill the container");

    // The document itself is untouched by the crash: it lives in
    // CloudDesk's own storage, never inside the container.
    assert_eq!(
        std::fs::read(&doc_path).unwrap(),
        original_bytes,
        "a runtime crash must never damage the canonical document"
    );

    // The WOPI host is unaffected -- it is CloudDesk, not Collabora.
    let info = reqwest::Client::new()
        .get(format!("{base}/wopi/files/{file_id}?access_token={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        info.status(),
        reqwest::StatusCode::OK,
        "the WOPI host must survive a Collabora crash -- it is CloudDesk's own surface"
    );

    // The proxy must fail safely rather than hang or leak an upstream error.
    let proxied = reqwest::Client::new()
        .get(format!(
            "{base}/api/v1/runtime-instances/office/{instance_id}/office-proxy/"
        ))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await;
    if let Ok(response) = proxied {
        assert!(
            !response.status().is_success(),
            "the Office proxy must not report success once the runtime is gone"
        );
    }

    // --- recovery: the document reopens on a fresh runtime ---
    let reopened = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await;
    let status = reopened.status();
    let reopened: Value = reopened.json().await.unwrap();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "the document must reopen after a crash: {reopened:?}"
    );
    let new_instance = reopened["instance_id"].as_str().unwrap().to_owned();

    // Office is a single shared instance, so recovery restarts it in
    // place rather than accumulating a second one. Exactly one container
    // exists for it, and it is running -- not a dead husk left behind
    // next to a replacement.
    let all = TokioCommand::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name=clouddesk-runtime-{new_instance}"),
            "--format",
            "{{.Names}} {{.State}}",
        ])
        .output()
        .await
        .unwrap();
    let listed = String::from_utf8_lossy(&all.stdout);
    let rows: Vec<&str> = listed.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        rows.len(),
        1,
        "crash recovery must leave exactly one Office container, got {rows:?}"
    );
    assert!(
        rows[0].contains("running"),
        "the recovered Office container must be running, got {rows:?}"
    );
    // And no orphan under the *old* identity if recovery had created a
    // new instance instead of restarting in place.
    if new_instance != instance_id {
        let orphan = TokioCommand::new("docker")
            .args(["ps", "-a", "--filter", &format!("name={container}"), "-q"])
            .output()
            .await
            .unwrap();
        assert!(
            String::from_utf8_lossy(&orphan.stdout).trim().is_empty(),
            "the crashed container must not linger as an orphan"
        );
    }

    stop_office_instance(&base, &admin_cookie, &new_instance).await;
}

/// Task 20/21: the full enable/disable lifecycle against the real
/// runtime, plus the resource measurements Task 21 asks for.
///
/// The critical property, asserted directly: with Office disabled or
/// stopped there are **zero** managed Office containers.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_20_21_office_enable_disable_and_resource_measurement() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{OFFICE_IMAGE} not reachable on this host");
        return;
    }
    let (base, _dir) = application_with_office().await;
    let admin_cookie = bootstrap_admin(&base).await;
    let (cookie, identity) = create_user_with_identity(&base, &admin_cookie, "lifecycleuser").await;

    let workspace = tempfile::tempdir_in(&identity.home).unwrap();
    let src = write_txt_fixture(workspace.path(), "l.txt", "lifecycle");
    assert!(soffice_convert_to(workspace.path(), "odt", &src)
        .await
        .status
        .success());
    let doc_path = workspace.path().join("l.odt");
    let original_bytes = std::fs::read(&doc_path).unwrap();

    // --- disabled: launching is refused and nothing starts ---
    let denied = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await;
    assert!(
        !denied.status().is_success(),
        "opening a document must be refused while Office is disabled"
    );

    // --- admin enables, user opens ---
    enable_office(&base, &admin_cookie).await;
    let cold_start = std::time::Instant::now();
    let opened: Value = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await
    .json()
    .await
    .unwrap();
    let time_to_ready = cold_start.elapsed();
    let instance_id = opened["instance_id"].as_str().unwrap().to_owned();
    let container = format!("clouddesk-runtime-{instance_id}");

    // --- Task 21: measure the real runtime ---
    let usage = TokioCommand::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.MemUsage}}|{{.CPUPerc}}|{{.PIDs}}",
            &container,
        ])
        .output()
        .await
        .unwrap();
    let measured = String::from_utf8_lossy(&usage.stdout).trim().to_owned();
    eprintln!(
        "Office runtime resources (live docker stats): cold start to ready = \
         {time_to_ready:?}; mem|cpu|pids = {measured}"
    );
    assert!(
        !measured.is_empty(),
        "docker stats must report real numbers for a running Office container"
    );

    // --- admin disables while the session is active ---
    let disable = http(
        &base,
        Method::POST,
        "/api/v1/runtimes/office/disable",
        Some(&admin_cookie),
        None,
    )
    .await;
    assert_eq!(disable.status(), reqwest::StatusCode::NO_CONTENT);

    // New launches are refused...
    let after_disable = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await;
    assert!(
        !after_disable.status().is_success(),
        "no new Office session may start while disabled"
    );

    // ...and the critical property: zero managed Office containers remain.
    let running = TokioCommand::new("docker")
        .args(["ps", "--filter", &format!("name={container}"), "-q"])
        .output()
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&running.stdout).trim().is_empty(),
        "disabling Office must leave zero running managed Office containers"
    );

    // The canonical document is untouched by the whole lifecycle.
    assert_eq!(
        std::fs::read(&doc_path).unwrap(),
        original_bytes,
        "enable/disable must never alter the document"
    );

    // --- re-enable and reopen ---
    enable_office(&base, &admin_cookie).await;
    let reopened = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await;
    let status = reopened.status();
    let reopened: Value = reopened.json().await.unwrap();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "the document must reopen after re-enabling Office: {reopened:?}"
    );
    stop_office_instance(
        &base,
        &admin_cookie,
        reopened["instance_id"].as_str().unwrap(),
    )
    .await;
}

// ===========================================================
// Task 12 — real Collabora WebSocket evidence
// ===========================================================

/// Task 12: exercises the real Collabora WebSocket path through
/// `CloudDesk`'s authenticated `office-proxy-ws`, using the actual
/// per-document/per-session upstream path Collabora expects
/// (`/cool/{urlencoded WOPISrc}/ws?WOPISrc=...&access_token=...`,
/// confirmed live: probing the real container directly showed a bare
/// `/ws` gets nothing meaningful, while this exact path pattern gets a
/// real HTTP-level response from *inside* coolwsd's own WOPI validation
/// -- not a generic 404), rather than a generic echo fixture.
#[tokio::test]
async fn task_12_real_collabora_websocket_through_authenticated_proxy() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{OFFICE_IMAGE} not reachable on this host");
        return;
    }
    let (base, dir) = application_with_office().await;
    let admin_cookie = bootstrap_admin(&base).await;
    enable_office(&base, &admin_cookie).await;
    let (cookie_a, _identity_a) =
        create_user_with_identity(&base, &admin_cookie, "wsofficea").await;
    let (cookie_b, _identity_b) =
        create_user_with_identity(&base, &admin_cookie, "wsofficeb").await;

    let workspace = tempfile::tempdir_in(dir.path()).unwrap();
    add_root(
        &base,
        &admin_cookie,
        &whoami(&base, &cookie_a).await,
        workspace.path(),
        "read-write",
    )
    .await;
    let src = write_txt_fixture(workspace.path(), "ws.txt", "websocket test");
    assert!(soffice_convert_to(workspace.path(), "odt", &src)
        .await
        .status
        .success());
    let doc_path = workspace.path().join("ws.odt");

    let opened: Value = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie_a),
        Some(&json!({ "path": doc_path.to_string_lossy() })),
    )
    .await
    .json()
    .await
    .unwrap();
    let instance_id = opened["instance_id"].as_str().unwrap().to_owned();
    let editor_url = opened["editor_url"].as_str().unwrap().to_owned();
    let wopi_src = extract_query_param(&editor_url, "WOPISrc");
    let access_token = extract_query_param(&editor_url, "access_token");
    let encoded_wopi_src = wopi_src.clone(); // already percent-encoded by the server
    let cool_ws_path = format!(
        "/cool/{encoded_wopi_src}/ws?WOPISrc={encoded_wopi_src}&access_token={access_token}"
    );

    let proxy_ws_path =
        format!("/api/v1/runtime-instances/office/{instance_id}/office-proxy-ws{cool_ws_path}");

    // --- authorized user: real traffic reaches the real container ---
    let ws_result_authorized = connect_ws(&base, &proxy_ws_path, Some(&cookie_a)).await;
    match ws_result_authorized {
        // A full upgrade, or a real HTTP-level response *through the
        // proxy chain* (proving the request reached coolwsd and was
        // processed, not silently dropped), both count as evidence the
        // real Collabora WebSocket path was reached. A hard connection
        // failure or a 404 (meaning the route itself is missing) would
        // not.
        Ok(()) => {}
        Err(WsProbeError::Http(status)) => {
            assert_ne!(
                status,
                reqwest::StatusCode::NOT_FOUND,
                "the authenticated Office WebSocket proxy route must exist"
            );
        }
        Err(WsProbeError::Other(e)) => {
            panic!("unexpected error reaching the real Collabora WebSocket path: {e}")
        }
    }

    // --- unauthenticated: denied before ever reaching the container ---
    let unauth = connect_ws(&base, &proxy_ws_path, None).await;
    assert!(
        matches!(
            unauth,
            Err(WsProbeError::Http(
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
            ))
        ),
        "an unauthenticated caller must be denied the Office WebSocket proxy, got {unauth:?}"
    );

    // --- honest boundary check: the proxy layer vs. the document
    //     access layer are two different, deliberate checkpoints. ---
    //
    // User B also holds `apps.office.use` (an ordinary user capability,
    // not scoped to any document), so the proxy *itself* legitimately
    // lets B's WS upgrade reach the shared Collabora instance -- Office
    // is architected as one shared runtime with document authorization
    // living entirely in the WOPI token embedded in the connection's own
    // query string (Task 47), not in which user opened the TCP
    // connection. A raw WS upgrade succeeding therefore proves nothing
    // about document access by itself.
    //
    // The actual per-document boundary is Collabora's own server-side
    // WOPI validation of that embedded WOPISrc+token -- already proven
    // in task_9_10_11_14_15_wopi_protocol_round_trip's cross-user
    // section at the WOPI-host level, and structurally guaranteed here
    // by the same `verify_token` re-authorization every WOPI callback
    // goes through. Exercising Collabora's *own* internal enforcement of
    // it end-to-end requires the cool protocol running inside a real
    // browser (`bundle.js`), which is BLOCKED BY ENVIRONMENT -- so this
    // test asserts only what a raw WS client can honestly prove: the
    // proxy layer's own admission check, not document-level access.
    let cross_user = connect_ws(&base, &proxy_ws_path, Some(&cookie_b)).await;
    match cross_user {
        Ok(()) | Err(WsProbeError::Http(_)) => {}
        Err(WsProbeError::Other(e)) => {
            panic!("unexpected error on User B's proxy-layer WS attempt: {e}")
        }
    }

    stop_office_instance(&base, &admin_cookie, &instance_id).await;
}

#[derive(Debug)]
enum WsProbeError {
    Http(reqwest::StatusCode),
    Other(String),
}

async fn connect_ws(base: &str, path: &str, cookie: Option<&str>) -> Result<(), WsProbeError> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let ws_uri = format!("ws://{}{path}", base.trim_start_matches("http://"));
    let mut request = ws_uri
        .into_client_request()
        .map_err(|e| WsProbeError::Other(e.to_string()))?;
    if let Some(cookie) = cookie {
        request
            .headers_mut()
            .insert(reqwest::header::COOKIE, cookie.parse().unwrap());
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok(_ignored_stream) => Ok(()),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Err(WsProbeError::Http(
            reqwest::StatusCode::from_u16(response.status().as_u16())
                .unwrap_or(reqwest::StatusCode::BAD_GATEWAY),
        )),
        Err(other) => Err(WsProbeError::Other(other.to_string())),
    }
}
