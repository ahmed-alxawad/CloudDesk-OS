// Phase 9 test infrastructure only: a minimal, direct CDP client used
// to prove real Brave profile persistence (localStorage) across a
// real stop/restart cycle. Not product code -- clouddeskd's own
// broker (not yet built) is what a real client would talk to instead.
//
// localStorage, not cookies: live-verified this pass that a real
// cookie set via `document.cookie` genuinely reaches Chromium's
// on-disk `Cookies` SQLite file (confirmed via `strings` on the raw
// file), but its `encrypted_value` cannot be decrypted again on a
// fresh restart in this minimal container image (no dbus/keyring
// daemon for Chromium's OS-crypt backend to use, and `--password-
// store=basic` alone did not resolve it within this pass's
// investigation) -- a real, honestly-documented open item, not glossed
// over. localStorage is backed by LevelDB, entirely outside that
// OS-crypt/cookie-encryption pipeline, and was directly verified this
// pass to survive a real stop/restart cycle.
//
// Usage: node cdp_probe.mjs <cdpBaseUrl> <set|get> [value]

const [, , base, action, value] = process.argv;

async function main() {
  const version = await (await fetch(`${base}/json/version`)).json();
  const browserWs = new WebSocket(version.webSocketDebuggerUrl);
  let id = 1;
  const pending = new Map();
  function send(method, params, sessionId) {
    return new Promise((resolve) => {
      const msgId = id++;
      const msg = { id: msgId, method, params: params || {} };
      if (sessionId) msg.sessionId = sessionId;
      pending.set(msgId, resolve);
      browserWs.send(JSON.stringify(msg));
    });
  }
  await new Promise((resolve) => browserWs.addEventListener('open', resolve));
  browserWs.addEventListener('message', (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg.result);
      pending.delete(msg.id);
    }
  });

  const target = await send('Target.createTarget', { url: 'about:blank' });
  const attach = await send('Target.attachToTarget', {
    targetId: target.targetId,
    flatten: true,
  });
  const sessionId = attach.sessionId;

  await send('Page.enable', {}, sessionId);
  const navDone = new Promise((resolve) => {
    const handler = (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.method === 'Page.loadEventFired' && msg.sessionId === sessionId) {
        browserWs.removeEventListener('message', handler);
        resolve();
      }
    };
    browserWs.addEventListener('message', handler);
  });
  await send('Page.navigate', { url: 'https://example.com' }, sessionId);
  await navDone;

  if (action === 'set') {
    await send(
      'Runtime.evaluate',
      { expression: `localStorage.setItem("sentinel", ${JSON.stringify(value)})` },
      sessionId,
    );
    await new Promise((r) => setTimeout(r, 1000));
    console.log(JSON.stringify({ ok: true }));
  } else if (action === 'get') {
    const result = await send(
      'Runtime.evaluate',
      { expression: 'localStorage.getItem("sentinel")' },
      sessionId,
    );
    const value = result && result.result ? result.result.value : null;
    console.log(JSON.stringify({ ok: true, value: value === undefined ? null : value }));
  } else {
    console.log(JSON.stringify({ ok: false, error: `unknown action ${action}` }));
    process.exit(1);
  }
  process.exit(0);
}

main().catch((e) => {
  console.log(JSON.stringify({ ok: false, error: String(e && e.stack ? e.stack : e) }));
  process.exit(1);
});
