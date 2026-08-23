<script lang="ts">
  import { onDestroy } from 'svelte';
  import {
    conversionOperationFor,
    formatTime,
    hasPlayedEnoughToRecord,
    insertIntoQueue,
    isTerminalJobState,
    playbackUrl,
    removeFromQueue,
    trackDisplayTitle,
    trackSubtitle,
    truncateForDisplay,
    type JobState,
    type QueueInsertMode,
    type StreamPlan,
    type TrackSummary
  } from './music';

  // Set by the desktop shell when opened from Files. Reactive to prop
  // changes (not just mount), matching the Video app's pattern.
  export let initialPath: string | null = null;

  type View =
    | 'library'
    | 'artists'
    | 'albums'
    | 'playlists'
    | 'favorites'
    | 'search'
    | 'queue';

  interface Root {
    id: string;
    path: string;
  }
  interface Playlist {
    id: string;
    name: string;
  }
  interface PlaylistEntry {
    entry_id: string;
    track: TrackSummary;
  }

  let view: View = 'library';
  let error = '';
  let notice = '';

  let roots: Root[] = [];
  let tracks: TrackSummary[] = [];
  let totalTracks = 0;
  let artists: string[] = [];
  let albums: { album: string; artist: string | null; year: string | null }[] =
    [];
  let playlists: Playlist[] = [];
  let favorites: TrackSummary[] = [];
  let recent: TrackSummary[] = [];
  let searchQuery = '';
  let searchResults: TrackSummary[] = [];
  let scanning = false;

  let selectedPlaylistId: string | null = null;
  let selectedPlaylistEntries: PlaylistEntry[] = [];

  // -- playback state --
  let queue: string[] = [];
  let queueTracks: Map<string, TrackSummary> = new Map();
  let currentIndex: number | null = null;
  let currentTrack: TrackSummary | null = null;
  let plan: StreamPlan | null = null;
  let jobId: string | null = null;
  let jobState: JobState | null = null;
  let jobPollTimer: ReturnType<typeof setInterval> | undefined;
  let audioUrl: string | null = null;
  let loadingTrack = false;
  let audioEl: HTMLAudioElement | undefined;
  let playing = false;
  let currentTime = 0;
  let duration = 0;
  let volume = 1;
  let muted = false;
  let recordedThisTrack = false;

  $: currentTitle = currentTrack
    ? truncateForDisplay(trackDisplayTitle(currentTrack))
    : '';
  $: currentSubtitle = currentTrack ? trackSubtitle(currentTrack) : '';
  $: artworkUrl = currentTrack
    ? `/api/v1/music/tracks/${encodeURIComponent(currentTrack.id)}/artwork`
    : null;
  $: if (initialPath && initialPath !== loadedInitialPath) {
    loadedInitialPath = initialPath;
    void openFileFromFiles(initialPath);
  }
  let loadedInitialPath: string | null = null;

  async function api(input: string, init?: RequestInit): Promise<Response> {
    return fetch(input, {
      ...init,
      headers: init?.body
        ? { 'Content-Type': 'application/json', ...init.headers }
        : init?.headers
    });
  }

  async function readError(
    response: Response,
    fallback: string
  ): Promise<string> {
    if (response.status === 503)
      return 'Media conversion (FFmpeg) is currently disabled.';
    if (response.status === 404) return 'Not found.';
    if (response.status === 403 || response.status === 401)
      return 'Not authorized.';
    const body = (await response.json().catch(() => ({}))) as {
      error?: string;
    };
    return body.error ?? fallback;
  }

  async function loadAll() {
    await Promise.all([
      loadRoots(),
      loadTracks(),
      loadArtists(),
      loadAlbums(),
      loadPlaylists(),
      loadFavorites(),
      loadRecent(),
      loadQueue()
    ]);
  }

  async function loadRoots() {
    const response = await api('/api/v1/music/roots');
    if (response.ok) roots = (await response.json()) as Root[];
  }

  async function loadTracks() {
    const response = await api('/api/v1/music/tracks?limit=200');
    if (response.ok) {
      const body = (await response.json()) as {
        tracks: TrackSummary[];
        total: number;
      };
      tracks = body.tracks;
      totalTracks = body.total;
      for (const track of tracks) queueTracks.set(track.id, track);
    }
  }

  async function loadArtists() {
    const response = await api('/api/v1/music/artists');
    if (response.ok) artists = (await response.json()) as string[];
  }

  async function loadAlbums() {
    const response = await api('/api/v1/music/albums');
    if (response.ok) albums = await response.json();
  }

  async function loadPlaylists() {
    const response = await api('/api/v1/music/playlists');
    if (response.ok) playlists = (await response.json()) as Playlist[];
  }

  async function loadFavorites() {
    const response = await api('/api/v1/music/favorites');
    if (response.ok) favorites = (await response.json()) as TrackSummary[];
  }

  async function loadRecent() {
    const response = await api('/api/v1/music/recent');
    if (response.ok) recent = (await response.json()) as TrackSummary[];
  }

  async function loadQueue() {
    const response = await api('/api/v1/music/queue');
    if (response.ok) {
      const body = (await response.json()) as { track_ids: string[] };
      queue = body.track_ids;
    }
  }

  async function persistQueue() {
    await api('/api/v1/music/queue', {
      method: 'PUT',
      body: JSON.stringify({ track_ids: queue })
    });
  }

  async function addRootPrompt() {
    const path = window.prompt('Music folder (path within your files)', '/');
    if (!path) return;
    error = '';
    const response = await api('/api/v1/music/roots', {
      method: 'POST',
      body: JSON.stringify({ path })
    });
    if (!response.ok) {
      error = await readError(response, 'Could not add this folder.');
      return;
    }
    const root = (await response.json()) as Root;
    await scanRoot(root.id);
    await loadAll();
  }

  async function scanRoot(rootId: string) {
    scanning = true;
    error = '';
    try {
      const response = await api(`/api/v1/music/roots/${rootId}/scan`, {
        method: 'POST'
      });
      if (!response.ok) {
        error = await readError(response, 'Scan failed.');
        return;
      }
      const summary = (await response.json()) as {
        added: number;
        updated: number;
        removed: number;
        truncated: boolean;
      };
      notice = `Scan complete: ${summary.added} added, ${summary.updated} updated, ${summary.removed} removed${
        summary.truncated
          ? ' (library is large; rescan to continue indexing)'
          : ''
      }.`;
      setTimeout(() => (notice = ''), 6000);
    } finally {
      scanning = false;
    }
  }

  async function rescanAll() {
    for (const root of roots) {
      // eslint-disable-next-line no-await-in-loop
      await scanRoot(root.id);
    }
    await loadAll();
  }

  async function runSearch() {
    if (!searchQuery.trim()) {
      searchResults = [];
      return;
    }
    const response = await api(
      `/api/v1/music/search?q=${encodeURIComponent(searchQuery)}`
    );
    if (response.ok) searchResults = (await response.json()) as TrackSummary[];
  }

  async function openFileFromFiles(path: string) {
    // A file opened directly from Files may not be part of any indexed
    // library root yet -- play it immediately via a synthetic
    // single-track entry rather than requiring the user to add/scan a
    // root first.
    const synthetic: TrackSummary = {
      id: `unindexed:${path}`,
      root_id: '',
      virtual_path: path,
      updated_at: 0
    };
    await playTrack(
      synthetic,
      [synthetic.id],
      0,
      new Map([[synthetic.id, synthetic]])
    );
  }

  function selectView(next: View) {
    view = next;
    if (next === 'playlists') selectedPlaylistId = null;
  }

  async function openPlaylist(id: string) {
    selectedPlaylistId = id;
    const response = await api(`/api/v1/music/playlists/${id}`);
    if (response.ok) {
      selectedPlaylistEntries = (await response.json()) as PlaylistEntry[];
      for (const entry of selectedPlaylistEntries)
        queueTracks.set(entry.track.id, entry.track);
    }
  }

  async function createPlaylistPrompt() {
    const name = window.prompt('Playlist name');
    if (!name) return;
    const response = await api('/api/v1/music/playlists', {
      method: 'POST',
      body: JSON.stringify({ name })
    });
    if (response.ok) await loadPlaylists();
  }

  async function deletePlaylist(id: string) {
    if (!window.confirm('Delete this playlist?')) return;
    await api(`/api/v1/music/playlists/${id}`, { method: 'DELETE' });
    if (selectedPlaylistId === id) selectedPlaylistId = null;
    await loadPlaylists();
  }

  async function addToPlaylistPrompt(track: TrackSummary) {
    if (playlists.length === 0) {
      error = 'Create a playlist first.';
      return;
    }
    const names = playlists.map((p, i) => `${i + 1}. ${p.name}`).join('\n');
    const choice = window.prompt(
      `Add "${trackDisplayTitle(track)}" to which playlist?\n${names}`
    );
    const index = choice ? Number.parseInt(choice, 10) - 1 : -1;
    const target = playlists[index];
    if (!target) return;
    await api(`/api/v1/music/playlists/${target.id}/entries`, {
      method: 'POST',
      body: JSON.stringify({ track_id: track.id })
    });
    notice = `Added to ${target.name}.`;
    setTimeout(() => (notice = ''), 3000);
  }

  async function removePlaylistEntry(entryId: string) {
    if (!selectedPlaylistId) return;
    await api(
      `/api/v1/music/playlists/${selectedPlaylistId}/entries/${entryId}`,
      {
        method: 'DELETE'
      }
    );
    await openPlaylist(selectedPlaylistId);
  }

  async function toggleFavorite(track: TrackSummary) {
    const isFavorite = favorites.some((f) => f.id === track.id);
    if (isFavorite) {
      await api(`/api/v1/music/favorites/${track.id}`, { method: 'DELETE' });
    } else {
      await api(`/api/v1/music/favorites/${track.id}`, { method: 'PUT' });
    }
    await loadFavorites();
  }

  function isFavorited(trackId: string): boolean {
    return favorites.some((f) => f.id === trackId);
  }

  async function queueAction(track: TrackSummary, mode: QueueInsertMode) {
    queueTracks.set(track.id, track);
    const result = insertIntoQueue(queue, track.id, mode, currentIndex);
    queue = result.queue;
    await persistQueue();
    if (result.playIndex !== null) {
      currentIndex = result.playIndex;
      await loadCurrentFromQueue();
    }
  }

  async function removeQueueIndex(index: number) {
    queue = removeFromQueue(queue, index);
    if (currentIndex !== null) {
      if (index < currentIndex) currentIndex -= 1;
      else if (index === currentIndex) currentIndex = null;
    }
    await persistQueue();
  }

  async function clearQueue() {
    queue = [];
    currentIndex = null;
    await persistQueue();
  }

  async function loadCurrentFromQueue() {
    if (currentIndex === null) return;
    const trackId = queue[currentIndex];
    const track = queueTracks.get(trackId);
    if (track) await playTrack(track, queue, currentIndex, queueTracks);
  }

  async function playTrack(
    track: TrackSummary,
    withQueue: string[],
    index: number,
    tracksById: Map<string, TrackSummary>
  ) {
    stopJobPolling();
    await cancelActiveJobIfAny();
    currentTrack = track;
    queue = withQueue;
    currentIndex = index;
    queueTracks = tracksById;
    plan = null;
    jobId = null;
    jobState = null;
    audioUrl = null;
    error = '';
    loadingTrack = true;
    currentTime = 0;
    duration = 0;
    recordedThisTrack = false;

    try {
      const response = await api('/api/v1/media/probe', {
        method: 'POST',
        body: JSON.stringify({ path: track.virtual_path })
      });
      if (!response.ok) {
        if (response.status === 503) {
          // FFmpeg disabled/unavailable: attempt direct playback anyway
          // (see VideoApp.svelte for the identical, deliberate policy).
          plan = 'direct';
          audioUrl = playbackUrl('direct', track.virtual_path, null);
          loadingTrack = false;
          return;
        }
        error = await readError(response, 'Could not analyze this track.');
        loadingTrack = false;
        return;
      }
      const body = (await response.json()) as { plan: StreamPlan };
      plan = body.plan;
      if (plan === 'unsupported') {
        error = 'This track has no browser-playable audio.';
        loadingTrack = false;
        return;
      }
      if (plan === 'direct') {
        audioUrl = playbackUrl('direct', track.virtual_path, null);
        loadingTrack = false;
        return;
      }
      const jobResponse = await api('/api/v1/media/jobs', {
        method: 'POST',
        body: JSON.stringify({
          path: track.virtual_path,
          operation: conversionOperationFor(plan)
        })
      });
      if (!jobResponse.ok) {
        error = await readError(jobResponse, 'Could not prepare this track.');
        loadingTrack = false;
        return;
      }
      const jobBody = (await jobResponse.json()) as {
        job_id: string;
        state: JobState;
      };
      jobId = jobBody.job_id;
      jobState = jobBody.state;
      pollJob();
    } catch {
      error = 'Could not reach the media service.';
      loadingTrack = false;
    }
  }

  function pollJob() {
    stopJobPolling();
    jobPollTimer = setInterval(() => void checkJob(), 750);
    void checkJob();
  }

  function stopJobPolling() {
    if (jobPollTimer) clearInterval(jobPollTimer);
    jobPollTimer = undefined;
  }

  async function checkJob() {
    if (!jobId) return;
    const response = await api(`/api/v1/media/jobs/${jobId}`);
    if (!response.ok) {
      stopJobPolling();
      loadingTrack = false;
      error = await readError(response, 'Conversion failed.');
      return;
    }
    const body = (await response.json()) as { state: JobState };
    jobState = body.state;
    if (!isTerminalJobState(body.state)) return;
    stopJobPolling();
    loadingTrack = false;
    if (body.state === 'completed' && currentTrack) {
      audioUrl = playbackUrl(plan ?? 'remux', currentTrack.virtual_path, jobId);
    } else {
      error = 'Could not prepare this track for playback.';
    }
  }

  async function cancelActiveJobIfAny() {
    stopJobPolling();
    if (!jobId || (jobState && isTerminalJobState(jobState))) return;
    try {
      await api(`/api/v1/media/jobs/${jobId}`, { method: 'DELETE' });
    } catch {
      // best-effort
    }
  }

  function togglePlay() {
    if (!audioEl) return;
    if (audioEl.paused) void audioEl.play();
    else audioEl.pause();
  }

  async function next() {
    if (currentIndex === null || currentIndex + 1 >= queue.length) return;
    currentIndex += 1;
    await loadCurrentFromQueue();
  }

  async function previous() {
    if (currentIndex === null || currentIndex <= 0) return;
    currentIndex -= 1;
    await loadCurrentFromQueue();
  }

  function seekTo(fraction: number) {
    if (!audioEl || !Number.isFinite(duration) || duration <= 0) return;
    audioEl.currentTime = Math.max(0, Math.min(duration, fraction * duration));
  }

  function onTimeUpdate() {
    if (!audioEl) return;
    currentTime = audioEl.currentTime;
    if (
      !recordedThisTrack &&
      currentTrack &&
      !currentTrack.id.startsWith('unindexed:')
    ) {
      if (
        hasPlayedEnoughToRecord(
          currentTime,
          duration || currentTrack.duration_seconds || null
        )
      ) {
        recordedThisTrack = true;
        void api('/api/v1/music/recent', {
          method: 'POST',
          body: JSON.stringify({ track_id: currentTrack.id })
        }).then(() => void loadRecent());
      }
    }
  }

  function onLoadedMetadata() {
    if (audioEl) duration = audioEl.duration;
  }

  function onPlay() {
    playing = true;
  }
  function onPause() {
    playing = false;
  }
  function onEnded() {
    void next();
  }
  function onAudioError() {
    if (!error) error = 'Playback failed for this format.';
  }

  function toggleMute() {
    muted = !muted;
    if (audioEl) audioEl.muted = muted;
  }

  loadAll();

  onDestroy(() => {
    void cancelActiveJobIfAny();
  });
