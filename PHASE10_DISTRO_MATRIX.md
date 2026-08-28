# Phase 10 — Distro Installer/Service Matrix

Status: **PARTIAL**. This is the first Phase 10 pass (10A): harness
foundation plus one representative distro per required family. All
statuses below are from real execution -- the actual `installer/install.sh`
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

## Representative distros this pass

| Distro | Version | Arch | Harness | Service mgr | Install | Start | Restart | Enable | HTTPS | SQLite | Reinstall | Persistence | SELinux | Cgroup | Uninstall | Reboot | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Ubuntu | 24.04 (Noble) | x86_64 | Docker, real systemd 255 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | N/A | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| Fedora | 41 (Container Image) | x86_64 | Docker, real systemd 255 PID 1, cgroup v2 delegated | systemd | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | BLOCKED BY ENVIRONMENT | BLOCKED BY ENVIRONMENT | PASS | BLOCKED BY ENVIRONMENT | **PASS** |
| Rocky Linux | 9 | x86_64 | Docker, real systemd 252 PID 1 (boots; installer never reached) | systemd | **IMPLEMENTATION MISSING** (see below) | NOT EXECUTED | NOT EXECUTED | NOT EXECUTED | NOT EXECUTED | NOT EXECUTED | NOT EXECUTED | NOT EXECUTED | NOT EXECUTED | NOT EXECUTED | NOT EXECUTED | **PARTIAL** |

