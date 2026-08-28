#!/usr/bin/env bash
#
# Summarise a CloudDesk test run's real status.
#
# Rust's test harness has only "passed" and "failed". A test that skips
# because its live fixture is unavailable returns, so the harness counts
# it as passed -- which is exactly the false green this script exists to
# expose. `clouddesk-test-support` records every such skip as a marker
# line in a status log; this reads that log alongside the cargo output
# and reports the three real states separately.
#
# Usage (the status log is APPENDED to, so clear it first or the counts
# will mix this run with previous ones):
#   rm -f target/clouddesk-test-status.log
#   cargo test --workspace --no-fail-fast 2>&1 | tee run.log
#   scripts/test-status.sh run.log [target/clouddesk-test-status.log]
#
# Exit codes:
#   0  no FAIL and no BLOCKED_BY_ENVIRONMENT
#   1  at least one BLOCKED_BY_ENVIRONMENT (and no FAIL)
#   2  at least one FAIL
set -Eeuo pipefail

CARGO_LOG="${1:-}"
STATUS_LOG="${2:-target/clouddesk-test-status.log}"

if [[ -z "$CARGO_LOG" || ! -f "$CARGO_LOG" ]]; then
    printf 'usage: %s <cargo-output.log> [status-log]\n' "$0" >&2
    exit 64
fi

# Cargo's own arithmetic, summed across every test binary. These are
# the harness's labels, not the project's verdict.
harness_passed=0
harness_failed=0
while read -r p f; do
    harness_passed=$((harness_passed + p))
    harness_failed=$((harness_failed + f))
done < <(
    grep -E '^test result:' "$CARGO_LOG" 2>/dev/null |
        sed -E 's/.* ([0-9]+) passed; ([0-9]+) failed.*/\1 \2/'
)

blocked=0
if [[ -f "$STATUS_LOG" ]]; then
    blocked=$(grep -c 'CLOUDDESK_TEST_STATUS=BLOCKED_BY_ENVIRONMENT' "$STATUS_LOG" || true)
fi

# Every blocked test was counted by the harness as passed, so the real
# pass count is the harness's minus the blocked ones. Never derived from
# test names, only from markers the tests themselves emitted.
real_pass=$((harness_passed - blocked))
((real_pass < 0)) && real_pass=0

printf '=== CloudDesk test status ===\n'
printf 'cargo-reported passed : %s\n' "$harness_passed"
printf 'cargo-reported failed : %s\n' "$harness_failed"
printf -- '---\n'
printf 'real PASS             : %s\n' "$real_pass"
printf 'BLOCKED BY ENVIRONMENT: %s\n' "$blocked"
printf 'FAIL                  : %s\n' "$harness_failed"

if ((blocked > 0)); then
    printf -- '--- blocked, by reason ---\n'
    sed -E 's/.*CLOUDDESK_TEST_REASON=([^ ]+).*/\1/' "$STATUS_LOG" |
        sort | uniq -c | sort -rn |
        while read -r count reason; do
            printf '  %-6s %s\n' "$count" "$reason"
        done
fi

if ((harness_failed > 0)); then
    printf -- '--- failing tests ---\n'
    awk '/^failures:$/{f=1;next} /^test result:/{f=0} f && /^    [A-Za-z0-9_:]+$/{print "  "$1}' \
        "$CARGO_LOG" | sort -u
fi

if ((harness_failed > 0)); then
    printf '\nVERDICT: FAIL\n'
    exit 2
fi
if ((blocked > 0)); then
    printf '\nVERDICT: INCOMPLETE -- mandatory fixtures were unavailable.\n'
    printf 'Re-run with CLOUDDESK_REQUIRE_LIVE_ACCEPTANCE=1 to make these hard failures.\n'
    exit 1
fi
printf '\nVERDICT: PASS\n'
