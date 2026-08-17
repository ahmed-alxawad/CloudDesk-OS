use std::{
    collections::HashMap,
    fs,
    io::Read,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use clouddesk_linux::lookup_uid;
use clouddesk_privilege::{
    GrantSigner, PowerOperation, PrivdRequest, PrivdResponse, PrivilegedAction, ServiceOperation,
    ServiceUnit, SignedGrant,
};
use nix::unistd::{chown, Gid, Uid};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    process::Command,
    sync::Mutex,
};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct PrivdConfig {
    pub socket_path: PathBuf,
    pub socket_gid: u32,
    pub allowed_peer_uid: u32,
    pub sessiond_path: PathBuf,
    pub setpriv_path: PathBuf,
}

struct State {
    signer: GrantSigner,
    used_nonces: Mutex<HashMap<String, i64>>,
    config: PrivdConfig,
}

pub async fn run(config: PrivdConfig, key: &[u8]) -> anyhow::Result<()> {
    prepare_socket_path(&config.socket_path, config.socket_gid)?;
    let listener = UnixListener::bind(&config.socket_path)?;
    fs::set_permissions(&config.socket_path, fs::Permissions::from_mode(0o660))?;
    chown(
        &config.socket_path,
        Some(Uid::from_raw(0)),
        Some(Gid::from_raw(config.socket_gid)),
    )?;
    let state = Arc::new(State {
        signer: GrantSigner::new(key)?,
        used_nonces: Mutex::new(HashMap::new()),
        config,
    });

    tracing::info!(socket = %state.config.socket_path.display(), "cloudesk-privd listening");
    loop {
        let (stream, _) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, &state).await {
                tracing::warn!(%error, "privileged request rejected");
            }
        });
    }
}

fn prepare_socket_path(path: &Path, socket_gid: u32) -> anyhow::Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            anyhow::bail!("refusing to replace non-socket path: {}", path.display());
        }
        fs::remove_file(path)?;
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("socket path has no parent"))?;
    if fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        anyhow::bail!("refusing to use a symlink as the socket directory");
    }
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o750))?;
    chown(
        parent,
        Some(Uid::from_raw(0)),
        Some(Gid::from_raw(socket_gid)),
    )?;
    Ok(())
}

async fn handle_connection(mut stream: UnixStream, state: &State) -> anyhow::Result<()> {
    let peer = stream.peer_cred()?;
    if peer.uid() != state.config.allowed_peer_uid {
        anyhow::bail!("peer UID {} is not allowed", peer.uid());
    }
    let length = stream.read_u32().await? as usize;
    if length == 0 || length > MAX_REQUEST_BYTES {
        anyhow::bail!("request length is invalid");
    }
    let mut bytes = vec![0_u8; length];
    stream.read_exact(&mut bytes).await?;
    let request: PrivdRequest = serde_json::from_slice(&bytes)?;
    let response = process_request(state, request.grant).await;
    let response = serde_json::to_vec(&response)?;
    if response.len() > MAX_RESPONSE_BYTES {
        anyhow::bail!("privileged response is too large");
    }
    stream.write_u32(u32::try_from(response.len())?).await?;
    stream.write_all(&response).await?;
    Ok(())
}

