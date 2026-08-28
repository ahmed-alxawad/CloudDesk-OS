# Phase 10 — Distro Installer/Service Matrix

Status: **PARTIAL**. Two passes so far:

- **10A**: harness foundation, Ubuntu 24.04 (Debian family), Fedora 41 (RPM
  family). Found and fixed four defects that would have broken installation
  on every distro (three installer/packaging, one product-level TLS crash).
  Also found that the host-built release binary structurally cannot run on
  RHEL9-family systems (glibc mismatch) -- a release build-baseline problem,
  not an installer defect, deferred to the next pass.
- **10B** (this pass): built a glibc-portable release artifact and used it to
  execute Rocky Linux 9, AlmaLinux 9, and RHEL (via UBI9) for real, plus
  re-verified Ubuntu 24.04 and Fedora 41 against the SAME artifact.

All statuses below are from real execution -- the actual `installer/install.sh`
and `installer/uninstall.sh`, run as root inside a disposable container
booted with real `systemd` as PID 1, never a shell-branch/manifest check.
See `tests/distro/README.md` for how to reproduce every row.

## Local vs. remote-fetch install

The v1 product requirement (`Architecture/CloudDesk-OS-spec/GOAL.md` G1) is:

```sh
curl -fsSL <official-install-url> | sudo bash
```

No such URL is published yet. Every result below is **LOCAL INSTALLER
EXECUTION** -- the checked-out `installer/install.sh` run directly inside
the harness, byte-identical to what a real fetch would run once a URL
exists. The real remote-fetch contract itself remains untested and must
not be inferred from this.

## Declared v1 version matrix

No exact release-version policy existed anywhere in the spec before this
pass (`GOAL.md`/`MISSION.md`/`PLAN.md` name distro *families* only). Defined
here, against real, live-verified base images/UBI (never assumed from
memory) as of this pass's date:

| Distro | Version | glibc (confirmed live) |
| --- | --- | --- |
| Debian | 12 (bookworm, oldstable) | 2.36 |
| Debian | 13 (trixie, stable) | 2.41 |
| Ubuntu | 24.04 LTS (Noble) | 2.39 |
| RHEL | 9 (confirmed via UBI9, see below) | 2.34 |
| Fedora | 41 | 2.40 |
| Rocky Linux | 9 | 2.34 |
| AlmaLinux | 9 | 2.34 |
| Arch Linux | current (rolling) | 2.44 |
| Alpine Linux | 3.20 | musl, not glibc -- a different libc family entirely, own build required |

**Oldest glibc across every glibc-family target: 2.34** (RHEL9/Rocky9/Alma9
generation), exactly the candidate class this pass's own governing
instructions expected, verified rather than assumed.

## Portable release artifact (Phase 10B)

