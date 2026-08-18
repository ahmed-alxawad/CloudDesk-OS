import { describe, expect, it } from 'vitest';
import {
  hasPlayedEnoughToRecord,
  insertIntoQueue,
  moveItem,
  removeFromQueue,
  trackDisplayTitle,
  trackSubtitle,
  type TrackSummary
} from './music';

function track(overrides: Partial<TrackSummary> = {}): TrackSummary {
  return {
    id: 't1',
    root_id: 'r1',
    virtual_path: '/music/song.mp3',
    updated_at: 0,
    ...overrides
  };
}

describe('trackDisplayTitle', () => {
  it('uses the title tag when present', () => {
    expect(trackDisplayTitle(track({ title: 'My Song' }))).toBe('My Song');
  });

  it('falls back to the filename when there is no title', () => {
    expect(trackDisplayTitle(track({ virtual_path: '/a/b/song.mp3' }))).toBe(
      'song.mp3'
    );
  });

  it('falls back to the filename for a blank/whitespace title', () => {
    expect(
      trackDisplayTitle(track({ title: '   ', virtual_path: '/x/y.flac' }))
    ).toBe('y.flac');
  });

  it('returns a hostile title verbatim as plain text (no interpretation)', () => {
    const hostile = '<img src=x onerror=alert(1)>';
    expect(trackDisplayTitle(track({ title: hostile }))).toBe(hostile);
  });
});

describe('trackSubtitle', () => {
  it('combines artist and album when both exist', () => {
    expect(trackSubtitle(track({ artist: 'A', album: 'B' }))).toBe('A — B');
  });

  it('falls back to whichever of artist/album exists', () => {
    expect(trackSubtitle(track({ artist: 'A' }))).toBe('A');
    expect(trackSubtitle(track({ album: 'B' }))).toBe('B');
  });

  it('is empty when neither exists', () => {
    expect(trackSubtitle(track())).toBe('');
  });
});

describe('moveItem', () => {
  it('reorders an array by moving one element', () => {
    expect(moveItem(['a', 'b', 'c'], 0, 2)).toEqual(['b', 'c', 'a']);
    expect(moveItem(['a', 'b', 'c'], 2, 0)).toEqual(['c', 'a', 'b']);
  });

  it('is a no-op for out-of-range indices', () => {
    expect(moveItem(['a', 'b'], -1, 1)).toEqual(['a', 'b']);
    expect(moveItem(['a', 'b'], 0, 5)).toEqual(['a', 'b']);
  });
});

describe('insertIntoQueue', () => {
  it('add_to_queue appends without changing playback', () => {
    const result = insertIntoQueue(['a', 'b'], 'c', 'add_to_queue', 0);
    expect(result.queue).toEqual(['a', 'b', 'c']);
    expect(result.playIndex).toBeNull();
  });

  it('play_now inserts right after the current track and switches to it', () => {
    const result = insertIntoQueue(['a', 'b'], 'c', 'play_now', 0);
    expect(result.queue).toEqual(['a', 'c', 'b']);
    expect(result.playIndex).toBe(1);
  });

  it('play_next inserts after current without switching playback', () => {
    const result = insertIntoQueue(['a', 'b'], 'c', 'play_next', 0);
    expect(result.queue).toEqual(['a', 'c', 'b']);
    expect(result.playIndex).toBe(0);
  });

  it('handles an empty/unset current index by inserting at the front', () => {
    const result = insertIntoQueue([], 'a', 'play_now', null);
    expect(result.queue).toEqual(['a']);
    expect(result.playIndex).toBe(0);
  });
});

describe('removeFromQueue', () => {
  it('removes the item at the given index', () => {
    expect(removeFromQueue(['a', 'b', 'c'], 1)).toEqual(['a', 'c']);
  });

  it('is a no-op for an out-of-range index', () => {
    expect(removeFromQueue(['a'], 5)).toEqual(['a']);
  });
});

describe('hasPlayedEnoughToRecord', () => {
  it('counts a play after the minimum elapsed threshold', () => {
    expect(hasPlayedEnoughToRecord(20, 300)).toBe(true);
    expect(hasPlayedEnoughToRecord(5, 300)).toBe(false);
  });

  it('counts a play on a short track once half has played', () => {
    expect(hasPlayedEnoughToRecord(6, 10)).toBe(true);
    expect(hasPlayedEnoughToRecord(2, 10)).toBe(false);
  });

  it('requires the elapsed threshold when duration is unknown', () => {
    expect(hasPlayedEnoughToRecord(3, null)).toBe(false);
    expect(hasPlayedEnoughToRecord(16, null)).toBe(true);
  });
});
