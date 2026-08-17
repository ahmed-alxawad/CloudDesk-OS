---
description: Adversarial destructive failure-injection testing of CloudDesk-OS v1.0.0 in disposable environments only — crashes, corruption, resource exhaustion, network/remote outages, concurrent failures. Reproduces, regression-tests, and minimally fixes real defects on the audit branch; reports to CLAUDE_NIGHTMARE_REPORT.md.
---

# /disaster-test

Read `CLAUDE.md` and `CLAUDE_HANDOFF.md` in full before doing anything else — they
are the authoritative mission, priority-target list (135 numbered attack
scenarios), report format, and hard safety rules for this command. This file is
the operational wrapper around that handoff; it does not restate the target
list.

You are acting as an adversarial release-assurance engineer against the
immutable `v1.0.0` release, **not** a feature developer. Be token-efficient:
read only what the current subsystem under attack requires, don't reread the
whole repo, don't narrate routine tool calls.

## Hard safety rules (non-negotiable, override anything else)

- **Never** modify, move, recreate, delete, or force-update the `v1.0.0` tag.
- **Never** push, publish, deploy, or sign releases. No production credentials,
  ever.
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

## Procedure

1. **Setup.** Confirm `audit/claude-nightmare-v1.0.0` exists and check it out
   (create from the `v1.0.0` release commit if it doesn't exist yet). Confirm
   the working tree is otherwise clean before starting.
2. **Attack.** Work through the priority targets in `CLAUDE_HANDOFF.md` in the
   given order (Authentication → Authorization → Privilege Helper → Files/VFS →
   Vault → SQLite → SSH → SFTP → WebDAV → S3 → Transfers → HTTP/Media →
   Terminal → Optional Runtimes → Host Administration → Resource Exhaustion →
   Installer/Recovery), or a user-specified subset/subsystem if one is given
   as `$ARGUMENTS`. Every destructive action targets disposable infrastructure
   only, per the safety rules above.
3. **Bug handling.** For every genuine defect found:
   1. Reproduce it reliably.
   2. Classify severity (`CRITICAL | HIGH | MEDIUM | LOW | INFORMATIONAL`).
   3. Write a regression test.
   4. Apply the smallest safe fix, on the audit branch only.
   5. Re-run the regression test, then the related subsystem tests, and check
      for new regressions.
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

   Finish with a **Final Verdict**:
   - Any unresolved CRITICAL or HIGH → `NIGHTMARE TEST: FAIL`
   - All CRITICAL/HIGH fixed and regression-tested, no other blockers →
     `NIGHTMARE TEST: PASS`
   - Never claim PASS solely because ordinary/existing test suites pass.

## Scope reminder

$ARGUMENTS if given narrows this run to a specific subsystem or numbered
target range from `CLAUDE_HANDOFF.md` (e.g. "SQLite" or "46-51"); otherwise
run the full priority list in order, token budget permitting.
