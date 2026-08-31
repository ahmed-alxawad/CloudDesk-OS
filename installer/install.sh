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

# Publication Pass J1: distro/service-manager detection and per-distro
# package/account setup used to live in installer/lib/*.sh, sourced by
# path relative to $script_dir. That is only meaningful when install.sh
# is executed as a file with a real on-disk sibling directory -- it is
# not true for the documented `curl -fsSL <url> | sudo ... bash`
# bootstrap, where $0 is "bash" (or similar) and $script_dir resolves
# to the caller's arbitrary current working directory instead of
# installer/, producing a "No such file or directory" for
# lib/distro.sh before this script has done anything else. Embedded
# here instead so the one-command public bootstrap has no on-disk
# sibling-file dependency at all; installer/lib/*.sh no longer exists
# and install.sh is now the sole source of truth for this logic in
# both local/offline and public-bootstrap mode.
detect_distribution() {
    if [ -n "${CLOUDESK_DISTRO_ID:-}" ]; then
        distro_id=$CLOUDESK_DISTRO_ID
        distro_like=${CLOUDESK_DISTRO_LIKE:-}
    else
        os_release=${CLOUDESK_OS_RELEASE:-/etc/os-release}
        [ -r "$os_release" ] || return 1
        distro_id=$(sed -n 's/^ID=//p' "$os_release" | tr -d '"' | head -n 1)
        distro_like=$(sed -n 's/^ID_LIKE=//p' "$os_release" | tr -d '"' | head -n 1)
    fi

    case "$distro_id" in
        debian | ubuntu)
            distro_family=debian
            ;;
        rhel | rocky | almalinux)
            distro_family=rhel
            ;;
        fedora)
            distro_family=fedora
            ;;
        arch | archlinux)
            distro_family=arch
            ;;
        alpine)
            distro_family=alpine
            ;;
        *)
            case " $distro_like " in
                *" debian "*) distro_family=debian ;;
                *" rhel "* | *" fedora "*) distro_family=rhel ;;
                *" arch "*) distro_family=arch ;;
                *) return 1 ;;
            esac
            ;;
    esac

    export distro_id distro_family
}

detect_service_manager() {
    if [ -n "${CLOUDESK_INIT_SYSTEM:-}" ]; then
        init_system=$CLOUDESK_INIT_SYSTEM
    elif command -v systemctl >/dev/null 2>&1; then
        init_system=systemd
    elif command -v rc-update >/dev/null 2>&1; then
        init_system=openrc
    else
        init_system=none
    fi

    case "$init_system" in
        systemd | openrc | none) ;;
        *) return 1 ;;
    esac
    export init_system
}

install_packages() {
    case "$distro_family" in
        debian)
            export DEBIAN_FRONTEND=noninteractive
            apt-get update
            apt-get install -y --no-install-recommends ca-certificates openssh-client openssl sqlite3 util-linux
            ;;
        alpine)
            apk add --no-cache ca-certificates openssh-client-default openssl sqlite util-linux
            ;;
        fedora | rhel)
            dnf install -y ca-certificates openssh-clients openssl sqlite util-linux
            ;;
        arch)
            pacman -Syu --noconfirm --needed ca-certificates openssh openssl sqlite util-linux
            ;;
    esac
}

create_service_account() {
    case "$distro_family" in
        debian)
            useradd --system --home-dir /var/lib/clouddesk --shell /usr/sbin/nologin clouddesk
            ;;
        alpine)
            # Phase 10D found this live: unlike `useradd --system` on
            # every other distro family (Debian/RPM/Arch all
            # auto-create a matching same-named primary group), busybox
            # `adduser -S` on Alpine does NOT -- it silently falls back
            # to the shared `nogroup` (gid 65533) unless a group is
            # explicitly given, and that group must already exist.
            # Without this, every `chown clouddesk:clouddesk` later in
            # the installer failed with "unknown user/group", aborting
            # the install outright on every fresh Alpine system.
            addgroup -S clouddesk
            adduser -S -D -H -h /var/lib/clouddesk -s /sbin/nologin -G clouddesk clouddesk
            ;;
        fedora | rhel)
            useradd --system --home-dir /var/lib/clouddesk --shell /sbin/nologin clouddesk
            ;;
        arch)
            useradd --system --home-dir /var/lib/clouddesk --shell /usr/bin/nologin clouddesk
            ;;
    esac
}

