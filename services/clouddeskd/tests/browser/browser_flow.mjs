// Phase 9 Pass 3A-2 real-browser Browser-app evidence harness (Tasks
// 1-4). Runs inside a disposable, version-pinned Playwright/Chromium
// container (test infrastructure only -- never a CloudDesk runtime
// dependency). Talks to a real clouddeskd instance (started by the
// Rust test that invokes this script) over plain HTTP on the loopback
// interface, driving the ACTUAL product UI: login form, the app
// launcher, and the real compiled BrowserApp.svelte -- never Brave CDP
// directly, never the broker's WebSocket protocol from this script's
// own code (Playwright only ever clicks/types into the real page).
//
// Usage: node browser_flow.mjs <scenario> <jsonArgsFile>
// Writes a JSON result object to stdout as the LAST line.

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
  page.on('websocket', (ws) => {
    log('WEBSOCKET OPEN', ws.url());
    ws.on('close', () => log('WEBSOCKET CLOSE', ws.url()));
  });
  try {
    const result = await fn(page, context);
    return { ...result, consoleErrors, networkLog };
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

async function openBrowserApp(page) {
  await page.getByRole('button', { name: 'Open application launcher' }).click();
  await page.locator('.launcher-grid button', { hasText: 'Browser' }).first().click({ timeout: 8000 });
  await page.locator('canvas[aria-label="Remote browser page"]').waitFor({ timeout: 30000 });
  log('Browser app opened, canvas present');
}

// Waits for the canvas to contain real, non-blank pixel data -- proof
// a real screencast frame was decoded and drawn (Task 1/3), not merely
// that the DOM element exists.
async function waitForNonBlankCanvas(page, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const hasContent = await page.evaluate(() => {
      const canvas = document.querySelector('canvas[aria-label="Remote browser page"]');
      if (!canvas) return false;
      const ctx = canvas.getContext('2d');
      const { width, height } = canvas;
      if (!width || !height) return false;
      const data = ctx.getImageData(0, 0, width, height).data;
      for (let i = 0; i < data.length; i += 4) {
        if (data[i] !== 0 || data[i + 1] !== 0 || data[i + 2] !== 0) return true;
      }
      return false;
    });
    if (hasContent) return true;
    await page.waitForTimeout(500);
  }
  return false;
}

// Translates a Brave-viewport-space coordinate (the coordinate space
// the fixture page's own elements are laid out in) into a real
// on-screen click via the canvas's actual current CSS bounding box and
// internal width/height -- exercising the exact same scaling logic
// BrowserApp.svelte's own toViewportCoords uses, from the outside.
async function canvasClickAt(page, viewportX, viewportY) {
  const rect = await page.evaluate(() => {
    const canvas = document.querySelector('canvas[aria-label="Remote browser page"]');
    const r = canvas.getBoundingClientRect();
    return { left: r.left, top: r.top, width: r.width, height: r.height, cw: canvas.width, ch: canvas.height };
  });
  const cssX = rect.left + (viewportX / rect.cw) * rect.width;
  const cssY = rect.top + (viewportY / rect.ch) * rect.height;
  await page.mouse.click(cssX, cssY);
}

async function navigateAddressBar(page, url) {
  const addressBar = page.locator('.address-bar');
  await addressBar.fill(url);
  await addressBar.press('Enter');
}

async function waitForTabCount(page, expected, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const count = await page.locator('.tab-strip .tab').count();
    if (count === expected) return true;
    await page.waitForTimeout(500);
  }
  return false;
}

if (scenario === 'full_flow') {
  const result = await withBrowser(async (page) => {
    await login(page, args.base, args.username, args.password);
    await openBrowserApp(page);

    const frameArrived = await waitForNonBlankCanvas(page, 20000);
    log('non-blank canvas frame arrived:', frameArrived);

    // Task 3: the arbitrary target website must never appear as a
    // real iframe in the CloudDesk page, and no arbitrary-origin
    // iframe should exist anywhere on the page.
    const iframeCount = await page.locator('iframe').count();
    log('iframe count on CloudDesk page:', iframeCount);

    await navigateAddressBar(page, args.fixtureUrl);
    await page.waitForTimeout(3000);

    // Real click on the fixture's button (viewport coords 20,20 to
    // 120,60 -- click center).
    await canvasClickAt(page, 60, 40);
    await page.waitForTimeout(500);

    // Real typed input into the fixture's text field (click first to
    // focus, then type).
    await canvasClickAt(page, 60, 90);
    await page.keyboard.type('aA1 hello', { delay: 30 });
    await page.waitForTimeout(500);

    // Real checkbox toggle.
    await canvasClickAt(page, 25, 125);
    await page.waitForTimeout(500);

    // Real scroll on the canvas.
    await canvasClickAt(page, 400, 400);
    await page.mouse.wheel(0, 200);
    await page.waitForTimeout(500);

    // Task: second tab.
    await page.locator('.new-tab').click();
    const gotSecondTab = await waitForTabCount(page, 2, 10000);
    log('second tab created:', gotSecondTab);

    await navigateAddressBar(page, args.fixtureUrl);
    await page.waitForTimeout(3000);

    // Switch back to first tab.
    const firstTab = page.locator('.tab-strip .tab').first();
    await firstTab.click();
    await page.waitForTimeout(500);

    // Close the second tab.
    const tabs = page.locator('.tab-strip .tab');
    const secondTab = tabs.nth(1);
    await secondTab.locator('.tab-close').click();
    const backToOneTab = await waitForTabCount(page, 1, 10000);
    log('closed second tab, back to one:', backToOneTab);

    // Popup: click the fixture's popup-trigger button.
    await canvasClickAt(page, 60, 180);
    const popupBecameTab = await waitForTabCount(page, 2, 10000);
    log('popup became a managed tab:', popupBecameTab);

    return {
      ok: true,
      frameArrived,
      iframeCount,
      gotSecondTab,
      backToOneTab,
      popupBecameTab,
    };
  });
  console.log(JSON.stringify(result));
} else if (scenario === 'fixture_state') {
  // Not a browser scenario -- placeholder for symmetry; the Rust side
  // checks the fixture server's own state directly via HTTP instead.
  console.log(JSON.stringify({ ok: false, error: 'unused scenario' }));
} else if (scenario === 'failure_states') {
  const result = await withBrowser(async (page) => {
    await login(page, args.base, args.username, args.password);
    await page.getByRole('button', { name: 'Open application launcher' }).click();
    await page.locator('.launcher-grid button', { hasText: 'Browser' }).first().click({ timeout: 8000 });
    // Runtime disabled: the app must show a clear status, never an
    // endless spinner or a raw error leaking internal details.
    const disabledText = await page
      .locator('.browser-status')
      .first()
      .innerText({ timeout: 15000 })
      .catch(() => null);
    log('status text observed:', disabledText);
    const leaksInternal = disabledText
      ? /docker|container|cdp|devtools|127\.0\.0\.1:\d{4,5}/i.test(disabledText)
      : false;
    return { ok: true, disabledText, leaksInternal };
  });
  console.log(JSON.stringify(result));
} else {
  console.log(JSON.stringify({ ok: false, error: `unknown scenario ${scenario}` }));
}
