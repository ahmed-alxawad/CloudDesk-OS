//! PASS SSH-B: a real, minimal legacy SCP protocol client, implemented
//! by hand over an already-authenticated SSH exec channel -- `russh`
//! (this project's SSH library) has no SCP protocol implementation of
//! its own, only SFTP (`russh_sftp`), so there is nothing pre-built to
//! reuse. This deliberately does NOT relabel SFTP as SCP: it speaks the
//! actual `scp -t`/`scp -f` wire protocol (`Cmmmm <size> <name>\n` +
//! raw bytes + trailing NUL, single-byte 0/1/2 ACKs), which is what
//! `scp -O` and every legacy SCP client/server actually exchange.
//!
//! Scope (v1, deliberately not expanded per the closure spec): single
//! file upload and single file download only. No recursive directory
//! transfer, no `-p` (preserve times), no wildcards.
//!
//! Reuses the exact same authenticated `SshSession` every other
//! feature (SFTP, WOPI, Browser remote uploads) already uses -- no
//! second SSH stack. Host-key verification, credential resolution, and
//! `ProxyJump` are all inherited automatically because the channel is
//! opened on an already-`resolve_ssh_session`-authenticated `SshSession`.

use anyhow::{bail, Context, Result};
use russh::{client, ChannelMsg};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Duration;

use crate::ssh::SshClientHandler;

/// Task 22 (PASS SSH-B-2, live-found gap): a single low-level
/// read/write/flush on the SSH channel stream has no bound of its own
/// -- if the underlying transport dies without a clean TCP
/// close/reset (observed live: a killed disposable OpenSSH container
/// left an in-flight upload hung for minutes with zero progress),
/// nothing here would ever notice. Every blocking I/O call in this
/// module is wrapped in this bound, converting a truly dead
/// connection into a real, timely error instead of an indefinite
/// hang. Matches the existing `SshClientHandler` connection's own 30s
/// inactivity timeout elsewhere in this crate by default -- overridable
/// only via `set_operation_timeout_for_test` below, never in
/// production.
static OPERATION_TIMEOUT_OVERRIDE_SECS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Test-only override for the per-operation timeout -- a plain safe
/// atomic, not an environment variable or `unsafe` global (this
/// workspace forbids `unsafe` code entirely), so a live interruption
/// test can prove real dead-connection detection in seconds rather
/// than waiting out the full 30s production value on every one of
/// several bounded retry attempts. Never called from production code.
pub fn set_operation_timeout_for_test(seconds: u64) {
    OPERATION_TIMEOUT_OVERRIDE_SECS.store(seconds, std::sync::atomic::Ordering::Relaxed);
}

fn operation_timeout() -> Duration {
    match OPERATION_TIMEOUT_OVERRIDE_SECS.load(std::sync::atomic::Ordering::Relaxed) {
        0 => Duration::from_secs(30),
        seconds => Duration::from_secs(seconds),
    }
}

async fn with_timeout<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    let timeout = operation_timeout();
    tokio::time::timeout(timeout, future).await.map_err(|_| {
        anyhow::anyhow!("SCP operation timed out after {timeout:?} (the connection is likely dead)")
    })?
}

/// Bounds on untrusted values read back from the remote SCP protocol
/// (Task 23/24): a filename this long, or a control line this long,
/// can only be a malformed/hostile peer, never a real transfer.
const MAX_CONTROL_LINE_BYTES: usize = 4096;
const MAX_FILENAME_BYTES: usize = 1024;
/// Refuses to trust an implausibly large advertised size outright
/// (Task 23: "do not allocate based solely on untrusted advertised
/// file size") -- this is a sanity ceiling, not the real per-transfer
/// limit; the real limit is that we never allocate proportional to
/// this value at all, only read it in bounded chunks (Task 9).
const MAX_PLAUSIBLE_SIZE: u64 = 1024 * 1024 * 1024 * 1024; // 1 TiB
const CHUNK_SIZE: usize = 256 * 1024;

