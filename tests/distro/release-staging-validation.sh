#!/bin/sh
# Fail-closed pre-publication source/staging validator.
#
# Verifies that a given git ref contains every file established project
# licensing/release policy requires to be present in the SOURCE TREE before
# that commit can be treated as publication-ready, and that a given release
# staging directory (dist/release/<version>) contains the required build
# outputs. Does not publish, sign, tag, or push anything.
#
# Usage:
#   tests/distro/release-staging-validation.sh <git-ref> <release-version>
#
# Example:
#   tests/distro/release-staging-validation.sh 89bfe4690ff5b4b178cb68a1a40806a13fa04f99 1.0.1-rc.1
set -eu

ref=${1:?usage: release-staging-validation.sh <git-ref> <release-version>}
version=${2:?usage: release-staging-validation.sh <git-ref> <release-version>}
repo_root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo_root"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

echo "== validating source tree at $ref =="

required_source_files="LICENSE docs/THIRD_PARTY_NOTICES.md docs/RELEASE_INTEGRITY.md installer/install.sh RELEASE_NOTES.md"
for f in $required_source_files; do
    if git cat-file -e "$ref:$f" 2>/dev/null; then
        echo "  OK   $f"
    else
        fail "required source file '$f' is missing at $ref"
    fi
done

echo "== validating release staging dist/release/$version =="

staging_dir="dist/release/$version"
[ -d "$staging_dir" ] || fail "release staging directory '$staging_dir' does not exist"

required_staging_files="checksums/SHA256SUMS metadata/manifest.json sbom/cloudesk-os.cdx.json"
for f in $required_staging_files; do
    if [ -f "$staging_dir/$f" ]; then
        echo "  OK   $staging_dir/$f"
    else
        fail "required release staging file '$staging_dir/$f' is missing"
    fi
done

required_artifact_dirs="dist/linux-x86_64-glibc dist/linux-x86_64-musl"
for d in $required_artifact_dirs; do
    for bin in clouddeskd cloudesk-privd cloudesk-sessiond; do
        [ -f "$d/$bin" ] || fail "required built artifact '$d/$bin' is missing"
    done
    [ -f "$d/SHA256SUMS" ] || fail "required artifact manifest '$d/SHA256SUMS' is missing"
done

echo "PASS: $ref / $version is release-staging-complete"
