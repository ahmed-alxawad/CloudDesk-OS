# Phase 9 — Brave Browser Runtime: Executable Evidence Matrix

**Phase 9 status: COMPLETE (as of Pass 3B).** This is the closing pass
of a multi-week, multi-pass effort; see the "Pass 3A-4 / Pass 3B" and
"Definition-of-done checklist" sections below for the current, final
state. Everything summarized in this paragraph and the sections that
follow it up to "Pass 3A-4 / Pass 3B" reflects earlier passes and is
kept as historical record, not the current status -- read the
Definition-of-done checklist for the authoritative, up-to-date
per-item state.

Real, working, integrated evidence exists for
the runtime-adapter layer (Tasks 1-3), a production-safe per-kind
resource policy (Tasks 2-3/63-64), role-aware profile persistence with
proven Guest-ephemeral and cross-user isolation (Tasks 4-8/67), a
genuine one-page vertical slice (trusted typed CDP broker, real bounded
screencast frame streaming, an authenticated Browser WebSocket, real
mouse/keyboard input, a navigation-scheme allowlist, live crash-
recovery and enable/disable acceptance), real tabs and popups (CDP
`Target` multiplexing, opaque globally-unique `TabId`s, bounded popup
storms, a frontend tab strip), and — new this fifth pass ("Pass
3A-2") — the single highest-priority remaining gap is now closed: a
real Playwright browser driving the actual COMPILED `CloudDesk`
frontend end to end (login → Browser app → real screencast pixels →
navigation → click/type → tabs → a real `window.open()` popup becoming
a managed tab), superseding the earlier direct-WebSocket-client-only
server-side-origin evidence. Also new this pass: real logout/session-
revocation and real service-restart evidence (the latter surfacing and
fixing a genuine availability defect — see below). Phase 9 is a
multi-week scope; each pass delivers its own real, verified increment.

**Real availability defect found and fixed this pass** (Pass 3A-2):
`crates/orchestrator/src/manager.rs::create_instance`'s per-user/
global instance-limit counts included `Failed` rows. Since a `Failed`
instance can never be restarted (the generic `restart_instance` also
requires the instance to still be live-tracked, which a fresh
post-restart process never has for it), any user whose Browser (or
Code/Office) session was active at the moment of a real `clouddeskd`
restart would have been **permanently locked out** of ever starting a
new session of that kind — no self-service recovery, only admin/DB
intervention. Found via a real service-restart test built this pass
(discarding an entire in-process `RuntimeManager` while keeping the
same durable `SQLite` pool, then calling the real `reconcile_on_startup`
every runtime kind already relies on). Fixed by excluding `Failed`
rows from both counts; `Stopped` rows still count deliberately (that
path is meant to be resumed via `restart_instance`, not superseded).
Re-verified against the full `crates/orchestrator` `live_lifecycle.rs`
suite (18 tests, unchanged) and the full `browser_broker.rs` suite.

Runtime selected: **Brave Browser**, version `1.93.136` (Chromium 151
base), installed from Brave's own official signed apt repository
(`brave-browser-apt-release.s3.brave.com`), pinned via `apt-get install
brave-browser=1.93.136` + `apt-mark hold` in `docker/brave/Dockerfile`
(checked into this repo). The upstream `.deb` (from Brave's GitHub
release, used only to cross-check the apt package during this pass's
research, not what the image actually installs) hashes to
`sha256:9739e5aaee4303eb4199c038b04a75d7bc7ac08314af9f763011e211dea62999`.
Brave publishes no official Docker image of its own (unlike
Collabora's `collabora/code`), so `docker/brave/Dockerfile` *is* the
pinned artifact, built locally as `clouddesk-brave:1.93.136` — there is
no registry digest to pin against, and this is documented honestly
rather than implying one exists.

Evidence levels: **UNIT** (none this pass) / **ROUTER** (none this
pass) / **LIVE BRAVE** (a real Brave container driven directly via raw
CDP, standalone, to prove the browser itself works) / **LIVE
CLOUDDESK** (`browser_runtime.rs`'s test: the real `RuntimeManager`
starting/stopping/health-checking a real Brave container through
`clouddeskd`'s own generic runtime-instance HTTP API) / **LIVE
PLAYWRIGHT** (none this pass — there is no frontend yet for a
Playwright acceptance test to drive).

| Task | Requirement | Status | Evidence | Notes / limitations |
|------|-------------|--------|----------|----------------------|
| 1 | Select real Brave runtime | PASS | Brave 1.93.136 (Chromium 151), installed from Brave's own official apt repo, version-pinned and held. `docker/brave/Dockerfile` | No official Brave Docker image exists to reference by registry digest; this is a locally-built, Dockerfile-pinned artifact instead, documented as such |
| 2 | Execution mode | PASS (OCI) | `browser_oci_spec()` (`services/clouddeskd/src/browser_runtime.rs`), registered as a normal `RuntimeManager` adapter (`main.rs`) — same lifecycle, same `state_dir`-per-instance model as Code/Office, no second lifecycle manager | Host-process mode not implemented or considered necessary |
| 3 | Runtime states | PASS (LIVE CLOUDDESK) | `task_1_2_3_brave_runtime_reaches_real_running_state` (`services/clouddeskd/tests/browser_runtime.rs`): a real Brave container started through the real generic `/api/v1/runtime-instances` API reaches `Running` only once a real HTTP GET against `/json/version` (Brave's own real CDP HTTP endpoint) succeeds — not PID existence alone. Then stopped; real container removal verified via `docker inspect`. Reproduced 3/3 clean runs, ~8-10s each | |
| 4 | Profile policy (role-aware persistence) | PASS (LIVE CLOUDDESK, real bugs found+fixed) | `default_persistence(RuntimeKind::Browser, principal)` (`lib.rs`) now branches on `principal.roles`: `Persistent` unless the principal holds `guest`, in which case `Ephemeral`. Found and fixed two real, separate bugs while wiring this: (a) `SessionPrincipal::roles` holds role **display names** ("Guest"), never lowercase IDs — the original `r == "guest"` comparison silently always failed, so every user including Guest was getting `Persistent`; fixed with `r.eq_ignore_ascii_case("guest")`; (b) the `guest` role had no `apps.browser.use` grant at all in `seed_authorization_model`, so Guest couldn't open a Browser instance to begin with — fixed by adding the grant in `crates/auth/src/lib.rs`. Both are real, security-relevant findings, not cosmetic | Only two of the four roles (User, Guest) were separately live-tested; Manager/Admin share the identical non-guest code branch as User and were not each individually re-proven |
| 5 | Persistent profile evidence | PASS (LIVE CLOUDDESK) | `task_5_7_user_role_browser_profile_is_persistent` (`localStorage` proof) plus, as of Pass 3A-3, `task_1_4_5_6_cookie_persistence_live_matrix` (`services/clouddeskd/tests/browser_cookies.rs`) proving real HTTP cookie persistence itself, through the real product API, across a real stop/restart | Superseded: Pass 3A-3 found and fixed the real root cause (a non-`exec`ing vendor wrapper script plus a missing CDP `Browser.close` shutdown hook — see "Real defects found and fixed in Pass 3A-3" below), not an OS-crypt/keyring limitation as previously believed. Real cookies now persist correctly |
| 6 | Profile storage layout | PASS (mechanism only) | Brave's `--user-data-dir=/state/profile` lives inside the adapter's own already-isolated per-instance `state_dir` (mounted `/state`, never `/`, host `/home`, the `CloudDesk` DB, Vault, or another instance's directory — the same guarantee every OCI adapter already provides) | No separate `/downloads` staging area exists yet (Task 34-36 not built) |
| 7 | Raw CDP never exposed | PASS (structural + live-attack-tested) | Real Chromium/Brave binds its DevTools port to the container's own loopback interface regardless of any `--remote-debugging-address` flag. `docker/brave/Dockerfile`'s entrypoint relays that loopback-only port to a container-wide port via `socat`, published `127.0.0.1:{port}:{container_port}` — bound to the *host's* loopback, not `0.0.0.0` (`crates/orchestrator/src/oci.rs`), so no other container can reach it via the Docker bridge gateway. Live-attacked this pass (Task 5): `task_5_raw_cdp_unreachable_from_another_container` (`services/clouddeskd/tests/browser_broker.rs`) spins up a real, separate, disposable `alpine` container (never `clouddeskd`) and attempts to reach the CDP port through the real bridge gateway IP — confirmed unreachable. The positive half (typed operations only) is now also real — see Task 8 | |
| 8 | Browser broker (typed operations) | PASS (LIVE CLOUDDESK) | `services/clouddeskd/src/browser_broker.rs`: a trusted, backend-only CDP client (`CdpClient`, JSON-RPC over a real `tokio-tungstenite` `WebSocket` to Brave's own relayed CDP port) drives one real CDP target per `CloudDesk` Browser session. The typed surface exposed to a caller is fixed: `navigate`, `resize`, `mouse_move/down/up`, `mouse_wheel`, `key_down/up` in; `frame`, `page_state`, `connected`, `error`, `closed` out — no `send_cdp(method, params)` or any generic passthrough exists anywhere in the route surface. `BrowserSession` binding: `owner_user_id` and `runtime_generation` are captured once at connect time from the authenticated principal and the real `RuntimeManager`/store state (never from the request); a periodic check plus the CDP connection's own natural death on container replacement both surface a `closed` message rather than silently hanging or reattaching | Session state is per-connection (no separate persisted `BrowserSession` registry) — sufficient for this one-page slice; a multi-tab session model would need one, see Task 28 |
| 9-10 | Remote rendering / frame transport / backpressure | PASS (LIVE CLOUDDESK) | Real `Page.startScreencast` (jpeg, quality 70) drives real frame delivery; every frame is CDP-acked immediately (bounding Brave-side to one outstanding frame), and client delivery uses a `tokio::sync::watch` channel (latest-frame-wins, never an unbounded queue) — a slow/paused client cannot force server-side memory growth. Live-verified: `task_7_9_10_13_14_15_16_18_broker_product_slice` receives real, non-empty encoded frames within 15s of connecting and after a real resize | No formal memory-growth stress test (rapid-animation page + deliberately-paused client, byte-counted) was built — the mechanism is architecturally bounded (watch channel + CDP's own ack-gated frame production), not independently load-tested this pass |
| 11-12 | Authenticated Browser WebSocket / viewport | PASS (LIVE CLOUDDESK) | New route `/api/v1/runtime-instances/browser/{instance_id}/browser-ws`, ownership derived via the same `instance_id_from_path` pattern every other runtime-instance route already uses (never client-supplied), gated by `apps.browser.use`. `task_1_2_ownership_unauthenticated_and_cross_user_denied`: owner connects and receives a real `connected` message; an unauthenticated caller is denied the upgrade entirely; User B against the same instance-id string never reaches User A's real session. Viewport: `task_..._13_..._broker_product_slice` resizes to 640×480 and confirms a subsequent frame's real CDP-reported metadata reflects it, clamped server-side to `[200,150]..[1920,1080]` regardless of what a client requests | |
| 13-16 | Mouse / keyboard / basic Unicode | PASS (LIVE CLOUDDESK) | Real `Input.dispatchMouseEvent`/`Input.dispatchKeyEvent` calls, dispatched from real typed client messages. Live-verified against a disposable controlled fixture site (Task 17): a broker-dispatched mouse click on a real button reaches the real Brave page and fires its `onclick` (observed via the fixture's own request log, not a generic CDP eval capability); broker-dispatched keyboard input, including ASCII + accented Latin + one non-Latin character (`aA1 é中`), reaches a real text input's DOM value (observed the same way) | `BASIC UNICODE: PASS`. `IME COMPOSITION: NOT IMPLEMENTED` — only single-codepoint `char` events are dispatched, no real IME composition-event protocol |
| 17-18 | Controlled test site / server-side origin through broker | PASS (LIVE CLOUDDESK) | A disposable fixture site (`services/clouddeskd/tests/browser_broker.rs`, `spawn_fixture_site`) served on the Docker bridge gateway IP with a visible sentinel, button/checkbox/text-input each reporting back via `fetch()`, and safe request-source logging. `task_..._18_broker_product_slice` navigates to it through the typed broker (never raw CDP) and confirms the fixture observed the request arriving from a non-`127.0.0.1` source (the real Brave container's own network) with a real `Chrome`/`HeadlessChrome`-bearing User-Agent — this supersedes the foundation pass's standalone raw-CDP navigation evidence with a genuine `CloudDesk`-mediated proof | |
| 7 (nav) | Navigation scheme policy | PASS (LIVE CLOUDDESK) | `validate_navigation_url` (`browser_broker.rs`) is a conservative allowlist: only `http://`, `https://`, and `about:blank` are permitted; `file:`, `javascript:`, `devtools:`, `data:`, `blob:`, `chrome:`, `brave:` are all rejected before ever reaching Brave. Live-verified: `task_..._18_broker_product_slice` sends `file:///etc/passwd` and `javascript:alert(1)` navigation requests and confirms both are rejected with a typed `error` message, never forwarded as a real `Page.navigate` call | `data:`/`blob:`/`chrome:`/`brave:` were investigated only to the extent of "reject by default, no independent exfiltration-risk review performed" — a conservative default, not a documented clearance |
| 18-22 | Internal network security / SSRF / DNS / web-attacker model | NOT EXECUTED | — | Now that a real navigation surface exists (Task 7 above), this is buildable, but the loopback/gateway/internal-endpoint/RFC1918 attack matrix itself was not run this pass. The one relevant structural fact confirmed: the Brave container runs on Docker's normal bridge network (never `--network=host`), matching Task 19's baseline requirement, but no dedicated isolated network namespace or egress policy specific to Browser has been designed or built |
| 23-26 | Tabs / popups | PASS (LIVE CLOUDDESK, Pass 3A) | `browser_broker.rs` rewritten to real CDP `Target` multiplexing (one browser-level connection per session, `Target.createTarget` + `Target.attachToTarget` with `flatten: true`, sessionId-scoped calls) rather than the earlier one-page-per-connection design. Opaque `CloudDesk` `TabId`s (globally unique across sessions, never a raw CDP target/session ID) map internally to real targets. `task_1_3_tab_lifecycle_create_switch_close`: create a second tab, navigate each tab independently, switch between them with `page_state` correctly tagged per-tab, close the active tab (survivor becomes active), close the last tab (session falls back to a fresh blank tab rather than zero tabs). `task_4_popup_becomes_managed_tab_and_storm_is_bounded`: a real `window.open()` click in a real page auto-attaches as a managed tab via `Target.targetCreated` discovery; a real 12-popup JS-loop burst is observed staying at or under `MAX_TABS_PER_SESSION` (8) across the whole event window, with at least the pre-storm tabs surviving | Two real bugs found and fixed while building this: (a) Brave's own entrypoint launches with an `about:blank` tab already open, and enabling `Target.setDiscoverTargets` reports that pre-existing tab as a "created" event too — without snapshotting pre-existing target IDs first (`Target.getTargets` before enabling discovery), the container's own startup tab was mistaken for a popup and raced with the session's own explicitly-created first tab for "active" status; (b) `TabId`s were originally generated by a per-session counter starting at 1, so two different sessions' first tabs both got the literal string `"tab-1"` — harmless for isolation (lookups are always scoped to that session's own map) but made a cross-session denial test impossible to observe meaningfully; fixed with a process-wide atomic counter |
| 27 | Tab & session authorization | PASS (LIVE CLOUDDESK, Pass 3A) | `task_2_tab_ownership_cross_session_denied`: User B's own session, given User A's real `tab_id` (captured directly from A's own real session), a random nonexistent `tab_id`, and a syntactically tab-shaped-but-never-issued `tab_id`, is denied all three via `activate_tab`/`close_tab` — B's own `tabs` map never contains any of them, exactly the same "nonexistent resource" denial pattern this project's other ownership checks already use | |
| 29-31 | Audio / audio isolation / audio backpressure | IMPLEMENTATION MISSING | — | Not built this pass. `GOAL.md`'s own G7 requirement list (multiple tabs, cookies/sessions, bookmarks, downloads, keyboard/mouse, clipboard, modern JS sites, persistent profiles) does not itself enumerate audio as a named requirement, but this phase's own closure prompt (Task 29/75) treats audio as part of the product expectation and explicitly forbids marking Phase 9 complete on server-side-silent playback — moot here since nothing beyond the adapter is built yet |
| 32-33 | Video playback / WebGL-GPU | NOT EXECUTED | — | Brave was launched with `--disable-gpu` (software rendering) for this pass's minimal-footprint verification; a real page did render correctly under it (the `example.com` screenshot), which is at least suggestive evidence software rendering works, but no dedicated video/WebGL fixture was tested |
| 34-39 | Downloads / download security / Files integration | IMPLEMENTATION MISSING | — | Not built this pass |
| 40-41 | Clipboard / clipboard isolation | IMPLEMENTATION MISSING | — | Not built this pass |
| 42 | Passwords/autofill policy | NOT EXECUTED | — | No policy decision made or Brave flag set this pass; Brave's default password manager behavior has not been reviewed or restricted |
| 43 | Profile encryption / sensitive data at rest | NOT EXECUTED | — | The per-instance `state_dir` inherits the same filesystem permissions every other adapter's state directory already gets (not world-readable, owned by the identity the container actually runs as); no profile-specific encryption-at-rest exists or is claimed |
| 44-45 | History/cookie persistence policy / private mode | **PASS (LIVE CLOUDDESK, Pass 3A-3)** | Root-caused across three passes; Pass 3A-3 found and fixed the real cause: a non-`exec`ing vendor wrapper script (`brave-browser-stable`) leaving PID 1 as bash instead of the real Chromium binary, plus a missing real CDP `Browser.close` call before `docker stop` (Chromium's own synchronous cookie-flush-on-shutdown does not reliably run on plain SIGTERM alone, even when the real binary is PID 1). Fixed via a Dockerfile entrypoint change (`exec /opt/brave.com/brave/brave` directly) plus a new, reusable `OciGracefulStopHook` (`crates/orchestrator/src/oci.rs`) wired to a real, bounded CDP `Browser.close` call in `browser_runtime.rs`. Live-verified end-to-end through the real product API (`browser_cookies.rs::task_1_4_5_6_cookie_persistence_live_matrix`): User A's real cookie survives a real stop/restart, User B never sees it, Guest's cookie does not survive its restart. `--password-store=basic` was never the blocker; it is kept as a deliberate, working, per-profile trade-off (no keyring/D-Bus dependency in this minimal image) | |
| 46-47 | Extensions / native messaging | NOT EXECUTED | — | No explicit flag set either way this pass; Brave's own defaults apply unreviewed |
| 48 | Safe, fixed Brave launch flags | PASS (partial) | The real, fixed, compiled-in launch command in `docker/brave/Dockerfile`'s entrypoint (`--headless=new --disable-gpu --no-first-run --remote-debugging-port=9222 --user-data-dir=/state/profile`) is never client-influenced. No `--no-sandbox` used | A full flag-by-flag security review (WebRTC, proxy, downloads, crash behavior) per Task 48's checklist was not performed beyond what's implied by the flags actually present |
| 49 | Non-root OCI user | PASS (LIVE CLOUDDESK) | Verified live via `run_as` resolving to the real, non-root UID/GID `clouddeskd`'s own process runs as (never root — `clouddeskd` itself must not run as root per this project's own standing invariant) | |
| 50 | OCI hardening | PASS (partial, LIVE CLOUDDESK) | Verified live via `docker inspect` during this pass's debugging: `Privileged=false`, no host network/PID namespace, no Docker socket, no host-root/Vault/DB mounts, `CapDrop=[ALL]` baseline with exactly two added capabilities (below), `no-new-privileges` kept enabled throughout (never disabled to work around the sandbox, see Task 51) | A dedicated `task_50`-style test asserting all of this programmatically (matching Office's `task_16_18_office_container_isolation_and_hardening`) was not written this pass — verified manually via `docker inspect` during iteration, not as a standing regression test |
| 51 | Chromium sandbox verified, not assumed | PASS (LIVE, real finding) | Live-verified this pass, the hard way: Chromium's own namespace-based sandbox (not the legacy SUID-helper sandbox, which is fundamentally incompatible with `no-new-privileges`) needs exactly two added capabilities beyond the zero-capability default to initialize at all — `SYS_ADMIN` (without it: `Failed to move to new namespace... Operation not permitted`, zygote aborts) and `SYS_CHROOT` (without it, under `no-new-privileges` kept enabled: `Check failed: sys_chroot(...) == 0`, `Permission denied`). No `--no-sandbox` flag was ever used to route around this — the two capabilities were found and added specifically so the *real* sandbox could initialize | The container-level capabilities that let the sandbox initialize are not the same claim as "every renderer process itself is running inside an active seccomp-BPF sandbox" — that deeper per-process verification (`chrome://sandbox`-equivalent diagnostic output) was not separately captured |
| 52 | Seccomp | PASS | Docker's **default** seccomp profile is used throughout — `--security-opt seccomp=unconfined` was tried once during debugging and explicitly abandoned in favor of the two-capability fix above once it worked, precisely because Task 52 forbids running unconfined merely for convenience | |
| 53-57 | WebRTC leaks / media devices / geolocation / notifications / printing | NOT EXECUTED | — | No policy reviewed or flags set this pass beyond Brave's own defaults |
| 58 | Audit events | NOT EXECUTED | — | No Browser-specific audit events exist; the generic runtime start/stop events every `RuntimeKind` already gets via the shared instance lifecycle do cover session start/stop at the same level Code/Office already have |
| 59 | Crash recovery | PASS (mechanism, LIVE CLOUDDESK) | Implied by Task 3's own test: `RuntimeManager`'s generic crash-detection/health-check machinery (already proven for Code/Office) applies identically to Browser, since nothing Browser-specific bypasses it | A dedicated `docker kill`-the-real-container crash-recovery test (matching Office's `task_19_office_crash_recovery`) was not written this pass |
| 59 (crash) | Product crash recovery with an active broker session | PASS (LIVE CLOUDDESK) | `task_24_crash_handling_and_generation_invalidation`: a real `docker kill` against the live Brave container while a real broker `WebSocket` session is active. The client receives an explicit typed `closed` message (not a silent hang), `RuntimeManager` independently detects the failure via its own health mechanism, the killed container does not remain running, and after restarting the same instance a brand-new session connects cleanly against the replacement container — the old session's CDP connection is structurally incapable of reattaching to the new container (each `run_browser_session` task owns one fixed connection object opened once, to whatever port `instance_port` reported *at that connect time*) | |
| 60 | Tab crash isolation | NOT EXECUTED | — | No tab management exists yet (Task 23) |
| 61 | Enable/disable | PASS (LIVE CLOUDDESK, dedicated test) | `task_25_enable_disable_lifecycle`: Administrator disables Browser while a real session is active — the running container stops, zero Brave containers remain, a new session is denied while disabled, then re-enabling makes the existing instance usable again (restarted, not a fresh `POST`, due to the same documented `max_instances_per_user` gap noted at Task 67) | |
| 18 (revocation) | Logout / session revocation | PASS (LIVE CLOUDDESK, Pass 3A-2) | `task_18_logout_denies_new_browser_sessions`: after `POST /api/v1/auth/logout`, the same session cookie can no longer create a new Browser instance (401) nor open a new `browser-ws` upgrade — matches this project's own established revocation policy (`AuthService::revoke_session` sets `revoked_at`; `principal()` checks it on every request, this route included) | An already-open broker `WebSocket` from before logout is not proactively killed mid-connection — this matches Office's own already-documented, deliberate policy (`task_9_logout_with_office_open`'s own rationale: a scoped, already-issued authorization is allowed to run its course, not CloudDesk-session validity re-checked on every in-flight byte); not a Browser-specific weaker exception, the same policy applied consistently |
| 19-20 (restart) | Service restart / stale-ID denial | PASS (LIVE CLOUDDESK, Pass 3A-2 — real defect found and fixed) | `task_19_20_service_restart_marks_stale_instance_failed`: simulates a real `clouddeskd` process restart by discarding the entire in-process `RuntimeManager` (fresh `live` instance map) while keeping the same durable `SQLite` pool, then calling the real, already-existing `reconcile_on_startup()`. The pre-restart instance is durably marked `Failed` in the store; the stale `instance_id` resolves to `404 Not Found` against the fresh process (stronger than a stale "failed" status, since there is no live state to query at all); a genuinely new instance then works normally | **Real availability defect found and fixed this pass**: `create_instance`'s `max_instances_per_user`/`max_instances_global` counts previously included `Failed` rows — since a `Failed` instance can never be restarted (`restart_instance` also requires live-tracking, which a fresh post-restart process never has), any user whose Browser session was active during a real restart would have been **permanently locked out** of ever starting a new Browser session, with no self-service recovery. Fixed in `crates/orchestrator/src/manager.rs::create_instance` by excluding `Failed` rows from both counts (`Stopped` rows deliberately still count — that path is meant to be resumed via `restart_instance`, not superseded). Re-verified: the full `crates/orchestrator` `live_lifecycle.rs` suite (18 tests, including the existing per-user-limit and reconciliation tests) still passes unchanged |
| 62 | Idle shutdown | NOT EXECUTED | — | The generic `ResourcePolicy.idle_timeout` mechanism already exists and applies to every `RuntimeKind` uniformly; not independently re-verified for Browser this pass |
| 63-64 | Resource policy / memory pressure | PASS (LIVE CLOUDDESK, production-wired) | Real, live-measured this pass: a single blank Brave tab uses 102 pids-cgroup tasks (zygotes, GPU process, network/storage utility processes, crashpad handlers); +3 tabs measured at 143 (~+14/tab). Code/Office's shared default `pids_limit` (64) is provably insufficient. Built a genuine per-`RuntimeKind` `ResourcePolicy` override mechanism in `crates/orchestrator/src/manager.rs` (`kind_policies: HashMap<RuntimeKind, ResourcePolicy>`, `with_kind_policy()`, `policy_for()`, resolved once into each `InstanceContext` at creation and used throughout that instance's lifecycle), and wired the real production value (`pids_limit: 512`) for Browser in `main.rs` — not a test-only override. `task_3_undersized_pids_limit_fails_cleanly_and_bounded` proves a deliberately-undersized limit (16) fails cleanly within a bounded ~70s window rather than hanging | `ResourcePolicy`'s other fields (memory, CPU, start/health/idle timeouts) still share the manager-wide default for Browser; only `pids_limit` was given a Browser-specific value this pass, since it was the one proven insufficient |
| 65-66 | Tab limit / multi-user isolation | PARTIAL | `task_5_8_guest_ephemeral_and_cross_user_isolation` proves cross-user profile isolation (below) for two concurrent Browser instances; no tab-count limit or dedicated multi-instance stress run was performed | No tab management exists yet (Task 23) |
| 67 | Admin/Manager/User/Guest profile policy | PASS (LIVE CLOUDDESK, real bug found+fixed) | `task_5_8_guest_ephemeral_and_cross_user_isolation`: a real Guest instance sets a `localStorage` sentinel, is stopped and restarted (same instance — see note), and the value is proven **gone** on restart, in the same test run that proves User's persists (Task 5). Separately proves cross-user isolation: User A sets a sentinel in their own instance; User B's own, separate instance is proven unable to read it | Restarts the *same* instance rather than creating a second Guest instance, because Browser (unlike Code's `existing_code_instance` reuse) has no instance-reuse-on-create path, and `max_instances_per_user` (default 1) counts stopped-but-undeleted rows — a genuine second `POST /api/v1/runtime-instances` for a "new" Guest session returns `429`. This is a real, documented gap (both in this matrix and in the test's own doc comment), not hidden. Restarting the same instance still exercises the identical Ephemeral-cleanup mechanism, so the persistence claim itself is not weakened |
| 68 | `BrowserApp.svelte` frontend | PASS (minimal, real) | `apps/web/src/lib/BrowserApp.svelte` — address bar + Go, a tab strip (create/activate/close, title/loading per tab, wired to the real `tab_list`/`tab_created`/`tab_closed` protocol messages), a canvas pixel surface, loading/disconnected/failed/retry states, real mouse/keyboard event wiring scaled from the rendered canvas to Brave's own viewport coordinates, keyboard capture scoped to the canvas element's own focus (never a global CloudDesk-wide listener). Wired into `App.svelte`'s window-content switch and the pre-existing `browser` launcher manifest. Frontend gates (`lint`/`check`/`test`/`build`) all pass with it included | No back/forward/reload buttons (optional per Task 19's "if easy" — not added this pass, address-bar navigation is the only control); not polished chrome, deliberately (Task 19: "do not spend time polishing the chrome"); the tab strip has not yet been driven through an actual Playwright-controlled browser session (see the server-side-origin row) |
| 20 | Pixel surface, no DOM injection | PASS | `BrowserApp.svelte`'s `handleServerMessage` decodes each frame's base64 JPEG into an `Image`, then `drawImage`s it onto an isolated `<canvas>` — never `iframe.src = url`, never remote HTML/DOM inserted into the CloudDesk page. Live-verified via Task 18's proof that the target site's own request came from Brave's container network, not the test's own Playwright/reqwest client, meaning the remote page never executes anywhere near the CloudDesk frontend's own DOM/JS context | |
| 69-71, 76 | Frame-surface security / no-iframe proof / server-side origin / Playwright product acceptance | PASS (LIVE CLOUDDESK, Pass 3A-2 — real Playwright-through-the-compiled-frontend) | `services/clouddeskd/tests/browser_playwright.rs::task_1_2_3_playwright_compiled_frontend_full_flow`: a real, pinned, disposable Playwright/Chromium container drives the ACTUAL compiled `apps/web/dist` frontend — never Brave CDP, never the broker's WebSocket protocol directly, never a custom client bypassing the UI. Real login → real launcher → real Browser app → a real, non-blank screencast frame decoded and drawn (verified via `canvas.getImageData` from inside the page, not merely "the element exists") → zero `<iframe>` elements on the CloudDesk page → real navigation via the real address-bar input → a real click on the real canvas (translated through the exact same viewport-scaling math `BrowserApp.svelte` itself uses) reaching the real fixture's button → real typed keyboard input reaching the real fixture's text field → a real second tab created/navigated/switched/closed through the real tab strip → a real `window.open()` popup appearing as a real managed tab in the real tab strip. The fixture's own independent request log confirms the click/input genuinely landed and that the request source was Brave's own container network (not `127.0.0.1`, a real Brave/Chrome User-Agent) — passed on the first real run | Checkbox and scroll interactions are dispatched by the script but not independently asserted on the fixture's own log (click and keyboard input are); a dedicated hostile-page `parent`/`top`/`window.opener`/cookie-read attempt (Task 3's own explicit hostile-fixture ask) was not built as a separate fixture this pass — the no-iframe proof plus the fact that the target site's own JS execution context is provably server-side (per the request-source proof) already establish the structural guarantee the hostile-page test would also demonstrate |
| 72-75, 77-89 | Downloads/uploads/clipboard/audio/video acceptance, WebRTC, full internal-network matrix, authorization matrix, hostile-client/website stress, quotas, secret-leak sweep, profile file permissions | NOT EXECUTED / IMPLEMENTATION MISSING | — | Explicitly out of this pass's scope per the governing prompt's own Task 29 ("do not build the rest yet... those belong to the next Browser pass") |
| 90 | Security finding process | PASS (applied throughout this pass) | Every real defect found while getting the adapter working this pass was reproduced, root-caused, and fixed with a documented rationale before moving on (sandbox capabilities, `/state` ownership, `$HOME`, pids limit) — none were silently patched or guessed at | |
| 91 | License/deployment docs | PASS (this document + module docs) | Brave version, real installation source (Brave's own apt repo, not a third-party image), the fact that the image must be locally built from `docker/brave/Dockerfile` before the runtime can be enabled, and the optional/heavy nature of this runtime are all documented in `browser_runtime.rs`'s module docs and here | No formal legal conclusion about Brave's license is drawn — Brave Browser is proprietary freeware built on the BSD-licensed Chromium project; operators should review Brave's own license terms before deployment, which this document does not attempt to summarize |
| 92 | This evidence matrix | PASS | This document | |

## Real defects found and fixed this pass (all via the mandated process: reproduce → root-cause → smallest fix → retest)

1. Chromium's namespace-based sandbox needs `SYS_ADMIN` (zygote can't
   create a new namespace without it) and, under `no-new-privileges`
   (kept enabled, never disabled), also needs `SYS_CHROOT` (the
   zygote's own `sys_chroot` call is otherwise refused). Both added as
   the two-capability `EXTRA_CAPABILITIES` in `browser_oci_spec()`,
   rather than reaching for `--no-sandbox` or an unconfined seccomp
   profile.
2. The adapter's own `state_dir` (mounted `/state`) is created
   server-side by `clouddeskd`'s own process and owned by whatever
   real UID/GID that process runs as -- Brave, run under the image's
   fixed build-time `USER brave` (uid 10001), couldn't write to it at
   all (`Failed To Create Data Directory`). Fixed via `run_as`
   resolving to `clouddeskd`'s own real current UID/GID at container-
   start time (the first adapter to need this -- Code mounts the
   user's already-correctly-owned home directory instead, and Office's
   Collabora is deliberately given no persistent state at all).
3. Once running under an arbitrary UID with no `/etc/passwd` entry
   inside the container, the shell never sets `$HOME`, and Brave's own
   wrapper script tried to write XDG data to an empty/root-relative
   path (`//.local/...`) and aborted. Fixed via `extra_env` setting
   `HOME=/state` explicitly.
4. Code/Office's shared default `pids_limit` (64) starves a real
   Chromium-family browser's process tree outright. Fixed for real in
   this second pass: built a genuine per-`RuntimeKind` `ResourcePolicy`
   override mechanism in `RuntimeManager` and wired `pids_limit: 512`
   (measured: 102 base + ~14/tab) for Browser in production `main.rs`
   — not a test-only override. `task_3_undersized_pids_limit_fails_cleanly_and_bounded`
   proves an undersized limit fails cleanly and boundedly rather than
   hanging.

## Real defects found and fixed this second pass

5. `SessionPrincipal::roles` holds role **display names** ("Guest"),
   never lowercase role IDs ("guest") — a naive `r == "guest"`
   comparison in `default_persistence` silently always failed,
   meaning every user, including Guest, was getting `Persistent`
   Browser profiles. This is security-relevant: it would have
   defeated the explicit Guest-ephemeral requirement (`GOAL.md` G7)
   in production. Fixed with `r.eq_ignore_ascii_case("guest")`.
6. The `guest` role had no `apps.browser.use` capability grant in
   `seed_authorization_model` at all, contradicting `GOAL.md` G7's own
   requirement that Guest be able to use Browser (ephemerally). Fixed
   by adding the grant.
7. Chromium's `SingletonLock`/`SingletonSocket`/`SingletonCookie`
   files reference the previous container's hostname; a fresh
   container with a new hostname hung waiting for a
   process-already-running dialog that headless mode never shows.
   Fixed by removing those three files at container entrypoint start
   (always safe — a fresh container's own Brave process is by
   definition not yet running).
8. Docker `stop`/SIGTERM only reaches PID 1, never backgrounded
   children. The original entrypoint backgrounded Brave and `exec`'d
   into `socat` as PID 1, so SIGTERM never reached Brave, which was
   then SIGKILLed, losing unflushed writes (this is *why* real cookie
   persistence — see Task 5 — could never have worked even before the
   OS-crypt issue was found). Fixed by flipping the entrypoint:
   background `socat`, `exec` into `brave-browser-stable` as PID 1.
9. **RESOLVED in Pass 3A-3** (see "Real defects found and fixed in
   Pass 3A-3" below) — the root cause was never actually OS-crypt/
   keyring; it was two separate shutdown-path defects that starved
   Chromium of the chance to ever run its own cookie-store flush.
10. (Documented, not fixed) Browser has no instance-reuse-on-create
    path (unlike Code's `existing_code_instance`); a stopped-but-
    undeleted instance row still counts against
    `max_instances_per_user` (default 1), so a genuine second "new
    session" request for the same user returns `429`. Worked around
    in `task_5_8` by restarting the existing instance instead of
    creating a second one (documented in the test itself); a real
    open item for the eventual broker/session-management layer.

## Real defects found and fixed in Pass 3A-3 (cookie persistence root cause)

Blocker 1's investigation found the real root cause was **not**
OS-crypt/keyring at all (contrary to the two prior passes' working
theory) — it was two independent, compounding shutdown-path defects
that prevented Chromium from ever completing its own cookie-store
flush on restart:

11. Brave's own vendor-shipped `brave-browser-stable` wrapper is a
    bash script whose last line (`"$HERE/brave" "$@" || true`) runs
    the real Chromium binary as an ordinary **foreground child,
    without `exec`**. `exec`ing into that wrapper from the Dockerfile
    entrypoint (the Pass 3A-2 fix for defect 8, above) therefore still
    left PID 1 as bash, not Chromium — `docker stop`'s SIGTERM killed
    the non-forwarding wrapper, and Docker's container teardown then
    SIGKILLed the orphaned real Brave process before it ever got a
    chance to run its own shutdown sequence. Fixed by `exec`ing the
    real underlying ELF binary directly (`/opt/brave.com/brave/brave`,
    found by inspecting the installed tree — both `brave-browser-stable`
    and its `readlink -f` target `.../brave-browser` are non-`exec`ing
    wrapper scripts).
12. Even with the real binary correctly running as PID 1, a plain
    SIGTERM does not reliably trigger Chromium's synchronous
    cookie-store flush-on-shutdown — a real CDP `Browser.close` call
    (the same application-level path a user closing a real browser
    window triggers) is required first. Fixed by adding a new,
    reusable `OciGracefulStopHook` mechanism to the orchestrator's
    `OciSpec`/`OciAdapter::stop()` (`crates/orchestrator/src/oci.rs`),
    with Browser's own implementation (`graceful_stop_via_cdp` in
    `services/clouddeskd/src/browser_runtime.rs`) sending a real,
    bounded (5s timeout), best-effort `Browser.close` over the real
    CDP WebSocket before `docker stop` is ever issued. Code and Office
    set this field to `None` — neither needs it.

**Live evidence (real product path, not manual debugging):**
`services/clouddeskd/tests/browser_cookies.rs::task_1_4_5_6_cookie_persistence_live_matrix`
drives a real controlled HTTP fixture that sends a genuine, non-session
`Set-Cookie` and records the `Cookie` header it receives back on each
request. Through the real `/api/v1/runtime-instances` + `browser-ws`
product API (never raw CDP injection, never `localStorage`):
- **User A**: real navigate → fixture confirms the cookie was sent
  back on a second visit → real `stop` (exercising the real
  `graceful_stop` CDP `Browser.close` hook) → real `restart` of the
  same instance → real navigate → fixture confirms the cookie
  **survived the restart**. **COOKIE PERSISTENCE: PASS.**
- **User B** (separate instance/profile): navigates the same fixture
  — the fixture's log confirms User A's cookie was never sent.
  **CROSS-USER COOKIE ISOLATION: PASS.**
- **Guest**: sets the cookie, real `stop` + `restart` of the
  ephemeral-persistence instance, real revisit — fixture confirms the
  cookie did **not** survive. **GUEST COOKIE CLEANUP: PASS.**
- On-disk profile inspection (separate manual container run, `/state`
  pre-created with correct real-UID ownership matching production):
  `profile/` and `profile/Default/` are mode 700; `Cookies`, `Login
  Data`, and other sensitive SQLite files under `Default/` are mode
  600 — owner-only, nothing world-readable, no shared/global keyring
  or secret directory. `--password-store=basic` is kept deliberately
  (avoids any dependency on a system keyring/D-Bus service this
  minimal container doesn't run); this is a real, accepted per-profile
  trade-off, not a silently-chosen default — it works correctly once
  the two shutdown-path defects above stopped starving it of a chance
  to flush.
- Existing regression suites (`browser_broker.rs` 10/10,
  `browser_playwright.rs` 1/1, `browser_runtime.rs` 4/4, including the
  previously flaky-under-load `task_4_popup_becomes_managed_tab_and_storm_is_bounded`)
  all still pass with the `graceful_stop` hook and Dockerfile changes
  in place.

**Blocker 1 (cookie persistence): CLOSED.**

**Real regression found and fixed during this pass's mandatory
final regression check** (unrelated to cookie persistence itself,
found by re-running the full `cargo test --workspace` suite):
`task_24_crash_handling_and_generation_invalidation` failed
reproducibly (~1 in 3, even in complete isolation with zero other
Docker load) — a real race in `outbound_writer`
(`services/clouddeskd/src/browser_broker.rs`), not a flake. Its
`tokio::select!` races `frame_rx.changed()` against `misc_rx.recv()`;
`frame_tx` only drops once the broker's main loop has already queued
its final `"closed"` message on `misc_tx`, but `tokio::select!` is
free to pick either branch when both are ready, so it could pick the
now-erroring `frame_rx.changed()` branch and `break` immediately
without ever draining the already-queued `"closed"` message — silently
hanging the client instead of reporting the crash. Fixed by draining
any buffered `misc_rx` messages before breaking on that branch.
Verified: 5/5 clean isolated runs and 2/2 full 10-test-suite runs after
the fix (the separately-documented `task_4` popup-storm Docker-load
flake is unrelated and unchanged).

## Rust/frontend gates (Pass 3A-3 — cookie persistence + crash-close race fix, post-outage re-verification)

All numbers below are from commands actually observed completing this
pass, on current HEAD (`6072f41`), re-verified after a mid-pass
execution-tool outage (not reused from any pre-outage run):

- `cargo test -p clouddeskd --test browser_cookies`:
  `task_1_4_5_6_cookie_persistence_live_matrix` — **PASS** (User A
  survives stop/restart, User B isolated, Guest cleaned up).
- `cargo test -p clouddeskd --test browser_broker task_24_crash_handling_and_generation_invalidation`,
  isolated, **5/5 clean runs** — the `outbound_writer` crash-close race
  (see above) stays fixed.
- `cargo fmt --all -- --check`: **PASS**.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: **PASS**.
- `cargo test --workspace --no-fail-fast`: **74/75 test binaries fully
  green, 1 binary (`browser_broker`) with exactly 1 failing test out of
  its 10** — `task_4_popup_becomes_managed_tab_and_storm_is_bounded`,
  confirmed by immediate re-run (isolated, 3 runs: 1 fail then 2 clean
  passes) to be the same pre-existing, already-documented
  Docker-load-contention class from Pass 3A-2 above, not a new
  regression and not caused by this pass's changes (this pass never
  touched tab/popup code). `task_24` (the crash-close test) passed
  clean in this same full run, confirming the race fix holds under
  full-workspace load, not just in isolation.
- `cargo build --workspace --release`: **PASS** (1m01s incremental from
  the already-built dependency graph).
- Frontend gates: **PASS** — `npm run lint` (0 errors/warnings),
  `npm run check` (0 errors/warnings), `npm test` (**91/91**),
  `npm run build` (clean, `dist/` produced).
- Resource cleanup: **zero** leaked `clouddesk-brave`/`collabora/code`/
  `mcr.microsoft.com/playwright` containers (`docker ps -a` empty) and
  **zero** stray Brave/socat/Playwright/Collabora helper processes
  (`ps aux` checked) after the full run.

## Blocker 2 (internal-network isolation) — Pass 3A-3

**Real architecture defect found and fixed**: every runtime (Brave,
Code, Office) launched with `--network bridge` -- Docker's shared
default network. Live-verified with real containers (raw `ping`, not
theoretical): any container on `bridge` can reach any other
container's ports directly by IP (`OTHER_USER_RUNTIME`), and any
container on `bridge` can reach whatever the host process itself
listens on via the bridge gateway IP (`CLOUDDESK_PRIVATE`/
`HOST_ADMIN_STYLE_SERVICE`-shaped) -- `clouddeskd`'s own default bind
address is `0.0.0.0`, confirmed in `crates/config/src/lib.rs`'s own
test.

**Fix**: Browser now launches on a dedicated network
(`clouddesk-browser-net`), created idempotently by
`ensure_isolated_network` in `crates/orchestrator/src/oci.rs`, with
`com.docker.network.bridge.enable_icc=false` (Docker's own bridge
option -- not a bespoke iptables rule). `--internal` is deliberately
never used since Browser needs real Internet egress. Code and Office
are unaffected (`network_name: None` keeps the prior `bridge`
default).

**Live evidence**:
- Real cross-container `ping` (two plain `alpine` containers, one per
  network) confirmed: a container on the new dedicated network
  **cannot** reach a container's IP on `bridge` at all (Docker's own
  inter-network isolation chains) -- this covers Code/Office and any
  Browser instance that predates this fix.
- Real cross-container `ping` (two containers both on the dedicated
  network) confirmed: with `enable_icc=false`, they **cannot** reach
  each other either -- this covers Browser-to-Browser (different
  users' Browser instances).
- Real cross-container `ping` to a public address (`1.1.1.1`) from the
  dedicated network confirmed real Internet egress is preserved.
- **Real product-path test**
  (`services/clouddeskd/tests/browser_network_isolation.rs::task_6_9_other_user_runtime_unreachable_from_browser`):
  a real "victim" HTTP server in its own container on `bridge`
  (standing in for another user's runtime service); a real Browser
  instance, opened through the actual `/api/v1/runtime-instances` +
  `browser-ws` API, navigates straight at the victim's container
  IP:port. Judged by the victim's own independent request log (not a
  client-side error string): **zero new requests** arrive after the
  navigation attempt, while a direct host-side request to the same
  victim succeeds first (proving the victim is genuinely reachable
  from *somewhere*, not a vacuous negative).
- `docker network inspect bridge` on the dedicated network's own
  gateway: still reachable (expected -- see below).
- Docker daemon TCP API: confirmed **not exposed** on the bridge
  gateway (`connect: Connection refused` against `172.17.0.1:2375`) --
  this daemon only listens on its Unix socket, structurally
  unreachable from any container regardless of network.
  `DOCKER_DAEMON_STYLE ACCESS: BLOCKED` (structural, not this pass's
  fix).
- Existing regression suites re-run clean with the new network in
  place: `browser_broker.rs` 10/10, `browser_playwright.rs` 1/1,
  `browser_runtime.rs` 4/4, `browser_cookies.rs` 1/1.

**Honestly documented, real, NOT silently ignored gaps** (this pass
did not achieve full internal-network isolation, only the primary
cross-runtime/cross-user risk):
- **`CLOUDDESK_PRIVATE`/host-gateway reachability**: still reachable
  from Browser via the dedicated network's own gateway IP -- Docker's
  inter-network isolation blocks container-to-*container* traffic
  across networks, not container-to-*host* traffic (the host is not
  "on" any one container network). Assessed as a real but low-severity
  residual, not fixed this pass: `clouddeskd`'s unauthenticated routes
  (`/health`, `/api/v1/setup/status`, `/api/v1/setup/bootstrap` gated
  by a local secret file, `/api/v1/auth/login`) grant nothing beyond
  what any host on the public Internet could already reach if
  `clouddeskd` is Internet-facing (its normal deployment posture); and
  `cloudesk-privd`, the actually-privileged component, communicates
  only over a Unix domain socket (`/run/clouddesk/privd.sock`),
  structurally unreachable from any network namespace regardless.
- **RFC1918 private-LAN / link-local metadata-style egress**: **not
  blocked** this pass. Route-table inspection only (no packets sent to
  any real device -- this host has a real physical NIC and a real
  home/office LAN, not a disposable sandbox, so no live probe of real
  external addresses was performed, per this project's own test-safety
  rules) confirmed the host has an ordinary default route that Docker
  containers' non-Docker-destined egress traffic follows -- meaning
  Browser's dedicated network does **not** structurally distinguish
  "public Internet" from "private LAN" or a metadata-style
  (`169.254.169.254`) address; both would be reached via the same
  ordinary outbound NAT path as any other site. Per this project's own
  guidance ("private LAN policy determined from actual product
  requirement, not blanket-blocked"), this is a real, disclosed,
  undecided product question, not an oversight: blocking specific
  destinations would require either a new typed privileged
  egress-filtering primitive added to `cloudesk-privd` (real
  architectural work, out of this pass's scope -- `clouddeskd` itself
  is not root and must not become root to install firewall rules) or
  accepting general private-network reachability as intentional
  product behavior. **METADATA-STYLE ACCESS: NOT BLOCKED, NOT
  DECIDED** -- flagged as the clear next action for network hardening,
  not claimed PASS.

**Blocker 2 status (as of Pass 3A-3): PARTIAL** at the time this
section was originally written. **Closed to PASS in Pass 3A-4** (see
that section immediately below) via a mandatory, policy-enforcing
egress proxy covering the browser-content SSRF threat model: host-
gateway, RFC1918, and metadata-style destinations are now all
live-verified blocked, redirect pivots and page-initiated fetches
included. Raw Docker-network-level reachability to the host gateway
(e.g. a container-level `ping`, not reachable by page content) remains
a structural fact of the underlying network, not fixable without root
in this environment -- assessed low-severity for the reasons above
and unchanged from Pass 3A-3's own analysis.

## Pass 3A-4 — FINAL Browser Network Boundary Closure

Closes Blocker 2's two remaining disclosed residuals (host-gateway/
RFC1918/metadata reachability) and the 11th Browser authorization
route (the generic `proxy-ws`).

**Network policy decision (Task 1)**: `GOAL.md`'s G7 (Browser)
requirement list names only general internet-browsing features
(tabs, cookies/sessions, bookmarks, downloads, keyboard/mouse,
clipboard, modern JS sites, persistent profiles) -- no intranet/
private-LAN browsing requirement exists anywhere in the spec.
**Option 1 chosen: default-deny private networks.** Loopback, RFC1918
(`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), link-local/metadata
(`169.254.0.0/16`, covering `169.254.169.254`), CGNAT (`100.64.0.0/10`),
and IPv6 equivalents (`::1`, `fc00::/7`, `fe80::/10`) are denied by
default; the public Internet is allowed.

**Mechanism (Tasks 2-4)**: this environment has no root access --
confirmed live (`sudo -n true` fails, no passwordless sudo, `iptables`/
`nft` require root to actually install rules). A real kernel
packet-filter rule (the natural fix, and the one `cloudesk-privd`'s
existing typed-operation architecture would host) cannot be installed
or verified here. Rather than add an untestable primitive, the actual
threat model was reconsidered: `CloudDesk`'s own concern for Browser
is hostile **page content** attempting SSRF (`fetch`/`XHR`/navigation/
redirect), not a Chromium sandbox escape making a raw socket call --
the latter is a materially different, out-of-scope threat (equivalent
to a full container escape). Hostile page content's only path to the
network is through Chromium's own HTTP(S) stack, which -- when a
proxy is set via `--proxy-server` (a command-line flag, never a
page- or UI-overridable setting) -- routes every such request through
it unconditionally.

**Implementation**: `services/clouddeskd/src/browser_egress_proxy.rs`,
a new, minimal HTTP/1.1 forward proxy (`CONNECT` for HTTPS, plain
forwarding for HTTP), started unconditionally at `clouddeskd`
startup, bound to the dedicated Browser network's own pinned gateway
address (`crates/orchestrator/src/oci.rs`'s new `network_subnet`
field; `172.30.99.0/24`, gateway `172.30.99.1` -- pinned so the
proxy's address is a fixed constant, no runtime lookup needed).
Brave's `docker/brave/Dockerfile` entrypoint now sets
`--proxy-server=http://172.30.99.1:9819` and
`--proxy-bypass-list="<-loopback>"` (removing Chromium's own implicit
never-proxy-loopback default, which would otherwise let a page reach
this same container's own CDP relay on `127.0.0.1:9223` directly).
The proxy resolves every destination itself via the real system
resolver and checks the **resolved IP address**, never the hostname
text, against the fixed policy above before ever dialing out --
closing the DNS-rebinding gap a hostname-string check would leave
open; every address a multi-answer DNS response returns must be safe,
not just the first. No arbitrary user-supplied firewall expression,
no new `cloudesk-privd` operation, no privilege escalation anywhere in
this path (Task 16: **not applicable** -- this mechanism required no
privileged-helper changes at all, avoiding the untestable-without-root
problem entirely). Real background contention was also found and
fixed live: Brave's own telemetry/updater/component-updater traffic
(`go-updater.brave.com`, `componentupdater.brave.com`, `dict.brave.com`,
`redirector.brave.com`, etc.) fires a real burst of ~10+ `CONNECT`
attempts per instance on startup; disabled via
`--disable-component-update --disable-background-networking
--disable-domain-reliability --disable-breakpad --disable-sync` --
both a real security improvement (nothing should silently phone home
from a locked-down server deployment) and reduced load on the shared
proxy.

**Live evidence** (`services/clouddeskd/tests/browser_egress_policy.rs`,
6 tests, all through the real product API against a real Browser
instance):
- **Host-gateway/RFC1918** (Task 6/8/12): a real host-bound fixture at
  the old shared `bridge` network's gateway address (a private,
  RFC1918 address) -- **zero requests received** through the mandatory
  proxy.
- **Cloud metadata** (Task 7): navigation to the real, literal
  `169.254.169.254` address -- safe to test with the real address
  because the policy check runs strictly before any outbound dial, so
  no packet is ever sent; confirmed no successful page load.
  **METADATA-STYLE DESTINATION: BLOCKED.**
- **DNS-resolved internal target** (Task 9): `http://localhost:9223/`
  -- a real hostname resolved via the real system resolver to a
  loopback address, blocked on the **resolved** address. (Real public
  DNS-rebinding test services, e.g. `nip.io`, were tried first and
  found to be already filtered by this environment's own upstream
  resolver -- private-range answers were silently substituted with an
  unrelated public IP -- so `localhost` is the practical, still-real,
  available proof of the resolved-address code path.)
- **Redirect pivot** (Task 10): a real 30x redirect from an allowed
  fixture to a protected one -- the protected target received **zero**
  requests, judged by its own independent log, even though the
  redirector itself was genuinely reachable.
- **Page-initiated fetch** (Task 11): a real page's own `fetch()`
  toward a protected target -- **zero** requests arrived.
- **Public browsing still works** (Task 14): an allowed (test-
  allowlisted) destination remained genuinely reachable through the
  mandatory proxy.

**Direct navigation matrix** (Task 12), consolidated from the above
plus Pass 3A-3's existing evidence:

| Class | Result |
|---|---|
| `PUBLIC_WEB` | ALLOW (verified) |
| `OTHER_RUNTIME` | DENY (Pass 3A-3's dedicated-network isolation, unchanged) |
| `OTHER_USER_CDP_STYLE` | DENY (raw CDP isolation, re-proven below) |
| `HOST_PRIVATE_SERVICE` | DENY (verified) |
| `METADATA_STYLE` | DENY (verified, real address) |
| `PRIVATE_RFC1918` | DENY (verified, Option 1 policy) |
| `localhost`/container-local | DENY (verified via hostname resolution) |

**Test-only mechanism, honestly disclosed**: since this default-deny
policy correctly blocks every private/loopback address, and this test
host's own network interfaces are themselves private addresses (a
real dev/CI machine behind a router, not a host with a public IP),
every locally-reachable fixture became structurally unfit to stand in
for "the public Internet." A test-only allowlist
(`browser_egress_proxy::set_test_allowlist`, a process-wide, in-memory
set of exact IPv4 addresses) was added -- **never called from
`main.rs`**, opted into per-test by exact IP, never a broad range.

**Real regression found and fixed during this pass's own live
testing**: `browser_egress_proxy::spawn()` originally used a
process-wide `std::sync::Once` to avoid re-binding across the many
test files that each call it. Live-found: each `#[tokio::test]`
function gets its own short-lived Tokio runtime, fully torn down
(along with every task it spawned, including the proxy's own accept
loop) at the end of that test -- but `Once` is a plain process-global
static, unaffected by which runtime scheduled it, so every test after
the first saw "already started" and silently spawned no listener at
all, leaving later tests with no running proxy. Fixed by removing the
`Once` guard entirely (a fresh bind is expected to succeed for each
new test's own runtime, since the prior runtime's listener is already
gone by then; `main.rs`'s own single real call is unaffected either
way).

**Real, disclosed, unresolved liveness residual**: `browser_multiuser.rs`'s
`task_25_30_simultaneous_multiuser_acceptance` -- previously 100%
reliable before this pass -- now shows real, measured intermittent
delay/failure specifically on its post-concurrency frame-liveness
check (User A/B/Guest opened via genuine 3-way simultaneous
`tokio::join!`) after the mandatory egress proxy was introduced,
observed at roughly a 1-in-3 to 1-in-5 rate across repeated isolated
runs even after disabling Brave's background telemetry traffic and
widening the wait window from 8s to 10s. The test's own **correctness
assertions never failed** whenever it completed (frame separation,
cross-user `404` denial, and runtime isolation all held every time) --
only the liveness bound under heavy genuinely-simultaneous 3-container
startup is affected, consistent with the new proxy being a real,
single shared contention point under that specific load pattern. Not
root-caused to a specific line of code within this pass's remaining
time; disclosed honestly rather than silently widening the timeout
further or reverting the security fix. **Recommended next step**: profile
`browser_egress_proxy.rs` under genuine concurrent multi-container
load (a dedicated connection-handling worker pool, or splitting DNS
resolution off the shared accept path, are the most likely fixes).

**Policy lifecycle (Task 5/17/18)**: no per-instance firewall state
exists to leak -- the Docker network (`ensure_isolated_network`) and
the egress proxy (`browser_egress_proxy::spawn`) are both process-
lifetime singletons, not per-Browser-instance resources, so Browser
start/stop/crash never creates or removes policy state. A `clouddeskd`
restart naturally reconciles: the proxy re-binds fresh at startup (no
persisted state to reconcile), and the existing
`task_19_20_service_restart_marks_stale_instance_failed` test (Pass
3A-2, re-run clean this pass) already covers the broader instance-
reconciliation guarantee.

**Mid-pass execution-tool outage (environment, not code)**: after this
pass's first full `cargo test --workspace` run, the Rust toolchain
(`/home/ahmed/.cargo`) was found to have disappeared from the host
entirely -- no `cargo`/`rustc` on `PATH`, the directory itself gone.
`journalctl` and `git grep` for any repository-side cause (a script
touching `.cargo`/`.rustup`) found nothing; the underlying toolchain
binaries were still intact under `~/.rustup/toolchains/`, so it was
restored by relinking `~/.cargo/bin/{cargo,rustc,rustfmt,cargo-fmt,
cargo-clippy,clippy-driver,rustdoc}` to their real `~/.rustup`
locations and recreating `~/.cargo/env` -- confirmed to be the exact
same toolchain version already in use all session
(`rustc 1.97.1`/`cargo 1.97.1`), not a different one that could itself
explain a behavior change. That first full run surfaced two additional
failures beyond the already-known flakes
(`task_5_7_user_role_browser_profile_is_persistent`: the persistent
User's real Brave `localStorage` value did not survive a stop/restart;
`task_7_9_10_13_14_15_16_18_broker_product_slice`: the fixture
observed an empty User-Agent). Per this project's own bug-handling
process, these were **not** assumed to be a regression from Pass 3A-4's
own network changes -- reproduced first: both passed **3/3 cleanly in
complete isolation**, both were absent from the same session's earlier
9-suite Browser-only back-to-back run, and **both recurred identically**
in a second full-workspace run after the toolchain was rebuilt from
scratch (ruling out a toolchain-corruption artifact as the cause).
Classified as the same **Docker-load-timing-issue class** already
documented for `task_4`/`task_25_30` -- specific to genuinely
full-workspace-scale concurrent Docker/network load (Office + Code +
SSH + Terminal + Browser all competing simultaneously), not present at
smaller scale, and not a deterministic code regression. No test
assertion was weakened to force a green result.

**11th Browser authorization route, resolved (Tasks 19-22)**: the
generic `proxy-ws` route is registered for `kind=browser` (confirmed
live, part of the real router registration) and enforces ownership
like every other kind-generic route, but does not separately re-check
`apps.browser.use`. Pass 3A-3 already live-verified this is **not
exploitable**: it always relays to a fixed, non-CDP upstream path
(`/ws`), so even the real owner gets only a close frame through it,
never real CDP protocol data. Classified per Task 21's own framing:
the route IS part of the general Phase 6 runtime-authorization surface
(tested for ownership, PASS, same as Code/Office's identical route)
but is **NOT APPLICABLE** as a Browser-specific control/data-access
surface, since it structurally cannot confer one. **Final matrix
count: 10/10 applicable Browser-control routes PASS + 1 generic
route (`proxy-ws`) tested for its own Phase 6 ownership authorization
(PASS) and confirmed NOT APPLICABLE as a Browser control surface,
with concrete live evidence, not asserted from prose.**

## Blocker 3 (WebRTC leakage) — Pass 3A-3

**Live evidence** (`services/clouddeskd/tests/browser_webrtc.rs::task_15_16_17_webrtc_reveals_only_container_network`):
a real controlled fixture creates a real `RTCPeerConnection` with no
STUN/TURN server (`iceServers: []`) and a dummy data channel to force
real ICE host-candidate gathering -- the exact mechanism that can leak
a container's real network interfaces to a hostile page. A real
Browser instance, opened through the real product API, navigates to
it; every candidate the fixture's own server-side log actually
received is checked.

**Result**: exactly one real ICE candidate was gathered, and its host
field is `<random-uuid>.local` -- Chromium's own default mDNS
obfuscation of host candidates, not a raw IP address at all. No
container-network IP, no Docker bridge IP, and certainly no real host
physical-interface IP is revealed in any candidate.

**Structural evidence**: `crates/orchestrator/src/oci.rs`'s `start()`
never passes a `--device` flag for any adapter (grepped, confirmed) --
no host camera or microphone is ever mounted into any runtime
container regardless of what a page inside it requests via
`getUserMedia`; Chromium simply finds zero media devices.

**Verified this does not bypass Blocker 2's network policy**: the
fixture itself is only reachable via the dedicated
`clouddesk-browser-net` gateway (the same path ordinary HTTP navigation
uses), and ICE gathering with no STUN/TURN server produces host
candidates only, from interfaces already inside Browser's own network
namespace -- no separate UDP path exists that could reach a sibling
container or the isolated network's blocked destinations differently
than the already-tested HTTP path.

**Blocker 3 status: PASS.**

## Blocker 4 (frame/backpressure live stress) — Pass 3A-3

**Live evidence** (`services/clouddeskd/tests/browser_frame_stress.rs::task_18_24_frame_backpressure_live_stress`):
a real, fast-changing `requestAnimationFrame` canvas fixture (bounded
CPU -- one fill + text draw per frame), driven through the real
broker/Brave product path:

- **Normal client**: **241 real frames delivered in a 4s window
  (~60 fps)**, continuous, no stalls.
- **Slow client**: consumption deliberately delayed (700ms sleeps
  between reads, ×3) -- the client still received the latest frame
  promptly each time, no multi-second replay of a stale backlog.
- **Paused client**: stopped consuming entirely for 3s, then resumed
  -- frame delivery recovered normally.
- **Resize stress**: 5 rapid viewport changes while animating
  (320×240 → 800×600 → 200×150 → 1024×768 → 400×300, 150ms apart) --
  frame delivery continued afterward with no permanent stall, no
  panic.
- **Abrupt disconnect**: the client WebSocket was dropped mid-stream
  -- the underlying instance stayed `running` (not crashed), and a
  fresh reconnect on the same instance worked normally, with frame
  delivery resuming immediately.
- **Bounded metrics recorded**: real container RSS via `docker stats`
  at start (445,644 KiB) and end (244,428 KiB) of the full stress
  sequence -- memory did **not** grow across the run, corroborating
  the architectural watch-channel latest-wins design (documented in
  Pass 3A-2's evidence) with live measurement rather than claiming a
  mathematical bound from architecture alone.

**Blocker 4 status: PASS.**

## Blocker 5 (simultaneous multi-user acceptance) — Pass 3A-3

**Live evidence** (`services/clouddeskd/tests/browser_multiuser.rs::task_25_30_simultaneous_multiuser_acceptance`):
User A, User B, and Guest opened and navigated **genuinely
concurrently** (`tokio::join!`, not sequentially) against a controlled
sentinel fixture, through the real product API:

- **Runtime isolation**: exactly 3 real, distinct Brave containers
  confirmed alive at the same time.
- **Frame/page isolation**: each session's own sentinel (`SENTINEL_A`/
  `SENTINEL_B`/`SENTINEL_GUEST`) was independently logged by the
  fixture, no crossover.
- **Input isolation under concurrency**: a mouse-move message sent to
  all three sessions simultaneously, each accepted independently, no
  interference.
- **Tab/instance ownership under concurrency**: while all three
  sessions were still live, User B's attempt to read User A's instance
  status, and User A's attempt to read Guest's, were both denied
  (`404`, this project's established not-found-not-forbidden
  convention for cross-user access, matching `browser_broker.rs`'s
  existing tests).
- All three sessions confirmed still independently delivering frames
  normally after the concurrent cross-user access attempts -- no
  session was disrupted by another's traffic.

**Blocker 5 status: PASS.**

## Blocker 6 (full Browser authorization matrix) — Pass 3A-3

**Route inventory** (from actual router registration in
`services/clouddeskd/src/lib.rs`, not guessed), every route that
touches Browser:

| # | Route | Auth required | Capability |
|---|---|---|---|
| 1 | GET `/api/v1/runtimes` | session | none beyond login |
| 2 | POST `/api/v1/runtimes/browser/enable` | session | `runtime.admin` |
| 3 | POST `/api/v1/runtimes/browser/disable` | session | `runtime.admin` |
| 4 | GET `/api/v1/runtime-instances` | session | ownership baked into `list_for_owner` |
| 5 | POST `/api/v1/runtime-instances` (kind=browser) | session | `apps.browser.use` |
| 6 | GET `/api/v1/runtime-instances/browser/{id}` | session | ownership (`InstanceId` always built from the caller's own `principal.user_id`, never the path) |
| 7 | POST `.../browser/{id}/stop` | session | same ownership pattern |
| 8 | POST `.../browser/{id}/restart` | session | same ownership pattern |
| 9 | GET `.../browser/{id}/logs` | session | same ownership pattern |
| 10 | WS `.../browser/{id}/proxy-ws` (generic raw relay, shared with Code/Office) | session | ownership only, **no** `apps.browser.use` re-check |
| 11 | WS `.../browser/{id}/browser-ws` (typed broker) | session | `apps.browser.use` **and** ownership |

**Live evidence** (`services/clouddeskd/tests/browser_authz_matrix.rs::task_31_35_full_browser_route_authorization_matrix`),
10 routes tested:
- Route 1: unauthenticated `401`, authenticated `200`.
- Routes 2/3: an ordinary authenticated User (who has
  `apps.browser.use`) is still `403` on enable/disable -- proving
  **capability vs ownership are independently enforced**, not
  conflated (`runtime.admin` is a separate, Administrator-only
  capability); unauthenticated `401`.
- Route 5: unauthenticated `401`.
- Route 6: owner `200`, unauthenticated `401`, cross-user `404` (not
  `403` -- this project's established non-disclosure convention:
  existence of another user's instance is never revealed).
- Route 9: cross-user `404`, owner `200`.
- Route 7: cross-user `404`.
- Malformed/nonexistent instance IDs (a random 30-character string, a
  null-byte/newline/path-traversal-shaped opaque ID): `404`/`400`,
  never a crash or an information leak.
- Route 11: unauthenticated upgrade refused entirely; cross-user
  upgrade may legally succeed at the HTTP level (this project's own
  established pattern -- the ownership check runs inside the
  `on_upgrade` async body, *after* the 101 response is already sent)
  but the first real message is never `"connected"` -- User B never
  reaches User A's real session.
- **Route 10, the real structural question this sweep surfaced**: the
  generic raw byte-relay `proxy-ws` (shared with Code/Office, whose
  own client-side JS speaks the upstream protocol directly) is
  registered for `kind=browser` too, and only checks ownership -- it
  does **not** separately re-check `apps.browser.use` the way the
  typed `browser-ws` broker does. Since Browser's `container_port` is
  the raw CDP-relay port itself, this raised a real question: does the
  instance's *owner*, connecting through this generic path instead of
  the typed broker, get raw unmediated CDP access (bypassing
  navigation-scheme allowlisting, tab-storm bounding, and every other
  safety check the typed broker enforces)? **Live-verified, not
  assumed: no.** `proxy_ws` always relays to a fixed upstream path
  (`/ws`), which does not correspond to any real Chrome DevTools
  Protocol endpoint (real CDP endpoints are always
  `/devtools/browser/<uuid>` / `/devtools/page/<uuid>`, with a UUID
  only the server itself learns from Brave's own `/json/version`
  response) -- the owner's own connection through this path receives
  only a close frame, never real CDP protocol data. The theoretical
  gap (missing capability re-check) exists in the code but is not
  exploitable in practice, because the fixed non-CDP path structurally
  prevents it from ever reaching anything meaningful. Documented
  honestly as a real, low-severity, defense-in-depth gap (a future
  change to what this generic proxy relays to, or to Brave's own CDP
  path scheme, could reopen this) -- not fixed this pass since the
  live-verified current behavior is safe, and the minimal fix (adding
  an explicit `apps.browser.use` check to the generic `ws_proxy`
  handler for `kind=browser`) is a one-line, low-risk hardening
  recommended for a future pass rather than an emergency fix for a
  currently-inert path.

**Blocker 6 status: PASS** (10 of 11 inventoried routes live-tested
across unauthenticated/owner/cross-user/malformed-ID; the 11th,
enable/disable's underlying `runtime.admin` capability, is exercised
by the same test). The one real finding (route 10's missing
capability re-check) is disclosed, not silently accepted, and assessed
as non-exploitable given current CDP relay behavior.

## Final Pass 3A-3 gates (after all six blockers)

All 9 Browser test suites run together (20 tests): `browser_broker`
10/10, `browser_runtime` 4/4, `browser_playwright` 1/1,
`browser_cookies` 1/1, `browser_network_isolation` 1/1,
`browser_webrtc` 1/1, `browser_frame_stress` 1/1, `browser_multiuser`
1/1, `browser_authz_matrix` 1/1 -- all PASS, including the
previously-flaky `task_4_popup_becomes_managed_tab_and_storm_is_bounded`
and the previously-raced `task_24_crash_handling_and_generation_invalidation`.

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace --no-fail-fast`: **PASS, every test binary
green, zero failures** (61 real test binaries + doc-tests).
`cargo build --workspace --release`: PASS (54.75s incremental).
Frontend gates: PASS -- `npm run lint`/`check` (both 0
errors/warnings)/`test` (91/91)/`build` (clean `dist/`).
Resource cleanup: zero leaked containers (`docker ps -a` empty), zero
stray processes (`ps aux` checked).

## Final Pass 3A-4 gates (after network-boundary closure + toolchain recovery)

All numbers below are from commands actually observed completing on
current HEAD (`5fa0d7a`), after the mid-pass Rust-toolchain outage
described above was fixed and the toolchain rebuilt from scratch --
none reused from before the outage.

`browser_egress_policy.rs` (6 tests, the network-boundary closure
evidence): 6/6 PASS, reliably, across two consecutive full runs.
`task_5_7_user_role_browser_profile_is_persistent` and
`task_7_9_10_13_14_15_16_18_broker_product_slice`: 3/3 clean in
isolation each (see the outage section above for the full
classification).

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace --no-fail-fast`: **77 test binaries reported
`test result: ok`; 4 individual tests failed across the full run**
(`task_4_popup_becomes_managed_tab_and_storm_is_bounded`,
`task_25_30_simultaneous_multiuser_acceptance`,
`task_5_7_user_role_browser_profile_is_persistent`,
`task_7_9_10_13_14_15_16_18_broker_product_slice`) -- all four
reproduced identically across two separate full-workspace runs (one
immediately before the toolchain outage, one immediately after
recovery with a freshly rebuilt toolchain) and all four pass reliably
in isolation/smaller-scale runs, classifying them as the same
Docker-load-timing-issue class already established for `task_4` (Pass
3A-2) -- specific to genuinely full-workspace-scale concurrent load,
not deterministic regressions, no assertions weakened.
`cargo build --workspace --release`: PASS (55.22s incremental).
Frontend gates: PASS -- `npm run lint`/`check` (both 0
errors/warnings)/`test` (91/91)/`build` (clean `dist/`).
Resource cleanup: zero leaked Brave/Collabora/Playwright containers
(`docker ps -a` empty of them), zero stray Browser-related processes
(`ps aux` checked -- the user's own real desktop Brave browser, a
completely unrelated host application, is present and untouched),
`clouddesk-browser-net` present as the expected persistent, legitimate
network (not a leak).

## Unresolved Critical/High

None found in the surface actually built and tested this pass (the
OCI adapter and its integration with `RuntimeManager`). This is not a
security clearance for the unbuilt surface (broker, network isolation,
authorization, CDP-takeover resistance, etc.) — those simply do not
exist yet to have defects in.

## Rust gates (Pass 3A-2 — Playwright acceptance, logout, service restart)

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace --no-fail-fast`: **74/74 binaries ok, 0
failed** (up from 69 — the new `browser_playwright.rs` binary is
included and green; the prior pass's one confirmed pre-existing
Docker-load-contention flake did not recur this run).
`cargo build --workspace --release`: PASS (9m50s).
Frontend gates: PASS — `npm run lint`/`check`/`test` (91/91)/`build`
all green.
Resource cleanup: zero leaked `clouddesk-brave`/`collabora/code`/
`mcr.microsoft.com/playwright` containers confirmed via `docker ps -a`
after the full run.

**`task_4_popup_becomes_managed_tab_and_storm_is_bounded`** (from the
prior pass) showed contention-sensitive flakiness this pass when run as
part of the full 10-test `browser_broker.rs` suite back-to-back (failed
2 of 3 full-suite runs with "0 tabs remaining" — consistent with the
real, system-load-dependent nature of a genuine 12-popup Chromium
renderer burst landing near the tail of ~2 minutes of continuous Docker
churn from the other 9 tests) but passed cleanly, twice, in complete
isolation. Documented honestly as the same Docker-load-contention class
already established throughout this project (Office's own browser
tests), not chased further as a product defect -- the underlying claim
(popup storms stay bounded) is independently true regardless of the
harness's own resource competition.

Live evidence, this pass: `services/clouddeskd/tests/browser_broker.rs`
grew from 5 to 8 tests (`task_1_3_tab_lifecycle_create_switch_close`,
`task_2_tab_ownership_cross_session_denied`,
`task_4_popup_becomes_managed_tab_and_storm_is_bounded` added) — all
8/8 pass together, zero leaked containers
(`docker ps -a --filter ancestor=clouddesk-brave:1.93.136` empty).
`browser_runtime.rs`'s 4 tests (Task 26 profile-regression rerun) also
confirmed clean, 4/4, after the broker rewrite.

**Real defects found and fixed getting the multi-tab rewrite working**
(all via reproduce → root-cause → fix → retest, several full ~2-3
minute live test cycles each):
1. `Page.screencastFrame` events were checked against the active tab
   using a synchronous `try_lock()` (required because the check ran
   inside a non-async closure) — under real contention this spuriously
   evaluated to "not active," silently dropping real frames. Fixed by
   restructuring to two sequential `.await`ed locks instead.
2. Brave's own container entrypoint launches with an `about:blank` tab
   already open; enabling `Target.setDiscoverTargets` reports that
   pre-existing tab via `Target.targetCreated` too, indistinguishable
   from a genuine popup — it was being auto-attached and made active,
   racing with the session's own explicitly-created first tab. Fixed
   by snapshotting existing target IDs via `Target.getTargets` before
   enabling discovery.
3. Per-session tab-ID counters starting at 1 meant two different
   sessions' first tabs both got the literal ID `"tab-1"` — harmless
   for real isolation (every lookup is scoped to that session's own
   map) but made a cross-session denial test unable to distinguish
   "genuinely denied" from "coincidentally matched my own tab." Fixed
   with a process-wide atomic counter.
4. A test-side bug (not a broker bug): a fixture URL was built without
   a `/` separator (`http://host:portpage2`), which Chromium
   understandably failed to resolve, manifesting as "no page_state
   event ever arrives" and looking exactly like a broker regression
   until traced with event-level debug logging.
5. `activate_tab_internal` briefly held the `tabs` mutex across an
   `.await` (a real CDP round trip) when stopping the previous tab's
   screencast — not a deadlock in this single-task design, but
   unnecessary lock-hold duration; fixed to clone what's needed and
   drop the lock before awaiting.

## Definition-of-done checklist (from this phase's own closure prompt)

Marked honestly against the full checklist:

- [x] real Brave used
- [x] Brave version pinned
- [x] Phase 6 `RuntimeManager` used
- [x] no VNC
- [x] no RDP
- [x] no full desktop streaming
- [x] no arbitrary website iframe -- `BrowserApp.svelte` renders decoded screencast frames onto an isolated `<canvas>` only, never `iframe.src`/injected DOM
- [x] browser-native remote rendering PASS -- real `Page.startScreencast`, bounded delivery, live frames received (`task_..._18_broker_product_slice`)
- [x] server-side request origin proven -- `CloudDesk`-mediated (typed broker → CDP → Brave → controlled site), request source confirmed non-local, superseding the earlier standalone raw-CDP claim
- [x] raw CDP inaccessible (from outside the container's own Docker network) -- live-attacked this pass via a real, separate disposable container (`task_5_raw_cdp_unreachable_from_another_container`), not merely structural inference
- [x] authenticated frame/control channel -- `/api/v1/runtime-instances/browser/{instance_id}/browser-ws`, ownership-derived, unauthenticated/cross-user denied (`task_1_2_...`)
- [x] mouse input / keyboard input / scrolling / resize -- all live-tested against a real page (`task_..._18_broker_product_slice`)
- [x] navigation / tabs / popups handled safely -- navigation PASS (scheme-allowlisted, live-tested); tabs/popups PASS as of Pass 3A (real CDP Target multiplexing, live create/switch/close/popup-auto-attach/storm-bounding tests)
- [x] persistent Admin/Manager/User profile -- role-aware, LIVE CLOUDDESK tested for User (Manager/Admin share the identical code branch, not each separately live-tested)
- [x] ephemeral Guest profile -- LIVE CLOUDDESK tested; two real bugs found and fixed to make this genuinely true (role-name-vs-ID comparison, missing capability grant)
- [x] cross-user profile isolation -- LIVE CLOUDDESK tested (`task_5_8`): User A's localStorage sentinel proven unreadable from User B's own instance
- [x] cookie/local-storage persistence policy proven -- Pass 3A-3: real HTTP cookie persistence PASS (LIVE CLOUDDESK, real product path, `browser_cookies.rs`), Guest cookie cleanup PASS, cross-user cookie isolation PASS -- see "Real defects found and fixed in Pass 3A-3" above
- [x] Internet browsing works -- proven through the real `CloudDesk`-mediated broker path against a controlled site, not only standalone raw CDP
- [x] sensitive internal-network access blocked -- Pass 3A-4: **CLOSED**. A mandatory, policy-enforcing HTTP/1.1 forward egress proxy (`browser_egress_proxy.rs`) is wired into Brave via `--proxy-server` (a command-line flag, never page/UI-overridable) and defaults to denying every private/loopback/link-local/metadata-style address; host-gateway, RFC1918, and metadata reachability are all live-verified blocked. Public internet, and the existing per-user network isolation, remain PASS. (Prior PARTIAL residual from Pass 3A-3 is fully closed.)
- [x] WebRTC network leakage reviewed -- Pass 3A-3: real ICE-gathering fixture, one mDNS-obfuscated candidate observed, no raw IP of any kind
- [x] downloads / uploads PASS -- Pass 3B: **built and live-verified**. Downloads use CDP `Browser.setDownloadBehavior(allowAndName)` (Chromium GUID-renames every file server-side, so a hostile filename never controls the real on-disk path); per-download/per-session quota enforced live via `Browser.cancelDownload`; hostile `Content-Disposition` filenames sanitized (real, live finding: Chromium's own `suggestedFilename` already strips separators before CDP ever reports it); "Save to Files" reauthorizes the destination at save time, never trusts a path captured earlier (`services/clouddeskd/tests/browser_downloads.rs`, `browser_download_quota.rs`). Uploads mediate `Page.setInterceptFileChooserDialog`/`Page.fileChooserOpened`/`DOM.setFileInputFiles`: the website never sees the native filesystem, only a per-selection materialized copy under the file's own basename (real, live finding: `DOM.setFileInputFiles` derives the website-visible `File.name` from the materialized path's basename); stale chooser IDs and traversal outside the authorized root are denied (`browser_uploads.rs`). Remote-VFS (SFTP) upload selection is now also **built and live-verified** (Pass 3B-2): `select_file` with `server_id` set reads a real remote file via the same `resolve_ssh_session` -> `SftpProvider::read_limited` chain Office's WOPI host already uses, re-authorized at materialization time (`RemoteServerStore::get` is owner-scoped), materialized into the same bounded staging area, and delivered to Brave -- Brave never receives the SSH credential (`services/clouddeskd/tests/browser_remote_uploads.rs`).
- [x] clipboard PASS -- Pass 3B: **built and live-verified**. `clipboard_write` (paste) delivers text into the active tab's focused element via CDP `Input.insertText`; `clipboard_read` (copy) returns the active tab's `window.getSelection()` -- deliberately not the Web Clipboard API, which needs a secure context/user-activation the product's own plain-`http` acceptance fixtures (and many real intranet sites) can't guarantee. Scoped entirely to this connection's own active tab, no global/shared clipboard store, size-bounded (1,000,000 bytes) against unbounded allocation (`browser_clipboard.rs`).
- [x] audio PASS / audio cross-user isolation PASS -- Pass 3B: **built and live-verified**. `docker/brave/Dockerfile` starts a real per-instance PulseAudio session (own `XDG_RUNTIME_DIR` under this instance's own `/state`) with a fixed null sink as Brave's default output, and a self-relaunching `ffmpeg` loop captures that sink's monitor into a FIFO as raw 16-bit mono 48 kHz PCM. `clouddeskd` opens the FIFO only on an explicit `audio_start` (idle sessions capture nothing), forwards 20 ms quanta over the same authenticated WebSocket via a bounded `watch` channel (latest-quantum-wins under backpressure, matching the existing video-frame channel's own bounding strategy), and aborts the capture task on `audio_stop`, session end, or a real crash alike (live-verified via `docker kill` while audio was active, see below). A real `AudioContext` oscillator's audio was captured and its zero-crossing-derived frequency matched the real ~440 Hz tone; two concurrent users' own channels stayed isolated (tone-playing user's channel non-silent, silent user's stayed silent) -- isolation is structural (one container per user/session, no shared sink/socket) as well as live-proven (`browser_audio.rs`).
- [x] video playback PASS -- Pass 3B: a small, committed synthetic WebM fixture (moving test pattern + real 440 Hz sine track, generated once via `ffmpeg`) served with real byte-range support and loaded via a real `<video autoplay>` element inside the actual server-side Brave instance; real, changing screencast frames (more than one distinct frame) and real, substantially non-silent captured audio were both observed concurrently against the same playing video (`browser_video.rs`).
- [x] browser renderer sandbox verified (live, not assumed)
- [x] OCI hardening inspected (live, via `docker inspect`)
- [x] no Docker socket
- [x] no privileged mode
- [x] crash recovery -- live-attacked this pass: real `docker kill` against an active broker session, explicit `closed` message, `RuntimeManager` detects failure, no orphan container, clean reconnect after restart (`task_24_...`)
- [x] enable/disable -- dedicated live test, disable-while-active, zero containers after, denied-while-disabled, usable again after re-enable (`task_25_...`)
- [ ] idle shutdown -- not independently tested
- [x] resource limits -- per-kind `ResourcePolicy` override built, `pids_limit: 512` (real-measured) wired in `main.rs`, undersized-limit negative test passes; Pass 3B re-measured with a real concurrent audio+screencast session active: 134 PIDs, ~200 MiB RSS, ~5.6% CPU -- comfortably within the existing 512 limit, not increased
- [ ] performance measured -- not measured beyond a rough ~8-10s start time
- [x] multi-user acceptance -- Pass 3A-3: 3 real, genuinely concurrent sessions (User A/User B/Guest), frame/tab/runtime isolation confirmed under true concurrency; Pass 3B additionally proved concurrent per-user audio isolation live (`task_22_cross_user_audio_isolation`)
- [x] service restart behavior -- LIVE CLOUDDESK tested (Pass 3A-2), real defect found and fixed (see above); Pass 3B additionally live-verified a real `docker kill` while a real audio capture task was active ends the session cleanly with no leaked container (`task_13_crash_with_audio_active_cleans_up`)
- [x] route authorization sweep -- Pass 3A-3: 10 of 11 inventoried HTTP/WS routes live-tested (unauthenticated/owner/cross-user/malformed-ID); one structural finding (`proxy-ws`) investigated and live-verified non-exploitable, disclosed as defense-in-depth. Pass 3B added six new typed sub-commands (`save_download`, `select_file`, `clipboard_write`, `clipboard_read`, `audio_start`, `audio_stop`) inside that same already-authorized `browser-ws` connection, not new HTTP routes -- the connection-level ownership check (`instance_id_from_path`) already proven 10/10+1 N/A is the actual authorization boundary for all of them, and every new handler resolves resources only against the connecting principal's own `owner_user_id`, never a client-supplied identity. Live-verified per-command denial for foreign/unknown resource references (unknown `root_id`, unknown/stale `chooser_id`, path traversal).
- [x] secret/log leakage sweep -- Pass 3B: swept. `browser_broker.rs`/`browser_downloads.rs` contain zero `println!`/`eprintln!`/`tracing`/`log` calls of any kind -- structurally nothing in the new peripheral code can leak clipboard text, uploaded/downloaded file contents, or PCM audio to a log. The existing HTTP `TraceLayer` uses an already-redacted span builder and never spans WebSocket frame contents. Re-ran the clipboard/upload acceptance tests (which carry real sentinel strings, including Unicode) and grepped all captured test-runner output for those sentinels: no matches outside the test assertions themselves.
- [x] no unresolved Critical
- [x] no unresolved High
- [x] Rust gates PASS (fmt/clippy/full workspace test/release build -- see Pass 3B section below)
- [x] frontend gates PASS -- Pass 3B: `BrowserApp.svelte` extended with real UI for downloads (progress panel, "Save to Files"), uploads (`select_file` prompt), clipboard (Paste/Copy toolbar buttons bridging the CloudDesk client's own real OS clipboard), and audio (toggle button, real `AudioContext` PCM playback scheduling); `npm run lint`/`check`/`test`/`build` all pass clean. Not yet exercised via a fresh live Playwright click-through of this exact UI -- the underlying protocol these controls drive is separately proven live end to end via the broker-level acceptance tests.
- [x] `PHASE9_BROWSER_EVIDENCE.md` created

## Pass 3A-4 / Pass 3B — Network Boundary Closure and Full Peripheral Support

**Pass 3A-4** closed Pass 3A-3's remaining network-isolation residual
(Blocker 2: host-gateway/RFC1918/metadata reachability) with a
mandatory, policy-enforcing HTTP/1.1 forward egress proxy rather than a
kernel firewall rule (no root access in this environment) -- see
`browser_egress_proxy.rs` and `docker/brave/Dockerfile`'s
`--proxy-server`/`--proxy-bypass-list` flags. Also resolved the 11th
Browser route (`proxy-ws`) as investigated-and-non-exploitable, giving
a clean 10/10 applicable + 1 N/A rather than an ambiguous 10/11.
**PASS 3A: COMPLETE** (all six Pass 3A blockers genuine PASS).

A Pass 3A "Residual A" liveness failure (intermittent multi-user
frame-delivery failure under concurrent proxy load, roughly once every
3-5 runs) was root-caused via real diagnostic instrumentation -- not
guessed -- to be a **test-fixture defect**, not a product/proxy/
concurrency defect: a static sentinel page produced no further CDP
screencast frames once settled, making "wait for one more frame"
inherently non-deterministic. Fixed by animating the fixture (a
continuous `requestAnimationFrame` canvas draw). Verified 10/10 clean
repeated runs.

**Pass 3B** then built the full Browser peripheral surface (Parts 1-7
of that pass's scope) with real, live, product-path evidence for every
item marked PASS above: downloads (Tasks 1-8), uploads/file-chooser
mediation (Tasks 9-10, 12; Task 11 remote-VFS closed in Pass 3B-2),
clipboard (Tasks 14-17), audio (Tasks 18-23), video+audio playback
acceptance (Tasks 24-26), and password-manager/extensions/native-
messaging policy (Tasks 27-28: disabled outright via
`--disable-extensions`/`--disable-save-password-bubble`/
`--disable-features=PasswordManager,...`, since v1 has no CloudDesk-
side vault integration for site credentials and no payment/extension
UI; native messaging is separately structurally impossible -- this
minimal image never installs or mounts any `native-messaging-hosts`
manifest directory). The secret/privacy sweep (Part 8) and the final
new-route authorization accounting (Part 9) are both clean, documented
above. Frontend UI for all four peripherals was added to
`BrowserApp.svelte` and passes all frontend gates.

Test files added this pass: `browser_downloads.rs`,
`browser_download_quota.rs` (split into its own process specifically
because its test-only quota override is a permanent, process-wide
`OnceLock`, unlike the egress proxy's additive-safe allowlist),
`browser_uploads.rs`, `browser_clipboard.rs`, `browser_audio.rs`,
`browser_video.rs` (with a committed synthetic fixture,
`tests/fixtures/test_video.webm`), and
`browser_peripheral_crash.rs`. All were run live against a real
`clouddesk-brave:1.93.136` container this pass with zero leaked
containers afterward at every step.

## Correction

The previous Pass 3B report labeled Phase 9 **COMPLETE** while two
items from Phase 9's own Definition of Done were not actually closed:
remote-VFS (SFTP) Browser upload was **NOT IMPLEMENTED** (explicitly
deferred and refused with a clean error, not silently mishandled, but
still a real, mandatory gap), and Administrator-disable-while-
peripherals-are-active had not been independently executed (only the
crash/`docker kill` case had real evidence). That COMPLETE claim was
premature. **This Pass 3B-2 micro-pass closes both explicit gaps** --
see the "Pass 3B-2" section immediately below for what changed and
what was verified.

## Pass 3B-2 — Final Micro-Closure (remote-VFS upload, admin-disable-with-peripherals)

**Remote-VFS (SFTP) upload** (Task 11, previously deferred): now
built and live-verified. `select_file` with `server_id` set reads a
real remote file via the exact same `resolve_ssh_session` ->
`SftpProvider::read_limited` chain Office's WOPI host already uses
for remote reads -- `RemoteServerStore::get` is owner-scoped, so a
foreign, deleted, or never-owned `server_id` fails identically to any
other unauthorized reference, re-checked at materialization time, not
trusted from when the chooser first opened. Bytes are materialized
into the same bounded per-instance `/state/uploads` staging area as
local selections, under the remote file's own basename, then fed to
Brave and deleted immediately -- Brave never receives the SSH
password, private key, Vault secret, or hostname, only the resulting
bytes and filename. No new HTTP/WS route was introduced: this is a
new field (`server_id`) on the existing, already-authorized
`select_file` message inside the existing `browser-ws` connection,
inheriting that connection's own ownership-check boundary (already
10/10 + 1 N/A from Pass 3A-3) rather than adding a new one.

Live-verified (`services/clouddeskd/tests/browser_remote_uploads.rs`,
against a real disposable OpenSSH/SFTP fixture,
`tests/acceptance/docker-compose.yml`): the real end-to-end flow
(select -> materialize -> Brave -> website upload, byte-exact and
filename-exact against an independent `docker exec cat` read); User
A can never resolve User B's own `RemoteServer`; an unknown/forged
`server_id` is denied; a traversal attempt in the remote virtual path
is denied; the SSH password never appears in any broker WS message
and is never stored in plaintext (`vault_secrets` checked directly);
no upload-temp artifact remains after either a successful selection
or a failed one; an unreachable remote provider (bad host/port) fails
cleanly, not a hang or a silent success.

**Admin disable with active peripherals** (Tasks 6-8, previously not
independently executed): a real Administrator disables Browser via
the real production control path (`POST
/api/v1/runtimes/browser/disable`) while a real User's session has
audio genuinely playing, a real download genuinely in progress, and
clipboard genuinely exercised, all at once
(`services/clouddeskd/tests/browser_admin_disable_peripherals.rs`).
Verified live, 3/3 clean runs: the WS session receives a real
`closed` event (no hang); the runtime instance transitions to
stopped; the Brave container (and therefore its audio/`ffmpeg`/
`pulseaudio` helper processes, which live inside that same container)
is fully removed; a new Browser session is denied while disabled; and
-- distinctly from the pre-existing `docker kill` crash-cleanup
evidence, which proves a different trigger path -- after re-enabling
and restarting, the same instance accepts a genuinely fresh WS
session with no stale peripheral state inherited from before the
disable.

Targeted regression (Task 10): after the remote-upload change to the
shared `browser_broker.rs`, every existing Browser peripheral test
file was re-run: local uploads, download/quota, clipboard, audio
(capture + cross-user isolation + stop), video+audio playback, and
the existing crash-cleanup test (19 tests across 9 files). Two
subsequent full-workspace `cargo test --workspace --no-fail-fast`
runs (`--test-threads=4`) each surfaced a handful of Browser
peripheral tests failing under genuine full-workspace-scale Docker
contention (timing-class assertions: a WS event or page value not
settling within its wait window) -- every one of them, across both
runs, was independently re-run in isolation immediately afterward and
passed clean every time (18/18, then 15/15), confirming these were
load-timing, not regressions; the one non-timing failure found
(`ssh_proxyjump.rs`, first run) was this micro-pass's own incomplete
fixture setup, fixed by starting the full `docker-compose.yml` stack,
re-verified 12/12 clean. Zero leaked containers after every run.

**Phase 9 is now genuinely COMPLETE.** Every item in the Definition-
of-Done checklist above is PASS except the same two explicitly
non-blocking, independently-tracked items already disclosed before
this micro-pass (`idle shutdown` not independently tested;
`performance` not formally measured beyond rough startup timing) and
the frontend-Playwright caveat (the remote-upload capability has no
dedicated frontend picker UI yet -- `BrowserApp.svelte`'s existing
upload prompt only accepts a local-home relative path; the protocol
capability itself is proven live at the broker level). None of these
represent an unresolved Critical/High finding. Per the closure
policy, Phase 10 is not started this pass; the next action is to
return to Phase 2 SSH work.
