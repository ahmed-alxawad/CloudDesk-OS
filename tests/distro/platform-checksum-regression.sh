#!/bin/sh
# Publication Pass H1: regression test for the rc.3 hosted release
# failure (workflow run 33311438924) -- the release-attestation workflow
# calls packaging/build-release.sh and packaging/build-release-musl.sh,
# but at that time neither script wrote a SHA256SUMS file into its own
# output directory (they only printed the hash to the build log), so
# tests/distro/release-staging-validation.sh's requirement for
# dist/linux-x86_64-{glibc,musl}/SHA256SUMS failed on the very first
# real hosted run despite every local manual staging pass having always
# created that file by hand as a separate, uncommitted step.
#
# Fixed by making build-release.sh / build-release-musl.sh themselves
# write SHA256SUMS as part of their own last step, so both local runs
# and the hosted workflow (which invoke the same committed scripts) get
# it automatically -- no separate/duplicated staging logic to drift out
# of sync again.
#
# Requires dist/linux-x86_64-{glibc,musl}/ to already contain real
# output from the (fixed) build-release*.sh scripts.
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$project_dir"

glibc_dir="dist/linux-x86_64-glibc"
musl_dir="dist/linux-x86_64-musl"

for d in "$glibc_dir" "$musl_dir"; do
    for b in clouddeskd cloudesk-privd cloudesk-sessiond; do
        [ -f "$d/$b" ] || {
            printf 'SKIP: %s missing -- run packaging/build-release.sh / build-release-musl.sh first.\n' "$d/$b" >&2
            exit 0
        }
    done
done

failures=0
version="0.0.0-platform-checksum-regression-test"
staging_dir="dist/release/$version"
rm -rf "$staging_dir"
mkdir -p "$staging_dir/checksums" "$staging_dir/metadata" "$staging_dir/sbom"
cat "$glibc_dir/SHA256SUMS" "$musl_dir/SHA256SUMS" >"$staging_dir/checksums/SHA256SUMS" 2>/dev/null || true
printf '{"release_candidate": "%s", "source_commit": "%s"}' "$version" "$(git rev-parse HEAD)" >"$staging_dir/metadata/manifest.json"
echo '{}' >"$staging_dir/sbom/cloudesk-os.cdx.json"

cleanup() { rm -rf "$staging_dir"; }
trap cleanup EXIT INT TERM

echo "=== positive control: real build-script output must satisfy the staging validator ==="
if sh tests/distro/release-staging-validation.sh HEAD "$version" >/tmp/pcr-out.$$ 2>&1; then
    echo "PASS: real committed build-script output satisfies the staging validator"
else
    echo "FAIL: real committed build-script output does NOT satisfy the staging validator (this is the exact rc.3 hosted failure mode)" >&2
    cat /tmp/pcr-out.$$ >&2
    failures=$((failures + 1))
fi
rm -f /tmp/pcr-out.$$

echo "=== negative control: missing glibc/SHA256SUMS must fail closed (reproduces rc.3's exact hosted error) ==="
mv "$glibc_dir/SHA256SUMS" "$glibc_dir/SHA256SUMS.bak"
if sh tests/distro/release-staging-validation.sh HEAD "$version" >/tmp/pcr-out.$$ 2>&1; then
    echo "FAIL: validator passed despite missing $glibc_dir/SHA256SUMS" >&2
    failures=$((failures + 1))
elif grep -q "linux-x86_64-glibc/SHA256SUMS' is missing" /tmp/pcr-out.$$; then
    echo "PASS: missing glibc SHA256SUMS correctly rejected"
else
    echo "FAIL: validator failed for an unexpected reason:" >&2
    cat /tmp/pcr-out.$$ >&2
    failures=$((failures + 1))
fi
mv "$glibc_dir/SHA256SUMS.bak" "$glibc_dir/SHA256SUMS"
rm -f /tmp/pcr-out.$$

echo "=== negative control: missing musl/SHA256SUMS must fail closed ==="
mv "$musl_dir/SHA256SUMS" "$musl_dir/SHA256SUMS.bak"
if sh tests/distro/release-staging-validation.sh HEAD "$version" >/tmp/pcr-out.$$ 2>&1; then
    echo "FAIL: validator passed despite missing $musl_dir/SHA256SUMS" >&2
    failures=$((failures + 1))
elif grep -q "linux-x86_64-musl/SHA256SUMS' is missing" /tmp/pcr-out.$$; then
    echo "PASS: missing musl SHA256SUMS correctly rejected"
else
    echo "FAIL: validator failed for an unexpected reason:" >&2
    cat /tmp/pcr-out.$$ >&2
    failures=$((failures + 1))
fi
mv "$musl_dir/SHA256SUMS.bak" "$musl_dir/SHA256SUMS"
rm -f /tmp/pcr-out.$$

if [ "$failures" -ne 0 ]; then
    printf '%d platform-checksum regression control(s) failed.\n' "$failures" >&2
    exit 1
fi
printf 'All platform-checksum regression controls passed.\n'
