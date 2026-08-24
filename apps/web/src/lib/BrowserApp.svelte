<script lang="ts">
  import { onDestroy, onMount } from 'svelte';

  type Phase =
    | 'checking'
    | 'unavailable'
    | 'disabled'
    | 'starting'
    | 'running'
    | 'disconnected'
    | 'failed'
    | 'permission-denied';

  let phase: Phase = 'checking';
  let instanceId: string | null = null;
  let errorDetail = '';
  let pollTimer: ReturnType<typeof setInterval> | undefined;

  let canvas: HTMLCanvasElement;
  let container: HTMLDivElement;
  let socket: WebSocket | null = null;
  let addressBarValue = '';
  let pageUrl = '';
  let pageLoading = false;
  let viewportWidth = 1024;
  let viewportHeight = 768;
  let resizeObserver: ResizeObserver | null = null;
  let frameImage: HTMLImageElement = new Image();
  let disposed = false;

  type Tab = {
    tab_id: string;
    url: string;
    title: string;
    loading: boolean;
    active: boolean;
  };
  let tabs: Tab[] = [];

  // Pass 3B: downloads.
  type Download = {
    download_id: string;
    filename: string;
    total_bytes: number | null;
    received_bytes: number;
    state: 'in_progress' | 'completed' | 'cancelled' | 'failed';
    failure_reason: string | null;
  };
  let downloads: Download[] = [];
  let saveTargetId: string | null = null;
  let savePathValue = '';
  let saveError = '';

  // Pass 3B/3B-3: uploads/file chooser. Selection is always made from a
  // real, server-rendered list of this user's own authorized roots/
  // servers (never a free-text id) -- Files has no shared picker
  // component to reuse (Task 1's own research), so this reuses the
  // existing list APIs (`/api/v1/code/workspaces` for local roots,
  // `/api/v1/remote/servers` for remote servers) that Code/Servers
  // already expose, rather than inventing a Browser-only authority
  // model.
  type UploadRoot = { id: string | null; label: string };
  type UploadServer = { id: string; name: string; hostname: string };
  let pendingChooserId: string | null = null;
  let selectSource: 'local' | 'remote' = 'local';
  let selectRootId: string | null = null;
  let selectServerId: string | null = null;
  let selectPathValue = '';
  let selectError = '';
  let uploadRoots: UploadRoot[] = [];
  let uploadServers: UploadServer[] = [];

  // Pass 3B: clipboard.
  let clipboardStatus = '';

  // Pass 3B: audio.
  let audioEnabled = false;
  let audioContext: AudioContext | null = null;
  let audioNextStartTime = 0;

  onMount(() => void start());
  onDestroy(() => {
    disposed = true;
    if (pollTimer) clearInterval(pollTimer);
    resizeObserver?.disconnect();
    socket?.close();
    void audioContext?.close();
  });

  async function api(path: string, options?: RequestInit) {
    const response = await fetch(path, options);
    const body =
      response.status === 204 ? {} : await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(
        body.error ?? 'Browser request failed'
      ) as Error & {
        status?: number;
      };
      error.status = response.status;
      throw error;
    }
    return body;
  }

  async function start() {
    phase = 'checking';
    errorDetail = '';
    try {
      const status = await api('/api/v1/runtimes');
      const browser = (
        status.runtimes as {
          kind: string;
          available: boolean;
          enabled: boolean;
        }[]
      ).find((r) => r.kind === 'browser');
      if (!browser || !browser.available) {
        phase = 'unavailable';
        return;
      }
      if (!browser.enabled) {
        phase = 'disabled';
        return;
      }
      phase = 'starting';
      await requestInstance();
    } catch (reason) {
      applyFailure(reason);
    }
  }

  async function requestInstance() {
    // A known instance from earlier in this session (e.g. a real
    // WebSocket disconnect, or the runtime was disabled then
    // re-enabled through Settings) is still live-tracked server-side
    // as `Stopped` -- and a `Stopped` row deliberately still counts
    // against the per-user instance limit (`RuntimeManager::
    // create_instance`'s own documented policy: it's "meant to be
    // resumed via restart_instance, not superseded by a new row").
    // Always creating a fresh instance here silently exhausted that
    // limit (default max_instances_per_user = 1) on the very first
    // reconnect attempt -- a real defect found live during Phase 6
    // Settings browser acceptance. Resume the known instance first;
    // only fall back to creating a new one if the server reports it's
    // genuinely gone (e.g. reconciled to Failed by a real clouddeskd
    // restart).
    if (instanceId) {
      try {
        const restarted = (await api(
          `/api/v1/runtime-instances/browser/${instanceId}/restart`,
          { method: 'POST' }
        )) as { state: string };
        if (restarted.state === 'running') {
          connectSocket();
        } else {
          pollStatus();
        }
        return;
      } catch (reason) {
        const status = (reason as { status?: number }).status;
        if (status !== 404) {
          applyFailure(reason);
          return;
        }
        instanceId = null;
      }
    }
    const created = (await api('/api/v1/runtime-instances', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ kind: 'browser' })
    })) as { instance_id: string; state: string };
    instanceId = created.instance_id;
    if (created.state === 'running') {
      connectSocket();
    } else {
      pollStatus();
    }
  }

  function pollStatus() {
    pollTimer = setInterval(async () => {
      if (!instanceId) return;
      try {
        const status = (await api(
          `/api/v1/runtime-instances/browser/${instanceId}`
        )) as { state: string };
        if (status.state === 'running') {
          if (pollTimer) clearInterval(pollTimer);
          connectSocket();
        } else if (status.state === 'failed') {
          phase = 'failed';
          errorDetail = 'The Browser runtime failed to become ready.';
          if (pollTimer) clearInterval(pollTimer);
        }
      } catch (reason) {
        applyFailure(reason);
        if (pollTimer) clearInterval(pollTimer);
      }
    }, 1000);
  }

  function applyFailure(reason: unknown) {
    const status = (reason as { status?: number }).status;
    if (status === 403) {
      phase = 'permission-denied';
    } else if (status === 503) {
      phase = 'unavailable';
    } else {
      phase = 'failed';
    }
    errorDetail =
      reason instanceof Error && reason.message
        ? reason.message
        : 'Something went wrong.';
  }

  function connectSocket() {
    if (!instanceId || disposed) return;
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    socket = new WebSocket(
      `${protocol}//${window.location.host}/api/v1/runtime-instances/browser/${instanceId}/browser-ws`
    );
    socket.onopen = () => {
      phase = 'running';
      requestAnimationFrame(setupResizeObserver);
    };
    socket.onmessage = (event) => {
      let message: Record<string, unknown>;
      try {
        message = JSON.parse(String(event.data));
      } catch {
        return;
      }
      handleServerMessage(message);
    };
    socket.onerror = () => {
      phase = 'failed';
      errorDetail = 'The browser connection failed.';
    };
    socket.onclose = () => {
      if (phase === 'running') {
        phase = 'disconnected';
      }
    };
  }

  function handleServerMessage(message: Record<string, unknown>) {
    switch (message.type) {
      case 'frame': {
        const data = message.data_base64 as string;
        if (!data) return;
        // Never injected as DOM/HTML -- decoded as an isolated raster
        // image and drawn onto our own canvas surface only (Task 20).
        frameImage.onload = () => {
          const ctx = canvas.getContext('2d');
          if (ctx) ctx.drawImage(frameImage, 0, 0, canvas.width, canvas.height);
        };
        frameImage.src = `data:image/jpeg;base64,${data}`;
        break;
      }
      case 'page_state': {
        const activeTab = tabs.find((t) => t.active);
        if (!message.tab_id || message.tab_id === activeTab?.tab_id) {
          if (typeof message.url === 'string') {
            pageUrl = message.url;
            addressBarValue = message.url;
          }
          if (typeof message.loading === 'boolean')
            pageLoading = message.loading;
        }
        break;
      }
      case 'tab_list': {
        if (Array.isArray(message.tabs)) {
          tabs = message.tabs as Tab[];
          const activeTab = tabs.find((t) => t.active);
          if (activeTab) {
            addressBarValue = activeTab.url || addressBarValue;
            pageLoading = activeTab.loading;
          }
        }
        break;
      }
      case 'tab_created':
      case 'tab_closed':
        break;
      // Pass 3B: downloads.
      case 'download_started':
      case 'download_progress':
      case 'download_completed':
      case 'download_failed': {
        const download = message.download as Download | undefined;
        if (!download) break;
        const index = downloads.findIndex(
          (d) => d.download_id === download.download_id
        );
        if (index === -1) downloads = [...downloads, download];
        else downloads = downloads.map((d, i) => (i === index ? download : d));
        break;
      }
      case 'download_saved': {
        saveTargetId = null;
        savePathValue = '';
        saveError = '';
        break;
      }
      // Pass 3B/3B-3: uploads/file chooser.
      case 'file_chooser_opened': {
        pendingChooserId = String(message.chooser_id ?? '') || null;
        selectSource = 'local';
        selectRootId = null;
        selectServerId = null;
        selectPathValue = '';
        selectError = '';
        void loadUploadSources();
        break;
      }
      case 'file_selected': {
        pendingChooserId = null;
        selectPathValue = '';
        selectError = '';
        break;
      }
      // Pass 3B: clipboard.
      case 'clipboard_write_ok': {
        clipboardStatus = 'Pasted.';
        break;
      }
      case 'clipboard_read': {
        const text = String(message.text ?? '');
        void navigator.clipboard
          .writeText(text)
          .then(() => {
            clipboardStatus = 'Copied.';
          })
          .catch(() => {
            clipboardStatus = 'Copy failed.';
          });
        break;
      }
      // Pass 3B: audio.
      case 'audio_started': {
        const sampleRate = Number(message.sample_rate ?? 48000);
        startAudioPlayback(sampleRate);
        break;
      }
      case 'audio_chunk': {
        const data = message.data as string | undefined;
        if (data) playAudioChunk(data);
        break;
      }
      case 'audio_stopped': {
        audioEnabled = false;
        void audioContext?.close();
        audioContext = null;
        break;
      }
      case 'error': {
        errorDetail = String(message.message ?? 'browser error');
        // A `select_file`/`save_download` rejection surfaces inline
        // next to its own control, not as the page-level error banner
        // (which would otherwise incorrectly imply the whole session
        // failed over a single denied action).
        if (pendingChooserId) selectError = errorDetail;
        if (saveTargetId) saveError = errorDetail;
        break;
      }
      case 'closed': {
        phase = 'disconnected';
        errorDetail = String(message.reason ?? 'session closed');
        break;
      }
      default:
        break;
    }
  }

  function requestSaveDownload(downloadId: string) {
    saveTargetId = downloadId;
    savePathValue =
      downloads.find((d) => d.download_id === downloadId)?.filename ?? '';
    saveError = '';
  }

  function confirmSaveDownload() {
    if (!saveTargetId || !savePathValue.trim()) return;
    send({
      type: 'save_download',
      download_id: saveTargetId,
      relative_path: savePathValue.trim()
    });
  }

  // Task 1/2 (Pass 3B-3): reuses the existing `/api/v1/code/workspaces`
  // (this user's own assigned local roots) and `/api/v1/remote/servers`
  // (this user's own registered remote servers) list APIs -- the same
  // authorized-namespace data Code and Servers already expose -- so
  // the picker only ever offers entries the user is actually
  // authorized for, never a free-text server id. The remote list is
  // best-effort: a user without `remote.servers.read` (e.g. Guest)
  // simply sees no remote option, not an error.
  async function loadUploadSources() {
    try {
      const response = await fetch('/api/v1/code/workspaces');
      if (response.ok) {
        const body = (await response.json()) as {
          workspaces: { id: string | null; label: string }[];
        };
        uploadRoots = body.workspaces.map((w) => ({
          id: w.id,
          label: w.label
        }));
      }
    } catch {
      uploadRoots = [];
    }
    try {
      const response = await fetch('/api/v1/remote/servers');
      if (response.ok) {
        const body = (await response.json()) as {
          servers: { id: string; name: string; hostname: string }[];
        };
        uploadServers = body.servers.map((s) => ({
          id: s.id,
          name: s.name,
          hostname: s.hostname
        }));
      } else {
        uploadServers = [];
      }
    } catch {
      uploadServers = [];
    }
  }

  function confirmSelectFile() {
    if (!pendingChooserId || !selectPathValue.trim()) return;
    const message: Record<string, unknown> = {
      type: 'select_file',
      chooser_id: pendingChooserId,
      relative_path: selectPathValue.trim()
    };
    if (selectSource === 'remote') {
      if (!selectServerId) return;
      message.server_id = selectServerId;
    } else if (selectRootId) {
      message.root_id = selectRootId;
    }
    send(message);
  }

  function cancelSelectFile() {
    pendingChooserId = null;
    selectPathValue = '';
    selectError = '';
  }

  async function pasteFromClipboard() {
    try {
      const text = await navigator.clipboard.readText();
      send({ type: 'clipboard_write', text });
    } catch {
      clipboardStatus = 'Clipboard access denied.';
    }
  }

  function copySelection() {
    send({ type: 'clipboard_read' });
  }

  function toggleAudio() {
    if (audioEnabled) {
      send({ type: 'audio_stop' });
      audioEnabled = false;
      void audioContext?.close();
      audioContext = null;
    } else {
      audioEnabled = true;
      send({ type: 'audio_start' });
    }
  }

  function startAudioPlayback(sampleRate: number) {
    audioContext = new AudioContext({ sampleRate });
    audioNextStartTime = audioContext.currentTime;
  }

  // Task 20: chunks are scheduled back to back starting from whichever
  // of "now" or the end of the previous chunk is later -- a slow
  // consumer never accumulates an unbounded backlog of *scheduled*
  // audio, since the server side already bounds delivery to the
  // latest quantum (see `browser_broker.rs`'s `watch` channel).
  function playAudioChunk(base64Pcm: string) {
    if (!audioContext) return;
    const bytes = Uint8Array.from(atob(base64Pcm), (c) => c.charCodeAt(0));
    const view = new DataView(bytes.buffer);
    const sampleCount = bytes.length / 2;
    const float32 = new Float32Array(sampleCount);
    for (let i = 0; i < sampleCount; i += 1) {
      float32[i] = view.getInt16(i * 2, true) / 32768;
    }
    const buffer = audioContext.createBuffer(
      1,
      sampleCount,
      audioContext.sampleRate
    );
    buffer.copyToChannel(float32, 0);
    const source = audioContext.createBufferSource();
    source.buffer = buffer;
    source.connect(audioContext.destination);
    const startAt = Math.max(audioNextStartTime, audioContext.currentTime);
    source.start(startAt);
    audioNextStartTime = startAt + buffer.duration;
  }

  function createTab() {
    send({ type: 'create_tab' });
  }

  function activateTab(tabId: string) {
    send({ type: 'activate_tab', tab_id: tabId });
  }

  function closeTab(tabId: string, event: MouseEvent | KeyboardEvent) {
    event.stopPropagation();
    send({ type: 'close_tab', tab_id: tabId });
  }

  function setupResizeObserver() {
    if (!container) return;
    resizeObserver = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      // The Browser window's component instance survives a disable ->
      // re-enable cycle (App.svelte retargets the same window rather
      // than recreating it), but `canvas` is only bound while
      // `phase === 'running'` -- a resize can still fire after phase
      // has moved on (e.g. the runtime was disabled mid-session) while
      // this observer is still attached to `container`, which stays
      // mounted across more phases than `canvas` does. Found live
      // during Phase 6 Settings browser acceptance as a real
      // `TypeError: Cannot set properties of null (setting 'width')`.
      if (!canvas) return;
      const width = Math.max(
        200,
        Math.min(1920, Math.round(entry.contentRect.width))
      );
      const height = Math.max(
        150,
        Math.min(1080, Math.round(entry.contentRect.height))
      );
      if (width === viewportWidth && height === viewportHeight) return;
      viewportWidth = width;
      viewportHeight = height;
      canvas.width = width;
      canvas.height = height;
      send({ type: 'resize', width, height });
    });
    resizeObserver.observe(container);
  }

  function send(message: Record<string, unknown>) {
    if (socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(message));
    }
  }

  // Task 21: coordinates are mapped from the rendered canvas surface
  // back to Brave's own viewport pixel space -- the canvas element may
  // be scaled by the browser (CSS layout) relative to its `width`/
  // `height` attributes, which are what the broker's viewport actually
  // is.
  function toViewportCoords(event: MouseEvent): { x: number; y: number } {
    const rect = canvas.getBoundingClientRect();
    const scaleX = canvas.width / rect.width;
    const scaleY = canvas.height / rect.height;
    return {
      x: (event.clientX - rect.left) * scaleX,
      y: (event.clientY - rect.top) * scaleY
    };
  }

  function onCanvasMouseMove(event: MouseEvent) {
    const { x, y } = toViewportCoords(event);
    send({ type: 'mouse_move', x, y });
  }

  function buttonName(button: number): 'left' | 'middle' | 'right' {
    if (button === 1) return 'middle';
    if (button === 2) return 'right';
    return 'left';
  }

  function onCanvasMouseDown(event: MouseEvent) {
    canvas.focus();
    const { x, y } = toViewportCoords(event);
    send({ type: 'mouse_down', x, y, button: buttonName(event.button) });
  }

  function onCanvasMouseUp(event: MouseEvent) {
    const { x, y } = toViewportCoords(event);
    send({ type: 'mouse_up', x, y, button: buttonName(event.button) });
  }

  function onCanvasWheel(event: WheelEvent) {
    event.preventDefault();
    const { x, y } = toViewportCoords(event);
    send({
      type: 'mouse_wheel',
      x,
      y,
      delta_x: event.deltaX,
      delta_y: event.deltaY
    });
  }

  // Task 21: keyboard capture is scoped to the Browser surface itself
  // (only fires while the canvas element has focus), never a global
  // CloudDesk-wide keyboard listener.
  function onCanvasKeyDown(event: KeyboardEvent) {
    event.preventDefault();
    const text = event.key.length === 1 ? event.key : undefined;
    send({ type: 'key_down', key: event.key, text });
  }

  function onCanvasKeyUp(event: KeyboardEvent) {
    send({ type: 'key_up', key: event.key });
  }

  function navigate() {
    if (!addressBarValue.trim()) return;
    let url = addressBarValue.trim();
    if (!/^https?:\/\//i.test(url) && url !== 'about:blank') {
      url = `https://${url}`;
    }
    send({ type: 'navigate', url });
  }

  function retry() {
    // Deliberately preserves `instanceId` (see `requestInstance`'s own
    // doc comment) -- a fresh page load still starts with `instanceId
    // === null` regardless, since it's component-local state.
    void start();
  }
