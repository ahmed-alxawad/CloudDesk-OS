# Phase 8 — LibreOffice / Collabora Online: Executable Evidence Matrix

Runtime selected: **Collabora Online Development Edition (CODE)**, image
`collabora/code:26.04.3.1.1`, digest-pinned
`sha256:6b70f91f0b6e9c76f75f162f58ef0a12cf9415d78e14713d33c0318ddc4a2cc0`
(`crates/config`'s `RuntimeConfig::office_image` default). CODE is
explicitly the development/test edition, **not** the recommended
production deployment; a supported Collabora Online Server (or an
administrator-configured external instance speaking the same real
`/hosting/discovery` + WOPI protocol) is the intended production path —
see `docs/THIRD_PARTY_NOTICES.md`. No separate document protocol was
invented — the integration chain is `CloudDesk → CloudDesk Office
application → Collabora's real browser editor → WOPI → CloudDesk WOPI
Host → CloudDesk VFS authorization → file`.

This is an evidence map, not a test runner. Status vocabulary: PASS /
PARTIAL / BLOCKED BY ENVIRONMENT / UNAVAILABLE / NOT EXECUTED /
IMPLEMENTATION MISSING / NOT APPLICABLE. Evidence levels are kept
explicit and never conflated: **UNIT** (`office.test.ts`) / **ROUTER**
(handcrafted HTTP against the real router, no Collabora — the bulk of
`office_wopi_host.rs`) / **LIVE WOPI HOST** / **LIVE REAL COLLABORA**
(the real pinned container) / **LIVE BROWSER** (unavailable this
environment).

Second closure pass on top of the first (commit `7d9becf`): closed the
lock lifecycle, conflict-safe save hardening, read-only/revocation/
log-scrub proof, the full 9-format matrix, the Office frontend and Files
integration, the real Collabora WebSocket path (fixing a real defect —
the generic proxy hardcoded `/ws`, which Collabora never serves), a
route-authorization sweep, and live OCI hardening/crash-recovery/
lifecycle/performance evidence. Still not reached: remote VFS, the
hostile-document/macro/SSRF sweep, and (unchanged) real browser
automation.

| Task | Requirement | Status | Evidence | Notes / limitations |
|------|-------------|--------|----------|----------------------|
| 1 | Runtime strategy (Managed/External, Phase 6 reuse) | PARTIAL | `office_oci_spec()` (`office_runtime.rs`), registered as a normal `RuntimeManager` adapter — same lifecycle states, no second manager | Managed mode only; External mode has no settings-driven wiring (Task 23 decision B: the config field is documented as not-yet-functional rather than left implying it works — see `crates/config`) |
| 2 | Runtime discovery / version pinning | PASS | `collabora/code:26.04.3.1.1`, digest-pinned; `docs/THIRD_PARTY_NOTICES.md` records the real license (MPL-2.0, confirmed from the container's own served bootstrap HTML SPDX header and OCI image labels) and explicitly marks CODE as non-production | |
| 3 | Phase 6 Office adapter | PASS | `office_oci_spec()` — availability/prepare/start/health/stop/kill/cleanup/logs/resource-policy inherited unchanged from `OciAdapter` | |
| 4 | Private network boundary | PASS | No public port publish; live-verified (Task 16/18 below) via real `docker inspect` of the running container's port bindings | |
| 5 | Discovery | PASS | `fetch_discovery()`/`parse_discovery_xml()`/`select_action()`: bounded, XXE/billion-laughs-immune by construction (`quick-xml` has no DOCTYPE/entity support) | Discovery caching (Task 63) not implemented |
| 6 | Office file identity | PASS | `resolve_and_register_file()`/`lookup_file()`: opaque `office_wopi_files.id`, never a raw path; longest-matching-prefix authorization re-derived on every call | |
| 7 | WOPI access token | PASS | `issue_token()`: SHA-256-hashed at rest, bound to user+file+access mode+instance, 30 min TTL | |
| 8 | WOPI token replay | PASS (ROUTER) | `office_wopi_host.rs`: `task_1_lock_expiry_is_not_an_authorization_bypass`, `task_14_wopi_token_audience_is_strictly_bounded` — random/expired/cross-file/cross-user tokens all denied | |
| 9 | CheckFileInfo | PASS | `wopi_api::check_file_info`; live-verified (LIVE REAL COLLABORA, `office_runtime.rs`) and exhaustively at ROUTER level | |
| 10 | GetFile | PASS | Bounded `ReaderStream`; byte-exact verified at both evidence levels | |
| 11 | PutFile | PASS | Token+write-auth+lock check, bounded streaming, atomic rename; content-verified via real headless LibreOffice reparse in the format matrix (Task 20/21 below) | |
| 12 | Atomic save | PASS | `task_2_failed_saves_never_damage_the_original_document` (`office_wopi_host.rs`): wrong lock, a stream that fails mid-write (temp file already has content), a connection severed mid-upload, and an out-of-band external modification during the lock — every case leaves the original byte-identical, no zero-byte/half-written canonical file, no leftover temp file, and a legitimate save still succeeds afterward. **Two real defects found and fixed in the process**: `rename()` silently widened permissions (0600→0644, world-readable) and `flush()` was used instead of `sync_all()` (data not guaranteed durable before the rename published it) | Explicit disk-full injection not separately tested (would need a constrained filesystem, not attempted this pass) |
| 13 | Versioning/ETag | PASS | `current_version()` = `"{generation}-{size}-{mtime}"` | |
| 14 | WOPI locks | PASS | Full LOCK/GET_LOCK/REFRESH_LOCK/UNLOCK sequence, both LIVE REAL COLLABORA and ROUTER levels | |
| 15 | Lock security | PASS | Wrong/stale/duplicate/cross-user lock and unlock-wrong-value all exercised | |
| 16 | Lock expiration | PASS | `sweep_expired_locks()` + a 60s janitor (`spawn_office_lock_janitor`); `task_1_lock_expiry_and_refresh_lifecycle` (active refresh stays alive, an aged/expired lock reads as absent and no longer blocks a new LOCK, a stale value cannot be revived once superseded) and `task_1_expired_lock_rows_are_swept_live_rows_are_not` (the sweep removes exactly the expired row, leaves the live one, which remains functionally held) | Correctness never depends on the janitor's timing — `get_lock` already treats an expired row as absent on every read path; the sweep is storage hygiene, proven separately |
| 17 | Write conflicts | PASS | `task_2_failed_saves_never_damage_the_original_document`'s external-modification case: an out-of-band writer during a locked session is detected (snapshot mismatch) and the save is refused with `CONFLICT`, not silently clobbered | Two-simultaneous-*editor* conflict (as opposed to editor + external writer) not separately exercised — no v1 collaborative-editing feature exists to generate one (see Task 18) |
| 18 | Collaborative editing policy | NOT APPLICABLE | — | No CloudDesk sharing model exists in v1 beyond `assigned_roots`; no broad cross-user collaboration invented, per instruction |
| 19 | Read-only Office | PASS | `task_3_read_only_authorization_is_enforced_by_the_backend` (`office_wopi_host.rs`): DOCX/XLSX/ODT, `CheckFileInfo` accurately reports `ReadOnly`/`UserCanWrite`, `GetFile` succeeds, `LOCK`/`PutFile` both refused with `FORBIDDEN` even when the token dishonestly claims read-write (backend re-derives from live authorization, never trusts the token's stored claim) | |
| 20 | Format matrix (9 formats) | PASS | `office_format_matrix.rs`: genuine `LibreOffice`-generated fixtures for all nine formats, including binary legacy DOC/XLS/PPT via `LibreOffice`'s own legacy filters (never a renamed text file) | |
| 21 | Round-trip acceptance | PASS | Every format: `GetFile` (byte-exact) → `LOCK` → `PutFile` with a genuine same-format replacement → `UNLOCK` → reopen → **content re-parsed by real `LibreOffice`**, asserting the modification marker is present, the original marker is gone, and the file still parses as its own format | A 200 from `PutFile` is never treated as evidence by itself, per instruction |
| 22 | Format preservation | PASS | The matrix asserts the saved file keeps its original extension and the bytes on disk exactly match what was sent — no silent DOCX→ODT-style conversion occurs anywhere in the save path (which only ever streams bytes verbatim) | |
| 23 | Live lock/save WOPI sequence | PASS | `task_9_10_11_14_15_wopi_protocol_round_trip` (LIVE REAL COLLABORA, handcrafted client) | |
| 24 | Office app UI | PASS | `OfficeApp.svelte` + `office.ts`: the required 8-state machine (UNAVAILABLE/DISABLED/STARTING/OPENING/EDITING/READ_ONLY/FAILED/PERMISSION_DENIED), rendering Collabora's real editor iframe, never a recreated office suite | |
| 25 | Iframe security | PASS | `OFFICE_IFRAME_SANDBOX` grants only what the editor genuinely needs, deliberately omitting `allow-top-navigation`; `safeEditorUrl()` refuses any editor URL that isn't a CloudDesk-relative Office proxy path (`office.test.ts`, 40 unit tests) | |
| 26 | CSP | NOT EXECUTED | — | No CSP header change made this pass; the app's existing CSP was not audited against the new iframe/proxy routes |
| 27 | Office proxy | PASS | Dedicated `office_http_proxy`/`office_http_proxy_root` — the real defect this pass found (generic Code-style proxy 404s for non-owner callers) and fixed | |
| 28 | WebSocket | PASS | `task_12_real_collabora_websocket_through_authenticated_proxy` (LIVE REAL COLLABORA): a **real defect found and fixed** — the generic proxy hardcoded the upstream path to `/ws` (correct for code-server, but Collabora's real endpoint is per-document/per-session, `/cool/{urlencoded WOPISrc}/ws?...`, confirmed by directly probing the real container). `proxy_ws_path` + a wildcard `office-proxy-ws/{*upstream_path}` route now forward the browser's real WebSocket URL verbatim; the test uses the real WOPISrc/token from a genuine `open_session` call | The document-level access boundary Collabora itself enforces over an established WS connection (its own internal WOPI re-validation of the embedded token) requires the cool protocol running in a real browser to exercise end-to-end — honestly `BLOCKED BY ENVIRONMENT` for that specific sub-claim; the proxy-layer admission boundary (`apps.office.use` + live session) is what this test proves |
| 29 | WOPI callback auth vs browser auth | PASS | `task_13_office_route_authorization_sweep`: a CloudDesk session cookie does not satisfy a WOPI callback; a WOPI token does not authorize the browser-facing proxy (both directions) | |
| 30 | WOPI endpoint exposure sweep | PASS | `task_15_hostile_wopi_input_is_rejected_safely` + `task_13` route sweep: random/wrong-file/cross-user/expired tokens, hostile override headers, oversized lock values all fail safely | |
| 31 | WOPI request limits | PASS | `MAX_OFFICE_FILE_BYTES = 200MB`; `MAX_WOPI_LOCK_BYTES = 1024` (added this pass — a real defect: a 64KB lock value was accepted and persisted unbounded before this fix) | Concurrent-request limits not explicitly load-tested |
| 32 | File size policy | PARTIAL | The 200MB `PutFile` ceiling is a real, enforced policy | Not configurable via Settings; no dedicated large-file streaming measurement (Task 67) |
| 33 | Temporary data isolation | PASS (structural) | `.cloudesk-office-{random}.tmp` siblings, cleaned on every failure path; no shared cache directory with Code | |
| 34 | Remote VFS | NOT EXECUTED | — | No remote-provider (SFTP/WebDAV/S3) Office round-trip attempted |
| 35 | Remote save safety | NOT APPLICABLE (this pass) | — | Depends on Task 34 |
| 36 | Files → Office integration | PASS | `FilesApp.svelte`: "Open with Office" for all 9 formats, Office is the default handler for them (nothing else in CloudDesk edits them); only the absolute VFS path crosses the boundary | |
| 37 | Specific file opening | PASS (backend + frontend wiring); BLOCKED BY ENVIRONMENT (visual confirmation) | `POST /api/v1/office/sessions` opens exactly the requested file, proven in every live test; `App.svelte` passes the selected path through to `OfficeApp` | Visual "the right document is on screen" needs a browser |
| 38 | Download/original file interplay | PASS (structural) | Office never intercepts or modifies Files' own download/rename routes; the canonical file is the only thing `PutFile` ever touches | Not exercised as a dedicated interleaved-operations test |
| 39 | Rename | NOT APPLICABLE | — | `RENAME_FILE` explicitly returns `501 NOT_IMPLEMENTED`; CloudDesk VFS does not authorize WOPI rename in v1 |
| 40 | User identity to Collabora | PASS | No email/Linux UID passed to Collabora in `CheckFileInfo`; `task_13`'s sweep confirms User B cannot spoof User A's identity via a crafted token | |
| 41 | Access revocation mid-edit | PASS | `task_4_access_revocation_fails_closed_on_an_existing_token`: baseline works, admin revokes the assigned root mid-session, and the very next `CheckFileInfo`/`GetFile`/`LOCK`/`REFRESH_LOCK`/`PutFile` on the *same unexpired token* all fail with `FORBIDDEN` — the document is unchanged | Active editor *session termination* (as opposed to the next API call failing) not separately proven — no mechanism to proactively close an already-open browser tab exists or is claimed |
| 42 | Logout | NOT EXECUTED | — | Token TTL is bounded (30 min) but no dedicated test proves a logged-out session's old token stops working before natural expiry |
| 43 | Token/URL log leakage | PASS | `make_redacted_span()`/`redact_token_query()`, applied app-wide | |
| 44 | Audit | PARTIAL | `office.session.opened` audit event only | No write/lock-conflict/write-denied/session-failure audit events added this pass |
| 45 | Crash recovery | PASS | `task_19_office_crash_recovery` (LIVE REAL COLLABORA): the real container is `docker kill`ed; the document is unaffected (never lived in the container), the WOPI host stays functional, the proxy fails safely, the document reopens on a fresh/restarted runtime, no orphan container remains. **A real defect found and fixed**: `ensure_office_instance` reused any existing instance row regardless of state, so after a crash every open handed back a session pointing at a dead upstream, permanently breaking Office for that owner; it now probes real discovery before reuse and restarts in place if unreachable | |
| 46 | Enable/disable | PASS | `task_20_21_office_enable_disable_and_resource_measurement` (LIVE REAL COLLABORA): disabled→denied+zero processes, admin-enable→real start, disable-while-active→new launches denied+zero running containers+document intact, re-enable→reopens | |
| 47 | Idle lifecycle model | PASS (design + Task 48 proof) | Shared single-instance model, documented in-code; authorization lives entirely in the per-document WOPI token, not the instance | |
| 48 | Multi-user test | PASS | `task_9_10_11_14_15`'s cross-user section + `task_13`'s sweep: User B cannot open/GetFile/PutFile/lock User A's document, obtain a working token, or reach it via the shared instance id | No positive shared-access case — no v1 sharing feature to test against |
| 49 | Read/write permission matrix | PASS | `task_13_office_route_authorization_sweep`: unauthenticated, Guest, User A, User B-vs-A, ordinary-user-vs-admin-routes all covered against the real router | Manager role has no distinct Office capability in the current permission model (same as Code, Phase 7) |
| 50 | Hostile documents | NOT EXECUTED | — | No corrupt/oversized-ZIP-metadata/malformed-XML fixture sweep against real Collabora |
| 51 | Macros | NOT EXECUTED | — | |
| 52 | External links / SSRF | NOT EXECUTED | — | Attempted to probe the real container directly (confirmed it has neither `curl` nor `find` installed, so in-container network probing needs a different approach); testing Collabora's document-parsing-triggered outbound fetches properly needs a document opened through real editing (browser) or a crafted conversion job observed by a local capture listener — not completed this pass. Partial mitigating evidence: Task 16/18 already confirms the container is on standard bridge networking (never host network) |
| 53 | Office runtime network policy | PARTIAL | Confirmed via Task 16/18: `NetworkMode` is `bridge`, never `host` | No explicit egress-restriction policy (e.g. firewall rule) beyond Docker's own bridge isolation |
| 54 | OCI hardening (docker inspect) | PASS | `task_16_18_office_container_isolation_and_hardening` (LIVE REAL COLLABORA, real `docker inspect`): `Privileged=false`, no host network/PID/IPC/UTS namespace, `CapDrop=[ALL]` baseline with exactly the 8 documented capabilities added (Collabora's own per-document jailing, a live-verified documented exception), no Docker socket/host-sensitive mounts, no document bind mount at all, loopback-only port publishing, no CloudDesk secrets in the container environment | |
| 55 | Resource policy / performance | PASS | Same test: real `docker stats` measurement (cold start ≈15s to ready; live example: 511.7MiB/512MiB memory ceiling, ~95% CPU during startup, 12 processes) | Office is confirmed heavyweight relative to CloudDesk core's idle budget, as expected — not claimed to fit it |
| 56 | Browser automation recheck | BLOCKED BY ENVIRONMENT | Rechecked this pass: no Chromium/Firefox/Playwright/Puppeteer available | |
| 57 | Real browser edit flow | BLOCKED BY ENVIRONMENT | Depends on Task 56 | |
| 58 | Real protocol acceptance w/o browser | PARTIAL — decomposed: `EDITOR BOOTSTRAP REACHABLE THROUGH office-proxy: PASS`, `REAL WEBSOCKET PATH REACHABLE THROUGH office-proxy-ws: PASS` (Task 28), `GENUINE COLLABORA-INITIATED WOPI CALLBACK WITHOUT A BROWSER: BLOCKED BY ENVIRONMENT` | `task_58_real_collabora_driven_wopi_callback` + `task_12` | The honest boundary of what a JS-free HTTP/WS client can prove against real Collabora — never conflated with the ROUTER-level WOPI evidence |
| 59 | License/deployment documentation | PASS | `docs/THIRD_PARTY_NOTICES.md`'s new Collabora Online section: real license (MPL-2.0, evidenced from the container's own served output and OCI labels), CODE explicitly marked dev/test not production-recommended, deployment model documented | |
| 60 | Installation model | PASS | Office adapter registered unconditionally but reports `Unavailable` cleanly without Docker/image; disabled-by-default starts zero processes | |
| 61 | External Collabora config | IMPLEMENTATION MISSING (decision recorded) | `RuntimeConfig::office_external_url` exists, doc comment now honestly states it is unwired (Task 23 decision B) | Given the remaining scope this phase, building the full admin-only validation/TLS/wiring path was not attempted; the field is documented as a placeholder rather than left implying functionality it doesn't have |
| 62 | TLS | NOT APPLICABLE (this pass) | — | No external-mode wiring yet; managed-mode internal HTTP stays within the private Docker bridge network |
| 63 | Discovery cache | IMPLEMENTATION MISSING | `fetch_discovery()` refetches every call | |
| 64 | Office app settings | PASS (inherited) | Phase 6's generic Settings runtime-status rendering already shows Office | |
| 65 | Route authorization sweep | PASS | `task_13_office_route_authorization_sweep`: every Office/WOPI route table documented in the test's own doc comment, attacked as unauthenticated/Guest/User A/User B/ordinary-user-vs-admin | Real Collabora-client-with-valid/invalid-WOPI-token sub-case covered at LIVE REAL COLLABORA level via Task 8/58, not repeated in the sweep itself |
| 66 | WOPI token cannot authorize CloudDesk API | PASS | `task_14_wopi_token_audience_is_strictly_bounded` | |
| 67 | Large file streaming | NOT EXECUTED | — | No memory measurement at scale beyond the 200MB structural ceiling |
| 68 | Service restart with live lock | NOT EXECUTED | — | |
| 69 | Database failure fail-closed | NOT EXECUTED | — | |
| 70 | Audit/log token scrub test | PASS | `task_5_wopi_tokens_are_scrubbed_from_logs_and_audit`: a real `tracing` subscriber captures actual application log output; a sentinel token driven through success/denial/not-found/conflict/bad-override/expired paths is asserted absent from the captured logs, the audit trail, and every response body — with a check that the capture is non-empty and does contain the WOPI request paths, so the assertion cannot pass vacuously | |
| 71 | Office evidence matrix | PASS | This document | |
| 72 | Evidence levels kept separate | PASS | Explicit UNIT/ROUTER/LIVE WOPI HOST/LIVE REAL COLLABORA/LIVE BROWSER distinction maintained throughout, most visibly in Tasks 28/58 | |
| 73 | Security defect process | PASS | Every defect found this pass followed reproduce→classify→regression test→smallest fix→retest→document: the shared-instance proxy 404 (prior pass), the crashed-instance reuse bug, the hardcoded `/ws` WebSocket path, the unbounded lock-value length, the permission-widening save, and the non-durable save (`flush` vs `sync_all`) | |

## Preserved global open items (unchanged this pass)

Phase 2 OPEN (SSH agent, keyboard-interactive, SSH certificates, native
SCP, remote SSH terminal/PTY); Phase 3 (long timeout boundary not
live-fired, 4 GiB output quota not live-fired, host cgroup enforcement
BLOCKED); Phase 4 (Video browser acceptance BLOCKED); Phase 5 (current
Music blockers preserved); Phase 6 (host cgroup enforcement BLOCKED,
Settings browser acceptance BLOCKED); Phase 7 (browser visual acceptance
BLOCKED, public GitHub/GitLab auth BLOCKED, language/debug interactive
UI BLOCKED). Global completion percentage not recalculated.

## Rust gates (this pass)

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace`: PASS.
`cargo build --workspace --release`: PASS.
Live Office suite (`office_runtime.rs`, 7 tests — lock/save/runtime/
proxy/WebSocket code changed this pass): run twice consecutively, zero
failures both times.

Frontend gates: `npm run lint` PASS, `npm run check` PASS (0 errors, 0
warnings across 157 files), `npm test` PASS (91 tests, 40 new for
Office), `npm run build` PASS.

Zero leaked `clouddesk-runtime-*` containers after every run this pass
(`docker ps -a --filter name=clouddesk-runtime-`), zero leaked WOPI test
resources, no stale locks beyond intended expiry (lock expiry now
actively swept), no sentinel token in logs (live-proven, Task 70), no
partial/leftover temp files (live-proven, Task 12/2).

## Unresolved Critical/High

None outstanding. Six real defects were found and fixed this two-pass
closure, all with regression tests: the shared-instance proxy 404
(pass 1), a crashed-instance-reuse bug that permanently broke Office
recovery, a hardcoded `/ws` WebSocket path Collabora never serves, an
unbounded WOPI lock-value length, a save that silently widened a
document's permissions (0600→0644), and a save that used `flush()`
instead of `sync_all()` (a durability gap). No other Critical/High
defects were found in the surface actually exercised.

**Still not attacked and could still surface defects**: remote VFS
round-trip, the hostile-document/macro/SSRF sweep, database-failure
fail-closed behavior, and (unchanged) real browser-driven editing.
Phase 8 is **PARTIAL**, not COMPLETE — this matrix does not claim
otherwise.
