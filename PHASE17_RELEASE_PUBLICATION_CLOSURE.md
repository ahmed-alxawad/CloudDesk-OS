# Phase 17 Release/Publication Closure (17A–17D)

This document is the single reference for CloudDesk-OS's release-packaging
and publication-readiness work. It does not repeat evidence already fully
recorded elsewhere -- see `RELEASE_NOTES.md`, `docs/RELEASE_INTEGRITY.md`,
`docs/RELEASE_SIGNING_DECISION.md`, and `docs/RELEASE_TAG_PREVIEW.md`.

## Candidate lineage

```
v1.0.0            tag       9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2  IMMUTABLE, unchanged
v1.0.1-rc.1       tag       89bfe4690ff5b4b178cb68a1a40806a13fa04f99  LOCAL ONLY, frozen, superseded
v1.0.1-rc.2       (no tag)  6b1eaa81b7ec36980e5f01edbaeca3e7b1fd8fa0  candidate source frozen (Phase 17E), tag not yet created
```

`v1.0.1-rc.1` is an annotated, unsigned, local-only git tag. It has never
been pushed. `v1.0.1-rc.2`'s candidate source commit is frozen and fully
built/verified (Phase 17E, below) but its tag has deliberately not been
created yet, per that pass's own explicit instruction. No release,
artifact, or installer has ever been published.

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

**`v1.0.1-rc.2` tag was deliberately not created in Phase 17E**, per that
pass's own explicit instruction — this remains a documented, fully-verified
candidate awaiting separate tag-creation authorization.

## Security status (preserved, not reopened)

Phase 16: COMPLETE. Catalog: 135 (37 fresh + 96 revalidated-prior + 2 not-applicable).
Critical open: 0. High open: 0. Medium open: 0. Low open (accepted): 2.

## Phase 17 status

**PHASE 17: PARTIAL.** Local tooling, documentation, licensing, and
verification work is complete, but Phase 17's actual exit criteria (a
publication-ready, hosted, signed release) are not met and cannot be met
without a corrected `rc.2` candidate, real hosting, and a real signing
mechanism -- none of which this or any prior pass is authorized to create
without further explicit authorization.
