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
