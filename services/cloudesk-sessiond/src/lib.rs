use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use clouddesk_linux::{lookup_uid, LinuxIdentity};
use clouddesk_privilege::{TerminalClientMessage, TerminalServerMessage};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::mpsc,
};

const MAX_TERMINAL_FRAME: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IdentitySnapshot {
    pub effective_uid: u32,
    pub effective_gid: u32,
    pub username: String,
    pub groups: Vec<u32>,
    pub home: String,
}

#[allow(clippy::similar_names)]
pub fn identity_snapshot(
    expected_uid: u32,
    expected_gid: u32,
) -> Result<IdentitySnapshot, SessionError> {
    let effective_uid = rustix::process::geteuid().as_raw();
    let effective_gid = rustix::process::getegid().as_raw();
    if effective_uid == 0 {
        return Err(SessionError::RootWorker);
    }
    if effective_uid != expected_uid || effective_gid != expected_gid {
        return Err(SessionError::IdentityMismatch {
            expected_uid,
            expected_gid,
            effective_uid,
            effective_gid,
        });
    }
    let LinuxIdentity {
        username,
        groups,
        home,
        ..
    } = lookup_uid(effective_uid)?.ok_or(SessionError::UnknownIdentity)?;
    Ok(IdentitySnapshot {
        effective_uid,
        effective_gid,
        username,
        groups,
        home: home.to_string_lossy().into_owned(),
    })
}

#[allow(clippy::similar_names)]
pub fn execute_file_operation(
    expected_uid: u32,
    expected_gid: u32,
    root: &std::path::Path,
    writable: bool,
    operation: &clouddesk_vfs::LocalFileOperation,
) -> Result<clouddesk_vfs::LocalFileResult, SessionError> {
    identity_snapshot(expected_uid, expected_gid)?;
    Ok(clouddesk_vfs::execute_local(root, writable, operation)?)
}

#[allow(clippy::similar_names)]
pub async fn serve_terminal(
    expected_uid: u32,
    expected_gid: u32,
    socket_path: &Path,
    rows: u16,
    cols: u16,
    requested_shell: Option<&str>,
) -> anyhow::Result<()> {
    let identity = identity_snapshot(expected_uid, expected_gid)?;
    validate_terminal_socket_parent(socket_path, expected_uid)?;
    let shell = select_shell(&identity, requested_shell)?;
    if fs::symlink_metadata(socket_path).is_ok() {
        anyhow::bail!("terminal socket path already exists");
    }
    let listener = UnixListener::bind(socket_path)?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
    let (stream, _) = listener.accept().await?;
    drop(listener);
    run_terminal(stream, &identity, &shell, rows, cols).await?;
    let _ = fs::remove_file(socket_path);
    if let Some(parent) = socket_path.parent() {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}

fn validate_terminal_socket_parent(path: &Path, expected_uid: u32) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("terminal socket path has no parent"))?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != expected_uid
        || metadata.permissions().mode() & 0o002 != 0
    {
        anyhow::bail!("terminal socket parent is not a safe per-user directory");
    }
    Ok(())
}

fn select_shell(identity: &IdentitySnapshot, requested: Option<&str>) -> anyhow::Result<PathBuf> {
    let default_shell = lookup_uid(identity.effective_uid)?
        .ok_or_else(|| anyhow::anyhow!("terminal identity no longer exists"))?
        .shell;
    let shell = requested.map_or(default_shell, PathBuf::from);
    let allowed = fs::read_to_string("/etc/shells").unwrap_or_default();
    let listed = allowed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .any(|line| Path::new(line) == shell);
    let executable = fs::metadata(&shell)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
    if !shell.is_absolute() || !listed || !executable {
        anyhow::bail!("requested shell is not an executable listed in /etc/shells");
    }
    Ok(shell)
}

