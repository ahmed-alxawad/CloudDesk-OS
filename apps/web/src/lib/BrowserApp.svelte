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

  onMount(() => void start());
  onDestroy(() => {
    disposed = true;
    if (pollTimer) clearInterval(pollTimer);
    resizeObserver?.disconnect();
    socket?.close();
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
      case 'error': {
        errorDetail = String(message.message ?? 'browser error');
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
    instanceId = null;
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
    </div>
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
