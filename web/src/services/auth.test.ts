/**
 * Tests del flujo OIDC Authorization Code + PKCE (F14).
 *
 * Se mockean `fetch` y `crypto.getRandomValues`; se usa `sessionStorage` real
 * (limpiado en cada test) y la navegación (`window.location.assign`) se
 * intercepta con el seam `__setRedirectForTests`. El hashing S256 se valida
 * contra el vector de prueba canónico de RFC 7636.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  STATE_STORAGE_KEY,
  TOKENS_STORAGE_KEY,
  VERIFIER_STORAGE_KEY,
  __resetAuthForTests,
  __setRedirectForTests,
  clearAccessToken,
  deriveCodeChallenge,
  getAccessToken,
  handleRedirectCallback,
  hasRefreshToken,
  isAuthenticated,
  login,
  logout,
  refresh,
  setAccessToken,
} from './auth'

const AUTHORITY = 'https://idp.example'
const ENDPOINTS = {
  authorization_endpoint: `${AUTHORITY}/authorize`,
  token_endpoint: `${AUTHORITY}/token`,
  end_session_endpoint: `${AUTHORITY}/logout`,
}

const realSubtle = globalThis.crypto.subtle

/**
 * Deterministic-but-varying verifier/state bytes: every `getRandomValues` call
 * fills with an incrementing value, so two `login()` calls produce two
 * different `code_verifier` / `state` values (needed to test regeneration).
 */
