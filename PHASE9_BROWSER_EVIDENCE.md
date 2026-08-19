# Phase 9 — Brave Browser Runtime: Executable Evidence Matrix

**Phase 9 status: PARTIAL.** This is a single foundation pass, not a
full implementation. Real, working, integrated evidence exists for the
runtime-adapter layer (Tasks 1-3) only. The browser broker, frame
streaming, input handling, tab management, role-aware profile policy,
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
| 4 | Profile policy (role-aware persistence) | IMPLEMENTATION MISSING | — | `default_persistence(RuntimeKind::Browser)` in `lib.rs` still returns `Persistence::Ephemeral` unconditionally for every Browser instance regardless of the owning user's role — Administrator/Manager/User need `Persistent`, only Guest should be `Ephemeral`. This requires threading the calling principal's role into the instance-creation path, not done this pass |
| 5 | Persistent profile evidence | NOT EXECUTED | — | Blocked on Task 4; the mechanism itself (per-instance `/state` mount surviving stop/start for a `Persistent` instance) is architecturally the same one every other runtime already relies on and is not independently in doubt, but was not separately re-proven for Browser specifically this pass |
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
| 63-64 | Resource policy / memory pressure | PARTIAL (real finding) | Real, live-measured finding this pass: Code/Office's shared default `pids_limit` (64) is **far too low** for a real Chromium-family browser — Brave's own zygote/GPU/renderer process tree hit the pids cgroup ceiling immediately (`pthread_create: Resource temporarily unavailable`) at that limit and never reached a working state. Raised to 512 in this pass's own test harness only | `ResourcePolicy` is currently one struct shared by every adapter a single `RuntimeManager` registers, not yet per-kind — the product's own `main.rs` still uses the shared default, meaning **the real, shipped Browser adapter would fail to start under the current production resource policy**. Wiring a real per-kind resource policy is a concrete, documented, high-priority follow-up, not a cosmetic gap |
| 65-66 | Tab limit / multi-user isolation | NOT EXECUTED | — | No tab management or multi-instance acceptance run this pass |
| 67 | Admin/Manager/User/Guest profile policy | IMPLEMENTATION MISSING | — | Same gap as Task 4 |
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
   Chromium-family browser's process tree outright. Raised to 512 in
   this pass's own test harness; **the production default in
   `main.rs` is unchanged and this is a known, documented, real gap**
   (see Task 63-64) that must be closed with a real per-kind resource
   policy before Browser can actually be enabled in a production
   deployment.

## Unresolved Critical/High

None found in the surface actually built and tested this pass (the
OCI adapter and its integration with `RuntimeManager`). This is not a
security clearance for the unbuilt surface (broker, network isolation,
authorization, CDP-takeover resistance, etc.) — those simply do not
exist yet to have defects in.

## Rust gates (this pass)

`cargo fmt --all -- --check`: PASS.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
`cargo test --workspace`: see `CLAUDE_ENGINEERING_CHECKPOINT.md` for
this pass's final run result.
`cargo build --workspace --release`: PASS.

Live evidence: `task_1_2_3_brave_runtime_reaches_real_running_state`
(`services/clouddeskd/tests/browser_runtime.rs`) run 3/3 clean,
~8-10s each. Zero leaked Brave containers verified after every run
(`docker ps -a --filter ancestor=clouddesk-brave:1.93.136` empty).

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
- [ ] persistent Admin/Manager/User profile -- not built (Task 4 gap)
- [ ] ephemeral Guest profile -- current behavior (Ephemeral for everyone) accidentally satisfies Guest's requirement but is wrong for the other three roles
- [ ] cross-user profile isolation -- not independently tested (no multi-user Browser acceptance run)
- [ ] cookie/local-storage persistence policy proven -- not tested
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
- [ ] resource limits -- a **real gap found**: the production default is insufficient (see Task 63-64); the test-only override is not a production fix
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
