//! Live `ProxyJump` tests against real disposable OpenSSH fixtures
//! (`tests/acceptance/docker-compose.yml`: `openssh` as the bastion,
//! `openssh-target` as the target — deliberately given NO host port
//! mapping, so it is reachable only through the bastion's compose-internal
//! network, proving a successful test here genuinely went
//! client -> bastion -> target rather than reaching an independently
//! host-reachable "target").
//!
//! Requires `docker compose up -d` to have been run in `tests/acceptance/`
//! first. Skips (rather than fails) if the bastion isn't reachable, so
//! this doesn't break `cargo test --workspace` runs without Docker.

use clouddesk_remote::sftp::SftpProvider;
use clouddesk_remote::{NewRemoteServer, RemoteServerStore, SshAuthMethod};
use clouddesk_secrets::SecretCipher;
use clouddesk_vault::Vault;
use clouddesk_vfs::VfsProvider;
use clouddeskd::worker::{resolve_ssh_session, SshResolveError};

const BASTION_HOST: &str = "127.0.0.1";
const BASTION_PORT: u16 = 2222;
const BASTION_USER: &str = "testuser";
const BASTION_PASSWORD: &str = "testpassword";
// Resolvable only from inside the bastion container.
const TARGET_HOST: &str = "openssh-target";
const TARGET_PORT: u16 = 2222;
const TARGET_USER: &str = "targetuser";
const TARGET_PASSWORD: &str = "targetpassword";

async fn fixture_available() -> bool {
    tokio::net::TcpStream::connect((BASTION_HOST, BASTION_PORT))
        .await
        .is_ok()
}

