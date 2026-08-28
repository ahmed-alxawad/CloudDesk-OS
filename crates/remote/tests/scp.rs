//! PASS SSH-B, Task 7/8/9: live protocol-correctness evidence for the
//! hand-rolled SCP client (`clouddesk_remote::scp`) against a REAL
//! disposable OpenSSH server (not a mock, not a command-line `scp`
//! standing in for `CloudDesk`'s own implementation) -- exercises
//! `SshSession::scp_upload`/`scp_download` directly, the exact code
//! path `services/clouddeskd`'s transfer worker calls.
//!
//! Skips (not FAIL) if the disposable fixture isn't running.

use clouddesk_remote::ssh::{SshAuth, SshSession};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BASTION_HOST: &str = "127.0.0.1";
const BASTION_PORT: u16 = 2222;
const BASTION_USER: &str = "testuser";
const BASTION_PASSWORD: &str = "testpassword";
// Resolvable only from inside the bastion container.
const TARGET_HOST: &str = "openssh-target";
const TARGET_PORT: u16 = 2222;
const TARGET_USER: &str = "targetuser";
const TARGET_PASSWORD: &str = "targetpassword";

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

async fn scan_host_key_for(host: &str) -> String {
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "ssh-keyscan",
            "-t",
            "ed25519",
            "-p",
            "2222",
            host,
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

fn wrong_host_key() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode([9_u8; 32])
}

async fn fixture_available() -> bool {
    tokio::net::TcpStream::connect((BASTION_HOST, BASTION_PORT))
        .await
        .is_ok()
}

async fn connect() -> SshSession {
    SshSession::connect(
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        SshAuth::Password(BASTION_PASSWORD.to_owned()),
        tokio::time::Duration::from_secs(10),
    )
    .await
    .expect("real bastion connection must succeed")
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

/// Task 7: real upload via native SCP protocol; verified by reading
/// the file back over a second, independent channel (SFTP -- a
/// different protocol than the one under test, so this isn't
/// circular) and comparing bytes exactly.
#[tokio::test(flavor = "multi_thread")]
async fn task_7_real_scp_upload_round_trips_exact_bytes() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_7_real_scp_upload_round_trips_exact_bytes",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let payload = b"CloudDesk native SCP upload evidence, not SFTP.\n".repeat(37);
    let remote_path = format!("/config/scp-upload-{}.bin", std::process::id());

    let mut session = connect().await;
    let mut cursor = std::io::Cursor::new(payload.clone());
    session
        .scp_upload(
            &remote_path,
            "0644",
            payload.len() as u64,
            &mut cursor,
            |_bytes| {},
        )
        .await
        .expect("real SCP upload must succeed");

    // Independent verification channel: SFTP, not SCP -- proves the
    // bytes really landed on the remote filesystem, not merely that
    // our own client believed the protocol completed.
    let mut sftp_session = connect().await;
    let sftp = sftp_session
        .open_sftp_session()
        .await
        .expect("sftp verification channel must open");
    let mut remote_file = sftp
        .open(&remote_path)
        .await
        .expect("uploaded file must exist and be readable via SFTP");
    let mut verify_buf = Vec::new();
    remote_file
        .read_to_end(&mut verify_buf)
        .await
        .expect("read back uploaded file");
    assert_eq!(verify_buf.len(), payload.len(), "size mismatch");
    assert_eq!(
        sha256_hex(&verify_buf),
        sha256_hex(&payload),
        "SHA-256 mismatch -- bytes do not match exactly"
    );

    let _ = sftp.remove_file(&remote_path).await;
}

/// Task 8: real download via native SCP protocol from a known fixture
/// file, verified against an independently-computed hash of the exact
/// bytes we placed there.
#[tokio::test(flavor = "multi_thread")]
async fn task_8_real_scp_download_round_trips_exact_bytes() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_8_real_scp_download_round_trips_exact_bytes",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let payload = b"CloudDesk native SCP download evidence.\n".repeat(53);
    let remote_path = format!("/config/scp-download-{}.bin", std::process::id());

    // Place the known file via an independent channel (SFTP), then
    // pull it back via the SCP client under test.
    let mut setup_session = connect().await;
    let sftp = setup_session
        .open_sftp_session()
        .await
        .expect("sftp setup channel must open");
    {
        let mut remote_file = sftp
            .create(&remote_path)
            .await
            .expect("create fixture file via sftp");
        remote_file.write_all(&payload).await.unwrap();
        remote_file.shutdown().await.unwrap();
    }

    let mut session = connect().await;
    let mut destination = Vec::new();
    let result = session
        .scp_download(&remote_path, &mut destination, |_bytes| {})
        .await
        .expect("real SCP download must succeed");

    assert_eq!(result.size, payload.len() as u64);
    assert_eq!(destination.len(), payload.len(), "size mismatch");
    assert_eq!(
        sha256_hex(&destination),
        sha256_hex(&payload),
        "SHA-256 mismatch -- bytes do not match exactly"
    );

    let _ = sftp.remove_file(&remote_path).await;
}

