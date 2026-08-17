<script lang="ts">
  import { onMount } from 'svelte';

  interface DocumentEntry {
    name: string;
    path: string;
    size: number;
    modified_at: number | null;
  }

  let folder = '/';
  let docs: DocumentEntry[] = [];
  let selected: DocumentEntry | null = null;
  let loading = true;
  let error = '';

  onMount(() => void load(folder));

  async function load(path: string) {
    loading = true;
    error = '';
    selected = null;
    try {
      const response = await fetch('/api/v1/files/local/actions', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ operation: 'list', path })
      });
      const data = await response.json();
      if (!response.ok)
        throw new Error(data.error ?? 'Could not load documents');

      const entries = (data.output?.entries ?? []) as Array<{
        name: string;
        path: string;
        kind: string;
        size: number;
        modified_at: number | null;
      }>;

      const docExtensions = ['pdf', 'txt', 'md', 'json', 'log', 'csv'];
      docs = entries
        .filter((e) => {
          if (e.kind !== 'file') return false;
          const ext = e.name.split('.').pop()?.toLowerCase() ?? '';
          return docExtensions.includes(ext);
        })
        .map((e) => ({
          name: e.name,
          path: e.path,
          size: e.size,
          modified_at: e.modified_at
        }));
      folder = path;
      if (docs.length > 0) {
        selected = docs[0];
      }
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to load documents';
    } finally {
      loading = false;
    }
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  }
</script>

<section class="doc-app">
  <aside class="doc-sidebar">
    <header class="doc-sidebar-header">
      <strong>Documents</strong>
      <button onclick={() => void load('/')}>Refresh</button>
    </header>
    {#if error}
      <p class="files-error" role="alert">{error}</p>
    {/if}
    {#if loading}
      <p class="files-empty">Loading…</p>
    {:else if docs.length === 0}
      <p class="files-empty">No documents in {folder}</p>
    {:else}
      <ul class="doc-list">
        {#each docs as doc (doc.path)}
          <li>
            <button
              class="doc-item"
              class:active={selected?.path === doc.path}
              onclick={() => (selected = doc)}
            >
              <span class="doc-icon">▧</span>
              <div class="doc-meta">
                <strong>{doc.name}</strong>
                <small>{formatSize(doc.size)}</small>
              </div>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </aside>

  <main class="doc-viewer">
    {#if selected}
      <header class="doc-viewer-header">
        <span>{selected.name}</span>
        <a
          class="download-btn"
          href={`/api/v1/files/local/download?path=${encodeURIComponent(selected.path)}`}
          download={selected.name}
          target="_blank"
          rel="noopener"
        >
          Download
        </a>
      </header>
      <div class="doc-frame-wrapper">
        <iframe
          src={`/api/v1/files/local/download?path=${encodeURIComponent(selected.path)}`}
          title={selected.name}
          sandbox="allow-same-origin allow-scripts"
        ></iframe>
      </div>
    {:else}
      <div class="files-empty"><p>Select a document to preview.</p></div>
    {/if}
  </main>
</section>

<style>
  .doc-app {
    display: flex;
    height: 100%;
    box-sizing: border-box;
    overflow: hidden;
  }
  .doc-sidebar {
    width: 240px;
    border-right: 1px solid var(--border, #333);
    display: flex;
    flex-direction: column;
    background: rgba(0, 0, 0, 0.15);
  }
  .doc-sidebar-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border, #333);
  }
  .doc-list {
    list-style: none;
    margin: 0;
    padding: 6px;
    overflow-y: auto;
    flex: 1;
  }
  .doc-item {
    display: flex;
    align-items: center;
    width: 100%;
    padding: 6px 8px;
    background: none;
    border: 1px solid transparent;
    border-radius: 4px;
    text-align: left;
    color: var(--text, #eee);
    cursor: pointer;
    margin-bottom: 4px;
  }
  .doc-item:hover,
  .doc-item.active {
    background: rgba(255, 255, 255, 0.08);
    border-color: var(--accent, #4facfe);
  }
  .doc-icon {
    font-size: 16px;
    margin-right: 8px;
  }
  .doc-meta {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .doc-meta strong {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .doc-meta small {
    font-size: 10px;
    color: var(--muted, #888);
  }
  .doc-viewer {
    flex: 1;
    display: flex;
    flex-direction: column;
    background: var(--surface, #1e1e24);
  }
  .doc-viewer-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border, #333);
    background: rgba(0, 0, 0, 0.1);
  }
  .download-btn {
    padding: 4px 10px;
    font-size: 12px;
    background: var(--accent, #4facfe);
    color: #000;
    text-decoration: none;
    border-radius: 4px;
    font-weight: 500;
  }
  .doc-frame-wrapper {
    flex: 1;
    overflow: hidden;
  }
  .doc-frame-wrapper iframe {
    width: 100%;
    height: 100%;
    border: none;
    background: #fff;
  }
</style>
