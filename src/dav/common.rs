//! F08 — shared DAV helpers used by both the CalDAV ([`super::caldav`]) and
//! CardDAV ([`super::carddav`]) trees.
//!
//! These helpers were factored out of `caldav.rs` so the two protocol trees
//! reuse a single implementation of the cross-cutting DAV concerns:
//!
//! * `Depth` handling,
//! * owner / `403` enforcement,
//! * RFC 7232 conditional requests (`If-Match` / `If-None-Match`),
//! * opaque `ETag` headers,
//! * caller-supplied collection-id sanitisation and UUID minting,
//! * `PROPFIND` body parsing and property negotiation.
//!
//! Duplicating any of this between the two protocols would let their behaviours
//! drift — e.g. one tree honouring a weak validator for `If-Match` and the other
//! not. The protocol-specific parts (resource types, property sets, collection
//! kinds, payload parsers) stay in each tree.

use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use percent_encoding::{AsciiSet, CONTROLS};
use rand::RngCore;

use crate::auth::oidc::AuthUser;
use crate::db::{
    Db, ResourceKind, changes_since, get_calendar_object, get_card_object, next_sync_token,
};

use super::DavError;
use super::xml::{self, DavNs, DavProp, DavResponse, ParsePropfind, PropName, PropValue};

/// Maximum accepted length of a collection display name, in characters.
pub(crate) const MAX_NAME_LEN: usize = 255;

/// Maximum accepted length of a caller-supplied collection id (the
/// `{collection}` path segment adopted by `MKCALENDAR` / `MKCOL`), in
/// characters.
pub(crate) const MAX_ID_LEN: usize = 255;

/// Server-local prefix of the opaque `sync-token` URI.
pub(crate) const SYNC_TOKEN_URI_PREFIX: &str = "http://oxidav/ns/sync/";

/// Characters percent-encoded when a resource `UID` is emitted as a URL path
/// segment (RFC 3986 `pchar`): the C0 controls, everything that is not a `pchar`
/// — with `%` added so the encoding stays unambiguous — plus the path/fragment
/// query delimiters `/`, `#` and `?`. Bytes `>= 0x80` (non-ASCII) are always
/// encoded by [`utf8_percent_encode`](percent_encoding::utf8_percent_encode).
/// Note that `@`, `.` and `+` are *kept*: they are legal in a `pchar` and common
/// in `UID`s.
///
/// Shared by the CalDAV and CardDAV trees so a stored `UID` carrying spaces,
/// `#`, `/`, `?` or non-ASCII bytes always yields a URI-valid `href` in both.
pub(crate) const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// Requested PROPFIND recursion depth, reduced to the two values the server
/// actually distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Depth {
    /// `0`: the addressed resource only.
    Zero,
    /// `1` (and the default): the resource plus its immediate members.
    One,
}

/// Map the `Depth` header to the two depths the server distinguishes.
///
/// Missing, `infinity` and any unrecognised value are treated as `1` (see the
/// CalDAV module docs); only an explicit `0` narrows the response to the target.
pub(crate) fn requested_depth(headers: &HeaderMap) -> Depth {
    match headers.get("depth").and_then(|value| value.to_str().ok()) {
        Some("0") => Depth::Zero,
        _ => Depth::One,
    }
}

/// Validate the `Depth` header of a `REPORT sync-collection` (RFC 6578 §3.2).
///
/// RFC 6578 pins the `sync-collection` report to `Depth: 0`: it is a
/// collection-level query that reports the collection's own members, so any
/// other depth is out of contract. A missing header is accepted (the report
/// defaults to `0`); an explicit `0` is accepted; anything else — including
/// `1` and `infinity` — is a [`DavError::BadRequest`] rather than being silently
/// coerced, so a client never gets a depth-`1` sweep it did not ask for.
///
/// # Errors
///
/// [`DavError::BadRequest`] when `Depth` is present and not `0`.
pub(crate) fn validate_sync_depth(headers: &HeaderMap) -> Result<(), DavError> {
    match headers.get("depth").and_then(|value| value.to_str().ok()) {
        None | Some("0") => Ok(()),
        Some(_) => Err(DavError::BadRequest(
            "sync-collection sólo admite Depth: 0".to_string(),
        )),
    }
}

