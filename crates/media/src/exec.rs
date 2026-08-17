//! Bounded `FFmpeg` remux/transcode execution.
//!
//! Every invocation uses a fixed argv built entirely from paths this
//! process generated itself (the job's own temp directory, `output.mp4`)
//! plus a source path the caller already resolved and authorized. No user
//! input is ever concatenated into a command string or accepted as a raw
//! flag — `TranscodeOptions`/the remux path expose no free-form escape
//! hatch.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

/// Wall-clock ceiling for a single remux/transcode job. Chosen generously
/// for a media-server workload; not user-configurable, since letting a
/// request extend its own timeout would defeat the point.
pub const JOB_TIMEOUT: Duration = Duration::from_mins(10);
/// Stop killing the process kindly and escalate to SIGKILL after this long.
const GRACEFUL_SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
/// Stderr is diagnostic only; bound it so a chatty/hostile `ffmpeg` build
/// can't grow our memory unbounded.
const MAX_STDERR_BYTES: usize = 64 * 1024;
/// Reject an output file that somehow grew past this while running --
/// catches a runaway/hostile encode before it fills the disk.
pub const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const OUTPUT_SIZE_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("ffmpeg is not available")]
    Unavailable,
    #[error("failed to launch ffmpeg: {0}")]
    Spawn(String),
    #[error("job was cancelled")]
    Cancelled,
    #[error("job exceeded its time limit")]
    Timeout,
    #[error("output exceeded the size limit")]
    OutputTooLarge,
    #[error("ffmpeg exited with a failure status: {stderr_tail}")]
    ExitedWithFailure { stderr_tail: String },
    #[error("ffmpeg produced output that does not probe as valid media")]
    InvalidOutput,
    #[error("failed to prepare the job workspace: {0}")]
    Workspace(String),
}

pub struct RunOutcome {
    pub output_path: PathBuf,
    pub stderr_tail: String,
}

/// Typed, bounded transcode profile. There is deliberately no field that
/// accepts a raw `ffmpeg` flag or argument string.
#[derive(Clone, Copy, Debug)]
pub struct TranscodeOptions {
    /// Video is scaled down (never up) so its height does not exceed this.
    pub max_height: u32,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
}

impl Default for TranscodeOptions {
    fn default() -> Self {
        Self {
            max_height: 1080,
            video_bitrate_kbps: 4000,
            audio_bitrate_kbps: 160,
        }
    }
}

/// Creates an unpredictable, restrictively-permissioned per-job directory
/// under `cache_root`. `job_id` already comes from
/// `clouddesk_auth::random_identifier`, so the directory name itself is
/// not guessable; permissions further ensure no other local user can read
/// in-progress or finished output.
pub fn job_workspace(cache_root: &Path, job_id: &str) -> Result<PathBuf, ExecError> {
    let dir = cache_root.join(job_id);
    std::fs::create_dir_all(&dir).map_err(|e| ExecError::Workspace(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| ExecError::Workspace(e.to_string()))?;
    }
    Ok(dir)
}

pub fn cleanup_workspace(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

async fn drain_stderr_bounded(mut stderr: tokio::process::ChildStderr) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < MAX_STDERR_BYTES {
                    let remaining = MAX_STDERR_BYTES - buf.len();
                    buf.extend_from_slice(&chunk[..n.min(remaining)]);
                }
                // else: keep reading to drain the pipe (avoid ffmpeg
                // blocking on a full stderr buffer) but discard the bytes.
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Sends SIGTERM, waits `GRACEFUL_SHUTDOWN_GRACE` for a clean exit, and
/// falls back to SIGKILL (via `Child::start_kill`) if the process ignores
/// the polite request. Real `ffmpeg` honors SIGTERM, but this cannot rely
/// on that -- a hostile or hung process must still die.
async fn terminate(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap_or(i32::MAX));
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
        if timeout(GRACEFUL_SHUTDOWN_GRACE, child.wait()).await.is_ok() {
            return;
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

async fn watch_output_size(path: PathBuf, stop: CancellationToken) {
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(OUTPUT_SIZE_POLL_INTERVAL) => {}
        }
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            if meta.len() > MAX_OUTPUT_BYTES {
                stop.cancel();
                return;
            }
        }
    }
}

async fn run_ffmpeg(
    ffmpeg_path: &str,
    args: &[String],
    output_path: &Path,
    cancel: CancellationToken,
) -> Result<RunOutcome, ExecError> {
    let mut child = Command::new(ffmpeg_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ExecError::Spawn(e.to_string()))?;

    let stderr = child.stderr.take();
    let stderr_task = stderr.map(|s| tokio::spawn(drain_stderr_bounded(s)));

    let size_guard = CancellationToken::new();
    let size_watcher = tokio::spawn(watch_output_size(
        output_path.to_path_buf(),
        size_guard.clone(),
    ));

    let outcome = tokio::select! {
        status = timeout(JOB_TIMEOUT, child.wait()) => {
            match status {
                Ok(Ok(status)) if status.success() => Ok(()),
                Ok(Ok(_)) => Err(ExecError::ExitedWithFailure { stderr_tail: String::new() }),
                Ok(Err(e)) => Err(ExecError::Spawn(e.to_string())),
                Err(_) => Err(ExecError::Timeout),
            }
        }
        () = cancel.cancelled() => Err(ExecError::Cancelled),
        () = size_guard.cancelled() => Err(ExecError::OutputTooLarge),
    };

    size_guard.cancel();
    let _ = size_watcher.await;

    if outcome.is_err() {
        terminate(&mut child).await;
    }

    let stderr_tail = if let Some(task) = stderr_task {
        task.await.unwrap_or_default()
    } else {
        String::new()
    };

    match outcome {
        Ok(()) => Ok(RunOutcome {
            output_path: output_path.to_path_buf(),
            stderr_tail,
        }),
        Err(ExecError::ExitedWithFailure { .. }) => {
            Err(ExecError::ExitedWithFailure { stderr_tail })
        }
        Err(other) => Err(other),
    }
}

