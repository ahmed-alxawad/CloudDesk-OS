#!/bin/sh
set -eu

umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
root_prefix=${CLOUDESK_ROOT:-}
web_source=${CLOUDESK_WEB_DIR:-$project_dir/apps/web/dist}

fail() {
    printf 'CloudDesk installer: %s\n' "$*" >&2
    exit 1
}

path() {
    printf '%s%s' "$root_prefix" "$1"
}

[ -n "$root_prefix" ] || [ "$(id -u)" -eq 0 ] || fail "run as root"

# shellcheck source=installer/lib/distro.sh
. "$script_dir/lib/distro.sh"
detect_distribution || fail "unsupported Linux distribution"

# Phase 10D: select the correct pre-built release artifact by the SAME
# trusted distro_family classification detect_distribution just
# computed from /etc/os-release's own ID/ID_LIKE (or the equivalent
# CLOUDESK_DISTRO_ID/CLOUDESK_DISTRO_LIKE override) -- never arbitrary
# user-controlled interpolation. Alpine is musl-based; every other
# supported family is glibc-based; a glibc binary cannot even be
# loaded on Alpine at all (no /lib64/ld-linux-x86-64.so.2 exists
# there), confirmed live. This exists so a real curl-fetch install
# picks the right artifact by construction, never discovering a libc
# mismatch only at runtime.
case "$distro_family" in
    alpine) default_artifact_dir="$project_dir/dist/linux-x86_64-musl" ;;
    *) default_artifact_dir="$project_dir/dist/linux-x86_64-glibc" ;;
esac
binary_source=${CLOUDESK_BINARY:-$default_artifact_dir/clouddeskd}
privd_source=${CLOUDESK_PRIVD_BINARY:-$default_artifact_dir/cloudesk-privd}
sessiond_source=${CLOUDESK_SESSIOND_BINARY:-$default_artifact_dir/cloudesk-sessiond}

detect_service_manager || fail "unsupported service manager"

[ -f "$binary_source" ] || fail "missing release binary: $binary_source"
[ -f "$privd_source" ] || fail "missing privileged helper: $privd_source"
[ -f "$sessiond_source" ] || fail "missing session worker: $sessiond_source"
[ -f "$web_source/index.html" ] || fail "missing frontend build: $web_source/index.html"

# Phase 17A: verify each selected release artifact against a
# SHA256SUMS manifest when one is present in *that artifact's own*
# directory -- the file layout every real `packaging/build-release*.sh`
# run produces, and the same layout a future remote-fetch installer
# would download before ever reaching this script. Fails closed: any
# mismatch aborts before install_packages or any privileged step runs,
# never a warning that lets an unverified or corrupted binary proceed.
# No manifest present in that specific directory (e.g. a developer's
# CLOUDESK_BINARY override pointing at a raw `cargo build` output, or a
# test fixture in its own scratch directory) is a distinct, accepted
# case -- verification is skipped for that binary, not failed, since
# there is nothing to verify against. Deliberately keyed off each
# artifact's own directory rather than always `$default_artifact_dir`:
# an override pointing entirely outside the real release artifacts
# (as every non-release test in this repo does) must never be checked
# against an unrelated manifest that happens to exist for a different
# artifact entirely.
verify_artifact_checksum() {
    label=$1
    artifact_path=$2
    artifact_dir=$(dirname "$artifact_path")
    sums_file="$artifact_dir/SHA256SUMS"
    [ -f "$sums_file" ] || return 0
    artifact_name=$(basename "$artifact_path")
    expected=$(awk -v name="$artifact_name" '$2 == name { print $1; found=1 } END { exit !found }' "$sums_file") \
        || fail "$label: $artifact_name has no entry in $sums_file"
    actual=$(sha256sum "$artifact_path" | cut -d' ' -f1)
    if [ "$actual" != "$expected" ]; then
        fail "$label checksum mismatch: $artifact_path (expected $expected, got $actual) -- refusing to install a corrupted or tampered artifact"
    fi
}

verify_artifact_checksum "release binary" "$binary_source"
verify_artifact_checksum "privileged helper" "$privd_source"
verify_artifact_checksum "session worker" "$sessiond_source"

# shellcheck source=/dev/null
. "$script_dir/lib/$distro_family.sh"

if [ "${CLOUDESK_SKIP_PACKAGES:-0}" != 1 ]; then
    install_packages
fi

if [ -z "$root_prefix" ] && ! id clouddesk >/dev/null 2>&1; then
    create_service_account
