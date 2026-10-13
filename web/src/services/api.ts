/**
 * Tiny typed `fetch` client for the OxiDav REST API.
 *
 * Every request carries `Authorization: Bearer <token>` (from the
 * {@link getAccessToken} seam) and maps the backend's uniform error envelope
 * (`{"error":{"code","message"}}`, F06) onto a thrown {@link ApiError}, so the
 * UI never has to render raw JSON.
 */

import type { ApiErrorEnvelope } from '../types/api'
import { getAccessToken, isOidcEnabled, logout, refresh } from './auth'

const envBase = import.meta.env.VITE_API_BASE_URL

/** Base URL of the JSON API. Defaults to the Vite dev proxy prefix `/api`. */
export const API_BASE_URL: string =
  typeof envBase === 'string' && envBase.length > 0 ? envBase : '/api'

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

/** Build the request `Headers`, injecting the current bearer token. */
function buildHeaders(options: RequestOptions): Headers {
  const headers = new Headers({ Accept: 'application/json' })
  const token = getAccessToken()
  if (token !== null) {
    headers.set('Authorization', `Bearer ${token}`)
  }
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
    })
  } catch {
    // Network failure / abort: surface a friendly, non-leaky message.
    throw new ApiError(0, 'network', 'No se pudo contactar con el servidor')
  }
}

/**
 * Perform a JSON request and return the parsed body (or `undefined` on 204).
 *
 * Auth policy (F14): a `401` triggers **one** attempt to {@link refresh} the
 * access token and then a single retry of the original request. Concurrent
 * requests share the refresh via the lock in `auth.ts`. If the refresh fails,
 * the local session is dropped and the error is surfaced — never a refresh loop.
 */
export async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  let response = await send(path, options)

  if (response.status === 401 && isOidcEnabled()) {
    try {
      await refresh()
    } catch (error) {
      void logout()
      throw error instanceof ApiError ? error : await parseError(response)
    }
    response = await send(path, options)
  }

  if (!response.ok) {
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
  get: <T>(path: string, signal?: AbortSignal): Promise<T> =>
    request<T>(path, { signal }),
  post: <T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> =>
    request<T>(path, { method: 'POST', body, signal }),
  patch: <T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> =>
    request<T>(path, { method: 'PATCH', body, signal }),
  delete: (path: string, signal?: AbortSignal): Promise<void> =>
    request<void>(path, { method: 'DELETE', signal }),
}
