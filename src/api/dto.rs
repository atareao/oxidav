//! Public JSON DTOs for the REST API.
//!
//! These types are deliberately separate from the persistence models
//! ([`crate::db::models`]): the client sees a clean contract and never the raw
//! `raw_payload` / `secret_hash` internals. The model → DTO conversion is
//! explicit ([`CollectionDto::from`]), which isolates the HTTP contract from
//! schema changes.
//!
//! Timestamps are Unix epoch **milliseconds** (as stored in SQLite), exposed as
//! `i64`. No date-formatting dependency is introduced in the MVP.

use serde::{Deserialize, Serialize};

use crate::auth::AppToken;
use crate::dav::ics::EventProjection;
use crate::dav::vcf::{Address as CardAddress, CardProjection, Gender as CardGender};
use crate::dav::vcf::{MediaValue, MultiValue, RawProperty, StructuredName};
use crate::db::models::{CalendarObject, CardObject, Collection};

/// Public representation of a collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionDto {
    /// UUID (`collections.id` is `TEXT PRIMARY KEY`).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Calendar or address book.
    pub r#type: CollectionType,
    /// UI accent colour.
    pub color: Option<String>,
    /// Monotonic RFC 6578 change token.
    pub sync_token: i64,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
    /// Last modification time, Unix milliseconds.
    pub updated_at: i64,
}

/// Kind of a collection, as exposed over the API (`"calendar"` |
/// `"addressbook"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectionType {
    /// A CalDAV calendar.
    Calendar,
    /// A CardDAV address book.
    Addressbook,
}

impl CollectionType {
    /// Map the string stored in `collections.type` to the API enum.
    ///
    /// The schema's `CHECK` constraint restricts the column to the two known
    /// values, so anything else is treated defensively as a calendar.
    fn from_kind(kind: &str) -> Self {
        match kind {
            "addressbook" => Self::Addressbook,
            _ => Self::Calendar,
        }
    }
}

impl From<&Collection> for CollectionDto {
    fn from(collection: &Collection) -> Self {
        Self {
            id: collection.id.clone(),
            name: collection.name.clone(),
            r#type: CollectionType::from_kind(&collection.kind),
            color: Some(collection.color.clone()),
            sync_token: collection.sync_token,
            created_at: collection.created_at,
            updated_at: collection.updated_at,
        }
    }
}

/// Request body for creating a collection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreateCollectionRequest {
    /// Human-readable name.
    pub name: String,
    /// Calendar or address book.
    pub r#type: CollectionType,
    /// Optional UI accent colour.
    pub color: Option<String>,
}

/// Request body for a partial update of a collection.
///
/// Only `name` and `color` are mutable; omitted (`null`) fields are left
/// untouched. A `type` discriminant is **not** accepted on update — a collection
/// cannot switch between calendar and address book — so the field is decoded
/// explicitly only to be rejected with a `400` (rather than silently ignored).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UpdateCollectionRequest {
    /// New name, if provided.
    pub name: Option<String>,
    /// New colour, if provided.
    pub color: Option<String>,
    /// Immutable type; its presence is rejected by the handler.
    #[serde(default)]
    pub r#type: Option<CollectionType>,
}

/// Public representation of a calendar event (a `calendar_objects` row).
///
/// Deliberately excludes `raw_payload` (the composed `.ics`): the SPA consumes
/// the already-parsed fields and never parses iCalendar itself. `dtstart` /
/// `dtend` are Unix epoch **milliseconds**, mirroring the storage and the search
/// DTOs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDto {
    /// Internal calendar-object id (UUID).
    pub id: String,
    /// iCalendar `UID`, unique within the collection.
    pub uid: String,
    /// Owning collection.
    pub collection_id: String,
    /// Event title (`SUMMARY`).
    pub summary: Option<String>,
    /// Event body (`DESCRIPTION`).
    pub description: Option<String>,
    /// Event place (`LOCATION`), if any (F24). Free text; `null` means "no
    /// `LOCATION`".
    pub location: Option<String>,
    /// Event link (`URL`), if any (F24). An absolute `http`/`https` URI; `null`
    /// means "no `URL`".
    pub url: Option<String>,
    /// Start instant, Unix milliseconds.
    pub dtstart: Option<i64>,
    /// End instant, Unix milliseconds.
    pub dtend: Option<i64>,
    /// IANA time-zone the event's `DTSTART` is anchored to (F18), if any.
    ///
    /// `null` means the start is UTC (the pre-F18 behaviour). This is the zone
    /// the `.ics` is composed with (`DTSTART;TZID=<zone>` + a `VTIMEZONE`
    /// block); the `dtstart` instant itself is always UTC milliseconds.
    pub dtstart_tzid: Option<String>,
    /// IANA time-zone the event's `DTEND` is anchored to (F18), if any.
    ///
    /// A DAV client may anchor `DTSTART` and `DTEND` to different zones, so this
    /// is exposed independently of [`dtstart_tzid`](Self::dtstart_tzid).
    pub dtend_tzid: Option<String>,
    /// Raw `RRULE` text (never expanded).
    pub rrule: Option<String>,
    /// Opaque ETag.
    pub etag: String,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
    /// Last modification time, Unix milliseconds.
    pub updated_at: i64,
}

impl From<&CalendarObject> for EventDto {
    fn from(object: &CalendarObject) -> Self {
        Self {
            id: object.id.clone(),
            uid: object.uid.clone(),
            collection_id: object.collection_id.clone(),
            summary: object.summary.clone(),
            description: object.description.clone(),
            location: object.location.clone(),
            url: object.url.clone(),
            dtstart: object.dtstart,
            dtend: object.dtend,
            dtstart_tzid: object.dtstart_tzid.clone(),
            dtend_tzid: object.dtend_tzid.clone(),
            rrule: object.rrule.clone(),
            etag: object.etag.clone(),
            created_at: object.created_at,
            updated_at: object.updated_at,
        }
    }
}

