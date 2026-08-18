import { describe, expect, it } from 'vitest';
import {
  formatTime,
  isTerminalJobState,
  playbackUrl,
  shouldSaveResume,
  trackLabel,
  truncateForDisplay,
  PLAYBACK_SPEEDS
} from './video';

describe('formatTime', () => {
  it('formats sub-hour durations as m:ss', () => {
    expect(formatTime(0)).toBe('0:00');
    expect(formatTime(65)).toBe('1:05');
    expect(formatTime(599)).toBe('9:59');
  });

  it('formats hour-plus durations as h:mm:ss', () => {
    expect(formatTime(3661)).toBe('1:01:01');
  });

  it('never throws on hostile input', () => {
    expect(formatTime(Number.NaN)).toBe('0:00');
    expect(formatTime(-5)).toBe('0:00');
    expect(formatTime(Number.POSITIVE_INFINITY)).toBe('0:00');
  });
});

describe('isTerminalJobState', () => {
  it('classifies terminal vs. in-progress states', () => {
    expect(isTerminalJobState('completed')).toBe(true);
    expect(isTerminalJobState('failed')).toBe(true);
    expect(isTerminalJobState('cancelled')).toBe(true);
    expect(isTerminalJobState('expired')).toBe(true);
    expect(isTerminalJobState('queued')).toBe(false);
    expect(isTerminalJobState('running')).toBe(false);
  });
});

describe('playbackUrl', () => {
  it('streams direct-compatible media without a job', () => {
    expect(playbackUrl('direct', '/movies/one.mp4', null)).toBe(
      '/api/v1/media/stream?path=%2Fmovies%2Fone.mp4'
    );
  });

  it('requires a completed job id for remux/transcode', () => {
    expect(playbackUrl('remux', '/movies/one.mkv', null)).toBeNull();
    expect(playbackUrl('remux', '/movies/one.mkv', 'job-1')).toBe(
      '/api/v1/media/jobs/job-1/output'
    );
    expect(playbackUrl('transcode', '/movies/one.mkv', 'job-2')).toBe(
      '/api/v1/media/jobs/job-2/output'
    );
  });

  it('never produces a URL for unsupported media', () => {
    expect(playbackUrl('unsupported', '/movies/one.avi', 'job-3')).toBeNull();
  });

  it('percent-encodes hostile path characters instead of injecting them', () => {
    const url = playbackUrl('direct', '/a/<script>&b=1', null);
    expect(url).not.toContain('<script>');
    expect(url).toContain(encodeURIComponent('/a/<script>&b=1'));
  });
});

describe('shouldSaveResume', () => {
  it('throttles to at most once per interval', () => {
    expect(shouldSaveResume(1000, 1000)).toBe(false);
    expect(shouldSaveResume(1000, 4999)).toBe(false);
    expect(shouldSaveResume(1000, 6000)).toBe(true);
  });
});

describe('trackLabel', () => {
  it('builds a readable label from available metadata', () => {
    expect(
      trackLabel({ index: 1, language: 'eng', codec_name: 'aac' }, 0)
    ).toBe('Track 1 – ENG – aac');
    expect(trackLabel({ index: 2 }, 1)).toBe('Track 2');
  });

  it('never renders raw HTML-looking metadata as markup (plain string output only)', () => {
    const label = trackLabel(
      { index: 1, language: '<img src=x onerror=alert(1)>' },
      0
    );
    expect(typeof label).toBe('string');
    expect(label).toContain('<IMG');
  });
});

describe('truncateForDisplay', () => {
  it('leaves short strings untouched', () => {
    expect(truncateForDisplay('movie.mp4')).toBe('movie.mp4');
  });

  it('truncates hostile long strings', () => {
    const long = 'a'.repeat(500);
    const truncated = truncateForDisplay(long, 50);
    expect(truncated.length).toBe(50);
    expect(truncated.endsWith('…')).toBe(true);
  });
});

describe('PLAYBACK_SPEEDS', () => {
  it('includes the standard rate set', () => {
    expect(PLAYBACK_SPEEDS).toContain(1);
    expect(PLAYBACK_SPEEDS).toContain(2);
    expect(PLAYBACK_SPEEDS[0]).toBeLessThan(1);
  });
});
