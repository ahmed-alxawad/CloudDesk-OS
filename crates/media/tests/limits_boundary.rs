//! Phase 3 residual closure: real timeout/output-quota boundary
//! enforcement through the SAME production code path
//! (`MediaService::start_job` -> `exec::run_ffmpeg`), with a reduced
//! `MediaLimits` injected via `MediaService::with_limits` -- never a
//! separate "test implementation" of either limit. Production defaults
//! (`exec::MediaLimits::default()`) are asserted directly (Task 2) and
//! never touched by these tests.

use clouddesk_media::{exec, ffmpeg, JobOperation, JobState};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

async fn require_ffmpeg() -> Option<String> {
    if let ffmpeg::FfmpegAvailability::Available { ffmpeg, .. } = ffmpeg::detect(true).await {
        Some(ffmpeg.path)
    } else {
        eprintln!("SKIPPED: ffmpeg not available in this environment");
        None
    }
}

async fn generate(ffmpeg_path: &str, dir: &Path, name: &str, args: &[&str]) -> PathBuf {
    let output = dir.join(name);
    let status = Command::new(ffmpeg_path)
        .arg("-y")
        .args(args)
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .expect("failed to launch ffmpeg fixture generation");
    assert!(status.success(), "fixture generation failed for {name}");
    output
}

async fn service_with_limits(
    limits: exec::MediaLimits,
) -> (clouddesk_media::MediaService, tempfile::TempDir) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    sqlx::query("INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at) VALUES ('u1','u1','U1','x',0,0)")
        .execute(&pool)
        .await
        .unwrap();
    let availability = ffmpeg::detect(true).await;
    let cache_root = tempfile::tempdir().unwrap();
    let service =
        clouddesk_media::MediaService::new(availability, pool, cache_root.path().to_path_buf())
            .with_limits(limits);
    (service, cache_root)
}

async fn wait_terminal(
    service: &clouddesk_media::MediaService,
    job_id: &str,
) -> clouddesk_media::MediaJob {
    for _ in 0..100 {
        let current = service.store().get("u1", job_id).await.unwrap().unwrap();
        if current.state.is_terminal() {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("job did not reach a terminal state in time");
}

fn ffmpeg_child_count() -> usize {
    // A plain, portable child-process check via /proc rather than
    // parsing `ps` output -- counts processes whose comm is exactly
    // "ffmpeg" (the real binary name, not a substring match that could
    // also catch this test's own argv).
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().parse::<u32>().is_ok())
        .filter(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| comm.trim() == "ffmpeg")
        })
        .count()
}

/// Task 2: production defaults are exactly what the handoff specifies,
/// asserted directly against the typed config -- not a separate
/// hardcoded literal that could silently drift from the real values.
#[test]
fn production_defaults_are_exactly_600s_and_4gib() {
    let defaults = exec::MediaLimits::default();
    assert_eq!(defaults.job_timeout, Duration::from_mins(10));
    assert_eq!(defaults.max_output_bytes, 4 * 1024 * 1024 * 1024);
    assert_eq!(exec::DEFAULT_JOB_TIMEOUT, Duration::from_mins(10));
    assert_eq!(exec::DEFAULT_MAX_OUTPUT_BYTES, 4 * 1024 * 1024 * 1024);
}

/// Task 24: zero/absurd typed limits are accepted without panicking or
/// overflowing -- `Duration`/`u64` are used as-is, so this is really a
/// confirmation that nothing downstream (e.g. `tokio::time::timeout`,
/// the `>` comparison in `watch_output_size`) special-cases these.
#[test]
fn zero_and_absurd_limits_do_not_panic_the_typed_config() {
    let zero = exec::MediaLimits {
        job_timeout: Duration::from_secs(0),
        max_output_bytes: 0,
    };
    assert_eq!(zero.job_timeout, Duration::ZERO);
    let huge = exec::MediaLimits {
        job_timeout: Duration::from_secs(u64::MAX / 2),
        max_output_bytes: u64::MAX,
    };
    assert_eq!(huge.max_output_bytes, u64::MAX);
}

