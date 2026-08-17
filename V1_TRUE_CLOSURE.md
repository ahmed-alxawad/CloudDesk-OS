# CloudDesk-OS v1.0 — True Closure: Missing Implementations

Every `GOAL.md` requirement found to have **no real backend/runtime
implementation** — a manifest entry, an enum variant, a UI shell route, or
a database toggle does not count as implementation. Full evidence and the
requirements that *do* have real implementations are in
`RELEASE_EVIDENCE_AUDIT.md`. This document exists to make the release-
blocking gap list impossible to miss.

---

## 1. FFmpeg probing / remux / transcoding

- **Requirement:** `GOAL.md` G5 — "Provide a VLC-like playback experience
  using direct browser playback when possible and FFmpeg-based
  remux/transcode streaming when necessary."
- **Current reality:** `grep -r ffmpeg` over every `*.rs` file in the
  repository returns nothing. `stream_media`/`preview_media` in
  `services/clouddeskd/src/lib.rs` are plain HTTP byte-range file serving
  of whatever bytes are already on disk — no probing, no remux, no
  transcode, no codec detection.
- **Missing implementation:** An FFmpeg-invoking service (probe for
  codec/container info, on-demand remux to a browser-playable container,
  transcode fallback for unsupported codecs, with process time/resource
  limits and cancellation).
- **Required test:** Live test against real video files: direct-playable
  container streams unmodified; an unsupported codec triggers a transcode
  that produces valid output; a hung/runaway FFmpeg process is killed by a
  timeout; concurrent transcode requests are bounded.
- **Release severity:** **BLOCKING.** Explicitly named, non-optional v1.0
  requirement. (Corrects the previous audit session's "N/A" classification
  — this is missing, not out of scope.)

---

## 2. Video player application

- **Requirement:** `GOAL.md` G5 (Video), G4 (video → Video app routing).
- **Current reality:** No Video app component exists in
  `apps/web/src/lib`. `GalleryApp.svelte` (image-only, 263 lines) has no
  video-related code.
- **Missing implementation:** A Video player Svelte component with
  playback controls, seeking, and a data path to the (also missing)
  FFmpeg remux/transcode backend.
- **Required test:** Live playback of a direct-compatible file and a
  transcode-required file through a real browser session.
- **Release severity:** **BLOCKING.**

---

## 3. Music application

- **Requirement:** `GOAL.md` G5 (Music) — playback, persistent queue,
  playlists, artists/albums, folder browsing, metadata, album art,
  favorites, recent playback, search.
- **Current reality:** No Music app, playlist data model, or ID3-metadata
  parsing exists anywhere in the repository (Rust or Svelte).
- **Missing implementation:** The entire application — audio playback UI,
  a playlist/queue data model and API, metadata (ID3/Vorbis comment)
  extraction, album-art extraction/caching.
- **Required test:** Live playback + playlist persistence across a
  session restart, verified against real audio files.
- **Release severity:** **BLOCKING.**

---

## 4. Office editing runtime (LibreOffice/Collabora/WOPI)

- **Requirement:** `GOAL.md` G5 (Office) — browser editing of DOC/DOCX,
  XLS/XLSX, PPT/PPTX, ODT/ODS/ODP via "LibreOfficeKit/Collabora-compatible
  technology behind a CloudDesk application shell."
- **Current reality:** `crates/runtime` is a JSON manifest schema/
  validator only (116 lines: parses `id`/`name`/`icon`/`route`/
  `required_permissions`/`file_associations`/`enabled`, plus a
  `RuntimeDependency` enum with a `Office` variant). No launcher, no
  container orchestration, no WOPI host/client implementation exists.
  `DocumentApp.svelte`'s viewable-extension list
  (`['pdf','txt','md','json','log','csv']`) contains no office formats at
  all. No `OfficeApp` component exists.
- **Missing implementation:** A Collabora Online (or equivalent
  LibreOfficeKit) container launcher, a WOPI host implementation bridging
  CloudDesk's VFS/auth to the editing session, and an `OfficeApp` frontend
  component embedding it.
- **Required test:** Live open/edit/save/reopen round-trip for one file
  per format (DOCX/XLSX/PPTX at minimum) through a real browser session
  against a real Collabora container; VFS authorization still enforced
  inside the editing session.
