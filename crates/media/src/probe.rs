//! Typed `ffprobe`-backed media probing.
//!
//! `ffprobe` is invoked with a fixed argv (`-print_format json -show_format
//! -show_streams <path>`); the path is passed as a single argv element,
//! never interpolated into a shell string, so it cannot inject flags or
//! shell metacharacters. Output is treated as untrusted: it is bounded in
//! size, the process is bounded in time, and any parse/shape failure
//! returns a typed `ProbeError` rather than panicking.

use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

/// Hard ceiling on how much stdout we will ever read from `ffprobe`,
/// regardless of what the process claims it wants to write. Real probe
/// JSON for even a file with hundreds of streams is a few KB; this leaves
/// generous headroom while still bounding memory against a hostile or
/// buggy `ffprobe` build.
const MAX_PROBE_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("ffprobe is not available")]
    Unavailable,
    #[error("ffprobe timed out")]
    Timeout,
    #[error("failed to launch ffprobe: {0}")]
    Spawn(String),
    #[error("ffprobe exited with a failure status")]
    ExitedWithFailure,
    #[error("ffprobe output was not valid/well-formed media metadata")]
    Malformed,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StreamInfo {
    pub index: u32,
    pub codec_type: String,
    pub codec_name: Option<String>,
    pub profile: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<String>,
    pub sample_rate: Option<String>,
    pub channel_layout: Option<String>,
    pub bit_rate: Option<String>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MediaProbe {
    pub format_name: String,
    pub duration_seconds: Option<f64>,
    pub bit_rate: Option<String>,
    pub streams: Vec<StreamInfo>,
}

impl MediaProbe {
    #[must_use]
    pub fn video_streams(&self) -> Vec<&StreamInfo> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "video")
            .collect()
    }

    #[must_use]
    pub fn audio_streams(&self) -> Vec<&StreamInfo> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "audio")
            .collect()
    }

    #[must_use]
    pub fn subtitle_streams(&self) -> Vec<&StreamInfo> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "subtitle")
            .collect()
    }
}

// --- raw ffprobe JSON shape, kept separate from the public MediaProbe type
// so a change in ffprobe's schema can never silently reshape our API. ---

#[derive(Deserialize)]
struct RawProbe {
    #[serde(default)]
    format: Option<RawFormat>,
    #[serde(default)]
    streams: Vec<RawStream>,
}

#[derive(Deserialize)]
struct RawFormat {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Deserialize)]
struct RawStream {
    index: Option<u32>,
    codec_type: Option<String>,
    codec_name: Option<String>,
    profile: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default)]
    r_frame_rate: Option<String>,
    #[serde(default)]
    sample_rate: Option<String>,
    #[serde(default)]
    channel_layout: Option<String>,
    #[serde(default)]
    bit_rate: Option<String>,
    #[serde(default)]
    tags: Option<serde_json::Map<String, serde_json::Value>>,
}

fn parse_raw(bytes: &[u8]) -> Result<MediaProbe, ProbeError> {
    let raw: RawProbe = serde_json::from_slice(bytes).map_err(|_| ProbeError::Malformed)?;
    let Some(format) = raw.format else {
        return Err(ProbeError::Malformed);
    };
    let Some(format_name) = format.format_name else {
        return Err(ProbeError::Malformed);
    };
    let duration_seconds = format.duration.as_deref().and_then(|d| d.parse().ok());
    let streams = raw
        .streams
        .into_iter()
        .filter_map(|s| {
            Some(StreamInfo {
                index: s.index?,
                codec_type: s.codec_type?,
                codec_name: s.codec_name,
                profile: s.profile,
                width: s.width,
                height: s.height,
                frame_rate: s.r_frame_rate,
                sample_rate: s.sample_rate,
                channel_layout: s.channel_layout,
                bit_rate: s.bit_rate,
                language: s
                    .tags
                    .and_then(|t| t.get("language").and_then(|v| v.as_str().map(String::from))),
            })
        })
        .collect();
    Ok(MediaProbe {
        format_name,
        duration_seconds,
        bit_rate: format.bit_rate,
        streams,
    })
}

