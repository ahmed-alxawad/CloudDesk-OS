# CloudDesk-OS — Engineering Checkpoint

Branch: `engineering/v1-true-closure` (from `audit/claude-nightmare-v1.0.0`)
`v1.0.0` tag: untouched, unpublished. Nothing pushed.

## Last completed phase

**Phase 3 — FFmpeg Media Foundation.** Complete per this phase's own
definition of done (see "Phase 3 — what was built" below). Phase 2 (SSH
feature matrix) is still explicitly incomplete — proceeding to Phase 3
was a deliberate, instructed choice (the project owner's prompt named
Phase 3 directly), not a decision to abandon Phase 2's remaining items.
Phase 2's status is preserved untouched below.

## Phase 3 — what was built (FFmpeg Media Foundation)

New crate `clouddesk-media` + HTTP wiring in `services/clouddeskd`. Full
detail in the commit message (`624f6be`) and `V1_TRUE_CLOSURE.md` item
#1. Summary:

```
[x] FFmpeg/ffprobe discovery (typed, no shell, explicit unavailable state)
[x] ffprobe structured metadata (bounded, typed error on malformed input)
[x] DIRECT/REMUX/TRANSCODE/UNSUPPORTED decision (container+codec based)
[x] direct ranged stream (reused serve_file_stream; fixed a real 416/
    multi-range bug found while wiring this)
[x] real remux -- live-tested against real ffmpeg 8.1.2
[x] real transcode -- live-tested, output reprobed as DIRECT-playable
[x] cancellation -- live-tested (SIGTERM, 3s grace, SIGKILL fallback)
[x] timeout -- implemented (10 min const), NOT live-fired (impractical
    to wait 10 minutes in a test); only cancellation was live-exercised
[x] concurrency limits -- implemented (global + per-user semaphore),
    NOT exercised under real concurrent load
[x] temporary storage limits -- 4 GiB output-size guard implemented,
    NOT live-tripped
[x] cleanup -- per-job 0700 workspace removed on failure/cancel; startup
    reconciliation of jobs orphaned by a crash/restart
[x] cross-user authorization -- live HTTP-tested (404, not 403 -- a
    cross-user job ID doesn't even confirm existence)
[~] hostile media handling -- live-tested for empty/random-bytes/missing
    file; NOT tested: truncated MP4, corrupt MKV specifically, huge
    declared duration, broken subtitle track (huge/overflow dimensions
    ARE covered, but only at the parser-unit level, not via ffprobe)
[x] optional enable/disable -- live HTTP-tested (disabled -> 503 on
    every media endpoint; direct byte-range streaming is unaffected
    since it never touches FFmpeg)
[x] real FFmpeg live acceptance -- crates/media/tests/live_ffmpeg.rs
    (4 tests) + services/clouddeskd/tests/media_api.rs (5 tests),
    all against actually-installed ffmpeg/ffprobe 8.1.2, zero mocks
[x] Rust release gates -- fmt/clippy(-D warnings)/test --workspace/
    build --release all pass
```

