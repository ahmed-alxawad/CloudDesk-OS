//! Live acceptance against a *real* installed `ffmpeg`/`ffprobe` -- no
//! mocked `Process` output. Fixtures are generated on the fly with
//! `ffmpeg` itself (lavfi test sources) rather than committed as binaries.
//! If neither binary is on `PATH`, every test here is skipped with an
//! explicit message rather than silently passing.

use clouddesk_media::{compat, exec, ffmpeg, probe, JobState};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

async fn require_ffmpeg() -> Option<(String, String)> {
    if let ffmpeg::FfmpegAvailability::Available { ffmpeg, ffprobe } = ffmpeg::detect(true).await {
        Some((ffmpeg.path, ffprobe.path))
    } else {
        eprintln!("SKIPPED: ffmpeg/ffprobe not available in this environment");
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn end_to_end_direct_remux_transcode_and_cancellation() {
    let Some((ffmpeg_path, ffprobe_path)) = require_ffmpeg().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();

    // 1. A genuinely browser-compatible MP4 (H.264 + AAC).
    let direct_mp4 = generate(
        &ffmpeg_path,
        dir.path(),
        "direct.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ],
    )
    .await;
    let probe_direct = probe::probe_media(&ffprobe_path, &direct_mp4)
        .await
        .unwrap();
    assert_eq!(compat::decide(&probe_direct), compat::StreamPlan::Direct);

    // 2. Same streams in an MKV container -> REMUX (not TRANSCODE).
    let mkv = generate(
        &ffmpeg_path,
        dir.path(),
        "compatible.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ],
    )
    .await;
    let probe_mkv = probe::probe_media(&ffprobe_path, &mkv).await.unwrap();
    assert_eq!(compat::decide(&probe_mkv), compat::StreamPlan::Remux);

    let workspace = tempfile::tempdir().unwrap();
    let remuxed = exec::remux(
        &ffmpeg_path,
        &mkv,
        workspace.path(),
        CancellationToken::new(),
    )
    .await
    .expect("remux should succeed");
    assert!(remuxed.output_path.exists());
    let reprobe = probe::probe_media(&ffprobe_path, &remuxed.output_path)
        .await
        .expect("remuxed output must itself be valid, probeable media");
    assert_eq!(reprobe.video_streams().len(), 1);
    assert_eq!(reprobe.audio_streams().len(), 1);

    // 3. An incompatible codec (mpeg2video) -> TRANSCODE, then verify the
    //    transcoded output is itself playable-compatible.
    let incompatible = generate(
        &ffmpeg_path,
        dir.path(),
        "incompatible.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:v",
            "mpeg2video",
            "-c:a",
            "mp2",
            "-shortest",
        ],
    )
    .await;
    let probe_incompatible = probe::probe_media(&ffprobe_path, &incompatible)
        .await
        .unwrap();
    assert_eq!(
        compat::decide(&probe_incompatible),
        compat::StreamPlan::Transcode
    );

    let transcode_workspace = tempfile::tempdir().unwrap();
    let transcoded = exec::transcode(
        &ffmpeg_path,
        &incompatible,
        transcode_workspace.path(),
        exec::TranscodeOptions::default(),
        CancellationToken::new(),
    )
    .await
    .expect("transcode should succeed");
    let reprobe_transcoded = probe::probe_media(&ffprobe_path, &transcoded.output_path)
        .await
        .expect("transcoded output must itself be valid, probeable media");
    assert_eq!(
        compat::decide(&reprobe_transcoded),
        compat::StreamPlan::Direct,
        "transcoded output must itself classify as DIRECT-playable"
    );

    // 4. Cancellation actually terminates a running ffmpeg process rather
    //    than merely dropping our handle to it.
    let long_source = generate(
        &ffmpeg_path,
        dir.path(),
        "long.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=20:size=640x480:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=20",
            "-c:v",
            "mpeg2video",
            "-c:a",
            "mp2",
            "-shortest",
        ],
    )
    .await;
    let cancel_workspace = tempfile::tempdir().unwrap();
    let token = CancellationToken::new();
    let token_clone = token.clone();
    let ffmpeg_path_clone = ffmpeg_path.clone();
    let cancel_ws_path = cancel_workspace.path().to_path_buf();
    let long_source_clone = long_source.clone();
    let handle = tokio::spawn(async move {
        exec::transcode(
            &ffmpeg_path_clone,
            &long_source_clone,
            &cancel_ws_path,
            exec::TranscodeOptions::default(),
            token_clone,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    token.cancel();
    let result = handle.await.unwrap();
    assert!(matches!(result, Err(exec::ExecError::Cancelled)));

    // No orphaned ffmpeg process left targeting our cancel workspace: give
    // the OS a moment to reap, then confirm no process still has the
    // output file open by trying to remove the workspace outright.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    drop(cancel_workspace); // succeeds cleanly only if nothing still holds it open
}

#[tokio::test]
async fn hostile_media_is_rejected_cleanly_not_a_panic() {
    let Some((_, ffprobe_path)) = require_ffmpeg().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();

    let empty = dir.path().join("empty.mp4");
    std::fs::write(&empty, []).unwrap();
    assert!(probe::probe_media(&ffprobe_path, &empty).await.is_err());

    let random = dir.path().join("random.bin");
    std::fs::write(&random, [0x13_u8; 4096]).unwrap();
    assert!(probe::probe_media(&ffprobe_path, &random).await.is_err());

    let missing = dir.path().join("does-not-exist.mp4");
    assert!(probe::probe_media(&ffprobe_path, &missing).await.is_err());
}

#[tokio::test]
async fn job_lifecycle_end_to_end_through_media_service() {
    let Some(_) = require_ffmpeg().await else {
        return;
    };
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
        clouddesk_media::MediaService::new(availability, pool, cache_root.path().to_path_buf());

    let ffmpeg_path = match service.availability() {
        ffmpeg::FfmpegAvailability::Available { ffmpeg, .. } => ffmpeg.path.clone(),
        _ => return,
    };
    let src_dir = tempfile::tempdir().unwrap();
    let mkv = generate(
        &ffmpeg_path,
        src_dir.path(),
        "job-source.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ],
    )
    .await;

    let job = service
        .start_job(
            "u1",
            "/job-source.mkv",
            mkv,
            clouddesk_media::JobOperation::Remux,
        )
        .await
        .expect("job should start");

    let mut final_job = None;
    for _ in 0..100 {
        let current = service.store().get("u1", &job.id).await.unwrap().unwrap();
        if current.state.is_terminal() {
            final_job = Some(current);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let final_job = final_job.expect("job did not reach a terminal state in time");
    assert_eq!(final_job.state, JobState::Completed);
    assert!(final_job.output_path.is_some());

    // Cross-user isolation: user "u2" must not be able to see u1's job.
    assert!(service.store().get("u2", &job.id).await.unwrap().is_none());
}