// ---------------------------------------------------------------------------
// F25 — full-`VEVENT` detail DTO (PLAN-029)
// ---------------------------------------------------------------------------

/// The full projection of an event as returned by
/// `GET .../events/{uid}` (and echoed by `POST`/`PATCH`).
///
/// A **superset** of [`EventDto`]: it keeps every field the list exposes (same
/// names and types) and adds the six properties F25 models — `CATEGORIES`,
/// `ATTACH`, `STATUS`, `CLASS`, `TRANSP` and `DURATION` — derived at read time
/// from `raw_payload` by [`project_event`](crate::dav::ics::project_event), the
/// single source of truth. It deliberately still excludes `raw_payload`.
///
/// `dtend` is **not** derived from `DURATION`: an event that carries only a
/// `DURATION` reports `dtend: null` and its `duration`, so the SPA never
/// re-sends a computed end that would turn the duration into an explicit end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventDetailDto {
    /// Internal calendar-object id (UUID).
    pub id: String,
    /// iCalendar `UID`, unique within the collection.
    pub uid: String,
    /// Owning collection.
    pub collection_id: String,
    /// Event title (`SUMMARY`).
    pub summary: Option<String>,
    /// Event body (`DESCRIPTION`).
    pub description: Option<String>,
    /// Event place (`LOCATION`), if any (F24).
    pub location: Option<String>,
    /// Event link (`URL`), if any (F24).
    pub url: Option<String>,
    /// Start instant, Unix milliseconds.
    pub dtstart: Option<i64>,
    /// End instant, Unix milliseconds. `null` when the event carries only a
    /// `DURATION` (never derived).
    pub dtend: Option<i64>,
    /// IANA time-zone the event's `DTSTART` is anchored to (F18), if any.
    pub dtstart_tzid: Option<String>,
    /// IANA time-zone the event's `DTEND` is anchored to (F18), if any.
    pub dtend_tzid: Option<String>,
    /// Raw `RRULE` text (never expanded).
    pub rrule: Option<String>,
    /// Opaque ETag.
    pub etag: String,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
    /// Last modification time, Unix milliseconds.
    pub updated_at: i64,
    /// `CATEGORIES`, flattened to a single list (one entry per item, in order).
    /// Empty means "no categories".
    pub categories: Vec<String>,
    /// `ATTACH` URIs, one entry per occurrence (parameters dropped).
    pub attach: Vec<String>,
    /// `STATUS` (`TENTATIVE`/`CONFIRMED`/`CANCELLED`), verbatim.
    pub status: Option<String>,
    /// `CLASS` (`PUBLIC`/`PRIVATE`/`CONFIDENTIAL` or a client token), verbatim.
    /// Exposed as `class` (`class` is a Rust keyword).
    #[serde(rename = "class")]
    pub class_: Option<String>,
    /// `TRANSP` (`OPAQUE`/`TRANSPARENT`), verbatim.
    pub transp: Option<String>,
    /// `DURATION`, verbatim (an opaque RFC 5545 duration).
    pub duration: Option<String>,
    /// Instantes (ms Unix) de las líneas `EXDATE` del subconjunto soportado,
    /// ordenados ascendente y sin duplicados (F27). Vacío también cuando la
    /// propiedad es opaca o ausente.
    pub exdate: Vec<i64>,
    /// Texto **verbatim** de las líneas `EXDATE` cuando la propiedad **no**
    /// encaja en el subconjunto soportado; `null` si es modelable o ausente
    /// (F27).
    pub exdate_opaque: Option<String>,
    /// Instantes (ms Unix) de las líneas `RDATE` del subconjunto soportado,
    /// ordenados y sin duplicados; vacío también si es opaca o ausente (F27).
    pub rdate: Vec<i64>,
    /// Texto **verbatim** de las líneas `RDATE` cuando la propiedad es opaca;
    /// `null` si es modelable o ausente (F27).
    pub rdate_opaque: Option<String>,
}

impl EventDetailDto {
    /// Build the full detail DTO from the stored row and its projection.
    ///
    /// The projection is the source of truth for the event content (it is derived
    /// from `raw_payload`); the row supplies the identity, the opaque `etag` and
    /// the timestamps.
    #[must_use]
    pub fn new(object: &CalendarObject, projection: EventProjection) -> Self {
        Self {
            id: object.id.clone(),
            uid: object.uid.clone(),
            collection_id: object.collection_id.clone(),
            summary: projection.summary,
            description: projection.description,
            location: projection.location,
            url: projection.url,
            dtstart: Some(projection.dtstart_ms),
            dtend: projection.dtend_ms,
            dtstart_tzid: projection.dtstart_tzid,
            dtend_tzid: projection.dtend_tzid,
            rrule: projection.rrule,
            etag: object.etag.clone(),
            created_at: object.created_at,
            updated_at: object.updated_at,
            categories: projection.categories.into_iter().flatten().collect(),
            attach: projection.attach,
            status: projection.status,
            class_: projection.class,
            transp: projection.transp,
            duration: projection.duration,
            exdate: projection.exdate,
            exdate_opaque: projection.exdate_opaque,
            rdate: projection.rdate,
            rdate_opaque: projection.rdate_opaque,
        }
    }
}

