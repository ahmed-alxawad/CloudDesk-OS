# Upstream defect: TypeScript remote+web duplicate registration on Workspace Trust grant

**Status:** ISSUE DRAFT READY (not filed -- no authenticated upstream
submission access from this environment). Root cause identified and
traced to exact upstream source. **Fix built and verified in this
environment** as `clouddesk/code-server:4.133.0-patch1` (Phase 7B-12) --
see "Build results" below. CloudDesk now pins this image
(`crates/config/src/lib.rs`'s `code_image`) instead of stock
`codercom/code-server:4.133.0`.

## Summary

Granting Workspace Trust in `codercom/code-server:4.133.0` (code-server
commit `d2f7a122522456b351e9b3ddd39e4f3fb9fd5318`; bundled VS Code
1.133.0, commit `a5b500951314efd502d07465bd138dfbd714a960`) causes
`vscode.typescript-language-features` to be registered twice --once via
the remote extension management server, once via the web extension
management server-- producing:

```
Extension 'vscode.typescript-language-features' is already registered
```

and leaving TypeScript permanently unable to activate for the rest of
the session. This reproduces on a bare, standalone code-server
container with **zero CloudDesk code involved** -- see `reproduce.mjs`
in this directory.

## Upstream issue draft

**Title:** Workspace Trust grant causes `typescript-language-features`
(and other builtins with a `browser` entry point) to register twice
when both a remote and a web extension management server are
configured

**Versions:**
- code-server: `4.133.0`, commit `d2f7a122522456b351e9b3ddd39e4f3fb9fd5318`
- VS Code (`lib/vscode` submodule at that commit): `1.133.0`, commit
  `a5b500951314efd502d07465bd138dfbd714a960`
- Also confirmed present (identical logic) in `microsoft/vscode@main`
  as of 2026-08-27 -- not fixed by any release up to current upstream
  HEAD.

**Environment:** Linux, code-server run as
`code-server --bind-addr 0.0.0.0:8080 --auth none --abs-proxy-base-path <prefix> <workspace>`,
accessed through any reverse proxy that strips `<prefix>` before
forwarding (a real proxy is required in front for `--abs-proxy-base-path`
mode to serve anything other than 404 -- see "Gotchas" below).
Reproduces in headless Chromium via Playwright; not tested against a
real interactive browser session, though nothing in the trace is
Playwright-specific.

**Steps to reproduce** (see `reproduce.mjs` for the exact automated
version):
1. Start a fresh code-server container with a git-initialized
   TypeScript workspace mounted, `--abs-proxy-base-path` set, no
   persisted profile.
2. Open the workspace with a `.ts` file deep-linked via
   `?folder=<path>&payload=[["openFile","vscode-remote://remote/<path>/file.ts"]]`.
3. Open the integrated terminal (`Ctrl+\``) -- creating its PTY process
   requires executing code, so VS Code shows its standard "Do you
   trust the authors of the files in this folder?" dialog.
4. Click **Trust Folder & Continue**.

**Expected:** the trust transition succeeds and
`vscode.typescript-language-features` activates normally (one
registration, hover/completion/diagnostics all functional).

**Actual:** the browser console logs
`Extension 'vscode.typescript-language-features' is already registered`
immediately after the trust transition, followed later by
`Activating extension 'vscode.typescript-language-features' failed: Not Found.`
TypeScript hover/completion/diagnostics never work for the rest of the
session (until a full window reload).

**Root-cause trace** (confirmed via non-pausing CDP logpoints against
the exact pinned bundle -- see the parent investigation's Phase
7B-4..7B-10 history for the full derivation):

```
WorkspaceTrustTransitionParticipant (workbench.contrib.workspacesTrust)
  -> participate(trustGranted=true)
  -> IWorkbenchExtensionEnablementService.updateExtensionsEnablementsWhenWorkspaceTrustChanges()
       [src/vs/workbench/services/extensionManagement/browser/extensionEnablementService.ts]
  -> re-evaluates every extension in ExtensionsManager.extensions for
     trust-sensitivity, fires _onEnablementChanged with all that
     changed -- including BOTH:
       - vscode.typescript-language-features [remote-server copy]
       - vscode.typescript-language-features [web-server copy]
  -> AbstractExtensionService.onEnablementChanged handler
     [src/vs/workbench/services/extensions/common/abstractExtensionService.ts]
  -> _handleDeltaExtensions(new _de(added, removed))
  -> ExtensionHostExtensions.deltaExtensions(toAdd, toRemove)
     blindly concatenates toAdd -- toAdd already contains BOTH TS
     entries from the SAME single input, no in-array dedup
  -> ExtensionDescriptionRegistry._initialize()
     detects the second entry shares an identifier already in the map,
     logs the "already registered" error, drops it -- but the
     resulting registration state is inconsistent enough that TS never
     successfully activates afterward.
```

**Actual origin of the duplicate** (the two TS entries are legitimate,
independently-correct reports from two different servers -- neither
`ExtensionsManager` nor the registry is wrong to treat them as
distinct; the bug is one layer earlier):

```
ExtensionManagementService.getInstalled()
  [src/vs/workbench/services/extensionManagement/common/extensionManagementService.ts]
  -> queries every configured server (local/remote/web) in parallel
     and concatenates results verbatim, with NO deduplication
  -> code-server always configures BOTH remoteExtensionManagementServer
     (it's always a real remote host) AND webExtensionManagementServer
     (it always serves VS Code Web's static web-extension assets from
     the same origin) -- a combination upstream's more common
     deployment shapes (pure remote XOR pure web) rarely exercise
     together for a dual-capable builtin
  -> vscode.typescript-language-features ships both a `main` (Node,
     remote-capable) and a `browser` (web-worker-capable) entry point
     in its manifest, so BOTH servers correctly, independently report
     it as "installed" -- getInstalled() faithfully returns both as
     separate ILocalExtension entries (same identifier/version/uuid,
     different extensionLocation/targetPlatform)
```

**Security implication:** `--disable-workspace-trust` "fixes" this
symptom by never triggering the transition at all, but that flag
disables Workspace Trust globally and is **not an acceptable
workaround** for any deployment where users can open content from
sources they don't fully control (git clone, uploads, SFTP/S3/SSH
transfers, shared storage) -- Workspace Trust is the only thing gating
automatic terminal/debug process creation, "restricted" workspace
settings, and trust-sensitive extension activation in this build. A
deployment that legitimately needs Workspace Trust enabled has no
supported way to avoid this defect today.

