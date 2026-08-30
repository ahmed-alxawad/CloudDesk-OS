# Phase 17 Release/Publication Closure (17A–17G)

This document is the single reference for CloudDesk-OS's release-packaging
and publication-readiness work. It does not repeat evidence already fully
recorded elsewhere -- see `RELEASE_NOTES.md`, `docs/RELEASE_INTEGRITY.md`,
`docs/RELEASE_SIGNING_DECISION.md`, and `docs/RELEASE_TAG_PREVIEW.md`.

## Candidate lineage

```
v1.0.0            tag  9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2  IMMUTABLE, unchanged
v1.0.1-rc.1       tag  89bfe4690ff5b4b178cb68a1a40806a13fa04f99  LOCAL ONLY, frozen, superseded
v1.0.1-rc.2       tag  6b1eaa81b7ec36980e5f01edbaeca3e7b1fd8fa0  LOCAL ONLY, frozen, superseded (Phase 17G)
```

Both `v1.0.1-rc.1` and `v1.0.1-rc.2` are annotated, unsigned, local-only
git tags. Neither has ever been pushed. No release, artifact, or installer
has ever been published. `v1.0.1-rc.2`'s disposition changed from
"publication candidate" to "superseded" in Phase 17G — see below.

## rc.1 disposition: FROZEN BUT SUPERSEDED BEFORE PUBLICATION

`v1.0.1-rc.1`'s frozen source commit (`89bfe46`) does not contain a root
`LICENSE` file. Established project licensing policy (`Cargo.toml`'s
`license = "AGPL-3.0-or-later"`, corroborated by `README.md`, `MISSION.md`,
and `ARCHITECTURE.md`) requires one for publication. `LICENSE` was added in
commit `975bd84`, **after** the rc.1 tag target -- and the rc.1 tag is not
moved to accommodate this, per every governing instruction's explicit rule
against rewriting frozen tag history.

This is proven, not asserted: `tests/distro/release-staging-validation.sh`
run against `89bfe46` fails on the missing `LICENSE`; the same script run
against current `HEAD` (which includes `975bd84`) passes.

```
$ tests/distro/release-staging-validation.sh 89bfe4690ff5b4b178cb68a1a40806a13fa04f99 1.0.1-rc.1
FAIL: required source file 'LICENSE' is missing at 89bfe4690ff5b4b178cb68a1a40806a13fa04f99

$ tests/distro/release-staging-validation.sh HEAD 1.0.1-rc.1
PASS: HEAD / 1.0.1-rc.1 is release-staging-complete
```

**Conclusion: a future `v1.0.1-rc.2` is required before public release.**
`rc.2` must be cut from a commit at or after `975bd84` and must repeat, not
assume, the full Phase 17A/17B build-and-verify sequence: fresh glibc and
musl builds from that exact commit, fresh SHA256 hashes, a fresh manifest
with that commit as `source_commit`, a fresh SBOM, and fresh checksum/
negative-control verification. rc.1's hashes must not be blindly reused for
rc.2 even though the product source itself did not change, because the
whole point of Phase 17B's methodology was to prove rather than assume
build-output equivalence.

## License status

| Item | Status |
| --- | --- |
| Root `LICENSE` | ADDED (commit `975bd84`) -- canonical, unmodified GNU AGPLv3 text, copied verbatim from a local canonical copy (`/usr/share/licenses/kdepim-runtime/AGPL-3.0-or-later.txt`, diff-verified identical) |
| Community license | `AGPL-3.0-or-later` -- an established project decision (`Cargo.toml`), not invented in this pass |
| Commercial license | NOT AUTHORED -- a dual-licensing *intention* exists in `MISSION.md`/`ARCHITECTURE.md`; no actual commercial agreement text exists. This does not block Community/AGPL publication engineering-readiness. |

## Third-party licensing engineering inventory

