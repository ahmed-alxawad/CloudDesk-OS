# CloudDesk-OS v1.0 Implementation Plan

## Delivery Rule

Everything specified for CloudDesk-OS is required for v1.0.

Implementation must still be incremental. A feature being scheduled in a later engineering phase does not mean it is deferred beyond v1.0.

The project should keep `main` runnable throughout development and use feature flags for incomplete optional applications.

---

## Phase 0 - Repository, Standards, and Threat Model

### Work

- create monorepo structure;
- establish Rust workspace;
- establish Svelte/TypeScript frontend;
- add formatting/linting;
- add unit/integration test harness;
- define configuration format;
- define database migration framework;
- document threat model;
- define capability/permission names;
- define application manifest format;
- establish CI skeleton.

### Deliverables

- `cloudeskd` hello/health endpoint;
- static web shell;
- SQLite migration command;
- security invariants represented in tests where possible.

### Exit criteria

- project builds on Linux;
- frontend builds;
- Rust tests run in CI;
- architecture decisions match `ARCHITECTURE.md`.

---

## Phase 1 - Installer, Runtime Layout, and HTTPS

### Work

Implement `installer/install.sh`.

Distribution detection:

- Debian
- Ubuntu
- RHEL
- Fedora
- Rocky Linux
- AlmaLinux
- Arch Linux
- Alpine Linux

Add package-manager adapters and service-manager adapters.

Create:

- CloudDesk service user;
- filesystem layout;
- config;
- SQLite database;
- master encryption key;
- TLS certificate;
- systemd/OpenRC service definitions.

Bind CloudDesk to port `9870`.

### First-run flow

- generate one-time bootstrap secret;
- first administrator creation;
- choose Desktop mode or Dashboard mode;
- Desktop is default;
- configure optional runtime defaults.

### Exit criteria

Fresh installations succeed on every official v1.0 distribution.

A user can open:

`https://<server-ip>:9870`

and complete first-run setup.

---

## Phase 2 - Authentication, Sessions, RBAC, and Audit Foundation

### Work

Implement:

- users;
- Argon2id passwords;
- login/logout;
- TOTP;
- recovery codes;
- session management;
- remember-device flow;
- revoke sessions;
- rate limiting;
- login history;
- step-up authentication;
- roles;
- granular permissions;
- audit-event writer;
- tamper-evident audit hash chain.

Default roles:

- Administrator
- Manager
- User
- Guest

### Exit criteria

- unauthorized API calls fail server-side;
- 2FA flows are tested;
- session revocation works;
- privileged permissions require correct grants;
- audit events are written for authentication and authorization-sensitive events.

---

## Phase 3 - Privilege Separation and Linux Identity

### Work

Implement:

- `cloudesk-privd`;
- root-owned Unix socket;
- signed/scoped authorization grants;
- mapped Linux account lookup;
- `cloudesk-sessiond`;
- per-UID worker launch;
- local PTY identity tests;
- assigned-root policy for CloudDesk-only users.

Do not implement a generic privileged command runner.

### Exit criteria

- main API remains unprivileged;
- mapped file actions execute as the expected Linux user;
- mapped terminal executes with the expected UID/GID;
- privileged actions cannot be invoked without explicit permission and step-up grant;
- all helper actions are audited.

---

## Phase 4 - CloudDesk Shell

### Work

Build the custom minimal UI.

#### Desktop mode

- launcher;
- taskbar/dock;
- window create/close;
- drag;
- resize;
- minimize;
- maximize;
- z-index/focus;
- keyboard shortcuts;
- notifications;
- per-user layout persistence.

#### Dashboard mode

- lightweight app grid;
- recent items;
- system summary;
- transfer status;
- optional favorite apps.

### Exit criteria

- installation-selected mode works;
- Desktop mode is default;
- all built-in apps can register through the app manifest;
- UI remains responsive on low-end hardware.

---

## Phase 5 - VFS and Local File Manager

### Work

Build provider abstraction.

Implement local provider first.

File Manager features:

- list/grid;
- breadcrumbs;
- sidebar;
- upload/download;
- resumable large uploads;
- copy/move;
- rename;
- delete/trash;
- multi-select;
- search;
- sort/filter;
- properties;
- permissions;
- ownership;
- ACL;
- archive create/extract;
- favorites;
- recents;
- preview;
- Open With;
- drag/drop.

Security work:

- path normalization;
- symlink handling;
- traversal protection;
- assigned-root enforcement;
- TOCTOU tests.

### Exit criteria

Mapped users see their `$HOME` by default.

Administrators can explicitly enter authorized root-scope mode after step-up authentication.

CloudDesk-only users cannot escape assigned roots.

---

## Phase 6 - Gallery, Media, PDF, and File Associations