- **Release severity:** **BLOCKING.**

---

## 5. VS Code-compatible runtime

- **Requirement:** `GOAL.md` G6 (Code) — extensions, integrated terminal,
  Git, GitHub/GitLab workflows, multiple workspaces, language servers,
  debugging, per-user isolated sessions.
- **Current reality:** Same manifest-only pattern as Office —
  `RuntimeDependency::Code` is an enum variant with no code-server
  process/container launcher behind it. No `CodeApp` component exists in
  `apps/web/src/lib`.
- **Missing implementation:** A code-server (or equivalent) container
  launcher with per-user isolated sessions, workspace mounting scoped to
  the user's authorized filesystem roots, and a `CodeApp` frontend
  component.
- **Required test:** Live launch, file edit + save round-trip, integrated
  terminal command execution, and a cross-user isolation check (User A
  cannot reach User B's workspace) through a real browser session against
  a real code-server container.
- **Release severity:** **BLOCKING.**

---

## 6. Brave remote-browser runtime

- **Requirement:** `GOAL.md` G7 — isolated server-side Brave runtime
  (not iframe embedding), persistent profile per user, ephemeral Guest
  profile, must not expose the Linux desktop.
- **Current reality:** Same manifest-only pattern; `RuntimeDependency::
  Browser` is an enum variant with no KasmVNC/Brave container launcher
  anywhere in the codebase. No `BrowserApp` component exists.
- **Missing implementation:** A KasmVNC-or-equivalent container launcher
  running an isolated Brave instance per session, a streaming
  (WebRTC/VNC-over-WebSocket) frontend component, profile persistence
  keyed to CloudDesk user identity, and proof the underlying host desktop
  is never exposed.
- **Required test:** Live launch, page load (including a JS-heavy site),
  tab management, and a Guest-profile-does-not-persist check through a
  real browser session against a real Brave/KasmVNC container.
- **Release severity:** **BLOCKING.**

---

## 7. Archive create/extract — **CLOSED**

- **Requirement:** `GOAL.md` G3 — "archive creation/extraction."
- **Status:** Implemented on `engineering/v1-true-closure`
  (`crates/vfs/src/archive.rs`), wired into `LocalFileOperation::
  CreateArchive`/`ExtractArchive` and reachable through the existing
  `/api/v1/files/local/actions` endpoint — no new HTTP route was needed,
  the generic dispatcher already covers it. Supports ZIP and tar.gz.
  Preserves relative paths from the VFS root; never follows a symlink
  source into the archive (skips it); rejects symlink/hard-link entries
  on extract; validates every extracted entry name (no `..`, no absolute
  path, no backslash/drive-letter trick, no embedded NUL) before any
  filesystem call; enforces an entry-count and a *decompressed-bytes*
  quota checked against bytes actually read out of the decompressor (not
  a trusted header field); cleans up everything written by a failed
  extraction. Minimal Files UI: "Create archive"/"Extract" toolbar
  buttons in `apps/web/src/lib/FilesApp.svelte`.
- **Test evidence:** `crates/vfs/tests/archive.rs`, 10 tests — ordinary
  ZIP and tar.gz round-trips (nested directories, multiple sources), Zip
  Slip traversal entry rejected, absolute-path entry rejected,
  Windows-drive-letter/backslash entry rejected, symlink entry rejected,
  partial-extraction cleanup verified, write-capability denial on both
  create and extract, and a symlinked source is never followed into the
  archive. Not yet live-tested through the real HTTP API / a real browser
  session — only through the `crates/vfs` function boundary directly.

---

## 8. ACL viewing/editing — **CLOSED**

- **Requirement:** `GOAL.md` G3, G12 — "ACL viewing/editing when
  authorized," "CloudDesk must respect… ACLs."
- **Status:** Implemented on `engineering/v1-true-closure`
  (`crates/vfs/src/acl.rs`), wired into `LocalFileOperation::ReadAcl`/
  `SetAcl`. Shells out to the standard `getfacl`/`setfacl` binaries with a
  fixed argv (never a shell, never string-interpolated). The target is
  opened through the same `cap_std`-sandboxed `Dir` every other operation
  uses, then its exact real path is recovered in-process via
  `readlink("/proc/self/fd/<n>")` and *that* resolved path — not the
  caller's string — is what's handed to the external tool, so a symlink
  anywhere in the caller-supplied path can't redirect the external tool
  (real bug caught and fixed during testing: pointing `getfacl`/`setfacl`
  directly at `/proc/self/fd/<n>` doesn't work, because a spawned child
  process doesn't inherit that fd number by default). `SetAcl` requires
  its own `files.permissions.change` capability (previously declared but
  never checked anywhere) rather than the blanket `files.local.write`
  every other mutation shares — administrator-only by default, matching
  the existing role-capability seed. Named-user/group qualifiers are
  validated against a conservative charset so a crafted name can't
  confuse `setfacl`'s own comma/colon spec parser. A missing `getfacl`/
  `setfacl` binary, or a filesystem that rejects the ACL call, reports
  `supported: false` rather than silently no-op'ing. Minimal Files UI: an
  "Access control list" section with a "Grant access…" prompt in the
  existing file-details/Properties panel.
- **Test evidence:** `crates/vfs/tests/acl.rs`, 6 tests, run against this
  container's real `getfacl`/`setfacl` (not mocked) — base-entry read,
  add/modify/remove a named-user grant to `nobody` with read-back
  verification after each step, `chmod` still works unaffected, path
  outside the authorized root denied, write-capability denial, and an
  unsafe qualifier name (containing `setfacl` delimiter characters)
  rejected before it reaches the external tool. Not yet live-tested
  through the real HTTP API / a real browser session — only through the
  `crates/vfs` function boundary directly. Symlink-escape denial is not
  independently tested for ACL specifically (relies on the same
  `cap_std::fs::Dir::open` sandboxing already covered by other tests in
  this crate, e.g. `provider_lists_and_mutates_only_inside_its_capability_root`).

---

## 9. Resumable/chunked upload — **CLOSED**

- **Requirement:** `GOAL.md` G3 — "large-file and resumable upload
  support."
- **Status:** Implemented on `engineering/v1-true-closure`
  (`b4a4660 feat(files): implement resumable local-file uploads`).
  `upload_sessions` table (migration `0009_upload_sessions.sql`) +
  `POST/PUT/GET/DELETE /api/v1/files/local/uploads[/...]` +
  `.../complete`. Chunks stream to a temp file under bounded memory;
  finalize re-validates the destination path, optionally verifies a
  declared sha256, and atomically renames into place. Authorization
  (`files.local.write` + session-owner match) is checked on every
  chunk/status/finalize/cancel request. An hourly background sweep
  deletes sessions abandoned for >24h.
- **Test evidence:** `services/clouddeskd/tests/resumable_upload.rs` — a
  real multi-chunk upload through the actual HTTP router with a
  simulated dropped connection and resume (not a mock), a
  checksum-mismatch rejection case, and a cross-user isolation case.
  Not yet tested against a real browser client over a real network
  connection (only the axum `Router::oneshot` HTTP surface) — the
  underlying chunk-streaming mechanism is the same one the existing
  one-shot upload endpoint already uses in production, so this is a
  reasonable but not exhaustive substitute for a true live-network test.

---

## 10. SCP transfers

- **Requirement:** `GOAL.md` G9 — "SCP where appropriate for transfers."
- **Current reality:** No SCP-specific code exists anywhere; only SFTP is
  implemented.
- **Missing implementation:** An SCP protocol client/transfer path (or a
  documented, owner-approved decision that SFTP fully supersedes it for
  v1.0, which would need to be reflected back into `GOAL.md` rather than
  left as a silent gap).
- **Required test:** Live SCP transfer against a real SSH server.
- **Release severity:** **MEDIUM** — GOAL.md's own qualifier ("where
  appropriate") leaves room for a documented substitution by SFTP, but no
  such decision is currently recorded anywhere in the spec or release
  docs; as written, this is an unmet requirement.

---

## 11. SSH agent forwarding

- **Requirement:** `GOAL.md` G8 — "SSH agent" listed as a required
  authentication method.
- **Current reality:** `SshAuth::Agent` in `crates/remote/src/ssh.rs`
  immediately returns an error: `"Agent auth not fully implemented via
  sockets yet"`.
- **Missing implementation:** Unix-socket agent forwarding/consumption
  (`SSH_AUTH_SOCK` protocol) integrated with `russh`'s agent-client
  support.
- **Required test:** Live auth against a real SSH server using a key held
  only in a running `ssh-agent`, not on disk.
- **Release severity:** **BLOCKING** (explicit G8 bullet).

---

## 12. Keyboard-interactive authentication

- **Requirement:** `GOAL.md` G8 — "keyboard-interactive" listed as a
  required authentication method.
- **Current reality:** `SshAuth::KeyboardInteractive` immediately returns
  an error: `"Keyboard interactive auth not implemented in russh 0.62"`.
- **Missing implementation:** Interactive prompt/response handling in the
  SSH client (present/relay server challenges, submit responses).
- **Required test:** Live auth against a real SSH server configured for
  keyboard-interactive (e.g. PAM-based OTP) — including malformed-prompt
  handling per the Nightmare handoff's own target list (#58).
- **Release severity:** **BLOCKING** (explicit G8 bullet).

---

## 13. SSH certificate authentication

- **Requirement:** `GOAL.md` G8 — "SSH certificates" listed as a required
  authentication method.
- **Current reality:** `SshAuth::Certificate` decodes only `key_data` and
  silently ignores `cert_data`, falling back to plain key auth. The
  source code's own comment calls this "an implemented facade for the
  spec requirement" — i.e., it was written to look complete without being
  complete.
- **Missing implementation:** Real OpenSSH certificate parsing and
  validation (CA trust, principal/validity checks) via `russh`'s
  certificate support.
- **Required test:** Live auth against a real SSH server using a
  certificate-signed key, including rejection of an expired or
  wrong-principal certificate.
- **Release severity:** **BLOCKING** (explicit G8 bullet; doubly so
  because the current code actively misrepresents itself as supporting
  this).

---

## 14. ProxyJump / bastion hosts (product wiring) — **CLOSED (for transfers/SFTP; remote terminal not yet wired — see #16)**

- **Requirement:** `GOAL.md` G8 — "ProxyJump/bastion hosts."
- **Status:** Implemented on `engineering/v1-true-closure`
  (`services/clouddeskd/src/worker.rs::resolve_ssh_session`), consumed by
  the SFTP/transfer connection path (`TransferWorker::get_provider`). When
  a target `RemoteServer.proxy_jump_server_id` is set, the bastion is
  independently resolved (ownership re-checked via `RemoteServerStore::get`,
  its own pinned host key verified, its own Vault credential revealed —
  never reusing the target's) and `SshSession::connect_proxyjump` is used
  instead of a direct connection. Chain depth is bounded to target + one
  bastion (`MAX_PROXY_CHAIN_HOPS = 2`): a bastion whose own
  `proxy_jump_server_id` is set is refused outright
  (`SshResolveError::ChainTooDeep`), which also rejects every A→B→A loop
  as a side effect. Self-reference is explicitly rejected. A cross-owner
  bastion reference is rejected both by `RemoteServerStore::create`
  (can't be legitimately saved) and independently by `resolve_ssh_session`
  itself (defense in depth, proven by forcing one directly into the
  database in a test). Real bug found and fixed in the disposable fixture
  itself while live-testing: the `linuxserver/openssh-server` image ships
  with `AllowTcpForwarding no`, which silently breaks ProxyJump's
  `direct-tcpip` channel — fixed via the image's documented
  `sshd_config.d` drop-in mechanism
  (`tests/acceptance/fixtures/sshd_config.d/proxyjump.conf`, bind-mounted
  in `docker-compose.yml`), not a one-off manual patch.
- **Test evidence:** `services/clouddeskd/tests/ssh_proxyjump.rs`, 12
  tests, against a real two-container bastion+target topology
  (`tests/acceptance/docker-compose.yml`: `openssh` as bastion,
  `openssh-target` deliberately given **no host port mapping** — reachable
  only through the bastion's compose-internal network, so a passing test
  proves the connection genuinely traversed client→bastion→target, not an
  independently-reachable "target"). Covers: valid ProxyJump connection
  and command execution; wrong bastion host key rejected; wrong target
  host key rejected (even through an already-trusted bastion); bastion
  auth failure rejected; target auth failure rejected; the target is
  provably unreachable without the bastion (topology sanity check);
  self-reference rejected; A→B→A loop rejected as chain-too-deep;
  cross-owner bastion reference rejected; deleting a bastion nulls the
  dependent's reference (`ON DELETE SET NULL`) rather than leaving it
  dangling; missing target rejected. Re-verified passing on a fully fresh
  `docker compose down -v && up -d` (i.e. the fixture fix is reproducible,
  not dependent on a manually-patched running container).
- **Not yet done:** live bastion-dies-mid-session / connection-storm /
  auth-timeout scenarios from the original task's regression list; remote
  terminal over ProxyJump (no remote-terminal-over-SSH feature exists in
  the product at all yet — see #16, a new item, not merely "ProxyJump not
  wired into it").

---

## 15. Real distro-matrix installer/service verification

- **Requirement:** `GOAL.md` "Official v1.0 OS Matrix" — 8 distributions
  are release-blocking; "The installer and CI must test
  distribution-specific package management and service-manager behavior."
- **Current reality:** The only installer test
  (`tests/distro/installer-layout.sh`) explicitly skips package
  installation (`CLOUDESK_SKIP_PACKAGES=1`) and service registration
  (`CLOUDESK_INIT_SYSTEM=none`), and runs entirely on this single host. CI
  (`.github/workflows/ci.yml`) has no distro matrix.
- **Missing implementation:** Not missing product code — missing *test
  infrastructure*: real per-distro CI runners (or containers) that
  actually invoke the package manager and start/stop the real service.
- **Required test:** Per the spec's own words — package-manager and
  service-manager behavior, on real (or realistically emulated) instances
  of all 8 listed distributions.
- **Release severity:** **BLOCKING** per `GOAL.md`'s explicit
  "release-blocking supported platforms" language, though this is a test-
  gap rather than a product-code gap; **BLOCKED BY ENVIRONMENT** for this
  audit session specifically (no such infrastructure available here).

---

## 16. Remote terminal over SSH (new item, discovered during Phase 2)

- **Requirement:** implied by `GOAL.md` G8 (Remote Servers app lists
  "Terminal" as a per-server action) and the engineering-checkpoint
  Phase 2 task list ("open a real remote PTY through the bastion").
- **Current reality:** `crates/remote/src/ssh.rs::SshSession` only offers
  `run_command` (single non-interactive command execution, buffers the
  full output). No PTY allocation (`request_pty`), no interactive shell
  channel, and no WebSocket (or any other) endpoint in `services/
  clouddeskd` exposes a remote-server terminal session. The **local**
  terminal (`/api/v1/terminal/ws`, `open_terminal_websocket`) is a
  completely separate, already-working feature (mapped-UID local PTY via
  `cloudesk-privd`) — it has nothing to do with SSH.
- **Missing implementation:** PTY request + interactive shell channel on
  `SshSession`, a new WebSocket (or equivalent streaming) endpoint that
  opens one via `resolve_ssh_session` (so it gets ProxyJump support for
  free), and frontend wiring in the Remote Servers app's existing
  "Terminal" action.
- **Required test:** live interactive command execution, resize, and
  clean teardown (no orphaned SSH channel/process) against a real
  disposable server, both directly and through a bastion.
- **Release severity:** **BLOCKING** per `GOAL.md` G8's per-server
  Terminal action.

---

## Not included above (real implementations, listed for contrast)

To be clear this isn't "everything is missing" — the following do have
real, live-verified implementations and are not in this closure list:
local/SFTP/WebDAV/S3 file access, server-to-server transfer engine
architecture, SSH password/Ed25519/RSA(fixed)/PEM/encrypted-key auth,
SSH host-key pinning (fixed this session), the entire auth/RBAC/vault/
audit security core, the Terminal application, and the core desktop shell.
See `RELEASE_EVIDENCE_AUDIT.md` for the full requirement-by-requirement
map including these.