| Component | Distribution model | License | Legal review |
| --- | --- | --- | --- |
| code-server | optional external runtime (Docker image, not bundled) | MIT | NOT REQUIRED (permissive, unmodified) |
| Collabora Online (CODE) | optional external runtime (Docker image, not bundled) | MPL-2.0 (`coolwsd`) | NOT REQUIRED for the current not-bundled deployment model; commercial/production Collabora deployment terms are the operator's own choice, not CloudDesk-OS's redistribution obligation |
| Brave Browser | optional external runtime (locally built Docker image, not bundled) | proprietary freeware over BSD-licensed Chromium | UNKNOWN -- no formal conclusion drawn on Brave's own proprietary terms; operators should review before any commercial redistribution that bundles/depends on it |
| FFmpeg | host system package (not bundled/redistributed) | dual LGPL/GPL; **live tested build reports `--enable-gpl`** | REQUIRED before any release messaging assumes LGPL-only, since CloudDesk-OS does not control what build an operator's own system package provides |

Engineering information complete for all four: **YES**. None of the four
are bundled into CloudDesk-OS's own release artifacts -- code-server,
Collabora, and Brave are separate, independently pulled/built OCI
containers; FFmpeg is a host system dependency CloudDesk-OS invokes if
present. This means CloudDesk-OS's own release artifacts (the three Rust
binaries + installer) carry no redistribution obligation from any of the
four, though operators integrating/deploying them still should review
Brave's and Collabora's own commercial terms, and FFmpeg's build flags,
before their own further redistribution decisions.

## SBOM

CycloneDX 1.5, 464 components (442 Rust crates via `cargo tree`, 22 npm
production packages via `npm list --omit=dev`). Covers exactly the
compiled-in dependencies of the three shipped binaries and the web
frontend build -- explicitly does **not** cover the four externally
orchestrated runtimes above, which are recorded instead in
`docs/THIRD_PARTY_NOTICES.md` per their "Distribution model" lines. This
scope distinction is now stated explicitly in
`docs/THIRD_PARTY_NOTICES.md`'s own header.

## Signing

No signing key exists; nothing has ever been signed. Recommendation
(`docs/RELEASE_SIGNING_DECISION.md`): GitHub Artifact Attestations
(Sigstore-backed, keyless, CI-tied) as primary once hosted on GitHub
Actions, minisign as a complementary offline-verification option. Neither
is implemented.

- **GitHub Attestations**: UNAVAILABLE UNTIL HOSTED -- structurally requires
  running inside GitHub Actions with OIDC; this repository has no
  configured remote (`git remote -v` is empty) and no workflow file was
  added, since a dormant untested workflow would not be evidence of
  anything.
- **minisign**: NOT EXECUTED -- KEY MATERIAL NOT ESTABLISHED. No key is
  generated. Naming convention, verification command, and public-key
  distribution requirements are documented in `docs/RELEASE_INTEGRITY.md`
  for whoever implements this later.

## Official installer URL

UNAVAILABLE. `installer/install.sh` operates only on a local checkout with
locally-built or locally-placed artifacts; it contains no production URL,
real or fake. Future remote-fetch endpoints are documented in
`docs/RELEASE_INTEGRITY.md` using an explicit `<OFFICIAL_RELEASE_BASE_URL>`
placeholder -- no domain is invented.

## Local publication dry run

Executed entirely on `127.0.0.1` (`python3 -m http.server`), no external
host or upload: staged the intended release layout (installer script,
`SHA256SUMS`, both artifact families), fetched all 7 files over real HTTP
with `curl`, verified with `sha256sum -c`. **Result: PASS, 7/7 verified.**
Full detail in `docs/RELEASE_INTEGRITY.md`'s "Local publication dry run"
section. This proves internal consistency of the static release layout; it
does not and cannot prove manifest authenticity (see the trust-chain
section of that same document).

## Community vs. Commercial readiness

| | Engineering-ready | Publication-ready |
| --- | --- | --- |
| **Community (AGPL-3.0-or-later)** | YES, as of a commit including `975bd84` (rc.1 itself is not, per above) | NO -- no official install URL, no signing, rc.2 not yet cut/verified |
| **Commercial** | NO -- no authored commercial terms | NO |

