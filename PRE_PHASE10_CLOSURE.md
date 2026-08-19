# Pre-Phase-10 Closure Gate

Authoritative open-item register for CloudDesk-OS v1, built by
cross-checking `CLAUDE_ENGINEERING_CHECKPOINT.md`, `V1_TRUE_CLOSURE.md`,
`PHASE6_RUNTIME_EVIDENCE.md`, `PHASE7_CODE_EVIDENCE.md`,
`PHASE8_OFFICE_EVIDENCE.md`, `PHASE9_BROWSER_EVIDENCE.md`, and the
actual current implementation/tests as of commit `49908a6` plus this
pass's Office-fixture-leak fix. Where `V1_TRUE_CLOSURE.md`'s prose was
stale relative to newer phase evidence (e.g. it still describes
Browser as "no adapter exists" — false as of Phase 9), the newer,
executable evidence wins and is cited instead.

**This is PASS 1 of a multi-pass closure per the governing prompt's own
execution strategy.** PASS 1 scope actually completed this session:
Office fixture cleanup (extended to the 3 additional leaking test
files) and this register. The Browser product vertical slice (broker,
frame transport, frontend, audio, downloads, uploads, clipboard,
network isolation, multi-user acceptance — Parts D through S) and
Phase 2 SSH closure (Part V) are each realistically multi-day
implementation efforts and were **not attempted this pass** — they are
recorded below as `IMPLEMENTATION MISSING`, not silently deferred.

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
| 9 | Cookie persistence (as opposed to `localStorage`) | FAIL | `PHASE9_BROWSER_EVIDENCE.md` Task 5 note: real cookie values reach disk with a genuine encrypted value but cannot be decrypted after a real restart — no dbus/keyring daemon for Chromium's OS-crypt backend in this container; `--password-store=basic` does not fix it | Yes (Part C: "`localStorage` persistence is NOT sufficient evidence for a persistent Browser profile... Persistent browser profiles are expected to retain normal authenticated browser state such as cookies.") | Root-caused but not fixed — needs a real, secure, per-user keyring/password-store configuration, not a workaround | Investigate a deterministic per-user Linux password-store/keyring configuration whose storage stays inside the per-instance profile/state mount (e.g. a real, minimal keyring daemon started inside each container, backed by a per-instance key derived server-side, never shared across users/containers); live-test cookie set → stop → restart → persists (User), absent (Guest), isolated (User A vs B). Not attempted this pass — scoped to a future pass |
| 9 | Trusted typed CDP broker | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Task 8 | Yes | Never built | Design and implement typed `BrowserSession`/`TabId`/navigate/back/forward/reload/create-tab/activate-tab/close-tab/viewport/mouse/keyboard/screencast operations bound to authenticated user + runtime instance + runtime generation; raw CDP must never reach the frontend |
| 9 | Frame/screencast streaming + backpressure | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 9-12 | Yes | Never built | CDP-screencast-based bounded frame transport with ACK/backpressure so a slow client can't cause unbounded server memory growth |
| 9 | Authenticated Browser frame/control WebSocket | IMPLEMENTATION MISSING | Same | Yes | Never built | New CloudDesk WebSocket endpoint, tested against owner/unauthenticated/cross-user/stale-generation/logout/runtime-stop |
| 9 | Mouse/keyboard/IME input | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 13-15 | Yes | Never built | Typed input operations through the broker; live-test against a controlled webpage (text input, button, checkbox, select, scroll); basic Unicode, not full IME unless composition is genuinely implemented |
| 9 | Navigation, tabs, popups | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 16-17, 23-28 | Yes | Never built | Opaque `TabId`-scoped list/create/activate/close; `window.open`/`target=_blank` handled as managed tabs, never unmanaged GUI windows; bounded popup-storm test |
| 9 | Browser frontend (`BrowserApp.svelte`) | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Task 68 | Yes | Never built; only a launcher-tile manifest placeholder exists | Address bar, back/forward/reload, tabs, new/close tab, canvas rendering surface, loading/disconnected/failed-retry states; website content stays pixels, never injected DOM |
| 9 | Server-side-origin acceptance (Playwright client → CloudDesk UI → broker → server-side Brave → controlled site) | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` — only a standalone raw-CDP navigation test exists, driven directly, not through any CloudDesk-mediated path | Yes | Depends on the broker + frontend, neither built yet | Build the broker/frontend first; then run this acceptance path with a controlled website that records request source |
| 9 | Downloads (staging, quota, malicious-Content-Disposition, no auto-execution) | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 34-39 | Yes | Never built | Brave download → per-user staging → progress/status → completion → Files save/move, with traversal/absolute/duplicate/oversized/quota/interrupted/malicious-header security tests |
| 9 | Uploads (file-chooser mediation) | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 34-39 | Yes | Never built | Website file request → CloudDesk chooser → backend reauthorization → materialize selected file only → temp local path to Brave → cleanup; no native filesystem chooser, no home-directory mount, no provider credential given to Brave |
| 9 | Clipboard bridge | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 40-41 | Yes | Never built | Scoped per-session bridge, not global host clipboard; User A content never reaches User B; Guest clipboard removed with session |
| 9 | Audio (per-user capture, cross-user isolation) | IMPLEMENTATION MISSING | `PHASE9_BROWSER_EVIDENCE.md` Tasks 29-31/75 | Yes (explicit Phase 9 closure requirement per Part O) | Never built | Isolated per-session audio sink → bounded encoded stream (Opus/WebRTC/WebSocket) → Browser UI; controlled-tone test; verify User A doesn't hear User B; bound buffering/latency/memory |
| 9 | Video playback acceptance (through real CloudDesk Browser, with audio) | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` Task 32-33 | Yes | Depends on frame transport + audio, neither built | Controlled website video test once the above exist |
| 9 | WebRTC leakage review | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` Task 53-57 | Yes | Not reviewed | Controlled WebRTC page; verify only expected runtime/container network info is revealed, no host mic/camera exposure unless required |
| 9 | Internal-network isolation (SSRF-class: loopback/gateway/internal endpoints/RFC1918/metadata-shaped route) | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` Task 18-22 | Yes | No navigation surface exists yet to attack; only "not `--network=host`" is structurally confirmed | Build once navigation exists; primary protection must be network architecture, not URL blacklists; never contact a real cloud metadata service |
| 9 | Service-restart reconciliation for active Browser sessions | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` Task 59 (mechanism inherited generically, not independently re-tested for Browser) | Yes | Not tested | Explicit reattach-or-terminate policy; old session/tab IDs must not attach to a replacement generation; no stale CDP takeover |
| 9 | Multi-user live acceptance (simultaneous User A/User B/Guest across all built surfaces) | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` — only sequential two-user profile isolation tested | Yes | Depends on the unbuilt surfaces above | Run after broker/frontend/audio/downloads/uploads/clipboard exist |
| 9 | Browser-specific route authorization matrix | NOT EXECUTED | `PHASE9_BROWSER_EVIDENCE.md` — no Browser-specific routes exist yet beyond the generic ones already swept for Code/Office | Yes | No broker routes exist to sweep | Build once the broker exists; attack as unauthenticated/Guest/User A/User B/Manager/Administrator |
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

