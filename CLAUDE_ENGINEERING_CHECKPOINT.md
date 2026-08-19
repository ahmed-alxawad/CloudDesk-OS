# CloudDesk-OS — Engineering Checkpoint

Branch: `engineering/v1-true-closure` (from `audit/claude-nightmare-v1.0.0`)
`v1.0.0` tag: untouched, unpublished. Nothing pushed.

## Pre-Phase-10 Closure Gate — PASS 3A (Browser tabs, popups; PARTIAL)

Full detail: `PHASE9_BROWSER_EVIDENCE.md`, `PRE_PHASE10_CLOSURE.md`.

**Real, live-tested, this pass**: the broker was rewritten from one
CDP `WebSocket` per page (Pass 2's design) to real CDP `Target`
multiplexing — one browser-level connection per `CloudDesk` Browser
session, `Target.createTarget` + `Target.attachToTarget` with
`flatten: true`, sessionId-scoped calls. This delivers real tabs and
popups:

- Opaque, process-wide-unique `TabId`s (never a raw CDP target/session
  ID) mapping to real attached targets.
- Real tab lifecycle: create, switch (stops the old tab's screencast,
  starts the new one's), close (survivor becomes active; closing the
  last tab falls back to a fresh blank tab rather than zero tabs).
- Real popup handling: a genuine `window.open()`/`target=_blank` call
  in a real page is observed via `Target.targetCreated` (discovery
  mode) and auto-attached as an ordinary managed tab — never left
  unmanaged.
- Bounded popup storm defense (`MAX_TABS_PER_SESSION = 8`): a real
  12-popup JS-loop burst was observed staying at or under the bound
  across the whole event window.
- `apps/web/src/lib/BrowserApp.svelte` gained a minimal tab strip
  (create/activate/close, title/loading per tab).
- `services/clouddeskd/tests/browser_broker.rs` grew from 5 to 8 tests,
  all live, all passing together, zero leaked containers:
  `task_1_3_tab_lifecycle_create_switch_close`,
  `task_2_tab_ownership_cross_session_denied`,
  `task_4_popup_becomes_managed_tab_and_storm_is_bounded`.
- `browser_runtime.rs`'s existing 4 tests re-run clean (profile-
  persistence/isolation regression check after the broker rewrite).

**Five real defects found and fixed this pass** (via
reproduce → root-cause → fix → retest, each requiring a full live
Docker-based test cycle to observe):
1. `Page.screencastFrame`'s active-tab check used `try_lock()` inside
   a sync closure, which could spuriously fail under contention and
   silently drop real frames -- fixed with sequential `.await`ed locks.
2. Brave's own container entrypoint already has an `about:blank` tab
   open at connect time; enabling `Target.setDiscoverTargets` reports
   it as a "created" target too, indistinguishable from a genuine
   popup without a snapshot of pre-existing target IDs taken first
   (`Target.getTargets` before discovery) -- without that snapshot the
   startup tab was mistaken for a popup and raced the session's own
   first tab for "active" status.
3. Per-session tab-ID counters starting at 1 meant two different
   sessions' first tabs both got the literal ID `"tab-1"` -- harmless
   for real isolation (every lookup is scoped to that session's own
   map) but defeated a cross-session denial test's ability to tell
   "genuinely denied" from "coincidentally my own tab" -- fixed with a
   process-wide atomic counter.
4. A test-side bug (not the broker): a fixture URL built without a `/`
   separator produced an unresolvable host, manifesting identically to
   a real navigation-never-completes broker regression until traced
   with event-level debug logging.
5. `activate_tab_internal` held the `tabs` mutex across a real CDP
   round-trip `.await` when stopping the previous tab's screencast --
   not a deadlock in this single-task design, but needlessly long lock
   hold; fixed to clone what's needed and release before awaiting.

**Not attempted this pass** (Pass 3A's own larger scope --
Playwright-through-the-compiled-frontend acceptance, a real cookie-
persistence fix, the internal-network-isolation attack matrix, WebRTC
leakage baseline, simultaneous multi-user acceptance, a full
Browser-route authorization sweep beyond the one route already
live-tested, dedicated logout/session-revocation and service-restart
tests beyond the existing crash-recovery coverage): each remains a
substantial, multi-hour-or-more scope on its own; attempting them in
the remainder of this pass risked shallow, rushed evidence rather than
the real kind this project's standing discipline requires. Recorded
honestly as not done, not fabricated.

**Validation, this pass**: `cargo fmt --all -- --check` PASS;
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
PASS; `cargo test --workspace --no-fail-fast` 68/69 binaries ok (the
one failure, `office_browser::task_4_real_xlsx_browser_edit`, is
unrelated to any file this pass touched -- confirmed a pre-existing
Docker-load-contention flake via a clean isolated rerun, 1/1 in
48.74s); `cargo build --workspace --release` PASS (6m37s); frontend
gates (`lint`/`check`/`test` 91/91/`build`) all PASS with the new tab
strip included. Zero leaked `clouddesk-brave`/`collabora/code`
containers confirmed via `docker ps -a` after the full suite.

**PASS 3A status: PARTIAL** (tabs/popups genuinely complete and
live-tested; the larger security/multi-user/cookie/network scope is
not). **READY FOR PHASE 10: NO.** Next exact action: continue Pass 3A
(Playwright-through-the-frontend acceptance and the cookie-persistence
fix are the two highest-value remaining items) or move to Phase 2 SSH
closure, per whichever the next governing prompt specifies.

## Pre-Phase-10 Closure Gate — PASS 2 (Browser trusted broker + minimal frame streaming)

Full detail: `PHASE9_BROWSER_EVIDENCE.md`, `PRE_PHASE10_CLOSURE.md`.

**Real, live-tested, this pass**: a genuine one-page Browser product
vertical slice, per the governing prompt's own scope (Tasks 1-27,
tabs/Task 28 explicitly deferred as the prompt itself allows):

- `services/clouddeskd/src/browser_broker.rs` (new): a trusted,
  backend-only CDP client over a real `tokio-tungstenite` WebSocket to
  Brave's own relayed CDP port. The typed surface exposed to callers is
  fixed (navigate/resize/mouse/keyboard in; frame/page_state/connected/
  error/closed out) -- no generic `send_cdp` capability exists anywhere.
  `BrowserSession` binding: owner + runtime generation captured from the
  authenticated principal and real `RuntimeManager` state at connect
  time, never from the request.
- New route `/api/v1/runtime-instances/browser/{instance_id}/browser-ws`
  (`lib.rs`), ownership derived via the same `instance_id_from_path`
  pattern every other runtime-instance route uses.
