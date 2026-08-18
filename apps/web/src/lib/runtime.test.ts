import { describe, expect, it } from 'vitest';
import {
  canManageRuntimes,
  describeRuntimeError,
  describeRuntimeStatus,
  isProductRuntimeKind,
  runtimeDisplayName,
  sanitizeDetail,
  visibleRuntimeCards,
  type RuntimeStatus
} from './runtime';

function status(overrides: Partial<RuntimeStatus> = {}): RuntimeStatus {
  return {
    kind: 'code',
    available: true,
    detail: 'ok',
    enabled: true,
    instance_count: 0,
    ...overrides
  };
}

describe('visibleRuntimeCards', () => {
  it('renders exactly the three product runtimes, in a fixed order', () => {
    const cards = visibleRuntimeCards([
      status({ kind: 'browser' }),
      status({ kind: 'code' }),
      status({ kind: 'office' })
    ]);
    expect(cards.map((c) => c.kind)).toEqual(['code', 'office', 'browser']);
  });

  it('never renders the disposable test fixture, no matter what the server sends', () => {
    const cards = visibleRuntimeCards([
      status({ kind: 'code' }),
      status({ kind: 'office' }),
      status({ kind: 'browser' }),
      status({ kind: 'test_fixture', available: true, enabled: true })
    ]);
    expect(cards.some((c) => c.kind === 'test_fixture')).toBe(false);
    expect(cards).toHaveLength(3);
  });

  it('renders an unavailable fallback card for a kind the server omitted entirely', () => {
    const cards = visibleRuntimeCards([status({ kind: 'code' })]);
    const office = cards.find((c) => c.kind === 'office');
    expect(office?.available).toBe(false);
    expect(office?.enabled).toBe(false);
  });

  it('drops any other unknown/hostile kind string from the server', () => {
    const cards = visibleRuntimeCards([
      status({ kind: '../../etc/passwd' }),
      status({ kind: '<script>alert(1)</script>' })
    ]);
    expect(cards).toHaveLength(3);
    expect(cards.every((c) => isProductRuntimeKind(c.kind))).toBe(true);
  });
});

describe('runtimeDisplayName', () => {
  it('names the three product kinds', () => {
    expect(runtimeDisplayName('code')).toBe('Code Runtime');
    expect(runtimeDisplayName('office')).toBe('Office Runtime');
    expect(runtimeDisplayName('browser')).toBe('Browser Runtime');
  });

  it('falls back safely for anything else instead of echoing raw input', () => {
    expect(runtimeDisplayName('test_fixture')).toBe('Runtime');
    expect(runtimeDisplayName('<img src=x onerror=alert(1)>')).toBe('Runtime');
  });
});

describe('canManageRuntimes', () => {
  it('is true only when runtime.admin is present', () => {
    expect(canManageRuntimes(['runtime.admin', 'apps.code.use'])).toBe(true);
    expect(canManageRuntimes(['apps.code.use'])).toBe(false);
    expect(canManageRuntimes([])).toBe(false);
  });
});

describe('describeRuntimeStatus', () => {
  it('reports Unavailable regardless of the enabled flag when not available', () => {
    expect(
      describeRuntimeStatus(status({ available: false, enabled: true }))
    ).toBe('Unavailable');
  });

  it('reports Enabled/Disabled only once available', () => {
    expect(
      describeRuntimeStatus(status({ available: true, enabled: true }))
    ).toBe('Enabled');
    expect(
      describeRuntimeStatus(status({ available: true, enabled: false }))
    ).toBe('Disabled');
  });
});

describe('sanitizeDetail', () => {
  it('strips control and ANSI escape characters', () => {
    expect(sanitizeDetail('hello[31mworld')).toBe('hello[31mworld');
  });

  it('truncates very long detail strings with a bounded output length', () => {
    const huge = 'x'.repeat(10_000);
    const result = sanitizeDetail(huge);
    expect(result.length).toBeLessThan(210);
    expect(result.endsWith('...')).toBe(true);
  });

  it('leaves ordinary safe text untouched', () => {
    expect(sanitizeDetail('docker (alpine:latest)')).toBe(
      'docker (alpine:latest)'
    );
  });
});

describe('describeRuntimeError', () => {
  it('surfaces a real Error message, sanitized', () => {
    expect(
      describeRuntimeError(new Error('runtime is currently disabled'))
    ).toBe('runtime is currently disabled');
  });

  it('never leaks a raw non-Error thrown value', () => {
    expect(describeRuntimeError({ stack: 'at internal.rs:42' })).toBe(
      'Runtime request failed'
    );
    expect(describeRuntimeError('some raw string')).toBe(
      'Runtime request failed'
    );
    expect(describeRuntimeError(undefined)).toBe('Runtime request failed');
  });

  it('sanitizes hostile error messages before display', () => {
    const hostile = new Error('<script>alert(1)</script>[31m');
    expect(describeRuntimeError(hostile)).toBe('<script>alert(1)</script>[31m');
  });
});
