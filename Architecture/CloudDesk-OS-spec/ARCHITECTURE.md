# CloudDesk-OS Architecture

## 1. Architecture Summary

CloudDesk-OS is a multi-user web desktop composed of a small native core plus isolated optional runtimes.

The architecture optimizes for:

- low idle resource usage;
- security;
- Linux permission fidelity;
- multi-distribution portability;
- resumable background work;
- modular applications;
- clean separation between unprivileged and privileged operations.

High-level design:

```text
Browser
  |
  | HTTPS / WebSocket
  v
+-------------------------------+
| CloudDesk Web UI              |
| Svelte + TypeScript           |
|                               |
| Desktop / Dashboard           |
| Files / Gallery / Media       |
| Documents / Transfers         |
| Settings / Remote Servers     |
+-------------------------------+
               |
               v
+----------------------------------------------+
| cloudeskd - Rust Core                         |
| Axum/Tokio                                    |
|                                               |
| Auth / Sessions / RBAC / API / WebSocket     |
| VFS / Files / SSH / Transfers / Audit        |
| Settings / App Registry / Job Scheduler      |
| SQLite / Vault Client / Runtime Orchestration|
+----------------------------------------------+
    |             |             |            |
    |             |             |            |
    v             v             v            v
sessiond      privd         optional       remote
per-user      root helper   runtimes       systems
workers                    / workers
    |             |       /     |      \
    |             |      /      |       \
 local FS      system   Brave   Code   Office/FFmpeg
 PTY           changes  runtime runtime runtime
 Linux UID
```

The user's browser is a control and presentation client. Background transfers and long-running jobs execute on the server.

---

## 2. Technology Choices

### Frontend

Recommended:

- Svelte
- TypeScript
- Vite
- native CSS/CSS variables
- minimal component dependencies

Why:

- small runtime and bundle potential;
- good fit for desktop-like reactive UI;
- avoids the overhead of a large server-rendered frontend framework;
- easy to build custom window management without bringing a full UI framework.

The frontend is served as static assets by `cloudeskd` or a reverse proxy.

### Core backend

Recommended:

- Rust
- Tokio
- Axum
- Tower middleware
- Serde
- SQLx with SQLite
- tracing for structured logs

Rust is the primary resident backend language because CloudDesk has strict memory, concurrency, filesystem, SSH, and security requirements.

### Optional ecosystem services

Node.js may be used only where an integration strongly benefits from the Node ecosystem, such as a Code or browser-control bridge.

Python must not be part of the always-on critical path unless there is a compelling dependency. If used for a converter or tool wrapper, it should run on demand.

### Database

v1.0:

- SQLite
- WAL mode
- foreign keys enabled
- bounded connection pool

Database access must be abstracted sufficiently to allow a future PostgreSQL backend without redesigning business logic.

---

## 3. Process Model

### `cloudeskd`

Unprivileged main service.

Responsibilities:

- HTTPS/API;
- WebSocket sessions;
- authentication;
- authorization;
- user and role management;
- VFS routing;
- application routing;
- remote connection metadata;
- transfer scheduling;
- audit event emission;
- settings;
- optional-runtime orchestration;
- SQLite access;
- encrypted-vault access.

It must run as a dedicated service user such as:

`clouddesk`

It must not run as root.

### `cloudesk-privd`

Small root-owned privileged helper.

Responsibilities are intentionally narrow:

- spawn a mapped-user worker under a specific UID/GID;
- perform approved service-manager actions;
- reboot/shutdown;
- controlled package-manager operations;
- controlled firewall operations;
- mount/unmount approved storage;
- manage approved system configuration;
- perform other explicitly enumerated privileged operations.

Rules:

- listens only on a root-owned Unix socket;
- accepts requests only from `cloudeskd`;
- validates every argument;
- no arbitrary shell strings;
- no "execute command as root" generic API;
- sensitive actions require a signed/short-lived authorization grant issued after step-up authentication;
- every request is audited.

### `cloudesk-sessiond`

Short-lived or per-session worker launched as a mapped Linux user.

Used for:

- local filesystem operations;
- PTY terminal;
- user-level Git;
- user-owned processes;
- permission-sensitive work.

For mapped accounts, it runs with the target UID/GID and HOME.

