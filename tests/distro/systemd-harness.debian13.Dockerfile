FROM debian:13
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    systemd systemd-sysv dbus sudo curl sqlite3 iproute2 procps \
 && apt-get clean && rm -rf /var/lib/apt/lists/*
# systemd-modules-load.service tries to load kernel modules from
# /lib/modules, which is meaningless inside a container (no module
# loading happens independent of the shared host kernel) -- fails and
# reports the whole system "degraded" on every Debian 13 container
# boot regardless of anything CloudDesk does. Masked so
# `systemctl is-system-running` stays a meaningful signal for genuine
# failures.
RUN systemctl mask systemd-modules-load.service
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
