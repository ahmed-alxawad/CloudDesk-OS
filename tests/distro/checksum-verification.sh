#!/bin/sh
# Phase 17A Part 44: installer checksum-verification negative control.
#
# Requires the real release artifacts already built at
# dist/linux-x86_64-{glibc,musl}/ WITH a SHA256SUMS manifest present in
# each directory (packaging/build-release.sh / build-release-musl.sh's
# own output). Runs against the REAL artifact files the installer would
# actually select for a real distro -- not a synthetic CLOUDESK_BINARY
# override -- because install.sh's checksum check keys off
# $default_artifact_dir (computed internally from distro_family), not
# from wherever a caller-supplied override happens to point. Corrupts
# one byte of a COPY placed at the real artifact path, requires the
# installer to abort with no partial install, then restores the
# original file unconditionally (trap, including on early exit).
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
glibc_dir="$project_dir/dist/linux-x86_64-glibc"
musl_dir="$project_dir/dist/linux-x86_64-musl"

for d in "$glibc_dir" "$musl_dir"; do
    [ -f "$d/SHA256SUMS" ] || {
        printf 'SKIP: %s/SHA256SUMS not present -- run packaging/build-release.sh (glibc) and the musl equivalent, then generate SHA256SUMS, before this test.\n' "$d" >&2
        exit 0
    }
done

failures=0

check_family() {
    distro_id=$1
    artifact_dir=$2
    target_binary="$artifact_dir/clouddeskd"
    backup=$(mktemp "${TMPDIR:-/tmp}/clouddesk-checksum-backup.XXXXXX")
    cp "$target_binary" "$backup"
    # Remove-and-recreate rather than in-place editing: the real
    # release artifacts are root-owned (produced by a Docker-internal
    # copy step), so this test's own unprivileged user cannot open them
    # for writing -- but *can* remove/replace them, since write
    # permission on the containing directory (which this test's user
    # does own) governs unlink/create, not the target file's own mode.
    trap 'rm -f "$target_binary"; cp "$backup" "$target_binary"; chmod 0755 "$target_binary"; rm -f "$backup"' EXIT INT TERM

    # Corrupt one byte near the middle of the file -- deep in a real
    # code/data section, not a header or an end-of-file padding region
    # (an earlier draft of this test picked a fixed end-of-file offset
    # that turned out to already be a zero-padding byte, so writing
    # zero there was a silent no-op that never actually changed the
    # file -- confirmed live: the "corrupted" file's hash was identical
    # to the original, which is why the installer correctly accepted
    # it and this test's own logic, not the installer, was wrong).
    # Reads the existing byte and writes a *different* value
    # (0x00 -> 0xFF, anything else -> 0x00), so the change is
    # unconditional regardless of what was already there.
    rm -f "$target_binary"
    cp "$backup" "$target_binary"
    chmod 0755 "$target_binary"
    size=$(wc -c <"$target_binary")
    offset=$((size / 2))
    existing=$(dd if="$target_binary" bs=1 count=1 skip="$offset" 2>/dev/null | od -An -tu1 | tr -d ' ')
    if [ "$existing" = "0" ]; then
        printf '\377' | dd of="$target_binary" bs=1 count=1 seek="$offset" conv=notrunc >/dev/null 2>&1
    else
        printf '\000' | dd of="$target_binary" bs=1 count=1 seek="$offset" conv=notrunc >/dev/null 2>&1
    fi
    chmod 0755 "$target_binary"
    after=$(dd if="$target_binary" bs=1 count=1 skip="$offset" 2>/dev/null | od -An -tu1 | tr -d ' ')
    [ "$existing" != "$after" ] || {
        printf 'FAIL: %s -> corruption technique itself did not change the byte at offset %s (existing=%s after=%s) -- this test cannot trust its own negative control\n' \
            "$distro_id" "$offset" "$existing" "$after" >&2
        failures=$((failures + 1))
        return
    }

    test_root=$(mktemp -d "${TMPDIR:-/tmp}/clouddesk-checksum-install.XXXXXX")
    set +e
    CLOUDESK_ROOT="$test_root" \
    CLOUDESK_DISTRO_ID="$distro_id" \
    CLOUDESK_INIT_SYSTEM=none \
    CLOUDESK_SKIP_PACKAGES=1 \
    CLOUDESK_WEB_DIR="$project_dir/apps/web/dist" \
        "$project_dir/installer/install.sh" >"$test_root.stdout" 2>"$test_root.stderr"
    status=$?
    set -e

    cp "$backup" "$target_binary"
    rm -f "$backup"
    trap - EXIT INT TERM

    if [ "$status" -eq 0 ]; then
        printf 'FAIL: %s -> installer exited 0 against a corrupted binary (must fail closed)\n' "$distro_id" >&2
        failures=$((failures + 1))
    elif ! grep -q 'checksum mismatch' "$test_root.stderr"; then
        printf 'FAIL: %s -> installer failed, but not with the expected checksum-mismatch message: %s\n' \
            "$distro_id" "$(cat "$test_root.stderr")" >&2
        failures=$((failures + 1))
    elif [ -e "$test_root/opt/clouddesk/bin/clouddeskd" ]; then
        printf 'FAIL: %s -> corrupted binary was installed despite checksum failure\n' "$distro_id" >&2
        failures=$((failures + 1))
    else
        printf 'PASS: %s -> checksum mismatch detected, install aborted, nothing installed\n' "$distro_id"
    fi
    rm -rf "$test_root" "$test_root.stdout" "$test_root.stderr"
}

check_family debian "$glibc_dir"
check_family alpine "$musl_dir"

if [ "$failures" -ne 0 ]; then
    printf '%d checksum-verification negative control(s) failed.\n' "$failures" >&2
    exit 1
fi
printf 'Checksum-verification negative control passed for glibc and musl artifacts.\n'