For CloudDesk-only users, local access is restricted to administrator-assigned roots and handled through tightly scoped service identity rules rather than impersonating a Linux account.

### Transfer worker

A Rust worker subsystem handles persistent jobs.

It may begin in-process behind a clean trait/interface and later be split into a separate `cloudesk-transferd` process if isolation or scaling requires it.

### Optional runtime services

- Browser Runtime
- Code Runtime
- Office Runtime
- Media Transcode Runtime

Each has:

- enable/disable setting;
- health state;
- resource limits;
- per-user isolation where required;
- no mandatory idle process when disabled.

---

## 4. Filesystem Layout

Recommended installation paths:

```text
/opt/clouddesk/
  bin/
  web/
  runtimes/

/etc/clouddesk/
  clouddesk.toml
  tls/
  keys/
  policy.d/

/var/lib/clouddesk/
  clouddesk.db
  vault/
  users/
  browser-profiles/
  code/
  office/
  cache/
  transfers/

/var/log/clouddesk/
  clouddesk.log
  audit-export.log
```

Permissions must be strict and installer-controlled.

Example:

- config: root-owned, readable by CloudDesk service only where needed;
- master key: root-owned `0600`;
- service data: CloudDesk service ownership;
- per-user runtime data: isolated permissions.

---

## 5. Repository Layout

Recommended monorepo:

```text
clouddesk-os/
  apps/
    web/

  crates/
    api/
    auth/
    audit/
    config/
    db/
    files/
    jobs/
    linux/
    media/
    permissions/
    remote/
    runtime/
    secrets/
    ssh/
    transfers/
    vfs/

  services/
    clouddeskd/
    privd/
    sessiond/
    browser-broker/

  integrations/
    code/
    office/
    media/

  installer/
    install.sh
    lib/
      distro.sh
      debian.sh
      rhel.sh
      fedora.sh
      arch.sh
      alpine.sh

  packaging/
    systemd/
    openrc/
    selinux/
    apparmor/

  tests/
    integration/
    security/
    distro/

  docs/
```

---

## 6. Identity Model

CloudDesk has its own application identity.

Each CloudDesk user has:

- CloudDesk user ID;
- username;
- password credentials;
- 2FA state;
- role membership;
- granular permissions;
- optional Linux UID/GID mapping;
- assigned filesystem roots;
- allowed applications;
- remote servers/storage permissions.

### Mapped Linux user

Example:

```text
CloudDesk user: ahmed
Linux mapping:  ahmed (uid 1001)
```

Operations that should obey native Linux access run under UID 1001.

This preserves:

- ownership;
- mode bits;
- groups;
- POSIX ACL behavior.

### CloudDesk-only user

A CloudDesk-only account has no implicit access to the host filesystem.

An Administrator must assign explicit roots and capabilities.

Example:

```text
/var/www/project-a   read/write
/srv/public          read-only
```

### Administrator filesystem behavior

An Administrator may request root-scope filesystem access, but this is not automatically active merely because they hold the Administrator role.

Root-scope access requires:

1. permission check;
2. step-up authentication;
3. explicit privileged action/session;
4. audit event.

---

## 7. Authentication

### Passwords

Store only Argon2id password hashes with unique salts.

Never store plaintext passwords.

### 2FA

v1.0 supports:

- TOTP;
- encrypted TOTP secret storage;
- single-use recovery codes stored as hashes;
- 2FA reset workflow with audit logging.

### Sessions

Use opaque, random session identifiers stored in secure cookies.

Cookie requirements:

- Secure;
- HttpOnly;
- SameSite=Lax or Strict where possible;
- short idle timeout;
- absolute maximum lifetime;
- server-side revocation.

Persist session metadata:

- user;
- creation time;
- last activity;
- IP history;
- user agent/device label;
- 2FA state;
- step-up expiration.

### Brute-force protection

Implement:

- per-account rate limits;
- per-IP rate limits;
- exponential delay/temporary lockout;
- security audit events;
- no username-enumeration responses.

### Step-up authentication

Required for actions such as:

- root-scope filesystem access;
- changing firewall;
- reboot/shutdown;
- package management;
- changing user roles;
- revealing/exporting credentials;
- disabling 2FA;
- high-risk SSH key operations.

