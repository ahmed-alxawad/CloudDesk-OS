# Release Artifact Integrity & Signing Design (Phase 17A)

This document establishes the intended trust model for CloudDesk-OS release
artifacts before public installer/artifact publication. It is a design
record, not a claim that publication has happened — no official install URL
exists yet, and nothing described here has been exercised against a real
public distribution channel.

## Current state (what actually exists today)

- `installer/install.sh` selects a release artifact (glibc or musl) by
  trusted `distro_family` classification and, since Phase 17A, verifies each
  selected binary's SHA256 against a `SHA256SUMS` manifest **when one is
  present alongside the artifact directory** — fails closed (aborts before
  any privileged step) on a mismatch, skips verification (not a failure)
  when no manifest is present at all. See `installer/install.sh`'s
  `verify_artifact_checksum` and `tests/distro/checksum-verification.sh`
  for the live negative-control evidence.
- `packaging/build-release.sh` (glibc) and `packaging/build-release-musl.sh`
  (musl) produce the binaries `install.sh` selects from, from pinned builder
  images (Rocky Linux 9 and Alpine 3.20, both digest-pinned) and a pinned
  Rust toolchain (`1.97.1`).
- There is **no remote-fetch installer today**. `install.sh` only operates
  on a locally checked-out repository with locally-built or locally-placed
  artifacts. The single-command `curl -fsSL <official-url> | sudo bash`
  contract described in `GOAL.md` G1 does not exist as working code yet —
  it is a release-publication item, not an engineering gap in the local
  installer itself.
- No release artifact has ever been signed. No signing key exists.

## Intended integrity chain (design, not yet implemented end-to-end)

1. **Transport**: the public install command fetches over HTTPS (TLS
   provides transport confidentiality/integrity against network tampering,
   but says nothing about whether the *content itself* is what the project
   actually published — this is the well-known limitation `curl | bash`
   critics correctly point out, and this project should not pretend
   otherwise).
2. **Artifact selection**: the fetch script determines the caller's
   platform (reusing the same `distro_family` classification `install.sh`
   already trusts) and requests the matching pre-built archive
   (`linux-x86_64-glibc` or `linux-x86_64-musl`).
3. **Checksum verification**: the fetch script downloads a `SHA256SUMS`
   manifest and verifies the downloaded archive against it before
   extracting or executing anything from it, using the same fail-closed
   logic `install.sh` already implements locally.
4. **Manifest authenticity**: this is the step that does **not** exist yet
   and is the actual hard problem. A `SHA256SUMS` file fetched from the
   *same* untrusted HTTPS endpoint as the artifact it's meant to verify
   only proves the archive matches what that endpoint served *right now* —
   it does not prove the endpoint itself wasn't compromised, and must never
   be described as a complete trust solution on its own.

## Recommended signing scheme (not implemented, no keys generated)

Given this project has no existing GPG/minisign/cosign/Sigstore
infrastructure, and this pass does not generate one (per this pass's own
explicit instruction not to create permanent signing keys or use an
operator's personal key without explicit direction), the recommended path
for whoever authorizes publication is:

- **If publishing through GitHub Releases**: use [GitHub Artifact
  Attestations](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds)
  (Sigstore-backed, keyless, tied to the specific CI workflow run that
  produced the artifact) as the primary mechanism — it requires no
  long-lived private key material to manage, and `gh attestation verify`
  gives users a real provenance check tied to the actual build, not just a
  checksum served alongside the file.
- **If a detached signature is also wanted for non-GitHub distribution**:
  `minisign` is the lower-operational-overhead choice for a project this
  size (a single Ed25519 keypair, tiny trusted-comment-carrying signature
  files, no keyserver/web-of-trust management) over classic GPG.
- Whichever is chosen, the public key (or Sigstore/Rekor transparency-log
  reference) must be published somewhere **other than** the same download
  location as the artifacts themselves (e.g. in this repository's own
  README, pinned in the install script's own source, or both) — a
  signature is only as trustworthy as the independence of where its
  verification key came from.

**This pass does not implement any of the above.** It documents the
recommendation and leaves signing key generation, CI wiring, and the actual
remote-fetch script as explicit follow-up work requiring its own
authorization.

## What Phase 17A does close

- Fail-closed local checksum verification exists and is tested (positive
  path: `tests/distro/artifact-selection.sh`; negative path:
  `tests/distro/checksum-verification.sh`, which corrupts a real staged
  binary and requires the installer to abort with nothing installed).
- The trust-model gap above (manifest authenticity, transport-only TLS) is
  now written down explicitly rather than left as an unstated assumption
  behind "the installer uses HTTPS" — Phase 10's own closure record already
  flagged this as open debt; this document is where that debt now lives
  with a concrete recommended next step.
