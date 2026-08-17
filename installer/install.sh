#!/bin/sh
set -eu

umask 077

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
root_prefix=${CLOUDESK_ROOT:-}
binary_source=${CLOUDESK_BINARY:-$project_dir/target/release/clouddeskd}
privd_source=${CLOUDESK_PRIVD_BINARY:-$project_dir/target/release/cloudesk-privd}
sessiond_source=${CLOUDESK_SESSIOND_BINARY:-$project_dir/target/release/cloudesk-sessiond}
web_source=${CLOUDESK_WEB_DIR:-$project_dir/apps/web/dist}

fail() {
    printf 'CloudDesk installer: %s\n' "$*" >&2
    exit 1
}

path() {
    printf '%s%s' "$root_prefix" "$1"
}

[ -n "$root_prefix" ] || [ "$(id -u)" -eq 0 ] || fail "run as root"
[ -f "$binary_source" ] || fail "missing release binary: $binary_source"
[ -f "$privd_source" ] || fail "missing privileged helper: $privd_source"
[ -f "$sessiond_source" ] || fail "missing session worker: $sessiond_source"
[ -f "$web_source/index.html" ] || fail "missing frontend build: $web_source/index.html"

# shellcheck source=installer/lib/distro.sh
. "$script_dir/lib/distro.sh"
detect_distribution || fail "unsupported Linux distribution"
detect_service_manager || fail "unsupported service manager"

# shellcheck source=/dev/null
. "$script_dir/lib/$distro_family.sh"

if [ "${CLOUDESK_SKIP_PACKAGES:-0}" != 1 ]; then
    install_packages
fi

if [ -z "$root_prefix" ] && ! id clouddesk >/dev/null 2>&1; then
    create_service_account
fi

install -d -m 0755 "$(path /opt/clouddesk/bin)" "$(path /opt/clouddesk/web)"
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
    host_name=${CLOUDESK_HOSTNAME:-$(hostname -f 2>/dev/null || hostname)}
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
