<script lang="ts">
  import { onMount } from 'svelte';
  import {
    canManageRuntimes,
    describeRuntimeError,
    describeRuntimeStatus,
    runtimeDisplayName,
    sanitizeDetail,
    visibleRuntimeCards,
    type RuntimeStatus
  } from './runtime';

  interface Summary {
    hostname: string;
    kernel: string;
    uptime_seconds: number;
    load_average: number[];
    memory_total_kib: number | null;
    memory_available_kib: number | null;
    container_engines: { docker: boolean; podman: boolean };
  }

  let summary: Summary | null = null;
  let password = '';
  let steppedUp = false;
  let serviceUnit = 'ssh.service';
  let serviceOperation = 'restart';
  let busy = false;
  let message = '';
  let error = '';

  // Runtime section state (Phase 6 Task 1-4).
  let runtimeCards: RuntimeStatus[] = [];
  let runtimeCapable = false;
  let runtimeLoading = true;
  let runtimeError = '';
  let runtimeActionError = '';
  let runtimeBusyKind: string | null = null;

  onMount(() => void load());
  onMount(() => void loadRuntimes());

  async function api(path: string, options?: RequestInit) {
    const response = await fetch(path, options);
    const body = response.status === 204 ? {} : await response.json();
    if (!response.ok) throw new Error(body.error ?? 'System request failed');
    return body;
  }

  async function load() {
    try {
      summary = await api('/api/v1/system/summary');
    } catch (reason) {
      error =
        reason instanceof Error
          ? reason.message
          : 'Could not load system information';
    }
  }

  async function loadRuntimes() {
    runtimeLoading = true;
    runtimeError = '';
    try {
      const [runtimesBody, principal] = await Promise.all([
        api('/api/v1/runtimes'),
        api('/api/v1/auth/me')
      ]);
      runtimeCards = visibleRuntimeCards(
        (runtimesBody.runtimes ?? []) as RuntimeStatus[]
      );
      runtimeCapable = canManageRuntimes(
        (principal.capabilities ?? []) as string[]
      );
    } catch (reason) {
      runtimeError = describeRuntimeError(reason);
      // Still show the three product cards in an unavailable state
      // rather than an endless spinner (Task 4).
      runtimeCards = visibleRuntimeCards([]);
    } finally {
      runtimeLoading = false;
    }
  }

  async function toggleRuntime(kind: string, enable: boolean) {
    if (runtimeBusyKind) return;
    runtimeBusyKind = kind;
    runtimeActionError = '';
    try {
      await api(
        `/api/v1/runtimes/${encodeURIComponent(kind)}/${enable ? 'enable' : 'disable'}`,
        {
          method: 'POST'
        }
      );
      await loadRuntimes();
    } catch (reason) {
      runtimeActionError = describeRuntimeError(reason);
    } finally {
      runtimeBusyKind = null;
    }
  }

  async function stepUp() {
    busy = true;
    error = '';
    try {
      await api('/api/v1/auth/step-up', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ password })
      });
      password = '';
      steppedUp = true;
      message = 'High-risk actions unlocked for five minutes.';
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Authentication failed';
    } finally {
      busy = false;
    }
  }

  async function serviceControl() {
    if (!window.confirm(`${serviceOperation} ${serviceUnit}?`)) return;
    await mutate('/api/v1/system/services/control', {
      unit: serviceUnit,
      operation: serviceOperation
    });
  }

  async function power(operation: 'reboot' | 'shutdown') {
    if (
      !window.confirm(
        `${operation === 'reboot' ? 'Reboot' : 'Shut down'} this server now?`
      )
    )
      return;
    await mutate('/api/v1/system/power', { operation });
  }

  async function mutate(path: string, body: Record<string, string>) {
    busy = true;
    error = '';
    message = '';
    try {
      await api(path, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
      });
      message = 'System operation accepted.';
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'System operation failed';
    } finally {
      busy = false;
    }
  }

  function gibibytes(kib: number | null): string {
    return kib === null ? 'Unknown' : `${(kib / 1024 / 1024).toFixed(2)} GiB`;
  }