fi

# Phase 10D found this live: BusyBox's `install -d` (Alpine's `install`,
# unlike GNU coreutils) applies `-m` only to the directories named
# explicitly here -- an implicit parent it has to auto-create gets the
# process's own umask instead. `/opt/clouddesk` was never named on its
# own, only its `bin`/`web` children, so with this script's own
# `umask 077` it came out 0700 root-only on Alpine -- unreadable by
# the clouddesk service account entirely, identical in shape to the
# /etc/clouddesk directory-traversal defect Phase 10A found (this is
# the same class of bug, on the one remaining path that was still an
# implicit parent). `/etc/clouddesk` and `/var/lib/clouddesk` already
# name themselves explicitly below and were unaffected.
install -d -m 0755 "$(path /opt/clouddesk)" "$(path /opt/clouddesk/bin)" "$(path /opt/clouddesk/web)"
install -d -m 0750 "$(path /etc/clouddesk)" "$(path /etc/clouddesk/tls)" \
    "$(path /etc/clouddesk/keys)" "$(path /etc/clouddesk/policy.d)"
install -d -m 0750 "$(path /var/lib/clouddesk)" "$(path /var/lib/clouddesk/vault)" \
    "$(path /var/lib/clouddesk/users)" "$(path /var/lib/clouddesk/cache)" \
    "$(path /var/lib/clouddesk/transfers)" "$(path /var/log/clouddesk)"

install -m 0755 "$binary_source" "$(path /opt/clouddesk/bin/clouddeskd)"
install -m 0755 "$privd_source" "$(path /opt/clouddesk/bin/cloudesk-privd)"
install -m 0755 "$sessiond_source" "$(path /opt/clouddesk/bin/cloudesk-sessiond)"
cp -R "$web_source/." "$(path /opt/clouddesk/web/)"
find "$(path /opt/clouddesk/web)" -type d -exec chmod 0755 {} \;
find "$(path /opt/clouddesk/web)" -type f -exec chmod 0644 {} \;

tls_key=$(path /etc/clouddesk/tls/server.key)
tls_cert=$(path /etc/clouddesk/tls/server.crt)
if [ ! -s "$tls_key" ] || [ ! -s "$tls_cert" ]; then
    # Phase 10A found this live: a minimal Fedora/RHEL-family install
    # has no `hostname` command at all (it isn't in any package this
    # installer requires), so this failed with "command not found"
    # before ever reaching TLS generation. `uname -n` is POSIX,
    # part of coreutils, and present on every target distro family
    # unconditionally -- used as the fallback `hostname` itself always
    # was meant to be here.
    host_name=${CLOUDESK_HOSTNAME:-$(hostname -f 2>/dev/null || hostname 2>/dev/null || uname -n)}
    openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 397 \
        -keyout "$tls_key" -out "$tls_cert" -subj "/CN=$host_name" \
        -addext "subjectAltName=DNS:$host_name,IP:127.0.0.1" >/dev/null 2>&1
fi
chmod 0640 "$tls_key"
chmod 0644 "$tls_cert"

master_key=$(path /etc/clouddesk/keys/master.key)
if [ ! -s "$master_key" ]; then
    openssl rand 32 >"$master_key"
fi
chmod 0600 "$master_key"

grant_key=$(path /etc/clouddesk/keys/privd-grant.key)
if [ ! -s "$grant_key" ]; then
    openssl rand 32 >"$grant_key"
fi
chmod 0600 "$grant_key"

bootstrap_file=$(path /var/lib/clouddesk/bootstrap.secret)
if [ ! -s "$bootstrap_file" ]; then
    openssl rand -base64 32 >"$bootstrap_file"
fi
chmod 0600 "$bootstrap_file"

config_file=$(path /etc/clouddesk/clouddesk.toml)
cat >"$config_file" <<EOF
[server]
address = "0.0.0.0"
port = 9870
development_http = false

[tls]
certificate = "/etc/clouddesk/tls/server.crt"
private_key = "/etc/clouddesk/tls/server.key"

[security]
master_key = "/etc/clouddesk/keys/master.key"
bootstrap_secret = "/var/lib/clouddesk/bootstrap.secret"

[privilege]
enabled = true
socket = "/run/clouddesk/privd.sock"
grant_key = "/etc/clouddesk/keys/privd-grant.key"

[database]
url = "sqlite:///var/lib/clouddesk/clouddesk.db"
max_connections = 5

[web]
static_dir = "/opt/clouddesk/web"
EOF
chmod 0640 "$config_file"