/// Remuxes `source` (a real, already-authorized filesystem path) into an
/// MP4 container in the job's own workspace, copying streams without
/// re-encoding. `-movflags +faststart` moves the MP4 index to the front
/// so the output is progressively playable/seekable as soon as it exists,
/// rather than requiring the whole file before the moov atom is readable.
pub async fn remux(
    ffmpeg_path: &str,
    source: &Path,
    workspace: &Path,
    cancel: CancellationToken,
) -> Result<RunOutcome, ExecError> {
    let output_path = workspace.join("output.mp4");
    let args = vec![
        "-y".to_owned(),
        "-i".to_owned(),
        source.to_string_lossy().into_owned(),
        "-c".to_owned(),
        "copy".to_owned(),
        "-movflags".to_owned(),
        "+faststart".to_owned(),
        output_path.to_string_lossy().into_owned(),
    ];
    run_ffmpeg(ffmpeg_path, &args, &output_path, cancel).await
}

/// Transcodes `source` into a browser-compatible H.264/AAC MP4 using a
/// fixed, bounded profile (`options`) -- never raw user-supplied flags.
pub async fn transcode(
    ffmpeg_path: &str,
    source: &Path,
    workspace: &Path,
    options: TranscodeOptions,
    cancel: CancellationToken,
) -> Result<RunOutcome, ExecError> {
    let output_path = workspace.join("output.mp4");
    let scale = format!("scale=-2:'min({},ih)'", options.max_height);
    let args = vec![
        "-y".to_owned(),
        "-i".to_owned(),
        source.to_string_lossy().into_owned(),
        "-vf".to_owned(),
        scale,
        "-c:v".to_owned(),
        "libx264".to_owned(),
        "-b:v".to_owned(),
        format!("{}k", options.video_bitrate_kbps),
        "-c:a".to_owned(),
        "aac".to_owned(),
        "-b:a".to_owned(),
        format!("{}k", options.audio_bitrate_kbps),
        "-movflags".to_owned(),
        "+faststart".to_owned(),
        output_path.to_string_lossy().into_owned(),
    ];
    run_ffmpeg(ffmpeg_path, &args, &output_path, cancel).await
}

/// Global + per-user concurrency guard. `ffmpeg` is CPU-heavy; without
/// this a handful of concurrent transcode requests can starve the whole
/// server. This is process-level admission control, not a cgroup/kernel
/// resource limit -- see `V1_TRUE_CLOSURE.md` / the engineering checkpoint
/// for exactly what is and isn't enforced at the OS level.
pub struct JobLimiter {
    global: Arc<tokio::sync::Semaphore>,
    per_user_max: u32,
    active_by_user: Arc<tokio::sync::Mutex<std::collections::HashMap<String, u32>>>,
}

pub struct JobPermit {
    _global: tokio::sync::OwnedSemaphorePermit,
    user_id: String,
    active_by_user: Arc<tokio::sync::Mutex<std::collections::HashMap<String, u32>>>,
}

impl Drop for JobPermit {
    fn drop(&mut self) {
        let map = self.active_by_user.clone();
        let user_id = self.user_id.clone();
        tokio::spawn(async move {
            let mut guard = map.lock().await;
            if let Some(count) = guard.get_mut(&user_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    guard.remove(&user_id);
                }
            }
        });
    }
}

#[derive(Debug, thiserror::Error)]
#[error("too many concurrent media jobs")]
pub struct LimiterFull;

impl JobLimiter {
    #[must_use]
    pub fn new(global_max: usize, per_user_max: u32) -> Self {
        Self {
            global: Arc::new(tokio::sync::Semaphore::new(global_max)),
            per_user_max,
            active_by_user: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub async fn acquire(&self, user_id: &str) -> Result<JobPermit, LimiterFull> {
        {
            let mut guard = self.active_by_user.lock().await;
            let count = guard.entry(user_id.to_owned()).or_insert(0);
            if *count >= self.per_user_max {
                return Err(LimiterFull);
            }
            *count += 1;
        }
        let Ok(permit) = self.global.clone().try_acquire_owned() else {
            let mut guard = self.active_by_user.lock().await;
            if let Some(count) = guard.get_mut(user_id) {
                *count = count.saturating_sub(1);
            }
            return Err(LimiterFull);
        };
        Ok(JobPermit {
            _global: permit,
            user_id: user_id.to_owned(),
            active_by_user: self.active_by_user.clone(),
        })
    }
}
