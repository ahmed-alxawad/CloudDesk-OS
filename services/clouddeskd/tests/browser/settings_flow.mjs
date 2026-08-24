// Phase 6 browser-acceptance evidence harness. Runs inside a
// disposable, version-pinned Playwright/Chromium container (test
// infrastructure only). Drives the ACTUAL compiled CloudDesk
// frontend: login form, the app launcher, the real SettingsApp.svelte
// runtime cards, and (for a real launch check) BrowserApp.svelte /
// CodeApp.svelte / FilesApp.svelte + OfficeApp.svelte.
//
// Split into one scenario per lifecycle phase (enable+launch / disable
// / re-enable+launch) rather than one giant scenario, so the Rust
// caller can check real Docker container state (0 resident while
// disabled) at exactly the right moment between phases -- a single
// continuous scenario would still be mid-relaunch (a real, expected
// running container) by the time control returned to Rust.
//
// Usage: node settings_flow.mjs <scenario> <jsonArgsFile>

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
  const toggle = page.getByRole('button', { name: 'Open application launcher' });
  await toggle.click();
  const button = page.locator('.launcher-grid button', { hasText: appName }).first();
  const expanded = await toggle.getAttribute('aria-expanded').catch((e) => `err: ${e}`);
  const cls = await button.getAttribute('class').catch((e) => `err: ${e}`);
  log(`DEBUG openLauncherApp(${appName}): aria-expanded=${expanded} buttonClass=${cls}`);
  await button.click({ timeout: 8000 });
  const notif = await page.locator('.notification p').innerText().catch(() => null);
  if (notif) log(`DEBUG notification after clicking ${appName}: ${notif}`);
}

async function openSettings(page) {
  await openLauncherApp(page, 'Settings');
  await page.locator('[data-testid="runtime-cards"]').waitFor({ timeout: 15000 });
  log('Settings opened, runtime cards loaded');
}