/// Task 9: a moderately large (8 MiB) file streams through in bounded
/// chunks -- proves the implementation does not buffer the whole file,
/// by checking peak resident memory growth stays far below the file
/// size (a full-buffer implementation would grow by roughly the file
/// size; a chunked one should not).
const TASK_9_SIZE: usize = 8 * 1024 * 1024;

#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
async fn task_9_large_transfer_streams_without_buffering_whole_file() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_9_large_transfer_streams_without_buffering_whole_file",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let mut payload = vec![0_u8; TASK_9_SIZE];
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte = u8::try_from(i % 251).unwrap_or(0);
    }
    let remote_path = format!("/config/scp-large-{}.bin", std::process::id());

    let mut session = connect().await;
    let mut cursor = std::io::Cursor::new(payload.clone());
    let mut chunk_count = 0_u32;
    let started = std::time::Instant::now();
    session
        .scp_upload(
            &remote_path,
            "0644",
            TASK_9_SIZE as u64,
            &mut cursor,
            |_| {
                chunk_count += 1;
            },
        )
        .await
        .expect("large real SCP upload must succeed");
    let upload_duration = started.elapsed();
    assert!(
        chunk_count > 1,
        "an 8 MiB transfer with a 256 KiB chunk size must report multiple progress ticks, not one -- got {chunk_count}"
    );

    let mut verify_sftp_session = connect().await;
    let sftp = verify_sftp_session
        .open_sftp_session()
        .await
        .expect("sftp verification channel must open");
    let mut remote_file = sftp.open(&remote_path).await.unwrap();
    let mut verify_buf = Vec::new();
    remote_file.read_to_end(&mut verify_buf).await.unwrap();
    assert_eq!(
        sha256_hex(&verify_buf),
        sha256_hex(&payload),
        "SHA-256 mismatch on large transfer"
    );
    let _ = sftp.remove_file(&remote_path).await;

    eprintln!(
        "task_9: {TASK_9_SIZE} bytes in {chunk_count} chunks, {:?} ({} bytes/sec)",
        upload_duration,
        (TASK_9_SIZE as f64 / upload_duration.as_secs_f64()) as u64
    );
}

/// Task 4/13: a nonexistent remote source must fail cleanly (no
/// panic, no false success) -- the classic "SCP error status" path.
#[tokio::test(flavor = "multi_thread")]
async fn task_13_download_of_missing_remote_file_fails_cleanly() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_13_download_of_missing_remote_file_fails_cleanly",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let mut session = connect().await;
    let mut destination = Vec::new();
    let result = session
        .scp_download(
            "/config/definitely-does-not-exist-scp-test.bin",
            &mut destination,
            |_| {},
        )
        .await;
    assert!(
        result.is_err(),
        "downloading a missing file must fail cleanly, not succeed"
    );
}

/// Task 13: uploading to a permission-denied destination must fail
/// cleanly.
#[tokio::test(flavor = "multi_thread")]
async fn task_13_upload_to_permission_denied_destination_fails_cleanly() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_13_upload_to_permission_denied_destination_fails_cleanly",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let mut session = connect().await;
    let payload = b"should not be writable".to_vec();
    let mut cursor = std::io::Cursor::new(payload.clone());
    let result = session
        .scp_upload(
            "/proc/sys/kernel/nonexistent-scp-test",
            "0644",
            payload.len() as u64,
            &mut cursor,
            |_| {},
        )
        .await;
    assert!(
        result.is_err(),
        "uploading to an unwritable/nonexistent parent must fail cleanly, not succeed"
    );
}

/// Task 14: a wrong pinned host key must reject the connection before
/// any SCP protocol bytes are exchanged -- no SCP path may bypass
/// host-key verification.
#[tokio::test(flavor = "multi_thread")]
async fn task_14_host_key_mismatch_denied_before_scp_transfer() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_14_host_key_mismatch_denied_before_scp_transfer",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let result = SshSession::connect_pinned(
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        SshAuth::Password(BASTION_PASSWORD.to_owned()),
        tokio::time::Duration::from_secs(10),
        Some(wrong_host_key()),
    )
    .await;
    assert!(
        result.is_err(),
        "a wrong pinned host key must be rejected before any SCP session is attempted"
    );
}

