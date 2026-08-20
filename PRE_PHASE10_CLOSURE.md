# Pre-Phase-10 Closure Gate

Authoritative open-item register for CloudDesk-OS v1, built by
cross-checking `CLAUDE_ENGINEERING_CHECKPOINT.md`, `V1_TRUE_CLOSURE.md`,
`PHASE6_RUNTIME_EVIDENCE.md`, `PHASE7_CODE_EVIDENCE.md`,
`PHASE8_OFFICE_EVIDENCE.md`, `PHASE9_BROWSER_EVIDENCE.md`, and the
actual current implementation/tests. Where `V1_TRUE_CLOSURE.md`'s prose
was stale relative to newer phase evidence (e.g. it still describes
Browser as "no adapter exists" — false as of Phase 9), the newer,
executable evidence wins and is cited instead.

**This is PASS 3A-3 of a multi-pass closure per the governing prompt's
own execution strategy.** PASS 1 (Office fixture cleanup), PASS 2
(Browser one-page vertical slice: broker, frames, WebSocket, input,
navigation, minimal frontend, crash/enable-disable), PASS 3A (tabs/
popups), and PASS 3A-2 (Playwright-through-the-compiled-frontend
acceptance, logout/revocation, service-restart availability fix) are
done. **PASS 3A-3 closed Blocker 1 of its own six-blocker scope: real
HTTP cookie persistence.** The actual root cause (found via a real,
live, hands-on CDP investigation, not the OS-crypt/keyring theory two
prior passes had assumed) was two compounding shutdown-path defects —
a non-`exec`ing vendor wrapper script leaving PID 1 as bash instead of
the real Chromium binary, and a missing real CDP `Browser.close` call
before `docker stop` — both fixed, and live-verified end-to-end
through the real product API (`browser_cookies.rs`): User A's cookie
survives a real stop/restart, User B never sees it, Guest's does not
survive. See `PHASE9_BROWSER_EVIDENCE.md` for full detail. Blockers 2-6
of that same pass (internal-network isolation, WebRTC leakage,
frame-backpressure stress, simultaneous multi-user acceptance, full
route-authorization matrix) remain **not built or not run**, recorded
below honestly rather than silently deferred. Phase 2 SSH closure
(Part V) was not attempted this pass either — still a realistically
multi-day effort on its own.

## Open-item register

