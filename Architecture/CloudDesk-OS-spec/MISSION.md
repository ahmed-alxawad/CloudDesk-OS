# CloudDesk-OS Mission

## Mission

CloudDesk-OS is a lightweight, browser-native workspace for managing and using Linux servers without requiring a traditional desktop environment, RDP, or VNC.

Its purpose is to make a Linux server feel approachable and productive from a web browser while preserving the security model, permissions, ownership, and operational power of Linux underneath.

CloudDesk-OS should provide a unified place to:

- manage local and remote files;
- view images, video, audio, PDF, and office documents;
- edit office documents;
- code in a VS Code-compatible workspace;
- use a full remote Brave browser;
- open local and remote terminals;
- administer Linux server settings;
- connect to remote Linux servers over SSH;
- transfer files between servers and storage providers;
- manage users, roles, sessions, credentials, and audit history.

The product must remain useful on extremely small Linux servers. Resource-heavy applications are part of v1.0, but they are optional runtimes that administrators can enable or disable independently.

## Product Promise

CloudDesk-OS should feel like a small web desktop, not like a remote desktop session.

The user opens:

`https://<server-ip>:9870`

and receives a CloudDesk interface containing applications such as:

- Files
- Gallery
- Video
- Music
- Documents
- Office
- Code
- Terminal
- Browser
- Remote Servers
- Transfers
- Settings

The default experience is a windowed web desktop. During installation, the administrator can instead choose a simpler application-dashboard mode.

## Core Principles

### 1. Browser-native, not desktop streaming

CloudDesk-OS does not install or stream KDE, GNOME, XFCE, Windows, or another desktop environment.

The CloudDesk shell, file manager, settings, transfer manager, media players, and administration UI are native web applications.

The Browser app is a special case: a dedicated Brave instance runs server-side and only that browser application is remotely rendered into CloudDesk. It is not VNC/RDP access to the Linux host desktop.

### 2. Lightweight by default

The always-on CloudDesk core must remain small.

Brave, VS Code, LibreOffice/office editing, and FFmpeg transcoding must be independently switchable from Settings. Disabling them must stop their resident services and release their resources.

CloudDesk should remain practical on:

- Minimum: 1 CPU, 512 MB-1 GB RAM
- Comfortable: 2 CPU, 2 GB RAM

Heavy runtimes may require more memory while active.

### 3. Linux remains the authority

CloudDesk must respect Linux:

- users and groups;
- filesystem permissions;
- ownership;
- ACLs;
- process identity;
- service boundaries.

CloudDesk supports a hybrid identity model. Administrators may map CloudDesk users to real Linux accounts or keep them as CloudDesk-only users with explicitly assigned filesystem roots and capabilities.

### 4. Least privilege

The main web/API service must never run permanently as root.

Privileged Linux operations must go through a narrowly scoped privileged helper with explicit command validation, audit logging, step-up authorization, and minimal attack surface.

### 5. Security is a product feature

Security is a non-negotiable design constraint.

CloudDesk must provide:

- password authentication;
- TOTP 2FA;
- recovery codes;
- session management;
- remembered-device controls;
- brute-force protection;
- role-based access control;
- granular permissions;
- encrypted secret storage;
- strict SSH host verification;
- audit logging;
- step-up authentication for sensitive actions;
- secure-by-default network configuration.

### 6. One file experience

Local files, remote SSH/SFTP files, S3-compatible storage, and WebDAV should appear through a common file abstraction.

A user should be able to move from:

`Local -> Remote Server -> S3 -> WebDAV`

without learning separate interfaces.

### 7. Transfers belong to the server

Large transfers must continue when the user closes the browser.

CloudDesk should prefer direct server-to-server transfers whenever possible and fall back to a server-side streaming relay when necessary.

The user's web browser must never be the data path for a server-to-server transfer.

### 8. Modular v1.0

Everything described in this specification is required for v1.0, but it must not be implemented as one monolith.

Applications and integrations must have clear boundaries so optional runtimes can be disabled and future CloudDesk applications can be added without rewriting the core.

### 9. Distribution portability

v1.0 must officially support:

- Debian
- Ubuntu
- RHEL
- Fedora
- Rocky Linux
- AlmaLinux
- Arch Linux
- Alpine Linux

Core functionality must be native on all supported systems.

When a heavy third-party runtime cannot be distributed cleanly on a specific host OS, CloudDesk may use an isolated OCI/container compatibility runtime for that optional feature while keeping CloudDesk Core native.

### 10. Open source and commercial use

Recommended licensing model:

- CloudDesk-OS Community: AGPL-3.0
- CloudDesk-OS Commercial: separate commercial license

The project should maintain contributor terms that preserve the owner's ability to offer commercial licensing. Third-party components remain governed by their own licenses and must be reviewed before release.

## Target Users

CloudDesk-OS is for:

- Linux server owners;
- home-lab users;
- developers;
- small hosting environments;
- server administrators;
- teams managing multiple Linux servers;
- users who want GUI-style server workflows without installing a desktop environment.

## What CloudDesk-OS Is Not

CloudDesk-OS is not:

- a replacement Linux distribution;
- an RDP or VNC server;
- a streamed KDE/GNOME desktop;
- a browser iframe wrapper;
- a root-running web control panel;
- a cloud-only SaaS dependency.

It is a web operating workspace layered safely on top of an existing Linux server.

## v1.0 Mission Outcome

CloudDesk-OS v1.0 succeeds when a user can install it with one command, open port 9870 in a browser, securely authenticate, work with local and remote files, open those files in the correct CloudDesk application, administer permitted Linux settings, code, edit documents, browse the web through an isolated Brave runtime, connect to other servers, and run reliable background transfers without installing a traditional Linux desktop.
