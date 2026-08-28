FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
# systemd/dbus/sudo: needed to boot as PID 1 with a real init.
# curl/sqlite3/iproute2/procps: harness-only diagnostic tools, never
# installed by CloudDesk's own installer -- used solely by this test
# driver to inspect the result of running the real installer.
RUN apt-get update && apt-get install -y --no-install-recommends \
    systemd systemd-sysv dbus sudo curl sqlite3 iproute2 procps \
 && apt-get clean && rm -rf /var/lib/apt/lists/*
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
