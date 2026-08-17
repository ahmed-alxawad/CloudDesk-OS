# Phase 0 architecture decisions

## Monorepo and process boundaries

The Rust workspace mirrors the documented split between reusable domains and
service processes. Phase 0 implements only the unprivileged `cloudeskd` process.
Future `privd` and `sessiond` services must remain separate executables with
their own trust boundaries.

## Configuration

Configuration is TOML and rejects unknown fields. Production installation will
place it at `/etc/clouddesk/clouddesk.toml`; the checked-in configuration is only
for local development. The architecture default remains port `9870`. Development
HTTP requires an explicit flag, and the Phase 0 binary refuses to imply that it
is production TLS.

## Persistence and migrations

SQLite is opened through SQLx with foreign keys, WAL mode, a bounded pool, and a
busy timeout. SQLx owns migration history in `_sqlx_migrations`; every schema
change is an ordered, append-only file under `migrations/`. SQLite contains
application state, never bulk file payloads.

## Applications and permissions

Application metadata is data, not hard-coded navigation. JSON manifests conform
to `schemas/app-manifest.schema.json`, and backend parsing also validates every
requested capability against the closed registry in `clouddesk-permissions`.
Browser, Code, and Office remain optional runtime dependencies.

