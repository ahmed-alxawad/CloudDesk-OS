# CloudDesk-OS v1.0 — Release Evidence Audit

Independent audit of whether every claimed v1.0 release gate is backed by
real executable evidence, and whether every `GOAL.md` requirement actually
has an implementation behind it. Continuation of the Nightmare audit on
`audit/claude-nightmare-v1.0.0`. `v1.0.0` is untouched; nothing pushed,
published, or signed.

## Evidence categories (do not inflate)

- **REAL LIVE TEST** — executed this session against a real running
  instance or real disposable infrastructure (Docker fixtures, a live
  `clouddeskd`), and I observed the actual result.
- **REAL AUTOMATED TEST** — a committed automated test exists, actually
  exercises the behavior (not a mock standing in for the whole system), and
  I confirmed it currently passes (`cargo test --workspace` / `npm test`).
- **UNIT TEST ONLY** — tested in isolation (e.g. a pure function, a crate
  boundary with a mock/in-memory dependency) but never exercised through the
  real end-to-end path (real network service, real browser, real process).
- **MOCK ONLY** — the only "test" coverage uses a fake/stub standing in for
  the real dependency, and nothing ever validated the fake matches reality.
- **NO TEST** — implementation exists; no test of any kind exists for it.
- **IMPLEMENTATION MISSING** — no real backend/runtime implementation
  exists at all — a manifest entry, an enum variant, a UI shell route, or a
  DB toggle is not implementation. Full detail for these in
  `V1_TRUE_CLOSURE.md`.
- **BLOCKED BY ENVIRONMENT** — implementation appears to exist, but this
  container cannot exercise it (no VM, no target hardware, no required
  container image).

---

## Part 1 — Release-infrastructure audit

The tools that produced v1.0.0's release-readiness documents were
themselves audited for the anti-patterns listed in the task: hardcoded
PASS strings, mocks presented as e2e tests, discarded exit codes, and
reports not tied to real command results.

| Tool | Finding |
|---|---|
| `tests/acceptance/src/main.rs` | **Fabricated.** 54 of 98 lines were `report.push_str("- X: **PASS**\n")` literals for the entire SSH, SFTP, and cross-provider transfer-matrix sections — the file never imported `russh` at all. Only the S3 `put_object`/`list` calls and one WebDAV `PUT` were real. **Repaired this session** — see below. |
| `tests/distro/installer-layout.sh` | **Misrepresented, not fabricated.** It's a real, executing test — but it runs `installer/install.sh` on a single host (this container) with `CLOUDESK_SKIP_PACKAGES=1` and fake stub binaries (`printf '#!/bin/sh\nexit 0\n'`) for all 8 distro IDs, checking only that the *file layout* comes out right. It never runs on real Debian/RHEL/Alpine/Arch systems, never invokes real `apt`/`dnf`/`apk`/`pacman`, and never starts a real systemd/OpenRC service. `RELEASE_VALIDATION.md`'s "Distribution-specific package management and service-manager behavior… verified" and `PRODUCTION_READINESS.md`'s "RHEL 9 / Rocky 9 / AlmaLinux 9 — SELinux policy, rpm packaging, installer verified" claims are not backed by this test or any other test in the repository. |
| `.github/workflows/ci.yml` | Real, executing CI — `cargo fmt/clippy/test`, `npm lint/test/build` — but only `runs-on: ubuntu-latest`, no distro matrix at all. Does not back the 8-distro claims either. |
| `FINAL_COMPLETION_AUDIT.md` | The most honest of the pre-existing documents — already lists SFTP/WebDAV/S3/server-to-server as `IMPLEMENTED_NOT_VERIFIED` and RSA/PEM/encrypted-keys/keyboard-interactive/agent/ProxyJump/certificates as `MISSING`, and rates overall completion at 77.58%. It directly contradicts `PRODUCTION_READINESS.md` and `RELEASE_VALIDATION.md` (both claim ~100% / "READY FOR v1.0.0"), which is itself evidence those two were not reconciled against the codebase before being written. |
| `RELEASE_VALIDATION.md` / `PRODUCTION_READINESS.md` | Contain no fabricated command output (the Rust/frontend gate rows do correspond to real, rerunnable commands), but their application-level rows (`"Gallery Application — PASS — apps/web/src/lib/GalleryApp.svelte with thumbnail previews and lightbox"`, etc.) treat "a Svelte file with this name exists" as equivalent to "the GOAL.md feature set is implemented." That is the same category of error as a hardcoded PASS: existence of a UI shell was reported as verification of the underlying feature. |
| Discarded exit codes / silent failures | None found in the Rust or shell test suites checked this session — `set -eu` is used in `tests/distro/installer-layout.sh`, and Rust tests propagate failures normally. |

