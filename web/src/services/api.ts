/**
 * Tiny typed `fetch` client for the OxiDav REST API.
 *
 * Since F17 the API authenticates **only** with the server-side session cookie,
 * so every request carries `credentials: "include"` and none sends an
 * `Authorization` header. The backend's uniform error envelope
 * (`{"error":{"code","message"}}`, F06) is mapped onto a thrown
 * {@link ApiError}, so the UI never has to render raw JSON.
 */

import type { ApiErrorEnvelope } from '../types/api'
import type { AuthUser } from '../contexts/AuthContext'

const envBase = import.meta.env.VITE_API_BASE_URL

/** Base URL of the JSON API. Defaults to the Vite dev proxy prefix `/api`. */
export const API_BASE_URL: string =
  typeof envBase === 'string' && envBase.length > 0 ? envBase : '/api'

/** Evento global que emite el cliente al recibir un 401 en una ruta no-auth. */
export const UNAUTHORIZED_EVENT = 'oxidav:unauthorized'

/**
 * Las rutas de autenticación (`/auth/*`) quedan exentas de la detección global
 * de 401: un 401 en `me`/`login`/`logout` es parte del propio flujo y no una
 * sesión caducada, así que no debe disparar el evento global.
 */
function isAuthPath(path: string): boolean {
  return path.startsWith('/auth/')
}

/** An error carrying the HTTP status and the backend error `code`. */
export class ApiError extends Error {
  readonly status: number
  readonly code: string

  constructor(status: number, code: string, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }
}

/** Narrows an arbitrary parsed body to the F06 error envelope. */
function isErrorEnvelope(value: unknown): value is ApiErrorEnvelope {
  if (typeof value !== 'object' || value === null || !('error' in value)) {
    return false
  }
  const { error } = value as { error: unknown }
  if (typeof error !== 'object' || error === null) {
    return false
  }
  const { code, message } = error as { code?: unknown; message?: unknown }
  return typeof code === 'string' && typeof message === 'string'
}

async function parseError(response: Response): Promise<ApiError> {
  let code = 'unknown'
  let message = `Error ${response.status}`
  try {
    const body: unknown = await response.json()
    if (isErrorEnvelope(body)) {
      code = body.error.code
      message = body.error.message
    }
  } catch {
    // A non-JSON error body keeps the generic status message.
  }
  return new ApiError(response.status, code, message)
}

export interface RequestOptions {
  method?: string
  body?: unknown
  signal?: AbortSignal
}

/** Build the request `Headers`. No `Authorization`: auth travels in the cookie. */
function buildHeaders(options: RequestOptions): Headers {
  const headers = new Headers({ Accept: 'application/json' })
  if (options.body !== undefined) {
    headers.set('Content-Type', 'application/json')
  }
  return headers
}

/** Perform one `fetch`, mapping network/abort failures onto an {@link ApiError}. */
async function send(path: string, options: RequestOptions): Promise<Response> {
  try {
    return await fetch(`${API_BASE_URL}${path}`, {
      method: options.method ?? 'GET',
      headers: buildHeaders(options),
      body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
      signal: options.signal,
      // La cookie de sesión `HttpOnly` solo viaja si se incluyen credenciales.
      credentials: 'include',
    })
  } catch {
    // Network failure / abort: surface a friendly, non-leaky message.
    throw new ApiError(0, 'network', 'No se pudo contactar con el servidor')
  }
}

/**
 * Perform a JSON request and return the parsed body (or `undefined` on 204).
 *
 * Auth policy (F17): there is no token to refresh. A `401` on a non-auth route
 * emits {@link UNAUTHORIZED_EVENT} so `AuthProvider` can drop the session;
 * a `401` on `/auth/*` is part of the flow and stays silent.
 */
export async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const response = await send(path, options)

  if (!response.ok) {
    if (response.status === 401 && !isAuthPath(path)) {
      window.dispatchEvent(new Event(UNAUTHORIZED_EVENT))
    }
    throw await parseError(response)
  }
  if (response.status === 204) {
    return undefined as T
  }
  return (await response.json()) as T
}

/** Convert any thrown value into a user-facing message. */
export function toErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    return error.message
  }
  if (error instanceof Error && error.message.length > 0) {
    return error.message
  }
  return 'Ha ocurrido un error inesperado'
}

/** Convenience wrappers for the HTTP verbs used by the app. */
export const api = {
  get: <T>(path: string, signal?: AbortSignal): Promise<T> => request<T>(path, { signal }),
  post: <T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> =>
    request<T>(path, { method: 'POST', body, signal }),
  patch: <T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> =>
    request<T>(path, { method: 'PATCH', body, signal }),
  delete: (path: string, signal?: AbortSignal): Promise<void> =>
    request<void>(path, { method: 'DELETE', signal }),

  // --- Autenticación (F17) -------------------------------------------------
  // La sesión se resuelve con la cookie, no con un token. `getMe` devuelve la
  // identidad o lanza un `401` (parte del flujo, no dispara el evento global).
  getMe: (signal?: AbortSignal): Promise<AuthUser> => request<AuthUser>('/auth/me', { signal }),
  // `end_session_url` es `null` cuando solo se cierra la sesión local.
  logout: (signal?: AbortSignal): Promise<{ end_session_url: string | null }> =>
    request<{ end_session_url: string | null }>('/auth/logout', { method: 'POST', signal }),
}
