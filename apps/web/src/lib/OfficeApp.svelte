<script lang="ts">
  import { onMount } from 'svelte';
  import {
    canRetry,
    classifyOfficeError,
    describeOfficeState,
    officeState,
    OFFICE_IFRAME_ALLOW,
    OFFICE_IFRAME_SANDBOX,
    openSessionRequest,
    safeEditorUrl,
    showsEditor,
    type OfficeError,
    type OfficeRuntimeStatus,
    type OfficeSession
  } from './office';

  // The document to open, passed by the desktop shell when the user
  // chose "Open with Office" in Files. Office is a document editor, so
  // unlike Code there is nothing meaningful to show without one.
  export let initialPath: string | null = null;

  let runtime: OfficeRuntimeStatus | null = null;
  let session: OfficeSession | null = null;
  let starting = false;
  let error: OfficeError | null = null;

  // Every decision about what to render lives in `office.ts` so it is
  // unit-testable without a component harness (see office.test.ts).
  $: state = officeState({ runtime, session, starting, error });
  $: editorUrl = session ? safeEditorUrl(session.editor_url) : null;

  onMount(() => void open());

  async function api(path: string, options?: RequestInit) {
    const response = await fetch(path, options);
    const body =
      response.status === 204 ? {} : await response.json().catch(() => ({}));
    if (!response.ok) {
      throw classifyOfficeError(
        response.status,
        typeof body.error === 'string' ? body.error : ''
      );
    }
    return body;
  }

  async function open() {
    error = null;
    session = null;
    starting = false;
    try {
      const status = await api('/api/v1/runtimes');
      runtime = (
        status.runtimes as {
          kind: string;
          available: boolean;
          enabled: boolean;
        }[]
      ).find((entry) => entry.kind === 'office') ?? {
        available: false,
        enabled: false
      };
      if (!runtime.available || !runtime.enabled) return;
      if (!initialPath) return;

      // Starting the Collabora runtime on first use can take a while, so
      // the state machine shows STARTING until the server answers.
      starting = true;
      const opened = (await api('/api/v1/office/sessions', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(openSessionRequest(initialPath))
      })) as OfficeSession;
      session = opened;
    } catch (reason) {
      error =
        reason && typeof reason === 'object' && 'kind' in reason
          ? (reason as OfficeError)
          : { kind: 'failed', message: 'Office request failed' };
    } finally {
      starting = false;
    }
  }

  $: documentName = initialPath ? (initialPath.split('/').pop() ?? '') : '';
</script>

<section class="office-app">
  {#if showsEditor(state) && editorUrl}
    <div class="office-bar">
      <span class="office-name">{documentName}</span>
      {#if state === 'READ_ONLY'}
        <span class="office-badge">Read-only</span>
      {/if}
    </div>
    <!-- The editor is Collabora's real Writer/Calc/Impress UI, served
         through CloudDesk's own authenticated proxy. CloudDesk never
         recreates an office suite of its own. -->
    <iframe
      src={editorUrl}
      title={documentName ? `Office — ${documentName}` : 'Office'}
      sandbox={OFFICE_IFRAME_SANDBOX}
      allow={OFFICE_IFRAME_ALLOW}
    ></iframe>
  {:else}
    <div
      class="office-status"
      aria-live="polite"
      role={state === 'PERMISSION_DENIED' || state === 'FAILED'
        ? 'alert'
        : undefined}
    >
      <p>{describeOfficeState(state)}</p>
      {#if error && state === 'FAILED'}<small>{error.message}</small>{/if}
      {#if !initialPath && state === 'OPENING'}
        <small>Choose a document in Files and select “Open with Office”.</small>
      {/if}
      {#if canRetry(state)}
        <button onclick={() => void open()}>Retry</button>
      {/if}
    </div>
  {/if}
</section>

<style>
  .office-app {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #1b1b1f;
  }
  .office-status {
    margin: auto;
    text-align: center;
    color: #ddd;
    padding: 2rem;
  }
  .office-status small {
    display: block;
    margin-top: 0.5rem;
    opacity: 0.7;
  }
  .office-bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.75rem;
    background: #26262b;
    color: #ccc;
    font-size: 0.85rem;
  }
  .office-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .office-badge {
    padding: 0.05rem 0.4rem;
    border: 1px solid #6b6b75;
    border-radius: 3px;
    font-size: 0.75rem;
    opacity: 0.85;
  }
  iframe {
    flex: 1;
    width: 100%;
    height: 100%;
    border: none;
  }
</style>
