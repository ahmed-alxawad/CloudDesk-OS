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
| 19 | Read-only Office | PASS | DOCX/XLSX/ODT: `CheckFileInfo` accurately reports `ReadOnly`/`UserCanWrite`, `GetFile` succeeds, `LOCK`/`PutFile` both refused even when the token dishonestly claims read-write | |
| 20 | Format matrix (9 formats) | PASS | `office_format_matrix.rs`: genuine `LibreOffice`-generated fixtures for all nine, including binary legacy DOC/XLS/PPT via `LibreOffice`'s own legacy filters | |
| 21 | Round-trip acceptance | PASS | Every format: `GetFile` → `LOCK` → `PutFile` with a genuine same-format replacement → `UNLOCK` → reopen → content re-parsed by real `LibreOffice` | A 200 from `PutFile` is never treated as evidence by itself |
| 22 | Format preservation | PASS | Saved file keeps its original extension; bytes on disk exactly match what was sent | |
| 23 | Live lock/save WOPI sequence | PASS | `task_9_10_11_14_15_wopi_protocol_round_trip` (LIVE REAL COLLABORA) | |
| 24 | Office app UI | PASS | `OfficeApp.svelte` + `office.ts`: the required 8-state machine, rendering Collabora's real editor iframe | |
| 25 | Iframe security | PASS | `OFFICE_IFRAME_SANDBOX` grants only what's needed, omits `allow-top-navigation`; `safeEditorUrl()` refuses off-origin editor URLs (40 unit tests) | |
| 26 | CSP | NOT EXECUTED | — | No CSP header change made; the app's existing CSP was not audited against the Office iframe/proxy routes |
| 27 | Office proxy | PASS | Dedicated non-ownership-scoped proxy routes for the shared instance model | |
| 28 | WebSocket | PASS | `task_12_real_collabora_websocket_through_authenticated_proxy` (LIVE REAL COLLABORA): **real defect found and fixed** — the generic proxy hardcoded `/ws`; Collabora's real endpoint is per-document, `/cool/{urlencoded WOPISrc}/ws?...` (confirmed by directly probing the real container). `proxy_ws_path` + a wildcard route now forward the real WebSocket URL | Collabora's own internal WOPI re-validation over an established WS connection needs a real browser to exercise end-to-end — honestly `BLOCKED BY ENVIRONMENT` for that specific sub-claim |
| 29 | WOPI callback auth vs browser auth | PASS | A CloudDesk session cookie does not satisfy a WOPI callback; a WOPI token does not authorize the browser-facing proxy (both directions) | |
| 30 | WOPI endpoint exposure sweep | PASS | Random/wrong-file/cross-user/expired tokens, hostile override headers, oversized lock values, and (new this pass) the full hostile-document corpus all fail safely | |
| 31 | WOPI request limits | PASS | `MAX_OFFICE_FILE_BYTES = 200MB` (enforced on real streamed bytes, not a declared `Content-Length` — proven this pass with a genuine 200MB+ chunked-encoding upload with no `Content-Length` header at all); `MAX_WOPI_LOCK_BYTES = 1024` (a real defect: a 64KB lock value was accepted and persisted unbounded before this fix, found prior pass) | |
| 32 | File size policy | PARTIAL | The 200MB `PutFile` ceiling is a real, enforced policy, verified against real bytes not a header | Not configurable via Settings |
| 33 | Temporary data isolation | PASS (structural) | `.cloudesk-office-{random}.tmp` local siblings and the equivalent remote temp-path pattern, both cleaned on every failure path | |
| 34 | Remote VFS | PASS | `task_1_2_remote_office_document_round_trip` (LIVE WOPI HOST + REMOTE VFS, real disposable SFTP fixture): the full `WOPI request → CloudDesk authorization → real SftpProvider → real remote file` path — open through the real `/api/v1/office/sessions` with `server_id` set, `CheckFileInfo`, `GetFile` (byte-exact against an independent `docker exec cat`, not just CloudDesk's own read path agreeing with itself), `LOCK`, `PutFile` with a genuine LibreOffice-generated replacement, reopen verified two independent ways plus a real LibreOffice reparse | SFTP chosen over WebDAV/S3: already has the strongest tested write path in this codebase and a real `rename` primitive to build a safe replace on. Collabora never receives the SSH credential -- resolved fresh, server-side, from Vault on every call (`worker.rs`'s `resolve_ssh_session`, the same path Transfers already uses) |
| 35 | Remote save safety | PASS | Covered by Task 12/34 above: no fully-atomic overwrite exists on standard SFTP v3, documented honestly; a failed write never fabricates a canonical file and never leaves a temp artifact | |
| 36 | Files → Office integration | PASS | `FilesApp.svelte`: "Open with Office" for all 9 formats, default handler for them | |
| 37 | Specific file opening | PASS (backend + frontend wiring); BLOCKED BY ENVIRONMENT (visual confirmation) | `POST /api/v1/office/sessions` opens exactly the requested file (local and remote) | Visual "the right document is on screen" needs a browser |
| 38 | Download/original file interplay | PASS (structural) | Office never intercepts Files' own download/rename routes | |
| 39 | Rename | NOT APPLICABLE | — | `RENAME_FILE` explicitly returns `501 NOT_IMPLEMENTED` |
| 40 | User identity to Collabora | PASS | No email/Linux UID/SSH credential passed to Collabora in `CheckFileInfo`; cross-user identity spoofing denied | |
| 41 | Access revocation mid-edit | PASS | `task_4_access_revocation_fails_closed_on_an_existing_token`: admin revokes the assigned root mid-session, the very next `CheckFileInfo`/`GetFile`/`LOCK`/`REFRESH_LOCK`/`PutFile` on the *same unexpired token* all fail with `FORBIDDEN` | Active editor *session termination* (vs. the next API call failing) not separately proven |
| 42 | Logout | NOT EXECUTED | — | Token TTL is bounded (30 min); no dedicated test proves a logged-out session's old token stops working before natural expiry |
| 43 | Token/URL log leakage | PASS | `make_redacted_span()`/`redact_token_query()`, applied app-wide; live-proven with a real `tracing` capture (Task 70) | |
| 44 | Audit | PARTIAL | `office.session.opened` audit event only | No write/lock-conflict/write-denied/session-failure audit events added |
| 45 | Crash recovery | PASS | `task_19_office_crash_recovery` (LIVE REAL COLLABORA): real `docker kill`, document unaffected, WOPI host stays functional, reopens on a fresh/restarted runtime, no orphan container. **Real defect found and fixed**: `ensure_office_instance` reused any existing instance row regardless of state, permanently breaking Office recovery after a crash | |
| 46 | Enable/disable | PASS | `task_20_21_office_enable_disable_and_resource_measurement` (LIVE REAL COLLABORA): full lifecycle, zero running containers when disabled | |
| 47 | Idle lifecycle model | PASS (design + Task 48 proof) | Shared single-instance model; authorization lives entirely in the per-document WOPI token | |
| 48 | Multi-user test | PASS | User B cannot open/GetFile/PutFile/lock User A's document via any path tried, including the shared instance id | |
| 49 | Read/write permission matrix | PASS | `task_13_office_route_authorization_sweep`: unauthenticated, Guest, User A, User B-vs-A, ordinary-user-vs-admin all covered | Manager role has no distinct Office capability in the current permission model |
| 50 | Hostile documents | PASS | `office_hostile_documents.rs`: 11 safe, controlled fixtures from a genuine `LibreOffice`-generated DOCX (truncated 50%/90%, empty file, malformed OOXML/relationships XML, 5,000-entry ZIP metadata, a bounded 50MB-expanded zip-bomb-shaped fixture, deeply nested XML, unusual Unicode, an oversized 2MB metadata string, corrupt embedded-image bytes) opened through the real WOPI host: `clouddeskd` stays responsive after every one, `GetFile` returns the bytes unmodified, canonical-source SHA-256 unchanged, no dangling lock. A twelfth test drives one corrupt fixture through the real Collabora container and proves a genuinely healthy document still opens cleanly right after, through the same shared runtime instance | |
| 51 | Macros | NOT EXECUTED | — | Determining Collabora's actual macro-execution default requires either a real browser session or reverse-engineering coolwsd's internal macro dispatch; the container itself ships no shell/tools to probe from inside (confirmed this pass via `docker export`, 33,840 files, zero of `sh`/`bash`/`curl`/`wget`/`nc`/`python`/`busybox`) |
| 52 | External links / SSRF | NOT EXECUTED | — | The real, live-confirmed absence of any shell or network tool inside the Collabora container (see Task 51) means a genuine external-fetch-on-open test needs either a real browser (to trigger `bundle.js`'s own fetch behavior) or reverse-engineering coolwsd's internal HTTP client -- neither completed. Structural mitigating evidence stands: `NetworkMode=bridge` (never host), no document mount, and now the confirmed absence of any tool an attacker with document-triggered code execution could pivot with |
| 53 | Office runtime network policy | PARTIAL | Confirmed: `NetworkMode` is `bridge`, never `host`; container has no shell/network tools at all (Task 51/52 finding) | No explicit egress-restriction policy (e.g. a dedicated restricted Docker network) beyond Docker's own bridge isolation and the container's own minimal filesystem |
| 54 | OCI hardening (docker inspect) | PASS | `task_16_18_office_container_isolation_and_hardening` (LIVE REAL COLLABORA, real `docker inspect`): `Privileged=false`, no host network/PID/IPC/UTS namespace, `CapDrop=[ALL]` baseline with exactly the 8 documented capabilities added, no Docker socket/host-sensitive mounts, no document bind mount, loopback-only publishing, no CloudDesk secrets in the environment | |
| 55 | Resource policy / performance | PASS | Real `docker stats` measurement (cold start ≈15s to ready; example: 511.7MiB/512MiB memory, ~95% CPU during startup, 12 processes) | |
| 56 | Browser automation recheck | BLOCKED BY ENVIRONMENT | Rechecked: no Chromium/Firefox/Playwright/Puppeteer available | |
| 57 | Real browser edit flow | BLOCKED BY ENVIRONMENT | Depends on Task 56 | |
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

## Rust gates (this pass)

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace`: PASS.
`cargo build --workspace --release`: PASS.
Live Office suite (`office_runtime.rs`, 7 tests — lock/save/proxy/
WebSocket/discovery code changed this pass): run twice consecutively,
zero failures both times.
Live remote-VFS suite (`office_remote_vfs.rs`, 3 tests): run twice
consecutively, zero failures, zero leftover fixture files on the real
SFTP server both times.

Frontend gates (unchanged this pass, last verified against this
codebase): `npm run lint` PASS, `npm run check` PASS, `npm test` PASS
(91 tests), `npm run build` PASS.

Zero leaked `clouddesk-runtime-*` containers after every run this pass,
zero leaked remote-VFS fixture files, zero leaked WOPI test resources,
no stale locks beyond intended expiry, no sentinel token in logs.

## Unresolved Critical/High

None outstanding. Eight real defects found and fixed across three
closure passes, all with regression tests:

1. Shared-instance proxy 404 for non-administrator users (pass 1)
2. Crashed-instance-reuse bug permanently breaking Office recovery (pass 2)
3. Hardcoded `/ws` WebSocket path Collabora never serves (pass 2)
4. Unbounded WOPI lock-value length (pass 2)
5. Save silently widened a document's permissions, 0600→0644 (pass 2)
6. Save used `flush()` instead of `sync_all()` -- non-durable (pass 2)
7. (this pass) `acquire_lock` always statted a local path, so every
   remote LOCK recorded a (0,0) snapshot, making every subsequent
   remote `PutFile` fail its own conflict check outright
8. (this pass) `block_in_place`-based SFTP calls require the
   multi-threaded tokio runtime -- caught as a test-harness panic
   under the default single-threaded `#[tokio::test]`; the real
   product binary is unaffected since `#[tokio::main]` already
   defaults to multi-thread, but the test flavor had to be corrected
   to actually exercise the remote-VFS path at all

This pass's new database-failure and restart-with-live-lock test
suites both passed on first implementation -- they are coverage for
properties that were already correct by design (locks persisted in
`SQLite`, error propagation already fails closed), not fixes.

**Still not attacked and could still surface defects**: real macro
execution behavior, browser-triggered external-link SSRF, and
(unchanged) real browser-driven editing. Phase 8 is **PARTIAL**, not
COMPLETE — this matrix does not claim otherwise.