function runtimeCard(page, displayName) {
  return page.locator('[data-testid="runtime-cards"] article', { has: page.locator('h3', { hasText: displayName }) });
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

async function cardStatus(page, displayName) {
  return runtimeCard(page, displayName).locator('.runtime-status').getAttribute('data-status');
}

/// Toggles a runtime card (Enable/Disable) and waits for the card's
/// data-status attribute to reflect the requested end state -- the
/// real product's own polling/refetch (loadRuntimes() after every
/// mutation), never a blind sleep.
async function toggleAndWait(page, displayName, expectStatus) {
  await runtimeCard(page, displayName).locator('button').click();
  return waitForCondition(
    page,
    `${displayName} reaches ${expectStatus}`,
    async () => (await cardStatus(page, displayName)) === expectStatus,
    30000
  );
}

async function launchBrowserAndWait(page, timeoutMs) {
  await openLauncherApp(page, 'Browser');
  // A previously-open Browser window (App.svelte retargets rather than
  // recreates it) can land on a real, intended 'disconnected' state
  // with an explicit Reconnect affordance instead of auto-reconnecting
  // -- click it, matching what a real user would do. A brand-new
  // window (first-ever launch this session) never shows this, so this
  // is a short opportunistic check, not a precondition the normal
  // starting/checking phase would ever need to satisfy.
  const reconnectButton = page.locator('.browser-status button', { hasText: 'Reconnect' });
  try {
    await reconnectButton.waitFor({ timeout: 3000 });
    await reconnectButton.click();
    log('clicked Reconnect');
  } catch {
    // no Reconnect affordance shown -- a normal first launch.
  }
  const launched = await waitForCondition(
    page,
    'Browser reaches a running remote-page canvas',
    async () => (await page.locator('canvas[aria-label="Remote browser page"]').count()) > 0,
    timeoutMs
  );
  if (!launched) {
    const statusText = await page.locator('.browser-status').first().innerText().catch((e) => `err: ${e}`);
    log('DEBUG browser-status:', statusText);
    const appCount = await page.locator('.browser-app').count();
    log('DEBUG .browser-app count:', appCount);
    const windowTitles = await page.locator('.window .window-bar strong').allTextContents().catch((e) => `err: ${e}`);
    log('DEBUG open window titles:', JSON.stringify(windowTitles));
    const windowCount = await page.locator('.window').count();
    log('DEBUG window count:', windowCount);
  }
  return launched;
}

async function launchCodeAndWait(page, timeoutMs) {
  await openLauncherApp(page, 'Code');
  return waitForCondition(
    page,
    'Code reaches a running iframe',
    async () => (await page.locator('.code-app iframe').count()) > 0,
    timeoutMs
  );
}

async function openOfficeDocAndWait(page, timeoutMs) {
  await openLauncherApp(page, 'Files');
  await page.locator('.file-list[aria-busy="false"]').waitFor({ timeout: 15000 });
  await page.locator('.file-list button', { hasText: args.folderName }).first().dblclick({ timeout: 8000 });
  await page.locator('.file-list[aria-busy="false"]').waitFor({ timeout: 15000 });
  await page.locator('.file-list button', { hasText: args.officeFileName }).first().waitFor({ timeout: 8000 });
  await page.locator('.file-list button', { hasText: args.officeFileName }).first().dblclick({ timeout: 8000 });
  return waitForCondition(
    page,
    'Office reaches a real editor iframe',
    async () => (await page.locator('iframe[title^="Office"]').count()) > 0,
    timeoutMs
  );
}

const scenarios = {
  // -- cards inventory (Task 3) --
  async settings_cards_visible() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const allThreeCardsVisible =
        (await runtimeCard(page, 'Browser Runtime').count()) > 0 &&
        (await runtimeCard(page, 'Code Runtime').count()) > 0 &&
        (await runtimeCard(page, 'Office Runtime').count()) > 0;
      return { ok: true, allThreeCardsVisible };
    });
  },

  // -- Browser (Task 5/6) --
  async browser_enable_and_launch() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const enabled = await toggleAndWait(page, 'Browser Runtime', 'Enabled');
      const launched = await launchBrowserAndWait(page, 60000);
      return { ok: true, enabled, launched };
    });
  },
  async browser_disable() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const disabled = await toggleAndWait(page, 'Browser Runtime', 'Disabled');
      return { ok: true, disabled };
    });
  },
  async browser_reenable_and_launch() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const enabled = await toggleAndWait(page, 'Browser Runtime', 'Enabled');
      const launched = await launchBrowserAndWait(page, 60000);
      return { ok: true, enabled, launched };
    });
  },

  // -- Code (Task 7/8) --
  async code_enable_and_launch() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const enabled = await toggleAndWait(page, 'Code Runtime', 'Enabled');
      const launched = await launchCodeAndWait(page, 60000);
      return { ok: true, enabled, launched };
    });
  },
  async code_disable() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const disabled = await toggleAndWait(page, 'Code Runtime', 'Disabled');
      return { ok: true, disabled };
    });
  },
  async code_reenable_and_launch() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const enabled = await toggleAndWait(page, 'Code Runtime', 'Enabled');
      const launched = await launchCodeAndWait(page, 60000);
      return { ok: true, enabled, launched };
    });
  },

  // -- Office (Task 9/10) --
  async office_enable_and_open() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const enabled = await toggleAndWait(page, 'Office Runtime', 'Enabled');
      const opened = await openOfficeDocAndWait(page, 60000);
      return { ok: true, enabled, opened };
    });
  },
  async office_disable() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const disabled = await toggleAndWait(page, 'Office Runtime', 'Disabled');
      return { ok: true, disabled };
    });
  },
  async office_reenable_and_open() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const enabled = await toggleAndWait(page, 'Office Runtime', 'Enabled');
      const opened = await openOfficeDocAndWait(page, 60000);
      return { ok: true, enabled, opened };
    });
  },

  // -- Task 4 --
  async non_admin_no_runtime_controls() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openSettings(page);
      const cardsVisible =
        (await runtimeCard(page, 'Browser Runtime').count()) > 0 &&
        (await runtimeCard(page, 'Code Runtime').count()) > 0 &&
        (await runtimeCard(page, 'Office Runtime').count()) > 0;
      const anyButtons = await page.locator('[data-testid="runtime-cards"] article button').count();
      return { ok: true, cardsVisibleWithoutControls: cardsVisible && anyButtons === 0 };
    });
  }
};

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
