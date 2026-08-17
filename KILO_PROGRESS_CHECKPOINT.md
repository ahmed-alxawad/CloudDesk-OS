# CloudDesk-OS — Codex Progress Checkpoint

This checkpoint was reconstructed from the Codex session transcript supplied by the project owner.

It is **evidence of intended/completed work, not a substitute for inspecting the repository**. Kilo must verify each item against the actual source tree, migrations, tests, Git state, and build results before marking it complete.

## Known Codex stopping point

Codex stopped immediately after identifying and beginning to fix this architecture mismatch:

> The Vault encrypted records directly with the installation key, but `ARCHITECTURE.md` requires true envelope encryption.

The next implementation task is therefore expected to be:

**Finish and validate true per-record envelope encryption in the Vault.**

Kilo must confirm the repository actually contains the partial change before continuing.

---

## Work reported complete or substantially complete by Codex

### Repository / Phase 0 foundation

Reported complete:

- Rust workspace
- Svelte + TypeScript frontend
- SQLite migrations
- `cloudeskd`
- health endpoint
- configuration layer
- application manifest validation
- capability registry
- CI
- formatting/linting
- documentation
- threat model
- architecture decisions
- distro installer layout tests

Reported validation:

- strict Rust formatting
- Clippy with warnings denied
- backend tests
- frontend lint/test/build

### Installer / HTTPS

Reported implemented:

- installer structure for all eight official distributions
- port `9870`
- production TLS support
- explicit local HTTP opt-in
- first-run/bootstrap flow
- service packaging
- OpenRC + systemd packaging

Reported supported distribution IDs:

- Debian
- Ubuntu
- RHEL
- Fedora
- Rocky Linux
- AlmaLinux
- Arch Linux
- Alpine Linux

Codex also fixed an installer ownership defect involving the encryption key and the unprivileged core service.

### Authentication / authorization / audit

Reported implemented:

- Argon2id password hashing
- TOTP
- encrypted TOTP secret storage
- recovery codes
- opaque sessions
- session revocation
- account/IP rate limiting
- granular capabilities
- role/permission administration
- step-up authentication
- one-time bootstrap
- login/logout
- session listing
- audit hash chain
- serialized persisted audit chain head
- HSTS
- CSRF/cross-site rejection
- browser security headers
- atomic bootstrap transaction
- audit concurrency tests

### Privilege separation / Linux identity

Reported implemented:

- root-owned `cloudesk-privd`
- root-owned Unix socket
- peer UID checks
- short-lived signed grants
- grant binding to server-side user mapping
- non-root `cloudesk-sessiond`
- PTY launch under mapped UID/GID
- fixed/typed privileged actions
- no generic arbitrary root command endpoint
- service/power controls
- protection preventing CloudDesk security/core units from being targeted
- real root-to-non-root boundary tests

### CloudDesk shell

Reported implemented:

- first-run UI
- login/logout UI
- Desktop mode
- Dashboard mode
- Desktop default
- manifest-driven app launcher
- drag/resize/minimize/maximize/focus
- dock/taskbar state
- notifications
- keyboard launch shortcuts
- per-user layout persistence
- policy-aware app visibility

### VFS / Files foundation

Reported implemented:

- capability-relative local filesystem handles
- parent traversal rejection
- symlink escape protection
- Unix UID/GID/mode metadata
- provider capability model
- list/stat/create/rename/copy/trash
- bounded preview
- mapped-user execution via `sessiond`
- API-signed typed file operations
- real dropped-UID listing boundary test

This does not prove every final File Manager feature from `GOAL.md` is complete. Kilo must compare the implementation with the full v1.0 acceptance requirements.

### Web boundary hardening

Reported implemented:

- strict same-site cookie model
- JSON-only mutation assumptions
- explicit cross-site request rejection
- baseline security headers
- HSTS
- manifest validation in Rust runtime

### Vault / secrets

Reported implemented before the identified mismatch:

- encrypted secret records
- owner scoping
- plaintext absent from SQLite
- cross-user reveal denial
- rotation
- scoped deletion
- step-up gated API mutations
- auditing

**Important:** Codex then found that the implementation encrypted records directly with the installation key. This is not compliant with the required envelope-encryption architecture. The fix was started but not completed before usage limits were reached.

### Transfer control plane

Reported implemented:

