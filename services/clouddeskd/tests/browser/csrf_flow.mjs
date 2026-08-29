// Phase 16D: real, two-origin CSRF browser control. Runs inside a
// disposable, version-pinned Playwright/Chromium container (test
// infrastructure only). Origin A is the real compiled CloudDesk HTTP
// API; Origin B is a plain, disposable "attacker" static page served
// from a different port on the same host -- different port already
// makes it a different origin from the browser's own perspective
// (scheme+host+port), sufficient to exercise real SameSite/Sec-Fetch-
// Site/Origin browser behavior without needing a second real domain.
//
// Usage: node csrf_flow.mjs <scenario> <jsonArgsFile>

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
  try {
    return await fn(page, context);
  } finally {
    await context.close();
    await browser.close();
  }
}

async function login(page, base, username, password) {
  const response = await page.request.post(`${base}/api/v1/auth/login`, {
    data: { username, password, remember_device: false },
  });
  if (response.status() !== 200) {
    throw new Error(`login failed: ${response.status()} ${await response.text()}`);
  }
}

async function csrfCrossOriginAttack(page, context, cloudeskBase, attackerBase) {
  await login(page, cloudeskBase, args.username, args.password);
  // `login()` authenticates via Playwright's API-only request context
  // (never navigates `page` anywhere), so without this, every
  // subsequent `page.evaluate()` fetch below would run from `page`'s
  // still-`about:blank` state -- a null/opaque-origin JS realm, not a
  // real `cloudeskBase` origin -- which is exactly what made an
  // earlier draft of this test fail confusingly ("origin 'null'"
  // CORS errors on the *legitimate*, same-origin control too, not
  // just the intended cross-origin attack).
  await page.goto(cloudeskBase, { waitUntil: 'domcontentloaded' });
  const cookiesBefore = await context.cookies();
  const sessionCookie = cookiesBefore.find((c) => c.name === 'clouddesk_session');
  log(
    'authenticated session cookie attributes:',
    JSON.stringify({
      secure: sessionCookie?.secure,
      httpOnly: sessionCookie?.httpOnly,
      sameSite: sessionCookie?.sameSite,
      path: sessionCookie?.path,
      domain: sessionCookie?.domain,
    })
  );

  // Positive control first: the SAME mutation, executed from the
  // legitimate origin with the same authenticated session, must
  // succeed -- proving the operation/fixture itself is functional
  // before we claim the cross-origin attempt failed.
  const legitimate = await page.evaluate(async (base) => {
    const r = await fetch(`${base}/api/v1/preferences`, {
      method: 'PUT',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        ui_mode: 'desktop',
        layout: { legitimate: true },
        favorites: [],
        recent: [],
      }),
    });
    return { status: r.status };
  }, cloudeskBase);
  log('legitimate same-origin mutation result:', JSON.stringify(legitimate));

  const afterLegitimate = await page.evaluate(async (base) => {
    const r = await fetch(`${base}/api/v1/preferences`, { credentials: 'include' });
    return r.json();
  }, cloudeskBase);
  log('preferences after legitimate mutation:', JSON.stringify(afterLegitimate));

  // Now the actual attack: navigate to the real attacker origin (a
  // real different port -- a different origin to the browser, served
  // by its own real HTTP response, not a Playwright-faked one) while
  // the CloudDesk session cookie is still set, and let IT attempt the
  // cross-origin fetch.
  await page.goto(attackerBase, { waitUntil: 'load' });
  const attackResult = await page.evaluate(() => window.__attackResult);
  log('cross-origin attack result:', JSON.stringify(attackResult));

  // Verify actual server-side state: re-authenticate is unnecessary
  // (the cookie, if it still exists, is the same session) -- read
  // preferences back from the legitimate origin and prove the
  // attacker's payload (ui_mode: 'dashboard', layout.attacker: true)
  // never landed, regardless of what the attack's own fetch response
  // claimed.
  await page.goto(cloudeskBase, { waitUntil: 'domcontentloaded' });
  const finalState = await page.evaluate(async (base) => {
    const r = await fetch(`${base}/api/v1/preferences`, { credentials: 'include' });
    return r.json();
  }, cloudeskBase);
  log('final server-side preferences state:', JSON.stringify(finalState));

  return {
    sessionCookie: {
      secure: sessionCookie?.secure ?? null,
      httpOnly: sessionCookie?.httpOnly ?? null,
      sameSite: sessionCookie?.sameSite ?? null,
    },
    legitimateMutation: legitimate,
    attackResult,
    finalState,
    attackerPayloadLanded:
      finalState?.ui_mode === 'dashboard' && finalState?.layout?.attacker === true,
  };
}

const scenarios = { csrf_cross_origin_attack: csrfCrossOriginAttack };

const fn = scenarios[scenario];
if (!fn) {
  process.stderr.write(`unknown scenario: ${scenario}\n`);
  process.exit(1);
}

withBrowser((page, context) => fn(page, context, args.cloudeskBase, args.attackerBase))
  .then((result) => {
    process.stdout.write(JSON.stringify(result));
  })
  .catch((error) => {
    process.stdout.write(JSON.stringify({ error: String(error) }));
    process.exitCode = 1;
  });
