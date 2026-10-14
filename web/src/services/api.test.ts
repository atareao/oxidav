/**
 * Tests del cliente HTTP (F17): toda petición viaja con `credentials: "include"`
 * y **sin** cabecera `Authorization`; un 401 en una ruta no auth emite el evento
 * global `oxidav:unauthorized`, mientras que un 401 en `/auth/*` (parte del
 * flujo) queda silencioso.
 */

import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError, api, request, UNAUTHORIZED_EVENT } from './api'

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    json: async () => body,
  } as unknown as Response
}

function errorResponse(status: number, code = 'unauthorized', message = 'No autorizado'): Response {
  return jsonResponse({ error: { code, message } }, status)
}

function lastInit(fetchMock: ReturnType<typeof vi.fn>): RequestInit {
  return fetchMock.mock.calls[0][1] as RequestInit
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('api request — credenciales y sin token', () => {
  it('envía credentials: include en un GET', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ ok: true }))
    vi.stubGlobal('fetch', fetchMock)

    await request('/v1/me')

    expect(lastInit(fetchMock)).toMatchObject({ credentials: 'include' })
    expect(new Headers(lastInit(fetchMock).headers).has('Authorization')).toBe(false)
  })

  it('envía credentials: include en un POST y no añade Authorization', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ ok: true }))
    vi.stubGlobal('fetch', fetchMock)

    await api.post('/v1/app-tokens', { name: 'iOS' })

    const init = lastInit(fetchMock)
    expect(init).toMatchObject({ method: 'POST', credentials: 'include' })
    expect(new Headers(init.headers).has('Authorization')).toBe(false)
  })
})

describe('api request — detección global de 401', () => {
  it('un 401 en una ruta no auth dispara oxidav:unauthorized', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(errorResponse(401)))
    const handler = vi.fn()
    window.addEventListener(UNAUTHORIZED_EVENT, handler)

    await api.get('/v1/me').catch(() => undefined)

    window.removeEventListener(UNAUTHORIZED_EVENT, handler)
    expect(handler).toHaveBeenCalledTimes(1)
  })

  it.each(['/auth/me', '/auth/logout'])(
    'un 401 en %s NO dispara el evento global (es parte del flujo)',
    async (path) => {
      vi.stubGlobal('fetch', vi.fn().mockResolvedValue(errorResponse(401)))
      const handler = vi.fn()
      window.addEventListener(UNAUTHORIZED_EVENT, handler)

      await request(path, path === '/auth/logout' ? { method: 'POST' } : undefined).catch(
        () => undefined,
      )

      window.removeEventListener(UNAUTHORIZED_EVENT, handler)
      expect(handler).not.toHaveBeenCalled()
    },
  )
})

describe('api request — mapeo de errores', () => {
  it('mapea el sobre de error F06 a un ApiError con code y message', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(errorResponse(422, 'invalid', 'Datos inválidos')))

    const error = await request('/v1/me').catch((e: unknown) => e)

    expect(error).toBeInstanceOf(ApiError)
    expect((error as ApiError).status).toBe(422)
    expect((error as ApiError).code).toBe('invalid')
    expect((error as ApiError).message).toBe('Datos inválidos')
  })

  it('mapea una caída de red a un ApiError(0, network)', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('fetch failed')))

    const error = await request('/v1/me').catch((e: unknown) => e)

    expect(error).toBeInstanceOf(ApiError)
    expect((error as ApiError).code).toBe('network')
    expect((error as ApiError).status).toBe(0)
  })

  it('devuelve undefined ante un 204', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse(undefined, 204)))

    await expect(api.delete('/v1/app-tokens/1')).resolves.toBeUndefined()
  })
})

describe('api — rutas de autenticación', () => {
  it('getMe resuelve la identidad', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse({ sub: 'u1', email: 'a@b.c', name: 'Lorenzo' })),
    )

    await expect(api.getMe()).resolves.toEqual({ sub: 'u1', email: 'a@b.c', name: 'Lorenzo' })
    expect(lastInit(vi.mocked(fetch))).toMatchObject({ credentials: 'include' })
  })

  it('logout lee end_session_url tal y como lo serializa el backend', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse({ end_session_url: 'https://idp.example/logout' })),
    )

    const result = await api.logout()

    expect(result).toEqual({ end_session_url: 'https://idp.example/logout' })
    expect(vi.mocked(fetch).mock.calls[0][0]).toContain('/api/auth/logout')
    expect(lastInit(vi.mocked(fetch))).toMatchObject({ method: 'POST' })
  })

  it('logout acepta end_session_url: null (solo cierre local)', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ end_session_url: null })))

    await expect(api.logout()).resolves.toEqual({ end_session_url: null })
  })
})
