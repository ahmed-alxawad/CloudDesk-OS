# CloudDesk-OS — Engineering Checkpoint

Branch: `engineering/v1-true-closure` (from `audit/claude-nightmare-v1.0.0`)
`v1.0.0` tag: untouched, unpublished. Nothing pushed.

## Last completed phase

**Phase 1 — File Manager Closure.** Complete (resumable uploads, archive
create/extract, ACL read/edit — see prior checkpoint entry, preserved
below).

## Current phase

**Phase 2 — Complete SSH Feature Matrix.** Partial. Do not treat this as
done — see "Phase 2 status" below for exactly what is and isn't real.

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
ce48b74 feat(ssh): wire ProxyJump through the SFTP/transfer connection path
```
on top of (preserved, untouched, still passing):
```
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

All five prior Nightmare fixes preserved and untouched this session.

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

## Security findings

None new this session beyond the fixture-config bug (not a CloudDesk
product defect — a disposable test fixture default that would have made
every ProxyJump live test silently fail to prove anything if left
unfixed). No CloudDesk product security defect found in the ProxyJump
implementation itself during this pass.

## Next phase

**Phase 3 — FFmpeg Media Foundation** (per the task's own instruction —
NOT continuing Phase 2's remaining SSH tasks in this session).

## Next exact action

Per the checkpoint discipline instruction ("do not rush... update the
checkpoint and stop cleanly" applies here too): the next session should
make its own judgment call between finishing Phase 2 (agent,
keyboard-interactive, certificates, SCP — each a substantial standalone
feature, see the detailed breakdown below) versus proceeding to Phase 3
as this prompt's own template suggests. If continuing Phase 2, in
dependency order:

1. **SSH agent** — check whether the currently-pinned `russh`/
   `russh-keys` version has agent-client support (`russh::keys::agent`
   was not investigated this session). Real Unix-socket agent protocol,
   never exporting key material out of the agent. Live-test against a
   real disposable `ssh-agent` + the existing OpenSSH fixture.
2. **Keyboard-interactive** — real challenge/response exchange
   (currently `bail!`s with a comment claiming russh 0.62 doesn't support
   it; re-verify that claim against the actually-pinned version before
   assuming it's still accurate). `linuxserver/openssh-server` would need
   PAM/keyboard-interactive configured, which the current fixture doesn't
   have — likely needs a third disposable container or a config change to
   an existing one.
3. **SSH certificates** — real OpenSSH certificate parsing/validation via
   `russh`'s certificate support. Needs a disposable CA fixture (generate
   a CA keypair, sign a user key with `ssh-keygen -s`, configure a
   disposable sshd with `TrustedUserCAKeys`). Be careful not to conflate
   user-certificate work with host-key verification (already correct,
   from the Nightmare-audit fixes) — don't touch that code path.
4. **Native SCP** — a real transport, not an SFTP alias. Needs its own
   protocol implementation (or a maintained crate) integrated into the
   existing transfer architecture (`clouddesk_transfers`), with careful
   handling of remote-provided filenames (no shell interpolation, ever —
   reject/escape shell metacharacters, `../`, absolute paths, matching
   the discipline already used for archive/ACL work in Phase 1).
5. Remote terminal over SSH (new, `V1_TRUE_CLOSURE.md` #16) — PTY
   allocation on `SshSession`, a new streaming endpoint, frontend wiring.
   Only relevant to Phase 2's original Task 8 once this exists.

For each: reproduce/implement, add regression tests (deterministic mock
where practical, real disposable fixture where it matters — e.g.
keyboard-interactive and certificates genuinely need real sshd behavior,
agent needs a real agent socket), hostile/failure cases, gates, update
`V1_TRUE_CLOSURE.md`.

## Remaining closure blockers

Everything in `V1_TRUE_CLOSURE.md` except items 7, 8, 9 (Phase 1, closed)
and the ProxyJump/SFTP-over-ProxyJump portion of item 14 (this session).
In priority/dependency order:

1. SSH agent, keyboard-interactive, SSH certificates, native SCP — not
   started (rest of Phase 2)
2. Remote terminal over SSH — not started (new item #16, discovered this
   session)
3. FFmpeg compatibility pipeline — not started, zero implementation
4. Video application — not started, depends on #3
5. Music application — not started, depends on #3 for unsupported codecs
6. Optional-runtime orchestrator (Code/Office/Browser/Media) — not started
7. VS Code-compatible runtime — not started, depends on #6
8. LibreOffice/Collabora runtime — not started, depends on #6
9. Brave remote-browser runtime — not started, depends on #6
10. Real multi-distro CI/testing — not started; `tests/distro/
    installer-layout.sh` explicitly skips package/service-manager testing
11. Acceptance-suite expansion for all of the above

Do not create `v1.0.1-rc.1` until all of the above are done, per the
task's own final gate.
