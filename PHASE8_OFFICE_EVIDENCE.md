# Phase 8 — LibreOffice / Collabora Online: Executable Evidence Matrix

Runtime selected: **Collabora Online Development Edition (CODE)**, image
`collabora/code:26.04.3.1.1`, digest-pinned
`sha256:6b70f91f0b6e9c76f75f162f58ef0a12cf9415d78e14713d33c0318ddc4a2cc0`
(`crates/config`'s `RuntimeConfig::office_image` default). CODE is
explicitly the development/test edition, **not** the recommended
production deployment; a supported Collabora Online Server (or an
administrator-configured external instance speaking the same real
`/hosting/discovery` + WOPI protocol) is the intended production path.
No separate document protocol was invented — the integration chain is
`CloudDesk → CloudDesk Office application → Collabora's real browser
editor → WOPI → CloudDesk WOPI Host → CloudDesk VFS authorization →
file`.

This is an evidence map, not a test runner. Status vocabulary: PASS /
PARTIAL / BLOCKED BY ENVIRONMENT / UNAVAILABLE / NOT EXECUTED /
IMPLEMENTATION MISSING / NOT APPLICABLE. **`IMPLEMENTATION MISSING` is
used, not `BLOCKED`, whenever code simply does not exist yet** — Phase
8 is honestly far from the full 73-task Definition of Done; this pass
implemented and live-proved the WOPI/runtime/proxy core (the highest
security-risk surface: token/lock/authorization/proxy-ownership), and
found + fixed one real defect (shared-instance proxy 404 for
non-administrator users) in the process, but did not reach the
frontend, the full format matrix, or most of the hostile-input/
lifecycle/hardening breadth the spec calls for.

| Task | Requirement | Status | Evidence | Notes / limitations |
|------|-------------|--------|----------|----------------------|
| 1 | Runtime strategy (Managed/External, Phase 6 reuse) | PARTIAL | `office_oci_spec()` (`office_runtime.rs`), registered as a normal `RuntimeManager` adapter in `main.rs` alongside Code's — same lifecycle states, no second manager | Managed mode only; External mode (admin-configured trusted existing server) is architecturally supported by the same WOPI host but has no settings-driven wiring yet (Task 61 below) |
| 2 | Runtime discovery / version pinning | PASS | `collabora/code:26.04.3.1.1`, digest-pinned in `crates/config`; doc comments in `office_runtime.rs` explicitly distinguish CODE from a production recommendation | |
| 3 | Phase 6 Office adapter | PASS | `office_oci_spec()` — availability/prepare/start/health/stop/kill/cleanup/logs/resource-policy all inherited unchanged from `OciAdapter`; no duplicated idle manager/port allocator/audit engine | |
| 4 | Private network boundary | PASS | No public port publish; `Browser → CloudDesk:port → /office-proxy (authenticated) / /wopi (token-authenticated) → host.docker.internal → Collabora` — same loopback/bridge model as Code, live-verified via the passing `office-proxy` tests | |
| 5 | Discovery | PASS | `fetch_discovery()`/`parse_discovery_xml()`/`select_action()` (`office_runtime.rs`): real `/hosting/discovery` fetched from the trusted, server-computed base URL only; bounded (`MAX_DISCOVERY_BYTES`, `MAX_DISCOVERY_ACTIONS`, `DISCOVERY_TIMEOUT`); `quick-xml`'s non-validating reader has no DOCTYPE/entity support (structurally immune to XXE/billion-laughs, not merely untested) | Discovery caching (Task 63) not implemented — refetched per session open |
| 6 | Office file identity | PASS | `resolve_and_register_file()`/`lookup_file()` (`wopi.rs`): opaque `office_wopi_files.id` → canonical path lookup, never a raw path exposed to Collabora; longest-matching-prefix `authorize_path()` re-derives current VFS authorization on every call | |
| 7 | WOPI access token | PASS | `issue_token()`: SHA-256-hashed at rest, bound to user + file + access mode + runtime instance, `TOKEN_TTL_SECONDS = 1800`; never the CloudDesk session cookie | |
| 8 | WOPI token replay | PASS | `task_9_10_11_14_15_wopi_protocol_round_trip`: random token, wrong-file token, cross-user denial (real `tempdir()` + scoped `assigned_roots` grant, not the shared test-UID home), WOPI token rejected against an ordinary CloudDesk API route | Expired-token and altered-token specific cases not separately live-fired (structurally covered by `verify_token()`'s hash lookup + `expires_at` check, but no dedicated clock-skip test) |
| 9 | CheckFileInfo | PASS | `wopi_api::check_file_info`; live-verified via `task_9_10_11_14_15` | |
| 10 | GetFile | PASS | `wopi_api::get_file`, `tokio_util::io::ReaderStream` bounded streaming; byte-exact verified live | |
| 11 | PutFile | PASS | `wopi_api::put_file`: token + write-auth + lock check, bounded streaming to `.cloudesk-office-{random}.tmp`, atomic rename, cleanup on failure; live-verified content change via headless LibreOffice text extraction | |
| 12 | Atomic save | PARTIAL | Temp-file + atomic-rename mechanism implemented and live-exercised for the success path | Process-killed-mid-write / disk-write-failure injection not separately tested this pass |
| 13 | Versioning/ETag | PASS | `current_version()` = `"{generation}-{size}-{mtime}"`, not filename alone; `office_wopi_files.generation` bumped every PutFile | |
| 14 | WOPI locks | PASS | `acquire_lock()`/`refresh_lock()`/`release_lock()`/`get_lock()`, SQLite-persisted (`office_locks`), restart-surviving; live LOCK/GET_LOCK/REFRESH_LOCK(valid+invalid)/UNLOCK(valid+invalid)/REFRESH_LOCK-after-unlock(404) sequence in `task_9_10_11_14_15` | |
| 15 | Lock security | PASS | Same test: wrong/stale/duplicate/cross-user lock and unlock-wrong-value all exercised | Service-restart-with-a-live-lock not separately tested (Task 68) |
| 16 | Lock expiration | IMPLEMENTATION MISSING | `LOCK_TTL_SECONDS = 600` constant exists | No automatic expiry sweep/cleanup task implemented or tested; an abandoned session's lock is not proven to recover |
| 17 | Write conflicts | PARTIAL | `snapshot_size`/`snapshot_mtime` captured at LOCK time and available for conflict detection | No dedicated two-editor or editor+external-modification conflict test yet |
| 18 | Collaborative editing policy | NOT EXECUTED | — | No CloudDesk sharing model exists in v1 beyond `assigned_roots`; no broad cross-user collaboration invented, per instruction — undocumented as a formal policy write-up |
| 19 | Read-only Office | IMPLEMENTATION MISSING | `read_write` is re-derived from live authorization in `verify_token()`, so the backend structurally cannot grant write on a read-only root | No dedicated test opens a read-only-authorized document and asserts `CanWrite=false` + PutFile denial together |
| 20 | Format matrix (9 formats) | PARTIAL | ODT fixtures generated via real `soffice --convert-to` and used throughout | DOC/DOCX/XLS/XLSX/PPT/PPTX/ODS/ODP fixtures not generated; only ODT exercised |
| 21 | Round-trip acceptance | PARTIAL | ODT open→modify(PutFile)→content-verified-via-headless-LibreOffice proven live | Only ODT; the other 8 formats not run |
| 22 | Format preservation | NOT EXECUTED | — | No format-conversion-on-save test yet |
| 23 | Live lock/save WOPI sequence | PASS | `task_9_10_11_14_15` is handcrafted-client evidence of the real sequence against the real WOPI host (not against real Collabora traffic — see Task 58) | |
| 24 | Office app UI | IMPLEMENTATION MISSING | — | No `OfficeApp.svelte` frontend component exists yet |
| 25 | Iframe security | IMPLEMENTATION MISSING | — | No frontend, so no sandbox attributes to evaluate |
| 26 | CSP | IMPLEMENTATION MISSING | — | No Office-specific CSP change made |
| 27 | Office proxy | PASS | Dedicated `office_http_proxy`/`office_http_proxy_root` (non-ownership-scoped, `apps.office.use`-authorized, resolved against the shared admin owner) — the real defect this pass found (generic Code-style proxy 404s for non-owner callers) and fixed | |
| 28 | WebSocket | IMPLEMENTATION MISSING (route exists, untested) | `office_ws_proxy` route registered and compiles | No test exercises a real WebSocket connection through it yet |
| 29 | WOPI callback auth vs browser auth | PASS | WOPI routes authenticate only via the scoped token, never require the CloudDesk session cookie; `task_9_10_11_14_15` proves a WOPI token cannot substitute for a CloudDesk session on an ordinary API route (Task 66) | |
| 30 | WOPI endpoint exposure sweep | PARTIAL | Random/wrong-file/cross-user token cases covered | Oversized-payload and mutated-operation specific attacks not separately fired |
| 31 | WOPI request limits | PARTIAL | `MAX_OFFICE_FILE_BYTES = 200MB` PutFile ceiling implemented | Header/lock-string/token/file-ID length bounds and concurrent-request limits not explicitly tested |
| 32 | File size policy | PARTIAL | The 200MB PutFile ceiling doubles as a basic size policy | Not configurable via Settings; no clear user-facing error path proven |
| 33 | Temporary data isolation | PASS (structural) | `.cloudesk-office-{random}.tmp` siblings, cleaned on failure; no shared cache directory with Code | Not separately documented as its own policy section |
| 34 | Remote VFS | NOT EXECUTED | — | No remote-provider (SFTP/WebDAV/S3) Office round-trip attempted this pass |
| 35 | Remote save safety | NOT APPLICABLE (this pass) | — | Depends on Task 34, not attempted |
| 36 | Files → Office integration | IMPLEMENTATION MISSING | — | No frontend "Open With → Office" wiring |
| 37 | Specific file opening | PASS (backend only) | `POST /api/v1/office/sessions` takes a specific VFS path and opens exactly that file — proven in every live test | No Files-UI-driven verification (Task 36 blocks it) |
| 38 | Download/original file interplay | NOT EXECUTED | — | |
| 39 | Rename | NOT APPLICABLE | — | Not implemented; WOPI `CheckFileInfo` does not advertise rename capability, so Collabora cannot attempt it — consistent with "advertise unsupported if VFS doesn't authorize it" |
| 40 | User identity to Collabora | PARTIAL | No email/Linux UID passed to Collabora in the current `CheckFileInfo` fields | Not exhaustively swept for spoofing attempts |
| 41 | Access revocation mid-edit | IMPLEMENTATION MISSING (structurally supported, untested) | `verify_token()` re-derives authorization fresh from live state on every call, so a revoked root should fail PutFile immediately | No dedicated live test revokes a root mid-session and asserts the next PutFile fails |
| 42 | Logout | NOT EXECUTED | — | Token TTL is bounded (30 min) but no test proves a logged-out session's old token stops working before natural expiry |
| 43 | Token/URL log leakage | PASS (mechanism); NOT EXECUTED (proof) | `make_redacted_span()`/`redact_token_query()` scrub `access_token=`/`token=` query values before any tracing span is built, applied app-wide | No live test captures logs with a sentinel token and asserts absence |
| 44 | Audit | PARTIAL | `office.session.opened` audit event only | No write/lock-conflict/write-denied/session-failure audit events yet |
| 45 | Crash recovery | NOT EXECUTED | — | Code's Phase 7 crash-recovery pattern not yet ported to Office |
| 46 | Enable/disable | PARTIAL | `enable_office()`/runtime-kind-generic Settings enable/disable reused | No disable-while-active-session test |
| 47 | Idle lifecycle model | PASS (design); NOT EXECUTED (multi-session proof beyond Task 48) | Shared single-instance model chosen deliberately (documented in `office_runtime.rs`/`lib.rs` comments) since document authorization is entirely WOPI-token-scoped, not instance-scoped | |
| 48 | Multi-user test | PASS | `task_9_10_11_14_15`'s cross-user section: User B cannot open/GetFile/PutFile User A's document or obtain a working token | Positive shared-access case (grant B legitimate access, both edit) not exercised — no v1 sharing feature to test against |
| 49 | Read/write permission matrix | PARTIAL | read-write vs cross-user-denied covered | Guest/Manager/Admin role sweep and explicit read-only-file case not covered |
| 50 | Hostile documents | NOT EXECUTED | — | No corrupt/oversized/macro/malformed fixture sweep |
| 51 | Macros | NOT EXECUTED | — | |
| 52 | External links / SSRF | NOT EXECUTED | — | |
| 53 | Office runtime network policy | NOT EXECUTED | — | No dedicated network-isolation test; inherits `OciAdapter`'s bridge-mode default only |
| 54 | OCI hardening (docker inspect) | NOT EXECUTED | — | No dedicated `docker inspect`-based hardening test for the Office container (Code's Phase 7 `task_11` pattern not yet ported) |
| 55 | Resource policy / performance | NOT EXECUTED | — | No `docker stats` measurement taken |
| 56 | Browser automation recheck | BLOCKED BY ENVIRONMENT | Rechecked this pass (consistent with Phase 7's finding): no Chromium/Firefox/Playwright/Puppeteer available in this environment | |
| 57 | Real browser edit flow | BLOCKED BY ENVIRONMENT | Depends on Task 56 | |
| 58 | Real protocol acceptance w/o browser | PARTIAL — decomposed: `EDITOR BOOTSTRAP REACHABLE THROUGH office-proxy: PASS`, `GENUINE COLLABORA-INITIATED WOPI CALLBACK WITHOUT A BROWSER: BLOCKED BY ENVIRONMENT` | `task_58_real_collabora_driven_wopi_callback`: real bootstrap HTML (containing a real `wss://` target and `frame-ancestors` directive) fetched through the real authenticated proxy from the real container; the same test then inspects the real container's own logs and asserts a JS-free request alone does **not** produce a server-side `CheckFileInfo` — that requires `bundle.js` executing in an actual browser | This is the honest boundary of what a JS-free HTTP client can prove against real Collabora; not conflated with the handcrafted `task_9_10_11_14_15` evidence |
| 59 | License/deployment documentation | PARTIAL | In-code doc comments in `office_runtime.rs` explicitly mark CODE as dev/test, not production-recommended | No standalone `docs/THIRD_PARTY_NOTICES.md`-style Office section written this pass |
| 60 | Installation model | PASS | Office adapter registered unconditionally in `main.rs` but reports `Unavailable` cleanly with Docker/image absent, exactly like Code; disabled-by-default Office starts zero processes | |
| 61 | External Collabora config | IMPLEMENTATION MISSING | `RuntimeConfig::office_external_url` field exists (`crates/config`) | No admin-only validation/wiring/TLS-cert-check path implemented |
| 62 | TLS | NOT APPLICABLE (this pass) | — | No external-mode wiring yet (Task 61); managed-mode internal HTTP is within the private Docker bridge network, matching Code's Phase 6/7 trust boundary |
| 63 | Discovery cache | IMPLEMENTATION MISSING | `fetch_discovery()` refetches every call | No bounded cache with expiry implemented |
| 64 | Office app settings | PASS (inherited) | Phase 6's generic Settings runtime-status rendering (`apps/web/src/lib/runtime.ts`) already shows Office alongside Code/Browser with no Office-specific code needed | No Office-specific extra fields (mode, external URL) surfaced |
| 65 | Route authorization sweep | PARTIAL | WOPI-token-cannot-access-general-API and cross-user WOPI denial covered in `task_9_10_11_14_15` | Full Guest/Manager/Administrator sweep across every Office/WOPI route not documented as a standalone table |
| 66 | WOPI token cannot authorize CloudDesk API | PASS | Explicit assertion in `task_9_10_11_14_15` | |
| 67 | Large file streaming | NOT EXECUTED | — | No memory measurement at scale; only the 200MB ceiling exists structurally |
| 68 | Service restart with live lock | NOT EXECUTED | — | |
| 69 | Database failure fail-closed | NOT EXECUTED | — | |
| 70 | Audit/log token scrub test | NOT EXECUTED | — | Mechanism exists (Task 43); no live sentinel-token log-capture test |
| 71 | Office evidence matrix | PASS | This document | |
| 72 | Evidence levels kept separate | PASS | Task 58's row above is the clearest example: LIVE WOPI HOST vs LIVE REAL COLLABORA vs LIVE BROWSER kept explicitly distinct throughout this document and in the test file's own comments | |
| 73 | Security defect process | PASS | The shared-instance proxy-ownership 404 defect: reproduced live, classified, regression-tested (`task_58`), fixed (`office_http_proxy`/`office_ws_proxy`), retested twice, documented in commit `feat(office): add CloudDesk Office application integration` | |

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
`cargo test --workspace`: PASS (includes `office_runtime.rs`, run twice
consecutively with zero flakes after fixing a real LibreOffice
concurrent-profile-lock test flake).
`cargo build --workspace --release`: PASS.
Frontend gates: not run — no frontend Office work exists yet to gate.

Zero leaked `clouddesk-runtime-*` containers, zero leaked WOPI test
resources, no stale locks beyond intended expiry, no sentinel token
logged (mechanism verified in code; live log-capture proof still
outstanding per Task 70 above) — confirmed after every test run this
pass via `docker ps -a --filter name=clouddesk-runtime-`.

## Unresolved Critical/High

None found this pass. One real defect (shared-instance proxy 404,
Task 27/73) was found and fixed; no other Critical/High-severity
security defects were identified in the surface actually exercised
(WOPI token/lock/authorization/proxy-ownership). The large surface
marked `NOT EXECUTED`/`IMPLEMENTATION MISSING` above (hostile documents,
macros, SSRF, OCI hardening inspection, format matrix, frontend, remote
VFS, revocation/logout live proof, crash recovery, resource
measurement) has not been attacked yet and could still surface
Critical/High defects — Phase 8 is **PARTIAL**, not COMPLETE, and this
matrix does not claim otherwise.