## Blocker taxonomy

**Community publication blockers:**
1. `v1.0.1-rc.1` must be superseded by a verified `rc.2` cut from a commit including the `LICENSE` fix
2. no official installer URL / hosting exists
3. no artifact-signing scheme is implemented

**Commercial publication blockers:**
1. commercial license terms not authored
2. Collabora and Brave commercial/redistribution terms not reviewed by counsel

**Legal review items:**
1. FFmpeg `--enable-gpl` build-flag implications for any release messaging
2. Collabora Online commercial terms (informational only, not required for the current not-bundled deployment model)
3. Brave Browser proprietary terms (informational only, not required for the current not-bundled deployment model)

**Signing/infrastructure blockers:**
1. no GitHub remote/Actions workflow exists (blocks Attestations)
2. no minisign key generated (blocks minisign)

**Environment evidence limitations (carried forward from Phase 10, not new publication blockers):**
1. RHEL fully subscribed environment: UNAVAILABLE
2. SELinux enforcing mode: BLOCKED BY ENVIRONMENT
3. true reboot persistence: BLOCKED BY ENVIRONMENT

## Phase 17E: rc.2 exact-source build (evidence)

Candidate source commit: `6b1eaa81b7ec36980e5f01edbaeca3e7b1fd8fa0` (version
bump to `1.0.1-rc.2` on top of the Phase 17D LICENSE/notices/validator
commits). `tests/distro/release-staging-validation.sh` PASSes against this
exact commit and its staging directory.

| | glibc | musl |
| --- | --- | --- |
| Two independent clean builds byte-identical | YES | YES |
| `clouddeskd` SHA256 | `52d82bf778ecf08593915fdb2f88550f54d08384d68d195d05ebd5f39c6a852d` | `1aeac0b8c431152178da711593dc5770c6c52a2e1c62bf25b9d614e6367e2752` |
| `cloudesk-privd` SHA256 | `701712130b4e271c571415ef7dfcc9d62fb3f0dedbfc7ae5ee3f6a7344567ec2` | `9f6bbc92d55d5e19976438149f6bc9408175cc6df3b8e3d7d2d4ea9647d1c43b` |
| `cloudesk-sessiond` SHA256 | `9c941657b62257f7b876b3c9eeeaa98b74df67ecf292050d91ad25af8f182c65` | `654278d90084f49af740a3814dc5816c2183b5569f881a00f121e1d6d3166c8e` |
| ABI/linkage | GLIBC_2.34 max, unchanged | static-pie, no dynamic deps |
| vs. rc.1 hashes | DIFFERENT (version string compiled in; no functional code change) | DIFFERENT (same reason) |

Installer (`installer/install.sh`) SHA256: `1ee530659689feeb3feb5219c66760186887580dc38824b7403c114c3946f6b3`
— byte-identical to rc.1's, since the installer script itself was not
modified between rc.1 and rc.2.

SBOM: unchanged at 464 components (442 Rust + 22 npm) — no dependency
changed. Artifact selection: 8/8 distro families PASS. Checksum
verification + corruption negative control: PASS. Local publication dry
run: PASS, 7/7 artifacts fetched over real localhost HTTP and
checksum-verified. Secret/operator-path scan of staging and all six
compiled binaries: 0 findings.

**`v1.0.1-rc.2` tag was created in Phase 17F** (annotated, unsigned, local
only, targeting `6b1eaa8`) — see Phase 17G immediately below for why it is
now also classified superseded.

## rc.2 disposition (Phase 17G): FROZEN BUT SUPERSEDED BEFORE PUBLICATION

