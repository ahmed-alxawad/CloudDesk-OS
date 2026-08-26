// Minimal standalone reproducer for: workspace-trust transition causes
// `vscode.typescript-language-features` to be registered twice (once
// via the remote extension management server, once via the web
// extension management server), producing:
//
//   Extension 'vscode.typescript-language-features' is already registered
//
// and leaving TypeScript permanently unable to activate for the rest
// of the session.
//
// Requires ZERO CloudDesk code, login, or product UI -- this is the
// pinned code-server image, a fresh profile, and a plain TypeScript
// workspace, driven by Playwright.
//
// Usage:
//   npm install playwright@1.49.0
//   docker run -d --rm --name ts-dup-repro \
//     -v "$(pwd)/workspace":/workspace \
//     codercom/code-server:4.133.0 \
//     --bind-addr 0.0.0.0:8080 --auth none --disable-telemetry \
//     --disable-update-check --disable-proxy \
//     --abs-proxy-base-path /proxy /workspace
//   # front it with any reverse proxy that strips the /proxy prefix
//   # (see README.md for why a real proxy in front is required to
//   # reach --abs-proxy-base-path mode), then:
//   node reproduce.mjs http://127.0.0.1:<relay-port>/proxy/
//
// workspace/ must contain a git-initialized folder with at least one
// .ts file (see README.md for the exact fixture used originally).

import { chromium } from 'playwright';

const base = process.argv[2];
if (!base) {
  console.error('usage: node reproduce.mjs <base-url-ending-in-/proxy/>');
  process.exit(1);
}

function log(...parts) {
  process.stderr.write(parts.join(' ') + '\n');
}

async function waitFor(page, desc, pred, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await pred()) return true;
    await page.waitForTimeout(300);
  }
  log('TIMEOUT waiting for', desc);
  return false;
}

// Non-pausing CDP logpoint at deltaExtensions(e,t){ -- exact source
// location confirmed against codercom/code-server:4.133.0's bundled
// workbench.js (VS Code 1.133.0, commit
// d2f7a122522456b351e9b3ddd39e4f3fb9fd5318). Re-locate via:
//   grep -o 'deltaExtensions(e,t){' workbench.js
// if reproducing against a different pinned version.
const DELTA_LOCATION = { lineNumber: 5413, columnNumber: 22226 };
const DELTA_CONDITION = `
  (function(){
    try{
      function ids(list){ return list.map(function(x){ return x && x.identifier ? x.identifier.value : (x && x.value !== undefined ? x.value : String(x)); }); }
      var existing = this._extensionDescriptions || [];
      var tsEntries = existing.filter(function(x){ return x.identifier && x.identifier.value === 'vscode.typescript-language-features'; });
      var payload = {
        toAdd: ids(e), toRemove: ids(t), existingCount: existing.length, existingTsCount: tsEntries.length,
        toAddTsEntries: e.filter(function(x){ return x.identifier && x.identifier.value === 'vscode.typescript-language-features'; }).map(function(x){
          return { id: x.identifier.value, version: x.version, isBuiltin: x.isBuiltin, targetPlatform: x.targetPlatform, scheme: x.extensionLocation && x.extensionLocation.scheme, path: x.extensionLocation && x.extensionLocation.path };
        })
      };
      console.error('REPRO_DELTA::' + JSON.stringify(payload));
    }catch(err){ console.error('REPRO_DELTA_ERR::' + String(err)); }
    false
  }).call(this)
`.replace(/\n/g, ' ');

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ ignoreHTTPSErrors: true });
const page = await context.newPage();
const client = await context.newCDPSession(page);
await client.send('Debugger.enable');
let bpSet = false;
client.on('Debugger.scriptParsed', async (evt) => {
  if (!evt.url || !evt.url.includes('workbench.js') || bpSet) return;
  bpSet = true;
  await client
    .send('Debugger.setBreakpoint', { location: { scriptId: evt.scriptId, ...DELTA_LOCATION }, condition: DELTA_CONDITION })
    .then(() => log('logpoint set on deltaExtensions'))
    .catch((e) => log('logpoint FAILED', String(e)));
});
client.on('Debugger.paused', async () => { await client.send('Debugger.resume').catch(() => {}); });

page.on('console', (msg) => { if (msg.type() === 'error') log('CONSOLE ERROR', msg.text()); });

const fileUri = 'vscode-remote://remote/workspace/src/example.ts';
const params = new URLSearchParams();
params.set('folder', '/workspace');
params.set('payload', JSON.stringify([['openFile', fileUri]]));
const url = `${base}?${params.toString()}`;
log('navigating to', url);
await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 });
await page.locator('.monaco-workbench').waitFor({ timeout: 30000 });
log('workbench ready (PRE-TRUST checkpoint)');
await page.waitForTimeout(3000);

// Open the integrated terminal -- creating its PTY process requires
// executing code, so a fresh/untrusted workspace shows VS Code's
// standard trust dialog here.
await page.keyboard.press('Control+`');
await waitFor(page, 'terminal panel visible', async () => (await page.locator('.terminal-wrapper, .xterm').count()) > 0, 20000);

const trustDialog = page.locator('.monaco-dialog-box', { hasText: 'trust the authors' });
const trustAppeared = await waitFor(page, 'workspace trust dialog appears', async () => (await trustDialog.count()) > 0, 10000);
log('trust dialog appeared:', trustAppeared);
if (trustAppeared) {
  await page.getByRole('button', { name: 'Trust Folder & Continue' }).click({ timeout: 5000 });
  await waitFor(page, 'trust dialog closed', async () => (await trustDialog.count()) === 0, 10000);
  log('explicit "Trust Folder & Continue" clicked -- POST-TRUST');
} else {
  log('WARNING: trust dialog never appeared -- cannot demonstrate the explicit-trust reproduction');
}

await page.waitForTimeout(5000);
await browser.close();