Step-up grants are short-lived and scoped.

---

## 8. Authorization

Use RBAC plus granular permissions.

Default roles:

- Administrator
- Manager
- User
- Guest

Do not hard-code behavior solely by role name.

Permissions should be capability strings, for example:

```text
files.local.read
files.local.write
files.root.request
files.permissions.change
remote.servers.read
remote.servers.manage
remote.terminal.open
transfers.create
transfers.cancel
terminal.local.open
apps.browser.use
apps.code.use
apps.office.use
system.services.manage
system.packages.manage
system.firewall.manage
system.power.manage
users.manage
roles.manage
audit.read
secrets.manage
```

Administrators may customize role grants.

Authorization is enforced in the backend even if the frontend hides controls.

---

## 9. Virtual Filesystem (VFS)

The Files application uses a provider interface rather than directly binding the UI to local POSIX paths.

Provider types:

```text
local://
sftp://
webdav://
s3://
```

SCP is primarily a transfer mechanism rather than a browsable filesystem.

Core VFS operations:

```text
stat
list
read
write
mkdir
rename
copy
move
delete
trash
search
open_stream
upload_stream
download_stream
permissions
acl
metadata
checksum
```

Providers advertise capabilities.

Example:

```text
supports_acl = true/false
supports_resume = true/false
supports_server_side_copy = true/false
supports_atomic_rename = true/false
```

The UI must adapt to provider capabilities.

### File identifiers

Never trust a browser-supplied raw path as authorization.

The backend must:

- normalize paths;
- prevent traversal;
- validate assigned roots;
- bind operations to a provider and authenticated user;
- re-check authorization for every request.

### MIME handling

Use content-aware MIME detection where safe, with extension as a secondary hint.

Application associations are stored centrally.

---

## 10. Files Application

Features:

- grid/list views;
- sidebar tree;
- breadcrumbs;
- favorites;
- recent files;
- remote mounts/providers;
- upload/download;
- drag/drop;
- copy/move;
- trash;
- permissions;
- ACL editor;
- properties;
- archive operations;
- context menus;
- preview pane;
- Open With.

### Large files

Uploads/downloads must stream.

For resumable browser uploads:

- use chunk IDs or an upload session ID;
- persist uploaded ranges;
- validate final length/checksum;
- clean abandoned sessions.

Never load entire large files into process memory.

---

## 11. Gallery Architecture

Frontend handles browser-native formats directly when practical.

For unsupported formats:

```text
source file
  |
  v
preview worker
  |
  +-- libvips / image libraries
  +-- optional RAW decoder
  |
  v
cached browser-safe preview
```

Preview cache keys should include:

- source identity;
- modification time;
- requested dimensions;
- decoder version.

Originals remain unchanged.

SVG must be treated carefully to avoid active-content security problems when rendered.

---

## 12. Video Architecture

Playback flow:

```text
File
 |
 +-- browser-compatible container/codec --> direct ranged stream
 |
 +-- incompatible but remuxable ---------> FFmpeg remux stream
 |
 +-- incompatible codec -----------------> FFmpeg transcode stream
```

FFmpeg jobs:

- run with resource limits;
- run as the correct user where local permissions matter;
- use temporary/cache storage with quotas;
- terminate when playback session ends unless background caching is intended.

The player should support:

- seek;
- subtitles where available;
- audio tracks;
- playback speed;
- fullscreen;
- resume position.

---

## 13. Music Architecture

The Music app maintains a lightweight media index.

Store:

- path/provider identity;
- track metadata;
- artist;
- album;
- artwork cache reference;
- duration;
- favorites;
- play history;
- playlist membership.

Indexing must be incremental and administratively bounded.

Playback uses direct streaming when the browser supports the codec and FFmpeg conversion otherwise.

---

## 14. Documents and Office

### PDF

Use a browser-native PDF application based on a maintained PDF rendering library such as PDF.js inside the CloudDesk UI.

Do not embed untrusted PDF content with more browser privileges than necessary.

### Office

Office editing is an optional runtime controlled by Settings.

Preferred architecture:

```text
CloudDesk Office UI
      |
      v
Office integration gateway
      |
      v
LibreOfficeKit / Collabora-compatible runtime
      |
      v
VFS file
```

