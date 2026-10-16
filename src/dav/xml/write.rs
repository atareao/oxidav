//! F07 — serialisation of DAV XML: the `207 Multi-Status` document and the
//! escaping of text nodes.
//!
//! Writing is done by hand (no XML serialiser), with every text node escaped.
//! The parser for request bodies lives in the parent module.

use super::{DavProp, DavResponse, NS_CALDAV, NS_CALENDARSERVER, NS_CARDDAV, NS_DAV, PropValue};

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

#[cfg(test)]
mod tests {
    use super::super::DavNs;
    use super::super::parse::{local_name, namespace_of};
    use super::*;
    use quick_xml::events::Event;
    use quick_xml::reader::NsReader;

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
}