**What is explicitly NOT enforced** (per Task 8's "record exactly what
is and isn't enforced, do not fake resource isolation"): no cgroup CPU
or memory limit exists anywhere in this implementation. The
`JobLimiter` in `crates/media/src/exec.rs` is process-level admission
control (a `tokio::sync::Semaphore`), not a kernel resource limit -- a
job that gets a permit can still use as much CPU/RAM as the box has
while it runs. This was a conscious, documented choice given the
project's existing privilege/runtime architecture has no cgroup
integration to hook into yet, not an oversight.

**Audit coverage is partial**: only `media.job.requested` is audited
today (with operation type, never a raw path or FFmpeg command line).
Task 18 also asks for `started`/`completed`/`failed`/`cancelled` audit
events -- not added this session.

**No frontend changes** -- Phase 3 was explicitly scoped to the backend
foundation only ("Do NOT build the full Video UI in this phase"), so
`npm run lint/check/test/build` were not re-run (nothing in `apps/web`
changed; Phase 1's last real run is still valid).

## Current phase

**Phase 2 — Complete SSH Feature Matrix.** Partial. Do not treat this as
done — see "Phase 2 status" below for exactly what is and isn't real.
(Unchanged this session -- Phase 3 was worked in parallel per explicit
instruction, not as a replacement for finishing Phase 2.)

## Phase 2 status

```
[x] ProxyJump product wiring          -- DONE, live-tested (12 tests,
                                          real 2-container bastion+target)
[x] independent bastion host verification -- DONE (part of the above)
[x] independent target host verification  -- DONE (part of the above)
[ ] SSH agent                          -- NOT STARTED
[ ] keyboard-interactive               -- NOT STARTED
[ ] SSH certificates                   -- NOT STARTED
[ ] native SCP                         -- NOT STARTED
[x] SFTP over ProxyJump                -- DONE, live-tested (1 test,
                                          list/upload/download/rename/delete)
[ ] remote terminal over ProxyJump     -- NOT STARTED (blocked on a
                                          prerequisite that doesn't exist
                                          yet -- see below)
[~] authorization isolation            -- no NEW HTTP endpoints were added
                                          this session (resolve_ssh_session
                                          is only called from the existing,
                                          already-authorized transfer path),
                                          so there was nothing new to sweep;
                                          not independently re-verified
[~] audit redaction                    -- not reviewed this session; the
                                          new code path doesn't add any new
                                          audit events (it replaced inline
                                          logic that had none either) --
                                          this was true before and after,
                                          not verified either way
[x] live disposable OpenSSH fixtures   -- used throughout, including a
                                          real fixture bug found and fixed
                                          (see below)
[x] Rust release gates                 -- fmt/clippy/test --workspace all
                                          pass
```

**Do not call Phase 2 complete.** Four of five mandatory Task-1-through-5
targets (agent, keyboard-interactive, certificates, SCP) have zero
implementation — not a stub, not an enum, genuinely nothing beyond what
was already there before this session (`SshAuth::Agent` and
`SshAuth::KeyboardInteractive` still `bail!`; `SshAuth::Certificate` still
silently ignores `cert_data`; no SCP code exists at all).

## What was actually built and verified this session

**ProxyJump product wiring** (`services/clouddeskd/src/worker.rs::
resolve_ssh_session`), consumed by the SFTP/transfer connection path:
- Resolves a target `RemoteServer`; if `proxy_jump_server_id` is set,
  independently resolves the bastion too (separate `RemoteServerStore::get`
  ownership check, separate pinned host key, separate Vault credential
  reveal — never reusing the target's credential for the bastion) and
  connects via `SshSession::connect_proxyjump` instead of a direct
  connection.
- Chain depth bounded to target + one bastion hop
  (`MAX_PROXY_CHAIN_HOPS = 2`); a bastion whose own
  `proxy_jump_server_id` is set is refused (`ChainTooDeep`), which also
  rejects every A→B→A loop as a side effect. Self-reference explicitly
  rejected. Cross-owner bastion reference rejected independently of
  `RemoteServerStore::create`'s own check (proven by forcing one directly
  into the database — `create()` itself already makes this
  unconstructable through the normal API, so this is defense in depth,
  not the only guard).
- **Real bug found and fixed in the test fixture itself**, not just
  product code: `linuxserver/openssh-server` ships with
  `AllowTcpForwarding no`, silently breaking ProxyJump's `direct-tcpip`
  channel. Fixed via the image's own documented `sshd_config.d` drop-in
  mechanism (`tests/acceptance/fixtures/sshd_config.d/proxyjump.conf`,
  bind-mounted in `docker-compose.yml`) — reproducible on a fresh
  `docker compose down -v && up -d`, verified by actually doing that and
  rerunning the suite clean, not a one-off manual patch to a running
  container that would be lost on the next teardown.

**Test evidence** (`services/clouddeskd/tests/ssh_proxyjump.rs`, 12
tests) against a real two-container topology
(`tests/acceptance/docker-compose.yml`): `openssh` (bastion, host port
2222) and `openssh-target` (target, **deliberately no host port
mapping** — reachable only through the bastion's compose-internal
network, so a passing test proves the connection genuinely went
client→bastion→target). Covers: valid connection + command execution,
wrong bastion/target host key rejected, bastion/target auth failure
rejected, topology sanity check, self-reference, A→B→A loop,
cross-owner bastion reference, bastion-deletion-nulls-reference
(`ON DELETE SET NULL`), missing target, and SFTP
list/upload/download/rename/delete over the ProxyJump path with target
host-key pinning still enforced (Task 7).

**Not covered even for what was built**: live bastion-dies-mid-session,
connection-storm, and auth-timeout scenarios from the original task's
regression list were not tested this session.

## New closure item discovered

**`V1_TRUE_CLOSURE.md` #16 (new): Remote terminal over SSH does not
exist.** `SshSession` only has `run_command` (single buffered
non-interactive exec — no PTY, no interactive channel). No endpoint in
`services/clouddeskd` opens a remote-server terminal session. The
existing local terminal (`/api/v1/terminal/ws`) is a completely separate
feature (mapped-UID local PTY, nothing to do with SSH). This is a bigger
gap than "ProxyJump isn't wired into remote terminal" — the remote
terminal feature itself was never built. Task 8 in the original Phase 2
prompt assumed this existed; it doesn't.

## Verified items (all phases so far)

- **Resumable uploads** — persisted session table, chunked HTTP surface,
  cross-user isolation, checksum verification, atomic finalize, janitor.
- **Archive create/extract** — ZIP + tar.gz, Zip Slip/Tar Slip/symlink/
  quota defenses, 10 tests.
- **ACL read/edit** — real `getfacl`/`setfacl`, in-process path
  resolution (bug found and fixed: `/proc/self/fd` in a spawned child
  doesn't refer to the parent's fd table), dedicated capability, 6 tests.
- **ProxyJump + SFTP-over-ProxyJump** — see above, 12 tests against a
  real bastion+target topology.

## Validation

```
cargo fmt --all -- --check                                          PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings PASS
cargo test --workspace                                              PASS (0 failures)
```
No frontend changes this session — frontend gates not re-run (Phase 1's
were the last real run, still valid since nothing in `apps/web` changed).

`services/clouddeskd/tests/ssh_proxyjump.rs` (12 tests) run separately
against live Docker fixtures, as instructed:
```
cd tests/acceptance && docker compose up -d
cargo test -p clouddeskd --test ssh_proxyjump   # 12 passed, 0 failed
docker compose down -v   # torn down cleanly after
```
Re-verified on a from-scratch `docker compose down -v && up -d` (not
just the already-running, manually-patched containers) to confirm the
`sshd_config.d` fixture fix is actually reproducible.

## Current commit

```
624f6be feat(media): FFmpeg-backed media compatibility foundation (Phase 3)
```
on top of (preserved, untouched, still passing):
```
0d7a2da docs(engineering): checkpoint after Phase 2 partial (ProxyJump wiring)
ce48b74 feat(ssh): wire ProxyJump through the SFTP/transfer connection path
c86da38 docs(engineering): checkpoint after Phase 1 (File Manager) closure
9b7aa74 feat(files): complete phase 1 file manager closure — archives and ACLs
d277393 docs(engineering): checkpoint after resumable-upload closure
b4a4660 feat(files): implement resumable local-file uploads (GOAL.md G3)
dfdfade audit(evidence): repair fabricated acceptance runner, audit spec vs implementation, fix RSA SSH auth
289904b audit(nightmare): fix SSH host-key bypass and SFTP upload/list breakage; prep v1.0.1-rc.1
d6517bf audit(nightmare): require system.services.manage for /api/v1/system/summary
ffbc336 test: prepare Claude v1.0.0 nightmare audit
9b8f49a release: CloudDesk-OS v1.0.0   <- immutable tag v1.0.0 points here
```

All five prior Nightmare fixes, Phase 1, and Phase 2's ProxyJump work
preserved and untouched this session (full `cargo test --workspace`
still green, including all 12 `ssh_proxyjump.rs` tests, all archive/ACL
tests, and both Nightmare regression tests).

## Actual live authentication methods verified (through the real product path)

```
Password              -- yes (this session's ProxyJump tests, plus
                          prior Nightmare-audit live tests)
PEM/private key        -- yes (prior session, crates/remote/tests/ssh.rs
                          test_ssh_rsa_pem_private_key_auth_succeeds)
RSA                    -- yes, fixed this-audit-lineage
                          (CLAUDE-NIGHTMARE-005)
Ed25519                -- yes (prior session)
Encrypted key+passphrase -- yes (prior session)
SSH Agent               -- NO -- SshAuth::Agent still bail!s
Keyboard-interactive     -- NO -- SshAuth::KeyboardInteractive still bail!s
SSH certificate          -- NO -- SshAuth::Certificate still ignores cert_data
Custom port              -- yes (fixture runs SSH on 2222, tested throughout)
ProxyJump                -- yes, THIS session
```

## ProxyJump verified: YES (for SFTP/transfers; NOT for a remote terminal, which doesn't exist)

## SCP verified: NO — not implemented at all

## Security findings (Phase 3)

One real, pre-existing product bug found and fixed while wiring direct
playback (not introduced this session, just uncovered by it):
`serve_file_stream`'s `Range` handling fell through to a full 200 body
for out-of-bounds/reversed ranges instead of 416, and misparsed
multi-range `Range` headers instead of ignoring them. Both fixed to be
RFC 7233 compliant, both covered by regression tests in
`services/clouddeskd/tests/media_api.rs`. No defect found in the new
media code itself during this pass.

## Next phase

**Phase 4 — Video Application**, per the task's own prerequisite check:
build the Video app over this session's verified Phase 3 media service,
not a duplicate FFmpeg pipeline. Phase 2's remaining SSH items (agent,
keyboard-interactive, certificates, SCP, remote terminal) are still open
and were deliberately not touched this session — see the unchanged
"Phase 2 status" section above for their state and dependency order
when a future session picks them back up.

## Next exact action

Build the Video Svelte component (`apps/web/src/lib`) wired to
`GET/POST /api/v1/media/probe`, `/media/jobs`, `/media/jobs/{id}`,
`/media/jobs/{id}/output`, and the existing `/api/v1/media/stream` for
the DIRECT case. Per Phase 4's own spec: playback session abstraction
(user/source/mode/resume position, owner-scoped like the job store),
real seek/controls/speed via the browser's native `<video>` element for
DIRECT, a "preparing" state for REMUX/TRANSCODE that polls job status
and switches to `/media/jobs/{id}/output` on completion, subtitle/
audio-track listing from the existing probe response, resume-position
persistence per user+media identity (not filename alone), and hostile-
Range/hostile-media UI error states. Needs `npm run lint/check/test/
build` this time, since this phase touches `apps/web`.

## Remaining closure blockers

Everything in `V1_TRUE_CLOSURE.md` except items 1 (FFmpeg pipeline, this
session), 7, 8, 9 (Phase 1) and the ProxyJump/SFTP-over-ProxyJump
portion of item 14 (Phase 2, partial). In priority/dependency order:

1. Video application — not started, now unblocked (Phase 3 closed)
2. Music application — not started, now unblocked (Phase 3 closed)
3. SSH agent, keyboard-interactive, SSH certificates, native SCP — not
   started (rest of Phase 2)
4. Remote terminal over SSH — not started (item #16)
5. Optional-runtime orchestrator (Code/Office/Browser/Media) — not started
6. VS Code-compatible runtime — not started, depends on #5
7. LibreOffice/Collabora runtime — not started, depends on #5
8. Brave remote-browser runtime — not started, depends on #5
9. Real multi-distro CI/testing — not started; `tests/distro/
   installer-layout.sh` explicitly skips package/service-manager testing
10. Acceptance-suite expansion for all of the above

Do not create `v1.0.1-rc.1` until all of the above are done, per the
task's own final gate.
