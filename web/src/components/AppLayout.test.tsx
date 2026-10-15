/**
 * Tests del shell `AppLayout` (F17): el control de cierre de sesión.
 *
 * Verifican que el header expone un control accesible de cierre de sesión, que
 * al pulsarlo invoca `logout` (y, con el `AuthProvider` real, navega al destino
 * `https` que decide el backend) y que queda deshabilitado mientras
 * `loggingOut` es `true`, para evitar dobles pulsaciones.
 */

import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import { AppLayout } from './AppLayout'
import { AuthProvider } from '../contexts/AuthProvider'
import { AuthContext, type AuthContextValue } from '../contexts/AuthContext'
import { ThemeProvider } from '../contexts/ThemeContext'

function renderWithAuth(value: AuthContextValue) {
  return render(
    <MemoryRouter>
      <ThemeProvider>
        <AuthContext.Provider value={value}>
          <AppLayout>
            <div>contenido</div>
          </AppLayout>
        </AuthContext.Provider>
      </ThemeProvider>
    </MemoryRouter>,
  )
}

function renderAt(path: string, value: AuthContextValue = authValue()) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <ThemeProvider>
        <AuthContext.Provider value={value}>
          <AppLayout>
            <div>contenido</div>
          </AppLayout>
        </AuthContext.Provider>
      </ThemeProvider>
    </MemoryRouter>,
  )
}

function authValue(overrides: Partial<AuthContextValue> = {}): AuthContextValue {
  return {
    user: { sub: 'u1', email: 'a@b.c', name: 'Lorenzo' },
    loading: false,
    loggingOut: false,
    logout: vi.fn(async () => undefined),
    ...overrides,
  }
}

const originalLocation = window.location
const originalMatchMedia = window.matchMedia

/**
 * Simulate a phone-sized viewport so `Grid.useBreakpoint()` reports `xs: true`
 * (the default test stub reports every breakpoint as `false`, i.e. desktop).
 */
function useMobileViewport() {
  window.matchMedia = (query: string): MediaQueryList =>
    ({
      matches: query.includes('max-width'),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList
}

function stubLocation() {
  const location = {
    href: '',
    assign: vi.fn((url: string) => {
      location.href = url
    }),
    replace: vi.fn((url: string) => {
      location.href = url
    }),
  }
  Object.defineProperty(window, 'location', {
    configurable: true,
    writable: true,
    value: location,
  })
  return location
}

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  window.matchMedia = originalMatchMedia
  Object.defineProperty(window, 'location', {
    configurable: true,
    writable: true,
    value: originalLocation,
  })
})

describe('AppLayout — control de cierre de sesión', () => {
  it('expone un control accesible de cierre de sesión con sesión activa', () => {
    renderWithAuth(authValue())

    const button = screen.getByRole('button', { name: /cerrar sesión/i })
    expect(button).toBeInTheDocument()
    expect(button).toHaveAttribute('title', 'Cerrar sesión')
  })

  it('no muestra el control cuando no hay sesión', () => {
    renderWithAuth(authValue({ user: null }))

    expect(screen.queryByRole('button', { name: /cerrar sesión/i })).toBeNull()
  })

  it('al pulsarlo invoca logout', async () => {
    const user = userEvent.setup()
    const logout = vi.fn(async () => undefined)
    renderWithAuth(authValue({ logout }))

    await user.click(screen.getByRole('button', { name: /cerrar sesión/i }))

    await waitFor(() => expect(logout).toHaveBeenCalledTimes(1))
  })

  it('queda deshabilitado mientras loggingOut es true', () => {
    renderWithAuth(authValue({ loggingOut: true }))

    expect(screen.getByRole('button', { name: /cerrar sesión/i })).toBeDisabled()
  })

  it('con el AuthProvider real navega al end_session_url https que decide el backend', async () => {
    const location = stubLocation()
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: RequestInfo | URL): Promise<Response> => {
        const target = String(url)
        if (target.includes('/api/auth/logout')) {
          return jsonResponse({ end_session_url: 'https://idp.example/logout' })
        }
        return jsonResponse({ sub: 'u1', email: 'a@b.c', name: 'Lorenzo' })
      }),
    )
    const user = userEvent.setup()

    render(
      <MemoryRouter>
        <AuthProvider>
          <ThemeProvider>
            <AppLayout>
              <div>contenido</div>
            </AppLayout>
          </ThemeProvider>
        </AuthProvider>
      </MemoryRouter>,
    )

    await user.click(await screen.findByRole('button', { name: /cerrar sesión/i }))

    await waitFor(() => expect(location.href).toBe('https://idp.example/logout'))
  })
})

describe('AppLayout — navegación del encabezado', () => {
  const NAV_LABELS = [/calendario/i, /contactos/i, /sincronización/i, /ajustes/i] as const

  it('muestra los 4 items de navegación y marca el activo según la ruta', () => {
    renderAt('/contacts')

    const nav = screen.getByRole('navigation', { name: /principal/i })
    for (const label of NAV_LABELS) {
      expect(within(nav).getByRole('menuitem', { name: label })).toBeInTheDocument()
    }

    expect(within(nav).getByRole('menuitem', { name: /contactos/i })).toHaveClass(
      'ant-menu-item-selected',
    )
    expect(within(nav).getByRole('menuitem', { name: /calendario/i })).not.toHaveClass(
      'ant-menu-item-selected',
    )
  })

  it('en móvil, el botón hamburguesa abre el Drawer con los mismos items', async () => {
    useMobileViewport()
    const user = userEvent.setup()
    renderAt('/calendar')

    // The inline desktop navigation is not rendered on small screens.
    expect(screen.queryByRole('navigation', { name: /principal/i })).toBeNull()

    await user.click(screen.getByRole('button', { name: /abrir menú/i }))

    const drawerNav = await screen.findByRole('navigation', { name: /principal/i })
    for (const label of NAV_LABELS) {
      expect(within(drawerNav).getByRole('menuitem', { name: label })).toBeInTheDocument()
    }
  })
})

describe('AppLayout — contratos de regresión', () => {
  it('mantiene el control de cierre de sesión en loading y disabled bajo loggingOut', () => {
    renderWithAuth(authValue({ loggingOut: true }))

    const button = screen.getByRole('button', { name: /cerrar sesión/i })
    expect(button).toHaveAttribute('title', 'Cerrar sesión')
    expect(button).toBeDisabled()
  })

  it('se renderiza sin <AuthProvider> y sin control de cierre de sesión', () => {
    render(
      <MemoryRouter>
        <ThemeProvider>
          <AppLayout>
            <div>contenido</div>
          </AppLayout>
        </ThemeProvider>
      </MemoryRouter>,
    )

    expect(screen.getByText('contenido')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /cerrar sesión/i })).toBeNull()
  })
})

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : '',
    json: async () => body,
  } as unknown as Response
}
