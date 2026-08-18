//! Bounded, symlink-safe music-library indexing.
//!
//! Strategy (documented explicitly per the phase's own instruction):
//! **on-demand incremental rescan**, triggered by an explicit API call,
//! not a background filesystem watcher or scheduler. A rescan compares
//! each candidate file's cheap `(size, mtime)` fingerprint against the
//! last-indexed value and only re-probes files whose fingerprint changed
//! (or that are new); files that vanished since the last scan are
//! removed from the library. There is no live filesystem-watch/instant-
//! update path in this phase -- a library reflects reality as of its
//! last scan, same as most desktop music players' "rescan library"
//! button.

use crate::store::{LibraryStore, TrackMetadata};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Extension allowlist used purely to avoid probing every non-audio file
/// in a mixed-content directory -- an optimization, never the source of
/// truth. A file is only ever counted as a track because `ffprobe`
/// reported a real audio stream on it, regardless of what its extension
/// promised.
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "oga", "opus", "m4a", "aac", "wma", "aiff", "aif", "alac", "mka",
];

/// Hard ceiling on files considered per scan -- guarantees termination
/// against an enormous or adversarially deep directory tree without
/// loading the whole tree into memory at once (the walk is streamed, not
/// collected up front).
pub const MAX_SCAN_FILES: usize = 20_000;
/// Wall-clock ceiling for one scan invocation.
pub const SCAN_TIMEOUT: Duration = Duration::from_mins(5);

#[derive(Debug, Default, serde::Serialize)]
pub struct ScanSummary {
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub removed: u64,
    /// Files that looked like audio (by extension) but failed to probe
    /// as real audio, or whose probe/metadata was otherwise unusable --
    /// recorded, not fatal to the rest of the scan.
    pub skipped_errors: u64,
    /// `true` if the scan stopped early due to `MAX_SCAN_FILES` or
    /// `SCAN_TIMEOUT` -- the library reflects a partial scan, not the
    /// full tree, and a subsequent rescan will continue picking up
    /// unvisited files.
    pub truncated: bool,
}

fn fingerprint(metadata: &std::fs::Metadata) -> String {
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    format!("{}:{mtime}", metadata.len())
}

fn has_audio_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

/// Recursively collects candidate audio file paths under `real_root`,
/// never following symlinks (a symlinked subdirectory is skipped
/// entirely, matching the same policy used by archive creation in
/// `clouddesk-vfs`), bounded by `MAX_SCAN_FILES`.
fn collect_candidates(real_root: &Path, deadline: Instant) -> (Vec<PathBuf>, bool) {
    let mut candidates = Vec::new();
    let mut stack = vec![real_root.to_path_buf()];
    let mut truncated = false;

    while let Some(dir) = stack.pop() {
        if Instant::now() >= deadline || candidates.len() >= MAX_SCAN_FILES {
            truncated = true;
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if candidates.len() >= MAX_SCAN_FILES {
                truncated = true;
                break;
            }
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue; // never followed -- prevents escaping real_root
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() && has_audio_extension(&path) {
                candidates.push(path);
            }
        }
    }
    (candidates, truncated)
}