- persistent transfer queue
- restart persistence
- state machine
- retries/backoff
- pause/resume/cancel
- direct/provider-native strategy selection
- server-side relay strategy
- no browser data-path strategy
- authenticated enqueue/list/get/pause/resume/cancel APIs
- ownership isolation
- audited transfer mutations
- atomic queue claim
- concurrency test preventing double claim
- Transfers UI

**Important:** the logs distinguish the control plane from "actual background execution". Kilo must verify whether the real data-plane transfer workers/providers were completed.

### Terminal / typed server controls

Reported implemented:

- typed service actions
- typed power actions
- root helper enforcement
- step-up authorization
- audit coverage
- mapped-user PTY WebSocket
- origin protection
- terminal session start/stop audit
- real UID/GID PTY boundary test
- xterm-based web terminal
- resize
- binary I/O
- reconnect backoff
- explicit disconnect
- lazy loading/code splitting

### Remote Server Manager foundation

Reported implemented:

- owner-scoped server records
- authentication method metadata
- Vault credential references
- ProxyJump references
- tags
- pinned host keys
- host verification fail-closed
- host-key scanning workflow
- fingerprint selection
- deletion
- live re-verification
- remote UI
- OpenSSH client installer dependency on supported distributions

The transcript claims support foundations for:

- password
- PEM
- RSA
- Ed25519
- encrypted keys/passphrases
- SSH agent
- keyboard-interactive
- custom ports
- ProxyJump
- SSH certificates

Kilo must verify which of these are real end-to-end connection paths versus persisted metadata/API scaffolding.

---

## Known files created/modified by Codex

The project owner supplied a large change list including:

- root workspace files (`Cargo.toml`, `Makefile`, `README.md`, `.editorconfig`, CI)
- `apps/web/`
- app manifests
- `crates/audit`
- `crates/auth`
- `crates/config`
- `crates/db`
- `crates/linux`
- `crates/permissions`
- `crates/privilege`
- `crates/remote`
- `crates/runtime`
- `crates/secrets`
- `crates/transfers`
- `crates/vault`
- `crates/vfs`
- migrations `0001` through `0008`
- installer and distro adapters
- systemd/OpenRC packaging
- `services/clouddeskd`
- `services/cloudesk-privd`
- `services/cloudesk-sessiond`
- integration/security/distro test skeletons

Kilo must use Git and the filesystem as the source of truth.

---

## Expected first continuation task

1. Inspect current Git status and partial Vault edits.
2. Read:
   - `ARCHITECTURE.md`
   - Vault-related sections of `GOAL.md`
   - Vault-related `PLAN.md` items
   - `crates/vault/`
   - `crates/secrets/`
   - vault migrations, especially `0004_vault.sql` and `0008_vault_envelope_keys.sql`
   - relevant API handlers in `services/clouddeskd`
3. Use Graphify to update the project graph.
4. Finish the envelope encryption design:
   - random per-record DEK;
   - authenticated encryption of the secret with the DEK;
   - wrapping/encrypting the DEK with the installation KEK/master key;
   - record identity/owner binding in AAD;
   - no plaintext secret or DEK in SQLite;
   - rotation behavior;
   - deletion semantics;
   - migration/backward compatibility for existing records if needed;
   - zeroization where practical;
   - no secret leakage in logs/errors/API.
5. Add/finish tests.
6. Run focused Rust tests and strict Clippy.
7. Then continue to the next genuinely incomplete `PLAN.md` item.

---

## Do not assume these are complete

The transcript does **not** prove production completeness for:

- real background transfer data plane
- full SFTP/SCP transfer execution
- WebDAV provider
- S3-compatible provider implementations
- server-to-server transfer matrix
- Gallery
- full Video/FFmpeg pipeline
- full Music library/player
- PDF viewer completeness
- VS Code-compatible runtime
- LibreOffice/Collabora editing runtime
- isolated Brave runtime
- full Linux server administration adapters
- all File Manager UX requirements
- all SSH authentication methods end-to-end
- all eight distributions under real install/upgrade/uninstall CI
- SELinux/AppArmor/seccomp hardening
- performance target validation
- release security review
- production backup/restore/upgrade docs
- release packaging/signatures
- final license/dependency review

Kilo must inspect the real repository and `PLAN.md` rather than inferring completion from this checkpoint.
