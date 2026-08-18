//! Music library indexing and storage. Reuses `clouddesk_media` for all
//! `ffprobe`/`ffmpeg` invocation (metadata extraction, artwork
//! extraction, DIRECT/REMUX/TRANSCODE playback) -- this crate never
//! shells out to `ffmpeg` itself and never reimplements the
//! compatibility engine or job queue.

pub mod scan;
pub mod store;

pub use scan::{scan_root, ScanSummary, MAX_SCAN_FILES, SCAN_TIMEOUT};
pub use store::{LibraryRoot, LibraryStore, Playlist, Track, TrackMetadata};
