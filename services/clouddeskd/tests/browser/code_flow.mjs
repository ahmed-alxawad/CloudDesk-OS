// Phase 7 browser-acceptance evidence harness. Runs inside a
// disposable, version-pinned Playwright/Chromium container (test
// infrastructure only). Drives the ACTUAL compiled CloudDesk
// frontend: login form, the app launcher, the real FilesApp.svelte,
// the real CodeApp.svelte, and -- through its authenticated proxy
// iframe -- the real code-server VS Code web UI.
//
// Usage: node code_flow.mjs <scenario> <jsonArgsFile>

import { chromium } from 'playwright';
import fs from 'node:fs';

const [, , scenario, argsFile] = process.argv;
const args = JSON.parse(fs.readFileSync(argsFile, 'utf8'));

function log(...parts) {
  process.stderr.write(`[${scenario}] ${parts.join(' ')}\n`);
}

async function withBrowser(fn) {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  const consoleErrors = [];
  page.on('console', (msg) => {
    if (msg.type() === 'error') {
      consoleErrors.push(msg.text());
      log('CONSOLE ERROR', msg.text());
    }
  });
  page.on('pageerror', (err) => {
    consoleErrors.push(String(err));
    log('PAGE ERROR', String(err));
  });
  try {
    const result = await fn(page);
    return { ...result, consoleErrors };
  } finally {
    await context.close();
    await browser.close();
  }
}

async function login(page, base, username, password) {
  await page.goto(base, { waitUntil: 'domcontentloaded' });
  await page.getByLabel('Username').fill(username);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: /Enter workspace/i }).click();
  await page.getByRole('button', { name: 'Open application launcher' }).waitFor({ timeout: 15000 });
  log('logged in as', username);
}

async function openLauncherApp(page, appName) {
  await page.getByRole('button', { name: 'Open application launcher' }).click();
  await page.locator('.launcher-grid button', { hasText: appName }).first().click({ timeout: 8000 });
}

async function waitForListingLoaded(page, timeoutMs) {
  await page.locator('.file-list[aria-busy="false"]').waitFor({ timeout: timeoutMs });
}

async function waitForCondition(page, description, predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return true;
    await page.waitForTimeout(300);
  }
  log('TIMEOUT waiting for', description);
  return false;
}

/// Opens the fixture folder -> src subfolder -> selects example.ts ->
/// clicks "Open with Code" through the real Files UI, then waits for
/// the real code-server workbench iframe to load. Returns a
/// FrameLocator scoped to code-server's own document.
async function openFileWithCode(page, folderName, subfolder, fileName) {
  await openLauncherApp(page, 'Files');
  await waitForListingLoaded(page, 15000);
  // A second `openFileWithCode` call in the same browser session (Task
  // C: already-running Code, second file) re-focuses the *existing*
  // Files window, which is still showing wherever the first call left
  // it (e.g. a subfolder) rather than a fresh root listing -- the
  // "Home" breadcrumb resets navigation regardless of that history, so
  // this is always root-relative like a first-ever open.
  await page
    .locator('nav[aria-label="Current folder"] button', { hasText: 'Home' })
    .first()
    .click({ timeout: 8000 })
    .catch(() => {});
  await waitForListingLoaded(page, 15000);
  await page.locator('.file-list button', { hasText: folderName }).first().dblclick({ timeout: 8000 });
  await waitForListingLoaded(page, 15000);
  if (subfolder) {
    await page.locator('.file-list button', { hasText: subfolder }).first().dblclick({ timeout: 8000 });
    await waitForListingLoaded(page, 15000);
  }
  await page.locator('.file-list button', { hasText: fileName }).first().click({ timeout: 8000 });
  await page.locator('button', { hasText: 'Open with Code' }).click({ timeout: 8000 });
  const gotIframe = await waitForCondition(
    page,
    '.code-app iframe present',
    async () => (await page.locator('.code-app iframe').count()) > 0,
    15000
  );
  if (!gotIframe) {
    const statusText = await page.locator('.code-status').first().innerText().catch((e) => `err: ${e}`);
    log('DEBUG code-status:', statusText);
    const windowTitles = await page.locator('.window .window-bar strong').allTextContents().catch((e) => `err: ${e}`);
    log('DEBUG open window titles:', JSON.stringify(windowTitles));
  }

  const codeFrame = page.frameLocator('.code-app iframe');
  const workbenchReady = await waitForCondition(
    page,
    'code-server workbench ready',
    async () => (await codeFrame.locator('.monaco-workbench').count()) > 0,
    60000
  );
  log('workbench ready:', workbenchReady);
  if (!workbenchReady) {
    const iframeSrc = await page.locator('.code-app iframe').getAttribute('src').catch((e) => `err: ${e}`);
    log('DEBUG iframe src:', iframeSrc);
    const bodyHtml = await codeFrame.locator('body').innerHTML().catch((e) => `err: ${e}`);
    log('DEBUG iframe body HTML (first 3000 chars):', bodyHtml.slice(0, 3000));
  }
  return codeFrame;
}

async function openTabTitles(codeFrame) {
  return codeFrame.locator('.tabs-container .tab .label-name').allTextContents();
}

