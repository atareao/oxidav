//! F07 — DAV XML helpers: `207 Multi-Status` serialisation and the parsing of
//! `PROPFIND` / REPORT request bodies.
//!
//! # Security (no XXE, no billion-laughs)
//!
//! Reading is done with `quick-xml`, which **never parses DTDs nor resolves
//! external or general entities** — it is a pull parser with no entity
//! resolution machinery. That structurally rules out XXE and billion-laughs.
//! On top of that, both parsers here **reject any `<!DOCTYPE ...>` declaration
//! outright** ([`XmlError::DoctypeNotAllowed`]), so a document that tries to
//! smuggle an entity/DTD payload is refused rather than merely ignored.
//!
//! Namespaces are resolved with [`quick_xml::NsReader`]: element local names are
//! matched after stripping any prefix, and the resolved namespace URI is kept on
//! the parsed structures, so arbitrary client prefixes (`D:`, `cal:`, …) are
//! handled correctly for the `DAV:` and `urn:ietf:params:xml:ns:caldav` namespaces.
//!
//! Writing is done by hand (no XML serialiser — see the `write` submodule), with
//! every text node escaped; request bodies are parsed in the `parse` submodule.

mod parse;
mod write;

pub use parse::{
    ParseMkcalendar, parse_addressbook_multiget, parse_addressbook_query, parse_calendar_query,
    parse_carddav_report, parse_mkcalendar, parse_propfind, parse_sync_collection,
    report_root_name,
};
pub use write::{
    escape, multistatus, multistatus_with_sync_token, status_line, valid_sync_token_error,
};

/// Namespace URI of the DAV: vocabulary (RFC 4918).
pub const NS_DAV: &str = "DAV:";
/// Namespace URI of the CalDAV vocabulary (RFC 4791).
pub const NS_CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
/// Namespace URI of the CardDAV vocabulary (RFC 6352).
pub const NS_CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
/// Namespace URI of the CalendarServer extensions (where `getctag` lives).
pub const NS_CALENDARSERVER: &str = "http://calendarserver.org/ns/";

/// Namespace of a written DAV property.
///
/// Only the vocabularies OxiDav speaks are representable, so a property can
/// never be serialised under the wrong namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavNs {
    /// `DAV:`, written with the `d:` prefix.
    Dav,
    /// `urn:ietf:params:xml:ns:caldav`, written with the `c:` prefix.
    CalDav,
    /// `urn:ietf:params:xml:ns:carddav`, written with the `card:` prefix.
    CardDav,
    /// `http://calendarserver.org/ns/`, written with the `cs:` prefix.
    Calendarserver,
}

impl DavNs {
    /// The prefix used when serialising a property in this namespace.
    #[must_use]
    fn prefix(self) -> &'static str {
        match self {
            Self::Dav => "d",
            Self::CalDav => "c",
            Self::CardDav => "card",
            Self::Calendarserver => "cs",
        }
    }

    /// The resolved namespace URI of this vocabulary.
    #[must_use]
    pub fn uri(self) -> &'static str {
        match self {
            Self::Dav => NS_DAV,
            Self::CalDav => NS_CALDAV,
            Self::CardDav => NS_CARDDAV,
            Self::Calendarserver => NS_CALENDARSERVER,
        }
    }
}

/// Map a resolved namespace URI to the vocabulary OxiDav can serialise.
///
/// Returns `None` for any namespace without a [`DavNs`] variant; such a
/// requested property cannot be written back (the serialiser only emits the
/// `d:` / `c:` / `cs:` prefixes) and is therefore dropped by the PROPFIND
/// negotiator rather than reported.
#[must_use]
pub fn dav_ns_from_uri(uri: &str) -> Option<DavNs> {
    match uri {
        NS_DAV => Some(DavNs::Dav),
        NS_CALDAV => Some(DavNs::CalDav),
        NS_CARDDAV => Some(DavNs::CardDav),
        NS_CALENDARSERVER => Some(DavNs::Calendarserver),
        _ => None,
    }
}

/// The content of a `<d:prop>` child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropValue {
    /// A self-closing empty element, e.g. `<d:getetag/>`.
    Empty,
    /// Escaped text content.
    Text(String),
    /// Pre-serialised child XML. This is **trusted**: it is produced by
    /// server-side code (e.g. `<d:collection/>`), never from client input.
    Xml(String),
}

/// A single property inside a `propstat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavProp {
    /// Vocabulary the property belongs to.
    pub ns: DavNs,
    /// Local element name (must be a valid NCName; supplied by server code).
    pub name: String,
    /// Element content.
    pub value: PropValue,
}

/// One `<d:response>` of a `207 Multi-Status`.
///
/// When `status` is `200` and `props` is non-empty, the properties are emitted
/// inside a `200` `propstat`; requested properties the resource does not define
/// go into a `404` `propstat` (RFC 4918 §9.1). Otherwise the whole
/// `<d:response>` carries a response-level `<d:status>` (used for per-resource
/// errors such as `404`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavResponse {
    /// Resource path, e.g. `/dav/calendars/alice/`.
    pub href: String,
    /// HTTP status of this response.
    pub status: u16,
    /// Properties returned for this resource (only meaningful for `200`).
    pub props: Vec<DavProp>,
    /// Requested properties the resource does not define, emitted in a `404`
    /// `propstat`. Empty for `allprop` / `propname` and for error responses.
    pub not_found: Vec<DavProp>,
}

/// A property requested by a client in a `PROPFIND` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropName {
    /// Resolved namespace URI (empty when the prefix was unbound).
    pub namespace: String,
    /// Local element name.
    pub name: String,
}

