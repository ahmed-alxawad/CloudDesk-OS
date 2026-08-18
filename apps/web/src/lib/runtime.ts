// Pure, unit-testable logic for the Settings app's Runtime section
// (Phase 6 Task 1-4/20). The Svelte component itself stays a thin
// fetch/render shell -- everything that decides *what* to show lives
// here so it can be tested without a component-rendering harness
// (this project has none; `video.ts`/`music.ts` follow the same split).

/** The only runtime kinds a real product Settings page may ever show.
 * `test_fixture` is deliberately not in this list -- even if a server
 * response somehow included it, `visibleRuntimeCards` below drops it. */
export const PRODUCT_RUNTIME_KINDS = ['code', 'office', 'browser'] as const;
export type ProductRuntimeKind = (typeof PRODUCT_RUNTIME_KINDS)[number];

export interface RuntimeStatus {
  kind: string;
  available: boolean;
  detail: string;
  enabled: boolean;
  instance_count: number;
}

const DISPLAY_NAMES: Record<ProductRuntimeKind, string> = {
  code: 'Code Runtime',
  office: 'Office Runtime',
  browser: 'Browser Runtime'
};

/** Human label for a runtime kind, defensively falling back for
 * anything outside the known product set rather than showing a raw
 * internal identifier. */
export function runtimeDisplayName(kind: string): string {
  return isProductRuntimeKind(kind) ? DISPLAY_NAMES[kind] : 'Runtime';
}

export function isProductRuntimeKind(kind: string): kind is ProductRuntimeKind {
  return (PRODUCT_RUNTIME_KINDS as readonly string[]).includes(kind);
}

/** Filters a raw `/api/v1/runtimes` response down to exactly the three
 * real product runtimes, in a fixed display order, regardless of what
 * the backend returned -- the disposable test fixture (or any future
 * unknown kind) must never render as a Settings card (Task 16). */
export function visibleRuntimeCards(
  runtimes: RuntimeStatus[]
): RuntimeStatus[] {
  const byKind = new Map(runtimes.map((entry) => [entry.kind, entry]));
  return PRODUCT_RUNTIME_KINDS.map(
    (kind) =>
      byKind.get(kind) ?? {
        kind,
        available: false,
        detail: 'not reported by the server',
        enabled: false,
        instance_count: 0
      }
  );
}

/** Whether the signed-in principal may see the global enable/disable
 * control at all -- the backend (`runtime.admin`) remains authoritative
 * regardless of this; hiding the control is UX, not the security
 * boundary (Task 2). */
export function canManageRuntimes(capabilities: string[]): boolean {
  return capabilities.includes('runtime.admin');
}

/** Safe, bounded status text for a runtime card. Never passes raw
 * backend/internal error text through un-truncated (Task 4: no raw
 * debug dumps in the UI) and never renders as HTML -- callers must use
 * Svelte's default text interpolation, never `{@html}`. */
export function describeRuntimeStatus(status: RuntimeStatus): string {
  if (!status.available) return 'Unavailable';
  return status.enabled ? 'Enabled' : 'Disabled';
}

const MAX_DETAIL_LENGTH = 200;
const CONTROL_CODE_MAX = 0x1f;
const DEL_CODE = 0x7f;

/** Truncates and strips control characters from any operator-facing
 * detail string before it reaches the DOM -- defense in depth even
 * though the backend already returns typed, safe strings. */
export function sanitizeDetail(detail: string): string {
  let stripped = '';
  for (const char of detail) {
    const code = char.codePointAt(0) ?? 0;
    const isControl = code <= CONTROL_CODE_MAX || code === DEL_CODE;
    if (!isControl) stripped += char;
  }
  return stripped.length > MAX_DETAIL_LENGTH
    ? `${stripped.slice(0, MAX_DETAIL_LENGTH)}...`
    : stripped;
}

/** Turns a thrown value from a failed runtime-management request into
 * a bounded, user-safe message -- never the raw exception/stack. */
export function describeRuntimeError(reason: unknown): string {
  if (reason instanceof Error && reason.message) {
    return sanitizeDetail(reason.message);
  }
  return 'Runtime request failed';
}
