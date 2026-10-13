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
//! Writing is done by hand (no XML serialiser), with every text node escaped.

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;

use super::ics;

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

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Serialise a `207 Multi-Status` document.
///
/// The root declares the `d:` (`DAV:`) and `c:` (CalDAV) namespaces, so every
/// response and property can be written unambiguously.
#[must_use]
pub fn multistatus(responses: &[DavResponse]) -> String {
    multistatus_inner(responses, None)
}

/// Serialise a `207 Multi-Status` for a `sync-collection` report (RFC 6578).
///
/// Identical to [`multistatus`] but appends a single `<d:sync-token>` child to
/// the root, **after** every `<d:response>`, which is what a client uses as the
/// starting token for its next incremental sync. The token value is escaped even
/// though OxiDav only ever emits its own `http://oxidav/ns/sync/<n>` form.
#[must_use]
pub fn multistatus_with_sync_token(responses: &[DavResponse], sync_token: &str) -> String {
    multistatus_inner(responses, Some(sync_token))
}

/// Serialise the RFC 6578 §3.2 `valid-sync-token` precondition error.
///
/// When a client presents a `sync-token` the server cannot recognise, RFC 6578
/// requires the `403 Forbidden` response to carry a DAV `error` body naming the
/// `valid-sync-token` precondition (rather than a protocol-agnostic envelope),
/// so strict DAV clients can react to the specific failure.
#[must_use]
pub fn valid_sync_token_error() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<d:error xmlns:d=\"{NS_DAV}\"><d:valid-sync-token/></d:error>"
    )
}

/// Shared body of [`multistatus`] and [`multistatus_with_sync_token`].
fn multistatus_inner(responses: &[DavResponse], sync_token: Option<&str>) -> String {
    let mut out = String::with_capacity(128 + responses.len() * 128);
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<d:multistatus xmlns:d=\"");
    out.push_str(NS_DAV);
    out.push_str("\" xmlns:c=\"");
    out.push_str(NS_CALDAV);
    out.push_str("\" xmlns:card=\"");
    out.push_str(NS_CARDDAV);
    out.push_str("\" xmlns:cs=\"");
    out.push_str(NS_CALENDARSERVER);
    out.push_str("\">");

    for response in responses {
        write_response(&mut out, response);
    }

    // RFC 6578 §3.2: the new sync-token is a direct child of the multistatus,
    // following the responses.
    if let Some(token) = sync_token {
        out.push_str("<d:sync-token>");
        escape_into(&mut out, token);
        out.push_str("</d:sync-token>");
    }

    out.push_str("</d:multistatus>");
    out
}

/// Serialise a single `<d:response>`.
fn write_response(out: &mut String, response: &DavResponse) {
    out.push_str("<d:response><d:href>");
    escape_into(out, &response.href);
    out.push_str("</d:href>");

    let has_props = response.status == 200 && !response.props.is_empty();
    if has_props {
        write_propstat(out, 200, &response.props);
    }
    if response.status == 200 && !response.not_found.is_empty() {
        // Requested-but-undefined properties are reported in their own `404`
        // `propstat`, alongside the `200` one (RFC 4918 §9.1).
        write_propstat(out, 404, &response.not_found);
    } else if !has_props {
        // A per-resource error, or a `200` with neither found nor missing
        // properties, is reported with a response-level status.
        out.push_str("<d:status>");
        out.push_str(&status_line(response.status));
        out.push_str("</d:status>");
    }

    out.push_str("</d:response>");
}

/// Serialise a single `<d:propstat>` for `props` with the given HTTP `status`.
fn write_propstat(out: &mut String, status: u16, props: &[DavProp]) {
    out.push_str("<d:propstat><d:prop>");
    for prop in props {
        write_prop(out, prop);
    }
    out.push_str("</d:prop><d:status>");
    out.push_str(&status_line(status));
    out.push_str("</d:status></d:propstat>");
}

/// Serialise a single property element.
fn write_prop(out: &mut String, prop: &DavProp) {
    let prefix = prop.ns.prefix();
    let open = |out: &mut String| {
        out.push('<');
        out.push_str(prefix);
        out.push(':');
        out.push_str(&prop.name);
    };
    let close = |out: &mut String| {
        out.push_str("</");
        out.push_str(prefix);
        out.push(':');
        out.push_str(&prop.name);
        out.push('>');
    };

    match &prop.value {
        PropValue::Empty => {
            open(out);
            out.push_str("/>");
        }
        PropValue::Text(text) => {
            open(out);
            out.push('>');
            escape_into(out, text);
            close(out);
        }
        PropValue::Xml(xml) => {
            open(out);
            out.push('>');
            out.push_str(xml);
            close(out);
        }
    }
}

/// A `HTTP/1.1 <code> <reason>` status line, as required in DAV status elements.
#[must_use]
pub fn status_line(code: u16) -> String {
    format!("HTTP/1.1 {code} {}", reason_phrase(code))
}

