/**
 * Tests del `AuthProvider` (F17).
 *
 * Cubre el contrato del change: resolución de `GET /api/auth/me` (200 →
 * autenticado, 401 → no autenticado), reacción al evento global de 401,
 * la caché por `ref` bajo `StrictMode`, `useAuth` fuera del provider y el
 * destino del logout (solo URLs absolutas `https:` con host y sin credenciales
 * embebidas; el resto cae a la raíz).
 */

import { act, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { StrictMode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { AuthProvider } from './AuthProvider'
import { useAuth } from './AuthContext'
import { UNAUTHORIZED_EVENT } from '../services/api'

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : '',
    json: async () => body,
  } as unknown as Response
}

function Probe() {
  const { user, loading, loggingOut, logout } = useAuth()
  return (
    <div>
      <span data-testid="loading">{String(loading)}</span>
      <span data-testid="loggingOut">{String(loggingOut)}</span>
      <span data-testid="email">{user?.email ?? ''}</span>
      <button onClick={() => void logout()}>salir</button>
    </div>
  )
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

function stubFetchWithLogout(endSessionUrl: string | null | undefined) {
  const fetchMock = vi.fn(async (url: RequestInfo | URL): Promise<Response> => {
    const target = String(url)
    if (target.includes('/api/auth/logout')) {
      return jsonResponse({ end_session_url: endSessionUrl })
    }
    return jsonResponse({ sub: 'u1', email: 'a@b.c', name: 'Lorenzo' })
  })
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
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

describe('AuthProvider — resolución de la sesión', () => {
  it('expone loading en curso y resuelve el usuario tras un 200', async () => {
    let resolveFetch: (value: unknown) => void = () => undefined
    const pending = new Promise<unknown>((resolve) => {
      resolveFetch = resolve
    })
    const fetchMock = vi.fn().mockReturnValue(pending)
    vi.stubGlobal('fetch', fetchMock)

    render(
      <AuthProvider>
        <Probe />
      </AuthProvider>,
    )

    expect(screen.getByTestId('loading')).toHaveTextContent('true')
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('/api/auth/me'),
      expect.objectContaining({ credentials: 'include' }),
    )

    await act(async () => {
      resolveFetch(
        jsonResponse({ sub: 'u1', email: 'a@b.c', name: 'Lorenzo' }),
      )
    })

    await waitFor(() => expect(screen.getByTestId('loading')).toHaveTextContent('false'))
    expect(screen.getByTestId('email')).toHaveTextContent('a@b.c')
  })

  it('deja al usuario no autenticado ante un 401 en /auth/me', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ error: 'unauthorized' }, 401)))

    render(
      <AuthProvider>
        <Probe />
      </AuthProvider>,
    )

    await waitFor(() => expect(screen.getByTestId('loading')).toHaveTextContent('false'))
    expect(screen.getByTestId('email')).toHaveTextContent('')
  })

  it('deja al usuario no autenticado ante el evento global de 401', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse({ sub: 'u1', email: 'a@b.c', name: 'Lorenzo' })),
    )

    render(
      <AuthProvider>
        <Probe />
      </AuthProvider>,
    )

    await waitFor(() => expect(screen.getByTestId('email')).toHaveTextContent('a@b.c'))

    act(() => {
      window.dispatchEvent(new Event(UNAUTHORIZED_EVENT))
    })

    expect(screen.getByTestId('email')).toHaveTextContent('')
  })

  it('bajo StrictMode lanza un único GET /api/auth/me (caché por ref)', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse({ sub: 'u1', email: 'a@b.c', name: 'Lorenzo' }))
    vi.stubGlobal('fetch', fetchMock)

    render(
      <StrictMode>
        <AuthProvider>
          <Probe />
        </AuthProvider>
      </StrictMode>,
    )

    await waitFor(() => expect(screen.getByTestId('loading')).toHaveTextContent('false'))

    const meCalls = fetchMock.mock.calls.filter(([url]) => String(url).includes('/api/auth/me'))
    expect(meCalls).toHaveLength(1)
  })

  it('useAuth fuera del provider lanza un error explícito', () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    expect(() => render(<Probe />)).toThrowError(/AuthProvider/)

    consoleError.mockRestore()
  })
})

describe('AuthProvider — destino del logout', () => {
  it('con end_session_url https navega al proveedor y deja user a null', async () => {
    const location = stubLocation()
    stubFetchWithLogout('https://idp.example/logout')
    const user = userEvent.setup()

    render(
      <AuthProvider>
        <Probe />
      </AuthProvider>,
    )

    await waitFor(() => expect(screen.getByTestId('email')).toHaveTextContent('a@b.c'))

    await user.click(screen.getByRole('button', { name: /salir/i }))

    await waitFor(() => expect(location.href).toBe('https://idp.example/logout'))
    expect(screen.getByTestId('email')).toHaveTextContent('')
  })

  it('con end_session_url null cae a la raíz / de la app (solo cierre local)', async () => {
    const location = stubLocation()
    stubFetchWithLogout(null)
    const user = userEvent.setup()

    render(
      <AuthProvider>
        <Probe />
      </AuthProvider>,
    )

    await waitFor(() => expect(screen.getByTestId('email')).toHaveTextContent('a@b.c'))

    await user.click(screen.getByRole('button', { name: /salir/i }))

    await waitFor(() => expect(location.href).toBe('/'))
    expect(location.href).not.toBe('https://idp.example/logout')
  })

  it.each([
    ['http://evil.example/logout'],
    ['javascript:alert(1)'],
    ['/relative/path'],
    // Userinfo: el host real es `evil.example`, no `legit-idp.example`. Si se
    // siguiera, el `id_token_hint` viajaría a un host ajeno.
    ['https://legit-idp.example@evil.example/'],
    ['https://user:pass@idp.example/logout'],
    // `https:` sin host.
    ['https://'],
    // Protocol-relative: hereda el esquema pero apunta a un host ajeno.
    ['//evil.example/logout'],
    ['data:text/html,<script>alert(1)</script>'],
    [''],
    [null],
    [undefined],
  ])('descarta el end_session_url inseguro %s y cae a la raíz', async (unsafeUrl) => {
    const location = stubLocation()
    stubFetchWithLogout(unsafeUrl)
    const user = userEvent.setup()

    render(
      <AuthProvider>
        <Probe />
      </AuthProvider>,
    )

    await waitFor(() => expect(screen.getByTestId('email')).toHaveTextContent('a@b.c'))

    await user.click(screen.getByRole('button', { name: /salir/i }))

    await waitFor(() => expect(location.href).toBe('/'))
    expect(location.href).not.toBe(unsafeUrl)
    expect(location.href).not.toContain('evil.example')
    expect(screen.getByTestId('email')).toHaveTextContent('')
  })

  it('acepta un end_session_url con el esquema en mayúsculas (HTTPS) y navega al proveedor', async () => {
    const location = stubLocation()
    stubFetchWithLogout('HTTPS://idp.example/logout')
    const user = userEvent.setup()

    render(
      <AuthProvider>
        <Probe />
      </AuthProvider>,
    )

    await waitFor(() => expect(screen.getByTestId('email')).toHaveTextContent('a@b.c'))

    await user.click(screen.getByRole('button', { name: /salir/i }))

    // `new URL` normaliza el esquema a minúsculas, así que el destino válido se
    // conserva (navegación al proveedor, no caída a la raíz).
    await waitFor(() => expect(location.href).toBe('HTTPS://idp.example/logout'))
    expect(location.href).not.toBe('/')
    expect(screen.getByTestId('email')).toHaveTextContent('')
  })
})
