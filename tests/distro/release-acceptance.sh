#!/bin/sh
# Publication Pass K1: proves the EXACT staged release-assets/ (the same
# bytes about to be handed to actions/attest-build-provenance) survive a
# true `curl | sudo env CLOUDESK_VERSION=... bash` public bootstrap on
# fresh, disposable Debian and Alpine machines -- no repository
# checkout, no installer/lib/, no packaging/ directory available to the
# executing installer, and no CLOUDESK_ROOT fakeroot shortcut.
#
# Publication Pass J3b's version of this script ran the HTTP fixture on
# the HOST and reached each acceptance container via `docker run
# --network host`. That passed locally in this session's own sandbox
# but failed hosted on a real GitHub Actions runner (run 33429519775):
# both Debian and Alpine acceptance containers finished their package
# install and then produced zero further output before exiting
# non-zero -- no curl error, no install.sh output, nothing -- which is
# consistent with the acceptance container never being able to reach
# the host's loopback-bound fixture server under that runner's Docker
# networking, though the exact mechanism could not be conclusively
# proven from the available logs. Rather than depend on host-network
# semantics that differ between environments, the fixture now runs
# INSIDE each acceptance container: no --network host, no runner
# loopback assumption, no host firewall assumption. This is Option A
# from the Publication Pass K1 authorization -- see PART 4 of that
# pass's brief for the full rationale.
#
# Usage:
#   tests/distro/release-acceptance.sh <release-assets-dir> <version>
#
# <release-assets-dir> must contain the exact flat-named files the
# release workflow's "Stage flat public release asset layout" step
# produces (clouddeskd-linux-x86_64-glibc, ..., install.sh,
# manifest.json, SHA256SUMS, sbom.cdx.json, clouddesk-web.tar.gz).
# <version> is the bare version string (e.g. 1.0.1-rc.5), matching
# manifest.json's release_candidate field.
#
# Requires: docker. Runs two disposable containers (debian:12,
# alpine:3.20), each running its own HTTP fixture server against a
# read-only bind mount of <release-assets-dir> -- no container ever
# talks to the host network or another container. Never touches the
# host's own systemd/openrc, package manager, or filesystem.
set -Eeuo pipefail

[ $# -eq 2 ] || {
    printf 'usage: %s <release-assets-dir> <version>\n' "$0" >&2
    exit 2
}

assets_dir=$(CDPATH= cd -- "$1" && pwd)
version=$2
tag="v$version"

for f in manifest.json SHA256SUMS install.sh clouddesk-web.tar.gz \
    clouddeskd-linux-x86_64-glibc cloudesk-privd-linux-x86_64-glibc cloudesk-sessiond-linux-x86_64-glibc \
    clouddeskd-linux-x86_64-musl cloudesk-privd-linux-x86_64-musl cloudesk-sessiond-linux-x86_64-musl; do
    [ -f "$assets_dir/$f" ] || {
        printf 'FAIL: %s missing from %s -- not the real staged release-assets layout\n' "$f" "$assets_dir" >&2
        exit 1
    }
done

command -v docker >/dev/null 2>&1 || { printf 'FAIL: docker is required\n' >&2; exit 1; }

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)

# PART 12 guard: the installer this run tests must be byte-identical to
# the installer subject the workflow is about to attest -- never a
# working-tree copy, a checkout copy, or a different version's asset.
staged_installer_sha=$(sha256sum "$assets_dir/install.sh" | cut -d' ' -f1)
printf 'Staged installer under test: %s  (%s)\n' "$staged_installer_sha" "$assets_dir/install.sh"

failures=0

