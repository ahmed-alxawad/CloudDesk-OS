# Distribution tests

Installer test fixtures for Debian, Ubuntu, RHEL, Fedora, Rocky, AlmaLinux, Arch,
and Alpine begin in PLAN.md Phase 1 and become mandatory CI in Phase 15.

## Fast layout smoke test

`installer-layout.sh` runs `installer/install.sh` with `CLOUDESK_ROOT` set to a
disposable prefix and `CLOUDESK_SKIP_PACKAGES=1`/`CLOUDESK_INIT_SYSTEM=none`,
for every declared distro ID. It proves the installer's own file-layout logic
(paths, TLS/key generation, config rendering) is distro-ID-agnostic in
isolation. It does **not** touch a package manager, create a real service
account, install a real service unit, or start a real service -- it is a
shell-branch check, not installer/service acceptance evidence.

```sh
tests/distro/installer-layout.sh
```

## Real installer + service lifecycle harness (Phase 10)

`systemd-harness.*.Dockerfile` build disposable, real-`systemd`-as-PID-1
container images for representative distros -- a real init, not a bare
container without service-manager support, so `systemctl`
enable/start/restart/stop and `journalctl` all behave like a real host.
`systemd-lifecycle-test.sh` then runs the **actual** `installer/install.sh`
and `installer/uninstall.sh` inside one, and checks the real, running result:
service identity/hardening, port 9870 HTTPS reachability, TLS material,
filesystem permissions, SQLite migration, restart/enable semantics, reinstall
idempotence + data preservation, and uninstall (both with and without
`--purge`).

Build the **portable release artifact** first (Phase 10B) -- never the
operator host's own `target/release/*` binaries. A binary built on
whatever glibc the host happens to have only works on distros with an
equal-or-newer glibc; this is exactly how Phase 10A found the installer
completely unable to run on Rocky/AlmaLinux/RHEL9 in the first place:

```sh
packaging/build-release.sh
(cd apps/web && npm run build)
```

This produces `dist/linux-x86_64-glibc/{clouddeskd,cloudesk-privd,cloudesk-sessiond}`
(gitignored), linked against no newer than `GLIBC_2.34` -- the floor across
every currently declared v1 glibc-family target (see `PHASE10_DISTRO_MATRIX.md`
for the live-verified per-distro glibc table). Then build a harness image and
run it, mounting **both** the repo (for `installer/`, read-only) and the
portable artifact (read-only, at `/portable`):

```sh
docker build -t clouddesk-distro-test/ubuntu2404:systemd \
    -f tests/distro/systemd-harness.ubuntu2404.Dockerfile tests/distro

docker run -d --name cd-ubuntu2404 --privileged --cgroupns=host \
    -v /sys/fs/cgroup:/sys/fs/cgroup:rw --tmpfs /run --tmpfs /run/lock \
    -v "$PWD:/repo:ro" \
    -v "$PWD/dist/linux-x86_64-glibc:/portable:ro" \
    clouddesk-distro-test/ubuntu2404:systemd

tests/distro/systemd-lifecycle-test.sh cd-ubuntu2404 ubuntu2404
docker rm -f cd-ubuntu2404
```

Swap the Dockerfile/image/container name for `fedora41`, `rocky9`,
`almalinux9`, or `ubi9` (RHEL, via Red Hat's own official redistributable
UBI9 userspace -- see the RHEL caveat in `PHASE10_DISTRO_MATRIX.md` before
treating that one as equivalent to a subscribed RHEL install) the same way.
All five have real, live-verified PASSes against this one portable artifact.

### What this harness needs

- Docker with cgroup v2 and enough privilege to boot systemd as PID 1
  (`--privileged --cgroupns=host` plus the host's `/sys/fs/cgroup` bind-mounted
  in, as above). A plain unprivileged container cannot run a real init and
  is not sufficient for a service-lifecycle PASS.
- The portable artifact already built (above) and `apps/web/dist` built --
  the harness bind-mounts the checked-out repo read-only at `/repo` and runs
  the installer exactly as it exists on disk (**local installer execution**,
  not the real `curl -fsSL <url> | sudo bash` remote-fetch contract, which
  needs an actual published URL that does not exist yet).

Reports land in `tests/distro/reports/` (gitignored -- host-specific
container IDs/hostnames appear in the TLS subject and journal output).
