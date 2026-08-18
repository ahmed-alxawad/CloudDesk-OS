// Pure logic for the Music application. Reuses ./media for the shared
// DIRECT/REMUX/TRANSCODE plan and job-lifecycle types rather than
// duplicating them -- Music consumes the same Phase 3 pipeline Video
// does, never a second compatibility engine.

export {
  formatTime,
  isTerminalJobState,
  playbackUrl,
  truncateForDisplay,
  type JobState,
  type StreamPlan
} from './media';

export interface TrackSummary {
  id: string;
  root_id: string;
  virtual_path: string;
  title?: string | null;
  artist?: string | null;
  album?: string | null;
  album_artist?: string | null;
  track_number?: number | null;
  disc_number?: number | null;
  duration_seconds?: number | null;
  codec?: string | null;
  bit_rate?: number | null;
  year?: string | null;
  genre?: string | null;
  updated_at: number;
}

/** Falls back to the filename when a track has no `title` tag -- never
 * blank, never "undefined". */
export function trackDisplayTitle(track: TrackSummary): string {
  if (track.title && track.title.trim()) return track.title;
  const name = track.virtual_path.split('/').pop();
  return name && name.trim() ? name : 'Unknown track';
}

export function trackSubtitle(track: TrackSummary): string {
  const artist = track.artist?.trim();
  const album = track.album?.trim();
  if (artist && album) return `${artist} — ${album}`;
  return artist || album || '';
}

/**
 * Reorders `ids` by moving the element at `from` to `to` (both indices
 * into the array), returning a new array -- used for playlist/queue
 * drag-reorder. Out-of-range indices are a no-op (returns the input
 * unchanged) rather than throwing or silently corrupting order.
 */
export function moveItem<T>(
  items: readonly T[],
  from: number,
  to: number
): T[] {
  if (
    from < 0 ||
    from >= items.length ||
    to < 0 ||
    to >= items.length ||
    from === to
  ) {
    return [...items];
  }
  const next = [...items];
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved);
  return next;
}

export type QueueInsertMode = 'play_now' | 'play_next' | 'add_to_queue';

/**
 * Applies a queue-insert action, returning the new queue and the index
 * that should become "now playing" (or `null` if playback shouldn't
 * change, e.g. `add_to_queue`).
 */
export function insertIntoQueue(
  queue: readonly string[],
  trackId: string,
  mode: QueueInsertMode,
  currentIndex: number | null
): { queue: string[]; playIndex: number | null } {
  if (mode === 'add_to_queue') {
    return { queue: [...queue, trackId], playIndex: null };
  }
  if (mode === 'play_now') {
    const insertAt = (currentIndex ?? -1) + 1;
    const next = [...queue];
    next.splice(insertAt, 0, trackId);
    return { queue: next, playIndex: insertAt };
  }
  // play_next
  const insertAt = (currentIndex ?? -1) + 1;
  const next = [...queue];
  next.splice(insertAt, 0, trackId);
  return { queue: next, playIndex: currentIndex };
}

export function removeFromQueue(
  queue: readonly string[],
  index: number
): string[] {
  if (index < 0 || index >= queue.length) return [...queue];
  const next = [...queue];
  next.splice(index, 1);
  return next;
}

/**
 * Decides whether enough of a track has actually played to count as
 * "recently played" -- avoids recording history (and write
 * amplification) for a track someone merely clicked and immediately
 * skipped. Matches common desktop-player convention: either a minimum
 * elapsed time, or most of a very short track.
 */
export function hasPlayedEnoughToRecord(
  elapsedSeconds: number,
  durationSeconds: number | null
): boolean {
  const MIN_ELAPSED_SECONDS = 15;
  if (elapsedSeconds >= MIN_ELAPSED_SECONDS) return true;
  if (durationSeconds && durationSeconds > 0) {
    return elapsedSeconds / durationSeconds >= 0.5;
  }
  return false;
}
