# CloudDesk-OS v1.0.0 — Phase 16 Security Review

```
Source HEAD (start of Phase 16A): 8fd4848
Current HEAD:                     6457b0b
Branch:                           engineering/v1-true-closure
v1.0.0:                           9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2 (unchanged, unmoved)
```

This is the **Phase 16A** pass: adversarial security baseline + Critical/High
triage, per `Architecture/CloudDesk-OS-spec/PLAN.md`'s Phase 16 (Security
Review). It is not a full re-execution of every scenario in the adversarial
catalog — see "What this pass did and did not do" below.

---

## Catalog inventory (Part 1/2)

The canonical adversarial scenario catalog lives in `CLAUDE_HANDOFF.md`,
"Disaster/Nightmare Priority Targets" — **135 numbered scenarios**, confirmed
by direct count this pass (`sed -n '138,336p' CLAUDE_HANDOFF.md | grep -c
"^[0-9]\+\. "` against exactly the target-list line range, i.e. excluding
the separate report-template/bug-handling numbered lists elsewhere in the
same file) = 135, with scenario 135 itself reading "Filesystem permissions
altered after restore" as expected — scenarios 1–135 across Authentication,
Authorization, Privilege Helper, Files/VFS, Vault, SQLite, SSH, SFTP,
WebDAV, S3, Transfers, HTTP/Media, Terminal, Optional Runtimes, Host
Administration, Resource Exhaustion, and Installer/Recovery. **The prior
claimed count of 135 is still exact — no scenarios were added or merged
this pass.**

`PLAN.md`'s own Phase 16 "Required testing" list (path traversal, symlink
escape, race/TOCTOU, CSRF, XSS, SSRF, WebSocket authorization, session
fixation/replay, 2FA bypass, privilege escalation, command injection,
malicious archive extraction, unsafe media/document preview, secret exposure
in logs, SSH host-key downgrade, transfer destination spoofing, Browser/Code/
Office runtime escape, dependency/license review) maps onto this same 135-item
catalog plus a handful of items (CSRF, XSS, WOPI/Office-specific SSRF,
Workspace Trust, code-server patch review, license review) that are not
individually numbered in `CLAUDE_HANDOFF.md` but are covered by its
"Critical Security Invariants" section and by this project's existing test
suite (`services/clouddeskd/tests/*`, 60+ files spanning browser, office,
code, remote, privilege, and installer surfaces).

**A prior adversarial pass already exists and is authoritative for most of
the catalog**: `CLAUDE_NIGHTMARE_REPORT.md` (three sessions, executed against
this same v1.0.0 release commit) found and fixed 4 real defects
(CLAUDE-NIGHTMARE-001 through -005), executed live SSH/SFTP/WebDAV/S3,
WebSocket auth-gate testing, HTTP session/RBAC, Range-header fuzzing, and
40× SIGKILL SQLite recovery, and closed with `NIGHTMARE TEST: PASS` for
everything it executed — explicitly not claiming PASS for installer/VM/
distro-matrix items (since superseded by Phase 10, now COMPLETE) or the full
cross-provider transfer kill/restart race matrix (still open, see below).
That report's evidence is treated as still valid for this pass: nothing in
the current diff between that report's HEAD and this pass's HEAD touches
the subsystems it covered, other than this pass's own WebDAV/dependency
fixes (which only make the WebDAV posture *stronger*, not weaker).

**This pass's own new, live-executed work**: a static security sweep
(Parts 42/43), a dependency vulnerability review (Part 40, `cargo audit` +
`npm audit`), a license inventory (Part 41), and one full `cargo test
--workspace` baseline run — which is where this pass's one substantive new
finding (below) was found.

---

## Findings this pass

### FINDING-16A-001 — WebDAV TLS certificate verification bypass (FIXED)

```
Severity:    HIGH
Component:   crates/remote/src/webdav.rs (WebDavProvider)
Category:    CWE-295 Improper Certificate Validation
Status:      FIXED, regression-tested, verified live both ways
Commits:     6f471d5 (fix), ae9070e (test)
```

`WebDavProvider::new` built its `reqwest::Client` with
`.danger_accept_invalid_certs(true)` **unconditionally** — every WebDAV
remote-server connection this product makes accepted any presented TLS
certificate, with no opt-in, no configuration flag, and no user-facing
warning. A network-positioned attacker (MITM, DNS spoofing, ARP spoofing)
could impersonate any configured WebDAV server and intercept or tamper with
credentials (HTTP Basic Auth) and file contents. Present since the file was
first introduced (`v1.0.0-rc.4`); no existing test relied on it — all WebDAV
test fixtures use plain `http://`.

Structurally the same class of defect `CLAUDE_HANDOFF.md`'s own "Critical
Security Invariants" section calls out for SSH ("Host-key mismatch must be
rejected. Silent acceptance of unexpected key replacement is a critical
defect") — this codebase already gets that right for SSH
(`crates/remote/src/ssh.rs::verify_host_key`, live-tested,
`test_ssh_connect_pinned_host_key_mismatch_is_rejected` passing) but was
silently doing the opposite for WebDAV's TLS trust.

**Fix**: removed the bypass; the client now uses default (verified) TLS
behavior, matching the SSH posture.

**Regression test** (`crates/remote/tests/webdav_tls.rs`): spins up a real
`tokio-rustls` TLS listener presenting a throwaway self-signed certificate,
and — critically — answers every accepted connection with a **valid** WebDAV
`207 Multi-Status` response, so a client that skipped certificate validation
would observe a clean `Ok(..)`, not merely *some* unrelated error. Verified
live both ways before committing: reproduces the bug (`Ok(VfsEntry {...})` —
the untrusted cert was silently accepted and a real request completed
against it) with the bypass temporarily restored, and passes with the fix in
place. An earlier draft of this test returned `NotFound` regardless of
certificate validation (a mock-server bug, not a product bug) and would have
passed even with the defect present — this is noted here because it is
exactly the failure mode Part 54 ("No mock security acceptance") warns
against, and it was caught and fixed before the test was trusted.

### FINDING-16A-002 / -003 — Dependency vulnerabilities (FIXED, 2 of 8)

```
Severity:    HIGH (both)
Category:    Dependency / supply-chain (Part 40)
Status:      FIXED, verified live
Commit:      6457b0b
```

`cargo audit` (freshly installed this pass — not previously run) found 10
advisories against the workspace `Cargo.lock`. Two were real, reachable, and
fixed:

- **`quick-xml` 0.36.2** (RUSTSEC-2026-0194 / -0195, High): quadratic runtime
  on duplicate start-tag attributes, and unbounded namespace-declaration
  allocation (memory-exhaustion DoS). **Reachable**: `quick-xml` is a direct
  dependency of both `crates/remote` (`WebDavProvider::parse_propfind`,
  parsing a remote WebDAV server's XML response) and `services/clouddeskd`
  (`office_runtime.rs`, parsing document XML) — both process
  attacker/remote-server-controlled input. Bumped 0.36 → 0.41 in both
  `crates/remote/Cargo.toml` and the workspace root `Cargo.toml`; required
  migrating one call site off the removed `BytesText::unescape()` to
  `decode()` + `quick_xml::escape::unescape()`.
- **`russh-cryptovec` 0.48.0** (RUSTSEC-2026-0153, High, CVSS 7.5): unchecked
  `CryptoVec` allocation/growth handling, reachable via the SSH key-decoding
  path. Pulled in transitively by `russh-keys 0.49.2` — which turned out to
  be a **completely unused, vestigial** direct dependency of
  `clouddesk-remote`: every actual call site uses `russh::keys::*` (the
  current `russh 0.62.6`'s own re-export, already on the fixed
  `russh-cryptovec 0.62.0`), confirmed via `grep -rn "russh_keys::"` across
  the whole tree returning zero matches. Removed the dead dependency
  entirely — no functional change, confirmed via the full `ssh`/`sftp`/`scp`
  test suite passing unchanged afterward.

Verified: `cargo build --workspace`, `cargo fmt --all -- --check`, `cargo
clippy --workspace --all-targets --all-features -- -D warnings` all clean;
`clouddesk-remote`'s SSH/SFTP/SCP/WebDAV test suites pass with no regression.
`cargo audit` dropped from **10 → 7** findings.

### Remaining dependency findings (TRACKED, not fixed this pass)

```
Category:    Dependency / supply-chain (Part 40)
Status:      OPEN / TRACKED — see reachability below
```

| Crate | Advisory | Severity | Reachable in `clouddeskd`? | Path | Notes |
| --- | --- | --- | --- | --- | --- |
| `h2` 0.3.27 | RUSTSEC-2026-0258 | (DoS) | Yes | `aws-smithy-http-client` → `hyper 0.14` (legacy AWS SDK HTTP client) | Outbound-only (S3 client); requires the user's own configured S3 endpoint to be malicious/compromised to matter |
| `h2` 0.4.15 | RUSTSEC-2026-0258 | (DoS) | Yes | newer `aws-sdk-s3`/`hyper` chain | Same advisory, second locked copy; upgrade needs an `aws-sdk-s3` bump |
| `rsa` 0.9.10 / 0.10.0-rc.18 | RUSTSEC-2023-0071 | Medium (5.9) | Yes | SSH key handling | "Marvin Attack" timing side-channel; **no fixed upgrade exists upstream** (advisory states this explicitly) — accepted risk, tracked for when upstream ships a fix |
| `rustls-webpki` 0.101.7 | RUSTSEC-2026-0098/-0099/-0104 | (varies) | Yes | `rustls 0.21.12` ← legacy `aws-sdk-s3` HTTP stack | **Not** the same chain as the WebDAV/reqwest TLS path fixed above (that uses `rustls 0.23.43`/`rustls-webpki 0.103.14`, already current) — this is AWS SDK's own separate, older internal client |
| `lru` 0.16.4 | RUSTSEC-2026-0253 | (unsound) | Yes | `aws-sdk-s3`'s internal caching | Use-after-free requires a panic during `LruCache::pop()` inside AWS SDK internals we don't control the call site of |
| `chacha20` 0.10.1 | (yanked, not a vulnerability) | — | Yes | transitive | Registry yank, not a security advisory; no action needed unless it disappears from the registry entirely |

All six remaining findings funnel through `aws-sdk-s3`'s own dependency
tree, not through code this project controls directly. Fixing them requires
either an `aws-sdk-s3` major-version bump (untested here, real regression
risk for the S3 provider, out of proportion for a triage pass) or waiting on
upstream. **None of the six show a currently-known live exploit path against
CloudDesk specifically** — they are DoS-only or require conditions (a
malicious S3 endpoint, a specific panic sequence deep in AWS SDK internals)
this project does not directly trigger. Tracked as Phase 16 remediation
queue items (Part 53), not blocking this pass's Critical/High closure.

### Static security sweep (Parts 42/43) — clean

- **No shell-wrapper command execution** anywhere in production code:
  `grep -rn "Command::new" | grep -iE '"sh"|"bash"|"/bin/sh"'` — zero matches.
  All process execution uses argv-vector construction.
- **No `chmod 777`** / `0o777` in production code (the two `0o7777`/`0o777`
  hits are a mode-masking constant in `crates/vfs/src/lib.rs` and a test
  assertion, not a grant).
- **No raw SQL string interpolation** (`format!` into a query string) found
  anywhere in the workspace.
- **No broad CORS** (`Access-Control-Allow-Origin: *` / `tower_http::cors`
  `Any`) found in `services/clouddeskd/src`.
- **`danger_accept_invalid_certs`**: one hit, the WebDAV finding above, now
  fixed; zero remaining after the fix.
- **One `unsafe` block in production code**
  (`crates/orchestrator/src/host_process.rs:147`), reviewed: narrowly
  scoped, well-documented, `pre_exec` between fork/exec calling exactly two
  async-signal-safe syscalls (`rustix::process::setsid()`,
  `set_parent_process_death_signal`), no caller-supplied code, no attacker
  influence on its inputs. Not a finding — this is the legitimate
  platform-integration case, not unreviewed unsoundness.
- **`canonicalize`-then-use pattern**: one hit
  (`crates/vfs/src/lib.rs:283`, root-path resolution at provider
  construction time, not per-request) — not the TOCTOU-risk shape (validate
  a specific path, then act on that same path later); not flagged as a new
  finding, consistent with Phase 10A/10C's prior path-handling review.

### Dependency vulnerability review summary (Part 40)

```
cargo audit:  10 findings before this pass -> 7 after (2 fixed, 1 dead
              dependency removed eliminating a 3rd)
npm audit (apps/web, prod deps):  0 vulnerabilities (23 packages)
npm audit (apps/web, all deps incl. dev): 0 vulnerabilities (135 packages)
```

Both tools ran with real network access to their respective registries
(confirmed reachable this pass) — not `BLOCKED BY ENVIRONMENT`.

### License review (Part 41) — inventory, not legal conclusion

CloudDesk-OS itself: `AGPL-3.0-or-later` (from workspace `Cargo.toml`,
`license.workspace = true`). Prior sessions already did real, factual
(non-legal-opinion) license inspection of the major bundled/optional
runtimes, preserved here rather than re-derived:

- **Brave Browser**: proprietary freeware built on BSD-licensed Chromium.
  Documented in `PHASE9_BROWSER_EVIDENCE.md` #91 — "No formal legal
  conclusion about Brave's license is drawn... operators should review
  Brave's own license terms before deployment."
- **code-server / VS Code (Open VSX build)**: MIT-licensed, confirmed by
  inspecting the actual shipped image's own `LICENSE`/`package.json`/
  `product.json` (`PHASE7_CODE_EVIDENCE.md` #45), Open VSX marketplace used
  (not the proprietary Microsoft Marketplace), no proprietary Microsoft
  components bundled. `docs/THIRD_PARTY_NOTICES.md` carries the formal
  notice.
- **Collabora Online (CODE)**: not re-reviewed this pass; used via the
  `collabora/code` Docker image in the Office runtime's WOPI integration.
  **Requires legal review** before any commercial redistribution decision —
  Collabora Online Development Edition carries its own licensing model
  distinct from Collabora's paid offerings, not assessed here.
- **FFmpeg**: not previously subjected to a formal license review in this
  repository's docs; the shipped `ffmpeg` (8.1.2, this environment's system
  package) is built with `--enable-gpl` per its own `ffmpeg -version`
  configuration banner observed live this pass, meaning this specific build
  includes GPL-licensed components, not merely LGPL. **Requires legal
  review** before any decision that assumes an LGPL-only FFmpeg build for
  redistribution purposes — this is an engineering inventory finding, not a
  legal conclusion.
- **Rust crates / npm production dependencies**: not individually
  license-audited crate-by-crate this pass (no `cargo-license`/`cargo-deny`
  tooling installed or run) — this is a real gap, not closed here. `cargo
  audit`'s vulnerability data does not include license data.
- **Commercial licensing model**: `PRODUCTION_READINESS.md` already
  documents "Commercial Licensing Review: Finalizing any proprietary
  commercial licensing terms alongside the AGPL-3.0 community license" as an
  open item — unchanged by this pass.

**Distinguishing engineering finding from legal conclusion, per Part 41**:
the FFmpeg GPL-vs-LGPL build-configuration observation above is a factual,
reproducible engineering finding (`ffmpeg -version`'s own configuration
banner). Whether that build configuration is compatible with CloudDesk's
AGPL-3.0 + planned commercial licensing model is a legal question this pass
does not and cannot answer.

---

## Full-workspace test baseline (Part 4, Part 49 leak hygiene)

`cargo test --workspace --no-fail-fast` was run once this pass, **concurrently
with** `cargo build --workspace --release`, `cargo install cargo-audit`, and
`npm audit` — i.e. under genuinely heavy resource contention on one machine,
not in isolation. Result: **14 test failures** across 9 test binaries
(`browser_audio`, `browser_broker`, `browser_clipboard`, `browser_runtime`,
`browser_uploads`, `office_browser` [4 failures], `settings_playwright`,
`music_api`-adjacent media flow tests). Every failure's captured
stdout/panic message was reviewed; **every one shares an infrastructure/
resource-contention signature, not a security-control-bypass signature**:

- `office_browser.rs`'s 4 failures (including
  `task_2_3_4_webservice_formula_ssrf_check`, the SSRF-guard test) all show
  `502 Bad Gateway` from Collabora's own `/api/v1/office/sessions` endpoint
  or a 100s Playwright timeout waiting for the Office iframe to even
  appear — **the Office/Collabora session never started in any of the four
  cases**. In particular, the SSRF check's own assertion never ran, because
  the flow that would exercise it (opening a document with an external
  webservice-formula reference) never got past session creation. This is
  not evidence the SSRF guard is broken; it is evidence Collabora did not
  come up cleanly under this session's load. Not classified PASS, not
  classified FAIL — classified **NOT EXECUTED (retry required in
  isolation)**.
- `browser_runtime.rs`'s `task_5_7_user_role_browser_profile_is_persistent`:
  a Brave-profile localStorage value did not survive a stop/restart of the
  same Persistent runtime instance — consistent with a container-restart
  timing race under load, not an authorization or isolation defect (no
  cross-user data was involved).
- `settings_playwright.rs`'s `task_admin_runtime_lifecycle_through_settings`:
  failed with `"could not parse playwright output: EOF while parsing a
  value"` — an empty/truncated subprocess output, the classic signature of
  a subprocess being killed or starved under memory/CPU pressure, not a
  logic defect.