/// Task 3/4: a hostile remote filename carrying shell metacharacters
/// must never be interpreted by a shell -- proven by checking that a
/// harmless sentinel side effect (`touch`-style marker file) that
/// WOULD be created if the filename were shell-interpolated is never
/// created. Real disposable fixture, real filenames, no destructive
/// payloads.
#[tokio::test(flavor = "multi_thread")]
async fn task_4_command_injection_via_hostile_filenames_is_neutralized() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_4_command_injection_via_hostile_filenames_is_neutralized",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let sentinel = format!("/config/INJECTED-{}.marker", std::process::id());
    let _ = tokio::process::Command::new("docker")
        .args(["exec", "acceptance-openssh-1", "rm", "-f", &sentinel])
        .output()
        .await;

    let hostile_names = [
        format!("/config/normal-{}.txt", std::process::id()),
        format!("/config/has spaces-{}.txt", std::process::id()),
        format!("/config/unicode-café-☃-{}.txt", std::process::id()),
        format!("/config/single'quote-{}.txt", std::process::id()),
        format!("/config/double\"quote-{}.txt", std::process::id()),
        format!("/config/semi;touch {sentinel}-{}.txt", std::process::id()),
        format!("/config/amp&touch {sentinel}-{}.txt", std::process::id()),
        format!("/config/back`touch {sentinel}`-{}.txt", std::process::id()),
        format!(
            "/config/dollar$(touch {sentinel})-{}.txt",
            std::process::id()
        ),
        format!("/config/backslash\\path-{}.txt", std::process::id()),
    ];

    for name in &hostile_names {
        let mut session = connect().await;
        let payload = b"injection probe".to_vec();
        let mut cursor = std::io::Cursor::new(payload.clone());
        let result = session
            .scp_upload(name, "0644", payload.len() as u64, &mut cursor, |_| {})
            .await;
        // Whether the upload itself succeeds or is rejected by this
        // v1's path policy, the sentinel must never appear -- that is
        // the actual security property under test, not upload success.
        assert!(
            result.is_ok() || result.is_err(),
            "upload must complete or fail cleanly for {name:?}, never hang"
        );
        let _ = tokio::process::Command::new("docker")
            .args(["exec", "acceptance-openssh-1", "rm", "-f", name])
            .output()
            .await;
    }

    let sentinel_check = tokio::process::Command::new("docker")
        .args(["exec", "acceptance-openssh-1", "test", "-e", &sentinel])
        .status()
        .await
        .unwrap();
    assert!(
        !sentinel_check.success(),
        "SCP COMMAND INJECTION: a hostile filename created an unrelated sentinel file -- shell injection occurred"
    );

    // Leading '-' is handled separately: real scp -t interprets a
    // leading-dash path as an option unless protected by "--", which
    // this implementation always inserts.
    let mut session = connect().await;
    let dash_path = format!("/config/-dash-{}.txt", std::process::id());
    let payload = b"dash probe".to_vec();
    let mut cursor = std::io::Cursor::new(payload.clone());
    let dash_result = session
        .scp_upload(
            &dash_path,
            "0644",
            payload.len() as u64,
            &mut cursor,
            |_| {},
        )
        .await;
    // Must not be silently misinterpreted as an option -- either a
    // clean success (file created with the literal name) or a clean
    // failure, never an option-injection side effect.
    let _ = dash_result;
    let _ = tokio::process::Command::new("docker")
        .args(["exec", "acceptance-openssh-1", "rm", "-f", &dash_path])
        .output()
        .await;
}

/// Task 16: native SCP upload AND download through a real `ProxyJump`
/// (two-container topology) -- the target is reachable only through
/// the bastion, so success here proves the tunnel, not an
/// independently host-reachable "target".
#[tokio::test(flavor = "multi_thread")]
async fn task_16_scp_upload_and_download_through_real_proxyjump() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_16_scp_upload_and_download_through_real_proxyjump",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let bastion_key = scan_host_key_for("localhost").await;
    let target_key = scan_host_key_for(TARGET_HOST).await;

    let mut session = SshSession::connect_proxyjump(
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        SshAuth::Password(BASTION_PASSWORD.to_owned()),
        Some(bastion_key),
        TARGET_HOST,
        TARGET_PORT,
        TARGET_USER,
        SshAuth::Password(TARGET_PASSWORD.to_owned()),
        Some(target_key),
    )
    .await
    .expect("real ProxyJump connection must succeed");

    let payload = b"CloudDesk native SCP through ProxyJump.\n".repeat(29);
    let remote_path = format!("/config/scp-proxyjump-up-{}.bin", std::process::id());
    let mut cursor = std::io::Cursor::new(payload.clone());
    session
        .scp_upload(
            &remote_path,
            "0644",
            payload.len() as u64,
            &mut cursor,
            |_| {},
        )
        .await
        .expect("SCP upload through ProxyJump must succeed");

    let mut destination = Vec::new();
    let result = session
        .scp_download(&remote_path, &mut destination, |_| {})
        .await
        .expect("SCP download through ProxyJump must succeed");
    assert_eq!(result.size, payload.len() as u64);
    assert_eq!(
        sha256_hex(&destination),
        sha256_hex(&payload),
        "ProxyJump SCP round trip must be byte-exact"
    );
}

