use clouddesk_auth::AuthService;
use clouddesk_remote::s3::S3Provider;
use clouddesk_remote::sftp::SftpProvider;
use clouddesk_remote::webdav::WebDavProvider;
use clouddesk_remote::{
    ssh::{SshAuth, SshSession},
    RemoteServer, RemoteServerStore, SshAuthMethod,
};
use clouddesk_transfers::{TransferEndpoint, TransferError, TransferJob, TransferQueue};
use clouddesk_vault::Vault;
use clouddesk_vfs::VfsProvider;
use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;

/// `ProxyJump` chains are bounded to a target plus a single bastion hop.
/// `RemoteServer.proxy_jump_server_id` is only ever set by the owning
/// user on their own records (`RemoteServerStore::create` enforces
/// ownership), and there is no update endpoint that could extend an
/// existing record's chain — so today a chain longer than this cannot
/// actually be constructed through the product. The bound is still
/// enforced here defensively (and cheaply): if a bastion itself has a
/// `proxy_jump_server_id` set, the connection is refused outright, which
/// also rejects every A -> B -> A style loop as a side effect (any loop
/// requires a bastion whose own `proxy_jump_server_id` is set).
const MAX_PROXY_CHAIN_HOPS: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum SshResolveError {
    #[error("remote server not found")]
    NotFound,
    #[error("a remote server cannot use itself as a ProxyJump bastion")]
    SelfReference,
    #[error("ProxyJump chain exceeds the maximum supported depth ({MAX_PROXY_CHAIN_HOPS} hops)")]
    ChainTooDeep,
    #[error("SSH auth method is not yet supported for this connection path: {0:?}")]
    UnsupportedAuthMethod(SshAuthMethod),
    #[error(transparent)]
    Remote(#[from] clouddesk_remote::RemoteError),
    #[error(transparent)]
    Vault(#[from] clouddesk_vault::VaultError),
    #[error("SSH connection failed: {0}")]
    Connect(String),
}

struct ResolvedHop {
    server: RemoteServer,
    pinned_host_key_base64: String,
    auth: SshAuth,
}

async fn resolve_auth(
    vault: &Vault,
    owner_user_id: &str,
    server: &RemoteServer,
) -> Result<SshAuth, SshResolveError> {
    match server.auth_method {
        SshAuthMethod::Password => {
            let bytes = vault
                .reveal(
                    owner_user_id,
                    server.credential_secret_id.as_deref().unwrap_or_default(),
                )
                .await?;
            Ok(SshAuth::Password(
                String::from_utf8_lossy(&bytes).into_owned(),
            ))
        }
        SshAuthMethod::PrivateKey => {
            let bytes = vault
                .reveal(
                    owner_user_id,
                    server.credential_secret_id.as_deref().unwrap_or_default(),
                )
                .await?;
            Ok(SshAuth::PrivateKey {
                key_data: String::from_utf8_lossy(&bytes).into_owned(),
                passphrase: None,
            })
        }
        // SshAgent / KeyboardInteractive / Certificate: not yet implemented
        // as real connection paths (see V1_TRUE_CLOSURE.md #11-13). Persisting
        // one of these as a server's auth method must not silently fall back
        // to some other method -- it must fail loudly here instead.
        other => Err(SshResolveError::UnsupportedAuthMethod(other)),
    }
}

async fn resolve_hop(
    store: &RemoteServerStore,
    vault: &Vault,
    owner_user_id: &str,
    server_id: &str,
) -> Result<ResolvedHop, SshResolveError> {
    let server = store
        .get(owner_user_id, server_id)
        .await
        .map_err(|error| match error {
            clouddesk_remote::RemoteError::NotFound => SshResolveError::NotFound,
            other => SshResolveError::Remote(other),
        })?;
    let pinned = store.pinned_host_key(owner_user_id, server_id).await?;
    let auth = resolve_auth(vault, owner_user_id, &server).await?;
    Ok(ResolvedHop {
        server,
        pinned_host_key_base64: pinned.key_base64,
        auth,
    })
}

/// Resolves `target_server_id` to a live, authenticated [`SshSession`],
/// transparently following a single `ProxyJump` bastion hop when the
/// target's `proxy_jump_server_id` is set. Both hops' host keys are
/// independently verified against their own pinned values (never
/// "trusted because the other hop was trusted") and both hops'
/// credentials are independently resolved from Vault, scoped to
/// `owner_user_id` throughout -- `RemoteServerStore::get` refuses to
/// return a record owned by anyone else, so a bastion reference can never
/// resolve to another user's server (also enforced earlier, at record
/// creation time, by `RemoteServerStore::create`).
pub async fn resolve_ssh_session(
    store: &RemoteServerStore,
    vault: &Vault,
    owner_user_id: &str,
    target_server_id: &str,
) -> Result<SshSession, SshResolveError> {
    let target = resolve_hop(store, vault, owner_user_id, target_server_id).await?;

    let Some(bastion_id) = &target.server.proxy_jump_server_id else {
        return SshSession::connect_pinned(
            &target.server.hostname,
            target.server.port,
            &target.server.username,
            target.auth,
            Duration::from_secs(30),
            Some(target.pinned_host_key_base64),
        )
        .await
        .map_err(|error| SshResolveError::Connect(error.to_string()));
    };

    if bastion_id == target_server_id {
        return Err(SshResolveError::SelfReference);
    }
    let bastion = resolve_hop(store, vault, owner_user_id, bastion_id).await?;
    if bastion.server.proxy_jump_server_id.is_some() {
        return Err(SshResolveError::ChainTooDeep);
    }

    SshSession::connect_proxyjump(
        &bastion.server.hostname,
        bastion.server.port,
        &bastion.server.username,
        bastion.auth,
        Some(bastion.pinned_host_key_base64),
        &target.server.hostname,
        target.server.port,
        &target.server.username,
        target.auth,
        Some(target.pinned_host_key_base64),
    )
    .await
    .map_err(|error| SshResolveError::Connect(error.to_string()))
}

pub struct TransferWorker {
    queue: TransferQueue,
    vault: Vault,
    pool: sqlx::SqlitePool,
}

impl TransferWorker {
    #[must_use]
    pub fn new(auth: &AuthService) -> Self {
        Self {
            queue: TransferQueue::new(auth.pool().clone()),
            vault: Vault::new(auth.pool().clone(), auth.secret_cipher()),
            pool: auth.pool().clone(),
        }
    }

    pub fn spawn(self) {
        let queue_clone = self.queue.clone();
        tokio::spawn(async move {
            let _ = queue_clone.recover_interrupted().await;
        });

        tokio::spawn(async move {
            loop {
                match self.queue.claim_next().await {
                    Ok(Some(job)) => {
                        let id = job.id.clone();
                        let res = self.process_job(&job).await;
                        if let Err(e) = res {
                            let _ = self.queue.retry(&id, &e.to_string()).await;
                        } else {
                            let _ = self.queue.complete(&id, Some("sha256:completed")).await;
                        }
                    }
                    Ok(None) => {
                        sleep(Duration::from_secs(2)).await;
                    }
                    Err(e) => {
                        tracing::error!(%e, "Failed to claim next transfer job");
                        sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }

    #[allow(clippy::too_many_lines)]
    async fn get_provider(
        &self,
        endpoint: &TransferEndpoint,
        owner_user_id: &str,
    ) -> Result<Box<dyn VfsProvider + Send + Sync>, TransferError> {
        match endpoint {
            TransferEndpoint::Local { .. } => {
                let provider = clouddesk_vfs::LocalProvider::open("/", true)
                    .map_err(|e| TransferError::Io(e.to_string()))?;
                Ok(Box::new(provider))
            }
            TransferEndpoint::WebDav { connection_id, .. } => {
                let json_bytes = self
                    .vault
                    .reveal(owner_user_id, connection_id)
                    .await
                    .map_err(|e| TransferError::Io(e.to_string()))?;
                let val: Value = serde_json::from_slice(&json_bytes)
                    .map_err(|e| TransferError::Io(e.to_string()))?;
                let url = val["url"].as_str().unwrap_or("").to_string();
                let user = val["username"]
                    .as_str()
                    .map(std::string::ToString::to_string);
                let pass = val["password"]
                    .as_str()
                    .map(std::string::ToString::to_string);
                let handle = tokio::runtime::Handle::current();
                Ok(Box::new(WebDavProvider::new(url, user, pass, handle)))
            }
            TransferEndpoint::S3 { connection_id, .. } => {
                let json_bytes = self
                    .vault
                    .reveal(owner_user_id, connection_id)
                    .await
                    .map_err(|e| TransferError::Io(e.to_string()))?;
                let val: Value = serde_json::from_slice(&json_bytes)
                    .map_err(|e| TransferError::Io(e.to_string()))?;
                let access_key = val["access_key"].as_str().unwrap_or("");
                let secret_key = val["secret_key"].as_str().unwrap_or("");
                let region = val["region"].as_str().unwrap_or("us-east-1");
                let bucket = val["bucket"].as_str().unwrap_or("").to_string();
                let endpoint_url = val["endpoint"].as_str().unwrap_or("");

                let mut config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .credentials_provider(aws_sdk_s3::config::Credentials::new(
                        access_key, secret_key, None, None, "static",
                    ))
                    .region(aws_sdk_s3::config::Region::new(region.to_string()));
                if !endpoint_url.is_empty() {
                    config_loader = config_loader.endpoint_url(endpoint_url);
                }
                let config = config_loader.load().await;
                let handle = tokio::runtime::Handle::current();
                Ok(Box::new(S3Provider::new(&config, bucket, handle)))
            }
            TransferEndpoint::Sftp { server_id, .. } => {
                // CLAUDE-NIGHTMARE-002: the SSH client must reject a server
                // that presents a different host key than the one pinned
                // when this remote server was saved — otherwise a MITM'd or
                // replaced host is silently trusted. Also transparently
                // follows a ProxyJump bastion hop when the target server has
                // one configured (see `resolve_ssh_session`), verifying both
                // hops' host keys and credentials independently.
                let store = RemoteServerStore::new(self.pool.clone());
                let mut session =
                    resolve_ssh_session(&store, &self.vault, owner_user_id, server_id)
                        .await
                        .map_err(|e| TransferError::Io(e.to_string()))?;

                let sftp = session
                    .open_sftp_session()
                    .await
                    .map_err(|e| TransferError::Io(e.to_string()))?;
                let handle = tokio::runtime::Handle::current();
                Ok(Box::new(SftpProvider::new(sftp, handle)))
            }
        }
    }

    async fn process_job(&self, job: &TransferJob) -> Result<(), TransferError> {
        let src_provider = self.get_provider(&job.source, &job.owner_user_id).await?;
        let dst_provider = self
            .get_provider(&job.destination, &job.owner_user_id)
            .await?;

        let src_path = match &job.source {
            TransferEndpoint::Local { path }
            | TransferEndpoint::WebDav { path, .. }
            | TransferEndpoint::S3 { key: path, .. }
            | TransferEndpoint::Sftp { path, .. } => path,
        };
        let dst_path = match &job.destination {
            TransferEndpoint::Local { path }
            | TransferEndpoint::WebDav { path, .. }
            | TransferEndpoint::S3 { key: path, .. }
            | TransferEndpoint::Sftp { path, .. } => path,
        };

        // chunked copy - doing a simple read all since read_limited doesn't support offset easily yet
        // but we'll record the full size for progress tracking
        let bytes = src_provider
            .read_limited(src_path, usize::MAX)
            .map_err(|e| TransferError::Io(e.to_string()))?;
        dst_provider
            .write_file(dst_path, &bytes)
            .map_err(|e| TransferError::Io(e.to_string()))?;
        let transferred = bytes.len() as u64;
        let _ = self.queue.update_progress(&job.id, transferred).await;

        Ok(())
    }
}
