# CloudDesk-OS v1.0.0 Release Notes

CloudDesk-OS v1.0.0 is the first production release of the lightweight, multi-user web desktop for Linux servers. It delivers a modern, secure, web-accessible workspace combining native browser applications, remote infrastructure access, isolated container runtimes, and strict operating-system-level privilege separation.

## Highlights

### 1. Multi-User Web Desktop Platform
* **Clean Web Shell**: Modern Svelte 5 desktop interface featuring responsive window management, taskbar, launcher, system tray, workspace manager, and dark/light system themes.
* **Role-Based Access Control (RBAC)**: Fine-grained permissions framework with role snapshots, granular capability checks, and session governance.
* **Two-Factor Authentication (2FA)**: RFC 6238 TOTP two-factor authentication with single-use cryptographic recovery codes and server-side rate-limited login throttling.

### 2. Native Applications & Media
* **Files / VFS**: High-performance Virtual File System with path traversal protection, symlink escape prevention, root capability sandboxing, and directory search.
* **Gallery**: Media browsing application supporting responsive image thumbnails and client-side format previews.
* **Audio & Video Player**: Streaming audio and video playback directly inside the desktop environment.
* **PDF & Document Viewer**: Integrated document and PDF viewing engine with in-window rendering.

### 3. Integrated Runtimes
* **Interactive Terminal**: Low-latency pseudo-terminal (PTY) session streaming over WebSockets with standard terminal emulation.
* **VS Code-Compatible Runtime**: Remote code editing and workspace development integration.
* **LibreOffice / Office Runtime**: Office document viewing and productivity suite runtime integration.
* **Brave Browser Runtime**: Isolated browser runtime integration for secure browsing sessions.

### 4. Remote Infrastructure & Transfers
* **SSH & SFTP**: Native SSH terminal sessions and SFTP client supporting password, RSA/Ed25519 public keys, encrypted passphrase-protected keys, and jump-host proxying.
* **WebDAV**: Remote WebDAV server client supporting browsing, upload, download, MKCOL, MOVE, and DELETE operations.
* **S3-Compatible Storage**: Object storage connectivity supporting listing, upload, download, and multipart streaming.
* **Server-to-Server Transfers**: Background transfer engine executing direct server-to-server transfers (SFTP ↔ SFTP, S3 ↔ S3, WebDAV ↔ SFTP) without routing intermediary payload data through the local browser client.

### 5. Security & Privilege Separation
* **Non-Root Core Service (`clouddeskd`)**: Refuses to run as root (`UID 0`) and enforces server-side permission validation on all endpoints.
* **Privileged Helper (`cloudesk-privd`)**: Isolated helper communicating over secure Unix domain sockets for minimal, strictly audited Linux administrative actions.
* **Vault Envelope Encryption**: AES-256-GCM envelope encryption for stored credentials and remote keys using master keys and dynamic data encryption keys (DEKs).
* **Tamper-Evident Audit Logging**: Cryptographic SHA-256 hash chain linking all security-critical system events with linear integrity verification and lock-contention backoff.

### 6. Linux Distribution Support & Efficiency
* **Distribution Families**: Validated across 8 major Linux distribution families (Debian, Ubuntu, Fedora, RHEL/Rocky/Alma, Alpine, Arch Linux, openSUSE, Amazon Linux).
* **Resource Footprint**: Minimal idle resource consumption (<30 MB baseline memory) and fast cold-start performance.

---

## v1.0.1-rc.6 — release candidate source (in preparation, not yet tagged)

**Not yet published.** This section describes the source state prepared to
supersede `v1.0.1-rc.5`; it is not a GitHub Release and has no published
binaries yet. This is a release candidate, not a stable release — `v1.0.0`
remains the latest stable tagged release.

`v1.0.1-rc.5` was tagged but failed hosted release verification (see the
section below) before ever reaching publication, so none of its fixes ever
shipped. `v1.0.1-rc.6` carries the same fixes, now with real hosted proof
behind them:

* **A standalone public `curl | bash` installer.** Distro detection,
  service-manager detection, per-distro package/account setup, and the
  systemd/OpenRC unit content are embedded directly in `install.sh`, so the
  one-command public bootstrap has no dependency on a real on-disk checkout
  — the defect that broke `v1.0.1-rc.4`'s published installer.
* **Correct first-run administrator setup over HTTP/2.** The setup screen's
  `Create administrator` button previously failed every request with
  "cross-site request rejected", even from the server's own address,
  because origin validation only recognized HTTP/1.1 requests and every
  modern browser negotiates HTTP/2 by default. Same-origin requests over
  both HTTP/1.1 and HTTP/2 are now accepted; foreign origins, mismatched
  schemes, and mismatched ports are still correctly rejected.
