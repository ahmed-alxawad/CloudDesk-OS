<script lang="ts">
  import { onDestroy, onMount } from 'svelte';

  export let initialPath: string | null = null;

  type Phase =
    | 'checking'
    | 'unavailable'
    | 'disabled'
    | 'starting'
    | 'running'
    | 'failed'
    | 'permission-denied';

  let phase: Phase = 'checking';
  let instanceId: string | null = null;
  let errorDetail = '';
  let pollTimer: ReturnType<typeof setInterval> | undefined;

  onMount(() => void start());
  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
  });

  async function api(path: string, options?: RequestInit) {
    const response = await fetch(path, options);
    const body =
      response.status === 204 ? {} : await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(body.error ?? 'Code request failed') as Error & {
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
      const code = (
        status.runtimes as {
          kind: string;
          available: boolean;
          enabled: boolean;
        }[]
      ).find((r) => r.kind === 'code');
      if (!code || !code.available) {
        phase = 'unavailable';
        return;
      }
      if (!code.enabled) {
        phase = 'disabled';
        return;
      }
      phase = 'starting';
      const created = (await api('/api/v1/runtime-instances', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ kind: 'code' })
      })) as { instance_id: string; state: string };
      instanceId = created.instance_id;
      if (created.state === 'running') {
        phase = 'running';
      } else {
        pollStatus();
      }
    } catch (reason) {
      applyFailure(reason);
    }
  }

  function pollStatus() {
    pollTimer = setInterval(async () => {
      if (!instanceId) return;
      try {
        const status = (await api(
          `/api/v1/runtime-instances/code/${instanceId}`
        )) as {
          state: string;
        };
        if (status.state === 'running') {
          phase = 'running';
          if (pollTimer) clearInterval(pollTimer);
        } else if (status.state === 'failed') {
          phase = 'failed';
          errorDetail = 'The Code runtime failed to become ready.';
          if (pollTimer) clearInterval(pollTimer);
        }
      } catch (reason) {
        applyFailure(reason);
        if (pollTimer) clearInterval(pollTimer);
      }
    }, 1500);
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

  $: proxyUrl = instanceId
    ? `/api/v1/runtime-instances/code/${instanceId}/proxy/`
    : '';
  $: void initialPath; // reserved: Files -> Open With Code workspace targeting
</script>

<section class="code-app">
  {#if phase === 'checking' || phase === 'starting'}
    <div class="code-status" aria-live="polite">
      <p>
        {phase === 'checking'
          ? 'Checking Code runtime…'
          : 'Starting your Code instance…'}
      </p>
    </div>
  {:else if phase === 'unavailable'}
    <div class="code-status">
      <p>Code runtime is not available on this server.</p>
    </div>
  {:else if phase === 'disabled'}
    <div class="code-status">
      <p>Code runtime is currently disabled by an administrator.</p>
    </div>
  {:else if phase === 'permission-denied'}
    <div class="code-status" role="alert">
      <p>You do not have permission to use the Code runtime.</p>
    </div>
  {:else if phase === 'failed'}
    <div class="code-status" role="alert">
      <p>Code runtime failed to start.</p>
      {#if errorDetail}<small>{errorDetail}</small>{/if}
      <button onclick={() => void start()}>Retry</button>
    </div>
  {:else if phase === 'running' && proxyUrl}
    <iframe
      src={proxyUrl}
      title="Code"
      sandbox="allow-same-origin allow-scripts allow-forms allow-popups allow-modals allow-downloads"
    ></iframe>
  {/if}
</section>

<style>
  .code-app {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #1e1e1e;
  }
  .code-status {
    margin: auto;
    text-align: center;
    color: #ddd;
    padding: 2rem;
  }
  .code-status small {
    display: block;
    margin-top: 0.5rem;
    opacity: 0.7;
  }
  iframe {
    flex: 1;
    width: 100%;
    height: 100%;
    border: none;
  }
</style>
