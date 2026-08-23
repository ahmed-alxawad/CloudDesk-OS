//! Shared media compatibility foundation: `FFmpeg` discovery, `ffprobe`
//! metadata, a browser-compatibility decision engine, and a bounded
//! remux/transcode job lifecycle. Video/Music applications consume this
//! crate rather than shelling out to `ffmpeg` themselves.

pub mod compat;
pub mod exec;
pub mod ffmpeg;
pub mod jobs;
pub mod probe;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub use compat::StreamPlan;
pub use ffmpeg::FfmpegAvailability;
pub use jobs::{JobOperation, JobState, MediaJob, MediaJobStore};
pub use probe::{MediaProbe, ProbeError};

/// Default global concurrency: `ffmpeg` transcodes are CPU-bound, and this
/// keeps a handful of simultaneous requests from starving the rest of the
/// server's async runtime. Not derived from measured hardware -- a fixed,
/// conservative default, documented as such rather than presented as
/// tuned capacity planning.
pub const DEFAULT_GLOBAL_CONCURRENCY: usize = 4;
pub const DEFAULT_PER_USER_CONCURRENCY: u32 = 2;

/// Live registry of cancellation handles for jobs currently running in
/// this process. Separate from `MediaJobStore` (which is the durable
/// source of truth for job *state*) because a `CancellationToken` cannot
/// be persisted or reconstructed after a restart -- a job whose token is
/// gone because the process restarted is exactly what
/// `MediaJobStore::expire_stale` reconciles on startup.
#[derive(Clone, Default)]
pub struct JobRegistry {
    tokens: Arc<tokio::sync::Mutex<std::collections::HashMap<String, CancellationToken>>>,
}

impl JobRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, job_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        self.tokens
            .lock()
            .await
            .insert(job_id.to_owned(), token.clone());
        token
    }

    pub async fn unregister(&self, job_id: &str) {
        self.tokens.lock().await.remove(job_id);
    }

    /// Returns `true` if a live job was found and told to cancel. `false`
    /// means either the job doesn't exist, isn't this process's to
    /// cancel, or has already finished -- callers must not treat `false`
    /// as an error; the job may simply already be terminal.
    pub async fn cancel(&self, job_id: &str) -> bool {
        if let Some(token) = self.tokens.lock().await.get(job_id) {
            token.cancel();
            true
        } else {
            false
        }
    }
}

#[derive(Clone)]
pub struct MediaService {
    availability: FfmpegAvailability,
    store: MediaJobStore,
    registry: JobRegistry,
    limiter: Arc<exec::JobLimiter>,
    cache_root: PathBuf,
    limits: exec::MediaLimits,
}

