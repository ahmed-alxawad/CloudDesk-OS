// Pure, unit-testable logic for the Office application (Phase 8 Task
// 6/7/8/28). The Svelte component stays a thin fetch/render shell --
// everything that decides *what* to show lives here so it can be tested
// without a component-rendering harness (this project has none;
// `runtime.ts`/`video.ts`/`music.ts` follow the same split).

/** Every document format CloudDesk Office can open (Task 6). Kept as a
 * flat set of lowercase extensions -- the backend independently decides
 * whether it can actually serve a given file, so this list only governs
 * which files the UI offers to open, never authorization. */
export const OFFICE_EXTENSIONS = [
  'doc',
  'docx',
  'xls',
  'xlsx',
  'ppt',
  'pptx',
  'odt',
  'ods',
  'odp'
] as const;

export type OfficeExtension = (typeof OFFICE_EXTENSIONS)[number];

const OFFICE_EXTENSION_SET: ReadonlySet<string> = new Set(OFFICE_EXTENSIONS);

/** Extensions Office should be the *default* handler for, as opposed to
 * merely an available "Open with" target. All supported formats qualify:
 * nothing else in CloudDesk edits them. */
export function isOfficeDocument(nameOrPath: string): boolean {
  return OFFICE_EXTENSION_SET.has(officeExtensionOf(nameOrPath) ?? '');
}

/** The lowercase extension of a filename or path, or null when it has
 * none. Splits on the basename so a dot in a parent directory name is
 * never mistaken for the file's extension. */
export function officeExtensionOf(nameOrPath: string): string | null {
  const base = nameOrPath.split('/').pop() ?? '';
  const dot = base.lastIndexOf('.');
  if (dot <= 0 || dot === base.length - 1) return null;
  return base.slice(dot + 1).toLowerCase();
}

/** The states the Office window can be in (Task 7). These are exhaustive
 * and mutually exclusive -- the component renders exactly one. */
export type OfficeState =
  | 'UNAVAILABLE'
  | 'DISABLED'
  | 'STARTING'
  | 'OPENING'
  | 'EDITING'
  | 'READ_ONLY'
  | 'FAILED'
  | 'PERMISSION_DENIED';

export interface OfficeRuntimeStatus {
  available: boolean;
  enabled: boolean;
}

export interface OfficeSession {
  /** Opaque CloudDesk file identity -- never a host filesystem path. */
  file_id: string;
  /** The Collabora editor URL, already routed through CloudDesk's own
   * authenticated proxy by the server. */
  editor_url: string;
  instance_id: string;
  read_write: boolean;
}

/** Decides which state to render, given what is currently known.
 *
 * Ordering matters and encodes the product rules: runtime availability
 * and the administrator's enable/disable decision outrank everything
 * (a disabled runtime never shows "starting"), an explicit permission
 * denial outranks a generic failure (so the user is told the real
 * reason), and read-only is derived from the session the *server*
 * issued -- never from anything the client decided for itself. */
export function officeState(input: {
  runtime: OfficeRuntimeStatus | null;
  session: OfficeSession | null;
  starting: boolean;
  error: OfficeError | null;
}): OfficeState {
  const { runtime, session, starting, error } = input;
  if (error?.kind === 'permission') return 'PERMISSION_DENIED';
  if (runtime && !runtime.available) return 'UNAVAILABLE';
  if (runtime && !runtime.enabled) return 'DISABLED';
  if (error) return 'FAILED';
  if (session) return session.read_write ? 'EDITING' : 'READ_ONLY';
  if (starting) return 'STARTING';
  return 'OPENING';
}

export interface OfficeError {
  kind: 'permission' | 'unavailable' | 'failed';
  message: string;
}

const MAX_MESSAGE_LENGTH = 200;
const CONTROL_CODE_MAX = 0x1f;
const DEL_CODE = 0x7f;

/** Truncates and strips control characters from any user-facing message
 * before it reaches the DOM -- defense in depth even though the backend
 * returns typed, safe strings. Callers must render with Svelte's default
 * text interpolation, never `{@html}`. */
export function sanitizeMessage(message: string): string {
  let stripped = '';
  for (const char of message) {
    const code = char.codePointAt(0) ?? 0;
    const isControl = code <= CONTROL_CODE_MAX || code === DEL_CODE;
    if (!isControl) stripped += char;
  }
  return stripped.length > MAX_MESSAGE_LENGTH
    ? `${stripped.slice(0, MAX_MESSAGE_LENGTH)}...`
    : stripped;
}

