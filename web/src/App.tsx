import { App as AntdApp, ConfigProvider, theme } from 'antd'
import esES from 'antd/locale/es_ES'
import { Navigate, Outlet, Route, Routes } from 'react-router-dom'
import { AppLayout } from './components/AppLayout'
import { RequireAuth } from './components/RequireAuth'
import { ThemeProvider, useTheme } from './contexts/ThemeContext'
import { Calendar } from './pages/Calendar'
import { Callback } from './pages/Callback'
import { Contacts } from './pages/Contacts'
import { Login } from './pages/Login'
import { Settings } from './pages/Settings'
import { Sync } from './pages/Sync'

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
          <Route path="/login" element={<Login />} />
          <Route path="/callback" element={<Callback />} />
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
