/**
 * TypeScript mirror of the backend REST DTOs (`src/api/dto.rs`).
 *
 * Kept deliberately independent from the persistence models: the SPA only ever
 * sees these clean shapes and never `raw_payload` (the composed `.ics` for
 * events, `.vcf` for contacts). Timestamps are Unix epoch **milliseconds**
 * (`i64` on the Rust side).
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
 *
 * `dtstart`/`dtend` remain Unix epoch **milliseconds** (the single source of
 * truth for the instant). The optional `dtstart_tzid`/`dtend_tzid` are IANA
 * time-zone names (e.g. `"Europe/Madrid"`) the event is *anchored* to, so a
 * recurrence repeats in local wall-clock time instead of drifting across a DST
 * change. `null` means the event carries no zone (the pre-F18 behaviour).
 */
export interface CalendarEvent {
  id: string
  uid: string
  collection_id: string
  summary: string | null
  description: string | null
  /** `LOCATION`, verbatim, or `null` when the event carries none (F24). */
  location: string | null
  /** `URL`, verbatim, or `null` when the event carries none (F24). */
  url: string | null
  dtstart: number | null
  dtend: number | null
  /** IANA time zone anchoring `dtstart`, or `null` when the event has none. */
  dtstart_tzid: string | null
  /** IANA time zone anchoring `dtend`, or `null` when the event has none. */
  dtend_tzid: string | null
  rrule: string | null
  etag: string
  created_at: number
  updated_at: number
}

/**
 * Full, typed projection of an event (`GET .../events/{uid}`, F25).
 *
 * Mirror of the backend `EventDetailDto`: a **superset** of {@link CalendarEvent}
 * that keeps every field the listing exposes (same names and types) and adds the
 * six properties F25 models — `CATEGORIES`, `ATTACH`, `STATUS`, `CLASS`, `TRANSP`
 * and `DURATION` — derived at read time from the stored `.ics`. The client never
 * parses iCalendar itself; these shapes are already-parsed server fields.
 *
 * `dtend` is **not** derived from `duration`: an event carrying only a `DURATION`
 * reports `dtend: null` and its `duration` verbatim.
 */
export interface EventDetail extends CalendarEvent {
  /** `CATEGORIES`, flattened to a single list (one entry per item, in order). */
  categories: string[]
  /** `ATTACH` URIs, one entry per occurrence. */
  attach: string[]
  /** `STATUS` (`TENTATIVE`/`CONFIRMED`/`CANCELLED`), or `null`. */
  status: string | null
  /** `CLASS` (`PUBLIC`/`PRIVATE`/`CONFIDENTIAL` or a client token), or `null`. */
  class: string | null
  /** `TRANSP` (`OPAQUE`/`TRANSPARENT`), or `null`. */
  transp: string | null
  /** `DURATION`, verbatim (an opaque RFC 5545 duration), or `null`. */
  duration: string | null
}

/** Request body for creating a calendar event (`POST .../events`). */
export interface CreateEventInput {
  summary: string
  description?: string
  /** `LOCATION` (free text); omit for none. */
  location?: string
  /**
   * `URL`; must be an absolute `http://`/`https://` URI. The server allowlists
   * the scheme (security boundary: the SPA renders the value as a link).
   */
  url?: string
  dtstart: number
  dtend?: number
  rrule?: string
  uid?: string
  /** IANA time zone to anchor the event to (e.g. `"Europe/Madrid"`); omit for none. */
  tzid?: string
  /** `STATUS` (`TENTATIVE`/`CONFIRMED`/`CANCELLED`); omit for none. */
  status?: string
  /** `CLASS` (`PUBLIC`/`PRIVATE`/`CONFIDENTIAL`); omit for none. */
  class?: string
  /** `TRANSP` (`OPAQUE`/`TRANSPARENT`); omit for none. */
  transp?: string
  /** `DURATION`, verbatim (an RFC 5545 duration); exclusive with `dtend`. */
  duration?: string
  /** `CATEGORIES` items; omit/empty for none. */
  categories?: string[]
  /** `ATTACH` URIs, one line per value; omit/empty for none. */
  attach?: string[]
}