Requirements:

- per-user authorization;
- temporary edit locks;
- autosave;
- safe-save/atomic replacement where provider supports it;
- conflict detection;
- audit events.

The integration layer must not assume local files only.

---

## 15. Code Workspace

Use a VS Code-compatible server runtime such as OpenVSCode Server or another legally compatible provider selected during implementation.

CloudDesk should wrap it with:

- SSO/session handoff;
- per-user workspace policy;
- per-user process isolation;
- controlled folder roots;
- lifecycle management;
- audit events.

Each user receives their own runtime/session.

Settings exposes:

- enabled/disabled;
- memory/CPU limit;
- allowed extensions policy;
- default workspaces;
- dev-container permission.

When disabled, the runtime must not remain resident.

---

## 16. Local Terminal

Use a server-side PTY bridged over WebSocket.

Flow:

```text
Browser terminal emulator
   |
 WebSocket
   |
cloudeskd/session broker
   |
cloudesk-sessiond as user UID
   |
PTY -> bash/zsh/fish
```

Controls:

- origin validation;
- session authorization;
- bounded terminal sessions;
- idle timeout;
- explicit close;
- resize messages;
- no arbitrary UID selection by the client.

Root access, if allowed, uses normal audited elevation policy rather than an automatically root terminal.

---

## 17. Browser Runtime

Arbitrary websites cannot be implemented reliably by iframe embedding.

CloudDesk Browser therefore uses an isolated server-side Brave process.

Recommended architecture:

```text
CloudDesk Browser tab
       |
       | WebRTC/video + secure input channel
       v
Browser Broker
       |
       v
isolated Brave session
       |
       +-- user profile
       +-- virtual/headless display
       +-- audio capture
       +-- download integration
```

This is application-level browser streaming, not host desktop streaming.

### Isolation

Each browser session should use:

- separate Linux process namespace or container sandbox;
- seccomp/AppArmor/SELinux policy where available;
- cgroup CPU/memory limits;
- dedicated profile directory;
- no access to arbitrary host paths;
- controlled download/upload bridge;
- separate runtime user.

### Profiles

Recommended defaults:

- Administrator: persistent isolated profile
- Manager: persistent isolated profile
- User: persistent isolated profile
- Guest: ephemeral profile destroyed at logout/session expiration

Profile directories must never be shared between users.

### Downloads

Browser downloads enter a CloudDesk-controlled download directory and become visible through Files according to user policy.

### Alpine compatibility

If native Brave packaging is not practical on Alpine, the optional Browser Runtime may use a compatible isolated OCI image while the CloudDesk core remains native.

---

## 18. Remote Server Manager

Remote server record:

```text
id
name
host
port
username
tags
notes
auth_method
credential_reference
known_host_policy
known_host_fingerprint
proxy_jump_reference
created_by
```

Supported SSH authentication:

- password;
- PEM;
- RSA;
- Ed25519;
- encrypted private keys;
- passphrases;
- SSH agent;
- keyboard-interactive;
- custom port;
- ProxyJump/bastion;
- known_hosts verification;
- SSH certificates.

### Host verification

Default policy must be secure.

CloudDesk should use known_hosts semantics.

First connection should present the fingerprint to an authorized user or use an explicitly configured trust-on-first-use policy. Host key changes must fail closed until reviewed.

---

## 19. Secrets Vault

Use envelope encryption.

Recommended model:

```text
random secret data key (DEK)
        |
encrypt secret with XChaCha20-Poly1305 or AES-256-GCM
        |
wrap DEK with installation master key
        |
store ciphertext + wrapped DEK + metadata
```

Installation master key:

- generated during install;
- stored outside SQLite;
- root-owned;
- mode `0600`;
- never returned through the API;
- optionally migrated later to TPM/PKCS#11/KMS/HSM.

Sensitive decrypted values:

- exist only in memory;
- are held for the shortest useful time;
- are zeroized where practical;
- are never logged.

Vault access must be authorization-checked and audited.

The threat model must document that full root compromise of the host can ultimately compromise application secrets; CloudDesk protects secrets at rest and from lower-privilege compromise, not from an already-controlled kernel/root account.

