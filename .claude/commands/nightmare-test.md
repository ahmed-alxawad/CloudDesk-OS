---
description: Strongest adversarial security/reliability test of CloudDesk-OS v1.0.0 in disposable environments only — auth/session/TOTP replay, RBAC, cross-user access, privilege-grant and cloudesk-privd IPC attacks, VFS escape, Vault tampering, hostile SSH/SFTP/WebDAV/S3, transfer races, hostile WebSockets/Range requests, malicious media, runtime isolation, audit-chain corruption, backup/restore, concurrency. Reproduces, regression-tests, and minimally fixes real defects on the audit branch; verdict is exactly NIGHTMARE TEST: PASS or FAIL.
---

# /nightmare-test

Read `CLAUDE.md` and `CLAUDE_HANDOFF.md` in full before doing anything else —
they are the authoritative mission, the full numbered priority-target list
(135 scenarios across all subsystems), the report format, and the hard safety
rules for this command. This file is the operational wrapper; it does not
restate the target list.

You are the final adversarial release-assurance agent against the immutable
`v1.0.0` release, not a feature developer. This is the strongest, most
thorough pass — cover the full attack surface unless `$ARGUMENTS` narrows it.
Be token-efficient: read only what the subsystem under attack requires, don't
reread the whole repo, don't narrate routine tool calls.

## Hard safety rules (non-negotiable, override anything else)

- **Never** modify, move, recreate, delete, or force-update the `v1.0.0` tag.
- **Never** push, publish, deploy, or sign releases. No production
  credentials, ever.
- **Never** touch the owner's real home directory, real data, real SSH keys,
  real Vault keys, git repository metadata, or any other repository.
- All destructive/failure-injection work happens **only** inside disposable
  containers, VMs, temp directories, and test databases (test SSH/SFTP/WebDAV/
  MinIO servers, temporary CloudDesk installations). If a scenario can't be
  isolated that way, skip it and note why in the report rather than risk real
  state.
- All work — fixes, regression tests, scratch scripts — lives only on branch
  `audit/claude-nightmare-v1.0.0`. Before doing anything destructive, confirm
  (or create) that branch and confirm you are on it, not on a release tag or
  `main`/`master`.

## Attack surface (this run)

Drawing from the security invariants and priority targets in
`CLAUDE_HANDOFF.md`, attack in roughly this order — full detail and numbered
scenarios live in that file, do not re-read them here if already loaded this
session:

1. **Authentication & sessions** — concurrent logins, brute force / rate
   limiting, session fixation, stolen/replayed session tokens, revoked- and
   expired-session reuse, TOTP replay, recovery-code reuse, step-up replay,
   clock manipulation / TOTP window abuse.
2. **Authorization / RBAC** — cross-user data read/write, guest reaching
   Manager/Admin endpoints, stale permissions after role change, direct API
   calls bypassing hidden UI controls.
3. **Privilege helper (cloudesk-privd)** — malformed IPC, forged/expired/
   replayed grants, grant mutation mid-flight, arbitrary UID/GID requests,
   command injection in service names, env-var injection, Unix socket
   permission attacks. `clouddeskd` must never run as UID 0; no arbitrary
   root-command API may exist.
4. **VFS / files** — path traversal (`../`, percent-encoded), symlink escape,
   TOCTOU symlink swap, hardlink surprises, malicious filenames, permission
   races, ACL bypass, Zip Slip, huge directories / inode exhaustion.
5. **Vault** — ciphertext/nonce/wrapped-DEK corruption, AAD owner-field
   mutation, record-ID mutation for cross-record decryption, wrong-KEK
   application, interrupted KEK rotation, simultaneous secret rotations,
   delete + recovery. Secrets and SSH keys must never be recoverable
   plaintext; tampering must fail closed.
