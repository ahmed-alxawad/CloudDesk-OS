<script lang="ts">
  import { onMount } from 'svelte';
  import FilesApp from './lib/FilesApp.svelte';
  import TransfersApp from './lib/TransfersApp.svelte';
  import SettingsApp from './lib/SettingsApp.svelte';
  import ServersApp from './lib/ServersApp.svelte';
  import GalleryApp from './lib/GalleryApp.svelte';
  import DocumentApp from './lib/DocumentApp.svelte';
  import VideoApp from './lib/VideoApp.svelte';
  import {
    applicationById,
    applications,
    loadApplicationManifests,
    type AppDefinition
  } from './lib/apps';
  import {
    clampWindow,
    DEFAULT_PREFERENCES,
    defaultWindow,
    type WindowLayout,
    type WorkspacePreferences
  } from './lib/workspace';

  type Screen = 'loading' | 'setup' | 'login' | 'workspace';
  type RuntimeFlags = {
    browser: boolean;
    code: boolean;
    office: boolean;
    media: boolean;
  };
  type OpenWindow = WindowLayout & {
    id: string;
    z: number;
    params?: { path: string };
  };

  let screen: Screen = 'loading';
  let error = '';
  let busy = false;
  let username = '';
  let password = '';
  let displayName = '';
  let linuxUsername = '';
  let bootstrapSecret = '';
  let setupMode: 'desktop' | 'dashboard' = 'desktop';
  let runtimes: RuntimeFlags = {
    browser: false,
    code: false,
    office: false,
    media: false
  };
  let capabilities: string[] = [];
  let registeredApplications = applications;
  let preferences: WorkspacePreferences = structuredClone(DEFAULT_PREFERENCES);
  let windows: OpenWindow[] = [];
  let launcherOpen = false;
  let clock = new Date();
  let notification = '';
  let saveTimer: ReturnType<typeof setTimeout> | undefined;

  onMount(() => {
    void initialize();
    const clockTimer = setInterval(() => (clock = new Date()), 30_000);
    const keydown = (event: KeyboardEvent) => {
      if (screen !== 'workspace') return;
      if (event.key === 'Escape') launcherOpen = false;
      if (event.altKey && /^[1-9]$/.test(event.key)) {
        event.preventDefault();
        const application = availableApplications()[Number(event.key) - 1];
        if (application) openApplication(application);
      }
    };
    window.addEventListener('keydown', keydown);
    return () => {
      clearInterval(clockTimer);
      window.removeEventListener('keydown', keydown);
    };
  });

  async function api(path: string, init?: RequestInit): Promise<Response> {
    const response = await fetch(path, {
      ...init,
      headers: init?.body
        ? { 'Content-Type': 'application/json', ...init.headers }
        : init?.headers
    });
    if (!response.ok) {
      const body = (await response.json().catch(() => ({}))) as {
        error?: string;
      };
      throw new Error(body.error ?? `Request failed (${response.status})`);
    }
    return response;
  }

  async function initialize() {
    try {
      registeredApplications = await loadApplicationManifests();
      const status = (await (await api('/api/v1/setup/status')).json()) as {
        bootstrap_required: boolean;
      };
      if (status.bootstrap_required) {
        screen = 'setup';
        return;
      }
      try {
        await api('/api/v1/auth/me');
        await enterWorkspace();
      } catch {
        screen = 'login';
      }
    } catch (reason) {
      error = message(reason);
      screen = 'login';
    }
  }

  async function completeSetup() {
    await run(async () => {
      await api('/api/v1/setup/bootstrap', {
        method: 'POST',
        body: JSON.stringify({
          secret: bootstrapSecret,
          username,
          display_name: displayName,
          password,
          linux_username: linuxUsername || null,
          ui_mode: setupMode,
          enable_browser: runtimes.browser,
          enable_code: runtimes.code,
          enable_office: runtimes.office
        })
      });
      preferences.ui_mode = setupMode;
      screen = 'login';
      password = '';
      bootstrapSecret = '';
    });
  }

  async function login() {
    await run(async () => {
      await api('/api/v1/auth/login', {
        method: 'POST',
        body: JSON.stringify({
          username,
          password,
          remember_device: true,
          device_label: 'Web shell'
        })
      });
      password = '';
      await enterWorkspace();
    });
  }

  async function logout() {
    await run(async () => {
      await api('/api/v1/auth/logout', { method: 'POST', body: '{}' });
      windows = [];
      screen = 'login';
    });
  }

  async function enterWorkspace() {
    const [preferencesResponse, runtimeResponse, principalResponse] =
      await Promise.all([
        api('/api/v1/preferences'),
        api('/api/v1/runtime-settings'),
        api('/api/v1/auth/me')
      ]);
    preferences = (await preferencesResponse.json()) as WorkspacePreferences;
    runtimes = (await runtimeResponse.json()) as RuntimeFlags;
    const activePrincipal = (await principalResponse.json()) as {
      username: string;
      capabilities: string[];
    };
    username = activePrincipal.username;
    capabilities = activePrincipal.capabilities;
    preferences.favorites = preferences.favorites.length
      ? preferences.favorites
      : DEFAULT_PREFERENCES.favorites;
    screen = 'workspace';
  }

  async function run(action: () => Promise<void>) {
    busy = true;
    error = '';
    try {
      await action();
    } catch (reason) {
      error = message(reason);
    } finally {
      busy = false;
    }
  }

  function message(reason: unknown): string {
    return reason instanceof Error ? reason.message : 'Something went wrong';
  }

  function availableApplications(): AppDefinition[] {
    return registeredApplications.filter(isAvailable);
  }

  function isAvailable(application: AppDefinition): boolean {
    const runtimeReady =
      application.runtime === null || runtimes[application.runtime];
    return (
      runtimeReady &&
      application.requiredPermissions.every((permission) =>
        capabilities.includes(permission)
      )
    );
  }

  function openApplication(
    application: AppDefinition,
    params?: { path: string }
  ) {
    if (!isAvailable(application)) {
      showNotification(
        `${application.name} is unavailable under current policy.`
      );
      return;
    }
    const existing = windows.find((entry) => entry.id === application.id);
    const top = Math.max(0, ...windows.map((entry) => entry.z)) + 1;
    if (existing) {
      // Re-opening an already-open single-instance app with a new target
      // (e.g. double-clicking a second video in Files) retargets that
      // window rather than opening a second one -- there is only ever
      // one window per application id in this shell.
      windows = windows.map((entry) =>
        entry.id === application.id
          ? {
              ...entry,
              minimized: false,
              z: top,
              params: params ?? entry.params
            }
          : entry
      );
    } else {
      const saved =
        preferences.layout[application.id] ?? defaultWindow(windows.length);
      const bounded = clampWindow(saved, window.innerWidth, window.innerHeight);
      windows = [
        ...windows,
        { ...bounded, id: application.id, z: top, params }
      ];
      preferences.recent = [
        application.id,
        ...preferences.recent.filter((id) => id !== application.id)
      ].slice(0, 8);
    }
    launcherOpen = false;
    scheduleSave();
  }

  function focusWindow(id: string) {
    const top = Math.max(0, ...windows.map((entry) => entry.z)) + 1;
    windows = windows.map((entry) =>
      entry.id === id ? { ...entry, z: top } : entry
    );
  }

  function updateWindow(id: string, update: Partial<OpenWindow>) {
    windows = windows.map((entry) =>
      entry.id === id ? { ...entry, ...update } : entry
    );
  }

  function closeWindow(id: string) {
    windows = windows.filter((entry) => entry.id !== id);
    scheduleSave();
  }

  function startMove(event: PointerEvent, entry: OpenWindow) {
    if ((event.target as HTMLElement).closest('button') || entry.maximized)
      return;
    event.preventDefault();
    focusWindow(entry.id);
    const origin = {
      x: event.clientX,
      y: event.clientY,
      left: entry.x,
      top: entry.y
    };
    const move = (next: PointerEvent) => {
      updateWindow(entry.id, {
        x: Math.max(0, origin.left + next.clientX - origin.x),
        y: Math.max(48, origin.top + next.clientY - origin.y)
      });
    };
    const stop = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
      scheduleSave();
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', stop);
  }

  function startResize(event: PointerEvent, entry: OpenWindow) {
    event.preventDefault();
    event.stopPropagation();
    const origin = {
      x: event.clientX,
      y: event.clientY,
      width: entry.width,
      height: entry.height
    };
    const move = (next: PointerEvent) =>
      updateWindow(entry.id, {
        width: Math.max(360, origin.width + next.clientX - origin.x),
        height: Math.max(260, origin.height + next.clientY - origin.y)
      });
    const stop = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
      scheduleSave();
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', stop);
  }

  function switchMode(mode: 'desktop' | 'dashboard') {
    preferences.ui_mode = mode;
    preferences = { ...preferences };
    scheduleSave();
  }

  function scheduleSave() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => void savePreferences(), 500);
  }

  async function savePreferences() {
    const layout: Record<string, WindowLayout> = {};
    for (const { id, z: _z, params: _params, ...windowLayout } of windows) {
      layout[id] = windowLayout;
    }
    preferences.layout = layout;
    try {
      await api('/api/v1/preferences', {
        method: 'PUT',
        body: JSON.stringify(preferences)
      });
    } catch {
      showNotification('Workspace changes could not be saved.');
    }
  }

  function showNotification(text: string) {
    notification = text;
    setTimeout(() => {
      if (notification === text) notification = '';
    }, 3600);
  }

  function icon(name: string): string {
    return (
      (
        {
          folder: '▰',
          image: '◫',
          terminal: '›_',
          server: '▤',
          transfer: '⇄',
          settings: '⚙',
          globe: '◎',
          code: '</>',
          document: '▧',
          video: '▶'
        } as Record<string, string>
      )[name] ?? '◇'
    );
  }
