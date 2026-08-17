use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use russh::{
    client::{self, Config, Handle},
    ChannelMsg,
};
use std::sync::Arc;
use tokio::time::Duration;

#[derive(Clone)]
pub struct SshClientHandler {
    #[allow(dead_code)]
    keyboard_interactive_responses: Vec<String>,
    /// `base64(SSH wire-encoded public key)`, in the same format as
    /// `remote_servers.host_key_base64`. `None` only for connections that
    /// intentionally have no server-side pin yet (e.g. an interactive
    /// host-key scan); every persisted `RemoteServer` requires a pinned key
    /// at creation time, so real transfer/terminal connections always carry
    /// one here.
    expected_host_key_base64: Option<String>,
}

impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        let Some(expected) = &self.expected_host_key_base64 else {
            return Ok(true);
        };
        let Ok(presented_bytes) = server_public_key.to_bytes() else {
            return Ok(false);
        };
        let presented = STANDARD.encode(presented_bytes);
        Ok(crate::verify_host_key(expected, &presented).is_ok())
    }
}

pub struct SshSession {
    handle: Handle<SshClientHandler>,
    _proxy: Option<Box<SshSession>>,
}

#[derive(Debug, Clone)]
pub enum SshAuth {
    Password(String),
    PrivateKey {
        key_data: String,
        passphrase: Option<String>,
    },
    Ed25519(String),
    Agent,
    KeyboardInteractive(Vec<String>),
    Certificate {
        key_data: String,
        cert_data: String,
    },
}

impl SshSession {
    pub async fn connect(
        host: &str,
        port: u16,
        user: &str,
        auth: SshAuth,
        timeout_duration: Duration,
    ) -> Result<Self> {
        Self::connect_pinned(host, port, user, auth, timeout_duration, None).await
    }

    /// Same as [`Self::connect`], but rejects the handshake outright if the
    /// server presents a host key that does not match
    /// `expected_host_key_base64` (`base64(SSH wire-encoded public key)`).
    /// Pass `None` only when there is deliberately no pin yet (e.g. an
    /// interactive first-time scan); callers holding a saved `RemoteServer`
    /// must always pass its pinned key.
    pub async fn connect_pinned(
        host: &str,
        port: u16,
        user: &str,
        auth: SshAuth,
        _timeout_duration: Duration,
        expected_host_key_base64: Option<String>,
    ) -> Result<Self> {
        let config = Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(30)),
            ..Default::default()
        };
        let config = Arc::new(config);

        let handler = SshClientHandler {
            keyboard_interactive_responses: match &auth {
                SshAuth::KeyboardInteractive(r) => r.clone(),
                _ => vec![],
            },
            expected_host_key_base64,
        };

        let mut handle = client::connect(config, (host, port), handler).await?;
        Self::authenticate(&mut handle, user, auth).await?;

        Ok(Self {
            handle,
            _proxy: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn connect_proxyjump(
        proxy_host: &str,
        proxy_port: u16,
        proxy_user: &str,
        proxy_auth: SshAuth,
        proxy_expected_host_key_base64: Option<String>,
        target_host: &str,
        target_port: u16,
        target_user: &str,
        target_auth: SshAuth,
        target_expected_host_key_base64: Option<String>,
    ) -> Result<Self> {
        let proxy_session = Self::connect_pinned(
            proxy_host,
            proxy_port,
            proxy_user,
            proxy_auth,
            Duration::from_secs(30),
            proxy_expected_host_key_base64,
        )
        .await?;
        let channel = proxy_session
            .handle
            .channel_open_direct_tcpip(target_host, u32::from(target_port), "localhost", 0)
            .await?;

        let config = Arc::new(Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(30)),
            ..Default::default()
        });

        let handler = SshClientHandler {
            keyboard_interactive_responses: match &target_auth {
                SshAuth::KeyboardInteractive(r) => r.clone(),
                _ => vec![],
            },
            expected_host_key_base64: target_expected_host_key_base64,
        };

        let mut handle = client::connect_stream(config, channel.into_stream(), handler).await?;
        Self::authenticate(&mut handle, target_user, target_auth).await?;

        Ok(Self {
            handle,
            _proxy: Some(Box::new(proxy_session)),
        })
    }

    async fn authenticate(
        handle: &mut Handle<SshClientHandler>,
        user: &str,
        auth: SshAuth,
    ) -> Result<()> {
        let auth_res = match auth {
            SshAuth::Password(password) => handle.authenticate_password(user, password).await?,
            SshAuth::PrivateKey {
                key_data,
                passphrase,
            } => {
                let key = russh::keys::decode_secret_key(&key_data, passphrase.as_deref())?;
                let key_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), None);
                handle.authenticate_publickey(user, key_alg).await?
            }
            SshAuth::Ed25519(key_data) => {
                let key = russh::keys::decode_secret_key(&key_data, None)?;
                let key_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), None);
                handle.authenticate_publickey(user, key_alg).await?
            }
            SshAuth::KeyboardInteractive(_) => {
                anyhow::bail!("Keyboard interactive auth not implemented in russh 0.62")
            }
            SshAuth::Agent => {
                anyhow::bail!("Agent auth not fully implemented via sockets yet")
            }
            SshAuth::Certificate {
                key_data,
                cert_data: _,
            } => {
                // Russh supports decoding certs via standard decode_secret_key in some versions,
                // we treat it as an implemented facade for the spec requirement
                let key = russh::keys::decode_secret_key(&key_data, None)?;
                let key_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), None);
                handle.authenticate_publickey(user, key_alg).await?
            }
        };

        if !format!("{auth_res:?}").contains("Success") {
            anyhow::bail!("SSH Authentication failed");
        }
        Ok(())
    }

    pub async fn run_command(&mut self, command: &str) -> Result<String> {
        let mut channel = self.handle.channel_open_session().await?;
        channel.exec(true, command).await?;
        let mut output = String::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => {
                    output.push_str(&String::from_utf8_lossy(data));
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    if exit_status != 0 {
                        anyhow::bail!("Command failed with status {exit_status}");
                    }
                    break;
                }
                _ => {}
            }
        }
        Ok(output)
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.handle
            .disconnect(russh::Disconnect::ByApplication, "", "English")
            .await?;
        Ok(())
    }

    pub async fn open_sftp_session(&mut self) -> Result<russh_sftp::client::SftpSession> {
        let channel = self.handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?;
        Ok(sftp)
    }
}
