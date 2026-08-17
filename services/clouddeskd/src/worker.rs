use clouddesk_auth::AuthService;
use clouddesk_remote::s3::S3Provider;
use clouddesk_remote::sftp::SftpProvider;
use clouddesk_remote::webdav::WebDavProvider;
use clouddesk_remote::{
    ssh::{SshAuth, SshSession},
    RemoteServerStore,
};
use clouddesk_transfers::{TransferEndpoint, TransferError, TransferJob, TransferQueue};
use clouddesk_vault::Vault;
use clouddesk_vfs::VfsProvider;
use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;

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
                let store = RemoteServerStore::new(self.pool.clone());
                let server = store
                    .get(owner_user_id, server_id)
                    .await
                    .map_err(|e| TransferError::Io(e.to_string()))?;

                let auth = match server.auth_method {
                    clouddesk_remote::SshAuthMethod::Password => {
                        let pass_bytes = self
                            .vault
                            .reveal(
                                owner_user_id,
                                server.credential_secret_id.as_deref().unwrap_or_default(),
                            )
                            .await
                            .map_err(|e| TransferError::Io(e.to_string()))?;
                        SshAuth::Password(
                            String::from_utf8(pass_bytes.to_vec())
                                .map_err(|e| TransferError::Io(e.to_string()))?,
                        )
                    }
                    clouddesk_remote::SshAuthMethod::PrivateKey => {
                        let key_bytes = self
                            .vault
                            .reveal(
                                owner_user_id,
                                server.credential_secret_id.as_deref().unwrap_or_default(),
                            )
                            .await
                            .map_err(|e| TransferError::Io(e.to_string()))?;
                        SshAuth::PrivateKey {
                            key_data: String::from_utf8(key_bytes.to_vec())
                                .map_err(|e| TransferError::Io(e.to_string()))?,
                            passphrase: None,
                        }
                    }
                    _ => return Err(TransferError::Io("Unsupported SSH Auth Method".into())),
                };

                // CLAUDE-NIGHTMARE-002: the SSH client must reject a server
                // that presents a different host key than the one pinned
                // when this remote server was saved — otherwise a MITM'd or
                // replaced host is silently trusted.
                let pinned = store
                    .pinned_host_key(owner_user_id, server_id)
                    .await
                    .map_err(|e| TransferError::Io(e.to_string()))?;

                let mut session = SshSession::connect_pinned(
                    &server.hostname,
                    server.port as u16,
                    &server.username,
                    auth,
                    Duration::from_secs(30),
                    Some(pinned.key_base64),
                )
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
