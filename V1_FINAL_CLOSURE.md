# V1 Final Closure Checklist

This document tracks the final implementation and validation steps required to push CloudDesk-OS from `97.13%` to a true, fully-verified `100.00%` completion.

## 1. Core Platform & Installer Runtime

### 1.1 Multi-Distro Smoke Testing (Runtime Evidence)
- [ ] **Debian / Ubuntu**: Full install, service start, HTTPS response, login.
- [ ] **RHEL / Rocky / AlmaLinux**: Install, SELinux enforcement validation, service start.
- [ ] **Fedora**: Install, service start, database migration.
- [ ] **Arch Linux**: Install, service start.
- [ ] **Alpine Linux**: Install, OpenRC integration validation, service start.
- **Current state**: `PACKAGE/LAYOUT TESTED`
- **Missing validation**: Actual container/VM runtime execution of the installer and service validation.

### 1.2 Fresh Install & Upgrade Testing
- [ ] **Fresh Install End-to-End Test**: Clean checkout `v1.0.0-rc.1` -> install -> :9870 -> HTTPS -> bootstrap -> 2FA -> use apps -> restart host -> verify persistence.
- [ ] **Upgrade Test**: Setup older schema SQLite DB -> run `v1.0.0-rc.1` migration -> verify users and Vault envelope encryption migration.
- [ ] **Backup / Restore Test**: Snapshot DB/keys -> destroy instance -> restore -> verify all secrets and sessions survive.

### 1.3 Application & System Performance Under Load
- [ ] Re-measure `~42 MB` idle RSS and `0.35 ms` health API latency on a designated test environment.
- [ ] Measure system footprint with **Brave active**.
- [ ] Measure system footprint with **Code active**.
- [ ] Measure system footprint with **Office active**.
- [ ] Measure system footprint with **FFmpeg transcoding active**.

## 2. Heavy Runtimes & Container Orchestration

### 2.1 Brave Browser Runtime
- **Requirement**: Real server-side Brave, rendered into CloudDesk, per-user isolation, memory limits.
- **Current state**: `IMPLEMENTED_NOT_VERIFIED` (manifest & lifecycle stubs exist).
- **Missing implementation**: Real OCI container/process orchestration bridging Brave's CDP/rendering pipeline to the browser shell via WebSockets or WebRTC, sandboxing, guest profiles, persistence.

### 2.2 VS Code-Compatible Runtime (code-server)
- **Requirement**: Real code-server backend, per-user workspace, terminal integration, extensions.
- **Current state**: `IMPLEMENTED_NOT_VERIFIED` (manifest & lifecycle stubs exist).
- **Missing implementation**: Real code-server binary download/execution, reverse-proxying into `clouddeskd`, workspace VFS mounting, idle shutdown.

### 2.3 LibreOffice / Office Runtime (Collabora)
- **Requirement**: Real Collabora Online backend for editing DOCX, XLSX, PPTX, ODT.
- **Current state**: `IMPLEMENTED_NOT_VERIFIED` (manifest & lifecycle stubs exist).
- **Missing implementation**: Real Collabora container integration, WOPI protocol implementation for VFS access, safe atomic saves.

## 3. Storage, Files, & Media Applications

### 3.1 Files Completeness
- **Missing features to implement**: Archive create/extract (zip/tar), Trash bin management, drag/drop frontend integration, properties view, resumable large upload (tus/chunked), multi-select batch operations, favorites/recents.

### 3.2 Gallery Completeness
- **Missing features to implement**: TIFF, HEIC, RAW fallback, SVG active-content sanitization, malformed image failure validation.

### 3.3 Video Compatibility Layer
- **Missing features to implement**: FFmpeg probing, unsupported container remuxing, unsupported codec transcoding, seeking over transcoded streams, subtitles extraction, temporary file cleanup.

### 3.4 Music Application
- **Missing features to implement**: Player queue, artists, albums, playlists, favorites, metadata parsing (ID3), format compatibility conversion.

### 3.5 PDF Application
- **Missing features to implement**: Actual multi-page viewer (e.g., pdf.js integration), search, zoom, thumbnails.

## 4. Remote Infrastructure & Transfer Engine

### 4.1 SSH Authentication Matrix
- **Current state**: Password and basic Ed25519 implemented.
- **Missing implementation/validation**: PEM, RSA, Encrypted private key + passphrase, SSH agent forwarding, Keyboard-interactive, Custom port, ProxyJump/bastion, SSH certificates.

### 4.2 SFTP / WebDAV / S3 Endpoints
- **Current state**: `IMPLEMENTED_NOT_VERIFIED`.
- **Missing implementation/validation**:
  - **SFTP**: Upload, download, streaming, connection loss, large file.
  - **WebDAV**: Upload, download, failure recovery.
  - **S3**: Multipart upload, configurable custom endpoints (MinIO, R2, Wasabi), copy, list.

### 4.3 Transfer Engine Matrix
- **Missing validation**: End-to-end execution of `SFTP -> S3`, `Local -> S3`, `WebDAV -> SFTP`, etc., strictly via the server-side backend (no browser proxying), with SHA-256 verification, resume, network interruption tests.

## 5. Security Validation
- [ ] Try to break the RC: path traversal, cross-user Vault access, arbitrary root commands, CSRF, SSH host-key replacement, Vault wrapped-DEK tampering.