## Later-source-history findings (Part 3/4)

Fetched `src/vs/workbench/services/extensionManagement/{browser/extensionEnablementService.ts,common/extensionManagementService.ts}`
directly from `https://raw.githubusercontent.com/microsoft/vscode/main/...`
(2026-08-27) and confirmed:

- `updateExtensionsEnablementsWhenWorkspaceTrustChanges()` is
  byte-for-byte the same logic as the pinned 1.133.0 build: no
  deduplication, sources its list from `this.extensionsManager.extensions`.
- `ExtensionsManager.updateExtensions()` intentionally dedupes only
  within `(identifier, server)` pairs -- unchanged, and correctly so
  (see patch rationale above).
- `ExtensionManagementService.getInstalled()` is **also unchanged**:
  still queries `this.servers` in parallel and concatenates with no
  cross-server dedup.

**Conclusion: no released or even unreleased (current `main`) VS Code
version fixes this.** An upgrade is not a viable path (Part 6 is
therefore N/A) -- this genuinely requires either an upstream fix (not
yet written, per this investigation) or a maintained downstream patch.

## The patch

See `0001-dedupe-remote-web-extension-servers.patch` in this directory
for the full rationale and diff. Summary: `getInstalled()` should drop
a builtin's web-server copy when the *same* builtin identifier is also
present via the remote server, since the web copy exists specifically
to cover "no remote connection available," which is never true when a
remote server is configured at all.

## Build results (Phase 7B-12)

Built successfully using code-server's own real build pipeline --
no minified-bundle editing, no `sed` on compiled output.

**Toolchain actually used** (matches code-server 4.133.0's own pins:
`.node-version` = `24.18.0`, `engines.node` = `24`; this environment's
Node v24.19.0 was compatible):
- `node:24-bookworm` base image (disposable Docker builder, never the
  host)
- `apt-get install git quilt build-essential python3 libkrb5-dev
  pkg-config libsecret-1-dev libx11-dev libxkbfile-dev rsync jq curl`
  (`libx11-dev`/`libxkbfile-dev` needed for `native-keymap`'s
  node-gyp build, a desktop-only dependency VS Code's monorepo still
  compiles during `npm ci` even for a web/remote-only build target)
- npm (code-server ships `package-lock.json`, not a yarn lockfile --
  no `yarn` needed)

**Exact steps:**
1. `git clone --depth 1 --branch v4.133.0 https://github.com/coder/code-server.git`
2. `git submodule update --init --depth 1 lib/vscode` -- confirmed
   checked out at `a5b500951314efd502d07465bd138dfbd714a960`
3. `quilt push -a` -- code-server's own 25-patch official series
   applied cleanly
4. Copied `0001-dedupe-remote-web-extension-servers.patch` into
   `patches/`, appended it to `patches/series`, `quilt push` -- applied
   cleanly on top of the official series; verified present in the
   actual `lib/vscode/src/vs/workbench/services/extensionManagement/
   common/extensionManagementService.ts` working tree afterward
5. `SKIP_SUBMODULE_DEPS=1 npm ci` (code-server's own deps, 382
   packages) then `npm run build` (`tsc`, code-server's own
   TypeScript + frontend)
