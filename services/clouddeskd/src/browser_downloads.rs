//! Phase 9 Pass 3B: Browser downloads.
//!
//! Websites navigate/download inside the real, isolated, server-side
//! Brave container -- never the client's own browser, never a client-
//! chosen server path. CDP's own `Browser.setDownloadBehavior` with
//! `behavior: "allowAndName"` is the actual security boundary here:
//! Chromium renames every downloaded file to its own opaque GUID on
//! disk, so a hostile site's `Content-Disposition` filename (or lack
//! of one) never has any influence over the real on-disk path -- the
//! suggested filename is trusted only as *display text*, sanitized
//! before it is ever used again (e.g. as a save-to-Files destination
//! name).
//!
//! Downloads land inside the instance's own already-isolated `/state`
//! mount (`{instance_state_dir}/downloads/<guid>`), which `clouddeskd`
//! (running on the host) can read directly -- no client ever learns
//! or controls that path.

use std::path::PathBuf;

/// Production default (Task 4): no single download may exceed this.
pub const DEFAULT_MAX_DOWNLOAD_BYTES: u64 = 500 * 1024 * 1024;
/// Production default: total staged download bytes per Browser
/// session (not per Browser *instance* -- a fresh WS connection to
/// the same instance starts a fresh accounting window, matching this
/// project's already-established "per-connection session state"
/// scope for the broker).
pub const DEFAULT_MAX_SESSION_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Test-only override, read once by [`max_download_bytes`]/
/// [`max_session_download_bytes`] -- never set outside a test. Lets
/// live tests exercise the exact same cancellation code path with a
/// small, fast-to-exceed limit instead of staging real hundreds-of-
/// megabytes payloads.
static TEST_QUOTA_OVERRIDE: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();

/// Test-only. Never called from `main.rs`.
pub fn set_test_quota_override(max_download_bytes: u64, max_session_bytes: u64) {
    let _ = TEST_QUOTA_OVERRIDE.set((max_download_bytes, max_session_bytes));
}

#[must_use]
pub fn max_download_bytes() -> u64 {
    TEST_QUOTA_OVERRIDE
        .get()
        .map_or(DEFAULT_MAX_DOWNLOAD_BYTES, |(d, _)| *d)
}

#[must_use]
pub fn max_session_download_bytes() -> u64 {
    TEST_QUOTA_OVERRIDE
        .get()
        .map_or(DEFAULT_MAX_SESSION_DOWNLOAD_BYTES, |(_, s)| *s)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStateKind {
    InProgress,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug)]
pub struct DownloadRecord {
    /// CDP's own download GUID -- opaque, unguessable, doubles as the
    /// public `DownloadId` (Task 1): never derived from or exposing
    /// any server filesystem path.
    pub guid: String,
    pub suggested_filename: String,
    pub sanitized_filename: String,
    pub url: String,
    pub total_bytes: Option<u64>,
    pub received_bytes: u64,
    pub state: DownloadStateKind,
    pub failure_reason: Option<String>,
    /// Host-side path -- never serialized to a client (Task 1: "never
    /// expose arbitrary server path").
    pub staging_path: PathBuf,
}

impl DownloadRecord {
    #[must_use]
    pub fn public_json(&self) -> serde_json::Value {
        serde_json::json!({
            "download_id": self.guid,
            "filename": self.sanitized_filename,
            "url": self.url,
            "total_bytes": self.total_bytes,
            "received_bytes": self.received_bytes,
            "state": self.state,
            "failure_reason": self.failure_reason,
        })
    }
}

/// Task 3: normalizes a site-supplied `Content-Disposition`/
/// `suggestedFilename` into a safe display name. Never used to
/// construct the actual on-disk staging path (CDP's own GUID renaming
/// already makes that impossible to influence) -- only for display and
/// as the *default* target filename when a user later saves the
/// download into Files, where it is re-validated by the normal Files
/// destination-authorization path regardless.
#[must_use]
pub fn sanitize_download_filename(suggested: &str) -> String {
    const MAX_LEN: usize = 200;
    // Take only the final path component, discarding both Unix and
    // Windows-style separators and any traversal segment -- a hostile
    // suggested name of "../../evil", "/etc/passwd",
    // "C:\Windows\system32\evil.exe", or embedded NUL/newline bytes
    // must never survive into the display/destination name.
    let last_component = suggested.rsplit(['/', '\\']).next().unwrap_or(suggested);
    let cleaned: String = last_component.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    let cleaned = cleaned.trim_start_matches('.'); // no dotfiles, no bare ".."/"."
    let cleaned = cleaned.replace(['/', '\\', ':', '\0'], "_");
    let truncated: String = cleaned.chars().take(MAX_LEN).collect();
    if truncated.trim().is_empty() {
        "download".to_owned()
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_hostile_filenames() {
        assert_eq!(sanitize_download_filename("../../evil"), "evil");
        assert_eq!(sanitize_download_filename("../"), "download");
        assert_eq!(sanitize_download_filename("/etc/passwd"), "passwd");
        assert_eq!(
            sanitize_download_filename("C:\\Windows\\system32\\evil.exe"),
            "evil.exe"
        );
        assert_eq!(sanitize_download_filename(".hidden"), "hidden");
        assert_eq!(sanitize_download_filename(""), "download");
        assert_eq!(sanitize_download_filename("   "), "download");
        assert!(!sanitize_download_filename("a\0b").contains('\0'));
        assert!(!sanitize_download_filename("a\nb\r").contains('\n'));
        let long = "a".repeat(500);
        assert!(sanitize_download_filename(&long).len() <= 200);
        assert_eq!(sanitize_download_filename("réal.txt"), "réal.txt");
    }
}
