/**
 * Typed access to the F13 app-token REST endpoints (`/api/v1/app-tokens`).
 *
 * App tokens back the DAV *Basic Auth* used by native clients (iOS, DAVx⁵,
 * Thunderbird). The clear-text secret is returned **only** by
 * {@link createAppToken}; every other call deals with secret-free metadata.
 *
 * Mirrors the shape of {@link file://./events.ts} and {@link file://./contacts.ts}:
 * functions take an optional `AbortSignal` and delegate the HTTP/JSON handling
 * (auth header, error envelope) to {@link file://./api.ts}.
 */

import type { AppToken, CreatedAppToken, CreateAppTokenInput, CurrentUser } from '../types/api'
import { api } from './api'

/** List the caller's app tokens (secret-free metadata, newest first). */
export function listAppTokens(signal?: AbortSignal): Promise<AppToken[]> {
  return api.get<AppToken[]>('/v1/app-tokens', signal)
}

/**
 * Create an app token.
 *
 * Returns the secret-free metadata **plus** the clear-text `token`
 * (`oxidav_<id>_<secret>`) — the single point in the whole API where the secret
 * is exposed. Callers must surface it once and never persist it.
 */
export function createAppToken(name: string, signal?: AbortSignal): Promise<CreatedAppToken> {
  const body: CreateAppTokenInput = { name }
  return api.post<CreatedAppToken>('/v1/app-tokens', body, signal)
}

/**
 * Rename one of the caller's app tokens.
 *
 * `PATCH /api/v1/app-tokens/{id}` returns the updated secret-free metadata
 * (`AppTokenMetadata`), mirroring the backend contract.
 */
export function renameAppToken(id: string, name: string, signal?: AbortSignal): Promise<AppToken> {
  const body: CreateAppTokenInput = { name }
  return api.patch<AppToken>(`/v1/app-tokens/${encodeURIComponent(id)}`, body, signal)
}

/** Revoke (permanently delete) one of the caller's app tokens. */
export function revokeAppToken(id: string, signal?: AbortSignal): Promise<void> {
  return api.delete(`/v1/app-tokens/${encodeURIComponent(id)}`, signal)
}

/**
 * Resolve the authenticated subject.
 *
 * `GET /api/v1/me` echoes the `sub` claim of the bearer token, which for Pocket
 * ID is the username used in the canonical DAV paths
 * (`/dav/calendars/{username}/`). Kept in this module so the Sync view has a
 * single, easily-mockable data dependency.
 */
export function getCurrentUser(signal?: AbortSignal): Promise<CurrentUser> {
  return api.get<CurrentUser>('/v1/me', signal)
}
