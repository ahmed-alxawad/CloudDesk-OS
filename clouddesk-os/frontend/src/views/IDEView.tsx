import React, { useEffect, useState } from 'react'
import { api } from '../lib/api'
import { Code2, Play, Square, Loader2, RefreshCw } from 'lucide-react'

const IDEView: React.FC = () => {
  const [status, setStatus] = useState<string>('unknown')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const checkStatus = async () => {
    try {
      const res = await api.ideStatus()
      setStatus(res.status)
    } catch {
      setStatus('error')
    }
  }

  useEffect(() => {
    checkStatus()
    const interval = setInterval(checkStatus, 10000)
    return () => clearInterval(interval)
  }, [])

  const handleStart = async () => {
    setLoading(true)
    setError(null)
    try {
      await api.ideStart()
      setStatus('running')
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to start IDE')
    } finally {
      setLoading(false)
    }
  }

  const handleStop = async () => {
    setLoading(true)
    try {
      await api.ideStop()
      setStatus('stopped')
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to stop IDE')
    } finally {
      setLoading(false)
    }
  }

  const statusColor: Record<string, string> = {
    running: 'text-green-400',
    stopped: 'text-surface-400',
    starting: 'text-amber-400',
    error: 'text-red-400',
    unknown: 'text-surface-500',
  }
  const statusBg: Record<string, string> = {
    running: 'bg-green-500',
    stopped: 'bg-surface-500',
    starting: 'bg-amber-500 animate-pulse',
    error: 'bg-red-500',
    unknown: 'bg-surface-600',
  }

  return (
    <div className="h-full flex flex-col p-4">
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 bg-brand-500/20 rounded-lg flex items-center justify-center">
            <Code2 className="w-5 h-5 text-brand-400" />
          </div>
          <div>
            <h1 className="text-xl font-bold text-white">VS Code Workspace</h1>
            <div className="flex items-center gap-2">
              <div className={`w-2 h-2 rounded-full ${statusBg[status] || 'bg-surface-600'}`} />
              <span className={`text-sm capitalize ${statusColor[status] || 'text-surface-400'}`}>{status}</span>
            </div>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <button onClick={checkStatus} className="btn-secondary p-2" title="Refresh status">
            <RefreshCw className="w-4 h-4" />
          </button>
          {status === 'running' ? (
            <button onClick={handleStop} disabled={loading} className="btn-danger flex items-center gap-2">
              <Square className="w-4 h-4" />
              Stop
            </button>
          ) : (
            <button onClick={handleStart} disabled={loading} className="btn-primary flex items-center gap-2">
              {loading ? <Loader2 className="w-4 h-4 animate-spin" /> : <Play className="w-4 h-4" />}
              Start
            </button>
          )}
        </div>
      </div>

      {error && (
        <div className="mb-3 bg-red-500/10 border border-red-500/30 rounded-lg p-3">
          <p className="text-sm text-red-400">{error}</p>
        </div>
      )}

      {status === 'running' ? (
        <div className="flex-1 border border-surface-700 rounded-xl overflow-hidden bg-surface-800">
          <iframe
            src="/api/v1/ide/proxy/"
            className="w-full h-full border-0"
            title="VS Code"
            allow="clipboard-read; clipboard-write"
          />
        </div>
      ) : (
        <div className="flex-1 flex items-center justify-center border border-surface-700 rounded-xl bg-surface-800/50">
          <div className="text-center space-y-4">
            <div className="w-20 h-20 bg-surface-700 rounded-2xl flex items-center justify-center mx-auto">
              <Code2 className="w-10 h-10 text-surface-400" />
            </div>
            <div>
              <h2 className="text-lg font-semibold text-white mb-1">
                {status === 'stopped' ? 'Start your workspace' : 'IDE is not available'}
              </h2>
              <p className="text-sm text-surface-400 max-w-md">
                Click "Start" to launch a persistent VS Code instance. Your workspace
                will continue running in the background even after you close this tab.
              </p>
            </div>
            {status === 'stopped' && (
              <button onClick={handleStart} disabled={loading} className="btn-primary flex items-center gap-2 mx-auto">
                {loading ? <Loader2 className="w-4 h-4 animate-spin" /> : <Play className="w-4 h-4" />}
                Launch VS Code
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  )
}

export default IDEView
