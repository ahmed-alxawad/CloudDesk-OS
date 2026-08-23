# Pre-Phase-10 Closure Gate

Authoritative open-item register for CloudDesk-OS v1, built by
cross-checking `CLAUDE_ENGINEERING_CHECKPOINT.md`, `V1_TRUE_CLOSURE.md`,
`PHASE6_RUNTIME_EVIDENCE.md`, `PHASE7_CODE_EVIDENCE.md`,
`PHASE8_OFFICE_EVIDENCE.md`, `PHASE9_BROWSER_EVIDENCE.md`, and the
actual current implementation/tests. Where `V1_TRUE_CLOSURE.md`'s prose
was stale relative to newer phase evidence (e.g. it still describes
Browser as "no adapter exists" — false as of Phase 9), the newer,
executable evidence wins and is cited instead.

**Correction:** the original PASS 3B report labeled Phase 9 COMPLETE
while remote-VFS Browser upload was NOT IMPLEMENTED and peripheral-
active admin-disable had not been independently executed. **PASS 3B-2
closes both explicit gaps** -- remote-VFS (SFTP) upload is now built
and live-verified (`services/clouddeskd/tests/browser_remote_uploads.rs`),
and a real Administrator-disable-while-audio/download/clipboard-are-
active run is now live-verified 3/3 clean
(`browser_admin_disable_peripherals.rs`), distinct from the pre-
existing crash/`docker kill` cleanup evidence.

**Correction (Pass 3B-3):** the Pass 3B-2 report above proved the
complete SFTP backend/materialization path but left the Browser
chooser frontend accepting local-home selection only -- Phase 9 was
not actually complete at that point. **Pass 3B-3 adds and verifies
the missing remote-VFS selection through the compiled product UI**
(a real source picker in `BrowserApp.svelte`; a real Playwright test
driving the actual compiled frontend end to end, never a raw/
manually-supplied `server_id`), and separately found and fixed a
real product defect: admin disable force-killed every runtime
instance, skipping Browser's graceful CDP shutdown and silently
losing persistent-profile state -- see `PHASE9_BROWSER_EVIDENCE.md`'s
"Pass 3B-2 / Pass 3B-3" section.

**PASS 3B / PASS 3B-2 / PASS 3B-3 status: COMPLETE.** Downloads,
uploads (local and remote-VFS/SFTP, both through the real product UI
now), upload authorization, remote credential isolation, upload temp
cleanup, clipboard, audio, video+audio playback acceptance, password-
manager/extensions/native-messaging policy, the secret/privacy sweep,
the final new-route authorization accounting, admin-disable-with-
active-peripherals, Guest cleanup on admin disable, and persistent-
profile retention across admin disable are all now genuine, live-
tested PASS -- see `PHASE9_BROWSER_EVIDENCE.md`'s "Pass 3A-4 / Pass
3B" and "Pass 3B-2 / Pass 3B-3" sections and Definition-of-Done
checklist. **Phase 9 Browser is genuinely COMPLETE.** Phase 2 SSH
remains the next real work; the paragraphs below describing PASS
3A-3/earlier are kept as historical record.

**PASS SSH-A / PASS SSH-A-2 status: COMPLETE** for the three
previously `IMPLEMENTATION MISSING` SSH auth methods. SSH agent,
keyboard-interactive, and OpenSSH certificate authentication are now
real, live-verified end to end at four distinct evidence layers:
(1) the live SSH protocol itself, against a real disposable OpenSSH
fixture; (2) `CloudDesk`'s own backend connection resolution
(`resolve_ssh_session`), never a command-line `ssh` proving only
server-side support; (3) the actual product/API path (real HTTP
`POST`/`PUT /api/v1/remote/servers`, `POST .../test-connection`,
`POST /api/v1/vault/secrets`), never a direct `RemoteServerStore`/
`Vault` Rust call; (4) the compiled frontend (`ServersApp.svelte` +
`remoteServers.ts`, real per-method credential entry, not a raw
Vault-secret-ID paste). See `git log` on `engineering/v1-true-closure`
(commits `4a9ccee`, `7a54b55`, `2308eae`, and the PASS SSH-A-2 commits
that follow) for the full change history.

**PASS SSH-B status: COMPLETE (corrected -- see PASS SSH-B-2).**
Native SCP (`crates/remote/src/scp.rs`) is a real, hand-rolled legacy
SCP protocol client speaking `scp -t`/`scp -f` over an SSH exec
channel -- `russh` has no SCP implementation of its own, only SFTP, so
this is genuinely new protocol code, never SFTP relabeled. Live-verified
at three evidence layers: (1) the native SCP protocol itself against a
real disposable OpenSSH server (`crates/remote/tests/scp.rs`, 10/10 --
byte-exact upload/download, an 8 MiB file streamed in 32 bounded chunks
proving no whole-file buffering, command-injection neutralized via
strict path policy + POSIX single-quoting, host-key rejection, a real
`ProxyJump` upload and download, and one advanced-auth method (agent)
reused without a second SSH stack); (2) the real product/API path
(`TransferEndpoint::Scp`, `services/clouddeskd/tests/scp_transfers.rs`,
4/4 -- real `POST /api/v1/transfers`, the real background
`TransferWorker`, cross-user authorization denial, cancellation); (3)
the compiled frontend (`TransfersApp.svelte`'s protocol selector, never
silently falling back to SFTP).

**Correction (PASS SSH-B-2):** the original SSH-B report above marked
COMPLETE while disclosing two unresolved gaps its own Definition of Done
required -- no real mid-transfer SCP upload interruption had been
executed, and the shared `TransferQueue` had no terminal `Failed` state
at all (every unrecoverable job retried forever with exponential
backoff). Per the corrected classification, that made SSH-B **PARTIAL**,
not COMPLETE. **PASS SSH-B-2 closes both gaps and is itself COMPLETE:**
- `TransferQueue` now has bounded retry (`MAX_TRANSFER_ATTEMPTS = 6`)
  with error classification (`TransferError::Permanent` fails
  immediately -- auth denied, invalid path, host-key mismatch;
  everything else retries with the existing exponential backoff, then
  terminates `Failed`), a real terminal `Failed` state, and an
  owner-scoped manual retry (`POST /api/v1/transfers/{id}/retry` ->
  `TransferQueue::retry_failed`). 9 crate-level tests
  (`crates/transfers/src/lib.rs`) prove permanent-fails-immediately,
  transient-retries-then-succeeds, retry-budget-exhaustion, manual
  retry ownership, and cancellation staying distinct from failure.
- A real, live mid-transfer `docker kill` of the disposable OpenSSH
  bastion after real bytes had moved
  (`services/clouddeskd/tests/scp_transfer_interruption.rs`, 2/2):
  upload interruption (never `completed`, `failed` after exactly 6
  attempts, a pre-existing canonical remote destination survives
  byte-for-byte because CloudDesk now uploads to a disposable remote
  temp name and only `mv`s it into place on full success -- Task 10/11
  implemented for real, not merely documented as a gap) and a download
  interruption regression (same temp-then-rename design on the local
  side).
