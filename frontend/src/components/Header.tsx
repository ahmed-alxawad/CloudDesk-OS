import React from 'react'
import { useLocation } from 'react-router-dom'
import { Bell } from 'lucide-react'

const Header: React.FC = () => {
  const location = useLocation()

  const titles: Record<string, string> = {
    '/': 'Dashboard',
    '/files': 'File Manager',
    '/ide': 'Code Editor',
    '/terminal': 'Terminal',
  }

  const title = Object.entries(titles).find(([path]) =>
    location.pathname === path || (path !== '/' && location.pathname.startsWith(path))
  )?.[1] || 'CloudDesk OS'

  return (
    <header className="h-14 border-b border-surface-700 flex items-center justify-between px-4 bg-surface-800/50">
      <h2 className="text-lg font-semibold text-white">{title}</h2>
      <div className="flex items-center gap-2">
        <button className="btn-secondary p-2" title="Notifications">
          <Bell className="w-4 h-4 text-surface-400" />
        </button>
        <div className="text-xs text-surface-500 px-2 py-1 bg-surface-800 rounded">
          v0.1.1
        </div>
      </div>
    </header>
  )
}

export default Header
