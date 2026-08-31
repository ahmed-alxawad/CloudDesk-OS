#!/bin/sh
# Publication Pass J1: installer/install.sh embeds the systemd/openrc
# unit content directly (see the comment above the `case "$init_system"`
# block there) so the public curl|bash bootstrap has no dependency on a
# packaging/ checkout existing on disk. packaging/systemd/*.service and
# packaging/openrc/* remain the canonical files for local packaging
# tooling and the systemd/openrc lifecycle harnesses in this directory.
# This test fails closed if the two ever diverge, since nothing at
# runtime would otherwise catch that drift.
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
install_sh="$project_dir/installer/install.sh"
work=$(mktemp -d "${TMPDIR:-/tmp}/clouddesk-installer-lib-sync.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

# Extracts the Nth <<'EOF' ... EOF heredoc body from install.sh.
extract_heredoc() {
    n=$1
    awk -v want="$n" '
        /<<\x27EOF\x27$/ { count++; if (count == want) { capturing = 1; next } }
        capturing && /^EOF$/ { capturing = 0; exit }
        capturing { print }
    ' "$install_sh"
}

extract_heredoc 1 >"$work/cloudesk-privd.service"
extract_heredoc 2 >"$work/clouddesk.service"
extract_heredoc 3 >"$work/cloudesk-privd.openrc"
extract_heredoc 4 >"$work/clouddesk.openrc"

failures=0

compare_n=0
compare() {
    label=$1
    extracted=$2
    canonical=$3
    compare_n=$((compare_n + 1))
    diff_file="$work/diff.$compare_n"
    if ! diff -u "$canonical" "$extracted" >"$diff_file"; then
        printf 'FAIL: %s -- installer/install.sh embedded copy has drifted from %s\n' "$label" "$canonical" >&2
        cat "$diff_file" >&2
        failures=$((failures + 1))
    else
        printf 'PASS: %s matches embedded copy in installer/install.sh\n' "$label"
    fi
}

compare "packaging/systemd/cloudesk-privd.service" "$work/cloudesk-privd.service" \
    "$project_dir/packaging/systemd/cloudesk-privd.service"
compare "packaging/systemd/clouddesk.service" "$work/clouddesk.service" \
    "$project_dir/packaging/systemd/clouddesk.service"
compare "packaging/openrc/cloudesk-privd" "$work/cloudesk-privd.openrc" \
    "$project_dir/packaging/openrc/cloudesk-privd"
compare "packaging/openrc/clouddesk" "$work/clouddesk.openrc" \
    "$project_dir/packaging/openrc/clouddesk"

if [ "$failures" -ne 0 ]; then
    printf '%d embedded unit file(s) have drifted from packaging/.\n' "$failures" >&2
    exit 1
fi
printf 'All embedded installer unit files match packaging/.\n'
