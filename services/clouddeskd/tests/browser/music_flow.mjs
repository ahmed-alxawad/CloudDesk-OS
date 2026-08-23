// Phase 5K browser-acceptance evidence harness. Runs inside a disposable,
// version-pinned Playwright/Chromium container (test infrastructure
// only). Drives the ACTUAL compiled CloudDesk frontend: login form, the
// app launcher, the real FilesApp.svelte, and the real
// MusicApp.svelte -- never a direct media URL, never a component test.
//
// Usage: node music_flow.mjs <scenario> <jsonArgsFile>

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
  const responses = [];
  page.on('response', (res) => {
    if (res.url().includes('/api/v1/media/') || res.url().includes('/api/v1/music/')) {
      responses.push({ url: res.url(), status: res.status() });
    }
  });
  // Music uses window.prompt/window.confirm for Add folder / New playlist /
  // Add-to-playlist / Delete playlist -- real product UI, not a mock.
  // This handler answers each dialog based on its real message text.
  page.on('dialog', async (dialog) => {
    const msg = dialog.message();
    log('DIALOG', msg);
    if (msg.startsWith('Music folder')) {
      await dialog.accept(`/${args.folderName}`);
    } else if (msg === 'Playlist name') {
      await dialog.accept('Journey Playlist');
    } else if (msg.startsWith('Add "')) {
      await dialog.accept('1');
    } else if (msg.includes('Delete this playlist')) {
      await dialog.accept();
    } else {
      await dialog.dismiss();
    }
  });
  try {
    const result = await fn(page);
    return { ...result, consoleErrors, mediaResponses: responses };
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

async function waitForListingLoaded(page, timeoutMs) {
  await page
    .locator('.file-list[aria-busy="false"]')
    .waitFor({ timeout: timeoutMs });
}

async function openFilesApp(page) {
  await page.getByRole('button', { name: 'Open application launcher' }).click();
  await page
    .locator('.launcher-grid button', { hasText: 'Files' })
    .first()
    .click({ timeout: 8000 });
  await page.locator('.file-list').waitFor({ timeout: 10000 });
  await waitForListingLoaded(page, 15000);
  log('Files app opened');
}

async function openMusicFromFiles(page, folderName, fileName) {
  await page
    .locator('.file-list button', { hasText: folderName })
    .first()
    .dblclick({ timeout: 8000 });
  await waitForListingLoaded(page, 15000);
  await page.locator('.file-list button', { hasText: fileName }).first().waitFor({ timeout: 8000 });
  await page
    .locator('.file-list button', { hasText: fileName })
    .first()
    .dblclick({ timeout: 8000 });
  await page.locator('.music-app').waitFor({ timeout: 10000 });
  log('Music app opened for', fileName);
}

async function waitForCondition(page, description, predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return true;
    await page.waitForTimeout(200);
  }
  log('TIMEOUT waiting for', description);
  return false;
}

async function audioState(page) {
  return page.evaluate(() => {
    const el = document.querySelector('.music-player audio');
    if (!el) return null;
    return {
      readyState: el.readyState,
      errorCode: el.error ? el.error.code : null,
      paused: el.paused,
      currentTime: el.currentTime,
      duration: el.duration,
      ended: el.ended
    };
  });
}

async function nowPlayingTitle(page) {
  return page.evaluate(() => {
    const strong = document.querySelector('.music-player .player-info strong');
    return strong ? strong.textContent : null;
  });
}

