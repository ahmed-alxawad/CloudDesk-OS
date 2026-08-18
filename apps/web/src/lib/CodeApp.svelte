<script lang="ts">
  import { onDestroy, onMount } from 'svelte';

  export let initialPath: string | null = null;

  type Phase =
    | 'checking'
    | 'unavailable'
    | 'disabled'
    | 'starting'
    | 'switching'
    | 'running'
    | 'failed'
    | 'permission-denied';

  type Workspace = {
    id: string | null;
    label: string;
    read_write: boolean;
    default: boolean;
  };

  let phase: Phase = 'checking';
  let instanceId: string | null = null;
  let errorDetail = '';
  let pollTimer: ReturnType<typeof setInterval> | undefined;
  let workspaces: Workspace[] = [];
  let activeWorkspaceId: string | null = null;
  // Consumed once on the very first start() -- a deep-linked file is a
  // one-shot action, not something later workspace switches replay.
  let pendingOpenPath: string | null = initialPath;

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

  async function loadWorkspaces() {
    try {
      const result = (await api('/api/v1/code/workspaces')) as {
        workspaces: Workspace[];
        last_workspace_id: string | null;
      };
      workspaces = result.workspaces;
    } catch {
      // Non-fatal: the workspace picker just stays empty/unavailable;
      // Code itself may still start against the default workspace.
      workspaces = [];
    }
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
      void loadWorkspaces();
      phase = 'starting';
      await requestInstance(undefined, pendingOpenPath ?? undefined);
      pendingOpenPath = null;
    } catch (reason) {
      applyFailure(reason);
    }
  }

  async function requestInstance(
    workspaceId?: string,
    openAbsolutePath?: string
  ) {
    const body: Record<string, string> = { kind: 'code' };
    if (workspaceId) body.workspace_id = workspaceId;
    if (openAbsolutePath) body.open_absolute_path = openAbsolutePath;
    const created = (await api('/api/v1/runtime-instances', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body)
    })) as { instance_id: string; state: string };
    instanceId = created.instance_id;
    activeWorkspaceId = workspaceId ?? activeWorkspaceId;
    if (created.state === 'running') {
      phase = 'running';
      void loadWorkspaces();
    } else {
      pollStatus();
    }
  }

  /// Switching workspace stops the current instance and starts a fresh
  /// one against the new mount (Code cannot live-remount) -- shown to
  /// the user as an explicit "Switching workspace…" phase, not a silent
  /// reload.
  async function switchWorkspace(workspaceId: string | null) {
    if (pollTimer) clearInterval(pollTimer);
    phase = 'switching';
    errorDetail = '';
    try {
      await requestInstance(workspaceId ?? undefined);
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
          void loadWorkspaces();
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
</script>

<section class="code-app">
  {#if phase === 'checking' || phase === 'starting' || phase === 'switching'}
    <div class="code-status" aria-live="polite">
      <p>
        {phase === 'checking'
          ? 'Checking Code runtime…'
          : phase === 'switching'
            ? 'Switching workspace…'
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
    {#if workspaces.length > 1}
      <div class="workspace-bar">
        <label for="code-workspace-select">Workspace</label>
        <select
          id="code-workspace-select"
          value={activeWorkspaceId ?? ''}
          onchange={(e) =>
            void switchWorkspace(
              (e.currentTarget as HTMLSelectElement).value || null
            )}
        >
          {#each workspaces as workspace (workspace.id ?? 'home')}
            <option value={workspace.id ?? ''}>
              {workspace.label}{workspace.read_write ? '' : ' (read-only)'}
            </option>
          {/each}
        </select>
      </div>
    {/if}
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
  .workspace-bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.75rem;
    background: #252526;
    color: #ccc;
    font-size: 0.85rem;
  }
  .workspace-bar select {
    background: #1e1e1e;
    color: #ccc;
    border: 1px solid #454545;
    border-radius: 3px;
    padding: 0.15rem 0.3rem;
  }
  iframe {
    flex: 1;
    width: 100%;
    height: 100%;
    border: none;
  }
</style>
