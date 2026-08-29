# Proposed v1.0.1-rc.2 Tag (NOT CREATED — preview only)

`v1.0.1-rc.1` was previously proposed and tagged locally (`89bfe46`), but its
tagged source commit lacked a root `LICENSE` file. `v1.0.1-rc.1` is not
moved, deleted, or reused — it remains a frozen, local-only, superseded
candidate. This document now proposes `v1.0.1-rc.2` instead. No tag is
created in this pass. This document records the exact proposed tag metadata
for a future, separately authorized `git tag` command.

```
Proposed command (DO NOT RUN without explicit authorization):
  git tag -a v1.0.1-rc.2 -m "<message below>" 6b1eaa81b7ec36980e5f01edbaeca3e7b1fd8fa0

Tag name:    v1.0.1-rc.2
Target:      6b1eaa81b7ec36980e5f01edbaeca3e7b1fd8fa0
Type:        annotated
Signed:      no (no signing key exists yet -- see docs/RELEASE_SIGNING_DECISION.md)
```

## Proposed annotated tag message

```
CloudDesk-OS v1.0.1-rc.2 (release candidate)

Patch release candidate on top of v1.0.0, prepared on
engineering/v1-true-closure. v1.0.0 itself is unchanged and remains the
last tagged release; this candidate is not yet published.

Supersedes v1.0.1-rc.1 (frozen locally at 89bfe46, never published):
rc.1's tagged source commit lacked a root LICENSE file, which established
project licensing policy (AGPL-3.0-or-later, see Cargo.toml) requires
before publication. rc.1 is left unchanged and unpublished; rc.2 adds the
fix on top of everything rc.1 already had.

What rc.2 adds on top of rc.1:
- Root LICENSE (canonical, unmodified GNU AGPLv3 text).
- Third-party redistribution clarification: explicit "Distribution model"
  classification for code-server, Collabora, and Brave; explicit SBOM
  scope statement.
- Fail-closed release-staging validator
  (tests/distro/release-staging-validation.sh), proven to correctly
  reject rc.1's own source commit for the missing LICENSE.
- Precise artifact-integrity trust-chain documentation, future
  publication endpoint placeholders, and local publication dry-run
  evidence (7/7 artifacts fetched and checksum-verified over real
  localhost HTTP).

Highlights carried forward from rc.1 (no application/product code
changed since rc.1 -- only licensing, documentation, and release-tooling
content):
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
PHASE16_SECURITY_REVIEW.md, PHASE17_RELEASE_PUBLICATION_CLOSURE.md,
RELEASE_NOTES.md.

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
git rev-parse v1.0.0^{commit}      ->  9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2  (unchanged)
git rev-parse v1.0.1-rc.1^{commit} ->  89bfe4690ff5b4b178cb68a1a40806a13fa04f99  (unchanged)
git rev-parse v1.0.1-rc.2          ->  fails, tag does not exist
```

Neither `v1.0.1-rc.2` tag creation nor `v1.0.1-rc.1` modification has
happened. Creating the `v1.0.1-rc.2` tag requires separate, explicit
authorization, per this pass's own governing instructions.