/// Reject a request whose `{username}` is not the authenticated subject.
pub(crate) fn ensure_owner(username: &str, user: &AuthUser) -> Result<(), DavError> {
    if username == user.sub {
        Ok(())
    } else {
        Err(DavError::Forbidden)
    }
}

/// How an `If-Match` / `If-None-Match` entity-tag is compared (RFC 7232 §2.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EtagCompare {
    /// Strong comparison: a weak validator (`W/"…"`) can **never** match.
    /// Used for `If-Match`.
    Strong,
    /// Weak comparison: the `W/` prefix is ignored and only the opaque part is
    /// compared. Used for `If-None-Match`.
    Weak,
}

/// Evaluate the `PUT` / `DELETE` preconditions before touching the database.
///
/// `If-None-Match` fails the request when it *matches* the current state (its
/// common form, `*`, therefore blocks overwriting/deleting an existing resource)
/// and is evaluated with a **weak** comparison; `If-Match` fails it when it does
/// *not* match and is evaluated with a **strong** comparison (so a weak
/// validator in the header can never satisfy it). A missing header is a no-op; a
/// header that is present but not a valid (ASCII, non-empty) entity-tag list is
/// a `400 Bad Request`, never a silent allow.
///
/// `existing_etag` is the current opaque entity-tag of the resource, or `None`
/// when it does not exist.
pub(crate) fn check_conditionals(
    headers: &HeaderMap,
    existing_etag: Option<&str>,
) -> Result<(), DavError> {
    if let Some(value) = header_value(headers, header::IF_NONE_MATCH)?
        && etag_condition_matches(&value, existing_etag, EtagCompare::Weak)
    {
        return Err(DavError::PreconditionFailed);
    }
    if let Some(value) = header_value(headers, header::IF_MATCH)?
        && !etag_condition_matches(&value, existing_etag, EtagCompare::Strong)
    {
        return Err(DavError::PreconditionFailed);
    }
    Ok(())
}

/// Whether an `If-Match` / `If-None-Match` header value matches the resource.
///
/// `*` matches iff the resource exists (has an entity-tag). Otherwise the value
/// is a comma-separated list of entity-tags compared as `mode` dictates:
/// **strong** comparisons reject any weak validator (`W/"…"`), while **weak**
/// comparisons strip the `W/` prefix and compare only the opaque part. The
/// quoted `etag` is matched byte-for-byte. A missing resource never matches a
/// non-`*` value.
fn etag_condition_matches(value: &str, existing_etag: Option<&str>, mode: EtagCompare) -> bool {
    let value = value.trim();
    let Some(etag) = existing_etag else {
        // A missing resource never matches a conditional (not even `*`).
        return false;
    };
    if value == "*" {
        return true;
    }
    value.split(',').any(|candidate| {
        let candidate = candidate.trim();
        match mode {
            // A weak validator can never satisfy a strong comparison.
            EtagCompare::Strong => match candidate.strip_prefix("W/") {
                Some(_) => false,
                None => candidate == etag,
            },
            EtagCompare::Weak => {
                let candidate = candidate.strip_prefix("W/").unwrap_or(candidate);
                candidate == etag
            }
        }
    })
}

/// Build the `ETag` response header from a stored opaque entity-tag.
pub(crate) fn etag_header(etag: &str) -> Result<HeaderValue, DavError> {
    HeaderValue::from_str(etag).map_err(|_| DavError::Internal)
}

