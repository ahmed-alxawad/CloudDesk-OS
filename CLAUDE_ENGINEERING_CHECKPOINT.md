# CloudDesk-OS — Engineering Checkpoint

Branch: `engineering/v1-true-closure` (from `audit/claude-nightmare-v1.0.0`)
`v1.0.0` tag: untouched, unpublished. Nothing pushed.

## Last completed phase

**Phase 1 — File Manager Closure.** Complete.

## Verified items

- **Resumable uploads** (`b4a4660`) — persisted `upload_sessions` table,
  create/chunk/status/finalize/cancel HTTP surface, bounded-memory
  streaming, cross-user session isolation, checksum verification, atomic
  finalize, hourly abandoned-session janitor. `services/clouddeskd/tests/
  resumable_upload.rs`, real HTTP-router round-trips.
- **Archive create/extract** (`9b7aa74`) — ZIP + tar.gz,
  `crates/vfs/src/archive.rs`, wired through the existing
  `LocalFileOperation`/`/api/v1/files/local/actions` dispatcher (no new
  route). Zip Slip / Tar Slip / drive-letter / symlink-entry defenses,
  decompressed-byte quota checked against real bytes read (not a trusted
  header field), partial-extraction cleanup. `crates/vfs/tests/
  archive.rs`, 10 tests.
- **ACL read/edit** (`9b7aa74`) — `crates/vfs/src/acl.rs`, shells out to
  `getfacl`/`setfacl` with a fixed argv against an in-process-resolved
  real path (never a caller-supplied string, never a shell). Gated by its
  own `files.permissions.change` capability, administrator-only by
  default. `crates/vfs/tests/acl.rs`, 6 tests, run against this
  container's real `getfacl`/`setfacl`.

All three include a minimal Files UI surface
(`apps/web/src/lib/FilesApp.svelte`: toolbar buttons + an ACL section in
the existing Properties/file-details panel) — no UI redesign.

## What Phase 1's own task did NOT require and is genuinely not done

Be precise about this so the next session doesn't assume more than what
was tested:
- None of the three features have been exercised through a real browser
  session against a real running `clouddeskd` — all test evidence is at
  the `crates/vfs` function boundary (archives, ACL) or the axum
  `Router::oneshot` HTTP surface (resumable uploads), not a live end-to-end
  browser/network path.
- ACL entry *removal* isn't a distinct code path — "remove" today means
  calling `set_acl` with an entry whose permission bits are all `false`,
  which is what's tested. A dedicated `setfacl -x` removal isn't
  implemented separately (this achieves the same practical effect but
  leaves a zero-permission entry in the ACL rather than deleting the
  entry itself — cosmetically different from a real `getfacl` after a true
  removal, functionally equivalent).
- Symlink-escape denial for ACL specifically isn't independently tested —
  it relies on `cap_std::fs::Dir::open`'s sandboxing, which *is* tested
  elsewhere in the crate, but not from an ACL-specific test.
- The Task 4 "focused Files security review" in the prompt that produced
  this commit was satisfied primarily by the test suites above (which
  already cover traversal, symlink swap, archive traversal, malicious
  archive symlinks, cross-user session IDs, unauthorized destination,
  unauthorized chmod-adjacent ACL escalation, write-capability denial) —
  concurrent-upload-finalize and abandoned-session-cleanup races were
  reviewed by reading the code (finalize's `bytes_received == total_size`
  check plus an idempotent `rename` means a second concurrent finalize
  simply fails harmlessly; the janitor's 24h threshold makes a real race
  with an in-flight upload exceedingly unlikely) rather than proven with a
  dedicated concurrency test.

## Validation

```
cargo fmt --all -- --check                                          PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings PASS
cargo test --workspace                                              PASS (0 failures)
cd apps/web && npm run lint && npm run check && npm test && npm run build   PASS
```
(`cargo build --workspace --release` was not run — Phase 1's own
validation section only calls for the above; the release build is the
*final* engineering gate across all phases, not a per-phase gate.)

## Current commit

```
9b7aa74 feat(files): complete phase 1 file manager closure — archives and ACLs
```
on top of (preserved, untouched, still passing):
```
d277393 docs(engineering): checkpoint after resumable-upload closure
b4a4660 feat(files): implement resumable local-file uploads (GOAL.md G3)
dfdfade audit(evidence): repair fabricated acceptance runner, audit spec vs implementation, fix RSA SSH auth
289904b audit(nightmare): fix SSH host-key bypass and SFTP upload/list breakage; prep v1.0.1-rc.1
d6517bf audit(nightmare): require system.services.manage for /api/v1/system/summary
ffbc336 test: prepare Claude v1.0.0 nightmare audit
9b8f49a release: CloudDesk-OS v1.0.0   <- immutable tag v1.0.0 points here
```

