# Proposed v1.0.1-rc.1 Tag (NOT CREATED — preview only)

Per Phase 17B's explicit instruction, no tag is created in this pass. This
document records the exact proposed tag metadata for a future, separately
authorized `git tag` command.

```
Proposed command (DO NOT RUN without explicit authorization):
  git tag -a v1.0.1-rc.1 -m "<message below>" 89bfe46

Tag name:    v1.0.1-rc.1
Target:      89bfe4690ff5b4b178cb68a1a40806a13fa04f99
Type:        annotated
Signed:      no (no signing key exists yet -- see docs/RELEASE_SIGNING_DECISION.md)
```

## Proposed annotated tag message

```
CloudDesk-OS v1.0.1-rc.1 (release candidate)

Patch release candidate on top of v1.0.0, prepared on
engineering/v1-true-closure. v1.0.0 itself is unchanged and remains the
last tagged release; this candidate is not yet published.

Highlights:
- 4 real defects fixed from an independent adversarial audit of v1.0.0
  (CLAUDE-NIGHTMARE-001..004), including a CRITICAL SSH host-key
  verification bypass.
- Native musl release artifact and OpenRC service-lifecycle support for
  Alpine Linux, alongside the existing glibc artifact covering
  Debian/Ubuntu/Fedora/RHEL-family/Arch (Phase 10).
- Fresh adversarial security review (Phase 16): one HIGH-severity
  WebDAV TLS certificate-verification bypass fixed, two HIGH-severity
  dependency vulnerabilities fixed, deterministic filesystem TOCTOU/
  symlink-race regression coverage added (0 escapes), audit
  tamper-evidence tests added, and a real two-origin browser CSRF
  control added.
- Fail-closed release-artifact checksum verification in the installer.

Full evidence: CLAUDE_NIGHTMARE_REPORT.md, PHASE10_DISTRO_MATRIX.md,
PHASE16_SECURITY_REVIEW.md, RELEASE_NOTES.md.

Known limitations (environment evidence gaps, not product defects):
SELinux enforcing mode, true reboot persistence, and a genuinely
subscribed RHEL 9 environment were not exercised in this project's own
test environment. No official install URL is published yet, and no
artifact-signing scheme is implemented yet -- see
docs/RELEASE_INTEGRITY.md and docs/RELEASE_SIGNING_DECISION.md.

Not signed. Not published.
```

## Safety confirmation (re-verified immediately before this document was written)

```
git rev-parse v1.0.0^{commit}  ->  9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2  (unchanged)
git rev-parse v1.0.1-rc.1      ->  fails, tag does not exist
```

This tag has **not** been created. Creating it requires separate, explicit
authorization, per this pass's own governing instructions.