if [ -z "$root_prefix" ]; then
    clouddesk_uid=$(id -u clouddesk)
    clouddesk_gid=$(id -g clouddesk)
    setpriv_path=$(command -v setpriv || true)
    [ -n "$setpriv_path" ] || fail "setpriv is required but was not found"

    chown -R clouddesk:clouddesk /var/lib/clouddesk /var/log/clouddesk
    # The directories themselves must also grant the clouddesk group
    # traversal (x), not just the files inside -- Phase 10A found this
    # missing: with /etc/clouddesk left root:root 0750, the clouddesk
    # service account could not open any file inside it regardless of
    # the file's own owner/mode, so both `clouddeskd migrate` here and
    # the real clouddesk.service (which runs as User=clouddesk) failed
    # identically with "Permission denied" on a fresh install.
    chown root:clouddesk /etc/clouddesk /etc/clouddesk/tls /etc/clouddesk/keys
    chown root:clouddesk /etc/clouddesk/clouddesk.toml /etc/clouddesk/tls/server.key \
        /etc/clouddesk/keys/master.key /etc/clouddesk/keys/privd-grant.key
    chmod 0640 /etc/clouddesk/keys/master.key /etc/clouddesk/keys/privd-grant.key

    cat >/etc/clouddesk/privd.env <<EOF
CLOUDESK_UID=$clouddesk_uid
CLOUDESK_GID=$clouddesk_gid
CLOUDESK_SETPRIV=$setpriv_path
EOF
    chown root:root /etc/clouddesk/privd.env
    chmod 0600 /etc/clouddesk/privd.env

    if command -v runuser >/dev/null 2>&1; then
        runuser -u clouddesk -- /opt/clouddesk/bin/clouddeskd migrate --config /etc/clouddesk/clouddesk.toml
    else
        su -s /bin/sh clouddesk -c "/opt/clouddesk/bin/clouddeskd migrate --config /etc/clouddesk/clouddesk.toml"
    fi
    # Phase 10C found this live: this migrate step is the only place
    # the SQLite database gets created, and its mode was left entirely
    # to whatever umask the shell that ran it happened to have --
    # every other secret this installer creates (master key, grant
    # key, bootstrap secret) gets an explicit chmod right after
    # creation for exactly this reason, but the database was missed.
    # On Debian/Ubuntu/Fedora/RHEL-family, `runuser` inherits this
    # script's own `umask 077` (set at the very top), so it happened
    # to come out 0600 anyway -- but confirmed live on Arch Linux,
    # `runuser`'s own PAM stack resets the umask to 0022 regardless of
    # the caller's, producing a world-readable 0644 database
    # containing vault_secrets/sessions/recovery_codes/etc. on a fresh
    # install. Explicit, not umask-dependent, like every sibling secret.
    chmod 0600 "$(path /var/lib/clouddesk/clouddesk.db)"
fi

case "$init_system" in
    systemd)
        install -D -m 0644 "$project_dir/packaging/systemd/cloudesk-privd.service" \
            "$(path /etc/systemd/system/cloudesk-privd.service)"
        install -D -m 0644 "$project_dir/packaging/systemd/clouddesk.service" \
            "$(path /etc/systemd/system/clouddesk.service)"
        if [ -z "$root_prefix" ] && [ "${CLOUDESK_NO_START:-0}" != 1 ]; then
            systemctl daemon-reload
            systemctl enable --now cloudesk-privd.service clouddesk.service
        fi
        ;;
    openrc)
        install -D -m 0755 "$project_dir/packaging/openrc/cloudesk-privd" \
            "$(path /etc/init.d/cloudesk-privd)"
        install -D -m 0755 "$project_dir/packaging/openrc/clouddesk" \
            "$(path /etc/init.d/clouddesk)"
        if [ -z "$root_prefix" ] && [ "${CLOUDESK_NO_START:-0}" != 1 ]; then
            install -D -m 0600 /etc/clouddesk/privd.env /etc/conf.d/cloudesk-privd
            rc-update add cloudesk-privd default
            rc-update add clouddesk default
            rc-service cloudesk-privd start
            rc-service clouddesk start
        fi
        ;;
    none) ;;
esac

printf '\nCloudDesk installed for %s (%s).\n' "$distro_id" "$distro_family"
printf 'Open https://<server-ip>:9870 and use this one-time bootstrap secret:\n\n'
cat "$bootstrap_file"
printf '\nThe browser warning is expected for the initial self-signed certificate.\n'
