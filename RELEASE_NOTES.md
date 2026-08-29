# CloudDesk-OS v1.0.0 Release Notes

CloudDesk-OS v1.0.0 is the first production release of the lightweight, multi-user web desktop for Linux servers. It delivers a modern, secure, web-accessible workspace combining native browser applications, remote infrastructure access, isolated container runtimes, and strict operating-system-level privilege separation.

## Highlights

### 1. Multi-User Web Desktop Platform
* **Clean Web Shell**: Modern Svelte 5 desktop interface featuring responsive window management, taskbar, launcher, system tray, workspace manager, and dark/light system themes.
* **Role-Based Access Control (RBAC)**: Fine-grained permissions framework with role snapshots, granular capability checks, and session governance.
* **Two-Factor Authentication (2FA)**: RFC 6238 TOTP two-factor authentication with single-use cryptographic recovery codes and server-side rate-limited login throttling.

### 2. Native Applications & Media
* **Files / VFS**: High-performance Virtual File System with path traversal protection, symlink escape prevention, root capability sandboxing, and directory search.
* **Gallery**: Media browsing application supporting responsive image thumbnails and client-side format previews.
* **Audio & Video Player**: Streaming audio and video playback directly inside the desktop environment.
* **PDF & Document Viewer**: Integrated document and PDF viewing engine with in-window rendering.

### 3. Integrated Runtimes
* **Interactive Terminal**: Low-latency pseudo-terminal (PTY) session streaming over WebSockets with standard terminal emulation.
* **VS Code-Compatible Runtime**: Remote code editing and workspace development integration.
* **LibreOffice / Office Runtime**: Office document viewing and productivity suite runtime integration.
* **Brave Browser Runtime**: Isolated browser runtime integration for secure browsing sessions.

### 4. Remote Infrastructure & Transfers
* **SSH & SFTP**: Native SSH terminal sessions and SFTP client supporting password, RSA/Ed25519 public keys, encrypted passphrase-protected keys, and jump-host proxying.
* **WebDAV**: Remote WebDAV server client supporting browsing, upload, download, MKCOL, MOVE, and DELETE operations.
* **S3-Compatible Storage**: Object storage connectivity supporting listing, upload, download, and multipart streaming.
* **Server-to-Server Transfers**: Background transfer engine executing direct server-to-server transfers (SFTP ↔ SFTP, S3 ↔ S3, WebDAV ↔ SFTP) without routing intermediary payload data through the local browser client.

### 5. Security & Privilege Separation
* **Non-Root Core Service (`clouddeskd`)**: Refuses to run as root (`UID 0`) and enforces server-side permission validation on all endpoints.
* **Privileged Helper (`cloudesk-privd`)**: Isolated helper communicating over secure Unix domain sockets for minimal, strictly audited Linux administrative actions.
* **Vault Envelope Encryption**: AES-256-GCM envelope encryption for stored credentials and remote keys using master keys and dynamic data encryption keys (DEKs).
* **Tamper-Evident Audit Logging**: Cryptographic SHA-256 hash chain linking all security-critical system events with linear integrity verification and lock-contention backoff.

### 6. Linux Distribution Support & Efficiency
* **Distribution Families**: Validated across 8 major Linux distribution families (Debian, Ubuntu, Fedora, RHEL/Rocky/Alma, Alpine, Arch Linux, openSUSE, Amazon Linux).
* **Resource Footprint**: Minimal idle resource consumption (<30 MB baseline memory) and fast cold-start performance.

---

## v1.0.1-rc.1 — audit fixes (candidate, not yet released)

Prepared on `audit/claude-nightmare-v1.0.0` from an independent adversarial
audit of v1.0.0. `v1.0.0` itself is unchanged and remains the current
release; this is a candidate for the next patch release. Full findings in
`CLAUDE_NIGHTMARE_REPORT.md`.

Fixes:
* **CLAUDE-NIGHTMARE-001** (MEDIUM): `GET /api/v1/system/summary` was
  reachable by any authenticated user, including Guest, with no capability
  check — now requires `system.services.manage` like its sibling
  host-administration endpoints.
* **CLAUDE-NIGHTMARE-002** (CRITICAL): the SSH client accepted *any* host
  key unconditionally — a MITM'd or replaced remote host was silently
  trusted on every transfer/terminal connection. Real connections now
  reject a host key that doesn't match the one pinned when the remote
  server was saved.