/// Runs `ffprobe` against `path` (already resolved to a real, authorized
/// filesystem location by the caller — this function never does path
/// resolution or authorization itself) and returns typed metadata.
pub async fn probe_media(
    ffprobe_path: &str,
    path: &std::path::Path,
) -> Result<MediaProbe, ProbeError> {
    let mut child = Command::new(ffprobe_path)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ProbeError::Spawn(e.to_string()))?;

    let mut stdout = child.stdout.take().ok_or(ProbeError::Malformed)?;
    let mut buf = Vec::new();
    let read_result = timeout(
        PROBE_TIMEOUT,
        Box::pin(async {
            let mut chunk = vec![0_u8; 64 * 1024];
            loop {
                let n = stdout
                    .read(&mut chunk)
                    .await
                    .map_err(|e| ProbeError::Spawn(e.to_string()))?;
                if n == 0 {
                    break;
                }
                if buf.len() + n > MAX_PROBE_OUTPUT_BYTES {
                    return Err(ProbeError::Malformed);
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Ok(())
        }),
    )
    .await;

    match read_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = child.kill().await;
            return Err(e);
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(ProbeError::Timeout);
        }
    }

    let status = match timeout(PROBE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(ProbeError::Spawn(e.to_string())),
        Err(_) => {
            let _ = child.kill().await;
            return Err(ProbeError::Timeout);
        }
    };
    if !status.success() {
        return Err(ProbeError::ExitedWithFailure);
    }

    parse_raw(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_json_is_a_typed_error_not_a_panic() {
        assert!(matches!(parse_raw(b"not json"), Err(ProbeError::Malformed)));
    }

    #[test]
    fn empty_object_is_malformed() {
        assert!(matches!(parse_raw(b"{}"), Err(ProbeError::Malformed)));
    }

    #[test]
    fn truncated_json_is_malformed_not_a_panic() {
        let truncated = br#"{"format":{"format_name":"mov,mp4","duration":"10.5","#;
        assert!(matches!(parse_raw(truncated), Err(ProbeError::Malformed)));
    }

    #[test]
    fn parses_a_realistic_mp4_probe() {
        let json = br#"{
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "12.345", "bit_rate": "500000"},
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "h264", "profile": "High", "width": 1280, "height": 720, "r_frame_rate": "30/1"},
                {"index": 1, "codec_type": "audio", "codec_name": "aac", "sample_rate": "48000", "channel_layout": "stereo", "tags": {"language": "eng"}}
            ]
        }"#;
        let probe = parse_raw(json).unwrap();
        assert_eq!(probe.format_name, "mov,mp4,m4a,3gp,3g2,mj2");
        assert_eq!(probe.duration_seconds, Some(12.345));
        assert_eq!(probe.video_streams().len(), 1);
        assert_eq!(probe.audio_streams().len(), 1);
        assert_eq!(probe.audio_streams()[0].language.as_deref(), Some("eng"));
    }

    #[test]
    fn streams_with_missing_required_fields_are_dropped_not_panicking() {
        let json = br#"{
            "format": {"format_name": "matroska"},
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "vp9"},
                {"codec_type": "audio", "codec_name": "opus"}
            ]
        }"#;
        let probe = parse_raw(json).unwrap();
        assert_eq!(probe.streams.len(), 1);
    }

    #[test]
    fn huge_declared_dimensions_do_not_overflow_parsing() {
        let json = br#"{
            "format": {"format_name": "mov,mp4"},
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "h264", "width": 4294967295, "height": 4294967295}
            ]
        }"#;
        let probe = parse_raw(json).unwrap();
        assert_eq!(probe.video_streams()[0].width, Some(u32::MAX));
    }
}