</script>

<svelte:head>
  <title>CloudDesk</title>
  <meta name="description" content="A secure browser-native Linux workspace" />
</svelte:head>

{#if screen === 'loading'}
  <main class="loading-screen" aria-live="polite">
    <div class="logo large">C</div>
    <p>Starting your workspace…</p>
  </main>
{:else if screen === 'setup' || screen === 'login'}
  <main class="auth-screen">
    <section class="auth-intro">
      <a class="wordmark" href="/" aria-label="CloudDesk home"
        ><span class="logo">C</span>CloudDesk</a
      >
      <div>
        <p class="kicker">Your Linux server, beautifully close.</p>
        <h1>{screen === 'setup' ? 'Make it yours.' : 'Welcome back.'}</h1>
        <p class="lead">
          A focused, multi-user workspace with real Linux identity, durable
          background work, and a deliberately small privileged boundary.
        </p>
      </div>
      <p class="auth-foot">
        Secure by default · HTTPS · Server-side authorization
      </p>
    </section>
    <section class="auth-panel">
      {#if screen === 'setup'}
        <form
          onsubmit={(event) => {
            event.preventDefault();
            void completeSetup();
          }}
        >
          <div class="form-heading">
            <span>Initial setup</span><strong>01 / 01</strong>
          </div>
          <label
            >Bootstrap secret<input
              bind:value={bootstrapSecret}
              type="password"
              autocomplete="one-time-code"
              required
            /></label
          >
          <div class="field-row">
            <label
              >Username<input
                bind:value={username}
                autocomplete="username"
                minlength="3"
                required
              /></label
            ><label
              >Display name<input
                bind:value={displayName}
                autocomplete="name"
                required
              /></label
            >
          </div>
          <label
            >Password<input
              bind:value={password}
              type="password"
              autocomplete="new-password"
              minlength="12"
              required
            /></label
          >
          <label
            >Linux account <small>Optional mapping for Files and Terminal</small
            ><input
              bind:value={linuxUsername}
              autocomplete="off"
              placeholder="e.g. ahmed"
            /></label
          >
          <fieldset>
            <legend>Default workspace</legend><button
              class:chosen={setupMode === 'desktop'}
              type="button"
              onclick={() => (setupMode = 'desktop')}
              ><strong>Desktop</strong><span>Windows, dock, and launcher</span
              ></button
            ><button
              class:chosen={setupMode === 'dashboard'}
              type="button"
              onclick={() => (setupMode = 'dashboard')}
              ><strong>Dashboard</strong><span
                >Lightweight overview and app grid</span
              ></button
            >
          </fieldset>
          <div class="runtime-options">
            <span>Optional runtimes</span><label
              ><input bind:checked={runtimes.browser} type="checkbox" /> Browser</label
            ><label
              ><input bind:checked={runtimes.code} type="checkbox" /> Code</label
            ><label
              ><input bind:checked={runtimes.office} type="checkbox" /> Office</label
            >
          </div>
          {#if error}<p class="form-error" role="alert">{error}</p>{/if}
          <button class="primary" type="submit" disabled={busy}
            >{busy ? 'Creating workspace…' : 'Create administrator'}</button
          >
        </form>
      {:else}
        <form
          onsubmit={(event) => {
            event.preventDefault();
            void login();
          }}
        >
          <div class="form-heading">
            <span>Sign in</span><strong>CloudDesk</strong>
          </div>
          <label
            >Username<input
              bind:value={username}
              autocomplete="username"
              required
            /></label
          >
          <label
            >Password<input
              bind:value={password}
              type="password"
              autocomplete="current-password"
              required
            /></label
          >
          {#if error}<p class="form-error" role="alert">{error}</p>{/if}
          <button class="primary" type="submit" disabled={busy}
            >{busy ? 'Signing in…' : 'Enter workspace'}</button
          >
        </form>
      {/if}
    </section>
  </main>
{:else}
  <main class="workspace" class:dashboard={preferences.ui_mode === 'dashboard'}>
    <header class="topbar">
      <button
        class="brand-button"
        type="button"
        onclick={() => (launcherOpen = !launcherOpen)}
        aria-expanded={launcherOpen}
        aria-label="Open application launcher"
        ><span class="logo small">C</span></button
      >
      <div class="workspace-title">
        <strong>CloudDesk</strong><span
          >{preferences.ui_mode === 'desktop' ? 'Desktop' : 'Dashboard'}</span
        >
      </div>
      <div class="mode-switch" aria-label="Workspace mode">
        <button
          class:active={preferences.ui_mode === 'desktop'}
          onclick={() => switchMode('desktop')}>Desktop</button
        ><button
          class:active={preferences.ui_mode === 'dashboard'}
          onclick={() => switchMode('dashboard')}>Dashboard</button
        >
      </div>
      <div class="top-actions">
        <time
          >{clock.toLocaleTimeString([], {
            hour: '2-digit',
            minute: '2-digit'
          })}</time
        ><button
          type="button"
          onclick={() => showNotification('You’re all caught up.')}>◌</button
        ><button
          class="avatar"
          type="button"
          onclick={() => void logout()}
          title="Sign out">{username.slice(0, 1).toUpperCase() || 'U'}</button
        >
      </div>
    </header>
    {#if launcherOpen}
      <section class="launcher" aria-label="Application launcher">
        <div>
          <p>Applications</p>
          <span>Alt + 1–9 to launch</span>
        </div>
        <div class="launcher-grid">
          {#each registeredApplications as application}<button
              class:disabled={!isAvailable(application)}
              type="button"
              onclick={() => openApplication(application)}
              ><span class="app-symbol" style:--accent={application.accent}
                >{icon(application.icon)}</span
              ><strong>{application.name}</strong><small
                >{application.description}</small
              ></button
            >{/each}
        </div>
      </section>
    {/if}
    {#if preferences.ui_mode === 'dashboard'}
      <section class="dashboard-content">
        <div class="dashboard-heading">
          <div>
            <p class="kicker">Workspace overview</p>
            <h1>
              Good {clock.getHours() < 12
                ? 'morning'
                : clock.getHours() < 18
                  ? 'afternoon'
                  : 'evening'}.
            </h1>
          </div>
          <button onclick={() => switchMode('desktop')}>Open desktop →</button>
        </div>
        <div class="metric-grid">
          <article>
            <span>System</span><strong>Online</strong><small
              >Core services responding</small
            >
          </article>
          <article>
            <span>Transfers</span><strong>0 active</strong><small
              >No queued work</small
            >
          </article>
          <article>
            <span>Sessions</span><strong>1</strong><small>This browser</small>
          </article>
        </div>
        <div class="dashboard-section">
          <div>
            <h2>Favorites</h2>
            <span>Your everyday tools</span>
          </div>
          <div class="favorite-grid">
            {#each preferences.favorites
              .map(applicationById)
              .filter(Boolean) as application}<button
                onclick={() => openApplication(application as AppDefinition)}
                ><span
                  class="app-symbol"
                  style:--accent={(application as AppDefinition).accent}
                  >{icon((application as AppDefinition).icon)}</span
                ><strong>{(application as AppDefinition).name}</strong><small
                  >{(application as AppDefinition).description}</small
                ></button
              >{/each}
          </div>
        </div>
        <div class="dashboard-section">
          <div>
            <h2>Recent</h2>
            <span>Continue where you left off</span>
          </div>
          <div class="recent-list">
            {#if preferences.recent.length === 0}<p>
                Applications you open will appear here.
              </p>{/if}{#each preferences.recent
              .map(applicationById)
              .filter(Boolean) as application}<button
                onclick={() => openApplication(application as AppDefinition)}
                ><span>{icon((application as AppDefinition).icon)}</span><strong
                  >{(application as AppDefinition).name}</strong
                ><small>Open application</small></button
              >{/each}
          </div>
        </div>
      </section>
    {:else}
      <section class="desktop" aria-label="Desktop">
        <div class="desktop-shortcuts">
          {#each availableApplications().slice(0, 6) as application}<button
              ondblclick={() => openApplication(application)}
              onclick={() =>
                showNotification(`Double-click to open ${application.name}.`)}
              ><span class="app-symbol" style:--accent={application.accent}
                >{icon(application.icon)}</span
              ><span>{application.name}</span></button
            >{/each}
        </div>
        {#each windows as entry (entry.id)}
          {@const application = applicationById(entry.id)}
          {#if application && !entry.minimized}
            <article
              class="window"
              class:maximized={entry.maximized}
              style:left={`${entry.x}px`}
              style:top={`${entry.y}px`}
              style:width={`${entry.width}px`}
              style:height={`${entry.height}px`}
              style:z-index={entry.z}
              onpointerdown={() => focusWindow(entry.id)}
            >
              <header
                class="window-bar"
                role="toolbar"
                tabindex="0"
                aria-label={`${application.name} window controls`}
                onpointerdown={(event) => startMove(event, entry)}
              >
                <span
                  class="app-symbol mini"
                  style:--accent={application.accent}
                  >{icon(application.icon)}</span
                ><strong>{application.name}</strong>
                <div>
                  <button
                    aria-label="Minimize"
                    onclick={() => updateWindow(entry.id, { minimized: true })}
                    >−</button
                  ><button
                    aria-label="Maximize"
                    onclick={() =>
                      updateWindow(entry.id, { maximized: !entry.maximized })}
                    >□</button
                  ><button
                    aria-label="Close"
                    onclick={() => closeWindow(entry.id)}>×</button
                  >
                </div>
              </header>
              <div class="window-content">
                {#if application.id === 'files'}
                  <FilesApp
                    onOpenWithVideo={(path) => {
                      const videoApp = applicationById('video');
                      if (videoApp) openApplication(videoApp, { path });
                    }}
                  />
                {:else if application.id === 'video'}
                  <VideoApp initialPath={entry.params?.path ?? null} />
                {:else if application.id === 'transfers'}
                  <TransfersApp />
                {:else if application.id === 'settings'}
                  <SettingsApp />
                {:else if application.id === 'terminal'}
                  {#await import('./lib/TerminalApp.svelte')}
                    <div class="empty-app"><p>Loading terminal…</p></div>
                  {:then module}
                    {@const TerminalApp = module.default}
                    <TerminalApp />
                  {/await}
                {:else if application.id === 'servers'}
                  <ServersApp />
                {:else if application.id === 'gallery' || application.id === 'photos'}
                  <GalleryApp />
                {:else if application.id === 'documents' || application.id === 'reader'}
                  <DocumentApp />
                {:else}<div class="empty-app">
                    <span
                      class="app-symbol hero-icon"
                      style:--accent={application.accent}
                      >{icon(application.icon)}</span
                    >
                    <p class="kicker">{application.name}</p>
                    <h2>{application.description}</h2>
                    <p>
                      The secure application surface is ready for its service
                      module.
                    </p>
                    <button
                      onclick={() =>
                        showNotification(
                          `${application.name} opened securely.`
                        )}>Continue</button
                    >
                  </div>{/if}
              </div>
              {#if !entry.maximized}<button
                  class="resize-handle"
                  aria-label={`Resize ${application.name}`}
                  onpointerdown={(event) => startResize(event, entry)}
                ></button>{/if}
            </article>
          {/if}
        {/each}
      </section>
      <nav class="dock" aria-label="Running and favorite applications">
        <button
          class="launcher-button"
          onclick={() => (launcherOpen = !launcherOpen)}
          aria-label="Applications">•••</button
        ><span></span>{#each preferences.favorites
          .map(applicationById)
          .filter(Boolean) as application}{@const app =
            application as AppDefinition}<button
            class:running={windows.some((entry) => entry.id === app.id)}
            onclick={() => openApplication(app)}
            title={app.name}
            ><span class="app-symbol mini" style:--accent={app.accent}
              >{icon(app.icon)}</span
            ></button
          >{/each}
      </nav>
    {/if}
    {#if notification}<aside class="notification" aria-live="polite">
        <span>CloudDesk</span>
        <p>{notification}</p>
      </aside>{/if}
  </main>
{/if}
