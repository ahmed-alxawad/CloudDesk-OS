<script lang="ts">
  import { onMount } from 'svelte';

  interface Entry {
    name: string;
    path: string;
    kind: 'file' | 'directory' | 'symlink' | 'other';
    size: number;
    modified_at: number | null;
    mode: number;
    uid: number;
    gid: number;
  }

  let path = '/';
  let entries: Entry[] = [];
  let selected = '';
  let query = '';
  let view: 'list' | 'grid' = 'list';
  let loading = true;
  let error = '';
  let preview = '';

  $: visible = entries.filter((entry) =>
    entry.name.toLocaleLowerCase().includes(query.toLocaleLowerCase())
  );
  $: selectedEntry = entries.find((entry) => entry.path === selected);
  $: breadcrumbs = path
    .split('/')
    .filter(Boolean)
    .map((name, index, all) => ({
      name,
      path: `/${all.slice(0, index + 1).join('/')}`
    }));

  onMount(() => void load('/'));

  async function action(operation: Record<string, unknown>) {
    const response = await fetch('/api/v1/files/local/actions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(operation)
    });
    const body = (await response.json()) as {
      error?: string;
      output?: Record<string, unknown>;
    };
    if (!response.ok) throw new Error(body.error ?? 'File operation failed');
    return body.output ?? {};
  }

  async function load(nextPath: string) {
    loading = true;
    error = '';
    preview = '';
    try {
      const output = await action({ operation: 'list', path: nextPath });
      entries = (output.entries ?? []) as Entry[];
      path = nextPath;
      selected = '';
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'Could not list this folder';
    } finally {
      loading = false;
    }
  }

  function open(entry: Entry) {
    if (entry.kind === 'directory') void load(entry.path);
    else void showPreview(entry);
  }

  async function showPreview(entry: Entry) {
    selected = entry.path;
    preview = '';
    if (entry.kind !== 'file' || entry.size > 12_288) return;
    try {
      const output = await action({
        operation: 'read_preview',
        path: entry.path,
        maximum_bytes: 12_288
      });
      preview = new TextDecoder().decode(
        new Uint8Array((output.bytes ?? []) as number[])
      );
    } catch {
      preview = 'Preview is unavailable for this file.';
    }
  }

  async function createFolder() {
    const name = window.prompt('New folder name');
    if (!name || name.includes('/') || name === '.' || name === '..') return;
    await mutate({ operation: 'create_directory', path: join(path, name) });
  }

  async function renameSelected() {
    if (!selectedEntry) return;
    const name = window.prompt('Rename', selectedEntry.name);
    if (!name || name.includes('/') || name === '.' || name === '..') return;
    await mutate({
      operation: 'rename',
      from: selectedEntry.path,
      to: join(path, name)
    });
  }

  async function copySelected() {
    if (!selectedEntry || selectedEntry.kind !== 'file') return;
    const name = window.prompt('Copy name', `Copy of ${selectedEntry.name}`);
    if (!name || name.includes('/')) return;
    await mutate({
      operation: 'copy_file',
      from: selectedEntry.path,
      to: join(path, name)
    });
  }

  async function trashSelected() {
    if (
      !selectedEntry ||
      !window.confirm(`Move “${selectedEntry.name}” to trash?`)
    )
      return;
    await mutate({ operation: 'trash', path: selectedEntry.path });
  }

  async function mutate(operation: Record<string, unknown>) {
    error = '';
    try {
      await action(operation);
      await load(path);
    } catch (reason) {
      error =
        reason instanceof Error ? reason.message : 'File operation failed';
    }
  }

  function parent(value: string): string {
    const parts = value.split('/').filter(Boolean);
    parts.pop();
    return parts.length ? `/${parts.join('/')}` : '/';
  }

  function join(base: string, name: string): string {
    return base === '/' ? `/${name}` : `${base}/${name}`;
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  }
</script>

<section class="files-app">
  <header class="files-toolbar">
    <button
      aria-label="Parent folder"
      disabled={path === '/'}
      onclick={() => void load(parent(path))}>←</button
    >
    <nav aria-label="Current folder">
      <button onclick={() => void load('/')}>Home</button
      >{#each breadcrumbs as crumb}<span>/</span><button
          onclick={() => void load(crumb.path)}>{crumb.name}</button
        >{/each}
    </nav>
    <input
      bind:value={query}
      type="search"
      placeholder="Search this folder"
      aria-label="Search this folder"
    />
    <button
      class:active={view === 'list'}
      onclick={() => (view = 'list')}
      aria-label="List view">☷</button
    ><button
      class:active={view === 'grid'}
      onclick={() => (view = 'grid')}
      aria-label="Grid view">▦</button
    >
  </header>
  <div class="files-actions">
    <button onclick={() => void createFolder()}>New folder</button><button
      disabled={!selectedEntry}
      onclick={() => void renameSelected()}>Rename</button
    ><button
      disabled={!selectedEntry || selectedEntry.kind !== 'file'}
      onclick={() => void copySelected()}>Copy</button
    ><button
      class="danger"
      disabled={!selectedEntry}
      onclick={() => void trashSelected()}>Trash</button
    ><span>{visible.length} items</span>
  </div>
  {#if error}<p class="files-error" role="alert">{error}</p>{/if}
  <div class="files-body">
    <div class:grid={view === 'grid'} class="file-list" aria-busy={loading}>
      {#if loading}<p class="files-empty">
          Loading files…
        </p>{:else if visible.length === 0}<p class="files-empty">
          This folder is empty.
        </p>{/if}
      {#each visible as entry (entry.path)}
        <button
          class:selected={selected === entry.path}
          onclick={() => (selected = entry.path)}
          ondblclick={() => open(entry)}
        >
          <span class="file-icon"
            >{entry.kind === 'directory'
              ? '▰'
              : entry.kind === 'symlink'
                ? '↗'
                : '▧'}</span
          ><strong>{entry.name}</strong><small
            >{entry.kind === 'directory'
              ? 'Folder'
              : formatSize(entry.size)}</small
          ><small
            >{entry.modified_at
              ? new Date(entry.modified_at * 1000).toLocaleDateString()
              : '—'}</small
          >
        </button>
      {/each}
    </div>
    <aside class="file-details">
      {#if selectedEntry}<span class="file-icon large"
          >{selectedEntry.kind === 'directory' ? '▰' : '▧'}</span
        >
        <h3>{selectedEntry.name}</h3>
        <dl>
          <div>
            <dt>Type</dt>
            <dd>{selectedEntry.kind}</dd>
          </div>
          <div>
            <dt>Size</dt>
            <dd>{formatSize(selectedEntry.size)}</dd>
          </div>
          <div>
            <dt>Owner</dt>
            <dd>{selectedEntry.uid}:{selectedEntry.gid}</dd>
          </div>
          <div>
            <dt>Mode</dt>
            <dd>
              {(selectedEntry.mode & 0o7777).toString(8).padStart(4, '0')}
            </dd>
          </div>
        </dl>
        {#if preview}<pre>{preview}</pre>{/if}{:else}<p>
          Select a file to see its Linux metadata.
        </p>{/if}
    </aside>
  </div>
</section>
