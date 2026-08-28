//! Live indexing tests against real `ffmpeg`-generated audio fixtures
//! with real embedded metadata tags -- no mocked probe output. Skips
//! cleanly if `ffmpeg`/`ffprobe` aren't installed.

use clouddesk_library::{scan_root, LibraryStore};
use std::process::Stdio;
use tokio::process::Command;

async fn ffmpeg_available() -> bool {
    clouddesk_media::ffmpeg::detect(true).await.is_available()
}

async fn pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
         VALUES ('u1', 'u1', 'U1', 'x', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
         VALUES ('u2', 'u2', 'U2', 'x', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

async fn generate_track(
    dir: &std::path::Path,
    name: &str,
    title: &str,
    artist: &str,
    track_no: &str,
) {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "libmp3lame",
            "-metadata",
            &format!("title={title}"),
            "-metadata",
            &format!("artist={artist}"),
            "-metadata",
            "album=Test Album",
            "-metadata",
            &format!("track={track_no}"),
        ])
        .arg(dir.join(name))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn indexes_real_tagged_audio_with_incremental_rescan_and_removal() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "indexes_real_tagged_audio_with_incremental_rescan_and_removal",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let pool = pool().await;
    let store = LibraryStore::new(pool);
    let dir = tempfile::tempdir().unwrap();

    generate_track(dir.path(), "one.mp3", "Song One", "Artist A", "1").await;
    generate_track(dir.path(), "two.mp3", "Song Two", "Artist A", "2").await;
    // A file with an audio-looking extension but no real audio content
    // must be skipped, not crash the scan.
    std::fs::write(
        dir.path().join("not-audio.mp3"),
        b"this is not a real mp3 file",
    )
    .unwrap();

    let root = store.add_root("u1", "/music").await.unwrap();

    let summary = scan_root(&store, "u1", &root.id, dir.path(), "/music")
        .await
        .unwrap();
    assert_eq!(summary.added, 2);
    assert_eq!(summary.skipped_errors, 1);
    assert_eq!(summary.removed, 0);

    let tracks = store.list_tracks("u1", 100, 0).await.unwrap();
    assert_eq!(tracks.len(), 2);
    let titles: Vec<_> = tracks
        .iter()
        .filter_map(|t| t.metadata.title.clone())
        .collect();
    assert!(titles.contains(&"Song One".to_owned()));
    assert!(titles.contains(&"Song Two".to_owned()));

    let artists = store.list_artists("u1").await.unwrap();
    assert_eq!(artists, vec!["Artist A".to_owned()]);

    // Rescanning with nothing changed re-probes nothing.
    let rescan = scan_root(&store, "u1", &root.id, dir.path(), "/music")
        .await
        .unwrap();
    assert_eq!(rescan.added, 0);
    assert_eq!(rescan.updated, 0);
    assert_eq!(rescan.unchanged, 2);

    // Removing a file and rescanning removes it from the library.
    std::fs::remove_file(dir.path().join("one.mp3")).unwrap();
    let after_removal = scan_root(&store, "u1", &root.id, dir.path(), "/music")
        .await
        .unwrap();
    assert_eq!(after_removal.removed, 1);
    let tracks = store.list_tracks("u1", 100, 0).await.unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].metadata.title.as_deref(), Some("Song Two"));

    // Cross-user isolation: u2's library is empty despite u1's scan.
    let u2_tracks = store.list_tracks("u2", 100, 0).await.unwrap();
    assert!(u2_tracks.is_empty());
}

#[tokio::test]
async fn search_and_album_view_reflect_real_metadata() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "search_and_album_view_reflect_real_metadata",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let pool = pool().await;
    let store = LibraryStore::new(pool);
    let dir = tempfile::tempdir().unwrap();
    generate_track(dir.path(), "a.mp3", "Alpha Song", "Beta Artist", "1").await;
    let root = store.add_root("u1", "/music").await.unwrap();
    scan_root(&store, "u1", &root.id, dir.path(), "/music")
        .await
        .unwrap();

    let by_title = store.search("u1", "Alpha", 10).await.unwrap();
    assert_eq!(by_title.len(), 1);
    let by_artist = store.search("u1", "Beta", 10).await.unwrap();
    assert_eq!(by_artist.len(), 1);
    let no_match = store.search("u1", "Gamma", 10).await.unwrap();
    assert!(no_match.is_empty());

    // A LIKE-wildcard-looking query is treated as literal text, not a
    // wildcard the caller controls.
    let literal = store.search("u1", "%", 10).await.unwrap();
    assert!(literal.is_empty());

    let albums = store.list_albums("u1").await.unwrap();
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].0, "Test Album");
    let album_tracks = store.tracks_by_album("u1", "Test Album").await.unwrap();
    assert_eq!(album_tracks.len(), 1);
}