* **CLAUDE-NIGHTMARE-003** (HIGH): SFTP upload could never create a file
  that didn't already exist on the remote (`OpenFlags::WRITE` only, no
  `CREATE`) — every upload of a new file failed. Fixed to create-or-overwrite.
* **CLAUDE-NIGHTMARE-004** (HIGH): SFTP directory listing failed entirely
  against any non-chrooted SFTP server (the common case) because per-entry
  metadata lookups used an absolute path assumed to equal the server's real
  filesystem root. Fixed to address entries relative to the server's own
  working directory.

Also documented (not a runtime defect, but a release-process integrity
finding): `tests/acceptance` — the tool that produced the v1.0.0
`LIVE_ACCEPTANCE_REPORT.md` — hardcodes `"**PASS**"` for its entire SSH,
SFTP, and cross-provider transfer-matrix sections without exercising any of
that code. Those v1.0.0 acceptance claims were not actually verified; this
audit verified the real code paths directly instead.

### Subsequent engineering-closure work (still part of the same v1.0.1-rc.1 candidate)

The nightmare-audit fixes above were the start, not the end, of closing
v1.0.1-rc.1. Later engineering-closure passes on `engineering/v1-true-closure`
added substantially to this candidate:

* **Multi-distribution release hardening (Phase 10)**: a pinned native musl
  release artifact for Alpine Linux (in addition to the existing pinned
  glibc artifact covering Debian/Ubuntu/Fedora/RHEL-family/Arch), full real
  OpenRC service-lifecycle evidence on Alpine, and two Alpine-specific
  installer defects found and fixed (missing service-account group under
  BusyBox `adduser -S`; an implicit-parent-directory permission defect
  under BusyBox `install -d`). Full evidence: `PHASE10_DISTRO_MATRIX.md`.
* **Security review (Phase 16)**: a fresh adversarial pass found and fixed
  one HIGH-severity defect predating this candidate — `crates/remote::
  webdav::WebDavProvider` disabled TLS certificate verification
  unconditionally for every WebDAV connection — plus two HIGH-severity
  dependency vulnerabilities (`quick-xml` DoS, an unused dependency pulling
  a vulnerable `russh-cryptovec`). Added deterministic filesystem
  TOCTOU/symlink-race regression coverage (0 escapes across all attempted
  races), fresh audit tamper-evidence tests (file-level byte tampering and
  historical-record deletion both detected), a real two-origin browser CSRF
  control, and reconciled dependency/license review. Full evidence:
  `PHASE16_SECURITY_REVIEW.md`.
* **Version consistency**: `apps/web/package.json`'s version was corrected
  to match the workspace's `1.0.1-rc.1`.

No product features were added in this later work beyond what v1.0.0
already shipped — it is exclusively defect-fixing, portability, and
security-hardening work on the existing v1.0.0 feature set.

---

## v1.0.1-rc.2 — publication-readiness fixes (candidate, not yet released)

`v1.0.1-rc.1` was tagged locally (never pushed or published) but its tagged
source commit did not contain a root `LICENSE` file, which established
project licensing policy (`Cargo.toml`'s `AGPL-3.0-or-later`) requires
before publication. `v1.0.1-rc.1` is not moved or reused — it remains a
frozen, local-only, superseded candidate. `v1.0.1-rc.2` is a new candidate
containing everything `v1.0.1-rc.1` had, plus:

* **Root `LICENSE`**: the canonical, unmodified GNU AGPLv3 text, added to
  implement the project's already-established licensing decision — not a
  new legal decision.
* **Third-party redistribution clarification**: explicit "Distribution
  model" classification added to the code-server, Collabora, and Brave
  notices in `docs/THIRD_PARTY_NOTICES.md`, and an explicit statement of
  what the SBOM does and does not cover.
* **Fail-closed release-staging validator**
  (`tests/distro/release-staging-validation.sh`): verifies a given source
  commit and release staging directory contain every file established
  release policy requires before it can be treated as publication-ready.
  Confirmed to correctly fail against `v1.0.1-rc.1`'s frozen source commit
  (missing `LICENSE`) and pass against the corrected source.
* **Release integrity documentation**: a precise statement of the
  artifact-integrity trust chain (what SHA256 does and does not
  authenticate), future publication endpoint placeholders, and local
  publication dry-run evidence. See `docs/RELEASE_INTEGRITY.md` and
  `PHASE17_RELEASE_PUBLICATION_CLOSURE.md`.

This is not a new product-feature release. No application code changed
between `v1.0.1-rc.1` and `v1.0.1-rc.2` — only licensing, documentation,
and release-tooling content.
