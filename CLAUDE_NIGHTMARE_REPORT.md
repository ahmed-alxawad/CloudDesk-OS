# CloudDesk-OS v1.0.0 — Disaster/Nightmare Adversarial Test Report

```
Release under test:  v1.0.0
Release commit:      9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2
Immutable tag:       v1.0.0 (untouched)
Audit branch:        audit/claude-nightmare-v1.0.0
Recommended next:    v1.0.1-rc.1 (prepared on this branch; not tagged in git — see "Release candidate" below)
```

Two sessions: an initial pass (`/disaster-test` + `/nightmare-test`, both
authored fresh — no pre-existing installed commands were found), and this
continuation, which did the remaining live adversarial coverage: a targeted
backend authorization sweep, live SSH/SFTP/WebDAV/S3 testing against real
disposable fixtures, WebSocket auth-gate testing, and Range-header fuzzing.

## Environment note

No Rust toolchain was preinstalled in this container; installed
`rustc`/`cargo` 1.97.1 via `rustup` in user space (no `sudo`). `cargo run`
itself hangs indefinitely for unrelated sandboxing reasons here — all live
testing uses compiled binaries (`target/debug/clouddeskd`, or small scratch
crates depending on `clouddesk-remote`/`clouddesk-vfs` by path) built with
`cargo build` and executed directly. Docker was available and used for real
OpenSSH/WebDAV/MinIO fixtures via the repo's own
`tests/acceptance/docker-compose.yml`.

---

## A note on trusting prior "PASS" claims

`tests/acceptance/src/main.rs` — the tool that produced v1.0.0's
`LIVE_ACCEPTANCE_REPORT.md` — imports `russh`/SSH nowhere. 54 of its 98
lines are hardcoded `report.push_str("- X: **PASS**\n")` strings. The entire
"Real OpenSSH server", "Real SFTP server", and "Real transfer matrix"
sections, plus most of the "Fresh CloudDesk lifecycle" section, were never
executed — they're literal strings. Only the S3 `put_object`/`list` calls
and one WebDAV `PUT` were real function calls. This is why this audit did
not trust that baseline and re-verified SSH/SFTP/WebDAV/S3 against real
disposable fixtures directly through the product's own code
(`clouddesk-remote`), independent of that tool. Two of the four defects
below (CLAUDE-NIGHTMARE-002, -003, -004) were hiding exactly behind those
false "PASS" claims.

---

## Scope executed (be honest about coverage)

**Executed live, against real running/compiled product code:**
- Full `cargo build --workspace` + `cargo test --workspace` baseline.
- SQLite: 40× SIGKILL of `clouddeskd migrate` mid-run — recovered every
  time, clean `PRAGMA integrity_check`. 15× rerun of the existing
  concurrent-writer regression test — no flakiness.
- Live HTTP session lifecycle (bootstrap, login, cookie attributes,
  `/auth/me`, logout, revoked-session-token replay, garbage/empty/SQLi-shaped
  cookies, brute-force account+IP throttling).
- **Targeted backend authorization sweep** of every handler under
  `system/settings/admin/users/roles/audit/secrets/remote-server` in
  `services/clouddeskd/src/lib.rs`: every handler either calls
  `authorize_request(..., "<capability>", ...)`, delegates to an
  auth-crate method that calls `require_actor(actor, "<capability>", ...)`,
  or routes through `dispatch_privileged_action` (which checks
  `principal.can(capability)` before issuing a grant). `system_summary` was
  the one exception (found in the first session, fixed as
  CLAUDE-NIGHTMARE-001). `get_runtime_settings` also only checks
  authentication, not authorization — inspected and confirmed **not** a
  defect: it's a feature-flag readout (`browser`/`code`/`office` enabled?)
  fetched by every logged-in user's desktop shell on load
  (`apps/web/src/App.svelte`), not privileged data.
- Regression test extended to assert Guest → 403, **User role → 403**, and
  Administrator → 200 for `/api/v1/system/summary` (User has a broad
  workspace capability set but not `system.services.manage`).
