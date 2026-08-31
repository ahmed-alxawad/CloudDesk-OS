#!/bin/sh
# Publication Pass J3b: proves the EXACT staged release-assets/ (the same
# bytes about to be handed to actions/attest-build-provenance) survive a
# true `curl | sudo env CLOUDESK_VERSION=... bash` public bootstrap on
# fresh, disposable Debian and Alpine machines -- no repository
# checkout, no installer/lib/, no packaging/ directory available to the
# executing installer, and no CLOUDESK_ROOT fakeroot shortcut.
#
# This exists because v1.0.1-rc.4's hosted release-attest workflow
# built, staged, and attested a release without ever exercising this
# exact execution path: it ran tests/distro/release-staging-validation.sh
# (a file-presence/manifest-consistency check, not an installer
# execution test) and nothing else before attesting. That gap is why a
# real fresh Debian VM running the documented public install command
# failed immediately on a missing installer/lib/distro.sh while every
# prior hosted release run had reported success. See
# installer/install.sh's own top-of-file comment and
# tests/distro/remote-fetch.sh for the underlying fix and its
# local/fakeroot-mode regression coverage; this script is the
# container-based, real-package-manager, real-service-account
# counterpart intended to run inside the hosted release workflow
# itself, gating attestation rather than merely existing as a
# developer-run local test.
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
# Requires: docker, python3, curl. Runs two disposable containers
# (debian:12, alpine:3.20); neither the containers nor the fixture
# HTTP server outlive this script. Never touches the host's own
# systemd/openrc, package manager, or filesystem beyond a temp dir.
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

port=$((20000 + $$ % 20000))
fixture=$(mktemp -d "${TMPDIR:-/tmp}/clouddesk-release-acceptance-fixture.XXXXXX")
rel="$fixture/$tag"
mkdir -p "$rel"
cp "$assets_dir"/* "$rel/"

(cd "$fixture" && exec python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) &
server_pid=$!
cleanup() {
    kill "$server_pid" 2>/dev/null || true
    rm -rf "$fixture"
}
trap cleanup EXIT INT TERM

for _ in 1 2 3 4 5 6 7 8 9 10; do
    curl -fsS "http://127.0.0.1:$port/$tag/manifest.json" >/dev/null 2>&1 && break
    sleep 0.3
done

failures=0

# $1=label $2=docker image $3=init system $4=unit dir on guest
# $5 $6=canonical packaging unit files (relative to project_dir) to
# diff the installed units against, space-separated pairs continue in $@
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
    docker run --rm --network host -w /home/clouddesk "$image" sh -c "
        set -e
        if command -v apt-get >/dev/null 2>&1; then apt-get update -qq >/dev/null && apt-get install -y -qq curl >/dev/null 2>&1
        else apk add --no-cache curl bash >/dev/null 2>&1
        fi
        mkdir -p /home/clouddesk && cd /home/clouddesk
        before=\$(ls -A . | wc -l)
        curl -fsSL 'http://127.0.0.1:$port/$tag/install.sh' | \
            env CLOUDESK_VERSION='$version' CLOUDESK_RELEASE_BASE_URL='http://127.0.0.1:$port' CLOUDESK_ALLOW_INSECURE_TEST_URL=1 \
                CLOUDESK_INIT_SYSTEM='$init_system' CLOUDESK_NO_START=1 bash
        after=\$(ls -A . | wc -l)
        [ \"\$before\" -eq \"\$after\" ] || { echo 'FAIL: install.sh left files in the caller cwd -- repository-relative dependency leaked in'; exit 1; }
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
    printf 'PASS: %s true stdin bootstrap (no checkout, no installer/lib, no packaging/) completed\n' "$label"
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