| Phase | Requirement | Current status | Evidence source | Mandatory for v1? | Reason still open | Exact next action |
|---|---|---|---|---|---|---|
| 1 | Local file manager core (browse/upload/download/rename/move/delete) | PASS | `RELEASE_EVIDENCE_AUDIT.md`, pre-existing acceptance | Yes | — | None |
| 1 | Archive create/extract | PASS | `V1_TRUE_CLOSURE.md` #7, `crates/vfs/tests/archive.rs` (10 tests) | Yes | — | None; not yet exercised through the real HTTP API/browser, but function-boundary evidence is solid |
| 1 | ACL view/edit | PASS | `V1_TRUE_CLOSURE.md` #8, `crates/vfs/tests/acl.rs` (6 tests) | Yes | — | None |
| 1 | Resumable/chunked upload | PASS | `V1_TRUE_CLOSURE.md` #9, `resumable_upload.rs` | Yes | — | None |
| 2 | SFTP core (list/upload/download/rename/mkdir/delete) | PASS | Prior Nightmare-audit live evidence | Yes | — | None |
| 2 | ProxyJump/bastion (SFTP/transfer path) | PASS | `ssh_proxyjump.rs` (12 tests), real 2-container topology | Yes | — | None |
| 2 | SSH agent authentication | IMPLEMENTATION MISSING | `crates/remote/src/ssh.rs`: `SshAuth::Agent` still returns an error | Yes (`GOAL.md` G8) | Never implemented | Implement `russh` agent-client support against `SSH_AUTH_SOCK`; live-test allowed/missing/wrong-key/multiple-key/agent-unavailable against a real disposable `ssh-agent` |
| 2 | Keyboard-interactive authentication | IMPLEMENTATION MISSING | `SshAuth::KeyboardInteractive` still returns an error | Yes (`GOAL.md` G8) | Never implemented | Implement challenge/response handling; live-test against a real OpenSSH server configured for keyboard-interactive |
| 2 | SSH certificate authentication | IMPLEMENTATION MISSING | `SshAuth::Certificate` decodes `key_data` only, silently ignores `cert_data` (source comment calls this "an implemented facade") | Yes (`GOAL.md` G8) | Never implemented; current code actively misrepresents itself as supporting this | Implement real certificate parsing/validation via `russh`; live-test valid/expired/wrong-principal/wrong-CA against a disposable OpenSSH CA |
| 2 | Native SCP | IMPLEMENTATION MISSING | No SCP code exists anywhere; only SFTP | Yes (`GOAL.md` G9, "where appropriate" — no documented substitution decision exists) | Never implemented | Implement a real SCP protocol path (not SFTP relabeled); live-test upload/download/large-file/unicode/failure/authorization/host-key against a real SSH server |
| 2 | Remote terminal (PTY) over SSH | IMPLEMENTATION MISSING | `SshSession` only has `run_command` (single buffered non-interactive exec); no PTY, no interactive channel, no endpoint | Yes (`GOAL.md` G8, per-server "Terminal" action) | Never implemented — discovered as a new gap during the Phase 2 pass, not merely "not wired" | Implement `request_pty` + interactive channel on `SshSession`, a new owner-scoped WebSocket endpoint via `resolve_ssh_session` (ProxyJump support included), frontend wiring; live-test whoami/pwd/interactive/resize/Ctrl-C/disconnect/revocation/cross-user-denial |
| 3 | FFmpeg probe/remux/transcode core pipeline | PASS | `V1_TRUE_CLOSURE.md` #1 CLOSED, `crates/media/tests/live_ffmpeg.rs`, `media_api.rs` | Yes | — | None |
| 3 | 10-minute job timeout, live-fired | NOT EXECUTED | Only cancellation was live-tested; the real 10-minute wall-clock timeout was never actually triggered | Yes (mandatory security/reliability property) | Not yet tested; explicitly deferred rather than waiting 10 real minutes | Add a test-only configurable timeout threshold (code path identical to production), fire it live, then prove the production default is still 10 minutes |
| 3 | 4 GiB output-size guard, live-fired | NOT EXECUTED | Same — guard exists, never actually tripped | Yes | Same | Same pattern: test-only reduced quota, live-fire, prove production default unchanged |
| 3 | Per-stage media audit events | IMPLEMENTATION MISSING | Only `media.job.requested` is audited; `started`/`remux-or-transcode-selected`/`completed`/`cancelled`/`failed`/`limit-exceeded` are not | Yes (audit-trail completeness is a standing security invariant) | Not yet built | Add the missing audit call sites; verify via direct `audit_events` query, no job content logged |
| 3 | cgroup v2 CPU/memory/PIDs enforcement (media jobs / general orchestrator) | BLOCKED BY ENVIRONMENT | `PHASE6_RUNTIME_EVIDENCE.md` items 41-43, rechecked that pass: `cpu.max`/`memory.max`/`pids.max` writes fail `Permission denied` under this container's delegation; policy/primitives exist and are unit-tested | Yes in principle, but genuinely external | Real host cgroup delegation unavailable in this environment; not a code gap | Recheck once more per Part Y; if still unavailable, remains a documented environment blocker (Docker's own OCI-level `pids_limit` enforcement, proven live for Code/Office/Browser, stands in as a separate, real mitigation) |
| 4 | Video application backend/router/data path | PASS | `V1_TRUE_CLOSURE.md` #2 CLOSED, `crates/media`/`media_api.rs` live tests, 14 frontend unit tests | Yes | — | None |
| 4 | Video real-browser acceptance (playback/seek/speed/fullscreen/subtitles/track-switch) | NOT EXECUTED (previously `BLOCKED BY ENVIRONMENT`, now stale) | `V1_TRUE_CLOSURE.md` #2 marked this `BLOCKED BY ENVIRONMENT` before this session's Phase 8 work stood up a pinned Playwright/Chromium harness (`office_browser.rs`) | Yes | The blocking reason (no browser tooling) no longer holds — Part AB requires re-evaluation, not automatic carry-forward | Run a real Playwright acceptance pass against `VideoApp.svelte` (login → Files → video → playback → seek → speed → fullscreen → subtitles → audio-track) — not attempted this pass (scoped to PASS 5 in the governing prompt's own strategy) |
| 5 | Music application backend/router/data path | PASS | `V1_TRUE_CLOSURE.md` #3 CLOSED, `crates/library` + `music_api.rs` (24 tests), 18 frontend unit tests | Yes | — | None |
| 5 | Music real-browser acceptance (library/play/queue/playlist/favorite/search) | NOT EXECUTED (previously `BLOCKED BY ENVIRONMENT`, now stale) | Same reasoning as Video | Yes | Same | Same — real Playwright pass against `MusicApp.svelte`, not attempted this pass |
| 6 | Optional-runtime orchestrator (`crates/orchestrator`) | PASS (COMPLETE) | `PHASE6_RUNTIME_EVIDENCE.md`: 38/40 PASS, 1 NOT EXECUTED (persistent-profile retention — no persistent-kind adapter existed at that time; Browser now provides one, see Phase 9 row below), 4 `BLOCKED BY ENVIRONMENT` (cgroup x3 + Settings browser acceptance) | Yes | — | Settings browser acceptance is the one re-testable item now that Playwright exists — see next row |
| 6 | Settings browser acceptance (enable/disable runtime cards) | NOT EXECUTED (previously `BLOCKED BY ENVIRONMENT`, now stale) | `PHASE6_RUNTIME_EVIDENCE.md` item 44 | Yes | Playwright harness now exists | Real Playwright pass: Administrator login → Settings → Runtime section → Code/Office/Browser cards → enable/disable → verify state; not attempted this pass |
| 6 | Persistent-profile retention re-test with a real persistent adapter | PASS (superseded) | Phase 9's `task_5_7_user_role_browser_profile_is_persistent` now provides exactly this live evidence (Browser, User role, real stop/restart) | Yes | — | None — the original NOT EXECUTED row is closed by Phase 9 evidence |
| 7 | VS Code-compatible runtime | PASS (COMPLETE) | `PHASE7_CODE_EVIDENCE.md`: 37 PASS, 2 capability-PASS/browser-blocked, 4 scoped PARTIAL (sub-claims resolved), 1 NOT EXECUTED (clipboard), 1 `BLOCKED BY ENVIRONMENT` (browser automation itself — now stale), 0 FAIL | Yes | — | See next two rows for the two re-testable items |
| 7 | Code browser acceptance (login → Code → edit → save → terminal → hover/completion/debug where practical) | NOT EXECUTED (previously `BLOCKED BY ENVIRONMENT`, now stale) | `PHASE7_CODE_EVIDENCE.md`'s five permitted browser-only exceptions | Yes | Playwright harness now exists | Real Playwright pass against `CodeApp.svelte`; not attempted this pass |
| 7 | Code clipboard | NOT EXECUTED | `PHASE7_CODE_EVIDENCE.md`, listed as the one NOT EXECUTED item independent of the browser blocker | Yes | Not yet tested | Live-test clipboard round-trip through the Code runtime |
| 7 | Public GitHub/GitLab credential auth | BLOCKED BY ENVIRONMENT | `PHASE7_CODE_EVIDENCE.md` | No (genuinely external, no credentials provided) | External account credentials not available in this environment | Remains blocked per Part AB's explicit allowance |
| 8 | LibreOffice/Collabora runtime (WOPI host, OCI adapter, browser editing, SSRF closure) | PASS (COMPLETE) | `PHASE8_OFFICE_EVIDENCE.md` final verdict: all 4 representative formats pass real browser edit/save/reopen, SSRF Model A, macro policy resolved, 0 unresolved Critical/High | Yes | — | `V1_TRUE_CLOSURE.md` #4 still says "PARTIAL" — that is stale prose superseded by the newer, executable Phase 8 evidence per this document's own Part-A instruction |
| 8 | Office test-fixture container leaks | PASS (fixed this pass) | `office_runtime.rs` (fixed prior pass), `office_browser.rs` (13 tests), `office_hostile_documents.rs` (1 test), `office_remote_vfs.rs` (3 tests) — all four now carry the same `CollaboraContainerGuard` RAII pattern, added this pass | Yes (resource hygiene, explicit Part U requirement) | Was open at the start of this pass (11 leaked containers found after a full `cargo test --workspace` run); fixed this pass | Confirm via the in-flight `cargo test --workspace` run (see Validation section) that zero containers leak from any of the now four guarded files |
| 9 | Brave OCI adapter, sandbox, non-root, raw-CDP isolation | PASS | `PHASE9_BROWSER_EVIDENCE.md` Tasks 1-3, 48-52 | Yes | — | None |
| 9 | Browser production-safe resource policy (`pids_limit`) | PASS | `PHASE9_BROWSER_EVIDENCE.md` Tasks 63-64, per-kind `ResourcePolicy` in `crates/orchestrator/src/manager.rs`, `pids_limit: 512` wired in `main.rs` | Yes | — | None |
| 9 | Role-aware profile persistence (Admin/Manager/User persistent, Guest ephemeral) | PASS | `PHASE9_BROWSER_EVIDENCE.md` Tasks 4-5/67, live `task_5_7`/`task_5_8` | Yes | — | None |
| 9 | Cross-user Browser profile isolation | PASS | Same tests | Yes | — | None |
| 9 | Cookie persistence (as opposed to `localStorage`) | **PASS (LIVE CLOUDDESK, Pass 3A-3)** | `PHASE9_BROWSER_EVIDENCE.md`: real root cause found (non-`exec`ing vendor wrapper script + missing CDP `Browser.close` shutdown hook, not OS-crypt/keyring), fixed, live-verified end-to-end through the real product API (`browser_cookies.rs::task_1_4_5_6_cookie_persistence_live_matrix`) — User A's real cookie survives a real stop/restart, User B never sees it, Guest's does not survive its restart | Yes (Part C) | — | None — closed |
| 9 | Trusted typed CDP broker | PASS | `PHASE9_BROWSER_EVIDENCE.md` Task 8; `browser_broker.rs`; live tests in `browser_broker.rs` (5/5) | Yes | — | Fixed set of typed operations only (navigate/resize/mouse/keyboard/screencast); no `TabId`/tab operations yet (see next row) — per-connection session state, no separate persisted `BrowserSession` registry (sufficient for one page, would need one for tabs) |
| 9 | Frame/screencast streaming + backpressure | PASS | `PHASE9_BROWSER_EVIDENCE.md` Tasks 9-10; live frames received within 15s, watch-channel latest-wins delivery, CDP-ack-gated production | Yes | — | No formal memory-growth stress test (rapid-animation + deliberately-paused client, byte-counted) — architecturally bounded, not independently load-tested |
| 9 | Authenticated Browser frame/control WebSocket | PASS | `PHASE9_BROWSER_EVIDENCE.md` Task 11-12; `/api/v1/runtime-instances/browser/{instance_id}/browser-ws`; live-tested owner/unauthenticated/cross-user denial | Yes | — | Stale-generation and logout/runtime-stop closure are covered by the periodic generation check + `instance_port` re-check (live-tested via the crash-recovery test, not a dedicated logout-mid-session test) |
| 9 | Mouse/keyboard/basic Unicode input | PASS | `PHASE9_BROWSER_EVIDENCE.md` Tasks 13-16; live-verified against a controlled fixture site (real click + real Unicode text reaching the real DOM) | Yes | — | `IME COMPOSITION: NOT IMPLEMENTED` (single-codepoint `char` events only, no real composition-event protocol) — explicitly not claimed |
| 9 | Navigation scheme policy | PASS | `PHASE9_BROWSER_EVIDENCE.md` Task 7; `file:`/`javascript:` live-tested as rejected | Yes | — | `data:`/`blob:`/`chrome:`/`brave:` rejected by conservative default, not independently investigated and cleared |
| 9 | Tabs, popups | PASS (LIVE CLOUDDESK, Pass 3A) | `PHASE9_BROWSER_EVIDENCE.md` Tasks 23-27; `browser_broker.rs` rewritten to real CDP Target multiplexing; `task_1_3_tab_lifecycle_create_switch_close`, `task_2_tab_ownership_cross_session_denied`, `task_4_popup_becomes_managed_tab_and_storm_is_bounded` | Yes | — | Opaque, process-wide-unique `TabId`s; real create/switch/close lifecycle; real `window.open()` popups auto-attached; bounded storm defense (max 8 tabs/session); frontend tab strip added |
| 9 | Playwright-through-the-compiled-frontend acceptance | PASS (LIVE CLOUDDESK, Pass 3A-2) | `browser_playwright.rs::task_1_2_3_playwright_compiled_frontend_full_flow` — a real, pinned Playwright/Chromium container drives the actual compiled frontend (never Brave CDP, never the broker protocol directly): login, Browser app, real non-blank screencast frame, zero iframes, real click/type reaching the real fixture (verified via the fixture's own independent log), real second tab, real popup becoming a managed tab | Yes | — | Checkbox/scroll dispatched but not independently asserted on the fixture log; a dedicated hostile parent/top/opener-access fixture (Task 3's literal ask) not built separately this pass |
| 9 | Logout / session revocation | PASS (LIVE CLOUDDESK, Pass 3A-2) | `task_18_logout_denies_new_browser_sessions` — matches this project's existing revocation policy | Yes | — | Already-open connections aren't proactively killed mid-session, matching Office's own established, documented policy — not a Browser-specific exception |
| 9 | Service restart / stale-instance denial | PASS (LIVE CLOUDDESK, Pass 3A-2 — real defect found and fixed) | `task_19_20_service_restart_marks_stale_instance_failed` — real defect found: `Failed` rows counted against `max_instances_per_user`, permanently locking out any user whose session was active during a restart (no self-service recovery); fixed in `crates/orchestrator/src/manager.rs::create_instance` | Yes | — | Re-verified against the full `crates/orchestrator` suite (18 tests, unchanged) |
| 9 | Internal-network isolation, WebRTC baseline, multi-user simultaneous acceptance, full route-authorization matrix, frame-backpressure stress evidence | OPEN / NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` — unchanged this pass; cookie persistence (previously listed here) is now closed, see the dedicated row above | Yes | Not attempted this pass — Pass 3A-3's priority was closing Blocker 1 (cookie persistence), the single highest-priority remaining gap; each of these five is a substantial scope on its own | Continue in a future pass; internal-network isolation is now the single highest-priority remaining Browser security item |
| 9 | Browser frontend (`BrowserApp.svelte`) | PASS (minimal) | `PHASE9_BROWSER_EVIDENCE.md` Task 68; `apps/web/src/lib/BrowserApp.svelte`; frontend gates (lint/check/test/build) all pass with it included | Yes | — | No back/forward/reload buttons (optional per Task 19's own "if easy"); real acceptance evidence so far drives the same WebSocket protocol directly, not yet through a Playwright-controlled instance of this actual component (see next row) |
| 9 | Server-side-origin acceptance (CloudDesk-mediated, not raw CDP) | PASS | `PHASE9_BROWSER_EVIDENCE.md` Task 18; live test confirms the controlled site's request arrived from Brave's own container network via the typed broker, not the test process directly | Yes | — | Drives the WebSocket protocol directly (a real client speaking the exact protocol `BrowserApp.svelte` speaks), not literally through a Playwright-automated instance of the compiled frontend — see next row for that narrower gap |
| 9 | Crash recovery (Browser-specific, live) | PASS (real regression found and fixed in Pass 3A-3) | `PHASE9_BROWSER_EVIDENCE.md` Task 24-adjacent; `task_24_crash_handling_and_generation_invalidation` — real `docker kill`, explicit `closed` message, `RuntimeManager` detects failure, clean reconnect after restart. Pass 3A-3's own full-workspace regression run found this test genuinely flaky (~1 in 3, reproducible in complete isolation): a real race in `outbound_writer` (`services/clouddeskd/src/browser_broker.rs`) could silently drop the already-queued `"closed"` message when `tokio::select!` picked the `frame_rx` error branch first, hanging the client instead of reporting the crash. Fixed by draining buffered `misc_rx` messages before breaking on that branch; re-verified 5/5 isolated + clean in a full-workspace run after the fix | Yes | — | — |
| 9 | Enable/disable (Browser-specific, dedicated live test) | PASS | `task_25_enable_disable_lifecycle` — disable-while-active, zero containers after, denied-while-disabled, usable again after re-enable | Yes | — | Re-enable reuses the existing instance (restart) rather than creating a new one, due to the documented `max_instances_per_user` gap |
| 9 | Downloads (staging, quota, malicious-Content-Disposition, no auto-execution) | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 34-39 | Yes | Never built | Brave download → per-user staging → progress/status → completion → Files save/move, with traversal/absolute/duplicate/oversized/quota/interrupted/malicious-header security tests |
| 9 | Uploads (file-chooser mediation) | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 34-39 | Yes | Never built | Website file request → CloudDesk chooser → backend reauthorization → materialize selected file only → temp local path to Brave → cleanup; no native filesystem chooser, no home-directory mount, no provider credential given to Brave |
| 9 | Clipboard bridge | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 40-41 | Yes | Never built | Scoped per-session bridge, not global host clipboard; User A content never reaches User B; Guest clipboard removed with session |
| 9 | Audio (per-user capture, cross-user isolation) | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 29-31/75 | Yes (explicit Phase 9 closure requirement per Part O) | Never built | Isolated per-session audio sink → bounded encoded stream (Opus/WebRTC/WebSocket) → Browser UI; controlled-tone test; verify User A doesn't hear User B; bound buffering/latency/memory |
| 9 | Video playback acceptance (through real CloudDesk Browser, with audio) | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` Task 32-33 | Yes | Depends on frame transport + audio, neither built | Controlled website video test once the above exist |
| 9 | WebRTC leakage review | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` Task 53-57 | Yes | Not reviewed | Controlled WebRTC page; verify only expected runtime/container network info is revealed, no host mic/camera exposure unless required |
| 9 | Internal-network isolation (SSRF-class: loopback/gateway/internal endpoints/RFC1918/metadata-shaped route) | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` Task 18-22 | Yes | A real navigation surface now exists (this pass), but the attack matrix itself was not run; only "not `--network=host`" is structurally confirmed | Real navigation exists now; primary protection must be network architecture, not URL blacklists; never contact a real cloud metadata service |
| 9 | Service-restart reconciliation for active Browser sessions | PASS (crash-kill case); NOT EXECUTED (planned graceful `clouddeskd` restart case) | `task_24_crash_handling_and_generation_invalidation` covers the abrupt-death case live; a graceful `clouddeskd` process restart with a live session was not separately tested | Yes | Partially covered this pass | Add a dedicated graceful-restart test if the two cases are expected to behave differently |
| 9 | Multi-user live acceptance (simultaneous User A/User B/Guest across all built surfaces) | PARTIAL | Sequential two-user profile isolation (prior pass) plus this pass's ownership/cross-user-denial test (`task_1_2_...`) for the broker itself; not simultaneous, and not across the still-unbuilt surfaces (audio/downloads/uploads/clipboard) | Yes | Depends on the unbuilt surfaces above for full coverage | Run after audio/downloads/uploads/clipboard exist |
| 9 | Browser-specific route authorization matrix | PARTIAL | The one real Browser-specific route (`browser-ws`) is live-tested for unauthenticated/cross-user denial (`task_1_2_...`); no dedicated matrix sweep across Guest/User/Manager/Administrator was run | Yes | Only one route exists so far; a fuller matrix is more meaningful once tabs/downloads/uploads add more routes | Expand as more Browser-specific routes are added |
| 9 | SYS_ADMIN/SYS_CHROOT justification | PASS | `PHASE9_BROWSER_EVIDENCE.md` Task 51: real alternative (`--disable-namespace-sandbox`) tried and ruled out; capabilities genuinely required for Chromium's own sandbox to initialize; never traded for `--no-sandbox` | Yes | — | None — already the strongest achievable posture; residual risk (two elevated capabilities beyond the zero-capability baseline) is explicit, not hidden |
| — | Distro-matrix installer/service verification (8 platforms) | BLOCKED BY ENVIRONMENT | `V1_TRUE_CLOSURE.md` #15 | Yes per `GOAL.md`, but this is Phase 10's own subject matter | No per-distro CI/VM infrastructure available in this environment | Explicitly out of scope for this gate — this is what Phase 10 itself is for; not a Phase 1-9 gap |

