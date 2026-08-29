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
| Alpine Linux | 3.20.10 (`alpine@sha256:d9e853e8...`), musl 1.2.5 | x86_64 | Docker (`--init`), real OpenRC (`openrc sysinit`), native musl artifact | OpenRC | PASS | PASS | PASS | PASS | PASS | PASS | N/A | BLOCKED BY ENVIRONMENT / NOT EXECUTED | PASS | BLOCKED BY ENVIRONMENT | **PASS** |

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

No other new installer/packaging defects were found in Phase 10C -- the
same four Phase 10A fixes plus this one made Debian 12, Debian 13, and
Arch Linux all pass on the first real attempt with the portable artifact.

### Phase 10D: two Alpine-specific installer defects (new, found and fixed this pass)

**1. Missing service-account group (`installer/lib/alpine.sh`).**
Unlike `useradd --system` on every other tested distro family
(Debian/RPM/Arch all auto-create a matching same-named primary group),
BusyBox's `adduser -S` on Alpine does **not** -- it silently falls back
to the shared `nogroup` (gid 65533) unless a group is explicitly given
and already exists. Without a fix, every `chown clouddesk:clouddesk`
later in the installer failed with "unknown user/group", aborting
installation outright on every fresh Alpine system. **Fixed** with an
explicit `addgroup -S clouddesk` before `adduser -S ... -G clouddesk
clouddesk`. Verified live: `id clouddesk` shows the correct
`gid=101(clouddesk)` after the fix.

**2. Implicit-parent-directory mode (`installer/install.sh`).**
BusyBox's `install -d -m MODE dir1 dir2` (Alpine's `install`, unlike GNU
coreutils) applies `-m` only to the directories named explicitly on the
command line -- an implicit parent it has to auto-create along the way
instead gets the *calling process's own umask*. `/opt/clouddesk` was
never named on its own, only its `bin`/`web` children, so under this
script's own `umask 077` it came out `0700` root-only on Alpine
specifically -- unreadable by the `clouddesk` service account entirely.
Structurally the same class of bug as the Phase 10A `/etc/clouddesk`
traversal defect, on the one remaining path that was still an implicit
parent. **Fixed** by naming `/opt/clouddesk` itself explicitly in the
`install -d -m 0755` call. Verified live: `runuser -u clouddesk --
/opt/clouddesk/bin/clouddeskd --version` succeeds after the fix (real
`--version` output).

No other new installer/packaging defects were found on Alpine -- the four
Phase 10A fixes, the Phase 10C Arch fix, and these two Alpine fixes made
the full OpenRC lifecycle pass on the next real attempt.

## Service identity / privilege (all nine distros)

Identical everywhere: `clouddesk`/`cloudesk-privd` run as `User=clouddesk
Group=clouddesk`, confirmed live (via `systemctl show` on systemd distros,
via `ps -o pid,user,group,args` on Alpine/OpenRC, not read from the unit
file alone). systemd distros additionally show empty
`CapabilityBoundingSet=`/`AmbientCapabilities=`, `NoNewPrivileges=yes`,
`ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes` (OpenRC has no
equivalent unit-level sandboxing directive; CloudDesk's own privilege
separation -- non-root main service, typed `cloudesk-privd` IPC -- is
identical regardless). **Main service never runs as root, on any tested
distro, including Alpine.** Secrets are `0600`/`0640` everywhere
(including the database, explicit on every distro since the Arch fix, now
confirmed on Alpine too); no `chmod 777` anywhere in the installer; no
world-writable path found on Alpine (`find ... -perm -o+w`: empty).

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

