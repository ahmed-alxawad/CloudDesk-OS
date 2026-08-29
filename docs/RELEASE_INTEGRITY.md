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

## Phase 17D additions: exact trust chain, endpoints, and local dry-run evidence

### Precise trust chain (what each link actually authenticates)

```
Git tag (v1.0.1-rc.1, local only today)
  -> commit content is fixed by its sha (89bfe4690ff5b4b178cb68a1a40806a13fa04f99)
  -> CI build identity (NOT YET IMPLEMENTED: no GitHub Actions workflow exists
     yet that builds and attests from a tag)
  -> artifact digest (SHA256, already computed and reproducibly verified —
     see PHASE17B evidence in dist/release/1.0.1-rc.1/metadata/manifest.json)
  -> attestation/signature over that digest (NOT YET IMPLEMENTED)
  -> checksum manifest (SHA256SUMS) binds a filename to a digest, but a
     SHA256SUMS file is only as trustworthy as whatever authenticates it —
     see "Manifest authenticity" above; SHA256 alone proves the download
     matches the manifest, not that the manifest reflects what the project
     actually intended to publish
  -> installer artifact selection (installer/install.sh, trusted
     distro_family classification, existing today)
  -> installer verification (verify_artifact_checksum, fail-closed, existing
     today)
```

**Precisely stated**: SHA256 protects against accidental corruption and
against a download that was tampered with *after* the manifest was fetched
but *not* verified independently of the manifest's own source. It does
**not** authenticate that the manifest itself is genuine — that requires an
attestation or signature whose verification key/identity is published
somewhere independent of the download host, which is exactly the gap
GitHub Attestations or minisign close and which is not implemented yet.

### Future publication endpoints (placeholders, no domain is authoritative)

No official domain/organization/repository URL exists yet. Future tooling
and documentation should reference these only as configurable placeholders:

```
<OFFICIAL_RELEASE_BASE_URL>/<version>/installer/install.sh
<OFFICIAL_RELEASE_BASE_URL>/<version>/SHA256SUMS
<OFFICIAL_RELEASE_BASE_URL>/<version>/linux-x86_64-glibc/{clouddeskd,cloudesk-privd,cloudesk-sessiond}
<OFFICIAL_RELEASE_BASE_URL>/<version>/linux-x86_64-musl/{clouddeskd,cloudesk-privd,cloudesk-sessiond}
<OFFICIAL_RELEASE_BASE_URL>/<version>/sbom/cloudesk-os.cdx.json
<OFFICIAL_RELEASE_BASE_URL>/<version>/attestations/... (once GitHub Attestations or minisign exists)
```

Any future remote-fetch installer code must: require HTTPS only, construct
the artifact path deterministically from an explicitly-passed version
string (no arbitrary interpolation of remote-supplied metadata into a
shell command or path), map platform to artifact via the same allowlisted
`distro_family` classification already used locally, and fail closed on
any checksum mismatch — exactly as the local path already does.

### GitHub Artifact Attestation preparation

Status: **UNAVAILABLE UNTIL HOSTED.** This repository has no configured
GitHub remote (`git remote -v` is empty) and no GitHub Actions workflow
exists. Attestation generation is structurally tied to running inside
GitHub Actions with OIDC — it cannot be produced or meaningfully tested
from a local machine. No workflow file is added in this pass because a
dormant, untested workflow file would not constitute evidence of anything
and risks silently drifting from whatever the eventual hosted repository
actually needs. This remains explicit follow-up work for whoever
authorizes hosting.

### minisign preparation

Status: **NOT IMPLEMENTED — KEY MATERIAL NOT ESTABLISHED.** No private or
public minisign key is generated by this or any prior pass. If minisign
support is added later:

- Signature files use the `<artifact>.minisig` naming convention alongside
  each release artifact.
- Verification command for operators: `minisign -Vm <artifact> -P <public-key>`.
- The public key must be published independently of the download host
  (e.g. pinned in this repository's own README/install script source, not
  only alongside the artifacts).
- Until a real key exists, any code path that would need one must leave the
  public key value explicitly unconfigured and refuse to claim verification
  succeeded — never substitute a placeholder value that could be mistaken
  for a real project key.

### Local publication dry run (Phase 17D, evidence)

Exercised entirely on localhost, no external host or upload involved:

1. Staged a synthetic release directory mirroring the intended endpoint
   layout above (`installer/install.sh`, `SHA256SUMS`, both artifact
   family directories) in a scratch directory.
2. Served it with `python3 -m http.server` bound to `127.0.0.1`.
3. Fetched all 7 files with `curl` exactly as a future remote installer
   would, then verified them with `sha256sum -c` against the fetched
   `SHA256SUMS`.
4. Result: **all 7 files verified OK.** This proves the static release
   layout and checksum manifest are internally consistent and servable —
   it does not and cannot substitute for the still-missing manifest
   authenticity link above.

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
