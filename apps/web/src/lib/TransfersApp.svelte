<script lang="ts">
  import { onMount } from 'svelte';

  interface Endpoint {
    provider: 'local' | 'sftp' | 'web_dav' | 's3';
    path?: string;
    key?: string;
  }

  interface Transfer {
    id: string;
    source: Endpoint;
    destination: Endpoint;
    strategy: 'direct' | 'server-relay';
    state:
      'queued' | 'running' | 'paused' | 'completed' | 'failed' | 'cancelled';
    bytes_total: number | null;
    bytes_transferred: number;
    attempts: number;
    last_error: string | null;
    created_at: number;
  }

  let transfers: Transfer[] = [];
  let source = '';
  let destination = '';
  let loading = true;
  let busy = false;
  let error = '';

  onMount(() => {
    void load();
    const timer = window.setInterval(() => void load(false), 3000);
    return () => window.clearInterval(timer);
  });

  async function api(path: string, options?: RequestInit) {
    const response = await fetch(path, options);
    const body = response.status === 204 ? {} : await response.json();
    if (!response.ok) throw new Error(body.error ?? 'Transfer request failed');
    return body;
  }

  async function load(showSpinner = true) {
    if (showSpinner) loading = true;
    try {
      const body = await api('/api/v1/transfers');
      transfers = body.transfers ?? [];
      error = '';
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Could not load transfers';
    } finally {
      loading = false;
    }
  }

  async function create() {
    if (!source.trim() || !destination.trim()) return;
    busy = true;
    try {
      await api('/api/v1/transfers', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          source: { provider: 'local', path: source.trim() },
          destination: { provider: 'local', path: destination.trim() },
          bytes_total: null
        })
      });
      source = '';
      destination = '';
      await load(false);
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Could not create transfer';
    } finally {
      busy = false;
    }
  }

  async function control(
    transfer: Transfer,
    operation: 'pause' | 'resume' | 'cancel'
  ) {
    try {
      await api(`/api/v1/transfers/${transfer.id}/${operation}`, {
        method: 'POST'
      });
      await load(false);
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Could not update transfer';
    }
  }

  function endpoint(value: Endpoint): string {
    return value.path ?? value.key ?? value.provider;
  }

  function progress(transfer: Transfer): number {
    if (!transfer.bytes_total) return transfer.state === 'completed' ? 100 : 0;
    return Math.min(
      100,
      Math.round((transfer.bytes_transferred / transfer.bytes_total) * 100)
    );
  }
</script>

<section class="transfers-app">
  <header>
    <div>
      <p class="kicker">Persistent queue</p>
      <h2>Transfers</h2>
    </div>
    <button onclick={() => void load()}>Refresh</button>
  </header>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      void create();
    }}
  >
    <label
      >Source path<input
        bind:value={source}
        placeholder="Documents/source.iso"
      /></label
    >
    <span>→</span>
    <label
      >Destination path<input
        bind:value={destination}
        placeholder="Backups/source.iso"
      /></label
    >
    <button
      class="primary"
      disabled={busy || !source.trim() || !destination.trim()}
    >
      {busy ? 'Queueing…' : 'Queue transfer'}
    </button>
  </form>
  {#if error}<p class="files-error" role="alert">{error}</p>{/if}
  <div class="transfer-list" aria-busy={loading}>
    {#if loading}<p>Loading transfer history…</p>
    {:else if transfers.length === 0}<div class="transfer-empty">
        <strong>No transfers yet</strong><span
          >Queued work continues after this browser closes.</span
        >
      </div>
    {/if}
    {#each transfers as transfer (transfer.id)}
      <article>
        <div class="transfer-route">
          <strong>{endpoint(transfer.source)}</strong><span>→</span><strong
            >{endpoint(transfer.destination)}</strong
          >
        </div>
        <div class="transfer-meta">
          <span class={`state ${transfer.state}`}>{transfer.state}</span><span
            >{transfer.strategy}</span
          ><span>{progress(transfer)}%</span><span
            >{transfer.attempts} attempts</span
          >
        </div>
        <div class="transfer-progress">
          <i style:width={`${progress(transfer)}%`}></i>
        </div>
        {#if transfer.last_error}<p>{transfer.last_error}</p>{/if}
        <div class="transfer-controls">
          {#if transfer.state === 'queued' || transfer.state === 'running'}<button
              onclick={() => void control(transfer, 'pause')}>Pause</button
            >{/if}
          {#if transfer.state === 'paused'}<button
              onclick={() => void control(transfer, 'resume')}>Resume</button
            >{/if}
          {#if ['queued', 'running', 'paused'].includes(transfer.state)}<button
              class="danger"
              onclick={() => void control(transfer, 'cancel')}>Cancel</button
            >{/if}
        </div>
      </article>
    {/each}
  </div>
</section>
