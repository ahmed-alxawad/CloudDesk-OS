//! Shared acceptance-test status contract.
//!
//! Rust's test harness has exactly two outcomes: a test function that
//! returns is "passed", one that panics is "failed". CloudDesk's
//! acceptance suites need a third, because many of them drive real
//! external fixtures (an SSH server, MinIO, WebDAV, Collabora, Brave, a
//! privileged Linux identity) that are legitimately absent on an
//! ordinary developer machine.
//!
//! Historically those suites simply `return`ed when their fixture was
//! missing. The harness then reported `ok`, which is indistinguishable
//! from a real product PASS -- `ssh_advanced_auth` "passed" 12 tests in
//! 0.11s against no SSH server at all, where a live run takes ~32s.
//! That is a false green, and it is the reason this module exists.
//!
//! The vocabulary is the project's existing one, not a new one:
//!
//! ```text
//! PASS
//! FAIL
//! BLOCKED_BY_ENVIRONMENT
//! ```
//!
//! # Two modes
//!
//! * **Normal** (developer, full-workspace): a missing fixture emits an
//!   explicit, machine-detectable `BLOCKED_BY_ENVIRONMENT` marker and
//!   the test returns, so unrelated tests keep running.
//! * **Strict** (release acceptance): the same missing fixture panics,
//!   so release validation can never silently accept it. Enabled with
//!   `CLOUDDESK_REQUIRE_LIVE_ACCEPTANCE=1`.
//!
//! # Reading the markers
//!
//! Markers go to stdout (visible with `--nocapture`) *and* are appended
//! to a status log, because the harness swallows stdout for tests it
//! considers passing -- which, in normal mode, is exactly the blocked
//! ones. The log is therefore the reliable channel:
//!
//! ```text
//! target/clouddesk-test-status.log
//! ```
//!
//! Override with `CLOUDDESK_TEST_STATUS_LOG`. `scripts/test-status.sh`
//! summarises it.

use std::io::Write as _;

/// Environment variable that turns on strict live acceptance.
pub const STRICT_ENV: &str = "CLOUDDESK_REQUIRE_LIVE_ACCEPTANCE";

/// Environment variable overriding where markers are recorded.
pub const STATUS_LOG_ENV: &str = "CLOUDDESK_TEST_STATUS_LOG";

/// The one status value this module emits. `PASS`/`FAIL` remain the
/// harness's own outcomes; only the third needs representing.
pub const STATUS_BLOCKED: &str = "BLOCKED_BY_ENVIRONMENT";

/// Stable reason codes. Keep these greppable and additive -- the
/// summariser and the closure documents refer to them by name.
pub mod reason {
    /// The disposable privileged Linux identity the Code runtime
    /// acceptance suites require (`clouddesk-code-test`, uid/gid 963,
    /// `/var/lib/clouddesk-code-test`, and the root-owned
    /// `cloudesk-sessiond-test` helper) is not provisioned on this
    /// host. Deliberately removed after Phase 7; recreating it is a
    /// privileged operation requiring explicit operator approval.
    pub const CODE_PRIVILEGED_TEST_IDENTITY_UNAVAILABLE: &str =
        "CODE_PRIVILEGED_TEST_IDENTITY_UNAVAILABLE";

    /// The disposable SSH/SFTP/SCP acceptance fixture stack
    /// (`tests/acceptance/docker-compose.yml`) is not running.
    pub const SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE: &str = "SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE";

    /// Docker, or a required prebuilt runtime image, is unavailable.
    pub const CONTAINER_RUNTIME_UNAVAILABLE: &str = "CONTAINER_RUNTIME_UNAVAILABLE";

    /// The test process cannot map a non-root Linux identity (e.g. it
    /// is running as root, or the mapped user does not exist).
    pub const LINUX_IDENTITY_UNAVAILABLE: &str = "LINUX_IDENTITY_UNAVAILABLE";

    /// A required media tool (`ffmpeg`/`ffprobe`) is not installed.
    pub const MEDIA_TOOLING_UNAVAILABLE: &str = "MEDIA_TOOLING_UNAVAILABLE";
}

/// Whether strict live acceptance is demanded.
///
/// Deliberately *not* inferred from `CI`: this repository has no such
/// convention, and guessing would make release strictness depend on an
/// unrelated variable.
#[must_use]
pub fn strict_live_acceptance() -> bool {
    strict_from(std::env::var(STRICT_ENV).ok().as_deref())
}

