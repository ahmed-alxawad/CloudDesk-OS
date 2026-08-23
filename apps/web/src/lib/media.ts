// Shared pure logic between the Video and Music applications -- the
// DIRECT/REMUX/TRANSCODE plan and job-lifecycle types/helpers are generic
// media concepts (Phase 3's backend), not video-specific, so both apps
// import from here rather than Music reimplementing its own copy.

export type StreamPlan = 'direct' | 'remux' | 'transcode' | 'unsupported';

/** Backend media job states, as returned by GET /media/jobs/{id}. */
export type JobState =
  | 'queued'
  | 'probing'
  | 'running'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'expired';

export function isTerminalJobState(state: JobState): boolean {
  return (
    state === 'completed' ||
    state === 'failed' ||
    state === 'cancelled' ||
    state === 'expired'
  );
}

/**
 * Builds the URL a <video>/<audio> element should point at for a given
 * plan. DIRECT streams the original bytes; REMUX/TRANSCODE require a
 * completed job's output and only resolve once `jobId` is known.
 */
export function playbackUrl(
  plan: StreamPlan,
  path: string,
  jobId: string | null
): string | null {
  if (plan === 'direct') {
    return `/api/v1/media/stream?path=${encodeURIComponent(path)}`;
  }
  if ((plan === 'remux' || plan === 'transcode') && jobId) {
    return `/api/v1/media/jobs/${encodeURIComponent(jobId)}/output`;
  }
  return null;
}

/**
 * Which conversion job operation to request for a probed `plan` that
 * isn't `direct`/`unsupported`. The backend takes `operation` as the
 * caller's authoritative choice -- it does not re-derive it from the
 * probe -- so this must track `plan` exactly. Shared by Video and Music
 * (Phase 5 found the identical bug in `MusicApp.svelte` that Phase 4
 * found and fixed in `VideoApp.svelte`: the caller always requested
 * `'remux'`, so a `plan === 'transcode'` file -- a codec the browser
 * cannot decode at all -- was remuxed instead of re-encoded, copying
 * the still-undecodable codec into a new container and never actually
 * becoming playable).
 */
export function conversionOperationFor(
  plan: StreamPlan
): 'remux' | 'transcode' {
  return plan === 'transcode' ? 'transcode' : 'remux';
}

export function formatTime(totalSeconds: number): string {
  if (!Number.isFinite(totalSeconds) || totalSeconds < 0) return '0:00';
  const seconds = Math.floor(totalSeconds % 60);
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);
  const pad = (value: number) => value.toString().padStart(2, '0');
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${minutes}:${pad(seconds)}`;
}

/**
 * A defensive display-length cap for filenames/track metadata shown in
 * the UI. Svelte's text interpolation already prevents HTML/script
 * injection; this only prevents a hostile filename/metadata string from
 * visually breaking the window chrome.
 */
export function truncateForDisplay(value: string, maxLength = 200): string {
  if (value.length <= maxLength) return value;
  return `${value.slice(0, maxLength - 1)}…`;
}