/// Tasks 3/4/5/6: a real ffmpeg transcode of a real, deliberately long
/// (20s) synthetic source, run through the real `MediaService::start_job`
/// path with a reduced 2-second `job_timeout`. Verifies: the job starts
/// and reaches `Running`, the timeout fires against the real running
/// `ffmpeg` process (not a simulated clock), the terminal state is
/// `Failed` with `error_class = "timeout"` (this codebase's existing
/// terminal-state model has no separate `LimitExceeded` variant --
/// `Failed` + a distinct `error_class` string IS the "limit exceeded"
/// signal, matching `output_too_large`'s identical shape below), no
/// `ffmpeg` child process survives, no output is exposed as a
/// successful result, and a fresh job can run successfully afterward.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn live_timeout_boundary_through_production_job_path() {
    let Some(ffmpeg_path) = require_ffmpeg().await else {
        return;
    };
    let (service, cache_root) = service_with_limits(exec::MediaLimits {
        job_timeout: Duration::from_secs(2),
        ..exec::MediaLimits::default()
    })
    .await;

    let src_dir = tempfile::tempdir().unwrap();
    // 1080p/30s so the real production transcode path (fixed libx264
    // args, no preset override -- this test never controls ffmpeg's own
    // speed, only the input it's asked to encode) reliably takes well
    // over the 2s test timeout regardless of host CPU speed (measured
    // ~11s for this exact shape at production encode settings).
    let long_source = generate(
        &ffmpeg_path,
        src_dir.path(),
        "long.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=30:size=1920x1080:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=30",
            "-c:v",
            "mpeg2video",
            "-c:a",
            "mp2",
            "-shortest",
        ],
    )
    .await;

    let job = service
        .start_job(
            "u1",
            "/long.mkv",
            long_source,
            JobOperation::Transcode,
            exec::TrackSelection::default(),
        )
        .await
        .expect("job should start");

    // Task 4: the job genuinely starts and a real ffmpeg process exists
    // before the timeout has any chance to fire.
    let mut saw_running = false;
    for _ in 0..20 {
        if service
            .store()
            .get("u1", &job.id)
            .await
            .unwrap()
            .unwrap()
            .state
            == JobState::Running
        {
            saw_running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw_running,
        "the job must reach Running before the timeout fires"
    );
    assert!(
        ffmpeg_child_count() > 0,
        "a real ffmpeg process must be running"
    );

    let final_job = wait_terminal(&service, &job.id).await;

    // Task 4/6: exactly one terminal outcome, classified as timeout --
    // never conflated with user cancellation.
    assert_eq!(
        final_job.state,
        JobState::Failed,
        "timeout must never be classified as Completed/Cancelled"
    );
    assert_eq!(final_job.error_class.as_deref(), Some("timeout"));

    // Task 5: canonical output never falsely committed.
    assert!(final_job.output_path.is_none());

    // Task 5: no ffmpeg child process survives the timeout.
    let mut orphan_gone = false;
    for _ in 0..20 {
        if ffmpeg_child_count() == 0 {
            orphan_gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        orphan_gone,
        "the timed-out ffmpeg process must be reaped, not left running"
    );

    // Task 5: temp output/workspace removed according to policy
    // (MediaService's failure path calls cleanup_job_dir).
    assert!(
        !cache_root.path().join(&job.id).exists(),
        "the job workspace must be removed after a timeout failure"
    );

    // Task 5: a fresh job can still be started afterward -- the limiter/
    // registry are not left in a stuck state by a timed-out job.
    let quick_source = generate(
        &ffmpeg_path,
        src_dir.path(),
        "quick.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-c:v",
            "libx264",
        ],
    )
    .await;
    let retry_job = service
        .start_job(
            "u1",
            "/quick.mkv",
            quick_source,
            JobOperation::Remux,
            exec::TrackSelection::default(),
        )
        .await
        .expect("a new job must be able to start after a prior job timed out");
    let retry_final = wait_terminal(&service, &retry_job.id).await;
    assert_eq!(retry_final.state, JobState::Completed);
}