**Rocky Linux 9 detail**: the release binary this pass built has to
satisfy `GLIBC_2.39` (this build host's glibc is 2.43), but Rocky
Linux 9 ships glibc 2.34 -- confirmed live: `/lib64/libc.so.6: version
'GLIBC_2.39' not found`. This is a **release build-baseline** issue (build
on an older-glibc baseline, or ship per-family builds), not a defect in
`installer/install.sh` itself, and not evidence the installer's own logic
is broken for RHEL-family systems. Fedora was substituted as this pass's
RPM-family representative, exactly as this pass's own governing
instructions permitted ("Fedora or Rocky/Alma"). **Rocky/Alma/RHEL remain
genuinely unproven and must not be read as PASS.**

## Remaining official targets: NOT EXECUTED

Debian, Rocky Linux, AlmaLinux, RHEL, Arch Linux, Alpine Linux.

## Cross-distro differences observed

- **`hostname` command**: present by default on Ubuntu 24.04 (Debian
  family), absent on a minimal Fedora 41 install (fixed, see below --
  applies to every RPM-family target, not just Fedora).
- **Package manager**: `apt-get`/`dpkg` (Debian family, verified) vs.
  `dnf` (RPM family, verified against Fedora only -- **not** verified
  that the exact same package names resolve on RHEL/Rocky/Alma, which
  draw from different default repositories than Fedora).
- **glibc baseline**: Ubuntu 24.04 and Fedora 41 both satisfy this
  build's `GLIBC_2.39` requirement; Rocky Linux 9 (glibc 2.34, matching
  RHEL 9's baseline) does not. This is the single most consequential
  cross-distro finding this pass -- it affects every RHEL/Rocky/Alma
  target identically, independent of anything installer-specific.
- **systemd version**: 255 (Ubuntu 24.04) vs. 252 (Rocky 9, confirmed
  bootable) vs. 255 (Fedora 41) -- no version-specific installer behavior
  observed in either family actually exercised.
- **SELinux**: both harnesses report `getenforce` = `Disabled` inside the
  container (the host's own kernel is `Enforcing`, but a Docker container
  does not carry independent SELinux file-context labeling without
  additional host configuration this pass did not set up). `packaging/selinux/`
  is an existing, empty placeholder directory -- **no SELinux policy module
  is shipped**. This is a real, documented gap for a genuinely
  SELinux-enforcing RHEL-family production host: untested, not proven safe,
  not proven broken.
- **Cgroup delegation**: unchanged from the pre-existing, already-documented
  finding elsewhere in this project (host process-migration into a leaf
  cgroup returns `ENOTSUP` despite partial controller delegation) --
  BLOCKED BY ENVIRONMENT, not re-litigated in depth this pass.

## Defects found and fixed this pass

All three were **confirmed live** (not inferred from source reading) and
would have affected **every** target distro identically, not just the one
that happened to surface them first:

1. **`/etc/clouddesk` directory ownership** (`installer/install.sh`,
   commit `4d2d5ef`) -- individual files inside `/etc/clouddesk` were
   chowned `root:clouddesk`, but the containing directories stayed
   `root:root 0750`, so the `clouddesk` service account (not a member of
   `root`) could never traverse in to read them at all. Broke `clouddeskd
   migrate` during install, and would have broken the real
   `clouddesk.service` (`User=clouddesk`) identically, on every distro,
   every time.
2. **Missing `hostname` command on RPM-family minimal installs**
   (`installer/install.sh`, commit `0aa5e76`) -- TLS material generation
   failed outright with "command not found" (exit 127) before creating
   anything. Fixed with a `uname -n` fallback (POSIX, always present).
3. **`cloudesk-privd.service` mount-namespace ordering**
   (`packaging/systemd/cloudesk-privd.service`, commit `6c081dd`) --
   `ReadWritePaths=/run/clouddesk` required the directory to already exist
   before systemd's own sandbox setup ran, but nothing created it that
   early (the app's own `fs::create_dir_all` runs too late to matter).
   Crash-looped on every fresh boot with `Failed to set up mount
   namespacing`. Fixed with `RuntimeDirectory=clouddesk`.

Plus one **product-level** defect, found only because real service startup
was exercised (not merely file layout):

4. **No process-wide rustls `CryptoProvider` installed**
   (`services/clouddeskd/src/main.rs`, commit `b535826`) --
   `clouddesk.service` crash-looped on every fresh install with "Could not
   automatically determine the process-level CryptoProvider from Rustls
   crate features": both `ring` and `aws-lc-rs` end up in the dependency
   tree (axum-server pulls one, reqwest's `rustls-tls` pulls the other),
   so rustls refuses to auto-select. The release binary was, until this
   fix, **completely unable to serve HTTPS**, independent of any distro or
   installer concern whatsoever. Fixed by installing the `ring` provider
   explicitly at the top of `main()`.

Without these four fixes, **zero** of the declared eight distro targets
could have produced a working installation.

## Service identity / privilege (Part 8)

`clouddesk.service`: `User=clouddesk Group=clouddesk`,
`CapabilityBoundingSet=` (empty), `AmbientCapabilities=` (empty),
`NoNewPrivileges=yes`, `ProtectSystem=strict`, `ProtectHome=yes`,
`PrivateTmp=yes` -- confirmed live via `systemctl show`, not read from the
unit file alone. **Main service does not run as root.**

`cloudesk-privd.service` runs as root by design (the narrow privileged
helper architecture) with equivalent hardening otherwise
(`ProtectSystem=strict`, `RestrictAddressFamilies=AF_UNIX`,
`SystemCallArchitectures=native`, etc.) and now `RuntimeDirectory=clouddesk`.
It exposes only the typed operations `cloudesk-privd`/`cloudesk-sessiond`
implement -- no arbitrary root-command surface.

## Port 9870 / HTTPS (Parts 9-10)

Both distros: real TCP listener on `0.0.0.0:9870` owned by the
`clouddeskd` process, real TLS handshake (self-signed RSA-3072/SHA-256,
397-day validity, SAN covering the container's own hostname and
`127.0.0.1`), a genuine HTTP 200 with a real JSON body
(`{"bootstrap_required":true}`) from `/api/v1/setup/status` -- not merely
an open socket. `server.key` is `root:clouddesk 0640`; `server.crt` is
world-readable `0644` (a public certificate, correctly not restricted).

## Filesystem permissions (Part 11)

`/opt/clouddesk/bin/clouddeskd` `root:root 0755`; `/etc/clouddesk/clouddesk.toml`,
`master.key`, `privd-grant.key`, `tls/server.key` all `root:clouddesk 0640`;
`/var/lib/clouddesk` `clouddesk:clouddesk 0750`;
`/var/lib/clouddesk/clouddesk.db` `clouddesk:clouddesk 0600`. No path
resolves into a test user's home; all paths are the fixed
`/opt`, `/etc`, `/var/lib`, `/var/log` locations the installer declares.

## SQLite (Part 12)

Fresh install on both distros produced a non-empty `clouddesk.db` with
every expected table present (`_sqlx_migrations`, `users`, `sessions`,
`vault_secrets`, `runtime_instances`, and the rest of the full product
schema) -- real migrations applied from a genuinely clean database, not a
pre-baked fixture.

## Restart / enable (Parts 13-14)

Both distros: `restart` → `active` → real HTTP 200; `stop` → `inactive`;
`start` → `active` again; exactly one `clouddeskd` process at all times
(no orphans); `systemctl is-enabled` reports `enabled` for both units.

## Reinstall idempotence / data preservation (Parts 15-16)

Both distros: a marker file written to `/var/lib/clouddesk/` before
reinstall, and the TLS certificate's SHA-256 fingerprint recorded before
reinstall, both survived a second `install.sh` run byte-for-byte
identical. No duplicate `clouddesk` user was created (`getent passwd
clouddesk | wc -l` = 1 both times). The service remained active and
reachable (HTTP 200) throughout.

## Failure safety (Part 17)

- Unsupported distro (`CLOUDESK_DISTRO_ID=plan9`): clean `exit 1`,
  `CloudDesk installer: unsupported Linux distribution`, nothing
  installed. Confirmed via `test -e /opt/clouddesk` afterward.
- Missing required command (`openssl` removed from `PATH`, packages
  skipped): clean `exit 127` at the exact point `openssl` was needed, no
  false "installation complete" banner (that banner is the script's last
  line and was never reached).
- The installer does **not** perform automatic rollback of partial state
  on failure -- this is a straightforward `set -eu` script by design, not
  a transactional installer. Documented here rather than silently assumed.

## Uninstall (Part 21)

Both distros, both modes exercised for real:

- Without `--purge`: services stopped/disabled/removed, `/opt/clouddesk`
  removed; `/etc/clouddesk`, `/var/lib/clouddesk` (including the live
  database), `/var/log/clouddesk` all explicitly preserved, matching the
  script's own printed guidance.
- With `--purge`: all of the above additionally removed, including the
  `clouddesk` user/group.

## Logging (Part 22)

`journalctl -u clouddesk.service` on both distros shows a clean startup
sequence (media availability detection, HTTPS listener start) with no
crash loop and no secret values logged.

## Reboot (Part 23)

**BLOCKED BY ENVIRONMENT** on both distros -- these are containers, not
VMs; a true reboot is not meaningful here and was not faked as a service
restart. Enablement configuration itself (`systemctl is-enabled` =
`enabled` for both units) was verified as the practical substitute this
harness can honestly provide.

## Installer shell quality (Part 27)

`shellcheck -s sh` (via the official `koalaman/shellcheck` container, no
host installation) against `install.sh`, `uninstall.sh`, and every
`lib/*.sh`: zero warnings in any `lib/*.sh`; `install.sh`/`uninstall.sh`
show only `SC1007` (a false positive on the deliberate `CDPATH= cd --`
idiom, used intentionally throughout this codebase) and `SC2154`/`SC2034`
(variables assigned in a sourced file shellcheck wasn't given with `-x`).
No findings of real concern.

## Security review of install/uninstall scripts (Part 28)

Checked specifically for: unchecked downloads (none -- no network fetch of
binaries exists in these scripts at all; only the OS package manager
fetches packages, under its own repo/GPG trust model), unsafe temp files
(`umask 077` set at the top of `install.sh`; TLS/master/grant keys/bootstrap
secret all written with `openssl` directly to their final path, never a
predictable shared temp location), unquoted variables (none found;
`shellcheck` independently confirms), path/command injection through distro
metadata (`distro_id`/`distro_family` are matched against a fixed `case`
allowlist before ever being used, never interpolated into a command), root
writes through symlinks (all target paths are freshly `install -d`-created
under installer-owned prefixes, not user-writable locations), credential
echoing (the one intentional secret print is the bootstrap secret to the
installing admin's own terminal at the end of a successful install --
required by the product's own bootstrap UX, not a leak), `chmod 777`
(none). **The `curl -fsSL <url> | sudo bash` remote-fetch contract itself
relies on TLS-only trust with no separate checksum or signature
verification of the script** -- see Part 29. **Installer security
Critical: 0. High: 0.**

## Download/artifact integrity (Part 29)

`install.sh`/`uninstall.sh` download nothing themselves -- binaries come
from the local build tree (or a future release artifact location, not yet
defined), and OS packages come from the distro's own package manager under
its own signature-verified repo trust. The **product-level** remote-install
contract (`curl -fsSL <official-install-url> | sudo bash`) has no
cryptographic verification of the installer script itself beyond HTTPS/TLS
transport -- the same trust model rustup/Homebrew/get.docker.com use, and
a real, honestly-reported risk (a compromised or MITM'd fetch would run
arbitrary code as root) rather than a defect this pass introduced or can
fix: no official URL is published yet for this to be tested or hardened
against.

## Cgroup (Part 20)

Reported separately from installation, per this pass's own instruction:
application installation is **PASS** on both executed distros; host cgroup
process-migration enforcement remains the pre-existing, already-documented
**BLOCKED BY ENVIRONMENT** (kernel `ENOTSUP` on leaf-cgroup migration
despite partial controller delegation), not re-investigated in depth this
pass.

## Host safety (Part 4)

Confirmed after every run this pass: no `clouddesk` user, `/opt/clouddesk`,
or `/etc/clouddesk` on the **operator host** at any point; no new host
sudoers grants; 0 leaked containers (`docker ps -aq` = 0 after every
container removal). All installation/service state existed only inside
the disposable harness containers.

## Next Phase 10 pass

- Rebuild release binaries on an older-glibc baseline (or add a musl
  target) so the RHEL/Rocky/AlmaLinux family can actually be executed,
  not merely substituted for.
- Execute Debian directly (not inferred from the Ubuntu run, even though
  both share `distro_family=debian`).
- Execute Rocky Linux and/or AlmaLinux for real once the glibc issue is
  resolved, including a genuinely SELinux-enforcing host or VM to close
  the current BLOCKED BY ENVIRONMENT SELinux gap.
- Execute Arch Linux and Alpine Linux (the latter needs the OpenRC path,
  entirely unexercised this pass).
- Verify RPM package-name parity specifically on RHEL/Rocky/Alma (this
  pass only confirmed the `dnf install` package list resolves on Fedora).
- A true VM-based reboot test to close the reboot-persistence gap.
- Establish and test the real `curl -fsSL <url> | sudo bash` remote-fetch
  path once an official URL exists, including artifact-integrity
  hardening for that fetch.

**PHASE 10: PARTIAL.**