detect_distribution || fail "unsupported Linux distribution"

# Phase 10D: select the correct pre-built release artifact by the SAME
# trusted distro_family classification detect_distribution just
# computed from /etc/os-release's own ID/ID_LIKE (or the equivalent
# CLOUDESK_DISTRO_ID/CLOUDESK_DISTRO_LIKE override) -- never arbitrary
# user-controlled interpolation. Alpine is musl-based; every other
# supported family is glibc-based; a glibc binary cannot even be
# loaded on Alpine at all (no /lib64/ld-linux-x86-64.so.2 exists
# there), confirmed live.
case "$distro_family" in
    alpine) artifact_family="linux-x86_64-musl" ;;
    *) artifact_family="linux-x86_64-glibc" ;;
esac
default_artifact_dir="$project_dir/dist/$artifact_family"
binary_source=${CLOUDESK_BINARY:-$default_artifact_dir/clouddeskd}
privd_source=${CLOUDESK_PRIVD_BINARY:-$default_artifact_dir/cloudesk-privd}
sessiond_source=${CLOUDESK_SESSIOND_BINARY:-$default_artifact_dir/cloudesk-sessiond}

# Publication Pass B: GOAL.md G1's `curl -fsSL <official-install-url> |
# sudo bash` contract requires this script to fetch CloudDesk's own
# release artifacts itself on a fresh machine that has nothing built
# locally -- an explicit CLOUDESK_VERSION is the trigger, not any
# filesystem probing (a fragile signal Publication Pass A's design
# review explicitly rejected). When unset, behavior is byte-for-byte
# the existing local/offline path above, untouched.
release_version=${CLOUDESK_VERSION:-}
if [ -n "$release_version" ]; then
    printf '%s' "$release_version" | grep -Eq '^v?[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$' \
        || fail "invalid CLOUDESK_VERSION format: $release_version"
    version_norm=${release_version#v}
    tag_ref="v$version_norm"

    release_base_url=${CLOUDESK_RELEASE_BASE_URL:-https://github.com/ahmed-alxawad/CloudDesk-OS/releases/download}
    allow_insecure_test_url=${CLOUDESK_ALLOW_INSECURE_TEST_URL:-0}
    case "$release_base_url" in
        https://*) curl_proto=https ;;
        http://*)
            [ "$allow_insecure_test_url" = 1 ] \
                || fail "release base URL must use https:// (set CLOUDESK_ALLOW_INSECURE_TEST_URL=1 only for local test fixtures, never in production)"
            curl_proto=http,https
            ;;
        *) fail "unsupported release base URL scheme: $release_base_url" ;;
    esac

    # --proto/--proto-redir reject a redirect that would downgrade the
    # transfer to a scheme outside this allowlist (e.g. an https URL
    # redirecting to http), not merely the initial request's own
    # scheme. --max-redirs bounds redirect chains against a loop.
    fetch_url() {
        curl --fail --show-error --silent --location \
            --proto "=$curl_proto" --proto-redir "=$curl_proto" \
            --connect-timeout 10 --max-time 180 --max-redirs 5 \
            -o "$2" "$1" || fail "download failed: $1"
    }

    fetch_tmp=$(mktemp -d "${TMPDIR:-/tmp}/clouddesk-fetch.XXXXXX")
    trap 'rm -rf "$fetch_tmp"' EXIT INT TERM

    release_url="$release_base_url/$tag_ref"
    fetch_url "$release_url/manifest.json" "$fetch_tmp/manifest.json"
    fetch_url "$release_url/SHA256SUMS" "$fetch_tmp/SHA256SUMS"

    # Deliberately not a general JSON parser: only two scalar fields
    # are ever read from this project's own machine-generated
    # manifest.json. SHA256SUMS remains the sole source of truth for
    # artifact hashes -- see docs/RELEASE_INTEGRITY.md's "Public-
    # download manifest/checksum model" section for why cross-
    # validating manifest-embedded hashes too was judged not worth the
    # added POSIX-sh JSON-parsing fragility.
    manifest_field() {
        grep -o "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$fetch_tmp/manifest.json" \
            | head -1 | sed -E 's/.*:[[:space:]]*"([^"]*)"/\1/'
    }
    manifest_version=$(manifest_field release_candidate)
    manifest_source=$(manifest_field source_commit)

    [ -n "$manifest_version" ] || fail "release manifest missing release_candidate field"
    [ "$manifest_version" = "$version_norm" ] \
        || fail "requested version $version_norm does not match published manifest version $manifest_version -- refusing to install a version-mismatched release"
    printf '%s' "$manifest_source" | grep -Eq '^[0-9a-f]{40}$' \
        || fail "release manifest has a malformed source_commit field"

    # Only this platform's own 3 binaries plus the one shared web
    # bundle -- never fetch or verify the other libc family's
    # binaries. Exactly 4 lines are required: a missing entry (e.g. an
    # attacker-truncated manifest) must fail closed, not silently skip
    # verification for whichever artifact it omitted.
    family_sums=$(grep -E "^[0-9a-f]{64}  ($artifact_family/(clouddeskd|cloudesk-privd|cloudesk-sessiond)|clouddesk-web\.tar\.gz)\$" "$fetch_tmp/SHA256SUMS") || :
    family_sums_count=$(printf '%s\n' "$family_sums" | grep -c . || :)
    [ "$family_sums_count" -eq 4 ] \
        || fail "release checksum manifest has $family_sums_count entr(y/ies) for $artifact_family, expected exactly 4 -- refusing to install against an incomplete manifest"

    mkdir -p "$fetch_tmp/$artifact_family"
    fetch_url "$release_url/clouddeskd-$artifact_family" "$fetch_tmp/$artifact_family/clouddeskd"
    fetch_url "$release_url/cloudesk-privd-$artifact_family" "$fetch_tmp/$artifact_family/cloudesk-privd"
    fetch_url "$release_url/cloudesk-sessiond-$artifact_family" "$fetch_tmp/$artifact_family/cloudesk-sessiond"
    fetch_url "$release_url/clouddesk-web.tar.gz" "$fetch_tmp/clouddesk-web.tar.gz"

    printf '%s\n' "$family_sums" >"$fetch_tmp/family.SHA256SUMS"
    (cd "$fetch_tmp" && sha256sum -c family.SHA256SUMS >/dev/null) \
        || fail "release artifact checksum verification failed -- refusing to install a corrupted or tampered release"

    mkdir -p "$fetch_tmp/web"
    tar -C "$fetch_tmp/web" -xzf "$fetch_tmp/clouddesk-web.tar.gz" || fail "failed to extract web bundle"

    binary_source="$fetch_tmp/$artifact_family/clouddeskd"
    privd_source="$fetch_tmp/$artifact_family/cloudesk-privd"
    sessiond_source="$fetch_tmp/$artifact_family/cloudesk-sessiond"
    web_source="$fetch_tmp/web"

    # Re-expressed as a per-directory SHA256SUMS keyed by simple
    # basenames, so the existing local-mode verify_artifact_checksum()
    # below performs its own independent, redundant re-verification of
    # these same three binaries -- defense in depth, no special-casing
    # needed there for either mode.
    awk -v fam="$artifact_family/" 'index($2, fam) == 1 { sub(fam, "", $2); print }' \
        "$fetch_tmp/family.SHA256SUMS" >"$fetch_tmp/$artifact_family/SHA256SUMS"
fi

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

# Publication Pass J1: service unit content used to be installed from
# packaging/systemd/*.service and packaging/openrc/* via $project_dir --
# another on-disk sibling-file dependency the public curl|bash
# bootstrap cannot satisfy (no checkout exists). Embedded verbatim here
# instead; tests/distro/installer-lib-sync.sh byte-compares these
# heredocs against packaging/systemd/*.service and packaging/openrc/*
# so the two never silently drift apart.
case "$init_system" in
    systemd)
        install -d -m 0755 "$(path /etc/systemd/system)"
        cat >"$(path /etc/systemd/system/cloudesk-privd.service)" <<'EOF'
