<script lang="ts">
  import { onMount } from 'svelte';

  interface Endpoint {
    provider: 'local' | 'sftp' | 'scp' | 'web_dav' | 's3';
    path?: string;
    key?: string;
    server_id?: string;
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

  interface RemoteServer {
    id: string;
    name: string;
  }

  // v1 SCP scope (Task 2/28): a single side is a real RemoteServer via
  // native SCP, the other stays a plain local path -- matches the
  // backend's Local<->Scp-only support exactly. "Protocol" is
  // deliberately distinct from SFTP: choosing SCP never silently falls
  // back to SFTP.
  type SideProtocol = 'local' | 'scp';

  let transfers: Transfer[] = [];
  let remoteServers: RemoteServer[] = [];
  let sourceProtocol: SideProtocol = 'local';
  let destinationProtocol: SideProtocol = 'local';
  let source = '';
  let destination = '';
  let sourceServerId = '';
  let destinationServerId = '';
  let loading = true;
  let busy = false;
  let error = '';

  onMount(() => {
    void load();
    void loadRemoteServers();
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

  async function loadRemoteServers() {
    try {
      const body = await api('/api/v1/remote/servers');
      remoteServers = body.servers ?? [];
    } catch {
      // Remote servers are optional for this form (local-only
      // transfers still work); a failure here isn't fatal.
    }
  }

  function buildEndpoint(
    protocol: SideProtocol,
    path: string,
    serverId: string
  ): Endpoint {
    return protocol === 'scp'
      ? { provider: 'scp', server_id: serverId, path: path.trim() }
      : { provider: 'local', path: path.trim() };
  }

  function sideIsReady(
    protocol: SideProtocol,
    path: string,
    serverId: string
  ): boolean {
    return protocol === 'scp'
      ? path.trim().length > 0 && serverId.length > 0
      : path.trim().length > 0;
  }

  async function create() {
    if (
      !sideIsReady(sourceProtocol, source, sourceServerId) ||
      !sideIsReady(destinationProtocol, destination, destinationServerId)
    )
      return;
    busy = true;
    try {
      await api('/api/v1/transfers', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          source: buildEndpoint(sourceProtocol, source, sourceServerId),
          destination: buildEndpoint(
            destinationProtocol,
            destination,
            destinationServerId
          ),
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
    operation: 'pause' | 'resume' | 'cancel' | 'retry'
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
    if (value.provider === 'scp') {
      const server = remoteServers.find((s) => s.id === value.server_id);
      return `scp:${server?.name ?? value.server_id}:${value.path ?? ''}`;
    }
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
      >Source<select bind:value={sourceProtocol}
        ><option value="local">Local path</option><option value="scp"
          >Remote server (native SCP)</option
        ></select
      ></label
    >
    {#if sourceProtocol === 'scp'}<select bind:value={sourceServerId}
        ><option value="" disabled selected>Choose a server…</option
        >{#each remoteServers as server}<option value={server.id}
            >{server.name}</option
          >{/each}</select
      >{/if}
    <input
      bind:value={source}
      placeholder={sourceProtocol === 'scp'
        ? '/remote/path/file.iso'
        : 'Documents/source.iso'}
    />
    <span>→</span>
    <label
      >Destination<select bind:value={destinationProtocol}
        ><option value="local">Local path</option><option value="scp"
          >Remote server (native SCP)</option
        ></select
      ></label
    >
    {#if destinationProtocol === 'scp'}<select bind:value={destinationServerId}
        ><option value="" disabled selected>Choose a server…</option
        >{#each remoteServers as server}<option value={server.id}
            >{server.name}</option
          >{/each}</select
      >{/if}
    <input
      bind:value={destination}
      placeholder={destinationProtocol === 'scp'
        ? '/remote/path/file.iso'
        : 'Backups/source.iso'}
    />
    <button
      class="primary"
      disabled={busy ||
        !sideIsReady(sourceProtocol, source, sourceServerId) ||
        !sideIsReady(destinationProtocol, destination, destinationServerId)}
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
        {#if transfer.state === 'failed'}<p class="files-error" role="alert">
            Failed after {transfer.attempts} attempt{transfer.attempts === 1
              ? ''
              : 's'}{transfer.last_error ? `: ${transfer.last_error}` : ''}
          </p>
        {:else if transfer.last_error}<p>{transfer.last_error}</p>{/if}
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
          {#if transfer.state === 'failed'}<button
              class="primary"
              onclick={() => void control(transfer, 'retry')}>Retry</button
            >{/if}
        </div>
      </article>
    {/each}
  </div>
</section>