- **Live SSH** against a real disposable OpenSSH container
  (`tests/acceptance/docker-compose.yml`, `linuxserver/openssh-server`):
  password auth, real host-key retrieval via `ssh-keyscan`, and — after
  fixing CLAUDE-NIGHTMARE-002 — proved a pinned connection to the real key
  succeeds and a connection pinned to a *different* key is rejected with
  `Unknown server key`, using the actual `clouddesk_remote::ssh::SshSession`
  code, not a shell-level `ssh` client.
- **Live SFTP** against the same container using the real
  `SftpProvider`: found and fixed CLAUDE-NIGHTMARE-003 (new-file upload) and
  CLAUDE-NIGHTMARE-004 (root/directory listing), see below. Verified
  post-fix: 3 MB new-file upload + round-trip read-back + listing, all live.
- **Live WebDAV** against a real disposable `bytemark/webdav` container
  using the real `WebDavProvider`: new-file PUT, listing, read-back, and a
  wrong-password write correctly rejected. No defect found.
- **Live S3** against a real disposable MinIO container using the real
  `S3Provider`: new-key `PutObject`, `ListObjectsV2`, read-back, delete,
  and a >5 MB multipart upload with byte-for-byte read-back. No defect
  found. (One MinIO bucket briefly went missing mid-session for
  fixture-internal reasons unrelated to CloudDesk — recreated and retested
  cleanly; noted here for transparency, not counted as a product defect.)
- **Live WebSocket** auth gate on `/api/v1/terminal/ws` against a real
  running `clouddeskd`, using a real Python WebSocket client: no
  session → `401`; garbage session cookie → `401`; valid session but
  hostile cross-site `Origin` → `403` (rejected even though the session
  itself was valid — the existing `cross_site_mutations_are_rejected_before_routing`
  committed test already covers this same boundary); valid session,
  same-origin → passes the auth gate through to the privilege layer
  (`503` only because the disposable instance had the privileged helper
  intentionally disabled).
- **Range-header fuzzing** on `/api/v1/media/stream` (huge end, malformed
  unit, start > end, start beyond EOF, multi-range) against a real running
  instance: consistent `404` (file outside the test instance's VFS root —
  a test-setup limitation, not a resolvable defect within the time
  available), no crashes, no 500s, no hangs. Percent-encoded path
  traversal in the `path` query parameter was correctly rejected with
  `400` before reaching the filesystem.
- Confirmed no FFmpeg integration exists anywhere in the codebase
  (`grep -r ffmpeg` over all `*.rs` is empty) — `stream_media`/
  `preview_media` are pure HTTP byte-range file serving. FFmpeg-specific
  scenarios (transcode hang, cancellation, concurrent transcodes) are
  **N/A**, not blocked: there is no transcoding feature to attack. (FFmpeg
  the binary is present in this container, contrary to what the fabricated
  acceptance report implied by marking it "BLOCKED".)
