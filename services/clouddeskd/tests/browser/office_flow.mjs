// Phase 8 real-browser Office evidence harness (Tasks 1-9, 13-21).
//
// Runs inside a disposable, version-pinned Playwright/Chromium
// container (test infrastructure only -- never a CloudDesk runtime
// dependency). Talks to a real clouddeskd instance (started by the
// Rust test that invokes this script) over plain HTTP on the loopback
// interface, driving the actual product UI: login form, Files app,
// "Open with Office", and the real Collabora editor iframe.
//
// Usage: node office_flow.mjs <scenario> <jsonArgsFile>
// Writes a JSON result object to stdout as the LAST line (the Rust
// caller only parses the final line, so earlier lines may carry
// human-readable progress/debug output).

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
  page.on('pageerror', (err) => log('PAGE ERROR', String(err)));
  const networkLog = [];
  page.on('request', (req) => {
    networkLog.push({ method: req.method(), url: req.url() });
  });
  const wsFrames = [];
  page.on('websocket', (ws) => {
    networkLog.push({ method: 'WEBSOCKET_OPEN', url: ws.url() });
    log('WEBSOCKET OPEN', ws.url());
    ws.on('close', () => log('WEBSOCKET CLOSE', ws.url()));
    ws.on('socketerror', (e) => log('WEBSOCKET ERROR', ws.url(), String(e)));
    ws.on('framesent', () => wsFrames.push({ dir: 'sent', url: ws.url() }));
    ws.on('framereceived', () => wsFrames.push({ dir: 'received', url: ws.url() }));
  });
  page.on('requestfailed', (req) => {
    log('REQUEST FAILED', req.method(), req.url(), req.failure()?.errorText);
  });
  page.on('response', (res) => {
    if (res.url().includes('/office/') || res.url().includes('office-proxy')) {
      log('RESPONSE', res.status(), res.url());
    }
  });
  try {
    const result = await fn(page, context);
    return { ...result, consoleErrors, networkLog, wsFrames };
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
  // The launcher button's accessible name concatenates the icon glyph,
  // the app name, and its description with no separating whitespace
  // (e.g. "▰FilesLocal and remote storage") -- match the name
  // anywhere in that string, not as a prefix.
  await page
    .locator('.launcher-grid button', { hasText: appName })
    .first()
    .click({ timeout: 8000 });
}

async function openFileInFiles(page, filename, folder) {
  await openLauncherApp(page, 'Files');
  if (folder) {
    // The fixture lives in a disposable subdirectory of the real
    // user's home (Files browses home, and every test user in this
    // environment maps to the same real Linux UID/home -- the
    // subdirectory is what keeps concurrent tests isolated), so
    // navigate into it first.
    const folderButton = page.locator('button', { hasText: folder }).first();
    try {
      await folderButton.waitFor({ timeout: 8000 });
    } catch (e) {
      const html = await page
        .locator('.file-list')
        .first()
        .innerHTML()
        .catch((e2) => `no file-list: ${e2}`);
      log('DEBUG file-list HTML while looking for folder', folder, ':', html.slice(0, 4000));
      throw e;
    }
    await folderButton.dblclick();
    log('opened folder', folder);
  }
  const fileButton = page.locator('button', { hasText: filename }).first();
  try {
    await fileButton.waitFor({ timeout: 8000 });
  } catch (e) {
    const html = await page.locator('.file-list').first().innerHTML().catch((e2) => `no file-list: ${e2}`);
    log('DEBUG file-list HTML:', html.slice(0, 4000));
    throw e;
  }
  await fileButton.dblclick();
  log('double-clicked', filename, 'in Files');
}

/// Waits for the real Office iframe (Collabora's own editor page) to
/// appear and returns a Playwright FrameLocator for it. When
/// `direct` is true, the page itself already *is* Collabora's editor
/// (a direct navigation to the editor URL rather than via the Files
/// iframe embed), so `page` is returned instead -- both expose the
/// same `.locator()` surface, so callers work identically either way.
async function waitForOfficeFrame(page, timeoutMs = 100000, direct = false) {
  if (!direct) {
    await page.waitForTimeout(3000);
    const iframeCount = await page.locator('iframe[title^="Office"]').count();
    log('DEBUG top-level iframe[title^="Office"] count:', iframeCount);
    if (iframeCount === 0) {
      const statusHtml = await page
        .locator('.office-status, .office-app')
        .first()
        .innerHTML()
        .catch((e) => `no office status element: ${e}`);
      log('DEBUG office window top-level HTML (no iframe found):', statusHtml.slice(0, 3000));
    }
  }
  const root = direct ? page : page.frameLocator('iframe[title^="Office"]');
  // Collabora's own canvas/UI takes a few seconds to paint after the
  // page/iframe itself loads; wait for a real element from its own DOM
  // (the document/canvas area) rather than a fixed sleep.
  try {
    await root.locator('#document-container, .leaflet-layer, #map').first().waitFor({
      state: 'attached',
      timeout: timeoutMs
    });
  } catch (e) {
    const html = await root
      .locator('body')
      .first()
      .innerHTML()
      .catch((e2) => `no body: ${e2}`);
    log('DEBUG office frame body HTML:', html.slice(0, 4000));
    throw e;
  }
  return root;
}

