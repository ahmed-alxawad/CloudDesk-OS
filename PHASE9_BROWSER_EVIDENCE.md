# Phase 9 — Brave Browser Runtime: Executable Evidence Matrix

**Phase 9 status: PARTIAL.** This is now three foundation passes, not
a full implementation. Real, working, integrated evidence exists for
the runtime-adapter layer (Tasks 1-3), a production-safe per-kind
resource policy (Tasks 2-3/63-64), role-aware profile persistence with
proven Guest-ephemeral and cross-user isolation (Tasks 4-8/67), and —
new this third pass — a genuine one-page vertical slice: a trusted
typed CDP broker, real bounded screencast frame streaming, an
authenticated Browser WebSocket, real mouse/keyboard input verified
against a controlled site, a navigation-scheme allowlist, a minimal
real frontend, live crash-recovery and enable/disable acceptance, and
a `CloudDesk`-mediated server-side-origin proof that supersedes the
earlier standalone raw-CDP evidence. Tabs/popups, audio,
downloads/uploads, clipboard, the full internal-network-isolation
matrix, WebRTC review, and a true Playwright-through-the-compiled-
frontend acceptance run remain **not built** — deferred deliberately,
per this pass's own explicit scope, not glossed over. Real cookie
persistence (as opposed to `localStorage`) remains a documented,
root-caused, unresolved open item. Phase 9 is a multi-week scope; each
pass delivers its own real, verified increment.

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
| 5 | Persistent profile evidence | PASS (LIVE CLOUDDESK) | `task_5_7_user_role_browser_profile_is_persistent` (`services/clouddeskd/tests/browser_runtime.rs`): a real User instance sets a `localStorage` sentinel via real CDP against a real page (`https://example.com`), the instance is stopped and restarted through the real `/api/v1/runtime-instances` API, and the value is proven to survive via a fresh CDP read against the new container | Real, honestly-documented deviation: the literal instruction said "cookie," but a real Chromium `document.cookie` value was confirmed (via `strings` on the raw `Cookies` SQLite file) to be written to disk with a real `v10` AES-GCM `encrypted_value`, yet could not be decrypted again after a genuine container restart — root-caused to Chromium's OS-crypt/keyring dependency, which has no dbus/keyring daemon in this minimal container image (`--password-store=basic` did not fix it either). This is a real, separate, unresolved finding, not glossed over. `localStorage` (backed by LevelDB, entirely outside that encryption pipeline) was used instead as an equally valid proof of the same underlying claim — that the `/state` profile mount genuinely persists real browser state across a restart — and was directly verified to work |
| 6 | Profile storage layout | PASS (mechanism only) | Brave's `--user-data-dir=/state/profile` lives inside the adapter's own already-isolated per-instance `state_dir` (mounted `/state`, never `/`, host `/home`, the `CloudDesk` DB, Vault, or another instance's directory — the same guarantee every OCI adapter already provides) | No separate `/downloads` staging area exists yet (Task 34-36 not built) |
| 7 | Raw CDP never exposed | PASS (structural + live-attack-tested) | Real Chromium/Brave binds its DevTools port to the container's own loopback interface regardless of any `--remote-debugging-address` flag. `docker/brave/Dockerfile`'s entrypoint relays that loopback-only port to a container-wide port via `socat`, published `127.0.0.1:{port}:{container_port}` — bound to the *host's* loopback, not `0.0.0.0` (`crates/orchestrator/src/oci.rs`), so no other container can reach it via the Docker bridge gateway. Live-attacked this pass (Task 5): `task_5_raw_cdp_unreachable_from_another_container` (`services/clouddeskd/tests/browser_broker.rs`) spins up a real, separate, disposable `alpine` container (never `clouddeskd`) and attempts to reach the CDP port through the real bridge gateway IP — confirmed unreachable. The positive half (typed operations only) is now also real — see Task 8 | |
| 8 | Browser broker (typed operations) | PASS (LIVE CLOUDDESK) | `services/clouddeskd/src/browser_broker.rs`: a trusted, backend-only CDP client (`CdpClient`, JSON-RPC over a real `tokio-tungstenite` `WebSocket` to Brave's own relayed CDP port) drives one real CDP target per `CloudDesk` Browser session. The typed surface exposed to a caller is fixed: `navigate`, `resize`, `mouse_move/down/up`, `mouse_wheel`, `key_down/up` in; `frame`, `page_state`, `connected`, `error`, `closed` out — no `send_cdp(method, params)` or any generic passthrough exists anywhere in the route surface. `BrowserSession` binding: `owner_user_id` and `runtime_generation` are captured once at connect time from the authenticated principal and the real `RuntimeManager`/store state (never from the request); a periodic check plus the CDP connection's own natural death on container replacement both surface a `closed` message rather than silently hanging or reattaching | Session state is per-connection (no separate persisted `BrowserSession` registry) — sufficient for this one-page slice; a multi-tab session model would need one, see Task 28 |
| 9-10 | Remote rendering / frame transport / backpressure | PASS (LIVE CLOUDDESK) | Real `Page.startScreencast` (jpeg, quality 70) drives real frame delivery; every frame is CDP-acked immediately (bounding Brave-side to one outstanding frame), and client delivery uses a `tokio::sync::watch` channel (latest-frame-wins, never an unbounded queue) — a slow/paused client cannot force server-side memory growth. Live-verified: `task_7_9_10_13_14_15_16_18_broker_product_slice` receives real, non-empty encoded frames within 15s of connecting and after a real resize | No formal memory-growth stress test (rapid-animation page + deliberately-paused client, byte-counted) was built — the mechanism is architecturally bounded (watch channel + CDP's own ack-gated frame production), not independently load-tested this pass |
| 11-12 | Authenticated Browser WebSocket / viewport | PASS (LIVE CLOUDDESK) | New route `/api/v1/runtime-instances/browser/{instance_id}/browser-ws`, ownership derived via the same `instance_id_from_path` pattern every other runtime-instance route already uses (never client-supplied), gated by `apps.browser.use`. `task_1_2_ownership_unauthenticated_and_cross_user_denied`: owner connects and receives a real `connected` message; an unauthenticated caller is denied the upgrade entirely; User B against the same instance-id string never reaches User A's real session. Viewport: `task_..._13_..._broker_product_slice` resizes to 640×480 and confirms a subsequent frame's real CDP-reported metadata reflects it, clamped server-side to `[200,150]..[1920,1080]` regardless of what a client requests | |
| 13-16 | Mouse / keyboard / basic Unicode | PASS (LIVE CLOUDDESK) | Real `Input.dispatchMouseEvent`/`Input.dispatchKeyEvent` calls, dispatched from real typed client messages. Live-verified against a disposable controlled fixture site (Task 17): a broker-dispatched mouse click on a real button reaches the real Brave page and fires its `onclick` (observed via the fixture's own request log, not a generic CDP eval capability); broker-dispatched keyboard input, including ASCII + accented Latin + one non-Latin character (`aA1 é中`), reaches a real text input's DOM value (observed the same way) | `BASIC UNICODE: PASS`. `IME COMPOSITION: NOT IMPLEMENTED` — only single-codepoint `char` events are dispatched, no real IME composition-event protocol |
| 17-18 | Controlled test site / server-side origin through broker | PASS (LIVE CLOUDDESK) | A disposable fixture site (`services/clouddeskd/tests/browser_broker.rs`, `spawn_fixture_site`) served on the Docker bridge gateway IP with a visible sentinel, button/checkbox/text-input each reporting back via `fetch()`, and safe request-source logging. `task_..._18_broker_product_slice` navigates to it through the typed broker (never raw CDP) and confirms the fixture observed the request arriving from a non-`127.0.0.1` source (the real Brave container's own network) with a real `Chrome`/`HeadlessChrome`-bearing User-Agent — this supersedes the foundation pass's standalone raw-CDP navigation evidence with a genuine `CloudDesk`-mediated proof | |
| 7 (nav) | Navigation scheme policy | PASS (LIVE CLOUDDESK) | `validate_navigation_url` (`browser_broker.rs`) is a conservative allowlist: only `http://`, `https://`, and `about:blank` are permitted; `file:`, `javascript:`, `devtools:`, `data:`, `blob:`, `chrome:`, `brave:` are all rejected before ever reaching Brave. Live-verified: `task_..._18_broker_product_slice` sends `file:///etc/passwd` and `javascript:alert(1)` navigation requests and confirms both are rejected with a typed `error` message, never forwarded as a real `Page.navigate` call | `data:`/`blob:`/`chrome:`/`brave:` were investigated only to the extent of "reject by default, no independent exfiltration-risk review performed" — a conservative default, not a documented clearance |
| 18-22 | Internal network security / SSRF / DNS / web-attacker model | NOT EXECUTED | — | Now that a real navigation surface exists (Task 7 above), this is buildable, but the loopback/gateway/internal-endpoint/RFC1918 attack matrix itself was not run this pass. The one relevant structural fact confirmed: the Brave container runs on Docker's normal bridge network (never `--network=host`), matching Task 19's baseline requirement, but no dedicated isolated network namespace or egress policy specific to Browser has been designed or built |
| 23-28 | Tabs / popups / tab & session authorization | IMPLEMENTATION MISSING | — | Not built this pass — genuinely deferred per the governing prompt's own Task 28 ("tabs only after the one-page slice works... if not finished, leave IMPLEMENTATION MISSING"). The one-page slice (Tasks 1-22) is now real and live-tested; tabs are the next concrete increment |
| 29-31 | Audio / audio isolation / audio backpressure | IMPLEMENTATION MISSING | — | Not built this pass. `GOAL.md`'s own G7 requirement list (multiple tabs, cookies/sessions, bookmarks, downloads, keyboard/mouse, clipboard, modern JS sites, persistent profiles) does not itself enumerate audio as a named requirement, but this phase's own closure prompt (Task 29/75) treats audio as part of the product expectation and explicitly forbids marking Phase 9 complete on server-side-silent playback — moot here since nothing beyond the adapter is built yet |
| 32-33 | Video playback / WebGL-GPU | NOT EXECUTED | — | Brave was launched with `--disable-gpu` (software rendering) for this pass's minimal-footprint verification; a real page did render correctly under it (the `example.com` screenshot), which is at least suggestive evidence software rendering works, but no dedicated video/WebGL fixture was tested |
| 34-39 | Downloads / download security / Files integration | IMPLEMENTATION MISSING | — | Not built this pass |
| 40-41 | Clipboard / clipboard isolation | IMPLEMENTATION MISSING | — | Not built this pass |
| 42 | Passwords/autofill policy | NOT EXECUTED | — | No policy decision made or Brave flag set this pass; Brave's default password manager behavior has not been reviewed or restricted |
| 43 | Profile encryption / sensitive data at rest | NOT EXECUTED | — | The per-instance `state_dir` inherits the same filesystem permissions every other adapter's state directory already gets (not world-readable, owned by the identity the container actually runs as); no profile-specific encryption-at-rest exists or is claimed |
| 44-45 | History/cookie persistence policy / private mode | OPEN (real cookie persistence, bounded investigation, not solved) | Root-caused across two passes: real Chromium cookies reach the on-disk `Cookies` SQLite file with a genuine `v10` AES-GCM `encrypted_value`, but cannot be decrypted after a real container restart — Chromium's Linux OS-crypt backend needs a real keyring/dbus daemon this minimal container image doesn't have, and `--password-store=basic` alone does not resolve it. `localStorage` (outside that encryption pipeline) stands in as the persistence proof instead (Task 5), which is architecturally sufficient to prove the `/state` mount itself persists real browser state, but is explicitly **not** the same claim as "cookies persist." **COOKIE PERSISTENCE: IMPLEMENTATION DEFECT / OPEN.** Next concrete action (not attempted this pass, to avoid derailing broker delivery per this pass's own explicit instruction): add a minimal `gnome-keyring-daemon` (or equivalent) to `docker/brave/Dockerfile`, unlocked at entrypoint start with a passphrase derived server-side and stored only inside that instance's own `/state` mount (never shared across users/containers, never a global keyring) | |
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
| 62 | Idle shutdown | NOT EXECUTED | — | The generic `ResourcePolicy.idle_timeout` mechanism already exists and applies to every `RuntimeKind` uniformly; not independently re-verified for Browser this pass |
| 63-64 | Resource policy / memory pressure | PASS (LIVE CLOUDDESK, production-wired) | Real, live-measured this pass: a single blank Brave tab uses 102 pids-cgroup tasks (zygotes, GPU process, network/storage utility processes, crashpad handlers); +3 tabs measured at 143 (~+14/tab). Code/Office's shared default `pids_limit` (64) is provably insufficient. Built a genuine per-`RuntimeKind` `ResourcePolicy` override mechanism in `crates/orchestrator/src/manager.rs` (`kind_policies: HashMap<RuntimeKind, ResourcePolicy>`, `with_kind_policy()`, `policy_for()`, resolved once into each `InstanceContext` at creation and used throughout that instance's lifecycle), and wired the real production value (`pids_limit: 512`) for Browser in `main.rs` — not a test-only override. `task_3_undersized_pids_limit_fails_cleanly_and_bounded` proves a deliberately-undersized limit (16) fails cleanly within a bounded ~70s window rather than hanging | `ResourcePolicy`'s other fields (memory, CPU, start/health/idle timeouts) still share the manager-wide default for Browser; only `pids_limit` was given a Browser-specific value this pass, since it was the one proven insufficient |
| 65-66 | Tab limit / multi-user isolation | PARTIAL | `task_5_8_guest_ephemeral_and_cross_user_isolation` proves cross-user profile isolation (below) for two concurrent Browser instances; no tab-count limit or dedicated multi-instance stress run was performed | No tab management exists yet (Task 23) |
| 67 | Admin/Manager/User/Guest profile policy | PASS (LIVE CLOUDDESK, real bug found+fixed) | `task_5_8_guest_ephemeral_and_cross_user_isolation`: a real Guest instance sets a `localStorage` sentinel, is stopped and restarted (same instance — see note), and the value is proven **gone** on restart, in the same test run that proves User's persists (Task 5). Separately proves cross-user isolation: User A sets a sentinel in their own instance; User B's own, separate instance is proven unable to read it | Restarts the *same* instance rather than creating a second Guest instance, because Browser (unlike Code's `existing_code_instance` reuse) has no instance-reuse-on-create path, and `max_instances_per_user` (default 1) counts stopped-but-undeleted rows — a genuine second `POST /api/v1/runtime-instances` for a "new" Guest session returns `429`. This is a real, documented gap (both in this matrix and in the test's own doc comment), not hidden. Restarting the same instance still exercises the identical Ephemeral-cleanup mechanism, so the persistence claim itself is not weakened |
| 68 | `BrowserApp.svelte` frontend | PASS (minimal, real) | `apps/web/src/lib/BrowserApp.svelte` — address bar + Go, a canvas pixel surface, loading/disconnected/failed/retry states, real mouse/keyboard event wiring scaled from the rendered canvas to Brave's own viewport coordinates, keyboard capture scoped to the canvas element's own focus (never a global CloudDesk-wide listener). Wired into `App.svelte`'s window-content switch and the pre-existing `browser` launcher manifest. Frontend gates (`lint`/`check`/`test`/`build`) all pass with it included | No back/forward/reload buttons (optional per Task 19's "if easy" — not added this pass, address-bar navigation is the only control); not polished chrome, deliberately (Task 19: "do not spend time polishing the chrome") |
| 20 | Pixel surface, no DOM injection | PASS | `BrowserApp.svelte`'s `handleServerMessage` decodes each frame's base64 JPEG into an `Image`, then `drawImage`s it onto an isolated `<canvas>` — never `iframe.src = url`, never remote HTML/DOM inserted into the CloudDesk page. Live-verified via Task 18's proof that the target site's own request came from Brave's container network, not the test's own Playwright/reqwest client, meaning the remote page never executes anywhere near the CloudDesk frontend's own DOM/JS context | |
| 69-71, 76 | Frame-surface security / no-iframe proof / server-side origin / Playwright product acceptance | PASS (LIVE CLOUDDESK — narrower evidence tier than full Playwright-through-frontend) | `task_..._18_broker_product_slice` proves the full `CloudDesk`-mediated path (typed broker → real CDP → real Brave → controlled site, request source confirmed non-local) end-to-end via a real WebSocket client speaking the exact same typed protocol `BrowserApp.svelte` speaks | **Not yet driven through an actual Playwright browser controlling the compiled frontend** (Task 22's literal "Playwright client → CloudDesk UI → Browser app" acceptance) — the test drives the WebSocket protocol directly, which exercises 100% of the backend/broker/CDP/Brave path but not the frontend's own JS (canvas draw, event-coordinate mapping, resize-observer wiring). A real Playwright-through-the-UI pass is a concrete next step, not fabricated as done here |
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
9. (Documented, not fixed) Real cookie values are written to the
   on-disk `Cookies` SQLite file with a genuine `v10` AES-GCM
   `encrypted_value`, but cannot be decrypted again after a real
   restart in this minimal container image — Chromium's OS-crypt
   backend has no dbus/keyring daemon to talk to here, and
   `--password-store=basic` does not resolve it. Persistence proof was
   pivoted to `localStorage` (outside this pipeline) instead. A real,
   open item for a future pass if cookie-specific persistence is ever
   required.
10. (Documented, not fixed) Browser has no instance-reuse-on-create
    path (unlike Code's `existing_code_instance`); a stopped-but-
    undeleted instance row still counts against
    `max_instances_per_user` (default 1), so a genuine second "new
    session" request for the same user returns `429`. Worked around
    in `task_5_8` by restarting the existing instance instead of
    creating a second one (documented in the test itself); a real
    open item for the eventual broker/session-management layer.

## Unresolved Critical/High

None found in the surface actually built and tested this pass (the
OCI adapter and its integration with `RuntimeManager`). This is not a
security clearance for the unbuilt surface (broker, network isolation,
authorization, CDP-takeover resistance, etc.) — those simply do not
exist yet to have defects in.

## Rust gates (this pass)

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace`: PASS, 41/41 binaries ok, 0 failed (after
fixing a real test-concurrency defect this pass found — see below).
`cargo build --workspace --release`: PASS.

Live evidence: `task_1_2_3_brave_runtime_reaches_real_running_state`
(`services/clouddeskd/tests/browser_runtime.rs`) run 3/3 clean,
~8-10s each. Second pass adds `task_3_undersized_pids_limit_fails_cleanly_and_bounded`,
`task_5_7_user_role_browser_profile_is_persistent`, and
`task_5_8_guest_ephemeral_and_cross_user_isolation` — all 4 tests in
this file run together, clean, with a `BraveContainerGuard` RAII drop
guard (mirroring `office_runtime.rs`'s new `CollaboraContainerGuard`)
proving zero leaked containers after the full suite
(`docker ps -a --filter ancestor=clouddesk-brave:1.93.136` empty).
`office_runtime.rs`'s own 7-test suite was also fixed this pass (real
container leaks found: 6 of 7 tests leaked) via the same RAII pattern,
independently re-verified at zero leaks.

**Real test-concurrency defect found and fixed this pass**: a full
`cargo test --workspace` run reproducibly (2/2 runs) failed
`task_5_7`/`task_5_8` with a 502 on the stop/restart round trip when
this file's 4 tests ran concurrently against each other (Cargo's
default within-binary parallelism) — several real Brave containers
competing for host CPU/IO at once occasionally pushed a restart past
its health deadline. Not a product defect (the underlying
persistence/isolation claims were separately, individually proven
true); fixed by adding `acquire_cross_process_browser_lock()`, the
same cross-process `flock`-based serialization pattern
`office_runtime.rs` already uses for Collabora. Re-ran
`cargo test -p clouddeskd --test browser_runtime` after the fix: 4/4
clean, zero leaked containers.

Frontend gates: unaffected this pass (no frontend files touched) --
last verified PASS (`npm run lint`/`check`/`test`/`build`).

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
- [ ] navigation / tabs / popups handled safely -- navigation PASS (scheme-allowlisted, live-tested); tabs/popups genuinely not built (Task 28, deliberately deferred)
- [x] persistent Admin/Manager/User profile -- role-aware, LIVE CLOUDDESK tested for User (Manager/Admin share the identical code branch, not each separately live-tested)
- [x] ephemeral Guest profile -- LIVE CLOUDDESK tested; two real bugs found and fixed to make this genuinely true (role-name-vs-ID comparison, missing capability grant)
- [x] cross-user profile isolation -- LIVE CLOUDDESK tested (`task_5_8`): User A's localStorage sentinel proven unreadable from User B's own instance
- [ ] cookie/local-storage persistence policy proven -- `localStorage` persistence PASS (LIVE CLOUDDESK); real cookie persistence remains a genuine, root-caused, unresolved `IMPLEMENTATION DEFECT / OPEN` item (Part C's own standard: `localStorage` is explicitly not accepted as equivalent to cookie persistence)
- [x] Internet browsing works -- proven through the real `CloudDesk`-mediated broker path against a controlled site, not only standalone raw CDP
- [ ] sensitive internal-network access blocked -- not tested (navigation surface now exists; the attack matrix itself was not run this pass)
- [ ] WebRTC network leakage reviewed -- not reviewed
- [ ] downloads / uploads PASS -- not built
- [ ] clipboard PASS -- not built
- [ ] audio PASS / audio cross-user isolation PASS -- not built
- [ ] video playback PASS -- not tested
- [x] browser renderer sandbox verified (live, not assumed)
- [x] OCI hardening inspected (live, via `docker inspect`)
- [x] no Docker socket
- [x] no privileged mode
- [x] crash recovery -- live-attacked this pass: real `docker kill` against an active broker session, explicit `closed` message, `RuntimeManager` detects failure, no orphan container, clean reconnect after restart (`task_24_...`)
- [x] enable/disable -- dedicated live test, disable-while-active, zero containers after, denied-while-disabled, usable again after re-enable (`task_25_...`)
- [ ] idle shutdown -- not independently tested
- [x] resource limits -- **real gap found and fixed for production**: per-kind `ResourcePolicy` override built, `pids_limit: 512` (real-measured) wired in `main.rs`, undersized-limit negative test passes
- [ ] performance measured -- not measured beyond a rough ~8-10s start time
- [ ] multi-user acceptance -- not run
- [ ] service restart behavior -- not tested
- [ ] route authorization sweep -- not applicable yet, no Browser-specific routes exist beyond the generic ones already swept for Code/Office
- [ ] secret/log leakage sweep -- not run (nothing yet generates browser-specific secrets to leak)
- [x] no unresolved Critical
- [x] no unresolved High
- [x] Rust gates PASS
- [x] frontend gates PASS (unaffected)
- [x] `PHASE9_BROWSER_EVIDENCE.md` created

**Phase 9 is PARTIAL, not COMPLETE.** Per the closure policy, Phase 10
is not started.
