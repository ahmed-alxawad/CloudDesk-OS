export type RuntimeDependency = 'browser' | 'code' | 'office' | null;

export interface AppDefinition {
  id: string;
  name: string;
  icon: string;
  route: string;
  description: string;
  requiredPermissions: string[];
  runtime: RuntimeDependency;
  accent: string;
}

export const applications: AppDefinition[] = [
  {
    id: 'files',
    name: 'Files',
    icon: 'folder',
    route: '/files',
    description: 'Local and remote storage',
    requiredPermissions: ['files.local.read'],
    runtime: null,
    accent: '#47b5ff'
  },
  {
    id: 'gallery',
    name: 'Gallery',
    icon: 'image',
    route: '/gallery',
    description: 'Photos, video, and previews',
    requiredPermissions: ['files.local.read'],
    runtime: null,
    accent: '#b984ff'
  },
  {
    id: 'video',
    name: 'Video',
    icon: 'video',
    route: '/video',
    description: 'Playback for local and remote video',
    requiredPermissions: ['files.local.read'],
    runtime: null,
    accent: '#ff5f9e'
  },
  {
    id: 'terminal',
    name: 'Terminal',
    icon: 'terminal',
    route: '/terminal',
    description: 'Your Linux shell',
    requiredPermissions: ['terminal.local.open'],
    runtime: null,
    accent: '#47d7a4'
  },
  {
    id: 'servers',
    name: 'Servers',
    icon: 'server',
    route: '/servers',
    description: 'Remote hosts and sessions',
    requiredPermissions: ['remote.servers.read'],
    runtime: null,
    accent: '#ffb454'
  },
  {
    id: 'transfers',
    name: 'Transfers',
    icon: 'transfer',
    route: '/transfers',
    description: 'Persistent background jobs',
    requiredPermissions: ['transfers.create'],
    runtime: null,
    accent: '#57d8e5'
  },
  {
    id: 'settings',
    name: 'Settings',
    icon: 'settings',
    route: '/settings',
    description: 'Workspace and system policy',
    requiredPermissions: [],
    runtime: null,
    accent: '#9aa9bf'
  },
  {
    id: 'browser',
    name: 'Browser',
    icon: 'globe',
    route: '/browser',
    description: 'Isolated Brave runtime',
    requiredPermissions: ['apps.browser.use'],
    runtime: 'browser',
    accent: '#ff6d5f'
  },
  {
    id: 'code',
    name: 'Code',
    icon: 'code',
    route: '/code',
    description: 'VS Code-compatible workspace',
    requiredPermissions: ['apps.code.use'],
    runtime: 'code',
    accent: '#38a9ff'
  },
  {
    id: 'office',
    name: 'Office',
    icon: 'document',
    route: '/office',
    description: 'Documents and spreadsheets',
    requiredPermissions: ['apps.office.use'],
    runtime: 'office',
    accent: '#ff8066'
  }
];

export function applicationById(id: string): AppDefinition | undefined {
  return applications.find((application) => application.id === id);
}

interface RawManifest {
  id: string;
  name: string;
  icon: string;
  route: string;
  required_permissions: string[];
  runtime_dependency: 'browser' | 'code' | 'office' | 'media' | null;
  enabled: boolean;
}

export async function loadApplicationManifests(): Promise<AppDefinition[]> {
  try {
    const names = (await (
      await fetch('/manifests/index.json')
    ).json()) as string[];
    const manifests = await Promise.all(
      names.map(
        async (name) =>
          (await (await fetch(`/manifests/${name}`)).json()) as RawManifest
      )
    );
    return manifests
      .filter((manifest) => manifest.enabled)
      .map((manifest) => {
        const fallback = applications.find(
          (application) => application.id === manifest.id
        );
        if (!fallback)
          throw new Error(
            `Unknown built-in application manifest: ${manifest.id}`
          );
        return {
          ...fallback,
          name: manifest.name,
          icon: manifest.icon,
          route: manifest.route,
          requiredPermissions: manifest.required_permissions,
          runtime:
            manifest.runtime_dependency === 'media'
              ? null
              : manifest.runtime_dependency
        };
      });
  } catch {
    return applications;
  }
}
