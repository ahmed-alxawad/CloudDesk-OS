#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/clouddesk-installer.XXXXXX")
trap 'rm -rf -- "$test_root"' EXIT INT TERM

mkdir -p "$test_root/artifacts/web"
printf '#!/bin/sh\nexit 0\n' >"$test_root/artifacts/clouddeskd"
chmod 0755 "$test_root/artifacts/clouddeskd"
cp "$test_root/artifacts/clouddeskd" "$test_root/artifacts/cloudesk-privd"
cp "$test_root/artifacts/clouddeskd" "$test_root/artifacts/cloudesk-sessiond"
printf '<!doctype html><title>CloudDesk</title>\n' >"$test_root/artifacts/web/index.html"

for distro in debian ubuntu rhel fedora rocky almalinux arch alpine; do
    target="$test_root/$distro"
    CLOUDESK_ROOT="$target" \
    CLOUDESK_DISTRO_ID="$distro" \
    CLOUDESK_INIT_SYSTEM=none \
    CLOUDESK_SKIP_PACKAGES=1 \
    CLOUDESK_BINARY="$test_root/artifacts/clouddeskd" \
    CLOUDESK_PRIVD_BINARY="$test_root/artifacts/cloudesk-privd" \
    CLOUDESK_SESSIOND_BINARY="$test_root/artifacts/cloudesk-sessiond" \
    CLOUDESK_WEB_DIR="$test_root/artifacts/web" \
        "$project_dir/installer/install.sh" >/dev/null

    test -x "$target/opt/clouddesk/bin/clouddeskd"
    test -x "$target/opt/clouddesk/bin/cloudesk-privd"
    test -x "$target/opt/clouddesk/bin/cloudesk-sessiond"
    test -f "$target/opt/clouddesk/web/index.html"
    test -s "$target/etc/clouddesk/tls/server.crt"
    test -s "$target/etc/clouddesk/keys/master.key"
    test "$(wc -c <"$target/etc/clouddesk/keys/privd-grant.key")" -eq 32
    test -s "$target/var/lib/clouddesk/bootstrap.secret"
    grep -q 'port = 9870' "$target/etc/clouddesk/clouddesk.toml"
done

printf 'Installer layout passed for all official distribution IDs.\n'