/// Generates a real short audio fixture with the given codec/container
/// and (optionally) tags, via real `ffmpeg` -- proves indexing is
/// genuinely format-agnostic (driven by what `ffprobe` reports, not a
/// hardcoded MP3 assumption) across the actual v1-required format set:
/// MP3, FLAC, WAV, OGG/Vorbis, AAC/M4A.
async fn generate_track_ext(
    dir: &std::path::Path,
    name: &str,
    codec_args: &[&str],
    tags: &[(&str, &str)],
) {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        "sine=frequency=440:duration=1".into(),
    ];
    args.extend(codec_args.iter().map(|s| (*s).to_owned()));
    for (k, v) in tags {
        args.push("-metadata".into());
        args.push(format!("{k}={v}"));
    }
    let status = Command::new("ffmpeg")
        .args(&args)
        .arg(dir.join(name))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    assert!(status.success(), "ffmpeg failed to generate {name}");
}

#[tokio::test]
async fn indexing_is_format_agnostic_across_the_v1_required_audio_formats() {
    if !ffmpeg_available().await {
        clouddesk_test_support::blocked_by_environment(
            "indexing_is_format_agnostic_across_the_v1_required_audio_formats",
            clouddesk_test_support::reason::MEDIA_TOOLING_UNAVAILABLE,
        );
        return;
    }
    let pool = pool().await;
    let store = LibraryStore::new(pool);
    let dir = tempfile::tempdir().unwrap();

    // MP3 with Unicode tags (title/artist use non-ASCII text throughout).
    generate_track_ext(
        dir.path(),
        "unicode.mp3",
        &["-c:a", "libmp3lame"],
        &[("title", "日本語タイトル"), ("artist", "Ünïcødé Ärtist")],
    )
    .await;
    // FLAC, lossless.
    generate_track_ext(
        dir.path(),
        "lossless.flac",
        &["-c:a", "flac"],
        &[("title", "Flac Song")],
    )
    .await;
    // WAV, uncompressed PCM.
    generate_track_ext(
        dir.path(),
        "raw.wav",
        &["-c:a", "pcm_s16le"],
        &[("title", "Wav Song")],
    )
    .await;
    // OGG/Vorbis.
    generate_track_ext(
        dir.path(),
        "vorbis.ogg",
        &["-c:a", "libvorbis"],
        &[("title", "Ogg Song")],
    )
    .await;
    // AAC in an M4A container.
    generate_track_ext(
        dir.path(),
        "aac.m4a",
        &["-c:a", "aac"],
        &[("title", "Aac Song")],
    )
    .await;
    // No tags at all -- must index with a sensible fallback, not fail.
    generate_track_ext(dir.path(), "untagged.mp3", &["-c:a", "libmp3lame"], &[]).await;

    let root = store.add_root("u1", "/music").await.unwrap();
    let summary = scan_root(&store, "u1", &root.id, dir.path(), "/music")
        .await
        .unwrap();
    assert_eq!(summary.added, 6, "every real audio format must be indexed");
    assert_eq!(summary.skipped_errors, 0);

    let tracks = store.list_tracks("u1", 100, 0).await.unwrap();
    assert_eq!(tracks.len(), 6);

    let unicode = tracks
        .iter()
        .find(|t| t.virtual_path.ends_with("unicode.mp3"))
        .unwrap();
    assert_eq!(unicode.metadata.title.as_deref(), Some("日本語タイトル"));
    assert_eq!(unicode.metadata.artist.as_deref(), Some("Ünïcødé Ärtist"));

    for (suffix, expected_title) in [
        ("lossless.flac", "Flac Song"),
        ("raw.wav", "Wav Song"),
        ("vorbis.ogg", "Ogg Song"),
        ("aac.m4a", "Aac Song"),
    ] {
        let track = tracks
            .iter()
            .find(|t| t.virtual_path.ends_with(suffix))
            .unwrap_or_else(|| panic!("{suffix} was not indexed"));
        assert_eq!(track.metadata.title.as_deref(), Some(expected_title));
        assert!(
            track.metadata.codec.is_some(),
            "{suffix} must have a real probed codec"
        );
    }

    // Untagged file: no title/artist tags, but it is still indexed --
    // the product-level "filename as title" fallback lives in the
    // frontend (music.ts::trackDisplayTitle), the store just returns
    // what ffprobe actually reported (nothing, here).
    let untagged = tracks
        .iter()
        .find(|t| t.virtual_path.ends_with("untagged.mp3"))
        .unwrap();
    assert!(untagged.metadata.title.is_none());
    assert!(untagged.metadata.artist.is_none());
}