6. **SSH / SFTP / WebDAV / S3** — hostile host keys (must reject silently on
   mismatch), broken ProxyJump, bastion death mid-session, malformed
   keyboard-interactive prompts, connection loss mid-transfer, malformed
   PROPFIND, hostile XML (billion laughs / XXE), failed multipart uploads,
   wrong credentials mid-job, checksum mismatches, concurrent overwrites.
7. **Transfers** — kill `clouddeskd` / restart worker / full server restart
   mid-transfer, browser/WebSocket drop, source/destination disappearance,
   disk full, concurrent jobs to same destination, cancel/pause races, retry
   storms, transfer-history corruption. Remote-to-remote data must never
   transit the browser; large transfers must use bounded memory.
8. **HTTP / media / terminal** — malformed/huge/overlapping Range requests,
   zero-byte streaming, malformed video/PDF/SVG, decompression bombs, huge
   image dimensions, WebSocket origin attacks, invalid PTY resize, binary
   garbage in PTY stream, zombie terminal processes, UID escape attempts.
9. **Optional runtimes** (Brave/Code/Office/FFmpeg) — crash cleanup, disabled
   mid-session, cross-user data leakage, host filesystem escape.
10. **SQLite / audit chain** — locked/concurrent-writer DB, corrupt DB file,
    disk full mid-transaction, interrupted schema migration, SIGKILL during
    sensitive update, audit-log flood, audit-chain corruption/tamper
    detection.
11. **Backup / restore / installer** — installer killed halfway, rerun on
    existing install, upgrade interrupted mid-migration, missing/wrong master
    key, backup with missing key, restore permissions altered.
12. **Resource exhaustion & concurrency** — OOM pressure, disk full, FD/
    process exhaustion, connection floods, Slowloris-style slow clients,
    transfer-queue floods, and concurrent-failure combinations across any of
    the above (e.g. concurrent writers + kill mid-transaction).

## Procedure

1. **Setup.** Confirm `audit/claude-nightmare-v1.0.0` exists and check it out
   (create from the `v1.0.0` release commit if it doesn't exist yet). Confirm
   the working tree is otherwise clean before starting.
2. **Attack.** Work through the attack surface above (or a user-specified
   subset via `$ARGUMENTS`) against disposable infrastructure only — temp
   dirs, test databases, test SSH/MinIO/WebDAV servers, temporary CloudDesk
   installations, containers/VMs. Cross-reference `CLAUDE_HANDOFF.md`'s
   numbered scenarios (1–135) for concrete reproduction steps per subsystem.
3. **Bug handling.** For every genuine defect found:
   1. Reproduce it reliably.
   2. Classify severity (`CRITICAL | HIGH | MEDIUM | LOW | INFORMATIONAL`).
   3. Write a regression test.
   4. Apply the smallest safe fix, on the audit branch only.
   5. Re-run the regression test, then related subsystem tests, and check for
      new regressions.
   6. Document it (see report format below).
   Do not fix a bug you cannot reproduce unless it is statically undeniable.
4. **Report.** Write/update `CLAUDE_NIGHTMARE_REPORT.md` at the repo root.
   Every finding uses exactly this block:

   ```
   ID:
   Severity:           CRITICAL | HIGH | MEDIUM | LOW | INFORMATIONAL
   Subsystem:
   Release affected:   v1.0.0
   Reproduction:
   Expected:
   Actual:
   Security impact:
   Data-loss impact:
   Availability impact:
   Root cause:
   Fix:
   Regression test:
   Retest:
   ```

   Finish with a line containing exactly one of:
   - `NIGHTMARE TEST: FAIL` — if any CRITICAL or HIGH finding is unresolved.
   - `NIGHTMARE TEST: PASS` — only if every CRITICAL/HIGH finding has been
     fixed and regression-tested, with no other blockers outstanding.
   Never claim PASS solely because ordinary/existing test suites pass.

## Scope reminder

$ARGUMENTS if given narrows this run to a specific subsystem or numbered
target range from `CLAUDE_HANDOFF.md` (e.g. "Vault" or "37-45"); otherwise run
the full attack surface above in order, token budget permitting.