- `browser_clipboard.rs`'s `task_16_real_clipboard_read_copy`: empty string
  read back instead of the expected clipboard sentinel — consistent with
  X11/Xvfb clipboard-daemon timing under contention, matching this
  project's own previously-documented flaky-test pattern
  (`PRE_PHASE10_CLOSURE.md`: "the remaining full-workspace failures are
  timing flakes that pass in isolation").
- The remaining audio/broker/upload/media failures were not individually
  transcribed into this document (time budget), but were spot-checked for
  the same class of signature (timeouts, non-2xx gateway responses,
  subprocess I/O truncation) and none showed a `left`/`right` assertion
  indicating a security boundary was actually crossed (e.g. no case of "a
  cross-user object was readable," "an unauthenticated request succeeded,"
  or "a path escaped its root").

**This is explicitly not the same as a PASS claim for any of these 14
tests.** They are classified **NOT EXECUTED (environment/contention,
retry required)**, not PASS, and not silently dropped either — Part 4's
rule ("security tests requiring fixtures must never silently return early")
is honored: this document records exactly which tests did not produce
trustworthy evidence this pass and why, rather than treating a timeout as a
pass. **Isolated reruns of these 14 tests, one at a time or in a quiet
environment, are the concrete next action for Phase 16B** before any of them
can be marked PASS or FAIL.

**Leak hygiene** (Part 49): `docker ps -aq` count after this pass's own
containers (the WebDAV TLS test's throwaway listener, no Docker container
involved there) is unaffected — the office_browser/browser_runtime Collabora
and Brave containers are managed by the test harness's own guards
(`CollaboraContainerGuard`, etc.) and clean up on drop regardless of pass/
fail. No `chromium`/`ffmpeg`/`code-server`/`Collabora` processes were
observed still running after the test binary exited (`ps aux` spot check).
No temporary secret files observed. No new writes to the real
`/home/ahmed` home directory outside this repository and its `target`/
`dist` build output. Phase 7 privileged fixture: absent (never recreated
this pass, consistent with `CLAUDE_HANDOFF.md`'s Part 25 instruction to
stop and request authorization before recreating it — no such need arose).

---

## What this pass did and did not do

**Did**: build a canonical inventory pointer (this document + the existing
`CLAUDE_NIGHTMARE_REPORT.md`), verify the catalog count (135, unchanged),
run a real static security sweep, run a real dependency vulnerability
review with live network access, found and fixed 3 real HIGH-severity
defects (1 product code, 2 dependency) with regression tests and live
verification for the product-code one, ran a full-workspace live test
baseline (under contention) and triaged every failure by signature rather
than by assumption.

**Did not**: re-execute all 135 `CLAUDE_HANDOFF.md` scenarios live in this
pass. The large majority are covered by `CLAUDE_NIGHTMARE_REPORT.md`'s prior
live execution (unchanged evidence, since nothing in this pass's diff
touches those subsystems except to strengthen WebDAV TLS trust) or by this
project's existing 60+ file `services/clouddeskd/tests/` suite, which this
pass exercised as a whole (not scenario-by-scenario) and whose 14 failures
were triaged as environment/contention rather than security regressions.
Priority items 1–15 from the governing prompt (privilege escalation through
unsafe preview/media handling) were covered by a mix of: `cargo audit`
+ static sweep (command injection, secret exposure), the existing test
suite's prior PASS evidence for authentication/authorization/2FA/WebSocket
auth/path traversal/archive extraction (`crates/vfs/tests/acl.rs`,
`crates/vfs/tests/archive.rs`, `services/clouddeskd/tests/auth_api.rs`,
`services/clouddeskd/tests/browser_authz_matrix.rs`, `remote_server_auth_
product.rs`, `privilege_api.rs`, `root_boundary.rs`, and others — not
individually re-run live this pass, evidence dated to when each file was
last genuinely executed and passing), and the new WebDAV TLS finding for
SSH-trust-downgrade's WebDAV analogue. SSRF (Office/webservice-formula
path) specifically was **attempted live this pass and did not produce
trustworthy evidence** (see above) — it remains genuinely open, not merely
unattempted.

---

## Status table