</script>

<section class="music-app">
  <nav class="music-sidebar" aria-label="Music views">
    <button
      class:active={view === 'library'}
      onclick={() => selectView('library')}>Library</button
    >
    <button
      class:active={view === 'artists'}
      onclick={() => selectView('artists')}>Artists</button
    >
    <button
      class:active={view === 'albums'}
      onclick={() => selectView('albums')}>Albums</button
    >
    <button
      class:active={view === 'playlists'}
      onclick={() => selectView('playlists')}>Playlists</button
    >
    <button
      class:active={view === 'favorites'}
      onclick={() => selectView('favorites')}>Favorites</button
    >
    <button
      class:active={view === 'search'}
      onclick={() => selectView('search')}>Search</button
    >
    <button class:active={view === 'queue'} onclick={() => selectView('queue')}
      >Queue ({queue.length})</button
    >
    <hr />
    <button onclick={() => void addRootPrompt()}>Add folder…</button>
    <button
      disabled={scanning || roots.length === 0}
      onclick={() => void rescanAll()}
    >
      {scanning ? 'Scanning…' : 'Rescan library'}
    </button>
    {#if recent.length > 0}
      <h4>Recently played</h4>
      {#each recent.slice(0, 5) as track (track.id)}
        <button
          class="mini-track"
          onclick={() =>
            void playTrack(track, [track.id], 0, new Map([[track.id, track]]))}
        >
          {truncateForDisplay(trackDisplayTitle(track), 40)}
        </button>
      {/each}
    {/if}
  </nav>

  <div class="music-content">
    {#if error}<p class="music-error" role="alert">{error}</p>{/if}
    {#if notice}<p class="music-notice">{notice}</p>{/if}

    {#if view === 'library'}
      <h3>Library ({totalTracks} tracks)</h3>
      {#if roots.length === 0}
        <p>No music folders yet. Add one to get started.</p>
      {/if}
      <ul class="track-list">
        {#each tracks as track (track.id)}
          <li>
            <button
              class="track-row"
              onclick={() =>
                void playTrack(
                  track,
                  tracks.map((t) => t.id),
                  tracks.findIndex((t) => t.id === track.id),
                  new Map(tracks.map((t) => [t.id, t]))
                )}
            >
              <strong>{truncateForDisplay(trackDisplayTitle(track))}</strong>
              <small>{trackSubtitle(track)}</small>
              <small>{formatTime(track.duration_seconds ?? 0)}</small>
            </button>
            <button
              onclick={() => void queueAction(track, 'play_next')}
              aria-label="Play next">➕⏭</button
            >
            <button
              onclick={() => void queueAction(track, 'add_to_queue')}
              aria-label="Add to queue">➕</button
            >
            <button
              onclick={() => void toggleFavorite(track)}
              aria-label="Toggle favorite"
            >
              {isFavorited(track.id) ? '★' : '☆'}
            </button>
            <button
              onclick={() => void addToPlaylistPrompt(track)}
              aria-label="Add to playlist">＋list</button
            >
          </li>
        {/each}
      </ul>
    {:else if view === 'artists'}
      <h3>Artists</h3>
      <ul class="simple-list">
        {#each artists as artist}
          <li>{truncateForDisplay(artist)}</li>
        {/each}
      </ul>
    {:else if view === 'albums'}
      <h3>Albums</h3>
      <ul class="simple-list">
        {#each albums as album}
          <li>
            <strong>{truncateForDisplay(album.album)}</strong>
            <small
              >{album.artist ? truncateForDisplay(album.artist) : ''}{album.year
                ? ` · ${album.year}`
                : ''}</small
            >
          </li>
        {/each}
      </ul>
    {:else if view === 'playlists'}
      <h3>Playlists</h3>
      <button onclick={() => void createPlaylistPrompt()}>New playlist</button>
      <ul class="simple-list">
        {#each playlists as playlist (playlist.id)}
          <li>
            <button onclick={() => void openPlaylist(playlist.id)}
              >{truncateForDisplay(playlist.name)}</button
            >
            <button
              onclick={() => void deletePlaylist(playlist.id)}
              aria-label="Delete playlist">✕</button
            >
          </li>
        {/each}
      </ul>
      {#if selectedPlaylistId}
        <ul class="track-list">
          {#each selectedPlaylistEntries as entry (entry.entry_id)}
            <li>
              <button
                class="track-row"
                onclick={() =>
                  void playTrack(
                    entry.track,
                    selectedPlaylistEntries.map((e) => e.track.id),
                    selectedPlaylistEntries.findIndex(
                      (e) => e.entry_id === entry.entry_id
                    ),
                    new Map(
                      selectedPlaylistEntries.map((e) => [e.track.id, e.track])
                    )
                  )}
              >
                <strong
                  >{truncateForDisplay(trackDisplayTitle(entry.track))}</strong
                >
                <small>{trackSubtitle(entry.track)}</small>
              </button>
              <button
                onclick={() => void removePlaylistEntry(entry.entry_id)}
                aria-label="Remove from playlist">✕</button
              >
            </li>
          {/each}
        </ul>
      {/if}
    {:else if view === 'favorites'}
      <h3>Favorites</h3>
      <ul class="track-list">
        {#each favorites as track (track.id)}
          <li>
            <button
              class="track-row"
              onclick={() =>
                void playTrack(
                  track,
                  favorites.map((t) => t.id),
                  favorites.findIndex((t) => t.id === track.id),
                  new Map(favorites.map((t) => [t.id, t]))
                )}
            >
              <strong>{truncateForDisplay(trackDisplayTitle(track))}</strong>
              <small>{trackSubtitle(track)}</small>
            </button>
            <button
              onclick={() => void toggleFavorite(track)}
              aria-label="Unfavorite">★</button
            >
          </li>
        {/each}
      </ul>
    {:else if view === 'search'}
      <h3>Search</h3>
      <input
        type="search"
        placeholder="Title, artist, album, genre…"
        bind:value={searchQuery}
        oninput={() => void runSearch()}
        aria-label="Search music library"
      />
      <ul class="track-list">
        {#each searchResults as track (track.id)}
          <li>
            <button
              class="track-row"
              onclick={() =>
                void playTrack(
                  track,
                  searchResults.map((t) => t.id),
                  searchResults.findIndex((t) => t.id === track.id),
                  new Map(searchResults.map((t) => [t.id, t]))
                )}
            >
              <strong>{truncateForDisplay(trackDisplayTitle(track))}</strong>
              <small>{trackSubtitle(track)}</small>
            </button>
          </li>
        {/each}
      </ul>
    {:else if view === 'queue'}
      <h3>Queue</h3>
      <button disabled={queue.length === 0} onclick={() => void clearQueue()}
        >Clear queue</button
      >
      <ul class="track-list">
        {#each queue as trackId, index (index)}
          {@const track = queueTracks.get(trackId)}
          <li class:now-playing={index === currentIndex}>
            {#if track}
              <button
                class="track-row"
                onclick={() => {
                  currentIndex = index;
                  void loadCurrentFromQueue();
                }}
              >
                <strong>{truncateForDisplay(trackDisplayTitle(track))}</strong>
                <small>{trackSubtitle(track)}</small>
              </button>
            {:else}
              <span class="track-row">Unavailable track</span>
            {/if}
            <button
              onclick={() => void removeQueueIndex(index)}
              aria-label="Remove from queue">✕</button
            >
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</section>

{#if currentTrack}
  <div class="music-player" role="region" aria-label="Now playing">
    <div class="player-art">
      {#if artworkUrl}
        <img
          src={artworkUrl}
          alt=""
          onerror={(event) =>
            ((event.target as HTMLImageElement).style.display = 'none')}
        />
      {/if}
    </div>
    <div class="player-info">
      <strong title={currentTitle}>{currentTitle}</strong>
      <small>{currentSubtitle}</small>
    </div>
    <div class="player-controls">
      <button
        onclick={() => void previous()}
        disabled={currentIndex === null || currentIndex <= 0}
        aria-label="Previous">⏮</button
      >
      <button
        onclick={togglePlay}
        aria-label={playing ? 'Pause' : 'Play'}
        disabled={loadingTrack || !audioUrl}
      >
        {playing ? '⏸' : '▶'}
      </button>
      <button
        onclick={() => void next()}
        disabled={currentIndex === null || currentIndex + 1 >= queue.length}
        aria-label="Next">⏭</button
      >
      <span class="time"
        >{formatTime(currentTime)} / {formatTime(duration)}</span
      >
      <input
        class="timeline"
        type="range"
        min="0"
        max="1"
        step="0.001"
        value={duration > 0 ? currentTime / duration : 0}
        oninput={(event) =>
          seekTo(Number((event.target as HTMLInputElement).value))}
        aria-label="Seek"
      />
      <button onclick={toggleMute} aria-label={muted ? 'Unmute' : 'Mute'}
        >{muted ? '🔇' : '🔊'}</button
      >
      <input
        type="range"
        min="0"
        max="1"
        step="0.05"
        bind:value={volume}
        oninput={() => {
          if (audioEl) audioEl.volume = volume;
        }}
        aria-label="Volume"
      />
    </div>
    {#if loadingTrack}
      <span class="player-state"
        >{jobId ? `Preparing (${jobState ?? 'starting'})…` : 'Loading…'}</span
      >
    {/if}
    {#if audioUrl}
      <!-- svelte-ignore a11y-media-has-caption -->
      <audio
        bind:this={audioEl}
        src={audioUrl}
        bind:muted
        ontimeupdate={onTimeUpdate}
        onloadedmetadata={onLoadedMetadata}
        onplay={onPlay}
        onpause={onPause}
        onended={onEnded}
        onerror={onAudioError}
      ></audio>
    {/if}
  </div>
{/if}

<style>
  .music-app {
    display: flex;
    height: calc(100% - 64px);
    color: #eee;
    background: #0b0b0e;
  }
  .music-sidebar {
    width: 180px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px;
    border-right: 1px solid #222;
    overflow-y: auto;
  }
  .music-sidebar button {
    text-align: left;
    background: none;
    border: none;
    color: #ccc;
    padding: 6px 8px;
    border-radius: 4px;
    cursor: pointer;
  }
  .music-sidebar button.active {
    background: #223;
    color: #fff;
  }
  .mini-track {
    font-size: 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .music-content {
    flex: 1;
    overflow-y: auto;
    padding: 12px;
  }
  .music-error {
    color: #ff8a8a;
  }
  .music-notice {
    color: #8adfff;
  }
  .track-list,
  .simple-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .track-list li,
  .simple-list li {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 0;
    border-bottom: 1px solid #1c1c22;
  }
  .track-list li.now-playing {
    background: rgba(255, 255, 255, 0.05);
  }
  .track-row {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    text-align: left;
  }
  .music-player {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    height: 64px;
    box-sizing: border-box;
    background: #141418;
    border-top: 1px solid #222;
    color: #eee;
  }
  .player-art img {
    width: 44px;
    height: 44px;
    object-fit: cover;
    border-radius: 4px;
  }
  .player-info {
    display: flex;
    flex-direction: column;
    min-width: 120px;
    max-width: 200px;
    overflow: hidden;
  }
  .player-info strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .player-controls {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .timeline {
    flex: 1;
  }
  .player-state {
    font-size: 11px;
    color: #9ad;
  }
</style>