---

## 20. Transfer Engine

Transfers are persistent server-side jobs.

State machine:

```text
QUEUED
  |
RESOLVING
  |
CONNECTING
  |
TRANSFERRING
  |
VERIFYING
  |
COMPLETED

Any active state may enter:
PAUSED / RETRY_WAIT / FAILED / CANCELED
```

Persist:

- source;
- destination;
- owner;
- protocol;
- bytes total;
- bytes completed;
- timestamps;
- retry count;
- error category;
- checksum state;
- current state.

### Strategy selection

The engine chooses the best available path.

Priority:

1. provider-native/server-side copy;
2. direct remote-to-remote command or protocol flow where safely supported;
3. CloudDesk server-side streaming relay;
4. fail with a clear unsupported reason.

Never use:

```text
Remote A -> user's browser -> Remote B
```

### Relay behavior

Fallback relay should:

- stream in bounded chunks;
- avoid permanent local copies;
- use temporary spooling only when required;
- enforce disk quotas;
- resume if protocol capabilities allow it.

### Protocols

Required v1 providers/transports:

- local filesystem;
- SSH;
- SFTP;
- SCP;
- WebDAV;
- S3-compatible APIs.

### Reliability

Support:

- retries with backoff;
- restart persistence;
- pause/resume when supported;
- cancel;
- checksums;
- transfer history;
- progress WebSocket events.

---

## 21. Audit Architecture

Use an append-only audit table plus tamper-evident chaining.

Example event fields:

```text
id
timestamp
user_id
role_snapshot
session_id
source_ip
user_agent
action
resource_type
resource_id
path
remote_target
result
metadata_json
previous_hash
event_hash
```

`event_hash` should cover canonicalized event content plus `previous_hash`.

This makes deletion/modification detectable.

Audit exports may also be forwarded to:

- journald/syslog;
- JSON log files;
- external SIEM in a future integration.

Ordinary users cannot modify audit data.

The UI may filter what each role is allowed to view.

---

## 22. Settings and Server Administration

Settings has two classes of options.

### CloudDesk settings

Examples:

- UI mode;
- theme;
- session policy;
- 2FA policy;
- default role;
- file upload limits;
- transfer concurrency;
- cache quotas;
- application enable/disable;
- Browser runtime;
- Code runtime;
- Office runtime;
- FFmpeg transcoding;
- audit retention/export;
- reverse proxy/trusted proxy settings.

### Linux system settings

Examples:

- system information;
- network;
- firewall;
- users/groups;
- services;
- SSH daemon;
- packages;
- mounts;
- hostname;
- timezone;
- updates;
- logs;
- reboot;
- shutdown.

Linux system changes are executed only through explicit privileged-helper operations.

---

## 23. API Design

Use versioned REST-style APIs plus WebSocket streams.

Base:

```text
/api/v1/
```

Representative resources:

```text
auth/
sessions/
users/
roles/
files/
providers/
media/
remote-servers/
transfers/
terminal/
apps/
settings/
system/
audit/
```

WebSocket endpoints handle:

- terminal I/O;
- transfer progress;
- job progress;
- browser session signaling;
- desktop notifications.

Every endpoint must perform backend authorization.

Do not rely on the frontend for enforcement.

---

## 24. Database Domains

SQLite stores application state, not large file payloads.

Suggested tables/domains:

```text
users
linux_mappings
roles
permissions
role_permissions
user_permissions
sessions
totp
recovery_codes
assigned_roots
remote_servers
secret_records
storage_providers
transfer_jobs
transfer_events
favorites
recent_files
media_index
playlists
app_settings
system_settings
audit_events
schema_migrations
```

Secrets are encrypted before insertion.

---

## 25. Network Architecture

Default listener:

`0.0.0.0:9870`

Default protocol:

HTTPS.

### Initial IP access

On first install:

- generate a self-signed certificate;
- expose `https://<server-ip>:9870`;
- print the certificate warning expectation;
- print a one-time administrator/bootstrap flow.

Plain HTTP may be explicitly enabled for trusted development networks but must not be the production default.

### Reverse proxy

Support:

```text
Internet
  |
Caddy / Nginx / Traefik
  |
CloudDesk :9870
```