</script>

<section class="settings-app">
  <header>
    <p class="kicker">Host administration</p>
    <h2>System settings</h2>
  </header>
  {#if error}<p class="files-error" role="alert">{error}</p>{/if}
  {#if message}<p class="settings-message" aria-live="polite">{message}</p>{/if}
  {#if summary}
    <div class="system-cards">
      <article>
        <span>Hostname</span><strong>{summary.hostname}</strong><small
          >{summary.kernel}</small
        >
      </article>
      <article>
        <span>Uptime</span><strong
          >{Math.floor(summary.uptime_seconds / 3600)}h</strong
        ><small
          >Load {summary.load_average
            .map((value) => value.toFixed(2))
            .join(' · ')}</small
        >
      </article>
      <article>
        <span>Memory available</span><strong
          >{gibibytes(summary.memory_available_kib)}</strong
        ><small>of {gibibytes(summary.memory_total_kib)}</small>
      </article>
      <article>
        <span>Containers</span><strong
          >{summary.container_engines.docker || summary.container_engines.podman
            ? 'Available'
            : 'Not detected'}</strong
        ><small
          >Docker {summary.container_engines.docker ? 'on' : 'off'} · Podman {summary
            .container_engines.podman
            ? 'on'
            : 'off'}</small
        >
      </article>
    </div>
  {/if}
  <div class="settings-grid">
    <article>
      <h3>Administrative step-up</h3>
      <p>
        Re-enter your password before service, package, firewall, or power
        changes.
      </p>
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void stepUp();
        }}
      >
        <input
          bind:value={password}
          type="password"
          autocomplete="current-password"
          placeholder="Current password"
        />
        <button class="primary" disabled={busy || !password}>Unlock</button>
      </form>
    </article>
    <article class:locked={!steppedUp}>
      <h3>Service control</h3>
      <p>
        Actions are sent as typed grants; shell commands are never accepted.
      </p>
      <input bind:value={serviceUnit} aria-label="Service unit" />
      <select bind:value={serviceOperation} aria-label="Service operation"
        ><option>start</option><option>stop</option><option>restart</option
        ><option>enable</option><option>disable</option></select
      >
      <button
        disabled={busy || !steppedUp}
        onclick={() => void serviceControl()}>Apply</button
      >
    </article>
    <article class:locked={!steppedUp}>
      <h3>Power</h3>
      <p>
        Power operations require both permission and a fresh step-up session.
      </p>
      <div>
        <button
          disabled={busy || !steppedUp}
          onclick={() => void power('reboot')}>Reboot</button
        ><button
          class="danger"
          disabled={busy || !steppedUp}
          onclick={() => void power('shutdown')}>Shut down</button
        >
      </div>
    </article>
  </div>

  <header class="runtime-header">
    <p class="kicker">Optional runtimes</p>
    <h2>Runtime</h2>
  </header>
  {#if runtimeError}<p class="files-error" role="alert">{runtimeError}</p>{/if}
  {#if runtimeActionError}<p class="files-error" role="alert">
      {runtimeActionError}
    </p>{/if}
  {#if runtimeLoading}
    <p aria-live="polite">Loading runtime status…</p>
  {:else}
    <div class="settings-grid" data-testid="runtime-cards">
      {#each runtimeCards as card (card.kind)}
        <article>
          <h3>{runtimeDisplayName(card.kind)}</h3>
          <p class="runtime-status" data-status={describeRuntimeStatus(card)}>
            {describeRuntimeStatus(card)}
          </p>
          {#if card.available}
            <small>{sanitizeDetail(card.detail)}</small>
            <small
              >{card.instance_count} running instance{card.instance_count === 1
                ? ''
                : 's'}</small
            >
          {:else}
            <small>{sanitizeDetail(card.detail)}</small>
          {/if}
          {#if runtimeCapable}
            <button
              disabled={runtimeBusyKind === card.kind}
              onclick={() => void toggleRuntime(card.kind, !card.enabled)}
            >
              {card.enabled ? 'Disable' : 'Enable'}
            </button>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</section>
