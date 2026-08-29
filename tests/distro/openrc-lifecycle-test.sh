#!/usr/bin/env bash
# Phase 10D Alpine/OpenRC installer/service lifecycle test driver --
# exercises the REAL installer/uninstaller against REAL rc-service/
# rc-update (see README.md for the harness image and how to launch
# one). Never a shell-branch/manifest check; every step here runs the
# actual product.
#
# Usage: openrc-lifecycle-test.sh <container-name> <distro-label>
#   <container-name>  a running container from
#                      openrc-harness.alpine320.Dockerfile, started
#                      with the repo bind-mounted read-only at /repo AND
#                      dist/linux-x86_64-musl bind-mounted read-only at
#                      /portable (see README.md for the exact
#                      `docker run`).
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
install_env() {
    docker exec \
        -e CLOUDESK_BINARY=/portable/clouddeskd \
        -e CLOUDESK_PRIVD_BINARY=/portable/cloudesk-privd \
        -e CLOUDESK_SESSIOND_BINARY=/portable/cloudesk-sessiond \
        "$CONTAINER" "$@"
}

step "PART 12: PRE-FLIGHT (clean environment)"
dexec sh -c 'id clouddesk 2>&1' | tee -a "$LOG" || true
dexec sh -c '[ -e /opt/clouddesk ] && echo "PRESENT (unexpected)" || echo "absent (expected)"' | tee -a "$LOG"
dexec sh -c '[ -e /etc/clouddesk ] && echo "PRESENT (unexpected)" || echo "absent (expected)"' | tee -a "$LOG"
dexec sh -c '[ -e /var/lib/clouddesk/clouddesk.db ] && echo "PRESENT (unexpected)" || echo "absent (expected)"' | tee -a "$LOG"
dexec sh -c 'ss -ltn 2>/dev/null | grep -q ":9870 " && echo "9870 IN USE (unexpected)" || echo "9870 free (expected)"' | tee -a "$LOG"

step "PART 13: ACTUAL INSTALL"
INSTALL_EXIT=0
install_env sh -c '/repo/installer/install.sh' >>"$LOG" 2>&1 || INSTALL_EXIT=$?
rec "install.sh exit code: $INSTALL_EXIT"

step "PART 16: OPENRC SCRIPTS INSTALLED"
dexec sh -c 'ls -l /etc/init.d/clouddesk /etc/init.d/cloudesk-privd' | tee -a "$LOG"
dexec sh -c 'cat /etc/conf.d/cloudesk-privd 2>&1 | sed "s/=.*/=<redacted>/"' | tee -a "$LOG"

step "PART 17: MAIN SERVICE PROCESS IDENTITY (live, not read from the unit file)"
dexec sh -c 'ps -o pid,user,group,args -C clouddeskd 2>/dev/null || ps aux | grep "[c]louddeskd"' | tee -a "$LOG"

step "PART 19: /run/clouddesk"
dexec stat -c '%U:%G %a %n' /run/clouddesk 2>&1 | tee -a "$LOG"

step "PART 20: PORT 9870 + HTTPS REACHABILITY"
sleep 2
dexec rc-service clouddesk status | tee -a "$LOG" || true
dexec rc-service cloudesk-privd status | tee -a "$LOG" || true
dexec sh -c 'ss -ltnp 2>/dev/null | grep 9870 || echo "no listener"' | tee -a "$LOG"
dexec sh -c 'curl -sk -o /dev/null -w "HTTP_STATUS=%{http_code}\n" https://127.0.0.1:9870/api/v1/setup/status || echo "CURL FAILED"' | tee -a "$LOG"
dexec sh -c 'curl -sk https://127.0.0.1:9870/api/v1/setup/status' | tee -a "$LOG" || true

step "TLS CERT/KEY"
dexec sh -c 'ls -l /etc/clouddesk/tls/server.crt /etc/clouddesk/tls/server.key' | tee -a "$LOG"
dexec sh -c 'openssl x509 -in /etc/clouddesk/tls/server.crt -noout -subject -dates -ext subjectAltName' | tee -a "$LOG"

step "PART 21/22: SQLITE + SECRET FILE MODES"
dexec sh -c 'stat -c "%U:%G %a %n" /opt/clouddesk/bin/clouddeskd /etc/clouddesk/clouddesk.toml /etc/clouddesk/keys/master.key /etc/clouddesk/keys/privd-grant.key /etc/clouddesk/tls/server.key /var/lib/clouddesk /var/lib/clouddesk/clouddesk.db 2>&1' | tee -a "$LOG"
dexec sh -c 'test -s /var/lib/clouddesk/clouddesk.db && echo "db exists and non-empty" || echo "MISSING/EMPTY"' | tee -a "$LOG"
dexec sh -c 'sqlite3 /var/lib/clouddesk/clouddesk.db ".tables" | tr -s " \n" " "' | tee -a "$LOG"