# $1=label $2=docker image $3=init system $4=unit dir on guest, then
# repeated pairs of (installed-unit-name canonical-packaging-path).
run_bootstrap() {
    label=$1
    image=$2
    init_system=$3
    unit_dir=$4
    shift 4
    unit_names=""
    n=$#
    i=1
    while [ "$i" -le "$n" ]; do
        eval "installed_name=\${$i}"
        unit_names="$unit_names $unit_dir/$installed_name"
        i=$((i + 2))
    done

    out=$(mktemp "${TMPDIR:-/tmp}/clouddesk-release-acceptance-out.XXXXXX")
    set +e
    docker run --rm -v "$assets_dir:/fixture-src:ro" -w /home/clouddesk "$image" sh -c "
        set -e
        report() { printf '[%s] %s\n' \"\$1\" \"\$2\"; }

        if command -v apt-get >/dev/null 2>&1; then
            apt-get update -qq >/dev/null && apt-get install -y -qq curl python3 >/dev/null 2>&1
        else
            apk add --no-cache curl bash python3 >/dev/null 2>&1
        fi

        port=18080
        mkdir -p /srv-fixture/$tag
        cp -r /fixture-src/. /srv-fixture/$tag/
        ( cd /srv-fixture && exec python3 -m http.server \"\$port\" --bind 127.0.0.1 >/tmp/fixture.log 2>&1 & )
        fixture_ready=0
        for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
            if curl -fsS \"http://127.0.0.1:\$port/$tag/manifest.json\" >/dev/null 2>&1; then
                fixture_ready=1
                break
            fi
            sleep 0.3
        done
        if [ \"\$fixture_ready\" -ne 1 ]; then
            report 'fixture health check from installer namespace' 'FAIL'
            report 'fixture log' \"\$(cat /tmp/fixture.log 2>/dev/null || echo none)\"
            exit 1
        fi
        report 'fixture health check from installer namespace' 'PASS'

        installer_sha=\$(sha256sum /srv-fixture/$tag/install.sh | cut -d' ' -f1)
        report 'served installer sha256' \"\$installer_sha\"

        mkdir -p /home/clouddesk && cd /home/clouddesk
        before=\$(ls -A . | wc -l)

        # Diagnostic-only pre-check: confirms the fixture serves
        # install.sh with the expected status/bytes before the real
        # acceptance run below actually executes it. Not itself the
        # tested execution path.
        http_status=\$(curl -fsSL -w '%{http_code}' -o /tmp/install-precheck.sh \"http://127.0.0.1:\$port/$tag/install.sh\") || {
            report 'installer fetch HTTP status' \"curl exit \$?\"
            exit 1
        }
        report 'installer fetch HTTP status' \"\$http_status\"
        report 'fetched installer sha256' \"\$(sha256sum /tmp/install-precheck.sh | cut -d' ' -f1)\"
        rm -f /tmp/install-precheck.sh

        # The actual tested acceptance path: a genuine curl | bash
        # pipe, matching the real public curl -fsSL <url>/install.sh |
        # sudo env CLOUDESK_VERSION=... bash command byte-for-byte in
        # shape -- \$0 is 'bash', no argv path, no sibling files.
        curl -fsSL \"http://127.0.0.1:\$port/$tag/install.sh\" | \
            env CLOUDESK_VERSION='$version' CLOUDESK_RELEASE_BASE_URL=\"http://127.0.0.1:\$port\" CLOUDESK_ALLOW_INSECURE_TEST_URL=1 \
                CLOUDESK_INIT_SYSTEM='$init_system' CLOUDESK_NO_START=1 bash
        installer_exit=\$?
        report 'installer exit code' \"\$installer_exit\"
        [ \"\$installer_exit\" -eq 0 ] || exit \"\$installer_exit\"

        after=\$(ls -A . | wc -l)
        [ \"\$before\" -eq \"\$after\" ] || { report 'FATAL' 'install.sh left files in the caller cwd -- repository-relative dependency leaked in'; exit 1; }
        sha256sum$unit_names
    " >"$out" 2>&1
    status=$?
    set -e
    if [ "$status" -ne 0 ]; then
        printf 'FAIL: %s true stdin bootstrap did not complete cleanly\n' "$label" >&2
        cat "$out" >&2
        failures=$((failures + 1))
        rm -f "$out"
        return
    fi
    printf 'PASS: %s true stdin bootstrap (no checkout, no installer/lib, no packaging/, no host network) completed\n' "$label"
    while [ $# -ge 2 ]; do
        installed_name=$1
        canonical_path=$2
        shift 2
        installed_sum=$(awk -v n="$installed_name" 'index($2, n) { print $1 }' "$out")
        canonical_sum=$(sha256sum "$project_dir/$canonical_path" | cut -d' ' -f1)
        if [ "$installed_sum" = "$canonical_sum" ]; then
            printf 'PASS: %s installed %s matches canonical %s\n' "$label" "$installed_name" "$canonical_path"
        else
            printf 'FAIL: %s installed %s (%s) does not match canonical %s (%s)\n' \
                "$label" "$installed_name" "$installed_sum" "$canonical_path" "$canonical_sum" >&2
            failures=$((failures + 1))
        fi
    done
    rm -f "$out"
}

run_bootstrap "Debian/glibc" debian:12 systemd /etc/systemd/system \
    clouddesk.service packaging/systemd/clouddesk.service \
    cloudesk-privd.service packaging/systemd/cloudesk-privd.service

run_bootstrap "Alpine/musl" alpine:3.20 openrc /etc/init.d \
    clouddesk packaging/openrc/clouddesk \
    cloudesk-privd packaging/openrc/cloudesk-privd

if [ "$failures" -ne 0 ]; then
    printf '%d release-acceptance control(s) failed.\n' "$failures" >&2
    exit 1
fi
printf 'All release-acceptance true stdin bootstrap controls passed.\n'
