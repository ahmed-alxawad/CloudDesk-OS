#!/bin/sh
# Publication Pass B: direct-fetch installer negative/positive controls.
#
# Builds a local HTTP fixture mirroring the flat GitHub Releases asset
# layout install.sh's CLOUDESK_VERSION path expects, then exercises the
# real installer against it (CLOUDESK_ROOT fake-root, no privileges
# required). Requires dist/linux-x86_64-{glibc,musl}/ to already contain
# built artifacts (run packaging/build-release*.sh first) and
# apps/web/dist to exist (run packaging/build-web.sh or `npm run build`
# in apps/web first).
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
glibc_dir="$project_dir/dist/linux-x86_64-glibc"
musl_dir="$project_dir/dist/linux-x86_64-musl"
web_dist="$project_dir/apps/web/dist"

for d in "$glibc_dir" "$musl_dir"; do
    for b in clouddeskd cloudesk-privd cloudesk-sessiond; do
        [ -f "$d/$b" ] || {
            printf 'SKIP: %s missing -- run packaging/build-release.sh / build-release-musl.sh first.\n' "$d/$b" >&2
            exit 0
        }
    done
done
[ -f "$web_dist/index.html" ] || {
    printf 'SKIP: %s missing -- run packaging/build-web.sh or `npm run build` in apps/web first.\n' "$web_dist/index.html" >&2
    exit 0
}

failures=0
port=$((20000 + $$ % 20000))
fixture=$(mktemp -d "${TMPDIR:-/tmp}/clouddesk-remote-fetch-fixture.XXXXXX")
rel="$fixture/v1.0.1-rc.3"
mkdir -p "$rel"

tar -C "$web_dist" -czf "$rel/clouddesk-web.tar.gz" .

cp "$glibc_dir/clouddeskd" "$rel/clouddeskd-linux-x86_64-glibc"
cp "$glibc_dir/cloudesk-privd" "$rel/cloudesk-privd-linux-x86_64-glibc"
cp "$glibc_dir/cloudesk-sessiond" "$rel/cloudesk-sessiond-linux-x86_64-glibc"
cp "$musl_dir/clouddeskd" "$rel/clouddeskd-linux-x86_64-musl"
cp "$musl_dir/cloudesk-privd" "$rel/cloudesk-privd-linux-x86_64-musl"
cp "$musl_dir/cloudesk-sessiond" "$rel/cloudesk-sessiond-linux-x86_64-musl"

gen_sums() {
    {
        printf '%s  linux-x86_64-glibc/clouddeskd\n' "$(sha256sum "$rel/clouddeskd-linux-x86_64-glibc" | cut -d' ' -f1)"
        printf '%s  linux-x86_64-glibc/cloudesk-privd\n' "$(sha256sum "$rel/cloudesk-privd-linux-x86_64-glibc" | cut -d' ' -f1)"
        printf '%s  linux-x86_64-glibc/cloudesk-sessiond\n' "$(sha256sum "$rel/cloudesk-sessiond-linux-x86_64-glibc" | cut -d' ' -f1)"
        printf '%s  linux-x86_64-musl/clouddeskd\n' "$(sha256sum "$rel/clouddeskd-linux-x86_64-musl" | cut -d' ' -f1)"
        printf '%s  linux-x86_64-musl/cloudesk-privd\n' "$(sha256sum "$rel/cloudesk-privd-linux-x86_64-musl" | cut -d' ' -f1)"
        printf '%s  linux-x86_64-musl/cloudesk-sessiond\n' "$(sha256sum "$rel/cloudesk-sessiond-linux-x86_64-musl" | cut -d' ' -f1)"
        printf '%s  clouddesk-web.tar.gz\n' "$(sha256sum "$rel/clouddesk-web.tar.gz" | cut -d' ' -f1)"
    } >"$rel/SHA256SUMS"
}
gen_sums

gen_manifest() {
    version=$1
    printf '{"release_candidate": "%s", "source_commit": "5a3a1da6faeeb5370ae751635433cfbcbbbbf7ee"}' "$version" >"$rel/manifest.json"
}
gen_manifest 1.0.1-rc.3