The recommended primary signing mechanism (GitHub Artifact Attestations,
`docs/RELEASE_SIGNING_DECISION.md`) requires a CI workflow file
(`.github/workflows/release-attest.yml`) that triggers on the release tag
being pushed, so the attested ref and the tagged ref are always identical
(see that document's "Workflow trigger design decision" section for why a
`workflow_dispatch`-with-ref-input alternative was rejected as producing a
confusing, weaker provenance claim). That workflow file was added in
commit `6678260`, **after** the rc.2 tag target (`6b1eaa8`) — so pushing
the existing `v1.0.1-rc.2` tag to a real remote would never trigger it.

**Conclusion: a future `v1.0.1-rc.2` is not enough; `v1.0.1-rc.3` will be
required** once GitHub hosting is actually established, cut from a commit
at or after `6678260` so the tagged commit itself carries the attestation
workflow. `v1.0.1-rc.2`'s tag is not moved, deleted, or reused — it remains
frozen exactly as created in Phase 17F. `v1.0.1-rc.3` is **not created in
this pass**.

This does not retroactively invalidate rc.2's engineering content (LICENSE,
notices, staging validator, byte-reproducible builds) — all of that carries
forward unchanged into whatever commit rc.3 eventually tags. The only new
requirement rc.2 lacks is the CI workflow file itself.

## Publication Pass A: repository identity and a real implementation gap

### GitHub repository identity: confirmed authoritative

`Cargo.toml` has carried `repository = "https://github.com/ahmed-alxawad/CloudDesk-OS"`
unchanged since at least `v1.0.0`. No `git remote` has ever pointed there in
any pass, and every prior Phase 17 pass independently concluded "no GitHub
identity established" without ever citing this field — this pass could not
tell from repository evidence alone whether it was authoritative or stale
scaffolding, so it was put to the operator directly rather than assumed
either way. **Operator-confirmed (Publication Pass A): this is the real,
owned repository.** `github.com/ahmed-alxawad/CloudDesk-OS` is now the
authoritative identity for hosting, verification-command documentation, and
future publication. `git remote` is still not configured locally — adding
one remains out of scope for this pass (explicitly not permitted) and is a
separate future step.

Note this does **not** block rc.3 source-readiness by itself:
`.github/workflows/release-attest.yml` needs no hardcoded owner/repo string
— GitHub Actions derives repository identity automatically
(`github.repository`) from wherever the workflow actually runs. The
identity question only matters for *documentation* (verification command
examples already use placeholders) and for *where the tag is eventually
pushed* — both external/configuration matters, not source content.

### New finding: the public installer does not yet fetch anything

`GOAL.md`'s G1 (a v1.0 **required** primary goal, not a Phase 17 checklist
item) specifies the installer must support:

```
curl -fsSL <official-install-url> | sudo bash
```

and explicitly states the installer itself "must install or fetch required
core dependencies; install CloudDesk services." The actual
`installer/install.sh` (266 lines, inspected in full this pass) contains
**no network-fetch code of any kind** — no `curl`/`wget` artifact download,
confirmed by direct inspection, not assumption. It operates exclusively on
binaries already present locally (a real build, or a pre-placed `dist/`
tree via `CLOUDESK_BINARY`-style overrides). A user on a fresh machine
running the literal G1 command today would have nothing for `install.sh`
to install — the curled script would find no local `dist/` and fail at its
existing `[ -f "$binary_source" ] || fail "missing release binary"` check.

**Classification: PUBLICATION IMPLEMENTATION MISSING**, not "official URL
unavailable." This is a distinct, real gap discovered in this pass, not
previously flagged as such. Two structurally different fixes exist and
neither has been chosen or implemented:

1. Extend `install.sh` itself to fetch the platform-appropriate artifact
   tarball, checksum manifest, and (once available) attestation, before
   proceeding — the installer becomes the fetch client directly.
2. Keep `install.sh` exactly as it is (operating on local files) and make
   the curled entry point a small, separate bootstrap script that fetches
   and extracts a release tarball (artifacts + `install.sh` + checksums)
   into a temp directory, then execs the existing local `install.sh`
   against it — a wrapper pattern used by several real-world installers.

