//! F07 — parsing of DAV XML request bodies: `PROPFIND`, the CalDAV / CardDAV
//! `REPORT`s (`sync-collection`, `calendar-query`, `addressbook-query`,
//! `addressbook-multiget`) and `MKCALENDAR`.
//!
//! Reading is done with `quick-xml`. The anti-XXE / anti-billion-laughs policy and
//! the namespace handling are documented once in [`crate::dav::xml`] (its
//! `# Security` section); this submodule only implements that policy verbatim.
//!
//! The `207 Multi-Status` serialiser lives in the sibling `write` submodule, and
//! the shared model types (`Parse*`, `TimeRange`, `XmlError`, …) stay in the parent.

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;

use super::{
    CardDavReport, NS_DAV, ParseAddressbookMultiget, ParseAddressbookQuery, ParseCalendarQuery,
    ParsePropfind, ParseSyncCollection, PropName, TimeRange, XmlError,
};
use crate::dav::ics;

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
pub(super) fn local_name(raw: &str) -> String {
    let local = match raw.as_bytes().iter().rposition(|&byte| byte == b':') {
        Some(index) => &raw[index + 1..],
        None => raw,
    };
    local.to_string()
}

/// The resolved namespace URI of an element, or an empty string when unbound.
pub(super) fn namespace_of(resolved: &ResolveResult<'_>) -> String {
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
    use crate::dav::xml::{CardDavReport, NS_CALDAV, NS_DAV, PropName, TimeRange, XmlError};

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