All five prior Nightmare fixes are preserved and were not touched this
session: `system_summary` authorization, SSH host-key verification, SFTP
upload create-or-overwrite fix, SFTP non-chroot listing fix, RSA SHA-2
auth fix. Regression tests for all five still pass as part of the full
suite above.

## Next phase

**Phase 2 — Complete SSH feature matrix.**

## Next exact task

Wire and live-test ProxyJump through `RemoteServerStore`/Vault/API, then
implement/verify SSH agent, keyboard-interactive, SSH certificates, and
actual SCP streaming.

Concretely, in dependency order:

1. **ProxyJump product wiring** (the data model already has it —
   `RemoteServer.proxy_jump_server_id` exists and is saved, but nothing
   reads it). `SshSession::connect_proxyjump` already exists in
   `crates/remote/src/ssh.rs` and already has unit-test coverage
   (`crates/remote/tests/ssh.rs`, from the Nightmare-audit sessions). The
   gap is entirely in `services/clouddeskd/src/worker.rs`'s
   `get_provider` (and wherever terminal-open constructs an SSH
   connection): when `server.proxy_jump_server_id` is `Some`, look up
   that bastion `RemoteServer` (and its own pinned host key + auth
   material from Vault), and call `connect_proxyjump` instead of
   `connect_pinned`, verifying *both* the bastion's and the target's
   pinned host keys. Live-test against the repo's own disposable
   `tests/acceptance/docker-compose.yml` fixture (`openssh` container) —
   note that fixture is a single container, so a real ProxyJump chain
   needs either a second `openssh` service added to that compose file (a
   bastion + a target) or reusing one container as both hops with
   different ports/users; decide which when you get there.
2. **SSH agent** — `SshAuth::Agent` currently `bail!`s. `russh 0.62`
   likely has agent-client support (check `russh::keys::agent` — not
   investigated this session); wire real Unix-socket agent-protocol
   forwarding, never exporting the private key material itself out of the
   agent.
3. **Keyboard-interactive** — `SshAuth::KeyboardInteractive` currently
   `bail!`s ("not implemented in russh 0.62" per the existing comment;
   verify that's still accurate for whatever `russh` version is pinned by
   the time you pick this up). Needs real interactive prompt/response
   handling matching the SSH protocol's keyboard-interactive exchange.
4. **SSH certificates** — `SshAuth::Certificate` currently decodes only
   `key_data` and silently discards `cert_data` (a facade, flagged
   explicitly in `V1_TRUE_CLOSURE.md` item 13). Needs real OpenSSH
   certificate parsing/validation via `russh`'s certificate support.
5. **SCP** — no SCP-specific code exists at all (only SFTP).
   `V1_TRUE_CLOSURE.md` item 10 rates this MEDIUM, not BLOCKING, since
   `GOAL.md` G9's own wording ("SCP where appropriate") leaves room for a
   documented SFTP substitution — but no such decision is recorded
   anywhere, so treat it as still owed unless you get an explicit product
   decision to skip it.

For every one of the above: reproduce/implement, add a regression test
(prefer the deterministic in-process mock-server pattern already
established in `crates/remote/tests/ssh.rs` for the parts that don't need
real infrastructure; use the real disposable OpenSSH fixture via `docker
compose up -d` in `tests/acceptance/` for the parts that do), run the
hostile/failure cases the original task listed (bad key, bad certificate,
changed host key, dead bastion, wrong passphrase, unavailable agent,
interrupted SCP), `cargo fmt`/`clippy -D warnings`/targeted `cargo test`,
then update `V1_TRUE_CLOSURE.md` for whichever items close.

## Remaining closure blockers

Everything in `V1_TRUE_CLOSURE.md` except items 7, 8, 9 (closed). In
priority/dependency order:

1. SSH agent, keyboard-interactive, SSH certificates, ProxyJump product
   wiring, SCP — not started (Phase 2, next)
2. FFmpeg compatibility pipeline (probe/remux/transcode/job
   lifecycle/limits) — not started, zero implementation exists
3. Video application — not started, depends on #2
4. Music application — not started, depends on #2 for unsupported codecs
5. Optional-runtime orchestrator (shared lifecycle system for
   Code/Office/Browser/Media) — not started
6. VS Code-compatible runtime — not started, depends on #5
7. LibreOffice/Collabora runtime — not started, depends on #5
8. Brave remote-browser runtime — not started, depends on #5
9. Real multi-distro CI/testing — not started; current
   `tests/distro/installer-layout.sh` explicitly skips package
   installation and service-manager testing (see
   `RELEASE_EVIDENCE_AUDIT.md` Part 1)
10. Acceptance-suite expansion for all of the above — not started beyond
    the SSH/SFTP/WebDAV/S3 coverage already rebuilt in
    `tests/acceptance/src/main.rs`

Do not create `v1.0.1-rc.1` until all of the above are done, per the
task's own final gate.
