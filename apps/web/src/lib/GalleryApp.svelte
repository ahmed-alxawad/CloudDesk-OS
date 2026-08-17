<script lang="ts">
  import { onMount } from 'svelte';

  interface ImageEntry {
    name: string;
    path: string;
    size: number;
    modified_at: number | null;
  }

  let folder = '/';
  let images: ImageEntry[] = [];
  let selected: ImageEntry | null = null;
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
      if (!response.ok) throw new Error(data.error ?? 'Could not load images');

      const entries = (data.output?.entries ?? []) as Array<{
        name: string;
        path: string;
        kind: string;
        size: number;
        modified_at: number | null;
      }>;

      const imageExtensions = [
        'png',
        'jpg',
        'jpeg',
        'gif',
        'webp',
        'svg',
        'bmp',
        'ico'
      ];
      images = entries
        .filter((e) => {
          if (e.kind !== 'file') return false;
          const ext = e.name.split('.').pop()?.toLowerCase() ?? '';
          return imageExtensions.includes(ext);
        })
        .map((e) => ({
          name: e.name,
          path: e.path,
          size: e.size,
          modified_at: e.modified_at
        }));
      folder = path;
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to load gallery';
    } finally {
      loading = false;
    }
  }

  function selectImage(img: ImageEntry) {
    selected = img;
  }

  function nextImage() {
    if (!selected || images.length === 0) return;
    const index = images.findIndex((img) => img.path === selected?.path);
    if (index >= 0 && index < images.length - 1) {
      selected = images[index + 1];
    } else {
      selected = images[0];
    }
  }

  function prevImage() {
    if (!selected || images.length === 0) return;
    const index = images.findIndex((img) => img.path === selected?.path);
    if (index > 0) {
      selected = images[index - 1];
    } else {
      selected = images[images.length - 1];
    }
  }
</script>

<section class="gallery-app">
  <header class="gallery-toolbar">
    <button onclick={() => void load('/')}>Home</button>
    <span>{images.length} images found</span>
  </header>

  {#if error}
    <p class="files-error" role="alert">{error}</p>
  {/if}

  {#if loading}
    <div class="files-empty"><p>Loading gallery…</p></div>
  {:else if images.length === 0}
    <div class="files-empty">
      <p>No images found in {folder}.</p>
    </div>
  {:else}
    <div class="gallery-grid">
      {#each images as img (img.path)}
        <button
          class="gallery-card"
          class:active={selected?.path === img.path}
          onclick={() => selectImage(img)}
        >
          <img
            src={`/api/v1/media/preview?path=${encodeURIComponent(img.path)}`}
            alt={img.name}
            loading="lazy"
          />
          <span class="gallery-caption">{img.name}</span>
        </button>
      {/each}
    </div>
  {/if}

  {#if selected}
    <div
      class="lightbox"
      role="dialog"
      aria-modal="true"
      aria-label="Image preview"
    >
      <div class="lightbox-content">
        <header class="lightbox-header">
          <strong>{selected.name}</strong>
          <div class="lightbox-controls">
            <button onclick={prevImage} aria-label="Previous image">‹</button>
            <button onclick={nextImage} aria-label="Next image">›</button>
            <button onclick={() => (selected = null)} aria-label="Close preview"
              >✕</button
            >
          </div>
        </header>
        <div class="lightbox-image-wrapper">
          <img
            src={`/api/v1/media/stream?path=${encodeURIComponent(selected.path)}`}
            alt={selected.name}
          />
        </div>
      </div>
    </div>
  {/if}
</section>

<style>
  .gallery-app {
    display: flex;
    flex-direction: column;
    height: 100%;
    padding: 12px;
    box-sizing: border-box;
    overflow: hidden;
  }
  .gallery-toolbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--border, #333);
  }
  .gallery-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
    gap: 12px;
    padding-top: 12px;
    overflow-y: auto;
    flex: 1;
  }
  .gallery-card {
    display: flex;
    flex-direction: column;
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 6px;
    padding: 6px;
    cursor: pointer;
    text-align: center;
    transition:
      transform 0.15s ease,
      border-color 0.15s ease;
  }
  .gallery-card:hover,
  .gallery-card.active {
    border-color: var(--accent, #4facfe);
    transform: translateY(-2px);
  }
  .gallery-card img {
    width: 100%;
    height: 100px;
    object-fit: cover;
    border-radius: 4px;
  }
  .gallery-caption {
    font-size: 11px;
    margin-top: 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text, #eee);
  }
  .lightbox {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.85);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }
  .lightbox-content {
    display: flex;
    flex-direction: column;
    max-width: 90%;
    max-height: 90%;
    background: #1e1e24;
    border-radius: 8px;
    border: 1px solid #444;
    overflow: hidden;
  }
  .lightbox-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    background: #141418;
    color: #fff;
  }
  .lightbox-controls button {
    background: none;
    border: 1px solid #555;
    color: #fff;
    padding: 4px 8px;
    margin-left: 4px;
    cursor: pointer;
    border-radius: 4px;
  }
  .lightbox-image-wrapper {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 12px;
    overflow: auto;
  }
  .lightbox-image-wrapper img {
    max-width: 100%;
    max-height: 70vh;
    object-fit: contain;
  }
</style>