/** Classifies a failed session-open into the state machine's error
 * kinds. A 403 is a real authorization decision and must be surfaced as
 * such rather than as a generic failure, so the user is not told to
 * "retry" something that will never succeed. */
export function classifyOfficeError(
  status: number,
  message: string
): OfficeError {
  const safe = sanitizeMessage(message) || 'Office request failed';
  if (status === 401 || status === 403) {
    return { kind: 'permission', message: safe };
  }
  if (status === 404 || status === 503) {
    return { kind: 'unavailable', message: safe };
  }
  return { kind: 'failed', message: safe };
}

/** Whether the UI should offer a retry control. Retrying a permission
 * denial is pointless and misleading; retrying a transient runtime
 * failure is not. */
export function canRetry(state: OfficeState): boolean {
  return state === 'FAILED';
}

/** The request body for opening a document. The client sends only the
 * VFS path it already had from Files; the server resolves it to an
 * opaque WOPI identity and mints the scoped token. The client never
 * constructs a file id or token itself (Task 6). */
export function openSessionRequest(path: string): { path: string } {
  return { path };
}

/** A short human label for the current state, used for both the visible
 * status text and the window's accessible status announcement. */
export function describeOfficeState(state: OfficeState): string {
  switch (state) {
    case 'UNAVAILABLE':
      return 'The Office runtime is not available on this server.';
    case 'DISABLED':
      return 'The Office runtime is disabled by an administrator.';
    case 'STARTING':
      return 'Starting the Office runtime…';
    case 'OPENING':
      return 'Opening document…';
    case 'EDITING':
      return 'Editing';
    case 'READ_ONLY':
      return 'Read-only — you do not have write access to this document.';
    case 'PERMISSION_DENIED':
      return 'You do not have permission to open this document.';
    case 'FAILED':
      return 'The document could not be opened.';
  }
}

/** Whether the editor frame should be mounted at all. Mounting it in any
 * other state would either show a stale document or point the frame at
 * nothing. */
export function showsEditor(state: OfficeState): boolean {
  return state === 'EDITING' || state === 'READ_ONLY';
}

/** The iframe `sandbox` attribute for the Collabora editor (Task 7).
 *
 * Each token is present because the real editor genuinely needs it:
 * - `allow-scripts`: the editor is a JavaScript application.
 * - `allow-same-origin`: it is served through CloudDesk's own proxy on
 *   this origin and must reach its own WebSocket and static assets.
 * - `allow-forms`: Collabora's dialogs are real forms.
 * - `allow-popups` + `allow-popups-to-escape-sandbox`: hyperlinks inside
 *   a document open in a new tab, which must not inherit this sandbox.
 * - `allow-downloads`: "Download as…" is a real editor feature.
 *
 * Deliberately absent: `allow-top-navigation` (a document must never be
 * able to navigate the CloudDesk shell away) and
 * `allow-modals`/`allow-pointer-lock`, which the editor does not need. */
export const OFFICE_IFRAME_SANDBOX = [
  'allow-scripts',
  'allow-same-origin',
  'allow-forms',
  'allow-popups',
  'allow-popups-to-escape-sandbox',
  'allow-downloads'
].join(' ');

/** The iframe `allow` (permissions policy) attribute. Collabora needs
 * clipboard access for cut/copy/paste inside the editor; it is granted
 * only to this origin's own frame. Camera, microphone, geolocation, and
 * payment are explicitly never granted. */
export const OFFICE_IFRAME_ALLOW = 'clipboard-read; clipboard-write';

/** Guards the editor URL the server returned before it is used as an
 * iframe `src`. The server always returns a CloudDesk-relative proxy
 * path, so anything absolute or scheme-bearing is rejected rather than
 * framed -- a defense-in-depth check against a compromised or confused
 * server response ever pointing the editor frame off-origin. */
export function safeEditorUrl(url: string): string | null {
  if (!url.startsWith('/')) return null;
  // `//host` is protocol-relative and would leave this origin.
  if (url.startsWith('//')) return null;
  if (!url.startsWith('/api/v1/runtime-instances/office/')) return null;
  return url;
}
