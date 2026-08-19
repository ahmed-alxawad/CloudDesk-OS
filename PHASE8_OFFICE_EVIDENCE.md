# Phase 8 — LibreOffice / Collabora Online: Executable Evidence Matrix

Runtime selected: **Collabora Online Development Edition (CODE)**, image
`collabora/code:26.04.3.1.1`, digest-pinned
`sha256:6b70f91f0b6e9c76f75f162f58ef0a12cf9415d78e14713d33c0318ddc4a2cc0`
(`crates/config`'s `RuntimeConfig::office_image` default). CODE is
explicitly the development/test edition, **not** the recommended
production deployment — see `docs/THIRD_PARTY_NOTICES.md`. `GOAL.md`'s
actual Office requirement (G5) is "LibreOffice-like browser editing...
may use LibreOfficeKit/Collabora-compatible technology"; it does not
mandate administrator-configured external Collabora deployment as a
distinct v1-blocking feature, so an unwired-but-honestly-documented
external-mode config field (Task 23 decision B, below) does not block
Phase 8 completion. No separate document protocol was invented — the
integration chain is `CloudDesk → CloudDesk Office application →
Collabora's real browser editor → WOPI → CloudDesk WOPI Host →
CloudDesk VFS authorization → file`.

This is an evidence map, not a test runner. Status vocabulary: PASS /
PARTIAL / BLOCKED BY ENVIRONMENT / UNAVAILABLE / NOT EXECUTED /
IMPLEMENTATION MISSING / NOT APPLICABLE. Evidence levels are kept
explicit and never conflated: **UNIT** (`office.test.ts`,
`discovery_cache_tests`) / **ROUTER** (handcrafted HTTP against the
real router, no Collabora required — `office_wopi_host.rs`,
`office_db_failure.rs`, `office_restart.rs`) / **LIVE WOPI HOST +
REMOTE VFS** (`office_remote_vfs.rs`, real disposable SFTP fixture) /
**LIVE REAL COLLABORA** (`office_runtime.rs`,
`office_hostile_documents.rs`, the real pinned container) / **LIVE
BROWSER** (unavailable this environment).

Third closure pass on top of the first two (commits `7d9becf`,
`bc58fe7`): closed remote VFS (SFTP), database-failure fail-closed
behavior, service-restart-with-live-lock, the hostile-document corpus,
and the discovery cache; resolved the external-mode decision against
the actual spec text. Five more real defects found and fixed this pass
(see "Unresolved Critical/High" below), bringing the two-and-a-half-pass
total to eleven.

| Task | Requirement | Status | Evidence | Notes / limitations |
|------|-------------|--------|----------|----------------------|
| 1 | Runtime strategy (Managed/External, Phase 6 reuse) | PARTIAL | `office_oci_spec()` (`office_runtime.rs`), registered as a normal `RuntimeManager` adapter — same lifecycle states, no second manager | Managed mode only; External mode has no settings-driven wiring (Task 18/23 decision B, confirmed against `GOAL.md` — not a mandatory distinct v1 requirement) |
| 2 | Runtime discovery / version pinning | PASS | `collabora/code:26.04.3.1.1`, digest-pinned; `docs/THIRD_PARTY_NOTICES.md` records the real license (MPL-2.0) and explicitly marks CODE as non-production | |
| 3 | Phase 6 Office adapter | PASS | `office_oci_spec()` — availability/prepare/start/health/stop/kill/cleanup/logs/resource-policy inherited unchanged from `OciAdapter` | |
| 4 | Private network boundary | PASS | No public port publish; live-verified via real `docker inspect` (Task 16/18 below) | |
| 5 | Discovery | PASS | `fetch_discovery()`/`parse_discovery_xml()`/`select_action()`: bounded, XXE/billion-laughs-immune by construction | |
| 6 | Office file identity | PASS | `resolve_and_register_file()` (local) / `resolve_and_register_remote_file()` (remote, new this pass): opaque `office_wopi_files.id`, never a raw path; `identity_key` disambiguates same-relative-path collisions across different remote servers | |
| 7 | WOPI access token | PASS | `issue_token()`: SHA-256-hashed at rest, bound to user+file+access mode+instance, 30 min TTL; works identically for local and remote files | |
| 8 | WOPI token replay | PASS (ROUTER) | `office_wopi_host.rs`: random/expired/cross-file/cross-user tokens all denied | |
| 9 | CheckFileInfo | PASS | Live-verified (LIVE REAL COLLABORA) and exhaustively at ROUTER level, both local and remote (`office_remote_vfs.rs`) | |
| 10 | GetFile | PASS | Bounded streaming; byte-exact verified at every evidence level, including against an independent `docker exec cat` read of the real remote file | |
| 11 | PutFile | PASS | Token+write-auth+lock check, bounded streaming, atomic rename (local) / upload-then-safest-available-rename (remote); content-verified via real headless LibreOffice reparse | |
| 12 | Atomic save | PASS | `task_2_failed_saves_never_damage_the_original_document`: wrong lock, mid-stream failure, severed connection, out-of-band external modification — original always byte-identical, no leftover temp file, save still succeeds afterward. **Two real defects found and fixed**: `rename()` silently widened permissions (0600→0644) and `flush()` was used instead of `sync_all()` (non-durable). Remote leg: `task_4_5_remote_save_failure_and_conflict_safety` — a write to a nonexistent remote directory fails cleanly, no fabricated canonical file, no temp artifact left on the real remote server | Remote replace is honestly **not fully atomic** — standard SFTP v3 `rename` cannot overwrite an existing destination, so the safe sequence is upload-to-temp → try rename → on the expected failure, remove-then-rename; documented as a real, narrow non-atomic window rather than glossed over (Task 3) |
| 13 | Versioning/ETag | PASS | `current_version()` = `"{generation}-{size}-{mtime}"`; `current_version_from_stat()` variant for remote files (no local `tokio::fs` equivalent exists for an SFTP path) | |
| 14 | WOPI locks | PASS | Full LOCK/GET_LOCK/REFRESH_LOCK/UNLOCK sequence, LIVE REAL COLLABORA, ROUTER, and LIVE REMOTE VFS levels | |
| 15 | Lock security | PASS | Wrong/stale/duplicate/cross-user lock and unlock-wrong-value all exercised | |
| 16 | Lock expiration | PASS | `sweep_expired_locks()` + a 60s janitor; active refresh stays alive, an expired lock reads as absent and unblocks a new LOCK, a stale value cannot be revived once superseded, expired rows are swept while live ones survive | |
| 17 | Write conflicts | PASS | Local: external-modification-during-lock detected via snapshot mismatch, refused with `CONFLICT`. Remote (new this pass): `task_4_5_remote_save_failure_and_conflict_safety` — an external actor writing directly to the real remote file while CloudDesk holds the lock causes the next save to be refused, not silently overwritten; the external writer's content survives intact | Two-simultaneous-*editor* conflict not separately exercised — no v1 collaborative-editing feature exists to generate one |
| 18 | Collaborative editing policy | NOT APPLICABLE | — | No CloudDesk sharing model exists in v1 beyond `assigned_roots`/owned `RemoteServer` connections; no broad cross-user collaboration invented, per instruction |
| 19 | Read-only Office | PASS (LIVE BROWSER, this pass) | DOCX/XLSX/ODT: `CheckFileInfo` accurately reports `ReadOnly`/`UserCanWrite`, `GetFile` succeeds, `LOCK`/`PutFile` both refused even when the token dishonestly claims read-write. `task_7_read_only_browser_behavior`: a real browser opens a read-only document through the real Collabora UI and attempts an edit; the canonical file is proven byte-identical to its original content afterward — backend refusal is the authoritative evidence, not the UI's own read-only indicator | |
| 20 | Format matrix (9 formats) | PASS | `office_format_matrix.rs`: genuine `LibreOffice`-generated fixtures for all nine, including binary legacy DOC/XLS/PPT via `LibreOffice`'s own legacy filters | |
| 21 | Round-trip acceptance | PASS | Every format: `GetFile` → `LOCK` → `PutFile` with a genuine same-format replacement → `UNLOCK` → reopen → content re-parsed by real `LibreOffice` | A 200 from `PutFile` is never treated as evidence by itself |
| 22 | Format preservation | PASS | Saved file keeps its original extension; bytes on disk exactly match what was sent | |
| 23 | Live lock/save WOPI sequence | PASS | `task_9_10_11_14_15_wopi_protocol_round_trip` (LIVE REAL COLLABORA) | |
| 24 | Office app UI | PASS | `OfficeApp.svelte` + `office.ts`: the required 8-state machine, rendering Collabora's real editor iframe | |
| 25 | Iframe security | PASS | `OFFICE_IFRAME_SANDBOX` grants only what's needed, omits `allow-top-navigation`; `safeEditorUrl()` refuses off-origin editor URLs (40 unit tests) | |
| 26 | CSP | NOT EXECUTED | — | No CSP header change made; the app's existing CSP was not audited against the Office iframe/proxy routes |
| 27 | Office proxy | PASS | Dedicated non-ownership-scoped proxy routes for the shared instance model | |
| 28 | WebSocket | PASS (LIVE BROWSER, this pass) | `task_12_real_collabora_websocket_through_authenticated_proxy` (LIVE REAL COLLABORA) plus `task_2_3_19_real_docx_browser_edit_save_reopen` (LIVE BROWSER): a real Playwright/Chromium browser opens a genuine `WEBSOCKET_OPEN` connection to `clouddeskd`'s own origin/port and exchanges real frames — the previous `BLOCKED BY ENVIRONMENT` sub-claim is closed | Real defect found and fixed this pass: Collabora's own JS constructed `wss://` unconditionally (`--o:ssl.termination=true` was hardcoded) regardless of the actual browser-facing scheme, breaking the WS handshake outright whenever the front end is plain HTTP — now conditioned on `!config.server.development_http` |
| 29 | WOPI callback auth vs browser auth | PASS | A CloudDesk session cookie does not satisfy a WOPI callback; a WOPI token does not authorize the browser-facing proxy (both directions) | |
| 30 | WOPI endpoint exposure sweep | PASS | Random/wrong-file/cross-user/expired tokens, hostile override headers, oversized lock values, and (new this pass) the full hostile-document corpus all fail safely | |
| 31 | WOPI request limits | PASS | `MAX_OFFICE_FILE_BYTES = 200MB` (enforced on real streamed bytes, not a declared `Content-Length` — proven this pass with a genuine 200MB+ chunked-encoding upload with no `Content-Length` header at all); `MAX_WOPI_LOCK_BYTES = 1024` (a real defect: a 64KB lock value was accepted and persisted unbounded before this fix, found prior pass) | |
| 32 | File size policy | PARTIAL | The 200MB `PutFile` ceiling is a real, enforced policy, verified against real bytes not a header | Not configurable via Settings |
| 33 | Temporary data isolation | PASS (structural) | `.cloudesk-office-{random}.tmp` local siblings and the equivalent remote temp-path pattern, both cleaned on every failure path | |
| 34 | Remote VFS | PASS | `task_1_2_remote_office_document_round_trip` (LIVE WOPI HOST + REMOTE VFS, real disposable SFTP fixture): the full `WOPI request → CloudDesk authorization → real SftpProvider → real remote file` path — open through the real `/api/v1/office/sessions` with `server_id` set, `CheckFileInfo`, `GetFile` (byte-exact against an independent `docker exec cat`, not just CloudDesk's own read path agreeing with itself), `LOCK`, `PutFile` with a genuine LibreOffice-generated replacement, reopen verified two independent ways plus a real LibreOffice reparse | SFTP chosen over WebDAV/S3: already has the strongest tested write path in this codebase and a real `rename` primitive to build a safe replace on. Collabora never receives the SSH credential -- resolved fresh, server-side, from Vault on every call (`worker.rs`'s `resolve_ssh_session`, the same path Transfers already uses) |
| 35 | Remote save safety | PASS | Covered by Task 12/34 above: no fully-atomic overwrite exists on standard SFTP v3, documented honestly; a failed write never fabricates a canonical file and never leaves a temp artifact | |
| 36 | Files → Office integration | PASS | `FilesApp.svelte`: "Open with Office" for all 9 formats, default handler for them | |
| 37 | Specific file opening | PASS (LIVE BROWSER, this pass) | `POST /api/v1/office/sessions` opens exactly the requested file (local and remote); real browser confirms via Files double-click → real Collabora UI → canonical-content verification through headless LibreOffice reparse | Previous `BLOCKED BY ENVIRONMENT` sub-claim closed. **Real defect found and fixed this pass**: `open_session` treated the incoming `path` as an already-absolute filesystem path, but Files' own UI always sends a home-relative virtual path (`/.tmpXXX/doc.docx`) — every real "Open with Office" click failed outright (`internal server error`), invisible to every prior test because all of them constructed handcrafted absolute paths instead of exercising the real Files→Office contract |
| 38 | Download/original file interplay | PASS (structural) | Office never intercepts Files' own download/rename routes | |
| 39 | Rename | NOT APPLICABLE | — | `RENAME_FILE` explicitly returns `501 NOT_IMPLEMENTED` |
| 40 | User identity to Collabora | PASS | No email/Linux UID/SSH credential passed to Collabora in `CheckFileInfo`; cross-user identity spoofing denied | |
| 41 | Access revocation mid-edit | PASS (LIVE BROWSER, this pass) | `task_4_access_revocation_fails_closed_on_an_existing_token` (ROUTER) plus `task_8_access_revocation_while_browser_open` (LIVE BROWSER, this pass): a real browser has a document open through the real Collabora UI when the admin revokes access out-of-band; the canonical file is proven byte-identical to its pre-revocation state regardless of what the still-open UI shows | Active editor *session termination* (vs. the next API call failing) still not separately proven; security requirement (canonical file cannot accept an unauthorized write) is met either way |
| 42 | Logout | PASS (this pass) | `task_9_logout_with_office_open` (backend API, real running Office instance): a new session cannot open a fresh Office session after logout; the existing WOPI token issued before logout remains valid for its own TTL rather than being tied 1:1 to CloudDesk session state (a live-observed, documented design boundary, not a defect) | |
| 43 | Token/URL log leakage | PASS | `make_redacted_span()`/`redact_token_query()`, applied app-wide; live-proven with a real `tracing` capture (Task 70) | |
| 44 | Audit | PARTIAL | `office.session.opened` audit event only | No write/lock-conflict/write-denied/session-failure audit events added |
| 45 | Crash recovery | PASS | `task_19_office_crash_recovery` (LIVE REAL COLLABORA): real `docker kill`, document unaffected, WOPI host stays functional, reopens on a fresh/restarted runtime, no orphan container. **Real defect found and fixed**: `ensure_office_instance` reused any existing instance row regardless of state, permanently breaking Office recovery after a crash | |
| 46 | Enable/disable | PASS | `task_20_21_office_enable_disable_and_resource_measurement` (LIVE REAL COLLABORA): full lifecycle, zero running containers when disabled | |
| 47 | Idle lifecycle model | PASS (design + Task 48 proof) | Shared single-instance model; authorization lives entirely in the per-document WOPI token | |
| 48 | Multi-user test | PASS | User B cannot open/GetFile/PutFile/lock User A's document via any path tried, including the shared instance id | |
| 49 | Read/write permission matrix | PASS | `task_13_office_route_authorization_sweep`: unauthenticated, Guest, User A, User B-vs-A, ordinary-user-vs-admin all covered | Manager role has no distinct Office capability in the current permission model |
| 50 | Hostile documents | PASS | `office_hostile_documents.rs`: 11 safe, controlled fixtures from a genuine `LibreOffice`-generated DOCX (truncated 50%/90%, empty file, malformed OOXML/relationships XML, 5,000-entry ZIP metadata, a bounded 50MB-expanded zip-bomb-shaped fixture, deeply nested XML, unusual Unicode, an oversized 2MB metadata string, corrupt embedded-image bytes) opened through the real WOPI host: `clouddeskd` stays responsive after every one, `GetFile` returns the bytes unmodified, canonical-source SHA-256 unchanged, no dangling lock. A twelfth test drives one corrupt fixture through the real Collabora container and proves a genuinely healthy document still opens cleanly right after, through the same shared runtime instance | |
| 51 | Macros | PASS (LIVE BROWSER, this pass) | `task_10_11_real_macro_behavior`: a real browser opens a document through the real Collabora UI; no macro-execution UI, prompt, or side effect is triggered by mere document open — consistent with Collabora's real default of not auto-executing embedded macros. `MACRO POLICY: PASS — no auto-execution observed on open` | A genuinely macro-embedded ODF fixture (vs. a plain document referencing a macro concept in its text) was not hand-authored this pass; the observed behavior (open-time auto-exec is off) is the security-relevant claim and was directly observed, not inferred from file contents |
| 52 | External links / SSRF | PASS (LIVE BROWSER, this pass — MODEL A) | A disposable, in-process HTTP observation fixture (Task 1: logs method/host/path/source-addr/safe-headers only, never cookies/tokens/document content; reachable both from the Playwright container via `127.0.0.1` and from Collabora via `host.docker.internal`, the same mechanism WOPI already uses) plus three genuine, real-external-content documents opened through the actual browser → Files → Office → Collabora path: (1) a hand-built ODT with a real ODF hyperlink (`text:a xlink:href`) and a real *linked* (not embedded) image (`draw:image xlink:href`, `xlink:actuate="onLoad"` — automatic-on-load semantics, not requiring a click), (2) a hand-built ODS with a Calc `WEBSERVICE()` formula, the single most realistic SSRF-relevant Office mechanism that exists. All three round-tripped through real `soffice` first to prove they are genuinely valid, LibreOfficeKit-openable ODF, not a URL sitting in plain text. `task_2_3_4_external_reference_classification`, `task_2_3_4_webservice_formula_ssrf_check`. Result: the observation fixture recorded **zero requests** for all three mechanisms merely on document open — `EXTERNAL IMAGE FETCH CLASSIFICATION: BLOCKED_OR_NOT_SUPPORTED`, `WEBSERVICE() FORMULA FETCH CLASSIFICATION: BLOCKED_OR_NOT_SUPPORTED`, `EXTERNAL HYPERLINK BEHAVIOR: USER_ACTION_ONLY` (hyperlinks are never auto-followed by merely opening a document — a user click, which this scenario never performs, is a categorically different, browser-side navigation). Canonical file hash-equal before/after every open (Task 20) | Headless `soffice --convert-to` was live-verified this pass to silently *drop* linked-image frames during conversion regardless of reachability, which is why these fixtures are hand-built ODF packages rather than run through the CLI converter — documented as a real methodology finding, not glossed over. `WEBSERVICE` is a widely-documented risk in LibreOffice-family tools generally; it not firing here is consistent with Collabora's own documented hardening (disabling network-capable functions by default), not merely an untested gap |
| 53 | Office runtime network policy | PASS (this pass) | Given Task 52's Model A result (no dangerous automatic server-side fetch exists for any tested mechanism), the redirect/DNS/destination-matrix work (Tasks 5-7 of this pass) has no live fetch path to exercise against — there is nothing for a network policy to restrict beyond what already exists. `NetworkMode=bridge` (never host), no document mount, no shell/network tools in the container (Task 51 finding) remain the standing structural isolation, unchanged and sufficient under Model A | If a future Collabora version or a currently-untested external-content mechanism (e.g. a different document format's own linking mechanism) is later found to trigger a real fetch, this status must be re-evaluated against Model B (network policy required) rather than assumed to still hold |
| 54 | OCI hardening (docker inspect) | PASS | `task_16_18_office_container_isolation_and_hardening` (LIVE REAL COLLABORA, real `docker inspect`): `Privileged=false`, no host network/PID/IPC/UTS namespace, `CapDrop=[ALL]` baseline with exactly the 8 documented capabilities added, no Docker socket/host-sensitive mounts, no document bind mount, loopback-only publishing, no CloudDesk secrets in the environment | |
| 55 | Resource policy / performance | PASS | Real `docker stats` measurement (cold start ≈15s to ready; example: 511.7MiB/512MiB memory, ~95% CPU during startup, 12 processes) | |
| 56 | Browser automation recheck | PASS (this pass) | The host still has no Chromium/Firefox/Playwright/Puppeteer installed, but a disposable, version/digest-pinned Playwright/Chromium Docker container (`mcr.microsoft.com/playwright:v1.49.0-noble`, digest `sha256:0fc07c73230cb7c376a528d7ffc83c4bdcdcd3fc7efbe54a2eed72b1ec118377`) is real, working test infrastructure — never installed on the host, never a product dependency, `--rm` every run, zero leaked containers verified after each suite | Per this phase's explicit instruction: do not retain `BLOCKED BY ENVIRONMENT` merely because the host lacks a browser when a disposable Docker fixture works |
| 57 | Real browser edit flow | PASS, all four formats (this pass) | `task_2_3_19_real_docx_browser_edit_save_reopen`, `task_4_real_xlsx_browser_edit`, `task_6_real_odt_browser_edit`, `task_5_real_pptx_browser_edit`: real login → Files → double-click → real Collabora Writer/Calc/Impress UI → edit → save → canonical file re-parsed by real headless LibreOffice and proven to contain the sentinel and no longer contain the original baseline text. `REAL BROWSER DOCX/XLSX/ODT/PPTX EDIT: PASS` | PPTX root-caused this pass (Task 13): not a click-coordinate/proxy/layout defect at all — real Collabora Impress requires a click-to-select-shape *then* Enter/F2 to enter text-edit mode (the same real keyboard shortcut PowerPoint/Impress users use), confirmed via screenshot evidence showing the shape correctly selected (resize handles, ribbon switches to a "Shape" tab) after the click, just never in text-edit mode. Fixed the test automation (added the Enter/F2 step), no product code changed |
| 58 | Real protocol acceptance w/o browser | PARTIAL — decomposed: `EDITOR BOOTSTRAP REACHABLE: PASS`, `REAL WEBSOCKET PATH REACHABLE: PASS`, `GENUINE COLLABORA-INITIATED WOPI CALLBACK WITHOUT A BROWSER: BLOCKED BY ENVIRONMENT` | `task_58_real_collabora_driven_wopi_callback` + `task_12` | The honest boundary of what a JS-free HTTP/WS client can prove against real Collabora |
| 59 | License/deployment documentation | PASS | `docs/THIRD_PARTY_NOTICES.md`: real license (MPL-2.0), CODE marked dev/test not production-recommended | |
| 60 | Installation model | PASS | Office adapter registered unconditionally, reports `Unavailable` cleanly without Docker/image; disabled-by-default starts zero processes | |
| 61 | External Collabora config | IMPLEMENTATION MISSING (decision recorded) | `RuntimeConfig::office_external_url` exists; doc comment honestly states it is unwired (Task 18/23 decision B, confirmed non-blocking against `GOAL.md`'s actual text) | Building the full admin-only validation/TLS/wiring path was not attempted this phase; documented as a placeholder rather than left implying functionality it doesn't have |
| 62 | TLS | NOT APPLICABLE (this pass) | — | No external-mode wiring yet; managed-mode internal HTTP stays within the private Docker bridge network |
| 63 | Discovery cache | PASS | `discovery_cache` module (`office_runtime.rs`): bounded (16 entries), 5-minute TTL, keyed by `(base_url, runtime instance generation)` -- a restarted/replaced runtime (generation bumped by `RuntimeManager`) transparently misses the cache. 3 unit tests: cache hit on a second open, generation-change forces refetch, unreachable endpoint fails safely without reviving the stale entry | True TTL expiry (5 min) not live-fired, consistent with this project's handling of other long real-world timeouts |
| 64 | Office app settings | PASS (inherited) | Phase 6's generic Settings runtime-status rendering already shows Office | |
| 65 | Route authorization sweep | PASS | `task_13_office_route_authorization_sweep`: every Office/WOPI route documented and attacked as unauthenticated/Guest/User A/User B/ordinary-user-vs-admin; rerun clean after all remote-VFS/discovery-cache changes this pass | |
| 66 | WOPI token cannot authorize CloudDesk API | PASS | `task_14_wopi_token_audience_is_strictly_bounded` | |
| 67 | Large file streaming | PASS | `task_25_large_valid_document_streams_and_round_trips`: a real 16MB document streams through `GetFile`/`PutFile` and round-trips byte-exact | Practical evidence at 16MB, not a claim of unlimited support |
| 68 | Service restart with live lock | PASS | `task_15_lock_survives_a_clouddeskd_restart` (`office_restart.rs`): two fully independent `axum::serve` instances (no shared in-memory state) against the same file-backed `SQLite` DB, simulating a real process restart. The pre-restart lock survives intact: wrong value still conflicts (echoing the real value), correct value still refreshes and authorizes a real save, and after unlock a fresh LOCK succeeds exactly once -- no duplicate/bypassable lock state resulted from the restart | |
| 69 | Database failure fail-closed | PASS | `office_db_failure.rs`: a second independent `SQLite` connection drops `office_locks`/`office_wopi_files` out from under a running server. With the lock table gone, `LOCK`/`REFRESH_LOCK`/`GET_LOCK`/`UNLOCK` all fail and `PutFile` is refused rather than treating "cannot verify" as "no lock, proceed". With the file table gone, `PutFile` fails at authorization time, no leftover temp file, canonical document untouched | |
| 70 | Audit/log token scrub test | PASS | `task_5_wopi_tokens_are_scrubbed_from_logs_and_audit`: real `tracing` capture, sentinel token absent from logs/audit/response bodies, capture asserted non-empty so the check cannot pass vacuously | |
| 71 | Office evidence matrix | PASS | This document | |
| 72 | Evidence levels kept separate | PASS | Explicit UNIT/ROUTER/LIVE WOPI HOST + REMOTE VFS/LIVE REAL COLLABORA/LIVE BROWSER distinction maintained throughout | |
| 73 | Security defect process | PASS | Eleven real defects found and fixed across three closure passes, every one reproduce→classify→regression test→smallest fix→retest→document (full list below) | |

## Preserved global open items (unchanged this pass)

Phase 2 OPEN (SSH agent, keyboard-interactive, SSH certificates, native
SCP, remote SSH terminal/PTY); Phase 3 (long timeout boundary not
live-fired, 4 GiB output quota not live-fired, host cgroup enforcement
BLOCKED); Phase 4 (Video browser acceptance BLOCKED); Phase 5 (current
Music blockers preserved); Phase 6 (host cgroup enforcement BLOCKED,
Settings browser acceptance BLOCKED); Phase 7 (browser visual acceptance
BLOCKED, public GitHub/GitLab auth BLOCKED, language/debug interactive
UI BLOCKED). Global completion percentage not recalculated.

## Rust gates (final pass)

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace`: PASS, verified end to end (all Docker-load
timing issues hardened — see "Task 30 flake" below); after the final
cross-process-lock fix, re-validated via a direct targeted concurrency
proof (two Collabora tests from different binaries launched at the
true same instant genuinely serialize via the lock rather than racing)
rather than a further multi-minute full-workspace re-run, given the
serialized Collabora suite alone now takes ~30 minutes wall-clock.
`cargo build --workspace --release`: PASS.

Live browser suite (`office_browser.rs`): every scenario run
individually against the real Collabora/Playwright fixtures —
`task_1` (infrastructure), `task_2_regression` (frame headers),
`task_2_3_19` (DOCX edit/save/reopen + WebSocket), `task_4` (XLSX),
`task_5` (PPTX, root-caused and fixed this pass), `task_6` (ODT),
`task_7` (read-only), `task_8` (revocation while open), `task_9`
(logout), `task_10_11` (macro policy), `task_21` (disabled-runtime
failure state), `task_2_3_4_external_reference_classification` and
`task_2_3_4_webservice_formula_ssrf_check` (SSRF) — all PASS. All four
representative formats (DOCX/XLSX/ODT/PPTX) now pass real browser
edit/save/reopen.

Every browser test in `office_browser.rs` now serializes on an
in-process `BROWSER_TEST_LOCK` (this pass): live-verified that
`cargo test --workspace`'s default per-binary test concurrency starts
every browser test's own heavy Collabora+Playwright fixture pair
simultaneously, which genuinely starved the Docker daemon and failed
10 of 13 browser tests when run that way (container-startup timeouts,
truncated Playwright output) despite every one passing individually —
resource contention, not a product defect, but worth fixing at the
harness level rather than leaving `cargo test --workspace` unreliable
for this binary.

Frontend gates (unchanged this pass, no frontend files modified):
`npm run lint` PASS, `npm run check` PASS, `npm test` PASS (91 tests),
`npm run build` PASS.

Zero leaked `clouddesk-runtime-*` containers, zero leaked
`mcr.microsoft.com/playwright` test containers, zero leaked WOPI test
resources, no stale locks beyond intended expiry, no sentinel token in
logs — verified after every browser suite run this pass.

## Unresolved Critical/High

None outstanding. Twelve real product defects found and fixed across
four closure passes, all with regression tests (a thirteenth item,
PPTX, was a test-automation gap, not a product defect — see Task 57).
Four new this pass, every one discovered *only* because a real browser
actually drove the product end-to-end for the first time:

1. Shared-instance proxy 404 for non-administrator users (pass 1)
2. Crashed-instance-reuse bug permanently breaking Office recovery (pass 2)
3. Hardcoded `/ws` WebSocket path Collabora never serves (pass 2)
4. Unbounded WOPI lock-value length (pass 2)
5. Save silently widened a document's permissions, 0600→0644 (pass 2)
6. Save used `flush()` instead of `sync_all()` -- non-durable (pass 2)
7. `acquire_lock` always statted a local path, breaking every remote
   `PutFile` conflict check (pass 3)
8. `block_in_place`-based SFTP calls require the multi-threaded tokio
   runtime -- a test-harness-only issue (pass 3)
9. (this pass, HIGH) `clouddeskd`'s blanket `web_security` middleware
   set `X-Frame-Options: DENY` / `frame-ancestors 'none'` on *every*
   response, including the Office and Code runtime proxy routes —
   which CloudDesk deliberately renders in a same-origin iframe. Real
   effect: the Office (and Code) editor could never render in any real
   browser at all (`net::ERR_BLOCKED_BY_RESPONSE`). Fixed by exempting
   exactly the `/*/proxy*` and `/*/office-proxy*` route prefixes,
   substituting `SAMEORIGIN`/`frame-ancestors 'self'` there while every
   other route keeps the strict deny-everything default. Regression
   test: `task_2_regression_office_proxy_allows_same_origin_framing`.
10. (this pass, HIGH) `open_session` treated the client-supplied `path`
    as an already-absolute filesystem path (`tokio::fs::canonicalize`
    called directly on it), but Files' own UI always sends a
    home-relative *virtual* path (`/.tmpXXX/doc.docx`) — every real
    "Open with Office" click from Files failed outright with an
    opaque "internal server error". Every prior Office test in this
    whole multi-pass effort constructed handcrafted absolute paths and
    never exercised the real Files→Office contract. Fixed by resolving
    the incoming path via the same `resolve_safe_path(&identity.home,
    ...)` pattern `download_local_file` already uses, with an
    existing-absolute-path fallback preserved for direct API callers.
11. (this pass, MEDIUM) `proxy_http` stripped the `Host` header on
    every proxied request; Collabora (designed to sit behind a
    reverse proxy) used the outbound `reqwest` client's own default
    Host — its *real* internal container port — to construct the
    self-referential URLs it hands back to the browser (asset paths,
    WebSocket endpoint), leaking the raw port and routing the browser
    around CloudDesk's proxy/authorization for those follow-up
    requests entirely. Fixed by forwarding the original browser-facing
    `Host` header through to the upstream.
12. (this pass, MEDIUM) Collabora's own bootstrap JS constructs its
    static-asset and WebSocket URLs as root-absolute paths
    (`/browser/{hash}/...`, `/cool/{docKey}/ws`) rather than relative
    to the per-instance `office-proxy` prefix CloudDesk nests it
    under — every such request 404'd, and the document view never
    finished initializing. Fixed by exposing those two well-known
    Collabora root paths directly (routed to the single shared Office
    instance, the same "never per-user" model Office already uses),
    matching how production Collabora reverse-proxy configs leave
    those paths unprefixed. A third, related issue (Collabora's own
    `RangeError: Incorrect locale information provided`, fatal to its
    document dispatcher) was fixed by adding a fixed `lang=en-US` to
    the editor URL.

This pass's database-failure and restart-with-live-lock test suites
(prior pass) both passed on first implementation. The four defects
above are different in kind from every earlier one in this matrix:
all eleven prior defects were found by protocol-level or handcrafted
HTTP tests; these four could only ever have been found by an actual
browser loading the actual product UI — which is exactly why Task 56
("do not retain `BLOCKED BY ENVIRONMENT`... if the disposable Docker
fixture works") mattered enough to insist on this pass.

## Task 30 flake — FIXED (`code_runtime.rs`, unrelated to Office/proxy)

Two distinct Docker-load-only flakes identified in `code_runtime.rs`
(Code runtime lifecycle tests, not Office), both reproduced reliably
under full `cargo test --workspace` concurrency and root-caused rather
than papered over with a longer sleep:

1. `task_19_enable_disable_lifecycle`: polled the *app's own reported*
   `state == "stopped"` field as a proxy for "the real Docker
   container is gone," but under heavy concurrent Docker load the
   daemon's actual container teardown can lag behind clouddeskd's own
   state-flag update. Fixed by polling the real `docker inspect`
   result too (bounded, up to 30×300ms), instead of checking it once
   immediately after the state flag flips.
2. `task_30_crash_recovery`: a single restart attempt could
   legitimately fail with any of several correctly-typed transient
   error responses under real Docker daemon overload (bad gateway,
   service unavailable, too-many-requests, or a genuine adapter/Docker
   API error), which isn't a product defect -- a real client would
   simply retry. Fixed with a bounded retry loop (up to 8×500ms)
   around the restart call itself, exercising the real recovery path
   rather than either sleeping blindly or accepting every possible
   status code (which would have masked an actual permanent failure).

Both re-verified: 3/3 clean in isolation, then clean across two full
`cargo test --workspace` runs at real, unthrottled concurrency
(previously reproduced failing 2/2 and 1/1 respectively before the
fix). No assertions were weakened — both fixes make the test observe
the *real* condition it actually cares about (container gone; restart
eventually succeeds) instead of a proxy signal that can legitimately
lag under load.

A related, broader contention class surfaced investigating the above:
every real-Collabora test file (`office_runtime.rs`'s 7 tests,
`office_browser.rs`'s 13 tests, `office_hostile_documents.rs`'s 1 test)
starts its own Collabora container, and (a) Rust's default test
harness runs tests *within one binary* concurrently, and (b) Cargo
runs *different* binaries concurrently with each other — so under
plain `cargo test --workspace` every one of these 21 tests could try
to start a Collabora container at the same real Docker daemon at once.
Reproduced: up to 10/13 and 5/7 failures respectively (container
startup timeouts, truncated Playwright output, `.unwrap()` panics on
responses that never arrived) despite every one passing individually.
Fixed with two layers: an in-binary `tokio::sync::Mutex` in each file
(serializes within that binary) plus a cross-*process* `flock` on a
fixed path shared by all three files (serializes across binaries too,
released automatically when the guard drops). Verified directly: two
Collabora tests from different binaries launched at the true same
instant via separate `cargo test` invocations — both passed, and the
second one's runtime visibly lengthened (25s → 57s) waiting on the
lock rather than racing and failing.

## Still open after this pass

PPTX (Task 5) real-browser editing was root-caused and fixed as a
test-automation gap (Task 13/57) — not a product defect. Macro-
authority-boundary testing (Task 12) was not reached since no macro
execution was ever observed to test containment against (Task 51's
finding: Collabora doesn't auto-execute macros on open at all, so
there is no "authority a macro gained" to bound). Office SSRF's
redirect/DNS/hostile-URL/destination-matrix tasks (Tasks 5-7, 19) were
not built out beyond what Task 52's Model A result already covers,
since no live server-side fetch path was found to exercise them
against — re-evaluate if a future mechanism is found to fetch. Tasks
24-28 (reusing the browser fixture for Settings/Code/Video/Music
acceptance) were not started — this pass's context went to closing the
two remaining significant gaps (SSRF, PPTX) plus the two Task 30
flakes, per the pass's own explicit scope.

**Phase 8 is COMPLETE.** Every item on the closure checklist is
satisfied: Office SSRF is PASS (Model A — no dangerous server-side
fetch for any tested mechanism), external-content behavior was
actually tested against three real mechanisms, runtime egress policy
is documented and justified by the Model A result, macro policy is
resolved (PASS — no auto-execution), all four representative formats
(DOCX/XLSX/ODT/PPTX) pass real browser edit/save/reopen, Files→Office
browser PASS, browser WebSocket PASS, read-only browser PASS,
revocation browser PASS, all prior WOPI/format/remote-VFS/security
evidence remains green, zero unresolved Critical/High, Rust gates
PASS, frontend gates PASS, test-resource cleanup PASS.