</script>

<section class="browser-app">
  {#if phase === 'checking' || phase === 'starting'}
    <div class="browser-status" aria-live="polite">
      <p>Starting Browser…</p>
    </div>
  {:else if phase === 'unavailable'}
    <div class="browser-status">
      <p>Browser runtime is not available on this server.</p>
    </div>
  {:else if phase === 'disabled'}
    <div class="browser-status">
      <p>Browser runtime is disabled by the administrator.</p>
    </div>
  {:else if phase === 'permission-denied'}
    <div class="browser-status">
      <p>You do not have permission to use Browser.</p>
    </div>
  {:else if phase === 'failed'}
    <div class="browser-status">
      <p>Browser failed to start.</p>
      {#if errorDetail}<p class="detail">{errorDetail}</p>{/if}
      <button on:click={retry}>Retry</button>
    </div>
  {:else if phase === 'disconnected'}
    <div class="browser-status">
      <p>Browser session disconnected.</p>
      {#if errorDetail}<p class="detail">{errorDetail}</p>{/if}
      <button on:click={retry}>Reconnect</button>
    </div>
  {:else}
    <div class="tab-strip">
      {#each tabs as tab (tab.tab_id)}
        <button
          class="tab"
          class:active={tab.active}
          on:click={() => activateTab(tab.tab_id)}
        >
          <span class="tab-title"
            >{tab.loading
              ? 'Loading…'
              : tab.title || tab.url || 'New tab'}</span
          >
          <span
            class="tab-close"
            role="button"
            tabindex="0"
            aria-label="Close tab"
            on:click={(e) => closeTab(tab.tab_id, e)}
            on:keydown={(e) => e.key === 'Enter' && closeTab(tab.tab_id, e)}
            >×</span
          >
        </button>
      {/each}
      <button class="new-tab" on:click={createTab} aria-label="New tab"
        >+</button
      >
    </div>
    <div class="browser-toolbar">
      <input
        class="address-bar"
        type="text"
        bind:value={addressBarValue}
        placeholder="Enter a URL…"
        on:keydown={(e) => e.key === 'Enter' && navigate()}
      />
      <button on:click={navigate}>Go</button>
      {#if pageLoading}<span class="loading-indicator">Loading…</span>{/if}
      <button
        on:click={pasteFromClipboard}
        title="Paste your clipboard into the page">Paste</button
      >
      <button
        on:click={copySelection}
        title="Copy the page's selection to your clipboard">Copy</button
      >
      <button
        on:click={toggleAudio}
        class:active={audioEnabled}
        title={audioEnabled ? 'Mute page audio' : 'Play page audio'}
        >{audioEnabled ? '🔊' : '🔇'}</button
      >
      {#if clipboardStatus}<span class="loading-indicator"
          >{clipboardStatus}</span
        >{/if}
    </div>
    {#if downloads.length > 0}
      <div class="download-panel">
        {#each downloads as download (download.download_id)}
          <div class="download-row">
            <span class="download-name">{download.filename}</span>
            <span class="download-state">
              {#if download.state === 'in_progress'}
                {download.received_bytes}{download.total_bytes
                  ? ` / ${download.total_bytes}`
                  : ''}
                bytes
              {:else if download.state === 'completed'}
                complete
              {:else if download.state === 'failed'}
                failed{download.failure_reason
                  ? `: ${download.failure_reason}`
                  : ''}
              {:else}
                cancelled
              {/if}
            </span>
            {#if download.state === 'completed'}
              <button on:click={() => requestSaveDownload(download.download_id)}
                >Save to Files</button
              >
            {/if}
          </div>
        {/each}
      </div>
    {/if}
    {#if saveTargetId}
      <div class="modal-panel">
        <label for="save-path">Save as (relative to your home):</label>
        <input id="save-path" type="text" bind:value={savePathValue} />
        <button on:click={confirmSaveDownload}>Save</button>
        <button on:click={() => (saveTargetId = null)}>Cancel</button>
        {#if saveError}<span class="modal-error">{saveError}</span>{/if}
      </div>
    {/if}
    {#if pendingChooserId}
      <div class="modal-panel chooser-panel">
        <div class="chooser-source">
          <label
            ><input
              type="radio"
              name="chooser-source"
              value="local"
              bind:group={selectSource}
            /> CloudDesk file</label
          >
          <label
            ><input
              type="radio"
              name="chooser-source"
              value="remote"
              bind:group={selectSource}
              disabled={uploadServers.length === 0}
            />
            Remote server file{uploadServers.length === 0
              ? ' (none available)'
              : ''}</label
          >
        </div>
        {#if selectSource === 'local'}
          <label for="select-root">Location:</label>
          <select id="select-root" bind:value={selectRootId}>
            {#each uploadRoots as root (root.id ?? 'home')}
              <option value={root.id}>{root.label}</option>
            {/each}
          </select>
        {:else}
          <label for="select-server">Server:</label>
          <select id="select-server" bind:value={selectServerId}>
            <option value={null} disabled selected={!selectServerId}
              >Choose a server…</option
            >
            {#each uploadServers as server (server.id)}
              <option value={server.id}
                >{server.name} ({server.hostname})</option
              >
            {/each}
          </select>
        {/if}
        <label for="select-path">File path (relative):</label>
        <input id="select-path" type="text" bind:value={selectPathValue} />
        <button on:click={confirmSelectFile}>Select</button>
        <button on:click={cancelSelectFile}>Cancel</button>
        {#if selectError}<span class="modal-error">{selectError}</span>{/if}
      </div>
    {/if}
    <div class="browser-surface" bind:this={container}>
      <canvas
        bind:this={canvas}
        width={viewportWidth}
        height={viewportHeight}
        tabindex="0"
        aria-label="Remote browser page"
        on:mousemove={onCanvasMouseMove}
        on:mousedown={onCanvasMouseDown}
        on:mouseup={onCanvasMouseUp}
        on:wheel={onCanvasWheel}
        on:keydown={onCanvasKeyDown}
        on:keyup={onCanvasKeyUp}
      ></canvas>
    </div>
  {/if}
</section>

<style>
  .browser-app {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
    background: #101418;
  }
  .browser-status {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    color: #cfd8e3;
    gap: 0.75rem;
  }
  .browser-status .detail {
    color: #8a96a8;
    font-size: 0.85rem;
  }
  .tab-strip {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 0.35rem 0.5rem 0;
    background: #12161b;
    overflow-x: auto;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    max-width: 160px;
    padding: 0.3rem 0.5rem;
    border: none;
    border-radius: 6px 6px 0 0;
    background: #1a2028;
    color: #9aa9bf;
    cursor: pointer;
    font-size: 0.8rem;
  }
  .tab.active {
    background: #171c22;
    color: #e5edf5;
  }
  .tab-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tab-close {
    padding: 0 0.2rem;
    border-radius: 3px;
  }
  .tab-close:hover {
    background: #2c3947;
  }
  .new-tab {
    padding: 0.3rem 0.6rem;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: #9aa9bf;
    cursor: pointer;
  }
  .browser-toolbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem;
    background: #171c22;
    border-bottom: 1px solid #263140;
  }
  .address-bar {
    flex: 1;
    padding: 0.4rem 0.6rem;
    border-radius: 6px;
    border: 1px solid #2c3947;
    background: #0d1116;
    color: #e5edf5;
  }
  .loading-indicator {
    color: #8a96a8;
    font-size: 0.8rem;
  }
  .browser-toolbar button.active {
    background: #2c3947;
    border-radius: 4px;
  }
  .download-panel {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.4rem 0.6rem;
    background: #171c22;
    border-bottom: 1px solid #263140;
    font-size: 0.8rem;
    color: #cfd8e3;
  }
  .download-row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }
  .download-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .download-state {
    color: #8a96a8;
  }
  .modal-panel {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem;
    background: #171c22;
    border-bottom: 1px solid #263140;
    font-size: 0.85rem;
    color: #cfd8e3;
  }
  .modal-panel input {
    flex: 1;
    padding: 0.3rem 0.5rem;
    border-radius: 6px;
    border: 1px solid #2c3947;
    background: #0d1116;
    color: #e5edf5;
  }
  .modal-error {
    color: #e5787a;
  }
  .chooser-panel {
    flex-wrap: wrap;
  }
  .chooser-source {
    display: flex;
    gap: 1rem;
  }
  .modal-panel select {
    padding: 0.3rem 0.5rem;
    border-radius: 6px;
    border: 1px solid #2c3947;
    background: #0d1116;
    color: #e5edf5;
  }
  .browser-surface {
    flex: 1;
    display: flex;
    align-items: stretch;
    justify-content: stretch;
    overflow: hidden;
  }
  canvas {
    width: 100%;
    height: 100%;
    outline: none;
    background: #000;
  }
</style>
