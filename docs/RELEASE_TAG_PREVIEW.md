# Proposed v1.0.1-rc.3 Tag (NOT CREATED — preview only)

`v1.0.1-rc.1` (`89bfe46`) and `v1.0.1-rc.2` (`6b1eaa8`) were previously
tagged locally, both never pushed or published, both now frozen and
superseded — rc.1 for a missing root `LICENSE`, rc.2 because it predated
the corrected GitHub repository identity and had no direct-fetch installer
implementation. Neither is moved, deleted, or reused. This document now
proposes `v1.0.1-rc.3`. No tag is created in this pass. This document
records the exact proposed tag metadata for a future, separately
authorized `git tag` command.

```
Proposed command (DO NOT RUN without explicit authorization):
  git tag -a v1.0.1-rc.3 -m "<message below>" 43b31a9d54b68f851eadb7c54e9c50135c5fa5d5

Tag name:    v1.0.1-rc.3
Target:      43b31a9d54b68f851eadb7c54e9c50135c5fa5d5
Type:        annotated
Signed:      no (no signing key exists yet -- see docs/RELEASE_SIGNING_DECISION.md)
```

## Proposed annotated tag message

```
CloudDesk-OS v1.0.1-rc.3 (release candidate)

Patch release candidate on top of v1.0.0, prepared on
engineering/v1-true-closure. v1.0.0 itself is unchanged and remains the
last tagged release; this candidate is not yet published.

Supersedes v1.0.1-rc.2 (frozen locally at 6b1eaa8, never published) and
v1.0.1-rc.1 (frozen locally at 89bfe46, never published). Neither prior
candidate is altered.

What rc.3 adds on top of rc.2:
- Corrected GitHub repository identity: ahmed-alxawad/CloudDesk-OS
  (Cargo.toml's repository field had carried an unverified placeholder
  value since before v1.0.0).
- Direct-fetch installer (installer/install.sh): when CLOUDESK_VERSION
  is set, fetches its own native binaries and web bundle from GitHub
  Releases, verifying version consistency and SHA256 checksums before
  installing anything -- closing GOAL.md G1's `curl -fsSL <url> | sudo
  bash` gap. Local/offline behavior is unchanged.
- Web frontend as a real, checksummed, attested, reproducible release
  artifact (packaging/build-web.sh, clouddesk-web.tar.gz) -- previously
  never built or shipped as part of any release at all.
- Completed release-attestation workflow: generates manifest.json
  (previously never produced by CI), stages the flat GitHub-Releases
  asset layout the installer expects, and attests it; both third-party
  GitHub Actions pinned to exact commit SHAs.
- 11 real end-to-end installer controls (tests/distro/remote-fetch.sh)
  against a local HTTP fixture: valid installs, HTTPS enforcement,
  version/manifest/checksum tampering, artifact-swap, and missing
  artifacts all behave correctly (fail closed except the valid case).

Highlights carried forward from rc.1/rc.2 (no application/product code
changed since rc.1 -- only licensing, documentation, and release/
installer infrastructure):
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
- Root LICENSE (AGPL-3.0-or-later, canonical unmodified text) and a
  fail-closed release-staging validator.

Full evidence: CLAUDE_NIGHTMARE_REPORT.md, PHASE10_DISTRO_MATRIX.md,
PHASE16_SECURITY_REVIEW.md, PHASE17_RELEASE_PUBLICATION_CLOSURE.md,
RELEASE_NOTES.md.

Known limitations (environment evidence gaps, not product defects):
SELinux enforcing mode, true reboot persistence, and a genuinely
subscribed RHEL 9 environment were not exercised in this project's own
test environment. No release has actually been published to GitHub
Releases yet, and no artifact-signing scheme is implemented yet -- see
docs/RELEASE_INTEGRITY.md and docs/RELEASE_SIGNING_DECISION.md.

Not signed. Not published.
```

## Safety confirmation (re-verified immediately before this document was written)

```
git rev-parse v1.0.0^{commit}      ->  9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2  (unchanged)
git rev-parse v1.0.1-rc.1^{commit} ->  89bfe4690ff5b4b178cb68a1a40806a13fa04f99  (unchanged)
git rev-parse v1.0.1-rc.2^{commit} ->  6b1eaa81b7ec36980e5f01edbaeca3e7b1fd8fa0  (unchanged)
git rev-parse v1.0.1-rc.3          ->  fails, tag does not exist
```

No tag creation and no existing-tag modification has happened. Creating the
`v1.0.1-rc.3` tag requires separate, explicit authorization, per this
pass's own governing instructions.