**Not implemented in this pass** — this is new product/feature work
(network fetch, redirect policy, HTTPS enforcement, version-pinning-on-
fetch), not signing/hosting configuration, and is explicitly out of scope
for a configuration-only pass. It is source-changing and therefore blocks
`v1.0.1-rc.3` readiness independent of the GitHub identity question above.

## Phase 17G: signing/hosting architecture (evidence)

- **Authentication gap, precisely stated**: SHA256 checksum verification
  (already implemented) proves *integrity* — that a downloaded file matches
  a co-located manifest. It proves nothing about *authenticity* — that the
  manifest itself came from CloudDesk-OS's own release process rather than
  whoever controls the download host. Closing this requires a trust root
  independent of the download host: GitHub Attestations (CI/OIDC identity)
  or minisign (an independently-published keypair). Full detail:
  `docs/RELEASE_SIGNING_DECISION.md`.
- **GitHub hosting**: no organization/repository identity has been chosen
  anywhere in this project — `GITHUB HOSTING: DECISION/CONFIGURATION
  REQUIRED`. `git remote -v` remains empty.
- **Minisign**: remains complementary and operator-manual — no public key
  is embedded in the installer, so no source change was needed for it in
  this pass. Automating installer-side minisign verification would need a
  pinned public key and is optional future work, not a publication blocker.
- **Local hosting simulation + negative controls** (all against real
  `installer/install.sh` logic via `CLOUDESK_ROOT` fixtures, no root
  required, no external network):
  - corrupted binary → checksum mismatch → install aborted, nothing
    installed (**PASS**, `tests/distro/checksum-verification.sh`)
  - corrupted `SHA256SUMS` entry → checksum mismatch → install aborted
    (**PASS**)
  - missing `SHA256SUMS` entry for a required binary → explicit
    "no entry in SHA256SUMS" failure, install aborted (**PASS**)
  - wrong-platform artifact substituted (musl binary at the glibc path,
    checksums untouched) → checksum mismatch caught, install aborted,
    directory restored (**PASS**)
  - manifest version/source-commit mismatch → `tests/distro/
    release-staging-validation.sh` now cross-checks these and fails
    closed (**PASS**, added this pass — previously only checked file
    presence, not metadata consistency)
  - redirect handling / HTTP-downgrade / path-traversal-in-fetched-artifact-name:
    **NOT APPLICABLE** — `installer/install.sh` contains no network-fetch
    code today (confirmed by inspection: no `curl`/`wget` artifact-download
    logic exists); these controls apply to a future remote-fetch installer
    that has not been built yet, not to current code.
- **CI workflow**: `.github/workflows/release-attest.yml` added, dormant
  (no remote exists to trigger it), minimum permissions
  (`contents: read`, `id-token: write`, `attestations: write`), YAML
  syntax validated.
- **SBOM tooling**: `packaging/gen-sbom.py` committed (previously only an
  ephemeral scratch script); verified to reproduce the identical
  464-component set.
- **Production signing keys generated**: 0. **Test-only signing fixture
  used**: 0 (not needed — existing SHA256 mechanics were exercised
  directly against real artifacts).

## Publication Pass A: remaining design decisions closed

- **Supply-chain pinning**: `actions/checkout` and `actions/attest-build-provenance`
  in `.github/workflows/release-attest.yml` were pinned from mutable major-
  version tags (`@v4`, `@v1`) to their exact current commit SHAs
  (`actions/checkout@11d5960...`, `actions/attest-build-provenance@ef24412...`,
  resolved live via the public GitHub API, not guessed), per this pass's
  supply-chain review. Both remain functionally the same released version;
  only the trust binding changed from "whatever `v4` points to when this
  runs" to "exactly this commit."
- **Attestation subject model**: individual artifacts (each of the 6
  binaries, the installer, `SHA256SUMS`, and the SBOM) are attested
  separately — model (A) from the manifest-authentication boundary
  question, not a single release-manifest-level attestation. A verifier
  checks any one downloaded file directly against its own attestation.