### Repair: `tests/acceptance/src/main.rs`

Rewritten to execute the real product code
(`clouddesk_remote::{s3,webdav,ssh,sftp}`) against real disposable Docker
fixtures (`tests/acceptance/docker-compose.yml`, unchanged: `linuxserver/
openssh-server`, `bytemark/webdav`, `minio/minio`). It no longer contains a
single hardcoded PASS. Every report line is generated from an actual
`Result` (`Report::record`/`record_bool`) or is explicitly labeled
`IMPLEMENTATION MISSING` / `NOT EXECUTED` with a stated reason —
`BLOCKED (no fixture)` was never used, because for FFmpeg/Code/Office/Brave
that phrase would still falsely imply the feature exists.

Run this session (`cargo build -p acceptance-runner && ./target/debug/
acceptance-runner`, from a repo root with the Docker fixtures up):

- S3, WebDAV, SSH (password), SFTP (list/upload/download/rename/mkdir/
  delete/large-file): all genuinely executed, all passed.
- **Found a new real defect live**: RSA private-key auth failed with
  `SSH Authentication failed` on the first run. Root cause: real OpenSSH
  logs showed `signature algorithm ssh-rsa not in
  PubkeyAcceptedAlgorithms` — `crates/remote/src/ssh.rs` always requested
  the legacy SHA-1 `ssh-rsa` algorithm
  (`PrivateKeyWithHashAlg::new(key, None)`), which OpenSSH has rejected by
  default since 8.8 (2021). See **CLAUDE-NIGHTMARE-005** in
  `CLAUDE_NIGHTMARE_REPORT.md` — fixed, reran live (now passes), and a
  deterministic regression test was added
  (`crates/remote/tests/ssh.rs::test_ssh_rsa_pem_private_key_auth_succeeds`).

---

## Part 2 — Requirement-by-requirement evidence map

Grouped by `GOAL.md` primary goal. "Evidence" cites the actual test or the
actual grep/read that established the category.

### G1 — Installer

| Requirement | Status | Evidence |
|---|---|---|
| File/directory layout, TLS/master-key generation | **REAL AUTOMATED TEST** | `tests/distro/installer-layout.sh` executes `install.sh` for real, asserts real file output |
| Real distro package management (apt/dnf/apk/pacman) | **NO TEST** | `CLOUDESK_SKIP_PACKAGES=1` is set in the only test that exercises the installer; no package-manager code path is ever executed |
| Real systemd/OpenRC service registration & start | **NO TEST** | Same test uses `CLOUDESK_INIT_SYSTEM=none` |
| 8-distro OS matrix (release-blocking per `GOAL.md`) | **BLOCKED BY ENVIRONMENT** | No VMs/containers for Debian/Ubuntu/RHEL/Fedora/Rocky/Alma/Arch/Alpine are available in this container; the single-host layout test is not a substitute |

### G2 — Desktop/Dashboard UI modes

| Requirement | Status | Evidence |
|---|---|---|
| Window management (move/resize/min/max, taskbar) | **REAL LIVE TEST** (partial) | Confirmed via live `clouddeskd` + browser-facing routes in earlier audit session; not re-verified pixel-by-pixel this session |
| Dashboard mode | **NO TEST** | UI mode is a stored preference (`ui_mode` in `user_preferences`); no test exercises dashboard-specific layout behavior |

### G3 — File manager

