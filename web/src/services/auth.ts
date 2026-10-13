/**
 * OIDC Authorization Code + PKCE token provider for the SPA (F14).
 *
 * Implements the redirect dance against **Pocket ID** without a third-party
 * library: `code_verifier` / `code_challenge` are derived with `crypto.subtle`
 * (S256) and the flow is driven by a small, fully testable state machine.
 * See `plans/PLAN-015-settings-profile.md` (decision: own implementation over
 * `oidc-client-ts`) and `docs/F14-settings-profile.md`.
 *
 * Storage policy (see plan):
 * - the primary token cache lives **in memory**;
 * - it is mirrored into `sessionStorage` so a tab reload survives, but never
 *   into `localStorage` (long-lived XSS exposure).
 * - the ephemeral `code_verifier` / `state` are kept in `sessionStorage` only
 *   for the duration of one login round-trip and deleted right after use.
 *
 * Dev / mock mode: when `VITE_OIDC_AUTHORITY` is not configured the module
 * degrades to the previous manual provider (`VITE_DEV_ACCESS_TOKEN` or a token
 * set via {@link setAccessToken}) so `npm run dev` and the existing tests keep
 * working. In that mode the route guard lets every request through.
 */

/** `sessionStorage` keys (see module docstring). All are tab-scoped. */
export const TOKENS_STORAGE_KEY = 'oxidav.oidc.tokens'
export const VERIFIER_STORAGE_KEY = 'oxidav.oidc.code_verifier'
export const STATE_STORAGE_KEY = 'oxidav.oidc.state'
/**
 * `sessionStorage` key holding the in-app route the user wanted before being
 * bounced to `/login`. Survives the full-page OIDC round-trip (React Router
 * location state does not) and is single-use.
 */
export const RETURN_TO_STORAGE_KEY = 'oxidav.oidc.return_to'

/** `sessionStorage` key for a manually-provisioned dev token. */
export const ACCESS_TOKEN_STORAGE_KEY = 'oxidav.access_token'

/**
 * Refresh the access token when it is this close (ms) to expiring, so an
 * in-flight request does not race the expiry.
 */
const EXPIRY_SKEW_MS = 60_000

/** Cached token set. `expires_at` is an absolute epoch timestamp (ms). */
export interface TokenSet {
  access_token: string
  refresh_token: string | null
  expires_at: number
}

/** Relevant subset of the OpenID Connect discovery document. */
export interface OidcEndpoints {
  authorization_endpoint: string
  token_endpoint: string
  end_session_endpoint?: string
}

/** Shape of a successful token endpoint response (`application/json`). */
interface TokenResponse {
  access_token: string
  refresh_token?: string
  expires_in?: number
}

/** Resolved OIDC configuration (all fields required once enabled). */
interface OidcConfig {
  authority: string
  clientId: string
  redirectUri: string
  /**
   * Where Pocket ID sends the browser back after the end-session endpoint.
   * Must be a dedicated route (default `/login`), never the OIDC `/callback`,
   * which would replay the (code-less) redirect and surface a spurious error.
   */
  postLogoutRedirectUri: string
  scope: string
}

