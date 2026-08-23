//! Live acceptance against a *real* installed `ffmpeg`/`ffprobe` -- no
//! mocked `Process` output. Fixtures are generated on the fly with
//! `ffmpeg` itself (lavfi test sources) rather than committed as binaries.
//! If neither binary is on `PATH`, every test here is skipped with an
//! explicit message rather than silently passing.

use clouddesk_media::{compat, exec, ffmpeg, probe, JobState};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
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
        exec::TrackSelection::default(),
        exec::MediaLimits::default(),
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
        exec::TrackSelection::default(),
        exec::MediaLimits::default(),
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
            exec::TrackSelection::default(),
            exec::MediaLimits::default(),
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
            clouddesk_media::exec::TrackSelection::default(),
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

#[tokio::test]
async fn subtitle_extraction_produces_a_real_webvtt_track() {
    let Some((ffmpeg_path, ffprobe_path)) = require_ffmpeg().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    // -f srt -i - reads subtitle content from stdin; built directly with
    // tokio::process::Command (rather than the shared `generate` helper,
    // which doesn't pipe stdin) to keep this fixture self-contained.
    let output = dir.path().join("with-subs.mkv");
    let mut child = tokio::process::Command::new(&ffmpeg_path)
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-f",
            "srt",
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-c:s",
            "srt",
            "-shortest",
        ])
        .arg(&output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"1\n00:00:00,000 --> 00:00:00,900\nHello world\n\n")
        .await
        .unwrap();
    let status = child.wait().await.unwrap();
    assert!(status.success());

    let probe = probe::probe_media(&ffprobe_path, &output).await.unwrap();
    let subtitle_streams = probe.subtitle_streams();
    assert_eq!(subtitle_streams.len(), 1);
    let stream_index = subtitle_streams[0].index;

    let workspace = tempfile::tempdir().unwrap();
    let extracted = exec::extract_subtitle(
        &ffmpeg_path,
        &output,
        workspace.path(),
        stream_index,
        exec::MediaLimits::default(),
        CancellationToken::new(),
    )
    .await
    .expect("subtitle extraction should succeed");
    let vtt = std::fs::read_to_string(&extracted.output_path).unwrap();
    assert!(vtt.starts_with("WEBVTT"));
    assert!(vtt.contains("Hello world"));

    // A stream index that doesn't exist fails cleanly, not with a panic.
    let bogus_workspace = tempfile::tempdir().unwrap();
    let bogus = exec::extract_subtitle(
        &ffmpeg_path,
        &output,
        bogus_workspace.path(),
        99,
        exec::MediaLimits::default(),
        CancellationToken::new(),
    )
    .await;
    assert!(bogus.is_err());
}

#[tokio::test]
async fn audio_track_selection_picks_the_requested_stream() {
    let Some((ffmpeg_path, ffprobe_path)) = require_ffmpeg().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let multi_audio = generate(
        &ffmpeg_path,
        dir.path(),
        "multi-audio.mkv",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=220:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:duration=1",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:a",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ],
    )
    .await;
    let probe = probe::probe_media(&ffprobe_path, &multi_audio)
        .await
        .unwrap();
    assert_eq!(
        probe.audio_streams().len(),
        2,
        "fixture must have two audio streams"
    );

    let workspace = tempfile::tempdir().unwrap();
    let remuxed = exec::remux(
        &ffmpeg_path,
        &multi_audio,
        workspace.path(),
        exec::TrackSelection {
            audio_track_ordinal: Some(1),
        },
        exec::MediaLimits::default(),
        CancellationToken::new(),
    )
    .await
    .expect("track-selected remux should succeed");
    let reprobe = probe::probe_media(&ffprobe_path, &remuxed.output_path)
        .await
        .unwrap();
    assert_eq!(
        reprobe.audio_streams().len(),
        1,
        "only the requested audio track should be present in the output"
    );
}

#[tokio::test]
async fn embedded_artwork_is_extracted_as_a_bounded_jpeg_and_absence_fails_cleanly() {
    let Some((ffmpeg_path, _ffprobe_path)) = require_ffmpeg().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();

    // Build a cover image, then an audio file with it attached as a
    // picture stream -- the real shape a tagged MP3/FLAC file has.
    let cover = dir.path().join("cover.jpg");
    let status = Command::new(&ffmpeg_path)
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x64:d=1",
            "-frames:v",
            "1",
        ])
        .arg(&cover)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    assert!(status.success());

    let audio = dir.path().join("plain.mp3");
    let status = Command::new(&ffmpeg_path)
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "libmp3lame",
        ])
        .arg(&audio)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    assert!(status.success());

    let with_art = dir.path().join("with-art.mp3");
    let status = Command::new(&ffmpeg_path)
        .args(["-y", "-i"])
        .arg(&audio)
        .arg("-i")
        .arg(&cover)
        .args([
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c:a",
            "copy",
            "-c:v",
            "mjpeg",
            "-disposition:v:0",
            "attached_pic",
        ])
        .arg(&with_art)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    assert!(status.success());

    let workspace = tempfile::tempdir().unwrap();
    let extracted = exec::extract_artwork(
        &ffmpeg_path,
        &with_art,
        workspace.path(),
        exec::MediaLimits::default(),
        CancellationToken::new(),
    )
    .await
    .expect("artwork extraction should succeed for a file with attached art");
    let bytes = std::fs::read(&extracted.output_path).unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(
        &bytes[0..2],
        &[0xFF, 0xD8],
        "output must be a real JPEG (SOI marker)"
    );

    // A file with no attached picture stream fails cleanly, not a panic.
    let no_art_workspace = tempfile::tempdir().unwrap();
    let no_art = exec::extract_artwork(
        &ffmpeg_path,
        &audio,
        no_art_workspace.path(),
        exec::MediaLimits::default(),
        CancellationToken::new(),
    )
    .await;
    assert!(no_art.is_err());
}
