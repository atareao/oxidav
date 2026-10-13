import { App as AntdApp, ConfigProvider, theme } from 'antd'
import esES from 'antd/locale/es_ES'
import { lazy, Suspense } from 'react'
import { Navigate, Outlet, Route, Routes } from 'react-router-dom'
import { AppLayout } from './components/AppLayout'
import { RequireAuth } from './components/RequireAuth'
import { RouteFallback } from './components/RouteFallback'
import { ThemeProvider, useTheme } from './contexts/ThemeContext'

// Pages are lazy-loaded so each route ships in its own chunk. They use named
// exports, hence the `.then(m => ({ default: m.X }))` mapping.
const Calendar = lazy(() => import('./pages/Calendar').then((m) => ({ default: m.Calendar })))
const Callback = lazy(() => import('./pages/Callback').then((m) => ({ default: m.Callback })))
const Contacts = lazy(() => import('./pages/Contacts').then((m) => ({ default: m.Contacts })))
const Login = lazy(() => import('./pages/Login').then((m) => ({ default: m.Login })))
const Settings = lazy(() => import('./pages/Settings').then((m) => ({ default: m.Settings })))
const Sync = lazy(() => import('./pages/Sync').then((m) => ({ default: m.Sync })))

function ThemedApp() {
  const { mode } = useTheme()

  return (
    <ConfigProvider
      locale={esES}
      theme={{
        algorithm: mode === 'dark' ? theme.darkAlgorithm : theme.defaultAlgorithm,
      }}
    >
      <AntdApp>
        <Routes>
          {/* Layout-less routes get their own boundary; layout routes suspend
              locally inside AppLayout so the shell never unmounts. */}
          <Route
            path="/login"
            element={
              <Suspense fallback={<RouteFallback />}>
                <Login />
              </Suspense>
            }
          />
          <Route
            path="/callback"
            element={
              <Suspense fallback={<RouteFallback />}>
                <Callback />
              </Suspense>
            }
          />
          <Route
            element={
              <RequireAuth>
                <AppLayout>
                  <Outlet />
                </AppLayout>
              </RequireAuth>
            }
          >
            <Route path="/" element={<Navigate to="/calendar" replace />} />
            <Route path="/calendar" element={<Calendar />} />
            <Route path="/contacts" element={<Contacts />} />
            <Route path="/sync" element={<Sync />} />
            <Route path="/settings" element={<Settings />} />
          </Route>
          <Route path="*" element={<Navigate to="/calendar" replace />} />
        </Routes>
      </AntdApp>
    </ConfigProvider>
  )
}

export default function App() {
  return (
    <ThemeProvider>
      <ThemedApp />
    </ThemeProvider>
  )
}