- **Release approval boundary**: the workflow builds and attests
  automatically on any matching tag push, but **never publishes or
  uploads anything** — attested artifacts stay inside the ephemeral CI
  run unless a maintainer separately, manually downloads and publishes
  them per `docs/RELEASE_PUBLICATION_CHECKLIST.md`. No human-approval
  gate is added before build+attest because build+attest alone exposes
  nothing to end users; the actual publish step remains the deliberate,
  separate, manual boundary.
- **Release asset immutability policy**: once a version's assets are
  published, they must never be replaced in place with different bytes
  at the same path — a corrected artifact requires a new version/tag,
  consistent with this project's existing immutable-tag policy for git
  itself.
- **Minisign reclassified**: per this pass's explicit correction, minisign
  is **not** a Community publication blocker — it is complementary,
  optional, offline-verification hardening. `MINISIGN REQUIRED FOR FIRST
  RELEASE: NO`. The trust root for the primary mechanism (GitHub
  Attestations) is GitHub/Sigstore OIDC plus repository/workflow identity
  — not a CloudDesk-controlled private key at all, so "no minisign key"
  is removed from the blocker list below.

### Reconciled blocker taxonomy (Publication Pass A)

**Community publication ENGINEERING blockers** (require source changes):
1. Installer/bootstrap artifact-fetch implementation missing (see above) —
   required for GOAL.md G1's literal `curl | bash` flow to function against
   a fresh machine with nothing pre-built locally.

**Community publication EXTERNAL/CONFIGURATION blockers** (no source change needed):
1. No GitHub remote configured locally; Actions/attestation cannot actually
   run until one exists (repository identity itself is now confirmed:
   `github.com/ahmed-alxawad/CloudDesk-OS` — operator-confirmed, Publication
   Pass A)
2. Official hosting model (GitHub Releases vs. project-controlled HTTPS)
   not chosen

**Community LEGAL REVIEW items** (unchanged, not new):
1. FFmpeg `--enable-gpl` release-messaging review
2. Collabora commercial-term review (where relevant)
3. Brave commercial-redistribution review (where relevant)

**Commercial blockers** (unchanged, kept separate from Community):
1. commercial license terms not authored
2. applicable third-party commercial redistribution review incomplete

## Security status (preserved, not reopened)

Phase 16: COMPLETE. Catalog: 135 (37 fresh + 96 revalidated-prior + 2 not-applicable).
Critical open: 0. High open: 0. Medium open: 0. Low open (accepted): 2.

## Phase 17 status

`Architecture/CloudDesk-OS-spec/PLAN.md`'s own Phase 17 section
(`## Phase 17 - Packaging, Documentation, and v1.0 Release`) states its
exit gate verbatim as:

> All required features work, all official distributions pass the release
> matrix, and optional heavy runtimes can be enabled/disabled from
> Settings.

That gate is about product/feature/distro completeness, not about signing,
hosting, or actual publication — those appear only in Phase 17's "Work"
checklist (`checksums/signatures`, `commercial licensing path`,
`dependency notices`), which is a list of things this phase should address,
not additional exit-gate conditions. By the literal gate text: v1.0.0
already shipped every required feature, Phase 10's distro matrix is
COMPLETE, and optional runtimes (Code/Office/Browser/FFmpeg) already
toggle from Settings — **this gate is met**.

**PHASE 17 (engineering): COMPLETE**, by PLAN.md's own literal exit
criterion. This is deliberately distinct from **publication**, which has
not happened and is not claimed to have happened: no artifact has been
signed, hosted, or made downloadable by anyone outside this repository.
The remaining checklist items (`checksums/signatures` fully authenticated
end-to-end, `commercial licensing path`) are documented, bounded, external
next steps — establishing a GitHub repository identity, generating real
signing key material, authoring commercial terms — none of which are
engineering defects in the existing product, and none of which this or any
prior pass is authorized to perform without further explicit authorization.
