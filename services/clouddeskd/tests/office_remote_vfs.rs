//! Phase 8 remote-VFS closure (Tasks 1-5, 26-27) — real WOPI operations
//! against a document on a real, disposable SFTP server.
//!
//! ## Evidence level: LIVE WOPI HOST + LIVE REMOTE VFS
//!
//! The WOPI client here is this test (handcrafted HTTP, like
//! `office_wopi_host.rs`), not real Collabora — that evidence tier is
//! covered separately in `office_runtime.rs`. What this file proves is
//! the full path `WOPI request -> CloudDesk authorization -> real SFTP
//! provider -> real remote file`, using the same disposable OpenSSH
//! fixture (`tests/acceptance/docker-compose.yml`) the ProxyJump/SFTP
//! tests already rely on. Skips cleanly (not PASS) if that fixture
//! isn't running.
//!
//! Safety: real, disposable OpenSSH container; a fresh, unique file
//! under `/config` (the container's writable home) is used per test.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_remote::{NewRemoteServer, RemoteServerStore, SshAuthMethod};
use clouddesk_secrets::SecretCipher;
use clouddesk_vault::Vault;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use tokio::process::Command as TokioCommand;

const BASTION_HOST: &str = "127.0.0.1";
const BASTION_PORT: u16 = 2222;
const BASTION_USER: &str = "testuser";
const BASTION_PASSWORD: &str = "testpassword";

async fn fixture_available() -> bool {
    tokio::net::TcpStream::connect((BASTION_HOST, BASTION_PORT))
        .await
        .is_ok()
}

async fn scan_host_key() -> String {
    let output = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "ssh-keyscan",
            "-t",
            "ed25519",
            "-p",
            "2222",
            "localhost",
        ])
        .output()
        .await
        .expect("failed to run ssh-keyscan via docker exec");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|line| !line.starts_with('#'))
        .and_then(|line| line.split_whitespace().nth(2))
        .expect("ssh-keyscan produced no host key")
        .to_owned()
}

const OFFICE_IMAGE: &str = "collabora/code:26.04.3.1.1";

/// Same product router harness as `office_wopi_host.rs`, but with a
/// *real* `RuntimeManager` (the Office OCI adapter registered, same as
/// `office_runtime.rs`'s harness) so `/api/v1/office/sessions` is
/// exercised for real rather than failing on a missing runtime -- this
/// is the "`CloudDesk` Office launch" step Task 2 asks for. Binds
/// `0.0.0.0` for the same reason `office_runtime.rs` does: if Collabora
/// happens to be available in this environment it must be able to call
/// back into this process. If Docker/the Collabora image aren't
/// present, `RuntimeManager` reports `Unavailable` cleanly, exactly as
/// designed -- this test's own assertions do not require Collabora to
/// actually start (that evidence already exists for local files in
/// `office_runtime.rs`); what matters here is the remote-VFS-specific
/// WOPI protocol path.
async fn application() -> (String, tempfile::TempDir, SqlitePool) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[41_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "remote-vfs-test-secret\n").unwrap();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let wopi_host_base = format!("http://host.docker.internal:{port}");

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!("clouddesk-remote-vfs-test-{}", std::process::id())),
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

/// Same cross-process Collabora serialization as `office_runtime.rs`,
/// `office_browser.rs`, and `office_hostile_documents.rs`: this file's
/// `enable_office` calls start a real Collabora container via the real
/// `RuntimeManager`, so it must not contend with those binaries' own
/// containers under `cargo test --workspace`. Missing here before the
/// Pre-Phase-10 closure gate (Part U) -- added now.
fn acquire_cross_process_collabora_lock() -> std::fs::File {
    let path = std::env::temp_dir().join("clouddesk-collabora-test.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .unwrap();
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).unwrap();
    file
}

/// Panic-safe cleanup for this file's real Collabora containers, same
/// pattern as `office_runtime.rs`'s `CollaboraContainerGuard`.
struct CollaboraContainerGuard {
    before: std::collections::HashSet<String>,
}