async fn process_request(state: &State, grant: SignedGrant) -> PrivdResponse {
    let timestamp = unix_time();
    if let Err(error) = state.signer.verify(&grant, timestamp) {
        return rejected(format!("invalid authorization grant: {error}"));
    }
    {
        let mut used = state.used_nonces.lock().await;
        used.retain(|_, expires_at| *expires_at >= timestamp);
        if used.contains_key(&grant.claims.nonce) {
            return rejected("authorization grant was already used".to_owned());
        }
        used.insert(grant.claims.nonce.clone(), grant.claims.expires_at);
    }

    let nonce = grant.claims.nonce.clone();
    let action = grant.claims.action;
    let result = match action {
        PrivilegedAction::SpawnUserWorker { uid, gid, worker } => {
            spawn_user_worker(state, uid, gid, worker).await
        }
        PrivilegedAction::LocalFileOperation {
            uid,
            gid,
            root,
            writable,
            operation,
        } => spawn_file_worker(state, uid, gid, &root, writable, &operation).await,
        PrivilegedAction::OpenTerminalSession {
            uid,
            gid,
            rows,
            cols,
            shell,
        } => spawn_terminal_session(state, uid, gid, rows, cols, shell.as_deref(), &nonce).await,
        PrivilegedAction::ServiceControl { unit, operation } => {
            execute_service_control(&unit, operation).await
        }
        PrivilegedAction::Power { operation } => execute_power(operation).await,
    };

    match result {
        Ok(output) => {
            tracing::info!(
                user_id = %grant.claims.subject_user_id,
                session = %grant.claims.session_id_hash,
                nonce = %nonce,
                "privileged action completed"
            );
            PrivdResponse {
                accepted: true,
                message: "action completed".to_owned(),
                output: Some(output),
            }
        }
        Err(error) => {
            tracing::warn!(
                user_id = %grant.claims.subject_user_id,
                session = %grant.claims.session_id_hash,
                nonce = %nonce,
                %error,
                "privileged action failed"
            );
            rejected(error.to_string())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn spawn_terminal_session(
    state: &State,
    uid: u32,
    gid: u32,
    rows: u16,
    cols: u16,
    shell: Option<&str>,
    nonce: &str,
) -> anyhow::Result<serde_json::Value> {
    let identity = lookup_uid(uid)?.ok_or_else(|| anyhow::anyhow!("target UID does not exist"))?;
    if uid == 0 || gid == 0 || identity.gid != gid {
        anyhow::bail!("target UID/GID is invalid");
    }
    if !(2..=500).contains(&rows) || !(2..=500).contains(&cols) {
        anyhow::bail!("terminal dimensions are invalid");
    }
    if !nonce
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        anyhow::bail!("terminal session identifier is invalid");
    }
    let run_root = state
        .config
        .socket_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("privileged socket has no parent"))?;
    let sessions_root = run_root.join("sessions");
    if fs::symlink_metadata(&sessions_root).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        anyhow::bail!("refusing to use a symlink as terminal session root");
    }
    fs::create_dir_all(&sessions_root)?;
    fs::set_permissions(&sessions_root, fs::Permissions::from_mode(0o750))?;
    chown(
        &sessions_root,
        Some(Uid::from_raw(0)),
        Some(Gid::from_raw(state.config.socket_gid)),
    )?;
    let session_directory = sessions_root.join(nonce);
    fs::create_dir(&session_directory)?;
    fs::set_permissions(&session_directory, fs::Permissions::from_mode(0o750))?;
    chown(
        &session_directory,
        Some(Uid::from_raw(uid)),
        Some(Gid::from_raw(state.config.socket_gid)),
    )?;
    let socket_path = session_directory.join("terminal.sock");
    let mut command = Command::new(&state.config.setpriv_path);
    command
        .arg("--reuid")
        .arg(uid.to_string())
        .arg("--regid")
        .arg(gid.to_string())
        .arg("--init-groups")
        .arg("--reset-env")
        .arg(&state.config.sessiond_path)
        .arg("terminal")
        .arg("--expected-uid")
        .arg(uid.to_string())
        .arg("--expected-gid")
        .arg(gid.to_string())
        .arg("--socket")
        .arg(&socket_path)
        .arg("--rows")
        .arg(rows.to_string())
        .arg("--cols")
        .arg(cols.to_string())
        .kill_on_drop(false);
    if let Some(shell) = shell {
        command.arg("--shell").arg(shell);
    }
    let mut child = command.spawn()?;
    for _ in 0..80 {
        if fs::symlink_metadata(&socket_path).is_ok_and(|metadata| metadata.file_type().is_socket())
        {
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o660))?;
            chown(
                &socket_path,
                Some(Uid::from_raw(uid)),
                Some(Gid::from_raw(state.config.socket_gid)),
            )?;
            let displayed = socket_path.to_string_lossy().into_owned();
            tokio::spawn(async move {
                match child.wait().await {
                    Ok(status) if status.success() => {}
                    Ok(status) => tracing::warn!(%status, "terminal session worker exited"),
                    Err(error) => tracing::warn!(%error, "could not reap terminal session worker"),
                }
            });
            return Ok(json!({ "socket_path": displayed }));
        }
        if let Some(status) = child.try_wait()? {
            let _ = fs::remove_dir_all(&session_directory);
            anyhow::bail!("terminal session worker exited before readiness: {status}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let _ = child.kill().await;
    let _ = fs::remove_dir_all(&session_directory);
    anyhow::bail!("terminal session worker did not become ready")
}

#[derive(Clone, Copy)]
enum ServiceManager {
    Systemd,
    OpenRc,
}

struct CommandSpec {
    program: &'static str,
    arguments: Vec<String>,
}

async fn execute_service_control(
    unit: &ServiceUnit,
    operation: ServiceOperation,
) -> anyhow::Result<serde_json::Value> {
    let unit_name = unit.as_str();
    if matches!(
        unit_name,
        "clouddesk" | "clouddesk.service" | "cloudesk-privd" | "cloudesk-privd.service"
    ) {
        anyhow::bail!("CloudDesk's own security services cannot be controlled through this API");
    }
    let manager = detect_service_manager()?;
    let spec = service_command(manager, unit_name, operation);
    run_fixed_command(spec).await?;
    Ok(json!({ "unit": unit_name, "operation": operation }))
}

async fn execute_power(operation: PowerOperation) -> anyhow::Result<serde_json::Value> {
    let spec = if Path::new("/usr/bin/systemctl").is_file() {
        CommandSpec {
            program: "/usr/bin/systemctl",
            arguments: vec![
                "--no-ask-password".to_owned(),
                match operation {
                    PowerOperation::Reboot => "reboot",
                    PowerOperation::Shutdown => "poweroff",
                }
                .to_owned(),
            ],
        }
    } else {
        CommandSpec {
            program: match operation {
                PowerOperation::Reboot => "/sbin/reboot",
                PowerOperation::Shutdown => "/sbin/poweroff",
            },
            arguments: Vec::new(),
        }
    };
    run_fixed_command(spec).await?;
    Ok(json!({ "operation": operation }))
}

fn detect_service_manager() -> anyhow::Result<ServiceManager> {
    if Path::new("/usr/bin/systemctl").is_file() {
        Ok(ServiceManager::Systemd)
    } else if Path::new("/sbin/rc-service").is_file() {
        Ok(ServiceManager::OpenRc)
    } else {
        anyhow::bail!("no supported service manager was found")
    }
}

fn service_command(
    manager: ServiceManager,
    unit: &str,
    operation: ServiceOperation,
) -> CommandSpec {
    match manager {
        ServiceManager::Systemd => CommandSpec {
            program: "/usr/bin/systemctl",
            arguments: vec![
                "--no-ask-password".to_owned(),
                match operation {
                    ServiceOperation::Start => "start",
                    ServiceOperation::Stop => "stop",
                    ServiceOperation::Restart => "restart",
                    ServiceOperation::Enable => "enable",
                    ServiceOperation::Disable => "disable",
                }
                .to_owned(),
                unit.to_owned(),
            ],
        },
        ServiceManager::OpenRc => {
            let service = unit.strip_suffix(".service").unwrap_or(unit).to_owned();
            match operation {
                ServiceOperation::Enable | ServiceOperation::Disable => CommandSpec {
                    program: "/sbin/rc-update",
                    arguments: vec![
                        match operation {
                            ServiceOperation::Enable => "add",
                            ServiceOperation::Disable => "del",
                            _ => unreachable!(),
                        }
                        .to_owned(),
                        service,
                        "default".to_owned(),
                    ],
                },
                ServiceOperation::Start | ServiceOperation::Stop | ServiceOperation::Restart => {
                    CommandSpec {
                        program: "/sbin/rc-service",
                        arguments: vec![
                            service,
                            match operation {
                                ServiceOperation::Start => "start",
                                ServiceOperation::Stop => "stop",
                                ServiceOperation::Restart => "restart",
                                _ => unreachable!(),
                            }
                            .to_owned(),
                        ],
                    }
                }
            }
        }
    }
}

async fn run_fixed_command(spec: CommandSpec) -> anyhow::Result<()> {
    if !Path::new(spec.program).is_file() {
        anyhow::bail!("required system executable is unavailable");
    }
    let output = Command::new(spec.program)
        .args(spec.arguments)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .output()
        .await?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "system operation failed: {}",
            message.trim().chars().take(1024).collect::<String>()
        );
    }
    Ok(())
}

