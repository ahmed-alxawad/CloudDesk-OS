<script lang="ts">
  import { onDestroy } from 'svelte';
  import {
    formatTime,
    isTerminalJobState,
    playbackUrl,
    shouldSaveResume,
    trackLabel,
    truncateForDisplay,
    PLAYBACK_SPEEDS,
    type JobState,
    type StreamPlan,
    type TrackInfo
  } from './video';

  // Set by the desktop shell when opened from Files ("Open With" /
  // double-click). Reactive to prop changes, not just mount, so
  // double-clicking a second video while this window is already open
  // retargets the same player instead of being ignored (App.svelte keeps
  // exactly one window per application id).
  export let initialPath: string | null = null;

  interface ProbeResponse {
    probe: {
      format_name: string;
      duration_seconds: number | null;
      streams: (TrackInfo & {
        codec_type: string;
        width?: number | null;
        height?: number | null;
      })[];
    };
    plan: StreamPlan;
  }

  let path: string | null = null;
  let loadedPath: string | null = null;
  let loading = false;
  let error = '';
  let plan: StreamPlan | null = null;
  let audioTracks: TrackInfo[] = [];
  let subtitleTracks: TrackInfo[] = [];
  let jobId: string | null = null;
  let jobState: JobState | null = null;
  let jobPollTimer: ReturnType<typeof setInterval> | undefined;
  let videoUrl: string | null = null;
  let selectedAudioOrdinal: number | null = null;
  let subtitleUrl: string | null = null;
  let subtitleLoading = false;
  let subtitleError = '';

  let videoEl: HTMLVideoElement | undefined;
  let playing = false;
  let muted = false;
  let volume = 1;
  let speed = 1;
  let currentTime = 0;
  let duration = 0;
  let fullscreen = false;

  let resumeOffer: number | null = null;
  let lastResumeSaveMs = 0;

  $: filename = path ? (path.split('/').pop() ?? path) : '';
  $: displayName = truncateForDisplay(filename || 'Video');
  $: if (initialPath && initialPath !== loadedPath) {
    void openPath(initialPath);
  }

  async function api(input: string, init?: RequestInit): Promise<Response> {
    const response = await fetch(input, {
      ...init,
      headers: init?.body
        ? { 'Content-Type': 'application/json', ...init.headers }
        : init?.headers
    });
    return response;
  }

  async function readError(
    response: Response,
    fallback: string
  ): Promise<string> {
    const body = (await response.json().catch(() => ({}))) as {
      error?: string;
    };
    if (response.status === 503)
      return 'Media conversion (FFmpeg) is currently disabled.';
    if (response.status === 404) return 'This file could not be found.';
    if (response.status === 403 || response.status === 401)
      return 'You are not authorized to play this file.';
    return body.error ?? fallback;
  }

  async function openPath(nextPath: string) {
    stopJobPolling();
    await cancelActiveJobIfAny();
    path = nextPath;
    loadedPath = nextPath;
    loading = true;
    error = '';
    plan = null;
    audioTracks = [];
    subtitleTracks = [];
    jobId = null;
    jobState = null;
    videoUrl = null;
    subtitleUrl = null;
    subtitleError = '';
    selectedAudioOrdinal = null;
    resumeOffer = null;
    playing = false;
    currentTime = 0;
    duration = 0;

    void loadResume(nextPath);

    try {
      const response = await api('/api/v1/media/probe', {
        method: 'POST',
        body: JSON.stringify({ path: nextPath })
      });
      if (!response.ok) {
        if (response.status === 503) {
          // FFmpeg is disabled/unavailable: we can't classify the file
          // at all, but a browser-compatible file may still play via
          // direct byte-range streaming -- attempt it and let the
          // <video> element's own error handling report failure if the
          // format genuinely needs conversion. This is the only path
          // that satisfies "DIRECT-compatible videos must still play
          // when FFmpeg is disabled."
          plan = 'direct';
          videoUrl = playbackUrl('direct', nextPath, null);
          loading = false;
          return;
        }
        error = await readError(response, 'Could not analyze this file.');
        loading = false;
        return;
      }
      const data = (await response.json()) as ProbeResponse;
      plan = data.plan;
      audioTracks = data.probe.streams.filter((s) => s.codec_type === 'audio');
      subtitleTracks = data.probe.streams.filter(
        (s) => s.codec_type === 'subtitle'
      );

      if (plan === 'unsupported') {
        error = 'This file has no browser-playable video or audio stream.';
        loading = false;
        return;
      }
      if (plan === 'direct') {
        videoUrl = playbackUrl('direct', nextPath, null);
        loading = false;
        return;
      }
      await startConversionJob('remux');
    } catch {
      error = 'Could not reach the media service.';
      loading = false;
    }
  }

  async function startConversionJob(
    operation: 'remux' | 'transcode',
    audioTrackOrdinal?: number
  ) {
    if (!path) return;
    loading = true;
    error = '';
    videoUrl = null;
    try {
      const response = await api('/api/v1/media/jobs', {
        method: 'POST',
        body: JSON.stringify({
          path,
          operation,
          ...(audioTrackOrdinal !== undefined
            ? { audio_track_ordinal: audioTrackOrdinal }
            : {})
        })
      });
      if (!response.ok) {
        error = await readError(
          response,
          'Could not prepare this file for playback.'
        );
        loading = false;
        return;
      }
      const body = (await response.json()) as {
        job_id: string;
        state: JobState;
      };
      jobId = body.job_id;
      jobState = body.state;
      pollJob();
    } catch {
      error = 'Could not reach the media service.';
      loading = false;
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
    try {
      const response = await api(`/api/v1/media/jobs/${jobId}`);
      if (!response.ok) {
        stopJobPolling();
        error = await readError(response, 'The conversion job failed.');
        loading = false;
        return;
      }
      const body = (await response.json()) as {
        state: JobState;
        error_class?: string | null;
      };
      jobState = body.state;
      if (!isTerminalJobState(body.state)) return;
      stopJobPolling();
      loading = false;
      if (body.state === 'completed') {
        videoUrl = playbackUrl(plan ?? 'remux', path ?? '', jobId);
      } else if (body.state === 'cancelled') {
        error = 'Playback preparation was cancelled.';
      } else {
        error = body.error_class
          ? `Could not prepare this file for playback (${body.error_class}).`
          : 'Could not prepare this file for playback.';
      }
    } catch {
      stopJobPolling();
      loading = false;
      error = 'Lost contact with the media service while preparing this file.';
    }
  }

  async function cancelActiveJobIfAny() {
    stopJobPolling();
    if (!jobId || (jobState && isTerminalJobState(jobState))) return;
    try {
      await api(`/api/v1/media/jobs/${jobId}`, { method: 'DELETE' });
    } catch {
      // best-effort: the janitor/startup reconciliation will reclaim an
      // orphaned job's temp workspace even if this request is lost.
    }
  }

  async function selectAudioTrack(ordinal: number) {
    selectedAudioOrdinal = ordinal;
    await cancelActiveJobIfAny();
    // Direct playback of a specific embedded audio track isn't reliably
    // controllable across browsers with a plain <video> element -- route
    // every explicit track switch through a remux with that track
    // selected, same as an incompatible-container REMUX would use.
    await startConversionJob('remux', ordinal);
  }

  async function toggleSubtitle(track: TrackInfo | null) {
    subtitleError = '';
    if (!track || !path) {
      subtitleUrl = null;
      return;
    }
    subtitleLoading = true;
    try {
      const response = await api('/api/v1/media/subtitles', {
        method: 'POST',
        body: JSON.stringify({ path, stream_index: track.index })
      });
      if (!response.ok) {
        subtitleError = await readError(
          response,
          'Could not load this subtitle track.'
        );
        subtitleUrl = null;
        return;
      }
      const blob = await response.blob();
      if (subtitleUrl) URL.revokeObjectURL(subtitleUrl);
      subtitleUrl = URL.createObjectURL(blob);
    } catch {
      subtitleError = 'Could not load this subtitle track.';
    } finally {
      subtitleLoading = false;
    }
  }

  async function loadResume(forPath: string) {
    try {
      const response = await api(
        `/api/v1/media/resume?path=${encodeURIComponent(forPath)}`
      );
      if (!response.ok) return;
      const body = (await response.json()) as {
        position_seconds: number;
      } | null;
      if (body && body.position_seconds > 5 && forPath === path) {
        resumeOffer = body.position_seconds;
      }
    } catch {
      // Resume is a convenience, not a correctness requirement -- fail
      // silently and just start from the beginning.
    }
  }

  function acceptResume() {
    if (resumeOffer !== null && videoEl) {
      videoEl.currentTime = resumeOffer;
    }
    resumeOffer = null;
  }

  function dismissResume() {
    resumeOffer = null;
  }

  async function saveResumePosition(force = false) {
    if (!path || !videoEl || !Number.isFinite(videoEl.currentTime)) return;
    const now = Date.now();
    if (!force && !shouldSaveResume(lastResumeSaveMs, now)) return;
    lastResumeSaveMs = now;
    try {
      await api('/api/v1/media/resume', {
        method: 'PUT',
        body: JSON.stringify({
          path,
          position_seconds: videoEl.currentTime,
          duration_seconds: Number.isFinite(videoEl.duration)
            ? videoEl.duration
            : null
        })
      });
    } catch {
      // Best-effort; losing one resume-position write is not worth
      // surfacing an error to the user.
    }
  }

  function onTimeUpdate() {
    if (!videoEl) return;
    currentTime = videoEl.currentTime;
    void saveResumePosition();
  }

  function onLoadedMetadata() {
    if (!videoEl) return;
    duration = videoEl.duration;
  }

  function onPlay() {
    playing = true;
  }
  function onPause() {
    playing = false;
    void saveResumePosition(true);
  }

  function togglePlay() {
    if (!videoEl) return;
    if (videoEl.paused) void videoEl.play();
    else videoEl.pause();
  }

  function seekTo(fraction: number) {
    if (!videoEl || !Number.isFinite(duration) || duration <= 0) return;
    videoEl.currentTime = Math.max(0, Math.min(duration, fraction * duration));
  }

  function setSpeed(next: number) {
    speed = next;
    if (videoEl) videoEl.playbackRate = next;
  }

  function toggleMute() {
    muted = !muted;
    if (videoEl) videoEl.muted = muted;
  }

  function setVolume(next: number) {
    volume = next;
    if (videoEl) videoEl.volume = next;
  }

  function toggleFullscreen() {
    if (!videoEl) return;
    if (document.fullscreenElement) {
      void document.exitFullscreen();
    } else {
      void videoEl.requestFullscreen();
    }
  }

  function onFullscreenChange() {
    fullscreen = document.fullscreenElement === videoEl;
  }

  function onVideoError() {
    if (!error) {
      error =
        'This browser could not play this file. It may require a format this deployment does not support.';
    }
  }

  onDestroy(() => {
    void saveResumePosition(true);
    void cancelActiveJobIfAny();
    if (subtitleUrl) URL.revokeObjectURL(subtitleUrl);
  });
</script>

<svelte:window onfullscreenchange={onFullscreenChange} />

<section class="video-app">
  {#if !path}
    <div class="video-empty">
      <p>Open a video from Files to start playback.</p>
    </div>
  {:else}
    <header class="video-header">
      <strong title={filename}>{displayName}</strong>
      {#if plan}<span class="plan-badge plan-{plan}">{plan}</span>{/if}
    </header>

    <div class="video-viewport">
      {#if error}
        <div class="video-state" role="alert">
          <p>{error}</p>
          <button onclick={() => path && void openPath(path)}>Retry</button>
        </div>
      {:else if loading}
        <div class="video-state" aria-busy="true">
          <p>
            {jobId
              ? `Preparing playback (${jobState ?? 'starting'})…`
              : 'Analyzing this file…'}
          </p>
          {#if jobId}
            <button onclick={() => void cancelActiveJobIfAny()}>Cancel</button>
          {/if}
        </div>
      {:else if videoUrl}
        <!-- svelte-ignore a11y-media-has-caption -->
        <video
          bind:this={videoEl}
          src={videoUrl}
          bind:muted
          bind:volume
          ontimeupdate={onTimeUpdate}
          onloadedmetadata={onLoadedMetadata}
          onplay={onPlay}
          onpause={onPause}
          onerror={onVideoError}
        >
          {#if subtitleUrl}
            <track
              kind="subtitles"
              src={subtitleUrl}
              default
              label="Subtitles"
            />
          {/if}
        </video>
        {#if resumeOffer !== null}
          <div class="resume-banner" role="dialog" aria-label="Resume playback">
            <span>Resume from {formatTime(resumeOffer)}?</span>
            <button onclick={acceptResume}>Resume</button>
            <button onclick={dismissResume}>Start over</button>
          </div>
        {/if}
      {/if}
    </div>

    {#if videoUrl && !error}
      <div class="video-controls">
        <button onclick={togglePlay} aria-label={playing ? 'Pause' : 'Play'}
          >{playing ? '⏸' : '▶'}</button
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
        <span class="time"
          >{formatTime(currentTime)} / {formatTime(duration)}</span
        >
        <button onclick={toggleMute} aria-label={muted ? 'Unmute' : 'Mute'}
          >{muted ? '🔇' : '🔊'}</button
        >
        <input
          type="range"
          min="0"
          max="1"
          step="0.05"
          value={volume}
          oninput={(event) =>
            setVolume(Number((event.target as HTMLInputElement).value))}
          aria-label="Volume"
        />
        <select
          aria-label="Playback speed"
          value={speed}
          onchange={(event) =>
            setSpeed(Number((event.target as HTMLSelectElement).value))}
        >
          {#each PLAYBACK_SPEEDS as rate}
            <option value={rate}>{rate}×</option>
          {/each}
        </select>
        {#if audioTracks.length > 1}
          <select
            aria-label="Audio track"
            value={selectedAudioOrdinal ?? 0}
            onchange={(event) =>
              void selectAudioTrack(
                Number((event.target as HTMLSelectElement).value)
              )}
          >
            {#each audioTracks as track, index}
              <option value={index}>{trackLabel(track, index)}</option>
            {/each}
          </select>
        {/if}
        {#if subtitleTracks.length > 0}
          <select
            aria-label="Subtitles"
            onchange={(event) => {
              const value = (event.target as HTMLSelectElement).value;
              const track = value === '' ? null : subtitleTracks[Number(value)];
              void toggleSubtitle(track ?? null);
            }}
          >
            <option value="">Subtitles off</option>
            {#each subtitleTracks as track, index}
              <option value={index}>{trackLabel(track, index)}</option>
            {/each}
          </select>
          {#if subtitleLoading}<span>Loading…</span>{/if}
          {#if subtitleError}<span class="video-error-inline"
              >{subtitleError}</span
            >{/if}
        {/if}
        <button onclick={toggleFullscreen} aria-label="Fullscreen"
          >{fullscreen ? '⤡' : '⤢'}</button
        >
      </div>
    {/if}
  {/if}
</section>

<style>
  .video-app {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #0b0b0e;
    color: #eee;
  }
  .video-empty {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: #999;
  }
  .video-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid #222;
  }
  .video-header strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .plan-badge {
    font-size: 10px;
    text-transform: uppercase;
    padding: 2px 6px;
    border-radius: 4px;
    background: #222;
    color: #9ad;
  }
  .video-viewport {
    position: relative;
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    min-height: 0;
  }
  video {
    max-width: 100%;
    max-height: 100%;
  }
  .video-state {
    text-align: center;
    color: #ccc;
    padding: 16px;
  }
  .resume-banner {
    position: absolute;
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    background: rgba(20, 20, 24, 0.92);
    border: 1px solid #444;
    border-radius: 6px;
    padding: 8px 12px;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .video-controls {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-top: 1px solid #222;
    flex-wrap: wrap;
  }
  .timeline {
    flex: 1;
    min-width: 120px;
  }
  .time {
    font-variant-numeric: tabular-nums;
    font-size: 12px;
    white-space: nowrap;
  }
  .video-error-inline {
    color: #ff8a8a;
    font-size: 12px;
  }
</style>