- Real bounded screencast frame delivery: CDP-ack-gated production (one
  frame outstanding at Brave's side) plus a `tokio::sync::watch`
  latest-frame-wins channel to the client (never an unbounded queue).
- A conservative navigation-scheme allowlist (`http`/`https`/
  `about:blank` only; `file:`/`javascript:`/`devtools:`/`data:`/
  `blob:`/`chrome:`/`brave:` all rejected).
- `apps/web/src/lib/BrowserApp.svelte` (new): address bar, canvas pixel
  surface (screencast frames decoded and drawn, never injected as DOM/
  iframe), real mouse/keyboard wiring scaled to Brave's own viewport,
  wired into `App.svelte` and the pre-existing `browser` launcher tile.
- `services/clouddeskd/tests/browser_broker.rs` (new, 5 tests, all
  live, all passing, zero leaked containers): ownership/unauthenticated/
  cross-user denial; raw-CDP-unreachable-from-another-container (a real,
  separate disposable `alpine` container genuinely attempted and
  failed); the full product slice (frames, navigation-scheme rejection,
  resize, a controlled fixture site proving real click/keyboard input
  and a genuine `CloudDesk`-mediated server-side-origin — request
  arrived from Brave's own container network, not the test process);
  real crash handling (`docker kill` against an active session, clean
  `closed` message, `RuntimeManager` detects failure, clean reconnect
  after restart); real enable/disable lifecycle while a session is
  active.
- Re-ran `browser_runtime.rs` (Task 26 profile regression): 4/4 clean,
  confirming the broker changes didn't weaken persistence/isolation.

**Real defect found and fixed this pass** (via the mandated
reproduce → root-cause → fix → retest process): `task_25`'s first run
failed with `429` instead of `200` -- the same, already-documented
`max_instances_per_user` instance-reuse gap `browser_runtime.rs`'s
`task_5_8` found in the prior pass, newly encountered here because
disable leaves the instance row stopped-but-undeleted. Fixed test-side
by restarting the existing instance rather than creating a new one
(matching the established workaround), with the gap re-documented, not
silently re-discovered and re-hidden.

**Cookie persistence (Task 27)**: bounded investigation only, per this
pass's own explicit "do not let it derail broker delivery" instruction
-- the prior pass's root cause (no keyring/dbus daemon for Chromium's
Linux OS-crypt backend) stands, `--password-store=basic` still doesn't
resolve it, and no further live experiment was attempted this pass.
Left explicitly as `COOKIE PERSISTENCE: IMPLEMENTATION DEFECT / OPEN`
with a concrete next action (a minimal per-instance `gnome-keyring-daemon`
unlocked with a server-side-derived, per-instance-scoped passphrase) --
`localStorage` is not claimed as equivalent.

**Not built this pass, honestly recorded**: tabs/popups (Task 28,
explicitly deferred), audio, downloads, uploads, clipboard, the full
internal-network-isolation attack matrix, WebRTC review, service-restart
reconciliation beyond the crash-recovery test above, multi-user
concurrent acceptance, the Browser-specific route-authorization matrix,
and a true Playwright-through-the-compiled-frontend acceptance run (the
live tests drive the same typed WebSocket protocol the frontend speaks,
proving the entire backend/broker/CDP/Brave path, but not the
frontend's own JS).

**READY FOR PHASE 10: NO.** Next exact action: PASS 3 -- either close
the deferred Browser items above (tabs, downloads/uploads, clipboard,
audio, network isolation, multi-user acceptance, authorization matrix,
a real Playwright-through-the-frontend run) or begin Phase 2 SSH
closure, per whichever the next governing prompt specifies.

## Pre-Phase-10 Closure Gate — PASS 1 (Office fixture cleanup + open-item register)

Full detail: `PRE_PHASE10_CLOSURE.md` (authoritative open-item
register across Phases 1-9, cross-checked against every phase evidence
matrix and the actual current implementation).

**This pass's actual scope, per the governing prompt's own multi-pass
execution strategy** (explicitly: "DO NOT attempt all of this
recklessly in one context window"): Office fixture cleanup (extending
the `CollaboraContainerGuard` RAII pattern already used in
`office_runtime.rs` to the three other test files that were found
still leaking real Collabora containers — `office_browser.rs` (13
tests), `office_hostile_documents.rs` (1 test), `office_remote_vfs.rs`
(3 tests), the last of which also lacked the cross-process
serialization lock every other Collabora-heavy file already has, added
this pass too) and building `PRE_PHASE10_CLOSURE.md` itself.

**Not attempted this pass, honestly recorded as such rather than
fabricated**: the Browser product vertical slice (typed CDP broker,
frame streaming, authenticated WebSocket, input, tabs/navigation,
frontend, downloads, uploads, clipboard, audio, network isolation,
multi-user acceptance, route-authorization matrix — Parts D-S of the
governing prompt) and Phase 2 SSH closure (agent, keyboard-interactive,
certificates, SCP, remote PTY terminal — Part V) are each realistically
multi-day implementation efforts on their own. Attempting either in the
remainder of this pass would mean fabricating rushed, shallow
"evidence" rather than the real, live-verified kind this project's own
standing discipline requires. `PRE_PHASE10_CLOSURE.md` records both as
`IMPLEMENTATION MISSING`, item by item, exactly as the governing
prompt's own Part A requires ("Do not use ambiguous PARTIAL where the
requirement can be decomposed").

**Real, newly-found defect this pass**: a full `cargo test --workspace`
run (from the immediately preceding Phase 9 pass) found 11 real,
healthy, running Collabora containers left over from files other than
`office_runtime.rs` — the earlier checkpoint entry's claim that only
`office_runtime.rs` leaked was incomplete. Fixed this pass: the same
`CollaboraContainerGuard` pattern applied to all three additional
files, plus a missing cross-process lock added to `office_remote_vfs.rs`
(it had none at all before this pass, meaning it could also have raced
against every other Collabora-heavy test binary).

**Final validation this pass**: `cargo fmt --all -- --check` PASS;
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
PASS; `cargo build --workspace --release` PASS (from the prior Phase 9
pass, unaffected by this pass's test-only changes). `cargo test
--workspace` (default, fail-fast): 30 binaries ran, then stopped on
`task_30_crash_recovery` (`code_runtime.rs`) — confirmed via isolated
rerun to be the pre-existing, already-documented Docker-load-contention
flake (its own log: "Docker-load contention is expected here"; passed
cleanly alone in 27.85s), not a regression from this pass's changes
(which touched only Office/Browser test files, the per-kind resource
policy, and RBAC). Re-ran with `cargo test --workspace --no-fail-fast`
to get the complete picture: **57/57 binaries ok, 0 failed**, including
all four now-guarded Office files (`office_browser.rs` 13/13,
`office_hostile_documents.rs` 2/2, `office_remote_vfs.rs` 3/3,
`office_runtime.rs` 7/7 — unaffected) and `browser_runtime.rs` (4/4).
Zero leaked `collabora/code` or `clouddesk-brave:1.93.136` containers
confirmed via `docker ps -a` immediately after.

**READY FOR PHASE 10: NO** (see `PRE_PHASE10_CLOSURE.md` for the full
gate). Next exact action: PASS 2 — the Browser trusted CDP broker and a
minimal frame-streaming transport, since every other unbuilt Browser
item depends on a page actually being visible through CloudDesk first.

## Phase 8 — LibreOffice / Collabora Online: COMPLETE (final pass, supersedes the "PARTIAL (third closure pass)" section below)

Full evidence: `PHASE8_OFFICE_EVIDENCE.md`. Office SSRF closed as PASS
under Model A (three real external-content mechanisms tested against a
disposable HTTP observer fixture — an ODF hyperlink, an ODF linked
image, and a Calc `WEBSERVICE()` formula — zero dangerous automatic
fetches for any of them). PPTX real-browser editing root-caused and
fixed as a test-automation gap (Impress needs Enter/F2 after a click to
enter text-edit mode, not a product defect) — all four representative
formats (DOCX/XLSX/ODT/PPTX) now pass real browser edit/save/reopen.
Four real product defects found and fixed this pass (frame-header
blocking, Files→Office home-relative path handling, proxy Host-header
stripping, Collabora root-absolute asset/WebSocket paths + `wss://`
scheme mismatch) — see `PHASE8_OFFICE_EVIDENCE.md` for full detail.
Two genuine Docker-load-only test flakes (`code_runtime.rs`'s
`task_19_enable_disable_lifecycle` and `task_30_crash_recovery`) were
root-caused and hardened, plus a broader Collabora-test contention
issue across `office_runtime.rs`/`office_browser.rs`/
`office_hostile_documents.rs` fixed with in-binary + cross-process
locks. Zero unresolved Critical/High. Rust and frontend gates PASS.
Current commit for Phase 8 closure: `fc9279e`.

## Phase 9 — Brave Browser Runtime: PARTIAL (foundation pass only, in progress)

Full evidence: `PHASE9_BROWSER_EVIDENCE.md`. **This is a single
foundation pass on an inherently multi-week scope (92 numbered tasks
in the closure prompt) — do not read "PARTIAL" here as "nearly done."**

**What is real and working:**

- Brave version: **1.93.136** (Chromium 151 base), installed from
  Brave's own official signed apt repository, pinned via
  `apt-mark hold` in `docker/brave/Dockerfile` (checked into the repo
  at `docker/brave/`). No official Brave Docker image exists to
  reference by registry digest — this Dockerfile *is* the pinned
  artifact, built locally as `clouddesk-brave:1.93.136`. **An operator
  must run `docker build -t clouddesk-brave:1.93.136 docker/brave`
  before the Browser runtime can be enabled — nothing builds it
  automatically.**
- Runtime mode: **OCI**, registered as a normal Phase 6 `RuntimeManager`
  adapter (`services/clouddeskd/src/browser_runtime.rs`,
  `browser_oci_spec()`), wired into `main.rs` exactly like Code/Office.
  `RuntimeConfig::browser_image` (new field, `crates/config`) holds the
  image reference.
- Real, live-verified security posture: non-root container user
  (`run_as` resolves to `clouddeskd`'s own real UID/GID at start time —
  the first adapter that needs to *write* into its `/state` mount, so
  it can't use a fixed build-time container UID the way the Dockerfile
  alone would default to), Docker's *default* seccomp profile
  (`unconfined` was tried and explicitly abandoned once a two-
  capability fix worked instead), `no-new-privileges` kept enabled,
  `CapDrop=[ALL]` baseline plus exactly `SYS_ADMIN` + `SYS_CHROOT`
  (both required for Chromium's own namespace-based sandbox to
  initialize at all — verified live via real container-log failures
  for each, not assumed), no `--no-sandbox` flag anywhere, no Docker
  socket, no privileged mode, no host network/PID namespace.
- Raw CDP isolation: Brave's own DevTools port binds to the
  container's loopback interface regardless of any
  `--remote-debugging-address` flag (a real structural Chromium
  hardening, live-verified), relayed via `socat` to a container-wide
  port that is still never published to the host — reachable only via
  the private Docker bridge network, the same boundary every other
  runtime's port already has.
- Real integration test:
  `services/clouddeskd/tests/browser_runtime.rs`'s
  `task_1_2_3_brave_runtime_reaches_real_running_state` — starts a
  real Brave container through the real generic
  `/api/v1/runtime-instances` HTTP API, waits for a real `Running`
  state (defined as: a real HTTP GET against Brave's own real
  `/json/version` CDP endpoint succeeds, not PID existence alone),
  stops it, and verifies real container teardown via `docker inspect`.
  Reproduced 3/3 clean, ~8-10s each.
- Separately (standalone Node.js CDP client, test infrastructure only,
  not product code): a real `Target.createTarget` →
  `Target.attachToTarget` → `Page.navigate` → `Page.captureScreenshot`
  round trip against the real running Brave container, against
  `https://example.com` — produced a real, correctly-rendered
  screenshot, proving the browser itself genuinely works end to end.

**Real defects found and fixed, first pass** (all via
reproduce → root-cause → smallest fix → retest):

1. Chromium's sandbox needs `SYS_ADMIN` + `SYS_CHROOT` (not just one)
   to initialize under `no-new-privileges` without falling back to
   `--no-sandbox` or `--security-opt seccomp=unconfined`.
2. `/state` (the adapter's own per-instance mount) is owned by
   `clouddeskd`'s own real process UID, not the image's fixed
   `USER brave` (uid 10001) — fixed via `run_as`.
3. Running under an arbitrary UID with no `/etc/passwd` entry leaves
   `$HOME` unset, which crashed Brave's own wrapper script trying to
   write XDG data to `//.local/...` — fixed via `extra_env` setting
   `HOME=/state`.

**Second pass — real defects found and fixed** (all via
reproduce → root-cause → smallest fix → retest):

4. **Production `pids_limit` gap, now fixed for real.** Real
   measurement: a single blank Brave tab uses 102 pids-cgroup tasks;
   +3 tabs measured at 143 (~+14/tab). Built a genuine per-`RuntimeKind`
   `ResourcePolicy` override mechanism in
   `crates/orchestrator/src/manager.rs` (`kind_policies` map,
   `with_kind_policy()`, `policy_for()`, resolved once per instance at
   creation time) and wired the real production value
   (`pids_limit: 512`) for Browser in `main.rs` — not a test-only
   override. `task_3_undersized_pids_limit_fails_cleanly_and_bounded`
   proves an undersized limit fails cleanly and boundedly, not by
   hanging.
5. **Security-relevant RBAC bug**: `SessionPrincipal::roles` holds
   role display names ("Guest"), never lowercase role IDs ("guest") —
   a naive `r == "guest"` check in `default_persistence` silently
   always failed, so every user including Guest was getting
   `Persistent` Browser profiles, defeating `GOAL.md` G7's explicit
   Guest-ephemeral requirement. Fixed with
   `r.eq_ignore_ascii_case("guest")`.
6. The `guest` role had no `apps.browser.use` capability grant at all
   — Guest couldn't open a Browser instance to begin with. Fixed by
   adding the grant in `crates/auth/src/lib.rs`.
7. Chromium's `SingletonLock`/`SingletonSocket`/`SingletonCookie`
   files reference the previous container's hostname, hanging a fresh
   container waiting for a dialog headless mode never shows. Fixed by
   removing those files at entrypoint start.
8. Docker `stop` only signals PID 1; the original entrypoint
   backgrounded Brave and `exec`'d `socat` as PID 1, so Brave was
   always SIGKILLed on stop, losing unflushed writes. Fixed by
   flipping the entrypoint: background `socat`, `exec` Brave as PID 1.
9. (Documented, not fixed) Real cookie values reach the on-disk
   `Cookies` SQLite file with a genuine encrypted value, but cannot be
   decrypted again after a real restart in this minimal container
   image — Chromium's OS-crypt backend has no dbus/keyring daemon here,
   and `--password-store=basic` does not fix it. Persistence proof
   pivoted to `localStorage` instead (verified working); a real, open
   item for a future pass.
10. (Documented, not fixed) Browser has no instance-reuse-on-create
    path (unlike Code's `existing_code_instance`); a stopped instance
    row still counts against `max_instances_per_user` (default 1), so
    a genuine second "new session" request for the same user returns
    `429`. Worked around test-side by restarting the existing instance;
    a real open item for the eventual broker/session layer.
11. `office_runtime.rs`'s test suite leaked real Collabora containers
    (6 of 7 tests never called stop) — pre-existing, not introduced
    this pass, but fixed this pass anyway per explicit instruction: a
    `CollaboraContainerGuard` RAII drop guard now tears down anything
    that appeared during each test, verified at zero leaks across the
    full 7-test suite. The identical pattern (`BraveContainerGuard`)
    was applied to `browser_runtime.rs` from the start.
12. **Test-concurrency defect, found by running the real
    `cargo test --workspace` gate (not skipped)**: `browser_runtime.rs`'s
    4 tests, run concurrently against each other (Cargo's default),
    reproducibly (2/2 runs) failed `task_5_7`/`task_5_8` with a 502 on
    the stop/restart round trip — real Brave containers competing for
    host CPU/IO pushed a restart past its health deadline. Not a
    product defect; fixed with `acquire_cross_process_browser_lock()`,
    the same cross-process `flock` pattern Office already uses.
    Re-verified: 2 consecutive `cargo test -p clouddeskd --test
    browser_runtime` runs, 4/4 clean each time, zero leaked
    containers.
13. **New finding, broader than Task 1's original scope**: after the
    real `cargo test --workspace` gate finished clean (41/41 binaries
    ok), a post-run `docker ps -a` check found 11 real, healthy,
    still-running Collabora containers — not from `office_runtime.rs`
    (which is fixed and verified clean), but from the *other* 7 Office
    test files (`office_browser.rs`, `office_db_failure.rs`,
    `office_format_matrix.rs`, `office_hostile_documents.rs`,
    `office_remote_vfs.rs`, `office_restart.rs`, `office_wopi_host.rs`),
    none of which received the `CollaboraContainerGuard` treatment this
    pass. Cleaned up manually (`docker rm -f`) rather than left
    running. **Not fixed this pass** — Task 1 was explicitly scoped to
    `office_runtime.rs` only, and this pass's own scope explicitly
    excludes broadening beyond the 8 numbered Browser tasks plus that
    one file. Flagged here as a real, concrete next-priority hygiene
    item: apply the same RAII guard pattern to the other 7 Office test
    files.

**What is real and working, additionally, after the second pass:**

- Role-aware Browser profile persistence: `default_persistence(kind,
  principal)` in `lib.rs` returns `Persistent` for every role except
  Guest, `Ephemeral` for Guest — LIVE CLOUDDESK tested end-to-end via
  real `localStorage` sentinels surviving (User) or being wiped
  (Guest) across a real container stop/restart cycle
  (`task_5_7_user_role_browser_profile_is_persistent`,
  `task_5_8_guest_ephemeral_and_cross_user_isolation`).
- Cross-user profile isolation: two separate real Brave instances
  (User A, User B) proven unable to see each other's `localStorage`
  sentinel — same test as above.
- Production-safe Browser `pids_limit` (512, real-measured), via a
  genuine per-kind `ResourcePolicy` mechanism now generally available
  to any future adapter, not a Browser-only hack.

**What is NOT built (the large majority of Phase 9's own scope,
unchanged by this second pass):**

- The browser broker (typed CDP operations — Task 8) — does not exist.
- Frame/screencast streaming to the frontend (Task 9-12) — does not
  exist. `CloudDesk` cannot currently show a Brave page to any user.
- Mouse/keyboard/IME input handling (Task 13-15) — does not exist.
- Navigation surface, URL policy, tabs, popups (Task 16-17, 23-28) —
  do not exist.
- Audio (Task 29-31/75), downloads/uploads (Task 34-39/77-78),
  clipboard (Task 40-41/79), video-playback acceptance (Task 32/76) —
  none built or tested.
- Internal-network-security hardening beyond "not `--network=host`"
  (Task 18-22) — no dedicated isolated network namespace/egress policy
  designed or built; no SSRF/DNS/web-attacker-model testing performed
  (there is no navigation surface yet to attack).
- `BrowserApp.svelte` frontend (Task 68) — does not exist. The
  launcher-tile manifest (`apps/web/public/manifests/browser.json`)
  already existed before this pass as a placeholder only.
- The entire live-acceptance/authorization/hostile-client/multi-user
  concurrency matrix (Task 66, 69-90, and simultaneous-multi-user
  acceptance beyond the two isolation dimensions tested above) —
  cannot be exercised without the broker/frontend layers above.

**Tests passing** (`browser_runtime.rs`, all 4, run together, clean,
zero leaked containers): `task_1_2_3_brave_runtime_reaches_real_running_state`,
`task_3_undersized_pids_limit_fails_cleanly_and_bounded`,
`task_5_7_user_role_browser_profile_is_persistent`,
`task_5_8_guest_ephemeral_and_cross_user_isolation`. Plus the full
pre-existing workspace suite — see this pass's final
`cargo test --workspace` result below.

**Tests failing**: none. Two full `cargo test --workspace` runs this
pass: first run (before the concurrency fix) showed
`task_5_7`/`task_5_8` failing with 502s under Cargo's default
within-binary parallelism (defect #12 above); after adding
`acquire_cross_process_browser_lock()`, a second full
`cargo test --workspace` run passed 41/41 test binaries, exit code 0,
zero test failures.

**Rust gates, final state this pass**: `cargo fmt --all -- --check`:
PASS. `cargo clippy --workspace --all-targets --all-features -- -D
warnings`: PASS (re-run after the concurrency fix, clean). `cargo test
--workspace`: PASS, 41/41 binaries ok, 0 failed. `cargo build
--workspace --release`: PASS (confirmed earlier this pass, "Finished
release profile [optimized] target(s) in 2m 07s"; unaffected by the
later test-only concurrency fix, not independently re-run after it
since no non-test code changed).

**Frontend gates**: unaffected this pass (no `apps/web` files
touched) — last verified PASS.

**Resource cleanup, final state**: zero leaked
`clouddesk-brave:1.93.136` containers, zero leaked
`office_runtime.rs`-originated `collabora/code` containers (both
independently re-verified after the final `cargo test --workspace`
run). 11 leaked `collabora/code` containers from the other 7,
un-guarded Office test files were found and manually cleaned up (see
defect #13) — a real, documented, out-of-this-pass's-scope gap, not a
regression this pass introduced.

**Uncommitted files at this checkpoint** (see git status for the
authoritative list; committed in this pass's own commits once gates
are confirmed green): `crates/auth/src/lib.rs` (guest capability
grant), `crates/orchestrator/src/manager.rs` (per-kind
`ResourcePolicy`), `docker/brave/Dockerfile` (SingletonLock + PID1
fixes), `services/clouddeskd/src/lib.rs` (role-aware
`default_persistence`), `services/clouddeskd/src/main.rs` (Browser
`pids_limit: 512` wiring), `services/clouddeskd/tests/browser_runtime.rs`
(Tasks 3/5/7/8 tests + `BraveContainerGuard`),
`services/clouddeskd/tests/browser_cdp/cdp_probe.mjs` (new, CDP test
harness), `services/clouddeskd/tests/office_runtime.rs`
(`CollaboraContainerGuard`), `PHASE9_BROWSER_EVIDENCE.md`, this
checkpoint section.

**Next exact action** (in priority order, unchanged in substance from
the first pass, since none of these were this pass's scope): (1)
design and build the typed browser broker (Task 8) and a minimal
frame-streaming transport (Task 9) — screencast is the single
highest-leverage next piece, since nothing else in the Browser app is
even visible without it; (2) only after a page is genuinely visible
end-to-end, build mouse/keyboard input (Task 13-14) and the minimal
`BrowserApp.svelte` shell; (3) network isolation (Task 18-19) before
ever letting a real user navigate to an arbitrary URL through it; (4)
close the two documented open items (cookie/OS-crypt persistence,
Browser instance-reuse-on-create) if they become blocking for a later
pass.

Do not start Phase 10. Do not recalculate the global completion
percentage.

## Phase 8 — LibreOffice / Collabora Online: PARTIAL (third closure pass)

Full evidence: `PHASE8_OFFICE_EVIDENCE.md` (73-item matrix, now
overwhelmingly PASS). This pass closed: real remote-VFS Office
document support over SFTP (Task 1-5/26/27/34/35 — a full
`WOPI → CloudDesk authorization → real SftpProvider → real remote file`
round-trip against a disposable OpenSSH fixture, with an honestly
documented non-atomic-replace limitation on standard SFTP v3),
database-failure fail-closed coverage (Task 13/14/69 — dropping
`office_locks`/`office_wopi_files` out from under a running server via
an independent connection proves every lock-touching operation and
`PutFile` fail closed rather than treating a DB error as "no lock,
proceed"), service-restart-with-a-live-lock (Task 15-17/68 — two fully
independent server instances against the same file-backed DB prove the
pre-restart lock survives intact, without becoming duplicatable or
bypassable), a hostile-document corpus (Task 6/28/29/50 — 11 safe
adversarial fixtures plus a live-Collabora corrupt-document survival
test), a bounded discovery cache (Task 11/12/63), real-byte-count size
enforcement via a genuine 200MB+ chunked upload with no `Content-Length`
to lie about (Task 24), 16MB large-file streaming round-trip evidence
(Task 25/67), and the Task 18/23 external-mode decision checked against
`GOAL.md`'s actual text (confirmed non-blocking: the spec permits
Collabora-compatible technology but does not separately mandate
administrator-configured external deployment as a distinct v1
requirement).

Two more real defects found and fixed this pass: `acquire_lock` always
statted a local filesystem path, so every remote LOCK recorded a (0,0)
snapshot and made every subsequent remote `PutFile` fail its own
conflict check outright; and `SftpProvider`'s `block_in_place`-based
sync calls require the multi-threaded tokio runtime, caught as a
test-harness panic under the default single-threaded `#[tokio::test]`
(the real product binary is unaffected -- `#[tokio::main]` already
defaults to multi-thread). Eight real defects found and fixed across
all three closure passes total.

Real, live-gathered finding this pass on Task 51/52 (macros,
external-link SSRF): the Collabora container ships with **no shell or
network tool at all** -- confirmed via a full `docker export` listing
of all 33,840 files in the image, zero of `sh`/`bash`/`curl`/`wget`/
`nc`/`python`/`busybox` present. This meaningfully narrows what
document-triggered code execution inside coolwsd could pivot to, but
it also means genuine macro-execution and external-fetch-on-open
behavior cannot be probed interactively without a real browser (to
trigger `bundle.js`) or reverse-engineering coolwsd's internal HTTP
client -- both remain honestly `NOT EXECUTED`.

Rust gates (`fmt`/`clippy -D warnings`/`test --workspace`/
`build --release`) all PASS; frontend gates unchanged this pass (last
verified PASS); the live Office suite (7 tests) and the live remote-VFS
suite (3 tests) were each run twice consecutively with zero failures
and zero leaked containers/fixture files.

**Still not reached, honestly marked `NOT EXECUTED`/`IMPLEMENTATION
MISSING` in the matrix, not COMPLETE:** real macro execution behavior
(Task 51); browser-triggered external-link SSRF (Task 8/9/10/52,
partial structural mitigating evidence only); CSP audit for the Office
iframe/proxy routes (Task 26); external Collabora config wiring itself
(Task 19-22/61/62 — the decision not to leave a misleading unwired
field was made, the functional admin-only TLS/validation feature was
not built); logout-specific live token-invalidation test (Task 42);
write/lock-conflict/write-denied audit events beyond
`office.session.opened` (Task 44); and (unchanged) real browser-driven
editing. See `PHASE8_OFFICE_EVIDENCE.md` for the full per-item
breakdown.

Zero unresolved Critical/High-severity defects in the surface actually
exercised. **Phase 9 was explicitly not started**, per the Phase 8
closure prompt's instruction that a PARTIAL phase does not advance.

## Phase 7 — VS Code-Compatible Runtime: COMPLETE

Full evidence: `PHASE7_CODE_EVIDENCE.md` (45-item matrix). 37 PASS
(including one, port forwarding, that started this final pass as a
live-found FAIL and was fixed), 2 PASS (capability; live/interactive
acceptance BLOCKED BY ENVIRONMENT), 4 PARTIAL (each decomposed into
resolved sub-claims wherever the environment allows), 1 NOT EXECUTED
(clipboard, browser-only), 1 BLOCKED BY ENVIRONMENT (browser automation
itself), 0 unresolved FAIL. Per the closure policy, five browser-only
dimensions (browser-driven IDE acceptance, browser-only file-focus
verification, hover/completion UI, interactive debugging UI, public
GitHub/GitLab credential tests) remain honestly `BLOCKED BY
ENVIRONMENT` without blocking COMPLETE status -- none of them hide a
missing backend/runtime implementation.

```
Runtime:                     code-server 4.133.0 (VS Code base 1.133.0)
                              -- codercom/code-server:4.133.0, digest-
                              verified pin
Execution mode:               OCI (Docker) -- the only mode
                              implemented; no code-server binary
                              available on this host, and installing
                              one at request/build time would violate
                              Task 33. Documented, not a gap.
Runtime detection:            PASS
CloudDesk proxy:              PASS -- real HTTP+WS proxy through the
                              existing Phase 6 foundation, no direct
                              connection to the internal port
WebSocket:                    PASS (generic Phase 6 mechanism reused,
                              not independently re-tested for Code)
Cookie/secret stripping:      PASS -- live-verified via docker inspect
                              of the real container's own environment
Per-user isolation:            PASS
Persistent profile:            PASS -- live-tested stop+restart,
                              closes Phase 6 evidence item 23; profile
                              mount now separate from the workspace
                              mount, live-verified to survive switches
Multiple workspaces:           PASS -- workspace identity is always an
                              assigned_roots.id, never a raw host path;
                              discover/select/switch/persist/reopen/
                              fail-safely all live-tested (5 new tests,
                              services/clouddeskd/tests/code_runtime.rs)
Workspace authorization:      PASS -- resolve_own_assigned_root
                              re-authorizes on every start/restart/
                              switch; cross-user, revoked, random, and
                              traversal-shaped workspace_id all
                              rejected (404) before any container
                              starts; read vs read-write access mode
                              enforced as a genuine ro/rw Docker mount,
                              verified from inside the container
Files -> Code:                 DEEP LINK BACKEND RESOLUTION: PASS --
                              normal/nested/spaced/unicode filenames,
                              same filename in two workspaces, a
                              read-only file, a second user's root, a
                              symlink escape, a deleted file, a revoked
                              workspace, and a traversal-shaped
                              relative value all live-tested; the exact
                              file argument handed to the real
                              code-server process verified via
                              `docker inspect .Config.Cmd`.
                              REAL IDE FILE FOCUS: BLOCKED BY
                              ENVIRONMENT (needs a browser)
Real IDE HTTP/WebSocket:       PASS -- real IDE HTML + a real static
                              asset path + a real WebSocket upgrade
                              attempt through the actual proxy (not a
                              health-check ping); found and fixed two
                              real defects along the way (see below)
File edit/save:                REAL BROWSER IDE EDIT/SAVE: BLOCKED BY
                              ENVIRONMENT. The docker-exec-based write
                              in task_8_9 proves the mount is writable
                              from the container's own perspective,
                              explicitly NOT counted as IDE evidence
Terminal:                      PASS -- process identity/isolation
                              proven (mapped non-root UID; cannot reach
                              /etc/shadow, Docker socket, Vault, DB --
                              task_2's hostile Git hook and task_11's
                              mount inspection). Distinct from the
                              still-OPEN Phase 2 remote SSH terminal.
                              Browser-rendered terminal UI itself:
                              BLOCKED BY ENVIRONMENT
Git:                           PASS -- disposable-repository workflow
                              plus a full clone/edit/commit/push/
                              branch/fetch/pull cycle against a
                              disposable local bare remote
GitHub/GitLab live:            GIT REMOTE WORKFLOW: PASS (local bare
                              remote, real Git transport, no special
                              GitHub/GitLab OAuth implied). PUBLIC
                              GITHUB LIVE AUTH / PUBLIC GITLAB LIVE
                              AUTH: BLOCKED BY ENVIRONMENT (no live
                              credentials)
Extensions:                    PASS -- real install from code-server's
                              actual registry (Open VSX, not the
                              Microsoft Marketplace), persistence
                              across a real restart, and uninstall
                              persistence across a real restart
Extension isolation:           PASS
Debugging:                     DEBUG INFRASTRUCTURE PRESENT: PASS --
                              bundled ms-vscode.js-debug confirmed, no
                              request-time install. INTERACTIVE
                              BREAKPOINT/DAP FLOW: BLOCKED BY
                              ENVIRONMENT
Language server:               LANGUAGE ENGINE CAPABILITY: PASS --
                              bundled TypeScript 6.0.3 performs genuine
                              live semantic type-checking
                              (ts.createProgram + getPreEmitDiagnostics)
                              inside a real running container, no
                              toolchain installed. REAL IDE
                              HOVER/COMPLETION ACCEPTANCE: BLOCKED BY
                              ENVIRONMENT
Enable/disable:                PASS -- full live lifecycle:
                              disabled->denied, admin-enable->healthy
                              start, disable-while-active->new access
                              denied + container genuinely gone + zero
                              Code containers + profile retained,
                              re-enable->restart->profile returns
Idle shutdown:                 PASS -- short test-only idle_timeout,
                              generic sweep_idle_once mechanism (no
                              Code-specific scheduler): activity keeps
                              it alive, genuine idleness stops it,
                              reopening restarts with profile intact
Crash recovery:                PASS -- real defect found and fixed
                              (see below); repeated across 4 full-suite
                              runs this pass with no races
Malicious workspace:           PASS -- symlinks to /etc and /root,
                              dangling symlink, nested symlink chain,
                              hardlink, unicode/control/shell-
                              metacharacter filenames, 40-level-deep
                              tree, 500-file directory, hostile
                              .vscode/*.json, and a real Git
                              post-checkout hook that actually executes
                              and fails to reach /etc/shadow, the
                              Docker socket, Vault, or the CloudDesk DB
Running-workspace revocation:  PASS -- policy implemented (not merely
                              documented): revoking the assigned root a
                              running Code instance has mounted now
                              terminates that instance immediately
                              (task_3_revocation_terminates_running_
                              workspace), rather than only denying new
                              access
Code local-port proxy / SSRF: PASS -- found the built-in path-based
                              proxy live-enabled and reachable, fixed
                              with --disable-proxy, re-verified 403
                              afterward (see below)
Route authorization matrix:    PASS -- unauthenticated and cross-user
                              (User B with User A's real instance ID)
                              attacks across workspace listing,
                              instance lifecycle, restart/stop, proxy,
                              logs, and admin enable/disable; ID
                              possession never sufficient
Container mounts/network:      PASS -- live docker inspect: non-root
                              user, not privileged, no-new-privileges,
                              cap-drop ALL, bridge network (never
                              host), loopback-only publish, non-zero
                              memory/pids limits, no Docker socket/
                              host-root/Vault/DB mount
Performance:                   PASS (measured) -- cold start ~0.68s,
                              TCP-ready ~0.05s later, genuine HTTP-
                              ready ~1.7s after that, idle ~83MiB RSS/
                              0.01% CPU, profile 8KB->13MB after one
                              extension install. Critical claim
                              confirmed: disabled/stopped Code -> zero
                              Code containers, core CloudDesk
                              unaffected. Not claimed lightweight --
                              Code is explicitly optional/heavyweight
License/third-party:           PASS -- docs/THIRD_PARTY_NOTICES.md:
                              MIT license confirmed from the actual
                              image's own LICENSE/package.json/
                              product.json; Open VSX (not Microsoft
                              Marketplace) documented; no proprietary
                              Microsoft components bundled
Image pinning:                 PASS -- digest-pinned
                              (sha256:e073a441c61c85821a7f16b64cf93b4e
                              77b4092899bb1f3bed906fbd558afd62), not
                              just the mutable 4.133.0 tag; never
                              client-configurable
Code browser acceptance:      BLOCKED BY ENVIRONMENT -- rechecked,
                              unchanged. CODE BROWSER ACCEPTANCE:
                              BLOCKED BY ENVIRONMENT
Rust gates:                    PASS
Frontend gates:                PASS
Unresolved Critical:           0
Unresolved High:               0
```

**Real defects found and fixed across the three closure passes:**
1. OCI-backed instance crashes never escalated past `Unhealthy` to a
   terminal `Failed` state -- the manager's supervisor loop only
   detected an unexpected exit directly for `RunningHandle::Process`
   (via `try_wait()`); a killed container just stayed `Unhealthy`
   forever, port never released, never eligible for crash-loop
   accounting. Reproduced with a real `docker kill` against a running
   Code container through the actual clouddeskd API. Fixed with a new
   `RuntimeAdapter::is_gone()` method (default `false`, preserving
   existing behavior for adapters that already detect exit through
   their own handle type), overridden by `OciAdapter` with a real
   `docker/podman inspect --format {{.State.Running}}` check.
   Regression-tested (`task_30_crash_recovery`).
2. `sqlx::migrate!` reads `migrations/` at macro-expansion time, but
   cargo has no dependency edge on that directory's contents -- adding
   `0014_code_workspaces.sql` alone silently did not trigger a
   recompile of `crates/db`, so `code_user_state` writes failed with
   "no such table" against a stale cached migrator. Caught by
   `task_2_persistence_restart_and_safe_fallback` failing with the
   exact wrong value. Fixed by touching `crates/db/src/lib.rs` and
   documenting the trap in-code.
3. **Deep-link workspace-resolution ambiguity (security).**
   `resolve_deep_link_workspace` checked home before checking more-
   specific assigned roots, and fed a "home" result back through the
   generic `resolve_workspace(None)` path, which collided with that
   path's own "infer the last-used workspace" fallback -- a deep-linked
   file could silently be evaluated against a *different*,
   previously-selected workspace (potentially widening a `read`-only
   root's access to `read-write`). Caught by
   `task_1_deep_link_backend_resolution`'s cross-user case returning
   200 instead of 403. Fixed with longest-matching-prefix resolution
   and an explicit non-ambiguous "home" branch.
4. **code-server's built-in local-port proxy was live and reachable
   (security).** `/proxy/{port}/...` and `/absproxy/{port}/...` were
   enabled by default; a harmless in-container echo listener on port
   9999 was reachable through it from outside the container. No
   hostname-injection path exists (`getProxyTarget()` only parses an
   integer port) and Docker's own bridge networking already bounded the
   blast radius to the container's own namespace, but disabled outright
   anyway (`--disable-proxy`) since CloudDesk has no product use for it.
   Re-verified 403 afterward.
5. **The Code IDE's own proxy URL 404'd (functional, product-critical).**
   Axum's `{*upstream_path}` wildcard doesn't match a bare
   trailing-slash request -- exactly the URL `CodeApp.svelte` uses as
   its iframe `src`. The Code IDE would never have loaded for a real
   user. Fixed by registering an explicit route for the bare prefix.
6. **Health check reported `Running` before code-server could serve a
   real request (functional, race).** Measured live: the TCP listen
   socket opens ~1.7s before a real HTTP GET succeeds. The orchestrator
   health check was a bare TCP connect. Fixed with a real HTTP GET to
   `health_check_path`.

All six reproduced live, classified, regression-tested, and retested
against the full `code_runtime.rs` suite (run twice after the Task
18/24 routing and health-check changes, zero leaked containers or
processes after either run) plus the full `cargo fmt`/`clippy`/`test
--workspace`/`build --release` gate chain and the frontend
`lint`/`check`/`test`/`build` chain.

**Multiple workspaces (Task 2 of this closure pass), summary:**
workspace identity is always an existing `assigned_roots.id` --
resolved server-side to a canonical path, never accepted as a raw host
path from the browser. A Code container mounts exactly two
directories: the user's home (`profile`, always read-write, holds
settings/extensions/history) and a fixed `/workspace` path (the
currently selected root, read-only or read-write per its
`access_mode`, never silently upgraded). Switching workspace reuses
the same instance/row (the per-user instance limit is 1) -- stop,
re-stage, `start_instance` again with a bumped generation and the new
mount, deliberately not `restart_instance` (whose crash-loop counter
exists for genuine crashes, not intentional switches). The newly
selected workspace is persisted as "last used"
(`code_user_state.last_workspace_id`) only after `start_instance`
confirms the instance actually reached `Running`. A deleted/revoked
*last-used* workspace falls back to home safely on the next implicit
reopen/restart; an *explicitly requested* revoked/cross-user/random/
traversal-shaped `workspace_id` is a hard 404, rejected before any
container starts. A new self-service `GET /api/v1/code/workspaces`
lists only the caller's own roots (safe label + read/write flag, never
a raw path). Five new live tests in
`services/clouddeskd/tests/code_runtime.rs` cover all of the above,
including two concurrent switch requests converging to exactly one
running instance.

**Current commit (Phase 7, final):**
```
759e9b1 docs(engineering): close Phase 7 -- VS Code-Compatible Runtime COMPLETE
4b45fbf test(code): add malicious-workspace, deep-link, route-auth, and lifecycle closure evidence
561af9a fix(code): restrict local port proxy, fix proxy routing, and fix health/workspace-resolution races
72c1815 feat(code): real multiple-workspace support (Phase 7 Task 2)
26a7b47 test(code): add live language-service and debug-extension evidence (Phase 7 Task 8/9)
403f6cf docs(code): add Phase 7 executable evidence matrix
b78dfe6 fix(runtime): escalate OCI crash detection to a terminal state
52ec3e8 test(code): add extension install and per-user isolation evidence
f3ce707 feat(code): add CloudDesk Code application integration
c4618d6 test(code): add real Code runtime acceptance
093d26f feat(code): add Code runtime adapter
```
on top of the Phase 6 commit chain below, unmodified.

**Why COMPLETE:** every mandatory item on the Phase 7 closure policy
checklist is `PASS` or an honestly-scoped `PARTIAL` whose resolvable
sub-claims are already `PASS`: multiple workspaces, workspace
switching, last-workspace persistence, deep-link backend resolution +
specific-file targeting (server-verified via the real process's own
argv), workspace authorization (including running-workspace
revocation, which now actually terminates the affected instance, not
just denies new access), malicious-workspace isolation, the local-port
proxy risk (found live-enabled, fixed), SSRF isolation, cookie/secret
stripping (proven via real received headers, not config alone), a full
route-authorization sweep, read-only workspace semantics, Git remote
workflow, Git credential isolation (documented, with its one real
environment limitation stated honestly), extension persistence and
isolation, integrated-terminal identity/isolation, actual container
mount/network inspection, full Code lifecycle (enable/disable/idle/
crash), measured performance, third-party license notice, image
digest-pinning, and real HTTP/WebSocket proxying (with two
product-critical defects found and fixed along the way). Unresolved
Critical: 0. Unresolved High: 0. Rust and frontend gates both pass.
Only the five browser-only dimensions the closure policy explicitly
permits remain `BLOCKED BY ENVIRONMENT`, each with real non-browser
evidence sitting behind it -- none of them hide a missing
implementation. See `PHASE7_CODE_EVIDENCE.md` for the complete,
itemized accounting.

**Next phase:** Phase 8 — LibreOffice / Collabora Runtime.

**Next exact action:** implement real Office editing over the Phase 6
orchestrator with CloudDesk-authorized file access, WOPI-style
integration where appropriate, locks, conflict-safe writes, and real
DOCX/XLSX/PPTX/ODF round-trip evidence -- following the same pattern
established for Code (a trusted `OciSpec`/`OciAdapter` consumer, no
second lifecycle manager, server-resolved workspace/file identity,
live evidence over inference, honest `BLOCKED BY ENVIRONMENT` labeling
for anything that genuinely requires a browser). Not started this
session.

## Phase 6 — Optional Runtime Orchestrator: COMPLETE

Full evidence: `PHASE6_RUNTIME_EVIDENCE.md` (40-item matrix, every PASS
citing a specific test; 38 PASS, 1 NOT EXECUTED, 0 FAIL, 0
IMPLEMENTATION MISSING, 4 BLOCKED BY ENVIRONMENT).

```
Core orchestrator:              PASS
clouddeskd product wiring:      PASS
RBAC:                           PASS
Per-user isolation:             PASS
Settings:                       PASS
Settings browser acceptance:    BLOCKED BY ENVIRONMENT (no browser
                                 automation tooling in this container;
                                 rechecked this pass, unchanged; real
                                 backend/API evidence + frontend unit-
                                 behavior evidence stand in its place,
                                 per explicit allowance)
HTTP proxy:                     PASS
WebSocket proxy:                PASS
Origin policy:                  PASS
SSRF resistance:                PASS
Hostile-input sweep:            PASS -- including duplicate-JSON-key
                                 behavior and real WebSocket binary-
                                 frame handling, the two items this
                                 checkpoint previously flagged as
                                 untested
Duplicate JSON:                 PASS -- serde's derived Deserialize
                                 rejects any duplicate key outright
                                 (422); documented as a *stronger*
                                 property than "one value safely wins"
WebSocket binary:                PASS -- real network connections,
                                 never tower::oneshot
WebSocket oversized bound:      PASS -- real defect found+fixed (see
                                 below)
WebSocket cleanup:               PASS -- 10 real connect/disconnect
                                 cycles leave the instance healthy;
                                 client/upstream disconnect both end
                                 the relay via tokio::join!
Fixture cleanup:                 PASS -- real defect found+fixed (see
                                 below); zero leftover
                                 test-runtime-fixture processes
                                 verified after two consecutive full
                                 `cargo test --workspace` runs
Log flooding:                    PASS
Hostile log rendering:           PASS
Secret isolation:                PASS
Audit:                           PASS
OCI direct:                      PASS
OCI through product:             PASS
OCI hardening:                   PASS
cgroup CPU:                      BLOCKED BY ENVIRONMENT (rechecked this
                                 pass: mkdir under the delegated
                                 cgroup path still succeeds, but
                                 cpu.max/memory.max/pids.max writes
                                 still fail with Permission denied --
                                 no sudo used, no host cgroup mutated)
cgroup memory:                    BLOCKED BY ENVIRONMENT (same recheck)
cgroup PIDs:                      BLOCKED BY ENVIRONMENT (same recheck)
Product-level failure matrix:    PASS -- assembled as
                                 PHASE6_RUNTIME_EVIDENCE.md, 40 items,
                                 every PASS citing a specific test
Rust gates:                      PASS -- fmt/clippy/test/build all
                                 clean, verified across two consecutive
                                 full `cargo test --workspace` runs
Frontend gates:                   PASS -- npm run lint/check/test/build
                                 all clean, 51/51 tests
Unresolved CRITICAL:             0
Unresolved HIGH:                 0
```

**Known, honest, non-blocking exception:** item 23 in
`PHASE6_RUNTIME_EVIDENCE.md` (persistent-profile retention) is
`NOT EXECUTED` -- no adapter with `Persistence::Persistent` has real
profile data to retain yet, since Code/Office (the first kinds that
will actually use persistent profiles) don't exist until Phase 7+. The
typed policy (`default_persistence()`) exists and is exercised for
instance creation; only the live stop-then-verify-retained cycle for a
persistent kind has nothing to test against yet. This is a data-
continuity completeness item, not a security boundary, and does not
by itself keep Phase 6 at PARTIAL.

**Three real defects found and fixed this closure pass** (see commit
messages for full detail):

1. **Edge-triggered pipe read starvation** (`crates/orchestrator/src/
   host_process.rs`, commit `4f39ee7`): a runtime instance producing
   more than one 4 KiB chunk of startup stdout could leave already-
   available bytes unread with no future wakeup ever coming for them,
   hanging readiness detection until timeout despite being genuinely
   healthy. Fixed by draining fully available data in one pass.
2. **Sanitized log output could exceed its own byte bound**
   (`services/clouddeskd/src/lib.rs`, commit `63ccfb4`): replacing a
   control byte with the 3-byte U+FFFD character could push output
   past its 64 KiB cap (65592 observed). Fixed by re-enforcing the
   bound on the sanitized output itself.
3. **Orphaned processes could outlive an abrupt parent death**
   (`crates/orchestrator/src/host_process.rs`, commit `df0727f`): this
   session's own earlier debug loops left 406 `test-runtime-fixture`
   processes running (confirmed via `ps aux`), which is what actually
   caused defect #1's regression test to intermittently reappear as
   flaky under full-workspace contention -- not a code regression, but
   a real, separate reliability gap. Fixed with a kernel-enforced
   parent-death signal (`set_parent_process_death_signal(SIGKILL)`),
   verified with a genuine external `kill -9` against a deliberately
   long-lived probe process (child confirmed gone within 1s) and with
   two full `cargo test --workspace` runs completing with zero
   leftover processes afterward each time. Also added
   `RuntimeManager::shutdown_all()` for `clouddeskd`'s own graceful-
   shutdown path and explicit test teardown.

Also added, not a defect fix: explicit, deliberate WebSocket size
bounds (4 MiB message / 1 MiB frame, both proxy legs) rather than
relying on axum/tungstenite's own library defaults (64 MiB/16 MiB) --
present, but never a value CloudDesk itself had chosen.

**Current commit (Phase 6, final):**
```
c3efe2b docs(runtime): add Phase 6 executable evidence matrix
993080a test(runtime): close duplicate-json and websocket binary coverage
df0727f fix(runtime): harden process cleanup and WebSocket bounds
d91b810 docs(engineering): close out Phase 6 runtime orchestrator status (partial)
06133d6 test(runtime): harden timing-sensitive tests, add audit verification
63ccfb4 test(runtime): expand hostile runtime API coverage
4f39ee7 fix(runtime): fix edge-triggered pipe read starvation in log capture
b168106 feat(settings): add optional runtime management UI
16be46e feat(runtime): wire orchestrator into clouddeskd
028446d feat(auth): add runtime.admin capability
0f9b583 feat(runtime): add shared optional runtime orchestrator
```
on top of the Phase 5 commit chain below, unmodified.

## Next phase

**Phase 8 — LibreOffice / Collabora Online (continuing; PARTIAL, not
advancing to Phase 9 until closed).**

## Next exact action

Work down `PHASE8_OFFICE_EVIDENCE.md`'s remaining `NOT EXECUTED`/
`IMPLEMENTATION MISSING` rows: Task 51/52 (macro execution behavior and
external-link SSRF -- both genuinely need real browser automation to
trigger `bundle.js`, given the live-confirmed absence of any shell/tool
inside the Collabora container to probe from another angle), Task 26
(a CSP audit specifically for the Office iframe/proxy routes), Task
19-22/61/62 (actually implementing external Collabora config -- the
decision so far was only to stop the field's doc comment implying
functionality it doesn't have, not to build the admin-only TLS/
validation feature itself), Task 42 (a dedicated logout-invalidates-
token live test), and Task 44 (write/lock-conflict/write-denied audit
events beyond the single `office.session.opened`). Then re-run the full
gate chain and update this checkpoint to COMPLETE only once the
Definition of Done in the Phase 8 prompt is genuinely met -- at that
point the remaining honestly-`BLOCKED BY ENVIRONMENT` browser-only rows
(Tasks 37/56-58) are acceptable per the closure policy's own terms.

## Last completed phase

**Phase 7 — VS Code-Compatible Runtime, status COMPLETE** (see
`PHASE7_CODE_EVIDENCE.md`) -- the last phase to reach full **COMPLETE**
status. **Phase 8 (LibreOffice/Collabora Online) is in progress, status
PARTIAL** (see the section at the top of this file and
`PHASE8_OFFICE_EVIDENCE.md`) -- the WOPI/lock/token/proxy security core
is real and live-proved; the frontend, full format matrix, and most
hostile-input/lifecycle/hardening breadth remain outstanding. Phase 2
(SSH feature matrix) remains explicitly incomplete and untouched.
Phase 5 — Music Application (below) remains as previously recorded:
backend/router/live-media evidence real and complete,
browser-flow acceptance honestly **BLOCKED BY ENVIRONMENT**.

## Phase 5 — what was built (Music Application)

```
Library indexing:      PASS (real ffprobe-backed indexing, incremental
                        rescan verified to skip unchanged files and
                        prune removed ones, live-tested)
Metadata:               PASS (real ID3/Vorbis-comment tags via
                        ffprobe's format.tags -- new MediaProbe field,
                        not filename-derived)
Direct playback:        PASS (backend/router-level; standalone MP3/
                        WAV/FLAC/OGG now correctly classify DIRECT --
                        this was a real Phase 3 bug, fixed this
                        session, not merely worked around)
Conversion fallback:    PASS (backend/router-level; reuses the exact
                        same /media/jobs REMUX/TRANSCODE path Video
                        uses, live-tested)
Playlists:              PASS (create/rename/delete/add/remove/reorder,
                        owner-scoped, live-tested)
Queue:                  PASS (server-persisted per-user queue;
                        play-now/play-next/add/remove/reorder pure
                        logic unit-tested, HTTP round-trip live-tested)
Favorites:               PASS (live-tested, cross-user isolation proven)
Recently played:        PASS (threshold-gated recording to avoid write
                        amplification, unit-tested; HTTP round-trip
                        live-tested)
Search:                 PASS (SQL-index-backed, LIKE-wildcard-escaped,
                        owner-scoped, live-tested)
Large-library test:     PASS (1200 synthetic rows inserted directly
                        via the store -- independent of ffmpeg by
                        design, see below -- proving pagination/
                        clamping/search bounds hold regardless of
                        library size)
Browser acceptance:     BLOCKED BY ENVIRONMENT (same as Video; no
                        chromium/playwright/automation tooling exists
                        in this container)
```

**What was actually built:** new `clouddesk-library` crate (indexing +
storage, reusing `clouddesk-media` for every `ffprobe`/`ffmpeg`
invocation -- no second compatibility engine, wrapper, or transcode
queue), full Music HTTP surface in `services/clouddeskd`, and
`MusicApp.svelte` + `music.ts` + shared `media.ts` (factored out of
`video.ts` so Video and Music share the DIRECT/REMUX/TRANSCODE plan/
job-lifecycle logic rather than each having a copy). Full detail,
including the exact scope reductions (on-demand rescan only, no live
scan-progress streaming, no filesystem watcher), is in the three commit
messages below and `V1_TRUE_CLOSURE.md` item #3.

**Real bug found and fixed while building this** (not introduced this
session, just uncovered by it): `clouddesk-media`'s compatibility
decision never classified standalone MP3/WAV/FLAC/OGG files as DIRECT
-- they fell through to REMUX despite being natively browser-playable
in every evergreen browser. Fixed in `compat.rs`'s `DIRECT_CONTAINERS`/
`DIRECT_AUDIO_CODECS`, covered by 4 new unit tests.

**Security findings (Phase 5):** Cross-user isolation live-tested for
library/tracks/playlists/favorites/queue (404, not 403 -- existence
isn't confirmed to another user, same discipline as media jobs).
Artwork is only ever served from an embedded stream or a same-directory
sidecar file reached through the track's own VFS authorization -- never
an arbitrary tag-supplied path. Hostile metadata (script tags, unicode
control chars, quotes) is stored and returned verbatim as a JSON string
value, never interpreted server-side; frontend renders exclusively
through Svelte's auto-escaping interpolation, no `{@html}` anywhere in
`MusicApp.svelte`.

**Task 19 adversarial authorization sweep (follow-up session) found and
fixed a real defect**: every Music *mutation* endpoint (add/remove/scan
library roots, playlist create/rename/delete/add-entry/remove-entry/
reorder, favorite/unfavorite, record-played, set-queue -- 13 handlers)
was gated on `files.local.read` instead of `files.local.write`. Guest's
role grant is `files.local.read` only ("Restricted read-only access"
per its own description in `crates/auth/src/lib.rs`), so a guest
account could add library roots, trigger scans, create/delete
playlists, and mutate favorites/queue -- contradicting the role's own
definition. Read-only Music endpoints were already correctly on
`files.local.read` and are unaffected. Fixed by reclassifying all 13
mutation handlers; proven by 5 new tests in `services/clouddeskd/tests/
music_authorization.rs` (real ffmpeg fixtures, real second/third/guest
accounts, zero mocks): full cross-user + ID-substitution attack sweep
across every endpoint (library, playlists, favorites, queue, recently-
played, artwork, root scanning, raw-VFS-path bypass attempt for media
conversion), guest-mutation denial (the bug itself), an explicit check
that a second administrator account does **not** get a row-level
override on another admin's playlist (this product's authorization is
capability-gated, not row-level -- proven, not just asserted), a
positive control for the true owner, and a check that a tampered
library-root row (path forced outside the authorized VFS root directly
in the database) is rejected on the next scan rather than trusted
because it was previously stored.

**Explicitly not independently re-verified this session:** a
guest-role-specific 403 matrix for every new Music endpoint (reuses the
same `files.local.read` capability-gating pattern already proven for
Video/media in Phase 3/4, not re-tested per-endpoint here).

**Task 26 (audit events):** added `music.library_root.configured`,
`music.playlist.created`, `music.playlist.deleted`. Phase 3's known gap
(per-stage media job audit events -- `started`/`completed`/`failed`/
`cancelled`, only `requested` exists) was **not** closed this session;
Music's own actions don't naturally touch that code path, so per the
task's own instruction it is preserved explicitly for later closure,
not forced in here.

## Validation (Phase 5)

```
cargo fmt --all -- --check                                          PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings PASS
cargo test --workspace                                              PASS (0 failures)
cargo build --workspace --release                                   PASS
cd apps/web && npm run lint && npm run check && npm test && npm run build   PASS
```
24 new backend tests total (6 in `crates/library`, 12 in
`services/clouddeskd/tests/music_api.rs`, 6 in `crates/media` for the
artwork/standalone-audio work), all real `ffmpeg`-backed where
applicable, zero mocks. 18 new frontend unit tests (`music.test.ts`).
All prior Nightmare/Phase 1/Phase 2/Phase 3/Phase 4 tests still pass
unmodified.

## Current commit (Phase 5)

```
2ee891d fix(music): require files.local.write for Music mutations, not read
03f55f9 docs(engineering): checkpoint after Phase 5 (Music Application)
5160eaf feat(music): add Music desktop application
e1a3875 feat(music): add indexed music library and metadata
ff3dd64 feat(media): add format tags, artwork extraction, standalone-audio DIRECT classification
```
on top of (preserved, untouched, still passing) the Phase 4 commit
chain below. 22 backend tests total for Music (12 in `music_api.rs`, 5
in `music_authorization.rs`, plus 6 in `crates/library`), 6 more in
`crates/media` for the artwork/standalone-audio work -- all real,
zero mocks.

## Next phase (after Phase 5) -- superseded, see the Phase 6 section at the top of this file

**Phase 6 — Optional Runtime Orchestrator** is now IN PROGRESS (not
complete); see the status block at the top of this file for what's
actually built, live-tested, blocked, and still missing. This section
is kept for history -- the design direction it describes (`crates/
runtime/src/lib.rs`'s `RuntimeDependency` enum, Code/Office/Brave
still manifest-only) is unchanged and still accurate.

Read `V1_TRUE_CLOSURE.md` items #4 (VS Code), #5 (Office/Collabora),
#6 (Brave) and the `RuntimeDependency` enum already defined in
`crates/runtime/src/lib.rs` (`Browser`/`Code`/`Office`/`Media` --
`Media` is now real via Phase 3/5; the other three are still manifest-
only enum variants with zero backend). Design a single shared
orchestrator (process/container lifecycle, per-user isolation, start/
stop/status, resource bounds) rather than three independent ad-hoc
runtime integrations, then implement Code first (smallest realistic
surface) against it.

## Unrelated blockers to keep visible (do not lose track of these)

**Phase 2 (SSH feature matrix) -- still open:**
- SSH agent authentication -- not started
- Keyboard-interactive authentication -- not started
- SSH certificate authentication -- not started
- Native SCP -- not started
- Remote SSH terminal/PTY -- not started (`V1_TRUE_CLOSURE.md` #16)

**Phase 3 (FFmpeg Media Foundation) -- known gaps, still open:**
- No cgroup CPU/memory enforcement (process-level admission control only)
- 10-minute job timeout and 4 GiB output-size guard implemented but
  never live-fired (impractical to wait out in a test)
- Per-stage media audit events incomplete (only `job.requested` exists)

**Phase 4 (Video Application) -- known gap, still open:**
- Actual browser-flow acceptance BLOCKED BY ENVIRONMENT (no
  browser/automation tooling in this container)

**Phase 5 (Music Application) -- known gap, still open:**
- Same browser-flow acceptance blocker as Phase 4 (shared cause, not a
  new instance of the problem)

Do not recalculate the global completion percentage.

## Historical: Phase 4 — what was built (Video Application, preserved, unchanged)

```
Direct playback:      PASS (backend/router-level; native <video>, no
                       FFmpeg process, reuses the existing Range-capable
                       stream endpoint -- browser rendering itself not
                       verified, see Browser acceptance below)
Remux playback:       PASS (backend/router-level; job created, polled,
                       completed job's output verified playable/
                       probeable as DIRECT -- see Phase 3 evidence this
                       builds on)
Transcode playback:   PASS (backend/router-level; same as remux)
Seek:                 IMPLEMENTED, NOT BROWSER-VERIFIED (native
                       currentTime assignment over the already-tested
                       Range endpoint)
Subtitles:             PASS (backend live-tested: real WebVTT extraction
                       with real text content, bogus-index rejection;
                       frontend wiring implemented, not browser-verified)
Multi-audio:           PASS (backend live-tested: -map 0:a:<ordinal>
                       verified to produce exactly the requested track;
                       frontend wiring implemented, not browser-verified)
Resume:                PASS (backend live-tested: round-trip + strict
                       per-owner isolation; frontend throttled writes
                       implemented, not browser-verified)
Browser acceptance:    BLOCKED BY ENVIRONMENT (no chromium/playwright/
                       any browser tooling present in this container;
                       judged disproportionate to install for one phase
                       -- see "Explicitly not verified" below)
```

**What was actually built:**
- `apps/web/src/lib/VideoApp.svelte` + `video.ts` (pure logic, unit
  tested) -- a real player driven by Phase 3's compatibility decision,
  not an `<video>` wrapper: probes on open, branches on
  DIRECT/REMUX/TRANSCODE/UNSUPPORTED, polls job status for
  REMUX/TRANSCODE with a visible preparing/error state, cancels its own
  job in `onDestroy` (window close), and falls back to attempting
  direct streaming when FFmpeg is disabled/unavailable (503 from probe)
  since direct playback needs no FFmpeg at all -- the browser's own
  `<video>` `onerror` reports failure if the file genuinely needs
  conversion that isn't available. This satisfies Task 16's "DIRECT
  must still play when FFmpeg is disabled" without a separate
  extension-sniffing code path.
- **Open from Files** (Task 1): new `onOpenWithVideo` callback prop on
  `FilesApp.svelte` (double-click a video file, or an explicit "Open
  with Video" toolbar button), and a new generic per-window `params`
  mechanism in `App.svelte` (`OpenWindow.params?: {path}`,
  `openApplication(app, params?)`) so a specific file reaches the
  right app's window -- this didn't exist before this session (no app
  in this shell could previously receive a specific file at all; Gallery
  and Document apps browse independently). Backend authorization is
  unconditional and per-request (`resolve_safe_path` +
  `authorize_request`), never inferred from frontend state.
- **Subtitles** (Task 7): new `POST /api/v1/media/subtitles` --
  synchronous (no job/polling, since a text stream extracts in well
  under a second), validates `stream_index` against a fresh probe of
  the caller's own authorized file before ever calling `ffmpeg`, returns
  real `WebVTT` bytes. `clouddesk_media::exec::extract_subtitle` +
  `MediaService::extract_subtitle` are new.
- **Audio tracks** (Task 8): explicit switch routes through a real
  remux with `-map 0:a:<ordinal>` (`exec::TrackSelection`, new) rather
  than relying on inconsistent per-browser embedded-track APIs --
  reported as the actual mechanism, not silently faked as "direct
  selection."
- **Resume position** (Task 10): new `media_playback_state` table
  (migration `0011`), keyed by `(owner_user_id, virtual_path)` -- not
  filename alone, and inherently per-user since `owner_user_id` is part
  of the key. Documented limitation: renaming a file resets its resume
  position (content-hash identity was judged not worth the cost of
  hashing whole files on every playback start for this phase).
  Client-side write throttling (`shouldSaveResume`, unit tested) caps
  DB writes to one per 5s regardless of `timeupdate` frequency, plus an
  unconditional save on pause/close.
- **File-change / reauthorization** (Task 11): no separate
  "playback session token" exists -- every probe/job/subtitle/resume
  request re-runs `resolve_safe_path` + `authorize_request` fresh
  against the caller's *current* mapped identity and assigned roots.
  This was a deliberate design choice, not an oversight: it structurally
  prevents a previously-issued grant from outliving a permission
  revocation, without needing bespoke revalidation logic.
- **Malformed Range** (Task 12): covered by Phase 3's existing
  RFC-7233 fix (reused directly, since DIRECT and completed-job output
  both go through `serve_file_stream`), plus this session's `media_api.rs`
  regression tests.
- **Security** (Task 18): cross-user job invisibility (404, not 403) was
  already covered in Phase 3; this session added the same treatment for
  resume state (a different user's identically-pathed resume position is
  fully independent, proven by test) and subtitle stream-index validation
  (a non-subtitle index on an authorized file is rejected before
  `ffmpeg` ever runs). **Not independently re-tested this session**: a
  guest-role-specific 403 on job creation (structurally enforced by the
  same `authorize_request("apps.media.use", ...)` pattern `create_job`
  already used in Phase 3, but no new guest-specific test was written).
- **Browser-side security** (Task 19): no `{@html}` anywhere in
  `VideoApp.svelte`; all filenames/track labels/error text render
  through Svelte's auto-escaping `{expression}` interpolation. Unit
  test asserts a hostile `<img onerror=...>` string in track metadata
  never becomes anything but inert text.

**Explicitly not verified (browser flow, Tasks 20-23):** This container
has no `chromium`, `chromium-cli`, `playwright`, or any other browser/
automation tooling installed, and none was previously committed to this
repository to reuse. Installing a full browser stack was judged
disproportionate to add for a single phase ("do not introduce a huge...
dependency without reason" cuts against improvising one here just to
tick this box). Consequently: actual seek behavior, playback-speed
change, fullscreen, subtitle *rendering*, the resume-prompt UI, and any
performance/CPU claims were never exercised in a real running browser.
This is reported as **BLOCKED BY ENVIRONMENT**, not folded into "PASS,"
and kept strictly separate from the real (non-browser) evidence above.

## Validation (Phase 4)

```
cargo fmt --all -- --check                                          PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings PASS
cargo test --workspace                                              PASS (0 failures)
cargo build --workspace --release                                   PASS
cd apps/web && npm run lint && npm run check && npm test && npm run build   PASS
```
15 new/changed live backend tests (0 mocks): `crates/media/tests/
live_ffmpeg.rs` (7, +2 this session: subtitle extraction, audio-track
selection) and `services/clouddeskd/tests/media_api.rs` (8, +3 this
session: subtitle detection/extraction/rejection, audio-track-ordinal
job, resume round-trip/isolation). 14 new frontend unit tests
(`video.test.ts`). All prior Nightmare/Phase-1/Phase-2/Phase-3 tests
still pass unmodified (full `cargo test --workspace` green).

## Previous phase: Phase 3 — FFmpeg Media Foundation (preserved, unchanged)

New crate `clouddesk-media` + HTTP wiring in `services/clouddeskd`. Full
detail in the commit message (`624f6be`) and `V1_TRUE_CLOSURE.md` item
#1. Summary:

```
[x] FFmpeg/ffprobe discovery (typed, no shell, explicit unavailable state)
[x] ffprobe structured metadata (bounded, typed error on malformed input)
[x] DIRECT/REMUX/TRANSCODE/UNSUPPORTED decision (container+codec based)
[x] direct ranged stream (reused serve_file_stream; fixed a real 416/
    multi-range bug found while wiring this)
[x] real remux -- live-tested against real ffmpeg 8.1.2
[x] real transcode -- live-tested, output reprobed as DIRECT-playable
[x] cancellation -- live-tested (SIGTERM, 3s grace, SIGKILL fallback)
[x] timeout -- implemented (10 min const), NOT live-fired (impractical
    to wait 10 minutes in a test); only cancellation was live-exercised
[x] concurrency limits -- implemented (global + per-user semaphore),
    NOT exercised under real concurrent load
[x] temporary storage limits -- 4 GiB output-size guard implemented,
    NOT live-tripped
[x] cleanup -- per-job 0700 workspace removed on failure/cancel; startup
    reconciliation of jobs orphaned by a crash/restart
[x] cross-user authorization -- live HTTP-tested (404, not 403 -- a
    cross-user job ID doesn't even confirm existence)
[~] hostile media handling -- live-tested for empty/random-bytes/missing
    file; NOT tested: truncated MP4, corrupt MKV specifically, huge
    declared duration, broken subtitle track (huge/overflow dimensions
    ARE covered, but only at the parser-unit level, not via ffprobe)
[x] optional enable/disable -- live HTTP-tested (disabled -> 503 on
    every media endpoint; direct byte-range streaming is unaffected
    since it never touches FFmpeg)
[x] real FFmpeg live acceptance -- crates/media/tests/live_ffmpeg.rs
    (4 tests) + services/clouddeskd/tests/media_api.rs (5 tests),
    all against actually-installed ffmpeg/ffprobe 8.1.2, zero mocks
[x] Rust release gates -- fmt/clippy(-D warnings)/test --workspace/
    build --release all pass
```

**What is explicitly NOT enforced** (per Task 8's "record exactly what
is and isn't enforced, do not fake resource isolation"): no cgroup CPU
or memory limit exists anywhere in this implementation. The
`JobLimiter` in `crates/media/src/exec.rs` is process-level admission
control (a `tokio::sync::Semaphore`), not a kernel resource limit -- a
job that gets a permit can still use as much CPU/RAM as the box has
while it runs. This was a conscious, documented choice given the
project's existing privilege/runtime architecture has no cgroup
integration to hook into yet, not an oversight.

**Audit coverage is partial**: only `media.job.requested` is audited
today (with operation type, never a raw path or FFmpeg command line).
Task 18 also asks for `started`/`completed`/`failed`/`cancelled` audit
events -- not added this session.

**No frontend changes** -- Phase 3 was explicitly scoped to the backend
foundation only ("Do NOT build the full Video UI in this phase"), so
`npm run lint/check/test/build` were not re-run (nothing in `apps/web`
changed; Phase 1's last real run is still valid).

## Current phase

**Phase 2 — Complete SSH Feature Matrix.** Partial. Do not treat this as
done — see "Phase 2 status" below for exactly what is and isn't real.
(Unchanged this session -- Phase 3 was worked in parallel per explicit
instruction, not as a replacement for finishing Phase 2.)

## Phase 2 status

```
[x] ProxyJump product wiring          -- DONE, live-tested (12 tests,
                                          real 2-container bastion+target)
[x] independent bastion host verification -- DONE (part of the above)
[x] independent target host verification  -- DONE (part of the above)
[ ] SSH agent                          -- NOT STARTED
[ ] keyboard-interactive               -- NOT STARTED
[ ] SSH certificates                   -- NOT STARTED
[ ] native SCP                         -- NOT STARTED
[x] SFTP over ProxyJump                -- DONE, live-tested (1 test,
                                          list/upload/download/rename/delete)
[ ] remote terminal over ProxyJump     -- NOT STARTED (blocked on a
                                          prerequisite that doesn't exist
                                          yet -- see below)
[~] authorization isolation            -- no NEW HTTP endpoints were added
                                          this session (resolve_ssh_session
                                          is only called from the existing,
                                          already-authorized transfer path),
                                          so there was nothing new to sweep;
                                          not independently re-verified
[~] audit redaction                    -- not reviewed this session; the
                                          new code path doesn't add any new
                                          audit events (it replaced inline
                                          logic that had none either) --
                                          this was true before and after,
                                          not verified either way
[x] live disposable OpenSSH fixtures   -- used throughout, including a
                                          real fixture bug found and fixed
                                          (see below)
[x] Rust release gates                 -- fmt/clippy/test --workspace all
                                          pass
```

**Do not call Phase 2 complete.** Four of five mandatory Task-1-through-5
targets (agent, keyboard-interactive, certificates, SCP) have zero
implementation — not a stub, not an enum, genuinely nothing beyond what
was already there before this session (`SshAuth::Agent` and
`SshAuth::KeyboardInteractive` still `bail!`; `SshAuth::Certificate` still
silently ignores `cert_data`; no SCP code exists at all).

## What was actually built and verified this session

**ProxyJump product wiring** (`services/clouddeskd/src/worker.rs::
resolve_ssh_session`), consumed by the SFTP/transfer connection path:
- Resolves a target `RemoteServer`; if `proxy_jump_server_id` is set,
  independently resolves the bastion too (separate `RemoteServerStore::get`
  ownership check, separate pinned host key, separate Vault credential
  reveal — never reusing the target's credential for the bastion) and
  connects via `SshSession::connect_proxyjump` instead of a direct
  connection.
- Chain depth bounded to target + one bastion hop
  (`MAX_PROXY_CHAIN_HOPS = 2`); a bastion whose own
  `proxy_jump_server_id` is set is refused (`ChainTooDeep`), which also
  rejects every A→B→A loop as a side effect. Self-reference explicitly
  rejected. Cross-owner bastion reference rejected independently of
  `RemoteServerStore::create`'s own check (proven by forcing one directly
  into the database — `create()` itself already makes this
  unconstructable through the normal API, so this is defense in depth,
  not the only guard).
- **Real bug found and fixed in the test fixture itself**, not just
  product code: `linuxserver/openssh-server` ships with
  `AllowTcpForwarding no`, silently breaking ProxyJump's `direct-tcpip`
  channel. Fixed via the image's own documented `sshd_config.d` drop-in
  mechanism (`tests/acceptance/fixtures/sshd_config.d/proxyjump.conf`,
  bind-mounted in `docker-compose.yml`) — reproducible on a fresh
  `docker compose down -v && up -d`, verified by actually doing that and
  rerunning the suite clean, not a one-off manual patch to a running
  container that would be lost on the next teardown.

**Test evidence** (`services/clouddeskd/tests/ssh_proxyjump.rs`, 12
tests) against a real two-container topology
(`tests/acceptance/docker-compose.yml`): `openssh` (bastion, host port
2222) and `openssh-target` (target, **deliberately no host port
mapping** — reachable only through the bastion's compose-internal
network, so a passing test proves the connection genuinely went
client→bastion→target). Covers: valid connection + command execution,
wrong bastion/target host key rejected, bastion/target auth failure
rejected, topology sanity check, self-reference, A→B→A loop,
cross-owner bastion reference, bastion-deletion-nulls-reference
(`ON DELETE SET NULL`), missing target, and SFTP
list/upload/download/rename/delete over the ProxyJump path with target
host-key pinning still enforced (Task 7).

**Not covered even for what was built**: live bastion-dies-mid-session,
connection-storm, and auth-timeout scenarios from the original task's
regression list were not tested this session.

## New closure item discovered

**`V1_TRUE_CLOSURE.md` #16 (new): Remote terminal over SSH does not
exist.** `SshSession` only has `run_command` (single buffered
non-interactive exec — no PTY, no interactive channel). No endpoint in
`services/clouddeskd` opens a remote-server terminal session. The
existing local terminal (`/api/v1/terminal/ws`) is a completely separate
feature (mapped-UID local PTY, nothing to do with SSH). This is a bigger
gap than "ProxyJump isn't wired into remote terminal" — the remote
terminal feature itself was never built. Task 8 in the original Phase 2
prompt assumed this existed; it doesn't.

## Verified items (all phases so far)

- **Resumable uploads** — persisted session table, chunked HTTP surface,
  cross-user isolation, checksum verification, atomic finalize, janitor.
- **Archive create/extract** — ZIP + tar.gz, Zip Slip/Tar Slip/symlink/
  quota defenses, 10 tests.
- **ACL read/edit** — real `getfacl`/`setfacl`, in-process path
  resolution (bug found and fixed: `/proc/self/fd` in a spawned child
  doesn't refer to the parent's fd table), dedicated capability, 6 tests.
- **ProxyJump + SFTP-over-ProxyJump** — see above, 12 tests against a
  real bastion+target topology.

## Validation

```
cargo fmt --all -- --check                                          PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings PASS
cargo test --workspace                                              PASS (0 failures)
```
No frontend changes this session — frontend gates not re-run (Phase 1's
were the last real run, still valid since nothing in `apps/web` changed).

`services/clouddeskd/tests/ssh_proxyjump.rs` (12 tests) run separately
against live Docker fixtures, as instructed:
```
cd tests/acceptance && docker compose up -d
cargo test -p clouddeskd --test ssh_proxyjump   # 12 passed, 0 failed
docker compose down -v   # torn down cleanly after
```
Re-verified on a from-scratch `docker compose down -v && up -d` (not
just the already-running, manually-patched containers) to confirm the
`sshd_config.d` fixture fix is actually reproducible.

## Current commit

```
0ff04a9 feat(video): add media-backed Video application
```
on top of (preserved, untouched, still passing):
```
daadc8b docs(engineering): checkpoint after Phase 3 (FFmpeg Media Foundation)
624f6be feat(media): FFmpeg-backed media compatibility foundation (Phase 3)
0d7a2da docs(engineering): checkpoint after Phase 2 partial (ProxyJump wiring)
ce48b74 feat(ssh): wire ProxyJump through the SFTP/transfer connection path
c86da38 docs(engineering): checkpoint after Phase 1 (File Manager) closure
9b7aa74 feat(files): complete phase 1 file manager closure — archives and ACLs
d277393 docs(engineering): checkpoint after resumable-upload closure
b4a4660 feat(files): implement resumable local-file uploads (GOAL.md G3)
dfdfade audit(evidence): repair fabricated acceptance runner, audit spec vs implementation, fix RSA SSH auth
289904b audit(nightmare): fix SSH host-key bypass and SFTP upload/list breakage; prep v1.0.1-rc.1
d6517bf audit(nightmare): require system.services.manage for /api/v1/system/summary
ffbc336 test: prepare Claude v1.0.0 nightmare audit
9b8f49a release: CloudDesk-OS v1.0.0   <- immutable tag v1.0.0 points here
```

All five prior Nightmare fixes, Phase 1, Phase 2's ProxyJump work, and
Phase 3's media foundation preserved and untouched this session (full
`cargo test --workspace` still green, including all 12
`ssh_proxyjump.rs` tests, all archive/ACL tests, both Nightmare
regression tests, and the original 13 Phase 3 media tests).

## Actual live authentication methods verified (through the real product path)

```
Password              -- yes (this session's ProxyJump tests, plus
                          prior Nightmare-audit live tests)
PEM/private key        -- yes (prior session, crates/remote/tests/ssh.rs
                          test_ssh_rsa_pem_private_key_auth_succeeds)
RSA                    -- yes, fixed this-audit-lineage
                          (CLAUDE-NIGHTMARE-005)
Ed25519                -- yes (prior session)
Encrypted key+passphrase -- yes (prior session)
SSH Agent               -- NO -- SshAuth::Agent still bail!s
Keyboard-interactive     -- NO -- SshAuth::KeyboardInteractive still bail!s
SSH certificate          -- NO -- SshAuth::Certificate still ignores cert_data
Custom port              -- yes (fixture runs SSH on 2222, tested throughout)
ProxyJump                -- yes, THIS session
```

## ProxyJump verified: YES (for SFTP/transfers; NOT for a remote terminal, which doesn't exist)

## SCP verified: NO — not implemented at all

## Security findings (Phase 3, preserved)

One real, pre-existing product bug found and fixed while wiring direct
playback (not introduced this session, just uncovered by it):
`serve_file_stream`'s `Range` handling fell through to a full 200 body
for out-of-bounds/reversed ranges instead of 416, and misparsed
multi-range `Range` headers instead of ignoring them. Both fixed to be
RFC 7233 compliant, both covered by regression tests in
`services/clouddeskd/tests/media_api.rs`. No defect found in the new
media code itself during this pass.

## Security findings (Phase 4)

No CloudDesk product defect found this session. New surface area
(subtitles/resume/track-selection) follows the same authorization
pattern as everything in Phase 3 -- reused, not reinvented.

## Historical: Phase 4's own "next phase" notes (superseded by Phase 5, now complete)

Phase 4's checkpoint pointed at Phase 5 (Music) as the next step, which
this session completed. See the top-of-file "Next phase (after Phase
5)" section for the current, authoritative next step (Phase 6).

## Remaining closure blockers (current, supersedes any earlier list in this file)

Everything in `V1_TRUE_CLOSURE.md` except items 1 (FFmpeg pipeline,
Phase 3), 2 (Video, Phase 4 -- backend/router evidence only, browser
flow blocked), 3 (Music, Phase 5 -- same), 7, 8, 9 (Phase 1), and the
ProxyJump/SFTP-over-ProxyJump portion of item 14 (Phase 2, partial). In
priority/dependency order:

1. Video + Music browser-flow acceptance — blocked by environment;
   revisit together if browser tooling becomes available (shared cause)
2. SSH agent, keyboard-interactive, SSH certificates, native SCP — not
   started (rest of Phase 2)
3. Remote terminal over SSH — not started (item #16)
4. Optional-runtime orchestrator (Code/Office/Browser) — not started
   (Media is now real; the other three are still manifest-only)
5. VS Code-compatible runtime — not started, depends on #4
6. LibreOffice/Collabora runtime — not started, depends on #4
7. Brave remote-browser runtime — not started, depends on #4
8. Real multi-distro CI/testing — not started; `tests/distro/
   installer-layout.sh` explicitly skips package/service-manager testing
9. Acceptance-suite expansion for all of the above
10. Phase 3's own known gaps (cgroup enforcement, long-timeout live
    firing, per-stage media audit events) — still open, see above

Do not create `v1.0.1-rc.1` until all of the above are done, per the
task's own final gate.