/** Request body for a partial update of a calendar event (`PATCH .../events/{uid}`). */
export interface UpdateEventInput {
  summary?: string
  /** An empty string (`""`) *clears* the description. Omit to leave it untouched. */
  description?: string
  /**
   * `LOCATION`; an empty string (`""`) *clears* the field (the server normalises
   * it to `None` and the `LOCATION` line disappears from the stored `.ics`).
   * Omit to leave it untouched.
   */
  location?: string
  /**
   * `URL`; an empty string (`""`) *clears* the field. Otherwise it must be an
   * absolute `http://`/`https://` URI — the server allowlists the scheme, both
   * as a storage-safety boundary and to keep the SPA's rendered link safe.
   * Omit to leave it untouched.
   */
  url?: string
  dtstart?: number
  dtend?: number
  rrule?: string
  /**
   * IANA time zone to anchor the event to, e.g. `"Europe/Madrid"`. On a
   * `PATCH`, an absent `tzid` means "keep the current zone" — the zone cannot be
   * removed with a patch. To preserve an existing anchor, the client must send
   * it back explicitly (the server does not re-derive it from `dtstart`).
   */
  tzid?: string
  /** `STATUS`; an empty string (`""`) *clears* it. Omit to leave it untouched. */
  status?: string
  /** `CLASS`; an empty string (`""`) *clears* it. Omit to leave it untouched. */
  class?: string
  /** `TRANSP`; an empty string (`""`) *clears* it. Omit to leave it untouched. */
  transp?: string
  /**
   * `DURATION`; an empty string (`""`) *clears* it (leaving `DTEND` untouched).
   * A non-empty value sets it and removes `DTEND` (they are exclusive). Omit to
   * leave it untouched.
   */
  duration?: string
  /** `CATEGORIES`; `[]` *clears* them. Omit to leave them untouched. */
  categories?: string[]
  /** `ATTACH` URIs; `[]` *clears* them. Omit to leave them untouched. */
  attach?: string[]
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

/**
 * Full, typed projection of a contact (`GET .../contacts/{uid}`, F22).
 *
 * Mirror of the backend `ContactDetailDto`: it keeps the light fields the
 * listing exposes and adds every vCard group, derived from the stored payload.
 * It is **not** a structural superset of {@link Contact} — the server drops the
 * denormalised `email`/`phone`/`organization` columns here (they are redundant
 * with {@link ContactDetail.emails}/{@link ContactDetail.tels}). The client
 * never parses vCard itself; these shapes are already-parsed server fields.
 */
export interface ContactDetail {
  id: string
  uid: string
  collection_id: string
  /** Formatted name (`FN`); the canonical field. */
  fn: string | null
  /** Alias of `fn`; always mirrors it (never `null` on the detail). */
  full_name: string | null
  etag: string
  created_at: number
  updated_at: number
  /** `VERSION`, verbatim (`null` when absent). */
  version: string | null
  /** `REV`, verbatim. */
  rev: string | null
  /** Structured `N`. */
  name: StructuredName | null
  /** `NICKNAME` occurrences. */
  nicknames: string[]
  /** `PHOTO` (external URI or embedded `data:` URI). */
  photos: Media[]
  /** `BDAY`, verbatim. */
  birthday: string | null
  /** `ANNIVERSARY`, verbatim. */
  anniversary: string | null
  /** `GENDER`. */
  gender: Gender | null
  /** `TEL` occurrences with their `TYPE`/`PREF`. */
  tels: LabelledValue[]
  /** `EMAIL` occurrences with their `TYPE`/`PREF`. */
  emails: LabelledValue[]
  /** `IMPP` occurrences with their `TYPE`/`PREF`. */
  impps: LabelledValue[]
  /** `ADR` occurrences. */
  addresses: Address[]
  /** `GEO` occurrences. */
  geos: string[]
  /** `TZ` occurrences. */
  timezones: string[]
  /** `TITLE` occurrences. */
  titles: string[]
  /** `ROLE` occurrences. */
  roles: string[]
  /** `ORG` occurrences, one entry per line. */
  organizations: string[][]
  /** `LOGO` (external URI or embedded `data:` URI). */
  logos: Media[]
  /** `NOTE` occurrences. */
  notes: string[]
  /** `CATEGORIES` occurrences, one entry per line. */
  categories: string[][]
  /** `URL` occurrences. */
  urls: string[]
  /** `SOUND` (external URI or embedded `data:` URI). */
  sounds: Media[]
  /** Everything outside the modelled groups (`X-*` and unmapped RFC properties). */
  others: RawProperty[]
}

/** The five components of a structured `N`. */
export interface StructuredName {
  family: string | null
  given: string | null
  /** Additional names (may hold several). */
  additional: string[]
  /** Honorific prefixes. */
  prefix: string[]
  /** Honorific suffixes. */
  suffix: string[]
}

/** `GENDER`: a sex code (`M`/`F`/`O`/`N`/`U`) plus an optional identity text. */
export interface Gender {
  sex: string | null
  identity: string | null
}

/** A single value with its `TYPE` parameters and `PREF` (lower is preferred). */
export interface LabelledValue {
  value: string
  types: string[]
  pref: number | null
}

/** A structured `ADR` with its seven components, `LABEL` and parameters. */
export interface Address {
  po_box: string | null
  extended: string | null
  street: string | null
  locality: string | null
  region: string | null
  postal_code: string | null
  country: string | null
  /** The `LABEL` parameter, when present. */
  label: string | null
  types: string[]
  pref: number | null
}

/**
 * `PHOTO`/`LOGO`/`SOUND`: a discriminated union on `kind`.
 *
 * The backend omits the fields not belonging to the active variant
 * (`skip_serializing_if`), so a `"data"` value carries no `uri` and a `"uri"`
 * value carries neither `media_type` nor `data_uri`. Branch on `kind` alone.
 */
export type Media =
  | { kind: 'uri'; uri: string }
  | { kind: 'data'; media_type?: string; data_uri: string }

/** A property outside the modelled groups, kept as written. */
export interface RawProperty {
  /** The property name (`X-ABLabel`, `PRODID`, …). */
  name: string
  /** The group prefix (`item1` in `item1.X-ABLabel`), when present. */
  group: string | null
  /** The property parameters as `[name, value]` pairs. */
  parameters: [string, string][]
  /** The property value. */
  value: string
}

/**
 * Request body for creating a contact (`POST .../contacts`).
 *
 * The four legacy scalars (`fn`/`email`/`phone`/`organization`) keep their
 * existing semantics; the F22 fields are additive and each optional. Binary
 * media (`photos`/`logos`/`sounds`) is **not** editable and never sent.
 */
export interface CreateContactInput {
  fn: string
  email?: string
  phone?: string
  organization?: string
  /** Optional client-chosen `UID`; a fresh one is minted when omitted. */
  uid?: string
  /** Structured `N`. */
  name?: StructuredName
  /** `NICKNAME` values. */
  nicknames?: string[]
  /** `BDAY`, as written. */
  birthday?: string
  /** `ANNIVERSARY`, as written. */
  anniversary?: string
  /** `GENDER`. */
  gender?: Gender
  /** `TEL` values. */
  tels?: LabelledValue[]
  /** `EMAIL` values. */
  emails?: LabelledValue[]
  /** `IMPP` values. */
  impps?: LabelledValue[]
  /** `ADR` values. */
  addresses?: Address[]
  /** `GEO` values. */
  geos?: string[]
  /** `TZ` values. */
  timezones?: string[]
  /** `TITLE` values. */
  titles?: string[]
  /** `ROLE` values. */
  roles?: string[]
  /** `ORG` values, one entry per line. */
  organizations?: string[][]
  /** `NOTE` values. */
  notes?: string[]
  /** `CATEGORIES` values, one entry per line. */
  categories?: string[][]
  /** `URL` values. */
  urls?: string[]
}

/**
 * Request body for a partial update of a contact (`PATCH .../contacts/{uid}`).
 *
 * Presence means replacement: a field present substitutes **all** of its
 * occurrences; an absent field is left untouched; an empty array (`[]`)
 * deletes the property. `uid` is immutable and deliberately not exposed here.
 */
export interface UpdateContactInput {
  fn?: string
  /**
   * Legacy scalar `EMAIL`. The form never uses it: mixing it with {@link emails}
   * is a `400` (the server rejects both at once), so prefer the array forms.
   */
  email?: string
  /** Legacy scalar `TEL`. The form never uses it; see {@link email}. */
  phone?: string
  /** Legacy scalar `ORG`. The form never uses it; see {@link email}. */
  organization?: string
  /** Structured `N`. */
  name?: StructuredName
  /** `NICKNAME` values (`[]` deletes). */
  nicknames?: string[]
  /** `BDAY`. */
  birthday?: string
  /** `ANNIVERSARY`. */
  anniversary?: string
  /** `GENDER`. */
  gender?: Gender
  /** `TEL` values (`[]` deletes). */
  tels?: LabelledValue[]
  /** `EMAIL` values (`[]` deletes). */
  emails?: LabelledValue[]
  /** `IMPP` values (`[]` deletes). */
  impps?: LabelledValue[]
  /** `ADR` values (`[]` deletes). */
  addresses?: Address[]
  /** `GEO` values (`[]` deletes). */
  geos?: string[]
  /** `TZ` values (`[]` deletes). */
  timezones?: string[]
  /** `TITLE` values (`[]` deletes). */
  titles?: string[]
  /** `ROLE` values (`[]` deletes). */
  roles?: string[]
  /** `ORG` values (`[]` deletes). */
  organizations?: string[][]
  /** `NOTE` values (`[]` deletes). */
  notes?: string[]
  /** `CATEGORIES` values (`[]` deletes). */
  categories?: string[][]
  /** `URL` values (`[]` deletes). */
  urls?: string[]
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
