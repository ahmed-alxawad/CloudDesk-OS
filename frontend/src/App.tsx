import React from 'react'
import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom'
import { useAuthStore } from './store/authStore'
import Login from './views/Login'
import Dashboard from './views/Dashboard'
import FileManager from './views/FileManager'
import IDEView from './views/IDEView'
import TerminalView from './views/TerminalView'
import Sidebar from './components/Sidebar'
import Header from './components/Header'

const ProtectedRoute: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { isAuthenticated } = useAuthStore()
  if (!isAuthenticated) {
    return <Navigate to="/login" replace />
  }
  return <>{children}</>
}

const AppLayout: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div className="flex h-screen overflow-hidden">
    <Sidebar />
    <div className="flex flex-col flex-1 overflow-hidden">
      <Header />
      <main className="flex-1 overflow-hidden">
        {children}
      </main>
    </div>
  </div>
)

const App: React.FC = () => {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/login" element={<Login />} />
        <Route
          path="/"
          element={
            <ProtectedRoute>
              <AppLayout><Dashboard /></AppLayout>
            </ProtectedRoute>
          }
        />
        <Route
          path="/files"
          element={
            <ProtectedRoute>
              <AppLayout><FileManager /></AppLayout>
            </ProtectedRoute>
          }
        />
        <Route
          path="/files/*"
          element={
            <ProtectedRoute>
              <AppLayout><FileManager /></AppLayout>
            </ProtectedRoute>
          }
        />
        <Route
          path="/ide"
          element={
            <ProtectedRoute>
              <AppLayout><IDEView /></AppLayout>
            </ProtectedRoute>
          }
        />
        <Route
          path="/terminal"
          element={
            <ProtectedRoute>
              <AppLayout><TerminalView /></AppLayout>
            </ProtectedRoute>
          }
        />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  )
}

export default App
