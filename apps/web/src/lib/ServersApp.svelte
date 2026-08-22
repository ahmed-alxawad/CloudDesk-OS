<script lang="ts">
  import { onMount } from 'svelte';
  import {
    AUTH_METHODS,
    authMethodFieldsAreComplete,
    buildAuthPayload,
    secretKindForAuthMethod,
    secretValueForAuthMethod,
    type AuthMethodFields
  } from './remoteServers';

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
  let agentSocketPath = '';
  let passwordValue = '';
  let privateKeyValue = '';
  let certKeyValue = '';
  let certCertValue = '';
  let kiResponses: string[] = [''];
  let tags = '';
  let unlockPassword = '';
  let steppedUp = false;
  let busy = false;
  let error = '';
  let message = '';

  // Editing an existing server's auth method only (Part E) -- a
  // separate, smaller panel from "add server", since name/hostname/
  // host key are unaffected and change through their own existing
  // flows (rescanning, deleting).
  let editingServerId: string | null = null;
  let editAuthMethod = 'ssh_agent';
  let editAgentSocketPath = '';
  let editPasswordValue = '';
  let editPrivateKeyValue = '';
  let editCertKeyValue = '';
  let editCertCertValue = '';
  let editKiResponses: string[] = [''];

  let testResults: Record<string, string> = {};

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
        body: JSON.stringify({ password: unlockPassword })
      });
      unlockPassword = '';
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

  /** Creates the Vault secret an auth method needs (if any) and
   * returns the fields to send on `RemoteServer` create/update.
   * Never sends raw secret values to `/api/v1/remote/servers` itself --
   * only the resulting opaque secret ID (Part G). The actual payload
   * shapes and validation live in `remoteServers.ts`, unit-tested
   * there; this is only the fetch side-effect. */
  async function materializeAuthFields(
    method: string,
    fields: AuthMethodFields
  ): Promise<{
    credential_secret_id: string | null;
    agent_socket_path: string | null;
  }> {
    const kind = secretKindForAuthMethod(method);
    if (kind === null) {
      return buildAuthPayload(method, null, fields.agentSocketPath);
    }
    const value = secretValueForAuthMethod(method, fields);
    const body = await api('/api/v1/vault/secrets', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        kind,
        label: `${kind} for ${name || hostname}`,
        value
      })
    });
    return buildAuthPayload(method, body.secret_id as string, null);
  }

  async function save() {
    if (!selectedKey) return;
    busy = true;
    error = '';
    try {
      const authFields = await materializeAuthFields(authMethod, {
        agentSocketPath,
        password: passwordValue,
        privateKey: privateKeyValue,
        certKey: certKeyValue,
        certCert: certCertValue,
        kiResponses
      });
      await api('/api/v1/remote/servers', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name,
          hostname,
          port,
          username,
          ...authFields,
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
      agentSocketPath = '';
      passwordValue = '';
      privateKeyValue = '';
      certKeyValue = '';
      certCertValue = '';
      kiResponses = [''];
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

  function startEdit(server: Server) {
    editingServerId = server.id;
    editAuthMethod = server.auth_method;
    editAgentSocketPath = '';
    editPasswordValue = '';
    editPrivateKeyValue = '';
    editCertKeyValue = '';
    editCertCertValue = '';
    editKiResponses = [''];
    error = '';
  }

  function cancelEdit() {
    editingServerId = null;
  }

  async function saveEdit(server: Server) {
    busy = true;
    error = '';
    try {
      const authFields = await materializeAuthFields(editAuthMethod, {
        agentSocketPath: editAgentSocketPath,
        password: editPasswordValue,
        privateKey: editPrivateKeyValue,
        certKey: editCertKeyValue,
        certCert: editCertCertValue,
        kiResponses: editKiResponses
      });
      await api(`/api/v1/remote/servers/${server.id}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(authFields)
      });
      editingServerId = null;
      message = `${server.name}: authentication method updated.`;
      await load();
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Could not update server';
    } finally {
      busy = false;
    }
  }

  async function testConnection(server: Server) {
    busy = true;
    error = '';
    try {
      const body = await api(
        `/api/v1/remote/servers/${server.id}/test-connection`,
        {
          method: 'POST'
        }
      );
      testResults = {
        ...testResults,
        [server.id]: body.connected
          ? 'Connected.'
          : `Failed: ${body.reason ?? 'connection failed'}`
      };
    } catch (reason) {
      testResults = {
        ...testResults,
        [server.id]: reason instanceof Error ? reason.message : 'Test failed'
      };
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
          <p>Auth: {server.auth_method}</p>
          <div class="server-tags">
            {#each server.tags as tag}<span>{tag}</span>{/each}
          </div>
          {#if testResults[server.id]}<p class="settings-message">
              {testResults[server.id]}
            </p>{/if}
          <footer>
            <button onclick={() => void verify(server)}>Verify host key</button
            ><button
              disabled={busy || !steppedUp}
              onclick={() => void testConnection(server)}
              >Test connection</button
            ><button disabled={!steppedUp} onclick={() => startEdit(server)}
              >Change auth method</button
            ><button
              class="danger"
              disabled={!steppedUp}
              onclick={() => void remove(server)}>Delete</button
            >
          </footer>
          {#if editingServerId === server.id}<form
              onsubmit={(event) => {
                event.preventDefault();
                void saveEdit(server);
              }}
            >
              <select bind:value={editAuthMethod}
                >{#each AUTH_METHODS as method}<option value={method.value}
                    >{method.label}</option
                  >{/each}</select
              >
              {#if editAuthMethod === 'ssh_agent'}<input
                  bind:value={editAgentSocketPath}
                  placeholder="Agent socket path, e.g. /run/user/1000/ssh-agent"
                  required
                />{:else if editAuthMethod === 'password'}<input
                  bind:value={editPasswordValue}
                  type="password"
                  placeholder="SSH password"
                  required
                />{:else if editAuthMethod === 'private_key'}<textarea
                  bind:value={editPrivateKeyValue}
                  placeholder="Private key (PEM/OpenSSH format)"
                  required
                ></textarea>{:else if editAuthMethod === 'certificate'}<textarea
                  bind:value={editCertKeyValue}
                  placeholder="Private key (PEM/OpenSSH format)"
                  required
                ></textarea>
                <textarea
                  bind:value={editCertCertValue}
                  placeholder="OpenSSH user certificate (-cert.pub)"
                  required

                ></textarea>{:else if editAuthMethod === 'keyboard_interactive'}{#each editKiResponses as _, index}<input
                    bind:value={editKiResponses[index]}
                    placeholder={`Response ${index + 1}`}
                  />{/each}<button
                  type="button"
                  onclick={() => (editKiResponses = [...editKiResponses, ''])}
                  >Add response</button
                >{/if}
              <button
                class="primary"
                disabled={busy ||
                  !authMethodFieldsAreComplete(editAuthMethod, {
                    agentSocketPath: editAgentSocketPath,
                    password: editPasswordValue,
                    privateKey: editPrivateKeyValue,
                    certKey: editCertKeyValue,
                    certCert: editCertCertValue,
                    kiResponses: editKiResponses
                  })}>Save auth method</button
              ><button type="button" onclick={cancelEdit}>Cancel</button>
            </form>{/if}
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
            bind:value={unlockPassword}
            type="password"
            autocomplete="current-password"
            placeholder="Current password"
          /><button class="primary" disabled={busy || !unlockPassword}
            >Unlock</button
          >
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
            >{#each AUTH_METHODS as method}<option value={method.value}
                >{method.label}</option
              >{/each}</select
          >
          {#if authMethod === 'ssh_agent'}<input
              bind:value={agentSocketPath}
              placeholder="Agent socket path, e.g. /run/user/1000/ssh-agent"
              required
            />
            <p class="settings-message">
              Must be a real `ssh-agent` socket owned by your own Linux account
              -- re-checked on every connection.
            </p>{:else if authMethod === 'password'}<input
              bind:value={passwordValue}
              type="password"
              placeholder="SSH password"
              required
            />{:else if authMethod === 'private_key'}<textarea
              bind:value={privateKeyValue}
              placeholder="Private key (PEM/OpenSSH format)"
              required
            ></textarea>{:else if authMethod === 'certificate'}<textarea
              bind:value={certKeyValue}
              placeholder="Private key (PEM/OpenSSH format)"
              required
            ></textarea>
            <textarea
              bind:value={certCertValue}
              placeholder="OpenSSH user certificate (-cert.pub)"
              required
            ></textarea>{:else if authMethod === 'keyboard_interactive'}<p
              class="settings-message"
            >
              Responses are replayed in order against the server's real prompts
              (e.g. its account password) -- not a live per-connection
              challenge.
            </p>
            {#each kiResponses as _, index}<input
                bind:value={kiResponses[index]}
                placeholder={`Response ${index + 1}`}
              />{/each}<button
              type="button"
              onclick={() => (kiResponses = [...kiResponses, ''])}
              >Add response</button
            >{/if}
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
            <button
              class="primary"
              disabled={busy ||
                !selectedKey ||
                !authMethodFieldsAreComplete(authMethod, {
                  agentSocketPath,
                  password: passwordValue,
                  privateKey: privateKeyValue,
                  certKey: certKeyValue,
                  certCert: certCertValue,
                  kiResponses
                })}>Save pinned server</button
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
