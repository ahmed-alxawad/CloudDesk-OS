import { useAuthStore } from '../store/authStore'

const API_BASE = '/api/v1'

async function request<T>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  const token = useAuthStore.getState().token

  const headers: Record<string, string> = {
    ...(options.headers as Record<string, string>),
  }

  if (token) {
    headers['Authorization'] = `Bearer ${token}`
  }

  const response = await fetch(`${API_BASE}${path}`, {
    ...options,
    headers,
    credentials: 'include',
  })

  let data: any
  try {
    data = await response.json()
  } catch {
    throw new Error(
      `Non-JSON response from ${path} (status ${response.status})`
    )
  }

  if (!response.ok) {
    const error = new Error(data.message || 'API request failed')
    ;(error as any).code = data.code
    ;(error as any).details = data.details
    throw error
  }

  return data as T
}

export interface FileInfo {
  name: string
  path: string
  size: number
  mode: number
  mod_time: string
  is_dir: boolean
  is_symlink: boolean
  mime_type: string
}

export interface ListResponse {
  path: string
  entries: FileInfo[]
}

export interface UploadResponse {
  path: string
  size: number
  name: string
}

export interface DiskUsage {
  total: number
  used: number
  free: number
}

export const api = {
  listDirectory: (path: string) =>
    request<ListResponse>(`/fs/list?path=${encodeURIComponent(path)}`),

  stat: (path: string) =>
    request<FileInfo>(`/fs/stat?path=${encodeURIComponent(path)}`),

  upload: (path: string, file: File, onProgress?: (percent: number) => void) => {
    const token = useAuthStore.getState().token
    return new Promise<UploadResponse>((resolve, reject) => {
      const formData = new FormData()
      formData.append('file', file)
      formData.append('path', path)

      const xhr = new XMLHttpRequest()
      xhr.open('POST', `${API_BASE}/fs/upload`)
      if (token) {
        xhr.setRequestHeader('Authorization', `Bearer ${token}`)
      }
      xhr.withCredentials = true

      if (onProgress) {
        xhr.upload.onprogress = (e) => {
          if (e.lengthComputable) {
            onProgress(Math.round((e.loaded / e.total) * 100))
          }
        }
      }

      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          resolve(JSON.parse(xhr.responseText))
        } else {
          const data = JSON.parse(xhr.responseText)
          reject(new Error(data.message || 'Upload failed'))
        }
      }

      xhr.onerror = () => reject(new Error('Upload failed'))
      xhr.send(formData)
    })
  },

  download: (path: string) => {
    const token = useAuthStore.getState().token
    return new Promise<void>((resolve, reject) => {
      const xhr = new XMLHttpRequest()
      xhr.open('GET', `${API_BASE}/fs/download?path=${encodeURIComponent(path)}`)
      if (token) {
        xhr.setRequestHeader('Authorization', `Bearer ${token}`)
      }
      xhr.responseType = 'blob'
      xhr.withCredentials = true

      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          const blob = xhr.response as Blob
          const url = window.URL.createObjectURL(blob)
          const a = document.createElement('a')
          a.href = url
          a.download = path.split('/').pop() || 'download'
          document.body.appendChild(a)
          a.click()
          document.body.removeChild(a)
          window.URL.revokeObjectURL(url)
          resolve()
        } else {
          reject(new Error('Download failed'))
        }
      }

      xhr.onerror = () => reject(new Error('Download failed'))
      xhr.send()
    })
  },

  delete: (path: string) =>
    request<{ message: string }>(`/fs/delete?path=${encodeURIComponent(path)}`, {
      method: 'DELETE',
    }),

  mkdir: (path: string) =>
    request<{ message: string }>('/fs/mkdir', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path }),
    }),

  rename: (oldPath: string, newPath: string) =>
    request<{ message: string }>('/fs/rename', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ old_path: oldPath, new_path: newPath }),
    }),

  diskUsage: (path: string) =>
    request<DiskUsage>(`/fs/disk-usage?path=${encodeURIComponent(path)}`),

  ideStatus: () =>
    request<{ status: string; username: string }>('/ide/status'),

  ideStart: () =>
    request<{ status: string; username: string }>('/ide/start', {
      method: 'POST',
    }),

  ideStop: () =>
    request<{ status: string }>('/ide/stop', {
      method: 'POST',
    }),
}

export function formatFileSize(bytes: number): string {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(2))} ${sizes[i]}`
}

export function formatDate(isoString: string): string {
  const date = new Date(isoString)
  return date.toLocaleDateString('en-US', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function getFileExtension(name: string): string {
  const parts = name.split('.')
  return parts.length > 1 ? parts[parts.length - 1].toLowerCase() : ''
}