glibc builder: `clouddesk-release-builder:rocky9`
(`packaging/docker/release-builder.Dockerfile`), built from
`rockylinux@sha256:d7be1c094cc5845ee815d4632fe377514ee6ebcf8efaed6892889657e5ddaaa6`
(pinned digest, Phase 10C). Rust `1.97.1` via `rustup`, `--profile minimal`
(now the Dockerfile's own default). Build command: `packaging/build-release.sh` →
`cargo build --release -p clouddeskd -p cloudesk-privd -p cloudesk-sessiond`
inside the builder, output copied to `dist/linux-x86_64-glibc/`
(gitignored -- binaries are never committed).

musl builder (Phase 10D): `clouddesk-release-builder:musl`
(`packaging/docker/release-builder-musl.Dockerfile`), built from
`alpine@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc`
(pinned digest, Alpine 3.20.10). Same Rust `1.97.1`, target
`x86_64-unknown-linux-musl`. Build command: `packaging/build-release-musl.sh` →
`cargo build --release --target x86_64-unknown-linux-musl -p clouddeskd -p
cloudesk-privd -p cloudesk-sessiond` inside the builder, output copied to
`dist/linux-x86_64-musl/` (gitignored -- binaries are never committed).
**The glibc artifact is completely untouched by this** -- separate
Dockerfile, separate builder image, separate output directory; confirmed
byte-identical SHA256 to the Phase 10B/10C glibc artifact after the
`dist/portable-x86_64-glibc` → `dist/linux-x86_64-glibc` rename this pass
(a path-consistency rename only, propagated via `sed` to
`PHASE10_DISTRO_MATRIX.md`, `tests/distro/systemd-lifecycle-test.sh`,
`tests/distro/README.md`, `packaging/build-release.sh` -- no content
change).

## Alpine (Phase 10D: executed for real this pass)

**Core rule honored: no glibc-compatibility shims (gcompat or otherwise)
were installed.** CloudDesk ships one pinned glibc artifact for every
glibc-family distro (unchanged, see above) plus one pinned **native musl**
artifact for Alpine, proven through executable evidence, not assumed from
`Cargo.toml` inspection.

**musl vs. glibc is a different libc family, not an older glibc version.**
Confirmed live: the existing glibc artifact fails to even *load* on
Alpine 3.20 (`exec /clouddeskd: no such file or directory`; `readelf -l`
shows `[Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]`, a
path that does not exist on Alpine at all) -- this is the Part 33
wrong-artifact negative control, and it PASSED (fails exactly as
expected, with no silent partial-load or corruption).

**Native musl artifact -- VIABLE.** Built natively (not cross-compiled)
inside `packaging/docker/release-builder-musl.Dockerfile`
(`alpine@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc`,
Alpine 3.20.10, musl 1.2.5) via `packaging/build-release-musl.sh`, target
`x86_64-unknown-linux-musl`, Rust `1.97.1`. `openssl-sys`/`native-tls` are
present in `Cargo.lock` generally but confirmed via `cargo tree -p
clouddeskd -p cloudesk-privd -p cloudesk-sessiond -e normal` to NOT
actually be pulled into any of the three binaries' real dependency
trees -- TLS is rustls-only, and the only OpenSSL-adjacent crate actually
linked, `openssl-probe`, is pure-Rust CA-path discovery with no C linkage.
`libsqlite3-sys`'s bundled C build and rustls's `ring` `CryptoProvider`
(the Phase 10A `main.rs` fix) both compile and run correctly under musl.

| Binary | SHA256 | Linkage |
| --- | --- | --- |
| `clouddeskd` | `1d34d535a224c0e4452222cbc90baacca3eb3fee193c314f81cf97a9a66213c3` | static-pie, statically linked (`ldd`: "statically linked") |
| `cloudesk-privd` | `1e6c1af6e1c1bceb4ec7c0f2847615918c787149ed79801525053503815c856e` | static-pie, statically linked |
| `cloudesk-sessiond` | `ecfa3e1b7ac0a7494e7a5ee74a32d2a3a30e2a6a67544d79d8620e2a3d9493bd` | static-pie, statically linked |

Reproducibility verified live **twice** (once with a warm Docker layer
cache, once after `docker rmi` + `docker volume rm` +
`docker builder prune -af`, forcing a genuine cold rebuild) --
**byte-identical** SHA256 for all three binaries both times. Confirmed
present at `dist/linux-x86_64-musl/` (gitignored, artifact never
committed, matching the glibc artifact's own convention).

**Installer artifact selection**: `installer/install.sh` now selects
`dist/linux-x86_64-musl` vs. `dist/linux-x86_64-glibc` by the same
already-trusted `distro_family` classification `detect_distribution()`
computes from `/etc/os-release` (or the test-only
`CLOUDESK_DISTRO_ID`/`CLOUDESK_DISTRO_LIKE` overrides) -- never new
arbitrary user-controlled input. Verified for all 8 declared distro
families via `tests/distro/artifact-selection.sh`, comparing the SHA256
of the file the installer actually copied against the expected source
directory: **8/8 PASS**.

**OpenRC lifecycle -- full PASS**, via `tests/distro/openrc-lifecycle-test.sh`
against `tests/distro/openrc-harness.alpine320.Dockerfile`:

- Real install (`installer/install.sh`, `installer/lib/alpine.sh`), apk
  packages (`ca-certificates openssh-client-default openssl sqlite
  util-linux`), OpenRC init scripts installed to `/etc/init.d/`.
- `clouddesk`/`cloudesk-privd` both `rc-update add ... default` and
  confirmed via `rc-update show default` (a real enablement check, not a
  file-existence proxy).
- `rc-service clouddesk start` / `restart` / `stop` / `start` all clean
  `[ ok ]`, no duplicate processes, no zombies (see harness note below).
- Main service confirmed **non-root** via live `ps -o pid,user,group,args`
  (User=clouddesk, Group=clouddesk), matching every systemd distro.
- `/run/clouddesk` correctly `clouddesk:clouddesk 0750` -- OpenRC's
  existing `checkpath --directory --owner ...` in `start_pre()` already
  handled the precreation-ordering issue correctly from the start (unlike
  the systemd unit, which needed the Phase 10A `RuntimeDirectory=` fix).
- `cloudesk-privd` root-owned; `/run/clouddesk/privd.sock` `root:clouddesk
  0660` -- not world-writable.
- HTTPS on port 9870: real 200 response with genuine JSON body, both
  before and after restart and after reinstall.
- TLS material: `server.key` `root:clouddesk 0640`, `server.crt`
  `root:root 0644`.
- SQLite: full schema migrated via `runuser -u clouddesk -- clouddeskd
  migrate`; database `clouddesk:clouddesk 0600` -- confirms the Phase 10C
  Arch-umask fix (explicit `chmod 0600`, not umask-dependent) generalizes
  correctly to BusyBox's own `runuser`/PAM-less environment too.
- All other secrets (`master.key`, `privd-grant.key`, `bootstrap.secret`,
  `privd.env`) correctly `0600`/`0640`; no world-writable path anywhere
  under `/opt/clouddesk`, `/etc/clouddesk`, `/var/lib/clouddesk`,
  `/run/clouddesk` (`find ... -perm -o+w`: empty).
- Reinstall: idempotent, a pre-reinstall data marker survives, TLS/keys
  unchanged.
- Logs (`/var/log/clouddesk/{clouddesk,privd}.log`): clean startup, no
  secrets, no crash loop, `grep -riE "master.?key|grant.?key|bootstrap.
  secret|password"` empty.
- Uninstall correct both with and without `--purge`.
- Cgroup: `/sys/fs/cgroup/cgroup.controllers` present (unified v2 mounted
  in the harness), but no per-service cgroup accounting/limit enforcement
  was exercised -- same **BLOCKED BY ENVIRONMENT / NOT EXECUTED**
  classification as every systemd distro's cgroup row, not a new
  Alpine-specific gap.
- Reboot: containers, not VMs -- same **BLOCKED BY ENVIRONMENT**
  classification as every systemd distro's reboot row.  `rc-update show
  default` (proven working above) is the practical enablement substitute
  this project has used consistently in place of a genuine cold boot.
- Process/PTY sanity: no separate synthetic probe was written; the real
  install/service lifecycle above already exercises TLS handshake,
  SQLite, tokio async threading, process spawning (`runuser`, `migrate`),
  and POSIX signal handling (OpenRC's `start-stop-daemon` SIGTERM on
  `stop`, confirmed clean, no zombie/refused-to-stop) end-to-end -- this
  was treated as the authoritative evidence rather than a duplicate
  synthetic check.
- DNS/network sanity: implicitly exercised by the same live HTTPS
  round-trip above (container DNS/networking functioning is a
  precondition for that response); no separate probe was written for the
  same reason.

**Two real, distro-specific installer defects found and fixed** (see
below). **Security audit: Critical 0, High 0.**

### Alpine-only harness notes (not product defects)

Two environment-setup issues were found and fixed in the *test harness*
itself, not in CloudDesk:

- Alpine ships no installable `openrc-init` package (Gentoo-specific;
  confirmed via `apk search openrc`) -- there is no standard
  "OpenRC-in-Docker full PID1 boot" pattern the way there is for systemd.
  The harness instead runs OpenRC's own real `openrc sysinit` command
  explicitly in its `CMD`, which populates the full
  `/run/openrc/{starting,started,exclusive,daemons,...}` state tree
  `rc-service`/`rc-update` genuinely depend on. A naive `mkdir -p
  /run/openrc; touch softlevel` shortcut was tried first and confirmed
  insufficient (every `rc-service start` spuriously reported "already
  starting").
- A bare `sleep infinity` container PID1 has no subreaper capability, so
  a correctly-SIGTERM-responding `clouddeskd` (confirmed via `ps` showing
  state `Zs`/`<defunct>` -- i.e. it DID exit) went unreaped, making
  `start-stop-daemon`'s stop-confirmation report "1 process refused to
  stop" even though the app behaved correctly. Fixed by adding Docker's
  built-in `--init` (tini, a genuine subreaper) to `docker run`.

## Phase 10 completion analysis (Part 39)

`Architecture/CloudDesk-OS-spec/GOAL.md` G1 declares the release-blocking
OS matrix as exactly: Debian, Ubuntu, RHEL, Fedora, Rocky Linux,
AlmaLinux, Arch Linux, Alpine Linux -- eight families -- and requires "the
installer and CI must test distribution-specific package management and
service-manager behavior." It does not mention SELinux-enforcing mode,
genuine VM-based reboot testing, or install-script integrity/signing as
G1 (or any other G-goal) requirements; G15's "reboot" is an admin-UI
*feature* (the ability to trigger a host reboot from Settings), not a
requirement to test service-survives-reboot via a real cold boot in CI.

Against that standard, all eight declared distro families now have
executable PASS evidence (RHEL via its own official UBI9 redistributable
content as a compatibility control, explicitly not conflated with a
subscribed-RHEL full-acceptance row -- see the RHEL caveat above,
unchanged). **The distro matrix itself is satisfied.**

The four remaining blockers are classified as follows, each against
GOAL.md's actual text, not convenience:

1. **RHEL full subscribed-environment acceptance -- UNAVAILABLE.**
   Coexists with PHASE 10 COMPLETE. G1 requires the *RHEL platform* be
   supported, not that a paid RHEL entitlement be exercised in this
   environment; the UBI9 control uses Red Hat's own real
   `@rhel-9-for-x86_64-baseos-rpms` content and PASSed identically to
   every other systemd-family distro. This is a standing environment
   limitation, documented, not a missing implementation.
2. **SELinux enforcing -- BLOCKED BY ENVIRONMENT.** Coexists with PHASE
   10 COMPLETE. Not named in any G-goal; a defense-in-depth verification
   beyond the stated release matrix, not part of it.
3. **True VM-based reboot -- BLOCKED BY ENVIRONMENT.** Coexists with
   PHASE 10 COMPLETE. Containers, not VMs, on every distro tested,
   Alpine included; G1/G15 do not require a real cold-boot test as a
   release gate, and `systemctl enable`/`rc-update add` enablement
   (proven working everywhere) is the practical substitute this project
   has used consistently.
4. **Remote-fetch (`curl | bash`) script-integrity hardening -- NOT
   EXECUTED / UNAVAILABLE.** Coexists with PHASE 10 COMPLETE for the same
   reason it did in Phase 10A-C: no official install URL is published
   yet, so there is nothing to fetch or verify. This is a real,
   undischarged release-*publication* hardening item (script signing/
   checksum beyond TLS-transport trust), not a distro-matrix gap -- it
   remains open and should block *publishing*, not this engineering pass.

None of the four blockers is mandatory for Phase 10 (distro-matrix)
completion under GOAL.md's actual criteria. **PHASE 10: COMPLETE.**

## Next Phase 10 pass (residual, non-blocking items)

- A genuinely SELinux-enforcing host or VM to close the SELinux gap that
  now spans four distros (Fedora, Rocky, AlmaLinux, RHEL/UBI9) identically.
- A true VM-based reboot test to close the reboot-persistence gap on all
  nine distros.
- A genuinely subscribed/entitled RHEL 9 environment, if one becomes
  available, to close the RHEL 9 full-acceptance UNAVAILABLE row -- the
  UBI9 compatibility control does not substitute for this.
- Multi-arch: **only x86_64 has any executable evidence in this project so
  far.** aarch64/arm64 support is not implied by anything tested here and
  is not claimed.
- Establish and test the real `curl -fsSL <url> | sudo bash` remote-fetch
  path once an official URL exists, including artifact-integrity hardening
  for that fetch (see artifact integrity debt above) -- this blocks
  *publishing*, not Phase 10 engineering completion.
- Consider adding a real `rust-toolchain.toml` pin to close the
  reproducibility debt this pass had to work around by parameter instead.

**PHASE 10: COMPLETE.**

---

## Phase 10E — Final Closure Reconciliation

Reconciliation/documentation-only pass. No product code changed; no
artifacts were rebuilt (evidence was internally consistent, so no cause
to rebuild). Verified, not re-derived, everything below.

**Git/release invariants**: branch `engineering/v1-true-closure`, HEAD
`19c8234` at the start of this pass. `v1.0.0` → commit
`9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2`, annotated (`git cat-file -t`:
`tag`), unsigned (`git tag -v`: "error: no signature found"), unmoved.
No newer release/RC tag created. No git remotes configured -- nothing
pushed or published (this remains a structural fact of the repository,
not a decision made this pass).

**Distro matrix (final, authoritative)**:

| Distro | Status |
| --- | --- |
| Debian 12 | PASS |
| Debian 13 | PASS |
| Ubuntu 24.04 | PASS |
| Fedora 41 | PASS |
| Rocky Linux 9 | PASS |
| AlmaLinux 9 | PASS |
| Arch Linux | PASS |
| Alpine 3.20 | PASS |
| RHEL 9 full subscribed installation | **UNAVAILABLE** |
| RHEL UBI9 compatibility control | PASS (compatibility control only, not a substitute for the row above) |

**Artifact provenance (unchanged, reconfirmed by hash against the tables
above and against the actual files on disk)**:

- glibc: `dist/linux-x86_64-glibc/` -- `clouddeskd 749721c3...`,
  `cloudesk-privd 9199a871...`, `cloudesk-sessiond 54297a0c...`; highest
  required `GLIBC_2.34`; builder Rocky Linux 9 (digest-pinned), Rust
  `1.97.1` pinned; byte-identical across clean rebuilds; one artifact
  serves every glibc-family target.
- musl: `dist/linux-x86_64-musl/` -- `clouddeskd 1d34d535...`,
  `cloudesk-privd 1e6c1af6...`, `cloudesk-sessiond ecfa3e1b...`; target
  `x86_64-unknown-linux-musl`; builder Alpine 3.20 (digest-pinned), Rust
  `1.97.1`; static-pie, statically linked, zero dynamic library
  dependencies; byte-identical across three independent builds (two in
  Phase 10D, one via `packaging/build-release-musl.sh` this pass).
- **These are two distinct artifact families, not one universal Linux
  binary** -- `installer/install.sh` selects between them by
  `distro_family` (Alpine → musl; every other declared family → glibc),
  verified 8/8 via `tests/distro/artifact-selection.sh`, with the
  wrong-artifact Alpine negative control (glibc binary fails to load,
  missing `/lib64/ld-linux-x86-64.so.2`) still PASS.

**Installer defects found and fixed across Phase 10 (complete list,
reconciled against commit history)**:

1. `/etc/clouddesk` directory ownership/traversal (Phase 10A).
2. Missing `hostname` command on RPM-family minimal installs (Phase 10A).
3. `cloudesk-privd.service` mount-namespace/`/run/clouddesk`
   precreation ordering (Phase 10A).
4. Missing process-wide rustls `CryptoProvider` startup (Phase 10A).
5. World-readable SQLite database on Arch Linux -- explicit `chmod 0600`
   instead of relying on `runuser`'s umask (Phase 10C).
6. Alpine: OpenRC lifecycle unexercised prior to Phase 10D -- now real,
   full PASS (install/enable/start/restart/stop-start/uninstall).
7. Alpine: missing service-account group under BusyBox `adduser -S`,
   breaking every `chown clouddesk:clouddesk` (Phase 10D).
8. Alpine: implicit-parent-directory mode under BusyBox `install -d`
   leaving `/opt/clouddesk` `0700` root-only (Phase 10D).
9. Alpine: no artifact-selection logic existed at all before Phase 10D;
   now selects musl vs. glibc by trusted `distro_family` (Phase 10D).

No security-relevant finding has been omitted from this list.

**Security invariants (reconfirmed this pass by direct inspection, not
re-run)**: main `clouddeskd` non-root on every tested distro; database
`0600` where established (all distros, explicit since fix #5); TLS
private key `0640`, restricted; privileged socket `root:clouddesk 0660`,
not world-writable; `/etc/clouddesk` group-traversable, not broadly
exposed; `/run/clouddesk` correct owner/mode; no `chmod 777` anywhere in
the installer; no secret file depends solely on ambient umask (every one
gets an explicit `chmod`). **Installer Critical: 0. Installer High: 0.**

**Service-manager coverage**: systemd PASS across every applicable
tested distro; OpenRC PASS on Alpine 3.20. Actual exercised lifecycle:
install, enable, start, status, restart, stop→start, and
application-level recovery (clean restart with no duplicate/zombie
processes). **Enablement is not equated with reboot** -- see below.

**SQLite/HTTPS/persistence**: fresh SQLite migration PASS, HTTPS PASS,
port 9870 PASS, reinstall PASS, persistent controlled state PASS,
service remains functional after reinstall PASS -- each individually
evidenced per distro in the sections above, no row inferred from
another.

**Residual environment/publication blockers (preserved honestly, none
upgraded, none hidden)**:

- RHEL full subscribed installation: **UNAVAILABLE** (environment
  limitation -- no entitled RHEL 9 environment was available to this
  pass; UBI9 is Red Hat's own official redistributable content and
  stands as a compatibility control only, not a substitute).
- SELinux enforcing: **BLOCKED BY ENVIRONMENT** -- the disposable
  container harness did not provide a meaningful enforcing-SELinux
  environment. No executable evidence of an actual product defect
  exists, so this is not classified as implementation-missing.
- True boot/reboot persistence: **BLOCKED BY ENVIRONMENT** -- Phase 10
  used disposable init-capable containers, not rebootable VMs, on all
  nine distros including Alpine. Service enablement (`systemctl
  enable`/`rc-update add`) is PASS and is the practical substitute used
  throughout; it is not treated as equivalent to a genuine cold boot.
- Cgroup: containers exposed `cgroup v2` (`cgroup.controllers` readable),
  but no per-service cgroup accounting/limit enforcement was exercised
  anywhere in Phase 10. This is recorded as exactly that -- exposure,
  not delegated-enforcement evidence -- on every distro, Alpine included.
- Remote installer / artifact integrity: the single-command
  `curl -fsSL <official-url> | sudo bash` contract has not been executed
  against an official published URL, because no such URL is published
  yet. curl transport is TLS; no independent script signature/checksum
  verification has been exercised. **UNAVAILABLE / NOT EXECUTED** --
  this is release-*publication* hardening/evidence debt, not a distro
  implementation failure, and does not block Phase 10 completion.

**Host/test hygiene (reconfirmed this pass)**: 0 leftover Docker
containers; host `clouddesk` user absent; host `/etc/clouddesk` absent;
Phase 7 `clouddesk-codetest` identity, sudoers grants, and helper all
absent (never recreated this pass); no installer-created host service.

**Storage**: `target/` 133G, `dist/` 114M, `df .` 116G free (75% used);
`docker system df`: 41 images (21.1GB, 16.59GB reclaimable, not pruned --
preserves harness/evidence images for any future re-verification), 0
containers, 383.5MB volumes. Not materially unsafe for the next phase;
no pruning performed.

**Phase 10 completion rule -- all conditions hold**: declared distro
matrix execution complete; every executable distro target PASS; RHEL
full honestly UNAVAILABLE with UBI9 compatibility evidence retained
separately; SELinux enforcing and reboot remain explicit environment
blockers, not upgraded; remote-fetch integrity remains an explicit
publication-time blocker, not upgraded; Critical 0; High 0; mandatory
installer implementation missing 0; test leaks 0; host residue 0. No
evidence status has been upgraded dishonestly in this reconciliation.

**PHASE 10: COMPLETE.** (final, authoritative)

### Next authoritative phase

Cross-referenced against `Architecture/CloudDesk-OS-spec/PLAN.md`: this
project's own Phase 10 (the distro/installer matrix covered by this
document) corresponds to `PLAN.md`'s **Phase 15 -- Multi-Distribution
Release Hardening** (identical 8-distro matrix, identical SELinux/
OpenRC/musl call-outs). The next phase in `PLAN.md`'s own sequence is:

**Phase 16 -- Security Review**, whose required-testing list (path
traversal, symlink escape, race/TOCTOU, CSRF, XSS, SSRF, WebSocket
authorization, session fixation/replay, 2FA bypass, privilege
escalation, command injection, malicious archive extraction, unsafe
media/document preview, secret exposure in logs, SSH host-key downgrade,
transfer destination spoofing, Browser/Code/Office runtime escape) is
the same ground `CLAUDE_HANDOFF.md`'s adversarial scenario catalog
(135 numbered targets) already exists to drive. Exit criteria per
`PLAN.md`: no open critical or high-severity security issue accepted for
the v1.0 release. This was not assumed from memory -- it was read
directly from `PLAN.md`, not inferred from this project's own internal
Phase-10-lettering scheme.