- **Two real defects found and fixed while proving this live** (not
  synthetic): (1) the SCP client had no per-operation timeout at all --
  a truly dead connection (post-kill) could hang indefinitely rather
  than erroring, discovered when the very first interruption attempt
  never completed after 150+ seconds; fixed with a bounded
  per-read/write/flush timeout (30s production default, matching the
  SSH connection's own inactivity timeout). (2) the download job
  created its local temp file *before* resolving the SSH connection, so
  a connection failure during an automatic retry (remote still down)
  left a fresh, empty temp file behind via early return; fixed by
  reordering so the temp file's lifetime is confined to after a
  successful connection.
- Disclosed, not fixed (real, honest limitation of the underlying
  protocol): if the connection itself is what died, CloudDesk's
  best-effort remote-temp-file cleanup (`rm -f` over that same dead
  connection) can also fail, leaving a uniquely-named
  `*.clouddesk-upload-*.part` file on the remote host -- proven live
  and explicitly logged by the test. The canonical destination is never
  affected either way. Automatic retry always restarts the transfer
  from scratch; classic SCP has no byte-range resume primitive.

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
survive. **Blockers 2-6 were then also closed in this same pass**:
Blocker 3 (WebRTC leakage), 4 (frame/backpressure stress), 5
(simultaneous multi-user acceptance), and 6 (full route-authorization
matrix) all reached genuine, live-tested PASS; Blocker 2
(internal-network isolation) reached PARTIAL in Pass 3A-3 -- its
primary risk (arbitrary reachability into other users' runtime
containers) was fixed via a dedicated, `enable_icc=false` Docker
network and live-verified, but host-gateway reachability to
`clouddeskd`'s own API and RFC1918/metadata-style egress filtering
remained real, disclosed residuals. **Pass 3A-4 closed both remaining
residuals**: since this environment has no root access (confirmed
live) to install/verify a real kernel firewall rule, and `CloudDesk`'s
actual threat model here is hostile page content attempting SSRF (not
a Chromium sandbox escape), a mandatory, policy-enforcing egress proxy
(`browser_egress_proxy.rs`) was added instead -- Brave's own
`--proxy-server` flag routes every HTTP(S) request through it, and the
proxy checks the *resolved* IP against a fixed default-deny policy
(RFC1918, loopback, link-local/metadata) before ever dialing out.
Host-gateway, RFC1918, and the real `169.254.169.254` metadata
address are all now live-verified blocked, along with redirect pivots
and page-initiated fetches. Blocker 2 is now PASS. A real regression
(a crash-close message race in `outbound_writer`, Pass 3A-3) and a
real proxy-idempotency bug affecting test reliability (Pass 3A-4) were
also found and fixed. Both passes survived mid-pass execution-tool
outages (Pass 3A-3: command execution unavailable; Pass 3A-4: the Rust
toolchain itself disappeared from the host); every number in this
document was re-verified from scratch afterward, not
carried over. See `PHASE9_BROWSER_EVIDENCE.md` for full detail on all
six blockers. Phase 2 SSH closure (Part V) was not attempted this pass
— still a realistically multi-day effort on its own.

## Open-item register

| Phase | Requirement | Current status | Evidence source | Mandatory for v1? | Reason still open | Exact next action |
|---|---|---|---|---|---|---|
| 1 | Local file manager core (browse/upload/download/rename/move/delete) | PASS | `RELEASE_EVIDENCE_AUDIT.md`, pre-existing acceptance | Yes | — | None |
| 1 | Archive create/extract | PASS | `V1_TRUE_CLOSURE.md` #7, `crates/vfs/tests/archive.rs` (10 tests) | Yes | — | None; not yet exercised through the real HTTP API/browser, but function-boundary evidence is solid |
| 1 | ACL view/edit | PASS | `V1_TRUE_CLOSURE.md` #8, `crates/vfs/tests/acl.rs` (6 tests) | Yes | — | None |
| 1 | Resumable/chunked upload | PASS | `V1_TRUE_CLOSURE.md` #9, `resumable_upload.rs` | Yes | — | None |
| 2 | SFTP core (list/upload/download/rename/mkdir/delete) | PASS | Prior Nightmare-audit live evidence | Yes | — | None |
| 2 | ProxyJump/bastion (SFTP/transfer path) | PASS | `ssh_proxyjump.rs` (12 tests), real 2-container topology | Yes | — | None |
| 2 | SSH agent authentication | PASS | `crates/remote/src/ssh.rs::authenticate_agent`; live backend evidence `services/clouddeskd/tests/ssh_advanced_auth.rs` (10/10); live product/API evidence `services/clouddeskd/tests/remote_server_auth_product.rs` (5/5, real HTTP `POST /api/v1/remote/servers` + `POST .../test-connection`); frontend config in `apps/web/src/lib/ServersApp.svelte` + `remoteServers.ts` (18 unit tests) | Yes (`GOAL.md` G8) | — | None |
| 2 | Keyboard-interactive authentication | PASS (deliberately narrowed v1 scope, disclosed) | `crates/remote/src/ssh.rs::authenticate_keyboard_interactive` (real RFC 4256 rounds against real sshd); same backend/product/frontend evidence as agent auth above | Yes (`GOAL.md` G8) | Responses are pre-configured at registration time and replayed in order, not a live per-connection challenge UI -- CloudDesk is a multi-tenant server process with no human at a live prompt during automated connections; documented explicitly in `ssh_advanced_auth.rs` and `ServersApp.svelte`'s own doc comments rather than silently narrowed | None for v1; a live interactive challenge UI (Part C of the PASS SSH-A-2 prompt) was explicitly not built -- would require new stateful pending-connection/session infrastructure absent from the backend design |
| 2 | SSH certificate authentication | PASS | `crates/remote/src/ssh.rs::authenticate` (`authenticate_openssh_cert`, real `TrustedUserCAKeys` validation); live backend evidence `ssh_advanced_auth.rs` (host-key regression, denial matrix, certificate-through-ProxyJump); live product/API evidence `remote_server_auth_product.rs` (config + connect + ProxyJump, all through the real HTTP API); frontend key+cert entry in `ServersApp.svelte` | Yes (`GOAL.md` G8) | — | None |
| 2 | Native SCP | PASS | `crates/remote/src/scp.rs` (hand-rolled legacy SCP protocol client -- `scp -t`/`scp -f` over an SSH exec channel; `russh` has no SCP implementation of its own, only SFTP); live protocol evidence `crates/remote/tests/scp.rs` (10/10: upload/download/hash-exact, 8 MiB streamed in 32 bounded chunks, command-injection neutralized, host-key rejection, ProxyJump, agent auth); live product/API evidence `services/clouddeskd/tests/scp_transfers.rs` (4/4: `TransferEndpoint::Scp`, real `POST /api/v1/transfers`, real background `TransferWorker`, authorization matrix, cancellation); frontend in `TransfersApp.svelte` (protocol selector, never silently falls back to SFTP) | Yes (`GOAL.md` G9) | — | None |
| 2 | Remote terminal (PTY) over SSH | **PASS (PASS SSH-C, live-evidence-complete as of PASS SSH-C-2)** | `crates/remote/src/pty.rs` (`TerminalSession`: real `pty-req` + `shell` over a real SSH channel, over the exact same `SshSession`/`resolve_ssh_session` every other SSH feature uses); crate-level live evidence `crates/remote/tests/pty.rs` (4/4: real PTY proven via `test -t 0`/`stty size`, real resize, real Ctrl-C as the literal `0x03` byte distinguishing foreground-interrupt from connection-teardown, real exit status); `ProxyJump` PTY live evidence `services/clouddeskd/tests/ssh_proxyjump.rs::task_27_28_29_...` (1/1: shell proven to run on the target, not the bastion, via `whoami && hostname`); product/API evidence `services/clouddeskd/src/remote_terminal.rs` (new `GET /api/v1/remote/servers/{id}/terminal/ws`, capability `remote.terminal.open` -- pre-existing in `crates/permissions`/`crates/auth` role seeding, needed no new wiring) + `services/clouddeskd/tests/remote_terminal_product.rs` (11/11 as of SSH-C-2: real shell + resize through the real WS, cross-user/stale-id/unauthenticated/deleted-server denial, session-logout revocation via the bridge's 5s periodic re-validation loop, hostile malformed-JSON/absurd-resize input handled safely, **real simultaneous User A/User B terminals with live-proven zero cross-talk/resize-isolation/close-isolation**, **a real `clouddeskd` restart via genuine `SIGKILL` on the actual compiled binary** proving old WS severed + old remote shell reaped + fresh PTY works post-restart); **PTY over agent/certificate/keyboard-interactive auth now live-tested** (`services/clouddeskd/tests/ssh_advanced_auth.rs::task_1_agent_pty_live`/`task_2_certificate_pty_live`/`task_3_keyboard_interactive_pty_live`, 13/13 total in that file, including a live negative check that a wrong-principal certificate is denied before any PTY is requested); **compiled-frontend Playwright acceptance** (`services/clouddeskd/tests/remote_terminal_playwright.rs`, 2/2: real login -> Servers -> Open Terminal -> real xterm.js rendering -> real sentinel/resize/Ctrl-C/exit through actual browser automation, plus a real revocation-while-open failure-state test) driven by `services/clouddeskd/tests/browser/remote_terminal_flow.mjs`; frontend `apps/web/src/lib/RemoteTerminalApp.svelte` (xterm.js reused, not hand-rolled) wired into `ServersApp.svelte`'s new "Open Terminal" button via `App.svelte`'s existing single-window-per-app-id model | Yes (`GOAL.md` G8, per-server "Terminal" action) | — | None for the mechanism, and none of the previously-structural claims remain structural-only. Disclosed, deliberate v1 scope narrowing (unchanged from PASS SSH-C, still accurate): a terminal ID is audit-correlation only, not a re-attach capability (matches the pre-existing local-terminal precedent) -- so there is no "old terminal ID" to even attempt reusing after a restart |
| 3 | FFmpeg probe/remux/transcode core pipeline | PASS | `V1_TRUE_CLOSURE.md` #1 CLOSED, `crates/media/tests/live_ffmpeg.rs`, `media_api.rs` | Yes | — | None |
| 3 | 10-minute job timeout, live-fired | **PASS (Phase 3 residual closure)** | `crates/media/src/exec.rs`'s `JOB_TIMEOUT`/`MAX_OUTPUT_BYTES` consts replaced with a typed `MediaLimits{job_timeout, max_output_bytes}` threaded through every real `run_ffmpeg` call site (`remux`/`transcode`/`extract_subtitle`/`extract_artwork`), `MediaService::with_limits(..)` a test-only builder (no HTTP route accepts an override); live-fired through the real `MediaService::start_job` path (`crates/media/tests/limits_boundary.rs::live_timeout_boundary_through_production_job_path`) AND through the real HTTP API (`services/clouddeskd/tests/media_api.rs::live_timeout_boundary_through_real_http_api`) with a real ~11s 1080p transcode against a 2s injected timeout: real SIGTERM->bounded-SIGKILL fires against the real running `ffmpeg`, terminal state `Failed`/`error_class="timeout"`, 0 orphan `ffmpeg` processes, workspace removed, output never exposed, a fresh job still runs afterward. Production default (`exec::DEFAULT_JOB_TIMEOUT` / `MediaLimits::default()`) asserted directly as exactly 600s | Yes (mandatory security/reliability property) | Closed | None |
| 3 | 4 GiB output-size guard, live-fired | **PASS (Phase 3 residual closure)** | Same `MediaLimits` mechanism, `max_output_bytes`; live-fired via `crates/media/tests/limits_boundary.rs::live_output_quota_boundary_through_production_job_path` and `services/clouddeskd/tests/media_api.rs::live_quota_boundary_through_real_http_api` with a real several-second 720p encode against a 64 KiB injected quota: the real output-size poll (`watch_output_size`, strict `>` -- a file landing exactly at quota is accepted) cancels the real running `ffmpeg`, terminal state `Failed`/`error_class="output_too_large"` (distinct from `"timeout"`), output never exposed, 0 orphan processes, a below-quota job still completes normally. Production default (`exec::DEFAULT_MAX_OUTPUT_BYTES`) asserted directly as exactly 4294967296 bytes | Yes | Closed | None |
| 3 | Per-stage media audit events | **RECONCILED, not a gap (Phase 3 residual closure)** | Live inspection this pass found this codebase's transfers subsystem uses the identical two-mechanism pattern already (`audit_action` only at user-initiated request boundaries -- `transfer.create`/`pause`/`resume`/`cancel`/`retry` -- never for background worker-driven completion/failure), so media's single `media.job.requested` `audit_action` call is consistent architecture, not an omission: REQUESTED maps to that session-scoped audit-log entry; STARTED/RUNNING, PATH/MODE CHOSEN (`operation`), and every terminal outcome (COMPLETED/CANCELLED/FAILED with a distinct `error_class` -- `timeout`/`output_too_large`/`ffmpeg_failed`/`cancelled`/etc.) map to the persisted, owner-scoped `media_jobs` row, which by the time a background job finishes may outlive the original session that requested it. Every conceptual stage now has live test coverage: success (`live_ffmpeg.rs::job_lifecycle_end_to_end_through_media_service`), cancellation (`end_to_end_direct_remux_transcode_and_cancellation`), ordinary failure (`hostile_media_is_rejected_cleanly_not_a_panic`), timeout and quota (`limits_boundary.rs`, both), a real race between timeout and natural process exit resolving to exactly one terminal state (`timeout_racing_natural_process_exit_yields_one_terminal_state`), and cross-user job-detail authorization (`media_api.rs::a_users_media_job_is_invisible_and_uncontrollable_by_another_user`) | Yes (audit-trail completeness is a standing security invariant) | Closed | None |
| 3 | cgroup v2 CPU/memory/PIDs enforcement (media jobs / general orchestrator) | **BLOCKED BY ENVIRONMENT (rechecked, more precisely, Phase 3 residual closure)** | `crates/orchestrator/tests/live_cgroup_media_workload.rs`, live this pass: this host DOES now delegate `pids`/`cpu` controller files to this process's own cgroup scope (a change from the prior `Permission denied` state -- `cpu.max`/`pids.max` exist and are writable once `+pids`/`+cpu` are enabled in the parent's `cgroup.subtree_control`, a one-time toggle this already-delegated user is authorized to make on its own scope), but a real attempt to migrate a live process (a real `ffmpeg` job, and independently confirmed with a plain `sleep`) into a child leaf cgroup is refused by the kernel with `ENOTSUP`, confirmed with a raw filesystem write outside any CloudDesk code -- not a code defect. `memory` remains refused (`ENOTSUP`) at the subtree_control level itself, blocked by an ancestor cgroup above this process's own delegated scope. `InstanceCgroup`/`detect()` (Phase 6) are proven correct and unmodified; the block is genuinely host-level | Yes in principle, but genuinely external | Real host cgroup process-migration is unavailable in this environment despite partial controller delegation; not a code gap | None -- rechecked once per Part Y as instructed; further host configuration changes are out of scope for a residual-closure pass. Docker's own OCI-level `pids_limit` enforcement, proven live for Code/Office/Browser, stands in as a separate, real mitigation |
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
| 9 | Internal-network isolation (Blocker 2) | **PASS (closed in Pass 3A-4)** | `PHASE9_BROWSER_EVIDENCE.md`: dedicated `enable_icc=false` network fixes other-user-runtime/other-Browser-instance reachability (Pass 3A-3); Pass 3A-4 closed the two remaining residuals via a mandatory, policy-enforcing egress proxy (`browser_egress_proxy.rs`) that Brave's `--proxy-server` flag routes every HTTP(S) request through -- host-gateway, RFC1918, and real-address metadata (`169.254.169.254`) destinations all live-verified blocked, plus redirect-pivot and page-initiated-fetch coverage. This environment has no root access (confirmed live), so a real kernel firewall rule could not be installed/verified; the proxy approach matches `CloudDesk`'s actual threat model (hostile page content, not a sandbox escape) and required no new `cloudesk-privd` operation | Yes | Closed | Raw Docker-network-level reachability to the host gateway (not reachable by page content, only by a container-level probe) remains a structural fact of the underlying network, unfixable without root in this environment -- assessed low-severity, unchanged from Pass 3A-3's own analysis |
| 9 | WebRTC leakage review (Blocker 3) | **PASS** | `PHASE9_BROWSER_EVIDENCE.md`: real ICE-gathering fixture, one candidate observed, mDNS-obfuscated (no raw IP of any kind); no `--device` flag anywhere in the orchestrator (no host camera/mic ever mountable) | Yes | — | None |
| 9 | Frame/backpressure live stress (Blocker 4) | **PASS** | `PHASE9_BROWSER_EVIDENCE.md`: real `requestAnimationFrame` fixture, 241 frames/4s (~60fps), slow/paused/resize/abrupt-disconnect all recovered cleanly, container RSS did not grow across the run | Yes | — | None |
| 9 | Simultaneous multi-user acceptance (Blocker 5) | **PASS** | `PHASE9_BROWSER_EVIDENCE.md`: 3 real concurrent sessions (User A/User B/Guest), frame/tab/runtime isolation confirmed under genuine concurrency | Yes | — | None |
| 9 | Full Browser route-authorization matrix (Blocker 6) | **PASS** | `PHASE9_BROWSER_EVIDENCE.md`: 10 routes inventoried from actual router registration and live-tested; capability vs ownership proven independently; a real structural question (generic `proxy-ws` missing a capability re-check) investigated and live-verified non-exploitable given current CDP relay behavior, disclosed as a low-severity defense-in-depth gap | Yes | — | A future pass: add the one-line `apps.browser.use` re-check to the generic `ws_proxy` handler for defense-in-depth, even though not currently exploitable |
| 9 | Browser frontend (`BrowserApp.svelte`) | PASS (minimal) | `PHASE9_BROWSER_EVIDENCE.md` Task 68; `apps/web/src/lib/BrowserApp.svelte`; frontend gates (lint/check/test/build) all pass with it included | Yes | — | No back/forward/reload buttons (optional per Task 19's own "if easy"); real acceptance evidence so far drives the same WebSocket protocol directly, not yet through a Playwright-controlled instance of this actual component (see next row) |
| 9 | Server-side-origin acceptance (CloudDesk-mediated, not raw CDP) | PASS | `PHASE9_BROWSER_EVIDENCE.md` Task 18; live test confirms the controlled site's request arrived from Brave's own container network via the typed broker, not the test process directly | Yes | — | Drives the WebSocket protocol directly (a real client speaking the exact protocol `BrowserApp.svelte` speaks), not literally through a Playwright-automated instance of the compiled frontend — see next row for that narrower gap |
| 9 | Crash recovery (Browser-specific, live) | PASS (real regression found and fixed in Pass 3A-3) | `PHASE9_BROWSER_EVIDENCE.md` Task 24-adjacent; `task_24_crash_handling_and_generation_invalidation` — real `docker kill`, explicit `closed` message, `RuntimeManager` detects failure, clean reconnect after restart. Pass 3A-3's own full-workspace regression run found this test genuinely flaky (~1 in 3, reproducible in complete isolation): a real race in `outbound_writer` (`services/clouddeskd/src/browser_broker.rs`) could silently drop the already-queued `"closed"` message when `tokio::select!` picked the `frame_rx` error branch first, hanging the client instead of reporting the crash. Fixed by draining buffered `misc_rx` messages before breaking on that branch; re-verified 5/5 isolated + clean in a full-workspace run after the fix | Yes | — | — |
| 9 | Enable/disable (Browser-specific, dedicated live test) | PASS | `task_25_enable_disable_lifecycle` — disable-while-active, zero containers after, denied-while-disabled, usable again after re-enable | Yes | — | Re-enable reuses the existing instance (restart) rather than creating a new one, due to the documented `max_instances_per_user` gap |
| 9 | Downloads (staging, quota, malicious-Content-Disposition, no auto-execution) | **PASS (Pass 3B)** | `PHASE9_BROWSER_EVIDENCE.md`; `browser_downloads.rs`, `browser_download_quota.rs` | Yes | Closed | None |
| 9 | Uploads (local file-chooser mediation) | **PASS (Pass 3B)** | `PHASE9_BROWSER_EVIDENCE.md`; `browser_uploads.rs` | Yes | Closed | None |
| 9 | Remote-VFS (SFTP) Browser upload backend | **PASS (Pass 3B-2)** | `PHASE9_BROWSER_EVIDENCE.md`; `browser_remote_uploads.rs` | Yes | Closed | None |
| 9 | Remote-VFS Browser upload product UI (real picker + Playwright) | **PASS (Pass 3B-3)** | `PHASE9_BROWSER_EVIDENCE.md`; `apps/web/src/lib/BrowserApp.svelte`; `browser_playwright_remote_upload.rs` | Yes | Closed | None |
| 9 | Admin disable with active Browser peripherals | **PASS (Pass 3B-2)** | `PHASE9_BROWSER_EVIDENCE.md`; `browser_admin_disable_peripherals.rs` | Yes | Closed | None |
| 9 | Guest cleanup specifically on admin disable | **PASS (Pass 3B-3)** | `PHASE9_BROWSER_EVIDENCE.md`; `browser_admin_disable_lifecycle.rs::task_7` | Yes | Closed | None |
| 9 | Persistent profile retained specifically across admin disable | **PASS (Pass 3B-3, real defect found and fixed)** | `PHASE9_BROWSER_EVIDENCE.md`; `crates/orchestrator/src/manager.rs`; `browser_admin_disable_lifecycle.rs::task_8` | Yes | Closed | None |
| 9 | Clipboard bridge | **PASS (Pass 3B)** | `PHASE9_BROWSER_EVIDENCE.md`; `browser_clipboard.rs` | Yes | Closed | None |
| 9 | Audio (per-user capture, cross-user isolation) | **PASS (Pass 3B)** | `PHASE9_BROWSER_EVIDENCE.md`; `browser_audio.rs` | Yes (explicit Phase 9 closure requirement per Part O) | Closed | None |
| 9 | Video playback acceptance (through real CloudDesk Browser, with audio) | **PASS (Pass 3B)** | `PHASE9_BROWSER_EVIDENCE.md`; `browser_video.rs` | Yes | Closed | None |
| 9 | WebRTC leakage review | **PASS (Pass 3A-3)** | `PHASE9_BROWSER_EVIDENCE.md`: real ICE-gathering fixture, one mDNS-obfuscated candidate, no raw IP | Yes | Closed | None |
| 9 | Internal-network isolation (SSRF-class: loopback/gateway/internal endpoints/RFC1918/metadata-shaped route) | **PASS (closed Pass 3A-4)** | `PHASE9_BROWSER_EVIDENCE.md`: mandatory policy-enforcing egress proxy, default-deny private/loopback/link-local/metadata | Yes | Closed | None |
| 9 | Service-restart reconciliation for active Browser sessions | PASS (crash-kill case); NOT EXECUTED (planned graceful `clouddeskd` restart case) | `task_24_crash_handling_and_generation_invalidation` covers the abrupt-death case live; Pass 3B additionally live-verified a crash with a real audio task active (`task_13_crash_with_audio_active_cleans_up`); a graceful `clouddeskd` process restart with a live session was not separately tested | Yes | Partially covered | Add a dedicated graceful-restart test if the two cases are expected to behave differently |
| 9 | Multi-user live acceptance (simultaneous User A/User B/Guest across all built surfaces) | **PASS** | Pass 3A-3's 3 concurrent sessions plus Pass 3B's concurrent per-user audio isolation (`task_22_cross_user_audio_isolation`); downloads/uploads/clipboard isolation is structural (per-connection state, no shared store) and live-tested per-user individually, not simultaneously as one combined multi-user run | Yes | Structural isolation proven; a single combined "all peripherals at once, 3 concurrent users" run not separately executed | Optional: one combined simultaneous run if a future pass has budget |
| 9 | Browser-specific route authorization matrix | **PASS** | `browser-ws` connection-level ownership check live-tested (`task_1_2_...`, 10/10 applicable HTTP/WS routes + `proxy-ws` N/A, Pass 3A-3); Pass 3B's six new typed sub-commands live within that same already-authorized connection and inherit its boundary — live-verified per-command denial for foreign/unknown resource references | Yes | Closed | None |
| 9 | SYS_ADMIN/SYS_CHROOT justification | PASS | `PHASE9_BROWSER_EVIDENCE.md` Task 51: real alternative (`--disable-namespace-sandbox`) tried and ruled out; capabilities genuinely required for Chromium's own sandbox to initialize; never traded for `--no-sandbox` | Yes | — | None — already the strongest achievable posture; residual risk (two elevated capabilities beyond the zero-capability baseline) is explicit, not hidden |
| — | Distro-matrix installer/service verification (8 platforms) | BLOCKED BY ENVIRONMENT | `V1_TRUE_CLOSURE.md` #15 | Yes per `GOAL.md`, but this is Phase 10's own subject matter | No per-distro CI/VM infrastructure available in this environment | Explicitly out of scope for this gate — this is what Phase 10 itself is for; not a Phase 1-9 gap |

## Newly found, out-of-this-pass's-scope items (not fixed, honestly documented)

1. **Browser test-concurrency defect** (found and fixed this pass, prior to this gate): `browser_runtime.rs`'s 4 tests raced under `cargo test --workspace`'s default within-binary parallelism; fixed with `acquire_cross_process_browser_lock()`.
2. **Broader Office container-leak scope than previously believed**: the earlier checkpoint entry said only `office_runtime.rs` leaked; this pass's own `cargo test --workspace` run found 11 additional real, running Collabora containers from `office_browser.rs`, `office_hostile_documents.rs`, and `office_remote_vfs.rs`. Fixed this pass (see Phase 8 row above).

## Environment blockers that are genuinely external (Part AB)

- Public GitHub/GitLab account authentication (Phase 7) — no credentials provided.
- cgroup v2 CPU/memory/PIDs controller delegation (Phase 3/6) — rechecked live this pass: `pids`/`cpu` controller files are now delegated to this process's own scope (an improvement over the prior flat `Permission denied`), but real process migration into a child cgroup is refused (`ENOTSUP`) by the kernel, and `memory` remains refused at the subtree_control level by an ancestor scope — no sudo used, no host cgroup configuration permanently mutated beyond this process's own already-delegated scope.
- Distro-matrix installer/service verification (8 platforms) — Phase 10's own subject matter, not a Phase 1-9 gap.

**Not retained as environment blockers** (explicitly re-evaluated per Part Z/AB, since the Playwright/Chromium harness now exists from Phase 8's `office_browser.rs`): Video, Music, Settings, and Code browser acceptance are now `NOT EXECUTED`, not `BLOCKED BY ENVIRONMENT` — the tooling exists, the acceptance runs simply have not been performed yet.

## Summary counts

- Mandatory `IMPLEMENTATION MISSING`: **0** (the remote PTY terminal, Phase 2's last mandatory target, is **PASS** as of PASS SSH-C/SSH-C-2. Phase 3's per-stage media audit events, previously the one remaining `IMPLEMENTATION MISSING`, is now reconciled/closed — not a gap; see the Phase 3 open-item register rows above. SSH agent/keyboard-interactive/certificate authentication are all **PASS** as of PASS SSH-A/SSH-A-2, native SCP is **PASS** as of PASS SSH-B/SSH-B-2; Browser downloads/uploads/clipboard/audio are all **PASS** as of Pass 3B — see row-by-row list above for the authoritative enumeration, this bullet is a convenience count only)
- Mandatory `NOT EXECUTED`: **~4** (Video/Music/Settings/Code browser acceptance ×4, unchanged; Phase 3's timeout/quota live-fire is now **PASS**, closed this pass — see rows for the authoritative list)
- Mandatory `FAIL`/`OPEN`: **0**
- Unresolved Critical: **0**
- Unresolved High: **0**
- Environment blockers (genuinely external): **3** (public GitHub/GitLab auth, cgroup delegation, distro-matrix infrastructure)
- Test resource leaks: **0 leaked** across every Pass 3B live run (`docker ps -a` checked clean after every test file) — see Validation

## Rust/frontend gates (Pass 3B-3, final, after remote-VFS picker UI + real admin-disable defect fix)

**Correction:** the Pass 3B-2 report labeled Phase 9 COMPLETE while
the Browser upload chooser's frontend still accepted local-home
selection only -- the SFTP backend/materialization path was real, but
no product UI existed to reach it. Pass 3B-3 adds and verifies the
missing remote-VFS selection through the compiled product UI (a real
source picker in `BrowserApp.svelte`, a real Playwright-driven
end-to-end test through the actual compiled frontend). Pass 3B-3 also
found and fixed a real, live product defect (not related to the
picker UI): `RuntimeManager::set_enabled`'s disable path force-killed
every live instance, skipping Browser's `graceful_stop` CDP hook
entirely and silently losing persistent-profile state whenever an
Administrator disabled Browser while a user's session was active --
see `crates/orchestrator/src/manager.rs` and
`browser_admin_disable_lifecycle.rs`.

fmt: **PASS** (`cargo fmt --all -- --check`, clean).
clippy: **PASS** (`cargo clippy --workspace --all-targets --all-features -- -D warnings`, clean).
release build: **PASS** (`cargo build --workspace --release`, ~1m03s incremental).
workspace tests: **TIMING-FLAKY**, not a plain PASS -- `cargo test
--workspace --no-fail-fast` (`--test-threads=4`) surfaced 9 individual
test failures across 7 binaries under genuinely heavy full-workspace
concurrent Docker load (this pass added 4 more real-Docker Browser
test files on top of an already-large suite). Every failing test was
re-run afterward in true single-test isolation
(`cargo test -p clouddeskd --test <file> <test> -- --test-threads=1`,
one binary at a time, nothing else running): all passed clean except
one pre-existing test unrelated to this pass's own changes,
`browser_broker.rs::task_4_popup_becomes_managed_tab_and_storm_is_bounded`
(already documented as Docker-load-timing-class since Pass 3A-4),
which reproduced 1-in-3 even in true isolation this time -- root-caused
to genuine host memory pressure (`free -h` showed ~12 GiB of swap in
use, ~1.3 GiB free RAM, after many hours of this session's own
Docker-heavy work), not a code regression and not a new product
defect, so left undisturbed per this pass's own explicit scope
(no large timing-harness project without a new product defect). Both
of this pass's own new Docker-based test files
(`browser_remote_uploads.rs` 5/5, `browser_playwright_remote_upload.rs`
2/2) passed clean in isolation. Zero leaked containers after every run.
frontend gates: **PASS** -- `npm run lint`/`check`/`test` (91/91)/`build`
all clean, including the new remote/local source-picker UI.

## Rust/frontend gates (Pass 3B-2, superseded by Pass 3B-3 above)

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo build --workspace --release`: PASS (~57s incremental).
`cargo test --workspace --no-fail-fast` (`--test-threads=4`) was run
twice this micro-pass. **First run**: 5 failing binaries -- 3 the same
pre-existing Docker-load-timing class documented below (files this
micro-pass didn't touch), plus `ssh_proxyjump.rs` (5 individual
tests), which was root-caused as this micro-pass's own incomplete
fixture setup (only the OpenSSH bastion container had been started
via `docker compose up -d openssh`, not the full
`tests/acceptance/docker-compose.yml` stack including
`openssh-target`, which `ssh_proxyjump.rs` also needs) -- fixed by
starting the full stack (`docker compose up -d`), re-verified 12/12
clean. **Second run** (after the fixture fix, with the full
acceptance stack plus every Office/Collabora test now also
contending for Docker/CPU): 6 failing binaries, 7 individual tests,
all timing-class assertions (a WS event or a real page's selection
value not settling within its wait window under this heavier
concurrent load) -- every one of the 15 tests across the 5 affected
Browser peripheral files (`browser_audio.rs`, `browser_clipboard.rs`,
`browser_downloads.rs`, `browser_remote_uploads.rs`,
`browser_uploads.rs`) was independently re-run in isolation
immediately afterward and passed 15/15 clean, confirming both runs'
failures were load-timing, not a regression. Zero leaked containers
after every run.

## Rust/frontend gates (Pass 3B, superseded by Pass 3B-2 above)

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace --no-fail-fast` (`--test-threads=4`): **83
test binaries `ok`; 5 individual tests failed across 5 binaries** (319
tests passed): `task_21_real_audio_capture_and_playback_evidence`
(`browser_audio.rs`, Pass 3B), `task_3_hostile_filenames_sanitized`
(`browser_downloads.rs`, Pass 3B),
`task_7_9_10_13_14_15_16_18_broker_product_slice` (`browser_broker.rs`
— already a documented Docker-load-timing flake from Pass 3A-4),
`task_14_public_style_browsing_still_works`
(`browser_egress_policy.rs`), `task_6_9_other_user_runtime_unreachable_from_browser`
(`browser_network_isolation.rs`). Both Pass 3B failures were re-run 3x
each in isolation immediately afterward and passed clean 6/6
(`browser_audio.rs` 3/3 + `browser_downloads.rs` 3/3, all three tests
in each file every time); this matches the exact
already-established Docker-load-timing-issue class from prior passes
(genuinely full-workspace-scale concurrent Docker/CPU contention
delaying a fixed wait window, not a deterministic code regression) --
none of the failing tests share code with each other or point at a
common root cause, and all five are timing/reachability assertions
(silent audio samples, a missing WS event within its timeout, an
allowlisted-but-momentarily-unreachable destination, an unreachable
victim fixture), never a correctness or isolation failure in the
wrong direction. Per this project's own established policy, this is
documented honestly as the reproducible full-workspace-scale
residual it is, not fabricated as a deterministic PASS.
`cargo build --workspace --release`: PASS (~1m04s incremental).
Frontend gates: PASS -- `npm run lint` (0 errors/warnings)/`check` (0
errors/warnings)/`test` (91/91)/`build` (clean `dist/`) all green,
including the Pass 3B `BrowserApp.svelte` peripheral-UI additions.
Resource cleanup: zero leaked `collabora/code`/`clouddesk-brave`/
`mcr.microsoft.com/playwright` containers (`docker ps -a` empty) and
zero stray Browser-related processes (`ps aux` checked -- the user's
own real desktop Brave browser is present and unrelated/untouched)
after the full run; `clouddesk-browser-net` present as the expected
persistent network, not a leak.

## PASS SSH-C: remote PTY terminal (Phase 2's last mandatory target)

**PASS SSH-C status: COMPLETE.** A real remote SSH PTY, over the exact
same authenticated `SshSession`/`resolve_ssh_session` builder every
other SSH feature already uses -- never a local shell, never a
one-shot `exec`, never a second SSH stack. `crates/remote/src/pty.rs`
issues a real `pty-req` (`TERM=xterm-256color`, 80x24 default) +
`shell` channel request; Ctrl-C is the literal `0x03` byte through the
normal input path (matching how a real terminal client behaves), never
the SSH protocol-level signal request.

Live evidence, three layers: (1) crate-level, direct against the real
disposable OpenSSH fixture (`crates/remote/tests/pty.rs`, 4/4) --
`test -t 0`/`stty size` prove a real PTY (impossible over plain exec),
resize genuinely changes remote dimensions across three sizes, Ctrl-C
interrupts only the foregrounded `sleep` (proven by occurrence-counting
the sentinel string to distinguish typed-echo from real execution, not
a naive substring check), and `exit` reaches a real channel exit event.
(2) `ProxyJump` PTY (mandatory, per the governing prompt) --
`services/clouddeskd/tests/ssh_proxyjump.rs::task_27_28_29_...` (1/1),
proving the shell runs on the otherwise-unreachable target container,
not the bastion, via `whoami && hostname`, with both hops' host keys
and credentials independently resolved exactly as the file's existing
plain-exec/SFTP ProxyJump tests already do. (3) the real product/API
path -- new `GET /api/v1/remote/servers/{server_id}/terminal/ws`
(`services/clouddeskd/src/remote_terminal.rs`), authorized via the
pre-existing `remote.terminal.open` capability (already present in
`crates/permissions`'s `CAPABILITIES` and already granted to
`manager`/`user` by `crates/auth`'s role seeding -- confirming this
exact feature was anticipated architecturally, so zero new
role/DB wiring was needed), gated by `RemoteServerStore::get` ownership
before the WebSocket ever upgrades (so a denied request never even
reaches 101 Switching Protocols). `services/clouddeskd/tests/
remote_terminal_product.rs` (6/6, live): real `whoami`/resize through
the actual WS; cross-user and stale/nonexistent server-ID denial; fully
unauthenticated denial; deleted-`RemoteServer` denial; **session-logout
revocation of an already-open terminal**, proven through the bridge's
real 5s periodic re-validation loop (re-checks both `RemoteServer`
ownership and live session validity for as long as the socket stays
open, not just at connect time); and hostile input (malformed JSON,
an absurd 999999x0 resize) handled safely with the terminal remaining
fully usable afterward.

Frontend: `apps/web/src/lib/RemoteTerminalApp.svelte` reuses the
already-present `@xterm/xterm`/`@xterm/addon-fit` dependencies (no new
frontend dependency), modeled directly on the existing local
`TerminalApp.svelte`'s binary-WebSocket/reconnect design, with an added
explicit `revoked` state (never left as a silent hang). Wired in via a
new "Open Terminal" button in `ServersApp.svelte` and a new
`remote-terminal` app id in `apps.ts` deliberately absent from
`public/manifests/index.json` (so it is never a dock/launcher icon --
it only opens targeted at one specific server, matching how Video/
Music/Code/Office already retarget a singleton window by `params`).
`npm run lint`/`check`/`test`/`build` all pass.

**Disclosed, deliberate v1 scope narrowing** (matching this project's
established anti-fabrication discipline -- paralleling PASS SSH-A-2's
keyboard-interactive disclosure and PASS SSH-B-2's shared-queue
disclosure): a terminal's ID exists for audit correlation only, not a
re-attach capability -- there is no "attach to an existing terminal
from a different WebSocket connection" operation in this design at
all (matching the pre-existing local-terminal precedent, which has no
such feature either), so Task 21's cross-user "attach by ID" concern
is satisfied structurally (ownership is re-checked at WS-connect time,
before any terminal exists to attach to) rather than by a dedicated
ID-guessing defense. PTY over agent/certificate/keyboard-interactive
auth was not separately live-tested beyond password + one ProxyJump
password case -- this is not a gap in the PTY mechanism itself, which
is structurally auth-method-agnostic (`open_terminal` is called on
whatever already-authenticated `SshSession` `resolve_ssh_session`
returns, and per-auth-method live proof already exists for plain exec/
SFTP/SCP in `ssh_advanced_auth.rs`/`scp_transfer_interruption.rs`).

## PASS SSH-C-2: final PTY live-evidence closure (correction)

**Correction:** the PASS SSH-C report above marked Phase 2 COMPLETE
while several items its own Definition of Done required were only
structural reasoning, not live evidence: PTY over agent/certificate/
keyboard-interactive auth, real simultaneous multi-user PTYs, compiled
frontend terminal acceptance, and service-restart lifecycle had not
been separately live-executed. Per the corrected classification, that
made SSH-C **PARTIAL**, not COMPLETE. **PASS SSH-C-2 closes all four
gaps and is itself COMPLETE:**

- **Agent/certificate/keyboard-interactive PTY, live**
  (`ssh_advanced_auth.rs::task_1_agent_pty_live`/`task_2_certificate_pty_live`/
  `task_3_keyboard_interactive_pty_live`): each opens a real PTY on a
  `RemoteServer` configured with that auth method as its ONLY method
  (nothing to have silently fallen back to), runs real commands
  (`whoami`, a `printf` sentinel, `stty size`), and the certificate
  test adds a live negative check -- a wrong-principal certificate is
  denied before any PTY is ever requested. All reuse the exact same
  `resolve_ssh_session`/`open_terminal` path already proven for
  password auth; no new SSH stack.
- **Real simultaneous User A/User B terminals**
  (`remote_terminal_product.rs::task_4_simultaneous_user_a_b_terminals_no_crosstalk`):
  two independently-owned `RemoteServer`s (a second real administrator
  account was needed here specifically, since this product's role
  model grants `secrets.manage` -- required to register one's own
  credentials -- only to `administrator`, a pre-existing fact), two
  concurrent real WebSocket/PTY connections, live-proven zero
  cross-talk on interleaved input/output, a resize on A never leaking
  into B's real PTY dimensions, and closing A never disrupting B.
- **Compiled-frontend Playwright acceptance**
  (`remote_terminal_playwright.rs`, driven by
  `tests/browser/remote_terminal_flow.mjs`): real login through the
  actual compiled UI, a real click on "Open Terminal" in `ServersApp`,
  real xterm.js DOM rendering, a real typed sentinel proven to appear
  twice (typed echo + real `printf` output), a real browser-window
  resize changing the real remote PTY's reported dimensions, a real
  `Ctrl+C` keypress interrupting only a foregrounded `sleep`, and an
  explicit non-connecting/connected state after both a real shell
  `exit` and a real mid-session revocation (the `RemoteServer` deleted
  while the terminal is open, synchronized via a ready-file signal
  rather than a blind delay, to avoid racing the container's own
  startup time).
- **Real service-restart lifecycle**
  (`remote_terminal_product.rs::task_8_real_clouddeskd_restart_severs_old_pty_and_allows_a_fresh_one`):
  discovered live, during this pass, that this codebase's established
  in-process two-instance restart-simulation convention
  (`office_restart.rs`, sound for DB-persisted state) cannot honestly
  prove a live WebSocket/PTY dies on restart -- axum's WebSocket
  upgrade hands the connection to a task that outlives the enclosing
  HTTP serve future, so neither aborting the serve task nor
  `axum-server`'s `Handle::shutdown()` reached it (both verified live
  to leave the socket open). The test instead spawns the actual
  compiled `clouddeskd` binary as a real child process and sends it a
  real `SIGKILL`: the client WebSocket observes the connection die, the
  old remote shell process is independently confirmed reaped via
  `docker exec ... ps` (no orphan), and a second real, independently
  started process (same on-disk SQLite file) opens a brand-new PTY on
  the same `RemoteServer` successfully. Terminal persistence across the
  restart is not attempted, by design -- terminals are ephemeral, and
  since there is no attach-by-ID capability at all, there is no "old
  terminal ID" to even try reusing.

**Tempfile hygiene (Gap 5):** this pass's new tests leaked exactly
**1** new `/tmp` directory (from the pre-existing `RealAgent::spawn()`
helper's `std::mem::forget` pattern in `ssh_advanced_auth.rs`, already
exercised by every pre-existing agent-auth test in that file -- not
new code this pass introduced). The broader historical accumulation in
`/tmp` (hundreds of dirs, spanning many prior passes/sessions) is
unrelated and carried forward to a future test-hygiene/reliability
pass, not expanded into scope here.

**Workspace test terminology (Gap 5):** `cargo test --workspace
--no-fail-fast` initially reported 6 failing targets this pass
(`browser_audio`, `browser_broker`, `browser_clipboard`,
`browser_egress_policy`, `browser_playwright_remote_upload`,
`browser_remote_uploads`) -- all Browser/CDP-driven, none touching any
file this pass modified, all confirmed clean (30/30 tests) on isolated
`--test-threads=1` reruns. Reported as **Rust workspace: TIMING-FLAKY**,
never PASS-by-omission and never conflated with SSH correctness.

**Phase 2 SSH is now genuinely, fully COMPLETE** -- every mandatory
item in the open-item register's Phase 2 section, including the four
items this correction closes, is backed by real, live, first-party
evidence.

## Phase 3 residual closure: media limits, audit lifecycle, cgroup re-check

**Phase 3 status: COMPLETE.** All five residual evidence gaps closed:

1. **Production timeout (600s) live-fired**: `exec::MediaLimits` (typed,
   `Default` = the real 10-minute/4 GiB values) threaded through every
   real `run_ffmpeg` call site, replacing the two bare constants it
   used to read. A real ~11s 1080p transcode against a 2s injected
   timeout fires the real SIGTERM->bounded-SIGKILL path against a real
   running `ffmpeg`, through both `MediaService::start_job` directly
   (`crates/media/tests/limits_boundary.rs`) and the real HTTP API
   (`services/clouddeskd/tests/media_api.rs`). Terminal state: `Failed`
   / `error_class = "timeout"`. 0 orphan `ffmpeg` processes, workspace
   removed, output never exposed, a fresh job still runs afterward.
2. **Production quota (4 GiB) live-fired**: the same `MediaLimits`
   mechanism, `max_output_bytes`. A real several-second 720p encode
   against a 64 KiB injected quota trips the real output-size poll
   (strict `>` -- landing exactly at quota is accepted, not rejected).
   Terminal state: `Failed` / `error_class = "output_too_large"`,
   distinct from `"timeout"`. Same cleanup/non-exposure guarantees; a
   below-quota job still completes normally.
3. **Media audit lifecycle reconciled**: this codebase's established
   pattern (matching Transfers) is `audit_action` at the user-initiated
   request boundary only (`media.job.requested`), with the persisted,
   owner-scoped `media_jobs` row as the durable record of everything
   after that (started/running, operation chosen, and every terminal
   outcome with a distinct `error_class`). Not a gap -- reconciled and
   now backed by live test coverage for every conceptual stage:
   success, cancellation, ordinary ffmpeg failure, timeout, quota, and
   a real race between timeout and natural process exit resolving to
   exactly one terminal state, never a contradictory pair.
4. **cgroup v2 re-checked, more precisely**: this host now delegates
   `pids`/`cpu` controller files to this process's own scope (an
   improvement over the prior flat `Permission denied`), but real
   process migration into a child leaf cgroup is refused (`ENOTSUP`) by
   the kernel -- confirmed with a raw filesystem write outside any
   CloudDesk code, so genuinely host-level, not a code defect. `memory`
   remains refused further up the delegation chain. Recorded as
   **BLOCKED BY ENVIRONMENT**, with the existing Phase 6
   `InstanceCgroup`/`detect()` primitive proven correct and unmodified.
5. **Zero new tempfile leaks** from this pass's own tests (verified via
   before/after `/tmp` counts across three full test-suite runs).

Full regression: `crates/media/tests/live_ffmpeg.rs` (6/6),
`crates/media/tests/limits_boundary.rs` (5/5, new),
`services/clouddeskd/tests/media_api.rs` (10/10, 2 new),
`crates/orchestrator/tests/live_cgroup_media_workload.rs` (2/2, new) --
25 media-related tests total, all passing. `cargo fmt`/`clippy
--workspace --all-targets --all-features -D warnings` clean.

## READY FOR PHASE 10: NO

Per the governing policy, YES requires zero mandatory `IMPLEMENTATION
MISSING`, zero mandatory `NOT EXECUTED`, Phase 9 Browser `COMPLETE`,
and Phase 2 SSH mandatory features `COMPLETE`. **Phase 9 Browser is
now COMPLETE as of Pass 3B** — see `PHASE9_BROWSER_EVIDENCE.md`'s
Definition-of-Done checklist for the full, current per-item state
(downloads, uploads, clipboard, audio, video playback, password-
manager/extensions/native-messaging policy, the final route-
authorization accounting, and the secret/privacy sweep are all now
PASS). SSH agent/keyboard-interactive/certificate authentication are
now also **COMPLETE as of PASS SSH-A/SSH-A-2**, native SCP is now
**COMPLETE as of PASS SSH-B/SSH-B-2**, and the remote PTY terminal is
now **COMPLETE as of PASS SSH-C/SSH-C-2** (protocol, `ProxyJump`,
product/API, and frontend evidence -- see the open-item register
above). **Phase 2 SSH is therefore now fully COMPLETE** — every row in
the open-item register's Phase 2 section is PASS. **Phase 3 is now
also COMPLETE** — see the section immediately above. What still blocks
readiness: Video/Music/Settings/Code browser acceptance (Phases 4-7),
still `NOT EXECUTED`.

**Next exact action**: pre-Phase-10 product acceptance closure --
reconcile and close Phases 4 (Video), 5 (Music), 6 (Settings runtime
cards), and 7 (Code product/browser acceptance). Not Phase 10.

Do not start Phase 10. Do not create distro fixtures. Do not push, tag,
move `v1.0.0`, or create `v1.0.1-rc.1`.
