# Phase 6 — Optional Runtime Orchestrator: Executable Evidence Matrix

This is an evidence map, not a test runner. Every row cites the exact
test/file that produced the status on its left — not a claim carried
over from an earlier report. Statuses used: `PASS`, `FAIL`,
`BLOCKED BY ENVIRONMENT`, `UNAVAILABLE`, `NOT EXECUTED`,
`IMPLEMENTATION MISSING`.

Fixture disclaimer: `RuntimeKind::TestFixture` and
`services/test-runtime-fixture` are disposable test-only infrastructure
used to prove the orchestrator's own plumbing. They are never reachable
through a production router (item 02) and are not evidence that Code,
Office, or Brave are implemented.

Last assembled: alongside commits `df0727f` and `993080a` on
`engineering/v1-true-closure`.

| ID | Requirement | Status | Evidence type | Test name / file | Live fixture | Notes / limitations |
|----|-------------|--------|----------------|-------------------|---------------|----------------------|
| 01 | Production runtime list correct | PASS | HTTP integration | `task_1_2_3_admin_sees_status_only_admin_can_enable` (`services/clouddeskd/tests/runtime_api.rs`) | test fixture (backend), no adapter registered for code/office/browser | `GET /api/v1/runtimes` returns only kinds `is_selectable()` allows in production; code/office/browser report `available: false` since no adapter exists yet (Phase 7+) |
| 02 | TestFixture absent (production) | PASS | HTTP integration | `task_2_15_test_fixture_kind_is_rejected_by_the_production_router` (`runtime_api.rs`) | test fixture, deliberately misconfigured into a *production* router constructor | Proves registration alone isn't enough — `runtime_allow_test_kind: false` in every production builder is the actual gate |
| 03 | Admin global status | PASS | HTTP integration | `task_1_2_3_admin_sees_status_only_admin_can_enable` | test fixture | `GET /api/v1/runtimes` as administrator |
| 04 | Unauthorized global enable denied | PASS | HTTP integration | `task_1_2_3_admin_sees_status_only_admin_can_enable`, `task_3_settings_authorization_matches_role_policy_for_every_role` | test fixture | Guest/User/Manager all denied (403); only administrator (via blanket `runtime.admin` grant) succeeds |
| 05 | Admin enable | PASS | HTTP integration | `task_1_2_3_admin_sees_status_only_admin_can_enable` | test fixture | `POST /api/v1/runtimes/{kind}/enable` → 204 |
| 06 | User starts own instance | PASS | HTTP integration | `start_instance` helper, used throughout `runtime_api.rs` (e.g. `task_4_5_10_start_readiness_and_no_port_disclosure`) | test fixture | `POST /api/v1/runtime-instances` |
| 07 | Readiness requires health | PASS | HTTP + orchestrator | `task_4_5_10_start_readiness_and_no_port_disclosure` (HTTP); `task_2_enable_task_3_start_task_4_readiness` (orchestrator, `live_lifecycle.rs`) | test fixture | Asserts `state == "running"` only after a real `/healthz` 2xx, not merely process spawn |
| 08 | Own HTTP proxy | PASS | HTTP integration | `task_8_http_proxy_owner_succeeds_cross_user_denied` | test fixture | Real `GET .../proxy/echo` round-trip |
| 09 | Cross-user HTTP proxy denied | PASS | HTTP integration | `task_8_http_proxy_owner_succeeds_cross_user_denied` | test fixture | 404, not 403 — no existence oracle |
| 10 | Own WebSocket | PASS | HTTP integration, real network | `task_9_websocket_proxy_owner_succeeds_cross_user_denied`, `task_2_websocket_binary_frames_are_relayed` | test fixture | Real bound TCP listener + `tokio_tungstenite` client (never `tower::oneshot`, which cannot complete a real upgrade) |
| 11 | Cross-user WebSocket denied | PASS | HTTP integration, real network | `task_9_websocket_proxy_owner_succeeds_cross_user_denied`, `task_2_cross_user_and_unauthenticated_binary_attempts_denied` | test fixture | Handshake completes (axum upgrades before the handler's ownership check runs) but the socket is closed without reaching the fixture |
| 12 | Origin policy | PASS | HTTP integration | `task_10_websocket_proxy_rejects_cross_site_upgrade_before_auth` | test fixture | Runtime WS route inherits the existing project-wide `web_security` CSRF/origin middleware — cross-site upgrade rejected (403) before authorization runs, same as the terminal WS route |
| 13 | SSRF resistance | PASS | HTTP integration | `task_7_ssrf_header_sweep_has_no_effect_on_upstream_selection`; orchestrator-level `task_21_35_cross_user_proxy_access_is_denied_not_ssrf_capable` (`live_proxy.rs`) | test fixture | Host/X-Forwarded-Host/Forwarded/X-Forwarded-For/X-Original-Url/X-Rewrite-Url all spoofed — none affects upstream selection; structural proof (no parameter in `proxy_http`/`proxy_ws` accepts a client-chosen host/port), not a blacklist |
| 14 | Internal port non-disclosure | PASS | HTTP integration | `task_4_5_10_start_readiness_and_no_port_disclosure` | test fixture | Asserts response body never contains `"port"` or a `127.0.0.1:<port>` literal |
| 15 | Duplicate start safety | PASS | HTTP integration | `task_10_duplicate_start_is_refused_not_a_second_uncontrolled_instance` | test fixture | Per-user instance limit doubles as duplicate-start protection (429), not a second uncontrolled process |
| 16 | Concurrent lifecycle safety | PASS | HTTP + orchestrator | `task_11_simultaneous_lifecycle_requests_are_safe` (HTTP); `task_18_simultaneous_start`, `task_19_simultaneous_stop_start` (orchestrator) | test fixture | Concurrent stop+restart resolve to one coherent final state |
| 17 | Process-tree termination | PASS | orchestrator | `task_17_child_process_cleanup`, `task_17_ignore_sigterm_falls_back_to_sigkill` (`live_lifecycle.rs`) | test fixture (`SPAWN_CHILD`, `IGNORE_SIGTERM`) | Process-group signaling (`setsid`); SIGTERM-ignoring fixture forces the SIGKILL fallback |
| 18 | Restart | PASS | HTTP + orchestrator | `task_12_14_stop_and_restart` (HTTP); `task_9_stop_and_restart` (orchestrator) | test fixture | |
| 19 | Crash detection | PASS | orchestrator | `task_11_crash_detection` (`live_lifecycle.rs`) | test fixture (`CRASH_AFTER_MS`) | Not re-exercised at the HTTP layer this pass — same manager code path, no new HTTP-layer risk |
| 20 | Health failure | PASS | orchestrator | `task_10_start_timeout_and_health_failure` | test fixture | |
| 21 | Idle shutdown | PASS | orchestrator | `task_12_idle_shutdown_activity_resets_timer_and_sweep_stops_truly_idle_instance` | test fixture | Activity-resets-timer and sweep-stops-truly-idle both covered in one test |
| 22 | Disable while active | PASS | HTTP + orchestrator | `task_19_disable_while_active` (HTTP); `task_16_disable_while_active` (orchestrator) | test fixture | Full graceful-stop-then-verify-gone sequence, reflected through the API's own status endpoint |
| 23 | Persistent profile retention | NOT EXECUTED | — | — | — | No adapter with `Persistence::Persistent` has real profile data to retain yet (Code/Office are Phase 7+, and the test fixture's own default is `Ephemeral`) — the *policy* (`default_persistence()` in `services/clouddeskd/src/lib.rs`) is implemented and typed, but there is no live test proving data survives a stop for a persistent kind. Honest gap, not hidden. |
| 24 | Ephemeral cleanup | PASS | orchestrator | `crates/orchestrator/src/storage.rs` unit tests (`remove_instance_state_dir_deletes_only_that_instance`); exercised live via every `Ephemeral`-persistence test's stop path | test fixture | |
| 25 | Startup reconciliation | PASS | orchestrator | `task_20_startup_reconciliation` (`live_lifecycle.rs`) | test fixture | Non-terminal rows never trusted on a recovered bare PID (documented design boundary, not an oversight) |
| 26 | Environment-secret isolation | PASS | orchestrator | `environment_never_leaks_the_orchestrator_process_env` (`live_lifecycle.rs`) | test fixture | Live env-leak proof for Vault master key/session signing secret/DB credential/SSH passphrase/API token shaped names, plus `PATH` itself. Not re-proven at the HTTP layer (same code path, no new risk introduced by the HTTP handlers) |
| 27 | Bounded logs | PASS | HTTP integration | `task_12_bounded_sanitized_logs`, `task_11_12_hostile_log_content_is_sanitized_and_bounded` | test fixture | 64 KiB cap enforced on the *sanitized* output, not just the raw capture (see item 28 for the bug this caught) |
| 28 | Hostile log safety | PASS | HTTP integration | `task_11_12_hostile_log_content_is_sanitized_and_bounded` | test fixture (`LOG_TEST_PAYLOAD_HEX`) | Real bug found+fixed: replacing a control byte with the 3-byte U+FFFD character could make sanitized output exceed its own 64 KiB bound (65592 observed) — fixed by re-enforcing the bound on the output, not just the input |
| 29 | Hostile IDs | PASS | HTTP integration | `task_23_hostile_and_stale_instance_ids_denied` | test fixture | Path traversal, percent-encoded, SQL-injection-shaped, 4 KB, control-character instance IDs — all denied safely |
| 30 | Config injection denial | PASS | HTTP integration | `task_6_production_config_injection_is_rejected` | test fixture | Every field an attacker might try (`executable`/`command`/`argv`/`env`/`image`/`mounts`/`privileged`/`host_network`/`port`/`upstream`/`url`/...) makes the whole request fail closed (422) via `deny_unknown_fields`, never silently dropped |
| 31 | OCI via clouddeskd | PASS | HTTP integration, real Docker | `oci_lifecycle_and_hardening_through_clouddeskd_api`, `oci_image_missing_fails_closed_through_the_api` (`runtime_api.rs::oci_through_product_api`) | disposable `alpine:latest`-based OCI test spec (never registered under a production kind) | Real local Docker daemon (29.7.2). Skips (not PASS) if Docker isn't reachable — did not skip this session |
| 32 | OCI hardening inspection | PASS | HTTP integration, real Docker | `oci_lifecycle_and_hardening_through_clouddeskd_api` | same | Live `docker inspect`: not privileged, cap-drop ALL, no-new-privileges, not host network, not host PID namespace, no Docker socket mount — inspected against the real container, not inferred from argv |
| 33 | OCI stop/removal race | PASS | orchestrator, real Docker | `task_16_hardened_container_full_lifecycle_start_health_stop_cleanup` (`live_oci.rs`); preserved through `oci_lifecycle_and_hardening_through_clouddeskd_api` | same | Original async-`--rm`-race regression (container could still exist momentarily after `stop()` returned) fixed and still verified gone-before-report-complete |
| 34 | Audit events | PASS | HTTP integration, direct DB query | `task_17_audit_events_are_recorded_with_safe_fields` | test fixture | Real rows verified against the append-only `audit_events` table (enable/disable-requested/enabled/disabled, instance start-requested/started/stopped, capability denial), with an explicit check that no row's `metadata_json`/`resource_id` contains secret-shaped content |
| 35 | Settings backend authorization | PASS | HTTP integration | `task_3_settings_authorization_matches_role_policy_for_every_role` | test fixture | Guest/User/Manager denied, Administrator allowed — identical for enable and disable; the Settings UI calls this exact endpoint, so there is no alternate "Settings-only" authorization path to diverge from |
| 36 | Settings frontend tests | PASS | Vitest unit tests | `apps/web/src/lib/runtime.test.ts` (15 tests) | — | Tests the decision logic (`visibleRuntimeCards`, `canManageRuntimes`, `sanitizeDetail`, `describeRuntimeError`) the Svelte component is a thin shell around — this project has no component-rendering test harness (same pattern as `video.test.ts`/`music.test.ts`); Settings *browser* acceptance is separately `BLOCKED BY ENVIRONMENT` (item 41) |
| 37 | Duplicate JSON security behavior | PASS | HTTP integration | `task_1_duplicate_json_keys_cannot_bypass_security` (`runtime_api.rs::closure_pass`) | test fixture | Real, verified behavior (not assumed): `CreateInstanceBody`'s derived `Deserialize` rejects any duplicate key outright (422) — serde's struct visitor errors on a second occurrence rather than merging last-value-wins. Documented as a *stronger* property than "one value safely wins," since no divergent value is ever produced |
| 38 | WebSocket binary-frame handling | PASS | HTTP integration, real network | `task_2_websocket_binary_frames_are_relayed`, `task_2_cross_user_and_unauthenticated_binary_attempts_denied` | test fixture (echo now relays Binary, not just Text) | Small/multiple/zero-length/pseudo-random binary payloads round-trip; cross-user and unauthenticated binary attempts denied via a real handshake |
| 39 | Oversized WebSocket bound | PASS | HTTP integration, real network; real defect fixed | `task_2_oversized_websocket_frame_is_rejected_not_unbounded` | test fixture | Real gap found and fixed this pass: the proxy previously relied entirely on axum/tungstenite's own library defaults (64 MiB message / 16 MiB frame) — present, but never a value CloudDesk itself chose. Both proxy legs now enforce an explicit 4 MiB message / 1 MiB frame bound |
| 40 | Fixture/process/container cleanup | PASS | orchestrator, real out-of-process kill | `task_4_shutdown_all_leaves_no_live_process` (`live_lifecycle.rs`); `task_4_5_fixture_process_does_not_survive_shutdown_all` (`runtime_api.rs::closure_pass`); manual real-SIGKILL verification (see `CLAUDE_ENGINEERING_CHECKPOINT.md`) | test fixture | Real defect found and fixed: an orphaned instance could previously outlive an abrupt death of its parent (this session's own earlier debug loops left 406 such processes running). Fixed with a kernel-enforced parent-death signal (`set_parent_process_death_signal(SIGKILL)`); verified with a genuine external `kill -9` against a deliberately long-lived probe process (child confirmed gone within 1s), and with two full `cargo test --workspace` runs completing with zero leftover `test-runtime-fixture` processes afterward each time |

## Environmental blockers (not implementation gaps)

| ID | Requirement | Status | Notes |
|----|-------------|--------|-------|
| 41 | cgroup v2 CPU enforcement | BLOCKED BY ENVIRONMENT | Rechecked this pass: `mkdir` under the delegated cgroup subtree still succeeds, but writing `cpu.max` fails with `Permission denied` (no sudo used, no host cgroup hierarchy mutated). Policy/primitives (`crates/orchestrator/src/cgroup.rs`) exist and are unit-tested (`detect_never_panics_and_reports_a_real_answer`) |
| 42 | cgroup v2 memory enforcement | BLOCKED BY ENVIRONMENT | Same recheck; `memory.max` write fails with `Permission denied` |
| 43 | cgroup v2 PIDs enforcement | BLOCKED BY ENVIRONMENT | Same recheck; `pids.max` write fails with `Permission denied` |
| 44 | Settings browser acceptance | BLOCKED BY ENVIRONMENT | Rechecked this pass: `which chromium chromium-browser google-chrome playwright` all absent from this container. Real backend/API evidence (items 01-40 above) and frontend unit-behavior evidence (item 36) stand in its place, per the task's own explicit allowance |

## Summary

- 38 of 40 numbered product/security matrix items: `PASS`, each citing
  a specific executable test.
- 1 item (`23`, persistent-profile retention): `NOT EXECUTED` — no
  persistent-kind adapter exists yet to retain data for (Phase 7+); the
  typed policy exists and is exercised for creation, just not for a
  real stop-then-verify-retained cycle.
- 0 items: `FAIL`.
- 0 items: `IMPLEMENTATION MISSING`.
- 4 items: `BLOCKED BY ENVIRONMENT`, all rechecked this pass, all
  unchanged, none from a missing implementation.

Two real defects were found and fixed during the work that produced
this matrix (items 28 and 40 above); both have regression tests that
are part of the evidence cited.