/// Read a conditional header as an owned `String`.
///
/// `Ok(None)` when the header is absent; `Err(BadRequest)` when it is present
/// but not a valid ASCII, non-empty header value. A malformed `If-Match` /
/// `If-None-Match` therefore fails the request instead of being silently
/// skipped and letting a write through.
fn header_value(headers: &HeaderMap, name: header::HeaderName) -> Result<Option<String>, DavError> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| DavError::BadRequest("cabecera condicional inválida".to_string()))?;
    if value.trim().is_empty() {
        return Err(DavError::BadRequest(
            "cabecera condicional vacía".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

/// Validate and normalise a caller-supplied collection id.
///
/// A `MKCALENDAR` / `MKCOL` on a collection path adopts that path segment as the
/// new collection id, so the segment becomes part of both the stored primary key
/// and the emitted `Location` URL. It is therefore restricted to a conservative,
/// path-safe charset — ASCII alphanumerics plus `- _ . @ +` — and rejected when
/// it is empty, `.` / `..`, over [`MAX_ID_LEN`], or carries any other character
/// (notably `/`, whitespace or control characters). Those are
/// [`DavError::BadRequest`]; they can never be stored or reflected into a URL.
pub(crate) fn sanitize_collection_id(segment: &str) -> Result<String, DavError> {
    let id = segment.trim();
    let valid = !id.is_empty()
        && id != "."
        && id != ".."
        && id.chars().count() <= MAX_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+'));
    if valid {
        Ok(id.to_string())
    } else {
        Err(DavError::BadRequest(
            "identificador de colección inválido".to_string(),
        ))
    }
}

/// Generate a fresh RFC 4122 **version 4** UUID string.
///
/// Uses the CSPRNG already in the dependency graph (`rand`), without adding a
/// dedicated crate; the format matches `collections.id` as produced by F06.
pub(crate) fn new_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 1
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Parse an optional `PROPFIND` body.
///
/// An empty body is the RFC 4918 §9.1 default of `allprop`; a non-empty one must
/// be valid UTF-8 XML with no DTD (the parser refuses `<!DOCTYPE ...>`), else it
/// is a `400 Bad Request`.
pub(crate) fn parse_propfind_body(body: &[u8]) -> Result<ParsePropfind, DavError> {
    if body.is_empty() {
        return Ok(ParsePropfind {
            all_props: true,
            ..ParsePropfind::default()
        });
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| DavError::BadRequest("cuerpo no es UTF-8".to_string()))?;
    xml::parse_propfind(text)
        .map_err(|_| DavError::BadRequest("cuerpo PROPFIND inválido".to_string()))
}

/// Build a `200` response for `href`, honouring the requested property set.
///
/// * `propname` → the names of every property the resource defines, values
///   omitted (RFC 4918 §9.1).
/// * `allprop`, or a body that requested nothing explicit, → every property the
///   resource defines, with values.
/// * an explicit `<d:prop>` → only the requested properties the resource
///   defines; the rest become `not_found` (their own `404` `propstat`). A
///   requested property whose namespace OxiDav cannot serialise is dropped
///   rather than reported (see [`xml::dav_ns_from_uri`]).
pub(crate) fn negotiate(
    href: String,
    all_props: Vec<DavProp>,
    request: &ParsePropfind,
) -> DavResponse {
    if request.prop_names {
        let props = all_props
            .into_iter()
            .map(|prop| DavProp {
                ns: prop.ns,
                name: prop.name,
                value: PropValue::Empty,
            })
            .collect();
        return DavResponse {
            href,
            status: 200,
            props,
            not_found: Vec::new(),
        };
    }

    if request.props.is_empty() {
        // No explicit `<d:prop>`: `allprop` (the body may have said so or been
        // empty, which defaults to `allprop`).
        return DavResponse {
            href,
            status: 200,
            props: all_props,
            not_found: Vec::new(),
        };
    }

    let mut props = Vec::new();
    let mut not_found = Vec::new();
    for requested in &request.props {
        match find_prop(&all_props, requested) {
            Some(found) => props.push(found.clone()),
            None => {
                // Only properties in a namespace we can serialise back can be
                // reported as missing; unknown-namespace requests are dropped.
                if let Some(ns) = xml::dav_ns_from_uri(&requested.namespace) {
                    not_found.push(DavProp {
                        ns,
                        name: requested.name.clone(),
                        value: PropValue::Empty,
                    });
                }
            }
        }
    }

    DavResponse {
        href,
        status: 200,
        props,
        not_found,
    }
}

/// Find the property in `all` that a client requested, matching on both its
/// resolved namespace and its local name.
fn find_prop<'a>(all: &'a [DavProp], requested: &PropName) -> Option<&'a DavProp> {
    let ns = xml::dav_ns_from_uri(&requested.namespace)?;
    all.iter()
        .find(|prop| prop.ns == ns && prop.name == requested.name)
}

/// `<d:resourcetype><d:collection/></d:resourcetype>`.
pub(crate) fn collection_resourcetype() -> DavProp {
    DavProp {
        ns: DavNs::Dav,
        name: "resourcetype".to_string(),
        value: PropValue::Xml("<d:collection/>".to_string()),
    }
}

/// `<d:supported-report-set>` advertising the RFC 6578 `sync-collection` report.
///
/// RFC 6578 §3.2 lets a server announce `sync-collection` through this
/// `DAV:`-namespaced property, so a client can discover the incremental-sync
/// capability of a collection before issuing a `REPORT`. Shared by the CalDAV
/// and CardDAV collection property sets so the advertisement cannot drift.
pub(crate) fn supported_report_set() -> DavProp {
    DavProp {
        ns: DavNs::Dav,
        name: "supported-report-set".to_string(),
        value: PropValue::Xml(
            "<d:supported-report><d:report><d:sync-collection/></d:report></d:supported-report>"
                .to_string(),
        ),
    }
}

/// A text-valued property in `ns`.
pub(crate) fn text_prop(ns: DavNs, name: &str, value: &str) -> DavProp {
    DavProp {
        ns,
        name: name.to_string(),
        value: PropValue::Text(value.to_string()),
    }
}

/// Append `line` to `out` with a CRLF terminator, folding at 75 octets.
///
/// iCalendar (RFC 5545 §3.1) and vCard (RFC 6350 §3.2) share the same
/// content-line folding rule: a line longer than 75 octets is wrapped by
/// inserting a CRLF followed by a single space, which is counted towards the
/// continuation line. The split is driven by character width, so it never lands
/// inside a multi-byte UTF-8 character.
///
/// Shared by [`crate::dav::ics::build_ics`] and [`crate::dav::vcf::build_vcf`]
/// so the two serialisers cannot drift on folding.
pub(crate) fn fold_content_line(line: &str, out: &mut String) {
    let mut current_len = 0usize;
    for ch in line.chars() {
        let ch_len = ch.len_utf8();
        // A continuation line carries a leading space, counted in the 75.
        if current_len + ch_len > 75 {
            out.push_str("\r\n ");
            current_len = 1;
        }
        out.push(ch);
        current_len += ch_len;
    }
    out.push_str("\r\n");
}

// ---------------------------------------------------------------------------
// F09 — WebDAV sync-collection (RFC 6578)
// ---------------------------------------------------------------------------

/// Build the opaque `sync-token` URI for a raw counter value.
///
/// The token is deliberately opaque to clients (RFC 6578 §3.1): OxiDav exposes
/// the monotonic `collections.sync_token` integer only wrapped in a private URI
/// namespace, so a client can never construct or interpret one on its own.
#[must_use]
pub(crate) fn format_sync_token(value: i64) -> String {
    format!("{SYNC_TOKEN_URI_PREFIX}{value}")
}

/// Errors produced while decoding an opaque `sync-token`.
///
/// A malformed token is a client error the handler maps to `403` with the DAV
/// `valid-sync-token` precondition (see [`report_sync_collection`]); it is kept
/// as its own type so the token helpers stay free of HTTP concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SyncTokenError {
    /// The token does not start with [`SYNC_TOKEN_URI_PREFIX`].
    #[error("el sync-token no tiene el prefijo esperado")]
    BadPrefix,
    /// The part after the prefix is not a valid (non-negative) integer.
    #[error("el sync-token no es un entero válido")]
    NotAnInteger,
}