async fn scan_host_key(host: &str, port: u16) -> String {
    // Scans via the bastion container so `openssh-target` (a bare service
    // name) resolves correctly, exactly like a real client would need to.
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "ssh-keyscan",
            "-t",
            "ed25519",
            "-p",
            &port.to_string(),
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

struct Harness {
    store: RemoteServerStore,
    vault: Vault,
    owner: String,
    pool: sqlx::SqlitePool,
}

impl Harness {
    async fn new() -> Self {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
             VALUES ('owner-a', 'owner-a', 'Owner A', 'x', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
             VALUES ('owner-b', 'owner-b', 'Owner B', 'x', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        Self {
            store: RemoteServerStore::new(pool.clone()),
            vault: Vault::new(pool.clone(), SecretCipher::new(&[7_u8; 32]).unwrap()),
            owner: "owner-a".to_owned(),
            pool,
        }
    }

    async fn store_password(&self, owner: &str, password: &str) -> String {
        self.vault
            .create(
                owner,
                "ssh.password",
                "test credential",
                password.as_bytes(),
            )
            .await
            .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_server(
        &self,
        owner: &str,
        hostname: &str,
        port: u16,
        username: &str,
        password: &str,
        host_key_base64: &str,
        proxy_jump_server_id: Option<String>,
    ) -> String {
        let secret_id = self.store_password(owner, password).await;
        self.store
            .create(
                owner,
                &NewRemoteServer {
                    name: format!("{hostname}:{port}"),
                    hostname: hostname.to_owned(),
                    port,
                    username: username.to_owned(),
                    auth_method: SshAuthMethod::Password,
                    credential_secret_id: Some(secret_id),
                    agent_socket_path: None,
                    host_key_type: "ssh-ed25519".to_owned(),
                    host_key_base64: host_key_base64.to_owned(),
                    proxy_jump_server_id,
                    tags: vec![],
                },
            )
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn valid_proxyjump_connects_through_the_bastion_to_an_unreachable_target() {
    if !fixture_available().await {
        eprintln!("skipping: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let harness = Harness::new().await;
    let bastion_key = scan_host_key(BASTION_HOST, BASTION_PORT).await;
    let target_key = scan_host_key(TARGET_HOST, TARGET_PORT).await;

    let bastion_id = harness
        .create_server(
            &harness.owner,
            BASTION_HOST,
            BASTION_PORT,
            BASTION_USER,
            BASTION_PASSWORD,
            &bastion_key,
            None,
        )
        .await;
    let target_id = harness
        .create_server(
            &harness.owner,
            TARGET_HOST,
            TARGET_PORT,
            TARGET_USER,
            TARGET_PASSWORD,
            &target_key,
            Some(bastion_id),
        )
        .await;

    let mut session =
        resolve_ssh_session(&harness.store, &harness.vault, &harness.owner, &target_id)
            .await
            .expect("ProxyJump connection must succeed");
    let output = session.run_command("echo proxyjump-ok").await.unwrap();
    assert_eq!(output, "proxyjump-ok\n");
}

/// Task 7: SFTP already exists; this proves it works over the `ProxyJump`
/// path specifically (list/upload/download/rename/delete against the
/// otherwise-unreachable target, through the bastion), and that target
/// host-key pinning still applies on that path.
#[tokio::test(flavor = "multi_thread")]
async fn sftp_operations_work_through_proxyjump() {
    if !fixture_available().await {
        eprintln!("skipping: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let harness = Harness::new().await;
    let bastion_key = scan_host_key(BASTION_HOST, BASTION_PORT).await;
    let target_key = scan_host_key(TARGET_HOST, TARGET_PORT).await;

    let bastion_id = harness
        .create_server(
            &harness.owner,
            BASTION_HOST,
            BASTION_PORT,
            BASTION_USER,
            BASTION_PASSWORD,
            &bastion_key,
            None,
        )
        .await;
    let target_id = harness
        .create_server(
            &harness.owner,
            TARGET_HOST,
            TARGET_PORT,
            TARGET_USER,
            TARGET_PASSWORD,
            &target_key,
            Some(bastion_id),
        )
        .await;

    let mut session =
        resolve_ssh_session(&harness.store, &harness.vault, &harness.owner, &target_id)
            .await
            .expect("ProxyJump connection must succeed");
    let sftp = session.open_sftp_session().await.unwrap();
    let handle = tokio::runtime::Handle::current();
    let provider = SftpProvider::new(sftp, handle);

    let content = b"sftp over proxyjump".to_vec();
    provider
        .write_file("/proxyjump-sftp.bin", &content)
        .unwrap();
    let listed = provider.list("/").unwrap();
    assert!(listed.iter().any(|e| e.name == "proxyjump-sftp.bin"));
    let read_back = provider
        .read_limited("/proxyjump-sftp.bin", content.len())
        .unwrap();
    assert_eq!(read_back, content);
    provider
        .rename("/proxyjump-sftp.bin", "/proxyjump-sftp-renamed.bin")
        .unwrap();
    provider.trash("/proxyjump-sftp-renamed.bin").unwrap();
}

#[tokio::test]
async fn wrong_bastion_host_key_is_rejected() {
    if !fixture_available().await {
        eprintln!("skipping: disposable OpenSSH fixture not running");
        return;
    }
    let harness = Harness::new().await;
    let target_key = scan_host_key(TARGET_HOST, TARGET_PORT).await;
    let wrong_key = base64_of([3_u8; 32]);

    let bastion_id = harness
        .create_server(
            &harness.owner,
            BASTION_HOST,
            BASTION_PORT,
            BASTION_USER,
            BASTION_PASSWORD,
            &wrong_key,
            None,
        )
        .await;
    let target_id = harness
        .create_server(
            &harness.owner,
            TARGET_HOST,
            TARGET_PORT,
            TARGET_USER,
            TARGET_PASSWORD,
            &target_key,
            Some(bastion_id),
        )
        .await;

    let result =
        resolve_ssh_session(&harness.store, &harness.vault, &harness.owner, &target_id).await;
    assert!(
        result.is_err(),
        "a mismatched bastion host key must be rejected"
    );
}

#[tokio::test]
async fn wrong_target_host_key_is_rejected() {
    if !fixture_available().await {
        eprintln!("skipping: disposable OpenSSH fixture not running");
        return;
    }
    let harness = Harness::new().await;
    let bastion_key = scan_host_key(BASTION_HOST, BASTION_PORT).await;
    let wrong_key = base64_of([4_u8; 32]);

    let bastion_id = harness
        .create_server(
            &harness.owner,
            BASTION_HOST,
            BASTION_PORT,
            BASTION_USER,
            BASTION_PASSWORD,
            &bastion_key,
            None,
        )
        .await;
    let target_id = harness
        .create_server(
            &harness.owner,
            TARGET_HOST,
            TARGET_PORT,
            TARGET_USER,
            TARGET_PASSWORD,
            &wrong_key,
            Some(bastion_id),
        )
        .await;

    let result =
        resolve_ssh_session(&harness.store, &harness.vault, &harness.owner, &target_id).await;
    assert!(
        result.is_err(),
        "a mismatched target host key must be rejected, even through a trusted bastion"
    );
}

#[tokio::test]
async fn bastion_authentication_failure_is_rejected() {
    if !fixture_available().await {
        eprintln!("skipping: disposable OpenSSH fixture not running");
        return;
    }
    let harness = Harness::new().await;
    let bastion_key = scan_host_key(BASTION_HOST, BASTION_PORT).await;
    let target_key = scan_host_key(TARGET_HOST, TARGET_PORT).await;

    let bastion_id = harness
        .create_server(
            &harness.owner,
            BASTION_HOST,
            BASTION_PORT,
            BASTION_USER,
            "definitely-wrong-password",
            &bastion_key,
            None,
        )
        .await;
    let target_id = harness
        .create_server(
            &harness.owner,
            TARGET_HOST,
            TARGET_PORT,
            TARGET_USER,
            TARGET_PASSWORD,
            &target_key,
            Some(bastion_id),
        )
        .await;

    let result =
        resolve_ssh_session(&harness.store, &harness.vault, &harness.owner, &target_id).await;
    assert!(
        result.is_err(),
        "bastion authentication failure must abort the whole connection"
    );
}

#[tokio::test]
async fn target_authentication_failure_is_rejected() {
    if !fixture_available().await {
        eprintln!("skipping: disposable OpenSSH fixture not running");
        return;
    }
    let harness = Harness::new().await;
    let bastion_key = scan_host_key(BASTION_HOST, BASTION_PORT).await;
    let target_key = scan_host_key(TARGET_HOST, TARGET_PORT).await;

    let bastion_id = harness
        .create_server(
            &harness.owner,
            BASTION_HOST,
            BASTION_PORT,
            BASTION_USER,
            BASTION_PASSWORD,
            &bastion_key,
            None,
        )
        .await;
    let target_id = harness
        .create_server(
            &harness.owner,
            TARGET_HOST,
            TARGET_PORT,
            TARGET_USER,
            "definitely-wrong-password",
            &target_key,
            Some(bastion_id),
        )
        .await;

    let result =
        resolve_ssh_session(&harness.store, &harness.vault, &harness.owner, &target_id).await;
    assert!(
        result.is_err(),
        "target authentication failure must be reported, not silently succeed"
    );
}

#[tokio::test]
async fn target_unreachable_from_host_directly_proves_the_topology() {
    // Not a resolve_ssh_session test — a sanity check that the fixture
    // topology itself is real: the target must NOT be reachable without
    // going through the bastion, or the ProxyJump tests above would prove
    // nothing.
    if !fixture_available().await {
        eprintln!("skipping: disposable OpenSSH fixture not running");
        return;
    }
    let direct = tokio::net::TcpStream::connect(("127.0.0.1", 2223)).await;
    assert!(
        direct.is_err(),
        "the ProxyJump target must have no host-reachable port for these tests to mean anything"
    );
}

#[tokio::test]
async fn self_reference_is_rejected() {
    let harness = Harness::new().await;
    // No live fixture needed: self-reference is caught before any
    // network connection is attempted.
    let fake_key = base64_of([5_u8; 32]);
    let id = harness
        .create_server(
            &harness.owner,
            "example.invalid",
            22,
            "user",
            "pass",
            &fake_key,
            None,
        )
        .await;
    // A record can't literally self-reference through `create` (the ID is
    // unknown until after insert), so simulate it directly to exercise
    // `resolve_ssh_session`'s own defensive check.
    sqlx::query("UPDATE remote_servers SET proxy_jump_server_id = ? WHERE id = ?")
        .bind(&id)
        .bind(&id)
        .execute(&harness.pool)
        .await
        .unwrap();

    let result = resolve_ssh_session(&harness.store, &harness.vault, &harness.owner, &id).await;
    assert!(matches!(result, Err(SshResolveError::SelfReference)));
}

#[tokio::test]
async fn a_to_b_to_a_loop_is_rejected_as_chain_too_deep() {
    let harness = Harness::new().await;
    let fake_key = base64_of([6_u8; 32]);
    let a_id = harness
        .create_server(
            &harness.owner,
            "a.invalid",
            22,
            "user",
            "pass",
            &fake_key,
            None,
        )
        .await;
    let b_id = harness
        .create_server(
            &harness.owner,
            "b.invalid",
            22,
            "user",
            "pass",
            &fake_key,
            Some(a_id.clone()),
        )
        .await;
    // Point A back at B to complete the A -> B -> A loop (again, only
    // constructible by direct DB manipulation today, since there is no
    // update endpoint — but `resolve_ssh_session` must still refuse it).
    sqlx::query("UPDATE remote_servers SET proxy_jump_server_id = ? WHERE id = ?")
        .bind(&b_id)
        .bind(&a_id)
        .execute(&harness.pool)
        .await
        .unwrap();

    let result = resolve_ssh_session(&harness.store, &harness.vault, &harness.owner, &a_id).await;
    assert!(matches!(result, Err(SshResolveError::ChainTooDeep)));
}

#[tokio::test]
async fn missing_target_is_rejected() {
    let harness = Harness::new().await;
    // No server was ever created with this ID.
    let result = resolve_ssh_session(
        &harness.store,
        &harness.vault,
        &harness.owner,
        "no-such-server",
    )
    .await;
    assert!(matches!(result, Err(SshResolveError::NotFound)));
}

#[tokio::test]
async fn deleting_a_bastion_nulls_the_dependent_reference_instead_of_leaving_it_dangling() {
    // `remote_servers.proxy_jump_server_id` is declared
    // `REFERENCES remote_servers(id) ON DELETE SET NULL` — SQLite's own
    // foreign-key enforcement makes a truly dangling bastion reference
    // structurally impossible to create (confirmed live: forcing one via
    // a raw UPDATE fails with a FOREIGN KEY constraint error, not a
    // successful write). This test verifies the actual, reachable
    // consequence: deleting a bastion silently clears the dependent's
    // reference rather than leaving a stale pointer for
    // `resolve_ssh_session` to stumble over.
    let harness = Harness::new().await;
    let fake_key = base64_of([8_u8; 32]);
    let bastion_id = harness
        .create_server(
            &harness.owner,
            "bastion.invalid",
            22,
            "user",
            "pass",
            &fake_key,
            None,
        )
        .await;
    let target_id = harness
        .create_server(
            &harness.owner,
            "target.invalid",
            22,
            "user",
            "pass",
            &fake_key,
            Some(bastion_id.clone()),
        )
        .await;

    harness
        .store
        .delete(&harness.owner, &bastion_id)
        .await
        .unwrap();

    let proxy_after_delete: Option<String> =
        sqlx::query_scalar("SELECT proxy_jump_server_id FROM remote_servers WHERE id = ?")
            .bind(&target_id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
    assert_eq!(proxy_after_delete, None);
}

#[tokio::test]
async fn cross_user_bastion_reference_is_denied_even_if_directly_forced_into_the_database() {
    let harness = Harness::new().await;
    let fake_key = base64_of([9_u8; 32]);
    // Owner B's own bastion.
    let bastion_owned_by_b = harness
        .create_server(
            "owner-b",
            "bastion.invalid",
            22,
            "user",
            "pass",
            &fake_key,
            None,
        )
        .await;
    // Owner A's target — `create`'s own ownership check makes it
    // impossible to legitimately set this to owner B's bastion, so force
    // it directly to prove `resolve_ssh_session` independently refuses to
    // cross the ownership boundary too (defense in depth, not the only
    // guard).
    let target_owned_by_a = harness
        .create_server(
            &harness.owner,
            "target.invalid",
            22,
            "user",
            "pass",
            &fake_key,
            None,
        )
        .await;
    sqlx::query("UPDATE remote_servers SET proxy_jump_server_id = ? WHERE id = ?")
        .bind(&bastion_owned_by_b)
        .bind(&target_owned_by_a)
        .execute(&harness.pool)
        .await
        .unwrap();

    let result = resolve_ssh_session(
        &harness.store,
        &harness.vault,
        &harness.owner,
        &target_owned_by_a,
    )
    .await;
    assert!(
        matches!(result, Err(SshResolveError::NotFound)),
        "a bastion reference resolved under a different owner must be treated as not found, not silently used"
    );
}

fn base64_of(bytes: [u8; 32]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
