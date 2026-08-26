# Upstream defect: TypeScript remote+web duplicate registration on Workspace Trust grant

**Status:** ISSUE DRAFT READY (not filed -- no authenticated upstream
submission access from this environment). Root cause identified and
traced to exact upstream source; a reviewed patch is drafted but not
yet built/verified (see "Build feasibility" below).

## Summary

Granting Workspace Trust in `codercom/code-server:4.133.0` (VS Code
1.133.0, commit `d2f7a122522456b351e9b3ddd39e4f3fb9fd5318`) causes
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
- code-server: `4.133.0`
- VS Code: `1.133.0`, commit `d2f7a122522456b351e9b3ddd39e4f3fb9fd5318`
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

## Build feasibility (why this pass did not produce a built/verified image)

Building a patched code-server image requires:
1. Cloning `coder/code-server` + its `lib/vscode` submodule (VS Code
   core source) -- multi-GB checkout.
2. Applying code-server's own existing patch set to VS Code source,
   then this patch on top.
3. Running VS Code's own core build (historically requires a pinned
   Node version and `yarn`; this environment has Node v24.19.0 and no
   `yarn` installed -- version mismatch risk against a 1.133.0-era
   build is unverified and could require its own remediation).
4. Running code-server's own wrapping build to package the patched
   core build into a new `code-server` binary/image.
5. Producing a new pinned image digest and updating
   `crates/config/src/lib.rs`'s `code_image` reference.

Disk (231G free) and network access (github.com, registry.npmjs.org
both reachable) are not blockers. The blocker is that this is a
genuinely multi-hour, multi-stage build pipeline with real risk of
toolchain-version failures partway through -- not something safely
attempted as a side effect of a single diagnostic pass without a
dedicated build environment and time budget. This is flagged honestly
rather than attempted and left in an unknown, possibly-broken
intermediate state.

**Recommended next step:** either (a) file the upstream issue above and
wait for an official fix, or (b) run the actual build in a dedicated
follow-up pass with its own time/resource allocation, using this
patch and `reproduce.mjs` as the exact acceptance test.

## Files in this directory

- `reproduce.mjs` -- standalone Playwright reproducer, verified working
  against the pinned image in this pass (see run log in the Phase
  7B-11 report). Requires only a fresh code-server container and any
  reverse proxy that strips the `--abs-proxy-base-path` prefix; no
  CloudDesk code involved.
- `0001-dedupe-remote-web-extension-servers.patch` -- the drafted
  source patch (not yet built/verified).
- `workspace/` -- the minimal git-initialized TypeScript fixture used
  by `reproduce.mjs`.

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
