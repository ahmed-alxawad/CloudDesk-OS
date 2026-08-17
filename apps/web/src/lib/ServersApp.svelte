<script lang="ts">
  import { onMount } from 'svelte';

  interface HostKey {
    key_type: string;
    key_base64: string;
    fingerprint: string;
  }
  interface Server {
    id: string;
    name: string;
    hostname: string;
    port: number;
    username: string;
    auth_method: string;
    host_key_type: string;
    host_key_fingerprint: string;
    tags: string[];
  }

  let servers: Server[] = [];
  let keys: HostKey[] = [];
  let selectedKey: HostKey | null = null;
  let name = '';
  let hostname = '';
  let port = 22;
  let username = '';
  let authMethod = 'ssh_agent';
  let credentialSecretId = '';
  let tags = '';
  let password = '';
  let steppedUp = false;
  let busy = false;
  let error = '';
  let message = '';

  onMount(() => void load());

  async function api(path: string, options?: RequestInit) {
    const response = await fetch(path, options);
    const body = response.status === 204 ? {} : await response.json();
    if (!response.ok)
      throw new Error(body.error ?? 'Remote server request failed');
    return body;
  }

  async function load() {
    try {
      const body = await api('/api/v1/remote/servers');
      servers = body.servers ?? [];
    } catch (reason) {
      error =
        reason instanceof Error
          ? reason.message
          : 'Could not load remote servers';
    }
  }

  async function stepUp() {
    busy = true;
    try {
      await api('/api/v1/auth/step-up', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ password })
      });
      password = '';
      steppedUp = true;
      message = 'Server changes unlocked for five minutes.';
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Authentication failed';
    } finally {
      busy = false;
    }
  }

  async function scan() {
    busy = true;
    error = '';
    try {
      const body = await api('/api/v1/remote/host-keys/scan', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ hostname, port })
      });
      keys = body.keys ?? [];
      selectedKey = null;
      message = body.warning;
    } catch (reason) {
      error = reason instanceof Error ? reason.message : 'Host-key scan failed';
    } finally {
      busy = false;
    }
  }

  async function save() {
    if (!selectedKey) return;
    busy = true;
    try {
      await api('/api/v1/remote/servers', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name,
          hostname,
          port,
          username,
          auth_method: authMethod,
          credential_secret_id:
            authMethod === 'ssh_agent' ? null : credentialSecretId,
          host_key_type: selectedKey.key_type,
          host_key_base64: selectedKey.key_base64,
          proxy_jump_server_id: null,
          tags: tags
            .split(',')
            .map((tag) => tag.trim())
            .filter(Boolean)
        })
      });
      name = '';
      hostname = '';
      username = '';
      keys = [];
      selectedKey = null;
      await load();
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Could not save server';
    } finally {
      busy = false;
    }
  }

  async function verify(server: Server) {
    try {
      await api(`/api/v1/remote/servers/${server.id}/verify-host-key`, {
        method: 'POST'
      });
      message = `${server.name}: host identity verified.`;
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Host verification failed';
    }
  }

  async function remove(server: Server) {
    if (!window.confirm(`Delete ${server.name}?`)) return;
    try {
      await api(`/api/v1/remote/servers/${server.id}`, { method: 'DELETE' });
      await load();
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Could not delete server';
    }
  }
</script>

<section class="servers-app">
  <header>
    <div>
      <p class="kicker">Strict host identity</p>
      <h2>Remote servers</h2>
    </div>
    <span>{servers.length} saved</span>
  </header>
  {#if error}<p class="files-error" role="alert">{error}</p>{/if}
  {#if message}<p class="settings-message">{message}</p>{/if}
  <div class="server-layout">
    <div class="server-list">
      {#if servers.length === 0}<p>No servers saved.</p>{/if}
      {#each servers as server (server.id)}<article>
          <div>
            <strong>{server.name}</strong><span
              >{server.username}@{server.hostname}:{server.port}</span
            >
          </div>
          <code>{server.host_key_fingerprint}</code>
          <div class="server-tags">
            {#each server.tags as tag}<span>{tag}</span>{/each}
          </div>
          <footer>
            <button onclick={() => void verify(server)}>Verify host key</button
            ><button
              class="danger"
              disabled={!steppedUp}
              onclick={() => void remove(server)}>Delete</button
            >
          </footer>
        </article>{/each}
    </div>
    <aside>
      {#if !steppedUp}<form
          onsubmit={(event) => {
            event.preventDefault();
            void stepUp();
          }}
        >
          <h3>Unlock server changes</h3>
          <input
            bind:value={password}
            type="password"
            autocomplete="current-password"
            placeholder="Current password"
          /><button class="primary" disabled={busy || !password}>Unlock</button>
        </form>{:else}
        <form
          onsubmit={(event) => {
            event.preventDefault();
            void (keys.length ? save() : scan());
          }}
        >
          <h3>Add server</h3>
          <input bind:value={name} placeholder="Display name" required />
          <div>
            <input
              bind:value={hostname}
              placeholder="Hostname or IP"
              required
            /><input
              bind:value={port}
              type="number"
              min="1"
              max="65535"
              aria-label="SSH port"
            />
          </div>
          <input bind:value={username} placeholder="SSH username" required />
          <select bind:value={authMethod}
            ><option value="ssh_agent">SSH agent</option><option
              value="password">Password secret</option
            ><option value="private_key">Private-key secret</option><option
              value="certificate">Certificate secret</option
            ><option value="keyboard_interactive"
              >Keyboard-interactive secret</option
            ></select
          >
          {#if authMethod !== 'ssh_agent'}<input
              bind:value={credentialSecretId}
              placeholder="Vault secret ID"
              required
            />{/if}
          <input bind:value={tags} placeholder="Tags, comma separated" />
          {#if keys.length === 0}<button class="primary" disabled={busy}
              >Scan host keys</button
            >{:else}<div class="host-keys">
              {#each keys as key}<label
                  ><input
                    type="radio"
                    name="host-key"
                    checked={selectedKey === key}
                    onchange={() => (selectedKey = key)}
                  /><span
                    ><strong>{key.key_type}</strong><code
                      >{key.fingerprint}</code
                    ></span
                  ></label
                >{/each}
            </div>
            <button class="primary" disabled={busy || !selectedKey}
              >Save pinned server</button
            ><button
              type="button"
              onclick={() => {
                keys = [];
                selectedKey = null;
              }}>Scan again</button
            >{/if}
        </form>
      {/if}
    </aside>
  </div>
</section>