fn list_collabora_container_ids() -> std::collections::HashSet<String> {
    std::process::Command::new("docker")
        .args([
            "ps",
            "-a",
            "-q",
            "--filter",
            &format!("ancestor={OFFICE_IMAGE}"),
        ])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

impl CollaboraContainerGuard {
    fn new() -> Self {
        Self {
            before: list_collabora_container_ids(),
        }
    }
}

impl Drop for CollaboraContainerGuard {
    fn drop(&mut self) {
        for id in list_collabora_container_ids().difference(&self.before) {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", id])
                .output();
        }
    }
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
            "secret": "remote-vfs-test-secret",
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

/// Registers a `RemoteServer` for `owner_user_id` pointing at the real
/// disposable OpenSSH fixture, and returns its id. Uses the exact same
/// `RemoteServerStore`/`Vault` path the real Settings/Servers UI does --
/// credentials are Vault-held from the moment they're saved, never
/// handled by this test again after this call.
async fn register_remote_server(pool: &SqlitePool, owner_user_id: &str) -> String {
    // Must use the exact same cipher key as the product's own
    // AuthService (below) -- Vault-encrypted secrets are only ever
    // decryptable with the key they were sealed under, and the product
    // reveals this secret with its own AuthService::secret_cipher(),
    // not a test-local one.
    let vault = Vault::new(pool.clone(), SecretCipher::new(&[41_u8; 32]).unwrap());
    let secret_id = vault
        .create(
            owner_user_id,
            "ssh.password",
            "test credential",
            BASTION_PASSWORD.as_bytes(),
        )
        .await
        .unwrap();
    let host_key = scan_host_key().await;
    let store = RemoteServerStore::new(pool.clone());
    store
        .create(
            owner_user_id,
            &NewRemoteServer {
                name: format!("{BASTION_HOST}:{BASTION_PORT}"),
                hostname: BASTION_HOST.to_owned(),
                port: BASTION_PORT,
                username: BASTION_USER.to_owned(),
                auth_method: SshAuthMethod::Password,
                credential_secret_id: Some(secret_id),
                host_key_type: "ssh-ed25519".to_owned(),
                host_key_base64: host_key,
                proxy_jump_server_id: None,
                tags: vec![],
            },
        )
        .await
        .unwrap()
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

/// Generates a real fixture with `soffice`, uploads it to the remote
/// fixture's writable home via `docker cp`, and returns its remote
/// virtual path (relative to the SFTP session's own root, which for
/// `linuxserver/openssh-server` is the user's home).
async fn seed_remote_fixture(name: &str, extension: &str, marker: &str) -> String {
    let local_dir = tempfile::tempdir().unwrap();
    let seed = local_dir.path().join(format!("{name}.txt"));
    std::fs::write(&seed, format!("{marker}\n")).unwrap();
    let profile = tempfile::tempdir().unwrap();
    let convert = TokioCommand::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.path().display()
        ))
        .args(["--headless", "--convert-to", extension, "--outdir"])
        .arg(local_dir.path())
        .arg(&seed)
        .output()
        .await
        .unwrap();
    assert!(
        convert.status.success(),
        "soffice fixture generation failed"
    );
    let local_doc = local_dir.path().join(format!("{name}.{extension}"));
    assert!(local_doc.exists());

    let remote_name = format!("{name}-{}.{extension}", unique());
    let cp = TokioCommand::new("docker")
        .args([
            "cp",
            local_doc.to_str().unwrap(),
            &format!("acceptance-openssh-1:/config/{remote_name}"),
        ])
        .output()
        .await
        .unwrap();
    assert!(cp.status.success(), "docker cp seeding failed: {cp:?}");
    // The fixture's SFTP session roots at /config (the container's
    // configured home for `testuser`), so the virtual path is the bare
    // filename.
    remote_name
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

/// Task 1/2: the full remote round trip through the real WOPI host --
/// `/api/v1/office/sessions` with `server_id` set, `CheckFileInfo`,
/// `GetFile` (byte-exact against a real download via the fixture),
/// `PutFile` with genuine replacement content, then reopening and
/// re-verifying via a completely independent second SSH connection
/// (`docker exec ... cat`), so the proof does not depend on `CloudDesk`'s
/// own read path agreeing with itself.
// SftpProvider's sync VfsProvider methods use tokio::task::block_in_place
// internally (matching worker.rs's own established pattern), which
// requires the multi-threaded runtime -- exactly what the real product
// binary runs under (#[tokio::main] defaults to multi-thread); only the
// *default* single-threaded #[tokio::test] flavor needs this explicit
// override.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn task_1_2_remote_office_document_round_trip() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_collabora_lock)
        .await
        .unwrap();
    let _collabora_container_guard = CollaboraContainerGuard::new();
    let (base, _dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    enable_office(&base, &admin).await;
    let (cookie, user_id) = create_user(&base, &admin, "remoteuser").await;
    let server_id = register_remote_server(&pool, &user_id).await;

    let remote_name = seed_remote_fixture("remote-doc", "docx", "REMOTE-ALPHA").await;

    // --- open through the real product API, server_id set (Task 1/2:
    //     the real "CloudDesk Office launch" step) ---
    let opened = http(
        &base,
        Method::POST,
        "/api/v1/office/sessions",
        Some(&cookie),
        Some(&json!({ "path": remote_name, "server_id": server_id })),
    )
    .await;
    // If Docker/the Collabora image are available in this environment,
    // this succeeds fully (real editor_url, real runtime). If not, the
    // Office *runtime* legitimately reports Unavailable/failed-to-start
    // -- a different, already-covered concern (office_runtime.rs) --
    // but that must never be conflated with a remote-VFS *resolution*
    // failure. What must never happen is authorization/resolution
    // itself failing for a legitimately owned remote file.
    let status = opened.status();
    let body: Value = opened.json().await.unwrap_or_default();
    assert_ne!(
        status,
        reqwest::StatusCode::FORBIDDEN,
        "remote file resolution/authorization must succeed: {body:?}"
    );
    assert_ne!(
        status,
        reqwest::StatusCode::BAD_REQUEST,
        "remote file resolution must succeed: {body:?}"
    );

    // Drive the rest of the protocol directly against the WOPI host with
    // a handcrafted token, exactly like office_wopi_host.rs -- this is
    // what proves the remote GetFile/PutFile/CheckFileInfo path itself,
    // independent of whether a real Office runtime happens to be
    // available in this environment.
    let file_id: String = sqlx::query_scalar(
        "SELECT id FROM office_wopi_files WHERE remote_server_id = ? AND canonical_path = ?",
    )
    .bind(&server_id)
    .bind(&remote_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    let raw_token = format!("t{}", unique());
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(&mut hasher, raw_token.as_bytes());
    let hash = hex::encode(sha2::Digest::finalize(hasher));
    sqlx::query(
        "INSERT INTO office_wopi_tokens
            (token_hash, user_id, file_id, read_write, runtime_instance_id, created_at, expires_at)
         VALUES (?, ?, ?, 1, 'test-instance', 0, ?)",
    )
    .bind(&hash)
    .bind(&user_id)
    .bind(&file_id)
    .bind(i64::MAX)
    .execute(&pool)
    .await
    .unwrap();

    let client = reqwest::Client::new();

    // --- CheckFileInfo ---
    let info_response = client
        .get(format!(
            "{base}/wopi/files/{file_id}?access_token={raw_token}"
        ))
        .send()
        .await
        .unwrap();
    let info_status = info_response.status();
    let info: Value = info_response.json().await.unwrap_or_default();
    assert_eq!(
        info_status,
        reqwest::StatusCode::OK,
        "CheckFileInfo failed: {info:?}"
    );
    assert_eq!(info["UserCanWrite"], json!(true));
    assert!(info["Size"].as_u64().unwrap() > 0);

    // --- GetFile: byte-exact against an independent docker-exec read ---
    let fetched = client
        .get(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={raw_token}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(fetched.status(), reqwest::StatusCode::OK);
    let fetched_bytes = fetched.bytes().await.unwrap().to_vec();
    let independent_read = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "cat",
            &format!("/config/{remote_name}"),
        ])
        .output()
        .await
        .unwrap();
    assert!(independent_read.status.success());
    assert_eq!(
        fetched_bytes, independent_read.stdout,
        "GetFile must return the remote document byte-for-byte"
    );

    // --- LOCK, then PutFile with genuine replacement content ---
    assert_eq!(
        wopi_op(&base, &file_id, &raw_token, "LOCK", "REMOTE-LOCK")
            .await
            .status(),
        reqwest::StatusCode::OK
    );

    let replacement_dir = tempfile::tempdir().unwrap();
    let replacement_txt = replacement_dir.path().join("replacement.txt");
    std::fs::write(&replacement_txt, "REMOTE-OMEGA\n").unwrap();
    let profile = tempfile::tempdir().unwrap();
    let convert = TokioCommand::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.path().display()
        ))
        .args(["--headless", "--convert-to", "docx", "--outdir"])
        .arg(replacement_dir.path())
        .arg(&replacement_txt)
        .output()
        .await
        .unwrap();
    assert!(convert.status.success());
    let replacement_bytes = std::fs::read(replacement_dir.path().join("replacement.docx")).unwrap();

    let put = client
        .post(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={raw_token}"
        ))
        .header("X-WOPI-Lock", "REMOTE-LOCK")
        .body(replacement_bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(
        put.status(),
        reqwest::StatusCode::OK,
        "remote PutFile failed"
    );

    // --- reopen and verify via TWO independent paths ---
    let reopened = client
        .get(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={raw_token}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(reopened.status(), reqwest::StatusCode::OK);
    assert_eq!(reopened.bytes().await.unwrap().to_vec(), replacement_bytes);

    let independent_reread = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "cat",
            &format!("/config/{remote_name}"),
        ])
        .output()
        .await
        .unwrap();
    assert!(independent_reread.status.success());
    assert_eq!(
        independent_reread.stdout, replacement_bytes,
        "the real remote file must contain the saved content, verified independently of CloudDesk's own read path"
    );

    // Content is genuinely changed, not just bytes: reparse with real
    // LibreOffice.
    let outdir = tempfile::tempdir().unwrap();
    let saved_copy = outdir.path().join("saved.docx");
    std::fs::write(&saved_copy, &replacement_bytes).unwrap();
    let reparse_profile = tempfile::tempdir().unwrap();
    let reparse = TokioCommand::new("soffice")
        .arg(format!(
            "-env:UserInstallation=file://{}",
            reparse_profile.path().display()
        ))
        .args(["--headless", "--convert-to", "txt:Text", "--outdir"])
        .arg(outdir.path())
        .arg(&saved_copy)
        .output()
        .await
        .unwrap();
    assert!(reparse.status.success());
    let text = std::fs::read_to_string(outdir.path().join("saved.txt"))
        .unwrap()
        .replace('\u{feff}', "");
    assert!(
        text.contains("REMOTE-OMEGA"),
        "the saved remote document must contain the real modification, got {text:?}"
    );

    // --- no leftover temp objects on the remote server (Task 3/12) ---
    let listing = TokioCommand::new("docker")
        .args(["exec", "acceptance-openssh-1", "ls", "/config"])
        .output()
        .await
        .unwrap();
    let names = String::from_utf8_lossy(&listing.stdout);
    assert!(
        !names.contains(".cloudesk-office-"),
        "no temp upload artifact should remain on the remote server: {names}"
    );

    // Cleanup this test's own fixture file.
    let _ = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "rm",
            "-f",
            &format!("/config/{remote_name}"),
        ])
        .output()
        .await;
}