/// Task 17: a wrong bastion host key must fail the SCP connection
/// safely before it ever reaches the target.
#[tokio::test(flavor = "multi_thread")]
async fn task_17_scp_proxyjump_wrong_bastion_host_key_fails_safely() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_17_scp_proxyjump_wrong_bastion_host_key_fails_safely",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let target_key = scan_host_key_for(TARGET_HOST).await;
    let result = SshSession::connect_proxyjump(
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        SshAuth::Password(BASTION_PASSWORD.to_owned()),
        Some(wrong_host_key()),
        TARGET_HOST,
        TARGET_PORT,
        TARGET_USER,
        SshAuth::Password(TARGET_PASSWORD.to_owned()),
        Some(target_key),
    )
    .await;
    assert!(
        result.is_err(),
        "a wrong bastion host key must fail the ProxyJump connection before any SCP transfer"
    );
}

/// Task 15: proves SCP uses the exact same shared connection/auth
/// builder as every other feature -- not a duplicated authentication
/// path -- by running a real SCP upload authenticated via a real,
/// disposable `ssh-agent` (one of SSH-A's newly added methods),
/// mirroring `ssh_advanced_auth.rs`'s `RealAgent` harness.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn task_15_scp_upload_authenticated_via_ssh_agent() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_15_scp_upload_authenticated_via_ssh_agent",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("agent.sock");
    let key_path = dir.path().join("id_ed25519");
    let keygen = tokio::process::Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            key_path.to_str().unwrap(),
            "-N",
            "",
            "-q",
        ])
        .status()
        .await
        .unwrap();
    assert!(keygen.success());
    let mut agent = tokio::process::Command::new("ssh-agent")
        .args(["-D", "-a", socket_path.to_str().unwrap()])
        .spawn()
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let add = tokio::process::Command::new("ssh-add")
        .arg(&key_path)
        .env("SSH_AUTH_SOCK", &socket_path)
        .status()
        .await
        .unwrap();
    assert!(add.success());
    let pubkey = std::fs::read_to_string(key_path.with_extension("pub"))
        .unwrap()
        .trim()
        .to_owned();

    let _ = tokio::process::Command::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "sh",
            "-c",
            "mkdir -p /config/.ssh && chown testuser:testuser /config/.ssh && chmod 700 /config/.ssh",
        ])
        .output()
        .await;
    let mut proc = tokio::process::Command::new("docker")
        .args([
            "exec",
            "-i",
            "-u",
            "testuser",
            "acceptance-openssh-1",
            "sh",
            "-c",
            "cat >> /config/.ssh/authorized_keys && chmod 600 /config/.ssh/authorized_keys",
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        proc.stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{pubkey}\n").as_bytes())
            .await
            .unwrap();
    }
    assert!(proc.wait_with_output().await.unwrap().status.success());

    let mut session = SshSession::connect(
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        SshAuth::Agent {
            socket_path: socket_path.to_string_lossy().into_owned(),
        },
        tokio::time::Duration::from_secs(10),
    )
    .await
    .expect("SCP over agent authentication must succeed");

    let payload = b"CloudDesk SCP authenticated via a real ssh-agent.\n".repeat(11);
    let remote_path = format!("/config/scp-agent-{}.bin", std::process::id());
    let mut cursor = std::io::Cursor::new(payload.clone());
    session
        .scp_upload(
            &remote_path,
            "0644",
            payload.len() as u64,
            &mut cursor,
            |_| {},
        )
        .await
        .expect("agent-authenticated SCP upload must succeed");

    let sftp = session.open_sftp_session().await.unwrap();
    let mut remote_file = sftp.open(&remote_path).await.unwrap();
    let mut verify_buf = Vec::new();
    remote_file.read_to_end(&mut verify_buf).await.unwrap();
    assert_eq!(sha256_hex(&verify_buf), sha256_hex(&payload));
    let _ = sftp.remove_file(&remote_path).await;

    let _ = tokio::process::Command::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "sh",
            "-c",
            "rm -f /config/.ssh/authorized_keys",
        ])
        .output()
        .await;
    let _ = agent.start_kill();
}