[Unit]
Description=CloudDesk-OS narrow privileged helper
After=local-fs.target
Before=clouddesk.service

[Service]
Type=simple
User=root
Group=root
EnvironmentFile=/etc/clouddesk/privd.env
ExecStart=/opt/clouddesk/bin/cloudesk-privd --allowed-peer-uid ${CLOUDESK_UID} --socket-gid ${CLOUDESK_GID} --setpriv ${CLOUDESK_SETPRIV}
Restart=on-failure
RestartSec=5s
UMask=0077
NoNewPrivileges=no
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
ProtectHome=read-only
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
RestrictAddressFamilies=AF_UNIX
SystemCallArchitectures=native
# Phase 10A found this live: `/run` is a fresh tmpfs every boot, and
# nothing created `/run/clouddesk` before systemd itself tried to bind
# it in as a ReadWritePaths mount -- `ProtectSystem=strict`'s sandbox
# setup runs before the service's own first line ever executes, so the
# unit crash-looped with "Failed to set up mount namespacing:
# /run/clouddesk: No such file or directory" on every fresh boot.
# RuntimeDirectory has systemd itself create (and remove on stop) the
# directory with the service's own User/Group before sandboxing is
# applied, which is exactly what a fresh-install/fresh-boot needs.
RuntimeDirectory=clouddesk
RuntimeDirectoryMode=0750

