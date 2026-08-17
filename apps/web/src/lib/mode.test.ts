import { describe, expect, it } from 'vitest';
import { DEFAULT_MODE, normalizeMode } from './mode';

describe('workspace mode', () => {
  it('defaults to Desktop mode', () => {
    expect(DEFAULT_MODE).toBe('desktop');
    expect(normalizeMode(null)).toBe('desktop');
    expect(normalizeMode('unknown')).toBe('desktop');
  });

  it('allows Dashboard mode explicitly', () => {
    expect(normalizeMode('dashboard')).toBe('dashboard');
  });
});