/// Parsing half of [`strict_live_acceptance`], split out so it can be
/// asserted without mutating process-global environment state.
fn strict_from(value: Option<&str>) -> bool {
    value == Some("1")
}

fn status_log_path() -> std::path::PathBuf {
    if let Ok(explicit) = std::env::var(STATUS_LOG_ENV) {
        return std::path::PathBuf::from(explicit);
    }
    // Every workspace member sits exactly two levels below the repo
    // root (`crates/<x>`, `services/<x>`, `tests/<x>`), so this
    // resolves to the shared `target/` for all of them.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/clouddesk-test-status.log")
}

/// The exact marker line. One line, three `KEY=VALUE` fields, no
/// spaces inside values -- so a single `grep` finds it and a single
/// `awk` splits it.
#[must_use]
pub fn marker_line(test_name: &str, reason: &str) -> String {
    format!(
        "CLOUDDESK_TEST_STATUS={STATUS_BLOCKED} CLOUDDESK_TEST_REASON={reason} \
         CLOUDDESK_TEST_NAME={test_name}"
    )
}

fn record_to(path: &std::path::Path, line: &str) {
    // Appended, not rewritten: many test binaries -- and many threads
    // within them -- run concurrently. Single small `O_APPEND` writes
    // do not interleave.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Declare that `test_name` cannot run because an external fixture is
/// unavailable.
///
/// In normal mode this emits the marker and returns, so the caller
/// should `return` immediately afterwards. In strict mode it panics,
/// failing the test deterministically with the reason code in the
/// message.
///
/// # Panics
///
/// Panics when [`strict_live_acceptance`] is true -- that is the point:
/// release validation must not accept a missing mandatory fixture.
pub fn blocked_by_environment(test_name: &str, reason: &str) {
    let marker = marker_line(test_name, reason);
    record_to(&status_log_path(), &marker);
    assert!(
        !strict_live_acceptance(),
        "{marker} -- strict live acceptance ({STRICT_ENV}=1) requires this fixture to be \
         available; refusing to report a missing mandatory fixture as anything but a failure"
    );
    println!("{marker}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marker must be impossible to mistake for a pass: it names
    /// the status, the reason and the test, in a single greppable line.
    /// Asserted against the pure formatter so this test never touches
    /// process-global environment state (these run in parallel).
    #[test]
    fn marker_line_is_stable_and_greppable() {
        let line = marker_line("task_example", reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE);
        assert_eq!(
            line,
            "CLOUDDESK_TEST_STATUS=BLOCKED_BY_ENVIRONMENT \
             CLOUDDESK_TEST_REASON=SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE \
             CLOUDDESK_TEST_NAME=task_example"
        );
        assert_eq!(line.lines().count(), 1, "must stay a single line");
        assert!(
            !line.to_lowercase().contains("pass"),
            "the marker must never contain the token 'pass'"
        );
    }

    /// The recorded line must survive to the log verbatim, since in
    /// normal mode the harness swallows stdout for blocked tests and
    /// the log is the only reliable channel.
    #[test]
    fn recorded_marker_is_appended_verbatim() {
        let dir = std::env::temp_dir().join(format!(
            "cd-test-support-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("status.log");

        record_to(
            &log,
            &marker_line("task_a", reason::LINUX_IDENTITY_UNAVAILABLE),
        );
        record_to(
            &log,
            &marker_line("task_b", reason::MEDIA_TOOLING_UNAVAILABLE),
        );

        let written = std::fs::read_to_string(&log).unwrap();
        assert_eq!(written.lines().count(), 2, "appended, never overwritten");
        assert!(written.contains("CLOUDDESK_TEST_NAME=task_a"));
        assert!(written.contains("CLOUDDESK_TEST_REASON=MEDIA_TOOLING_UNAVAILABLE"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Strict mode is opt-in via an exact `1` and nothing else --
    /// notably never inferred from `CI`, which this repository does not
    /// use as a convention.
    #[test]
    fn strict_requires_an_exact_one() {
        assert!(strict_from(Some("1")));
        // Notably NOT any truthy-looking value, and NOT inferred from
        // `CI`, which this repository does not use as a convention.
        assert!(!strict_from(Some("0")));
        assert!(!strict_from(Some("true")));
        assert!(!strict_from(Some("yes")));
        assert!(!strict_from(Some("")));
        assert!(!strict_from(None));
    }
}