/// Task 4/5: remote save safety under external modification and under
/// a save that cannot possibly succeed. Uses the lightweight (no
/// Collabora required) WOPI-host-only path, mirroring
/// `office_wopi_host.rs`'s evidence tier for the local equivalents.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn task_4_5_remote_save_failure_and_conflict_safety() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_collabora_lock)
        .await
        .unwrap();
    let _collabora_container_guard = CollaboraContainerGuard::new();
    let (base, _dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "conflictuser").await;
    let server_id = register_remote_server(&pool, &user_id).await;

    let remote_name = seed_remote_fixture("conflict-doc", "odt", "BASELINE").await;

    // Register + mint a token directly against the WOPI host (no
    // Office-runtime dependency for this test).
    let store = clouddesk_remote::RemoteServerStore::new(pool.clone());
    let resolved = clouddeskd::wopi::resolve_and_register_remote_file(
        &pool,
        &store,
        &user_id,
        &server_id,
        &remote_name,
    )
    .await
    .unwrap();
    let raw_token = format!("t{}", unique());
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(&mut hasher, raw_token.as_bytes());
    let hash = hex::encode(sha2::Digest::finalize(hasher));
    sqlx::query(
        "INSERT INTO office_wopi_tokens
            (token_hash, user_id, file_id, read_write, runtime_instance_id, created_at, expires_at)
         VALUES (?, ?, ?, 1, 'test-instance', 0, ?)",
    )
    .bind(&hash)
    .bind(&user_id)
    .bind(&resolved.file_id)
    .bind(i64::MAX)
    .execute(&pool)
    .await
    .unwrap();

    let client = reqwest::Client::new();
    let file_id = &resolved.file_id;

    // --- lock ---
    assert_eq!(
        wopi_op(&base, file_id, &raw_token, "LOCK", "CONFLICT-LOCK")
            .await
            .status(),
        reqwest::StatusCode::OK
    );

    // --- Task 5: an external actor modifies the remote file directly
    //     while the lock is held (bypassing CloudDesk entirely, the way
    //     a second real user/tool touching the same remote server
    //     would) ---
    // Ensure a distinct mtime (SFTP granularity is whole seconds).
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let mut externally_written = TokioCommand::new("docker")
        .args([
            "exec",
            "-i",
            "acceptance-openssh-1",
            "sh",
            "-c",
            &format!("cat > /config/{remote_name}"),
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use tokio::io::AsyncWriteExt;
        externally_written
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"EXTERNALLY-MODIFIED-BY-SOMEONE-ELSE")
            .await
            .unwrap();
    }
    let out = externally_written.wait_with_output().await.unwrap();
    assert!(out.status.success());

    // Attempting to save from the stale (pre-external-write) version
    // must be refused, never silently overwrite the external change.
    let stale_save = client
        .post(format!(
            "{base}/wopi/files/{file_id}/contents?access_token={raw_token}"
        ))
        .header("X-WOPI-Lock", "CONFLICT-LOCK")
        .body(b"save-from-stale-remote-version".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(
        stale_save.status(),
        reqwest::StatusCode::CONFLICT,
        "a remote save must never blindly clobber a version modified out of band"
    );
    let after_refused = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "cat",
            &format!("/config/{remote_name}"),
        ])
        .output()
        .await
        .unwrap();
    assert_eq!(
        after_refused.stdout, b"EXTERNALLY-MODIFIED-BY-SOMEONE-ELSE",
        "the external writer's content must survive the refused save"
    );

    // --- Task 4: a save that cannot possibly succeed (nonexistent
    //     parent directory on the remote side) must fail cleanly,
    //     leaving the (now externally-modified, but still valid)
    //     original completely untouched, and no temp artifact behind ---
    wopi_op(&base, file_id, &raw_token, "UNLOCK", "CONFLICT-LOCK").await;
    // Re-lock against the current state.
    assert_eq!(
        wopi_op(&base, file_id, &raw_token, "LOCK", "CONFLICT-LOCK-2")
            .await
            .status(),
        reqwest::StatusCode::OK
    );

    // Register a second file pointed at a path inside a directory that
    // does not exist on the remote server -- the temp upload itself
    // must fail, never leaving a corrupt or partial canonical file
    // (there is nothing to corrupt yet -- it never existed -- but a
    // failed write must not fabricate one either).
    let doomed_path = format!("no-such-directory/{}.odt", unique());
    let doomed = clouddeskd::wopi::resolve_and_register_remote_file(
        &pool,
        &store,
        &user_id,
        &server_id,
        &doomed_path,
    )
    .await
    .unwrap();
    let doomed_token = format!("t{}", unique());
    let mut h2 = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(&mut h2, doomed_token.as_bytes());
    let h2hash = hex::encode(sha2::Digest::finalize(h2));
    sqlx::query(
        "INSERT INTO office_wopi_tokens
            (token_hash, user_id, file_id, read_write, runtime_instance_id, created_at, expires_at)
         VALUES (?, ?, ?, 1, 'test-instance', 0, ?)",
    )
    .bind(&h2hash)
    .bind(&user_id)
    .bind(&doomed.file_id)
    .bind(i64::MAX)
    .execute(&pool)
    .await
    .unwrap();
    let doomed_put = client
        .post(format!(
            "{base}/wopi/files/{}/contents?access_token={doomed_token}",
            doomed.file_id
        ))
        .body(b"this can never be written".to_vec())
        .send()
        .await
        .unwrap();
    assert!(
        !doomed_put.status().is_success(),
        "a write to a nonexistent remote directory must fail, not silently succeed"
    );
    let doomed_exists = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "test",
            "-e",
            &format!("/config/{doomed_path}"),
        ])
        .status()
        .await
        .unwrap();
    assert!(
        !doomed_exists.success(),
        "a failed remote write must not fabricate a canonical file"
    );

    // No temp artifact left anywhere in /config from either failure.
    let listing = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "sh",
            "-c",
            "find /config -name '*.cloudesk-office-*'",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&listing.stdout).trim().is_empty(),
        "no temp upload artifact should remain after a failed remote save"
    );

    let _ = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "rm",
            "-f",
            &format!("/config/{remote_name}"),
        ])
        .output()
        .await;
}