const scenarios = {
  async full_product_journey() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openFilesApp(page);

      // -- Task 23/24: Files double-click opens the exact clicked track
      // (not a substitute) -- a not-yet-indexed file plays via a
      // synthetic single-track entry, so its displayed title is exactly
      // the real filename (no tag to fall back to a different display
      // string).
      await openMusicFromFiles(page, args.folderName, args.trackAFileName);
      const openedTitle = await nowPlayingTitle(page);
      const openedExactTrack = openedTitle === args.trackAFileName;

      const metadataOk = await waitForCondition(
        page,
        'audio metadata loaded',
        async () => {
          const state = await audioState(page);
          return state && state.readyState >= 1 && Number.isFinite(state.duration) && state.duration > 0;
        },
        15000
      );

      await page.locator('.player-controls button[aria-label="Play"]').click();
      const playing = await waitForCondition(
        page,
        'playback started',
        async () => {
          const state = await audioState(page);
          return state && !state.paused && state.readyState >= 2;
        },
        15000
      );
      await page.waitForTimeout(500);
      const afterPlay = await audioState(page);

      await page.locator('.player-controls button[aria-label="Pause"]').click();
      await page.waitForTimeout(300);
      const paused1 = await audioState(page);
      await page.waitForTimeout(400);
      const paused2 = await audioState(page);
      const pauseHeld = Math.abs(paused2.currentTime - paused1.currentTime) < 0.05;

      await page.locator('.player-controls button[aria-label="Play"]').click();
      const resumed = await waitForCondition(
        page,
        'resumed playback advancing',
        async () => {
          const state = await audioState(page);
          return state && !state.paused && state.currentTime > paused2.currentTime + 0.05;
        },
        10000
      );

      const beforeSeek = await audioState(page);
      await page.locator('.timeline').evaluate((el) => {
        el.value = '0.6';
        el.dispatchEvent(new Event('input', { bubbles: true }));
      });
      const seeked = await waitForCondition(
        page,
        'seek landed',
        async () => {
          const state = await audioState(page);
          return state && Math.abs(state.currentTime - 0.6 * beforeSeek.duration) < 1.5;
        },
        10000
      );

      // -- Now populate the real library (Add folder -> real scan, via
      // the real product prompt UI -- see the dialog handler above) so
      // favorite/queue/playlist/search/artist operate on real, indexed
      // tracks (the only tracks these controls are ever reachable for
      // in the real UI -- there is no favorite/playlist affordance on
      // the just-opened synthetic Files preview above).
      await page.locator('.music-sidebar button', { hasText: /^Library$/ }).click();
      await page.locator('.music-sidebar button', { hasText: 'Add folder' }).click();
      const tracksIndexed = await waitForCondition(
        page,
        'library indexed after add+scan',
        async () => (await page.locator('.track-list li', { hasText: 'Alpha Song' }).count()) > 0,
        15000
      );

      // Play the real, now-indexed Alpha Song row (real track id).
      // Clicking a track row loads a fresh <audio> element (no
      // `autoplay` attribute, matching real browser/UX convention) --
      // it must be started with a real Play click, same as the very
      // first track above.
      const alphaRow = page.locator('.track-list li', { hasText: 'Alpha Song' }).first();
      await alphaRow.locator('.track-row').click();
      await waitForCondition(
        page,
        'real indexed track metadata loaded',
        async () => {
          const state = await audioState(page);
          return state && state.readyState >= 1 && Number.isFinite(state.duration) && state.duration > 0;
        },
        15000
      );
      await page.locator('.player-controls button[aria-label="Play"]').click();
      await waitForCondition(
        page,
        'real indexed track playing',
        async () => {
          const state = await audioState(page);
          return state && !state.paused;
        },
        15000
      );
      // hasPlayedEnoughToRecord requires >=15s elapsed OR >=50% of a
      // short track -- these fixtures are 3s, so >=1.5s of real
      // playback is enough to be recorded as "recently played".
      await page.waitForTimeout(1800);

      await alphaRow.locator('button[aria-label="Toggle favorite"]').click();
      await page.locator('.music-sidebar button', { hasText: 'Favorites' }).click();
      const favorited = await waitForCondition(
        page,
        'favorite reflected',
        async () => (await page.locator('.track-list li', { hasText: 'Alpha Song' }).count()) > 0,
        8000
      );

      await page.locator('.music-sidebar button', { hasText: /^Library$/ }).click();
      const betaRow = page.locator('.track-list li', { hasText: 'Beta Song' }).first();
      await betaRow.locator('button[aria-label="Add to queue"]').click();
      await page.locator('.music-sidebar button', { hasText: /^Queue/ }).click();
      const queuedNext = await waitForCondition(
        page,
        'queue reflects added track',
        async () => (await page.locator('.track-list li', { hasText: 'Beta Song' }).count()) > 0,
        8000
      );

      await page.locator('.music-sidebar button', { hasText: 'Playlists' }).click();
      await page.locator('button', { hasText: 'New playlist' }).click();
      await page
        .locator('.simple-list li button', { hasText: 'Journey Playlist' })
        .waitFor({ timeout: 8000 });
      await page.locator('.music-sidebar button', { hasText: /^Library$/ }).click();
      await alphaRow.locator('button[aria-label="Add to playlist"]').click();
      await page.locator('.music-sidebar button', { hasText: 'Playlists' }).click();
      await page
        .locator('.simple-list li button', { hasText: 'Journey Playlist' })
        .click();
      const playlistCreatedAndTrackAdded = await waitForCondition(
        page,
        'playlist holds the added track',
        async () => (await page.locator('.track-list li', { hasText: 'Alpha Song' }).count()) > 0,
        8000
      );

      await page.locator('.music-sidebar button', { hasText: 'Search' }).click();
      await page.getByLabel('Search music library').fill('Alpha');
      const searchFoundTrack = await waitForCondition(
        page,
        'search finds the real indexed track',
        async () => (await page.locator('.track-list li', { hasText: 'Alpha Song' }).count()) > 0,
        8000
      );

      await page.locator('.music-sidebar button', { hasText: 'Artists' }).click();
      const artistItems = await page.locator('.simple-list li').allTextContents();
      const artistGroupingCorrect = artistItems.some((t) => t.includes('Test Artist'));

      // Recently-played: switch away and back to force the sidebar's
      // "Recently played" mini-list (loaded on mount) to reflect the
      // real play recorded above -- reload the whole app for a clean,
      // unambiguous re-fetch of persisted server state.
      await page.reload({ waitUntil: 'domcontentloaded' });
      await page
        .getByRole('button', { name: 'Open application launcher' })
        .waitFor({ timeout: 15000 });
      await page.getByRole('button', { name: 'Open application launcher' }).click();
      await page
        .locator('.launcher-grid button', { hasText: 'Music' })
        .first()
        .click({ timeout: 8000 });
      await page.locator('.music-app').waitFor({ timeout: 10000 });
      const recentUpdated = await waitForCondition(
        page,
        'recently played reflects the real play',
        async () =>
          (await page.locator('.music-sidebar .mini-track', { hasText: 'Alpha Song' }).count()) > 0,
        8000
      );

      return {
        ok: true,
        openedExactTrack,
        openedTitle,
        metadataOk,
        playing,
        afterPlayCurrentTime: afterPlay.currentTime,
        pauseHeld,
        resumed,
        seeked,
        tracksIndexed,
        favorited,
        queuedNext,
        playlistCreatedAndTrackAdded,
        searchFoundTrack,
        artistGroupingCorrect,
        recentUpdated
      };
    });
  },

  async corrupt_fixture_flow() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openFilesApp(page);
      await page
        .locator('.file-list button', { hasText: args.folderName })
        .first()
        .dblclick({ timeout: 8000 });
      await page
        .locator('.file-list button', { hasText: args.malformedFileName })
        .first()
        .waitFor({ timeout: 8000 });
      await page
        .locator('.file-list button', { hasText: args.malformedFileName })
        .first()
        .dblclick({ timeout: 8000 });
      await page.locator('.music-app').waitFor({ timeout: 10000 });

      const sawError = await waitForCondition(
        page,
        'safe error state',
        async () => (await page.locator('.music-app [role="alert"]').count()) > 0,
        20000
      );
      const stuckLoading = await page.locator('.music-app [aria-busy="true"]').count();

      return { ok: true, sawError, stuckLoading };
    });
  },

  async close_during_playback_flow() {
    return withBrowser(async (page) => {
      await login(page, args.base, args.username, args.password);
      await openFilesApp(page);
      await openMusicFromFiles(page, args.folderName, args.trackAFileName);

      await waitForCondition(
        page,
        'audio metadata loaded',
        async () => {
          const state = await audioState(page);
          return state && state.readyState >= 1 && state.duration > 0;
        },
        15000
      );
      await page.locator('.player-controls button[aria-label="Play"]').click();
      const playing = await waitForCondition(
        page,
        'playback started',
        async () => {
          const state = await audioState(page);
          return state && !state.paused;
        },
        15000
      );
      await page.waitForTimeout(400);

      // Close the Music window itself (the real desktop shell's
      // window-close control -- App.svelte's `.window-bar` `Close`
      // button), not just navigate away -- proves no server-side
      // ffmpeg/ffprobe process survives a real window close during
      // active playback.
      await page.locator('.music-app').evaluate((el) => {
        const win = el.closest('.window');
        const btn = win ? win.querySelector('button[aria-label="Close"]') : null;
        if (btn) btn.click();
      });
      await page.waitForTimeout(300);

      return { ok: true, playing };
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