const scenarios = {
  async full_product_journey() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);

      const codeFrame = await openFileWithCode(page, args.folderName, 'src', 'example.ts');

      const openedExactFile = await waitForCondition(
        page,
        'example.ts tab visible',
        async () => (await openTabTitles(codeFrame)).some((t) => t.includes('example.ts')),
        30000
      );
      log('opened tabs:', JSON.stringify(await openTabTitles(codeFrame)));

      // -- Task 10/11: real edit + save.
      const editorLine = codeFrame.locator('.monaco-editor .view-line', { hasText: 'const message' });
      await editorLine.click({ timeout: 15000 });
      await page.keyboard.press('End');
      await page.keyboard.press('Enter');
      await page.keyboard.type(`// ${args.sentinel}`);
      const editApplied = await waitForCondition(
        page,
        'sentinel visible in editor',
        // `.first()`: this code-server build also renders a chat/
        // interactive-input Monaco editor instance once the workbench
        // is ready, which independently matches `.monaco-editor
        // .view-lines` -- the real text editor for the open file is
        // always the first one in DOM order.
        async () => (await codeFrame.locator('.monaco-editor .view-lines').first().innerText()).includes(args.sentinel),
        10000
      );
      const isDirtyBefore = (await openTabTitles(codeFrame)).length > 0
        ? (await codeFrame.locator('.tabs-container .tab.dirty').count()) > 0
        : false;
      await page.keyboard.press('Control+s');
      const saved = await waitForCondition(
        page,
        'tab no longer dirty after save',
        async () => (await codeFrame.locator('.tabs-container .tab.dirty').count()) === 0,
        15000
      );
      log('isDirtyBefore:', isDirtyBefore, 'saved:', saved);

      // -- Task 14-17: real integrated terminal. Real defect fixed
      // during Phase 7B-9: `vscode.typescript-language-features` is a
      // trust-gated builtin (confirmed in Phase 7B-7/7B-8) -- it is not
      // even *registered* until workspace trust resolves, so hover/
      // completion below can never work unless the trust transition
      // (triggered here by the terminal's PTY-creation trust request)
      // happens first. The previous scenario ordering ran hover/
      // completion *before* opening the terminal, meaning TypeScript
      // hover was structurally guaranteed to fail regardless of the
      // extension-duplicate defect under investigation.
      await page.keyboard.press('Control+`');
      const terminalPanelVisible = await waitForCondition(
        page,
        'terminal panel visible',
        async () => (await codeFrame.locator('.terminal-wrapper, .xterm').count()) > 0,
        20000
      );
      log('terminal panel visible:', terminalPanelVisible);

      // Real defect fixed during Phase 7B-9: creating a terminal PTY
      // process requires executing code, so a fresh/untrusted workspace
      // makes VS Code show its standard "Do you trust the authors of
      // the files in this folder?" modal here -- previously this driver
      // immediately sent `whoami`+Enter assuming terminal focus, and
      // the stray Enter accidentally activated the dialog's default
      // "Trust Folder & Continue" action instead (confirmed live via
      // CDP instrumentation in Phase 7B-8: the dialog was genuinely
      // open with that exact text at the moment those keystrokes were
      // sent). The trust grant this produced is real user-legitimate
      // behavior (a real user opening a terminal would see and need to
      // resolve the same dialog) -- but it must be handled
      // *deliberately*, not accidentally, so this reproduction is
      // controlled rather than a race.
      const trustDialogLocator = codeFrame.locator('.monaco-dialog-box', { hasText: 'trust the authors' });
      const trustDialogAppeared = await waitForCondition(
        page,
        'workspace trust dialog appears',
        async () => (await trustDialogLocator.count()) > 0,
        8000
      );
      if (trustDialogAppeared) {
        const dialogText = await trustDialogLocator.first().innerText().catch(() => '');
        log('workspace trust dialog detected:', JSON.stringify(dialogText.slice(0, 200)));
        await codeFrame.getByRole('button', { name: 'Trust Folder & Continue' }).click({ timeout: 5000 });
        const dialogClosed = await waitForCondition(
          page,
          'workspace trust dialog closed',
          async () => (await trustDialogLocator.count()) === 0,
          10000
        );
        log('workspace trust dialog dismissed via explicit click:', dialogClosed);
      }
      log('workspace trust dialog handled explicitly:', trustDialogAppeared);

      // Real defect fixed during Phase 7B-9: this code-server build's
      // terminal-suggest/Copilot CLI integration shows a "Type copilot
      // to use Copilot CLI. Don't show this again" banner INSIDE the
      // terminal buffer on first open, which the readiness check below
      // (and every later `innerText()` read of the terminal) picks up
      // instead of real shell output -- dismiss it deterministically so
      // readiness/output checks reflect the actual PTY, not this
      // overlay decoration.
      const copilotBannerDismiss = codeFrame.getByText("Don't show this again");
      if (await copilotBannerDismiss.count().catch(() => 0)) {
        await copilotBannerDismiss.click({ timeout: 3000 }).catch(() => {});
        log('dismissed Copilot CLI suggestion banner');
      }

      // Terminal readiness (Part 2 regression): DOM presence of
      // `.xterm`/`.terminal-wrapper` alone only proves the panel
      // rendered, not that the shell process is actually ready to
      // accept input -- require visible prompt content too. Granting
      // trust can trigger an extension-host restart, so this window is
      // generous.
      // Real defect fixed during Phase 7C: `.xterm-screen, .xterm-
      // accessible-buffer, .terminal-wrapper` as a union selector lets
      // Playwright's `.first()` return whichever matches first in DOM
      // order -- `.terminal-wrapper` (a real container, but only
      // reliably textual once the disposable test profile forces DOM
      // rendering below) sorted first, masking the real target.
      // `.xterm-accessible-buffer` never exists in this build (it's
      // only rendered in screen-reader mode). `.xterm-screen` alone is
      // the correct, confirmed-working target once
      // `terminal.integrated.gpuAcceleration` is set to `"off"` in the
      // test profile (see code_playwright.rs's
      // `write_test_only_terminal_settings` -- this build renders
      // real PTY output via WebGL canvas by default, which never
      // appears in any DOM text at all, regardless of selector).
      const terminalText = codeFrame.locator('.xterm-screen');
      // Require an actual shell prompt (a `$` character), not merely
      // non-empty text -- the Copilot CLI banner overlay is also real
      // DOM text and would otherwise satisfy a bare non-empty check
      // before the shell is genuinely ready to accept input.
      const terminalReady = await waitForCondition(
        page,
        'terminal shell ready (prompt content present)',
        async () => {
          const t = await terminalText.first().innerText().catch(() => '');
          return t.includes('$');
        },
        45000
      );
      log('terminal ready:', terminalReady);
      // The banner can also appear slightly later than the first
      // dismiss attempt -- check once more right before typing.
      if (await copilotBannerDismiss.count().catch(() => 0)) {
        await copilotBannerDismiss.click({ timeout: 3000 }).catch(() => {});
        log('dismissed Copilot CLI suggestion banner (second check)');
      }

      // Hard regression guard (Part 2): never send terminal input while
      // a workspace-trust modal is still open -- that is exactly the
      // accidental-keystroke defect this pass fixes. Fail loudly rather
      // than silently racing focus again.
      if ((await trustDialogLocator.count()) > 0) {
        throw new Error(
          'REGRESSION: workspace trust dialog still open -- refusing to send terminal input (would repeat the Phase 7B-8 accidental-Enter defect)'
        );
      }

      // Real defect fixed during Phase 7C: the workspace-trust dialog
      // click above (or the extension-host restart it can trigger)
      // does not reliably leave keyboard focus inside the terminal's
      // hidden xterm textarea -- the very first command sent here
      // landed nowhere and its output never appeared, even though the
      // *second* command a few lines below (which explicitly clicks
      // `.xterm` first, at what was originally the only such click in
      // this flow) worked correctly. A real user would click into the
      // terminal before typing; do the same here, deterministically,
      // rather than assuming focus survived the dialog/readiness dance.
      await codeFrame.locator('.xterm').first().click({ timeout: 8000 }).catch(() => {});
      await page.keyboard.type('whoami');
      await page.keyboard.press('Enter');
      // Poll rather than a fixed sleep -- xterm rendering/PTY output
      // timing varies more under full-journey CPU load than in
      // isolation, and a fixed 1200ms proved unreliable.
      let termOutput = '';
      await waitForCondition(
        page,
        'whoami output looks real',
        async () => {
          termOutput = await terminalText.first().innerText().catch(() => '');
          return terminalWhoamiLooksReal(termOutput);
        },
        10000
      );
      const terminalWhoamiOk = terminalWhoamiLooksReal(termOutput);
      log('terminal after whoami:', JSON.stringify(termOutput.slice(-400)));

      // -- Task 26: hover on `greet`. Real defect fixed during Phase
      // 7B-9: the terminal panel opened above shrinks the editor's
      // viewport, and Monaco virtualizes lines outside it -- since
      // hover now runs *after* opening the terminal (required so
      // TypeScript is actually registered first), `function greet`
      // (line 1) may no longer be rendered in the DOM. Click into the
      // editor and jump to the top explicitly rather than assuming it's
      // already visible.
      await codeFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 10000 }).catch(() => {});
      await page.keyboard.press('Control+Home');
      const greetLine = codeFrame.locator('.monaco-editor .view-line', { hasText: 'function greet' });
      await greetLine.hover({ position: { x: 40, y: 8 }, timeout: 10000 });
      const hoverShowed = await waitForCondition(
        page,
        'hover widget with type info',
        async () => {
          const text = await codeFrame.locator('.monaco-hover').allTextContents().catch(() => []);
          return text.some((t) => t.includes('greet') || t.includes('string'));
        },
        10000
      );
      await page.keyboard.press('Escape');

      // -- Task 27: completion. Type a new line invoking greet( and
      // trigger completion on a partial member-like expression.
      const messageLine = codeFrame.locator('.monaco-editor .view-line', { hasText: args.sentinel });
      await messageLine.click({ timeout: 10000 });
      await page.keyboard.press('End');
      await page.keyboard.press('Enter');
      await page.keyboard.type('gre');
      await page.keyboard.press('Control+Space');
      const completionShowed = await waitForCondition(
        page,
        'suggest widget with greet',
        async () => {
          const items = await codeFrame.locator('.suggest-widget .monaco-list-row').allTextContents().catch(() => []);
          return items.some((t) => t.includes('greet'));
        },
        10000
      );
      await page.keyboard.press('Escape');
      // Remove the partial "gre" line to keep the fixture deterministic.
      await page.keyboard.press('Home');
      await page.keyboard.down('Shift');
      await page.keyboard.press('End');
      await page.keyboard.up('Shift');
      await page.keyboard.press('Delete');
      await page.keyboard.press('Backspace');

      // -- Task 28: introduce and clear a real type diagnostic.
      // Real defect fixed during Phase 7C: this typed directly after
      // the previous step's revert with no `Enter` first, landing on
      // the END OF THE SENTINEL COMMENT LINE itself -- the "select
      // whole line and delete" below then destroyed the sentinel along
      // with the diagnostic text, permanently losing it for the rest
      // of the journey (masked until now by the separate reopen-
      // navigation defect, which always failed before this point in
      // the file was ever re-read). Put the diagnostic on its own line.
      await page.keyboard.press('Enter');
      await page.keyboard.type('const badType: number = "not a number";');

      // Phase 7C-2 Part 2: the rendered squiggle is the LEAST reliable
      // oracle available (a CSS decoration, subject to the same class
      // of rendering/virtualization quirks already found in this
      // build). The Problems panel reflects the marker service's
      // actual diagnostic state directly -- open it explicitly and use
      // it as the primary observable; keep the squiggle only as
      // supplementary corroborating evidence.
      await page.keyboard.press('Control+Shift+M');
      const problemsPanel = codeFrame.locator('.panel .markers-panel, [id="workbench.panel.markers"]');
      await problemsPanel.first().waitFor({ timeout: 5000 }).catch(() => {});
      const diagnosticAppeared = await waitForCondition(
        page,
        'Problems panel shows the type error for example.ts',
        async () => {
          const rows = await codeFrame
            .locator('.markers-panel .marker, .monaco-list-row', { hasText: 'example.ts' })
            .allTextContents()
            .catch(() => []);
          const allRows = await codeFrame.locator('.markers-panel .monaco-list-row').allTextContents().catch(() => []);
          return (
            rows.some((t) => /not assignable|badType/i.test(t)) ||
            allRows.some((t) => /not assignable|badType/i.test(t))
          );
        },
        15000
      );
      const squigglyAppeared = (await codeFrame.locator('.squiggly-error').count()) > 0;
      log('diagnosticAppeared (Problems panel):', diagnosticAppeared, '| squigglyAppeared (supplementary):', squigglyAppeared);

      // Real defect fixed during Phase 7C-2: opening the Problems panel
      // (`Control+Shift+M`) moves keyboard focus into that panel's own
      // list widget -- the "select whole line and delete" below
      // previously ran with focus still there instead of the editor,
      // so it never actually touched the file at all (Home/Shift+End/
      // Delete/Backspace against a list widget is a no-op for editor
      // content). Click back into the editor's `badType` line first to
      // restore focus and cursor position before editing.
      const badTypeLine = codeFrame.locator('.monaco-editor .view-line', { hasText: 'badType' });
      await badTypeLine.click({ timeout: 8000 }).catch(() => {});
      await page.keyboard.press('End');
      await page.keyboard.press('Home');
      await page.keyboard.down('Shift');
      await page.keyboard.press('End');
      await page.keyboard.up('Shift');
      await page.keyboard.press('Delete');
      await page.keyboard.press('Backspace');
      await page.keyboard.press('Control+s');
      const diagnosticCleared = await waitForCondition(
        page,
        'Problems panel no longer shows the type error',
        async () => {
          const allRows = await codeFrame.locator('.markers-panel .monaco-list-row').allTextContents().catch(() => []);
          return !allRows.some((t) => /not assignable|badType/i.test(t));
        },
        15000
      );
      const squigglyCleared = (await codeFrame.locator('.squiggly-error').count()) === 0;
      log('diagnosticCleared (Problems panel):', diagnosticCleared, '| squigglyCleared (supplementary):', squigglyCleared);
      await page.keyboard.press('Control+s');
      await waitForCondition(
        page,
        'tab clean after final save',
        async () => (await codeFrame.locator('.tabs-container .tab.dirty').count()) === 0,
        10000
      );
      // Real defect fixed during Phase 7C-2: the Problems panel shares
      // the bottom panel area with the Terminal (VS Code tabs between
      // them, it doesn't stack them) -- a bare toggle of the panel
      // that opened Problems left it showing (or, worse, closed the
      // whole panel area) rather than reliably returning to the
      // Terminal tab the rest of the journey depends on. Explicitly
      // reopen the Terminal panel by its own shortcut instead of
      // toggling whatever was last active.
      await page.keyboard.press('Control+`');
      await codeFrame.locator('.xterm').first().waitFor({ timeout: 8000 }).catch(() => {});

      // -- Task 14-17 (continued): the terminal from earlier (trust
      // already resolved deliberately, before hover/completion/
      // diagnostics above) is still open -- refocus it and continue.
      await codeFrame.locator('.xterm').first().click({ timeout: 8000 }).catch(() => {});
      termOutput = await terminalText.first().innerText().catch(() => '');

      // Real defect fixed during Phase 7C: `code_oci_spec` always
      // mounts the *entire* authorized workspace root at `/workspace`
      // (see `code_runtime.rs`'s module doc -- `/workspace` equals
      // `home` for the default workspace, by design, so switching
      // subfolders never requires a remount), so a freshly opened
      // integrated terminal's cwd is `/workspace` itself, not the
      // fixture subfolder the Files picker navigated into to open the
      // file. `pwd`/`git status` previously ran at that root and their
      // real output (confirmed once terminal capture itself was fixed)
      // correctly reflected that -- the test's own expectation that
      // cwd would already be the subfolder was wrong, not the product.
      // A real user in this situation would `cd` into their project
      // folder before running project-scoped shell commands; do the
      // same here.
      await page.keyboard.type(`cd ${args.folderName}`);
      await page.keyboard.press('Enter');
      // Same stale-prompt race as the `git status` check below --
      // require the just-typed `cd` command's own echoed text, not
      // merely a trailing prompt that could be left over from the
      // idle terminal before this command was even sent.
      await waitForCondition(
        page,
        'cd into fixture folder completes',
        async () => {
          termOutput = await terminalText.first().innerText().catch(() => '');
          return termOutput.includes(`cd ${args.folderName}`) && termOutput.trim().endsWith('$');
        },
        10000
      );

      await page.keyboard.type('pwd');
      await page.keyboard.press('Enter');
      await waitForCondition(
        page,
        'pwd output shows folder name',
        async () => {
          termOutput = await terminalText.first().innerText().catch(() => '');
          return termOutput.includes(args.folderName);
        },
        10000
      );
      const terminalPwdOk = termOutput.includes(args.folderName);
      log('terminal after pwd:', JSON.stringify(termOutput.slice(-400)));

      // Real defect fixed during Phase 7C: this test's fixture
      // (`generate_fixture`) runs `git add -A && git commit` for every
      // tracked file at creation time, and the editor edits above
      // (diagnostic type + revert) were fully undone before saving, so
      // the tree is genuinely clean here -- `git status --porcelain`
      // correctly prints nothing. The previous assertion expected
      // dirty output ("example.ts" listed as changed), which was never
      // going to happen against a clean commit; it was a wrong test
      // expectation, not a product defect. A clean, prompt-terminated,
      // error-free result *is* the correct evidence that Git
      // integration works against a disposable local repo (Part 17).
      await page.keyboard.type('git status --porcelain');
      await page.keyboard.press('Enter');
      // Real defect fixed during Phase 7C: a bare trailing-`$` check is
      // satisfiable by the *previous* command's already-idle prompt
      // before this command's keystrokes have even been rendered into
      // the DOM (`.xterm-screen` only reflects the visible viewport,
      // not the full scrollback, so stale state can look identical to
      // fresh state) -- require the just-typed command's own echoed
      // text to be present too, not merely a trailing prompt.
      await waitForCondition(
        page,
        'git status command completes with a fresh prompt',
        async () => {
          termOutput = await terminalText.first().innerText().catch(() => '');
          return termOutput.includes('git status --porcelain') && termOutput.trim().endsWith('$');
        },
        10000
      );
      const gitStatusShowedFile = termOutput.includes('git status --porcelain') && !termOutput.includes('fatal:');
      log('terminal after git status:', JSON.stringify(termOutput.slice(-400)));

      // Close the terminal panel before debugging to reduce UI clutter.
      await page.keyboard.press('Control+`');
      await page.waitForTimeout(500);

      // -- Task 30-33: real local debug session.
      const debugResult = await runDebugScenario(page, codeFrame);
      // Phase 7C-3 Part 1: natural completion (above) and explicit Stop
      // (below) are separate acceptances and must never be conflated --
      // asserting Stop against a target that already exited is exactly
      // the mistake the prior pass made.
      const debugStopResult = await runDebugStopScenario(page, codeFrame);

      // -- Task 12: close and reopen, confirm persisted edit.
      await page.locator('.tabs-container .tab .label-name', { hasText: 'example.ts' }).first().click({ timeout: 8000 }).catch(() => {});
      await page.keyboard.press('Control+w').catch(() => {});
      // Real defect fixed during Phase 7C: this reopen sequence
      // duplicated `openFileWithCode`'s Files-navigation logic by hand
      // but omitted its "Home" breadcrumb reset (see that function's
      // own comment) -- the Files window here is the SAME instance
      // left showing the fixture's `src` subfolder from the earlier
      // `openFileWithCode` call, so a bare
      // `.file-list button` filtered on `folderName` never matched
      // anything (the fixture folder button isn't shown one level
      // below itself), causing the observed `dblclick` timeout. Reuse
      // the helper instead of re-deriving the same navigation.
      await openFileWithCode(page, args.folderName, 'src', 'example.ts');
      // Real defect investigated during Phase 7C: by this point in the
      // journey a real debug session has run (Task 30-33), which can
      // leave its own Debug Console/chat-adjacent Monaco editor
      // instances in the DOM -- the same class of DOM-order-dependent
      // `.first()` fragility already fixed once for the terminal
      // selector (see `terminalText` above) applies here too, since
      // `.monaco-editor .view-lines` matches more than the real file
      // editor (per `openTabTitles`'s own comment about the chat
      // editor). Check every match rather than assuming DOM order.
      const reopenedPersisted = await waitForCondition(
        page,
        'sentinel still present after reopen',
        async () => {
          const texts = await page
            .frameLocator('.code-app iframe')
            .locator('.monaco-editor .view-lines')
            .allTextContents()
            .catch(() => []);
          return texts.some((t) => t.includes(args.sentinel));
        },
        30000
      );

      return {
        ok: true,
        openedExactFile,
        editApplied,
        saved,
        hoverShowed,
        completionShowed,
        diagnosticAppeared,
        diagnosticCleared,
        trustDialogAppeared,
        terminalWhoamiOk,
        terminalPwdOk,
        gitStatusShowedFile,
        debugPaused: debugResult.debugPaused,
        debugContinuedOk: debugResult.debugContinuedOk,
        debugNaturalEnd: debugResult.debugNaturalEnd,
        debugFinalMarkerSeen: debugResult.debugFinalMarkerSeen,
        debugLongTargetAlive: debugStopResult.debugLongTargetAlive,
        debugExplicitStopOk: debugStopResult.debugExplicitStopOk,
        reopenedPersisted
      };
    });
  },

  /// Phase 7A-2 Task A/B (returning-user profile proof, part 1): opens
  /// Code with no deep-linked file at all (plain launcher open, exactly
  /// like a user just opening their IDE), waits for a real
  /// empty-window/default workbench, then closes the tab -- establishes
  /// genuine "code-server has a persisted profile with prior session
  /// state" without ever touching Files. The disposable profile this
  /// runs against is provided by the Rust harness, not this script.
  async establish_empty_window_state() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openLauncherApp(page, 'Code');
      await waitForCondition(
        page,
        '.code-app iframe present',
        async () => (await page.locator('.code-app iframe').count()) > 0,
        15000
      );
      const codeFrame = page.frameLocator('.code-app iframe');
      const workbenchReady = await waitForCondition(
        page,
        'code-server workbench ready (empty window)',
        async () => (await codeFrame.locator('.monaco-workbench').count()) > 0,
        60000
      );
      return { ok: workbenchReady, workbenchReady };
    });
  },

  /// Phase 7A-2 Task B (returning-user profile proof, part 2): against
  /// a profile that already has persisted empty-window state (see
  /// `establish_empty_window_state` above, run first by the Rust
  /// harness against the SAME disposable home), performs the real
  /// Files -> Open with Code flow and asserts the explicit user request
  /// wins over whatever workbench layout code-server had restored.
  async returning_user_files_to_code() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      const codeFrame = await openFileWithCode(page, args.folderName, 'src', 'example.ts');
      const openedExactFile = await waitForCondition(
        page,
        'example.ts tab visible (returning profile)',
        async () => (await openTabTitles(codeFrame)).some((t) => t.includes('example.ts')),
        30000
      );
      if (!openedExactFile) {
        log('opened tabs:', JSON.stringify(await openTabTitles(codeFrame)));
      }
      return { ok: openedExactFile, openedExactFile };
    });
  },

  /// Phase 7A-2 Task C: with Code already running (from a prior
  /// `openFileWithCode` in the SAME page session), selects a second,
  /// distinct file through Files and confirms that exact file becomes
  /// active -- without a page reload/new browser context, so a
  /// duplicate Code runtime can never be created by this flow (Task 9
  /// of the underlying orchestrator already guarantees single-instance
  /// reuse per user/kind; this proves the *product* handoff also
  /// reuses it, not just the API).
  async already_running_second_file_handoff() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openFileWithCode(page, args.folderName, 'src', 'example.ts');

      // Return to Files (the Code window stays open/running underneath)
      // and open a second, distinct fixture file.
      const secondFrame = await openFileWithCode(page, args.folderName, null, args.secondFileName);
      const openedSecondFile = await waitForCondition(
        page,
        `${args.secondFileName} tab visible`,
        async () => (await openTabTitles(secondFrame)).some((t) => t.includes(args.secondFileName)),
        30000
      );
      if (!openedSecondFile) {
        log('opened tabs:', JSON.stringify(await openTabTitles(secondFrame)));
      }
      return { ok: openedSecondFile, openedSecondFile };
    });
  },

  /// Phase 7D Part 9: mandatory security evidence -- Workspace Trust
  /// must gate DEBUG process creation, not just the terminal (already
  /// proven in the main journey). Runs against a genuinely fresh
  /// profile (the Rust harness resets it before this scenario, exactly
  /// like `full_product_journey`'s own setup) and never opens the
  /// terminal at all, so this is the FIRST trust-gated action the
  /// workspace ever sees -- proving debug itself is gated, not merely
  /// riding on a trust grant the terminal already obtained.
  async debug_before_trust_security_evidence() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      // `debug.js` lives directly in the fixture folder, not under
      // `src` (unlike `example.ts`) -- no subfolder to descend into.
      const codeFrame = await openFileWithCode(page, args.folderName, null, 'debug.js');

      const trustDialogLocator = codeFrame.locator('.monaco-dialog-box', { hasText: 'trust the authors' });

      // Real defect fixed while writing this scenario: `F5` sent
      // immediately after `openFileWithCode` returns did nothing
      // observable at all (no trust dialog, no debug session) --
      // reproducible across repeated runs, and unaffected by first
      // clicking into the editor for focus. Every OTHER scenario that
      // uses F5 in this file (`runDebugScenario`) does so only after
      // substantial prior workbench interaction (hover, completion,
      // diagnostics, terminal), giving the debug service time to
      // finish its own initialization; this scenario intentionally
      // skips all of that to make F5 the very first action, which
      // exposes the race. Explicitly opening the Run and Debug view
      // forces that initialization to complete before F5 is sent.
      await codeFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 8000 }).catch(() => {});
      await page.keyboard.press('Control+Shift+D');
      await codeFrame.locator('.debug-view-content, .debug-viewlet').first().waitFor({ timeout: 10000 }).catch(() => {});
      await codeFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 8000 }).catch(() => {});

      // Attempt debug BEFORE any trust decision. F5 must show the trust
      // gate, not launch a session.
      await page.keyboard.press('F5');
      const gateAppeared = await waitForCondition(
        page,
        'workspace trust dialog appears on first debug attempt',
        async () => (await trustDialogLocator.count()) > 0,
        10000
      );
      let effectiveGateAppeared = gateAppeared;
      if (!gateAppeared) {
        // Confirmed live (Phase 7D): the very first F5 on a genuinely
        // fresh workspace -- pressed as literally the first action,
        // with no prior hover/completion/diagnostics/terminal
        // interaction to let the debug service finish its own
        // initialization -- produces NO observable change at all: no
        // trust dialog, no config QuickPick, no notification, no
        // debug session. This is fail-safe (no session ever starts
        // either way) but is a real one-time "warm-up" quirk, not a
        // security bypass -- documented here rather than hidden. A
        // second F5 always shows the gate correctly.
        const quickPick = await codeFrame.locator('.quick-input-widget').isVisible().catch(() => false);
        const notif = await codeFrame.locator('.notifications-toasts').innerText().catch(() => '');
        log(
          'DEBUG first F5 was a no-op (fail-safe warm-up quirk, not a bypass -- retrying). quickPickVisible:',
          quickPick,
          '| notifications:',
          JSON.stringify(notif.slice(0, 500))
        );
        await page.keyboard.press('F5');
        effectiveGateAppeared = await waitForCondition(
          page,
          'workspace trust dialog appears on retried first debug attempt',
          async () => (await trustDialogLocator.count()) > 0,
          10000
        );
      }
      const stateBeforeDecision = await debugSessionState(codeFrame);
      const targetStartedBeforeTrust = stateBeforeDecision.toolbarVisible || stateBeforeDecision.callStackHasSession;

      // Cancel: the debug target must remain unstarted.
      await codeFrame.getByRole('button', { name: 'Cancel' }).click({ timeout: 5000 }).catch(() => {});
      await page.waitForTimeout(1000);
      const stateAfterCancel = await debugSessionState(codeFrame);
      const targetStartedAfterCancel = stateAfterCancel.toolbarVisible || stateAfterCancel.callStackHasSession;
      const dialogGoneAfterCancel = (await trustDialogLocator.count()) === 0;

      // Attempt debug again -- Cancel must not have silently granted
      // trust; the gate must still be there.
      await page.keyboard.press('F5');
      const gateReappeared = await waitForCondition(
        page,
        'workspace trust dialog reappears after Cancel',
        async () => (await trustDialogLocator.count()) > 0,
        10000
      );

      // Now explicitly trust the folder -- debug must be allowed to
      // proceed normally afterward, with Workspace Trust otherwise
      // untouched (never disabled globally, per the established Phase
      // 7B-10 security decision).
      await codeFrame.getByRole('button', { name: 'Trust Folder & Continue' }).click({ timeout: 5000 });
      const dialogGoneAfterTrust = await waitForCondition(
        page,
        'workspace trust dialog closes after explicit trust',
        async () => (await trustDialogLocator.count()) === 0,
        10000
      );
      const debugProceedsAfterTrust = await waitForCondition(
        page,
        'debug session starts after explicit trust',
        async () => {
          const state = await debugSessionState(codeFrame);
          return state.toolbarVisible || state.callStackHasSession;
        },
        20000
      );

      log(
        'gateAppeared (first press):', gateAppeared,
        '| effectiveGateAppeared:', effectiveGateAppeared,
        '| targetStartedBeforeTrust:', targetStartedBeforeTrust,
        '| dialogGoneAfterCancel:', dialogGoneAfterCancel,
        '| targetStartedAfterCancel:', targetStartedAfterCancel,
        '| gateReappeared:', gateReappeared,
        '| dialogGoneAfterTrust:', dialogGoneAfterTrust,
        '| debugProceedsAfterTrust:', debugProceedsAfterTrust
      );

      return {
        ok: true,
        gateAppeared: effectiveGateAppeared,
        gateAppearedOnFirstPress: gateAppeared,
        targetStartedBeforeTrust,
        dialogGoneAfterCancel,
        targetStartedAfterCancel,
        gateReappeared,
        dialogGoneAfterTrust,
        debugProceedsAfterTrust
      };
    });
  },

  /// Phase 7D Parts 11-12: JSON and Markdown are real, distinct
  /// language features -- not inferred from extension-host stability
  /// or from plain text coloring. Reuses `package.json` and
  /// `README.md`, already part of the fixture, rather than adding new
  /// files. Trust is already granted by the time this scenario runs
  /// (the Rust harness reuses the SAME profile as the already-passed
  /// `full_product_journey`, deliberately -- these are residual
  /// acceptance items, not a repeat of the trust-gate evidence).
  async residual_language_acceptance() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);

      // -- JSON: hover/completion via a LOCAL relative `$schema`
      // reference (`data.json` -> `./schema.json`, both part of the
      // fixture). Real defect found and fixed while writing this
      // scenario: the built-in npm `package.json` schema is fetched
      // remotely from schemastore.org, which this pinned build
      // explicitly refuses in an untrusted workspace ("Downloading
      // schemas is disabled in untrusted workspaces" -- confirmed live
      // via the Problems panel) and which would be blocked by this
      // test environment's own network isolation regardless. A LOCAL
      // relative schema path is a filesystem read, not a "download",
      // so it works deterministically offline and untrusted alike.
      const codeFrame = await openFileWithCode(page, args.folderName, null, 'data.json');
      await codeFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 8000 }).catch(() => {});
      await page.keyboard.press('Control+Home');
      // `greeting` is deliberately the FIRST key in `data.json`
      // (`{"greeting": ...`) precisely so a small, fixed pixel offset
      // reliably lands within it -- confirmed live that the original
      // ordering (`$schema` first) put "greeting" far enough into the
      // line that a small offset instead landed on `$schema`.
      const greetingKey = codeFrame.locator('.monaco-editor .view-line', { hasText: '"greeting"' }).first();
      // A settle delay before the first hover attempt: local schema
      // association/loading is itself async and, confirmed live, was
      // not always ready for the very first hover immediately after
      // the file opens.
      await page.waitForTimeout(2000);
      await greetingKey.hover({ position: { x: 25, y: 8 }, timeout: 10000 }).catch(() => {});
      let lastHoverText = [];
      const jsonHoverShowed = await waitForCondition(
        page,
        'JSON hover shows the local schema description for "greeting"',
        async () => {
          // Re-hover each poll: a hover widget can dismiss itself on
          // its own if the mouse position is judged "stale" between
          // long waits, so a single hover-then-poll isn't reliable.
          await greetingKey.hover({ position: { x: 25, y: 8 }, timeout: 5000 }).catch(() => {});
          lastHoverText = await codeFrame.locator('.monaco-hover').allTextContents().catch(() => []);
          return lastHoverText.some((t) => t.includes('PHASE7D_SCHEMA_GREETING_DESCRIPTION'));
        },
        10000
      );
      if (!jsonHoverShowed) {
        log('DEBUG JSON hover text seen:', JSON.stringify(lastHoverText));
      }
      await page.keyboard.press('Escape');

      // -- JSON: real syntax/validation -- a deliberately broken
      // trailing comma must produce a real diagnostic (Problems panel,
      // the stable observable established in Phase 7C-2 -- not the
      // rendered squiggle), then clear once fixed. Real defect fixed
      // here: `Home`/`Shift+End` selects only the CURRENT line, but
      // after clicking/hovering the cursor's line was not reliably
      // known -- `Control+A` (select the whole document) is robust
      // regardless of cursor position and is verified to have actually
      // replaced the content before checking for a diagnostic.
      await codeFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 8000 }).catch(() => {});
      await page.keyboard.press('Control+A');
      await page.keyboard.press('Delete');
      await page.keyboard.type('{"greeting": "hello", "$schema": "./schema.json",}');
      const editApplied = await waitForCondition(
        page,
        'broken JSON edit actually applied to the editor',
        async () => {
          const text = await codeFrame.locator('.monaco-editor .view-lines').first().innerText().catch(() => '');
          return text.includes('"schema.json",}');
        },
        8000
      );
      await page.keyboard.press('Control+Shift+M');
      let lastMarkerRows = [];
      const jsonDiagnosticAppeared = await waitForCondition(
        page,
        'Problems panel shows a JSON syntax diagnostic for the trailing comma',
        async () => {
          lastMarkerRows = await codeFrame.locator('.markers-panel .monaco-list-row').allTextContents().catch(() => []);
          return lastMarkerRows.some((t) => /comma|expected|unexpected/i.test(t));
        },
        10000
      );
      if (!jsonDiagnosticAppeared) {
        const editorText = await codeFrame.locator('.monaco-editor .view-lines').first().innerText().catch(() => '');
        log(
          'DEBUG editApplied:', editApplied,
          '| marker rows seen:', JSON.stringify(lastMarkerRows),
          '| editor content:', JSON.stringify(editorText)
        );
      }
      // Revert to the original, valid content.
      await codeFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 8000 }).catch(() => {});
      await page.keyboard.press('Control+A');
      await page.keyboard.press('Delete');
      await page.keyboard.type('{"greeting": "hello", "$schema": "./schema.json"}');
      const jsonDiagnosticCleared = await waitForCondition(
        page,
        'Problems panel no longer shows a JSON diagnostic',
        async () => {
          const rows = await codeFrame.locator('.markers-panel .monaco-list-row').allTextContents().catch(() => []);
          return !rows.some((t) => /comma|expected|unexpected/i.test(t));
        },
        10000
      );
      await page.keyboard.press('Control+s');
      await page.keyboard.press('Control+Shift+M');

      // -- Markdown: a real, supported feature -- the built-in preview
      // must render the file's actual heading text, not merely prove
      // the editor opened.
      const mdFrame = await openFileWithCode(page, args.folderName, null, 'README.md');
      await mdFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 8000 }).catch(() => {});
      const markdownPreviewText = async () => {
        const text = await mdFrame.locator('.markdown-preview, .monaco-tokenized-source').allTextContents().catch(() => []);
        return text.some((t) => t.includes('Phase 7 Code fixture'));
      };
      await page.keyboard.press('Control+Shift+V');
      let markdownPreviewShowed = await waitForCondition(
        page,
        'Markdown preview renders the real heading text',
        markdownPreviewText,
        10000
      );
      if (!markdownPreviewShowed) {
        // Real quirk fixed here, the same class already found for the
        // debug-before-trust scenario's first F5: the very first
        // invocation of a keybinding after a fresh file open can be a
        // complete no-op. Fall back to the Command Palette by the
        // command's real title, then retry the keybinding once more.
        await mdFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 8000 }).catch(() => {});
        await page.keyboard.press('Control+Shift+P');
        const paletteInput = mdFrame.locator('.quick-input-widget input');
        await paletteInput.waitFor({ timeout: 8000 }).catch(() => {});
        await page.keyboard.type('Markdown: Open Preview');
        await page.waitForTimeout(700);
        await page.keyboard.press('Enter');
        markdownPreviewShowed = await waitForCondition(
          page,
          'Markdown preview renders the real heading text (retry via Command Palette)',
          markdownPreviewText,
          10000
        );
      }
      if (!markdownPreviewShowed) {
        // Real, confirmed finding (Phase 7D Part 12), root-caused but
        // NOT fixed in this pass: the preview webview never renders
        // because its own resource loads (script/style/base-uri) are
        // blocked by ITS OWN Content-Security-Policy -- confirmed via
        // the browser's CSP violation errors (`consoleErrors`, e.g.
        // "Refused to load the script '.../markdown-language-features/
        // media/index.js' because it violates script-src 'self' ...").
        // Traced directly in the pinned image's own source
        // (`markdown-language-features/dist/extension.js`): this CSP
        // is an IN-DOCUMENT `<meta http-equiv="Content-Security-
        // Policy" content="${uo(m)}">` tag the extension generates
        // itself for its webview HTML body -- not an HTTP response
        // header at all. `crates/orchestrator/src/proxy.rs`'s CSP-
        // header handling (fixed this same pass, see
        // `merge_frame_ancestors_into_csp`) is verified correct but
        // structurally cannot affect this: it only touches response
        // headers, never response bodies. Whatever generates that
        // meta tag's specific allowed origins does not currently
        // include this webview's actual `vscode-resource.vscode-cdn
        // .net` pseudo-origin when served through CloudDesk's reverse
        // proxy -- a real, deeper limitation (extension-generated CSP
        // vs. reverse-proxy-relative resource origins) requiring its
        // own dedicated investigation, well beyond "residual
        // acceptance" scope. Left genuinely failing rather than
        // papered over.
        const tabs = await openTabTitles(mdFrame);
        const bodyHtml = await mdFrame.locator('body').innerHTML().catch((e) => `err: ${e}`);
        log('DEBUG markdown preview not observed. open tabs:', JSON.stringify(tabs));
        log('DEBUG markdown iframe body HTML (first 2000 chars):', bodyHtml.slice(0, 2000));
      }

      log(
        'jsonHoverShowed:', jsonHoverShowed,
        '| jsonDiagnosticAppeared:', jsonDiagnosticAppeared,
        '| jsonDiagnosticCleared:', jsonDiagnosticCleared,
        '| markdownPreviewShowed:', markdownPreviewShowed
      );

      return {
        ok: true,
        jsonHoverShowed,
        jsonDiagnosticAppeared,
        jsonDiagnosticCleared,
        markdownPreviewShowed
      };
    });
  }
};