/** Reads a Vite env var as a trimmed string (empty string when unset). */
function readEnv(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

/**
 * Resolve the OIDC configuration from the Vite environment.
 *
 * Returns `null` when `VITE_OIDC_AUTHORITY` is unset, which is the signal to
 * fall back to dev/mock behaviour.
 */
function getConfig(): OidcConfig | null {
  const authority = readEnv(import.meta.env.VITE_OIDC_AUTHORITY)
  if (authority.length === 0) {
    return null
  }
  const redirectUri =
    readEnv(import.meta.env.VITE_OIDC_REDIRECT_URI) || `${window.location.origin}/callback`
  const postLogoutRedirectUri =
    readEnv(import.meta.env.VITE_OIDC_POST_LOGOUT_REDIRECT_URI) ||
    `${window.location.origin}/login`
  return {
    authority: authority.replace(/\/+$/, ''),
    clientId: readEnv(import.meta.env.VITE_OIDC_CLIENT_ID),
    redirectUri,
    postLogoutRedirectUri,
    scope: readEnv(import.meta.env.VITE_OIDC_SCOPE) || 'openid profile email',
  }
}

/** `true` when the real Pocket ID flow is configured (authority set). */
export function isOidcEnabled(): boolean {
  return getConfig() !== null
}

// ---------------------------------------------------------------------------
// Storage (memory + sessionStorage)
// ---------------------------------------------------------------------------

/** Primary, in-memory copy of the token set. */
let memoryTokens: TokenSet | null = null

function safeSessionStorage(): Storage | null {
  try {
    return window.sessionStorage
  } catch {
    // `sessionStorage` can throw in some privacy modes; degrade to memory-only.
    return null
  }
}

function readTokens(): TokenSet | null {
  if (memoryTokens !== null) {
    return memoryTokens
  }
  const raw = safeSessionStorage()?.getItem(TOKENS_STORAGE_KEY)
  if (raw == null) {
    return null
  }
  try {
    const parsed = JSON.parse(raw) as Partial<TokenSet>
    if (typeof parsed.access_token !== 'string' || typeof parsed.expires_at !== 'number') {
      return null
    }
    memoryTokens = {
      access_token: parsed.access_token,
      refresh_token: typeof parsed.refresh_token === 'string' ? parsed.refresh_token : null,
      expires_at: parsed.expires_at,
    }
    return memoryTokens
  } catch {
    return null
  }
}

function writeTokens(tokens: TokenSet): void {
  memoryTokens = tokens
  safeSessionStorage()?.setItem(TOKENS_STORAGE_KEY, JSON.stringify(tokens))
}

function clearTokens(): void {
  memoryTokens = null
  safeSessionStorage()?.removeItem(TOKENS_STORAGE_KEY)
}

// ---------------------------------------------------------------------------
// Dev / mock mode fallback
// ---------------------------------------------------------------------------

function readManualToken(): string | null {
  return safeSessionStorage()?.getItem(ACCESS_TOKEN_STORAGE_KEY) ?? null
}

function readDevToken(): string | null {
  const token = readEnv(import.meta.env.VITE_DEV_ACCESS_TOKEN)
  return token.length > 0 ? token : null
}

// ---------------------------------------------------------------------------
// PKCE helpers
// ---------------------------------------------------------------------------

/** Base64url-encode a byte array (no padding), per RFC 7636. */
function base64UrlEncode(bytes: Uint8Array): string {
  let binary = ''
  for (const byte of bytes) {
    binary += String.fromCharCode(byte)
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

/** Generate a random base64url string of `byteLength` bytes (default 32). */
export function generateCodeVerifier(byteLength = 32): string {
  const bytes = new Uint8Array(byteLength)
  crypto.getRandomValues(bytes)
  return base64UrlEncode(bytes)
}

/** Derive `BASE64URL(SHA-256(verifier))` — the S256 `code_challenge`. */
export async function deriveCodeChallenge(verifier: string): Promise<string> {
  const data = new TextEncoder().encode(verifier)
  const digest = await crypto.subtle.digest('SHA-256', data)
  return base64UrlEncode(new Uint8Array(digest))
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

const discoveryCache = new Map<string, OidcEndpoints>()

/** Drop the cached discovery document (useful across env changes / tests). */
export function clearDiscoveryCache(): void {
  discoveryCache.clear()
}

async function discover(authority: string): Promise<OidcEndpoints> {
  const cached = discoveryCache.get(authority)
  if (cached !== undefined) {
    return cached
  }
  const response = await fetch(`${authority}/.well-known/openid-configuration`, {
    headers: { Accept: 'application/json' },
  })
  if (!response.ok) {
    throw new Error(`No se pudo obtener la configuración OIDC (${response.status})`)
  }
  const endpoints = (await response.json()) as OidcEndpoints
  discoveryCache.set(authority, endpoints)
  return endpoints
}

// ---------------------------------------------------------------------------
// Navigation seam (overridable in tests; jsdom cannot navigate)
// ---------------------------------------------------------------------------

function defaultRedirect(url: string): void {
  window.location.assign(url)
}

let redirectFn: (url: string) => void = defaultRedirect

/**
 * Replace the navigation implementation. Pass `null` to restore the default.
 *
 * @internal test seam — jsdom has no real navigation.
 */
export function __setRedirectForTests(fn: ((url: string) => void) | null): void {
  redirectFn = fn ?? defaultRedirect
}

// ---------------------------------------------------------------------------
// Login / callback / refresh / logout
// ---------------------------------------------------------------------------

/**
 * Accept only safe, internal return routes.
 *
 * Rejects external and protocol-relative URLs (`//evil.com` would be read as
 * `https://evil.com` by `window.location.assign`), as well as the auth routes
 * themselves (`/login`, `/callback` and their query/hash subroutes), which would
 * loop back into the OIDC flow. Anything else yields `null`.
 *
 * Backslashes and control characters are also rejected: the URL parser strips
 * control chars (`\t`, `\n`, `\r`) and normalizes `\` to `/` for special
 * schemes, so `/\evil.com` or `/\t/evil.com` could otherwise be read as the
 * protocol-relative `//evil.com` (external).
 *
 * The auth-route check is case-insensitive and ignores a trailing slash, to
 * match how React Router resolves `/LOGIN`, `/Login/`, `/callback/`, etc.
 */
function sanitizeReturnTo(value: unknown): string | null {
  if (typeof value !== 'string' || !value.startsWith('/') || value.startsWith('//')) {
    return null
  }
  // Control chars (which the URL parser strips, rebinding the path) and
  // backslashes (normalized to `/`) would let e.g. `/\t/evil.com` resolve to
  // the external `//evil.com`.
  for (const char of value) {
    const code = char.charCodeAt(0)
    if (code <= 0x1f || code === 0x7f || char === '\\') {
      return null
    }
  }
  const path = value.split(/[?#]/, 1)[0].toLowerCase().replace(/\/+$/, '')
  if (path === '/login' || path === '/callback') {
    return null
  }
  return value
}

/**
 * Start the Authorization Code + PKCE dance.
 *
 * Generates and stores the `code_verifier` + `state`, then redirects the
 * browser to Pocket ID's `authorization_endpoint`. No-op in dev mode.
 *
 * When `returnTo` is a safe internal route it is remembered in
 * `sessionStorage` so the callback can send the user back there; otherwise any
 * previously stored route is dropped.
 */
export async function login(returnTo?: string): Promise<void> {
  const sanitized = sanitizeReturnTo(returnTo)
  const storage = safeSessionStorage()
  if (sanitized === null) {
    storage?.removeItem(RETURN_TO_STORAGE_KEY)
  } else {
    storage?.setItem(RETURN_TO_STORAGE_KEY, sanitized)
  }

  const config = getConfig()
  if (config === null) {
    return
  }
  const endpoints = await discover(config.authority)

  const verifier = generateCodeVerifier()
  const challenge = await deriveCodeChallenge(verifier)
  const state = generateCodeVerifier(16)

  storage?.setItem(VERIFIER_STORAGE_KEY, verifier)
  storage?.setItem(STATE_STORAGE_KEY, state)

  const url = new URL(endpoints.authorization_endpoint)
  url.searchParams.set('response_type', 'code')
  url.searchParams.set('client_id', config.clientId)
  url.searchParams.set('redirect_uri', config.redirectUri)
  url.searchParams.set('scope', config.scope)
  url.searchParams.set('code_challenge', challenge)
  url.searchParams.set('code_challenge_method', 'S256')
  url.searchParams.set('state', state)

  redirectFn(url.toString())
}

/**
 * Validate the token endpoint payload at runtime before trusting it.
 *
 * A `200` with a malformed body (e.g. `{}` or `access_token: null`) must never
 * be persisted — that would silently poison the cache with an unusable token.
 * `expires_in` / `refresh_token` are optional, but when present must be of the
 * right type.
 */
function parseTokenResponse(payload: unknown): TokenResponse {
  if (typeof payload !== 'object' || payload === null) {
    throw new Error('El token endpoint devolvió una respuesta inválida')
  }
  const record = payload as Record<string, unknown>

  const accessToken = record.access_token
  if (typeof accessToken !== 'string' || accessToken.length === 0) {
    throw new Error('El token endpoint no devolvió un access_token válido')
  }
  if (record.expires_in !== undefined && typeof record.expires_in !== 'number') {
    throw new Error('El token endpoint devolvió un expires_in inválido')
  }
  if (record.refresh_token !== undefined && typeof record.refresh_token !== 'string') {
    throw new Error('El token endpoint devolvió un refresh_token inválido')
  }

  return {
    access_token: accessToken,
    refresh_token: typeof record.refresh_token === 'string' ? record.refresh_token : undefined,
    expires_in: typeof record.expires_in === 'number' ? record.expires_in : undefined,
  }
}

/** Turn a token endpoint response into a {@link TokenSet} and persist it. */
function storeTokenResponse(json: TokenResponse, previousRefresh: string | null): TokenSet {
  const expiresIn = json.expires_in ?? 3600
  const tokens: TokenSet = {
    access_token: json.access_token,
    refresh_token: json.refresh_token ?? previousRefresh,
    expires_at: Date.now() + expiresIn * 1000,
  }
  writeTokens(tokens)
  return tokens
}

/**
 * Handle the redirect back from Pocket ID.
 *
 * Validates `state`, exchanges `code` for tokens at the `token_endpoint`, then
 * strips the OAuth query parameters from the address bar. No-op in dev mode.
 *
 * A redirect **without** `code` and without `error` (e.g. the post-logout
 * bounce back from `end_session_endpoint`) is a no-op, not an error: there is
 * nothing to exchange. Ephemeral `code_verifier` / `state` and stale OAuth
 * query parameters are cleared in a `finally`, so both success and failure
 * leave the browser clean.
 */
export async function handleRedirectCallback(url = window.location.href): Promise<void> {
  const config = getConfig()
  if (config === null) {
    return
  }

  const parsed = new URL(url, window.location.origin)
  const params = parsed.searchParams
  const storage = safeSessionStorage()

  try {
    const error = params.get('error')
    if (error !== null) {
      throw new Error(params.get('error_description') ?? error)
    }

    const code = params.get('code')
    if (code === null) {
      // Post-logout redirect (or a stray visit): nothing to exchange.
      return
    }

    const expectedState = storage?.getItem(STATE_STORAGE_KEY) ?? null
    const state = params.get('state')
    if (expectedState === null || state === null || state !== expectedState) {
      throw new Error('El parámetro "state" no es válido')
    }

    const verifier = storage?.getItem(VERIFIER_STORAGE_KEY) ?? null
    if (verifier === null) {
      throw new Error('No se encontró el code_verifier de la sesión')
    }

    const endpoints = await discover(config.authority)
    const body = new URLSearchParams({
      grant_type: 'authorization_code',
      code,
      redirect_uri: config.redirectUri,
      client_id: config.clientId,
      code_verifier: verifier,
    })
    const response = await fetch(endpoints.token_endpoint, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
        Accept: 'application/json',
      },
      body: body.toString(),
    })
    if (!response.ok) {
      throw new Error(`El intercambio de tokens falló (${response.status})`)
    }

    const json = parseTokenResponse(await response.json())
    storeTokenResponse(json, null)
  } finally {
    // Consume the single-use PKCE material and strip ?code=…&state=… so a
    // reload does not replay the callback — on failure too.
    storage?.removeItem(VERIFIER_STORAGE_KEY)
    storage?.removeItem(STATE_STORAGE_KEY)
    window.history.replaceState({}, document.title, parsed.pathname + parsed.hash)
  }
}

/**
 * Read and clear the pending post-login route (single-use).
 *
 * Returns the sanitized route stored by {@link login} or `null` when there is
 * none — the key is always removed so a reload cannot replay the redirect.
 */
export function consumeReturnTo(): string | null {
  const storage = safeSessionStorage()
  const raw = storage?.getItem(RETURN_TO_STORAGE_KEY) ?? null
  storage?.removeItem(RETURN_TO_STORAGE_KEY)
  return sanitizeReturnTo(raw)
}

/** In-flight refresh promise, so concurrent callers share one round-trip. */
let refreshPromise: Promise<void> | null = null

async function doRefresh(): Promise<void> {
  const config = getConfig()
  const tokens = readTokens()
  if (config === null) {
    throw new Error('OIDC no está configurado')
  }
  if (tokens === null || tokens.refresh_token === null) {
    throw new Error('No hay refresh_token disponible')
  }

  const endpoints = await discover(config.authority)
  const body = new URLSearchParams({
    grant_type: 'refresh_token',
    refresh_token: tokens.refresh_token,
    client_id: config.clientId,
  })
  const response = await fetch(endpoints.token_endpoint, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
      Accept: 'application/json',
    },
    body: body.toString(),
  })
  if (!response.ok) {
    clearTokens()
    throw new Error(`El refresco de token falló (${response.status})`)
  }

  let json: TokenResponse
  try {
    json = parseTokenResponse(await response.json())
  } catch (err) {
    // A malformed 200 must not leave a poisoned cache behind.
    clearTokens()
    throw err
  }
  storeTokenResponse(json, tokens.refresh_token)
}

/**
 * Renew the access token using the stored `refresh_token`.
 *
 * Concurrent calls are coalesced into a single network round-trip so several
 * simultaneous `401`s never trigger parallel refreshes.
 */
export function refresh(): Promise<void> {
  if (refreshPromise === null) {
    refreshPromise = doRefresh().finally(() => {
      refreshPromise = null
    })
  }
  return refreshPromise
}

/**
 * Resolve the bearer token for the next API request.
 *
 * In dev mode this degrades to the manual token / `VITE_DEV_ACCESS_TOKEN`.
 * In OIDC mode it returns the cached token and, when it is expired or close to
 * expiring, kicks off a best-effort background refresh. The authoritative,
 * awaited refresh happens reactively in {@link file://./api.ts} on `401`.
 */
export function getAccessToken(): string | null {
  if (!isOidcEnabled()) {
    return readTokens()?.access_token ?? readManualToken() ?? readDevToken()
  }

  const tokens = readTokens()
  if (tokens === null) {
    return null
  }

  const remaining = tokens.expires_at - Date.now()
  if (remaining <= 0) {
    void refresh().catch(() => {
      // The HTTP client handles the failure via its own 401 path.
    })
    return null
  }
  if (remaining < EXPIRY_SKEW_MS) {
    void refresh().catch(() => {})
  }
  return tokens.access_token
}

/** `true` when a usable token exists (dev mode always passes). */
export function isAuthenticated(): boolean {
  if (!isOidcEnabled()) {
    return true
  }
  const tokens = readTokens()
  return tokens !== null && tokens.expires_at > Date.now()
}

/**
 * `true` when a stored `refresh_token` is available to renew an expired
 * access token without a full login round-trip. Always `false` in dev mode.
 */
export function hasRefreshToken(): boolean {
  if (!isOidcEnabled()) {
    return false
  }
  const tokens = readTokens()
  return tokens !== null && tokens.refresh_token !== null
}

/**
 * Clear the local session and, in OIDC mode, redirect to Pocket ID's
 * end-session endpoint (best effort: never throws on discovery failure).
 */
export async function logout(): Promise<void> {
  clearTokens()
  const storage = safeSessionStorage()
  storage?.removeItem(VERIFIER_STORAGE_KEY)
  storage?.removeItem(STATE_STORAGE_KEY)
  storage?.removeItem(RETURN_TO_STORAGE_KEY)

  const config = getConfig()
  if (config === null) {
    return
  }
  try {
    const endpoints = await discover(config.authority)
    if (typeof endpoints.end_session_endpoint === 'string') {
      const url = new URL(endpoints.end_session_endpoint)
      url.searchParams.set('post_logout_redirect_uri', config.postLogoutRedirectUri)
      redirectFn(url.toString())
    }
  } catch {
    // Offline logout still clears the local session.
  }
}

/** Persist a manually-provisioned token (dev tooling / tests). */
export function setAccessToken(token: string): void {
  safeSessionStorage()?.setItem(ACCESS_TOKEN_STORAGE_KEY, token)
}

/** Forget the manually-provisioned token. */
export function clearAccessToken(): void {
  safeSessionStorage()?.removeItem(ACCESS_TOKEN_STORAGE_KEY)
}

/** Shape of the seam consumed by the HTTP client. */
export interface TokenProvider {
  getAccessToken(): string | null
}

/** Default {@link TokenProvider} backed by {@link getAccessToken}. */
export const tokenProvider: TokenProvider = {
  getAccessToken,
}

/**
 * Reset every module-level cache (tokens, discovery, in-flight refresh and the
 * navigation seam). Intended for tests.
 *
 * @internal
 */
export function __resetAuthForTests(): void {
  memoryTokens = null
  refreshPromise = null
  discoveryCache.clear()
  redirectFn = defaultRedirect
  safeSessionStorage()?.removeItem(RETURN_TO_STORAGE_KEY)
}
