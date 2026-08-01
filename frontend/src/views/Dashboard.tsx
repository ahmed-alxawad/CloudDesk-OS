import React, { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { useAuthStore } from '../store/authStore'
import { api, formatFileSize, type DiskUsage } from '../lib/api'
import {
  FolderOpen, Code2, HardDrive, Shield, ArrowRight, Activity, Terminal
} from 'lucide-react'

const Dashboard: React.FC = () => {
  const { user } = useAuthStore()
  const [diskUsage, setDiskUsage] = useState<DiskUsage | null>(null)
  const [ideStatus, setIdeStatus] = useState<string>('unknown')
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    const fetchData = async () => {
      try {
        const [disk, ide] = await Promise.all([
          api.diskUsage('~/'),
          api.ideStatus(),
        ])
        setDiskUsage(disk)
        setIdeStatus(ide.status)
      } catch {
        // Ignore errors on dashboard
      } finally {
        setLoading(false)
      }
    }
    fetchData()
  }, [])

  const usagePercent = diskUsage && diskUsage.total > 0
    ? Math.round((diskUsage.used / diskUsage.total) * 100)
    : 0

  return (
    <div className="space-y-6 h-full overflow-auto p-4">
      <div>
        <h1 className="text-2xl font-bold text-white">
          Welcome back, {user?.username}
        </h1>
        <p className="text-surface-400 mt-1">
          Here is an overview of your workspace.
        </p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        <div className="card">
          <div className="flex items-center gap-3 mb-3">
            <div className="w-10 h-10 bg-brand-500/20 rounded-lg flex items-center justify-center">
              <HardDrive className="w-5 h-5 text-brand-400" />
            </div>
            <div>
              <p className="text-sm text-surface-400">Disk Usage</p>
              <p className="text-lg font-semibold text-white">
                {loading ? '...' : formatFileSize(diskUsage?.used ?? 0)}
              </p>
            </div>
          </div>
          <div className="w-full bg-surface-700 rounded-full h-2">
            <div
              className="bg-brand-500 h-2 rounded-full transition-all duration-500"
              style={{ width: `${usagePercent}%` }}
            />
          </div>
          <p className="text-xs text-surface-500 mt-1">
            {formatFileSize(diskUsage?.free ?? 0)} free of {formatFileSize(diskUsage?.total ?? 0)}
          </p>
        </div>

        <div className="card">
          <div className="flex items-center gap-3 mb-3">
            <div className={`w-10 h-10 rounded-lg flex items-center justify-center ${
              ideStatus === 'running' ? 'bg-green-500/20' : 'bg-surface-700'
            }`}>
              <Code2 className={`w-5 h-5 ${
                ideStatus === 'running' ? 'text-green-400' : 'text-surface-400'
              }`} />
            </div>
            <div>
              <p className="text-sm text-surface-400">VS Code</p>
              <p className="text-lg font-semibold text-white capitalize">{ideStatus}</p>
            </div>
          </div>
          <Link to="/ide" className="text-sm text-brand-400 hover:text-brand-300 flex items-center gap-1">
            Open IDE <ArrowRight className="w-3 h-3" />
          </Link>
        </div>

        <div className="card">
          <div className="flex items-center gap-3 mb-3">
            <div className="w-10 h-10 bg-purple-500/20 rounded-lg flex items-center justify-center">
              <Shield className="w-5 h-5 text-purple-400" />
            </div>
            <div>
              <p className="text-sm text-surface-400">User</p>
              <p className="text-lg font-semibold text-white">{user?.username}</p>
            </div>
          </div>
          <p className="text-xs text-surface-500">UID: {user?.uid} &middot; GID: {user?.gid}</p>
        </div>

        <div className="card">
          <div className="flex items-center gap-3 mb-3">
            <div className="w-10 h-10 bg-amber-500/20 rounded-lg flex items-center justify-center">
              <Activity className="w-5 h-5 text-amber-400" />
            </div>
            <div>
              <p className="text-sm text-surface-400">System</p>
              <p className="text-lg font-semibold text-white">Linux</p>
            </div>
          </div>
          <p className="text-xs text-surface-500">PAM authenticated</p>
        </div>
      </div>

      <div>
        <h2 className="text-lg font-semibold text-white mb-4">Quick Actions</h2>
        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
          <Link
            to="/files"
            className="card hover:border-brand-500/30 transition-all group"
          >
            <div className="flex items-center gap-3">
              <FolderOpen className="w-8 h-8 text-brand-400 group-hover:scale-110 transition-transform" />
              <div>
                <p className="font-medium text-white">File Manager</p>
                <p className="text-sm text-surface-400">Browse, upload, and manage files</p>
              </div>
              <ArrowRight className="w-4 h-4 text-surface-500 ml-auto group-hover:text-brand-400 transition-colors" />
            </div>
          </Link>

          <Link
            to="/ide"
            className="card hover:border-green-500/30 transition-all group"
          >
            <div className="flex items-center gap-3">
              <Code2 className="w-8 h-8 text-green-400 group-hover:scale-110 transition-transform" />
              <div>
                <p className="font-medium text-white">Code Editor</p>
                <p className="text-sm text-surface-400">Persistent VS Code in the browser</p>
              </div>
              <ArrowRight className="w-4 h-4 text-surface-500 ml-auto group-hover:text-green-400 transition-colors" />
            </div>
          </Link>

          <Link
            to="/terminal"
            className="card hover:border-emerald-500/30 transition-all group"
          >
            <div className="flex items-center gap-3">
              <Terminal className="w-8 h-8 text-emerald-400 group-hover:scale-110 transition-transform" />
              <div>
                <p className="font-medium text-white">Terminal</p>
                <p className="text-sm text-surface-400">Full bash shell in your browser</p>
              </div>
              <ArrowRight className="w-4 h-4 text-surface-500 ml-auto group-hover:text-emerald-400 transition-colors" />
            </div>
          </Link>
        </div>
      </div>
    </div>
  )
}

export default Dashboard