- Mandatory `IMPLEMENTATION MISSING`: **13** (SSH agent, keyboard-interactive, certificates, SCP, remote PTY terminal; Browser broker, frame transport, authenticated WebSocket, input, tabs/navigation, frontend, downloads, uploads, clipboard, audio — Phase 3's per-stage media audit events also counts, bringing the precise figure to **14**; see row-by-row list above for the authoritative enumeration, this bullet is a convenience count only)
- Mandatory `NOT EXECUTED`: **11** (Phase 3 timeout/quota live-fire ×2; Video/Music/Settings/Code browser acceptance ×4; Phase 7 clipboard; Phase 9 server-side-origin, video-playback, WebRTC review, service-restart reconciliation, multi-user acceptance, route-authorization matrix — again, see rows for the authoritative list)
- Mandatory `FAIL`: **1** (Browser cookie persistence)
- Unresolved Critical: **0**
- Unresolved High: **0**
- Environment blockers (genuinely external): **3** (public GitHub/GitLab auth, cgroup delegation, distro-matrix infrastructure)
- Test resource leaks: **0 leaked**, confirmed via a full `cargo test --workspace --no-fail-fast` run (57/57 binaries ok) followed by `docker ps -a` — see Validation

## Rust/frontend gates (this pass)

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace`: PASS. Default (fail-fast) run stopped after
`task_30_crash_recovery` (`code_runtime.rs`) failed under heavy
concurrent Docker load; confirmed via isolated rerun to be the
pre-existing Docker-load-contention flake, not a regression (its own
log literally says "Docker-load contention is expected here"; passed
alone in 27.85s). `cargo test --workspace --no-fail-fast` then
confirmed the complete picture: **57/57 binaries ok, 0 failed**,
including all four newly-guarded Office files and `browser_runtime.rs`.
`cargo build --workspace --release`: PASS (confirmed in the prior
Phase 9 pass; unaffected by this pass's test-only changes).
Frontend gates: unaffected this pass (no `apps/web` files touched).
Resource cleanup: zero leaked `collabora/code` or
`clouddesk-brave:1.93.136` containers confirmed via `docker ps -a`
immediately after the full `--no-fail-fast` run.

## READY FOR PHASE 10: NO

Per the governing policy, YES requires zero mandatory `IMPLEMENTATION
MISSING`, zero mandatory `NOT EXECUTED`, Phase 9 Browser `COMPLETE`,
and Phase 2 SSH mandatory features `COMPLETE`. None of those hold yet:
Phase 9 remains a foundation (broker/frames/frontend/audio/downloads/
uploads/clipboard/network-isolation/multi-user acceptance all
`IMPLEMENTATION MISSING` or `NOT EXECUTED`), and Phase 2 SSH's five
mandatory targets (agent, keyboard-interactive, certificates, SCP,
remote terminal) remain entirely unimplemented.

**Next exact action** (PASS 2 per the governing prompt's own
execution strategy): begin the Browser product vertical slice —
trusted CDP broker (Part D) and a minimal frame-streaming transport
(Part E) first, since every other unbuilt Browser item depends on
having a page actually visible through CloudDesk. PASS 3 (Phase 2 SSH
closure) and the remaining passes should follow only after PASS 2
lands, per the prompt's explicit "do not attempt all of this
recklessly in one context window" instruction.

Do not start Phase 10. Do not create distro fixtures. Do not push, tag,
move `v1.0.0`, or create `v1.0.1-rc.1`.
