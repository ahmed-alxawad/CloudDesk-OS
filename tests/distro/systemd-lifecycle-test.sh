#!/usr/bin/env bash
# Phase 10 distro installer/service lifecycle test driver -- exercises
# the REAL installer/uninstaller against a REAL systemd PID 1 inside a
# disposable container (see README.md for the harness images and how
# to launch one). Never a shell-branch/manifest check; every step here
# runs the actual product.
#
# Usage: systemd-lifecycle-test.sh <container-name> <distro-label>
#   <container-name>  a running container from one of the
#                      systemd-harness.*.Dockerfile images, started
#                      with the repo bind-mounted read-only at /repo AND
#                      dist/portable-x86_64-glibc bind-mounted read-only
#                      at /portable (see README.md for the exact
#                      `docker run` and how to produce that directory).
#                      Every distro test installs the SAME portable
#                      artifact (Phase 10B) -- never host target/release
#                      binaries, whose glibc requirement tracks whatever
#                      happened to build them.
#   <distro-label>     free-form label used only for the report filename.
#
# Writes tests/distro/reports/<distro-label>.report.txt (gitignored).
set -Eeuo pipefail

CONTAINER="$1"
LABEL="$2"
REPORT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/reports"
mkdir -p "$REPORT_DIR"
LOG="$REPORT_DIR/${LABEL}.report.txt"
: > "$LOG"

step() { printf '\n=== %s ===\n' "$*" | tee -a "$LOG"; }
rec() { printf '%s\n' "$*" | tee -a "$LOG"; }
dexec() { docker exec "$CONTAINER" "$@"; }

step "PART 6: PRE-FLIGHT (clean environment)"
dexec sh -c 'id clouddesk 2>&1' | tee -a "$LOG" || true
dexec sh -c '[ -e /opt/clouddesk ] && echo "PRESENT (unexpected)" || echo "absent (expected)"' | tee -a "$LOG"
dexec sh -c '[ -e /etc/clouddesk ] && echo "PRESENT (unexpected)" || echo "absent (expected)"' | tee -a "$LOG"
dexec sh -c 'command -v systemctl >/dev/null && systemctl is-active clouddesk.service 2>&1 || true' | tee -a "$LOG"
dexec sh -c 'ss -ltn 2>/dev/null | grep -q ":9870 " && echo "9870 IN USE (unexpected)" || echo "9870 free (expected)"' | tee -a "$LOG"

step "PART 7: ACTUAL INSTALL"
INSTALL_EXIT=0
docker exec -e CLOUDESK_BINARY=/portable/clouddeskd -e CLOUDESK_PRIVD_BINARY=/portable/cloudesk-privd \
    -e CLOUDESK_SESSIOND_BINARY=/portable/cloudesk-sessiond "$CONTAINER" \
    sh -c '/repo/installer/install.sh' >>"$LOG" 2>&1 || INSTALL_EXIT=$?
rec "install.sh exit code: $INSTALL_EXIT"

step "PART 6/7 detected distro/family/init (from installer's own output above)"

step "PART 8: SERVICE IDENTITY"
dexec sh -c 'systemctl show clouddesk.service -p User,Group,CapabilityBoundingSet,AmbientCapabilities,NoNewPrivileges,ProtectSystem,ProtectHome,PrivateTmp' | tee -a "$LOG"

step "PART 9/10: PORT 9870 + HTTPS REACHABILITY"
sleep 3
dexec sh -c 'systemctl is-active clouddesk.service cloudesk-privd.service' | tee -a "$LOG" || true
dexec sh -c 'ss -ltnp 2>/dev/null | grep 9870 || echo "no listener"' | tee -a "$LOG"
dexec sh -c 'curl -sk -o /dev/null -w "HTTP_STATUS=%{http_code}\n" https://127.0.0.1:9870/api/v1/setup/status || echo "CURL FAILED"' | tee -a "$LOG"
dexec sh -c 'curl -sk https://127.0.0.1:9870/api/v1/setup/status' | tee -a "$LOG" || true

step "PART 10: TLS CERT/KEY"
dexec sh -c 'ls -l /etc/clouddesk/tls/server.crt /etc/clouddesk/tls/server.key' | tee -a "$LOG"
dexec sh -c 'openssl x509 -in /etc/clouddesk/tls/server.crt -noout -subject -dates -ext subjectAltName' | tee -a "$LOG"

step "PART 11: FILESYSTEM PERMISSIONS"
dexec sh -c 'stat -c "%U:%G %a %n" /opt/clouddesk/bin/clouddeskd /etc/clouddesk/clouddesk.toml /etc/clouddesk/keys/master.key /etc/clouddesk/keys/privd-grant.key /etc/clouddesk/tls/server.key /var/lib/clouddesk /var/lib/clouddesk/clouddesk.db 2>&1' | tee -a "$LOG"

