#!/usr/bin/env bash
# Publication Pass D2: prevents a future staged public release asset from
# silently missing GitHub Artifact Attestation coverage.
#
# Extracts two sets directly from .github/workflows/release-attest.yml:
#   STAGED   -- every release-assets/<name> path written by the "Stage flat
#               public release asset layout" step (before subject-path:)
#   ATTESTED -- every release-assets/<name> path listed under subject-path:
# and requires them to be identical. A staged asset missing from ATTESTED
# means something downloadable is not covered by the project's chosen
# authenticity model; an ATTESTED entry missing from STAGED means the
# workflow references a file it never actually produces.
set -Eeuo pipefail

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
workflow=${1:-"$project_dir/.github/workflows/release-attest.yml"}

[ -f "$workflow" ] || {
    printf 'FAIL: %s not found\n' "$workflow" >&2
    exit 1
}

staged=$(awk '
    /subject-path:/ { exit }
    { while (match($0, /release-assets\/[A-Za-z0-9._-]+/)) {
        print substr($0, RSTART, RLENGTH)
        $0 = substr($0, RSTART + RLENGTH)
    } }
' "$workflow" | sort -u)

attested=$(awk '
    /subject-path:/ { in_subjects = 1; next }
    in_subjects && /^ *#/ { exit }
    in_subjects && /^ *release-assets\// {
        gsub(/^ +/, ""); print; next
    }
    in_subjects && !/release-assets\// { exit }
' "$workflow" | sort -u)

[ -n "$staged" ] || { printf 'FAIL: no release-assets/* paths found in the staging step\n' >&2; exit 1; }
[ -n "$attested" ] || { printf 'FAIL: no release-assets/* paths found in subject-path\n' >&2; exit 1; }

missing_from_attestation=$(comm -23 <(printf '%s\n' "$staged") <(printf '%s\n' "$attested"))
missing_from_staging=$(comm -13 <(printf '%s\n' "$staged") <(printf '%s\n' "$attested"))

status=0
if [ -n "$missing_from_attestation" ]; then
    printf 'FAIL: staged but NOT attested:\n%s\n' "$missing_from_attestation" >&2
    status=1
fi
if [ -n "$missing_from_staging" ]; then
    printf 'FAIL: attested but never staged (workflow references a file it never produces):\n%s\n' "$missing_from_staging" >&2
    status=1
fi

if [ "$status" -eq 0 ]; then
    count=$(printf '%s\n' "$staged" | grep -c .)
    printf 'PASS: %d staged release asset(s), all attested, no orphaned attestation subjects.\n' "$count"
fi
exit "$status"