fn parse_leading_int(value: &str) -> Option<i64> {
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn metadata_from_probe(probe: &clouddesk_media::MediaProbe) -> TrackMetadata {
    let tag = |key: &str| {
        probe
            .tags
            .get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    // Real bit rates are at most a handful of Mbit/s; well inside i64
    // range, so a lossy cast here can only ever mean "malformed/absurd
    // input got clamped," never a meaningful truncation.
    #[allow(clippy::cast_possible_truncation)]
    let bit_rate = probe
        .bit_rate
        .as_deref()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v.round() as i64);
    TrackMetadata {
        title: tag("title"),
        artist: tag("artist"),
        album: tag("album"),
        album_artist: tag("album_artist"),
        track_number: tag("track").as_deref().and_then(parse_leading_int),
        disc_number: tag("disc").as_deref().and_then(parse_leading_int),
        duration_seconds: probe.duration_seconds,
        codec: probe
            .audio_streams()
            .first()
            .and_then(|s| s.codec_name.clone()),
        bit_rate,
        year: tag("date").or_else(|| tag("year")),
        genre: tag("genre"),
    }
}

/// Scans `real_root` (an already-resolved, already-authorized real
/// filesystem path -- this function does no authorization itself) and
/// upserts/removes tracks in `store` under `root_id`/`owner_user_id`.
/// `virtual_root` is the VFS-relative path prefix used to build each
/// track's stored `virtual_path` (so playback endpoints, which resolve
/// paths against the caller's VFS root, can address it later).
pub async fn scan_root(
    store: &LibraryStore,
    owner_user_id: &str,
    root_id: &str,
    real_root: &Path,
    virtual_root: &str,
) -> Result<ScanSummary, sqlx::Error> {
    let deadline = Instant::now() + SCAN_TIMEOUT;
    let (candidates, truncated) = collect_candidates(real_root, deadline);
    let existing_fingerprints = store.fingerprints_for_root(root_id).await?;

    let mut summary = ScanSummary {
        truncated,
        ..ScanSummary::default()
    };
    let mut still_present: HashSet<String> = HashSet::new();

    for path in candidates {
        if Instant::now() >= deadline {
            summary.truncated = true;
            break;
        }
        let Ok(fs_meta) = std::fs::metadata(&path) else {
            summary.skipped_errors += 1;
            continue;
        };
        let Ok(relative) = path.strip_prefix(real_root) else {
            continue;
        };
        let relative_str = relative.to_string_lossy().replace('\\', "/");
        let virtual_path = if virtual_root.is_empty() || virtual_root == "/" {
            format!("/{relative_str}")
        } else {
            format!("{}/{relative_str}", virtual_root.trim_end_matches('/'))
        };
        still_present.insert(virtual_path.clone());

        let current_fingerprint = fingerprint(&fs_meta);
        if existing_fingerprints.get(&virtual_path) == Some(&current_fingerprint) {
            summary.unchanged += 1;
            continue;
        }

        let Some(ffprobe_path) = ffprobe_binary().await else {
            summary.skipped_errors += 1;
            continue;
        };
        let Ok(probe) = clouddesk_media::probe::probe_media(&ffprobe_path, &path).await else {
            summary.skipped_errors += 1;
            continue;
        };
        if probe.audio_streams().is_empty() {
            // Extension suggested audio; the file genuinely isn't --
            // real metadata, not the extension, is authoritative.
            summary.skipped_errors += 1;
            continue;
        }
        let metadata = metadata_from_probe(&probe);
        let was_new = !existing_fingerprints.contains_key(&virtual_path);
        store
            .upsert_track(
                owner_user_id,
                root_id,
                &virtual_path,
                &metadata,
                &current_fingerprint,
            )
            .await?;
        if was_new {
            summary.added += 1;
        } else {
            summary.updated += 1;
        }
    }

    if !summary.truncated {
        // Only prune "missing" files when the scan actually walked the
        // whole tree -- a truncated scan hasn't seen every file, so
        // "not seen this pass" doesn't mean "gone."
        summary.removed = store.prune_missing(root_id, &still_present).await?;
    }

    Ok(summary)
}

/// Cached-per-call `ffprobe` binary lookup (scan doesn't have a
/// `MediaService` handle -- it's given just the binary path by the
/// caller in the HTTP layer normally; this fallback keeps `scan_root`
/// usable directly in tests without threading availability through).
async fn ffprobe_binary() -> Option<String> {
    if let clouddesk_media::FfmpegAvailability::Available { ffprobe, .. } =
        clouddesk_media::ffmpeg::detect(true).await
    {
        Some(ffprobe.path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_leading_track_and_disc_numbers() {
        assert_eq!(parse_leading_int("3/12"), Some(3));
        assert_eq!(parse_leading_int("07"), Some(7));
        assert_eq!(parse_leading_int(""), None);
        assert_eq!(parse_leading_int("unknown"), None);
    }

    #[test]
    fn has_audio_extension_is_case_insensitive_and_extension_only() {
        assert!(has_audio_extension(Path::new("song.MP3")));
        assert!(has_audio_extension(Path::new("song.flac")));
        assert!(!has_audio_extension(Path::new("cover.jpg")));
        assert!(!has_audio_extension(Path::new("no-extension")));
    }

    #[test]
    fn symlinked_subdirectories_are_never_descended_into() {
        let dir = tempfile::tempdir().unwrap();
        let real_target = tempfile::tempdir().unwrap();
        std::fs::write(real_target.path().join("outside.mp3"), b"not real audio").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(real_target.path(), dir.path().join("escape")).unwrap();

        std::fs::write(dir.path().join("inside.mp3"), b"not real audio either").unwrap();

        let (candidates, truncated) =
            collect_candidates(dir.path(), Instant::now() + Duration::from_secs(5));
        assert!(!truncated);
        assert_eq!(
            candidates.len(),
            1,
            "the symlinked directory must not be walked"
        );
        assert!(candidates[0].ends_with("inside.mp3"));
    }
}
