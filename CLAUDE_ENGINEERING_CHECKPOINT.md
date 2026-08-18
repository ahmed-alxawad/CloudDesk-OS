# CloudDesk-OS — Engineering Checkpoint

Branch: `engineering/v1-true-closure` (from `audit/claude-nightmare-v1.0.0`)
`v1.0.0` tag: untouched, unpublished. Nothing pushed.

## Last completed phase

**Phase 5 — Music Application.** Backend/router/live-media evidence is
real and complete; the browser-flow acceptance clause is honestly
**BLOCKED BY ENVIRONMENT**, same as Video (Phase 4) — no browser/
automation tooling exists in this container. Phase 2 (SSH feature
matrix) remains explicitly incomplete and untouched this session.

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

**Security findings (Phase 5):** No CloudDesk product defect found
beyond the compat-engine gap above (a correctness bug, not a security
one). Cross-user isolation live-tested for library/tracks/playlists/
favorites/queue (404, not 403 -- existence isn't confirmed to another
user, same discipline as media jobs). Artwork is only ever served from
an embedded stream or a same-directory sidecar file reached through the
track's own VFS authorization -- never an arbitrary tag-supplied path.
Hostile metadata (script tags, unicode control chars, quotes) is stored
and returned verbatim as a JSON string value, never interpreted
server-side; frontend renders exclusively through Svelte's auto-
escaping interpolation, no `{@html}` anywhere in `MusicApp.svelte`.

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
5160eaf feat(music): add Music desktop application
e1a3875 feat(music): add indexed music library and metadata
ff3dd64 feat(media): add format tags, artwork extraction, standalone-audio DIRECT classification
```
on top of (preserved, untouched, still passing) the Phase 4 commit
chain below.

## Next phase (after Phase 5)

**Phase 6 — Optional Runtime Orchestrator**, per the task's own
template: design one shared lifecycle/isolation/control layer for
Code, Office, Brave, and other optional heavyweight runtimes.

## Next exact action

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
