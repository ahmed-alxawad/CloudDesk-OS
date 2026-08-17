# CloudDesk-OS Goals

## v1.0 Objective

Deliver a secure, resource-efficient, multi-user web desktop for Linux server management and productivity.

All major applications described in this document are required for v1.0. Heavy applications may be disabled by administrators, but the capability to install, enable, disable, and use them is part of v1.0.

## Primary Goals

### G1. Install and access CloudDesk easily

A new installation must be possible from a single Bash installer retrieved from the official GitHub repository.

Expected flow:

```bash
curl -fsSL <official-install-url> | sudo bash
```

The installer must:

- detect the Linux distribution;
- install or fetch required core dependencies;
- install CloudDesk services;
- create configuration and data directories;
- generate initial TLS material;
- initialize SQLite;
- create or bootstrap the first administrator;
- configure the service manager;
- start CloudDesk on TCP port `9870`;
- print the initial access URL and bootstrap instructions.

Initial access should work by IP address at:

`https://<server-ip>:9870`

A self-signed certificate is acceptable for first access. Production deployments should support a user-supplied certificate, ACME, or a reverse proxy.

### G2. Support both CloudDesk UI modes

CloudDesk must provide:

1. **Desktop mode** - default
   - movable windows;
   - resize;
   - minimize/maximize;
   - taskbar/dock;
   - multi-app use;
   - application state restoration where appropriate.

2. **Dashboard mode**
   - application launcher/dashboard;
   - simpler navigation;
   - lower UI complexity.

The installation wizard must allow the administrator to choose a default. Users may later be allowed to override it if policy permits.

### G3. Provide a complete lightweight file manager

The Files application must provide:

- list and grid views;
- breadcrumb navigation;
- tree/sidebar navigation;
- drag and drop;
- copy/move;
- rename;
- delete and Trash;
- upload/download;
- large-file and resumable upload support;
- multi-select;
- search;
- sorting and filtering;
- file properties;
- Linux ownership display;
- Linux permission editing when authorized;
- ACL viewing/editing when authorized;
- archive creation/extraction;
- favorites;
- recent files;
- shared/configured roots;
- context menus;
- preview pane;
- "Open With..." support.

Default local scope:

- normal mapped user: Linux `$HOME`;
- administrator: entire filesystem when explicitly authorized;
- CloudDesk-only user: administrator-assigned roots only.

### G4. Open files in native CloudDesk applications

Double-clicking or opening a file must route through a MIME-aware application association system.

Expected defaults include:

- images -> Gallery
- video -> Video
- audio -> Music
- PDF -> Documents
- office files -> Office
- source/text -> Code or text viewer
- archives -> Files/archive workflow

Users must be able to choose **Open With...** when more than one application is compatible.

### G5. Broad media and document compatibility

CloudDesk must use server-side compatibility layers when a browser cannot decode a format directly.

#### Gallery

Support common browser-native images and server-side preview conversion for formats such as:

- JPEG
- PNG
- GIF
- WebP
- AVIF
- SVG
- TIFF
- HEIF/HEIC
- BMP
- supported RAW camera formats

The original file must not be modified for preview.

#### Video

Provide a VLC-like playback experience using direct browser playback when possible and FFmpeg-based remux/transcode streaming when necessary.

#### Music

Provide:

- playback;
- persistent queue;
- playlists;
- artists/albums;
- folder browsing;
- metadata;
- album art;
- favorites;
- recent playback;
- search.

#### PDF

Provide an Okular-like PDF viewing application with:

- page thumbnails;
- search;
- zoom;
- fit modes;
- page navigation;
- download;
- print where browser policy permits.

#### Office

Provide LibreOffice-like browser editing for:

- DOC/DOCX
- XLS/XLSX
- PPT/PPTX
- ODT/ODS/ODP

The implementation may use LibreOfficeKit/Collabora-compatible technology behind a CloudDesk application shell.

### G6. Provide coding and terminal environments

#### Code

Provide a VS Code-compatible browser workspace with:

- extensions where supported;
- integrated terminal;
- Git;
- GitHub/GitLab workflows;
- multiple workspaces;
- language servers;
- debugging where supported;
- optional dev-container support.

Each CloudDesk user must have an isolated Code session.

#### Terminal

Provide a local PTY terminal running as the correct Linux user.

Support common user shells such as:

- bash
- zsh
- fish

Root shells must never be granted merely because the web application is privileged. Privilege elevation requires explicit authorization and step-up authentication.

### G7. Provide a true Browser application

The CloudDesk Browser application must launch an isolated server-side Brave runtime rather than embedding arbitrary websites in iframes.

Recommended profile behavior:

- Administrator/Manager/User: persistent isolated profile per CloudDesk user
- Guest: temporary/ephemeral profile

The Browser app should support:

- multiple tabs;
- cookies and sessions;
- bookmarks;
- downloads;
- keyboard/mouse input;
- clipboard subject to permission policy;
- modern JavaScript sites;
- persistent profile storage for non-guest users.

It must not expose the Linux desktop.

### G8. Manage remote Linux servers

The Remote Servers application must support saved connections with:

- name;
- host/IP;
- port;
- username;
- tags/groups;
- notes;
- authentication method;
- host fingerprint;
- actions for Terminal, Files, Transfer, Edit, Test Connection.

SSH authentication support must include:

- password;
- PEM;
- RSA keys;
- Ed25519 keys;
- encrypted private keys/passphrases;
- SSH agent;
- keyboard-interactive;
- custom ports;
- ProxyJump/bastion hosts;
- known_hosts verification;
- SSH certificates.