(cd "$fixture" && exec python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true; rm -rf "$fixture"' EXIT INT TERM
for _ in 1 2 3 4 5 6 7 8 9 10; do
    curl -fsS "http://127.0.0.1:$port/v1.0.1-rc.3/manifest.json" >/dev/null 2>&1 && break
    sleep 0.3
done

run_install() {
    distro_id=$1
    shift
    test_root=$(mktemp -d "${TMPDIR:-/tmp}/clouddesk-remote-fetch-install.XXXXXX")
    set +e
    CLOUDESK_ROOT="$test_root" CLOUDESK_DISTRO_ID="$distro_id" CLOUDESK_INIT_SYSTEM=none CLOUDESK_SKIP_PACKAGES=1 \
        "$@" "$project_dir/installer/install.sh" >"$test_root.out" 2>"$test_root.err"
    status=$?
    set -e
    rm -rf "$test_root" "$test_root.out" "$test_root.err" 2>/dev/null || true
    return $status
}

expect_pass() {
    label=$1; shift
    if run_install "$@"; then
        printf 'PASS: %s\n' "$label"
    else
        printf 'FAIL: %s -> expected success, installer failed\n' "$label" >&2
        failures=$((failures + 1))
    fi
}

expect_fail() {
    label=$1; shift
    if run_install "$@"; then
        printf 'FAIL: %s -> expected failure, installer succeeded\n' "$label" >&2
        failures=$((failures + 1))
    else
        printf 'PASS: %s -> correctly rejected\n' "$label"
    fi
}

base="http://127.0.0.1:$port"

expect_pass "valid public download (debian/glibc)" debian \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1
expect_pass "valid public download (alpine/musl)" alpine \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1

expect_fail "http:// base URL without insecure override" debian \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base"

expect_fail "shell-metacharacter version string" debian \
    env CLOUDESK_VERSION='1.0.1; rm -rf /' CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1

gen_manifest 1.0.1-rc.2
expect_fail "manifest version mismatch" debian \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1
gen_manifest 1.0.1-rc.3

printf '{"release_candidate": "1.0.1-rc.3", "source_commit": "not-a-real-commit"}' >"$rel/manifest.json"
expect_fail "malformed source_commit field" debian \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1
gen_manifest 1.0.1-rc.3

cp "$rel/SHA256SUMS" "$rel/SHA256SUMS.bak"
sed -i.tmp 's/^[0-9a-f]\{64\}  linux-x86_64-glibc\/clouddeskd$/0000000000000000000000000000000000000000000000000000000000000000  linux-x86_64-glibc\/clouddeskd/' "$rel/SHA256SUMS"
rm -f "$rel/SHA256SUMS.tmp"
expect_fail "corrupted checksum manifest entry" debian \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1
cp "$rel/SHA256SUMS.bak" "$rel/SHA256SUMS"

grep -v "linux-x86_64-glibc/cloudesk-privd" "$rel/SHA256SUMS.bak" >"$rel/SHA256SUMS"
expect_fail "missing checksum manifest entry" debian \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1
cp "$rel/SHA256SUMS.bak" "$rel/SHA256SUMS"

cp "$rel/clouddeskd-linux-x86_64-glibc" "$rel/clouddeskd-linux-x86_64-glibc.bak"
size=$(wc -c <"$rel/clouddeskd-linux-x86_64-glibc")
off=$((size / 2))
existing=$(dd if="$rel/clouddeskd-linux-x86_64-glibc" bs=1 count=1 skip="$off" 2>/dev/null | od -An -tu1 | tr -d ' ')
if [ "$existing" = "0" ]; then printf '\377' | dd of="$rel/clouddeskd-linux-x86_64-glibc" bs=1 count=1 seek="$off" conv=notrunc >/dev/null 2>&1
else printf '\000' | dd of="$rel/clouddeskd-linux-x86_64-glibc" bs=1 count=1 seek="$off" conv=notrunc >/dev/null 2>&1
fi
expect_fail "corrupted binary (checksums unmodified)" debian \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1
cp "$rel/clouddeskd-linux-x86_64-glibc.bak" "$rel/clouddeskd-linux-x86_64-glibc"

cp "$rel/clouddeskd-linux-x86_64-musl" "$rel/clouddeskd-linux-x86_64-glibc"
expect_fail "artifact-swap (musl bytes under glibc name)" debian \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1
cp "$rel/clouddeskd-linux-x86_64-glibc.bak" "$rel/clouddeskd-linux-x86_64-glibc"
rm -f "$rel/clouddeskd-linux-x86_64-glibc.bak"

mv "$rel/cloudesk-privd-linux-x86_64-glibc" "$fixture/cloudesk-privd-linux-x86_64-glibc.hidden"
expect_fail "missing artifact (404 from server)" debian \
    env CLOUDESK_VERSION=1.0.1-rc.3 CLOUDESK_RELEASE_BASE_URL="$base" CLOUDESK_ALLOW_INSECURE_TEST_URL=1
mv "$fixture/cloudesk-privd-linux-x86_64-glibc.hidden" "$rel/cloudesk-privd-linux-x86_64-glibc"

if [ "$failures" -ne 0 ]; then
    printf '%d remote-fetch control(s) failed.\n' "$failures" >&2
    exit 1
fi
printf 'All remote-fetch installer controls passed.\n'
