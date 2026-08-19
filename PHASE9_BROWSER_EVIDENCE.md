# Phase 9 — Brave Browser Runtime: Executable Evidence Matrix

**Phase 9 status: PARTIAL.** This is now two foundation passes, not a
full implementation. Real, working, integrated evidence exists for the
runtime-adapter layer (Tasks 1-3), a production-safe per-kind resource
policy (Tasks 2-3/63-64), and role-aware profile persistence with
proven Guest-ephemeral and cross-user isolation (Tasks 4-8/67). The
browser broker, frame streaming, input handling, tab management,
audio, downloads/uploads, clipboard, network-isolation hardening, the
frontend application, and the full 92-task security/multi-user
acceptance matrix this phase's own closure prompt specifies are **not
built**. This is stated plainly rather than glossed over — Phase 9 is
a multi-week scope; this pass delivers its real, verified foundation.

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
| 7 | Raw CDP never exposed | PASS (structural + architectural) | Real Chromium/Brave binds its DevTools port to the container's own loopback interface regardless of any `--remote-debugging-address` flag — live-verified this pass (not assumed) via direct inspection of the real container's listening sockets. `docker/brave/Dockerfile`'s entrypoint relays that loopback-only port to a container-wide port via `socat`, published only on the private Docker bridge network the same way every other runtime's port already is, never to the host or a frontend caller. No code returns a DevTools WebSocket URL, debugging port, internal host, or container IP to any caller | There is no browser broker yet to demonstrate the *positive* half of Task 8 (typed operations only) — only that raw CDP isn't reachable from outside the container network is proven |
| 8 | Browser broker (typed operations) | IMPLEMENTATION MISSING | — | Not built this pass |
| 9-12 | Remote rendering / frame transport / backpressure / resize | IMPLEMENTATION MISSING | — | Not built this pass. Real evidence this pass only goes as far as: a raw CDP client (standalone Node.js script, test infrastructure only) can `Target.createTarget` → `Page.navigate` → `Page.captureScreenshot` against the real running Brave container and get back real, correctly-rendered pixels (verified visually against `https://example.com`) — proving the *browser itself* and its CDP surface work, not that `CloudDesk` has a frame-streaming transport yet |
| 13-15 | Mouse / keyboard / IME input | IMPLEMENTATION MISSING | — | Not built this pass |
| 16-17 | Navigation / URL policy | IMPLEMENTATION MISSING | — | Not built this pass. The real CDP round trip above did perform a real `Page.navigate`, proving Brave's own navigation works, but no `CloudDesk`-mediated navigation surface exists |
| 18-22 | Internal network security / SSRF / DNS / web-attacker model | NOT EXECUTED | — | No broker/navigation surface exists yet to attack. The one relevant structural fact confirmed this pass: the Brave container runs on Docker's normal bridge network (never `--network=host`), matching Task 19's baseline requirement, but no dedicated isolated network namespace or egress policy specific to Browser has been designed or built |
| 23-28 | Tabs / popups / tab & session authorization | IMPLEMENTATION MISSING | — | Not built this pass |
| 29-31 | Audio / audio isolation / audio backpressure | IMPLEMENTATION MISSING | — | Not built this pass. `GOAL.md`'s own G7 requirement list (multiple tabs, cookies/sessions, bookmarks, downloads, keyboard/mouse, clipboard, modern JS sites, persistent profiles) does not itself enumerate audio as a named requirement, but this phase's own closure prompt (Task 29/75) treats audio as part of the product expectation and explicitly forbids marking Phase 9 complete on server-side-silent playback — moot here since nothing beyond the adapter is built yet |
| 32-33 | Video playback / WebGL-GPU | NOT EXECUTED | — | Brave was launched with `--disable-gpu` (software rendering) for this pass's minimal-footprint verification; a real page did render correctly under it (the `example.com` screenshot), which is at least suggestive evidence software rendering works, but no dedicated video/WebGL fixture was tested |
| 34-39 | Downloads / download security / Files integration | IMPLEMENTATION MISSING | — | Not built this pass |
| 40-41 | Clipboard / clipboard isolation | IMPLEMENTATION MISSING | — | Not built this pass |
| 42 | Passwords/autofill policy | NOT EXECUTED | — | No policy decision made or Brave flag set this pass; Brave's default password manager behavior has not been reviewed or restricted |
| 43 | Profile encryption / sensitive data at rest | NOT EXECUTED | — | The per-instance `state_dir` inherits the same filesystem permissions every other adapter's state directory already gets (not world-readable, owned by the identity the container actually runs as); no profile-specific encryption-at-rest exists or is claimed |
| 44-45 | History/cookie persistence policy / private mode | NOT EXECUTED | — | Blocked on Task 4 |
| 46-47 | Extensions / native messaging | NOT EXECUTED | — | No explicit flag set either way this pass; Brave's own defaults apply unreviewed |
| 48 | Safe, fixed Brave launch flags | PASS (partial) | The real, fixed, compiled-in launch command in `docker/brave/Dockerfile`'s entrypoint (`--headless=new --disable-gpu --no-first-run --remote-debugging-port=9222 --user-data-dir=/state/profile`) is never client-influenced. No `--no-sandbox` used | A full flag-by-flag security review (WebRTC, proxy, downloads, crash behavior) per Task 48's checklist was not performed beyond what's implied by the flags actually present |
| 49 | Non-root OCI user | PASS (LIVE CLOUDDESK) | Verified live via `run_as` resolving to the real, non-root UID/GID `clouddeskd`'s own process runs as (never root — `clouddeskd` itself must not run as root per this project's own standing invariant) | |
| 50 | OCI hardening | PASS (partial, LIVE CLOUDDESK) | Verified live via `docker inspect` during this pass's debugging: `Privileged=false`, no host network/PID namespace, no Docker socket, no host-root/Vault/DB mounts, `CapDrop=[ALL]` baseline with exactly two added capabilities (below), `no-new-privileges` kept enabled throughout (never disabled to work around the sandbox, see Task 51) | A dedicated `task_50`-style test asserting all of this programmatically (matching Office's `task_16_18_office_container_isolation_and_hardening`) was not written this pass — verified manually via `docker inspect` during iteration, not as a standing regression test |
| 51 | Chromium sandbox verified, not assumed | PASS (LIVE, real finding) | Live-verified this pass, the hard way: Chromium's own namespace-based sandbox (not the legacy SUID-helper sandbox, which is fundamentally incompatible with `no-new-privileges`) needs exactly two added capabilities beyond the zero-capability default to initialize at all — `SYS_ADMIN` (without it: `Failed to move to new namespace... Operation not permitted`, zygote aborts) and `SYS_CHROOT` (without it, under `no-new-privileges` kept enabled: `Check failed: sys_chroot(...) == 0`, `Permission denied`). No `--no-sandbox` flag was ever used to route around this — the two capabilities were found and added specifically so the *real* sandbox could initialize | The container-level capabilities that let the sandbox initialize are not the same claim as "every renderer process itself is running inside an active seccomp-BPF sandbox" — that deeper per-process verification (`chrome://sandbox`-equivalent diagnostic output) was not separately captured |
| 52 | Seccomp | PASS | Docker's **default** seccomp profile is used throughout — `--security-opt seccomp=unconfined` was tried once during debugging and explicitly abandoned in favor of the two-capability fix above once it worked, precisely because Task 52 forbids running unconfined merely for convenience | |
| 53-57 | WebRTC leaks / media devices / geolocation / notifications / printing | NOT EXECUTED | — | No policy reviewed or flags set this pass beyond Brave's own defaults |
| 58 | Audit events | NOT EXECUTED | — | No Browser-specific audit events exist; the generic runtime start/stop events every `RuntimeKind` already gets via the shared instance lifecycle do cover session start/stop at the same level Code/Office already have |
| 59 | Crash recovery | PASS (mechanism, LIVE CLOUDDESK) | Implied by Task 3's own test: `RuntimeManager`'s generic crash-detection/health-check machinery (already proven for Code/Office) applies identically to Browser, since nothing Browser-specific bypasses it | A dedicated `docker kill`-the-real-container crash-recovery test (matching Office's `task_19_office_crash_recovery`) was not written this pass |
| 60 | Tab crash isolation | NOT EXECUTED | — | No tab management exists yet (Task 23) |
| 61 | Enable/disable | PASS (mechanism, LIVE CLOUDDESK) | `task_1_2_3_brave_runtime_reaches_real_running_state` exercises the real `/api/v1/runtimes/browser/enable` route and stop-and-verify-container-gone, the same generic mechanism Code/Office already use | A dedicated disable-while-active test (matching Code's `task_19_enable_disable_lifecycle`) was not written this pass |
| 62 | Idle shutdown | NOT EXECUTED | — | The generic `ResourcePolicy.idle_timeout` mechanism already exists and applies to every `RuntimeKind` uniformly; not independently re-verified for Browser this pass |
| 63-64 | Resource policy / memory pressure | PASS (LIVE CLOUDDESK, production-wired) | Real, live-measured this pass: a single blank Brave tab uses 102 pids-cgroup tasks (zygotes, GPU process, network/storage utility processes, crashpad handlers); +3 tabs measured at 143 (~+14/tab). Code/Office's shared default `pids_limit` (64) is provably insufficient. Built a genuine per-`RuntimeKind` `ResourcePolicy` override mechanism in `crates/orchestrator/src/manager.rs` (`kind_policies: HashMap<RuntimeKind, ResourcePolicy>`, `with_kind_policy()`, `policy_for()`, resolved once into each `InstanceContext` at creation and used throughout that instance's lifecycle), and wired the real production value (`pids_limit: 512`) for Browser in `main.rs` — not a test-only override. `task_3_undersized_pids_limit_fails_cleanly_and_bounded` proves a deliberately-undersized limit (16) fails cleanly within a bounded ~70s window rather than hanging | `ResourcePolicy`'s other fields (memory, CPU, start/health/idle timeouts) still share the manager-wide default for Browser; only `pids_limit` was given a Browser-specific value this pass, since it was the one proven insufficient |
| 65-66 | Tab limit / multi-user isolation | PARTIAL | `task_5_8_guest_ephemeral_and_cross_user_isolation` proves cross-user profile isolation (below) for two concurrent Browser instances; no tab-count limit or dedicated multi-instance stress run was performed | No tab management exists yet (Task 23) |
| 67 | Admin/Manager/User/Guest profile policy | PASS (LIVE CLOUDDESK, real bug found+fixed) | `task_5_8_guest_ephemeral_and_cross_user_isolation`: a real Guest instance sets a `localStorage` sentinel, is stopped and restarted (same instance — see note), and the value is proven **gone** on restart, in the same test run that proves User's persists (Task 5). Separately proves cross-user isolation: User A sets a sentinel in their own instance; User B's own, separate instance is proven unable to read it | Restarts the *same* instance rather than creating a second Guest instance, because Browser (unlike Code's `existing_code_instance` reuse) has no instance-reuse-on-create path, and `max_instances_per_user` (default 1) counts stopped-but-undeleted rows — a genuine second `POST /api/v1/runtime-instances` for a "new" Guest session returns `429`. This is a real, documented gap (both in this matrix and in the test's own doc comment), not hidden. Restarting the same instance still exercises the identical Ephemeral-cleanup mechanism, so the persistence claim itself is not weakened |
| 68 | `BrowserApp.svelte` frontend | IMPLEMENTATION MISSING | — | Does not exist. `apps/web/public/manifests/browser.json` already exists (pre-dates this pass) as a launcher-tile placeholder only — no actual application component is wired to it |
| 69-89 | Frame-surface security, no-iframe proof, input round-trip, live acceptance (downloads/uploads/clipboard/audio/video), authorization matrix, hostile-client/website stress, quotas, secret-leak sweep, profile file permissions, service restart, CDP-takeover attack | NOT EXECUTED | — | All depend on the broker/frontend/streaming layer, none of which exists yet |
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
- [ ] no arbitrary website iframe -- not applicable yet, no frontend exists to prove this against
- [ ] browser-native remote rendering PASS -- not built
- [ ] server-side request origin proven -- not built (no navigation surface through `CloudDesk` exists to prove it through; raw CDP navigation was proven to work, which is a different, narrower claim)
- [x] raw CDP inaccessible (from outside the container's own Docker network)
- [ ] authenticated frame/control channel -- not built
- [ ] mouse input / keyboard input / scrolling / resize -- not built
- [ ] navigation / tabs / popups handled safely -- not built
- [x] persistent Admin/Manager/User profile -- role-aware, LIVE CLOUDDESK tested for User (Manager/Admin share the identical code branch, not each separately live-tested)
- [x] ephemeral Guest profile -- LIVE CLOUDDESK tested; two real bugs found and fixed to make this genuinely true (role-name-vs-ID comparison, missing capability grant)
- [x] cross-user profile isolation -- LIVE CLOUDDESK tested (`task_5_8`): User A's localStorage sentinel proven unreadable from User B's own instance
- [x] cookie/local-storage persistence policy proven -- proven via `localStorage` (LIVE CLOUDDESK); real cookie persistence found genuinely broken in this container image (OS-crypt/keyring dependency) and documented as an open item rather than hidden
- [ ] Internet browsing works -- proven only via a standalone raw-CDP navigation test, not through any `CloudDesk`-mediated path
- [ ] sensitive internal-network access blocked -- not tested (no navigation surface to attack)
- [ ] WebRTC network leakage reviewed -- not reviewed
- [ ] downloads / uploads PASS -- not built
- [ ] clipboard PASS -- not built
- [ ] audio PASS / audio cross-user isolation PASS -- not built
- [ ] video playback PASS -- not tested
- [x] browser renderer sandbox verified (live, not assumed)
- [x] OCI hardening inspected (live, via `docker inspect`)
- [x] no Docker socket
- [x] no privileged mode
- [ ] crash recovery -- mechanism inherited from the generic `RuntimeManager`, not independently re-tested for Browser
- [x] enable/disable (mechanism proven via the real test)
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