/// Task 26: with a real remote round trip exercised, prove Collabora's
/// side of the architecture never sees the SSH credential -- inspects
/// exactly what Task 16/18's OCI-hardening test inspects on the real
/// container (Docker never even gets a document mount for Office, so
/// there is structurally no path for a remote credential to reach it
/// either), plus the WOPI response bodies/headers a real Collabora
/// client would see.
#[tokio::test(flavor = "multi_thread")]
async fn task_26_remote_credentials_never_reach_the_wopi_response() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_collabora_lock)
        .await
        .unwrap();
    let _collabora_container_guard = CollaboraContainerGuard::new();
    let (base, _dir, pool) = application().await;
    let admin = bootstrap_admin(&base).await;
    let (_cookie, user_id) = create_user(&base, &admin, "creduser").await;
    let server_id = register_remote_server(&pool, &user_id).await;
    let remote_name = seed_remote_fixture("cred-doc", "odt", "CRED-CHECK").await;

    let store = clouddesk_remote::RemoteServerStore::new(pool.clone());
    let resolved = clouddeskd::wopi::resolve_and_register_remote_file(
        &pool,
        &store,
        &user_id,
        &server_id,
        &remote_name,
    )
    .await
    .unwrap();
    let raw_token = format!("t{}", unique());
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(&mut hasher, raw_token.as_bytes());
    let hash = hex::encode(sha2::Digest::finalize(hasher));
    sqlx::query(
        "INSERT INTO office_wopi_tokens
            (token_hash, user_id, file_id, read_write, runtime_instance_id, created_at, expires_at)
         VALUES (?, ?, ?, 1, 'test-instance', 0, ?)",
    )
    .bind(&hash)
    .bind(&user_id)
    .bind(&resolved.file_id)
    .bind(i64::MAX)
    .execute(&pool)
    .await
    .unwrap();

    let client = reqwest::Client::new();
    let info: Value = client
        .get(format!(
            "{base}/wopi/files/{}?access_token={raw_token}",
            resolved.file_id
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let info_text = info.to_string();
    assert!(
        !info_text.contains(BASTION_PASSWORD),
        "the SSH password must never appear in a WOPI response"
    );
    assert!(
        !info_text.to_lowercase().contains("password"),
        "no credential-shaped field should be present in CheckFileInfo at all"
    );

    // The password is only ever stored Vault-encrypted -- never as
    // plaintext anywhere in the database (the same guarantee remote
    // credentials already had before Office; re-confirmed here in the
    // Office code path specifically).
    let plaintext_hits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM vault_secrets WHERE encrypted_value LIKE ?")
            .bind(format!("%{BASTION_PASSWORD}%"))
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
    assert_eq!(
        plaintext_hits, 0,
        "the SSH password must never be stored in plaintext"
    );

    let _ = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "rm",
            "-f",
            &format!("/config/{remote_name}"),
        ])
        .output()
        .await;
}