/// The reason phrase for the HTTP status codes OxiDav emits.
#[must_use]
fn reason_phrase(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        207 => "Multi-Status",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        412 => "Precondition Failed",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        507 => "Insufficient Storage",
        _ => "Unknown",
    }
}

/// Escape `text` for inclusion in XML character data or a double-quoted
/// attribute, replacing the five predefined entities (`&`, `<`, `>`, `"`, `'`).
///
/// Public so DAV modules can safely interpolate a dynamic value into a raw
/// `PropValue::Xml` fragment (which, unlike [`PropValue::Text`], is emitted
/// verbatim).
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_into(&mut out, text);
    out
}

/// Append `text` to `out`, escaping the five XML predefined characters.
fn escape_into(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Parse a `PROPFIND` request body.
///
/// Recognises `<d:allprop/>`, `<d:propname/>` and the names inside `<d:prop>`.
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML, [`XmlError::DoctypeNotAllowed`] for any
/// DTD / `<!DOCTYPE ...>`.
pub fn parse_propfind(body: &str) -> Result<ParsePropfind, XmlError> {
    let mut reader = NsReader::from_str(body);
    let mut buf = Vec::new();
    let mut result = ParsePropfind::default();
    let mut in_prop = false;
    let mut saw_root = false;

    loop {
        let (resolved, event) = reader
            .read_resolved_event_into(&mut buf)
            .map_err(|err| XmlError::Malformed(err.to_string()))?;

        match event {
            Event::Eof => break,
            Event::DocType(_) => return Err(XmlError::DoctypeNotAllowed),
            Event::Start(element) => {
                saw_root = true;
                let local = local_name(element.name().as_ref());
                match local.as_str() {
                    "prop" => in_prop = true,
                    "allprop" => result.all_props = true,
                    "propname" => result.prop_names = true,
                    _ if in_prop => result.props.push(PropName {
                        namespace: namespace_of(&resolved),
                        name: local,
                    }),
                    _ => {}
                }
            }
            Event::Empty(element) => {
                saw_root = true;
                let local = local_name(element.name().as_ref());
                match local.as_str() {
                    "allprop" => result.all_props = true,
                    "propname" => result.prop_names = true,
                    _ if in_prop => result.props.push(PropName {
                        namespace: namespace_of(&resolved),
                        name: local,
                    }),
                    _ => {}
                }
            }
            Event::End(element) if local_name(element.name().as_ref()) == "prop" => {
                in_prop = false;
            }
            _ => {}
        }

        buf.clear();
    }

    if !saw_root {
        return Err(XmlError::Malformed(
            "el documento no contiene ningún elemento raíz".to_string(),
        ));
    }

    Ok(result)
}

/// Parse a `REPORT sync-collection` request body (RFC 6578 §3.2).
///
/// Captures the `<d:sync-token>` and `<d:sync-level>` text and the names inside a
/// `<d:prop>` child. Child elements are read from the `DAV:` namespace (or with
/// no namespace, for lenient clients); an unparseable/absent body is handled by
/// the caller. Any DTD / `<!DOCTYPE ...>` is refused (no XXE).
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML or a root element that is not
/// `sync-collection`, [`XmlError::DoctypeNotAllowed`] for any DTD /
/// `<!DOCTYPE ...>`.
pub fn parse_sync_collection(body: &str) -> Result<ParseSyncCollection, XmlError> {
    let mut reader = NsReader::from_str(body);
    let mut buf = Vec::new();
    let mut result = ParseSyncCollection::default();
    let mut saw_root = false;
    let mut root_checked = false;
    let mut in_prop = false;
    // `Some(true)` while capturing `sync-token`, `Some(false)` for `sync-level`.
    let mut capture: Option<bool> = None;
    let mut text = String::new();

    loop {
        let (resolved, event) = reader
            .read_resolved_event_into(&mut buf)
            .map_err(|err| XmlError::Malformed(err.to_string()))?;

        match event {
            Event::Eof => break,
            Event::DocType(_) => return Err(XmlError::DoctypeNotAllowed),
            Event::Start(element) => {
                saw_root = true;
                let local = local_name(element.name().as_ref());
                if !root_checked {
                    root_checked = true;
                    if local != "sync-collection" {
                        return Err(XmlError::Malformed(
                            "el REPORT no es un sync-collection".to_string(),
                        ));
                    }
                    continue;
                }
                if is_dav_namespace(&resolved) {
                    match local.as_str() {
                        "prop" => in_prop = true,
                        "sync-token" => {
                            capture = Some(true);
                            text.clear();
                        }
                        "sync-level" => {
                            capture = Some(false);
                            text.clear();
                        }
                        _ if in_prop => result.props.push(PropName {
                            namespace: namespace_of(&resolved),
                            name: local,
                        }),
                        _ => {}
                    }
                }
            }
            Event::Empty(element) => {
                saw_root = true;
                let local = local_name(element.name().as_ref());
                if !root_checked {
                    root_checked = true;
                    if local != "sync-collection" {
                        return Err(XmlError::Malformed(
                            "el REPORT no es un sync-collection".to_string(),
                        ));
                    }
                    continue;
                }
                if is_dav_namespace(&resolved) {
                    match local.as_str() {
                        "sync-token" => result.sync_token = Some(String::new()),
                        "sync-level" => result.sync_level = Some(String::new()),
                        _ if in_prop => result.props.push(PropName {
                            namespace: namespace_of(&resolved),
                            name: local,
                        }),
                        _ => {}
                    }
                }
            }
            Event::Text(value) => {
                if capture.is_some() {
                    text.push_str(value.as_ref());
                }
            }
            Event::GeneralRef(reference) => {
                if capture.is_some() {
                    if let Ok(Some(ch)) = reference.resolve_char_ref() {
                        text.push(ch);
                    } else {
                        match reference.into_inner().as_ref() {
                            "amp" => text.push('&'),
                            "lt" => text.push('<'),
                            "gt" => text.push('>'),
                            "quot" => text.push('"'),
                            "apos" => text.push('\''),
                            _ => {}
                        }
                    }
                }
            }
            Event::End(element) => {
                let local = local_name(element.name().as_ref());
                if local == "prop" {
                    in_prop = false;
                }
                match capture {
                    Some(true) if local == "sync-token" => {
                        result.sync_token = Some(std::mem::take(&mut text));
                        capture = None;
                    }
                    Some(false) if local == "sync-level" => {
                        result.sync_level = Some(std::mem::take(&mut text));
                        capture = None;
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        buf.clear();
    }

    if !saw_root {
        return Err(XmlError::Malformed(
            "el documento no contiene ningún elemento raíz".to_string(),
        ));
    }

    Ok(result)
}

/// The local name of a `REPORT` body's root element (any prefix stripped).
///
/// Used by the CalDAV/CardDAV handlers to dispatch between the protocol-specific
/// report types and the shared `sync-collection`.
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML or an empty document,
/// [`XmlError::DoctypeNotAllowed`] for any DTD / `<!DOCTYPE ...>`.
pub fn report_root_name(body: &str) -> Result<String, XmlError> {
    root_element_name(body)
}
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML, [`XmlError::DoctypeNotAllowed`] for any
/// DTD / `<!DOCTYPE ...>`, and [`XmlError::InvalidTimeRange`] when a
/// `<c:time-range>` element is present but its `start`/`end` attributes are not
/// valid iCalendar date-times.
pub fn parse_calendar_query(body: &str) -> Result<ParseCalendarQuery, XmlError> {
    let mut reader = NsReader::from_str(body);
    let mut buf = Vec::new();
    let mut time_range = None;
    let mut saw_root = false;

    loop {
        let (_resolved, event) = reader
            .read_resolved_event_into(&mut buf)
            .map_err(|err| XmlError::Malformed(err.to_string()))?;

        match event {
            Event::Eof => break,
            Event::DocType(_) => return Err(XmlError::DoctypeNotAllowed),
            Event::Start(element) | Event::Empty(element) => {
                saw_root = true;
                if local_name(element.name().as_ref()) == "time-range" {
                    time_range = Some(parse_time_range(&element)?);
                }
            }
            _ => {}
        }

        buf.clear();
    }

    if !saw_root {
        return Err(XmlError::Malformed(
            "el documento no contiene ningún elemento raíz".to_string(),
        ));
    }

    Ok(ParseCalendarQuery { time_range })
}

/// Parse the `start`/`end` attributes of a `<c:time-range>` element.
///
/// At least one of the two attributes must be present; a `<c:time-range>` with
/// neither is invalid. The missing bound becomes an unbounded sentinel (see
/// [`TimeRange`]). A present-but-unparseable attribute is also rejected.
fn parse_time_range(element: &BytesStart<'_>) -> Result<TimeRange, XmlError> {
    let mut start = None;
    let mut end = None;

    for attribute in element.attributes() {
        let attribute = attribute.map_err(|err| XmlError::Malformed(err.to_string()))?;
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|err| XmlError::Malformed(err.to_string()))?;
        match attribute.key.as_ref() {
            "start" => start = Some(value.into_owned()),
            "end" => end = Some(value.into_owned()),
            _ => {}
        }
    }

    if start.is_none() && end.is_none() {
        return Err(XmlError::InvalidTimeRange);
    }

    let start_ms = match start.as_deref() {
        Some(value) => ics::parse_datetime_ms(value).ok_or(XmlError::InvalidTimeRange)?,
        None => TimeRange::UNBOUNDED_START,
    };
    let end_ms = match end.as_deref() {
        Some(value) => ics::parse_datetime_ms(value).ok_or(XmlError::InvalidTimeRange)?,
        None => TimeRange::UNBOUNDED_END,
    };

    Ok(TimeRange { start_ms, end_ms })
}

/// Parse a CardDAV `REPORT` body, dispatching on its root element.
///
/// Recognises `addressbook-query` and `addressbook-multiget` (RFC 6352 §10.5 /
/// §10.6); any other root is refused. Entity, numeric-character and the five
/// predefined XML references inside captured text are decoded, and any DTD /
/// `<!DOCTYPE ...>` is refused (no XXE).
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML or an unsupported root element,
/// [`XmlError::DoctypeNotAllowed`] for any DTD / `<!DOCTYPE ...>`.
pub fn parse_carddav_report(body: &str) -> Result<CardDavReport, XmlError> {
    match root_element_name(body)?.as_str() {
        "addressbook-query" => Ok(CardDavReport::Query(parse_addressbook_query(body)?)),
        "addressbook-multiget" => Ok(CardDavReport::Multiget(parse_addressbook_multiget(body)?)),
        _ => Err(XmlError::Malformed(
            "tipo de REPORT CardDAV no soportado".to_string(),
        )),
    }
}

/// Parse an `addressbook-query` body, collecting its `text-match` terms.
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML, [`XmlError::DoctypeNotAllowed`] for any
/// DTD / `<!DOCTYPE ...>`.
pub fn parse_addressbook_query(body: &str) -> Result<ParseAddressbookQuery, XmlError> {
    Ok(ParseAddressbookQuery {
        terms: element_texts(body, "text-match")?,
    })
}

/// Parse an `addressbook-multiget` body, collecting its `<d:href>` values.
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML, [`XmlError::DoctypeNotAllowed`] for any
/// DTD / `<!DOCTYPE ...>`.
pub fn parse_addressbook_multiget(body: &str) -> Result<ParseAddressbookMultiget, XmlError> {
    Ok(ParseAddressbookMultiget {
        hrefs: element_texts(body, "href")?,
    })
}

/// The local name of the document's root element (any prefix stripped).
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML or an empty document,
/// [`XmlError::DoctypeNotAllowed`] for any DTD / `<!DOCTYPE ...>`.
fn root_element_name(body: &str) -> Result<String, XmlError> {
    let mut reader = NsReader::from_str(body);
    let mut buf = Vec::new();

    loop {
        let (_resolved, event) = reader
            .read_resolved_event_into(&mut buf)
            .map_err(|err| XmlError::Malformed(err.to_string()))?;

        match event {
            Event::Start(element) | Event::Empty(element) => {
                return Ok(local_name(element.name().as_ref()));
            }
            Event::DocType(_) => return Err(XmlError::DoctypeNotAllowed),
            Event::Eof => {
                return Err(XmlError::Malformed(
                    "el documento no contiene ningún elemento raíz".to_string(),
                ));
            }
            _ => {}
        }

        buf.clear();
    }
}

/// Collect the decoded text content of every element whose local name is
/// `target`, in document order.
///
/// A self-closing `target` element contributes an empty string. Nested elements
/// inside a captured target are tolerated (their text is still part of the
/// captured value); the capture ends at the matching close tag.
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML or an empty document,
/// [`XmlError::DoctypeNotAllowed`] for any DTD / `<!DOCTYPE ...>`.
fn element_texts(body: &str, target: &str) -> Result<Vec<String>, XmlError> {
    let mut reader = NsReader::from_str(body);
    let mut buf = Vec::new();
    let mut saw_root = false;
    let mut capturing = false;
    let mut depth: usize = 0;
    let mut current = String::new();
    let mut out = Vec::new();

    loop {
        let (_resolved, event) = reader
            .read_resolved_event_into(&mut buf)
            .map_err(|err| XmlError::Malformed(err.to_string()))?;

        match event {
            Event::Eof => break,
            Event::DocType(_) => return Err(XmlError::DoctypeNotAllowed),
            Event::Start(element) => {
                saw_root = true;
                if capturing {
                    depth += 1;
                } else if local_name(element.name().as_ref()) == target {
                    capturing = true;
                    current.clear();
                }
            }
            Event::Empty(element) => {
                saw_root = true;
                if !capturing && local_name(element.name().as_ref()) == target {
                    out.push(String::new());
                }
            }
            Event::Text(value) => {
                if capturing {
                    current.push_str(value.as_ref());
                }
            }
            Event::GeneralRef(reference) => {
                if capturing {
                    if let Ok(Some(ch)) = reference.resolve_char_ref() {
                        current.push(ch);
                    } else {
                        match reference.into_inner().as_ref() {
                            "amp" => current.push('&'),
                            "lt" => current.push('<'),
                            "gt" => current.push('>'),
                            "quot" => current.push('"'),
                            "apos" => current.push('\''),
                            _ => {}
                        }
                    }
                }
            }
            Event::End(_) if capturing => {
                if depth == 0 {
                    capturing = false;
                    out.push(std::mem::take(&mut current));
                } else {
                    depth -= 1;
                }
            }
            _ => {}
        }

        buf.clear();
    }

    if !saw_root {
        return Err(XmlError::Malformed(
            "el documento no contiene ningún elemento raíz".to_string(),
        ));
    }

    Ok(out)
}

/// Parsed `MKCALENDAR` request body.
///
/// `MKCALENDAR` may carry a `<d:set><d:prop>` block with the properties of the
/// calendar to create; OxiDav only consumes the human-readable `displayname`
/// (and tolerates a `calendar-description` without using it yet).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseMkcalendar {
    /// `<d:displayname>` text content, when present.
    pub display_name: Option<String>,
    /// `<c:calendar-description>` text content, when present.
    pub description: Option<String>,
}

/// Parse a `MKCALENDAR` request body.
///
/// Entity, numeric-character and the five predefined XML references are decoded
/// into the captured text. Any DTD / `<!DOCTYPE ...>` is refused.
///
/// # Errors
///
/// [`XmlError::Malformed`] for bad XML, [`XmlError::DoctypeNotAllowed`] for any
/// DTD / `<!DOCTYPE ...>`.
pub fn parse_mkcalendar(body: &str) -> Result<ParseMkcalendar, XmlError> {
    let mut reader = NsReader::from_str(body);
    let mut buf = Vec::new();
    let mut result = ParseMkcalendar::default();
    let mut text = String::new();
    // `Some(true)` while capturing a description, `Some(false)` for a display
    // name, `None` outside both elements.
    let mut capture: Option<bool> = None;
    let mut saw_root = false;

    loop {
        let (_resolved, event) = reader
            .read_resolved_event_into(&mut buf)
            .map_err(|err| XmlError::Malformed(err.to_string()))?;

        match event {
            Event::Eof => break,
            Event::DocType(_) => return Err(XmlError::DoctypeNotAllowed),
            Event::Start(element) => {
                saw_root = true;
                match local_name(element.name().as_ref()).as_str() {
                    "displayname" => {
                        capture = Some(false);
                        text.clear();
                    }
                    "calendar-description" => {
                        capture = Some(true);
                        text.clear();
                    }
                    _ => {}
                }
            }
            Event::Empty(element) => {
                saw_root = true;
                let _ = element;
            }
            Event::Text(value) => {
                if capture.is_some() {
                    text.push_str(value.as_ref());
                }
            }
            Event::GeneralRef(reference) => {
                if capture.is_some() {
                    if let Ok(Some(ch)) = reference.resolve_char_ref() {
                        text.push(ch);
                    } else {
                        match reference.into_inner().as_ref() {
                            "amp" => text.push('&'),
                            "lt" => text.push('<'),
                            "gt" => text.push('>'),
                            "quot" => text.push('"'),
                            "apos" => text.push('\''),
                            _ => {}
                        }
                    }
                }
            }
            Event::End(element) => match local_name(element.name().as_ref()).as_str() {
                "displayname" if capture == Some(false) => {
                    result.display_name = Some(text.clone());
                    capture = None;
                }
                "calendar-description" if capture == Some(true) => {
                    result.description = Some(text.clone());
                    capture = None;
                }
                _ => {}
            },
            _ => {}
        }

        buf.clear();
    }

    if !saw_root {
        return Err(XmlError::Malformed(
            "el documento no contiene ningún elemento raíz".to_string(),
        ));
    }

    Ok(result)
}

/// Strip any `prefix:` from a raw qualified name and return the local part.
fn local_name(raw: &str) -> String {
    let local = match raw.as_bytes().iter().rposition(|&byte| byte == b':') {
        Some(index) => &raw[index + 1..],
        None => raw,
    };
    local.to_string()
}

/// The resolved namespace URI of an element, or an empty string when unbound.
fn namespace_of(resolved: &ResolveResult<'_>) -> String {
    match resolved {
        ResolveResult::Bound(Namespace(uri)) => (*uri).to_string(),
        _ => String::new(),
    }
}

/// Whether an element resolves to the `DAV:` namespace (or has no namespace).
///
/// Lenient by design: a client that binds the DAV vocabulary as the default
/// namespace resolves to `DAV:` and one that omits the declaration entirely
/// resolves to no namespace; both are accepted for the `sync-collection`
/// children, while a foreign namespace (e.g. `c:sync-token`) is ignored.
fn is_dav_namespace(resolved: &ResolveResult<'_>) -> bool {
    matches!(resolved, ResolveResult::Bound(Namespace(uri)) if *uri == NS_DAV)
        || matches!(resolved, ResolveResult::Unbound)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect every element `(namespace, local_name)` in a document.
    fn elements(xml: &str) -> Vec<(String, String)> {
        let mut reader = NsReader::from_str(xml);
        let mut buf = Vec::new();
        let mut seen = Vec::new();
        loop {
            let (resolved, event) = reader
                .read_resolved_event_into(&mut buf)
                .expect("well-formed XML");
            match event {
                Event::Eof => break,
                Event::Start(element) | Event::Empty(element) => {
                    seen.push((namespace_of(&resolved), local_name(element.name().as_ref())))
                }
                _ => {}
            }
            buf.clear();
        }
        seen
    }

    #[test]
    fn multistatus_round_trips_through_a_parser() {
        let responses = vec![
            DavResponse {
                href: "/dav/calendars/alice/".to_string(),
                status: 200,
                props: vec![
                    DavProp {
                        ns: DavNs::Dav,
                        name: "displayname".to_string(),
                        value: PropValue::Text("Alice & Bob <team>".to_string()),
                    },
                    DavProp {
                        ns: DavNs::Dav,
                        name: "resourcetype".to_string(),
                        value: PropValue::Xml("<d:collection/>".to_string()),
                    },
                    DavProp {
                        ns: DavNs::CalDav,
                        name: "calendar-home-set".to_string(),
                        value: PropValue::Empty,
                    },
                ],
                not_found: Vec::new(),
            },
            DavResponse {
                href: "/dav/calendars/alice/missing/".to_string(),
                status: 404,
                props: Vec::new(),
                not_found: Vec::new(),
            },
        ];

        let xml = multistatus(&responses);

        // The root declares both vocabularies.
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"));
        assert!(xml.contains(&format!(
            "<d:multistatus xmlns:d=\"{NS_DAV}\" xmlns:c=\"{NS_CALDAV}\" xmlns:card=\"{NS_CARDDAV}\" xmlns:cs=\"{NS_CALENDARSERVER}\">"
        )));
        // Text is escaped, not injected.
        assert!(xml.contains("Alice &amp; Bob &lt;team&gt;"));
        // Per-resource error uses a response-level status.
        assert!(xml.contains("<d:status>HTTP/1.1 404 Not Found</d:status>"));

        // It is valid XML: an independent parser recovers every element with the
        // right namespace.
        let parsed = elements(&xml);
        assert!(parsed.contains(&(NS_DAV.to_string(), "multistatus".to_string())));
        assert!(parsed.contains(&(NS_DAV.to_string(), "href".to_string())));
        assert!(parsed.contains(&(NS_DAV.to_string(), "displayname".to_string())));
        assert!(
            parsed.contains(&(NS_CALDAV.to_string(), "calendar-home-set".to_string())),
            "caldav property must resolve to its namespace: {parsed:?}"
        );
    }

    #[test]
    fn multistatus_emits_a_404_propstat_for_missing_props() {
        let responses = vec![DavResponse {
            href: "/dav/calendars/alice/work/".to_string(),
            status: 200,
            props: vec![DavProp {
                ns: DavNs::Dav,
                name: "sync-token".to_string(),
                value: PropValue::Text("http://oxidav/ns/sync/7".to_string()),
            }],
            not_found: vec![DavProp {
                ns: DavNs::CalDav,
                name: "calendar-timezone".to_string(),
                value: PropValue::Empty,
            }],
        }];

        let xml = multistatus(&responses);

        assert!(
            xml.contains("<d:status>HTTP/1.1 200 OK</d:status>"),
            "{xml}"
        );
        assert!(
            xml.contains("<d:status>HTTP/1.1 404 Not Found</d:status>"),
            "{xml}"
        );
        // The missing property is present as an empty element in the CalDAV ns.
        let parsed = elements(&xml);
        assert!(parsed.contains(&(NS_DAV.to_string(), "sync-token".to_string())));
        assert!(parsed.contains(&(NS_CALDAV.to_string(), "calendar-timezone".to_string())));
    }

    #[test]
    fn multistatus_with_no_responses_is_valid() {
        let xml = multistatus(&[]);
        assert!(xml.contains("<d:multistatus"));
        assert!(xml.ends_with("</d:multistatus>"));
        assert!(elements(&xml).contains(&(NS_DAV.to_string(), "multistatus".to_string())));
    }

    #[test]
    fn parses_a_propfind_body() {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
            <d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
              <d:prop>
                <d:displayname/>
                <d:resourcetype/>
                <d:getetag/>
                <c:calendar-home-set/>
              </d:prop>
            </d:propfind>"#;

        let parsed = parse_propfind(body).expect("parse");

        assert!(!parsed.all_props);
        assert!(!parsed.prop_names);
        assert_eq!(parsed.props.len(), 4);
        assert_eq!(
            parsed.props[0],
            PropName {
                namespace: NS_DAV.to_string(),
                name: "displayname".to_string()
            }
        );
        assert_eq!(
            parsed.props[3],
            PropName {
                namespace: NS_CALDAV.to_string(),
                name: "calendar-home-set".to_string()
            }
        );
    }

    #[test]
    fn parses_allprop_and_propname() {
        let all = parse_propfind(r#"<d:propfind xmlns:d="DAV:"><d:allprop/></d:propfind>"#)
            .expect("parse");
        assert!(all.all_props);
        assert!(all.props.is_empty());

        let names = parse_propfind(r#"<d:propfind xmlns:d="DAV:"><d:propname/></d:propfind>"#)
            .expect("parse");
        assert!(names.prop_names);
        assert!(names.props.is_empty());
    }

    #[test]
    fn parses_arbitrary_client_prefixes() {
        let body =
            r#"<cal:propfind xmlns:cal="DAV:"><cal:prop><cal:getetag/></cal:prop></cal:propfind>"#;
        let parsed = parse_propfind(body).expect("parse");
        assert_eq!(
            parsed.props,
            vec![PropName {
                namespace: NS_DAV.to_string(),
                name: "getetag".to_string()
            }]
        );
    }

    #[test]
    fn parses_a_calendar_query_with_time_range() {
        let body = r#"<?xml version="1.0"?>
            <c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
              <d:prop><d:getetag/><c:calendar-data/></d:prop>
              <c:filter>
                <c:comp-filter name="VCALENDAR">
                  <c:comp-filter name="VEVENT">
                    <c:time-range start="20240101T000000Z" end="20240201T000000Z"/>
                  </c:comp-filter>
                </c:comp-filter>
              </c:filter>
            </c:calendar-query>"#;

        let parsed = parse_calendar_query(body).expect("parse");
        let range = parsed.time_range.expect("time range");
        assert_eq!(range.start_ms, 1_704_067_200_000);
        assert_eq!(range.end_ms, 1_706_745_600_000);
    }

    #[test]
    fn calendar_query_without_time_range_is_none() {
        let body = r#"<c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav"/>"#;
        let parsed = parse_calendar_query(body).expect("parse");
        assert_eq!(parsed.time_range, None);
    }

    #[test]
    fn rejects_a_malformed_time_range() {
        let body = r#"<c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav">
            <c:time-range start="nonsense" end="20240201T000000Z"/></c:calendar-query>"#;
        assert!(matches!(
            parse_calendar_query(body),
            Err(XmlError::InvalidTimeRange)
        ));
    }

    #[test]
    fn parses_a_time_range_with_only_a_start() {
        let body = r#"<c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav">
            <c:time-range start="20240101T000000Z"/></c:calendar-query>"#;
        let parsed = parse_calendar_query(body).expect("parse");
        let range = parsed.time_range.expect("time range");
        assert_eq!(range.start_ms, 1_704_067_200_000);
        assert_eq!(range.end_ms, TimeRange::UNBOUNDED_END);
    }

    #[test]
    fn parses_a_time_range_with_only_an_end() {
        let body = r#"<c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav">
            <c:time-range end="20240201T000000Z"/></c:calendar-query>"#;
        let parsed = parse_calendar_query(body).expect("parse");
        let range = parsed.time_range.expect("time range");
        assert_eq!(range.start_ms, TimeRange::UNBOUNDED_START);
        assert_eq!(range.end_ms, 1_706_745_600_000);
    }

    #[test]
    fn rejects_a_time_range_with_no_bounds() {
        let body = r#"<c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav">
            <c:time-range/></c:calendar-query>"#;
        assert!(matches!(
            parse_calendar_query(body),
            Err(XmlError::InvalidTimeRange)
        ));
    }

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

    #[test]
    fn rejects_malformed_xml() {
        assert!(matches!(
            parse_propfind("<d:propfind"),
            Err(XmlError::Malformed(_))
        ));
        assert!(matches!(
            parse_calendar_query("not xml at all"),
            Err(XmlError::Malformed(_))
        ));
    }

    #[test]
    fn rejects_a_doctype_with_external_entity_no_xxe() {
        // Classic XXE: an external entity that would read a local file.
        let xxe = r#"<?xml version="1.0"?>
            <!DOCTYPE propfind [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
            <d:propfind xmlns:d="DAV:"><d:prop><d:displayname>&xxe;</d:displayname></d:prop></d:propfind>"#;
        assert!(matches!(
            parse_propfind(xxe),
            Err(XmlError::DoctypeNotAllowed)
        ));
    }

    #[test]
    fn rejects_a_doctype_with_billion_laughs() {
        let laughs = r#"<?xml version="1.0"?>
            <!DOCTYPE lolz [
              <!ENTITY lol "lol">
              <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
              <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
            ]>
            <c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav">&lol3;</c:calendar-query>"#;
        assert!(matches!(
            parse_calendar_query(laughs),
            Err(XmlError::DoctypeNotAllowed)
        ));
    }

    #[test]
    fn parses_a_mkcalendar_body() {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
            <c:mkcalendar xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
              <d:set>
                <d:prop>
                  <d:displayname>Trabajo &amp; viajes</d:displayname>
                  <c:calendar-description>Agenda laboral</c:calendar-description>
                </d:prop>
              </d:set>
            </c:mkcalendar>"#;

        let parsed = parse_mkcalendar(body).expect("parse");
        assert_eq!(parsed.display_name.as_deref(), Some("Trabajo & viajes"));
        assert_eq!(parsed.description.as_deref(), Some("Agenda laboral"));
    }

    #[test]
    fn parses_a_mkcalendar_body_without_properties() {
        let body = r#"<c:mkcalendar xmlns:c="urn:ietf:params:xml:ns:caldav"/>"#;
        let parsed = parse_mkcalendar(body).expect("parse");
        assert_eq!(parsed, ParseMkcalendar::default());
    }

    #[test]
    fn mkcalendar_rejects_a_doctype() {
        let body = r#"<!DOCTYPE x [<!ENTITY y "z">]>
            <c:mkcalendar xmlns:c="urn:ietf:params:xml:ns:caldav"><d:displayname>&y;</d:displayname></c:mkcalendar>"#;
        assert!(matches!(
            parse_mkcalendar(body),
            Err(XmlError::DoctypeNotAllowed)
        ));
    }

    #[test]
    fn calendarserver_namespace_is_declared_and_resolves() {
        let responses = vec![DavResponse {
            href: "/dav/calendars/alice/work/".to_string(),
            status: 200,
            props: vec![DavProp {
                ns: DavNs::Calendarserver,
                name: "getctag".to_string(),
                value: PropValue::Text("\"7\"".to_string()),
            }],
            not_found: Vec::new(),
        }];

        let xml = multistatus(&responses);
        assert!(xml.contains("xmlns:cs=\"http://calendarserver.org/ns/\""));
        assert!(
            elements(&xml).contains(&(NS_CALENDARSERVER.to_string(), "getctag".to_string())),
            "getctag must resolve to its namespace: {xml}"
        );
    }

    #[test]
    fn parses_an_addressbook_query_text_match() {
        let body = r#"<?xml version="1.0"?>
            <card:addressbook-query xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
              <d:prop><d:getetag/><card:address-data/></d:prop>
              <card:filter>
                <card:prop-filter name="FN">
                  <card:text-match match-type="contains">Ada &amp; Co</card:text-match>
                </card:prop-filter>
              </card:filter>
            </card:addressbook-query>"#;

        let parsed = parse_addressbook_query(body).expect("parse");
        assert_eq!(parsed.terms, vec!["Ada & Co".to_string()]);
    }

    #[test]
    fn addressbook_query_without_filter_has_no_terms() {
        let body = r#"<card:addressbook-query xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
            <d:prop><card:address-data/></d:prop></card:addressbook-query>"#;
        let parsed = parse_addressbook_query(body).expect("parse");
        assert!(parsed.terms.is_empty());
    }

    #[test]
    fn parses_an_addressbook_multiget_href_list() {
        let body = r#"<card:addressbook-multiget xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
            <d:prop><d:getetag/><card:address-data/></d:prop>
            <d:href>/dav/addressbooks/alice/work/ada%40oxidav.vcf</d:href>
            <d:href>/dav/addressbooks/alice/work/grace%40oxidav.vcf</d:href>
          </card:addressbook-multiget>"#;
        let parsed = parse_addressbook_multiget(body).expect("parse");
        assert_eq!(
            parsed.hrefs,
            vec![
                "/dav/addressbooks/alice/work/ada%40oxidav.vcf".to_string(),
                "/dav/addressbooks/alice/work/grace%40oxidav.vcf".to_string(),
            ]
        );
    }

    #[test]
    fn dispatches_a_carddav_report_on_its_root() {
        let query = r#"<card:addressbook-query xmlns:card="urn:ietf:params:xml:ns:carddav"/>"#;
        assert!(matches!(
            parse_carddav_report(query),
            Ok(CardDavReport::Query(_))
        ));

        let multiget =
            r#"<card:addressbook-multiget xmlns:card="urn:ietf:params:xml:ns:carddav"/>"#;
        assert!(matches!(
            parse_carddav_report(multiget),
            Ok(CardDavReport::Multiget(_))
        ));

        let unknown = r#"<c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav"/>"#;
        assert!(matches!(
            parse_carddav_report(unknown),
            Err(XmlError::Malformed(_))
        ));
    }

    #[test]
    fn carddav_report_rejects_a_doctype_no_xxe() {
        let xxe = r#"<!DOCTYPE x [<!ENTITY y SYSTEM "file:///etc/passwd">]>
            <card:addressbook-multiget xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
            <d:href>&y;</d:href></card:addressbook-multiget>"#;
        assert!(matches!(
            parse_carddav_report(xxe),
            Err(XmlError::DoctypeNotAllowed)
        ));
    }
}