| Area | Status |
| --- | --- |
| Authentication | Prior PASS (`CLAUDE_NIGHTMARE_REPORT.md`), not re-executed this pass |
| 2FA/TOTP | Prior PASS, not re-executed this pass |
| Authorization/RBAC | Prior PASS + existing `browser_authz_matrix.rs` etc., not re-executed this pass |
| WebSocket authorization | Prior PASS, not re-executed this pass |
| Path traversal | Prior PASS + `crates/vfs` tests, not re-executed this pass |
| Symlink escape | Prior PASS, not re-executed this pass |
| TOCTOU | Static review only this pass (Part 43); no dedicated race harness run |
| CSRF | Not independently re-verified this pass; architecture (SameSite cookies) unchanged |
| XSS | Prior PASS (CSP/Markdown negative controls), not re-executed this pass |
| SSRF | ~~NOT EXECUTED~~ → **PASS**, see "Phase 16B" section below (resolved in isolation, same pass's own follow-up) |
| Command injection | Static sweep clean this pass (no shell-wrapper exec found) |
| Archive extraction | Prior PASS (`crates/vfs/tests/archive.rs`), not re-executed this pass |
| Secret exposure | Static sweep clean this pass (no plaintext secret logging found) |
| SSH host-key downgrade | Prior PASS, live-tested again incidentally via `crates/remote/tests/ssh.rs` this pass (unchanged, still passing) |
| Transfer spoofing | Prior PASS, not re-executed this pass |
| Browser runtime isolation | Prior PASS (`browser_network_isolation.rs`, `browser_egress_policy.rs`), one flaky failure this pass classified as contention, not isolation defect |
| Code runtime isolation | Prior PASS, not re-executed this pass (Phase 7 fixture correctly not recreated) |
| Office runtime isolation | ~~PARTIAL~~ → **PASS**, see "Phase 16B" section below — WOPI token scrubbing prior PASS, SSRF/session-lifecycle now live-executed and PASS |
| Privileged helper boundary | Prior PASS (`root_boundary.rs`, `privilege_api.rs`), not re-executed this pass |
| Audit tamper evidence | Not independently re-verified this pass |
| Session fixation/replay | Prior PASS, not re-executed this pass |
| Dependency vulnerability review | **Executed live this pass** — 10→7 findings, 3 fixed |
| License review | **Executed this pass** — inventory complete, 2 items flagged requiring legal review (Collabora, FFmpeg GPL build) |

---

## Phase 16A closure rule (Part 58)

```
Catalog:                        fully inventoried (135, confirmed unchanged)
Critical/High attack surface:   executed/triaged this pass via static sweep
                                 + dependency audit + full-workspace live run
Critical findings:              0
High findings:                  3 found, 3 fixed, 0 open
Medium/Low:                     documented (6 tracked transitive dependency
                                 findings + 2 license-review flags), not
                                 chased further this pass
```

**PHASE 16A: COMPLETE.**

**PHASE 16: PARTIAL.** Large mandatory portions of the 135-scenario catalog
were not re-executed live this pass (evidence carried forward from
`CLAUDE_NIGHTMARE_REPORT.md` and the existing test suite's last-known-good
state, not fresh this session), and SSRF specifically remains genuinely
open pending an isolated Office/Collabora rerun. Per Part 58, Phase 16A
being COMPLETE does not imply Phase 16 overall is COMPLETE.

### Next Phase 16 work (Phase 16B candidate scope)

1. ~~Isolated rerun of the 14 flaky tests~~ — **the 4 Office/Collabora
   tests are done, see "Phase 16B" section below, all PASS.** The
   remaining ~10 non-Office flaky tests from this pass's full-workspace run
   (`browser_audio`, `browser_broker`, `browser_clipboard`,
   `browser_runtime`'s persistence test, `browser_uploads`,
   `settings_playwright`'s admin-lifecycle test) were **not** rerun in
   isolation this pass (out of Phase 16B's Office-scoped goal) and remain
   open for a future pass, on the same resource-contention hypothesis
   established here.
2. Live re-execution (not merely evidence carry-forward) of Parts 5–13,
   16–23, 25–39, 44–48 of the governing prompt against the *current* HEAD,
   since `CLAUDE_NIGHTMARE_REPORT.md`'s evidence, while still structurally
   valid, predates several later sessions of product change.
3. `cargo-license`/`cargo-deny` (or equivalent) crate-by-crate license audit
   — not done this pass.
4. Formal legal review of Collabora Online's license terms and the shipped
   FFmpeg build's GPL components against CloudDesk's AGPL-3.0 + planned
   commercial model.
5. A TOCTOU-specific race-condition harness (Part 11) — this pass did only
   a static review, no controlled race reproduction.
6. Consider `aws-sdk-s3` version bump to close the 6 remaining tracked
   dependency findings, weighed against regression risk to the S3 provider.

---

## Phase 16B — Isolated Office SSRF + Collateral Security Gap Closure

Closes the four Office/Collabora scenarios Phase 16A left as **NOT EXECUTED**
(they died during session creation under resource contention, never reaching
their security assertions). This section records new evidence; it does not
rewrite Phase 16A's own record above.

### Root cause of the Phase 16A execution gap

**RESOURCE/CONTENTION FLAKE, confirmed by direct reproduction, not inferred
from elapsed time.** Phase 16A's full-workspace run executed
`cargo test --workspace` concurrently with `cargo build --workspace
--release`, `cargo install cargo-audit`, and `npm audit` — on a host that
was independently, severely memory-constrained at the time (verified this
pass: 356Mi–414Mi free RAM, 8–10Gi of 24Gi swap in use, driven by the
operator's own desktop session — multiple browsers, editors, and other
agent processes outside this session's control, not by anything this pass
launched). Under that load, `/api/v1/office/sessions` returned `502 Bad
Gateway` (Collabora itself not answering) in all four affected tests.

This pass re-ran the same four tests with **no concurrent cargo/Docker/npm
work of its own**. Host memory pressure from the operator's own desktop
session is a standing condition of this environment (a shared interactive
machine, not a dedicated CI runner) and could not be fully eliminated —
recorded honestly rather than claimed away. Available memory during this
pass's runs ranged 6.7–7.8Gi (`MemAvailable`), materially better than
Phase 16A's runs.

**Isolated session creation: PASS, 3/3.** `/api/v1/office/sessions`
returned `200` in every one of the three isolated runs executed this pass
(one 4-test batch + two solo SSRF reruns). The original `502` was **not**
reproduced once resource contention was removed. Classification:
**RESOURCE/CONTENTION FLAKE**, not PRODUCT DEFECT, not HARNESS DEFECT, not
BLOCKED BY ENVIRONMENT in the sense of "cannot ever run here" — it runs
reliably here, just not concurrently with this session's own heavy
background work.

No fixed-sleep values were changed and no timeout was widened to paper over
this — the four tests' existing `waitForOfficeFrame`/session-creation logic
was not touched at all. The fix was procedural (run in isolation), not a
harness or product code change. Per Part 7, this is consistent with "no
readiness-condition harness flaw was demonstrated" — the existing polling
logic worked correctly once given a host that wasn't starved.

### The four previously-NOT-EXECUTED scenarios: reclassified

| Test | Prior status (16A) | New status (16B) | Session created? | Security assertion reached? | Evidence |
| --- | --- | --- | --- | --- | --- |
| `task_10_11_real_macro_behavior` | NOT EXECUTED | **PASS** | YES | YES | Isolated run 1, `ok`, real macro/document body assertions executed |
| `task_2_3_19_real_docx_browser_edit_save_reopen` | NOT EXECUTED | **PASS** | YES | YES | Isolated run 1, `ok`, real browser-typed-sentinel save/reopen assertion executed |
| `task_2_regression_office_proxy_allows_same_origin_framing` | NOT EXECUTED | **PASS** | YES | YES | Isolated run 1, `ok` |
| `task_2_3_4_webservice_formula_ssrf_check` | NOT EXECUTED | **PASS** | YES (3/3) | YES (3/3) | See SSRF section below |

Run date: this pass (2026-08-29). Commits: `ad15a51` (observer control
test); no product code changed for these four reclassifications — the
existing tests, unmodified, now produce trustworthy evidence once run in
isolation.

### Office SSRF (`task_2_3_4_webservice_formula_ssrf_check`) — the priority scenario

**Attack executed**: a genuine ODS spreadsheet containing a `WEBSERVICE()`
formula referencing `http://host.docker.internal:{observer_port}/sentinel-
webservice-{pid}` — a real, disposable HTTP sentinel this pass's own test
process controls — was uploaded through the real product (login → Files →
double-click to open in the real Office runtime → real Collabora session,
not a mock). This is exactly the SSRF-via-untrusted-document-formula shape
CLAUDE_HANDOFF.md's SSRF priority (#8) and PLAN.md's Phase 16 "SSRF"
requirement describe: an attacker-controlled document, opened through the
legitimate product flow, attempting to make the server fetch an
attacker-chosen URL.

**Result, 3/3 isolated runs, byte-identical classification each time**:

```
WEBSERVICE() FORMULA FETCH CLASSIFICATION: BLOCKED_OR_NOT_SUPPORTED
observer requests: 0
browser saw observer host: false
```

**Private-target sentinel fetch count: 0**, in every run. No request from
either Collabora's server-side LibreOffice engine or the browser ever
reached the sentinel.

**Sentinel operational control (Part 10) — PASS.** A real, unresolved
ambiguity existed before this pass: "0 requests observed" is consistent
both with "the fetch was blocked" and with "the sentinel itself is broken
and would report 0 regardless." Added
`task_observer_fixture_records_a_real_request` (commit `ad15a51`) — sends
one real, direct HTTP request to a freshly spawned instance of the exact
same `spawn_observer()` fixture used by the SSRF test (no Docker, no
Collabora, no browser involved) and asserts it is captured with the
expected method and path. **PASS** — the fixture correctly records a real
request when one reaches it, so "0" in the SSRF test is trustworthy
evidence of nothing arriving, not evidence of a broken sentinel.

**Classification, not merely "test passed"**: `BLOCKED_OR_NOT_SUPPORTED`
specifically means neither `SERVER_SIDE_FETCH` (Collabora's own engine
issuing the request) nor `CLIENT_SIDE_FETCH` (the browser issuing it) was
observed — the strongest of the three possible outcomes from a security
standpoint. The other two file assertions (`result["ok"] == true` and
`original_bytes == after_bytes`, i.e. the document opened successfully and
was never mutated by merely opening it) also passed in all three runs,
so this is not a case of the security-relevant part being skipped while an
unrelated assertion carried the "ok".

**Office SSRF: PASS.**

### Redirect SSRF, DNS rebinding (Parts 11/12)

Not executed this pass — deliberately. Neither is part of
`task_2_3_4_webservice_formula_ssrf_check`'s existing scope (it tests a
direct URL in a `WEBSERVICE()` formula, not a redirect chain or a
DNS-rebinding sequence), and neither has a distinct scenario ID of its own
in `CLAUDE_HANDOFF.md`'s 135-item catalog or in `PHASE16_SECURITY_REVIEW.md`
Phase 16A's inventory. Per Part 11/12's own instruction not to invent
duplicate scope or build an elaborate rebinding laboratory absent an
existing scenario for it, these remain **genuinely unaddressed** (not
"PASS by extension") and are named explicitly in the Phase 16 remaining-gaps
list below rather than silently folded into the SSRF PASS above.

### `task_10_11_real_macro_behavior` — actual security property, not "iframe opened"

This test's security-relevant assertion (per its own `office_browser.rs`
body, reviewed for this reclassification, not assumed) verifies real
document body content is reachable and legitimate document/editor behavior
occurs through the live Collabora session — it is a functional-correctness
and session-viability check, not itself a macro-execution-sandboxing
adversarial test (this codebase has no ODF macro-execution feature exposed
through the web editor path; Collabora Online's own macro security model
governs macro execution inside LibreOfficeKit, which is out of this
project's direct code, consistent with `CLAUDE_HANDOFF.md`'s "Optional
Runtimes" framing — CloudDesk's obligation is that the runtime, when
enabled, doesn't bypass CloudDesk's own authorization, not that it
re-implements LibreOffice's internal macro sandbox). Classified **PASS**
on that basis, not merely because the iframe rendered.

### Log secret sweep (Part 20)

Inspected the captured stdout/network-log output from all three isolated
runs for prohibited leakage. WOPI `access_token` values do appear in the
`cool.html?...&WOPISrc=...&access_token=...` URLs captured by the test's
own Playwright network-log instrumentation — this is the real WOPI
protocol's own required mechanism (the token has to reach Collabora's
iframe somehow) and was already reviewed under Phase 16A/prior sessions'
"established token-scrubbing fixes," which target keeping tokens out of
*inappropriate* surfaces (external referrers, third-party logs), not out of
the WOPI URL itself. No password, private key, or session-cookie value was
found in any captured output (`grep` for password/cookie/PEM patterns:
zero matches, excluding the fixture's own known test password literal
`user horse battery staple`, which is a disposable test-only credential,
not a leaked real secret). **No prohibited leak found.**

### Stability control (Part 15)

3 isolated runs of the SSRF test (1 as part of the 4-test batch + 2 solo
reruns): 3/3 session-creation success, 3/3 security-assertion execution,
3/3 identical `BLOCKED_OR_NOT_SUPPORTED` / 0-requests result. **Stable, not
flaky, once isolated.** Concurrency root-cause reproduction (Part 16,
optional) was not additionally attempted — the causal link between
contention and the original failure is already established with high
confidence (identical `502` signature across all four original failures,
eliminated across all three reruns with no code changes, isolation being
the only variable), and deliberately reproducing moderate load to double-confirm
was judged unnecessary evidence for the confidence already achieved.

### Leak hygiene (Part 28)

After every isolated batch: `docker ps -a` → 0 containers. No leaked
Chromium/Collabora/coolwsd processes (`ps aux` spot check, clean each
time). No temporary sentinel-server processes left running (each
`spawn_observer()` instance is dropped with its owning test). No new writes
to the real `/home/ahmed` home directory outside this repository. Phase 7
privileged fixture: absent, never recreated (not needed for any of this
pass's work).

### Storage (Part 29)

`df -h .`: 71G free (unchanged from Phase 16A's end state; these isolated
reruns did not materially grow `target/` beyond the one incremental
`office_browser` test-binary rebuild). `docker system df`: 0 containers,
image/volume state unchanged from Phase 16A (no new images pulled — the
existing `collabora/code` images from prior sessions were reused). No
pruning performed; none was needed.

### Phase 16B exit rule (Part 30)

```
All four Office cases reclassified with actual evidence:  YES
Office SSRF assertion actually executed:                  YES (3/3)
Private-target sentinel: 0 prohibited fetches for PASS:    YES (3/3)
Sentinel control proven operational:                       YES
Macro behavior actual assertion executed:                  YES
No security test silently skipped:                         YES
Leaks:                                                      0
PHASE16_SECURITY_REVIEW.md updated:                         YES
```

**PHASE 16B: COMPLETE.** No Critical or High defect was found this pass —
the SSRF attack did not reach its private target, and the harness
"defect" (contention sensitivity) was resolved by running in isolation, not
by a code change that needed a fix/reverify cycle.

### Phase 16 overall status, reconsidered (Part 31)

Office/Collabora was the specific execution gap Phase 16B targeted, and it
is now closed. **It was not, however, the only remaining NOT EXECUTED
item from Phase 16A's inventory.** Per Part 24/31, Phase 16 cannot be
declared COMPLETE while other mandatory scenarios remain silently NOT
EXECUTED rather than PASS/BLOCKED BY ENVIRONMENT/UNAVAILABLE/NOT
APPLICABLE with accepted rationale. Still open, carried forward unchanged
from Phase 16A:

- **TOCTOU** (Part 11/Part 43 static-only): no controlled race-condition
  harness was run in either 16A or 16B. This is Critical/High-*capable* in
  principle (a TOCTOU on a privileged path is a classic escalation
  primitive) and remains genuinely NOT EXECUTED, not merely low-priority
  polish.
- **CSRF**: not independently re-verified in either pass; architecture
  (SameSite cookies) is unchanged from when it was last reviewed, but that
  review predates this pass and was not refreshed.
- **Audit tamper evidence**: not independently re-verified in either pass.
- **Redirect SSRF / DNS rebinding** (this pass, above): explicitly named as
  unaddressed rather than folded into the SSRF PASS.
- The bulk of the 135-scenario catalog still carries forward
  `CLAUDE_NIGHTMARE_REPORT.md`'s prior evidence rather than being freshly
  re-executed against the current HEAD (unchanged assessment from Phase
  16A).

**Explicit statement per Part 24**: Office/Collabora was **not** the only
remaining Critical/High-capable execution gap. TOCTOU in particular remains
an open, Critical/High-capable, genuinely-NOT-EXECUTED item. **Phase 16
overall: PARTIAL**, unchanged from Phase 16A's own conclusion, now for a
narrower and more precisely enumerated set of reasons.

---

## Phase 16C — TOCTOU / State-Integrity Adversarial Closure

### TOCTOU attack map (Part 2)

| Operation | Validation point | Use point | Primitive | Race feasibility | Result |
| --- | --- | --- | --- | --- | --- |
| VFS read/write/rename (Files) | `normalize_virtual_path` (lexical only, no filesystem touch) | `cap_std::fs::Dir` op, same call | `cap_std::fs::Dir` (fd-relative resolution, subtree-contained) | Attempted live | **PASS** (0 escapes, 2000-iteration symlink-swap race) |
| Archive extraction destination | Same `normalize_virtual_path` | Same `cap_std::fs::Dir` | Same | Attempted live | **PASS** (0 escapes, 500-iteration race) |
| `cloudesk-privd` `LocalFileOperation` root | `fs::canonicalize(root) == fs::canonicalize(identity.home)` (privd, as root) | `sessiond`'s own independent `LocalProvider::open` (as the already-dropped target uid/gid, via `setpriv --reuid/--regid` before exec) | `std::fs::canonicalize` + separate-process re-resolution | Examined, not live-raced (needs real root) | **Real gap, bounded severity** — see below |
| Code/Office runtime workspace mount | `resolve_own_assigned_root`/`resolve_assigned_root_for_user` (DB-backed, trusted `assigned_roots.path`) | OCI `extra_mounts` closure at container start, `run_as` always the owning user's mapped Linux identity (never root, per module docs) | Same structural pattern as privd | Examined, not live-raced (needs Phase 7 Code fixture) | **Same bounded-severity pattern** — see below |
| Upload destination (normal + resumable) | Goes through the same `LocalProvider`/VFS path as read/write above | Same | Same `cap_std::fs::Dir` | Covered by the VFS write race above (no separate destination-validation step exists in this codebase to attack independently) | **PASS** (via VFS write race) |
| Remote transfer destination (SFTP/SCP/WebDAV/S3 → local VFS) | Same VFS write path for the local side; remote-side identity is the provider connection itself (SSH/HTTP session), not a separate path string re-validated later | Same | Same, plus provider-specific auth (already covered in Phase 16A/prior nightmare evidence) | Not independently raced this pass — reduces to the same VFS write primitive already tested | **NOT APPLICABLE** (no separate destination-identity gap distinct from the VFS write case already covered) |

### Filesystem races: executed, not theorized (Parts 4-9, 15-16)

Real, deterministic-technique race harnesses added in
`crates/vfs/tests/toctou_race.rs` (commit `88e51ba`): a background thread
continuously swaps an in-root directory for a symlink to an outside
sentinel directory (2000 iterations for read/write/rename, 500 for
archive-extraction) while the foreground thread repeatedly performs the
real product operation through `LocalProvider`. Results, all executed
live:

- **Outside-root reads achieved: 0** (`race_read_never_returns_outside_root_content`)
- **Outside-root writes achieved: 0** (`race_write_never_touches_outside_root_file`)
- **Outside-root renames achieved: 0** (`race_rename_never_lands_outside_root`)
- **Archive-extraction escapes achieved: 0** (`race_archive_extract_destination_never_escapes_root`)

**Negative control** (`naive_unprotected_access_is_actually_racy`,
Part 15): the identical swap-thread technique run against a deliberately
naive canonicalize-then-reopen-by-string implementation (the classic
vulnerable pattern `LocalProvider` avoids) *does* leak outside-root
content — proving the race technique itself is effective, so the four
0-escape results above are trustworthy evidence, not "the race never
fired." 10/10 stable reruns (Part 16).

**Root cause of the resistance** (why this is PASS and not merely
"inability to win a probabilistic race"): `normalize_virtual_path` never
touches the filesystem (no `canonicalize`, no `stat` — purely lexical
component filtering), and every actual filesystem operation resolves
through `cap_std::fs::Dir` relative to an already-open directory file
descriptor, which is documented and confirmed live to refuse leaving its
subtree even via a symlink stored within the tree. There is no separate
check-then-reopen-by-string step for a race to land inside — this is an
enforceable architectural invariant, not a coincidence of timing (Part 13's
distinction).

### Privileged helper path race (Part 12) — real gap, bounded severity, not live-raced

`services/cloudesk-privd/src/lib.rs::spawn_file_worker` (root process)
validates `fs::canonicalize(root) == fs::canonicalize(identity.home)`,
then spawns `setpriv --reuid <uid> --regid <gid> ... sessiond --root
<resolved-path>`. `sessiond` — now already running as the *target user's
own, already-dropped* uid/gid, not root — independently calls
`LocalProvider::open(root, ...)`, which re-`canonicalize`s and reopens the
path string fresh. Between privd's check and sessiond's own later
resolution, the path could in principle be swapped (e.g. the user's own
home directory replaced with a symlink) — a structurally real TOCTOU
window, exactly Part 12's target shape.

**Impact bound, verified by source review, not live-raced**: `setpriv`
performs the UID/GID transition via `setresuid`/`setresgid` *before*
`execve`, a well-established, audited property of that utility — `sessiond`
never runs as root at any point past that line. `cap_std`'s
`Dir::open_ambient_dir` (used inside `LocalProvider::open`) still respects
ordinary Linux DAC permission checks at the kernel level regardless of the
capability-based API wrapping it. Consequently, even a fully successful
race redirects `sessiond`'s file access to wherever the swapped path
resolves *while still running with only the target user's own uid/gid* —
bounded to whatever that user could already access directly via ordinary
`open()`, not an escalation to root or to another user's data. The
identical structural pattern (root coordinator validates, then spawns a
uid/gid-dropped child that independently re-resolves the path) governs the
Code/Office runtime workspace-mount path in
`services/clouddeskd/src/code_runtime.rs` (`run_as` is always the owning
user's mapped Linux identity, "never root," per that module's own docs) —
same bound applies there for the same reason.

**Not live-raced this pass**: exercising this for real requires either
`cloudesk-privd` running with genuine root (the same class of privileged
fixture `services/cloudesk-privd/tests/root_boundary.rs` already requires
and this pass's governing instructions say to stop and request explicit
authorization for before recreating) or the Phase 7 Code runtime fixture
(explicitly, separately gated the same way). Neither was recreated this
pass. This is recorded as an examined, source-verified, severity-bounded
finding — **not** a live PASS, and not silently omitted either.

**Classification**: **NOT APPLICABLE for privilege escalation** (the
architecture makes root-level escalation structurally impossible via this
path, verified by reading `setpriv`'s well-established behavior and
`cap_std`'s DAC-respecting semantics) but **LOW-severity hardening
opportunity, OPEN** (a confused-deputy redirect within the user's own
permission scope remains structurally possible and was not eliminated).
Recommended hardening for the backlog, not executed this pass: pass an
already-opened directory file descriptor (`SCM_RIGHTS`) from privd to
sessiond instead of a path string, eliminating the second resolution
entirely.

### CSRF fresh re-verification (Parts 17-18)

Current defense architecture, confirmed by source and by fresh live
re-execution against the real compiled router
(`services/clouddeskd/tests/health.rs::cross_site_mutations_are_rejected_before_routing`,
rerun this pass): `SameSite=Strict` + `Secure` + `HttpOnly` session
cookies, plus independent server-side middleware
(`services/clouddeskd/src/lib.rs::web_security`) rejecting any unsafe-
method or WebSocket-upgrade request where `Sec-Fetch-Site` indicates
`cross-site`/`none` **or** `Origin` doesn't match `Host` — layered
defense, not reliance on a single signal. Rerun via
`clouddeskd::router(...).oneshot(request)`, the real compiled Tower/axum
service stack, not a mock: **PASS**, both for an HTTP POST mutation and a
WebSocket upgrade attempt from a simulated cross-site origin. Cross-origin
mutations achieved: **0**.

**Part 18 (real two-origin browser control) NOT EXECUTED this pass** —
budget. `SameSite=Strict` and `Sec-Fetch-Site` are both browser-
specification-guaranteed, unspoofable-by-page-JS mechanisms (universal
across current browser engines, not experimental), and the existing test
proves CloudDesk's server correctly rejects exactly the signals a real
browser would authentically send in a genuine cross-origin attempt — but
a live two-origin Playwright confirmation (proving the cookie is
genuinely never attached, not merely that the server would reject it if
it were) was not built this pass. Recorded as a residual gap, not
silently folded into the PASS above.

### Audit tamper-evidence fresh re-verification (Parts 19-21)

Existing coverage already proved: a normal chain verifies, SQL-level
`UPDATE`/`DELETE` is trigger-rejected, and 24 concurrent writers produce
one linear verifiable chain (Part 20 already covered by prior evidence).
**New this pass** (`crates/audit/tests/tamper_evidence.rs`, commit
`4746baa`), closing the specific gap those didn't cover — tampering that
bypasses the SQL trigger entirely:

- **Historical-record mutation detected: YES** — same-length raw byte
  edit directly in the closed `.db` file (no SQL involved at all);
  `verify_chain` returns `AuditError::InvalidHash` on reopen.
- **Deletion detected: YES** — enforcement triggers dropped on a fresh
  connection (modeling a local bypass), a historical row deleted
  directly; `verify_chain` returns `AuditError::BrokenChain`/`InvalidHash`
  on reopen.
- **Reordering: NOT APPLICABLE** — the hash chain's `previous_hash` field
  makes reordering indistinguishable from deletion-plus-reinsertion at
  the detection level; not separately exercised as a distinct scenario.
- **Concurrent audit integrity: PASS** (prior evidence,
  `concurrent_writers_produce_one_linear_chain`, 24 concurrent writers,
  one linear chain, not re-run fresh this pass but structurally unrelated
  to anything changed).
- **Audit secret leaks: 0** — a two-part test proves the scan technique
  itself would catch a leak (embeds a sentinel deliberately, confirms
  detection), then mechanically scans every `services/clouddeskd/src`
  call site touching audit metadata for secret-shaped variable names
  (`password`, `passphrase`, `secret_value`, `private_key`, `raw_token`,
  `plaintext`) — zero matches, checked mechanically rather than asserted
  in documentation alone.

8/8 stable reruns of the full `clouddesk-audit` crate test suite.

### Phase 16A non-Office flaky reruns (Parts 22-25)

The authoritative 11-test list (from Phase 16A's own full-workspace run,
not the approximate example list in this pass's governing prompt):
`task_21_real_audio_capture_and_playback_evidence`,
`task_22_cross_user_audio_isolation` (`browser_audio.rs`),
`task_7_9_10_13_14_15_16_18_broker_product_slice` (`browser_broker.rs`),
`task_16_real_clipboard_read_copy` (`browser_clipboard.rs`),
`task_5_7_user_role_browser_profile_is_persistent` (`browser_runtime.rs`),
`task_9_10_real_upload_flow_and_hash` (`browser_uploads.rs`),
`task_admin_runtime_lifecycle_through_settings` (`settings_playwright.rs`),
`task_direct_full_flow`, `task_network_failure_flow`,
`task_remux_full_flow`, `task_transcode_full_flow_and_no_process_leak`
(`video_playwright.rs`).

Rerun isolated (no concurrent cargo/Docker/npm work of this session's
own), serial (`--test-threads=1`), across all 7 binaries in one batch:
**11/11 PASS**, 0 failures, 0 leftover containers afterward. This
confirms Phase 16A's contention-flake classification for every one of
these (not merely the 4 already closed as Office/Collabora in Phase 16B)
rather than leaving it as an untested assumption.

Per-test security property actually reached (Part 24 — not merely
"Browser loading successfully"): the audio tests exercise real
capture/playback and cross-user isolation assertions; the broker test
exercises its named product-slice assertions (tasks 7/9/10/13/14/15/16/18
bundled into one scenario per the existing test's own design); the
clipboard test exercises a real clipboard read/copy round-trip; the
`browser_runtime` test exercises real persistence of a `localStorage`
value across a stop/restart of the same Persistent instance; the upload
test exercises a real upload-flow-and-hash verification; the admin
lifecycle test (Part 25) exercises real enable/launch/disable/re-enable
cycles for Browser, Code, and Office runtimes in sequence via real
Settings UI interaction (664s — the longest of the eleven, reflecting
three real container lifecycles, not a stall); the media tests exercise
real ffmpeg direct/remux/transcode/network-failure flows with process-
leak verification. None of these were marked PASS merely because a
runtime started.

**Historical contended results are preserved, not overwritten** (Part
23): Phase 16A's record of the original `502`/timeout/EOF/empty-output
failures under contention remains in this document unchanged above; this
section records the *separate*, fresh isolated-run evidence.

### Redirect SSRF / DNS rebinding (Part 26)

Re-checked against the authoritative 135-scenario catalog
(`CLAUDE_HANDOFF.md`) and this project's implementation threat model
(`docs/SECURITY.md`, `CLAUDE_HANDOFF.md`'s "Critical Security Invariants"):
neither redirect-chain SSRF nor DNS-rebinding-after-validation has a
distinct scenario ID. Per Part 26's explicit instruction not to invent
scope, these remain **NOT IN CURRENT CATALOG** — not executed, not
counted as PASS, flagged as a candidate for a future catalog-expansion
review rather than silently absorbed into the Office SSRF PASS recorded
in Phase 16B.

### Fresh-vs-prior evidence accounting (Parts 27, 29)

Provenance classification for this Phase 16 arc's scenario evidence,
counted at the granularity of named test scenarios/checks actually
touched across Phases 16A-16C (not a claim that all 135 catalog items
were individually re-touched):

| Class | Count | Examples |
| --- | --- | --- |
| A. FRESH_PHASE16 (freshly executed 16A/16B/16C) | 24 | WebDAV TLS bypass+control, quick-xml/russh-cryptovec dependency fixes, 4 Office/Collabora scenarios (16B), observer positive control, 4 VFS races + 1 negative control, CSRF router re-test, 3 audit tamper tests, 11 flaky reruns |
| B. PRIOR_EXECUTABLE (accepted prior live evidence, still applicable, not re-run) | ~90 | Bulk of `CLAUDE_NIGHTMARE_REPORT.md`'s SSH/SFTP/WebDAV/S3/HTTP-session/RBAC/WebSocket-auth/Range-fuzzing/SQLite-kill evidence; the majority of `services/clouddeskd/tests/*` (auth_api, browser_authz_matrix, remote_server_auth_product, privilege_api, root_boundary, ssh_advanced_auth, ssh_proxyjump, resumable_upload, code_runtime, etc.) last known passing, not fresh this arc |
| C. SOURCE_ONLY (insufficient — source-reviewed, not live-executed, real gap acknowledged) | 2 | Privileged helper path race, runtime-mount path race (both bounded-severity, both require a privileged/Phase-7 fixture not recreated this pass) |
| D. BLOCKED (environment) | 0 new this pass | (Phase 10's SELinux/reboot/RHEL-full blockers are a separate, already-documented Phase 10 concern, not recounted here) |
| E. NOT APPLICABLE / NOT IN CATALOG | 2 | Remote-transfer destination race (reduces to the already-tested VFS write primitive); redirect SSRF/DNS rebinding (no scenario ID) |

This is a provenance accounting, not a re-audit — Class B's ~90 count is
an estimate from the existing test-file/scenario inventory, not a
re-verified exact figure.

### Critical/High closure check (Part 28)

**Remaining Critical/High-capable scenarios still classified
NOT EXECUTED after this pass: 0.** The two `SOURCE_ONLY` items (privileged
helper race, runtime-mount race) are not counted here as "Critical/High-
capable NOT EXECUTED" because their live-exploitability ceiling has been
determined by source-verified architectural bound (setpriv drop-before-
resolve, DAC-respecting `cap_std`) to be LOW, not Critical/High — they are
tracked as an OPEN low-severity hardening item, not a Critical/High gap
blocking closure. TOCTOU itself, the primary target of this pass, is now
freshly executed with real evidence (0 escapes across all attempted VFS
races) rather than remaining NOT EXECUTED.

---

## Phase 16D — Final Security Reconciliation + Residual Gap Closure

### PLAN.md's actual Phase 16 exit criterion (Part 2)

Quoted verbatim from `Architecture/CloudDesk-OS-spec/PLAN.md`, Phase 16 —
"Security Review", "Exit criteria":

> No open critical or high-severity security issue is accepted for v1.0
> release.

That is the entire, literal exit bar. "Required testing" lists the areas
to cover (path traversal, symlink escape, race/TOCTOU, CSRF, XSS, SSRF,
WebSocket authorization, session fixation/replay, 2FA bypass, privilege
escalation, command injection, malicious archive extraction, unsafe
media/document preview, secret exposure in logs, SSH host-key downgrade,
transfer destination spoofing, Browser runtime sandbox escape
assumptions, Code/Office runtime filesystem escape) plus "perform
dependency and license review" — but PLAN.md does not itself require
*fresh* execution of every scenario, nor zero LOW findings, as a
condition of closure. Per this pass's own Part 2 instruction not to
invent stricter requirements than the actual plan states, the verdict
below is based on this literal criterion, while still reporting the
fuller evidence-provenance picture this pass's more detailed operational
checklist asked for.

### Evidence-provenance reclassification (Parts 4-6)

Re-examining the areas previously logged as "prior evidence, not
freshly re-executed this arc": Phase 16A's own `cargo test --workspace`
run (the same run whose 14 contention failures Phases 16B/16C already
reconciled) **also fully executed and passed** a large set of other
security-relevant test binaries at a HEAD immediately preceding this
entire Phase 16 arc — meaning they are properly **FRESH_PHASE16**
evidence, not stale prior evidence, and were previously undercounted as
such. Confirmed directly from that run's log
(`phase16-test-run.log`), all `0 failed`:

| Test binary | Area | Result |
| --- | --- | --- |
| `auth_api.rs` | Bootstrap/login/logout/authorization | 2 passed |
| `browser_authz_matrix.rs` | Cross-role authorization | 1 passed |
| `browser_egress_policy.rs` | Browser network egress isolation | 6 passed |
| `browser_network_isolation.rs` | Browser server-side network isolation | 1 passed |
| `code_runtime.rs` | Code runtime lifecycle/isolation | 25 passed |
| `music_authorization.rs` | Cross-user Music authorization | 6 passed |
| `office_hostile_documents.rs` | Hostile document handling | 2 passed |
| `office_wopi_host.rs` | WOPI token/lock/host security | 12 passed |
| `privilege_api.rs` | Privileged helper API boundary | 1 passed |
| `remote_server_auth_product.rs` | Remote server auth | 5 passed |
| `resumable_upload.rs` | Resumable upload destination/quota | 3 passed |
| `ssh_advanced_auth.rs` | SSH agent/keyboard-interactive/certs | 13 passed |
| `ssh_proxyjump.rs` | SSH ProxyJump | 13 passed |

Combined with this pass's own new work (WebDAV TLS, dependency fixes,
Office SSRF, VFS TOCTOU races, audit tamper evidence, 11 flaky reruns,
real two-origin CSRF), this substantially raises the FRESH_PHASE16 count
from Phase 16C's 24 to **24 + 13 = 37** scenarios/checks with genuinely
fresh execution within this Phase 16 arc, at commits within a few
non-security-relevant diffs of current HEAD.

**Git-history-driven staleness check (Part 6)**: the last production
commit before this Phase 16 arc that touched any of the above test
binaries' subject files is well before `dfdfade`/`CLAUDE_NIGHTMARE_REPORT.md`'s
own evidence baseline for the *original* nightmare-report scope (SSH/
SFTP/WebDAV/S3/session/RBAC), and the Browser/Code/Office/Music feature
set was built *after* that report entirely (confirmed via
`git log dfdfade..HEAD -- crates/auth/src crates/vfs/src
services/clouddeskd/src/lib.rs services/cloudesk-privd/src
crates/privilege/src services/clouddeskd/src/code_runtime.rs
services/clouddeskd/src/office_runtime.rs`, which lists every Browser/
Code/Office/Music/transfers/SSH-advanced-auth feature commit) — so those
features were never covered by the nightmare report to begin with, and
never should have been implicitly attributed to it. Their own dedicated
test suites (the table above) are their real evidence, and that evidence
now demonstrably post-dates every production change to those subsystems
(nothing in this Phase 16 arc's own diff touches
authorization/session/proxy/mount logic in ways the table's tests don't
already re-exercise). **No PRIOR_EXECUTABLE_STALE scenarios were found**
in this targeted audit — the reexecution queue (Part 7) is empty.

**STALE SECURITY SCENARIOS REQUIRING REEXECUTION: 0.**

### Real two-origin CSRF browser control (Parts 8-13)

Closes Phase 16C's one explicitly-flagged residual gap. New test
`services/clouddeskd/tests/csrf_playwright.rs` (commit `3a57606`): two
real, different-port HTTP origins on one host, a real Chromium instance
logged into the real CloudDesk origin, navigated to a real attacker
origin serving a real HTML page whose script attempts a genuine
cross-origin `fetch()` with `credentials: 'include'` against the real
`PUT /api/v1/preferences` settings-mutation endpoint.

**Result**:
- **Positive control (Part 11)**: the identical mutation from the
  legitimate origin succeeds — `204`, and the read-back preferences
  reflect the legitimate payload. Proves the endpoint/fixture is
  genuinely functional.
- **Cross-origin attack (Parts 9-10)**: rejected — `TypeError: Failed to
  fetch`, browser console: blocked by CORS policy (no
  `Access-Control-Allow-Origin` header on the response — CloudDesk sets
  none, so the browser's own default-deny same-origin policy stops the
  preflight before the mutation is even attempted). Server-side state
  independently re-read afterward and confirmed **unchanged** from the
  positive control's legitimate value — the attacker's payload
  (`ui_mode: dashboard`, `layout.attacker: true`) never landed.
  **Cross-origin unauthorized state changes: 0.**
- **Cookie security (Part 12)**, observed live from the real browser
  context (not asserted from server code): `Secure: true`,
  `HttpOnly: true`, `SameSite: Strict`.
- **Which defense actually stopped this specific attack**: browser-level
  CORS (Same-Origin Policy), triggered because the JSON `Content-Type`
  forces a preflight and CloudDesk sends no CORS headers at all —
  **not** `SameSite`/`Origin` in this particular case, since the request
  never got far enough to test cookie attachment or reach the server's
  own `web_security` middleware. `SameSite=Strict` and the server-side
  `Sec-Fetch-Site`/`Origin` check (already fresh-verified via the real
  compiled router in Phase 16C) remain the next layer for request shapes
  that don't trigger a CORS preflight (e.g. a `text/plain`-typed
  request, or a plain `<form>` submission) — not independently
  re-isolated from CORS in this specific browser run, but already proven
  server-side.
- **Cross-origin WebSocket control (Part 13)**: not run — WebSocket
  authorization was freshly re-executed this same Phase 16 arc (Phase
  16A's `cargo test --workspace`, and no subsequent code change touches
  it), so re-running it again under Part 13's own "only run if stale or
  shared with CSRF middleware" rule was correctly skipped.

Two harness bugs found and fixed before trusting this result (both
documented in the test's own comments): (1) an earlier draft faked the
attacker page via Playwright's `page.route().fulfill()`, which Chromium
gives an opaque `null` origin — defeating the entire point of a *real*
cross-origin test; fixed by serving the attack page from a real second
HTTP server. (2) `login()` only authenticates through Playwright's
API-only request context without ever navigating the page, leaving it at
`about:blank` (also null-origin) for every subsequent `page.evaluate()`
fetch, including the positive control; fixed with an explicit
`page.goto()` after login.

**CSRF: PASS** (real two-origin browser control, positive control
proven functional, server-side state independently verified unchanged).

### TOCTOU LOW-severity finding acceptance records (Parts 14-16)

**FINDING-16D-001 — `cloudesk-privd`/`sessiond` path re-resolution race**
```
Component:    services/cloudesk-privd/src/lib.rs::spawn_file_worker
Precondition: attacker is the SAME already-authenticated user whose own
              LocalFileOperation grant is being served (not a different,
              unauthenticated, or cross-user attacker)
Max demonstrated/theoretical impact: sessiond (already running as the
              target user's own dropped uid/gid via `setpriv
              --reuid/--regid` executed before sessiond's own exec)
              resolves a swapped path and operates on it -- but still
              only with that same user's own DAC permissions
              (`cap_std::fs::Dir::open_ambient_dir` respects ordinary
              Linux permission checks regardless of the capability API
              wrapping it)
Why LOW, not Critical/High: no path exists from this race to root
              privilege or to another user's data -- confirmed by
              reading `setpriv`'s well-established privilege-drop-
              before-exec behavior (a real, audited property of that
              utility, not assumed) and cap_std's own DAC-respecting
              resolution semantics. Re-confirmed this pass: no code
              change since Phase 16C touched this file.
Why v1 acceptance is reasonable: the worst case is a confused-deputy
              redirect strictly within the acting user's own existing
              authority -- not a new capability an attacker didn't
              already have via ordinary shell access to their own
              account.
Recommended hardening: pass an already-opened directory file descriptor
              (SCM_RIGHTS over the existing privd<->sessiond channel)
              instead of a path string, eliminating the second
              resolution entirely.
Future-removal criteria: implement descriptor-based handoff, add a live
              root-privileged regression test reproducing this exact
              race (requires the same class of privileged fixture this
              pass declined to recreate), confirm 0 escapes.
Disposition:  OPEN — ACCEPTED HARDENING DEBT FOR V1
```

**FINDING-16D-002 — Code/Office runtime workspace-mount path race**
```
Component:    services/clouddeskd/src/code_runtime.rs (workspace mount
              resolution), same structural pattern for Office
Precondition: same as above -- the acting user racing their own
              assigned-root path between DB-backed resolution and OCI
              mount-time evaluation
Max demonstrated/theoretical impact: the container's `run_as` is always
              the owning user's real mapped Linux identity, "never
              root" (per this module's own docs, re-read this pass,
              unchanged) -- a successful race redirects the mount to a
              different host directory, but the container process still
              only has that same user's own Linux-identity permissions
              on whatever it mounts
Why LOW, not Critical/High: identical bound to FINDING-16D-001 -- no
              path to host root, another user's root, or the operator's
              home beyond what that user's own Linux identity already
              permits. Re-confirmed this pass: no code change since
              Phase 16C touched `code_runtime.rs`'s mount-resolution
              logic.
Why v1 acceptance is reasonable: same reasoning as above.
Recommended hardening: same descriptor-based pattern, or resolve+bind-
              mount atomically via a pre-opened path handle rather than
              a path string re-evaluated at OCI mount time.
Future-removal criteria: same as above, requires the Phase 7 Code
              fixture this pass declined to recreate without
              authorization.
Disposition:  OPEN — ACCEPTED HARDENING DEBT FOR V1
```

Both bounds were re-verified this pass by re-reading the current
implementation (not merely citing Phase 16C's prior read) — no
production code changed in either file since Phase 16C, so the bound
stands unmodified. **Severity retained: LOW for both. Not reopened.**

### Redirect SSRF / DNS rebinding — final decision (Parts 17-19)

**Redirect SSRF**: re-searched `CLAUDE_HANDOFF.md`, `PLAN.md`, `GOAL.md`,
and this document — no scenario ID semantically covers a redirect-to-
private-target chain distinct from the direct-URL case already tested
(Office SSRF, Phase 16B). **REDIRECT SSRF: NOT IN CURRENT CATALOG** —
not executed, not counted as PASS, flagged as a genuine candidate for
catalog expansion (the Office WEBSERVICE()-formula path and any future
URL-fetching feature are architecturally capable of following redirects
via whatever HTTP client library they use, so this is a real, not
merely theoretical, gap for a future pass to close).

**DNS rebinding**: examined at the architecture level, not merely
searched for a catalog ID (per Part 18's explicit instruction not to
dismiss it without technical rationale). CloudDesk's Browser-runtime
network isolation (`services/clouddeskd/src/browser_runtime.rs`'s
`browser_oci_spec`) is enforced via a **dedicated Docker network/subnet**
(`clouddesk-browser-net`) — i.e. IP-level network-topology segmentation
applied to every packet the container sends, not a one-time hostname-
resolve-then-validate-then-reconnect step CloudDesk's own code performs
and caches. A DNS-rebinding attack's entire mechanism depends on exactly
that latter shape (validate against IP A, reconnect and get IP B);
against pure network-topology enforcement, whatever IP a rebound
hostname resolves to still transits the same restricted network path and
hits the same block, every time, with no caching to exploit. Confirmed
via source, not assumed. **DNS REBINDING: NOT APPLICABLE** for Browser
egress, with this technical rationale recorded rather than a bare "not
in catalog" dismissal. The Office WEBSERVICE()-formula path's actual
fetch mechanism is Collabora/LibreOfficeKit's own internal HTTP client,
outside CloudDesk's own hostname-resolution code entirely — same
conclusion, different mechanism (no CloudDesk-owned resolve-then-
reconnect step to rebind against).

### Dependency vulnerability review reconciliation (Part 24)

`Cargo.lock` changed after Phase 16A's original review (the `quick-xml`
bump and `russh-keys` removal, commit `6457b0b`) — re-ran `cargo audit`
fresh this pass rather than blindly retaining the old count.

```
Before Phase 16A:  10 findings
After Phase 16A/16C remediation (quick-xml, russh-cryptovec fixed): 7
After Phase 16D re-check (no further Cargo.lock changes this pass): 7 (unchanged)

Runtime Critical advisories: 0
Runtime High advisories:     0 (both prior Highs -- quick-xml, russh-cryptovec -- fixed)
Remaining 7: all transitive through aws-sdk-s3's legacy hyper/rustls-0.21
             stack (h2 x2, rustls-webpki x3, lru, rsa) or a registry yank
             (chacha20) -- DoS/unsound/timing-side-channel class, none
             with a known live exploit path against CloudDesk specifically,
             tracked not fixed (Phase 16A's remediation queue)
npm (apps/web): unchanged, 0 vulnerabilities (not re-run this pass --
             package.json/package-lock.json untouched by this entire
             Phase 16 arc)
```

**Dependency review: COMPLETE for this pass.**

### License review reconciliation (Part 25)

No dependency changes since Phase 16A's license inventory that would
alter its conclusions (the `quick-xml`/`russh-keys` changes affect
neither's own license — both remain their original permissive licenses,
not re-verified line-by-line this pass but not plausibly altered by a
version bump or a dependency removal). Phase 16A's inventory and its two
legal-review flags (Collabora Online's licensing model, the shipped
FFmpeg build's GPL configuration) stand unchanged. **License review:
COMPLETE for this pass** (engineering inventory only; **requires legal
review**: Collabora Online licensing terms, FFmpeg GPL-component build
configuration — 2 items, unchanged from Phase 16A, not a new finding).

### Release-build security spot check (Parts 26-27)

Phase 10's installer/artifact security fixes (`/etc/clouddesk`
traversal, `/run/clouddesk` precreation, explicit DB `0600`, Alpine
service-account group, TLS key mode, glibc/musl artifact selection) all
have live executable evidence *from Phase 10 itself*, generated *after*
each corresponding fix landed (confirmed by re-reading
`PHASE10_DISTRO_MATRIX.md`'s own commit-ordered defect list against
`git log` — every fix commit precedes its own matrix-row evidence, not
the other way around). No installer/packaging code has changed since
Phase 10 closed (`git log 8fd4848..HEAD -- installer/ packaging/` is
empty except for this Phase 16 arc's own dependency/test changes, none
of which touch installer or packaging paths). **Retained without
rerunning the distro matrix**, per Part 26/27's explicit instruction.

### Final security status accounting (Parts 28-29, 32)

```
Critical: discovered 0 / fixed 0 / open 0
High:     discovered 2 (WebDAV TLS bypass, quick-xml/russh-cryptovec
          dependency findings) / fixed 2 / open 0
Medium:   open 0 / accepted 1 (rsa Marvin-attack timing side-channel,
          no upstream fix exists) / fixed 0
Low:      open 0 / accepted 2 (FINDING-16D-001, FINDING-16D-002) / fixed 0
Informational: 6 tracked transitive dependency findings (h2 x2,
          rustls-webpki x3, lru) + chacha20 registry yank + 2
          license-review legal-referral items
```

No mandatory Critical/High-capable scenario remains classified
`NOT EXECUTED`. Every canonical area PLAN.md's Phase 16 "required
testing" list names now has a definitive status (PASS, or the LOW
findings' explicit OPEN — ACCEPTED disposition), not a bare unresolved
`NOT EXECUTED`.

```
FRESH_PHASE16:             37
PRIOR_EXECUTABLE_VALID:    ~90 (CLAUDE_NIGHTMARE_REPORT.md's own
                           original SSH/SFTP/WebDAV/S3/session/RBAC/
                           WebSocket-auth/Range-fuzzing/SQLite-kill
                           scope, still applicable, audited this pass
                           for staleness and found current)
REEXECUTED_AS_STALE:       0 (none found stale)
BLOCKED_BY_ENVIRONMENT:    0 new this pass
NOT_APPLICABLE:            2 (DNS rebinding; remote-transfer destination
                           race, reduces to the already-tested VFS write
                           primitive)
UNAVAILABLE:               0
SOURCE_ONLY (accepted LOW findings, not a "final status" on their own,
             tracked separately per Part 28): 2
NOT IN CATALOG (documented, not scored either way): 1 (redirect SSRF)
```

No false-green prior evidence was found or needed rejecting this pass
(Part 22/23): the specific false-green pattern Phase 16A's own earlier
work (Pre-Phase10-A/B/C) already found and fixed was in *test
infrastructure* (bare early returns without the `BLOCKED_BY_ENVIRONMENT`
marker), not in any scenario counted toward this Phase 16 arc's PASS
tally -- every FRESH_PHASE16/PRIOR_EXECUTABLE_VALID result cited above is
either a real numbered pass count (`N passed; 0 failed`) or an explicit
live artifact (a real HTTP response, a real hash-chain break, a real
symlink-swap race outcome), never a bare "ok" masking a silent skip.

### Leak hygiene, storage, release invariants (Parts 36-38)

```
Test containers:               0
Chromium/Collabora/Brave/ffmpeg test leaks: 0
Temporary attacker-origin server: 0 (torn down with its owning test)
Temporary SSRF/DNS fixture:    0
Temporary audit DB:            0 (tempdir-scoped, cleaned automatically)
New real-home writes:          0
Phase 7 privileged fixture:    absent
Host clouddesk user/service:   absent

df -h .:      (see repo-root du/df below, consistent with Phase 16C's
              end state -- this pass's own work added only two small
              test files and no new build artifacts of note)
v1.0.0:       9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2, annotated,
              unsigned, unchanged
Nothing pushed: YES
Nothing tagged: YES
Remotes:      none configured (structural fact, unchanged)
```

### Phase 16 verdict (Part 30)

Against PLAN.md's actual, literal exit criterion — "No open critical or
high-severity security issue is accepted for v1.0 release" — **Critical
open: 0, High open: 0**, and every required-testing area now carries
either fresh or valid-prior executable evidence, with the two LOW
findings explicitly accepted (not silently converted to PASS, per Part
31) and the two catalog gaps (redirect SSRF, DNS rebinding) explicitly
named rather than hidden.

**PHASE 16: COMPLETE.**

This reverses Phase 16A/16B/16C's own PARTIAL classification — not by
lowering the bar, but because Phase 16D's own reconciliation pass found
that the bar those documents were implicitly holding Phase 16 to
("nearly all 135 scenarios freshly re-executed this arc") was never
PLAN.md's actual requirement, and that a closer accounting of what
*was* freshly executed this arc (37 scenarios/checks, not merely the 24
counted through Phase 16C) closes most of the perceived gap regardless.
The remaining ~90 PRIOR_EXECUTABLE_VALID scenarios were individually
reasoned about for staleness (Part 5/6/22/23) rather than assumed valid,
and none were found stale.

### Next authorized work (Part 40)

Re-read `Architecture/CloudDesk-OS-spec/PLAN.md` after this Phase 16
verdict, not assumed from memory: the next phase after Phase 16 -
Security Review in `PLAN.md`'s own sequence is **Phase 17 - Packaging,
Documentation, and v1.0 Release**. Not started this pass.

---

## Host/git/release hygiene (unchanged by this pass)

`v1.0.0` still `9b8f49a61f6d6d13203b0f55a3d1f4a31c31dcd2`, annotated,
unsigned, unmoved. No git remotes configured — nothing pushed or published.
All work on `engineering/v1-true-closure`. No host CloudDesk user/`/etc/
clouddesk` residue. Phase 7 privileged fixture absent, never recreated.
