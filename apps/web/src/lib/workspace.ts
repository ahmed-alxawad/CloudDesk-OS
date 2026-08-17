export interface WindowLayout {
  x: number;
  y: number;
  width: number;
  height: number;
  minimized: boolean;
  maximized: boolean;
}

export interface WorkspacePreferences {
  ui_mode: 'desktop' | 'dashboard';
  layout: Record<string, WindowLayout>;
  favorites: string[];
  recent: string[];
}

export const DEFAULT_PREFERENCES: WorkspacePreferences = {
  ui_mode: 'desktop',
  layout: {},
  favorites: ['files', 'terminal', 'servers', 'settings'],
  recent: []
};

export function defaultWindow(index: number): WindowLayout {
  const offset = (index % 7) * 28;
  return {
    x: 76 + offset,
    y: 72 + offset,
    width: 760,
    height: 500,
    minimized: false,
    maximized: false
  };
}

export function clampWindow(
  layout: WindowLayout,
  width: number,
  height: number
): WindowLayout {
  const windowWidth = Math.min(
    Math.max(layout.width, 360),
    Math.max(360, width - 24)
  );
  const windowHeight = Math.min(
    Math.max(layout.height, 260),
    Math.max(260, height - 88)
  );
  return {
    ...layout,
    width: windowWidth,
    height: windowHeight,
    x: Math.min(Math.max(layout.x, 0), Math.max(0, width - windowWidth)),
    y: Math.min(
      Math.max(layout.y, 48),
      Math.max(48, height - windowHeight - 48)
    )
  };
}