async fn run_terminal(
    stream: UnixStream,
    identity: &IdentitySnapshot,
    shell: &Path,
    rows: u16,
    cols: u16,
) -> anyhow::Result<()> {
    let pair = native_pty_system().openpty(PtySize {
        rows: rows.clamp(2, 500),
        cols: cols.clamp(2, 500),
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new(shell);
    command.cwd(&identity.home);
    command.env("HOME", &identity.home);
    command.env("USER", &identity.username);
    command.env("LOGNAME", &identity.username);
    command.env("SHELL", shell);
    command.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let mut killer = child.clone_killer();
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let master = pair.master;
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(32);
    let reader_task = tokio::task::spawn_blocking(move || {
        let mut buffer = vec![0_u8; 32 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 || output_tx.blocking_send(buffer[..count].to_vec()).is_err() {
                return Ok::<(), std::io::Error>(());
            }
        }
    });
    let mut wait_task = tokio::task::spawn_blocking(move || child.wait());
    let (mut socket_reader, mut socket_writer) = stream.into_split();

    loop {
        tokio::select! {
            incoming = read_terminal_frame::<_, TerminalClientMessage>(&mut socket_reader) => {
                match incoming {
                    Ok(TerminalClientMessage::Data { data }) => {
                        if data.len() > MAX_TERMINAL_FRAME {
                            anyhow::bail!("terminal input frame is too large");
                        }
                        writer.write_all(&data)?;
                        writer.flush()?;
                    }
                    Ok(TerminalClientMessage::Resize { rows, cols }) => {
                        master.resize(PtySize {
                            rows: rows.clamp(2, 500),
                            cols: cols.clamp(2, 500),
                            pixel_width: 0,
                            pixel_height: 0,
                        })?;
                    }
                    Ok(TerminalClientMessage::Close) | Err(_) => {
                        let _ = killer.kill();
                        break;
                    }
                }
            }
            output = output_rx.recv() => {
                if let Some(data) = output {
                    write_terminal_frame(&mut socket_writer, &TerminalServerMessage::Output { data }).await?;
                } else {
                    let status = (&mut wait_task).await??;
                    write_terminal_frame(
                        &mut socket_writer,
                        &TerminalServerMessage::Exit { code: status.exit_code() },
                    ).await?;
                    break;
                }
            }
            status = &mut wait_task => {
                let status = status??;
                write_terminal_frame(
                    &mut socket_writer,
                    &TerminalServerMessage::Exit { code: status.exit_code() },
                ).await?;
                break;
            }
        }
    }
    reader_task.abort();
    wait_task.abort();
    Ok(())
}

async fn read_terminal_frame<R, T>(reader: &mut R) -> anyhow::Result<T>
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let length = reader.read_u32().await? as usize;
    if length == 0 || length > MAX_TERMINAL_FRAME {
        anyhow::bail!("terminal protocol frame is invalid");
    }
    let mut frame = vec![0_u8; length];
    reader.read_exact(&mut frame).await?;
    Ok(serde_json::from_slice(&frame)?)
}

async fn write_terminal_frame<W, T>(writer: &mut W, value: &T) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let frame = serde_json::to_vec(value)?;
    if frame.len() > MAX_TERMINAL_FRAME {
        anyhow::bail!("terminal protocol frame is too large");
    }
    writer.write_u32(u32::try_from(frame.len())?).await?;
    writer.write_all(&frame).await?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("cloudesk-sessiond must never execute as root")]
    RootWorker,
    #[error(
        "worker identity mismatch: expected {expected_uid}:{expected_gid}, got {effective_uid}:{effective_gid}"
    )]
    IdentityMismatch {
        expected_uid: u32,
        expected_gid: u32,
        effective_uid: u32,
        effective_gid: u32,
    },
    #[error("worker UID does not resolve to a Linux identity")]
    UnknownIdentity,
    #[error("Linux identity lookup failed: {0}")]
    Linux(#[from] clouddesk_linux::LinuxError),
    #[error("local file operation failed: {0}")]
    Vfs(#[from] clouddesk_vfs::VfsError),
}
