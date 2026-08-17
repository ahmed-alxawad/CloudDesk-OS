use base64::{engine::general_purpose::STANDARD, Engine};
use clouddesk_remote::ssh::{SshAuth, SshSession};
use russh::keys::ssh_key::PrivateKey;
use russh::{
    server::{Auth, Handler, Msg, Session},
    Channel, ChannelId,
};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::time::Duration;

#[derive(Clone)]
struct MockSshServer;

impl Handler for MockSshServer {
    type Error = anyhow::Error;

    fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> impl std::future::Future<Output = Result<Auth, Self::Error>> + Send {
        let auth = if user == "testuser" && password == "testpass" {
            Auth::Accept
        } else {
            Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            }
        };
        std::future::ready(Ok(auth))
    }

    fn auth_publickey(
        &mut self,
        user: &str,
        _public_key: &russh::keys::ssh_key::PublicKey,
    ) -> impl std::future::Future<Output = Result<Auth, Self::Error>> + Send {
        let auth = if user == "testuser" {
            Auth::Accept
        } else {
            Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            }
        };
        std::future::ready(Ok(auth))
    }

    fn env_request(
        &mut self,
        _channel: ChannelId,
        _name: &str,
        _value: &str,
        _session: &mut Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        std::future::ready(Ok(()))
    }

    #[allow(clippy::manual_async_fn)]
    fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        handle: russh::ChannelOpenHandleInner<Msg>,
        _session: &mut Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async move {
            handle.accept().await;
            Ok(())
        }
    }

    fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        let _ = session.data(channel, bytes::Bytes::from(data.to_vec()));
        std::future::ready(Ok(()))
    }

    fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        let cmd = String::from_utf8_lossy(data);
        if cmd == "echo 'harmless'" {
            let _ = session.data(channel, bytes::Bytes::from(b"harmless\n".to_vec()));
            let _ = session.exit_status_request(channel, 0);
        } else {
            let _ = session.data(channel, bytes::Bytes::from(b"command not found\n".to_vec()));
            let _ = session.exit_status_request(channel, 127);
        }
        let _ = session.close(channel);
        std::future::ready(Ok(()))
    }
}

const TEST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBswrGY1nEnqW9lIuvMn5oFfozsW7dJMC2oodSq2WFYowAAAJg7NuFaOzbh\nWgAAAAtzc2gtZWQyNTUxOQAAACBswrGY1nEnqW9lIuvMn5oFfozsW7dJMC2oodSq2WFYow\nAAAED8ijpQnhgmGHVQIr+UJ/ybod+6d22qAlbEH6ny0CwchmzCsZjWcSepb2Ui68yfmgV+\njOxbt0kwLaih1KrZYVijAAAADmNsb3VkZGVzay10ZXN0AQIDBAUGBw==\n-----END OPENSSH PRIVATE KEY-----\n";

/// `base64(SSH wire-encoded public key)` matching [`TEST_KEY`] — the same
/// format as `remote_servers.host_key_base64` / `PinnedHostKey::key_base64`.
fn test_key_public_base64() -> String {
    let key = PrivateKey::from_openssh(TEST_KEY).unwrap();
    STANDARD.encode(key.public_key().to_bytes().unwrap())
}

async fn start_mock_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let key = PrivateKey::from_openssh(TEST_KEY).unwrap();
        let config = russh::server::Config {
            inactivity_timeout: Some(std::time::Duration::from_hours(1)),
            auth_rejection_time: std::time::Duration::from_secs(1),
            auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
            keys: vec![key],
            ..Default::default()
        };
        let config = Arc::new(config);

        if let Ok((socket, _)) = listener.accept().await {
            let _ = russh::server::run_stream(config, socket, MockSshServer).await;
        }
    });

    port
}

#[tokio::test]
async fn test_ssh_password_auth_success() {
    let port = start_mock_server().await;

    let auth = SshAuth::Password("testpass".to_string());
    let mut session =
        SshSession::connect("127.0.0.1", port, "testuser", auth, Duration::from_secs(5))
            .await
            .expect("Failed to connect");

    let output = session
        .run_command("echo 'harmless'")
        .await
        .expect("Failed to run command");
    assert_eq!(output, "harmless\n");

    session.disconnect().await.expect("Failed to disconnect");
}

#[tokio::test]
async fn test_ssh_password_auth_failure() {
    let port = start_mock_server().await;

    let auth = SshAuth::Password("wrongpass".to_string());
    let res =
        SshSession::connect("127.0.0.1", port, "testuser", auth, Duration::from_secs(5)).await;
    assert!(res.is_err(), "Authentication should have failed");
}

/// Regression test for CLAUDE-NIGHTMARE-002: `SshClientHandler::check_server_key`
/// unconditionally returned `Ok(true)`, so any server — including one that
/// replaced its host key after a MITM or compromise — was silently trusted.
/// A connection pinned to the server's real key must still succeed.
#[tokio::test]
async fn test_ssh_connect_pinned_host_key_match_succeeds() {
    let port = start_mock_server().await;

    let auth = SshAuth::Password("testpass".to_string());
    let mut session = SshSession::connect_pinned(
        "127.0.0.1",
        port,
        "testuser",
        auth,
        Duration::from_secs(5),
        Some(test_key_public_base64()),
    )
    .await
    .expect("connection pinned to the real host key must succeed");

    let output = session
        .run_command("echo 'harmless'")
        .await
        .expect("Failed to run command");
    assert_eq!(output, "harmless\n");
}

/// Regression test for CLAUDE-NIGHTMARE-002: a server presenting a host key
/// that does not match the pinned key (host-key replacement / MITM) must be
/// rejected outright, never silently accepted.
#[tokio::test]
async fn test_ssh_connect_pinned_host_key_mismatch_is_rejected() {
    let port = start_mock_server().await;

    let wrong_key = STANDARD.encode([7_u8; 32]);
    assert_ne!(wrong_key, test_key_public_base64());

    let auth = SshAuth::Password("testpass".to_string());
    let res = SshSession::connect_pinned(
        "127.0.0.1",
        port,
        "testuser",
        auth,
        Duration::from_secs(5),
        Some(wrong_key),
    )
    .await;
    assert!(
        res.is_err(),
        "a host-key mismatch must reject the connection, not silently accept it"
    );
}
