import React from 'react'
import { NavLink } from 'react-router-dom'
import { useAuthStore } from '../store/authStore'
import { LayoutDashboard, FolderOpen, Code2, Settings, LogOut, Shield, Terminal } from 'lucide-react'

const Sidebar: React.FC = () => {
  const { user, logout } = useAuthStore()

  const navItems = [
    { to: '/', icon: LayoutDashboard, label: 'Dashboard' },
    { to: '/files', icon: FolderOpen, label: 'Files' },
    { to: '/ide', icon: Code2, label: 'Code Editor' },
    { to: '/terminal', icon: Terminal, label: 'Terminal' },
  ]

  return (
    <aside className="w-60 bg-surface-800 border-r border-surface-700 flex flex-col">
      <div className="h-14 flex items-center gap-2 px-4 border-b border-surface-700">
        <Shield className="w-6 h-6 text-brand-400" />
        <span className="font-bold text-white">CloudDesk OS</span>
      </div>

      <nav className="flex-1 py-2">
        {navItems.map(({ to, icon: Icon, label }) => (
          <NavLink
            key={to}
            to={to}
            end={to === '/'}
            className={({ isActive }) =>
              `flex items-center gap-3 px-4 py-2.5 text-sm transition-colors ${
                isActive
                  ? 'bg-brand-600/20 text-brand-400 border-r-2 border-brand-400'
                  : 'text-surface-300 hover:bg-surface-700 hover:text-white'
              }`
            }
          >
            <Icon className="w-4 h-4" />
            {label}
          </NavLink>
        ))}

        <div className="mt-4 px-4">
          <p className="text-xs text-surface-500 uppercase tracking-wider mb-2">Coming Soon</p>
          <div className="space-y-1">
            <div className="flex items-center gap-3 px-2 py-1.5 text-sm text-surface-500 cursor-not-allowed">
              <Settings className="w-4 h-4" />
              Settings
            </div>
          </div>
        </div>
      </nav>

      <div className="border-t border-surface-700 p-3">
        <div className="flex items-center gap-3">
          <div className="w-8 h-8 bg-brand-600 rounded-full flex items-center justify-center text-white text-sm font-medium">
            {user?.username?.[0]?.toUpperCase() || '?'}
          </div>
          <div className="flex-1 min-w-0">
            <p className="text-sm font-medium text-white truncate">{user?.username}</p>
            <p className="text-xs text-surface-500 truncate">UID {user?.uid}</p>
          </div>
          <button
            onClick={logout}
            className="p-1.5 hover:bg-surface-700 rounded transition-colors"
            title="Sign out"
          >
            <LogOut className="w-4 h-4 text-surface-400 hover:text-red-400" />
          </button>
        </div>
      </div>
    </aside>
  )
}

export default Sidebar