[Install]
WantedBy=multi-user.target
EOF
        chmod 0644 "$(path /etc/systemd/system/cloudesk-privd.service)"
        cat >"$(path /etc/systemd/system/clouddesk.service)" <<'EOF'
[Unit]
Description=CloudDesk-OS core service
After=network-online.target cloudesk-privd.service
Wants=network-online.target cloudesk-privd.service

[Service]
Type=simple
User=clouddesk
Group=clouddesk
ExecStart=/opt/clouddesk/bin/clouddeskd serve --config /etc/clouddesk/clouddesk.toml
Restart=on-failure
RestartSec=5s
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
ProtectHome=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
CapabilityBoundingSet=
AmbientCapabilities=
ReadWritePaths=/var/lib/clouddesk /var/log/clouddesk

[Install]
WantedBy=multi-user.target
EOF
        chmod 0644 "$(path /etc/systemd/system/clouddesk.service)"
        if [ -z "$root_prefix" ] && [ "${CLOUDESK_NO_START:-0}" != 1 ]; then
            systemctl daemon-reload
            systemctl enable --now cloudesk-privd.service clouddesk.service
        fi
        ;;
    openrc)
        install -d -m 0755 "$(path /etc/init.d)"
        cat >"$(path /etc/init.d/cloudesk-privd)" <<'EOF'
#!/sbin/openrc-run

name="CloudDesk-OS privileged helper"
description="CloudDesk-OS narrow privileged helper"
command="/opt/clouddesk/bin/cloudesk-privd"
command_args="--allowed-peer-uid ${CLOUDESK_UID} --socket-gid ${CLOUDESK_GID} --setpriv ${CLOUDESK_SETPRIV}"
command_user="root:root"
command_background="yes"
pidfile="/run/cloudesk-privd.pid"
output_log="/var/log/clouddesk/privd.log"
error_log="/var/log/clouddesk/privd.log"

depend() {
    need localmount
    before clouddesk
}

start_pre() {
    : "${CLOUDESK_UID:?missing CLOUDESK_UID in /etc/conf.d/cloudesk-privd}"
    : "${CLOUDESK_GID:?missing CLOUDESK_GID in /etc/conf.d/cloudesk-privd}"
    : "${CLOUDESK_SETPRIV:?missing CLOUDESK_SETPRIV in /etc/conf.d/cloudesk-privd}"
    checkpath --directory --owner root:root --mode 0750 /run/clouddesk
}
EOF
        chmod 0755 "$(path /etc/init.d/cloudesk-privd)"
        cat >"$(path /etc/init.d/clouddesk)" <<'EOF'
#!/sbin/openrc-run

name="CloudDesk-OS"
description="CloudDesk-OS core service"
command="/opt/clouddesk/bin/clouddeskd"
command_args="serve --config /etc/clouddesk/clouddesk.toml"
command_user="clouddesk:clouddesk"
command_background="yes"
pidfile="/run/clouddesk.pid"
output_log="/var/log/clouddesk/clouddesk.log"
error_log="/var/log/clouddesk/clouddesk.log"

depend() {
    need net
    need cloudesk-privd
    after firewall
}

start_pre() {
    checkpath --directory --owner clouddesk:clouddesk --mode 0750 /run/clouddesk
}
EOF
        chmod 0755 "$(path /etc/init.d/clouddesk)"
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