/// Decode an opaque `sync-token` URI back to its raw counter value.
///
/// # Errors
///
/// [`SyncTokenError::BadPrefix`] when the token does not carry
/// [`SYNC_TOKEN_URI_PREFIX`], and [`SyncTokenError::NotAnInteger`] when the
/// suffix is missing, negative or not a base-10 integer.
pub(crate) fn parse_sync_token(token: &str) -> Result<i64, SyncTokenError> {
    let suffix = token
        .strip_prefix(SYNC_TOKEN_URI_PREFIX)
        .ok_or(SyncTokenError::BadPrefix)?;
    let value = suffix
        .parse::<i64>()
        .map_err(|_| SyncTokenError::NotAnInteger)?;
    if value < 0 {
        return Err(SyncTokenError::NotAnInteger);
    }
    Ok(value)
}

/// Requested `sync-level` of a `sync-collection` report (RFC 6578 §3.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SyncLevel {
    /// `sync-level` `1` (the default): the direct members of the collection.
    #[default]
    Members,
    /// `sync-level` `infinite`: rejected by the MVP with `400 Bad Request`.
    Infinite,
}

/// Parsed `REPORT sync-collection` request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SyncCollectionRequest {
    /// The `sync-token` supplied by the client, or `None` for an initial sync.
    pub sync_token: Option<String>,
    /// The requested `sync-level`.
    pub sync_level: SyncLevel,
    /// Properties requested inside `<d:prop>`.
    pub props: Vec<PropName>,
}

