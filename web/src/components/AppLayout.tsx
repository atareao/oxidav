/**
 * Shell de la aplicación autenticada (F17).
 *
 * Un único `Header` con la marca, la navegación principal, el conmutador de
 * tema y el control de cierre de sesión. En pantallas pequeñas la navegación
 * se pliega en un `Drawer` que abre un botón hamburguesa, de modo que el
 * encabezado en línea solo se monta en escritorio.
 *
 * El `Suspense` interno mantiene el shell montado mientras se descarga el chunk
 * de la página activa (`React.lazy`).
 *
 * El cierre de sesión se consume vía `useContext(AuthContext)` (no `useAuth()`)
 * para que el layout siga siendo renderizable sin `<AuthProvider>` —hay tests
 * de layout que no lo montan— y el botón solo aparezca cuando hay sesión.
 */

import {
  CalendarOutlined,
  ContactsOutlined,
  LogoutOutlined,
  MenuOutlined,
  MoonOutlined,
  SettingOutlined,
  SunOutlined,
  SyncOutlined,
} from '@ant-design/icons'
import { Button, Drawer, Grid, Layout, Menu, Space } from 'antd'
import type { MenuProps } from 'antd'
import { Suspense, useContext, useState } from 'react'
import type { CSSProperties, ReactNode } from 'react'
import { Link, useLocation, useNavigate } from 'react-router-dom'
import { AuthContext } from '../contexts/AuthContext'
import { useTheme } from '../contexts/ThemeContext'
import { titleForPath, useDocumentTitle } from '../hooks/useDocumentTitle'
import { RouteFallback } from './RouteFallback'

const { Header, Content } = Layout

/** Accessible name shared by the desktop nav and the mobile drawer nav. */
const NAV_ARIA_LABEL = 'Navegación principal'

interface NavItem {
  key: string
  label: string
  icon: ReactNode
}

const NAV_ITEMS: readonly NavItem[] = [
  { key: '/calendar', label: 'Calendario', icon: <CalendarOutlined /> },
  { key: '/contacts', label: 'Contactos', icon: <ContactsOutlined /> },
  { key: '/sync', label: 'Sincronización', icon: <SyncOutlined /> },
  { key: '/settings', label: 'Ajustes', icon: <SettingOutlined /> },
]

const MENU_ITEMS: MenuProps['items'] = NAV_ITEMS.map((item) => ({
  key: item.key,
  icon: item.icon,
  label: <Link to={item.key}>{item.label}</Link>,
}))

const styles: Record<string, CSSProperties> = {
  header: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
  },
  brand: {
    fontSize: 20,
    fontWeight: 600,
    whiteSpace: 'nowrap',
    color: 'inherit',
  },
  nav: {
    flex: 1,
    minWidth: 0,
  },
  inlineMenu: {
    flex: 1,
    minWidth: 0,
    borderBottom: 'none',
  },
  actions: {
    marginInlineStart: 'auto',
  },
  content: {
    padding: 24,
  },
}

export interface AppLayoutProps {
  children: ReactNode
}

export function AppLayout({ children }: AppLayoutProps) {
  const navigate = useNavigate()
  const location = useLocation()
  const { mode, toggle } = useTheme()
  const screens = Grid.useBreakpoint()
  // Contexto directo para tolerar un render sin `<AuthProvider>` (tests de
  // layout): si no hay sesión no se muestra el control de cierre.
  const auth = useContext(AuthContext)
  const logout = auth?.logout
  const authenticated = auth?.user != null
  const loggingOut = auth?.loggingOut ?? false
  const [drawerOpen, setDrawerOpen] = useState(false)

  // Refleja la vista activa en la pestaña del navegador y restaura «OxiDav» al
  // desmontar el shell.
  useDocumentTitle(titleForPath(location.pathname))

  // Solo móvil cuando `xs` es *explícitamente* `true`. En un viewport de
  // escritorio todos los breakpoints son `false`/`undefined`, así que cualquier
  // comprobación basada en «falsy» clasificaría el escritorio como móvil.
  const isMobile = screens.xs === true

  const selectedKey =
    NAV_ITEMS.find(
      (item) => location.pathname === item.key || location.pathname.startsWith(`${item.key}/`),
    )?.key ?? '/calendar'

  const navMenu = (
    <Menu
      mode={isMobile ? 'inline' : 'horizontal'}
      selectedKeys={[selectedKey]}
      items={MENU_ITEMS}
      style={isMobile ? undefined : styles.inlineMenu}
      onClick={({ key }) => {
        setDrawerOpen(false)
        navigate(key)
      }}
    />
  )

  return (
    <Layout style={{ minHeight: '100vh' }}>
      <Header style={styles.header}>
        <Link to="/calendar" style={styles.brand}>
          OxiDav
        </Link>
        {!isMobile && (
          <nav aria-label={NAV_ARIA_LABEL} style={styles.nav}>
            {navMenu}
          </nav>
        )}
        <Space size="small" style={styles.actions}>
          {isMobile && (
            <Button
              type="text"
              aria-label="Abrir menú"
              title="Abrir menú"
              icon={<MenuOutlined />}
              onClick={() => setDrawerOpen(true)}
            />
          )}
          <Button
            type="text"
            onClick={toggle}
            aria-label="Cambiar tema"
            title={mode === 'dark' ? 'Activar tema claro' : 'Activar tema oscuro'}
            icon={mode === 'dark' ? <SunOutlined /> : <MoonOutlined />}
          />
          {authenticated && logout && (
            <Button
              type="text"
              onClick={() => {
                void logout()
              }}
              // `loading` + `disabled` bloquean una segunda pulsación mientras
              // el `POST /auth/logout` está en curso y se navega al proveedor.
              loading={loggingOut}
              disabled={loggingOut}
              aria-label="Cerrar sesión"
              title="Cerrar sesión"
              icon={<LogoutOutlined />}
            />
          )}
        </Space>
      </Header>
      {isMobile && (
        <Drawer
          open={drawerOpen}
          onClose={() => setDrawerOpen(false)}
          placement="left"
          title="OxiDav"
          destroyOnHidden
          styles={{ body: { padding: 0 } }}
        >
          <nav aria-label={NAV_ARIA_LABEL}>{navMenu}</nav>
        </Drawer>
      )}
      <Content style={styles.content}>
        {/* Local boundary so a lazy page chunk keeps the header shell on screen
            instead of unmounting the whole layout. */}
        <Suspense fallback={<RouteFallback />}>{children}</Suspense>
      </Content>
    </Layout>
  )
}