function terminalWhoamiLooksReal(text) {
  // A real, unprivileged, mapped Linux username -- never literally
  // "root" (Task 15's own explicit requirement) and never empty.
  const lines = text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);
  if (lines.some((l) => /^[a-z_][a-z0-9_-]*$/i.test(l) && l.toLowerCase() !== 'root' && l.length < 40)) {
    return true;
  }
  // Real, separate finding surfaced once terminal capture itself was
  // fixed during Phase 7C (documented in CLAUDE_NIGHTMARE-adjacent
  // Phase 7C report, not fixed in this pass): this image's container
  // runs as a real mapped, non-root UID (confirmed never "root" --
  // Task 15's actual security requirement, upheld), but `/etc/passwd`
  // inside the container has no entry for that UID, so `whoami`/`id`
  // report "cannot find name for user ID <n>" instead of a friendly
  // username. That is still real, deterministic, command-produced
  // output proving the terminal correctly executes commands and
  // returns genuine PTY output -- accept it as valid terminal-acceptance
  // evidence while leaving the underlying passwd-mapping gap open as
  // its own low-severity finding.
  return lines.some((l) => /^whoami: cannot find name for user id \d+$/i.test(l));
}

/// Task 30-33: sets a breakpoint on `console.log(y)`, launches the
/// fixture's real "Debug fixture" configuration (Node, against
/// code-server's own bundled Node binary), verifies execution actually
/// pauses there with a real variable value visible, then continues and
/// verifies the debugger terminates cleanly.
async function runDebugScenario(page, codeFrame) {
  // Open debug.js via Quick Open rather than Files (keeps this focused
  // on the debug UI itself, matching Task 30's "workspace-local
  // fixture" -- already present in the same opened folder).
  await page.keyboard.press('Control+p');
  const quickOpen = codeFrame.locator('.quick-input-widget input');
  await quickOpen.waitFor({ timeout: 10000 });
  await quickOpen.fill('debug.js');
  await page.waitForTimeout(500);
  await page.keyboard.press('Enter');
  await waitForCondition(
    page,
    'debug.js tab visible',
    async () => (await openTabTitles(codeFrame)).some((t) => t.includes('debug.js')),
    10000
  );

  // Set a breakpoint on the console.log(y) line via the gutter.
  // Real defect fixed during Phase 7C-2: `targetLine.boundingBox()` is
  // the CONTENT area (`.view-lines .view-line`, where the code text
  // renders) -- the glyph margin that actually toggles a breakpoint on
  // click is a separate DOM region (`.margin`) to its left, outside
  // that box entirely. `box.x + 5` therefore clicked a few pixels into
  // the text itself (just placing the cursor there), never the gutter,
  // so no breakpoint was ever requested regardless of how long the
  // glyph-visibility check waited. Click within the margin's own
  // bounding box, at the target line's Y coordinate.
  // Real defect fixed during Phase 7C-2: a one-shot `waitFor` +
  // `boundingBox()` raced Monaco's own DOM-node recycling -- the line
  // was genuinely visible when `waitFor` resolved, but Monaco replaced
  // that `.view-line` node moments later during its own render
  // settling, and the very next `boundingBox()` call returned null
  // (confirmed live: `waitFor` never threw, yet `lineBox` logged
  // `null`). Poll `boundingBox()` itself until it succeeds rather than
  // trusting a single snapshot right after `waitFor`.
  const targetLine = codeFrame.locator('.monaco-editor .view-line', { hasText: 'console.log(y)' });
  await targetLine.waitFor({ timeout: 10000 });
  let lineBox = null;
  await waitForCondition(
    page,
    'console.log(y) line has a stable bounding box',
    async () => {
      lineBox = await targetLine.boundingBox().catch(() => null);
      return lineBox !== null;
    },
    8000
  );
  const marginBox = await codeFrame.locator('.monaco-editor .margin').first().boundingBox();
  if (lineBox && marginBox) {
    // Glyph margin is the outer (leftmost) strip of `.margin`, with
    // line numbers to its right -- click near the left edge.
    await page.mouse.click(marginBox.x + 6, lineBox.y + lineBox.height / 2);
  }
  const breakpointGlyphVisible = await waitForCondition(
    page,
    'breakpoint glyph visible',
    async () => (await codeFrame.locator('.codicon-debug-breakpoint, .debug-breakpoint').count()) > 0,
    8000
  );
  if (!breakpointGlyphVisible) {
    const marginHtml = await codeFrame.locator('.monaco-editor .margin').first().innerHTML().catch((e) => `err: ${e}`);
    log('DEBUG margin innerHTML (first 2000 chars):', marginHtml.slice(0, 2000));
    log('DEBUG lineBox:', JSON.stringify(lineBox), 'marginBox:', JSON.stringify(marginBox));
  }

  await page.keyboard.press('F5');
  // Phase 7C-2 Part 9: on a workspace's first-ever debug invocation, F5
  // can show a "Select and Start Debugging" QuickPick instead of
  // launching directly (no configuration is "selected" in the Run and
  // Debug view yet) -- confirm/rule this out before assuming the
  // launch itself is silently failing.
  const debugQuickPick = codeFrame.locator('.quick-input-widget');
  const quickPickAppeared = await waitForCondition(
    page,
    'debug config QuickPick appears (or does not) after F5',
    async () => debugQuickPick.first().isVisible().catch(() => false),
    2500
  );
  if (quickPickAppeared) {
    const items = await codeFrame.locator('.quick-input-widget .monaco-list-row').allTextContents().catch(() => []);
    log('DEBUG QuickPick appeared after F5, items:', JSON.stringify(items));
    const fixtureItem = codeFrame.locator('.quick-input-widget .monaco-list-row', { hasText: 'Debug fixture' });
    if (await fixtureItem.count()) {
      await fixtureItem.first().click({ timeout: 5000 }).catch(() => {});
    } else {
      await page.keyboard.press('Enter');
    }
  }
  // Phase 7C-2 Part 9: trace the pipeline in stages rather than jumping
  // straight to the combined paused predicate, so a failure here is
  // attributable to a specific stage (session created vs. actually
  // reaching/pausing at the breakpoint) instead of one opaque timeout.
  const sessionCreated = await waitForCondition(
    page,
    'debug session/toolbar appears (session created)',
    async () => (await codeFrame.locator('.debug-toolbar').count()) > 0,
    10000
  );
  if (!sessionCreated) {
    const notif = await codeFrame.locator('.notifications-toasts').innerText().catch(() => '');
    log('DEBUG no debug-toolbar after F5. Notifications:', JSON.stringify(notif.slice(0, 1000)));
  }
  const debugPaused = await waitForCondition(
    page,
    'debugger paused at breakpoint',
    async () => {
      const toolbar = await codeFrame.locator('.debug-toolbar').count();
      const stackFrame = await codeFrame.locator('.monaco-editor .debug-top-stack-frame-line').count();
      return toolbar > 0 && stackFrame > 0;
    },
    30000
  );
  if (sessionCreated && !debugPaused) {
    const notif = await codeFrame.locator('.notifications-toasts').innerText().catch(() => '');
    const stillToolbar = await codeFrame.locator('.debug-toolbar').count();
    log(
      'DEBUG session created but never paused. toolbar still present:',
      stillToolbar,
      '| notifications:',
      JSON.stringify(notif.slice(0, 1000))
    );
    // Open the Debug Console explicitly and capture its real output --
    // most informative single observable for "did the target process
    // even start, and what did it print/error."
    await page.keyboard.press('Control+Shift+Y');
    const consoleText = await codeFrame.locator('.repl, .debug-console').first().innerText().catch(() => '');
    log('DEBUG Debug Console content:', JSON.stringify(consoleText.slice(0, 1500)));
  }
  let variableSeen = false;
  if (debugPaused) {
    // Real defect fixed during Phase 7C-2 Part 13: the Variables tree's
    // scope-expansion state proved unreliable across runs (the "Local"
    // scope's auto-expand behavior was inconsistent, and toggling it
    // manually raced the DAP round-trip either way -- confirmed live in
    // both directions). Evaluate the expression directly in the Debug
    // Console REPL instead: standard DAP `evaluate` request against the
    // paused frame, a deterministic, stable observable independent of
    // any tree widget's rendering/expansion quirks.
    await page.keyboard.press('Control+Shift+Y');
    const replInput = codeFrame.locator('.repl textarea, .debug-console textarea, .repl .monaco-editor textarea');
    await replInput.first().click({ timeout: 5000 }).catch(() => {});
    await page.keyboard.type('y');
    await page.keyboard.press('Enter');
    let consoleText = '';
    variableSeen = await waitForCondition(
      page,
      'Debug Console REPL evaluates y to 42',
      async () => {
        consoleText = await codeFrame.locator('.repl, .debug-console').first().innerText().catch(() => '');
        return /(^|\D)42(\D|$)/.test(consoleText);
      },
      10000
    );
    log('debug console after evaluating y:', JSON.stringify(consoleText.slice(-500)));

    // Call stack (Part 13): the paused frame should be visible too.
    const callStackText = await codeFrame.locator('.monaco-pane-view .pane', { hasText: 'Call Stack' }).innerText().catch(() => '');
    log('debug call stack panel:', JSON.stringify(callStackText.slice(0, 500)));
  }

  // Real defect fixed during Phase 7C-2: keyboard focus is left inside
  // the Debug Console REPL's input textarea after evaluating `y` above
  // -- sending the global Continue/Stop keybindings while a text input
  // has focus is exactly the class of focus-routing bug already fixed
  // once for the terminal (Part 6's hard guard). Click back into the
  // editor first, matching the focus discipline used everywhere else
  // in this flow.
  await codeFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 5000 }).catch(() => {});

  // -- Phase 7C-3 SCENARIO A: NATURAL COMPLETION (no explicit Stop).
  //
  // Real defect fixed during Phase 7C-3: the previous predicate asked
  // whether `.debug-toolbar` was still *in the DOM*
  // (`count() === 0`). VS Code hides the floating debug toolbar when
  // no session is active but leaves the element in the DOM, so that
  // predicate could never become true and every "session did not end"
  // conclusion drawn from it was unfounded. That single wrong
  // observable also explains, exactly, why all nine Stop strategies
  // "failed" identically in the prior pass: `debug.js` is three lines
  // with no open handles, so it had already run to completion and the
  // session had already ended by the time Stop was attempted -- the
  // toolbar was hidden (hence its `null` bounding box and every
  // "element is not visible" click error), and `Shift+F5` did nothing
  // because there was no live session left to stop. Nothing was
  // broken; the test was watching the wrong thing. Use visibility plus
  // the Call Stack's own session content as the semantic observable
  // instead of DOM presence.
  await page.keyboard.press('F5');
  const naturalEnd = await waitForCondition(
    page,
    'debug session ends naturally after continue (Scenario A)',
    async () => {
      const state = await debugSessionState(codeFrame);
      return !state.toolbarVisible && !state.callStackHasSession;
    },
    20000
  );
  const finalState = await debugSessionState(codeFrame);
  const replText = await codeFrame.locator('.repl, .debug-console').first().innerText().catch(() => '');
  // The known final marker: `debug.js` prints `42` on its last line.
  const finalMarkerSeen = /(^|\D)42(\D|$)/.test(replText);
  log(
    'ScenarioA natural end:', naturalEnd,
    '| toolbar in DOM:', finalState.toolbarCount,
    '| toolbar visible:', finalState.toolbarVisible,
    '| call stack has session:', finalState.callStackHasSession,
    '| final marker seen:', finalMarkerSeen,
    '| call stack:', JSON.stringify(finalState.callStackText.slice(0, 300))
  );

  return {
    debugPaused: debugPaused && variableSeen,
    debugContinuedOk: naturalEnd,
    debugNaturalEnd: naturalEnd,
    debugFinalMarkerSeen: finalMarkerSeen,
    debugToolbarInDomAfterEnd: finalState.toolbarCount,
    debugToolbarVisibleAfterEnd: finalState.toolbarVisible
  };
}

