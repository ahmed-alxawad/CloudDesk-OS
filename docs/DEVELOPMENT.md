# Development standards

## Supported tools

- stable Rust, formatted with `rustfmt` and linted with Clippy;
- Svelte and strict TypeScript, built by Vite;
- Prettier and `svelte-check` for frontend formatting and static checks;
- Rust unit/integration tests and Vitest frontend unit tests.

Run `make check` and `make test` before submitting a change. CI runs the same
checks. New schema changes require a new immutable SQL file in `migrations/`;
never edit an already released migration.

## Module boundaries

Business rules belong in small workspace crates. `cloudeskd` owns process
startup, transport, and composition. The frontend never grants authority: it may
hide unavailable actions, but every API or WebSocket operation must be authorized
by the backend.

New dependencies need a concrete purpose and a license compatible with the
project. Always-on services are not introduced when an in-process or on-demand
component is sufficient.

## Security review checklist

- Does the operation have a named capability?
- Is authorization enforced at the backend boundary?
- Is every browser-provided identifier treated as untrusted?
- Does the operation need an audit event?
- Could a secret reach logs, SQLite plaintext, or an API response?
- Does local work run under the correct Linux UID/GID?
- Are symlink, traversal, and time-of-check/time-of-use cases tested?