/// Parsed `REPORT sync-collection` request body (RFC 6578 §3.2).
///
/// The raw `sync-token` / `sync-level` text is captured verbatim (validated by
/// [`super::common::parse_sync_collection`]) together with any properties listed
/// in a `<d:prop>` child, so the handler can decide whether the client asked for
/// the resource payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseSyncCollection {
    /// Text content of `<d:sync-token>`, or `None` when absent (initial sync).
    pub sync_token: Option<String>,
    /// Text content of `<d:sync-level>`, or `None` when absent (defaults to `1`).
    pub sync_level: Option<String>,
    /// Properties requested inside `<d:prop>`.
    pub props: Vec<PropName>,
}

/// Parsed `PROPFIND` request body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsePropfind {
    /// `<d:allprop/>` was requested.
    pub all_props: bool,
    /// `<d:propname/>` was requested.
    pub prop_names: bool,
    /// Explicitly requested properties (from `<d:prop>`).
    pub props: Vec<PropName>,
}

/// A parsed `time-range` filter (RFC 4791 §9.9).
///
/// A `time-range` may carry a `start`, an `end`, or both. When an attribute is
/// absent the corresponding bound is represented by a sentinel: an open start is
/// [`TimeRange::UNBOUNDED_START`] and an open end is [`TimeRange::UNBOUNDED_END`],
/// so the range always covers the whole timeline in that direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    /// Inclusive start, Unix milliseconds. [`Self::UNBOUNDED_START`] when the
    /// `start` attribute was absent.
    pub start_ms: i64,
    /// Exclusive end, Unix milliseconds. [`Self::UNBOUNDED_END`] when the `end`
    /// attribute was absent.
    pub end_ms: i64,
}

impl TimeRange {
    /// Sentinel stored in [`Self::start_ms`] when `time-range` has no `start`
    /// attribute (the range is open towards the past).
    pub const UNBOUNDED_START: i64 = i64::MIN;
    /// Sentinel stored in [`Self::end_ms`] when `time-range` has no `end`
    /// attribute (the range is open towards the future).
    pub const UNBOUNDED_END: i64 = i64::MAX;

    /// Whether an event occupying the half-open span `[event_start, event_end)`
    /// intersects this range.
    ///
    /// This is the RFC 4791 §9.9 overlap test: the two intervals intersect iff
    /// the event starts before the range ends **and** the range starts before the
    /// event ends. With the unbounded sentinels the test degenerates correctly to
    /// a one-sided comparison.
    #[must_use]
    pub fn matches_event(&self, event_start: i64, event_end: i64) -> bool {
        event_start < self.end_ms && self.start_ms < event_end
    }
}

/// Parsed `REPORT calendar-query` request body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseCalendarQuery {
    /// The optional `time-range` filter.
    pub time_range: Option<TimeRange>,
}

/// Parsed `REPORT addressbook-query` request body (RFC 6352 §10.5).
///
/// OxiDav implements a deliberately small subset: the text content of every
/// `<card:text-match>` element is collected into [`Self::terms`] and used as a
/// scalar, case-insensitive `contains` filter across the denormalised card
/// fields (`fn`, `email`, `phone`, `organization`, `notes`). The surrounding
/// `<card:prop-filter>` name, the `match-type` and `collation` attributes are
/// **not** interpreted — advanced filters are out of scope for F08 — and the
/// collected terms are combined with AND. A request with no `text-match` selects
/// every resource of the collection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseAddressbookQuery {
    /// The `contains` terms extracted from the request's `text-match` elements,
    /// in document order.
    pub terms: Vec<String>,
}

/// Parsed `REPORT addressbook-multiget` request body (RFC 6352 §10.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseAddressbookMultiget {
    /// The requested `<d:href>` values, in document order.
    pub hrefs: Vec<String>,
}

/// A parsed CardDAV `REPORT` body, dispatched on its root element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardDavReport {
    /// An `addressbook-query` (RFC 6352 §10.5).
    Query(ParseAddressbookQuery),
    /// An `addressbook-multiget` (RFC 6352 §10.6).
    Multiget(ParseAddressbookMultiget),
}

/// Errors produced while reading DAV XML bodies.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum XmlError {
    /// The body is not well-formed XML.
    #[error("XML malformado: {0}")]
    Malformed(String),
    /// A DTD / `<!DOCTYPE ...>` declaration was present and is refused.
    #[error("declaraciones DOCTYPE/DTD no permitidas")]
    DoctypeNotAllowed,
    /// A `time-range` filter was present but not a valid date-time pair.
    #[error("rango temporal inválido")]
    InvalidTimeRange,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_range_overlap_follows_the_half_open_test() {
        let range = TimeRange {
            start_ms: 100,
            end_ms: 200,
        };
        // Straddling either boundary overlaps.
        assert!(range.matches_event(50, 150));
        assert!(range.matches_event(150, 250));
        assert!(range.matches_event(120, 130));
        // Fully outside does not.
        assert!(!range.matches_event(0, 100));
        assert!(!range.matches_event(200, 300));
        // Touching only the exclusive end does not overlap.
        assert!(!range.matches_event(200, 250));
    }

    #[test]
    fn unbounded_time_range_matches_any_event() {
        let open = TimeRange {
            start_ms: TimeRange::UNBOUNDED_START,
            end_ms: TimeRange::UNBOUNDED_END,
        };
        assert!(open.matches_event(i64::MIN + 1, i64::MAX - 1));
    }
}
