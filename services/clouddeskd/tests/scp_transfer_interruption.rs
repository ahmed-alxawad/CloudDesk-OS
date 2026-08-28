//! PASS SSH-B-2, Blocker 2: a genuine mid-transfer SCP upload
//! interruption -- not a permission-denied-before-transfer-starts
//! stand-in. Kills the real disposable OpenSSH bastion container after
//! real bytes have already moved (`bytes_transferred > 0 && <
//! bytes_total`, observed live through the real product/API), proving
//! the shared `TransferQueue`'s new bounded-retry/terminal-`Failed`
//! semantics (PASS SSH-B-2, Blocker 1) hold for a true transport
//! failure, not just a synthetic one.
//!
//! Holds the same cross-process fixture lock every other real-fixture
//! test in this project uses for the whole kill/restart cycle, and
//! restores the container before releasing it. This is real,
//! destructive infrastructure manipulation against a *disposable*
//! Docker fixture only -- never a production system.
//!
//! Skips (not FAIL) if the disposable fixture isn't running.

use axum::http::Method;
use serde_json::{json, Value};

const BASTION_HOST: &str = "127.0.0.1";
const BASTION_PORT: u16 = 2222;
const BASTION_USER: &str = "testuser";
const BASTION_PASSWORD: &str = "testpassword";
const BASTION_CONTAINER: &str = "acceptance-openssh-1";
const INTERRUPTION_TEST_SIZE: usize = 32 * 1024 * 1024;

async fn fixture_available() -> bool {
    tokio::net::TcpStream::connect((BASTION_HOST, BASTION_PORT))
        .await
        .is_ok()
}

fn acquire_cross_process_ssh_lock() -> std::fs::File {
    let path = std::env::temp_dir().join("clouddesk-browser-test.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .unwrap();
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).unwrap();
    file
}

async fn scan_host_key() -> String {
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            BASTION_CONTAINER,
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

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    Sha256::digest(data)
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

async fn application() -> (String, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = clouddesk_auth::AuthService::new(
        pool,
        clouddesk_secrets::SecretCipher::new(&[41_u8; 32]).unwrap(),
        clouddesk_auth::AuthPolicy::default(),
    )
    .unwrap();
    clouddeskd::worker::TransferWorker::new(&auth).spawn();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "scp-interruption-test-secret\n").unwrap();
    let router = clouddeskd::application_router(directory.path().to_owned(), auth, secret_path);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://127.0.0.1:{port}"), directory)
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

fn current_process_linux_identity() -> Option<clouddesk_linux::LinuxIdentity> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        return None;
    }
    clouddesk_linux::lookup_uid(uid).ok().flatten()
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