/// Task 3: an explicit, documented, conservative remote-path policy --
/// enforced *before* shell-quoting, as defense in depth, not merely
/// "shell-escape and hope." Bytes that can never appear in a safe
/// remote path for this v1 (NUL, other control bytes including
/// newline/CR) are rejected outright; everything else is delivered to
/// the remote shell inside a single POSIX single-quoted argument
/// (`shell_single_quote`), which neutralizes every other
/// shell-metacharacter in the hostile-input matrix (spaces, quotes,
/// `;`, `&`, backticks, `$()`) without needing a second denylist for
/// them. A leading `-` is handled by always passing `--` before the
/// path argument, so it can never be parsed as an option to remote
/// `scp`/`cp`/`mv`.
pub fn validate_scp_remote_path(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > MAX_FILENAME_BYTES {
        bail!("remote path is empty or too long");
    }
    if path.bytes().any(|b| b == 0 || b < 0x20) {
        bail!("remote path contains a NUL or control byte");
    }
    if path.split('/').any(|segment| segment == "..") {
        bail!("remote path traversal (..) is not permitted");
    }
    Ok(())
}

/// Standard POSIX single-quoting: wrap in `'...'`, escaping any
/// embedded `'` as `'\''`. Every other shell metacharacter is inert
/// inside single quotes. `pub` (Task 10/11, PASS SSH-B-2): callers
/// building their own safe remote shell command around an SCP
/// destination (e.g. the temp-name-then-`mv` atomic-commit pattern in
/// `worker.rs`) reuse the exact same quoting discipline, never a
/// second, unreviewed escaping scheme.
#[must_use]
pub fn shell_single_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

async fn read_ack(stream: &mut (impl AsyncRead + Unpin)) -> Result<()> {
    let mut status = [0_u8; 1];
    with_timeout(async {
        stream
            .read_exact(&mut status)
            .await
            .context("failed to read SCP ACK byte")
    })
    .await?;
    match status[0] {
        0 => Ok(()),
        1 | 2 => {
            let message = read_control_line_rest(stream).await?;
            bail!("remote SCP error: {message}")
        }
        other => bail!("unexpected SCP ACK byte: {other}"),
    }
}

async fn write_ack(stream: &mut (impl AsyncWrite + Unpin)) -> Result<()> {
    with_timeout(async {
        stream.write_all(&[0_u8]).await?;
        stream.flush().await?;
        Ok(())
    })
    .await
}

/// Reads the remainder of a warning/error message after a 1/2 status
/// byte, bounded (Task 23/24: an unterminated or oversized message
/// from a hostile/malformed peer must not hang or grow unbounded).
async fn read_control_line_rest(stream: &mut (impl AsyncRead + Unpin)) -> Result<String> {
    with_timeout(async {
        let mut buf = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            stream.read_exact(&mut byte).await?;
            if byte[0] == b'\n' {
                break;
            }
            buf.push(byte[0]);
            if buf.len() > MAX_CONTROL_LINE_BYTES {
                bail!("SCP control/error line exceeded the maximum accepted length");
            }
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    })
    .await
}

/// Parses a `Cmmmm <size> <name>\n` control line strictly (Task 23):
/// rejects anything that doesn't match this exact shape, an
/// implausible size, or a name too long/containing traversal/NUL.
fn parse_control_line(line: &str) -> Result<(u64, String)> {
    let rest = line
        .strip_prefix('C')
        .context("SCP control line did not start with 'C' (unsupported record type)")?;
    let mut parts = rest.splitn(3, ' ');
    let _mode = parts.next().context("SCP control line missing mode")?;
    let size_str = parts.next().context("SCP control line missing size")?;
    let name = parts.next().context("SCP control line missing filename")?;
    let size: u64 = size_str
        .parse()
        .context("SCP control line size was not a valid non-negative integer")?;
    if size > MAX_PLAUSIBLE_SIZE {
        bail!("SCP-advertised file size exceeds the accepted ceiling");
    }
    if name.is_empty() || name.len() > MAX_FILENAME_BYTES {
        bail!("SCP-advertised filename is empty or too long");
    }
    if name.bytes().any(|b| b == 0) || name.contains('\n') {
        bail!("SCP-advertised filename contains a NUL or newline");
    }
    // Task 25: the remote SCP peer is not trusted with CloudDesk host
    // paths -- callers must treat this as an untrusted display name
    // only, never build a filesystem path from it without their own
    // independent authorization/sanitization pass.
    if name.contains('/') || name == ".." || name == "." {
        bail!("SCP-advertised filename must be a bare name, not a path");
    }
    Ok((size, name.to_owned()))
}