/// Request body for creating a calendar event.
///
/// The backend composes the `.ics` stored in `raw_payload` from these fields
/// (the client never sends iCalendar). `summary` and `dtstart` are mandatory;
/// the rest are optional. `uid` is normally minted by the server, but a client
/// may pin one — a subsequent duplicate then conflicts (`409`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreateEventRequest {
    /// Event title (`SUMMARY`); required and non-blank.
    pub summary: String,
    /// Event body (`DESCRIPTION`).
    #[serde(default)]
    pub description: Option<String>,
    /// Event place (`LOCATION`); optional free text.
    #[serde(default)]
    pub location: Option<String>,
    /// Event link (`URL`); optional, must be an absolute `http`/`https` URI.
    #[serde(default)]
    pub url: Option<String>,
    /// Start instant, Unix milliseconds; required.
    pub dtstart: i64,
    /// End instant, Unix milliseconds; must be `>= dtstart` when present.
    #[serde(default)]
    pub dtend: Option<i64>,
    /// Raw `RRULE` text (never expanded).
    #[serde(default)]
    pub rrule: Option<String>,
    /// Optional IANA time-zone name the event is anchored to (e.g.
    /// `"Europe/Madrid"`).
    ///
    /// When present, the stored `.ics` is composed as
    /// `DTSTART;TZID=<zone>` / `DTEND;TZID=<zone>` plus a `VTIMEZONE` block, so
    /// an `RRULE` recurs in **local** wall-clock time. `dtstart`/`dtend` remain
    /// the UTC instants and are not shifted. An unknown zone is a `400`.
    #[serde(default)]
    pub tzid: Option<String>,
    /// Optional client-chosen `UID`; a fresh one is minted when omitted.
    #[serde(default)]
    pub uid: Option<String>,
    /// `STATUS` (`TENTATIVE`/`CONFIRMED`/`CANCELLED`); normalised to upper case.
    #[serde(default)]
    pub status: Option<String>,
    /// `CLASS` (`PUBLIC`/`PRIVATE`/`CONFIDENTIAL`); normalised to upper case.
    #[serde(default, rename = "class")]
    pub class_: Option<String>,
    /// `TRANSP` (`OPAQUE`/`TRANSPARENT`); normalised to upper case.
    #[serde(default)]
    pub transp: Option<String>,
    /// `DURATION`, verbatim (an RFC 5545 `dur-value`); mutually exclusive with
    /// `dtend`.
    #[serde(default)]
    pub duration: Option<String>,
    /// `CATEGORIES` items; emitted as a single joined line.
    #[serde(default)]
    pub categories: Option<Vec<String>>,
    /// `ATTACH` URIs, one content line per value (each an `http`/`https` URI).
    #[serde(default)]
    pub attach: Option<Vec<String>>,
    /// `EXDATE` instantes (ms Unix) excluidos de la recurrencia (F27). La lista
    /// vacía significa "sin la propiedad"; se emite como una única línea
    /// `EXDATE`, ordenada y sin duplicados.
    #[serde(default)]
    pub exdate: Option<Vec<i64>>,
    /// `RDATE` instantes (ms Unix) añadidos a la recurrencia (F27). Misma
    /// semántica que `exdate`.
    #[serde(default)]
    pub rdate: Option<Vec<i64>>,
}

/// Request body for a partial update of a calendar event.
///
/// Only the fields present are applied; omitted fields keep their stored value.
/// Note that a field cannot be cleared to `null` (the DTO cannot distinguish
/// "absent" from "null"): a documented MVP limitation. A blank text value
/// (`summary` aside) clears the field. As of F24 the stored `.ics` in
/// `raw_payload` is **reconciled** surgically from the merged fields — every
/// property the model does not know (`VALARM`, `ATTENDEE`, `X-*`…) is preserved
/// byte for byte.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct UpdateEventRequest {
    /// New title, if provided.
    #[serde(default)]
    pub summary: Option<String>,
    /// New body, if provided.
    #[serde(default)]
    pub description: Option<String>,
    /// New place (`LOCATION`), if provided. A blank value clears the field.
    #[serde(default)]
    pub location: Option<String>,
    /// New link (`URL`), if provided (must be an absolute `http`/`https` URI). A
    /// blank value clears the field.
    #[serde(default)]
    pub url: Option<String>,
    /// New start instant, if provided.
    #[serde(default)]
    pub dtstart: Option<i64>,
    /// New end instant, if provided.
    #[serde(default)]
    pub dtend: Option<i64>,
    /// New raw `RRULE` text, if provided.
    #[serde(default)]
    pub rrule: Option<String>,
    /// New IANA time-zone anchor, if provided (must resolve to a known zone).
    ///
    /// A single `tzid` is applied to **both** `DTSTART` and `DTEND`. An absent
    /// `tzid` means "leave the zone unchanged" — there is deliberately **no**
    /// way to clear the zone through a `PATCH` (the DTO cannot distinguish
    /// "absent" from "null", the same MVP limitation as every other field).
    #[serde(default)]
    pub tzid: Option<String>,
    /// New `STATUS`, if provided (a blank value clears the field).
    #[serde(default)]
    pub status: Option<String>,
    /// New `CLASS`, if provided (a blank value clears the field).
    #[serde(default, rename = "class")]
    pub class_: Option<String>,
    /// New `TRANSP`, if provided (a blank value clears the field).
    #[serde(default)]
    pub transp: Option<String>,
    /// New `DURATION`, if provided. A blank value clears it (leaving `DTEND`
    /// untouched); a value sets it and removes `DTEND` (they are exclusive).
    #[serde(default)]
    pub duration: Option<String>,
    /// New `CATEGORIES`, if provided (`[]` clears them).
    #[serde(default)]
    pub categories: Option<Vec<String>>,
    /// New `ATTACH` URIs, if provided (`[]` clears them).
    #[serde(default)]
    pub attach: Option<Vec<String>>,
    /// New `EXDATE` instantes (ms Unix), if provided (F27). `Some` **reemplaza**
    /// el conjunto entero; `Some(vec![])` **limpia** la propiedad —también una
    /// propiedad opaca—; ausente (`None`) = no cambiar.
    #[serde(default)]
    pub exdate: Option<Vec<i64>>,
    /// New `RDATE` instantes (ms Unix), if provided (F27). Misma semántica que
    /// `exdate`.
    #[serde(default)]
    pub rdate: Option<Vec<i64>>,
}

