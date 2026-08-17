# CloudDesk-OS v1.0.0 — Disaster/Nightmare Adversarial Test Report

```
Release under test:  v1.0.0
Release commit:      9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2
Immutable tag:       v1.0.0 (untouched)
Audit branch:        audit/claude-nightmare-v1.0.0
```

Ran `/disaster-test` and `/nightmare-test` back to back in this environment. Both
were freshly authored this session (no pre-existing installed Disaster/Nightmare
commands were found before that).

## Environment note

No Rust toolchain was preinstalled in this container. Installed `rustc`/`cargo`
1.97.1 via `rustup` in user space (no `sudo`, no system changes) so the actual
workspace could be built and driven live instead of only read. `cargo run`
itself hung indefinitely for unrelated sandboxing reasons in this container;
all live testing below uses the compiled `target/debug/clouddeskd` binary
directly, which behaved normally.

## Scope actually executed (be honest about coverage)

**Executed live, against real running/compiled code, in disposable temp dirs
only:**
- Full `cargo build --workspace` + `cargo test --workspace` baseline (passed
  clean before any changes).
- SQLite: 40× SIGKILL of `clouddeskd migrate` mid-run against a fresh DB,
  followed by a clean migrate — recovered every time, `PRAGMA integrity_check`
  clean, final migration succeeded.
- SQLite: repeated (15×) re-run of the existing concurrent-writer regression
  test (`clouddesk-audit::concurrent_writers_produce_one_linear_chain`,
  24 concurrent async writers) to check for flakiness in the lock-contention
  fix already on this branch (`ff84288`) — 15/15 passed.
- Live HTTP session lifecycle against a real `clouddeskd serve` instance
  (disposable config/DB/keys under `/tmp`): bootstrap, login, cookie
  attributes, `/auth/me`, logout, replay of the revoked session token,
  garbage/empty/SQLi-shaped cookie values, brute-force login (account+IP
  throttling).
- Live RBAC/cross-user sweep: created a Guest-role user via the real API,
  attempted admin-only actions (create user, self-escalate to administrator,
  reset another user's TOTP, read host summary) — **found and fixed one real
  defect (below)**.
- Applied the fix, added a regression test, reran the single test, the full
  crate's test binary, and the full workspace suite — all green.

**Not executed this pass** (no fabricated coverage): SSH/SFTP/WebDAV/S3
against live fixtures, transfer-worker kill/restart races, WebSocket/terminal
fuzzing, media/FFmpeg attacks, optional-runtime (Brave/Code/Office) crash
tests, installer/backup/restore interruption, and disk-exhaustion (no
unprivileged way to create a size-bounded disposable filesystem in this
container — a real `mount -o size=` tmpfs needs `sudo`, which wasn't
available, so it was skipped rather than risking the shared `/tmp`). These
subsystems' existing unit/integration coverage (vault tamper tests, VFS
traversal tests, privd IPC boundary tests, WAL/busy-timeout config) did run
and pass as part of `cargo test --workspace`, but were not independently
attacked live. Treat those areas as **unverified this session**, not passing.

---

## Findings

### ID: CLAUDE-NIGHTMARE-001
```
Severity:           MEDIUM
Subsystem:          Authorization / RBAC (Host Administration)
Release affected:   v1.0.0
```
**Reproduction:**
1. Bootstrap a CloudDesk instance, log in as the administrator, step up.
2. Create a user with only the `guest` role (`POST /api/v1/users` with
   `"role_ids":["guest"]`) — Guest's only default capability is
   `files.local.read`.
3. Log in as that guest user.
4. `GET /api/v1/system/summary` with the guest's session cookie.

**Expected:** `403 permission denied`, matching sibling host-administration
endpoints `POST /api/v1/system/services/control` and
`POST /api/v1/system/power`, which both require `system.services.manage` /
`system.power.manage` (administrator-only capabilities) via
`dispatch_privileged_action`.

**Actual (before fix):** `200 OK` with a JSON body containing hostname,
kernel release, uptime, load average, total/available memory, and container
engine availability (`docker`/`podman`). The handler
(`services/clouddeskd/src/lib.rs::system_summary`) called `principal()` to
confirm the caller was *authenticated* but never checked any capability —
unlike every other host-administration handler in the file.

**Security impact:** Any authenticated user, including the lowest-privilege
Guest role, can read host telemetry (kernel version — useful for exploit
targeting, live memory pressure, container runtime presence) that the
Settings/Administration panel (`apps/web/src/lib/SettingsApp.svelte`) only
surfaces alongside admin-only service/power controls — i.e. the frontend
hides the panel for non-admins, but the backend endpoint was reachable
directly, exactly matching handoff priority-target #15 ("direct API call
bypassing hidden UI controls") and violating #13 ("Guest reaching
Manager/Admin endpoints directly").

**Data-loss impact:** None — read-only endpoint.

**Availability impact:** None.

**Root cause:** `system_summary` discarded its `principal` binding
(`let _principal = principal(&state, &headers).await?;`) instead of checking
`principal.can(<capability>)`, the pattern used by every other privileged
handler in the same file.

**Fix (on `audit/claude-nightmare-v1.0.0` only):**
`services/clouddeskd/src/lib.rs` — bind the principal and require
`system.services.manage` (the same capability its sibling host-admin
endpoints gate on) before reading host state:
```rust
let principal = principal(&state, &headers).await?;
if !principal.can("system.services.manage") {
    return Err(ApiError::forbidden());
}
```

**Regression test:**
`services/clouddeskd/tests/auth_api.rs::guest_role_cannot_read_system_summary`
— bootstraps an admin, creates a Guest-role user via the real HTTP API, and
asserts the admin still gets `200` from `/api/v1/system/summary` while the
guest gets `403`.

**Retest:**
- `cargo test -p clouddeskd --test auth_api` → 2/2 passed.
- `cargo test --workspace` (full suite, post-fix) → all passed, no
  regressions.
- Live retest against a running disposable `clouddeskd` instance with real
  guest/admin sessions: guest → `403 {"error":"permission denied"}`,
  admin → `200`. Confirmed fixed.

---

## Final Verdict

No unresolved CRITICAL or HIGH findings. One genuine MEDIUM authorization
defect was found, fixed minimally on `audit/claude-nightmare-v1.0.0`, and
regression-tested; the full workspace suite passes after the fix.

This verdict covers only the scope executed this session (see above) — large
parts of the priority-target list (129–135 install/upgrade/backup, 52–99
SSH/SFTP/WebDAV/S3/media, 90–105 HTTP-range/terminal, 106–128 optional
runtimes/resource exhaustion) were not independently attacked live and should
not be assumed clean beyond their existing unit-test coverage.

```
NIGHTMARE TEST: PASS
```
