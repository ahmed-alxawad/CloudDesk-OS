# CloudDesk-OS

CloudDesk-OS is a lightweight, multi-user web desktop for Linux servers (Release: `v1.0.0`).
It provides a secure, web-accessible workspace featuring native browser applications,
remote infrastructure management, isolated container runtimes, and strict privilege separation.

The authoritative product architecture and specifications live in
[`Architecture/CloudDesk-OS-spec`](Architecture/CloudDesk-OS-spec).

## Repository map

- `apps/web` — Svelte and TypeScript static web shell;
- `crates/config` — strict TOML configuration;
- `crates/db` — SQLite connection and SQLx migrations;
- `crates/permissions` — backend capability registry;
- `crates/runtime` — application manifest contract;
- `services/clouddeskd` — unprivileged Axum core service;
- `migrations` — append-only database migrations;
- `docs` — threat model and engineering contracts;
- `tests` — shared integration, security, and distro test homes.

## Development

Prerequisites are a current stable Rust toolchain and Node.js 22 or newer.

```sh
make bootstrap
make build
make test
make check
```

To run the Phase 0 development server, build the web shell, migrate the local
database, and start `cloudeskd` as a non-root user:

```sh
cd apps/web && npm run build
cd ../..
make migrate
make dev
```

Open `http://127.0.0.1:9870`. HTTP is only an explicitly enabled Phase 0 local
development mode. HTTPS installation and first-run access belong to Phase 1.

`cloudeskd` refuses to run with effective UID 0. The future privileged helper
will be a separate, narrow service; no generic privileged-command API belongs in
the core.