impl UpdateEventRequest {
    /// Whether the patch carries no updatable field at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.summary.is_none()
            && self.description.is_none()
            && self.location.is_none()
            && self.url.is_none()
            && self.dtstart.is_none()
            && self.dtend.is_none()
            && self.rrule.is_none()
            && self.tzid.is_none()
            && self.status.is_none()
            && self.class_.is_none()
            && self.transp.is_none()
            && self.duration.is_none()
            && self.categories.is_none()
            && self.attach.is_none()
            && self.exdate.is_none()
            && self.rdate.is_none()
    }
}

/// A page of results with its pagination metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Page<T> {
    /// The page's items.
    pub items: Vec<T>,
    /// Total number of items across all pages.
    pub total: i64,
    /// Effective (clamped) page size.
    pub limit: i64,
    /// Effective (clamped) page offset.
    pub offset: i64,
}

/// `GET /api/v1/me` response: the authenticated subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeDto {
    /// The `sub` claim carried by the verified session cookie.
    pub sub: String,
}

/// One calendar-object search hit (`GET /api/v1/search/events`).
///
/// Deliberately excludes `raw_payload` (the raw `.ics`): the SPA consumes the
/// already-parsed fields and never parses iCalendar itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSearchResult {
    /// Internal calendar-object id.
    pub id: String,
    /// iCalendar `UID`.
    pub uid: String,
    /// Owning collection.
    pub collection_id: String,
    /// Event title (`SUMMARY`).
    pub summary: Option<String>,
    /// Start instant, Unix milliseconds.
    pub dtstart: Option<i64>,
    /// End instant, Unix milliseconds.
    pub dtend: Option<i64>,
}

impl From<&CalendarObject> for EventSearchResult {
    fn from(object: &CalendarObject) -> Self {
        Self {
            id: object.id.clone(),
            uid: object.uid.clone(),
            collection_id: object.collection_id.clone(),
            summary: object.summary.clone(),
            dtstart: object.dtstart,
            dtend: object.dtend,
        }
    }
}

/// One card-object search hit (`GET /api/v1/search/contacts`).
///
/// Deliberately excludes `raw_payload` (the raw `.vcf`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactSearchResult {
    /// Internal card-object id.
    pub id: String,
    /// vCard `UID`.
    pub uid: String,
    /// Owning address book.
    pub collection_id: String,
    /// Formatted name (`FN`).
    #[serde(rename = "fn")]
    pub fn_: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// Telephone number.
    pub phone: Option<String>,
    /// Organisation name (`ORG`).
    pub organization: Option<String>,
}

impl From<&CardObject> for ContactSearchResult {
    fn from(object: &CardObject) -> Self {
        Self {
            id: object.id.clone(),
            uid: object.uid.clone(),
            collection_id: object.collection_id.clone(),
            fn_: object.fn_.clone(),
            email: object.email.clone(),
            phone: object.phone.clone(),
            organization: object.organization.clone(),
        }
    }
}

/// Aggregate response of `GET /api/v1/search`: events and contacts together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResultsDto {
    /// Matching calendar objects.
    pub events: Vec<EventSearchResult>,
    /// Matching card objects.
    pub contacts: Vec<ContactSearchResult>,
}

/// Public representation of a contact (a `card_objects` row).
///
/// Deliberately excludes `raw_payload` (the composed `.vcf`): the SPA consumes
/// the already-parsed fields and never parses vCard itself. `created_at` /
/// `updated_at` are Unix epoch **milliseconds**.
///
/// `fn` (the vCard formatted name) is exposed both as `fn` and as `full_name`
/// for client convenience; both carry the same value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactDto {
    /// Internal card-object id (UUID).
    pub id: String,
    /// vCard `UID`, unique within the address book.
    pub uid: String,
    /// Owning address book.
    pub collection_id: String,
    /// Formatted name (`FN`), exposed as the canonical `fn` field.
    #[serde(rename = "fn")]
    pub fn_: Option<String>,
    /// Alias of `fn` (formatted name); always mirrors `fn`.
    pub full_name: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// Telephone number.
    pub phone: Option<String>,
    /// Organisation name (`ORG`).
    pub organization: Option<String>,
    /// Opaque ETag.
    pub etag: String,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
    /// Last modification time, Unix milliseconds.
    pub updated_at: i64,
}

impl From<&CardObject> for ContactDto {
    fn from(object: &CardObject) -> Self {
        Self {
            id: object.id.clone(),
            uid: object.uid.clone(),
            collection_id: object.collection_id.clone(),
            fn_: object.fn_.clone(),
            full_name: object.fn_.clone(),
            email: object.email.clone(),
            phone: object.phone.clone(),
            organization: object.organization.clone(),
            etag: object.etag.clone(),
            created_at: object.created_at,
            updated_at: object.updated_at,
        }
    }
}

// ---------------------------------------------------------------------------
// F22 — full-vCard detail DTOs (contract 4 of PLAN-023)
// ---------------------------------------------------------------------------