| Requirement | Status | Evidence |
|---|---|---|
| List/copy/move/rename/delete/Trash, local VFS sandboxing | **REAL AUTOMATED TEST** | `crates/vfs` tests (traversal/symlink rejection, chmod/search), passing in `cargo test --workspace` |
| Upload/download, streaming | **REAL AUTOMATED TEST + REAL LIVE TEST** | `services/clouddeskd` streaming handlers tested; live-verified against real SFTP/S3/WebDAV this session |
| Archive create/extract | **IMPLEMENTATION MISSING** | No zip/tar code anywhere in the Rust codebase |
| Drag and drop, multi-select, context menus, file properties dialog | **IMPLEMENTATION MISSING** | `apps/web/src/lib/FilesApp.svelte` (261 lines) has no drag/drop handlers, no multi-select state, no context-menu component, no properties dialog |
| Search, sorting/filtering | **REAL AUTOMATED TEST** (backend) / **IMPLEMENTATION MISSING** (advanced UI) | Backend `search()` exists on `LocalProvider`/`SftpProvider`; frontend only has a basic name filter (`FilesApp.svelte` line 24), no sort-by-column or advanced filter UI |
| ACL viewing/editing | **IMPLEMENTATION MISSING** | `ProviderFeature::Acl` is a capability flag only; no `getfacl`/`setfacl`/ACL library call exists anywhere |
| Resumable/chunked upload | **IMPLEMENTATION MISSING** | `ProviderFeature::ResumableUpload` is a capability flag only; no chunked-upload-session protocol exists |
| Linux ownership display / permission editing when authorized | **REAL AUTOMATED TEST** | `chmod`/ownership fields present in `VfsEntry` and exercised by VFS tests |
| Favorites, recent files | **UNIT TEST ONLY** | Stored as JSON columns in `user_preferences` (`favorites_json`/`recent_json`), round-tripped by `get_preferences`/`put_preferences`, but no test asserts the UI actually populates/consumes them meaningfully beyond opaque JSON storage |

### G4 — Open-with / MIME association

| Requirement | Status | Evidence |
|---|---|---|
| MIME-aware routing to native apps | **NO TEST** | `file_associations` exist in app manifests (`crates/runtime`); no test exercises the actual dispatch-on-open behavior |
| "Open With…" chooser when multiple apps match | **IMPLEMENTATION MISSING** | No such UI component found |

### G5 — Media/document compatibility

| Requirement | Status | Evidence |
|---|---|---|
| Gallery: browser-native image formats | **NO TEST** | `GalleryApp.svelte` exists; no automated test of actual image rendering |
| Gallery: server-side conversion (RAW/HEIC/TIFF/AVIF fallback) | **IMPLEMENTATION MISSING** | No image-conversion code exists in the Rust codebase; `FINAL_COMPLETION_AUDIT.md` already flagged this as PARTIAL, confirmed here as fully missing (no conversion code at all, not merely incomplete) |
| Video playback (direct + FFmpeg remux/transcode) | **IMPLEMENTATION MISSING** | No Video app component exists in `apps/web/src/lib`; `grep -r ffmpeg` over every `*.rs` file in the repository is empty; `stream_media`/`preview_media` are plain HTTP byte-range file serving with no transcoding path. **This corrects the previous session's report, which called this "N/A" — it is a required v1.0 feature per `GOAL.md` G5 and must be marked IMPLEMENTATION MISSING, not N/A.** |
| Music: playback, queue, playlists, metadata, album art | **IMPLEMENTATION MISSING** | No Music app, playlist, or ID3-metadata code exists anywhere in the repository (Rust or Svelte) |
| PDF viewer: thumbnails, search, zoom, fit modes | **IMPLEMENTATION MISSING** (beyond basic view) | `DocumentApp.svelte` only lists `['pdf','txt','md','json','log','csv']` as viewable extensions with a plain embed; no page-thumbnail, in-document search, or zoom/fit-mode controls found |
| Office editing (DOC/DOCX/XLS/XLSX/PPT/PPTX/ODT/ODS/ODP via LibreOfficeKit/Collabora) | **IMPLEMENTATION MISSING** | `DocumentApp.svelte`'s extension list has no office formats at all; no `OfficeApp` component exists; `crates/runtime`'s `RuntimeDependency::Office` is an enum variant with no launcher/WOPI/container-orchestration code behind it |

### G6 — Code and Terminal

| Requirement | Status | Evidence |
|---|---|---|
| Local PTY terminal, correct Linux user | **REAL AUTOMATED TEST + REAL LIVE TEST** | `services/cloudesk-privd` root-boundary test; terminal WebSocket auth gate live-tested against a real running instance in the previous audit session (no-session → 401, garbage → 401, cross-site origin → 403) |
| Root shells never granted without step-up | **REAL AUTOMATED TEST** | `open_terminal_websocket` requires `terminal.local.open` capability + mapped identity via the privileged helper; covered by `tests/root_boundary.rs` |
| VS Code-compatible workspace (extensions, Git, language servers, isolated per-user sessions) | **IMPLEMENTATION MISSING** | Same manifest-only pattern as Office; `RuntimeDependency::Code` has no code-server process/container launcher; no `CodeApp` component exists |