impl SyncCollectionRequest {
    /// Whether the client requested the property `name` in the namespace `ns`.
    ///
    /// Used to decide whether to embed the resource payload (`calendar-data` /
    /// `address-data`); with no `<d:prop>` nothing beyond `getetag` is returned.
    #[must_use]
    pub(crate) fn wants(&self, ns: DavNs, name: &str) -> bool {
        self.props
            .iter()
            .any(|prop| xml::dav_ns_from_uri(&prop.namespace) == Some(ns) && prop.name == name)
    }
}

/// Parse the body of a `REPORT sync-collection` request.
///
/// An empty body is a valid "initial sync of the default level" request. A
/// non-empty one must be UTF-8 XML whose root is `<d:sync-collection>` with no
/// DTD, else it is a `400 Bad Request`.
///
/// # Errors
///
/// [`DavError::BadRequest`] for a non-UTF-8 or malformed body, or an
/// unrecognised `sync-level`.
pub(crate) fn parse_sync_collection(body: &[u8]) -> Result<SyncCollectionRequest, DavError> {
    if body.is_empty() {
        return Ok(SyncCollectionRequest::default());
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| DavError::BadRequest("cuerpo no es UTF-8".to_string()))?;
    let parsed = xml::parse_sync_collection(text)
        .map_err(|_| DavError::BadRequest("cuerpo REPORT sync-collection inválido".to_string()))?;

    let sync_level = match parsed.sync_level.as_deref() {
        None | Some("1") => SyncLevel::Members,
        Some("infinite") => SyncLevel::Infinite,
        Some(_) => {
            return Err(DavError::BadRequest("sync-level no soportado".to_string()));
        }
    };
    // A present-but-empty `<d:sync-token/>` means the client has no token yet:
    // that is an initial sync, not an error. The captured text is trimmed so a
    // valid token wrapped in surrounding whitespace (`<d:sync-token> … </…>`) is
    // still accepted rather than failing the prefix check.
    let sync_token = parsed
        .sync_token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty());

    Ok(SyncCollectionRequest {
        sync_token,
        sync_level,
        props: parsed.props,
    })
}

/// Whether a `REPORT` body is a `sync-collection` (dispatched by root element).
///
/// Shared by the CalDAV and CardDAV handlers so the two trees cannot drift on
/// which report types they recognise. An empty body is **not** a sync (the
/// protocol-specific report keeps its own empty-body default).
///
/// # Errors
///
/// [`DavError::BadRequest`] for a non-UTF-8 or malformed body.
pub(crate) fn report_is_sync_collection(body: &[u8]) -> Result<bool, DavError> {
    if body.is_empty() {
        return Ok(false);
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| DavError::BadRequest("cuerpo no es UTF-8".to_string()))?;
    match xml::report_root_name(text) {
        Ok(name) => Ok(name == "sync-collection"),
        Err(_) => Err(DavError::BadRequest("cuerpo REPORT inválido".to_string())),
    }
}

