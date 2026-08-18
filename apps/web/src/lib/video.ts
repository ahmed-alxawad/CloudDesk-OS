// Pure logic for the Video application, kept out of VideoApp.svelte so it
// can be unit-tested without a component-testing framework (this project
// has none, and Phase 4 explicitly says not to add a large dependency
// without reason).
//
// The DIRECT/REMUX/TRANSCODE plan and job-lifecycle helpers are generic
// media concepts shared with Music (Phase 5) -- they live in ./media and
// are re-exported here so existing imports of `./video` keep working.

export {
  formatTime,
  isTerminalJobState,
  playbackUrl,
  truncateForDisplay,
  type JobState,
  type StreamPlan
} from './media';

export const PLAYBACK_SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2] as const;
export type PlaybackSpeed = (typeof PLAYBACK_SPEEDS)[number];

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