Trusted proxy headers must only be honored from explicitly configured proxy addresses.

---

## 26. Distro Abstraction

All listed distributions are official v1.0 targets.

### Package managers

```text
Debian/Ubuntu      apt
RHEL/Rocky/Alma    dnf
Fedora             dnf
Arch               pacman
Alpine             apk
```

### Service managers

Most targets:

- systemd

Alpine:

- OpenRC by default

CloudDesk must not embed systemd assumptions in core business logic.

Create interfaces for:

- service start/stop/enable;
- package install/update;
- firewall adapter;
- network inspection;
- logs;
- reboot/shutdown.

### Mandatory CI

Run installation/integration tests using containers or VMs for every supported distribution.

Some privileged/system tests require VM-based CI rather than containers.

---

## 27. Linux Security Integration

Where available, ship hardening profiles:

- systemd sandboxing;
- `NoNewPrivileges`;
- restricted capabilities;
- AppArmor profiles;
- SELinux policy;
- seccomp for optional runtimes;
- cgroup resource limits.

`cloudeskd` should receive no Linux capabilities by default unless a specific need is proven.

`cloudesk-privd` remains deliberately tiny.

---

## 28. Resource Management

### Core budget

Target the always-on core for small systems.

Strategies:

- Rust resident services;
- SQLite;
- no mandatory Redis;
- no mandatory message broker;
- no mandatory PostgreSQL;
- static frontend;
- bounded caches;
- streaming I/O;
- low-frequency event-driven monitoring;
- avoid polling loops.

### Optional runtime controls

Settings must expose:

```text
Browser: enabled/disabled
Code: enabled/disabled
Office: enabled/disabled
Media transcode: enabled/disabled
```

Each runtime should support optional:

- memory limit;
- CPU quota;
- max concurrent users;
- idle shutdown timeout.

---

## 29. App Model

Built-in applications should register through a common internal manifest.

Example:

```text
id
name
icon
route
required_permissions
file_associations
runtime_dependency
enabled
```

This provides a plugin-ready architecture without requiring a public third-party app store in v1.0.

A future SDK can use the same internal boundaries.

---

## 30. Licensing Architecture

Recommended product model:

### Community edition

AGPL-3.0.

### Commercial edition

Separate commercial license offered by the project owner.

For dual licensing to remain practical:

- project-owned code must remain relicensable;
- contributor terms should explicitly permit commercial relicensing;
- third-party dependencies must be tracked;
- incompatible licenses must not be introduced;
- optional proprietary modules must have clean boundaries if ever added.

A lawyer should review the final licensing and contributor agreement before commercial release.

---

## 31. Critical Security Rules

These are architectural invariants.

1. `cloudeskd` never runs permanently as root.
2. No generic root shell/command endpoint exists in `privd`.
3. Browser-supplied paths are never trusted directly.
4. Authorization is checked server-side for every operation.
5. Private keys and passwords are never stored plaintext.
6. Secrets never appear in logs.
7. SSH host verification is enabled by default.
8. Remote-to-remote transfer never uses the user's browser as the data path.
9. Heavy runtimes are isolated from the host filesystem.
10. Guest browser profiles are ephemeral.
11. Root-scope file access requires explicit step-up authorization.
12. All security-relevant actions generate audit events.
13. WebSocket connections repeat authentication/authorization checks.
14. File previews are treated as untrusted content.
15. Uploaded archives are never automatically extracted outside authorized roots.
16. Symlink traversal and TOCTOU file attacks must be explicitly tested.
17. Installer output must not expose reusable secrets beyond the initial bootstrap mechanism.

---

## 32. v1.0 Acceptance Architecture

A v1.0 build is not release-ready until:

- all official distributions install successfully;
- port 9870 HTTPS first access works;
- multi-user authentication and 2FA work;
- Linux-user mapping is enforced;
- root helper passes security review;
- Files works across local/SFTP/WebDAV/S3 providers;
- media and PDF apps work;
- Office editing works when enabled;
- Code works when enabled;
- Browser works when enabled without exposing a host desktop;
- Terminal works under the correct UID;
- transfers survive restart;
- audit logs are tamper-evident;
- disabled optional runtimes consume no resident application resources;
- all critical authorization tests pass.