* **A deterministic, hosted-proven release-acceptance harness.** The
  installer and setup-origin fixes above are now verified against a real
  fresh Debian and a real fresh Alpine machine, and against a real
  HTTP/1.1-and-HTTP/2 TLS listener, as a mandatory, fail-closed part of the
  release process itself — before this candidate existed, `v1.0.1-rc.4`'s
  release workflow never actually exercised either code path, which is
  exactly how both defects reached a previous publication undetected.
* **Release attestation is now gated on that acceptance passing.** The
  hosted workflow that produces this project's [GitHub Artifact
  Attestations](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds)
  will refuse to attest a candidate whose installer or setup-origin
  acceptance checks fail, rather than attesting first and finding out later.

### Build provenance (recorded after candidate freeze; not part of the tagged source)

This subsection is a later documentation-only addition and is **not** part of
the source that would be tagged `v1.0.1-rc.6` — the candidate itself is frozen
at commit `b31057cd6b64966b55e1a5239c43ebc9d11311bf`. It records the evidence
gathered for traceability.

An earlier candidate, `3ad559ef20e11c9a02346444bf4370934c5eb607`, was hosted-
verified first ([run
33438325575](https://github.com/ahmed-alxawad/CloudDesk-OS/actions/runs/33438325575),
SUCCESS) but was then superseded: pushing it to `main` also ran ordinary CI,
which surfaced a real, new failure unrelated to release acceptance — a
Playwright-driven test (`csrf_playwright.rs`) found that 28 browser-driven
integration tests were passing `enforce_hsts=true` to a plain-HTTP test
listener, which the new HTTP/2 same-origin fix's scheme check correctly
treated as "expects HTTPS", rejecting each test's own legitimate same-origin
request. This was a **test-harness defect, not a product defect** — `main.rs`
itself never produces that combination in real deployments — and is fixed in
`b31057cd6b64966b55e1a5239c43ebc9d11311bf`. No release-relevant source
(anything under `crates/`, `services/*/src`, `Cargo.lock`, or the npm
lockfile) differs between the two candidates, confirmed by diff and by
rebuilding `clouddeskd` from the new candidate and getting the identical
hash below.

* **Hosted non-release acceptance on the final candidate**: [run
  33444726164](https://github.com/ahmed-alxawad/CloudDesk-OS/actions/runs/33444726164)
  (workflow "Release acceptance check (non-release)"), triggered by and run
  against commit `b31057cd6b64966b55e1a5239c43ebc9d11311bf` exactly —
  **SUCCESS**. Installer/unit sync, Debian and Alpine true-stdin bootstrap,
  and all 8 setup-origin acceptance tests (same-origin HTTP/1.1, same-origin
  HTTP/2, localhost, a full same-origin bootstrap that creates a real
  administrator, foreign-origin rejection, scheme-mismatch rejection,
  wrong-port rejection, malformed-`Origin` rejection) all passed on a real
  GitHub Actions runner.
* **Hosted ordinary CI on the final candidate**: [run
  33444726136](https://github.com/ahmed-alxawad/CloudDesk-OS/actions/runs/33444726136)
  — `csrf_cross_origin_fetch_with_credentials_is_rejected` now passes; the
  only remaining failures are the pre-existing, unrelated, already-documented
  Playwright-browser-binary infrastructure gap
  (`task_1_browser_test_infrastructure_works`,
  `task_21_office_failure_states_disabled_and_unavailable`), unchanged from
  this project's established baseline.
* **glibc** (two independent clean builds, byte-identical, highest required
  `GLIBC_2.34`): `clouddeskd`
  `c398a5889cfaa6d1f3552abc443ee8412c5ca2e608abc6e895758c100ed640bd`,
  `cloudesk-privd`
  `802ff237efd92c020c128dafe4a37bf4d5ea6e99c8b7147e996fd4b4391ab459`,
  `cloudesk-sessiond`
  `2b2c73200408179bd9800d44de6cd4e3eadc5cb9a45da191483dd068a25a87a4`.
* **musl** (two independent clean builds, byte-identical, static-pie, no
  dynamic dependencies): `clouddeskd`
  `2321eeabba2fb8303a63e38dc5d637d2244d46daa9ebc8ae2d50bc6c8dc0c33c`,
  `cloudesk-privd`
  `bc3816f6dc9edd99fce4b20a7187e879f9e7e3e5cdcf38be778fa211f65473b3`,
  `cloudesk-sessiond`
  `ad53217f3def44cce6a7840668ca0bac841fb4ff1909e7ebe6e3d17d3a1dc090`.
* **Web bundle** (two independent clean builds, byte-identical):
  `clouddesk-web.tar.gz`
  `72394e82826ff12e3e92c61939af31365665d0089b1263a6e4459d481f6787df`.
* **Installer**: `install.sh`
  `1a881a296fbedb25f9f9f4186b3d89e66b8cfa925e1aaf492fe47f556997abee` (unchanged
  since the fix landed — same bytes verified hosted above).
* **SBOM**: 464 components (442 Rust + 22 npm production), unchanged from the
  established baseline.
* All local workflow-equivalent replay steps (staging validator, attestation
  coverage 11/11, negative controls for missing platform checksums / wrong
  version / wrong source commit / missing artifact, public installer security
  negatives) passed against this exact candidate.

## v1.0.1-rc.5 — tagged, failed hosted verification, never published

**`v1.0.1-rc.5` is tagged (`git tag v1.0.1-rc.5`, target
`617ab0a107f114ef30086894d7f1773e41e18501`) but was never published as a
GitHub Release and has zero published binaries or attestations.** Its hosted
release workflow run
([33429519775](https://github.com/ahmed-alxawad/CloudDesk-OS/actions/runs/33429519775))
completed the real-source build, staging, manifest, SBOM, staging
validation, attestation-coverage check, and installer/unit-synchronization
check, then failed at the true-stdin installer bootstrap acceptance step on
the hosted GitHub Actions runner — both the Debian and Alpine acceptance
containers finished their package install and then produced no further
output before exiting non-zero. The workflow correctly fail-closed: neither
the setup-origin acceptance step nor artifact attestation ever ran, so **rc.5
has zero attestations and was never a candidate for publication.**

The cause is currently classified as a **hosted CI acceptance-harness
failure** (the acceptance script's use of `docker run --network host` not
behaving the same way on that runner as in local testing), not a confirmed
defect in `v1.0.1-rc.5`'s actual product source — the fixes described below
were independently verified correct via extensive local real-container,
real-TLS testing before the hosted run. That harness has since been rewritten
(a `main`-only source change, not part of the immutable `v1.0.1-rc.5` tag) to
run each acceptance container's HTTP fixture inside the container itself
instead of depending on host networking.

Per this project's tag-immutability policy, `v1.0.1-rc.5` will not be moved,
deleted, recreated, or re-tagged now that it has been pushed — even though it
failed. Any further correction, hosted-verified or not, will ship as
`v1.0.1-rc.6`.

`v1.0.1-rc.4` was published as a
[GitHub prerelease](https://github.com/ahmed-alxawad/CloudDesk-OS/releases/tag/v1.0.1-rc.4)
but its one-command `curl | bash` installer did not work as published:
running the documented command against a real fresh machine failed
immediately (`bash: line 23: .../lib/distro.sh: No such file or
directory`), because `install.sh` depended on sibling files
(`installer/lib/distro.sh`, `installer/lib/<distro>.sh`,
`packaging/systemd/*`, `packaging/openrc/*`) from a real on-disk checkout
that a `curl | bash` pipe never has. `v1.0.1-rc.4`'s published binaries,
web bundle, and attestations were and remain unaffected and valid — only
its `install.sh` bootstrap script was broken.

`v1.0.1-rc.5` fixes this: distro detection, service-manager detection,
per-distro package/account setup, and the systemd/OpenRC unit content are
now embedded directly in `install.sh`, so the public bootstrap has no
on-disk dependency beyond the script's own bytes. A test
(`tests/distro/installer-lib-sync.sh`) keeps the embedded unit content
byte-synchronized with the canonical `packaging/` files, and a corrected
`tests/distro/remote-fetch.sh` now genuinely pipes the installer through
stdin from an empty directory (matching real `curl | bash` execution)
instead of executing it as a file path from inside a checkout, which is
what let the original defect through undetected. Real clean-room `curl |
sudo env CLOUDESK_VERSION=... bash` bootstraps against fresh, disposable
Debian and Alpine machines — no checkout, no pre-staged files — both pass.
Local/offline installation from a source checkout continues to work
unchanged.

A second, independent defect was found on a real `v1.0.1-rc.4` install
after the installer fix above: the first-run setup screen (`Create
administrator`) failed every request with `cross-site request rejected`,
even for a genuine same-origin request from the server's own address.
Browsers negotiate HTTP/2 whenever a server offers it, and HTTP/2 carries
its request authority in the `:authority` pseudo-header rather than a
`Host` header — this server's origin-validation check looked only at
`Host`, so it saw no host at all for any HTTP/2 request and rejected
every one, including legitimate first-run setup. `v1.0.1-rc.5` derives
the effective request authority from the request itself first (covering
HTTP/2) and falls back to `Host` (ordinary HTTP/1.1), and additionally
now rejects an `Origin` whose scheme doesn't match the deployment's own
(e.g. `http://` against an HTTPS server), which the previous check never
compared at all. Foreign origins, mismatched ports, and malformed
`Origin` headers are still rejected exactly as before — this closes a
false-negative (legitimate requests wrongly blocked), not a security
control. A new test suite
(`services/clouddeskd/tests/setup_origin_https.rs`) exercises a real TLS
listener over both HTTP/1.1 and HTTP/2, including a full same-origin
bootstrap that creates an administrator end to end.

The hosted release workflow (`.github/workflows/release-attest.yml`) now
also runs both regressions — the installer stdin-bootstrap acceptance and
this setup-origin acceptance — before it will produce any artifact
attestation, closing the gap that let both defects reach a previous
published prerelease undetected.

## v1.0.1-rc.4 — release candidate (current published prerelease)

**This is a release candidate, not a stable release.** `v1.0.0` remains
the latest stable tagged release. `v1.0.1-rc.4` is published as a
[GitHub prerelease](https://github.com/ahmed-alxawad/CloudDesk-OS/releases/tag/v1.0.1-rc.4)
and remains the latest *published* prerelease until `v1.0.1-rc.5` is
tagged and released; see the section above for why it is being
superseded.

**Known defect: the one-command curl\|bash installer does not work as
published.** Running the documented command against a real fresh machine
fails immediately (`bash: line 23: .../lib/distro.sh: No such file or
directory`) — the script assumed sibling files from a real on-disk
checkout that a `curl | bash` pipe never has. The `v1.0.1-rc.4` binaries,
web bundle, and attestations are unaffected and remain valid; only the
`install.sh` bootstrap script itself is broken. This is fixed in source
on `main` and will ship as `v1.0.1-rc.5` once tagged and released. In
the meantime, download `v1.0.1-rc.4` release assets manually rather than
via the one-command installer, or build from source.

**Known defect: first-run administrator setup fails with "cross-site
request rejected".** On a machine where the installer defect above has
been worked around, the setup screen's `Create administrator` button
fails even for a genuine same-origin request, because `v1.0.1-rc.4`'s
origin validation only recognizes HTTP/1.1 requests and every modern
browser negotiates HTTP/2 against it by default. See the `v1.0.1-rc.5`
section above for the root cause and fix. There is no workaround for
`v1.0.1-rc.4` itself short of forcing a client to use HTTP/1.1.

### Security fixes since v1.0.0

* An unauthenticated-adjacent authorization gap: a system-status endpoint
  was reachable by any logged-in user, including Guest, without the
  capability check its sibling administration endpoints already had.
* **Critical**: the SSH client previously accepted *any* host key
  unconditionally, meaning a machine-in-the-middle or a replaced remote
  host would be silently trusted on every SSH/SFTP transfer or terminal
  connection. Connections now reject a host key that doesn't match the
  one pinned when the remote server was first saved.
* SFTP uploads could never create a new file on the remote server (only
  overwrite an existing one) — fixed.
* SFTP directory listing failed against most real-world (non-chrooted)
  SFTP servers — fixed.
* WebDAV connections previously skipped TLS certificate verification
  entirely — fixed.
* Two vulnerable/unnecessary dependencies were removed or updated
  (an XML-parsing denial-of-service issue, and an unused dependency
  pulling in a vulnerable crate).

### Platform and installation

* **Native musl build for Alpine Linux**, alongside the existing glibc
  build covering Debian, Ubuntu, Fedora, the RHEL family (RHEL, Rocky,
  AlmaLinux), and Arch.
* **Public one-command installer** — the intent, verified against a real
  fixture, but **broken as published in this candidate's `install.sh`
  asset** when actually piped through stdin on a fresh machine; see the
  known-defect note above. Fixed on `main`, shipping in `v1.0.1-rc.5`.
  The installer's checksum/version verification logic itself (once the
  bootstrap issue is fixed) fails closed on any mismatch rather than
  installing an unverified binary.
* **Reproducible, attested release artifacts**: every published binary,
  the installer itself, and the web frontend bundle are built from an
  exact, immutable tagged source commit and cryptographically signed via
  [GitHub Artifact Attestations](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds).
  Verify any downloaded file yourself:
  ```sh
  gh attestation verify <downloaded-file> --repo ahmed-alxawad/CloudDesk-OS
  ```
* Root `LICENSE` file added (`AGPL-3.0-or-later`, the project's
  established Community license).

### Known limitations

* This project's automated test suite has one known gap unrelated to the
  release itself: browser end-to-end tests do not yet run in ordinary CI
  because the CI runner doesn't have browser binaries installed. This
  does not affect the release build or published artifacts.
* SELinux enforcing mode, true reboot persistence, and a fully subscribed
  RHEL 9 environment have not been exercised in this project's own test
  environment.

No application features were added since `v1.0.0` — this release is
exclusively security fixes, platform/installer work, and release
infrastructure.