async fn step_up(base: &str, cookie: &str, password: &str) {
    let response = http(
        base,
        Method::POST,
        "/api/v1/auth/step-up",
        Some(cookie),
        Some(&json!({"password": password})),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

async fn bootstrap_admin(base: &str) -> String {
    let linux_username = current_process_linux_identity().map(|i| i.username);
    let response = http(
        base,
        Method::POST,
        "/api/v1/setup/bootstrap",
        None,
        Some(&json!({
            "secret": "scp-interruption-test-secret",
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
            "linux_username": linux_username,
        })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let cookie = login(base, "admin", "correct horse battery staple").await;
    step_up(base, &cookie, "correct horse battery staple").await;
    cookie
}

async fn create_secret(base: &str, cookie: &str, kind: &str, value: &str) -> String {
    let response = http(
        base,
        Method::POST,
        "/api/v1/vault/secrets",
        Some(cookie),
        Some(&json!({"kind": kind, "label": "test", "value": value})),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    response.json::<Value>().await.unwrap()["secret_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    (u64::from(std::process::id()) << 20) + COUNTER.fetch_add(1, Ordering::Relaxed)
}

async fn create_remote_server(base: &str, cookie: &str, secret_id: &str, host_key: &str) -> String {
    let response = http(
        base,
        Method::POST,
        "/api/v1/remote/servers",
        Some(cookie),
        Some(&json!({
            "name": format!("scp-interrupt-{}", rand_suffix()),
            "hostname": BASTION_HOST,
            "port": BASTION_PORT,
            "username": BASTION_USER,
            "auth_method": "password",
            "credential_secret_id": secret_id,
            "agent_socket_path": null,
            "host_key_type": "ssh-ed25519",
            "host_key_base64": host_key,
            "proxy_jump_server_id": null,
            "tags": [],
        })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    response.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn get_transfer(base: &str, cookie: &str, id: &str) -> Value {
    http(
        base,
        Method::GET,
        &format!("/api/v1/transfers/{id}"),
        Some(cookie),
        None,
    )
    .await
    .json::<Value>()
    .await
    .unwrap()
}

async fn wait_for_container_reachable() {
    for _ in 0..100 {
        if fixture_available().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    panic!("bastion container did not become reachable again after restart");
}

/// Task 7/8/9/10/11/12/13: a real mid-transfer SCP upload kill.
///
/// Sequence: pre-create a legitimate file at the canonical remote
/// destination (Task 11) -> start a real product/API SCP upload of a
/// moderately large file -> poll until real bytes have moved
/// (0 < `bytes_transferred` < total) -> `docker kill` the bastion
/// (Task 8) -> confirm the transfer never reports `completed` and
/// eventually reaches `failed` once the bounded retry budget is
/// exhausted, with no infinite retrying (Task 9/13) -> confirm the
/// pre-existing destination content is byte-for-byte untouched
/// (Task 10/11: `CloudDesk` uploads to a disposable remote temp name and
/// only renames into the canonical destination after full success, so
/// an interrupted upload never touches it) -> restart the container,
/// issue an authorized manual retry (Task 5/12), and confirm it
/// completes with the correct hash and the canonical destination is
/// now the newly uploaded content.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn task_7_8_9_10_11_12_13_real_mid_transfer_scp_upload_interruption() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_7_8_9_10_11_12_13_real_mid_transfer_scp_upload_interruption",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    if current_process_linux_identity().is_none() {
        clouddesk_test_support::blocked_by_environment(
            "task_7_8_9_10_11_12_13_real_mid_transfer_scp_upload_interruption",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();

    // Task 22: the real per-operation timeout defaults to 30s
    // (matching the SSH connection's own inactivity timeout), which
    // would make proving real dead-connection detection take several
    // minutes end to end (six bounded retry attempts, each waiting out
    // the full timeout). Reduced here for this test only via a safe
    // atomic setter (never an environment variable or `unsafe` global
    // -- this workspace forbids `unsafe` entirely) -- production is
    // unaffected.
    clouddesk_remote::scp::set_operation_timeout_for_test(2);

    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let identity = current_process_linux_identity().unwrap();
    let workspace = tempfile::tempdir_in(&identity.home).unwrap();
    let host_key = scan_host_key().await;
    let secret_id = create_secret(&base, &admin, "ssh.password", BASTION_PASSWORD).await;
    let server_id = create_remote_server(&base, &admin, &secret_id, &host_key).await;

    // A large enough payload (32 MiB) that a real localhost/Docker
    // upload takes long enough to reliably observe partial progress
    // before killing the container, without creating an unnecessarily
    // huge fixture file.
    let size: usize = INTERRUPTION_TEST_SIZE;
    let mut payload = vec![0_u8; size];
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte = u8::try_from(i % 250).unwrap_or(0);
    }
    let source_path = workspace.path().join("interrupt-source.bin");
    tokio::fs::write(&source_path, &payload).await.unwrap();
    let relative = workspace.path().strip_prefix(&identity.home).unwrap();
    let source_relative = format!("{}/interrupt-source.bin", relative.display());

    let remote_path = format!("/config/scp-interrupt-{}.bin", std::process::id());

    // Task 11: a legitimate pre-existing file at the exact canonical
    // destination -- must survive the interrupted upload untouched.
    let known_good = b"this is the real, pre-existing, legitimate file content";
    let mut place = tokio::process::Command::new("docker")
        .args([
            "exec",
            "-i",
            BASTION_CONTAINER,
            "sh",
            "-c",
            &format!("cat > {remote_path} && chown testuser:testuser {remote_path}"),
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use tokio::io::AsyncWriteExt;
        place
            .stdin
            .as_mut()
            .unwrap()
            .write_all(known_good)
            .await
            .unwrap();
    }
    assert!(place.wait_with_output().await.unwrap().status.success());

    let create = http(
        &base,
        Method::POST,
        "/api/v1/transfers",
        Some(&admin),
        Some(&json!({
            "source": { "provider": "local", "path": source_relative },
            "destination": { "provider": "scp", "server_id": server_id, "path": remote_path },
            "bytes_total": size,
        })),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::CREATED);
    let transfer_id = create.json::<Value>().await.unwrap()["transfer_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Task 8: poll until real bytes have moved but the transfer is not
    // yet complete.
    let mut observed_partial: u64 = 0;
    for _ in 0..500 {
        let body = get_transfer(&base, &admin, &transfer_id).await;
        let bytes = body["transfer"]["bytes_transferred"].as_u64().unwrap_or(0);
        let state = body["transfer"]["state"].as_str().unwrap_or("");
        assert_ne!(
            state, "completed",
            "must not reach completed before the interruption: {body:?}"
        );
        if bytes > 0 && bytes < size as u64 {
            observed_partial = bytes;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        observed_partial > 0,
        "never observed real mid-transfer progress (0 < bytes_transferred < total) before timing out"
    );
    eprintln!(
        "task_8: observed {observed_partial} of {size} bytes transferred before interruption"
    );

    // Task 8: the real interruption -- SIGKILL the bastion container.
    let kill = tokio::process::Command::new("docker")
        .args(["kill", BASTION_CONTAINER])
        .status()
        .await
        .unwrap();
    assert!(kill.success(), "docker kill must succeed");

    // Task 9/13: wait for the bounded retry budget to exhaust and the
    // job to reach a genuine terminal Failed state -- never
    // completed, never infinite retrying.
    let mut final_body = Value::Null;
    let mut reached_failed = false;
    for _ in 0..300 {
        let body = get_transfer(&base, &admin, &transfer_id).await;
        let state = body["transfer"]["state"].as_str().unwrap_or("");
        assert_ne!(
            state, "completed",
            "an interrupted transfer must never report completed: {body:?}"
        );
        if state == "failed" {
            final_body = body;
            reached_failed = true;
            break;
        }
        final_body = body;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        reached_failed,
        "the interrupted transfer must eventually reach a terminal failed state, not retry forever: {final_body:?}"
    );
    eprintln!(
        "task_9: interrupted upload reached failed after {} attempts, last_error={:?}",
        final_body["transfer"]["attempts"], final_body["transfer"]["last_error"]
    );

    // Task 10/11: restart the container, then verify the canonical
    // destination still holds the ORIGINAL known-good content --
    // never corrupted by the interrupted upload, because CloudDesk
    // uploaded to a disposable remote temp name and only renames into
    // the canonical destination on full success (never reached here).
    let restart = tokio::process::Command::new("docker")
        .args(["start", BASTION_CONTAINER])
        .status()
        .await
        .unwrap();
    assert!(restart.success(), "docker start must succeed");
    wait_for_container_reachable().await;

    let canonical_after_interruption = tokio::process::Command::new("docker")
        .args(["exec", BASTION_CONTAINER, "cat", &remote_path])
        .output()
        .await
        .unwrap();
    assert_eq!(
        canonical_after_interruption.stdout, known_good,
        "Task 10/11: the pre-existing canonical destination must survive an interrupted upload byte-for-byte -- CloudDesk's remote-temp-name-then-rename design must have prevented any partial write from ever reaching it"
    );

    // Best-effort visibility into the actual partial-file policy
    // (Task 10): CloudDesk's own temp file may or may not still be on
    // the remote host depending on whether its cleanup `rm` succeeded
    // before the kill; documented, not asserted either way.
    let leftover_temp = tokio::process::Command::new("docker")
        .args([
            "exec",
            BASTION_CONTAINER,
            "sh",
            "-c",
            &format!("ls {remote_path}.clouddesk-upload-*.part 2>/dev/null || true"),
        ])
        .output()
        .await
        .unwrap();
    eprintln!(
        "task_10: remote temp-file leftovers after interruption+restart: {:?}",
        String::from_utf8_lossy(&leftover_temp.stdout)
    );
    let _ = tokio::process::Command::new("docker")
        .args([
            "exec",
            BASTION_CONTAINER,
            "sh",
            "-c",
            &format!("rm -f {remote_path}.clouddesk-upload-*.part"),
        ])
        .output()
        .await;

    // Task 5/12/15: an authorized manual retry, after the fixture is
    // restored, completes successfully with the correct hash --
    // restarting the upload from scratch (classic SCP has no
    // byte-range resume primitive; documented, not claimed otherwise).
    let retry = http(
        &base,
        Method::POST,
        &format!("/api/v1/transfers/{transfer_id}/retry"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(retry.status(), reqwest::StatusCode::NO_CONTENT);

    let mut completed_body = Value::Null;
    let mut reached_completed = false;
    for _ in 0..300 {
        let body = get_transfer(&base, &admin, &transfer_id).await;
        let state = body["transfer"]["state"].as_str().unwrap_or("");
        if state == "completed" {
            completed_body = body;
            reached_completed = true;
            break;
        }
        completed_body = body;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(
        reached_completed,
        "the manually-retried transfer must complete after the fixture is restored: {completed_body:?}"
    );

    let final_remote = tokio::process::Command::new("docker")
        .args(["exec", BASTION_CONTAINER, "sha256sum", &remote_path])
        .output()
        .await
        .unwrap();
    let final_hash = String::from_utf8_lossy(&final_remote.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_owned();
    assert_eq!(
        final_hash,
        sha256_hex(&payload),
        "after the successful retry, the canonical destination must be the newly uploaded content"
    );

    let _ = tokio::process::Command::new("docker")
        .args(["exec", BASTION_CONTAINER, "rm", "-f", &remote_path])
        .output()
        .await;
}

/// Task 17: real mid-download interruption regression -- the download
/// side already used a temp-local-file-then-atomic-rename design
/// since PASS SSH-B; this reconfirms it still holds after PASS
/// SSH-B-2's `TransferQueue` changes (bounded retry, terminal Failed).
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn task_17_real_mid_transfer_scp_download_interruption() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_17_real_mid_transfer_scp_download_interruption",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    if current_process_linux_identity().is_none() {
        clouddesk_test_support::blocked_by_environment(
            "task_17_real_mid_transfer_scp_download_interruption",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    clouddesk_remote::scp::set_operation_timeout_for_test(2);

    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let identity = current_process_linux_identity().unwrap();
    let workspace = tempfile::tempdir_in(&identity.home).unwrap();
    let host_key = scan_host_key().await;
    let secret_id = create_secret(&base, &admin, "ssh.password", BASTION_PASSWORD).await;
    let server_id = create_remote_server(&base, &admin, &secret_id, &host_key).await;

    let size: usize = INTERRUPTION_TEST_SIZE;
    let mut payload = vec![0_u8; size];
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte = u8::try_from(i % 250).unwrap_or(0);
    }
    let remote_path = format!("/config/scp-dl-interrupt-{}.bin", std::process::id());
    let mut place = tokio::process::Command::new("docker")
        .args([
            "exec",
            "-i",
            BASTION_CONTAINER,
            "sh",
            "-c",
            &format!("cat > {remote_path} && chown testuser:testuser {remote_path}"),
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use tokio::io::AsyncWriteExt;
        place
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&payload)
            .await
            .unwrap();
    }
    assert!(place.wait_with_output().await.unwrap().status.success());

    // A legitimate pre-existing local destination that must survive
    // the interrupted download untouched.
    let dest_relative_path = "download-interrupt-dest.bin";
    let dest_full_path = workspace.path().join(dest_relative_path);
    let known_good = b"pre-existing legitimate local content";
    tokio::fs::write(&dest_full_path, known_good).await.unwrap();
    let relative = workspace.path().strip_prefix(&identity.home).unwrap();
    let dest_relative = format!("{}/{dest_relative_path}", relative.display());

    let create = http(
        &base,
        Method::POST,
        "/api/v1/transfers",
        Some(&admin),
        Some(&json!({
            "source": { "provider": "scp", "server_id": server_id, "path": remote_path },
            "destination": { "provider": "local", "path": dest_relative },
            "bytes_total": size,
        })),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::CREATED);
    let transfer_id = create.json::<Value>().await.unwrap()["transfer_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut observed_partial: u64 = 0;
    for _ in 0..500 {
        let body = get_transfer(&base, &admin, &transfer_id).await;
        let bytes = body["transfer"]["bytes_transferred"].as_u64().unwrap_or(0);
        assert_ne!(body["transfer"]["state"], "completed");
        if bytes > 0 && bytes < size as u64 {
            observed_partial = bytes;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        observed_partial > 0,
        "never observed real mid-download progress"
    );
    eprintln!("task_17: observed {observed_partial} of {size} bytes before interruption");

    let kill = tokio::process::Command::new("docker")
        .args(["kill", BASTION_CONTAINER])
        .status()
        .await
        .unwrap();
    assert!(kill.success());

    let mut reached_failed = false;
    let mut final_body = Value::Null;
    for _ in 0..300 {
        let body = get_transfer(&base, &admin, &transfer_id).await;
        let state = body["transfer"]["state"].as_str().unwrap_or("");
        assert_ne!(
            state, "completed",
            "interrupted download must never report completed"
        );
        if state == "failed" {
            final_body = body;
            reached_failed = true;
            break;
        }
        final_body = body;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        reached_failed,
        "interrupted download must eventually fail: {final_body:?}"
    );

    let restart = tokio::process::Command::new("docker")
        .args(["start", BASTION_CONTAINER])
        .status()
        .await
        .unwrap();
    assert!(restart.success());
    wait_for_container_reachable().await;

    // Canonical local destination must be untouched -- the download
    // path writes to a local temp file first and only renames on
    // success.
    let preserved = tokio::fs::read(&dest_full_path).await.unwrap();
    assert_eq!(
        preserved, known_good,
        "Task 17: a pre-existing local destination must survive an interrupted download byte-for-byte"
    );
    let temp_path = dest_full_path.with_extension("scp-download.part");
    assert!(
        !temp_path.exists(),
        "the local temp file must be cleaned up after a failed download"
    );

    let _ = tokio::process::Command::new("docker")
        .args(["exec", BASTION_CONTAINER, "rm", "-f", &remote_path])
        .output()
        .await;
}
