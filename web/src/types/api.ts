/**
 * TypeScript mirror of the backend REST DTOs (`src/api/dto.rs`).
 *
 * Kept deliberately independent from the persistence models: the SPA only ever
 * sees these clean shapes and never `raw_payload` (the composed `.ics`).
 * Timestamps are Unix epoch **milliseconds** (`i64` on the Rust side).
 */

/** Kind of a collection, as exposed over the API. */
export type CollectionType = 'calendar' | 'addressbook'

/** Public representation of a collection. */
export interface Collection {
  id: string
  name: string
  type: CollectionType
  color: string | null
  sync_token: number
  created_at: number
  updated_at: number
}

/** A page of results with its pagination metadata. */
export interface Page<T> {
  items: T[]
  total: number
  limit: number
  offset: number
}

/**
 * Public representation of a calendar event (`calendar_objects` row).
 *
 * `summary` / `dtstart` etc. are nullable because the column may be absent;
 * `uid` is unique within a collection.
 */
export interface CalendarEvent {
  id: string
  uid: string
  collection_id: string
  summary: string | null
  description: string | null
  dtstart: number | null
  dtend: number | null
  rrule: string | null
  etag: string
  created_at: number
  updated_at: number
}

/** Request body for creating a calendar event (`POST .../events`). */
export interface CreateEventInput {
  summary: string
  description?: string
  dtstart: number
  dtend?: number
  rrule?: string
  uid?: string
}

/** Request body for a partial update of a calendar event (`PATCH .../events/{uid}`). */
export interface UpdateEventInput {
  summary?: string
  description?: string
  dtstart?: number
  dtend?: number
  rrule?: string
}

/**
 * Public representation of a card (contact) object (`card_objects` row).
 *
 * Like {@link CalendarEvent}, `raw_payload` (the composed `.vcf`) is never
 * exposed: the SPA consumes the already-parsed fields. `fn` is the formatted
 * name (`FN`); `full_name` is accepted by the API as an alias of `fn`.
 */
export interface Contact {
  id: string
  uid: string
  collection_id: string
  fn: string | null
  /** Alias of `fn` accepted by the API. */
  full_name?: string | null
  email: string | null
  phone: string | null
  organization: string | null
  etag: string
  created_at: number
  updated_at: number
}

/** Request body for creating a contact (`POST .../contacts`). */
export interface CreateContactInput {
  fn: string
  email?: string
  phone?: string
  organization?: string
  /** Optional client-chosen `UID`; a fresh one is minted when omitted. */
  uid?: string
}

/** Request body for a partial update of a contact (`PATCH .../contacts/{uid}`). */
export interface UpdateContactInput {
  fn?: string
  email?: string
  phone?: string
  organization?: string
}

/**
 * Public metadata of an app token (`GET /api/v1/app-tokens`).
 *
 * Deliberately excludes the secret and its hash: the clear-text token is only
 * ever returned by the creation endpoint (see {@link CreatedAppToken}).
 * Unlike the calendar/contact DTOs, the timestamps here are **ISO-8601 strings**
 * (not epoch milliseconds), mirroring the backend `AppTokenMetadata`.
 */
export interface AppToken {
  id: string
  name: string
  created_at: string
  last_used_at: string | null
}

/** Request body for creating an app token (`POST /api/v1/app-tokens`). */
export interface CreateAppTokenInput {
  name: string
}

/**
 * Response of `POST /api/v1/app-tokens`: the only place the clear-text token
 * (`oxidav_<id>_<secret>`) is ever exposed. Shown to the user exactly once.
 */
export interface CreatedAppToken {
  id: string
  name: string
  token: string
  created_at: string
}

/** Response of `GET /api/v1/me`: the authenticated subject (`sub` claim). */
export interface CurrentUser {
  sub: string
}

/** Uniform backend error envelope: `{"error":{"code","message"}}`. */
export interface ApiErrorEnvelope {
  error: {
    code: string
    message: string
  }
}
