#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
root_prefix=${CLOUDESK_ROOT:-}
purge=0

for arg in "$@"; do
    case "$arg" in
        --purge) purge=1 ;;
        --help|-h)
            printf "Usage: %s [--purge]\n" "$0"
            printf "  --purge  Remove all user data, SQLite database, keys, and logs.\n"
            exit 0
            ;;
    esac
done

fail() {
    printf 'CloudDesk uninstaller: %s\n' "$*" >&2
    exit 1
}

path() {
    printf '%s%s' "$root_prefix" "$1"
}

[ -n "$root_prefix" ] || [ "$(id -u)" -eq 0 ] || fail "run as root"

# Stop and disable services
if command -v systemctl >/dev/null 2>&1; then
    systemctl stop clouddesk.service cloudesk-privd.service 2>/dev/null || true
    systemctl disable clouddesk.service cloudesk-privd.service 2>/dev/null || true
    rm -f "$(path /etc/systemd/system/clouddesk.service)" \
          "$(path /etc/systemd/system/cloudesk-privd.service)"
    systemctl daemon-reload 2>/dev/null || true
elif command -v rc-service >/dev/null 2>&1; then
    rc-service clouddesk stop 2>/dev/null || true
    rc-service cloudesk-privd stop 2>/dev/null || true
    rc-update del clouddesk default 2>/dev/null || true
    rc-update del cloudesk-privd default 2>/dev/null || true
    rm -f "$(path /etc/init.d/clouddesk)" "$(path /etc/init.d/cloudesk-privd)" \
          "$(path /etc/conf.d/cloudesk-privd)"
fi

# Remove application binaries and web assets
rm -rf "$(path /opt/clouddesk)"
rm -f "$(path /etc/clouddesk/privd.env)"

if [ "$purge" -eq 1 ]; then
    printf 'Purging all CloudDesk configuration, databases, secrets, and logs...\n'
    rm -rf "$(path /etc/clouddesk)"
    rm -rf "$(path /var/lib/clouddesk)"
    rm -rf "$(path /var/log/clouddesk)"
    rm -rf "$(path /run/clouddesk)"
    
    if [ -z "$root_prefix" ]; then
        if command -v deluser >/dev/null 2>&1; then
            deluser clouddesk 2>/dev/null || true
        elif command -v userdel >/dev/null 2>&1; then
            userdel clouddesk 2>/dev/null || true
        fi
    fi
    printf 'CloudDesk completely purged.\n'
else
    printf 'CloudDesk binaries and services removed.\n'
    printf 'Configuration, database, and encryption keys preserved in:\n'
    printf '  - %s\n' "$(path /etc/clouddesk)"
    printf '  - %s\n' "$(path /var/lib/clouddesk)"
    printf '  - %s\n' "$(path /var/log/clouddesk)"
    printf 'To permanently delete all data, rerun with: %s --purge\n' "$0"
fi