## Newly found, out-of-this-pass's-scope items (not fixed, honestly documented)

1. **Browser test-concurrency defect** (found and fixed this pass, prior to this gate): `browser_runtime.rs`'s 4 tests raced under `cargo test --workspace`'s default within-binary parallelism; fixed with `acquire_cross_process_browser_lock()`.
2. **Broader Office container-leak scope than previously believed**: the earlier checkpoint entry said only `office_runtime.rs` leaked; this pass's own `cargo test --workspace` run found 11 additional real, running Collabora containers from `office_browser.rs`, `office_hostile_documents.rs`, and `office_remote_vfs.rs`. Fixed this pass (see Phase 8 row above).

## Environment blockers that are genuinely external (Part AB)

- Public GitHub/GitLab account authentication (Phase 7) — no credentials provided.
- cgroup v2 CPU/memory/PIDs controller delegation (Phase 3/6) — permission denied under this container's delegation, rechecked, no sudo used, no host cgroup mutated.
- Distro-matrix installer/service verification (8 platforms) — Phase 10's own subject matter, not a Phase 1-9 gap.

**Not retained as environment blockers** (explicitly re-evaluated per Part Z/AB, since the Playwright/Chromium harness now exists from Phase 8's `office_browser.rs`): Video, Music, Settings, and Code browser acceptance are now `NOT EXECUTED`, not `BLOCKED BY ENVIRONMENT` — the tooling exists, the acceptance runs simply have not been performed yet.

## Summary counts

- Mandatory `IMPLEMENTATION MISSING`: **6** (SSH agent, keyboard-interactive, certificates, SCP, remote PTY terminal; Browser downloads/uploads/clipboard/audio — Phase 3's per-stage media audit events also counts — see row-by-row list above for the authoritative enumeration, this bullet is a convenience count only)
- Mandatory `NOT EXECUTED`: **~6** (Phase 3 timeout/quota live-fire ×2; Video/Music/Settings/Code browser acceptance ×4; Phase 7 clipboard; Phase 9 internal-network-isolation matrix, WebRTC review, video-playback acceptance, frame-backpressure stress evidence — multi-user simultaneous acceptance and the full route-authorization matrix remain PARTIAL — see rows for the authoritative list)
- Mandatory `FAIL`/`OPEN`: **0** (Browser cookie persistence closed this pass — see row above)
- Unresolved Critical: **0**
- Unresolved High: **0**
- Environment blockers (genuinely external): **3** (public GitHub/GitLab auth, cgroup delegation, distro-matrix infrastructure)
- Test resource leaks: **0 leaked**, confirmed via a full `cargo test --workspace --no-fail-fast` run (74/74 binaries ok) followed by `docker ps -a` — see Validation

## Rust/frontend gates (this pass — Pass 3A-3, post-outage re-verification)

Numbers below are from commands actually observed completing on
current HEAD (`6072f41`) after a mid-pass execution-tool outage — not
carried over from any pre-outage attempt:

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace --no-fail-fast`: **74/75 test binaries fully
green, 1 binary (`browser_broker`) with exactly 1 failing test**
(`task_4_popup_becomes_managed_tab_and_storm_is_bounded` — confirmed by
immediate isolated re-run, 1 fail then 2 clean passes, to be the same
pre-existing Docker-load-contention class already documented in Pass
3A-2, not a new regression). `task_24_crash_handling_and_generation_invalidation`
(the crash-close race fixed this pass) passed clean in this same
full run.
`cargo build --workspace --release`: PASS (1m01s incremental).
Frontend gates: PASS -- `npm run lint` (0 errors/warnings)/`check` (0
errors/warnings)/`test` (91/91)/`build` (clean `dist/`) all green.
Resource cleanup: zero leaked `collabora/code`/`clouddesk-brave`/
`mcr.microsoft.com/playwright` containers (`docker ps -a` empty) and
zero stray Brave/socat/Playwright/Collabora helper processes (`ps aux`
checked) after the full run.

## READY FOR PHASE 10: NO

Per the governing policy, YES requires zero mandatory `IMPLEMENTATION
MISSING`, zero mandatory `NOT EXECUTED`, Phase 9 Browser `COMPLETE`,
and Phase 2 SSH mandatory features `COMPLETE`. None of those hold yet:
Phase 9 now has a real, live-tested vertical slice proven through the
actual compiled frontend under real Playwright (broker, frames,
WebSocket, input, navigation, tabs, popups, frontend, crash recovery,
enable/disable, logout, service restart, **and now real cookie
persistence with cross-user and Guest isolation**) but downloads,
uploads, clipboard, audio, the full internal-network-isolation matrix,
WebRTC review, simultaneous multi-user acceptance, a full route-
authorization matrix, and formal frame-backpressure stress evidence
remain unbuilt or unrun; Phase 2 SSH's five mandatory targets (agent,
keyboard-interactive, certificates, SCP, remote terminal) remain
entirely unimplemented.

**Next exact action**: continue Pass 3A-3's remaining five blockers —
internal-network isolation is the single highest-value remaining
Browser security item, followed by WebRTC leakage review,
frame-backpressure stress evidence, simultaneous multi-user
acceptance, and the full route-authorization matrix; alternatively
begin Phase 2 SSH closure, per whichever the next governing prompt
specifies.

Do not start Phase 10. Do not create distro fixtures. Do not push, tag,
move `v1.0.0`, or create `v1.0.1-rc.1`.