#[derive(Debug, thiserror::Error)]
pub enum MediaServiceError {
    #[error("media/FFmpeg support is disabled or unavailable")]
    Unavailable,
    #[error("too many concurrent media jobs; try again shortly")]
    Busy,
    #[error(transparent)]
    Probe(#[from] ProbeError),
    #[error("job not found")]
    NotFound,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Exec(#[from] exec::ExecError),
    #[error("failed to prepare a workspace: {0}")]
    Workspace(String),
}

impl MediaService {
    #[must_use]
    pub fn new(
        availability: FfmpegAvailability,
        pool: sqlx::SqlitePool,
        cache_root: PathBuf,
    ) -> Self {
        Self {
            availability,
            store: MediaJobStore::new(pool),
            registry: JobRegistry::new(),
            limiter: Arc::new(exec::JobLimiter::new(
                DEFAULT_GLOBAL_CONCURRENCY,
                DEFAULT_PER_USER_CONCURRENCY,
            )),
            cache_root,
            limits: exec::MediaLimits::default(),
        }
    }

    /// Test-only knob (Phase 3 residual closure, Task 1): overrides the
    /// job timeout/output-quota limits real jobs run under, so a test can
    /// exercise the SAME production timeout/quota-enforcement code path
    /// (`exec::run_ffmpeg`) with a reduced threshold instead of a
    /// separate "test implementation" of either limit. No HTTP route
    /// accepts a caller-supplied override (Task 23) -- production always
    /// constructs `MediaService` via `new`, which uses
    /// `MediaLimits::default()` (the real 10-minute/4 GiB values).
    #[must_use]
    pub fn with_limits(mut self, limits: exec::MediaLimits) -> Self {
        self.limits = limits;
        self
    }

    #[must_use]
    pub fn availability(&self) -> &FfmpegAvailability {
        &self.availability
    }

    #[must_use]
    pub fn store(&self) -> &MediaJobStore {
        &self.store
    }

    /// Probes `source` (already resolved + authorized by the caller) and
    /// returns both the raw metadata and the resulting compatibility
    /// decision.
    pub async fn probe(
        &self,
        source: &Path,
    ) -> Result<(MediaProbe, StreamPlan), MediaServiceError> {
        let FfmpegAvailability::Available { ffprobe, .. } = &self.availability else {
            return Err(MediaServiceError::Unavailable);
        };
        let probe = Box::pin(probe::probe_media(&ffprobe.path, source)).await?;
        let plan = compat::decide(&probe);
        Ok((probe, plan))
    }

    /// Starts a remux or transcode job for `owner_user_id` against
    /// `source`, subject to the concurrency limiter. Runs the `ffmpeg`
    /// process in a background task and updates the persisted job row as
    /// it progresses -- callers poll `MediaJobStore::get` for status
    /// rather than awaiting this call. `selection` optionally pins the
    /// job to a single audio track (see `exec::TrackSelection`); the
    /// track chosen is not persisted on the job row (only the operation/
    /// state/output path are), so it isn't recoverable from a restarted
    /// job's row -- a documented gap, not a security issue, since the
    /// output file itself is unaffected by that limitation.
    pub async fn start_job(
        &self,
        owner_user_id: &str,
        source_virtual_path: &str,
        source: PathBuf,
        operation: JobOperation,
        selection: exec::TrackSelection,
    ) -> Result<MediaJob, MediaServiceError> {
        let FfmpegAvailability::Available { ffmpeg, ffprobe } = &self.availability else {
            return Err(MediaServiceError::Unavailable);
        };
        let permit = self
            .limiter
            .acquire(owner_user_id)
            .await
            .map_err(|_| MediaServiceError::Busy)?;

        let job = self
            .store
            .create(owner_user_id, source_virtual_path, operation)
            .await?;
        let token = self.registry.register(&job.id).await;

        let ffmpeg_path = ffmpeg.path.clone();
        let ffprobe_path = ffprobe.path.clone();
        let store = self.store.clone();
        let registry = self.registry.clone();
        let cache_root = self.cache_root.clone();
        let job_id = job.id.clone();
        let limits = self.limits;

        tokio::spawn(async move {
            let _permit = permit;
            let _ = store.set_state(&job_id, JobState::Running, None).await;

            let outcome = async {
                let workspace = exec::job_workspace(&cache_root, &job_id)
                    .map_err(|e| exec::ExecError::Workspace(e.to_string()))?;
                let run = match operation {
                    JobOperation::Remux => {
                        exec::remux(
                            &ffmpeg_path,
                            &ffprobe_path,
                            &source,
                            &workspace,
                            selection,
                            limits,
                            token.clone(),
                        )
                        .await
                    }
                    JobOperation::Transcode => {
                        exec::transcode(
                            &ffmpeg_path,
                            &source,
                            &workspace,
                            exec::TranscodeOptions::default(),
                            selection,
                            limits,
                            token.clone(),
                        )
                        .await
                    }
                }?;
                Ok::<_, exec::ExecError>(run)
            }
            .await;

            match outcome {
                Ok(run) => {
                    let _ = store
                        .set_output(&job_id, &run.output_path.to_string_lossy())
                        .await;
                    let _ = store.set_state(&job_id, JobState::Completed, None).await;
                }
                Err(exec::ExecError::Cancelled) => {
                    let _ = store
                        .set_state(&job_id, JobState::Cancelled, Some("cancelled"))
                        .await;
                    cleanup_job_dir(&cache_root, &job_id);
                }
                Err(error) => {
                    let class = error_class(&error);
                    tracing::warn!(job_id = %job_id, error_class = class, "media job failed");
                    let _ = store
                        .set_state(&job_id, JobState::Failed, Some(class))
                        .await;
                    cleanup_job_dir(&cache_root, &job_id);
                }
            }
            registry.unregister(&job_id).await;
        });

        Ok(job)
    }

    /// Extracts subtitle stream `stream_index` from `source` (already
    /// resolved + authorized by the caller) to a standalone `WebVTT`
    /// file, subject to the same concurrency limiter as remux/transcode
    /// jobs. Unlike `start_job`, this is not tracked as a persisted job
    /// row -- it is expected to complete quickly (a text stream, not
    /// video/audio) and the caller awaits it directly rather than
    /// polling; the returned path lives in a freshly created, caller-
    /// owned-for-cleanup workspace under `cache_root`.
    ///
    /// `stream_index` must be validated by the caller against a fresh
    /// probe of `source` (must be a real `subtitle` stream on that file)
    /// before calling this -- this function does not re-probe, so an
    /// unvalidated index is passed straight to `ffmpeg` as a `-map`
    /// target and simply fails (a controlled `ExecError`, not a security
    /// issue: `ffmpeg` itself rejects an out-of-range map).
    pub async fn extract_subtitle(
        &self,
        source: &Path,
        stream_index: u32,
    ) -> Result<(PathBuf, PathBuf), MediaServiceError> {
        let FfmpegAvailability::Available { ffmpeg, .. } = &self.availability else {
            return Err(MediaServiceError::Unavailable);
        };
        let workspace_id = clouddesk_auth::random_identifier(16);
        let workspace = exec::job_workspace(&self.cache_root, &workspace_id)
            .map_err(|e| MediaServiceError::Workspace(e.to_string()))?;
        let run = exec::extract_subtitle(
            &ffmpeg.path,
            source,
            &workspace,
            stream_index,
            self.limits,
            CancellationToken::new(),
        )
        .await?;
        Ok((run.output_path, workspace))
    }

    /// Extracts `source`'s embedded cover art, if any. See
    /// `exec::extract_artwork` -- a normal `Err` here just means "this
    /// file has no embedded picture stream," not a fault.
    pub async fn extract_artwork(
        &self,
        source: &Path,
    ) -> Result<(PathBuf, PathBuf), MediaServiceError> {
        let FfmpegAvailability::Available { ffmpeg, .. } = &self.availability else {
            return Err(MediaServiceError::Unavailable);
        };
        let workspace_id = clouddesk_auth::random_identifier(16);
        let workspace = exec::job_workspace(&self.cache_root, &workspace_id)
            .map_err(|e| MediaServiceError::Workspace(e.to_string()))?;
        let run = exec::extract_artwork(
            &ffmpeg.path,
            source,
            &workspace,
            self.limits,
            CancellationToken::new(),
        )
        .await?;
        Ok((run.output_path, workspace))
    }

    /// Cancels `job_id` if it belongs to `owner_user_id` and is still
    /// live in this process. Ownership is enforced by looking the job up
    /// through the store first -- the registry itself is not
    /// ownership-aware.
    pub async fn cancel_job(
        &self,
        owner_user_id: &str,
        job_id: &str,
    ) -> Result<(), MediaServiceError> {
        let job = self
            .store
            .get(owner_user_id, job_id)
            .await?
            .ok_or(MediaServiceError::NotFound)?;
        if job.state.is_terminal() {
            return Ok(());
        }
        self.registry.cancel(job_id).await;
        Ok(())
    }
}

fn cleanup_job_dir(cache_root: &Path, job_id: &str) {
    exec::cleanup_workspace(&cache_root.join(job_id));
}

fn error_class(error: &exec::ExecError) -> &'static str {
    match error {
        exec::ExecError::Unavailable => "unavailable",
        exec::ExecError::Spawn(_) => "spawn_failed",
        exec::ExecError::Cancelled => "cancelled",
        exec::ExecError::Timeout => "timeout",
        exec::ExecError::OutputTooLarge => "output_too_large",
        exec::ExecError::ExitedWithFailure { .. } => "ffmpeg_failed",
        exec::ExecError::InvalidOutput => "invalid_output",
        exec::ExecError::Workspace(_) => "workspace_error",
    }
}

/// Startup reconciliation: rows left `running`/`queued`/`probing` by a
/// process that no longer exists (crash, SIGKILL, restart) are marked
/// `expired` and their temp workspaces reclaimed. `older_than_unix`
/// should be "now" at startup, since a genuinely-restarted process has no
/// way to have a live job younger than its own uptime.
pub async fn cleanup_abandoned_jobs(
    store: &MediaJobStore,
    cache_root: &Path,
    older_than_unix: i64,
) -> Result<usize, sqlx::Error> {
    let expired = store.expire_stale(older_than_unix).await?;
    for job in &expired {
        cleanup_job_dir(cache_root, &job.id);
    }
    Ok(expired.len())
}
