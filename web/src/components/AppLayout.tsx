/**
 * Shell de la aplicación autenticada (F17).
 *
 * Sider de navegación + header con el conmutador de tema y el control de cierre
 * de sesión. El `Suspense` interno mantiene el shell montado mientras se
 * descarga el chunk de la página activa (`React.lazy`).
 *
 * El cierre de sesión se consume vía `useContext(AuthContext)` (no `useAuth()`)
 * para que el layout siga siendo renderizable sin `<AuthProvider>` —hay tests
 * de layout que no lo montan— y el botón solo aparezca cuando hay sesión.
 */

import { Button, Layout, Menu, Space, Typography } from 'antd'
import type { MenuProps } from 'antd'
import { Suspense, useContext } from 'react'
import type { ReactNode } from 'react'
import { Link, useLocation, useNavigate } from 'react-router-dom'
import { AuthContext } from '../contexts/AuthContext'
import { useTheme } from '../contexts/ThemeContext'
import { RouteFallback } from './RouteFallback'

const { Header, Sider, Content } = Layout

const NAV_ITEMS = [
  { key: '/calendar', label: 'Calendario' },
  { key: '/contacts', label: 'Contactos' },
  { key: '/sync', label: 'Sincronización' },
  { key: '/settings', label: 'Ajustes' },
] as const satisfies readonly { key: string; label: string }[]

const MENU_ITEMS: MenuProps['items'] = NAV_ITEMS.map((item) => ({
  key: item.key,
  label: <Link to={item.key}>{item.label}</Link>,
}))

export interface AppLayoutProps {
  children: ReactNode
}

export function AppLayout({ children }: AppLayoutProps) {
  const navigate = useNavigate()
  const location = useLocation()
  const { mode, toggle } = useTheme()
  // Contexto directo para tolerar un render sin `<AuthProvider>` (tests de
  // layout): si no hay sesión no se muestra el control de cierre.
  const auth = useContext(AuthContext)
  const logout = auth?.logout
  const authenticated = auth?.user != null
  const loggingOut = auth?.loggingOut ?? false

  const selectedKey =
    NAV_ITEMS.find(
      (item) => location.pathname === item.key || location.pathname.startsWith(`${item.key}/`),
    )?.key ?? '/calendar'

  return (
    <Layout style={{ minHeight: '100vh' }}>
      <Sider breakpoint="lg" collapsedWidth="0">
        <div
          style={{
            height: 48,
            margin: 16,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            color: '#fff',
            fontSize: 20,
            fontWeight: 600,
          }}
        >
          OxiDav
        </div>
        <Menu
          theme="dark"
          mode="inline"
          selectedKeys={[selectedKey]}
          items={MENU_ITEMS}
          onClick={({ key }) => navigate(key)}
        />
      </Sider>
      <Layout>
        <Header
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            paddingInline: 16,
          }}
        >
          <Typography.Title level={4} style={{ margin: 0, color: 'inherit' }}>
            OxiDav
          </Typography.Title>
          <Space>
            <Button
              onClick={toggle}
              aria-label="Cambiar tema"
              title={mode === 'dark' ? 'Activar tema claro' : 'Activar tema oscuro'}
            >
              {mode === 'dark' ? '☀️' : '🌙'}
            </Button>
            {authenticated && logout && (
              <Button
                onClick={() => {
                  void logout()
                }}
                // `loading` + `disabled` bloquean una segunda pulsación mientras
                // el `POST /auth/logout` está en curso y se navega al proveedor.
                loading={loggingOut}
                disabled={loggingOut}
                aria-label="Cerrar sesión"
                title="Cerrar sesión"
              >
                Cerrar sesión
              </Button>
            )}
          </Space>
        </Header>
        <Content style={{ margin: 16, padding: 24, background: 'transparent' }}>
          {/* Local boundary so a lazy page chunk keeps the Sider/Header shell
              on screen instead of unmounting the whole layout. */}
          <Suspense fallback={<RouteFallback />}>{children}</Suspense>
        </Content>
      </Layout>
    </Layout>
  )
}