async fn spawn_file_worker(
    state: &State,
    uid: u32,
    gid: u32,
    root: &str,
    writable: bool,
    operation: &clouddesk_vfs::LocalFileOperation,
) -> anyhow::Result<serde_json::Value> {
    let identity = lookup_uid(uid)?.ok_or_else(|| anyhow::anyhow!("target UID does not exist"))?;
    if uid == 0 || gid == 0 || identity.gid != gid {
        anyhow::bail!("target UID/GID is invalid");
    }
    if operation.requires_write() && !writable {
        anyhow::bail!("write operation was requested through a read-only worker");
    }
    let requested_root = fs::canonicalize(root)?;
    let expected_home = fs::canonicalize(&identity.home)?;
    if requested_root != expected_home {
        anyhow::bail!("mapped file worker root must be the target account home");
    }
    let operation = serde_json::to_string(operation)?;
    let mut command = Command::new(&state.config.setpriv_path);
    command
        .arg("--reuid")
        .arg(uid.to_string())
        .arg("--regid")
        .arg(gid.to_string())
        .arg("--init-groups")
        .arg("--reset-env")
        .arg(&state.config.sessiond_path)
        .arg("files")
        .arg("--expected-uid")
        .arg(uid.to_string())
        .arg("--expected-gid")
        .arg(gid.to_string())
        .arg("--root")
        .arg(&requested_root);
    if writable {
        command.arg("--writable");
    }
    let output = command.arg("--operation").arg(operation).output().await?;
    if !output.status.success() {
        anyhow::bail!(
            "file worker failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

async fn spawn_user_worker(
    state: &State,
    uid: u32,
    gid: u32,
    worker: clouddesk_privilege::WorkerKind,
) -> anyhow::Result<serde_json::Value> {
    let identity = lookup_uid(uid)?.ok_or_else(|| anyhow::anyhow!("target UID does not exist"))?;
    if identity.gid != gid {
        anyhow::bail!("target primary GID does not match the Linux account");
    }
    if uid == 0 || gid == 0 {
        anyhow::bail!("root worker identities are forbidden");
    }

    if worker == clouddesk_privilege::WorkerKind::Terminal {
        let setpriv_path = state.config.setpriv_path.clone();
        let sessiond_path = state.config.sessiond_path.clone();
        let identity = tokio::task::spawn_blocking(move || {
            terminal_identity_probe(&setpriv_path, &sessiond_path, uid, gid)
        })
        .await??;
        return Ok(json!({ "worker": worker, "identity": identity }));
    }

    let output = Command::new(&state.config.setpriv_path)
        .arg("--reuid")
        .arg(uid.to_string())
        .arg("--regid")
        .arg(gid.to_string())
        .arg("--init-groups")
        .arg("--reset-env")
        .arg(&state.config.sessiond_path)
        .arg("identity")
        .arg("--expected-uid")
        .arg(uid.to_string())
        .arg("--expected-gid")
        .arg(gid.to_string())
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "session worker failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let identity: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(json!({ "worker": worker, "identity": identity }))
}

fn terminal_identity_probe(
    setpriv_path: &Path,
    sessiond_path: &Path,
    uid: u32,
    gid: u32,
) -> anyhow::Result<serde_json::Value> {
    let pair = native_pty_system().openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new(setpriv_path);
    command.args([
        "--reuid",
        &uid.to_string(),
        "--regid",
        &gid.to_string(),
        "--init-groups",
        "--reset-env",
    ]);
    command.arg(sessiond_path);
    command.args([
        "identity",
        "--expected-uid",
        &uid.to_string(),
        "--expected-gid",
        &gid.to_string(),
    ]);
    let mut reader = pair.master.try_clone_reader()?;
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let status = child.wait()?;
    let mut output = String::new();
    reader.read_to_string(&mut output)?;
    if !status.success() {
        anyhow::bail!("terminal session worker failed: {}", output.trim());
    }
    let normalized = output.replace('\r', "");
    Ok(serde_json::from_str(normalized.trim())?)
}

fn rejected(message: String) -> PrivdResponse {
    PrivdResponse {
        accepted: false,
        message,
        output: None,
    }
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

pub async fn request(socket_path: &Path, request: &PrivdRequest) -> anyhow::Result<PrivdResponse> {
    let mut stream = UnixStream::connect(socket_path).await?;
    let request = serde_json::to_vec(request)?;
    if request.len() > MAX_REQUEST_BYTES {
        anyhow::bail!("request is too large");
    }
    stream.write_u32(u32::try_from(request.len())?).await?;
    stream.write_all(&request).await?;
    let length = stream.read_u32().await? as usize;
    if length == 0 || length > MAX_RESPONSE_BYTES {
        anyhow::bail!("response length is invalid");
    }
    let mut response = vec![0_u8; length];
    stream.read_exact(&mut response).await?;
    Ok(serde_json::from_slice(&response)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_service_actions_have_fixed_programs_and_argument_boundaries() {
        let systemd = service_command(
            ServiceManager::Systemd,
            "ssh.service",
            ServiceOperation::Restart,
        );
        assert_eq!(systemd.program, "/usr/bin/systemctl");
        assert_eq!(
            systemd.arguments,
            ["--no-ask-password", "restart", "ssh.service"]
        );

        let openrc = service_command(
            ServiceManager::OpenRc,
            "sshd.service",
            ServiceOperation::Enable,
        );
        assert_eq!(openrc.program, "/sbin/rc-update");
        assert_eq!(openrc.arguments, ["add", "sshd", "default"]);
    }

    #[tokio::test]
    async fn clouddesk_security_services_are_protected_from_self_control() {
        let unit = ServiceUnit::new("cloudesk-privd.service").unwrap();
        let error = execute_service_control(&unit, ServiceOperation::Stop)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cannot be controlled"));
    }
}