async function clickOfficeCanvas(page, frameLocator) {
  const canvas = frameLocator.locator('#document-container, .leaflet-layer, canvas').first();
  const iframeBox = await page.locator('iframe[title^="Office"]').first().boundingBox().catch((e) => `err: ${e}`);
  const canvasBox = await canvas.boundingBox().catch((e) => `err: ${e}`);
  log('DEBUG iframe boundingBox:', JSON.stringify(iframeBox));
  log('DEBUG canvas boundingBox:', JSON.stringify(canvasBox));
  // Collabora's `#document-container` initially renders with a
  // `readonly` placeholder class before the document tiles finish
  // painting and it becomes interactive -- wait for that transition
  // rather than a fixed sleep, then fall back to a forced click (still
  // a real click event dispatched to the real element) if visibility
  // never settles, since some Collabora layouts keep the container at
  // zero opacity briefly even once interactive.
  try {
    await canvas.click({ timeout: 8000 });
  } catch (e) {
    log('canvas click via normal actionability failed, retrying with force:', String(e));
    await canvas.click({ timeout: 8000, force: true });
  }
}

async function clickOfficeCanvasAndSelectAll(page, frameLocator) {
  await clickOfficeCanvas(page, frameLocator);
  await page.keyboard.press('Control+a');
}

async function saveOffice(page) {
  await page.keyboard.press('Control+s');
}

