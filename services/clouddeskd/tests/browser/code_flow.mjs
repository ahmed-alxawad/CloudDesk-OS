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
      const terminalText = codeFrame.locator('.xterm-screen, .xterm-accessible-buffer, .terminal-wrapper');
      const terminalReady = await waitForCondition(
        page,
        'terminal shell ready (prompt content present)',
        async () => {
          const t = await terminalText.first().innerText().catch(() => '');
          return t.trim().length > 0;
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
      await page.keyboard.type('const badType: number = "not a number";');
      const diagnosticAppeared = await waitForCondition(
        page,
        'squiggly error decoration appears',
        async () => (await codeFrame.locator('.squiggly-error').count()) > 0,
        15000
      );
      // Select the whole line and delete it.
      await page.keyboard.press('Home');
      await page.keyboard.down('Shift');
      await page.keyboard.press('End');
      await page.keyboard.up('Shift');
      await page.keyboard.press('Delete');
      await page.keyboard.press('Backspace');
      const diagnosticCleared = await waitForCondition(
        page,
        'squiggly error decoration clears',
        async () => (await codeFrame.locator('.squiggly-error').count()) === 0,
        15000
      );
      await page.keyboard.press('Control+s');
      await waitForCondition(
        page,
        'tab clean after final save',
        async () => (await codeFrame.locator('.tabs-container .tab.dirty').count()) === 0,
        10000
      );

      // -- Task 14-17 (continued): the terminal from earlier (trust
      // already resolved deliberately, before hover/completion/
      // diagnostics above) is still open -- refocus it and continue.
      await codeFrame.locator('.xterm').first().click({ timeout: 8000 }).catch(() => {});
      termOutput = await terminalText.first().innerText().catch(() => '');

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

      await page.keyboard.type('git status --porcelain');
      await page.keyboard.press('Enter');
      await waitForCondition(
        page,
        'git status output shows example.ts',
        async () => {
          termOutput = await terminalText.first().innerText().catch(() => '');
          return termOutput.includes('example.ts');
        },
        10000
      );
      const gitStatusShowedFile = termOutput.includes('example.ts');
      log('terminal after git status:', JSON.stringify(termOutput.slice(-400)));

      // Close the terminal panel before debugging to reduce UI clutter.
      await page.keyboard.press('Control+`');
      await page.waitForTimeout(500);

      // -- Task 30-33: real local debug session.
      const debugResult = await runDebugScenario(page, codeFrame);

      // -- Task 12: close and reopen, confirm persisted edit.
      await page.locator('.tabs-container .tab .label-name', { hasText: 'example.ts' }).first().click({ timeout: 8000 }).catch(() => {});
      await page.keyboard.press('Control+w').catch(() => {});
      await openLauncherApp(page, 'Files');
      await waitForListingLoaded(page, 15000);
      await page.locator('.file-list button', { hasText: args.folderName }).first().dblclick({ timeout: 8000 });
      await waitForListingLoaded(page, 15000);
      await page.locator('.file-list button', { hasText: 'src' }).first().dblclick({ timeout: 8000 });
      await waitForListingLoaded(page, 15000);
      await page.locator('.file-list button', { hasText: 'example.ts' }).first().click({ timeout: 8000 });
      await page.locator('button', { hasText: 'Open with Code' }).click({ timeout: 8000 });
      const reopenedPersisted = await waitForCondition(
        page,
        'sentinel still present after reopen',
        async () => {
          const text = await page
            .frameLocator('.code-app iframe')
            .locator('.monaco-editor .view-lines')
            .first()
            .innerText()
            .catch(() => '');
          return text.includes(args.sentinel);
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
  }
};

function terminalWhoamiLooksReal(text) {
  // A real, unprivileged, mapped Linux username -- never literally
  // "root" (Task 15's own explicit requirement) and never empty.
  const lines = text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);
  return lines.some((l) => /^[a-z_][a-z0-9_-]*$/i.test(l) && l.toLowerCase() !== 'root' && l.length < 40);
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
  const targetLine = codeFrame.locator('.monaco-editor .view-line', { hasText: 'console.log(y)' });
  await targetLine.waitFor({ timeout: 10000 });
  const box = await targetLine.boundingBox();
  if (box) {
    await page.mouse.click(box.x + 5, box.y + box.height / 2);
  }
  await waitForCondition(
    page,
    'breakpoint glyph visible',
    async () => (await codeFrame.locator('.codicon-debug-breakpoint, .debug-breakpoint').count()) > 0,
    8000
  );

  await page.keyboard.press('F5');
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
  let variableSeen = false;
  if (debugPaused) {
    const varsText = await codeFrame.locator('.debug-variables').innerText().catch(() => '');
    log('debug variables panel:', JSON.stringify(varsText.slice(0, 500)));
    variableSeen = varsText.includes('x') || varsText.includes('21');
  }

  // Continue -- program finishes, session ends cleanly.
  await page.keyboard.press('F5');
  const debugContinuedOk = await waitForCondition(
    page,
    'debug session ends after continue',
    async () => (await codeFrame.locator('.debug-toolbar').count()) === 0,
    20000
  );

  return { debugPaused: debugPaused && variableSeen, debugContinuedOk };
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