### G9. Unified remote files and storage

The Files application must support providers for:

- local filesystem;
- SSH/SFTP;
- SCP where appropriate for transfers;
- WebDAV;
- AWS S3;
- Cloudflare R2;
- MinIO;
- Backblaze B2 S3;
- Wasabi;
- DigitalOcean Spaces;
- Ceph S3;
- custom S3-compatible endpoints.

### G10. Server-to-server transfers

Transfers must support:

- direct server-to-server movement when technically possible;
- server-side relay streaming when direct transfer is not possible;
- no browser data path for remote-to-remote transfers;
- queueing;
- progress;
- retry;
- pause/resume where the protocol allows it;
- cancel;
- transfer history;
- speed;
- ETA;
- transferred bytes;
- verification/checksum when possible;
- background continuation after browser closure;
- persistence across CloudDesk restart.

### G11. Multi-user authentication and authorization

v1.0 must support multiple CloudDesk users on one host.

Authentication:

- username/password;
- Argon2id password hashing;
- TOTP 2FA;
- recovery codes;
- remember-device controls;
- active session management;
- revoke other sessions;
- device/IP history;
- brute-force/rate-limit protections;
- secure password reset/administrator reset flow.

Authorization must combine four default roles with granular permissions:

- Administrator
- Manager
- User
- Guest

Role permissions must be customizable by an Administrator.

### G12. Respect Linux identity and permissions

CloudDesk identity mode is hybrid:

- CloudDesk account mapped to a real Linux user; or
- CloudDesk-only identity with explicit filesystem roots and capabilities.

For mapped users, local filesystem and terminal operations must run under the mapped UID/GID whenever possible.

CloudDesk must respect:

- Linux mode bits;
- ownership;
- groups;
- ACLs.

### G13. Secure secrets

SSH private keys, passwords, S3 secrets, WebDAV credentials, tokens, and similar values must:

- never be stored plaintext;
- be encrypted at rest;
- be decrypted only when needed;
- never be written to logs;
- be redacted from API responses;
- be accessible only after authorization checks;
- support re-authentication for high-risk access.

### G14. Complete audit trail

Audit events should include:

- user;
- role;
- action;
- target/resource;
- file/path when relevant;
- remote server when relevant;
- timestamp;
- source IP;
- user agent;
- session ID;
- result;
- error category;
- old/new values for sensitive settings changes where safe;
- privilege escalation;
- login/logout;
- 2FA changes;
- key/credential changes;
- SSH connections;
- transfer lifecycle;
- user/role changes.

Logs must be tamper-evident and unavailable for modification by ordinary users.

### G15. Server administration

Settings must provide authorized management for:

- CPU/RAM/disk status;
- network information/configuration;
- firewall integration;
- users/groups;
- SSH service;
- services;
- packages;
- storage/mounts;
- hostname;
- date/time;
- updates;
- logs;
- reboot;
- shutdown;
- Docker/Podman integration when available.

High-risk operations require step-up authentication and the privileged helper.

### G16. Optional heavy applications

Settings must expose explicit enable/disable controls for at least:

- Brave Browser Runtime
- VS Code-compatible Workspace Runtime
- LibreOffice/Office Runtime

Media transcoding should also be administratively controllable.

Disabled optional runtimes must not remain resident.

## Default Role Model

| Capability | Administrator | Manager | User | Guest |
|---|---:|---:|---:|---:|
| Own home/files | Full | Full | Full | Shared only |
| Assigned remote storage | Full | Full | Allowed | Read-only if shared |
| Remote servers | Full | Allowed by permission | Allowed by permission | No |
| Transfers | Full | Full | Own/allowed | No |
| Terminal | Full | Allowed | Allowed | No |
| Code | Full | Allowed | Allowed | No |
| Browser | Full | Allowed | Allowed | Ephemeral only if enabled |
| System settings | Full | Limited | No | No |
| User/role management | Full | No by default | No | No |
| Audit logs | Full | Read subset | Own activity | No |
| Root-scope filesystem | Step-up | No by default | No | No |

Administrators must be able to customize these defaults.

## Performance Goals

These are engineering targets, not promises independent of workload:

- CloudDesk core should boot and remain usable on a 512 MB RAM Linux VPS with heavy runtimes disabled.
- Core services should target an idle resident memory footprint below roughly 200 MB total, excluding the OS page cache and optional runtimes.
- Idle CPU use should be near zero.
- The web shell should avoid large client bundles and unnecessary background polling.
- Long-running work should use streaming and bounded memory rather than loading whole files into RAM.
- Large uploads and transfers must be chunked/streamed.
- Heavy runtimes should be started on demand where practical.

## Network Goals

Default application port:

`9870/tcp`

Supported deployment patterns:

1. Built-in HTTPS directly on port 9870.
2. Reverse proxy through Caddy/Nginx/Traefik.
3. User-provided TLS certificate.
4. ACME-managed certificate where configured.

Plain HTTP remote login must not be the production default.

## Official v1.0 OS Matrix

All of the following are release-blocking supported platforms:

- Debian
- Ubuntu
- RHEL
- Fedora
- Rocky Linux
- AlmaLinux
- Arch Linux
- Alpine Linux

The installer and CI must test distribution-specific package management and service-manager behavior.

## Non-Goals

CloudDesk-OS v1.0 will not:

- install a full Linux desktop environment;
- expose the host through VNC/RDP;
- route server-to-server transfers through the user's browser;
- run the primary API as root;
- guarantee that every codec or proprietary format in existence can be decoded;
- require external CloudDesk cloud services to function;
- keep Brave, Code, or Office resident when disabled.