/// Phase 7C-3 Part 4: a stable, semantic view of whether any debug
/// session is actually active -- deliberately NOT toolbar DOM presence
/// (which persists while hidden) and NOT geometry (a hidden toolbar has
/// no bounding box). Visibility plus the Call Stack's own session
/// content are both driven by real debug-service state.
async function debugSessionState(codeFrame) {
  const toolbar = codeFrame.locator('.debug-toolbar');
  const toolbarCount = await toolbar.count().catch(() => 0);
  const toolbarVisible = toolbarCount > 0 ? await toolbar.first().isVisible().catch(() => false) : false;
  const callStackText = await codeFrame
    .locator('.monaco-pane-view .pane', { hasText: 'Call Stack' })
    .innerText()
    .catch(() => '');
  // While a session is active the Call Stack lists it by its launch
  // configuration name; with no session it shows only the pane header
  // (and, in some states, a "Run and Debug"/start hint).
  const callStackHasSession = /Debug fixture|Debug long fixture|PAUSED|RUNNING/i.test(callStackText);
  return { toolbarCount, toolbarVisible, callStackText, callStackHasSession };
}

/// Phase 7C-3 SCENARIO B (Parts 1/6/17): EXPLICIT Stop against a target
/// that is deterministically STILL ALIVE. Scenario A above can only
/// ever prove auto-termination -- its three-line fixture exits on its
/// own, so a "Stop" issued afterward proves nothing (and, in the prior
/// pass, actively misled). `debug-long.js` holds the event loop open
/// with a timer, so the session is unambiguously live when Stop is
/// invoked here.
async function runDebugStopScenario(page, codeFrame) {
  // Start the long-running configuration through a real user-visible
  // interaction. Part 6: do not invent a command ID -- drive VS Code's
  // own Command Palette by the command's user-facing title, then pick
  // the configuration by its real name.
  await page.keyboard.press('Control+Shift+P');
  const paletteInput = codeFrame.locator('.quick-input-widget input');
  await paletteInput.waitFor({ timeout: 10000 }).catch(() => {});
  // Real defect fixed during Phase 7C-3: `Control+Shift+P` opens the
  // quick input pre-seeded with the Command Palette's `>` prefix, and
  // `fill()` replaces the WHOLE value -- wiping that prefix and
  // silently turning the palette into a plain file search, so the
  // command never ran at all (confirmed live: the picker returned
  // "No matching results", then plain file results once the filter was
  // cleared -- both file-search behaviour, never a command list).
  // Type instead of filling, so the `>` prefix survives.
  await page.keyboard.type('Debug: Select and Start Debugging');
  const commandRow = codeFrame.locator('.quick-input-widget .monaco-list-row', { hasText: 'Select and Start Debugging' });
  const commandOffered = await waitForCondition(
    page,
    'Debug: Select and Start Debugging command offered in palette',
    async () => (await commandRow.count()) > 0,
    10000
  );
  if (!commandOffered) {
    const items = await codeFrame.locator('.quick-input-widget .monaco-list-row').allTextContents().catch(() => []);
    log('DEBUG ScenarioB palette items (command step):', JSON.stringify(items.slice(0, 10)));
  }
  await page.keyboard.press('Enter');
  await page.waitForTimeout(700);
  const longConfigItem = codeFrame.locator('.quick-input-widget .monaco-list-row', { hasText: 'Debug long fixture' });
  const configListed = await waitForCondition(
    page,
    'long-running debug configuration offered',
    async () => (await longConfigItem.count()) > 0,
    10000
  );
  if (!configListed) {
    const items = await codeFrame.locator('.quick-input-widget .monaco-list-row').allTextContents().catch(() => []);
    log('DEBUG ScenarioB config picker items:', JSON.stringify(items));
  }
  await longConfigItem.first().click({ timeout: 5000 }).catch((e) => log('DEBUG ScenarioB config click threw:', String(e)));

  // The long fixture prints its marker and then stays alive on a timer.
  const started = await waitForCondition(
    page,
    'long-running debug session started and printed its marker',
    async () => {
      const replText = await codeFrame.locator('.repl, .debug-console').first().innerText().catch(() => '');
      return replText.includes('LONG_FIXTURE_MARKER');
    },
    30000
  );
  const aliveState = await debugSessionState(codeFrame);
  log(
    'ScenarioB started:', started,
    '| toolbar visible (target alive):', aliveState.toolbarVisible,
    '| call stack has session:', aliveState.callStackHasSession,
    '| call stack:', JSON.stringify(aliveState.callStackText.slice(0, 300))
  );
  const targetAlive = started && aliveState.toolbarVisible && aliveState.callStackHasSession;

  // Part 7: establish and record focus context before invoking Stop,
  // so a non-firing keybinding is attributable to input routing rather
  // than to the debug service.
  await codeFrame.locator('.monaco-editor .view-lines').first().click({ timeout: 5000 }).catch(() => {});
  const focusInfo = await codeFrame
    .locator('body')
    .evaluate(() => {
      const el = document.activeElement;
      return {
        activeTag: el ? el.tagName : null,
        activeClass: el ? String(el.className).slice(0, 120) : null,
        modalCount: document.querySelectorAll('.monaco-dialog-box, .quick-input-widget:not([style*="display: none"])').length
      };
    })
    .catch(() => null);
  log('ScenarioB focus before Stop:', JSON.stringify(focusInfo));

  // Now the target is definitely alive, so the toolbar is genuinely
  // visible and its Stop action is genuinely clickable -- this is the
  // real Stop acceptance the prior pass could never actually perform.
  let stopPerformed = false;
  const stopButton = codeFrame.locator('.debug-toolbar .action-container[role="button"]', {
    has: codeFrame.locator('[aria-label^="Stop" i]')
  });
  if (await stopButton.count().catch(() => 0)) {
    await stopButton
      .first()
      .click({ timeout: 5000 })
      .then(() => {
        stopPerformed = true;
      })
      .catch((e) => log('DEBUG ScenarioB stop click threw:', String(e)));
  }
  if (!stopPerformed) {
    log('DEBUG ScenarioB falling back to Shift+F5 keybinding for Stop');
    await page.keyboard.press('Shift+F5');
    stopPerformed = true;
  }

  const stopped = await waitForCondition(
    page,
    'debug session terminates after explicit Stop (Scenario B)',
    async () => {
      const state = await debugSessionState(codeFrame);
      return !state.toolbarVisible && !state.callStackHasSession;
    },
    20000
  );
  const afterStop = await debugSessionState(codeFrame);
  log(
    'ScenarioB stopped:', stopped,
    '| toolbar visible:', afterStop.toolbarVisible,
    '| call stack has session:', afterStop.callStackHasSession,
    '| call stack:', JSON.stringify(afterStop.callStackText.slice(0, 300))
  );

  return { debugLongTargetAlive: targetAlive, debugExplicitStopOk: stopped };
}

const fn = scenarios[scenario];
if (!fn) {
  console.log(JSON.stringify({ ok: false, error: `unknown scenario: ${scenario}` }));
  process.exit(1);
}
fn()
  .then((result) => console.log(JSON.stringify(result)))
  .catch((error) => {
    log('FATAL', String(error && error.stack ? error.stack : error));
    console.log(JSON.stringify({ ok: false, error: String(error) }));
  });