step "PART 23: SERVICE LIFECYCLE (start/status/restart/status/stop/status/start)"
dexec rc-service clouddesk restart | tee -a "$LOG"
dexec rc-service clouddesk status | tee -a "$LOG"
dexec sh -c 'curl -sk -o /dev/null -w "HTTP_STATUS_AFTER_RESTART=%{http_code}\n" https://127.0.0.1:9870/api/v1/setup/status' | tee -a "$LOG"
dexec rc-service clouddesk stop | tee -a "$LOG" || true
dexec rc-service clouddesk status | tee -a "$LOG" || true
dexec rc-service clouddesk start | tee -a "$LOG"
dexec rc-service clouddesk status | tee -a "$LOG"
dexec sh -c 'pgrep -c clouddeskd || echo 0' | tee -a "$LOG"
dexec sh -c 'pgrep -c cloudesk-privd || echo 0' | tee -a "$LOG"

step "PART 24: ENABLEMENT (real rc-update)"
dexec rc-update show default | tee -a "$LOG"

step "PART 35: OPENRC LOGGING"
dexec sh -c 'tail -n 40 /var/log/clouddesk/clouddesk.log 2>&1' | tee -a "$LOG"
dexec sh -c 'tail -n 40 /var/log/clouddesk/privd.log 2>&1' | tee -a "$LOG"

step "PART 27: DATA PRESERVATION MARKER (before reinstall)"
dexec sh -c 'echo "marker-before-reinstall" > /var/lib/clouddesk/PHASE10_MARKER.txt; grep -c "port = 9870" /etc/clouddesk/clouddesk.toml' | tee -a "$LOG"
dexec sh -c 'openssl x509 -in /etc/clouddesk/tls/server.crt -noout -fingerprint -sha256' | tee -a "$LOG"

step "PART 26: REINSTALL IDEMPOTENCE"
REINSTALL_EXIT=0
install_env sh -c '/repo/installer/install.sh' >>"$LOG" 2>&1 || REINSTALL_EXIT=$?
rec "reinstall exit code: $REINSTALL_EXIT"
dexec sh -c 'id -u clouddesk; getent passwd clouddesk | wc -l' | tee -a "$LOG"
dexec rc-service clouddesk status | tee -a "$LOG"
dexec sh -c 'test -f /var/lib/clouddesk/PHASE10_MARKER.txt && cat /var/lib/clouddesk/PHASE10_MARKER.txt || echo "MARKER LOST"' | tee -a "$LOG"
dexec sh -c 'openssl x509 -in /etc/clouddesk/tls/server.crt -noout -fingerprint -sha256' | tee -a "$LOG"
dexec sh -c 'curl -sk -o /dev/null -w "HTTP_STATUS_AFTER_REINSTALL=%{http_code}\n" https://127.0.0.1:9870/api/v1/setup/status' | tee -a "$LOG"
dexec sh -c 'stat -c "%a %U:%G %n" /var/lib/clouddesk/clouddesk.db' | tee -a "$LOG"

step "PART 18: PRIVD RE-VERIFICATION"
dexec sh -c 'ps -o pid,user,group,args -C cloudesk-privd 2>/dev/null || ps aux | grep "[c]loudesk-privd"' | tee -a "$LOG"
dexec sh -c 'stat -c "%U:%G %a %n" /run/clouddesk' | tee -a "$LOG"
dexec sh -c 'ls -l /run/clouddesk/privd.sock 2>&1' | tee -a "$LOG"

step "UNINSTALL (non-purge, then purge)"
install_env sh -c '/repo/installer/uninstall.sh' >>"$LOG" 2>&1 || rec "uninstall (no purge) exit: $?"
dexec sh -c 'rc-service clouddesk status 2>&1; test -e /opt/clouddesk && echo "binaries PRESENT (unexpected)" || echo "binaries removed"' | tee -a "$LOG"
dexec sh -c 'test -e /etc/clouddesk && echo "config PRESERVED (expected without --purge)" || echo "config REMOVED"' | tee -a "$LOG"
dexec sh -c 'test -e /var/lib/clouddesk/clouddesk.db && echo "db PRESERVED (expected without --purge)" || echo "db REMOVED"' | tee -a "$LOG"
install_env sh -c '/repo/installer/uninstall.sh --purge' >>"$LOG" 2>&1 || rec "uninstall --purge exit: $?"
dexec sh -c 'test -e /etc/clouddesk && echo "config PRESENT (unexpected after purge)" || echo "config purged"' | tee -a "$LOG"
dexec sh -c 'test -e /var/lib/clouddesk && echo "data PRESENT (unexpected after purge)" || echo "data purged"' | tee -a "$LOG"
dexec sh -c 'id clouddesk 2>&1' | tee -a "$LOG"

rec ""
rec "=== DONE: $LABEL ==="
