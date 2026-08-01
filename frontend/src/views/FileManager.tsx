import React, { useEffect, useState, useCallback, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { api, formatFileSize, formatDate, getFileExtension, type FileInfo } from '../lib/api'
import {
  FolderOpen, File, ChevronRight, Upload, Download,
  Trash2, FolderPlus, ArrowUp, Grid3x3, List, MoreVertical,
  RefreshCw, Home, X, Check, Search, ImageIcon,
  Film, Music, Archive, FileCode, FileText
} from 'lucide-react'

type ViewMode = 'list' | 'grid'

const FileManager: React.FC = () => {
  const [searchParams, setSearchParams] = useSearchParams()
  const initialPath = searchParams.get('path') || '~'
  const [currentPath, setCurrentPath] = useState(initialPath)
  const [files, setFiles] = useState<FileInfo[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [viewMode, setViewMode] = useState<ViewMode>('list')
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set())
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; file: FileInfo } | null>(null)
  const [uploadProgress, setUploadProgress] = useState<{ name: string; percent: number } | null>(null)
  const [newFolderName, setNewFolderName] = useState('')
  const [showNewFolder, setShowNewFolder] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [showSearch, setShowSearch] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)

  const loadFiles = useCallback(async (path: string) => {
    setLoading(true)
    setError(null)
    try {
      const response = await api.listDirectory(path)
      setFiles(response.entries)
      setCurrentPath(response.path)
      setSearchParams({ path: response.path })
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load directory')
    } finally {
      setLoading(false)
    }
  }, [setSearchParams])

  useEffect(() => {
    loadFiles(initialPath)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const navigateTo = (path: string) => {
    if (path === currentPath) return
    setSelectedFiles(new Set())
    loadFiles(path)
  }

  const goUp = () => {
    if (currentPath === '/' || currentPath === '~') return
    const parent = currentPath.split('/').slice(0, -1).join('/')
    if (parent === '') return
    navigateTo(parent)
  }

  const goHome = () => navigateTo('~')

  const handleUpload = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const inputFiles = e.target.files
    if (!inputFiles) return

    for (const file of Array.from(inputFiles)) {
      setUploadProgress({ name: file.name, percent: 0 })
      try {
        await api.upload(currentPath, file, (percent) => {
          setUploadProgress({ name: file.name, percent })
        })
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Upload failed')
      }
    }
    setUploadProgress(null)
    loadFiles(currentPath)
    e.target.value = ''
  }

  const handleDelete = async (path: string) => {
    if (!confirm(`Delete ${path}?`)) return
    try {
      await api.delete(path)
      loadFiles(currentPath)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Delete failed')
    }
  }

  const handleMkdir = async () => {
    if (!newFolderName.trim()) return
    const folderPath = `${currentPath}/${newFolderName.trim()}`
    try {
      await api.mkdir(folderPath)
      setShowNewFolder(false)
      setNewFolderName('')
      loadFiles(currentPath)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create folder')
    }
  }

  const toggleSelect = (path: string, e: React.MouseEvent) => {
    e.stopPropagation()
    const next = new Set(selectedFiles)
    if (next.has(path)) {
      next.delete(path)
    } else {
      next.add(path)
    }
    setSelectedFiles(next)
  }

  const getFileIcon = (file: FileInfo) => {
    const ext = getFileExtension(file.name)
    const iconClass = 'w-5 h-5'

    if (file.is_dir) return <FolderOpen className={`${iconClass} text-brand-400`} />

    const imageExts = ['jpg', 'jpeg', 'png', 'gif', 'svg', 'webp', 'ico']
    const videoExts = ['mp4', 'webm', 'mkv', 'avi', 'mov']
    const audioExts = ['mp3', 'wav', 'ogg', 'flac', 'aac']
    const archiveExts = ['zip', 'tar', 'gz', 'bz2', 'xz', '7z', 'rar']
    const codeExts = ['js', 'ts', 'tsx', 'jsx', 'go', 'py', 'rs', 'c', 'cpp', 'h', 'java', 'rb', 'php', 'sh', 'sql']
    const textExts = ['txt', 'md', 'log', 'conf', 'cfg', 'ini', 'yml', 'yaml', 'toml', 'json', 'xml', 'csv']

    if (imageExts.includes(ext)) return <ImageIcon className={`${iconClass} text-pink-400`} />
    if (videoExts.includes(ext)) return <Film className={`${iconClass} text-purple-400`} />
    if (audioExts.includes(ext)) return <Music className={`${iconClass} text-green-400`} />
    if (archiveExts.includes(ext)) return <Archive className={`${iconClass} text-amber-400`} />
    if (codeExts.includes(ext)) return <FileCode className={`${iconClass} text-cyan-400`} />
    if (textExts.includes(ext)) return <FileText className={`${iconClass} text-blue-400`} />
    return <File className={`${iconClass} text-surface-400`} />
  }

  const filteredFiles = searchQuery
    ? files.filter(f => f.name.toLowerCase().includes(searchQuery.toLowerCase()))
    : files

  const sortedFiles = [...filteredFiles].sort((a, b) => {
    if (a.is_dir && !b.is_dir) return -1
    if (!a.is_dir && b.is_dir) return 1
    return a.name.localeCompare(b.name)
  })

  const breadcrumbParts = currentPath.split('/').filter(Boolean)
  const isHomePath = currentPath.startsWith('~')
  const buildBreadcrumbPath = (index: number) => {
    const parts = breadcrumbParts.slice(0, index + 1)
    return isHomePath ? parts.join('/') : '/' + parts.join('/')
  }

  return (
    <div className="h-full flex flex-col p-4 overflow-auto">
      {/* Toolbar */}
      <div className="flex items-center justify-between mb-4 gap-2 flex-wrap">
        <div className="flex items-center gap-2">
          <button onClick={goHome} className="btn-secondary p-2" title="Home">
            <Home className="w-4 h-4" />
          </button>
          <button onClick={goUp} className="btn-secondary p-2" title="Go up">
            <ArrowUp className="w-4 h-4" />
          </button>
          <button onClick={() => loadFiles(currentPath)} className="btn-secondary p-2" title="Refresh">
            <RefreshCw className={`w-4 h-4 ${loading ? 'animate-spin' : ''}`} />
          </button>

          <div className="flex items-center gap-1 text-sm bg-surface-800 border border-surface-700 rounded-lg px-3 py-1.5">
            <button onClick={goHome} className="text-brand-400 hover:text-brand-300 font-medium">~</button>
            {breadcrumbParts
              .slice(isHomePath ? 1 : 0)
              .map((part, i) => {
                const actualIndex = isHomePath ? i + 1 : i
                return (
                  <React.Fragment key={actualIndex}>
                    <ChevronRight className="w-3 h-3 text-surface-500" />
                    <button
                      onClick={() => navigateTo(buildBreadcrumbPath(actualIndex))}
                      className={`hover:text-brand-400 ${
                        actualIndex === breadcrumbParts.length - 1 ? 'text-white font-medium' : 'text-surface-400'
                      }`}
                    >
                      {part}
                    </button>
                  </React.Fragment>
                )
              })}
          </div>
        </div>

        <div className="flex items-center gap-2">
          <button
            onClick={() => setShowSearch(!showSearch)}
            className={`btn-secondary p-2 ${showSearch ? 'bg-brand-600 text-white' : ''}`}
            title="Search"
          >
            <Search className="w-4 h-4" />
          </button>
          <button
            onClick={() => setViewMode(viewMode === 'list' ? 'grid' : 'list')}
            className="btn-secondary p-2"
            title="Toggle view"
          >
            {viewMode === 'list' ? <Grid3x3 className="w-4 h-4" /> : <List className="w-4 h-4" />}
          </button>
          <button onClick={() => setShowNewFolder(true)} className="btn-secondary p-2" title="New folder">
            <FolderPlus className="w-4 h-4" />
          </button>
          <button onClick={() => fileInputRef.current?.click()} className="btn-primary flex items-center gap-2">
            <Upload className="w-4 h-4" />
            Upload
          </button>
        </div>
      </div>

      {/* Search Bar */}
      {showSearch && (
        <div className="mb-3">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-surface-400" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="input-field pl-10"
              placeholder="Search files..."
              autoFocus
            />
            <button onClick={() => { setShowSearch(false); setSearchQuery('') }} className="absolute right-3 top-1/2 -translate-y-1/2">
              <X className="w-4 h-4 text-surface-400 hover:text-surface-200" />
            </button>
          </div>
        </div>
      )}

      {/* New Folder Input */}
      {showNewFolder && (
        <div className="mb-3 flex items-center gap-2">
          <input
            type="text"
            value={newFolderName}
            onChange={(e) => setNewFolderName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleMkdir()}
            className="input-field"
            placeholder="Folder name"
            autoFocus
          />
          <button onClick={handleMkdir} className="btn-primary p-2"><Check className="w-4 h-4" /></button>
          <button onClick={() => { setShowNewFolder(false); setNewFolderName('') }} className="btn-secondary p-2"><X className="w-4 h-4" /></button>
        </div>
      )}

      {/* Upload Progress */}
      {uploadProgress && (
        <div className="mb-3 bg-surface-800 border border-brand-500/30 rounded-lg p-3">
          <div className="flex items-center gap-3">
            <Upload className="w-4 h-4 text-brand-400 animate-pulse" />
            <span className="text-sm text-white flex-1">{uploadProgress.name}</span>
            <span className="text-sm text-brand-400">{uploadProgress.percent}%</span>
          </div>
          <div className="w-full bg-surface-700 rounded-full h-1.5 mt-2">
            <div className="bg-brand-500 h-1.5 rounded-full transition-all" style={{ width: `${uploadProgress.percent}%` }} />
          </div>
        </div>
      )}

      {/* Error */}
      {error && (
        <div className="mb-3 bg-red-500/10 border border-red-500/30 rounded-lg p-3 flex items-center justify-between">
          <p className="text-sm text-red-400">{error}</p>
          <button onClick={() => setError(null)}><X className="w-4 h-4 text-red-400" /></button>
        </div>
      )}

      <input ref={fileInputRef} type="file" multiple className="hidden" onChange={handleUpload} />

      {/* Content */}
      <div className="flex-1 overflow-auto">
        {loading && files.length === 0 ? (
          <div className="flex items-center justify-center h-64">
            <RefreshCw className="w-8 h-8 text-surface-500 animate-spin" />
          </div>
        ) : sortedFiles.length === 0 ? (
          <div className="flex items-center justify-center h-64 text-surface-500">
            <div className="text-center">
              <FolderOpen className="w-12 h-12 mx-auto mb-3 opacity-50" />
              <p>Empty directory</p>
            </div>
          </div>
        ) : viewMode === 'list' ? (
          <div className="border border-surface-700 rounded-xl overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-surface-800 border-b border-surface-700">
                  <th className="text-left p-3 text-surface-400 font-medium w-8"></th>
                  <th className="text-left p-3 text-surface-400 font-medium">Name</th>
                  <th className="text-left p-3 text-surface-400 font-medium hidden md:table-cell">Size</th>
                  <th className="text-left p-3 text-surface-400 font-medium hidden lg:table-cell">Modified</th>
                  <th className="text-right p-3 text-surface-400 font-medium w-10"></th>
                </tr>
              </thead>
              <tbody>
                {sortedFiles.map((file) => (
                  <tr
                    key={file.path}
                    className={`border-b border-surface-700/50 hover:bg-surface-800 cursor-pointer transition-colors ${
                      selectedFiles.has(file.path) ? 'bg-brand-600/10' : ''
                    }`}
                    onClick={() => file.is_dir ? navigateTo(file.path) : undefined}
                    onDoubleClick={() => { if (!file.is_dir) api.download(file.path) }}
                    onContextMenu={(e) => { e.preventDefault(); setContextMenu({ x: e.clientX, y: e.clientY, file }) }}
                  >
                    <td className="p-3">
                      <input
                        type="checkbox"
                        checked={selectedFiles.has(file.path)}
                        onChange={(e) => toggleSelect(file.path, e)}
                        className="rounded border-surface-600 bg-surface-800 text-brand-500 focus:ring-brand-500"
                      />
                    </td>
                    <td className="p-3">
                      <div className="flex items-center gap-2">
                        {getFileIcon(file)}
                        <span className="text-white truncate max-w-md">{file.name}</span>
                        {file.is_symlink && (
                          <span className="text-xs bg-surface-700 px-1.5 py-0.5 rounded text-surface-400">symlink</span>
                        )}
                      </div>
                    </td>
                    <td className="p-3 text-surface-400 hidden md:table-cell">
                      {file.is_dir ? '--' : formatFileSize(file.size)}
                    </td>
                    <td className="p-3 text-surface-400 hidden lg:table-cell">
                      {formatDate(file.mod_time)}
                    </td>
                    <td className="p-3 text-right">
                      <button onClick={(e) => e.stopPropagation()} className="p-1 hover:bg-surface-700 rounded">
                        <MoreVertical className="w-4 h-4 text-surface-400" />
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-3">
            {sortedFiles.map((file) => (
              <div
                key={file.path}
                className={`card p-3 cursor-pointer hover:border-brand-500/30 transition-all group ${
                  selectedFiles.has(file.path) ? 'border-brand-500/50 bg-brand-600/5' : ''
                }`}
                onClick={() => file.is_dir ? navigateTo(file.path) : undefined}
                onDoubleClick={() => { if (!file.is_dir) api.download(file.path) }}
                onContextMenu={(e) => { e.preventDefault(); setContextMenu({ x: e.clientX, y: e.clientY, file }) }}
              >
                <div className="flex items-center justify-center mb-2">
                  <div className="w-12 h-12 flex items-center justify-center">
                    {getFileIcon(file)}
                  </div>
                </div>
                <p className="text-sm text-white truncate text-center">{file.name}</p>
                {!file.is_dir && (
                  <p className="text-xs text-surface-500 text-center mt-1">{formatFileSize(file.size)}</p>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Context Menu */}
      {contextMenu && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setContextMenu(null)} />
          <div
            className="fixed z-50 bg-surface-800 border border-surface-700 rounded-lg shadow-xl py-1 min-w-[180px]"
            style={{ left: contextMenu.x, top: contextMenu.y }}
          >
            {contextMenu.file.is_dir ? (
              <button
                className="w-full text-left px-3 py-2 text-sm hover:bg-surface-700 flex items-center gap-2 text-white"
                onClick={() => { navigateTo(contextMenu.file.path); setContextMenu(null) }}
              >
                <FolderOpen className="w-4 h-4" /> Open
              </button>
            ) : (
              <>
                <button
                  className="w-full text-left px-3 py-2 text-sm hover:bg-surface-700 flex items-center gap-2 text-white"
                  onClick={() => { api.download(contextMenu.file.path); setContextMenu(null) }}
                >
                  <Download className="w-4 h-4" /> Download
                </button>
                <button
                  className="w-full text-left px-3 py-2 text-sm hover:bg-surface-700 flex items-center gap-2 text-white"
                  onClick={() => {
                    const newName = prompt('Rename to:', contextMenu.file.name)
                    if (newName) {
                      const dir = contextMenu.file.path.substring(0, contextMenu.file.path.lastIndexOf('/'))
                      api.rename(contextMenu.file.path, `${dir}/${newName}`).then(() => loadFiles(currentPath)).catch((err) => {
                        setError(err instanceof Error ? err.message : 'Rename failed')
                      })
                    }
                    setContextMenu(null)
                  }}
                >
                  <FileText className="w-4 h-4" /> Rename
                </button>
              </>
            )}
            <button
              className="w-full text-left px-3 py-2 text-sm hover:bg-red-500/20 flex items-center gap-2 text-red-400"
              onClick={() => { handleDelete(contextMenu.file.path); setContextMenu(null) }}
            >
              <Trash2 className="w-4 h-4" /> Delete
            </button>
          </div>
        </>
      )}
    </div>
  )
}

export default FileManager
