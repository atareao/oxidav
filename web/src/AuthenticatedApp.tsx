/**
 * Aplicación autenticada de OxiDav (F17).
 *
 * `App.tsx` la importa dinámicamente (`React.lazy`) para que `antd`, el router
 * y las vistas no formen parte del bundle inicial: solo se descarga cuando hay
 * una sesión válida.
 *
 * `main.tsx` ya monta el `BrowserRouter`, así que aquí solo viven el tema y las
 * rutas protegidas. Las rutas `/login` y `/callback` del modelo PKCE anterior
 * desaparecen: la autenticación la resuelve el backend con cookie de sesión.
 */

import { App as AntdApp, ConfigProvider, theme } from 'antd'
import type { ThemeConfig } from 'antd'
import esES from 'antd/locale/es_ES'
import { lazy } from 'react'
import { Navigate, Outlet, Route, Routes } from 'react-router-dom'
import { AppLayout } from './components/AppLayout'
import { ThemeProvider, useTheme } from './contexts/ThemeContext'

/**
 * Design tokens shared by both colour schemes.
 *
 * `colorPrimary` is deliberately absent: the plan keeps Ant Design's default
 * brand colour and only refines radii, typography, control density and spacing.
 */
const DESIGN_TOKENS: ThemeConfig['token'] = {
  borderRadius: 10,
  fontFamily:
    "system-ui, -apple-system, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif",
  fontSize: 14,
  controlHeight: 36,
  lineWidth: 1,
  wireframe: false,
}

/**
 * Per-component refinements layered on top of the seed tokens above.
 */
const COMPONENT_TOKENS: ThemeConfig['components'] = {
  Layout: {
    headerHeight: 56,
    headerPadding: '0 16px',
  },
  Menu: {
    itemBorderRadius: 8,
    itemHeight: 40,
  },
  Card: {
    bodyPadding: 20,
    headerHeight: 48,
  },
  Table: {
    cellPaddingBlock: 10,
  },
  Button: {
    contentFontSize: 14,
  },
}

// Pages are lazy-loaded so each route ships in its own chunk. They use named
// exports, hence the `.then(m => ({ default: m.X }))` mapping.
const Calendar = lazy(() => import('./pages/Calendar').then((m) => ({ default: m.Calendar })))
const Contacts = lazy(() => import('./pages/Contacts').then((m) => ({ default: m.Contacts })))
const Settings = lazy(() => import('./pages/Settings').then((m) => ({ default: m.Settings })))
const Sync = lazy(() => import('./pages/Sync').then((m) => ({ default: m.Sync })))

function ThemedApp() {
  const { mode } = useTheme()

  return (
    <ConfigProvider
      locale={esES}
      theme={{
        algorithm: mode === 'dark' ? theme.darkAlgorithm : theme.defaultAlgorithm,
        token: DESIGN_TOKENS,
        components: COMPONENT_TOKENS,
      }}
    >
      <AntdApp>
        <Routes>
          {/* The shared layout keeps the Sider/Header shell mounted while a
              protected page chunk suspends inside it. */}
          <Route
            element={
              <AppLayout>
                <Outlet />
              </AppLayout>
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

export default function AuthenticatedApp() {
  return (
    <ThemeProvider>
      <ThemedApp />
    </ThemeProvider>
  )
}
