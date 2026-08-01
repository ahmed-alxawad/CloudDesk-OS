import React, { useEffect, useRef, useState, useCallback } from 'react'
import { useAuthStore } from '../store/authStore'
import { Terminal as XTerm } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import '@xterm/xterm/css/xterm.css'

const TerminalView: React.FC = () => {
  const { token } = useAuthStore()
  const termRef = useRef<HTMLDivElement>(null)
  const xtermRef = useRef<XTerm | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const [status, setStatus] = useState<'connecting' | 'connected' | 'disconnected' | 'error'>('connecting')
  const [reconnectKey, setReconnectKey] = useState(0)

  const connect = useCallback(() => {
    if (!token || !termRef.current) return

    // Clean up previous instance.
    if (xtermRef.current) {
      xtermRef.current.dispose()
      xtermRef.current = null
    }
    if (wsRef.current) {
      wsRef.current.close()
      wsRef.current = null
    }

    // Create xterm instance.
    const xterm = new XTerm({
      cursorBlink: true,
      cursorStyle: 'block',
      fontSize: 14,
      fontFamily: '"JetBrains Mono", "Fira Code", "Cascadia Code", Menlo, Monaco, "Courier New", monospace',
      theme: {
        background: '#0d1117',
        foreground: '#c9d1d9',
        cursor: '#58a6ff',
        cursorAccent: '#0d1117',
        selectionBackground: '#264f78',
        selectionForeground: '#ffffff',
        black: '#484f58',
        red: '#ff7b72',
        green: '#3fb950',
        yellow: '#d29922',
        blue: '#58a6ff',
        magenta: '#bc8cff',
        cyan: '#39c5cf',
        white: '#b1bac4',
        brightBlack: '#6e7681',
        brightRed: '#ffa198',
        brightGreen: '#56d364',
        brightYellow: '#e3b341',
        brightBlue: '#79c0ff',
        brightMagenta: '#d2a8ff',
        brightCyan: '#56d4dd',
        brightWhite: '#f0f6fc',
      },
      allowProposedApi: true,
      scrollback: 10000,
      convertEol: false,
    })

    const fitAddon = new FitAddon()
    xterm.loadAddon(fitAddon)
    xterm.loadAddon(new WebLinksAddon())

    xterm.open(termRef.current)
    xtermRef.current = xterm
    fitAddonRef.current = fitAddon

    // Fit after a small delay to ensure the container has rendered.
    setTimeout(() => {
      try {
        fitAddon.fit()
      } catch {
        // Ignore fit errors during initialization.
      }
    }, 50)

    // Connect WebSocket.
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const cols = xterm.cols
    const rows = xterm.rows
    const wsUrl = `${protocol}//${window.location.host}/api/v1/terminal/ws?cols=${cols}&rows=${rows}&token=${encodeURIComponent(token)}`

    setStatus('connecting')
    const ws = new WebSocket(wsUrl)
    wsRef.current = ws

    ws.onopen = () => {
      setStatus('connected')
    }

    ws.onmessage = (event) => {
      if (typeof event.data === 'string') {
        xterm.write(event.data)
      }
    }

    ws.onclose = (event) => {
      setStatus('disconnected')
      xterm.write('\r\n\033[90m[Connection closed]\033[0m\r\n')
    }

    ws.onerror = () => {
      setStatus('error')
      xterm.write('\r\n\033[31m[Connection error — check your authentication]\033[0m\r\n')
    }

    // Send user input to WebSocket.
    xterm.onData((data: string) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(data)
      }
    })

    // Handle terminal resize.
    xterm.onResize(({ cols, rows }) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'resize', cols, rows }))
      }
    })

    // Handle window resize.
    const handleResize = () => {
      try {
        fitAddon.fit()
      } catch {
        // Ignore.
      }
    }
    window.addEventListener('resize', handleResize)

    // Focus the terminal.
    xterm.focus()

    // Cleanup on unmount.
    return () => {
      window.removeEventListener('resize', handleResize)
      ws.close()
      xterm.dispose()
    }
  }, [token, reconnectKey])

  useEffect(() => {
    const cleanup = connect()
    return () => {
      if (cleanup) cleanup()
    }
  }, [connect, reconnectKey])

  const handleReconnect = () => {
    setReconnectKey((k) => k + 1)
  }

  const statusColor = {
    connecting: 'text-yellow-400',
    connected: 'text-green-400',
    disconnected: 'text-surface-400',
    error: 'text-red-400',
  }

  const statusLabel = {
    connecting: 'Connecting...',
    connected: 'Connected',
    disconnected: 'Disconnected',
    error: 'Connection Error',
  }

  return (
    <div className="flex flex-col h-full">
      {/* Terminal toolbar */}
      <div className="flex items-center justify-between px-4 py-2 bg-surface-800 border-b border-surface-700">
        <div className="flex items-center gap-3">
          <div className="flex gap-1.5">
            <div className="w-3 h-3 rounded-full bg-red-500/80" />
            <div className="w-3 h-3 rounded-full bg-yellow-500/80" />
            <div className="w-3 h-3 rounded-full bg-green-500/80" />
          </div>
          <span className="text-sm text-surface-300 font-mono">bash</span>
        </div>
        <div className="flex items-center gap-3">
          <span className={`text-xs ${statusColor[status]}`}>
            {statusLabel[status]}
          </span>
          {(status === 'disconnected' || status === 'error') && (
            <button
              onClick={handleReconnect}
              className="text-xs px-3 py-1 bg-brand-600 hover:bg-brand-500 text-white rounded transition-colors"
            >
              Reconnect
            </button>
          )}
        </div>
      </div>

      {/* Terminal container */}
      <div
        ref={termRef}
        className="flex-1 bg-[#0d1117] p-1"
        style={{ minHeight: '400px' }}
      />
    </div>
  )
}

export default TerminalView