/// The full, typed projection of a contact as returned by
/// `GET .../contacts/{uid}`.
///
/// A **superset** of [`ContactDto`]: it keeps the light fields the list exposes
/// (`id`, `uid`, `collection_id`, `fn`/`full_name`, `etag`, timestamps) and adds
/// every group of the vCard projected by
/// [`crate::dav::vcf::project_card`]. It deliberately still excludes
/// `raw_payload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactDetailDto {
    /// Internal card-object id (UUID).
    pub id: String,
    /// vCard `UID`.
    pub uid: String,
    /// Owning address book.
    pub collection_id: String,
    /// Formatted name (`FN`), exposed as the canonical `fn` field.
    #[serde(rename = "fn")]
    pub fn_: Option<String>,
    /// Alias of `fn` (formatted name); always mirrors `fn`.
    pub full_name: Option<String>,
    /// Opaque ETag.
    pub etag: String,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
    /// Last modification time, Unix milliseconds.
    pub updated_at: i64,
    /// `VERSION`, verbatim.
    pub version: Option<String>,
    /// `REV`, verbatim.
    pub rev: Option<String>,
    /// Structured `N`.
    pub name: Option<StructuredNameDto>,
    /// `NICKNAME` occurrences.
    pub nicknames: Vec<String>,
    /// `PHOTO` (URI or embedded `data:` URI).
    pub photos: Vec<MediaDto>,
    /// `BDAY`, verbatim.
    pub birthday: Option<String>,
    /// `ANNIVERSARY`, verbatim.
    pub anniversary: Option<String>,
    /// `GENDER`.
    pub gender: Option<GenderDto>,
    /// `TEL` occurrences with their `TYPE`/`PREF`.
    pub tels: Vec<LabelledValueDto>,
    /// `EMAIL` occurrences with their `TYPE`/`PREF`.
    pub emails: Vec<LabelledValueDto>,
    /// `IMPP` occurrences with their `TYPE`/`PREF`.
    pub impps: Vec<LabelledValueDto>,
    /// `ADR` occurrences.
    pub addresses: Vec<AddressDto>,
    /// `GEO` occurrences.
    pub geos: Vec<String>,
    /// `TZ` occurrences.
    pub timezones: Vec<String>,
    /// `TITLE` occurrences.
    pub titles: Vec<String>,
    /// `ROLE` occurrences.
    pub roles: Vec<String>,
    /// `ORG` occurrences, one entry per line.
    pub organizations: Vec<Vec<String>>,
    /// `LOGO` (URI or embedded `data:` URI).
    pub logos: Vec<MediaDto>,
    /// `NOTE` occurrences.
    pub notes: Vec<String>,
    /// `CATEGORIES` occurrences, one entry per line.
    pub categories: Vec<Vec<String>>,
    /// `URL` occurrences.
    pub urls: Vec<String>,
    /// `SOUND` (URI or embedded `data:` URI).
    pub sounds: Vec<MediaDto>,
    /// Everything outside the modelled groups (`X-*` and RFC properties without
    /// a dedicated tab).
    pub others: Vec<RawPropertyDto>,
}

impl ContactDetailDto {
    /// Build the full detail DTO from the stored row and its projection.
    #[must_use]
    pub fn new(object: &CardObject, projection: CardProjection) -> Self {
        Self {
            id: object.id.clone(),
            uid: object.uid.clone(),
            collection_id: object.collection_id.clone(),
            fn_: projection.fn_.clone(),
            full_name: projection.fn_.clone(),
            etag: object.etag.clone(),
            created_at: object.created_at,
            updated_at: object.updated_at,
            version: non_empty(projection.version),
            rev: projection.rev,
            name: projection.name.map(StructuredNameDto::from),
            nicknames: projection.nicknames,
            photos: projection.photos.into_iter().map(MediaDto::from).collect(),
            birthday: projection.birthday,
            anniversary: projection.anniversary,
            gender: projection.gender.map(GenderDto::from),
            tels: projection
                .tels
                .into_iter()
                .map(LabelledValueDto::from)
                .collect(),
            emails: projection
                .emails
                .into_iter()
                .map(LabelledValueDto::from)
                .collect(),
            impps: projection
                .impps
                .into_iter()
                .map(LabelledValueDto::from)
                .collect(),
            addresses: projection
                .addresses
                .into_iter()
                .map(AddressDto::from)
                .collect(),
            geos: projection.geos,
            timezones: projection.timezones,
            titles: projection.titles,
            roles: projection.roles,
            organizations: projection.organizations,
            logos: projection.logos.into_iter().map(MediaDto::from).collect(),
            notes: projection.notes,
            categories: projection.categories,
            urls: projection.urls,
            sounds: projection.sounds.into_iter().map(MediaDto::from).collect(),
            others: projection
                .others
                .into_iter()
                .map(RawPropertyDto::from)
                .collect(),
        }
    }
}

/// An empty `String` becomes `None` (so an absent `VERSION` is `null`, not "").
fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// The five components of a structured `N`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredNameDto {
    /// Family name.
    pub family: Option<String>,
    /// Given name.
    pub given: Option<String>,
    /// Additional names.
    pub additional: Vec<String>,
    /// Honorific prefixes.
    pub prefix: Vec<String>,
    /// Honorific suffixes.
    pub suffix: Vec<String>,
}

impl From<StructuredName> for StructuredNameDto {
    fn from(name: StructuredName) -> Self {
        Self {
            family: name.family,
            given: name.given,
            additional: name.additional,
            prefix: name.prefix,
            suffix: name.suffix,
        }
    }
}

/// `GENDER`: sex code plus optional identity text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenderDto {
    /// The sex (`M`/`F`/`O`/`N`/`U`), when present.
    pub sex: Option<String>,
    /// The free-text gender identity, when present.
    pub identity: Option<String>,
}