### Gallery

- native browser formats;
- libvips/image preview worker;
- HEIC/TIFF/RAW fallback where libraries support them;
- preview cache;
- metadata.

### Video

- direct HTTP range streaming;
- codec/container inspection;
- FFmpeg remux;
- FFmpeg transcode;
- subtitles;
- seek;
- resume state.

### Music

- audio playback;
- metadata;
- library index;
- albums/artists;
- playlists;
- queue;
- favorites;
- recent playback;
- FFmpeg compatibility path.

### PDF

- PDF viewer;
- search;
- thumbnails;
- zoom;
- fit modes;
- print/download.

### Associations

Implement MIME-aware default apps and Open With.

### Exit criteria

Double-clicking supported files opens the correct CloudDesk application.

Unsupported browser codecs use the server-side compatibility path rather than failing without explanation.

---

## Phase 7 - Local Terminal and System Settings

### Terminal

- PTY WebSocket;
- resize;
- reconnect policy;
- shell selection;
- mapped Linux user identity;
- audit start/stop.

### Settings

Implement CloudDesk settings plus host administration adapters.

Initial administration modules:

- system summary;
- services;
- users/groups;
- SSH;
- packages;
- network information;
- firewall;
- mounts/storage;
- hostname;
- time/date;
- updates;
- logs;
- reboot/shutdown;
- Docker/Podman visibility/integration.

High-risk actions use `privd` and step-up authentication.

### Exit criteria

No system-setting workflow requires `cloudeskd` to run as root.

---

## Phase 8 - Secrets Vault and Remote Server Manager

### Vault

Implement:

- envelope encryption;
- installation master key;
- encrypted secret records;
- redaction;
- permission checks;
- audit;
- secure deletion/rotation semantics.

### Remote servers

Implement:

- saved servers;
- tags/groups;
- test connection;
- host fingerprints;
- known_hosts;
- password auth;
- PEM;
- RSA;
- Ed25519;
- encrypted keys/passphrases;
- SSH agent;
- keyboard-interactive;
- custom ports;
- ProxyJump;
- SSH certificates.

### Remote terminal

Provide SSH terminal sessions inside CloudDesk.

### Exit criteria

- private keys never appear plaintext in SQLite;
- SSH host changes fail safely;
- remote sessions are audited;
- credentials cannot be retrieved by unauthorized roles.

---

## Phase 9 - Remote VFS Providers

### SFTP

Add SFTP browsing and file operations.

### WebDAV

Add WebDAV browsing and file operations.

### S3

Add provider supporting:

- AWS S3;
- R2;
- MinIO;
- Backblaze B2 S3;
- Wasabi;
- DigitalOcean Spaces;
- Ceph;
- custom endpoints.

### Exit criteria

Files can display local and remote providers in one unified interface.

Provider-specific unsupported operations are clearly represented rather than silently emulated incorrectly.

---

## Phase 10 - Persistent Transfer Engine

### Work

Implement:

- transfer jobs;
- persistent queue;
- concurrency limits;
- direct strategy detection;
- server-side relay fallback;
- retry/backoff;
- pause/resume where supported;
- cancel;
- checksum verification;
- progress;
- speed;
- ETA;
- history;
- restart recovery;
- WebSocket progress events.

Test combinations including:

```text
local -> SFTP
SFTP -> local
SFTP -> SFTP
local -> S3
S3 -> local
S3 -> S3
WebDAV -> SFTP
SFTP -> WebDAV
```

### Exit criteria

Closing the browser does not stop transfers.

Restarting CloudDesk does not lose transfer state.

Remote-to-remote transfers never flow through the user's browser.

---

## Phase 11 - VS Code-Compatible Workspace

### Work

Integrate a legally suitable VS Code-compatible server runtime.

Implement:

- per-user sessions;
- workspace roots;
- SSO handoff;
- extensions policy;
- integrated terminal;
- Git;
- GitHub/GitLab;
- language servers;
- debugging;
- multiple workspaces;
- optional dev containers;
- runtime CPU/memory limits;
- idle shutdown.

Settings:

```text
Code Runtime: Enabled / Disabled
```

### Exit criteria

Disabling Code stops the runtime and removes it from user launchers according to policy.

Each user's workspace/process state is isolated.

---

## Phase 12 - Office Editing

### Work

Integrate LibreOfficeKit/Collabora-compatible editing.

Support:

- DOC/DOCX;
- XLS/XLSX;
- PPT/PPTX;
- ODT/ODS/ODP.

Implement:

- file authorization bridge;
- edit locks;
- autosave;
- safe save;
- remote-provider save;
- conflict detection;
- audit.

Settings:

```text
Office Runtime: Enabled / Disabled
```

### Exit criteria

