<script lang="ts">
  import { onMount } from 'svelte';
  import { isOfficeDocument } from './office';

  // Opens a video file in the Video application. Provided by the desktop
  // shell (App.svelte), which owns window management -- FilesApp has no
  // way to open another app's window itself. Defaults to a no-op so this
  // component still renders standalone (e.g. in isolation) without a
  // parent wiring it up.
  export let onOpenWithVideo: (path: string) => void = () => {};
  // Same as onOpenWithVideo, but for the Music application.
  export let onOpenWithMusic: (path: string) => void = () => {};
  // Same as onOpenWithVideo, but for the Code application. The Code
  // runtime currently opens the user's authorized workspace as a whole
  // (Phase 7); it does not yet deep-link to this specific file/folder
  // inside it -- see PHASE7_CODE_EVIDENCE.md for the exact scope.
  export let onOpenWithCode: (path: string) => void = () => {};
  // Same as onOpenWithVideo, but for the Office application. Only the
  // absolute VFS path is passed -- the server resolves it to an opaque
  // WOPI identity and mints the scoped token, so no raw host path or
  // file id is ever constructed here (Phase 8 Task 6).
  export let onOpenWithOffice: (path: string) => void = () => {};

  const VIDEO_EXTENSIONS = new Set(['mp4', 'm4v', 'mkv', 'webm', 'mov', 'avi']);
  const AUDIO_EXTENSIONS = new Set([
    'mp3',
    'flac',
    'wav',
    'ogg',
    'oga',
    'opus',
    'm4a',
    'aac',
    'wma'
  ]);

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

  interface AclEntry {
    kind: 'owning_user' | 'user' | 'owning_group' | 'group' | 'mask' | 'other';
    name?: string;
    read: boolean;
    write: boolean;
    execute: boolean;
  }

  let path = '/';
  let entries: Entry[] = [];
  let selected = '';
  let query = '';
  let view: 'list' | 'grid' = 'list';
  let loading = true;
  let error = '';
  let preview = '';
  let acl: AclEntry[] = [];
  let aclSupported = true;
  let aclLoading = false;

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

  function isVideo(entry: Entry): boolean {
    const ext = entry.name.split('.').pop()?.toLowerCase() ?? '';
    return entry.kind === 'file' && VIDEO_EXTENSIONS.has(ext);
  }

  function isAudio(entry: Entry): boolean {
    const ext = entry.name.split('.').pop()?.toLowerCase() ?? '';
    return entry.kind === 'file' && AUDIO_EXTENSIONS.has(ext);
  }

  function isOffice(entry: Entry): boolean {
    return entry.kind === 'file' && isOfficeDocument(entry.name);
  }

  function open(entry: Entry) {
    if (entry.kind === 'directory') void load(entry.path);
    else if (isVideo(entry)) onOpenWithVideo(entry.path);
    else if (isAudio(entry)) onOpenWithMusic(entry.path);
    // Office is the default handler for document formats: nothing else
    // in CloudDesk can edit them, and the text preview below would show
    // binary noise for a DOCX/XLSX/PPTX.
    else if (isOffice(entry)) onOpenWithOffice(entry.path);
    else void showPreview(entry);
  }

  async function showPreview(entry: Entry) {
    selected = entry.path;
    preview = '';
    void loadAcl(entry);
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

  async function loadAcl(entry: Entry) {
    acl = [];
    aclLoading = true;
    try {
      const output = await action({ operation: 'read_acl', path: entry.path });
      acl = (output.entries ?? []) as AclEntry[];
      aclSupported = (output.supported ?? true) as boolean;
    } catch {
      aclSupported = false;
    } finally {
      aclLoading = false;
    }
  }

  function isArchive(entry: Entry): boolean {
    return /\.(zip|tar\.gz|tgz)$/i.test(entry.name);
  }

  function archiveFormat(entry: Entry): 'zip' | 'tar_gz' {
    return /\.zip$/i.test(entry.name) ? 'zip' : 'tar_gz';
  }

  async function createArchive() {
    if (!selectedEntry) return;
    const name = window.prompt(
      'Archive name (.zip or .tar.gz)',
      `${selectedEntry.name}.zip`
    );
    if (!name || name.includes('/')) return;
    const format = /\.zip$/i.test(name) ? 'zip' : 'tar_gz';
    await mutate({
      operation: 'create_archive',
      sources: [selectedEntry.path],
      destination: join(path, name),
      format
    });
  }

  async function extractArchive() {
    if (!selectedEntry || !isArchive(selectedEntry)) return;
    const defaultTarget = selectedEntry.name.replace(
      /\.(zip|tar\.gz|tgz)$/i,
      ''
    );
    const target = window.prompt('Extract into folder', defaultTarget);
    if (!target || target.includes('/')) return;
    await mutate({
      operation: 'extract_archive',
      archive: selectedEntry.path,
      destination: join(path, target),
      format: archiveFormat(selectedEntry)
    });
  }

  async function addAclEntry() {
    if (!selectedEntry) return;
    const username = window.prompt('Grant access to Linux user');
    if (!username) return;
    const permissions = (
      window.prompt('Permissions (e.g. r--, rw-, rwx)', 'r--') ?? ''
    ).toLowerCase();
    const entry: AclEntry = {
      kind: 'user',
      name: username,
      read: permissions.includes('r'),
      write: permissions.includes('w'),
      execute: permissions.includes('x')
    };
    error = '';
    try {
      await action({
        operation: 'set_acl',
        path: selectedEntry.path,
        entries: [entry]
      });
      await loadAcl(selectedEntry);
    } catch (reason) {
      error = reason instanceof Error ? reason.message : 'Could not update ACL';
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
    ><button disabled={!selectedEntry} onclick={() => void createArchive()}
      >Create archive</button
    ><button
      disabled={!selectedEntry || !isArchive(selectedEntry)}
      onclick={() => void extractArchive()}>Extract</button
    ><button
      disabled={!selectedEntry || !isVideo(selectedEntry)}
      onclick={() => selectedEntry && onOpenWithVideo(selectedEntry.path)}
      >Open with Video</button
    ><button
      disabled={!selectedEntry || !isAudio(selectedEntry)}
      onclick={() => selectedEntry && onOpenWithMusic(selectedEntry.path)}
      >Open with Music</button
    ><button
      disabled={!selectedEntry || !isOffice(selectedEntry)}
      onclick={() => selectedEntry && onOpenWithOffice(selectedEntry.path)}
      >Open with Office</button
    ><button
      disabled={!selectedEntry}
      onclick={() => selectedEntry && onOpenWithCode(selectedEntry.path)}
      >Open with Code</button
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
        <div class="file-acl">
          <h4>Access control list</h4>
          {#if aclLoading}<p>Loading ACL…</p>
          {:else if !aclSupported}<p>ACLs are not supported for this file.</p>
          {:else}
            <ul>
              {#each acl as entry}
                <li>
                  <span
                    >{entry.kind === 'user'
                      ? `user:${entry.name}`
                      : entry.kind === 'group'
                        ? `group:${entry.name}`
                        : entry.kind.replace('_', ' ')}</span
                  ><code
                    >{entry.read ? 'r' : '-'}{entry.write
                      ? 'w'
                      : '-'}{entry.execute ? 'x' : '-'}</code
                  >
                </li>
              {/each}
            </ul>
            <button onclick={() => void addAclEntry()}>Grant access…</button>
          {/if}
        </div>
        {#if preview}<pre>{preview}</pre>{/if}{:else}<p>
          Select a file to see its Linux metadata.
        </p>{/if}
    </aside>
  </div>
</section>