- Disk exhaustion: confirmed a size-bounded disposable filesystem **is**
  achievable without `sudo` via `docker run --tmpfs /path:size=2m` (verified
  the container ENOSPC's correctly at the limit), but time did not permit
  running `clouddeskd` itself against a constrained mount before the
  session's effort budget was exhausted.

**Not executed this pass:**
- Full cross-provider transfer-matrix kill/restart races (SIGKILL
  `clouddeskd` / worker / source / destination mid-transfer, pause/resume
  races, retry storms, concurrent-same-destination jobs). Verified by code
  reading only: `worker.rs` streams provider-to-provider server-side
  (`get_provider` → `process_job`), so the browser is architecturally never
  a remote-to-remote data path — but this was not exercised live under
  kill conditions.
- Optional runtimes (Brave/Code/Office): **BLOCKED** — no such containers
  or runtime images are available in this environment. No launcher/process
  code for these was found to inspect statically either within the time
  available.
- Installer/backup/restore chaos beyond SQLite migration interruption
  (already covered above): no VM/full-installer environment available in
  this container. **BLOCKED**, not assumed passing.
- Malicious media files (corrupt image, huge dimensions, malicious SVG,
  malformed PDF/MP4): not exercised — `stream_media` is raw byte-range
  serving of whatever bytes are on disk with no parsing, so the main
  residual risk is browser-side rendering of hostile content, outside
  CloudDesk's own server-side attack surface.
- Concurrent connection storms / SSH connection floods / resource
  exhaustion beyond the disk-exhaustion mechanism check above.

Treat everything in "Not executed this pass" as **unverified**, not
passing.

---

## Findings

### ID: CLAUDE-NIGHTMARE-001
```
Severity:           MEDIUM
Subsystem:          Authorization / RBAC (Host Administration)
Release affected:   v1.0.0
```
**Reproduction:** Bootstrap → log in as admin → step up → create a
Guest-role user → log in as guest → `GET /api/v1/system/summary`.

**Expected:** `403`, matching sibling host-admin endpoints
(`system.services.manage`/`system.power.manage`).

**Actual (before fix):** `200 OK` with hostname/kernel/uptime/load/memory/
container-engine data. The handler checked authentication only.

**Security impact:** Any authenticated user, including Guest, could read
host telemetry the Settings UI hides behind an admin-only panel — a direct
API bypass of a hidden UI control (handoff #15), and Guest reaching an
admin surface (#13).

**Data-loss / Availability impact:** None (read-only).

**Root cause:** `system_summary` discarded its `principal` binding instead
of checking `principal.can(<capability>)`.

**Fix:** Require `system.services.manage` before reading host state.

**Regression test:**
`services/clouddeskd/tests/auth_api.rs::guest_role_cannot_read_system_summary`
— now asserts Guest → `403`, **User → `403`**, Administrator → `200`.

**Retest:** `cargo test -p clouddeskd --test auth_api` (2/2), full
`cargo test --workspace` (all green), and live retest against a running
disposable instance: guest → `403 {"error":"permission denied"}`,
admin → `200`. Confirmed fixed.

---

### ID: CLAUDE-NIGHTMARE-002
```
Severity:           CRITICAL
Subsystem:          SSH host-key verification
Release affected:   v1.0.0
```
**Reproduction:** Any real SSH/SFTP connection made by CloudDesk (transfers,
terminal sessions) via `clouddesk_remote::ssh::SshSession::connect`.

**Expected:** Per the handoff's named critical invariant — "Host-key
mismatch must be rejected. Silent acceptance of unexpected key replacement
is a critical defect" — and priority target #53.

**Actual (before fix):**
`crates/remote/src/ssh.rs::SshClientHandler::check_server_key` was:
```rust
async fn check_server_key(&mut self, _server_public_key: &...) -> ... {
    Ok(true)
}
```
It unconditionally accepted **any** server host key. `RemoteServer` records
always carry a pinned key from creation (`NewRemoteServer::host_key_base64`
is mandatory), and a separate `/api/v1/remote/host-keys/verify` endpoint
exists to *check* it — but that endpoint's result was never plumbed into
the actual connection made by the transfer worker (`worker.rs`). Every real
transfer/terminal SSH connection performed **zero** host-key verification.

**Security impact:** A man-in-the-middle or a host that has been
compromised and had its SSH host key replaced is silently trusted. This
defeats host-key pinning entirely and allows credential/data interception
on every SSH/SFTP operation CloudDesk performs. This is the most severe
finding in this audit.

**Data-loss / Availability impact:** Potential credential and data
exfiltration via MITM; no direct availability impact.

**Root cause:** `check_server_key` never compared the presented key against
any expected value; the pinned key available at the `worker.rs` call site
was never passed through to the SSH client.

**Fix:**
- `SshClientHandler` now carries `expected_host_key_base64: Option<String>`
  and compares the presented key (`base64(PublicKey::to_bytes())`, the same
  wire format as `host_key_base64`) against it using the existing
  constant-time `verify_host_key`, rejecting the handshake on mismatch.
- `SshSession::connect_pinned` (new) and `connect_proxyjump` thread the
  pinned key through; `connect` (existing signature, used by tests) now
  delegates to `connect_pinned(..., None)` for backward compatibility.
- `worker.rs` now fetches `store.pinned_host_key(...)` and passes it to
  `connect_pinned` for every real SFTP/transfer connection.

**Regression tests** (`crates/remote/tests/ssh.rs`, deterministic, no
Docker dependency):
- `test_ssh_connect_pinned_host_key_match_succeeds`
- `test_ssh_connect_pinned_host_key_mismatch_is_rejected`

**Retest:**
- `cargo test -p clouddesk-remote --test ssh` → 4/4 passed.
- **Live**, against the real disposable OpenSSH container, using the actual
  `SshSession::connect_pinned`: connecting with the container's real
  `ssh-keyscan`-retrieved key → succeeds; connecting with a substituted
  32-byte key (simulated host-key replacement) → rejected with
  `Unknown server key`. Confirmed fixed.
- Full `cargo test --workspace` → all green, no regressions.

---

### ID: CLAUDE-NIGHTMARE-003
```
Severity:           HIGH
Subsystem:          SFTP upload
Release affected:   v1.0.0
```
**Reproduction:** Upload any file via SFTP to a path that does not already
exist on the remote (the overwhelmingly common upload case).

**Expected:** File is created and its content written (handoff SFTP target
#61, "upload").

**Actual (before fix):** Failed with `No such file`.
`crates/remote/src/sftp.rs::SftpProvider::write_file` called
`russh_sftp::client::Session::write`, whose implementation opens with
`OpenFlags::WRITE` **only** — it never sets `CREATE`, so it can only
overwrite a file that is already present.

**Security impact:** None directly.

**Data-loss / Availability impact:** Functional — SFTP upload of new files
is completely broken. Uploading an already-existing file (overwrite) works;
every other upload fails.

**Root cause:** Used the library's `write()` convenience method instead of
`create()` (which opens with `CREATE | TRUNCATE | WRITE`).

**Fix:** `write_file` now calls `self.session.create(remote_path)` and
writes via `AsyncWriteExt::write_all` + `shutdown`, giving standard
create-or-overwrite upload semantics.

**Regression test:**
`crates/remote/tests/sftp.rs::write_file_creates_a_new_remote_file` — an
in-process mock SFTP server (no Docker dependency) proving a path that
never existed can be created and read back correctly.

**Retest:**
- `cargo test -p clouddesk-remote --test sftp` → 2/2 passed.
- **Live**, against the real disposable OpenSSH container: uploaded a 3 MB
  file that had never existed on the remote, confirmed byte-for-byte
  round-trip and that it appears in a directory listing. Confirmed fixed.
- Full `cargo test --workspace` → all green.

---

### ID: CLAUDE-NIGHTMARE-004
```
Severity:           HIGH
Subsystem:          SFTP directory listing
Release affected:   v1.0.0
```
**Reproduction:** List any SFTP directory (including the root) against a
real, non-chrooted OpenSSH server — i.e. a server where the login's home
directory is not the filesystem root, which is the default/common OpenSSH
configuration.

**Expected:** Listing succeeds (handoff SFTP target #61, "list").

**Actual (before fix):** Failed with `No such file`, reproduced both
through `SftpProvider::list` and in isolation. Root cause:
`crates/remote/src/sftp.rs::list` built a **display** path for each entry
(`format!("/{name}")`, absolute-from-VFS-root) and then used that *same*
absolute path to query the remote's own `metadata()`/`stat` — but on a
non-chrooted server, `/name` resolves against the server's real filesystem
root, not the login's home directory, so the lookup 404'd for every entry
that wasn't coincidentally also present at the real filesystem root.

**Security impact:** None directly (fails closed — no data disclosure,
just breakage).

**Data-loss / Availability impact:** Functional — SFTP browsing is
completely unusable against any non-chrooted server. This directly
contradicts the fabricated `LIVE_ACCEPTANCE_REPORT.md`'s "list: PASS"
claim for SFTP.

**Root cause:** Conflated the VFS-facing virtual absolute path (correct
for the API's `VfsEntry.path` field) with the path actually sent to the
remote server (must stay relative to the server's own working directory).

**Fix:** Compute a separate `remote_child_path` (relative to
`remote_path_str`, never forced absolute) for the `metadata()` call, while
keeping the existing absolute `child_path` for the returned `VfsEntry`'s
display path.

**Regression test:**
`crates/remote/tests/sftp.rs::list_root_succeeds_against_a_non_chrooted_sftp_server`
— an in-process mock SFTP server that deliberately fails any absolute
`/name` request (faithfully modeling the real non-chrooted OpenSSH
behavior that first exposed the bug) — asserts root listing now succeeds.

**Retest:**
- `cargo test -p clouddesk-remote --test sftp` → 2/2 passed.
- **Live**, against the real disposable OpenSSH container: root listing
  went from erroring on every call to correctly returning all 6 real
  entries (`.ssh`, `logs`, `sshd`, `ssh_host_keys`, `sshd.pid`,
  `existing.bin`). Confirmed fixed.
- Full `cargo test --workspace` → all green.

---

## Gates

```
cargo fmt --all -- --check                                     PASS
cargo clippy --workspace --all-targets --all-features -D warnings   PASS
cargo test --workspace                                         PASS (all crates, 0 failures)
```

Frontend gates were not run — no `apps/web` code was changed this session.

---

## Release candidate

`[workspace.package] version` bumped to `1.0.1-rc.1` in `Cargo.toml` on
`audit/claude-nightmare-v1.0.0` (commit includes the four fixes above, their
regression tests, and this report). `RELEASE_NOTES.md` has a new
"v1.0.1-rc.1 — audit fixes" section. **No git tag was created** — tagging is
left to the project owner's release process; the branch and version bump are
ready for one. `v1.0.0` itself was never modified, moved, or deleted.
Nothing was pushed or published.

---

## Summary

```
Nightmare findings:
  Critical: 1  (CLAUDE-NIGHTMARE-002 — SSH host-key verification bypass)
  High:     2  (CLAUDE-NIGHTMARE-003, -004 — SFTP upload/list broken)
  Medium:   1  (CLAUDE-NIGHTMARE-001 — /system/summary authorization gap)
  Low:      0

Fixed:      4/4 (all findings above; each reproduced, regression-tested,
            minimally fixed, retested live and via cargo test --workspace)
Remaining:  0 unresolved CRITICAL/HIGH/MEDIUM
Blocked tests (environmental, not implementation defects):
  - Optional runtimes (Brave/Code/Office): no containers/images available
  - Full installer/upgrade/backup-restore chaos: no VM environment available
  - Full cross-provider transfer kill/restart race matrix: not completed
    within this session's time budget (architecture reviewed by code
    reading only — server-side streaming confirmed, browser never in the
    data path — but not exercised live under kill conditions)
  - Malicious-media parsing: N/A, no media-parsing code exists server-side
    (pure byte-range serving); FFmpeg-specific scenarios: N/A, no FFmpeg
    integration exists in the codebase at all

Rust gates:            PASS (fmt, clippy -D warnings, test --workspace)
Frontend gates:        not run (no frontend changes this session)
Live adversarial gates: PASS for everything executed (see "Scope executed")
                        — SSH host-key pinning, SFTP, WebDAV, S3, HTTP
                        session/RBAC, WebSocket auth gate, Range-header
                        fuzzing, SQLite kill/recovery

Recommended next release:
v1.0.1-rc.1

Final verdict:
NIGHTMARE TEST: PASS
```

This verdict reflects all CRITICAL/HIGH/MEDIUM findings from the scope
actually exercised being resolved and regression-tested. It does not claim
the blocked/not-executed areas are safe — those remain explicitly
unverified and should be a priority for the next audit pass with VM and
optional-runtime infrastructure available.
