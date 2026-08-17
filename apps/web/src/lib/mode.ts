export const WORKSPACE_MODES = ['desktop', 'dashboard'] as const;

export type WorkspaceMode = (typeof WORKSPACE_MODES)[number];

export const DEFAULT_MODE: WorkspaceMode = 'desktop';

export function normalizeMode(value: string | null): WorkspaceMode {
  return value === 'dashboard' ? 'dashboard' : DEFAULT_MODE;
}
