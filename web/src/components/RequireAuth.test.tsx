/**
 * Tests del guard de rutas (F14).
 *
 * En modo dev (sin `VITE_OIDC_AUTHORITY`) el guard deja pasar; en modo real sin
 * sesión redirige a `/login`.
 */

import { render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { RequireAuth } from './RequireAuth'
import { TOKENS_STORAGE_KEY, __resetAuthForTests } from '../services/auth'

const AUTHORITY = 'https://idp.example'
const ENDPOINTS = {
  authorization_endpoint: `${AUTHORITY}/authorize`,
  token_endpoint: `${AUTHORITY}/token`,
  end_session_endpoint: `${AUTHORITY}/logout`,
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function seedTokens(tokens: {
  access_token: string
  refresh_token: string | null
  expires_at: number
}): void {
  window.sessionStorage.setItem(TOKENS_STORAGE_KEY, JSON.stringify(tokens))
}

function renderGuard() {
  return render(
    <MemoryRouter initialEntries={['/calendar']}>
      <Routes>
        <Route path="/login" element={<div>Página de login</div>} />
        <Route
          path="/calendar"
          element={
            <RequireAuth>
              <div>Contenido protegido</div>
            </RequireAuth>
          }
        />
      </Routes>
    </MemoryRouter>,
  )
}

describe('RequireAuth', () => {
  beforeEach(() => {
    window.sessionStorage.clear()
    __resetAuthForTests()
  })

  afterEach(() => {
    vi.unstubAllEnvs()
    vi.unstubAllGlobals()
    __resetAuthForTests()
    window.sessionStorage.clear()
  })

  it('en modo dev (sin OIDC) deja pasar a las rutas protegidas', () => {
    renderGuard()
    expect(screen.getByText('Contenido protegido')).toBeInTheDocument()
    expect(screen.queryByText('Página de login')).not.toBeInTheDocument()
  })

  it('en modo real sin sesión redirige a /login', () => {
    vi.stubEnv('VITE_OIDC_AUTHORITY', 'https://idp.example')
    renderGuard()
    expect(screen.getByText('Página de login')).toBeInTheDocument()
    expect(screen.queryByText('Contenido protegido')).not.toBeInTheDocument()
  })

  it('con access token expirado y refresh válido no redirige a /login', async () => {
    vi.stubEnv('VITE_OIDC_AUTHORITY', AUTHORITY)
    seedTokens({
      access_token: 'expired-token',
      refresh_token: 'valid-refresh-token',
      expires_at: Date.now() - 1_000,
    })

    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = input.toString()
      if (url.endsWith('/.well-known/openid-configuration')) {
        return Promise.resolve(jsonResponse(ENDPOINTS))
      }
      if (url === ENDPOINTS.token_endpoint) {
        return Promise.resolve(
          jsonResponse({
            access_token: 'fresh-token',
            refresh_token: 'valid-refresh-token',
            expires_in: 3_600,
          }),
        )
      }
      return Promise.reject(new Error(`fetch inesperado: ${url}`))
    })
    vi.stubGlobal('fetch', fetchMock)

    renderGuard()

    // Mientras el refresco silencioso está en vuelo el guard no renderiza nada,
    // así que una sesión válida nunca parpadea la redirección a login.
    expect(screen.queryByText('Página de login')).not.toBeInTheDocument()

    // Tras resolverse el refresco se muestra el contenido protegido.
    expect(await screen.findByText('Contenido protegido')).toBeInTheDocument()
    expect(screen.queryByText('Página de login')).not.toBeInTheDocument()
    expect(fetchMock).toHaveBeenCalledWith(
      ENDPOINTS.token_endpoint,
      expect.objectContaining({ method: 'POST' }),
    )
  })
})
