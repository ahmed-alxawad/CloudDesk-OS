// PASS SSH-C-2, Gap 3 (Task 6/7): real Playwright-driven evidence for
// the COMPILED CloudDesk frontend's remote terminal -- login -> the
// real Servers app -> a real click on "Open Terminal" -> the real
// RemoteTerminalApp.svelte -> real xterm.js rendering -> real keyboard
// input -> a real remote shell's real output rendered in the DOM.
// Never a direct WebSocket call from this script.
//
// Usage: node remote_terminal_flow.mjs <scenario> <jsonArgsFile>

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
  page.on('console', (msg) => {
    if (msg.type() === 'error') log('CONSOLE ERROR', msg.text());
  });
  page.on('pageerror', (err) => log('PAGE ERROR', String(err)));
  try {
    const result = await fn(page);
    return result;
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
  await page
    .getByRole('button', { name: 'Open application launcher' })
    .waitFor({ timeout: 15000 });
  log('logged in as', username);
}

async function openServersApp(page) {
  await page.getByRole('button', { name: 'Open application launcher' }).click();
  await page
    .locator('.launcher-grid button', { hasText: 'Servers' })
    .first()
    .click({ timeout: 8000 });
  await page.locator('.server-list article').first().waitFor({ timeout: 10000 });
  log('Servers app opened');
}

async function openTerminal(page) {
  await page
    .locator('.server-list article button', { hasText: 'Open Terminal' })
    .first()
    .click({ timeout: 8000 });
  await page.locator('.terminal-app .terminal-surface').waitFor({ timeout: 10000 });
  await page.locator('.terminal-app .xterm-rows').waitFor({ timeout: 15000 });
  log('RemoteTerminalApp opened, xterm rendered');
}

async function terminalText(page) {
  return page.evaluate(() => {
    const rows = document.querySelector('.terminal-app .xterm-rows');
    return rows ? rows.innerText : '';
  });
}

async function terminalStatus(page) {
  return page.evaluate(() => {
    const strong = document.querySelector('.terminal-app header strong');
    return strong ? strong.textContent : '';
  });
}

async function waitForTerminalText(page, predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let text = '';
  while (Date.now() < deadline) {
    text = await terminalText(page);
    if (predicate(text)) return text;
    await page.waitForTimeout(300);
  }
  return text;
}

async function typeIntoTerminal(page, text) {
  await page.locator('.terminal-app .terminal-surface').click();
  await page.keyboard.type(text);
}

const scenarios = {
  async full_flow() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openServersApp(page);
      await openTerminal(page);

      const sentinel = 'frontend-pty-ok';
      await typeIntoTerminal(page, `printf '${sentinel}\\n'\n`);
      const afterSentinel = await waitForTerminalText(
        page,
        (t) => (t.match(new RegExp(sentinel, 'g')) || []).length >= 2,
        20000,
      );
      const sentinelSeenTwice =
        (afterSentinel.match(new RegExp(sentinel, 'g')) || []).length >= 2;

      // Resize: shrink the terminal window, then ask the remote PTY for
      // its own real dimensions -- must differ from the 80x24 default.
      const beforeResizeBox = await page
        .locator('.terminal-app .terminal-surface')
        .boundingBox();
      await page.setViewportSize({ width: 700, height: 500 });
      await page.waitForTimeout(600);
      await typeIntoTerminal(page, 'stty size\n');
      const resizeOutput = await waitForTerminalText(
        page,
        (t) => /\d+ \d+/.test(t.split('stty size').pop() || ''),
        15000,
      );
      const resizeChanged = !resizeOutput.includes('24 80');

      // Ctrl-C: interrupt a long sleep, prove the shell survives and
      // runs the next real command.
      await typeIntoTerminal(page, 'sleep 30; echo SLEPT_FULL\n');
      await page.waitForTimeout(500);
      await page.locator('.terminal-app .terminal-surface').click();
      await page.keyboard.press('Control+c');
      await page.waitForTimeout(400);
      const afterCtrlC = 'shell-alive-again';
      await typeIntoTerminal(page, `printf '${afterCtrlC}\\n'\n`);
      const afterCtrlCText = await waitForTerminalText(
        page,
        (t) => (t.match(new RegExp(afterCtrlC, 'g')) || []).length >= 2,
        15000,
      );
      const ctrlCWorked =
        (afterCtrlCText.match(new RegExp(afterCtrlC, 'g')) || []).length >= 2 &&
        (afterCtrlCText.match(/SLEPT_FULL/g) || []).length <= 1;

      // Exit: the frontend must leave "connected" and never spin forever.
      await typeIntoTerminal(page, 'exit\n');
      const statusAfterExit = await (async () => {
        const deadline = Date.now() + 10000;
        let status = '';
        while (Date.now() < deadline) {
          status = await terminalStatus(page);
          if (status && status !== 'connected' && status !== 'connecting') return status;
          await page.waitForTimeout(300);
        }
        return status;
      })();

      return {
        ok: true,
        sentinelSeenTwice,
        beforeResizeBoxPresent: Boolean(beforeResizeBox),
        resizeChanged,
        ctrlCWorked,
        statusAfterExit,
      };
    });
  },

  async failure_state() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openServersApp(page);
      await openTerminal(page);
      // Signal readiness to the Rust harness (sharing this container's
      // /args volume) so it revokes the server only once the terminal
      // is genuinely open -- a blind fixed delay would race against
      // this container's own npm-install/launch startup time.
      if (args.readyFile) fs.writeFileSync(args.readyFile, 'ready');
      // The Rust harness deletes/revokes the server out from under this
      // already-open terminal once it observes the ready file.
      const status = await (async () => {
        const deadline = Date.now() + Number(args.waitMs || 20000);
        let status = '';
        while (Date.now() < deadline) {
          status = await terminalStatus(page);
          if (['error', 'revoked', 'disconnected', 'exited'].includes(status)) return status;
          await page.waitForTimeout(300);
        }
        return status;
      })();
      return { ok: true, status };
    });
  },
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