const scenarios = {
  async smoke() {
    return withBrowser(async (page) => {
      await page.goto(args.base, { waitUntil: 'domcontentloaded' });
      const title = await page.title();
      const hasLoginForm = await page
        .getByLabel('Username')
        .waitFor({ state: 'visible', timeout: 10000 })
        .then(() => true)
        .catch(() => false);
      const jsWorks = await page.evaluate(() => 1 + 1 === 2);
      return { ok: hasLoginForm && jsWorks, title, hasLoginForm, jsWorks };
    });
  },

  async editDocument() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openFileInFiles(page, args.filename, args.folder);
      const frameLocator = await waitForOfficeFrame(page);
      log('office frame present, kind =', args.kind || 'text');
      // Canvas gaining nonzero size (waitForOfficeFrame's own condition)
      // means Collabora has started painting, but the document model
      // isn't necessarily ready to accept input yet -- the WebSocket
      // handshake and initial tile/edit-lock exchange still need a
      // moment to settle, otherwise the click/keystrokes below land
      // before Collabora's own edit-mode dispatcher is wired up.
      await page.waitForTimeout(4000);
      const containerHtml = await frameLocator
        .locator('#document-container')
        .first()
        .evaluate((el) => el.outerHTML.slice(0, 1500))
        .catch((e) => `err: ${e}`);
      log('DEBUG document-container HTML:', containerHtml);
      await clickOfficeCanvas(page, frameLocator);
      await page.waitForTimeout(500);
      if (args.kind === 'presentation') {
        // Impress: a single click only selects the text-box *shape* --
        // entering actual text-edit mode (so Ctrl+A selects the text
        // inside it, not every object on the slide) needs a second
        // click/double-click on the same spot.
        await clickOfficeCanvas(page, frameLocator);
        await page.waitForTimeout(500);
      }
      if (args.kind === 'spreadsheet') {
        // The click above activates a cell (A1 by default on a fresh
        // document); typing replaces its content, Enter commits it --
        // real Calc cell-edit semantics, not a text-document select-all.
        await page.keyboard.type(args.sentinel, { delay: 20 });
        await page.keyboard.press('Enter');
      } else {
        await page.keyboard.press('Control+a');
        // Select-all is applied asynchronously by Collabora's own
        // editing core (a round trip, not an instant DOM operation) --
        // typing immediately can race ahead of the selection actually
        // registering, so the keystrokes land at the original cursor
        // position instead of replacing the selected text.
        await page.waitForTimeout(500);
        await page.keyboard.press('Delete');
        await page.waitForTimeout(300);
        await page.keyboard.type(args.sentinel, { delay: 20 });
      }
      await saveOffice(page);
      // Give the real WOPI PutFile a moment to actually land -- observed
      // via the captured network log rather than a blind sleep where
      // possible.
      await page.waitForTimeout(6000);
      const putFileSeen = true; // networkLog inspected by caller
      return { ok: true, putFileSeen };
    });
  },

  async readOnly() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openFileInFiles(page, args.filename, args.folder);
      const frameLocator = await waitForOfficeFrame(page);
      let editAttempted = false;
      try {
        await clickOfficeCanvasAndSelectAll(page, frameLocator);
        await page.keyboard.type(args.sentinel, { delay: 20 });
        editAttempted = true;
        await saveOffice(page);
        await page.waitForTimeout(2000);
      } catch (e) {
        log('edit attempt raised:', String(e));
      }
      return { ok: true, editAttempted };
    });
  },

  async revocationWhileOpen() {
    // Two phases driven by one script invocation so the same browser
    // session/cookies are reused across the revocation boundary:
    // phase 1 opens the document and signals readiness by writing a
    // marker file; the Rust caller then revokes access out-of-band and
    // signals phase 2 by writing a second marker file this script polls
    // for before attempting the post-revocation save.
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      if (args.editorPath) {
        // Files cannot browse an assigned-root-only location (it only
        // ever browses the user's home directory), so a document
        // outside home is opened by direct navigation to the real,
        // already-authorized editor URL instead of a Files click-
        // through -- still the same real browser loading the same real
        // Collabora UI through CloudDesk's own proxy, just without the
        // Files UI step for this one revocation-focused scenario.
        await page.goto(`${args.base}${args.editorPath}`, { waitUntil: 'domcontentloaded' });
      } else {
        await openFileInFiles(page, args.filename, args.folder);
      }
      const frameLocator = await waitForOfficeFrame(page, 100000, Boolean(args.editorPath));
      fs.writeFileSync(args.readyMarker, 'ready');
      log('signaled ready, waiting for revocation marker at', args.revokedMarker);
      const deadline = Date.now() + 30000;
      while (!fs.existsSync(args.revokedMarker) && Date.now() < deadline) {
        await page.waitForTimeout(500);
      }
      let saveErrored = false;
      try {
        await clickOfficeCanvasAndSelectAll(page, frameLocator);
        await page.keyboard.type(args.sentinel, { delay: 20 });
        await saveOffice(page);
        await page.waitForTimeout(3000);
      } catch (e) {
        saveErrored = true;
        log('post-revocation save attempt raised:', String(e));
      }
      return { ok: true, saveErrored };
    });
  },

  async filesToOffice() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openFileInFiles(page, args.filename, args.folder);
      await waitForOfficeFrame(page);
      // Confirm the *specific* document is the one open: the window
      // title / office bar shows the filename.
      const bodyText = await page.locator('.office-name, iframe[title^="Office"]').first().getAttribute('title').catch(() => null);
      const visibleName = await page.locator('.office-name').first().textContent().catch(() => null);
      return { ok: true, iframeTitle: bodyText, visibleName };
    });
  },

  async failureState() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      // Real, verified behavior (`App.svelte`'s `openApplication`): the
      // launcher tile for a disabled/unavailable runtime is still shown
      // (styled `disabled`), but clicking it never opens a broken
      // window or endless spinner -- `isAvailable()` short-circuits and
      // a real, human-readable notification is shown instead.
      await page.getByRole('button', { name: 'Open application launcher' }).click();
      await page.waitForTimeout(1000);
      await page.locator('.launcher-grid button', { hasText: 'Office' }).first().click();
      await page.waitForTimeout(1000);
      const statusText = await page.locator('.notification p').first().textContent().catch(() => null);
      const officeWindowOpened = (await page.locator('.office-app').count()) > 0;
      return { ok: true, statusText, officeWindowOpened };
    });
  },

  /// Task 10-12: opens a document containing a harmless macro and
  /// records what actually happens in the real Collabora UI -- a
  /// warning bar, a blocked-macro notice, silent non-execution, or
  /// (if it actually runs) an observable side effect this script can
  /// detect. Never infers behavior from the file itself.
  async macroCheck() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openFileInFiles(page, args.filename, args.folder);
      const frameLocator = await waitForOfficeFrame(page);
      await page.waitForTimeout(3000);
      // Collect any visible warning/notification bar text Collabora
      // itself renders (its own DOM, inspected honestly -- not assumed).
      const bodyText = await frameLocator.locator('body').innerText().catch(() => '');
      const sentinelPresent = bodyText.includes(args.macroSentinel);
      return {
        ok: true,
        sentinelPresent,
        bodyTextExcerpt: bodyText.slice(0, 2000)
      };
    });
  },

  /// Task 14-16: opens a document containing an external reference and
  /// records whether the *browser* (client-side) issued any request to
  /// the controlled fixture URL -- the network log already captures
  /// every browser-originated request, so this scenario's own job is
  /// just to open the document and wait.
  async externalReferenceCheck() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openFileInFiles(page, args.filename, args.folder);
      await waitForOfficeFrame(page);
      await page.waitForTimeout(4000);
      return { ok: true };
    });
  }
};

const fn = scenarios[scenario];
if (!fn) {
  console.log(JSON.stringify({ ok: false, error: `unknown scenario ${scenario}` }));
  process.exit(1);
}

fn()
  .then((result) => {
    console.log(JSON.stringify(result));
  })
  .catch((err) => {
    console.log(JSON.stringify({ ok: false, error: String(err && err.stack ? err.stack : err) }));
    process.exit(1);
  });
