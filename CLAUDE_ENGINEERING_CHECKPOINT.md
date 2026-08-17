# CloudDesk-OS — Engineering Checkpoint

Branch: `engineering/v1-true-closure` (from `audit/claude-nightmare-v1.0.0`)
`v1.0.0` tag: untouched, unpublished. Nothing pushed.

## Last fully completed phase

**Phase 1, item 1 of 4 (Resumable uploads)** — real, tested, committed
(`b4a4660 feat(files): implement resumable local-file uploads`).

## Current phase

**Phase 1 — File Manager closure.** Two of four items remain:
- ACL read/edit — **not started**
- Archive create/extract — **not started**

Phases 2–11 (SSH matrix/ProxyJump/SCP, FFmpeg pipeline, Video, Music,
runtime orchestrator, VS Code, LibreOffice/Collabora, Brave, distro CI,
acceptance-suite expansion) — **not started**. Be honest with yourself
about this if you're the next session picking this up: this is the
overwhelming majority of the remaining work. Do not let the one completed
item create an impression of broader progress than exists.

## Current file/function

N/A — clean commit boundary, no work in progress.

## Tests passing

Full workspace, confirmed after a clean rebuild:
```
cargo fmt --all -- --check                                          PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings PASS
cargo test --workspace                                              PASS (0 failures)
```
Frontend gates not run this session (no `apps/web` changes).

## Tests failing

None currently. **Gotcha for the next session:** `cargo test --workspace`
transiently failed with `no such table: upload_sessions` after adding
migration `0009_upload_sessions.sql` — `sqlx::migrate!`'s compile-time
embed did not pick up the new file under a stale incremental-build
artifact. `cargo clean -p clouddesk-db -p clouddeskd` (or a full `cargo
clean`) before the next `cargo test --workspace` after adding a migration
resolved it. If you add another migration and see a "no such table"
failure that doesn't reproduce standalone (`cargo test -p clouddeskd`
passes, `--workspace` fails), this is almost certainly the same issue —
clean before concluding it's a real bug.

## Live fixtures tested

None this session (Phase 1 work is local-filesystem only, no
SSH/SFTP/WebDAV/S3 fixtures needed). The Docker fixtures
(`tests/acceptance/docker-compose.yml`) used in prior sessions are not
currently running.

## Uncommitted files

```
 M CLAUDE.md   (pre-existing modification from before this session, not
                mine to resolve — leave as-is unless asked)
?? .claude/settings.json   (pre-existing, environment-managed, not mine)
?? SHA256SUMS              (pre-existing, not mine)
```
Nothing from this session's work is uncommitted.

## Current commit

```
b4a4660 feat(files): implement resumable local-file uploads (GOAL.md G3)
```
on top of (preserved, untouched, still passing):
```
dfdfade audit(evidence): repair fabricated acceptance runner, audit spec vs implementation, fix RSA SSH auth
289904b audit(nightmare): fix SSH host-key bypass and SFTP upload/list breakage; prep v1.0.1-rc.1
d6517bf audit(nightmare): require system.services.manage for /api/v1/system/summary
ffbc336 test: prepare Claude v1.0.0 nightmare audit
9b8f49a release: CloudDesk-OS v1.0.0   <- immutable tag v1.0.0 points here
```

All five prior Nightmare fixes are preserved and were not touched this
session:
- `system_summary` requires `system.services.manage`
  (`services/clouddeskd/src/lib.rs`)
- SSH host-key verification (`SshClientHandler::check_server_key`,
  `SshSession::connect_pinned`, `crates/remote/src/ssh.rs`)
- SFTP upload create-or-overwrite fix (`SftpProvider::write_file`,
  `crates/remote/src/sftp.rs`)
- SFTP listing non-chroot fix (`SftpProvider::list`, same file)
- RSA SHA-2 auth fix (`authenticate`, same file)

Regression tests for all five still pass as part of the full suite above.

## Next exact action

Implement **Archive create/extract** next (more self-contained than ACL —
no privileged-helper design needed, pure library + VFS-root-boundary
logic):

