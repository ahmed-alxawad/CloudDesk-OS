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

**Update**: the design below was written before CI attestation existed. It
is now implemented and live-verified as of `v1.0.1-rc.4` — see
`.github/workflows/release-attest.yml` and the "GitHub Artifact
Attestation preparation" status further down this document. The original
historical design rationale is preserved as-is below.

```
Git tag (v1.0.1-rc.1, local only at the time this was written)
  -> commit content is fixed by its sha (89bfe4690ff5b4b178cb68a1a40806a13fa04f99)
  -> CI build identity (now implemented -- see update note above)
  -> artifact digest (SHA256, already computed and reproducibly verified —
     see PHASE17B evidence in dist/release/1.0.1-rc.1/metadata/manifest.json)
  -> attestation/signature over that digest (now implemented -- see update note above)
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

### Publication Pass B: implemented public-download endpoints (real identity)

The operator confirmed the authoritative repository is
`ahmed-alxawad/CloudDesk-OS`. `installer/install.sh`'s direct-fetch mode
(active when `CLOUDESK_VERSION` is set) now implements this layout for
real, defaulting to:

```
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/install.sh
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/SHA256SUMS
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/manifest.json
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/sbom.cdx.json
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/clouddesk-web.tar.gz
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/clouddeskd-linux-x86_64-glibc
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/cloudesk-privd-linux-x86_64-glibc
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/cloudesk-sessiond-linux-x86_64-glibc
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/clouddeskd-linux-x86_64-musl
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/cloudesk-privd-linux-x86_64-musl
https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download/<tag>/cloudesk-sessiond-linux-x86_64-musl
```

where `<tag>` is `v<version>` (e.g. `v1.0.1-rc.3`). Binary asset names are
flat (`<binary>-<artifact_family>`, e.g. `clouddeskd-linux-x86_64-glibc`)
because GitHub Release assets cannot contain directory structure; the
checksum manifest still labels them with the `<artifact_family>/<binary>`
relative-path convention used since Phase 17A for consistency with the
local/offline evidence layout, and the installer maps between the two
deterministically (never from remote-supplied data). No release has
actually been published at this URL yet — the endpoint is implemented and
locally fixture-tested (`tests/distro/remote-fetch.sh`), not exercised
against the real host.

**Overridable for testing**: `CLOUDESK_RELEASE_BASE_URL` replaces the
default base; `CLOUDESK_ALLOW_INSECURE_TEST_URL=1` is required to permit
`http://` (never set in production — the installer refuses `http://`
otherwise).

**Enforced by the implementation**: HTTPS-only by default (`--proto`/
`--proto-redir` also reject a redirect that would downgrade the transfer
mid-flight, not just the initial request's scheme); the version string is
regex-validated (`^v?[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$`) before any URL
is constructed — no shell metacharacters, path traversal, or arbitrary
interpolation of remote-supplied data into a path or command; platform
maps to artifact via the same allowlisted `distro_family` classification
the local path already used; the requested version must equal the
downloaded manifest's own declared version, or the install fails closed
before touching a binary.

### Public-download manifest/checksum model (Model B, chosen)

Three plausible models existed for where trust in a downloaded artifact's
hash actually lives:

- **A**: the manifest itself carries expected hashes.
- **B (chosen)**: `SHA256SUMS` is the sole source of truth for hashes;
  `manifest.json` is authoritative only for version/source-commit
  provenance metadata.
- **C**: both are parsed and cross-checked for agreement.

Model B was chosen over C deliberately: `installer/install.sh` is POSIX
`sh` with no JSON parser available, and reliably cross-validating
nested per-artifact hash fields from `manifest.json` in shell (`grep`/
`sed` against machine-generated-but-still-untrusted-until-downloaded
JSON) adds meaningfully more fragile parsing surface for a benefit that's
largely redundant: both files are produced by the same CI job from the
same commit and are (once implemented) attested together — a real-world
attacker capable of forging one without detection could very likely forge
the other identically, so shell-level cross-checking mostly catches
*accidental* internal inconsistency in the release process, which the
staging validator (`tests/distro/release-staging-validation.sh`) already
catches earlier, before publication. `manifest.json` extraction is limited
to two scalar string fields via a targeted `grep`/`sed` pattern (not a
general parser), with the extracted version format independently
regex-validated as defense in depth.

### GitHub Artifact Attestation preparation

**Current status: LIVE.** As of `v1.0.1-rc.4`, `.github/workflows/release-attest.yml`
runs on every `v1.0.*` tag push, builds all release artifacts from that
exact tagged commit, and attests each one via GitHub's OIDC-backed
Sigstore integration. All 11 `v1.0.1-rc.4` release assets were
cryptographically verified against this workflow's attestations, both
immediately after upload and again after a fresh public download —
verify any asset yourself:

```sh
gh attestation verify <downloaded-file> --repo ahmed-alxawad/CloudDesk-OS
```

The section below is preserved as the original historical design record
from before this was implemented.

Historical status (superseded): **UNAVAILABLE UNTIL HOSTED.** This repository has no configured
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
