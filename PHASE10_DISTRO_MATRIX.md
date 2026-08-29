# Phase 10 — Distro Installer/Service Matrix

Status: **PARTIAL**. Three passes so far:

- **10A**: harness foundation, Ubuntu 24.04 (Debian family), Fedora 41 (RPM
  family). Found and fixed four defects that would have broken installation
  on every distro (three installer/packaging, one product-level TLS crash).
  Also found that the host-built release binary structurally cannot run on
  RHEL9-family systems (glibc mismatch) -- a release build-baseline problem,
  not an installer defect, deferred to the next pass.
- **10B**: built a glibc-portable release artifact and used it to execute
  Rocky Linux 9, AlmaLinux 9, and RHEL (via UBI9) for real, plus re-verified
  Ubuntu 24.04 and Fedora 41 against the SAME artifact.
- **10C** (this pass): pinned the release builder (exact base-image digest,
  explicit default Rust version), proved two from-scratch builds byte-
  identical, and used the same one artifact to execute Debian 12, Debian 13,
  and Arch Linux for real. Found and fixed one real, distro-specific
  security defect on Arch (below). Also corrected this document's RHEL
  wording, which had been conflating the genuine UBI9 compatibility
  evidence with full RHEL acceptance.

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

## Portable release artifact (Phase 10B, pinned in 10C)

Built by `packaging/build-release.sh`, which drives
`packaging/docker/release-builder.Dockerfile`:

- Base image: **`rockylinux@sha256:d7be1c094cc5845ee815d4632fe377514ee6ebcf8efaed6892889657e5ddaaa6`**
  (Phase 10C: pinned to this exact digest, not the floating `rockylinux:9`
  tag, which will eventually point at a newer point release with a newer
  glibc and silently raise this builder's compatibility floor).
- Rust: **`1.97.1`**, now the Dockerfile's own default (`ARG
  RUST_VERSION=1.97.1`), still overridable via `--build-arg`. **No
  `rust-toolchain.toml` exists anywhere in this repo -- that remains real
  reproducibility debt**; this default is the closest available substitute,
  not a fix for the underlying gap.
- No native dependencies beyond glibc/libgcc/libm are dynamically linked
  (confirmed via `ldd`: no OpenSSL, no dynamic SQLite -- both statically
  linked in, via `rustls` and `libsqlite3-sys`'s bundled build
  respectively).

| Binary | SHA256 | Highest GLIBC symbol |
| --- | --- | --- |
| `clouddeskd` | `749721c39c86ff8a07c5ff20220194c741e19a3e567810a867fb2347fd39a578` | `GLIBC_2.34` |
| `cloudesk-privd` | `9199a8717aaa22f87938c502ea2974b395664a5f178cfb238a05dc1a84702369` | `GLIBC_2.34` |
| `cloudesk-sessiond` | `54297a0c144f5fa4861ec85e52a20a051bb8bc15a0160029d68cc6f7c6533d86` | `GLIBC_2.34` |

**Identical to the Phase 10B artifact** -- same hashes, unchanged this pass.

Reproducibility verified live **twice**: a from-scratch second run in
Phase 10B, and again in Phase 10C after pinning the base-image digest and
the Rust version default (that second Phase 10C run additionally started
from a fully pruned Docker build cache and a deleted builder image, forcing
a genuine cold rebuild) -- all runs produced **byte-identical** SHA256
hashes for all three binaries.

**Negative control** (Part 25): the OLD host-built artifact
(`GLIBC_2.39`/`GLIBC_2.38` required, this build host's glibc is 2.43)
against Rocky Linux 9 still fails exactly as Phase 10A found:
`/lib64/libc.so.6: version 'GLIBC_2.39' not found`. The portable artifact
loads and executes cleanly on the same host image (reaching the
application's own root-refusal logic, never a loader error) -- proving the
build baseline change is the actual fix, not something else.

**Every glibc-family distro test below now consumes this SAME artifact** --
no per-distro builds, matching the default one-artifact-per-architecture
release contract. Confirmed by hash on every distro tested this pass
(installed `/opt/clouddesk/bin/*` binaries' SHA256 checked directly inside
the running container, not merely assumed from the mount): identical to
the table above on Debian 12, Debian 13, and Arch Linux, exactly as it was
on Ubuntu/Fedora/Rocky/AlmaLinux/UBI9 in Phase 10B.

## Distro results

| Distro | Version | Arch | Harness | Service mgr | Install | Service | HTTPS | SQLite | Reinstall | Persistence | SELinux | Cgroup | Uninstall | Reboot | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Ubuntu | 24.04 (Noble) | x86_64 | Docker, real systemd 255 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | N/A | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| Fedora | 41 | x86_64 | Docker, real systemd 255 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | BLOCKED BY ENVIRONMENT | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| Rocky Linux | 9.3 (Blue Onyx) | x86_64 | Docker, real systemd 252 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | BLOCKED BY ENVIRONMENT | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| AlmaLinux | 9.8 (Olive Jaguar) | x86_64 | Docker, real systemd 252 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | BLOCKED BY ENVIRONMENT | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| **RHEL 9 (full installer/service acceptance)** | 9 | x86_64 | -- | -- | **UNAVAILABLE** -- no genuine subscribed/entitled full RHEL 9 environment was executed. See the UBI9 compatibility-control row immediately below for what evidence *does* exist, and do not read it as a substitute for this row. | | | | | | | | | | **UNAVAILABLE** |
| RHEL UBI9 (compatibility control) | 9.8 (Plow) | x86_64 | Docker, real systemd 252 PID 1 (genuine `@rhel-9-for-x86_64-baseos-rpms` content), cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | BLOCKED BY ENVIRONMENT | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS (compatibility control only -- see below)** |
| Debian | 12 (bookworm, `sha256:6ebd97fa...`) | x86_64 | Docker, real systemd PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | N/A | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| Debian | 13 (trixie, `sha256:f324c7ff...`) | x86_64 | Docker, real systemd PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | N/A | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| Arch Linux | rolling, `VERSION_ID=20260823.0.578598` (`archlinux@sha256:b860afd5...`) | x86_64 | Docker, real systemd PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS\* | PASS | N/A | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS\*** |
| Alpine Linux | 3.20 | x86_64 | -- | -- | NOT EXECUTED (OpenRC path entirely unexercised) | | | | | | | | | | **NOT EXECUTED** |

\* Arch's first real run found a genuine security defect (below), fixed
and re-verified live on the same distro before this PASS was recorded.

### RHEL: two separate claims, not one -- read before trusting either row

**RHEL 9 full installer/service acceptance: UNAVAILABLE.** No genuinely
subscribed, entitled RHEL 9 environment (real registration state, the full
RHEL entitlement repo set, RHEL's own default installation-medium posture)
has been executed against this installer. This is the honest status for
"does CloudDesk install and run on a customer's real RHEL 9 server" -- it
is not answered by anything below, and must not be inferred from the
Rocky/AlmaLinux/UBI9 results.

**RHEL UBI9 compatibility control: PASS**, and this evidence is real, not
discarded: `registry.access.redhat.com/ubi9/ubi:latest` is Red Hat's own
official, publicly redistributable RHEL 9 userspace (no subscription
required for its `ubi-9-baseos-rpms`/`ubi-9-appstream-rpms`/
`ubi-9-codeready-builder-rpms` repos) -- its `systemd` package is literally
`systemd-252-67.el9_8.4` `@rhel-9-for-x86_64-baseos-rpms`, genuine
Red-Hat-built content, not a third-party rebuild. Every part of the real
lifecycle this pass's own governing instructions require -- install,
service identity/hardening, HTTPS, SQLite, restart, reinstall idempotence
with data preservation, uninstall -- ran successfully against it,
independent of the Rocky/Alma results.

**This control does not stand in for the UNAVAILABLE row above.** UBI does
not represent: `subscription-manager` registration state, the full RHEL
entitlement repo set (only the free UBI subset was used here), RHEL's own
default SELinux/firewalld posture on a real installation medium, or a true
VM boot (see Reboot below). Read it as strong, genuine, Red-Hat-sourced
evidence that the installer's actual logic is compatible with real RHEL 9
content at the package/glibc/systemd level -- never as equivalent to
testing a customer's real subscribed RHEL 9 server, and never cited as
"RHEL 9: PASS" on its own.

## Cross-distro differences (all eight real executions)

- **Package manager**: `apt-get`/`dpkg` (Ubuntu, Debian 12, Debian 13,
  verified) vs. `dnf` (Fedora, Rocky, AlmaLinux, RHEL/UBI9) vs. `pacman`
  (Arch) -- **no package-name discrepancies observed anywhere** across all
  three families for `installer/lib/*.sh`'s dependency lists.
- **glibc baseline**: irrelevant to installer behavior -- the portable
  artifact satisfies every target's floor by construction (confirmed by
  matching installed-binary SHA256 on every distro, not merely assumed).
- **systemd version**: 255 (Ubuntu, Fedora) vs. 252 (Rocky, AlmaLinux,
  RHEL/UBI9) vs. Debian 12/13's own bundled versions vs. Arch's rolling
  current -- no version-specific installer behavior observed anywhere.
- **`runuser`'s PAM-driven umask behavior differs by distro family** -- see
  the Arch defect below. This is the one genuine, distro-specific installer
  behavior difference found across all eight distros tested so far.
- **`systemd-modules-load.service`/`systemd-firstboot.service`** fail inside
  a plain Docker container on Debian 13 and Arch respectively (kernel
  module loading and first-boot locale/timezone setup are both meaningless
  inside a container, unrelated to CloudDesk) -- masked in the harness
  Dockerfiles only, so `systemctl is-system-running` stays a meaningful
  signal for genuine failures. Not present on Ubuntu/Fedora/Rocky/Alma/UBI9
  in earlier passes; a systemd-version/default-unit-set difference, not an
  installer concern.
- **Harness-only package conflict** (not a product/installer issue): a bare
  `dnf install curl` conflicts with Rocky/AlmaLinux/UBI9's preinstalled
  `curl-minimal` package; the test harness Dockerfiles use
  `--allowerasing` to resolve this. `installer/install.sh` itself never
  installs `curl` at all and was unaffected.
- **SELinux**: `getenforce` = `Disabled` inside all four RHEL9-family
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

## Defects found and fixed

### Phase 10A (reconfirmed still correct on all eight distros through this pass)

Correct identity/hardening, correct `/etc/clouddesk` traversal, correct
`/run/clouddesk` precreation, correct `hostname` fallback, correct TLS
startup, no crash loops, on every distro tested across all three passes:

1. `/etc/clouddesk` directory ownership (`installer/install.sh`, `4d2d5ef`).
2. Missing `hostname` command on RPM-family minimal installs
   (`installer/install.sh`, `0aa5e76`).
3. `cloudesk-privd.service` mount-namespace ordering
   (`packaging/systemd/cloudesk-privd.service`, `6c081dd`).
4. No process-wide rustls `CryptoProvider` (`services/clouddeskd/src/main.rs`,
   `b535826`).

### Phase 10C: world-readable database on Arch Linux (new, found and fixed this pass)

**The one new installer defect found this pass, and it is a real,
distro-specific security defect, not a harness artifact.** The SQLite
database is created exactly once, during `installer/install.sh`'s
`runuser -u clouddesk -- clouddeskd migrate` step, and its file mode was
never set explicitly -- every *other* secret this installer creates
(master key, grant key, bootstrap secret) gets an explicit `chmod` right
after creation for exactly this reason, but the database was missed,
relying implicitly on whatever umask `runuser` happened to produce.

`install.sh` sets `umask 077` at its own top. On Debian/Ubuntu/Fedora/RHEL-
family, `runuser` inherits that umask, so the database happened to come out
`0600` anyway. **Confirmed live: on Arch Linux, `runuser`'s own PAM stack
resets the umask to `0022` regardless of the caller's**, producing a
**world-readable `0644`** database -- containing `vault_secrets`,
`sessions`, `recovery_codes`, `user_permissions`, and the rest of the full
product schema -- on every fresh Arch install.

**Fixed** (`installer/install.sh`) with an explicit `chmod 0600` on the
database path immediately after the migrate step, matching the pattern
every sibling secret already used -- not relying on umask propagation
through an OS-specific `runuser`/PAM behavior at all. Verified live,
repeatedly: fresh install now produces `0600 clouddesk:clouddesk` on Arch;
no regression on Debian 12 (still `0600`, now redundantly explicit rather
than umask-dependent).

No other new installer/packaging defects were found this pass -- the same
four Phase 10A fixes plus this one made Debian 12, Debian 13, and Arch
Linux all pass on the first real attempt with the portable artifact.

## Service identity / privilege (all eight distros)

Identical everywhere, confirmed live via `systemctl show`, not read from the
unit file alone: `clouddesk.service` runs `User=clouddesk Group=clouddesk`,
empty `CapabilityBoundingSet=`/`AmbientCapabilities=`, `NoNewPrivileges=yes`,
`ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes`. **Main service
never runs as root, on any tested distro.** Secrets are `0600`/`0640`
everywhere (including the database, now explicitly on all distros after the
Arch fix above); no `chmod 777` anywhere in the installer.

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
built from `rockylinux@sha256:d7be1c094cc5845ee815d4632fe377514ee6ebcf8efaed6892889657e5ddaaa6`
(pinned digest, Phase 10C). Rust `1.97.1` via `rustup`, `--profile minimal`
(now the Dockerfile's own default). Build command: `packaging/build-release.sh` →
`cargo build --release -p clouddeskd -p cloudesk-privd -p cloudesk-sessiond`
inside the builder, output copied to `dist/portable-x86_64-glibc/`
(gitignored -- binaries are never committed).

## Alpine: preparation note only (not tested this pass)

Inspected, not executed, per this pass's own explicit scope boundary:

- **Installer path**: `installer/lib/alpine.sh` -- `apk add --no-cache
  ca-certificates openssh-client-default openssl sqlite util-linux` for
  packages, `adduser -S -D -H -h /var/lib/clouddesk -s /sbin/nologin
  clouddesk` for the service account. Structurally parallel to every other
  family; untested.
- **OpenRC units**: `packaging/openrc/{clouddesk,cloudesk-privd}` already
  exist and already handle the `/run/clouddesk` precreation issue Phase
  10A found on systemd correctly (`checkpath --directory --owner ...` in
  `start_pre()`), but have **never been executed against a real OpenRC
  init** in any pass so far.
- **Native dependencies**: `nix` (used by `crates/linux`, `crates/media`,
  `services/cloudesk-privd`) supports musl targets; nothing in the
  dependency tree is inspected as obviously glibc-only. This is a
  plausibility read from `Cargo.toml`, **not a verified musl build** --
  `rustls`'s `ring`/`aws-lc-rs` backends and `libsqlite3-sys`'s bundled C
  build both need to actually cross-compile cleanly under a musl
  toolchain, which is a real open question, not assumed answered here.
- **The current portable glibc artifact is not expected to be correct for
  Alpine** -- Alpine is musl-based, a different libc family entirely, not
  merely an older glibc. Do not attempt to run it there.

**Next-pass requirement**: decide between a native musl release build
(`x86_64-unknown-linux-musl` target, own builder image, own artifact
table) versus an explicitly supported compatibility model, then execute
the real OpenRC lifecycle -- install, `rc-service` start/stop/restart,
`rc-update add`, HTTPS, SQLite, reinstall, persistence -- the same rigor
every systemd distro already received.

## Next Phase 10 pass

- Execute Alpine Linux for real (see preparation note above) -- decide and
  implement the musl artifact strategy first.
- A genuinely SELinux-enforcing host or VM to close the SELinux gap that
  now spans four distros (Fedora, Rocky, AlmaLinux, RHEL/UBI9) identically.
- A true VM-based reboot test to close the reboot-persistence gap.
- A genuinely subscribed/entitled RHEL 9 environment, if one becomes
  available, to close the RHEL 9 full-acceptance UNAVAILABLE row -- the
  UBI9 compatibility control does not substitute for this.
- Multi-arch: **only x86_64 has any executable evidence in this project so
  far.** aarch64/arm64 support is not implied by anything tested here and
  is not claimed.
- Establish and test the real `curl -fsSL <url> | sudo bash` remote-fetch
  path once an official URL exists, including artifact-integrity hardening
  for that fetch (see artifact integrity debt above).
- Consider adding a real `rust-toolchain.toml` pin to close the
  reproducibility debt this pass had to work around by parameter instead.

**PHASE 10: PARTIAL.**