/// Handle a `REPORT sync-collection` against a collection (RFC 6578).
///
/// Control flow shared by CalDAV and CardDAV; only the resource href and the
/// payload property differ per protocol and are supplied by the caller.
///
/// * **Initial sync** (no token): every current resource, each `200` with its
///   `getetag`.
/// * **Incremental sync** (token `N`): the most recent change per resource with
///   `sync_token > N`; `added`/`updated` are `200` with `getetag`, `deleted`
///   are tombstones reported with a response-level `404`.
/// * The multistatus closes with the collection's current `<d:sync-token>`.
///
/// `sync-level infinite` is rejected with `400`; a malformed or unknown token
/// with `403` (the `valid-sync-token` precondition). A token **ahead** of the
/// collection's current value is treated as unknown and likewise refused.
///
/// # Errors
///
/// [`DavError::BadRequest`], [`DavError::SyncTokenInvalid`],
/// [`DavError::Internal`], or the final [`DavError::MultiStatus`].
pub(crate) async fn report_sync_collection(
    db: &Db,
    kind: ResourceKind,
    username: &str,
    collection_id: &str,
    body: &[u8],
    resource_href: fn(&str, &str, &str) -> String,
) -> Result<Response, DavError> {
    let request = parse_sync_collection(body)?;
    if request.sync_level == SyncLevel::Infinite {
        return Err(DavError::BadRequest(
            "sync-level infinite no está soportado".to_string(),
        ));
    }

    let current = next_sync_token(db, collection_id)
        .await
        .map_err(|_| DavError::Internal)?;

    let since = match request.sync_token.as_deref() {
        None => None,
        Some(token) => {
            let value = parse_sync_token(token).map_err(|_| DavError::SyncTokenInvalid)?;
            if value > current {
                // A token the server never issued: the client's state is not
                // recognised, so the precondition fails (RFC 6578 §3.2).
                return Err(DavError::SyncTokenInvalid);
            }
            Some(value)
        }
    };

    let changes = changes_since(db, collection_id, kind, since)
        .await
        .map_err(|_| DavError::Internal)?;

    let (data_ns, data_name) = match kind {
        ResourceKind::Calendar => (DavNs::CalDav, "calendar-data"),
        ResourceKind::Card => (DavNs::CardDav, "address-data"),
    };
    let wants_data = request.wants(data_ns, data_name);

    let mut responses = Vec::with_capacity(changes.len());
    for change in &changes {
        let href = resource_href(username, collection_id, &change.resource_uid);
        match change.etag.as_deref() {
            // A live resource: `200` with its ETag, plus the payload if asked.
            Some(etag) => {
                let mut props = vec![text_prop(DavNs::Dav, "getetag", etag)];
                if wants_data
                    && let Some(payload) =
                        load_payload(db, kind, collection_id, &change.resource_uid).await?
                {
                    props.push(text_prop(data_ns, data_name, &payload));
                }
                responses.push(DavResponse {
                    href,
                    status: 200,
                    props,
                    not_found: Vec::new(),
                });
            }
            // A tombstone: the object is gone, so its href is reported `404`.
            None => responses.push(DavResponse {
                href,
                status: 404,
                props: Vec::new(),
                not_found: Vec::new(),
            }),
        }
    }

    let token = format_sync_token(current);
    Err(DavError::MultiStatus(xml::multistatus_with_sync_token(
        &responses, &token,
    )))
}

/// Load the raw stored payload of a resource, for the sync report's optional
/// `calendar-data` / `address-data` property.
///
/// `Ok(None)` when the object is absent (e.g. it was deleted concurrently after
/// the change log was read), so the payload is simply omitted.
async fn load_payload(
    db: &Db,
    kind: ResourceKind,
    collection_id: &str,
    uid: &str,
) -> Result<Option<String>, DavError> {
    let payload = match kind {
        ResourceKind::Calendar => get_calendar_object(db, collection_id, uid)
            .await
            .map_err(|_| DavError::Internal)?
            .map(|object| object.raw_payload),
        ResourceKind::Card => get_card_object(db, collection_id, uid)
            .await
            .map_err(|_| DavError::Internal)?
            .map(|object| object.raw_payload),
    };
    Ok(payload)
}