Built by `packaging/build-release.sh`, which drives
`packaging/docker/release-builder.Dockerfile` -- Rocky Linux 9 (glibc 2.34,
the floor above), the same Rust toolchain version this repo's builds have
been using (`rustc 1.97.1`; **no `rust-toolchain.toml` exists in this repo,
which is real reproducibility debt** -- the builder takes the version as a
parameter rather than silently inventing a pin). No native dependencies
beyond glibc/libgcc/libm are dynamically linked (confirmed via `ldd`: no
OpenSSL, no dynamic SQLite -- both statically linked in, via `rustls` and
`libsqlite3-sys`'s bundled build respectively).

| Binary | SHA256 | Highest GLIBC symbol |
| --- | --- | --- |
| `clouddeskd` | `749721c39c86ff8a07c5ff20220194c741e19a3e567810a867fb2347fd39a578` | `GLIBC_2.34` |
| `cloudesk-privd` | `9199a8717aaa22f87938c502ea2974b395664a5f178cfb238a05dc1a84702369` | `GLIBC_2.34` |
| `cloudesk-sessiond` | `54297a0c144f5fa4861ec85e52a20a051bb8bc15a0160029d68cc6f7c6533d86` | `GLIBC_2.34` |

Reproducibility verified live: a from-scratch second run of
`packaging/build-release.sh` produced **byte-identical** SHA256 hashes for
all three binaries.

**Negative control** (Part 25): the OLD host-built artifact
(`GLIBC_2.39`/`GLIBC_2.38` required, this build host's glibc is 2.43)
against Rocky Linux 9 still fails exactly as Phase 10A found:
`/lib64/libc.so.6: version 'GLIBC_2.39' not found`. The portable artifact
loads and executes cleanly on the same host image (reaching the
application's own root-refusal logic, never a loader error) -- proving the
build baseline change is the actual fix, not something else.

**Every glibc-family distro test below now consumes this SAME artifact** --
no per-distro builds, matching the default one-artifact-per-architecture
release contract.

## Distro results

| Distro | Version | Arch | Harness | Service mgr | Install | Service | HTTPS | SQLite | Reinstall | Persistence | SELinux | Cgroup | Uninstall | Reboot | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Ubuntu | 24.04 (Noble) | x86_64 | Docker, real systemd 255 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | N/A | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| Fedora | 41 | x86_64 | Docker, real systemd 255 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | BLOCKED BY ENVIRONMENT | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| Rocky Linux | 9.3 (Blue Onyx) | x86_64 | Docker, real systemd 252 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | BLOCKED BY ENVIRONMENT | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| AlmaLinux | 9.8 (Olive Jaguar) | x86_64 | Docker, real systemd 252 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | BLOCKED BY ENVIRONMENT | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| RHEL (via UBI9) | 9.8 (Plow) | x86_64 | Docker, real systemd 252 PID 1 (genuine `@rhel-9-for-x86_64-baseos-rpms` content), cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | BLOCKED BY ENVIRONMENT | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS (see RHEL caveat below)** |
| Debian | 12 / 13 | x86_64 | -- | -- | NOT EXECUTED | | | | | | | | | | **NOT EXECUTED** |
| Arch Linux | current | x86_64 | -- | -- | NOT EXECUTED | | | | | | | | | | **NOT EXECUTED** |
| Alpine Linux | 3.20 | x86_64 | -- | -- | NOT EXECUTED (OpenRC path entirely unexercised) | | | | | | | | | | **NOT EXECUTED** |

### RHEL caveat -- read before trusting this row

`registry.access.redhat.com/ubi9/ubi:latest` is Red Hat's own official,
publicly redistributable RHEL 9 userspace (no subscription required for its
`ubi-9-baseos-rpms`/`ubi-9-appstream-rpms`/`ubi-9-codeready-builder-rpms`
repos) -- its `systemd` package is literally `systemd-252-67.el9_8.4`
`@rhel-9-for-x86_64-baseos-rpms`, genuine Red Hat-built content, not a
third-party rebuild. Every part of the real lifecycle this pass's own
governing instructions require -- install, service identity/hardening,
HTTPS, SQLite, restart, reinstall idempotence with data preservation,
uninstall -- ran successfully against it, independent of the Rocky/Alma
results.

**This is not the same as a fully subscribed, entitled RHEL installation.**
UBI does not represent: `subscription-manager` registration state, the
full RHEL entitlement repo set (only the free UBI subset was used here),
RHEL's own default SELinux/firewalld posture on a real installation medium,
or a true VM boot (see Reboot below). Treat this row as strong, genuine,
Red-Hat-sourced evidence that the installer's actual logic works
correctly against real RHEL 9 content -- not as equivalent to testing a
customer's real subscribed RHEL 9 server.

## Cross-distro differences (all five real executions)

- **Package manager**: `apt-get`/`dpkg` (Ubuntu, verified) vs. `dnf` (Fedora,
  Rocky, AlmaLinux, RHEL/UBI9 -- all four resolved the exact same
  `installer/lib/rhel.sh`/`fedora.sh` package list with **no name
  discrepancies observed** across Rocky/Alma/RHEL specifically, closing the
  open question Phase 10A left).
- **glibc baseline**: now irrelevant to installer behavior -- the portable
  artifact satisfies every target's floor by construction.
- **systemd version**: 255 (Ubuntu, Fedora) vs. 252 (Rocky, AlmaLinux,
  RHEL/UBI9) -- no version-specific installer behavior observed anywhere.
- **Harness-only package conflict** (not a product/installer issue): a bare
  `dnf install curl` conflicts with Rocky/AlmaLinux/UBI9's preinstalled
  `curl-minimal` package; the test harness Dockerfiles use
  `--allowerasing` to resolve this. `installer/install.sh` itself never
  installs `curl` at all and was unaffected.
- **SELinux**: `getenforce` = `Disabled` inside all three RHEL9-family
  harness containers (the host kernel is `Enforcing`, but a Docker
  container does not carry independent SELinux file-context labeling
  without additional host configuration this pass did not set up). Genuine
  enforcing-mode behavior remains untested -- `packaging/selinux/` is still
  an empty placeholder directory, no policy module shipped.
- **Firewall**: `installer/install.sh` does not touch `firewalld`/`ufw` at
  all (confirmed by direct inspection -- no reference anywhere in the
  installer or its `lib/*.sh`). External network exposure is intentionally
  left to the administrator/reverse-proxy, per the product's own design.
  This pass's harnesses reach the service over loopback, which does not
  exercise `firewalld` even where installed, so real external reachability
  through a firewalled host's rules remains genuinely untested.

## Defects found and fixed (Phase 10A, reconfirmed still correct on Rocky/Alma/RHEL this pass)

All four were re-verified live on Rocky Linux 9, AlmaLinux 9, and RHEL
(UBI9) this pass -- correct identity/hardening, correct `/etc/clouddesk`
traversal, correct `/run/clouddesk` precreation, correct `hostname`
fallback, correct TLS startup, no crash loops, no world-readable secrets,
on all three:

1. `/etc/clouddesk` directory ownership (`installer/install.sh`, `4d2d5ef`).
2. Missing `hostname` command on RPM-family minimal installs
   (`installer/install.sh`, `0aa5e76`).
3. `cloudesk-privd.service` mount-namespace ordering
   (`packaging/systemd/cloudesk-privd.service`, `6c081dd`).
4. No process-wide rustls `CryptoProvider` (`services/clouddeskd/src/main.rs`,
   `b535826`).

No new installer/packaging defects were found this pass -- the same four
fixes that made Ubuntu/Fedora work made Rocky/AlmaLinux/RHEL(UBI9) work too,
on the first real attempt with the portable artifact.

## Service identity / privilege (all five distros)

Identical everywhere, confirmed live via `systemctl show`, not read from the
unit file alone: `clouddesk.service` runs `User=clouddesk Group=clouddesk`,
empty `CapabilityBoundingSet=`/`AmbientCapabilities=`, `NoNewPrivileges=yes`,
`ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes`. **Main service
never runs as root, on any tested distro.**

## Artifact integrity debt (preserved from Phase 10A, still open)

`curl -fsSL <official-install-url> | sudo bash` relies on TLS-transport
trust only -- no separate checksum or signature verification of the
install script itself. No official URL is published yet, so
**remote-fetch integrity acceptance: NOT EXECUTED / UNAVAILABLE** (no
artifact exists to verify). The installer itself downloads no
binaries -- OS packages come from each distro's own signed-repo trust; the
release binaries come from this local/CI-produced portable artifact. This
release-hardening requirement (script integrity for the eventual public
`curl | bash` flow) remains a real, undischarged item for whenever
publishing begins.

## Storage / build provenance

Builder: `clouddesk-release-builder:rocky9` (`packaging/docker/release-builder.Dockerfile`),
built from `rockylinux:9`. Rust `1.97.1` via `rustup`, `--profile minimal`.
Build command: `packaging/build-release.sh` →
`cargo build --release -p clouddeskd -p cloudesk-privd -p cloudesk-sessiond`
inside the builder, output copied to `dist/portable-x86_64-glibc/`
(gitignored -- binaries are never committed).

## Next Phase 10 pass

- Execute Debian (12 and/or 13) directly -- still inferred from nothing
  this pass, `distro_family=debian` shared with Ubuntu notwithstanding.
- Execute Arch Linux and Alpine Linux (Alpine needs the entirely
  unexercised OpenRC path, and its own musl build -- this portable glibc
  artifact does not apply to it).
- A genuinely SELinux-enforcing host or VM to close the SELinux gap that
  now spans four distros (Fedora, Rocky, AlmaLinux, RHEL/UBI9) identically.
- A true VM-based reboot test to close the reboot-persistence gap.
- Multi-arch: **only x86_64 has any executable evidence in this project so
  far.** aarch64/arm64 support is not implied by anything tested here and
  is not claimed.
- Establish and test the real `curl -fsSL <url> | sudo bash` remote-fetch
  path once an official URL exists, including artifact-integrity hardening
  for that fetch (see artifact integrity debt above).
- Consider adding a real `rust-toolchain.toml` pin to close the
  reproducibility debt this pass had to work around by parameter instead.

**PHASE 10: PARTIAL.**
