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
    #[error("SSH agent socket is unavailable or not owned by this user")]
    AgentSocketUnavailable,
    #[error("stored SSH credential is malformed")]
    MalformedCredential,
    #[error(transparent)]
    Remote(#[from] clouddesk_remote::RemoteError),
    #[error(transparent)]
    Vault(#[from] clouddesk_vault::VaultError),
    #[error("SSH connection failed: {0}")]
    Connect(String),
}

/// PASS SSH-B-2 (Task 3): classifies a resolved-connection failure as
/// permanent (never worth retrying -- the server doesn't exist/isn't
/// owned by this caller, the host key doesn't match, the stored
/// credential is malformed/missing, the `ProxyJump` chain is
/// structurally invalid) or transient/unknown (an actual SSH
/// connection/protocol failure, conservatively bounded-retried like
/// everything else -- see `TransferError::Io`). Applied wherever a
/// transfer job resolves a `RemoteServer` connection, so a
/// misconfigured or cross-user-referenced server fails fast instead
/// of retrying for the full backoff budget.
fn classify_ssh_resolve_error(error: &SshResolveError) -> TransferError {
    match &error {
        SshResolveError::NotFound
        | SshResolveError::SelfReference
        | SshResolveError::ChainTooDeep
        | SshResolveError::AgentSocketUnavailable
        | SshResolveError::MalformedCredential
        | SshResolveError::Remote(clouddesk_remote::RemoteError::HostKeyChanged) => {
            TransferError::Permanent(error.to_string())
        }
        _ => TransferError::Io(error.to_string()),
    }
}

struct ResolvedHop {
    server: RemoteServer,
    pinned_host_key_base64: String,
    auth: SshAuth,
}

async fn resolve_auth(
    store: &RemoteServerStore,
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
        SshAuthMethod::SshAgent => {
            let socket_path = server
                .agent_socket_path
                .clone()
                .ok_or(SshResolveError::AgentSocketUnavailable)?;
            verify_agent_socket_owner(store, owner_user_id, &socket_path).await?;
            Ok(SshAuth::Agent { socket_path })
        }
        SshAuthMethod::KeyboardInteractive => {
            let bytes = vault
                .reveal(
                    owner_user_id,
                    server.credential_secret_id.as_deref().unwrap_or_default(),
                )
                .await?;
            let responses: Vec<String> =
                serde_json::from_slice(&bytes).map_err(|_| SshResolveError::MalformedCredential)?;
            Ok(SshAuth::KeyboardInteractive(responses))
        }
        SshAuthMethod::Certificate => {
            let bytes = vault
                .reveal(
                    owner_user_id,
                    server.credential_secret_id.as_deref().unwrap_or_default(),
                )
                .await?;
            let material: CertificateCredential =
                serde_json::from_slice(&bytes).map_err(|_| SshResolveError::MalformedCredential)?;
            Ok(SshAuth::Certificate {
                key_data: material.key_data,
                cert_data: material.cert_data,
            })
        }
    }
}

/// Task 5 (Phase 2 closure): the Vault-held credential for
/// `SshAuthMethod::Certificate` -- a private key plus its matching
/// OpenSSH user certificate, packed together since `RemoteServer` has
/// only the one `credential_secret_id` slot. Both pieces are handled
/// exactly like a private key already is (Vault-encrypted at rest,
/// only ever revealed for this owning user, never logged).
#[derive(serde::Deserialize, serde::Serialize)]
pub struct CertificateCredential {
    pub key_data: String,
    pub cert_data: String,
}

