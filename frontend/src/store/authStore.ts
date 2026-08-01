import { create } from 'zustand'

interface User {
  id: number
  username: string
  uid: number
  gid: number
  role: string
  home_dir: string
  shell: string
}

interface AuthState {
  token: string | null
  user: User | null
  isAuthenticated: boolean
  isLoading: boolean
  error: string | null
  login: (username: string, password: string) => Promise<void>
  logout: () => void
  refresh: () => Promise<void>
  clearError: () => void
  setToken: (token: string, user: User) => void
}

function loadStoredUser(): User | null {
  try {
    return JSON.parse(localStorage.getItem('clouddesk_user') || 'null')
  } catch {
    localStorage.removeItem('clouddesk_user')
    return null
  }
}

export const useAuthStore = create<AuthState>((set, get) => ({
  token: localStorage.getItem('clouddesk_token'),
  user: loadStoredUser(),
  isAuthenticated: !!localStorage.getItem('clouddesk_token'),
  isLoading: false,
  error: null,

  login: async (username: string, password: string) => {
    set({ isLoading: true, error: null })
    try {
      const response = await fetch('/api/auth/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ username, password }),
        credentials: 'include',
      })

      const data = await response.json()

      if (!response.ok) {
        throw new Error(data.message || 'Authentication failed')
      }

      const user: User = data.user
      const token: string = data.token

      localStorage.setItem('clouddesk_token', token)
      localStorage.setItem('clouddesk_user', JSON.stringify(user))

      set({ token, user, isAuthenticated: true, isLoading: false })
    } catch (error) {
      set({
        isLoading: false,
        error: error instanceof Error ? error.message : 'Login failed',
      })
      throw error
    }
  },

  logout: () => {
    localStorage.removeItem('clouddesk_token')
    localStorage.removeItem('clouddesk_user')
    set({ token: null, user: null, isAuthenticated: false, error: null })
  },

  refresh: async () => {
    const { token } = get()
    if (!token) return

    try {
      const response = await fetch('/api/auth/refresh', {
        method: 'POST',
        headers: { Authorization: `Bearer ${token}` },
        credentials: 'include',
      })

      if (!response.ok) {
        get().logout()
        return
      }

      const data = await response.json()
      localStorage.setItem('clouddesk_token', data.token)
      set({ token: data.token })
    } catch {
      // Silently fail — the token may still be valid
    }
  },

  clearError: () => set({ error: null }),

  setToken: (token: string, user: User) => {
    localStorage.setItem('clouddesk_token', token)
    localStorage.setItem('clouddesk_user', JSON.stringify(user))
    set({ token, user, isAuthenticated: true })
  },
}))
