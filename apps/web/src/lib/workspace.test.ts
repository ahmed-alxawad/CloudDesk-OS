import { describe, expect, it } from 'vitest';
import { clampWindow, defaultWindow } from './workspace';

describe('workspace layout', () => {
  it('cascades new windows', () => {
    expect(defaultWindow(1).x).toBeGreaterThan(defaultWindow(0).x);
  });

  it('keeps restored windows within the viewport', () => {
    const layout = clampWindow(
      {
        x: 2000,
        y: -40,
        width: 1400,
        height: 900,
        minimized: false,
        maximized: false
      },
      1024,
      768
    );
    expect(layout.x + layout.width).toBeLessThanOrEqual(1024);
    expect(layout.y).toBeGreaterThanOrEqual(48);
  });
});