/// Task 1/2/5 (Phase 2 closure): the real security boundary for SSH
/// agent auth -- re-checked at every single connection attempt, never
/// trusted merely because it passed the same check when the
/// `RemoteServer` was first registered. A socket owned by any UID
/// other than this exact server's owning `CloudDesk` user's own
/// mapped Linux UID is refused outright, structurally preventing one
/// user's `RemoteServer` from ever being pointed at another user's
/// agent.
async fn verify_agent_socket_owner(
    store: &RemoteServerStore,
    owner_user_id: &str,
    socket_path: &str,
) -> Result<(), SshResolveError> {
    let expected_uid: Option<i64> = sqlx::query_scalar("SELECT linux_uid FROM users WHERE id = ?")
        .bind(owner_user_id)
        .fetch_optional(store.pool())
        .await
        .map_err(clouddesk_remote::RemoteError::from)?
        .flatten();
    let Some(expected_uid) = expected_uid else {
        return Err(SshResolveError::AgentSocketUnavailable);
    };
    let metadata = tokio::fs::metadata(socket_path)
        .await
        .map_err(|_| SshResolveError::AgentSocketUnavailable)?;
    let actual_uid = i64::from(std::os::unix::fs::MetadataExt::uid(&metadata));
    if actual_uid != expected_uid {
        return Err(SshResolveError::AgentSocketUnavailable);
    }
    Ok(())
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
    let auth = resolve_auth(store, vault, owner_user_id, &server).await?;
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
                            // PASS SSH-B-2 (Task 2/3): bounded retry,
                            // with `Permanent` errors (auth denied,
                            // invalid path, host-key mismatch) failing
                            // immediately rather than spending the
                            // retry budget on something that can never
                            // succeed.
                            let (message, permanent) = e.retry_classification();
                            let _ = self.queue.retry(&id, &message, permanent).await;
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
            TransferEndpoint::Scp { .. } => Err(TransferError::Io(
                "SCP endpoints are handled by process_scp_job, never through the generic provider path".to_owned(),
            )),
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
        // PASS SSH-B: native SCP is a streamed exec-channel protocol,
        // not a random-access filesystem the generic
        // read_limited/write_file byte-array model fits -- handled by
        // its own dedicated path rather than forced into that shape.
        if matches!(job.source, TransferEndpoint::Scp { .. })
            || matches!(job.destination, TransferEndpoint::Scp { .. })
        {
            return self.process_scp_job(job).await;
        }

        let src_provider = self.get_provider(&job.source, &job.owner_user_id).await?;
        let dst_provider = self
            .get_provider(&job.destination, &job.owner_user_id)
            .await?;

        let src_path = match &job.source {
            TransferEndpoint::Local { path }
            | TransferEndpoint::WebDav { path, .. }
            | TransferEndpoint::S3 { key: path, .. }
            | TransferEndpoint::Sftp { path, .. } => path,
            TransferEndpoint::Scp { .. } => unreachable!("handled above"),
        };
        let dst_path = match &job.destination {
            TransferEndpoint::Local { path }
            | TransferEndpoint::WebDav { path, .. }
            | TransferEndpoint::S3 { key: path, .. }
            | TransferEndpoint::Sftp { path, .. } => path,
            TransferEndpoint::Scp { .. } => unreachable!("handled above"),
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

    /// PASS SSH-B (Task 5/6/9/10/26): a real, streamed (never
    /// whole-file-buffered) native SCP upload or download. v1 scope is
    /// deliberately narrow -- only Local<->Scp pairs -- so an
    /// unsupported pairing (e.g. Scp<->Sftp, Scp<->Scp) fails with a
    /// clear, typed error instead of being silently mishandled.
    ///
    /// The local side is reauthorized here, at execution time, against
    /// this exact owner's own mapped Linux home directory via the same
    /// `resolve_safe_path` jail every one-shot local upload/download
    /// already uses (Task 18/19) -- never trusted merely because it
    /// was accepted at `enqueue` time. The remote side goes through
    /// `resolve_ssh_session`, the same host-key-verified,
    /// `ProxyJump`-aware connection builder SFTP/WOPI/Browser remote
    /// uploads already use (Task 14/16/33) -- never a second SSH stack.
    #[allow(clippy::too_many_lines)]
    async fn process_scp_job(&self, job: &TransferJob) -> Result<(), TransferError> {
        let store = RemoteServerStore::new(self.pool.clone());
        match (&job.source, &job.destination) {
            (TransferEndpoint::Local { path: local_path }, TransferEndpoint::Scp { server_id, path: remote_path }) => {
                let home = self.local_home_for_owner(&job.owner_user_id).await?;
                let local_full_path = crate::resolve_safe_path(&home, local_path)
                    .map_err(|e| TransferError::Permanent(e.public_message.to_owned()))?;
                let metadata = tokio::fs::metadata(&local_full_path)
                    .await
                    .map_err(|e| TransferError::Io(e.to_string()))?;
                let mut file = tokio::fs::File::open(&local_full_path)
                    .await
                    .map_err(|e| TransferError::Io(e.to_string()))?;
                let mut session = resolve_ssh_session(&store, &self.vault, &job.owner_user_id, server_id)
                    .await
                    .map_err(|e| classify_ssh_resolve_error(&e))?;
                let job_id = job.id.clone();
                let queue = self.queue.clone();
                // Task 10/11 (PASS SSH-B-2): classic `scp -t` is not
                // atomic on the remote side -- it truncates/opens the
                // exact destination the moment the transfer starts, so
                // an interrupted upload would otherwise corrupt any
                // pre-existing file there. CloudDesk controls the
                // remote path passed to `scp -t`, so it uploads to a
                // disposable temp name in the same directory first,
                // and only `mv`s it into the canonical destination
                // after the SCP protocol itself reports full success --
                // a real, existing destination is never touched by a
                // failed/interrupted upload.
                let remote_temp_path = format!(
                    "{remote_path}.clouddesk-upload-{}.part",
                    clouddesk_auth::random_identifier(12)
                );
                let upload_result = session
                    .scp_upload(&remote_temp_path, "0644", metadata.len(), &mut file, move |bytes| {
                        let queue = queue.clone();
                        let job_id = job_id.clone();
                        tokio::spawn(async move {
                            let _ = queue.update_progress(&job_id, bytes).await;
                        });
                    })
                    .await;
                if let Err(e) = upload_result {
                    let quoted_temp = clouddesk_remote::scp::shell_single_quote(&remote_temp_path);
                    let _ = session.run_command(&format!("rm -f -- {quoted_temp}")).await;
                    self.audit_scp(&job.owner_user_id, "scp.upload.failed", server_id, remote_path, None)
                        .await;
                    return Err(TransferError::Io(e.to_string()));
                }
                let quoted_temp = clouddesk_remote::scp::shell_single_quote(&remote_temp_path);
                let quoted_dest = clouddesk_remote::scp::shell_single_quote(remote_path);
                session
                    .run_command(&format!("mv -- {quoted_temp} {quoted_dest}"))
                    .await
                    .map_err(|e| {
                        TransferError::Io(format!(
                            "SCP upload completed but the atomic remote rename failed: {e}"
                        ))
                    })?;
                self.audit_scp(&job.owner_user_id, "scp.upload.completed", server_id, remote_path, Some(metadata.len()))
                    .await;
                Ok(())
            }
            (TransferEndpoint::Scp { server_id, path: remote_path }, TransferEndpoint::Local { path: local_path }) => {
                let home = self.local_home_for_owner(&job.owner_user_id).await?;
                // Task 18/19/25: the destination is reauthorized against
                // this owner's own jail before a single byte is written --
                // the remote peer's own advertised filename (if any) is
                // never used to build this path.
                let local_full_path = crate::resolve_safe_path(&home, local_path)
                    .map_err(|e| TransferError::Permanent(e.public_message.to_owned()))?;
                // Resolved before any local temp file is created: a
                // connection failure on a retry attempt (e.g. the
                // remote is still unreachable) must never leave a
                // fresh, empty temp file behind via an early `?`
                // return -- the temp file's own lifetime is confined
                // to the block below, where every exit path either
                // renames or removes it.
                let mut session = resolve_ssh_session(&store, &self.vault, &job.owner_user_id, server_id)
                    .await
                    .map_err(|e| classify_ssh_resolve_error(&e))?;
                if let Some(parent) = local_full_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                let temp_path = local_full_path.with_extension("scp-download.part");
                let mut temp_file = tokio::fs::File::create(&temp_path)
                    .await
                    .map_err(|e| TransferError::Io(e.to_string()))?;
                let job_id = job.id.clone();
                let queue = self.queue.clone();
                let download_result = session
                    .scp_download(remote_path, &mut temp_file, move |bytes| {
                        let queue = queue.clone();
                        let job_id = job_id.clone();
                        tokio::spawn(async move {
                            let _ = queue.update_progress(&job_id, bytes).await;
                        });
                    })
                    .await;
                match download_result {
                    Ok(downloaded) => {
                        // Atomic local commit (Task 12): only rename
                        // into the real destination once the whole
                        // transfer succeeded; a failure never leaves a
                        // partial file at the real destination path.
                        tokio::fs::rename(&temp_path, &local_full_path)
                            .await
                            .map_err(|e| TransferError::Io(e.to_string()))?;
                        self.audit_scp(
                            &job.owner_user_id,
                            "scp.download.completed",
                            server_id,
                            remote_path,
                            Some(downloaded.size),
                        )
                        .await;
                        Ok(())
                    }
                    Err(e) => {
                        let _ = tokio::fs::remove_file(&temp_path).await;
                        self.audit_scp(&job.owner_user_id, "scp.download.failed", server_id, remote_path, None)
                            .await;
                        Err(TransferError::Io(e.to_string()))
                    }
                }
            }
            _ => Err(TransferError::Permanent(
                "SCP transfers are only supported between a local path and a remote SCP server in this release".to_owned(),
            )),
        }
    }

    /// The owner's mapped Linux home directory, resolved the same way
    /// `mapped_identity` does for the one-shot local upload/download
    /// HTTP handlers -- but from just `owner_user_id` (no live
    /// `SessionPrincipal` exists in this background worker), mirroring
    /// `verify_agent_socket_owner`'s existing raw-SQL pattern in this
    /// same file.
    async fn local_home_for_owner(
        &self,
        owner_user_id: &str,
    ) -> Result<std::path::PathBuf, TransferError> {
        let row: Option<(Option<i64>, Option<i64>)> =
            sqlx::query_as("SELECT linux_uid, linux_gid FROM users WHERE id = ?")
                .bind(owner_user_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| TransferError::Io(e.to_string()))?;
        let (Some(uid), Some(gid)) = row.unwrap_or((None, None)) else {
            return Err(TransferError::Permanent(
                "this user has no mapped Linux identity".to_owned(),
            ));
        };
        let uid = u32::try_from(uid)
            .map_err(|_| TransferError::Permanent("invalid mapped UID".to_owned()))?;
        let gid = u32::try_from(gid)
            .map_err(|_| TransferError::Permanent("invalid mapped GID".to_owned()))?;
        if uid == 0 || gid == 0 {
            return Err(TransferError::Permanent(
                "mapped Linux identity is not permitted".to_owned(),
            ));
        }
        let identity = clouddesk_linux::lookup_uid(uid)
            .map_err(|e| TransferError::Io(e.to_string()))?
            .ok_or_else(|| {
                TransferError::Permanent("mapped Linux UID no longer exists".to_owned())
            })?;
        if identity.gid != gid {
            return Err(TransferError::Permanent(
                "mapped Linux identity is no longer valid".to_owned(),
            ));
        }
        Ok(identity.home)
    }

    /// Task 26: SCP-specific audit events, safe metadata only -- user,
    /// `RemoteServer` ID, protocol = SCP, logical remote path, byte
    /// count where known. Never the local path (which may itself be
    /// considered sensitive) beyond what the generic transfer audit
    /// already records, and never any credential material.
    async fn audit_scp(
        &self,
        owner_user_id: &str,
        action: &str,
        server_id: &str,
        remote_path: &str,
        bytes: Option<u64>,
    ) {
        let event = clouddesk_audit::NewAuditEvent {
            timestamp: now_unix(),
            user_id: Some(owner_user_id.to_owned()),
            role_snapshot: Vec::new(),
            session_id_hash: None,
            source_ip: "background-transfer-worker".to_owned(),
            user_agent: "clouddesk-transfer-worker".to_owned(),
            action: action.to_owned(),
            resource_type: "remote_server".to_owned(),
            resource_id: Some(server_id.to_owned()),
            path: None,
            remote_target: Some(remote_path.to_owned()),
            result: if action.ends_with("failed") {
                "failure"
            } else {
                "success"
            }
            .to_owned(),
            metadata: serde_json::json!({ "protocol": "scp", "bytes": bytes }),
        };
        let _ = clouddesk_audit::append(&self.pool, &event).await;
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
