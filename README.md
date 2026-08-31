# CloudDesk-OS

CloudDesk-OS is a lightweight, multi-user web desktop for Linux servers.
Latest stable tagged release: `v1.0.0`. The current public prerelease is
[`v1.0.1-rc.4`](https://github.com/ahmed-alxawad/CloudDesk-OS/releases/tag/v1.0.1-rc.4)
— a release candidate, not a stable release — see `RELEASE_NOTES.md`.
Every release asset is built from an exact tagged source commit and
cryptographically attested via
[GitHub Artifact Attestations](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds);
verify any downloaded file yourself with
`gh attestation verify <file> --repo ahmed-alxawad/CloudDesk-OS`.

It provides a secure, self-hosted web workspace: a File Manager, Terminal,
Gallery/media viewer, Video and Music players, a Secrets Vault with a remote
server manager (SSH/SFTP/SCP, WebDAV, S3), server-to-server transfers, and
optional heavier runtimes — a VS Code-compatible editor (Code, via a patched
`code-server`), a full office suite (Office, via Collabora Online/LibreOffice),
and an isolated remote browser (Browser, via Brave/Chromium) — administered
through a Settings app, with strict Linux-identity-based privilege separation
throughout. It is **not** a full remote-desktop/VNC replacement, does not
expose the host over VNC/RDP, and does not route server-to-server transfers
through the user's own browser.

## Supported platforms

Installer and service-lifecycle evidence exists for, on `x86_64` only (no
`arm64`/`aarch64`/`armv7` support is implemented, tested, or claimed):

| Distro | Service manager | Artifact | Status |
| --- | --- | --- | --- |
| Debian 12/13 | systemd | glibc | PASS |
| Ubuntu 24.04 | systemd | glibc | PASS |
| Fedora 41 | systemd | glibc | PASS |
| Rocky Linux 9 | systemd | glibc | PASS |
| AlmaLinux 9 | systemd | glibc | PASS |
| Arch Linux | systemd | glibc | PASS |
| Alpine Linux 3.20 | OpenRC | musl (native) | PASS |
| RHEL 9 (full subscribed) | systemd | glibc | **UNAVAILABLE** — no subscribed environment tested; see RHEL UBI9 row |
| RHEL UBI9 (compatibility control) | systemd | glibc | PASS — Red Hat's own official redistributable content, not equivalent to a subscribed RHEL install |

## Installation

`installer/install.sh` auto-detects the distribution, installs required OS
packages, selects the correct pre-built artifact (glibc for every family
above except Alpine, native musl for Alpine), creates the `clouddesk`
service account, generates TLS material, initializes SQLite, configures the
service manager, and starts CloudDesk on TCP port `9870`. It supports two
modes:

- **Local/offline mode** (default): operates on a local checkout with
  locally-built or locally-placed artifacts (`dist/linux-x86_64-{glibc,musl}`,
  or `CLOUDESK_BINARY`-style overrides). Used for development, CI, and the
  distro-matrix test harness.
- **Public download mode** (recommended): the installer fetches its own
  binaries and web bundle from
  [GitHub Releases](https://github.com/ahmed-alxawad/CloudDesk-OS/releases),
  verifying version consistency and SHA256 checksums before installing
  anything, and failing closed on any mismatch:
  ```sh
  curl -fsSL https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/v1.0.1-rc.4/install.sh \
      | sudo env CLOUDESK_VERSION=1.0.1-rc.4 bash
  ```
  `v1.0.1-rc.4` is currently the latest published release candidate (a
  prerelease, not a stable release — see `RELEASE_NOTES.md`).

Initial access: `https://<server-ip>:9870`, using the bootstrap secret the
installer prints. A self-signed certificate is used on first install — expect
and accept the browser's certificate warning for that initial connection, or
place a reverse proxy with your own certificate/ACME in front of CloudDesk
(see `docs/DEPLOYMENT.md`). CloudDesk never asks you to disable TLS
certificate verification anywhere, in the browser or otherwise.

## System requirements

Minimum: 1 CPU, 512MB–1GB RAM for core CloudDesk with all optional runtimes
disabled. Comfortable baseline: 2 CPU, 2GB RAM. Optional heavier
runtimes (Browser, Code, Office) are disabled by default and, when disabled,
run zero resident containers/processes — enabling one adds its own real
resource cost on top of the baseline, proportional to what that runtime
itself needs.

## Security model

Non-root main service (`clouddeskd` refuses to run as UID 0); a narrow,
typed privileged helper (`cloudesk-privd`) for the small set of operations
that genuinely require elevated privilege, with no generic root-command API;
envelope-encrypted secrets/SSH-key storage (KEK→DEK→secret); SSH host-key
pinning with mismatch rejection; a tamper-evident, hash-chained audit log;
path/archive-traversal and symlink-escape protections; Workspace Trust
retained in the Code runtime; and isolated, non-root optional runtimes with
no host filesystem bind-mounts for Browser/Office. See `docs/SECURITY.md`
and `docs/THREAT_MODEL.md` for the full security model.

## Known environment limitations

These are evidence gaps from this project's own test environment, not known
product defects:

- **SELinux enforcing mode**: not exercised (`BLOCKED BY ENVIRONMENT`).
- **True reboot persistence**: tested via container restart and service
  enablement, not a genuine cold boot (`BLOCKED BY ENVIRONMENT`).
- **RHEL 9 full subscribed environment**: unavailable to this project's test
  environment (`UNAVAILABLE`); the UBI9 compatibility-control row above is
  real evidence but not a substitute.

## Licensing

CloudDesk-OS is licensed `AGPL-3.0-or-later` for the community edition (see
`Cargo.toml`/`apps/web/package.json`); a separate commercial licensing model
is planned but not yet finalized as of this release candidate. Third-party
components CloudDesk-OS runs as external, unmodified containers (code-server,
Collabora Online, Brave, FFmpeg) retain their own separate licenses — see
`docs/THIRD_PARTY_NOTICES.md`.

## Repository map

- `apps/web` — Svelte and TypeScript static web shell;
- `crates/` — shared Rust libraries (config, db, auth, vfs, vault, audit,
  privilege, remote, media, orchestrator, and more);
- `services/clouddeskd` — the unprivileged Axum core service;
- `services/cloudesk-privd` — the narrow, typed privileged helper;
- `services/cloudesk-sessiond` — the per-user session worker `privd` spawns;
- `installer/` — the distro-portable installer and per-family service units;
- `packaging/` — release builder Dockerfiles and build scripts;
- `migrations` — append-only database migrations;
- `docs` — threat model, security, deployment, and backup/restore docs;
- `tests` — shared integration, security, and distro test homes.

## Development

Prerequisites are a current stable Rust toolchain and Node.js 22 or newer.

```sh
make bootstrap
make build
make test
make check
```

To build the web shell, migrate the local database, and start `cloudeskd`
directly for local development (a plain-HTTP local dev mode, not the
installed HTTPS product):

```sh
cd apps/web && npm run build
cd ../..
make migrate
make dev
```

Open `http://127.0.0.1:9870`. For a real installed instance with HTTPS, the
real service manager, and the real privileged helper, use
`installer/install.sh` instead (see "Installation" above, and
`tests/distro/README.md` for the harness this project's own installer
evidence was produced with).
