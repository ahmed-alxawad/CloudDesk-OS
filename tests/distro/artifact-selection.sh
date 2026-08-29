#!/bin/sh
# Phase 10D: proves installer/install.sh picks the correct pre-built
# artifact directory (glibc vs. musl) by distro family, WITHOUT any
# CLOUDESK_BINARY/CLOUDESK_PRIVD_BINARY/CLOUDESK_SESSIOND_BINARY
# override -- exercising the selection logic itself, not bypassing it
# the way installer-layout.sh's own CLOUDESK_BINARY overrides do.
#
# Requires the real portable artifacts already built at
# dist/linux-x86_64-{glibc,musl}/ (packaging/build-release.sh and its
# musl counterpart). Runs a full CLOUDESK_ROOT-prefixed, package-
# install-skipped install per family (same harmless layout-only mode
# installer-layout.sh already uses) and compares the file the
# installer actually copied against the expected source directory by
# SHA256 -- a stronger proof than an error message, since it confirms
# real bytes came from the right place, not just that a path string
# was assembled correctly.
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
glibc_dir="$project_dir/dist/linux-x86_64-glibc"
musl_dir="$project_dir/dist/linux-x86_64-musl"

for d in "$glibc_dir" "$musl_dir"; do
    [ -f "$d/clouddeskd" ] || {
        printf 'SKIP: %s/clouddeskd not built -- run packaging/build-release.sh (glibc) and the musl equivalent first.\n' "$d" >&2
        exit 0
    }
done

test_root=$(mktemp -d "${TMPDIR:-/tmp}/clouddesk-artifact-selection.XXXXXX")
trap 'rm -rf -- "$test_root"' EXIT INT TERM
mkdir -p "$test_root/web"
printf '<!doctype html><title>CloudDesk</title>\n' >"$test_root/web/index.html"

failures=0

check() {
    distro_id="$1"
    expected_dir="$2"
    target="$test_root/$distro_id"
    CLOUDESK_ROOT="$target" \
    CLOUDESK_DISTRO_ID="$distro_id" \
    CLOUDESK_INIT_SYSTEM=none \
    CLOUDESK_SKIP_PACKAGES=1 \
    CLOUDESK_WEB_DIR="$test_root/web" \
        "$project_dir/installer/install.sh" >/dev/null

    installed_hash=$(sha256sum "$target/opt/clouddesk/bin/clouddeskd" | cut -d' ' -f1)
    expected_hash=$(sha256sum "$expected_dir/clouddeskd" | cut -d' ' -f1)
    if [ "$installed_hash" != "$expected_hash" ]; then
        printf 'FAIL: %s -> installed binary does not match %s/clouddeskd\n' \
            "$distro_id" "$expected_dir" >&2
        failures=$((failures + 1))
        return
    fi
    printf 'PASS: %-12s -> %s (sha256 %s)\n' "$distro_id" "$expected_dir" "$installed_hash"
}

check alpine "$musl_dir"
check debian "$glibc_dir"
check ubuntu "$glibc_dir"
check fedora "$glibc_dir"
check rhel "$glibc_dir"
check rocky "$glibc_dir"
check almalinux "$glibc_dir"
check arch "$glibc_dir"

if [ "$failures" -ne 0 ]; then
    printf '%d artifact-selection check(s) failed.\n' "$failures" >&2
    exit 1
fi
printf 'Artifact selection passed for every declared distro family.\n'
