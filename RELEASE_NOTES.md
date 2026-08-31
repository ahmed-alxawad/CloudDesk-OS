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

## v1.0.1-rc.4 — release candidate (current public prerelease)

**This is a release candidate, not a stable release.** `v1.0.0` remains
the latest stable tagged release. `v1.0.1-rc.4` is published as a
[GitHub prerelease](https://github.com/ahmed-alxawad/CloudDesk-OS/releases/tag/v1.0.1-rc.4)
and is the recommended way to try upcoming fixes ahead of the next stable
patch release.

### Security fixes since v1.0.0

* An unauthenticated-adjacent authorization gap: a system-status endpoint
  was reachable by any logged-in user, including Guest, without the
  capability check its sibling administration endpoints already had.
* **Critical**: the SSH client previously accepted *any* host key
  unconditionally, meaning a machine-in-the-middle or a replaced remote
  host would be silently trusted on every SSH/SFTP transfer or terminal
  connection. Connections now reject a host key that doesn't match the
  one pinned when the remote server was first saved.
* SFTP uploads could never create a new file on the remote server (only
  overwrite an existing one) — fixed.
* SFTP directory listing failed against most real-world (non-chrooted)
  SFTP servers — fixed.
* WebDAV connections previously skipped TLS certificate verification
  entirely — fixed.
* Two vulnerable/unnecessary dependencies were removed or updated
  (an XML-parsing denial-of-service issue, and an unused dependency
  pulling in a vulnerable crate).

### Platform and installation

* **Native musl build for Alpine Linux**, alongside the existing glibc
  build covering Debian, Ubuntu, Fedora, the RHEL family (RHEL, Rocky,
  AlmaLinux), and Arch.
* **Public one-command installer**, now fully working end-to-end:
  ```sh
  curl -fsSL https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/v1.0.1-rc.4/install.sh \
      | sudo env CLOUDESK_VERSION=1.0.1-rc.4 bash
  ```
  The installer verifies the requested version, downloads the correct
  platform artifact, and checks its SHA256 checksum before installing
  anything — it fails closed on any mismatch rather than installing an
  unverified binary.
* **Reproducible, attested release artifacts**: every published binary,
  the installer itself, and the web frontend bundle are built from an
  exact, immutable tagged source commit and cryptographically signed via
  [GitHub Artifact Attestations](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds).
  Verify any downloaded file yourself:
  ```sh
  gh attestation verify <downloaded-file> --repo ahmed-alxawad/CloudDesk-OS
  ```
* Root `LICENSE` file added (`AGPL-3.0-or-later`, the project's
  established Community license).

### Known limitations

* This project's automated test suite has one known gap unrelated to the
  release itself: browser end-to-end tests do not yet run in ordinary CI
  because the CI runner doesn't have browser binaries installed. This
  does not affect the release build or published artifacts.
* SELinux enforcing mode, true reboot persistence, and a fully subscribed
  RHEL 9 environment have not been exercised in this project's own test
  environment.

No application features were added since `v1.0.0` — this release is
exclusively security fixes, platform/installer work, and release
infrastructure.