impl From<CardGender> for GenderDto {
    fn from(gender: CardGender) -> Self {
        Self {
            sex: gender.sex,
            identity: gender.identity,
        }
    }
}

/// A single value with its `TYPE` parameters and `PREF`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelledValueDto {
    /// The property value.
    pub value: String,
    /// `TYPE` tokens.
    pub types: Vec<String>,
    /// `PREF` (lower is preferred), when present.
    pub pref: Option<u8>,
}

impl From<MultiValue> for LabelledValueDto {
    fn from(value: MultiValue) -> Self {
        Self {
            value: value.value,
            types: value.types,
            pref: value.pref,
        }
    }
}

/// A structured `ADR` with its seven components, `LABEL` and parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressDto {
    /// Post office box.
    pub po_box: Option<String>,
    /// Extended address (apartment, suite…).
    pub extended: Option<String>,
    /// Street address.
    pub street: Option<String>,
    /// Locality (city).
    pub locality: Option<String>,
    /// Region (state or province).
    pub region: Option<String>,
    /// Postal code.
    pub postal_code: Option<String>,
    /// Country name.
    pub country: Option<String>,
    /// The `LABEL` parameter, when present.
    pub label: Option<String>,
    /// `TYPE` tokens.
    pub types: Vec<String>,
    /// `PREF`, when present.
    pub pref: Option<u8>,
}

impl From<CardAddress> for AddressDto {
    fn from(address: CardAddress) -> Self {
        Self {
            po_box: address.po_box,
            extended: address.extended,
            street: address.street,
            locality: address.locality,
            region: address.region,
            postal_code: address.postal_code,
            country: address.country,
            label: address.label,
            types: address.types,
            pref: address.pref,
        }
    }
}

/// `PHOTO`/`LOGO`/`SOUND`: either an external URI or an embedded binary.
///
/// The `kind` discriminator is `"uri"` or `"data"`. The URI variant omits
/// `media_type`/`data_uri`; the data variant omits `uri` (`skip_serializing_if`),
/// so the SPA can branch on `kind` alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaDto {
    /// `"uri"` or `"data"`.
    pub kind: &'static str,
    /// The external URI (URI variant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// The media type (data variant only, when known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The ready-to-use `data:<media-type>;base64,<…>` URI (data variant only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_uri: Option<String>,
}

impl From<MediaValue> for MediaDto {
    fn from(value: MediaValue) -> Self {
        match value {
            MediaValue::Uri(uri) => Self {
                kind: "uri",
                uri: Some(uri),
                media_type: None,
                data_uri: None,
            },
            MediaValue::Data {
                media_type,
                data_uri,
            } => Self {
                kind: "data",
                uri: None,
                media_type,
                data_uri: Some(data_uri),
            },
        }
    }
}

/// A property outside the modelled groups, kept as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawPropertyDto {
    /// The property name (`X-ABLabel`, `PRODID`, …).
    pub name: String,
    /// The group prefix (`item1` in `item1.X-ABLabel`), when present.
    pub group: Option<String>,
    /// The property parameters as `(name, value)` pairs.
    pub parameters: Vec<(String, String)>,
    /// The property value.
    pub value: String,
}

impl From<RawProperty> for RawPropertyDto {
    fn from(property: RawProperty) -> Self {
        Self {
            name: property.name,
            group: property.group,
            parameters: property.parameters,
            value: property.value,
        }
    }
}

// ---------------------------------------------------------------------------
// F22 — write-side inputs (contract 4 of PLAN-023)
// ---------------------------------------------------------------------------

/// Structured `N` input for a create/patch request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct StructuredNameInput {
    /// Family name.
    #[serde(default)]
    pub family: Option<String>,
    /// Given name.
    #[serde(default)]
    pub given: Option<String>,
    /// Additional names.
    #[serde(default)]
    pub additional: Option<Vec<String>>,
    /// Honorific prefixes.
    #[serde(default)]
    pub prefix: Option<Vec<String>>,
    /// Honorific suffixes.
    #[serde(default)]
    pub suffix: Option<Vec<String>>,
}

/// `GENDER` input for a create/patch request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct GenderInput {
    /// The sex (`M`/`F`/`O`/`N`/`U`).
    #[serde(default)]
    pub sex: Option<String>,
    /// The free-text gender identity.
    #[serde(default)]
    pub identity: Option<String>,
}

/// A single value with optional `TYPE`/`PREF` for a create/patch request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LabelledValueInput {
    /// The property value.
    pub value: String,
    /// `TYPE` tokens.
    #[serde(default)]
    pub types: Option<Vec<String>>,
    /// `PREF` (1..=100).
    #[serde(default)]
    pub pref: Option<u8>,
}

/// A structured `ADR` input for a create/patch request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct AddressInput {
    /// Post office box.
    #[serde(default)]
    pub po_box: Option<String>,
    /// Extended address.
    #[serde(default)]
    pub extended: Option<String>,
    /// Street address.
    #[serde(default)]
    pub street: Option<String>,
    /// Locality (city).
    #[serde(default)]
    pub locality: Option<String>,
    /// Region (state or province).
    #[serde(default)]
    pub region: Option<String>,
    /// Postal code.
    #[serde(default)]
    pub postal_code: Option<String>,
    /// Country name.
    #[serde(default)]
    pub country: Option<String>,
    /// The `LABEL` parameter.
    #[serde(default)]
    pub label: Option<String>,
    /// `TYPE` tokens.
    #[serde(default)]
    pub types: Option<Vec<String>>,
    /// `PREF` (1..=100).
    #[serde(default)]
    pub pref: Option<u8>,
}

