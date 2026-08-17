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

## 7. Archive create/extract

- **Requirement:** `GOAL.md` G3 — "archive creation/extraction."
- **Current reality:** No zip/tar handling exists anywhere in the Rust
  codebase.
- **Missing implementation:** Server-side archive creation (zip at
  minimum) and extraction, with Zip Slip protection (path traversal
  during extraction into the VFS sandbox).
- **Required test:** Live create-then-extract round-trip; a hostile
  archive (Zip Slip payload) is rejected without escaping the VFS root.
- **Release severity:** **BLOCKING** (explicit G3 bullet).

---

## 8. ACL viewing/editing

- **Requirement:** `GOAL.md` G3, G12 — "ACL viewing/editing when
  authorized," "CloudDesk must respect… ACLs."
- **Current reality:** `ProviderFeature::Acl` is declared as a capability
  flag on `LocalProvider` (`crates/vfs/src/lib.rs`) but is never backed by
  a `getfacl`/`setfacl` call or any ACL library (e.g. `posix-acl`).
- **Missing implementation:** Real POSIX ACL read/write behind the flag.
- **Required test:** Live ACL round-trip on a real Linux filesystem entry
  with a non-trivial ACL, verified via `getfacl` independently of
  CloudDesk.
- **Release severity:** **BLOCKING** (explicit G3/G12 requirement).

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

## 14. ProxyJump / bastion hosts (product wiring)

- **Requirement:** `GOAL.md` G8 — "ProxyJump/bastion hosts."
- **Current reality:** `SshSession::connect_proxyjump` exists in
  `crates/remote/src/ssh.rs` and is unit-tested at the crate level (used
  by this audit's new host-key-pinning regression coverage), but has
  **zero callers** anywhere in `services/clouddeskd` — no HTTP endpoint,
  no transfer path, and no terminal-open path ever constructs a
  ProxyJump connection. A user cannot reach this code through the
  product.
- **Missing implementation:** Wiring `connect_proxyjump` into the actual
  connection-establishment path (`worker.rs`, terminal open) when a
  `RemoteServer.proxy_jump_server_id` is set — that field already exists
  in the data model (`crates/remote/src/lib.rs`) but nothing reads it to
  drive a bastion hop.
- **Required test:** Live transfer/terminal session through a real
  bastion host, plus the already-covered failure modes (bastion dies
  mid-session, broken hop).
- **Release severity:** **BLOCKING** — the data model implies this is
  supported (`proxy_jump_server_id` is a real, saved field), but using it
  does nothing.

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

## Not included above (real implementations, listed for contrast)

To be clear this isn't "everything is missing" — the following do have
real, live-verified implementations and are not in this closure list:
local/SFTP/WebDAV/S3 file access, server-to-server transfer engine
architecture, SSH password/Ed25519/RSA(fixed)/PEM/encrypted-key auth,
SSH host-key pinning (fixed this session), the entire auth/RBAC/vault/
audit security core, the Terminal application, and the core desktop shell.
See `RELEASE_EVIDENCE_AUDIT.md` for the full requirement-by-requirement
map including these.
