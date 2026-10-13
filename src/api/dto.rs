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
    /// Start instant, Unix milliseconds.
    pub dtstart: Option<i64>,
    /// End instant, Unix milliseconds.
    pub dtend: Option<i64>,
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
            dtstart: object.dtstart,
            dtend: object.dtend,
            rrule: object.rrule.clone(),
            etag: object.etag.clone(),
            created_at: object.created_at,
            updated_at: object.updated_at,
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
    /// Start instant, Unix milliseconds; required.
    pub dtstart: i64,
    /// End instant, Unix milliseconds; must be `>= dtstart` when present.
    #[serde(default)]
    pub dtend: Option<i64>,
    /// Raw `RRULE` text (never expanded).
    #[serde(default)]
    pub rrule: Option<String>,
    /// Optional client-chosen `UID`; a fresh one is minted when omitted.
    #[serde(default)]
    pub uid: Option<String>,
}

/// Request body for a partial update of a calendar event.
///
/// Only the fields present are applied; omitted fields keep their stored value.
/// Note that a field cannot be cleared to `null` (the DTO cannot distinguish
/// "absent" from "null"): a documented MVP limitation. The `.ics` in
/// `raw_payload` is recomposed from the merged fields after validation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UpdateEventRequest {
    /// New title, if provided.
    #[serde(default)]
    pub summary: Option<String>,
    /// New body, if provided.
    #[serde(default)]
    pub description: Option<String>,
    /// New start instant, if provided.
    #[serde(default)]
    pub dtstart: Option<i64>,
    /// New end instant, if provided.
    #[serde(default)]
    pub dtend: Option<i64>,
    /// New raw `RRULE` text, if provided.
    #[serde(default)]
    pub rrule: Option<String>,
}

impl UpdateEventRequest {
    /// Whether the patch carries no updatable field at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.summary.is_none()
            && self.description.is_none()
            && self.dtstart.is_none()
            && self.dtend.is_none()
            && self.rrule.is_none()
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
    /// The `sub` claim of the verified bearer token.
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

/// Request body for creating a contact.
///
/// The backend composes the `.vcf` stored in `raw_payload` from these fields
/// (the client never sends vCard). `fn` (also accepted as `full_name`) is
/// mandatory; the rest are optional. `uid` is normally minted by the server, but
/// a client may pin one — a subsequent duplicate then conflicts (`409`).
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
}

/// Request body for a partial update of a contact.
///
/// Only the fields present are applied; omitted fields keep their stored value.
/// `fn` (also accepted as `full_name`) is required to stay non-blank when
/// supplied. The `.vcf` in `raw_payload` is recomposed from the merged fields
/// after validation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
}

impl UpdateContactRequest {
    /// Whether the patch carries no updatable field at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fn_.is_none()
            && self.email.is_none()
            && self.phone.is_none()
            && self.organization.is_none()
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