step "PART 12: SQLITE INITIALIZATION"
dexec sh -c 'test -s /var/lib/clouddesk/clouddesk.db && echo "db exists and non-empty" || echo "MISSING/EMPTY"' | tee -a "$LOG"
dexec sh -c 'command -v sqlite3 >/dev/null && sqlite3 /var/lib/clouddesk/clouddesk.db ".tables" || echo "sqlite3 cli unavailable, skipping table listing"' | tee -a "$LOG"

step "PART 13: SERVICE RESTART"
dexec sh -c 'systemctl restart clouddesk.service && sleep 2 && systemctl is-active clouddesk.service' | tee -a "$LOG"
dexec sh -c 'curl -sk -o /dev/null -w "HTTP_STATUS_AFTER_RESTART=%{http_code}\n" https://127.0.0.1:9870/api/v1/setup/status' | tee -a "$LOG"
dexec sh -c 'systemctl stop clouddesk.service && systemctl is-active clouddesk.service; true' | tee -a "$LOG"
dexec sh -c 'systemctl start clouddesk.service && sleep 2 && systemctl is-active clouddesk.service' | tee -a "$LOG"
dexec sh -c 'pgrep -c clouddeskd || echo 0' | tee -a "$LOG"

step "PART 14: SERVICE ENABLEMENT"
dexec sh -c 'systemctl is-enabled clouddesk.service cloudesk-privd.service' | tee -a "$LOG"

step "PART 22: LOGGING / JOURNAL"
dexec sh -c 'journalctl -u clouddesk.service --no-pager -n 40' | tee -a "$LOG"

step "PART 16: DATA PRESERVATION MARKER (before reinstall)"
dexec sh -c 'echo "marker-before-reinstall" > /var/lib/clouddesk/PHASE10_MARKER.txt; cat /etc/clouddesk/clouddesk.toml | grep -c "port = 9870"' | tee -a "$LOG"
dexec sh -c 'openssl x509 -in /etc/clouddesk/tls/server.crt -noout -fingerprint -sha256' | tee -a "$LOG"

step "PART 15: REINSTALL IDEMPOTENCE"
REINSTALL_EXIT=0
docker exec -e CLOUDESK_BINARY=/portable/clouddeskd -e CLOUDESK_PRIVD_BINARY=/portable/cloudesk-privd \
    -e CLOUDESK_SESSIOND_BINARY=/portable/cloudesk-sessiond "$CONTAINER" \
    sh -c '/repo/installer/install.sh' >>"$LOG" 2>&1 || REINSTALL_EXIT=$?
rec "reinstall exit code: $REINSTALL_EXIT"
dexec sh -c 'id -u clouddesk; getent passwd clouddesk | wc -l' | tee -a "$LOG"
dexec sh -c 'systemctl is-active clouddesk.service' | tee -a "$LOG"
dexec sh -c 'test -f /var/lib/clouddesk/PHASE10_MARKER.txt && cat /var/lib/clouddesk/PHASE10_MARKER.txt || echo "MARKER LOST"' | tee -a "$LOG"
dexec sh -c 'openssl x509 -in /etc/clouddesk/tls/server.crt -noout -fingerprint -sha256' | tee -a "$LOG"
dexec sh -c 'curl -sk -o /dev/null -w "HTTP_STATUS_AFTER_REINSTALL=%{http_code}\n" https://127.0.0.1:9870/api/v1/setup/status' | tee -a "$LOG"

step "PART 21: UNINSTALL (non-purge, then purge)"
docker exec "$CONTAINER" sh -c '/repo/installer/uninstall.sh' >>"$LOG" 2>&1 || rec "uninstall (no purge) exit: $?"
dexec sh -c 'systemctl is-active clouddesk.service 2>&1; test -e /opt/clouddesk && echo "binaries PRESENT (unexpected)" || echo "binaries removed"' | tee -a "$LOG"
dexec sh -c 'test -e /etc/clouddesk && echo "config PRESERVED (expected without --purge)" || echo "config REMOVED"' | tee -a "$LOG"
dexec sh -c 'test -e /var/lib/clouddesk/clouddesk.db && echo "db PRESERVED (expected without --purge)" || echo "db REMOVED"' | tee -a "$LOG"
docker exec "$CONTAINER" sh -c '/repo/installer/uninstall.sh --purge' >>"$LOG" 2>&1 || rec "uninstall --purge exit: $?"
dexec sh -c 'test -e /etc/clouddesk && echo "config PRESENT (unexpected after purge)" || echo "config purged"' | tee -a "$LOG"
dexec sh -c 'test -e /var/lib/clouddesk && echo "data PRESENT (unexpected after purge)" || echo "data purged"' | tee -a "$LOG"
dexec sh -c 'id clouddesk 2>&1' | tee -a "$LOG"

rec ""
rec "=== DONE: $LABEL ==="
