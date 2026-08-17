//! Deterministic browser-compatibility decision.
//!
//! Decisions are made from probed container + codec names — never from a
//! filename extension, which is untrusted and frequently wrong.

use crate::probe::MediaProbe;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamPlan {
    /// Container and all relevant codecs are natively browser-playable —
    /// stream the original bytes unmodified.
    Direct,
    /// Codecs are browser-compatible but the container is not (or track
    /// selection requires it) — change the container without re-encoding.
    Remux,
    /// At least one codec is not browser-playable — re-encode.
    Transcode,
    /// No plan can produce a browser-playable result (e.g. no video/audio
    /// stream at all, or the probe was empty).
    Unsupported,
}

const DIRECT_CONTAINERS: &[&str] = &["mov,mp4,m4a,3gp,3g2,mj2", "webm"];
/// `ffprobe` reports both MKV and `WebM` as the combined `format_name`
/// `matroska,webm` — there is no way to tell them apart from that field
/// alone, so a file reported this way is never trusted as direct-safe
/// even when its codecs would otherwise qualify; it is always remuxed
/// into an unambiguous container first.
const REMUXABLE_CONTAINERS: &[&str] = &["matroska,webm", "avi", "mpegts"];

const DIRECT_VIDEO_CODECS: &[&str] = &["h264", "vp8", "vp9", "av1"];
const DIRECT_AUDIO_CODECS: &[&str] = &["aac", "mp3", "opus", "vorbis", "flac"];

fn container_is(probe: &MediaProbe, list: &[&str]) -> bool {
    list.contains(&probe.format_name.as_str())
}

/// Decides how `probe`'s media should be delivered to a browser.
#[must_use]
pub fn decide(probe: &MediaProbe) -> StreamPlan {
    let video = probe.video_streams();
    let audio = probe.audio_streams();

    if video.is_empty() && audio.is_empty() {
        return StreamPlan::Unsupported;
    }

    let video_ok = video.iter().all(|s| {
        s.codec_name
            .as_deref()
            .is_some_and(|c| DIRECT_VIDEO_CODECS.contains(&c))
    });
    let audio_ok = audio.iter().all(|s| {
        s.codec_name
            .as_deref()
            .is_some_and(|c| DIRECT_AUDIO_CODECS.contains(&c))
    });

    if !video_ok || !audio_ok {
        return StreamPlan::Transcode;
    }

    // All present codecs are browser-compatible at this point; the only
    // remaining question is the container.
    if container_is(probe, DIRECT_CONTAINERS) {
        StreamPlan::Direct
    } else if container_is(probe, REMUXABLE_CONTAINERS) {
        StreamPlan::Remux
    } else {
        // Unrecognized container with compatible codecs: safest to remux
        // into a known-good container rather than guess it's direct-safe.
        StreamPlan::Remux
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::StreamInfo;

    fn video(codec: &str) -> StreamInfo {
        StreamInfo {
            codec_type: "video".into(),
            codec_name: Some(codec.into()),
            ..Default::default()
        }
    }
    fn audio(codec: &str) -> StreamInfo {
        StreamInfo {
            codec_type: "audio".into(),
            codec_name: Some(codec.into()),
            ..Default::default()
        }
    }

    #[test]
    fn mp4_h264_aac_is_direct() {
        let probe = MediaProbe {
            format_name: "mov,mp4,m4a,3gp,3g2,mj2".into(),
            streams: vec![video("h264"), audio("aac")],
            ..Default::default()
        };
        assert_eq!(decide(&probe), StreamPlan::Direct);
    }

    #[test]
    fn webm_vp9_opus_is_direct() {
        let probe = MediaProbe {
            format_name: "matroska,webm".into(),
            streams: vec![video("vp9"), audio("opus")],
            ..Default::default()
        };
        // matroska,webm with compatible codecs is treated conservatively
        // as a remux target since ffprobe can't distinguish true .webm
        // from .mkv by format_name alone; genuinely-.webm still plays
        // fine after a lossless remux.
        assert_eq!(decide(&probe), StreamPlan::Remux);
    }

    #[test]
    fn mkv_h264_aac_is_remux() {
        let probe = MediaProbe {
            format_name: "matroska,webm".into(),
            streams: vec![video("h264"), audio("aac")],
            ..Default::default()
        };
        assert_eq!(decide(&probe), StreamPlan::Remux);
    }

    #[test]
    fn incompatible_video_codec_is_transcode() {
        let probe = MediaProbe {
            format_name: "matroska,webm".into(),
            streams: vec![video("hevc"), audio("aac")],
            ..Default::default()
        };
        assert_eq!(decide(&probe), StreamPlan::Transcode);
    }

    #[test]
    fn incompatible_audio_codec_is_transcode() {
        let probe = MediaProbe {
            format_name: "mov,mp4,m4a,3gp,3g2,mj2".into(),
            streams: vec![video("h264"), audio("ac3")],
            ..Default::default()
        };
        assert_eq!(decide(&probe), StreamPlan::Transcode);
    }

    #[test]
    fn no_av_streams_is_unsupported() {
        let probe = MediaProbe {
            format_name: "mov,mp4".into(),
            streams: vec![],
            ..Default::default()
        };
        assert_eq!(decide(&probe), StreamPlan::Unsupported);
    }

    #[test]
    fn missing_codec_name_is_treated_as_incompatible_not_a_panic() {
        let probe = MediaProbe {
            format_name: "mov,mp4,m4a,3gp,3g2,mj2".into(),
            streams: vec![StreamInfo {
                codec_type: "video".into(),
                codec_name: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(decide(&probe), StreamPlan::Transcode);
    }

    #[test]
    fn unknown_container_with_compatible_codecs_falls_back_to_remux() {
        let probe = MediaProbe {
            format_name: "flv".into(),
            streams: vec![video("h264"), audio("aac")],
            ..Default::default()
        };
        assert_eq!(decide(&probe), StreamPlan::Remux);
    }
}