/// Request body for creating a contact.
///
/// The backend composes the skeleton (`.vcf` with `UID`/`FN`) and then applies a
/// surgical merge for every other field (the client never sends vCard). `fn`
/// (also accepted as `full_name`) is mandatory; the rest are optional. `uid` is
/// normally minted by the server, but a client may pin one — a subsequent
/// duplicate then conflicts (`409`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreateContactRequest {
    /// Formatted name (`FN`); required and non-blank. Also accepted as
    /// `full_name`.
    #[serde(rename = "fn", alias = "full_name")]
    pub fn_: String,
    /// Email address.
    #[serde(default)]
    pub email: Option<String>,
    /// Telephone number.
    #[serde(default)]
    pub phone: Option<String>,
    /// Organisation name (`ORG`).
    #[serde(default)]
    pub organization: Option<String>,
    /// Optional client-chosen `UID`; a fresh one is minted when omitted.
    #[serde(default)]
    pub uid: Option<String>,
    /// Structured `N`.
    #[serde(default)]
    pub name: Option<StructuredNameInput>,
    /// `NICKNAME` values.
    #[serde(default)]
    pub nicknames: Option<Vec<String>>,
    /// `BDAY`, as written.
    #[serde(default)]
    pub birthday: Option<String>,
    /// `ANNIVERSARY`, as written.
    #[serde(default)]
    pub anniversary: Option<String>,
    /// `GENDER`.
    #[serde(default)]
    pub gender: Option<GenderInput>,
    /// `TEL` values.
    #[serde(default)]
    pub tels: Option<Vec<LabelledValueInput>>,
    /// `EMAIL` values.
    #[serde(default)]
    pub emails: Option<Vec<LabelledValueInput>>,
    /// `IMPP` values.
    #[serde(default)]
    pub impps: Option<Vec<LabelledValueInput>>,
    /// `ADR` values.
    #[serde(default)]
    pub addresses: Option<Vec<AddressInput>>,
    /// `GEO` values.
    #[serde(default)]
    pub geos: Option<Vec<String>>,
    /// `TZ` values.
    #[serde(default)]
    pub timezones: Option<Vec<String>>,
    /// `TITLE` values.
    #[serde(default)]
    pub titles: Option<Vec<String>>,
    /// `ROLE` values.
    #[serde(default)]
    pub roles: Option<Vec<String>>,
    /// `ORG` values, one entry per line.
    #[serde(default)]
    pub organizations: Option<Vec<Vec<String>>>,
    /// `NOTE` values.
    #[serde(default)]
    pub notes: Option<Vec<String>>,
    /// `CATEGORIES` values, one entry per line.
    #[serde(default)]
    pub categories: Option<Vec<Vec<String>>>,
    /// `URL` values.
    #[serde(default)]
    pub urls: Option<Vec<String>>,
}

/// Request body for a partial update of a contact.
///
/// Only the fields present are applied; omitted fields keep their stored value.
/// `fn` (also accepted as `full_name`) is required to stay non-blank when
/// supplied. The stored `raw_payload` is merged surgically after validation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct UpdateContactRequest {
    /// New formatted name, if provided. Also accepted as `full_name`.
    #[serde(default, rename = "fn", alias = "full_name")]
    pub fn_: Option<String>,
    /// New email address, if provided.
    #[serde(default)]
    pub email: Option<String>,
    /// New telephone number, if provided.
    #[serde(default)]
    pub phone: Option<String>,
    /// New organisation name, if provided.
    #[serde(default)]
    pub organization: Option<String>,
    /// The `UID` is immutable: a `PATCH` carrying it is rejected with a `400`.
    #[serde(default)]
    pub uid: Option<String>,
    /// New structured `N`, if provided.
    #[serde(default)]
    pub name: Option<StructuredNameInput>,
    /// New `NICKNAME` values, if provided (`[]` deletes).
    #[serde(default)]
    pub nicknames: Option<Vec<String>>,
    /// New `BDAY`, if provided.
    #[serde(default)]
    pub birthday: Option<String>,
    /// New `ANNIVERSARY`, if provided.
    #[serde(default)]
    pub anniversary: Option<String>,
    /// New `GENDER`, if provided.
    #[serde(default)]
    pub gender: Option<GenderInput>,
    /// New `TEL` values, if provided (`[]` deletes).
    #[serde(default)]
    pub tels: Option<Vec<LabelledValueInput>>,
    /// New `EMAIL` values, if provided (`[]` deletes).
    #[serde(default)]
    pub emails: Option<Vec<LabelledValueInput>>,
    /// New `IMPP` values, if provided (`[]` deletes).
    #[serde(default)]
    pub impps: Option<Vec<LabelledValueInput>>,
    /// New `ADR` values, if provided (`[]` deletes).
    #[serde(default)]
    pub addresses: Option<Vec<AddressInput>>,
    /// New `GEO` values, if provided (`[]` deletes).
    #[serde(default)]
    pub geos: Option<Vec<String>>,
    /// New `TZ` values, if provided (`[]` deletes).
    #[serde(default)]
    pub timezones: Option<Vec<String>>,
    /// New `TITLE` values, if provided (`[]` deletes).
    #[serde(default)]
    pub titles: Option<Vec<String>>,
    /// New `ROLE` values, if provided (`[]` deletes).
    #[serde(default)]
    pub roles: Option<Vec<String>>,
    /// New `ORG` values, if provided (`[]` deletes).
    #[serde(default)]
    pub organizations: Option<Vec<Vec<String>>>,
    /// New `NOTE` values, if provided (`[]` deletes).
    #[serde(default)]
    pub notes: Option<Vec<String>>,
    /// New `CATEGORIES` values, if provided (`[]` deletes).
    #[serde(default)]
    pub categories: Option<Vec<Vec<String>>>,
    /// New `URL` values, if provided (`[]` deletes).
    #[serde(default)]
    pub urls: Option<Vec<String>>,
}

