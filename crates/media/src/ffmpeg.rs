//! Typed `FFmpeg`/`ffprobe` discovery.
//!
//! Detection never shells out through `sh -c` and never installs anything —
//! it only tries to execute fixed argv (`<candidate> -version`) and parses
//! the first line of output. A missing binary is a normal, typed
//! "unavailable" outcome, not an error to propagate.

use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Where we look for the two binaries, in order. Fixed, not derived from
/// any request input.
const FFMPEG_CANDIDATES: &[&str] = &["ffmpeg", "/usr/bin/ffmpeg", "/usr/local/bin/ffmpeg"];
const FFPROBE_CANDIDATES: &[&str] = &["ffprobe", "/usr/bin/ffprobe", "/usr/local/bin/ffprobe"];

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct BinaryInfo {
    pub path: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FfmpegAvailability {
    /// Feature disabled via `runtime.media.enabled` — no process may be
    /// resident and no probe/remux/transcode job may be started.
    Disabled,
    /// Enabled but one or both binaries could not be found/executed.
    Unavailable { reason: String },
    /// Enabled and both binaries responded to `-version`.
    Available {
        ffmpeg: BinaryInfo,
        ffprobe: BinaryInfo,
    },
}

impl FfmpegAvailability {
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

async fn probe_binary(candidates: &[&str]) -> Option<BinaryInfo> {
    for candidate in candidates {
        let Ok(Ok(output)) = timeout(
            PROBE_TIMEOUT,
            Command::new(candidate)
                .arg("-version")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output(),
        )
        .await
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let Some(first_line) = text.lines().next() else {
            continue;
        };
        return Some(BinaryInfo {
            path: (*candidate).to_owned(),
            version: first_line.trim().to_owned(),
        });
    }
    None
}

/// Detects `ffmpeg`/`ffprobe` availability. `enabled` reflects the
/// `runtime.media.enabled` system setting: when `false`, detection is not
/// even attempted and no process is spawned, matching every other optional
/// runtime's "disabled means no resident process" contract.
pub async fn detect(enabled: bool) -> FfmpegAvailability {
    if !enabled {
        return FfmpegAvailability::Disabled;
    }
    let ffmpeg = probe_binary(FFMPEG_CANDIDATES).await;
    let ffprobe = probe_binary(FFPROBE_CANDIDATES).await;
    match (ffmpeg, ffprobe) {
        (Some(ffmpeg), Some(ffprobe)) => FfmpegAvailability::Available { ffmpeg, ffprobe },
        (None, _) => FfmpegAvailability::Unavailable {
            reason: "ffmpeg binary not found".to_owned(),
        },
        (_, None) => FfmpegAvailability::Unavailable {
            reason: "ffprobe binary not found".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_never_attempts_detection() {
        assert_eq!(detect(false).await, FfmpegAvailability::Disabled);
    }

    #[tokio::test]
    async fn missing_binary_is_unavailable_not_an_error() {
        let result = probe_binary(&["clouddesk-definitely-not-a-real-binary"]).await;
        assert!(result.is_none());
    }
}
