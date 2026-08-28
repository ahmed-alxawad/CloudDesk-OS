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

Build the release binaries and frontend first (the installer's own
prerequisite), then a harness image, then run it:

```sh
cargo build --release -p clouddeskd -p cloudesk-privd -p cloudesk-sessiond
(cd apps/web && npm run build)

docker build -t clouddesk-distro-test/ubuntu2404:systemd \
    -f tests/distro/systemd-harness.ubuntu2404.Dockerfile tests/distro

docker run -d --name cd-ubuntu2404 --privileged --cgroupns=host \
    -v /sys/fs/cgroup:/sys/fs/cgroup:rw --tmpfs /run --tmpfs /run/lock \
    -v "$PWD:/repo:ro" \
    clouddesk-distro-test/ubuntu2404:systemd

tests/distro/systemd-lifecycle-test.sh cd-ubuntu2404 ubuntu2404
docker rm -f cd-ubuntu2404
```

Swap the Dockerfile/image/container name for `fedora41` (RPM family) the same
way. `rocky9` is provided but **cannot currently run the locally built
binary**: this build environment's glibc (2.43) produces a binary requiring
`GLIBC_2.39`, and Rocky Linux 9 ships glibc 2.34 -- confirmed live
(`/lib64/libc.so.6: version 'GLIBC_2.39' not found`). This is a release
build-baseline concern (build on an older-glibc baseline, or ship per-family
builds), not an installer defect; Fedora was used as the RPM-family
representative for Phase 10A instead, exactly as its own governing pass
permitted.

### What this harness needs

- Docker with cgroup v2 and enough privilege to boot systemd as PID 1
  (`--privileged --cgroupns=host` plus the host's `/sys/fs/cgroup` bind-mounted
  in, as above). A plain unprivileged container cannot run a real init and
  is not sufficient for a service-lifecycle PASS.
- The repo's own `target/release/*` binaries and `apps/web/dist` already
  built -- the harness bind-mounts the checked-out repo read-only at `/repo`
  and runs the installer exactly as it exists on disk (**local installer
  execution**, not the real `curl -fsSL <url> | sudo bash` remote-fetch
  contract, which needs an actual published URL that does not exist yet).

Reports land in `tests/distro/reports/` (gitignored -- host-specific
container IDs/hostnames appear in the TLS subject and journal output).