impl UpdateContactRequest {
    /// Whether the patch carries no updatable field at all.
    ///
    /// The immutable `uid` is deliberately **not** counted: a `PATCH` carrying
    /// only a `uid` is rejected as a `400` (either "empty" or "uid immutable").
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fn_.is_none()
            && self.email.is_none()
            && self.phone.is_none()
            && self.organization.is_none()
            && self.name.is_none()
            && self.nicknames.is_none()
            && self.birthday.is_none()
            && self.anniversary.is_none()
            && self.gender.is_none()
            && self.tels.is_none()
            && self.emails.is_none()
            && self.impps.is_none()
            && self.addresses.is_none()
            && self.geos.is_none()
            && self.timezones.is_none()
            && self.titles.is_none()
            && self.roles.is_none()
            && self.organizations.is_none()
            && self.notes.is_none()
            && self.categories.is_none()
            && self.urls.is_none()
    }
}

// ---------------------------------------------------------------------------
// F13 — app tokens
// ---------------------------------------------------------------------------

/// Public **metadata** of an app token, as listed by `GET /api/v1/app-tokens`.
///
/// Deliberately excludes both the secret and its digest (`secret_hash`): these
/// fields are the only safe ones to expose after creation. Timestamps are
/// rendered as ISO-8601 UTC strings (`YYYY-MM-DDTHH:MM:SSZ`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppTokenMetadata {
    /// Public identifier (the `oxidav_<id>_…` segment).
    pub id: String,
    /// Human-readable label ("iPhone", "DAVx⁵", …).
    pub name: String,
    /// Creation time, ISO-8601 UTC.
    pub created_at: String,
    /// Last successful use, ISO-8601 UTC; `null` when never used.
    pub last_used_at: Option<String>,
}

impl From<&AppToken> for AppTokenMetadata {
    fn from(token: &AppToken) -> Self {
        Self {
            id: token.id.clone(),
            name: token.name.clone(),
            created_at: iso8601_from_unix_secs(token.created_at),
            last_used_at: token.last_used_at.map(iso8601_from_unix_secs),
        }
    }
}

/// Response of `POST /api/v1/app-tokens`.
///
/// The **only** place the secret is ever returned: `token` carries
/// `oxidav_<id>_<secret>` in the clear, shown to the user exactly once. Only the
/// secret's digest is persisted, so the value can never be retrieved again.
///
/// `Debug` is implemented by hand so the secret is redacted: the token must
/// never reach a log line via `{:?}`. The serialized (`Serialize`) form still
/// carries the clear-text token — that is the contract of this endpoint.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppTokenCreated {
    /// Public identifier.
    pub id: String,
    /// Human-readable label.
    pub name: String,
    /// Full token `oxidav_<id>_<secret>` (secret in the clear, one time only).
    pub token: String,
    /// Creation time, ISO-8601 UTC.
    pub created_at: String,
}

impl std::fmt::Debug for AppTokenCreated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppTokenCreated")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("token", &"…")
            .field("created_at", &self.created_at)
            .finish()
    }
}

impl AppTokenCreated {
    /// Build the creation DTO from the stored row and the freshly minted token.
    #[must_use]
    pub fn new(token: &AppToken, raw_token: String) -> Self {
        Self {
            id: token.id.clone(),
            name: token.name.clone(),
            token: raw_token,
            created_at: iso8601_from_unix_secs(token.created_at),
        }
    }
}

/// Request body for `POST /api/v1/app-tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreateAppTokenRequest {
    /// Human-readable label; required and non-blank.
    pub name: String,
}

/// Request body for `PATCH /api/v1/app-tokens/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RenameAppTokenRequest {
    /// New human-readable label; required and non-blank.
    pub name: String,
}

/// Format a Unix timestamp (whole **seconds**) as an ISO-8601 UTC instant.
///
/// Rendered as `YYYY-MM-DDTHH:MM:SSZ`. Implemented in-crate to avoid pulling a
/// date-formatting dependency for the single UTC instant the API needs: the
/// civil-date conversion (Howard Hinnant's `civil_from_days`) is a handful of
/// lines. Negative timestamps (pre-1970) are handled via Euclidean division.
fn iso8601_from_unix_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (hour, minute, second) = (
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60,
    );
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Convert a count of days since 1970-01-01 to a `(year, month, day)` civil
/// date. Algorithm from Howard Hinnant's `civil_from_days` (public domain).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // Shift the epoch to 0000-03-01 so leap days fall at the end of the cycle.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // day-of-era, [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day-of-year, [0, 365]
    let mp = (5 * doy + 2) / 153; // March-based month, [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::iso8601_from_unix_secs;

    #[test]
    fn iso8601_formats_known_instants() {
        assert_eq!(iso8601_from_unix_secs(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            iso8601_from_unix_secs(1_000_000_000),
            "2001-09-09T01:46:40Z"
        );
        assert_eq!(
            iso8601_from_unix_secs(1_700_000_000),
            "2023-11-14T22:13:20Z"
        );
        // A leap day is rendered correctly.
        assert_eq!(
            iso8601_from_unix_secs(1_582_934_400),
            "2020-02-29T00:00:00Z"
        );
    }

    #[test]
    fn iso8601_handles_pre_epoch() {
        assert_eq!(iso8601_from_unix_secs(-1), "1969-12-31T23:59:59Z");
        assert_eq!(iso8601_from_unix_secs(-86_400), "1969-12-31T00:00:00Z");
    }
}