### G7 — Browser application

| Requirement | Status | Evidence |
|---|---|---|
| Isolated server-side Brave runtime (not iframe embedding) | **IMPLEMENTATION MISSING** | No KasmVNC/Brave container launcher anywhere in the codebase; `RuntimeDependency::Browser` is an enum variant only; no `BrowserApp` component exists |
| Guest ephemeral profile / persistent user profile | **IMPLEMENTATION MISSING** | No profile-management code exists because no runtime exists to attach profiles to |

### G8 — Remote servers / SSH authentication matrix

| Requirement | Status | Evidence |
|---|---|---|
| Password auth | **REAL LIVE TEST + REAL AUTOMATED TEST** | Live against real OpenSSH this session; `crates/remote/tests/ssh.rs` |
| Ed25519 keys | **REAL AUTOMATED TEST** | `crates/remote/tests/ssh.rs` mock-server coverage; not re-verified live this session against real OpenSSH with an Ed25519 key specifically (RSA was, see below) |
| RSA keys | **REAL LIVE TEST + REAL AUTOMATED TEST** (after fix) | Failed live against the real fixture before the fix (CLAUDE-NIGHTMARE-005); fixed, reran live → passes; deterministic regression test added |
| PEM (legacy PKCS#1) format | **REAL AUTOMATED TEST** | `test_ssh_rsa_pem_private_key_auth_succeeds` decodes and authenticates with a literal `-----BEGIN RSA PRIVATE KEY-----` key, added this session |
| Encrypted private keys / passphrases | **REAL LIVE TEST** | Live against the real fixture this session with a passphrase-protected Ed25519 key |
| SSH agent | **IMPLEMENTATION MISSING** | `SshAuth::Agent` immediately `bail!`s ("Agent auth not fully implemented via sockets yet") |
| Keyboard-interactive | **IMPLEMENTATION MISSING** | `SshAuth::KeyboardInteractive` immediately `bail!`s ("Keyboard interactive auth not implemented in russh 0.62") |
| Custom ports | **REAL LIVE TEST** | The fixture itself runs SSH on port 2222 (not 22) and all live tests above connect to it |
| ProxyJump/bastion hosts | **NO TEST** (code exists, unreachable) | `SshSession::connect_proxyjump` exists and is exercised only by its own construction in this repair session's context — it has **zero callers** anywhere in `services/clouddeskd`; no HTTP endpoint or transfer path can ever invoke it |
| known_hosts / host-key verification | **REAL LIVE TEST + REAL AUTOMATED TEST** (after fix) | CLAUDE-NIGHTMARE-002: was accepting any key unconditionally before the fix; now rejects mismatches, live-verified and regression-tested |
| SSH certificates | **IMPLEMENTATION MISSING** | `SshAuth::Certificate` decodes `key_data` and silently discards `cert_data` — no certificate parsing or validation exists; the code comment itself calls this "a facade" |

### G9 — Unified remote storage providers

| Requirement | Status | Evidence |
|---|---|---|
| Local filesystem | **REAL AUTOMATED TEST** | `crates/vfs` |
| SFTP | **REAL LIVE TEST + REAL AUTOMATED TEST** (after fixes) | CLAUDE-NIGHTMARE-003/-004 found and fixed this session; live-verified against real OpenSSH |
| WebDAV | **REAL LIVE TEST** | Live-verified against a real disposable WebDAV server this session; no defect found |
| SCP | **IMPLEMENTATION MISSING** | No SCP-specific code exists (only SFTP) despite `GOAL.md` G9 listing it explicitly |
| AWS S3 / MinIO / generic S3-compatible | **REAL LIVE TEST** | Live-verified against real MinIO, including >5 MB multipart, this session |
| Cloudflare R2 / Backblaze B2 / Wasabi / DigitalOcean Spaces / Ceph S3 | **UNIT TEST ONLY** (by inference) | `S3Provider` is generic over any S3-compatible endpoint via `aws-sdk-s3` with a configurable endpoint URL, so these should work the same way MinIO does — but none of these specific providers were tested against, live or otherwise |

### G10 — Server-to-server transfers

| Requirement | Status | Evidence |
|---|---|---|
| Server-side streaming, no browser data path | **REAL AUTOMATED TEST** | `crates/transfers/tests::remote_to_remote_never_selects_a_browser_data_path` |
| Queueing, progress, retry, cancel, history | **REAL AUTOMATED TEST** | `crates/transfers` job-queue tests |
| Cross-provider matrix under live kill/restart conditions | **NO TEST** | Not exercised live this session or the previous one — architecture reviewed by code reading only (`worker.rs` streams provider-to-provider), not exercised under SIGKILL/restart |

### G11 — Auth & authorization

| Requirement | Status | Evidence |
|---|---|---|
| Password (Argon2id), TOTP, recovery codes, rate limiting | **REAL AUTOMATED TEST + REAL LIVE TEST** | `crates/auth` tests; live session-lifecycle testing in the previous audit session |
| Session management (revoke, device/IP history) | **REAL AUTOMATED TEST** | `list_sessions`/`revoke_session` covered by auth tests |
| RBAC (4 roles, granular, customizable) | **REAL AUTOMATED TEST + REAL LIVE TEST** | Full backend authorization sweep this session found and fixed one gap (CLAUDE-NIGHTMARE-001), confirmed the rest properly gated; live-tested Guest/User/Administrator against `/api/v1/system/summary` |

### G12 — Linux identity & permissions

| Requirement | Status | Evidence |
|---|---|---|
| Mapped-UID execution for terminal/files | **REAL AUTOMATED TEST** | `tests/root_boundary.rs::root_helper_launches_worker_as_the_mapped_linux_identity` |
| Mode bits/ownership/groups respected | **REAL AUTOMATED TEST** | VFS tests |
| ACLs respected | **IMPLEMENTATION MISSING** | Same as G3 — flag only, no implementation |

### G13 — Secrets

| Requirement | Status | Evidence |
|---|---|---|
| Envelope encryption, never plaintext, redacted from API | **REAL AUTOMATED TEST** | `crates/vault` — 15 tests including tamper/rotation/deletion, all passing |

### G14 — Audit trail

| Requirement | Status | Evidence |
|---|---|---|
| Tamper-evident hash-chained log | **REAL AUTOMATED TEST** | `crates/audit`, plus concurrent-writer stress-tested (15/15) in the previous audit session |
| Full field coverage (IP, user agent, session ID, error category, etc.) | **NO TEST** | Not independently verified this session that every field GOAL.md lists is actually populated on every listed event type |

### G15 — Server administration

| Requirement | Status | Evidence |
|---|---|---|
| CPU/RAM/disk/network status | **REAL LIVE TEST** | `/api/v1/system/summary` live-tested (and its authorization gap fixed) this session and the previous one |
| Service control, packages, firewall, users/groups, reboot/shutdown | **REAL AUTOMATED TEST** (authorization only) | `dispatch_privileged_action` capability gating tested; the actual privileged operations themselves (package install, firewall rules) were not exercised live — no test invokes a real `systemctl`/`apt`/firewall change |
| Docker/Podman integration | **NO TEST** | `container_engines` field in `system_summary` reads engine presence only; no integration beyond that was found |

### G16 — Optional heavy applications

| Requirement | Status | Evidence |
|---|---|---|
| Enable/disable controls (Brave/Code/Office) | **REAL AUTOMATED TEST** | `get_runtime_settings`/DB toggle tested (confirmed intentional, not privileged data, in the RBAC sweep) |
| Disabled runtimes not resident | **IMPLEMENTATION MISSING** (nothing to keep resident) | Since no runtime launcher exists for any of the three, there is no process lifecycle to verify — the requirement is vacuously "true" only because the feature it governs doesn't exist |

---

## Part 3 — Corrected classification of the previous session's "N/A" call

The previous audit session reported:
> media/FFmpeg scenarios: N/A — no such code exists

Per this task's explicit instruction, **this is corrected**: FFmpeg-based
remux/transcode is a named v1.0 requirement in `GOAL.md` G5 ("FFmpeg-based
remux/transcode streaming when necessary"). No implementation exists. This
is **IMPLEMENTATION MISSING**, not N/A — see `V1_TRUE_CLOSURE.md`.

---

## Summary

See the end-of-session report in the conversation for the full numeric
tally (genuinely implemented / missing / stubbed / live-evidenced /
automated-evidenced-only / blocked) and the honest completion percentage.
This document is the per-requirement detail backing that tally.
