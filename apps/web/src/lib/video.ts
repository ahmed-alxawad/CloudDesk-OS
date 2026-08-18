// Pure logic for the Video application, kept out of VideoApp.svelte so it
// can be unit-tested without a component-testing framework (this project
// has none, and Phase 4 explicitly says not to add a large dependency
// without reason).

export const PLAYBACK_SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2] as const;
export type PlaybackSpeed = (typeof PLAYBACK_SPEEDS)[number];

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
 * Builds the URL the <video> element should point at for a given plan.
 * DIRECT streams the original bytes; REMUX/TRANSCODE require a completed
 * job's output and only resolve once `jobId` is known -- callers must not
 * call this for those plans before a job exists.
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
 * Throttles resume-position writes: at most once per `minIntervalMs`,
 * regardless of how often the caller's timeupdate handler fires (which
 * can be many times a second) -- satisfies "avoid excessive DB writes"
 * without the caller having to manage its own timer bookkeeping beyond a
 * single `lastSavedAtMs` value it already needs for other purposes.
 */
export function shouldSaveResume(
  lastSavedAtMs: number,
  nowMs: number,
  minIntervalMs = 5000
): boolean {
  return nowMs - lastSavedAtMs >= minIntervalMs;
}

export interface TrackInfo {
  index: number;
  codec_name?: string | null;
  language?: string | null;
}

/** A human label for a track selector option -- never renders raw,
 * unescaped metadata into HTML; Svelte's `{expression}` text
 * interpolation already escapes this, so no `{@html}` is used anywhere
 * for track/file metadata. */
export function trackLabel(track: TrackInfo, ordinal: number): string {
  const language = track.language?.trim();
  const codec = track.codec_name?.trim();
  const parts = [`Track ${ordinal + 1}`];
  if (language) parts.push(language.toUpperCase());
  if (codec) parts.push(codec);
  return parts.join(' – ');
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