6. `cd lib/vscode && npm ci` (1574 packages including every bundled
   extension's own dependencies, ~13 min) then, from the repo root,
   `VERSION=0.0.0 VSCODE_TARGET=linux-x64 npm run build:vscode`
   (esbuild-based -- gulp's `compile-copilot-extension-full-build`,
   `core-ci`, `vscode-reh-web-linux-x64-min-ci`; total ~13 min,
   confirmed the patched function compiled into the real output
   `out/vs/code/browser/workbench/workbench.js`)
7. `KEEP_MODULES=1 npm run release` -- produced a complete, runnable
   `release/` directory (735M)
8. Packaged `release/` into `clouddesk/code-server:4.133.0-patch1`
   via a minimal `node:24-bookworm-slim`-based Dockerfile (does not
   use code-server's own `.deb`/nfpm release-image pipeline, which
   was judged unnecessary complexity for a locally-run, never-pushed
   image; entrypoint is `node /usr/lib/code-server/out/node/entry.js`,
   confirmed to accept the exact same CLI flags -- `--bind-addr`,
   `--auth`, `--disable-telemetry`, `--disable-update-check`,
   `--disable-proxy`, `--abs-proxy-base-path` -- as the real
   `code-server` binary)

No build errors after the two toolchain gaps above (native-keymap's
X11 headers; the initial bind-mounted build directory living on a
7.8G tmpfs and running out of space -- moved the build into the
container's own overlay filesystem, which had 220G+ free, before
retrying).

**Image:** `clouddesk/code-server:4.133.0-patch1`
**Image ID:** `sha256:3207500bf8d88cc47953f13729e08a938b71d684610eaddf5e7ed51b507c82ea`
**Patch SHA256:** see `sha256sum 0001-dedupe-remote-web-extension-servers.patch`
in this directory.
**Never pushed to any registry** -- built and used entirely locally,
matching `browser_image`'s existing local-build precedent in
`crates/config/src/lib.rs`.

### Standalone patched-image verification (`reproduce.mjs` + two follow-on checks)

- **`reproduce.mjs` against the patched image:** post-trust `toAdd`
  is `["vscode.git","vscode.terminal-suggest","vscode.typescript-language-features"]`
  -- exactly ONE TypeScript entry (`targetPlatform:"undefined"`,
  i.e. the remote copy; the web duplicate is gone). Zero "already
  registered" errors anywhere in the run.
- **Live hover check:** `function greet` hover correctly returned
  `function greet(name: string): string` -- TypeScript is genuinely
  functional, not just silent.
- **Negative control (Part 13):** clicked **Cancel** on the trust
  dialog, then typed and ran `echo NEGATIVE_CONTROL_MARKER_12345` in
  the terminal -- the marker never appeared in the terminal buffer
  (empty output), proving the PTY was never created and Workspace
  Trust's protection is fully intact with the patch applied.

### CloudDesk product-level verification

`crates/config/src/lib.rs`'s `code_image` (and the matching test
constants in `services/clouddeskd/tests/{code_playwright,
settings_playwright}.rs`) now point at `clouddesk/code-server:
4.133.0-patch1`. Running the real compiled CloudDesk frontend's
`task_full_product_journey` (login -> Files -> Open with Code -> edit
-> save -> open terminal -> real Workspace Trust dialog -> explicit
"Trust Folder & Continue" -> ...) produced **zero** "already
registered" console errors, versus reliably reproducing on every
prior run against stock `codercom/code-server:4.133.0`. Phase 7A
regressions (`task_oci_mount_isolation`,
`task_returning_and_already_running_files_to_code`, `code_runtime`
25/25) all pass unchanged against the patched image.

A pre-existing, unrelated test-harness limitation (Playwright cannot
reliably scrape rendered xterm terminal output from inside the full,
multi-step journey, though an isolated direct probe captures it fine)
still causes `task_full_product_journey`'s overall assertion to fail
-- this is a test-harness blocker, not a regression from this patch,
and is out of scope for this fix (see the Phase 7B-12 report).

## Files in this directory

- `reproduce.mjs` -- standalone Playwright reproducer, verified working
  against both the stock (reproduces) and patched (clean) images.
  Requires only a fresh code-server container and any reverse proxy
  that strips the `--abs-proxy-base-path` prefix; no CloudDesk code
  involved.
- `0001-dedupe-remote-web-extension-servers.patch` -- the source patch,
  built and verified (Phase 7B-12) into `clouddesk/code-server:
  4.133.0-patch1`.
- `workspace/` -- the minimal TypeScript fixture used by
  `reproduce.mjs` (a plain directory is sufficient; a git repo is not
  required for the reproduction).

## Gotchas (for whoever runs this next)

- A code-server container configured with `--abs-proxy-base-path`
  returns a bare `404` if accessed directly at that path with no real
  reverse proxy in front (it expects the proxy to have already
  stripped the prefix). A transparent relay that does nothing but
  strip that one prefix is sufficient and does not itself affect the
  reproduction (confirmed: the defect is identical whether accessed
  through such a relay or through CloudDesk's real authenticated
  proxy).
- The deep-link's `payload=[["openFile","vscode-remote://remote/..."]]`
  URI uses the literal string `remote` as authority; VS Code
  canonicalizes it to the real resolved `host:port` internally --
  using a different placeholder string does not change the
  reproduction.