let randomCall = 0
function variableRandom<T extends ArrayBufferView>(array: T): T {
  randomCall += 1
  new Uint8Array(array.buffer, array.byteOffset, array.byteLength).fill(randomCall)
  return array
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

let fetchMock: ReturnType<typeof vi.fn>
let redirectMock: ReturnType<typeof vi.fn<(url: string) => void>>

function enableOidc(): void {
  vi.stubEnv('VITE_OIDC_AUTHORITY', AUTHORITY)
  vi.stubEnv('VITE_OIDC_CLIENT_ID', 'oxidav-spa')
  vi.stubEnv('VITE_OIDC_REDIRECT_URI', 'https://app.example/callback')
  vi.stubEnv('VITE_OIDC_POST_LOGOUT_REDIRECT_URI', 'https://app.example/login')
  vi.stubEnv('VITE_OIDC_SCOPE', 'openid profile email')
}

describe('auth (OIDC Authorization Code + PKCE)', () => {
  beforeEach(() => {
    window.sessionStorage.clear()
    __resetAuthForTests()
    randomCall = 0
    fetchMock = vi.fn()
    redirectMock = vi.fn<(url: string) => void>()
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('crypto', { getRandomValues: variableRandom, subtle: realSubtle })
    __setRedirectForTests((url) => redirectMock(url))
  })

  it('deriva el code_challenge S256 según el vector de RFC 7636', async () => {
    // Appendix B de RFC 7636.
    const verifier = 'dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk'
    await expect(deriveCodeChallenge(verifier)).resolves.toBe('E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM')
  })

  afterEach(() => {
    vi.unstubAllEnvs()
    vi.unstubAllGlobals()
    __resetAuthForTests()
    window.sessionStorage.clear()
  })

  it('degrada a modo dev (no OIDC) cuando falta VITE_OIDC_AUTHORITY', async () => {
    expect(isAuthenticated()).toBe(true)
    // login() es inofensivo en modo dev.
    await login()
    expect(fetchMock).not.toHaveBeenCalled()
    expect(redirectMock).not.toHaveBeenCalled()
  })

  it('usa el token manual de dev cuando no hay OIDC', () => {
    setAccessToken('dev-token')
    expect(getAccessToken()).toBe('dev-token')
    clearAccessToken()
    expect(getAccessToken()).toBeNull()
  })

  describe('login()', () => {
    beforeEach(() => enableOidc())

    it('genera PKCE, guarda el verifier/state y redirige al authorization endpoint', async () => {
      fetchMock.mockResolvedValueOnce(jsonResponse(ENDPOINTS))

      await login()

      // Discovery contra el authority configurado.
      expect(fetchMock).toHaveBeenCalledWith(
        `${AUTHORITY}/.well-known/openid-configuration`,
        expect.objectContaining({ headers: { Accept: 'application/json' } }),
      )

      expect(redirectMock).toHaveBeenCalledTimes(1)
      const url = new URL(redirectMock.mock.calls[0][0] as string)
      expect(`${url.origin}${url.pathname}`).toBe(ENDPOINTS.authorization_endpoint)

      const params = url.searchParams
      expect(params.get('response_type')).toBe('code')
      expect(params.get('client_id')).toBe('oxidav-spa')
      expect(params.get('redirect_uri')).toBe('https://app.example/callback')
      expect(params.get('scope')).toBe('openid profile email')
      expect(params.get('code_challenge_method')).toBe('S256')

      const verifier = window.sessionStorage.getItem(VERIFIER_STORAGE_KEY)
      const state = window.sessionStorage.getItem(STATE_STORAGE_KEY)
      expect(verifier).not.toBeNull()
      expect(state).not.toBeNull()
      expect(params.get('code_challenge')).toBe(await deriveCodeChallenge(verifier as string))
      expect(params.get('state')).toBe(state)
    })

    it('genera un code_challenge distinto del code_verifier', async () => {
      fetchMock.mockResolvedValueOnce(jsonResponse(ENDPOINTS))
      await login()
      const verifier = window.sessionStorage.getItem(VERIFIER_STORAGE_KEY) as string
      const url = new URL(redirectMock.mock.calls[0][0] as string)
      expect(url.searchParams.get('code_challenge')).not.toBe(verifier)
    })

    it('regenera code_verifier y state en cada intento de login', async () => {
      // El discovery se cachea tras el primer login.
      fetchMock.mockResolvedValueOnce(jsonResponse(ENDPOINTS)).mockResolvedValueOnce(ENDPOINTS)

      await login()
      const verifier1 = window.sessionStorage.getItem(VERIFIER_STORAGE_KEY)
      const state1 = window.sessionStorage.getItem(STATE_STORAGE_KEY)

      await login()
      const verifier2 = window.sessionStorage.getItem(VERIFIER_STORAGE_KEY)
      const state2 = window.sessionStorage.getItem(STATE_STORAGE_KEY)

      expect(verifier2).not.toBe(verifier1)
      expect(state2).not.toBe(state1)
      // Y el segundo state es el que viaja en la segunda redirección.
      const url2 = new URL(redirectMock.mock.calls[1][0] as string)
      expect(url2.searchParams.get('state')).toBe(state2)
    })
  })

  describe('handleRedirectCallback()', () => {
    beforeEach(() => enableOidc())

    function primeLoginState(): { verifier: string; state: string } {
      const verifier = 'test-verifier-1234567890'
      const state = 'test-state-abcdef'
      window.sessionStorage.setItem(VERIFIER_STORAGE_KEY, verifier)
      window.sessionStorage.setItem(STATE_STORAGE_KEY, state)
      return { verifier, state }
    }

    it('valida el state, intercambia el code y guarda los tokens', async () => {
      const { verifier, state } = primeLoginState()
      fetchMock
        .mockResolvedValueOnce(jsonResponse(ENDPOINTS))
        .mockResolvedValueOnce(
          jsonResponse({ access_token: 'access-1', refresh_token: 'refresh-1', expires_in: 3600 }),
        )

      const callbackUrl = `${window.location.origin}/callback?code=the-code&state=${state}`
      await handleRedirectCallback(callbackUrl)

      // Segundo fetch = token endpoint, con body urlencoded.
      const [tokenUrl, tokenInit] = fetchMock.mock.calls[1] as [string, RequestInit]
      expect(tokenUrl).toBe(ENDPOINTS.token_endpoint)
      expect(tokenInit.method).toBe('POST')
      const headers = new Headers(tokenInit.headers)
      expect(headers.get('Content-Type')).toBe('application/x-www-form-urlencoded')
      const body = new URLSearchParams(tokenInit.body as string)
      expect(body.get('grant_type')).toBe('authorization_code')
      expect(body.get('code')).toBe('the-code')
      expect(body.get('redirect_uri')).toBe('https://app.example/callback')
      expect(body.get('client_id')).toBe('oxidav-spa')
      expect(body.get('code_verifier')).toBe(verifier)

      const stored = JSON.parse(window.sessionStorage.getItem(TOKENS_STORAGE_KEY) as string)
      expect(stored.access_token).toBe('access-1')
      expect(stored.refresh_token).toBe('refresh-1')
      expect(stored.expires_at).toBeGreaterThan(Date.now())

      // El verifier/state efímeros se consumen.
      expect(window.sessionStorage.getItem(VERIFIER_STORAGE_KEY)).toBeNull()
      expect(window.sessionStorage.getItem(STATE_STORAGE_KEY)).toBeNull()
    })

    it('rechaza un state que no coincide', async () => {
      primeLoginState()
      fetchMock.mockResolvedValueOnce(jsonResponse(ENDPOINTS))

      await expect(
        handleRedirectCallback(`${window.location.origin}/callback?code=c&state=wrong`),
      ).rejects.toThrow(/state/i)

      // El state se valida antes del discovery: no hay ninguna petición de red.
      expect(fetchMock).not.toHaveBeenCalled()
      expect(window.sessionStorage.getItem(TOKENS_STORAGE_KEY)).toBeNull()
    })

    it('propaga el error del proveedor', async () => {
      primeLoginState()
      await expect(
        handleRedirectCallback(
          `${window.location.origin}/callback?error=access_denied&error_description=Denegado`,
        ),
      ).rejects.toThrow('Denegado')
    })

    it('no hace nada cuando no hay code ni error (redirección post-logout)', async () => {
      // El end-session endpoint puede rebotar a /callback sin parámetros: no
      // debe lanzar ni iniciar ningún intercambio.
      await expect(
        handleRedirectCallback(`${window.location.origin}/callback`),
      ).resolves.toBeUndefined()
      expect(fetchMock).not.toHaveBeenCalled()
      expect(window.sessionStorage.getItem(TOKENS_STORAGE_KEY)).toBeNull()
    })

    it('rechaza una respuesta malformada del token endpoint sin guardar tokens', async () => {
      primeLoginState()
      fetchMock
        .mockResolvedValueOnce(jsonResponse(ENDPOINTS))
        .mockResolvedValueOnce(jsonResponse({ token_type: 'Bearer' }))

      await expect(
        handleRedirectCallback(`${window.location.origin}/callback?code=c&state=test-state-abcdef`),
      ).rejects.toThrow(/access_token/i)

      expect(window.sessionStorage.getItem(TOKENS_STORAGE_KEY)).toBeNull()
      // El material PKCE se limpia también en el camino de error.
      expect(window.sessionStorage.getItem(VERIFIER_STORAGE_KEY)).toBeNull()
      expect(window.sessionStorage.getItem(STATE_STORAGE_KEY)).toBeNull()
    })

    it('rechaza un expires_in no numérico', async () => {
      primeLoginState()
      fetchMock.mockResolvedValueOnce(jsonResponse(ENDPOINTS)).mockResolvedValueOnce(
        jsonResponse({ access_token: 'a', expires_in: '3600' }),
      )

      await expect(
        handleRedirectCallback(`${window.location.origin}/callback?code=c&state=test-state-abcdef`),
      ).rejects.toThrow(/expires_in/i)
      expect(window.sessionStorage.getItem(TOKENS_STORAGE_KEY)).toBeNull()
    })
  })

  describe('getAccessToken() / refresh() / isAuthenticated()', () => {
    beforeEach(() => enableOidc())

    function seedTokens(tokens: {
      access_token: string
      refresh_token: string | null
      expires_at: number
    }): void {
      window.sessionStorage.setItem(TOKENS_STORAGE_KEY, JSON.stringify(tokens))
    }

    it('devuelve el token vigente e isAuthenticated() es true', () => {
      seedTokens({ access_token: 'live', refresh_token: 'r', expires_at: Date.now() + 3_600_000 })
      expect(getAccessToken()).toBe('live')
      expect(isAuthenticated()).toBe(true)
    })

    it('refresca el token al expirar y lo persiste', async () => {
      seedTokens({ access_token: 'expired', refresh_token: 'r-1', expires_at: Date.now() - 1000 })
      fetchMock
        .mockResolvedValueOnce(jsonResponse(ENDPOINTS))
        .mockResolvedValueOnce(jsonResponse({ access_token: 'fresh', expires_in: 3600 }))

      await refresh()

      const body = new URLSearchParams(fetchMock.mock.calls[1][1]?.body as string)
      expect(body.get('grant_type')).toBe('refresh_token')
      expect(body.get('refresh_token')).toBe('r-1')

      const stored = JSON.parse(window.sessionStorage.getItem(TOKENS_STORAGE_KEY) as string)
      expect(stored.access_token).toBe('fresh')
      // Se conserva el refresh_token previo cuando el endpoint no emite uno nuevo.
      expect(stored.refresh_token).toBe('r-1')
      expect(getAccessToken()).toBe('fresh')
    })

    it('coalesce refrescos concurrentes en una sola petición', async () => {
      seedTokens({ access_token: 'expired', refresh_token: 'r-1', expires_at: Date.now() - 1000 })
      fetchMock
        .mockResolvedValueOnce(jsonResponse(ENDPOINTS))
        .mockResolvedValueOnce(jsonResponse({ access_token: 'fresh', expires_in: 3600 }))

      await Promise.all([refresh(), refresh()])

      const tokenCalls = fetchMock.mock.calls.filter(([url]) => url === ENDPOINTS.token_endpoint)
      expect(tokenCalls).toHaveLength(1)
    })

    it('isAuthenticated() es false sin tokens o con token expirado', () => {
      expect(isAuthenticated()).toBe(false)
      seedTokens({ access_token: 'expired', refresh_token: 'r', expires_at: Date.now() - 1 })
      expect(isAuthenticated()).toBe(false)
    })

    it('limpia los tokens cuando el refresco falla', async () => {
      seedTokens({ access_token: 'expired', refresh_token: 'r', expires_at: Date.now() - 1000 })
      fetchMock
        .mockResolvedValueOnce(jsonResponse(ENDPOINTS))
        .mockResolvedValueOnce(jsonResponse({ error: 'invalid_grant' }, 400))

      await expect(refresh()).rejects.toThrow(/refresco/i)
      expect(window.sessionStorage.getItem(TOKENS_STORAGE_KEY)).toBeNull()
    })

    it('limpia los tokens ante una respuesta de refresco malformada', async () => {
      seedTokens({ access_token: 'expired', refresh_token: 'r', expires_at: Date.now() - 1000 })
      fetchMock
        .mockResolvedValueOnce(jsonResponse(ENDPOINTS))
        .mockResolvedValueOnce(jsonResponse({ nope: true }))

      await expect(refresh()).rejects.toThrow(/access_token/i)
      expect(window.sessionStorage.getItem(TOKENS_STORAGE_KEY)).toBeNull()
    })

    it('hasRefreshToken() refleja la presencia de un refresh_token', () => {
      expect(hasRefreshToken()).toBe(false)
      seedTokens({ access_token: 'expired', refresh_token: 'r', expires_at: Date.now() - 1 })
      expect(hasRefreshToken()).toBe(true)
    })
  })

  describe('logout()', () => {
    beforeEach(() => enableOidc())

    it('limpia la sesión y redirige al end-session endpoint', async () => {
      window.sessionStorage.setItem(
        TOKENS_STORAGE_KEY,
        JSON.stringify({
          access_token: 'a',
          refresh_token: 'r',
          expires_at: Date.now() + 1000,
        }),
      )
      window.sessionStorage.setItem(VERIFIER_STORAGE_KEY, 'v')
      fetchMock.mockResolvedValueOnce(jsonResponse(ENDPOINTS))

      await logout()

      expect(window.sessionStorage.getItem(TOKENS_STORAGE_KEY)).toBeNull()
      expect(window.sessionStorage.getItem(VERIFIER_STORAGE_KEY)).toBeNull()
      expect(isAuthenticated()).toBe(false)

      const url = new URL(redirectMock.mock.calls[0][0] as string)
      expect(`${url.origin}${url.pathname}`).toBe(ENDPOINTS.end_session_endpoint)
      expect(url.searchParams.get('post_logout_redirect_uri')).toBe('https://app.example/login')
    })

    it('limpia la sesión aunque falle el discovery', async () => {
      window.sessionStorage.setItem(
        TOKENS_STORAGE_KEY,
        JSON.stringify({ access_token: 'a', refresh_token: 'r', expires_at: Date.now() + 1000 }),
      )
      fetchMock.mockRejectedValueOnce(new Error('offline'))

      await expect(logout()).resolves.toBeUndefined()
      expect(window.sessionStorage.getItem(TOKENS_STORAGE_KEY)).toBeNull()
      expect(redirectMock).not.toHaveBeenCalled()
    })
  })
})