Users can open an authorized office file from Files, edit it in the Office application, and save it back without bypassing CloudDesk permissions.

---

## Phase 13 - Brave Browser Runtime

### Work

Build Browser Broker.

Implement:

- isolated Brave launch;
- persistent per-user profiles;
- ephemeral Guest profile;
- virtual/headless display;
- remote application rendering;
- WebRTC or equivalent low-latency stream;
- keyboard/mouse input;
- audio;
- tabs;
- clipboard policy;
- downloads;
- upload bridge;
- bookmarks/session persistence;
- cgroup limits;
- seccomp/AppArmor/SELinux integration where applicable.

Settings:

```text
Browser Runtime: Enabled / Disabled
```

### Distribution compatibility

Core remains native everywhere.

Use a compatible isolated OCI runtime for the Browser feature on distributions where a safe native Brave package is not available, including Alpine if required.

### Exit criteria

- arbitrary modern sites work substantially like native Brave;
- no host desktop is exposed;
- profiles are isolated;
- Guest profile disappears after the session;
- disabling Browser stops its runtime.

---

## Phase 14 - Performance and Resource Hardening

### Work

Profile:

- idle RSS;
- cold start;
- dashboard bundle size;
- VFS directory listing;
- large uploads;
- transfer memory;
- media streaming;
- optional runtime startup/shutdown.

Optimize:

- allocations;
- SQLite queries;
- caches;
- WebSocket fanout;
- filesystem metadata calls;
- frontend bundles;
- polling.

### Acceptance target

A minimal CloudDesk installation with Browser/Code/Office disabled must remain usable on a 1 CPU, 512 MB RAM Linux VPS under a realistic light workload.

Heavy runtime requirements must be documented separately.

---

## Phase 15 - Multi-Distribution Release Hardening

### Required OS test matrix

- Debian
- Ubuntu
- RHEL
- Fedora
- Rocky Linux
- AlmaLinux
- Arch Linux
- Alpine Linux

For each:

- fresh install;
- upgrade;
- uninstall;
- service start/stop;
- TLS first access;
- database migration;
- Files;
- Terminal;
- privileged helper;
- SSH;
- transfers;
- optional runtime enable/disable.

Add:

- SELinux testing on enforcing systems;
- AppArmor testing where relevant;
- OpenRC testing on Alpine;
- musl build/runtime tests;
- firewall adapter tests.

---

## Phase 16 - Security Review

### Required testing

- path traversal;
- symlink escape;
- race/TOCTOU file access;
- CSRF;
- XSS;
- SSRF;
- WebSocket authorization;
- session fixation;
- session replay;
- 2FA bypass;
- privilege escalation;
- command injection;
- malicious archive extraction;
- unsafe media/document preview;
- secret exposure in logs;
- SSH host-key downgrade;
- transfer destination spoofing;
- Browser runtime sandbox escape assumptions;
- Code/Office runtime filesystem escape.

Perform dependency and license review.

### Exit criteria

No open critical or high-severity security issue is accepted for v1.0 release.

---

## Phase 17 - Packaging, Documentation, and v1.0 Release

### Work

- finalize install script;
- release binaries;
- checksums/signatures;
- upgrade path;
- backup/restore documentation;
- reverse proxy examples;
- TLS documentation;
- firewall documentation;
- admin guide;
- user guide;
- security guide;
- contributor guide;
- commercial licensing path;
- dependency notices.

### v1.0 release gate

All required features work, all official distributions pass the release matrix, and optional heavy runtimes can be enabled/disabled from Settings.

---

## Engineering Rules

1. Do not make `cloudeskd` root.
2. Do not add a generic privileged shell endpoint.
3. Do not bypass VFS authorization with direct frontend paths.
4. Do not store secrets plaintext.
5. Do not send remote-to-remote file data through the browser.
6. Do not make Redis, PostgreSQL, Docker, or another large service mandatory for the core.
7. Do not keep Browser, Code, or Office running when disabled.
8. Do not assume systemd; Alpine/OpenRC is a first-class target.
9. Do not silently ignore Linux permissions or ACLs.
10. Do not accept a feature as complete without authorization and audit tests.

---

## Suggested Milestone Order for Coding Agents

For a fresh repository, implement in this order:

```text
0. Repository/bootstrap
1. Installer + HTTPS + SQLite
2. Auth/RBAC/audit
3. Privilege separation/Linux identity
4. Shell
5. Local Files/VFS
6. Media/PDF
7. Terminal/Settings
8. Vault/Remote SSH
9. Remote providers
10. Transfers
11. Code
12. Office
13. Browser
14. Performance
15. Distro hardening
16. Security
17. Release
```

Do not begin with the Browser runtime. The security, identity, VFS, and application lifecycle foundations must exist first.
