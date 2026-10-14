/**
 * Tests del shell `AppLayout` (F17): el control de cierre de sesión.
 *
 * Verifican que el header expone un control accesible de cierre de sesión, que
 * al pulsarlo invoca `logout` (y, con el `AuthProvider` real, navega al destino
 * `https` que decide el backend) y que queda deshabilitado mientras
 * `loggingOut` es `true`, para evitar dobles pulsaciones.
 */

import { render, screen, waitFor } from '@testing-library/react'
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

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : '',
    json: async () => body,
  } as unknown as Response
}