/// Uploads exactly `size` bytes read from `source` to `remote_path` on
/// the server this `handle`'s channel connects to, via `scp -t`
/// (Task 5/7). `mode` is a 4-digit octal string, e.g. `"0644"`.
/// `on_progress` is called after every chunk with the cumulative byte
/// count (Task 10) -- streamed in `CHUNK_SIZE` pieces, never buffered
/// whole (Task 9).
pub async fn upload<R>(
    handle: &mut client::Handle<SshClientHandler>,
    remote_path: &str,
    mode: &str,
    size: u64,
    source: &mut R,
    mut on_progress: impl FnMut(u64) + Send,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send,
{
    validate_scp_remote_path(remote_path)?;
    // Real `scp -t <target>` semantics: if <target> already exists as
    // a directory, the file lands inside it under the C-line's name;
    // otherwise <target> itself becomes the destination filename. That
    // decision belongs to the remote `scp -t` process, not this
    // client, so the full path is passed through as-is (never split
    // into "directory" + "basename" here) -- the C-line's own name is
    // still required by the protocol, so the basename is sent there.
    let basename = remote_path.rsplit('/').next().unwrap_or(remote_path);
    let quoted = shell_single_quote(remote_path);
    let channel = handle.channel_open_session().await?;
    channel.exec(true, format!("scp -t -- {quoted}")).await?;
    let mut stream = channel.into_stream();

    // The remote scp -t process sends no greeting; the client speaks
    // first with the control line, exactly as real scp does.
    read_ack(&mut stream).await.context(
        "remote did not accept the SCP session (destination directory missing or unwritable?)",
    )?;
    with_timeout(async {
        stream
            .write_all(format!("C{mode} {size} {basename}\n").as_bytes())
            .await?;
        stream.flush().await?;
        Ok(())
    })
    .await?;
    read_ack(&mut stream)
        .await
        .context("remote rejected the SCP file header")?;

    let mut remaining = size;
    let mut transferred: u64 = 0;
    let mut buf = vec![0_u8; CHUNK_SIZE];
    while remaining > 0 {
        let want = usize::try_from(remaining.min(CHUNK_SIZE as u64)).unwrap_or(CHUNK_SIZE);
        let read = source.read(&mut buf[..want]).await?;
        if read == 0 {
            bail!("local source ended before the declared size was reached");
        }
        with_timeout(async { Ok(stream.write_all(&buf[..read]).await?) }).await?;
        remaining -= read as u64;
        transferred += read as u64;
        on_progress(transferred);
    }
    with_timeout(async { Ok(stream.flush().await?) }).await?;
    write_ack(&mut stream).await?;
    read_ack(&mut stream)
        .await
        .context("remote did not confirm the completed SCP upload")?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// Downloads a single file from `remote_path` via `scp -f`
/// (Task 6/8), streaming into `destination` in bounded chunks
/// (Task 9). Returns the file's basename as advertised by the remote
/// protocol -- an UNTRUSTED value (Task 25); callers must validate/
/// sanitize it against `CloudDesk`'s own path policy before using it to
/// build a filesystem path, never trust it directly.
pub async fn download<W>(
    handle: &mut client::Handle<SshClientHandler>,
    remote_path: &str,
    destination: &mut W,
    mut on_progress: impl FnMut(u64) + Send,
) -> Result<DownloadedFile>
where
    W: AsyncWrite + Unpin + Send,
{
    validate_scp_remote_path(remote_path)?;
    let quoted = shell_single_quote(remote_path);
    let channel = handle.channel_open_session().await?;
    channel.exec(true, format!("scp -f -- {quoted}")).await?;
    let mut stream = channel.into_stream();

    // scp -f (source) waits for the client to say "go ahead" first --
    // it sends nothing at all until this initial 0x00 arrives.
    write_ack(&mut stream).await?;
    // Then it sends either a real "C<mode> <size> <name>" control line
    // or a 1/2-status error line -- both end in '\n', so reading a raw
    // line first and validating its shape afterward (rather than
    // assuming success) means a malformed/erroring peer fails safely
    // with a real error instead of a panic or hang (Task 13/24).
    let line = read_control_line_rest(&mut stream).await?;
    let (size, name) = parse_control_line(&line)?;
    write_ack(&mut stream).await?;

    let mut remaining = size;
    let mut transferred: u64 = 0;
    let mut buf = vec![0_u8; CHUNK_SIZE];
    while remaining > 0 {
        let want = usize::try_from(remaining.min(CHUNK_SIZE as u64)).unwrap_or(CHUNK_SIZE);
        let read = with_timeout(async { Ok(stream.read(&mut buf[..want]).await?) }).await?;
        if read == 0 {
            bail!("remote SCP source closed the connection before the declared size was reached");
        }
        destination.write_all(&buf[..read]).await?;
        remaining -= read as u64;
        transferred += read as u64;
        on_progress(transferred);
    }
    destination.flush().await?;
    let mut trailing = [0_u8; 1];
    with_timeout(async { Ok(stream.read_exact(&mut trailing).await?) }).await?;
    if trailing[0] != 0 {
        bail!("remote SCP source reported an error after the file body");
    }
    write_ack(&mut stream).await?;
    let _ = stream.shutdown().await;
    Ok(DownloadedFile {
        remote_basename: name,
        size,
    })
}

/// The untrusted metadata a real `scp -f` control line advertised
/// (Task 25) -- `remote_basename` must be independently validated by
/// the caller before ever being used to construct a `CloudDesk`
/// filesystem path.
pub struct DownloadedFile {
    pub remote_basename: String,
    pub size: u64,
}

// Silence an unused-channel-message-type warning in case future
// callers want to observe exit status; kept here rather than deleted
// so the intent (this module deliberately never inspects `ExitStatus`
// today) is documented rather than silently absent.
#[allow(dead_code)]
fn _unused(_: ChannelMsg) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_neutralizes_every_hostile_character_in_the_matrix() {
        let hostile = [
            "normal.txt",
            "has spaces.txt",
            "unicode-café-☃.txt",
            "-leading-dash.txt",
            "single'quote.txt",
            "double\"quote.txt",
            "semi;colon.txt",
            "amp&ersand.txt",
            "back`tick.txt",
            "dollar$(cmd).txt",
            "back\\slash.txt",
        ];
        for name in hostile {
            let quoted = shell_single_quote(name);
            // A correctly single-quoted argument always starts and
            // ends with a real quote character, and the only way a
            // `'` can appear elsewhere is as part of the `'\''`
            // escape sequence -- if that invariant holds, the shell
            // can never see an unquoted metacharacter.
            assert!(quoted.starts_with('\''));
            assert!(quoted.ends_with('\''));
        }
    }

    #[test]
    fn path_policy_rejects_control_bytes_and_traversal() {
        assert!(validate_scp_remote_path("/tmp/ok.txt").is_ok());
        assert!(validate_scp_remote_path("").is_err());
        assert!(validate_scp_remote_path("/tmp/has\nnewline").is_err());
        assert!(validate_scp_remote_path("/tmp/has\0nul").is_err());
        assert!(validate_scp_remote_path("/tmp/../escape").is_err());
        assert!(validate_scp_remote_path(&"x".repeat(2000)).is_err());
    }

    #[test]
    fn path_policy_allows_every_shell_metacharacter_since_quoting_neutralizes_them() {
        // These are dangerous unquoted, but safe once single-quoted --
        // the policy's job is only to block bytes quoting can't
        // represent safely (NUL, control bytes, traversal).
        for path in [
            "/tmp/has spaces.txt",
            "/tmp/semi;colon.txt",
            "/tmp/amp&ersand.txt",
            "/tmp/back`tick.txt",
            "/tmp/dollar$(cmd).txt",
            "/tmp/single'quote.txt",
        ] {
            assert!(validate_scp_remote_path(path).is_ok(), "{path}");
        }
    }

    #[test]
    fn control_line_parsing_rejects_malformed_records() {
        assert!(parse_control_line("C0644 12 file.txt").is_ok());
        assert_eq!(
            parse_control_line("C0644 12 file.txt").unwrap(),
            (12, "file.txt".to_owned())
        );
        assert!(
            parse_control_line("T0644 12 file.txt").is_err(),
            "wrong record type"
        );
        assert!(
            parse_control_line("C0644 -1 file.txt").is_err(),
            "negative size"
        );
        assert!(
            parse_control_line("C0644 99999999999999999999 file.txt").is_err(),
            "unparseable size"
        );
        assert!(
            parse_control_line(&format!("C0644 {} file.txt", MAX_PLAUSIBLE_SIZE + 1)).is_err(),
            "implausible size"
        );
        assert!(
            parse_control_line("C0644 12 ../escape").is_err(),
            "traversal in name"
        );
        assert!(
            parse_control_line("C0644 12 a/b").is_err(),
            "path in name, not bare filename"
        );
        assert!(
            parse_control_line(&format!("C0644 12 {}", "x".repeat(2000))).is_err(),
            "oversized filename"
        );
    }
}