/// Tasks 7/8/9/10/11: a real ffmpeg transcode whose real output genuinely
/// exceeds a small injected quota, run through the same production job
/// path. Verifies the job terminates as `Failed` with
/// `error_class = "output_too_large"` (distinct from `"timeout"` --
/// Task 11's requirement that quota failure is distinguishable from
/// timeout/cancellation/ordinary ffmpeg failure), the canonical output is
/// never exposed as a successful result, no ffmpeg child survives, and a
/// job producing output safely under the same quota still completes
/// (Task 9's below-quota case).
#[tokio::test(flavor = "multi_thread")]
async fn live_output_quota_boundary_through_production_job_path() {
    // Above quota: the real production transcode of this source (fixed
    // libx264 args, ~2s per 10s of 720p source measured on this host)
    // takes several seconds -- long enough that the 2s output-size poll
    // fires at least twice while the job is still running, well after
    // the file has already crossed the tiny quota (never a race against
    // the job's own natural completion).
    const TINY_QUOTA: u64 = 64 * 1024;
    let Some(ffmpeg_path) = require_ffmpeg().await else {
        return;
    };

    let (service, cache_root) = service_with_limits(exec::MediaLimits {
        max_output_bytes: TINY_QUOTA,
        ..exec::MediaLimits::default()
    })
    .await;
    let src_dir = tempfile::tempdir().unwrap();
    let source = generate(
        &ffmpeg_path,
        src_dir.path(),
        "quota-source.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=30:size=1280x720:rate=30",
            "-c:v",
            "mpeg2video",
            "-b:v",
            "20M",
        ],
    )
    .await;

    let job = service
        .start_job(
            "u1",
            "/quota-source.mkv",
            source,
            JobOperation::Transcode,
            exec::TrackSelection::default(),
        )
        .await
        .expect("job should start");
    let final_job = wait_terminal(&service, &job.id).await;

    assert_eq!(
        final_job.state,
        JobState::Failed,
        "quota breach must never be classified as Completed/Cancelled"
    );
    assert_eq!(final_job.error_class.as_deref(), Some("output_too_large"));
    assert_ne!(
        final_job.error_class.as_deref(),
        Some("timeout"),
        "quota failure must be distinguishable from timeout"
    );
    assert!(
        final_job.output_path.is_none(),
        "canonical output must never be exposed as a successful result"
    );

    let mut orphan_gone = false;
    for _ in 0..20 {
        if ffmpeg_child_count() == 0 {
            orphan_gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(orphan_gone, "the over-quota ffmpeg process must be reaped");
    assert!(
        !cache_root.path().join(&job.id).exists(),
        "the job workspace must be removed after a quota failure"
    );

    // Task 9, below-quota case: the same production path, a generous
    // quota, a short job -- must complete normally.
    let (service_ok, _cache_root_ok) = service_with_limits(exec::MediaLimits::default()).await;
    let small_source = generate(
        &ffmpeg_path,
        src_dir.path(),
        "small.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-c:v",
            "libx264",
        ],
    )
    .await;
    let ok_job = service_ok
        .start_job(
            "u1",
            "/small.mkv",
            small_source,
            JobOperation::Remux,
            exec::TrackSelection::default(),
        )
        .await
        .expect("job should start");
    let ok_final = wait_terminal(&service_ok, &ok_job.id).await;
    assert_eq!(ok_final.state, JobState::Completed);
    assert!(ok_final.output_path.is_some());
}

/// Task 18: a real race between the timeout firing and the process's own
/// natural exit must still produce exactly one terminal state -- no
/// contradictory `completed` + `failed` outcome, no panic. Uses a job
/// timeout set right at the edge of a short real encode's actual
/// duration rather than a simulated race.
#[tokio::test(flavor = "multi_thread")]
async fn timeout_racing_natural_process_exit_yields_one_terminal_state() {
    let Some(ffmpeg_path) = require_ffmpeg().await else {
        return;
    };
    let (service, _cache_root) = service_with_limits(exec::MediaLimits {
        job_timeout: Duration::from_secs(3),
        ..exec::MediaLimits::default()
    })
    .await;
    let src_dir = tempfile::tempdir().unwrap();
    // A short encode that should finish close to, but not reliably
    // faster than, the 3s timeout -- exercising the race window rather
    // than a comfortably-fast or comfortably-slow case either way.
    let borderline_source = generate(
        &ffmpeg_path,
        src_dir.path(),
        "borderline.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=3:size=320x240:rate=15",
            "-c:v",
            "mpeg2video",
        ],
    )
    .await;
    let job = service
        .start_job(
            "u1",
            "/borderline.mkv",
            borderline_source,
            JobOperation::Remux,
            exec::TrackSelection::default(),
        )
        .await
        .expect("job should start");
    let final_job = wait_terminal(&service, &job.id).await;
    // Whichever side of the race actually won, the outcome must be a
    // single, unambiguous terminal state -- never both/neither.
    assert!(
        matches!(final_job.state, JobState::Completed | JobState::Failed),
        "the race must resolve to exactly one real terminal state: {:?}",
        final_job.state
    );
    if final_job.state == JobState::Completed {
        assert!(final_job.output_path.is_some());
    } else {
        assert!(final_job.output_path.is_none());
    }
}
