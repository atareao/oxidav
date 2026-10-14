/**
 * Tests del guard de sesión `AuthGate` (F17).
 *
 * - mientras se resuelve `/api/auth/me` (`loading`) → indicador de carga;
 * - sin sesión → pantalla de login;
 * - con sesión → app autenticada cargada en diferido (`React.lazy`).
 *
 * `AuthProvider` y `AuthenticatedApp` se mockean para no arrastrar el proveedor
 * real ni su chunk diferido (antd) al test del guard.
 */

import { render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import { useAuth } from './contexts/AuthContext'
import type { AuthContextValue, AuthUser } from './contexts/AuthContext'

vi.mock('./contexts/AuthContext', () => ({
  useAuth: vi.fn(),
}))

vi.mock('./contexts/AuthProvider', () => ({
  AuthProvider: ({ children }: { children: ReactNode }) => children,
}))

vi.mock('./AuthenticatedApp', () => ({
  default: () => <div data-testid="app-authenticated">app</div>,
}))

const mockUseAuth = vi.mocked(useAuth)

const authedUser: AuthUser = { sub: 'u1', email: 'a@b.c', name: 'Lorenzo' }

function authValue(overrides: Partial<AuthContextValue>): AuthContextValue {
  return {
    user: null,
    loading: false,
    loggingOut: false,
    logout: async () => undefined,
    ...overrides,
  }
}

describe('App — guard de sesión', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('mientras la sesión se resuelve muestra carga y ni login ni app', () => {
    mockUseAuth.mockReturnValue(authValue({ loading: true }))

    render(<App />)

    expect(screen.getByTestId('auth-loading')).toBeInTheDocument()
    expect(screen.queryByTestId('app-authenticated')).toBeNull()
    expect(screen.queryByRole('button', { name: /iniciar sesión/i })).toBeNull()
  })

  it('sin sesión muestra la pantalla de login y no la aplicación', () => {
    mockUseAuth.mockReturnValue(authValue({ user: null, loading: false }))

    render(<App />)

    expect(screen.getByRole('button', { name: /iniciar sesión/i })).toBeInTheDocument()
    expect(screen.queryByTestId('app-authenticated')).toBeNull()
  })

  it('con sesión muestra la aplicación y no la pantalla de login', async () => {
    mockUseAuth.mockReturnValue(authValue({ user: authedUser, loading: false }))

    render(<App />)

    expect(await screen.findByTestId('app-authenticated')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /iniciar sesión/i })).toBeNull()
  })
})
