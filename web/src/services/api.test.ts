/**
 * Tests del cliente HTTP (F14): inyección del `Authorization: Bearer` y política
 * de renovación `401 → refresh → reintento único`, con el módulo `auth` mockeado
 * para controlar token, modo OIDC y resultado del refresco.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError, request } from './api'
import { getAccessToken, isOidcEnabled, logout, refresh } from './auth'

vi.mock('./auth', () => ({
  getAccessToken: vi.fn(),
  isOidcEnabled: vi.fn(),
  refresh: vi.fn(),
  logout: vi.fn(),
}))

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function errorResponse(status: number, code = 'unauthorized', message = 'No autorizado'): Response {
  return jsonResponse({ error: { code, message } }, status)
}

let fetchMock: ReturnType<typeof vi.fn>

describe('api request', () => {
  beforeEach(() => {
    fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    vi.mocked(getAccessToken).mockReturnValue('token-1')
    vi.mocked(isOidcEnabled).mockReturnValue(false)
    vi.mocked(refresh).mockResolvedValue(undefined)
    vi.mocked(logout).mockResolvedValue(undefined)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.clearAllMocks()
  })

  it('inyecta Authorization: Bearer con el token actual', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse({ ok: true }))

    const result = await request<{ ok: boolean }>('/v1/me')

    expect(result).toEqual({ ok: true })
    const init = fetchMock.mock.calls[0][1] as RequestInit
    expect(new Headers(init.headers).get('Authorization')).toBe('Bearer token-1')
  })

  it('no añade Authorization cuando no hay token', async () => {
    vi.mocked(getAccessToken).mockReturnValue(null)
    fetchMock.mockResolvedValueOnce(jsonResponse({ ok: true }))

    await request('/v1/me')

    const init = fetchMock.mock.calls[0][1] as RequestInit
    expect(new Headers(init.headers).has('Authorization')).toBe(false)
  })

  it('en 401 refresca una vez y reintenta con el nuevo token', async () => {
    vi.mocked(isOidcEnabled).mockReturnValue(true)
    vi.mocked(getAccessToken).mockReturnValueOnce('stale').mockReturnValueOnce('fresh')
    fetchMock
      .mockResolvedValueOnce(errorResponse(401))
      .mockResolvedValueOnce(jsonResponse({ ok: true }))

    const result = await request<{ ok: boolean }>('/v1/me')

    expect(result).toEqual({ ok: true })
    expect(refresh).toHaveBeenCalledTimes(1)
    expect(fetchMock).toHaveBeenCalledTimes(2)
    const retryInit = fetchMock.mock.calls[1][1] as RequestInit
    expect(new Headers(retryInit.headers).get('Authorization')).toBe('Bearer fresh')
    expect(logout).not.toHaveBeenCalled()
  })

  it('si el refresco falla, cierra sesión y propaga el error (sin reintentar)', async () => {
    vi.mocked(isOidcEnabled).mockReturnValue(true)
    vi.mocked(refresh).mockRejectedValueOnce(new Error('refresh caído'))
    fetchMock.mockResolvedValueOnce(errorResponse(401))

    await expect(request('/v1/me')).rejects.toThrow('No autorizado')

    expect(refresh).toHaveBeenCalledTimes(1)
    expect(logout).toHaveBeenCalledTimes(1)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('nunca reintenta más de una vez aunque siga devolviendo 401', async () => {
    vi.mocked(isOidcEnabled).mockReturnValue(true)
    fetchMock.mockResolvedValue(errorResponse(401))

    await expect(request('/v1/me')).rejects.toBeInstanceOf(ApiError)

    expect(refresh).toHaveBeenCalledTimes(1)
    expect(fetchMock).toHaveBeenCalledTimes(2)
  })

  it('en modo dev (sin OIDC) no intenta refrescar ante 401', async () => {
    vi.mocked(isOidcEnabled).mockReturnValue(false)
    fetchMock.mockResolvedValueOnce(errorResponse(401))

    await expect(request('/v1/me')).rejects.toBeInstanceOf(ApiError)

    expect(refresh).not.toHaveBeenCalled()
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('mapea una caída de red a un ApiError(0, network)', async () => {
    fetchMock.mockRejectedValueOnce(new TypeError('fetch failed'))
    const error = await request('/v1/me').catch((e: unknown) => e)
    expect(error).toBeInstanceOf(ApiError)
    expect((error as ApiError).code).toBe('network')
  })
})