1. Add a `zip` crate dependency to `crates/vfs` (or a new
   `crates/archive` crate if it grows large — start in `crates/vfs` and
   split out only if it doesn't fit cleanly).
2. `create_archive(provider: &dyn VfsProvider, entries: &[String], dest: &str)`
   — stream entries into a zip, writing only within the VFS root; reuse
   the existing `LocalProvider`/`normalize_virtual_path` sandboxing
   already proven in `crates/vfs/src/lib.rs`.
3. `extract_archive(provider: &dyn VfsProvider, archive: &str, dest_dir: &str)`
   — **the security-critical half.** Every extracted entry path must be
   re-validated the same way `normalize_virtual_path` rejects
   `ParentDir`/`Prefix` components (Zip Slip), AND symlink entries inside
   the archive must not be extracted as symlinks pointing outside the
   root (or must be rejected outright — rejecting is simpler and safer,
   consistent with `GOAL.md`'s traversal-protection posture elsewhere).
   Add an extraction quota (max total uncompressed size, max entry count)
   to bound a zip-bomb.
4. Wire into `LocalFileOperation` (`crates/vfs/src/lib.rs`) as two new
   variants, then into `local_file_action`'s dispatch in
   `services/clouddeskd/src/lib.rs`.
5. Security regression tests: a hand-crafted Zip Slip payload
   (`../../etc/passwd`-style entry name) must fail to extract outside the
   root; a symlink entry must not escape; a decompression-bomb-shaped
   archive must hit the quota and fail cleanly, not exhaust memory/disk.
6. `cargo fmt` / `clippy -D warnings` / targeted `cargo test`.
7. Commit as its own coherent unit (`feat(files): add archive create and
   extract with Zip Slip protection`), separate from ACL.

Then **ACL** (needs more design work — read `crates/vfs`'s existing
`ProviderFeature::Acl` flag site and `crates/linux` for what Linux
identity primitives already exist before choosing a `posix-acl`-style
crate or shelling out to `setfacl`/`getfacl`; decide whether ACL edits on
files owned by the mapped Linux user need to go through
`cloudesk-privd` or can run in-process like the rest of `LocalProvider`
— they very likely can, since `chmod` already does, per
`crates/vfs/src/lib.rs`'s existing `chmod` implementation).

Then proceed to Phase 2 (SSH matrix) per the original task's phase
ordering.

## Remaining closure blockers

Everything in `V1_TRUE_CLOSURE.md` except item 9 (resumable uploads,
closed this session). In priority/dependency order per the task:

1. ACL read/edit — not started
2. Archive create/extract — not started
3. SSH agent, keyboard-interactive, SSH certificates, ProxyJump product
   wiring, SCP — not started (all library-adjacent code partially exists
   for some of these, e.g. `connect_proxyjump`, but none are wired
   through `RemoteServerStore` → API → UI as the task requires)
4. FFmpeg compatibility pipeline (probe/remux/transcode/job
   lifecycle/limits) — not started, zero implementation exists
5. Video application — not started, depends on #4
6. Music application — not started, depends on #4 for unsupported codecs
7. Optional-runtime orchestrator (shared lifecycle system for
   Code/Office/Browser/Media) — not started
8. VS Code-compatible runtime — not started, depends on #7
9. LibreOffice/Collabora runtime — not started, depends on #7
10. Brave remote-browser runtime — not started, depends on #7
11. Real multi-distro CI/testing — not started; current
    `tests/distro/installer-layout.sh` explicitly skips package
    installation and service-manager testing (see
    `RELEASE_EVIDENCE_AUDIT.md` Part 1)
12. Acceptance-suite expansion for all of the above — not started beyond
    the SSH/SFTP/WebDAV/S3 coverage already rebuilt in
    `tests/acceptance/src/main.rs`

Do not create `v1.0.1-rc.1` until all of the above are done, per the
task's own final gate. Given the scope, this checkpoint exists precisely
because that gate is realistically many further sessions away — do not
let anyone (including a future instance of yourself) mistake one merged
feature for being close to done.
