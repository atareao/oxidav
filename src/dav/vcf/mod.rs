//! F08 — vCard (`.vcf`) parsing for CardDAV.
//!
//! [`parse_vcf`] turns a raw `text/vcard` payload into the denormalised
//! [`ParsedCard`] that F03's `card_objects` table stores (plus the raw payload,
//! which the caller keeps verbatim in `raw_payload`).
//!
//! # Parsing strategy
//!
//! The structured properties come from the [`vcard4`] crate (RFC 6350 parser),
//! which handles content-line folding, parameter parsing and value escaping.
//! On top of that, a small **hardened** pre-pass normalises two cases the crate
//! cannot express on its own:
//!
//! * `vcard4` rejects a card that has no `FN` (formatted name) with
//!   [`vcard4::Error::NoFormattedName`], even in `parse_loose` mode. The spec
//!   requires such a card to be accepted with a deterministic synthetic `fn_`
//!   (the formatted `N`, or the `UID`). We therefore derive a name and inject an
//!   `FN` line **before `END:VCARD`** — never before `VERSION`, which RFC 6350
//!   (§6.7.1) mandates to be the first property (`vcard4` errors otherwise).
//! * The envelope (`BEGIN:VCARD`/`END:VCARD`) is verified up front so a
//!   non-card payload is rejected as [`VcfError::NotACard`] rather than being
//!   silently coerced.
//!
//! All the pre-pass code touches the payload only through panic-free string
//! operations ([`str::split_once`], [`str::get`], [`str::strip_suffix`],
//! [`str::split_at`] at ASCII boundaries); it never indexes raw bytes at an
//! unvalidated offset, mirroring the hardened slicing of `ics.rs`, so a payload
//! carrying multibyte characters in unexpected positions returns an error
//! instead of aborting the process.
//!
//! # Documented limitations
//!
//! * **Multiple `EMAIL`/`TEL`:** the columns are denormalised to a single value.
//!   We pick the entry with the smallest `PREF` parameter (vCard 4.0), falling
//!   back to the first entry when no `PREF` is present. The full set remains in
//!   `raw_payload`, which is the source of truth. vCard 3.0 `TYPE=PREF` is not
//!   interpreted as a preference (the 3.0 `TYPE` token space is different), so
//!   for 3.0 cards the first entry wins.
//! * **No `FN`:** accepted with a synthetic `fn_` derived from `N`
//!   (`"given family"`) or, failing that, the `UID`.
//! * **`ORG`/`NOTE`:** the first `ORG` (its components joined with `"; "`) and
//!   the first `NOTE` are denormalised; extra occurrences stay in `raw_payload`.
//! * **vCard 3.0 vs 4.0:** both are parsed (`vcard4` accepts a `VERSION:3.0`
//!   card); 3.0-only quirks that the crate cannot tokenise are tolerated by
//!   `parse_loose`.

pub mod merge;

use std::borrow::Cow;

use vcard4::property::{Property, TextOrUriProperty};

/// Errors produced while parsing a `.vcf` payload.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VcfError {
    /// The payload is empty (or only whitespace).
    #[error("el payload vCard está vacío")]
    Empty,
    /// The payload has no `BEGIN:VCARD`/`END:VCARD` envelope.
    #[error("el payload no es una vCard: falta BEGIN:VCARD/END:VCARD")]
    NotACard,
    /// The payload is enveloped but not a parseable vCard.
    #[error("vCard malformada: {0}")]
    Malformed(String),
    /// The card has no (or an empty) `UID` and no derivable fallback.
    #[error("la vCard no tiene UID")]
    MissingUid,
}

/// The denormalised fields extracted from a single vCard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCard {
    /// `UID`.
    pub uid: String,
    /// `FN`, or a deterministic synthetic fallback when `FN` is absent.
    pub fn_: String,
    /// `EMAIL` (preferred entry; see the module docs).
    pub email: Option<String>,
    /// `TEL` (preferred entry; see the module docs).
    pub phone: Option<String>,
    /// `ORG`, components joined with `"; "`.
    pub organization: Option<String>,
    /// `NOTE`.
    pub notes: Option<String>,
}

/// Parse a raw `.vcf` payload and extract the first vCard.
///
/// # Errors
///
/// [`VcfError::Empty`] for an empty payload, [`VcfError::NotACard`] when the
/// `BEGIN:VCARD`/`END:VCARD` envelope is missing, [`VcfError::Malformed`] when
/// the enveloped payload is not a parseable vCard, and [`VcfError::MissingUid`]
/// when the card has no usable `UID`.
pub fn parse_vcf(raw: &str) -> Result<ParsedCard, VcfError> {
    if raw.trim().is_empty() {
        return Err(VcfError::Empty);
    }
    if !has_envelope(raw) {
        return Err(VcfError::NotACard);
    }

    // Guarantee an `FN` is present so `vcard4` accepts the card. A card with no
    // `FN` and no `N`/`UID` to synthesise one from is rejected as `MissingUid`.
    let normalized = ensure_formatted_name(raw).ok_or(VcfError::MissingUid)?;

    let cards = vcard4::parse_loose(normalized.as_ref())
        .map_err(|err| VcfError::Malformed(err.to_string()))?;
    let card = cards.first().ok_or(VcfError::NotACard)?;

    let uid = card
        .uid
        .as_ref()
        .map(text_or_uri_value)
        .filter(|uid| !uid.trim().is_empty())
        .ok_or(VcfError::MissingUid)?;

    let fn_ = card
        .formatted_name
        .first()
        .map(|prop| prop.value.clone())
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| VcfError::Malformed("la vCard no tiene un FN utilizable".to_string()))?;

    let email = preferred_entry(
        card.email
            .iter()
            .map(|prop| (prop.value.clone(), pref_of(prop))),
    );
    let phone = card
        .tel
        .iter()
        .map(|prop| (text_or_uri_value(prop), pref_of(prop)))
        .min_by_key(|(_, pref)| pref.unwrap_or(u8::MAX))
        .map(|(value, _)| value)
        .filter(|value| !value.trim().is_empty());

    let organization = card
        .org
        .first()
        .map(|prop| prop.value.join("; "))
        .filter(|value| !value.trim().is_empty());
    let notes = card
        .note
        .first()
        .map(|prop| prop.value.clone())
        .filter(|value| !value.trim().is_empty());

    Ok(ParsedCard {
        uid,
        fn_,
        email,
        phone,
        organization,
        notes,
    })
}

/// String value of a `TEXT`-or-`URI` property.
fn text_or_uri_value(property: &TextOrUriProperty) -> String {
    match property {
        TextOrUriProperty::Text(text) => text.value.clone(),
        TextOrUriProperty::Uri(uri) => uri.to_string(),
    }
}

/// The `PREF` parameter of a property, when present (lower means preferred).
fn pref_of<P: Property>(property: &P) -> Option<u8> {
    property.parameters().and_then(|params| params.pref)
}

/// Select the preferred non-empty entry: smallest `PREF` first, else the first
/// occurrence. Empty values are skipped.
fn preferred_entry<I: Iterator<Item = (String, Option<u8>)>>(entries: I) -> Option<String> {
    entries
        .filter(|(value, _)| !value.trim().is_empty())
        .min_by_key(|(_, pref)| pref.unwrap_or(u8::MAX))
        .map(|(value, _)| value)
}

/// Whether the payload carries a well-formed `BEGIN:VCARD`/`END:VCARD` envelope.
fn has_envelope(raw: &str) -> bool {
    let mut begin = false;
    let mut end = false;
    for line in raw.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line).trim();
        if line.eq_ignore_ascii_case("BEGIN:VCARD") {
            begin = true;
        } else if line.eq_ignore_ascii_case("END:VCARD") {
            end = true;
        }
    }
    begin && end
}

/// Return the payload unchanged when it already contains an `FN` property, or a
/// copy with a synthetic `FN` injected before `END:VCARD`. `None` when neither
/// `FN` nor a derivable `N`/`UID` is available.
fn ensure_formatted_name(raw: &str) -> Option<Cow<'_, str>> {
    let lines = logical_lines(raw);
    if lines.iter().any(|line| has_property(line, "FN")) {
        return Some(Cow::Borrowed(raw));
    }
    let derived = derive_formatted_name(&lines)?;
    Some(Cow::Owned(inject_property(raw, "FN", &derived)))
}

/// Join folded content lines (RFC 6350 §3.2: a leading space or tab continues
/// the previous line) into logical lines, dropping the trailing `CR`.
fn logical_lines(raw: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for physical in raw.split('\n') {
        let physical = physical.strip_suffix('\r').unwrap_or(physical);
        let is_continuation = physical.starts_with(' ') || physical.starts_with('\t');
        if is_continuation && !lines.is_empty() {
            // `get(1..)` never panics: byte 1 follows an ASCII space/tab, a char
            // boundary by construction.
            let rest = physical.get(1..).unwrap_or("");
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
            }
        } else {
            lines.push(physical.to_string());
        }
    }
    lines
}

/// The property name of a content line: everything before the first `:`, with
/// any group prefix (`item1.TEL`) and parameters (`;TYPE=...`) removed.
fn property_name(line: &str) -> Option<&str> {
    let (head, _value) = line.split_once(':')?;
    let head = head.split(';').next()?;
    let name = head.rsplit('.').next()?.trim();
    if name.is_empty() { None } else { Some(name) }
}

/// Whether `line` declares the property `name` (case-insensitive).
fn has_property(line: &str, name: &str) -> bool {
    property_name(line).is_some_and(|found| found.eq_ignore_ascii_case(name))
}

/// Derive a synthetic `FN` from the structured `N` property, or fall back to
/// the `UID`.
fn derive_formatted_name(lines: &[String]) -> Option<String> {
    for line in lines {
        if has_property(line, "N")
            && let Some((_, value)) = line.split_once(':')
            && let Some(name) = format_structured_name(value)
        {
            return Some(name);
        }
    }
    for line in lines {
        if has_property(line, "UID")
            && let Some((_, value)) = line.split_once(':')
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Format the structured `N` value (`family;given;additional;prefix;suffix`)
/// into `"given family"`. Returns `None` when both name parts are empty.
fn format_structured_name(value: &str) -> Option<String> {
    let mut parts = value.split(';');
    let family = parts.next().unwrap_or("").trim();
    let given = parts.next().unwrap_or("").trim();
    match (given.is_empty(), family.is_empty()) {
        (false, false) => Some(format!("{given} {family}")),
        (false, true) => Some(given.to_string()),
        (true, false) => Some(family.to_string()),
        (true, true) => None,
    }
}

/// Insert `<name>:<value>\r\n` just before the last logical `END:VCARD` line,
/// keeping `VERSION` (which RFC 6350 requires first) in place.
///
/// The terminator is located by walking the *logical* content lines (RFC 6350
/// §3.2 folding: a leading space/tab continues the previous line) and comparing
/// each, case-insensitively, with `END:VCARD` — mirroring [`has_envelope`]. A
/// raw substring search would mis-locate a terminator that also appears inside a
/// value (e.g. a `NOTE`) and would miss a lowercase `end:vcard`.
fn inject_property(raw: &str, name: &str, value: &str) -> String {
    let mut insert_at: Option<usize> = None;
    let mut offset = 0usize;
    for physical in raw.split_inclusive('\n') {
        let line = physical.strip_suffix('\n').unwrap_or(physical);
        let line = line.strip_suffix('\r').unwrap_or(line);
        // Only a line that does not continue a folded one starts a new logical
        // line, so `offset` (its start) is the logical start to record. This
        // also keeps the *last* case-insensitive `END:VCARD` terminator, as
        // before.
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && line.trim().eq_ignore_ascii_case("END:VCARD")
        {
            insert_at = Some(offset);
        }
        offset += physical.len();
    }
    match insert_at {
        Some(index) => {
            let (head, tail) = raw.split_at(index);
            format!("{head}{name}:{value}\r\n{tail}")
        }
        None => format!("{raw}\r\n{name}:{value}\r\n"),
    }
}

/// The denormalised fields needed to compose a single-`VCARD` `.vcf`.
///
/// This is the composition-side counterpart of [`ParsedCard`]: where
/// [`parse_vcf`] extracts these fields *from* a payload, [`build_vcf`] renders
/// them *into* one. It is the single, shared home of vCard serialisation,
/// reused by the REST contact endpoints (F12) instead of duplicating `.vcf`
/// construction in the HTTP layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardFields {
    /// `UID`.
    pub uid: String,
    /// `FN` (formatted name).
    pub fn_: String,
    /// `EMAIL`.
    pub email: Option<String>,
    /// `TEL`.
    pub phone: Option<String>,
    /// `ORG`.
    pub organization: Option<String>,
    /// `NOTE`.
    pub notes: Option<String>,
}

/// Compose a minimal, RFC 6350-valid `VCARD` (version 4.0).
///
/// Only genuinely free-text values are escaped per RFC 6350 §3.4, which is
/// exactly the set of fields [`parse_vcf`] unescapes on the way back, so the
/// composed payload round-trips through [`parse_vcf`] for every field it
/// exposes (`uid`, `fn_`, `email`, `phone`, `organization`, `notes`). `UID` is
/// otherwise emitted verbatim — it is an opaque token whose separators (`.`,
/// `@`, `:`, …) must survive intact — but line-breaking characters are stripped
/// by [`safe_uid`], so a `uid` can never break out of its content line and
/// inject a property. A well-formed `uid` (with no such characters) is
/// unchanged, so the round-trip is preserved.
///
/// Every content line is folded at 75 octets (RFC 6350 §3.2) without splitting a
/// UTF-8 character, and CRLF line endings plus a stable property order make the
/// output deterministic for a given [`CardFields`].
#[must_use]
pub fn build_vcf(fields: &CardFields) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(10);
    lines.push("BEGIN:VCARD".to_string());
    lines.push("VERSION:4.0".to_string());
    lines.push(format!("UID:{}", safe_uid(&fields.uid)));
    lines.push(format!("FN:{}", escape_text(&fields.fn_)));
    if let Some(email) = &fields.email {
        lines.push(format!("EMAIL:{}", escape_text(email)));
    }
    if let Some(phone) = &fields.phone {
        lines.push(format!("TEL:{}", escape_text(phone)));
    }
    if let Some(organization) = &fields.organization {
        lines.push(format!("ORG:{}", escape_text(organization)));
    }
    if let Some(notes) = &fields.notes {
        lines.push(format!("NOTE:{}", escape_text(notes)));
    }
    lines.push("END:VCARD".to_string());

    let mut out = String::with_capacity(256);
    for line in &lines {
        super::common::fold_content_line(line, &mut out);
    }
    out
}

/// Render a `UID` value that cannot break out of its content line.
///
/// `UID` is emitted verbatim except that CR, LF, every other control character
/// and the Unicode line/paragraph separators U+2028/U+2029 are dropped. Without
/// this, a stored `uid` that originated from a client `.vcf` (CardDAV persists
/// the parsed `UID` verbatim) could carry a real newline and inject a content
/// line when the payload is recomposed. A well-formed `uid` is unchanged, so
/// [`parse_vcf`] still round-trips it.
fn safe_uid(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() && *c != '\u{2028}' && *c != '\u{2029}')
        .collect()
}

/// Escape a free-text value for a content line (RFC 6350 §3.4).
///
/// Backslash, comma and semicolon gain a leading backslash; LF becomes the
/// two-character sequence `\n`. A bare `\r` is dropped so CRLF input collapses
/// to a single logical newline, matching [`parse_vcf`]'s unescaping. Applied
/// only to the free-text fields; the structured `UID` is emitted verbatim.
fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// F22 — full vCard projection (contract 3 of PLAN-023).
//
// [`CardProjection`] is the typed, complete view of a vCard used by the REST
// detail DTO. It is *derived at read time* from `raw_payload` (the single
// source of truth); nothing here is persisted.
// ---------------------------------------------------------------------------

/// A typed, complete projection of a vCard, grouped by RFC 6350 category.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CardProjection {
    /// `VERSION`, verbatim (e.g. `"4.0"`); empty when the property is absent.
    pub version: String,
    /// `UID`.
    pub uid: String,
    /// `FN` (formatted name), when present.
    pub fn_: Option<String>,
    /// `REV`, **verbatim** as written by the client. Read from the raw content
    /// lines rather than the typed model, which reformats the timestamp (and
    /// drops the dashed form `2024-01-15T10:15:30Z` entirely).
    pub rev: Option<String>,
    /// `N`, split into its five components.
    pub name: Option<StructuredName>,
    /// `NICKNAME` occurrences, in order.
    pub nicknames: Vec<String>,
    /// `PHOTO` (URI or embedded binary).
    pub photos: Vec<MediaValue>,
    /// `BDAY`, **verbatim** as written by the client. Read from the raw content
    /// lines: the typed model would invent a year for a year-less value
    /// (`BDAY:--0203` → `00000203`).
    pub birthday: Option<String>,
    /// `ANNIVERSARY`, **verbatim** as written by the client (see [`Self::birthday`]).
    pub anniversary: Option<String>,
    /// `GENDER` (sex plus identity).
    pub gender: Option<Gender>,
    /// `TEL` occurrences, in order.
    pub tels: Vec<MultiValue>,
    /// `EMAIL` occurrences, in order.
    pub emails: Vec<MultiValue>,
    /// `IMPP` occurrences, in order.
    pub impps: Vec<MultiValue>,
    /// `ADR` occurrences, in order.
    pub addresses: Vec<Address>,
    /// `GEO` occurrences, in order.
    pub geos: Vec<String>,
    /// `TZ` occurrences, in order.
    pub timezones: Vec<String>,
    /// `TITLE` occurrences, in order.
    pub titles: Vec<String>,
    /// `ROLE` occurrences, in order.
    pub roles: Vec<String>,
    /// `ORG` occurrences, one entry per line.
    pub organizations: Vec<Vec<String>>,
    /// `LOGO` (URI or embedded binary).
    pub logos: Vec<MediaValue>,
    /// `NOTE` occurrences, in order.
    pub notes: Vec<String>,
    /// `CATEGORIES` occurrences, one entry per line.
    pub categories: Vec<Vec<String>>,
    /// `URL` occurrences, in order.
    pub urls: Vec<String>,
    /// `SOUND` (URI or embedded binary).
    pub sounds: Vec<MediaValue>,
    /// Everything not covered by the groups above: `X-*` and the modelled RFC
    /// properties outside the eight tabs (`KIND`, `SOURCE`, `XML`, `LANG`,
    /// `MEMBER`, `RELATED`, `PRODID`, `CLIENTPIDMAP`, `KEY`, `FBURL`,
    /// `CALADRURI`, `CALURI`).
    pub others: Vec<RawProperty>,
}

/// The five components of a structured `N` property.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructuredName {
    /// Family name.
    pub family: Option<String>,
    /// Given name.
    pub given: Option<String>,
    /// Additional names (empty components dropped).
    pub additional: Vec<String>,
    /// Honorific prefixes (empty components dropped).
    pub prefix: Vec<String>,
    /// Honorific suffixes (empty components dropped).
    pub suffix: Vec<String>,
}

/// `GENDER`: sex code plus optional identity text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Gender {
    /// The sex (`M`/`F`/`O`/`N`/`U`), when not empty.
    pub sex: Option<String>,
    /// The free-text gender identity, when present.
    pub identity: Option<String>,
}

/// A single value with its `TYPE` parameters and `PREF`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MultiValue {
    /// The property value.
    pub value: String,
    /// `TYPE` tokens, lower-cased as the parser preserves them.
    pub types: Vec<String>,
    /// `PREF` (lower is preferred), when present.
    pub pref: Option<u8>,
}

/// `PHOTO`/`LOGO`/`SOUND`: either an external URI or an embedded binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaValue {
    /// An external URI, verbatim.
    Uri(String),
    /// Embedded content, already shaped as a `data:` URI.
    Data {
        /// The media type (`image/jpeg`, `image/png`, …), when known.
        media_type: Option<String>,
        /// The ready-to-use `data:<media-type>;base64,<…>` URI.
        data_uri: String,
    },
}

/// A structured `ADR`, with its seven components, `LABEL` and parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Address {
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

/// A property outside the modelled groups, kept as written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawProperty {
    /// The property name (`X-ABLabel`, `PRODID`, …).
    pub name: String,
    /// The group prefix (`item1` in `item1.X-ABLabel`), when present.
    pub group: Option<String>,
    /// The property parameters as `(name, value)` pairs.
    pub parameters: Vec<(String, String)>,
    /// The property value.
    pub value: String,
}

/// Project the full payload of a vCard into a typed, grouped view.
///
/// The structured fields come from the typed [`vcard4`] model. The four scalar
/// fields `version`, `rev`, `birthday` and `anniversary` are read **verbatim**
/// from the raw content lines instead (see [`CardProjection::rev`] and
/// [`CardProjection::birthday`]): the model normalises their text —`VERSION` is
/// consumed and discarded, `BDAY:--0203` becomes `00000203`, `REV` reformats—
/// so the model cannot reproduce what the client wrote.
///
/// # Documented limitations
///
/// Everything below was measured against `vcard4` 0.7.3, not assumed:
///
/// * **Embedded base64 `LOGO`/`SOUND` are lost from the projection.** The crate
///   models them as [`vcard4::property::UriProperty`], so `parse_loose`
///   **discards** the property when the base64 cannot parse as a URI. `PHOTO`
///   works (it is a [`TextOrUriProperty`], hence the `data:` rendering below).
///   The payload itself is not lost — it stays in `raw_payload` and the merge
///   preserves it byte for byte — only *showing* it is impossible.
/// * **`URL` values are normalised** by the URI parser: `https://example.com`
///   comes back as `https://example.com/`.
/// * **`others` covers what the crate models** (the 36 RFC 6350 properties)
///   **plus the `X-*` extensions**. A non-`X-` property the crate does not know
///   is dropped by `parse_loose` and does not appear.
///
/// # Errors
///
/// The same errors as [`parse_vcf`]: [`VcfError::Empty`] for an empty payload,
/// [`VcfError::NotACard`] without a `BEGIN:VCARD`/`END:VCARD` envelope,
/// [`VcfError::Malformed`] when the enveloped payload is not parseable, and
/// [`VcfError::MissingUid`] when no usable `UID` can be derived.
pub fn project_card(raw: &str) -> Result<CardProjection, VcfError> {
    let parsed = parse_vcf(raw)?;

    // `parse_vcf` guarantees a usable `UID`/`FN`; the *full* card is re-parsed
    // here because `ParsedCard` exposes only the six denormalised fields. The
    // `FN` injection of `parse_vcf` is applied too, so the typed card and the
    // denormalised view always agree.
    let normalized = ensure_formatted_name(raw).ok_or(VcfError::MissingUid)?;
    let cards = vcard4::parse_loose(normalized.as_ref())
        .map_err(|err| VcfError::Malformed(err.to_string()))?;
    let card = cards.first().ok_or(VcfError::NotACard)?;

    Ok(CardProjection {
        // R1: verbatim from the raw lines, not from the typed model.
        version: raw_property_value(raw, "VERSION").unwrap_or_default(),
        uid: parsed.uid,
        fn_: Some(parsed.fn_),
        rev: raw_property_value(raw, "REV"),
        name: card.name.as_ref().map(structured_name),
        nicknames: card
            .nickname
            .iter()
            .map(|prop| prop.value.clone())
            .collect(),
        photos: card.photo.iter().map(media_value).collect(),
        birthday: raw_property_value(raw, "BDAY"),
        anniversary: raw_property_value(raw, "ANNIVERSARY"),
        gender: card.gender.as_ref().map(gender),
        tels: card.tel.iter().map(text_or_uri_multi).collect(),
        emails: card.email.iter().map(text_multi).collect(),
        impps: card.impp.iter().map(uri_multi).collect(),
        addresses: card.address.iter().map(address).collect(),
        geos: card.geo.iter().map(|prop| prop.value.to_string()).collect(),
        timezones: card
            .timezone
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        titles: card.title.iter().map(|prop| prop.value.clone()).collect(),
        roles: card.role.iter().map(|prop| prop.value.clone()).collect(),
        organizations: card.org.iter().map(|prop| prop.value.clone()).collect(),
        logos: card
            .logo
            .iter()
            .map(|prop| MediaValue::Uri(prop.value.to_string()))
            .collect(),
        notes: card.note.iter().map(|prop| prop.value.clone()).collect(),
        categories: card
            .categories
            .iter()
            .map(|prop| prop.value.clone())
            .collect(),
        urls: card.url.iter().map(|prop| prop.value.to_string()).collect(),
        sounds: card
            .sound
            .iter()
            .map(|prop| MediaValue::Uri(prop.value.to_string()))
            .collect(),
        others: others(card),
    })
}

/// Verbatim text value of the first occurrence of `name`, read from the logical
/// content lines of the payload (no unescaping). `None` when absent.
///
/// Reuses [`logical_lines`] and [`property_name`], so a folded property is
/// matched whole and a group/parameter prefix (`item1.TEL`, `TEL;TYPE=cell`) is
/// ignored. This is the escape hatch for the fields the typed model normalises.
fn raw_property_value(raw: &str, name: &str) -> Option<String> {
    logical_lines(raw)
        .into_iter()
        .find(|line| has_property(line, name))
        .and_then(|line| line.split_once(':').map(|(_, value)| value.to_string()))
}

/// The five components of a structured `N` (`family;given;additional;prefix;suffix`).
fn structured_name(prop: &vcard4::property::TextListProperty) -> StructuredName {
    let mut components = prop.value.iter().map(String::as_str);
    let family = components.next().filter(|value| !value.is_empty());
    let given = components.next().filter(|value| !value.is_empty());
    let additional = text_list(components.next());
    let prefix = text_list(components.next());
    let suffix = text_list(components.next());

    StructuredName {
        family: family.map(str::to_string),
        given: given.map(str::to_string),
        additional,
        prefix,
        suffix,
    }
}

/// Split a structured component into its comma-separated list, dropping empties.
fn text_list(component: Option<&str>) -> Vec<String> {
    component
        .into_iter()
        .flat_map(|value| value.split(','))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

/// `GENDER`, mapping the crate's `Sex` to its RFC token (`Sex::None` → `None`).
fn gender(prop: &vcard4::property::GenderProperty) -> Gender {
    let sex = match &prop.value.sex {
        vcard4::property::Sex::None => None,
        sex => Some(sex.to_string()),
    };
    Gender {
        sex,
        identity: prop.value.identity.clone().filter(|text| !text.is_empty()),
    }
}

/// `TEL`/`RELATED`: a text-or-URI value with its `TYPE`/`PREF`.
fn text_or_uri_multi(prop: &TextOrUriProperty) -> MultiValue {
    MultiValue {
        value: text_or_uri_value(prop),
        types: type_tokens(prop),
        pref: pref_of(prop),
    }
}

/// `EMAIL` (a plain text property) with its `TYPE`/`PREF`.
fn text_multi(prop: &vcard4::property::TextProperty) -> MultiValue {
    MultiValue {
        value: prop.value.clone(),
        types: type_tokens(prop),
        pref: pref_of(prop),
    }
}

/// `IMPP` (a URI property) with its `TYPE`/`PREF`.
fn uri_multi(prop: &vcard4::property::UriProperty) -> MultiValue {
    MultiValue {
        value: prop.value.to_string(),
        types: type_tokens(prop),
        pref: pref_of(prop),
    }
}

/// The `TYPE` tokens of a property, as the client wrote them.
///
/// [`TypeParameter::Home`]/[`Work`]/[`Telephone`]/[`Related`] render through
/// their `Display` (`home`, `cell`, `voice`…); [`TypeParameter::Extension`]
/// keeps its **raw token** (`internet`, `JPEG`) instead of the `Display`, which
/// would prepend `X-`.
fn type_tokens<P: Property>(prop: &P) -> Vec<String> {
    prop.parameters()
        .and_then(|params| params.types.as_ref())
        .into_iter()
        .flatten()
        .map(type_token)
        .collect()
}

/// A single `TYPE` token without the `X-` prefix the crate's `Display` adds.
fn type_token(param: &vcard4::parameter::TypeParameter) -> String {
    match param {
        vcard4::parameter::TypeParameter::Extension(token) => token.clone(),
        other => other.to_string(),
    }
}

/// `ADR`: seven components plus `LABEL` and the `TYPE`/`PREF` parameters.
fn address(prop: &vcard4::property::AddressProperty) -> Address {
    let value = &prop.value;
    Address {
        po_box: value.po_box.clone(),
        extended: value.extended_address.clone(),
        street: value.street_address.clone(),
        locality: value.locality.clone(),
        region: value.region.clone(),
        postal_code: value.postal_code.clone(),
        country: value.country_name.clone(),
        label: prop
            .parameters()
            .and_then(|params| params.label.clone())
            .filter(|label| !label.is_empty()),
        types: type_tokens(prop),
        pref: pref_of(prop),
    }
}

/// `PHOTO`: an external URI, or embedded base64 rendered as a `data:` URI.
///
/// The media type comes from the raw `TYPE` token (`JPEG` → `image/jpeg`) —
/// never from its `Display`, which would yield `X-JPEG`. When the token is
/// absent or not a known media type it falls back to
/// `application/octet-stream`: an empty media type in `data:;base64,…` means
/// `text/plain` (RFC 2397), so the UI would mistake an image for text.
fn media_value(prop: &TextOrUriProperty) -> MediaValue {
    match prop {
        TextOrUriProperty::Uri(uri) => MediaValue::Uri(uri.value.to_string()),
        TextOrUriProperty::Text(text) => {
            let media_type = prop
                .parameters()
                .and_then(|params| params.types.as_ref())
                .into_iter()
                .flatten()
                .find_map(media_type_of)
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let data_uri = format!("data:{media_type};base64,{}", text.value);
            MediaValue::Data {
                media_type: Some(media_type),
                data_uri,
            }
        }
    }
}

/// Map a raw `TYPE` token to a media type (case-insensitive), when known.
fn media_type_of(param: &vcard4::parameter::TypeParameter) -> Option<String> {
    let vcard4::parameter::TypeParameter::Extension(token) = param else {
        return None;
    };
    let media_type = match token.to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    Some(media_type.to_string())
}

/// Everything outside the modelled groups: the `X-*` extensions plus the RFC
/// properties not covered by a dedicated field.
///
/// The typed groups do not carry their own property name, so it is passed here
/// explicitly (`SOURCE`, `KIND`, `MEMBER`…). The `X-*` extensions bring theirs.
fn others(card: &vcard4::Vcard) -> Vec<RawProperty> {
    let mut others: Vec<RawProperty> = Vec::new();

    others.extend(
        card.source
            .iter()
            .map(|prop| raw("SOURCE", prop.value.to_string(), prop)),
    );
    if let Some(kind) = &card.kind {
        others.push(raw("KIND", kind.value.to_string(), kind));
    }
    others.extend(
        card.xml
            .iter()
            .map(|prop| raw("XML", prop.value.clone(), prop)),
    );
    others.extend(
        card.lang
            .iter()
            .map(|prop| raw("LANG", prop.value.to_string(), prop)),
    );
    others.extend(
        card.member
            .iter()
            .map(|prop| raw("MEMBER", prop.value.to_string(), prop)),
    );
    others.extend(
        card.related
            .iter()
            .map(|prop| raw("RELATED", text_or_uri_value(prop), prop)),
    );
    if let Some(prod_id) = &card.prod_id {
        others.push(raw("PRODID", prod_id.value.clone(), prod_id));
    }
    others.extend(
        card.client_pid_map
            .iter()
            .map(|prop| raw("CLIENTPIDMAP", prop.value.to_string(), prop)),
    );
    others.extend(
        card.key
            .iter()
            .map(|prop| raw("KEY", text_or_uri_value(prop), prop)),
    );
    others.extend(
        card.fburl
            .iter()
            .map(|prop| raw("FBURL", prop.value.to_string(), prop)),
    );
    others.extend(
        card.cal_adr_uri
            .iter()
            .map(|prop| raw("CALADRURI", prop.value.to_string(), prop)),
    );
    others.extend(
        card.cal_uri
            .iter()
            .map(|prop| raw("CALURI", prop.value.to_string(), prop)),
    );
    others.extend(card.extensions.iter().map(|prop| RawProperty {
        name: prop.name.clone(),
        group: prop.group.clone(),
        parameters: raw_parameters(prop),
        value: any_property_value(&prop.value),
    }));

    others
}

/// A `RawProperty` from a modelled property: `name` is the RFC property name
/// (the typed groups do not carry it), `value` its already-rendered value.
fn raw<P: Property>(name: &str, value: String, prop: &P) -> RawProperty {
    RawProperty {
        name: name.to_string(),
        group: prop.group().cloned(),
        parameters: raw_parameters(prop),
        value,
    }
}

/// The parameters of a property as `(name, value)` pairs, with the same raw
/// `TYPE` tokens as [`type_tokens`] (no `X-` prefix on extensions).
fn raw_parameters<P: Property>(prop: &P) -> Vec<(String, String)> {
    let Some(params) = prop.parameters() else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    if let Some(language) = &params.language {
        out.push(("LANGUAGE".to_string(), language.to_string()));
    }
    if let Some(value) = &params.value {
        out.push(("VALUE".to_string(), value.to_string()));
    }
    if let Some(pref) = params.pref {
        out.push(("PREF".to_string(), pref.to_string()));
    }
    if let Some(alt_id) = &params.alt_id {
        out.push(("ALTID".to_string(), alt_id.clone()));
    }
    if let Some(pids) = &params.pid {
        out.push((
            "PID".to_string(),
            pids.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ));
    }
    if let Some(types) = &params.types {
        out.push((
            "TYPE".to_string(),
            types.iter().map(type_token).collect::<Vec<_>>().join(","),
        ));
    }
    if let Some(media_type) = &params.media_type {
        out.push(("MEDIATYPE".to_string(), media_type.to_string()));
    }
    if let Some(calscale) = &params.calscale {
        out.push(("CALSCALE".to_string(), calscale.clone()));
    }
    if let Some(sort_as) = &params.sort_as {
        out.push(("SORT-AS".to_string(), sort_as.join(",")));
    }
    if let Some(geo) = &params.geo {
        out.push(("GEO".to_string(), geo.to_string()));
    }
    if let Some(label) = &params.label {
        out.push(("LABEL".to_string(), label.clone()));
    }
    if let Some(extensions) = &params.extensions {
        for (name, values) in extensions {
            out.push((name.clone(), values.join(",")));
        }
    }
    out
}

/// The value of a modelled extension property (`X-*`); text is returned as
/// written, other variants through their `Display`.
fn any_property_value(value: &vcard4::property::AnyProperty) -> String {
    match value {
        vcard4::property::AnyProperty::Text(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(body: &str) -> String {
        format!("BEGIN:VCARD\r\nVERSION:4.0\r\n{body}END:VCARD\r\n")
    }

    #[test]
    fn parses_a_simple_complete_card() {
        let raw = card(
            "UID:simple@oxidav\r\nFN:Ada Lovelace\r\nEMAIL:ada@example.com\r\n\
             TEL:+34600000000\r\nORG:Analytical Engines;Research\r\nNOTE:first programmer\r\n",
        );

        let parsed = parse_vcf(&raw).expect("parse");

        assert_eq!(parsed.uid, "simple@oxidav");
        assert_eq!(parsed.fn_, "Ada Lovelace");
        assert_eq!(parsed.email.as_deref(), Some("ada@example.com"));
        assert_eq!(parsed.phone.as_deref(), Some("+34600000000"));
        assert_eq!(
            parsed.organization.as_deref(),
            Some("Analytical Engines; Research")
        );
        assert_eq!(parsed.notes.as_deref(), Some("first programmer"));
    }

    #[test]
    fn accepts_a_vcard_3_0_payload() {
        let raw = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:grace@oxidav\r\nFN:Grace Hopper\r\n\
                   N:Hopper;Grace;;;\r\nEMAIL;TYPE=INTERNET:grace@example.com\r\n\
                   TEL;TYPE=CELL:+123\r\nORG:US Navy\r\nEND:VCARD\r\n";

        let parsed = parse_vcf(raw).expect("parse");

        assert_eq!(parsed.fn_, "Grace Hopper");
        assert_eq!(parsed.email.as_deref(), Some("grace@example.com"));
        assert_eq!(parsed.phone.as_deref(), Some("+123"));
    }

    #[test]
    fn multiple_emails_and_tels_choose_the_preferred_entry() {
        let raw = card(
            "UID:multi@oxidav\r\nFN:Multi\r\nEMAIL:first@example.com\r\n\
             EMAIL;PREF=1:preferred@example.com\r\nTEL:111\r\nTEL;PREF=2:222\r\n",
        );

        let parsed = parse_vcf(&raw).expect("parse");

        assert_eq!(parsed.email.as_deref(), Some("preferred@example.com"));
        // A `PREF`-marked entry outranks an unmarked one.
        assert_eq!(parsed.phone.as_deref(), Some("222"));
    }

    #[test]
    fn multiple_entries_without_pref_use_the_first() {
        let raw = card("UID:multi2@oxidav\r\nFN:Multi\r\nEMAIL:a@x.com\r\nEMAIL:b@x.com\r\n");

        let parsed = parse_vcf(&raw).expect("parse");

        assert_eq!(parsed.email.as_deref(), Some("a@x.com"));
    }

    #[test]
    fn without_fn_falls_back_to_the_structured_name() {
        let raw = card("UID:no-fn@oxidav\r\nN:Doe;John;;;\r\nEMAIL:john@example.com\r\n");

        let parsed = parse_vcf(&raw).expect("parse");

        assert_eq!(parsed.fn_, "John Doe");
        assert_eq!(parsed.uid, "no-fn@oxidav");
        assert_eq!(parsed.email.as_deref(), Some("john@example.com"));
    }

    #[test]
    fn without_fn_or_n_falls_back_to_the_uid() {
        let raw = card("UID:only-uid@oxidav\r\nNOTE:anonymous\r\n");

        let parsed = parse_vcf(&raw).expect("parse");

        assert_eq!(parsed.fn_, "only-uid@oxidav");
        assert_eq!(parsed.notes.as_deref(), Some("anonymous"));
    }

    #[test]
    fn rejects_empty_and_non_card_payloads() {
        assert_eq!(parse_vcf(""), Err(VcfError::Empty));
        assert_eq!(parse_vcf("   \r\n  "), Err(VcfError::Empty));
        assert_eq!(parse_vcf("garbage"), Err(VcfError::NotACard));
        assert_eq!(
            parse_vcf("BEGIN:VCARD\r\nVERSION:4.0\r\nFN:x\r\n"),
            Err(VcfError::NotACard),
            "a missing END:VCARD is not a card"
        );
    }

    #[test]
    fn injects_before_the_last_case_insensitive_terminator() {
        // The `NOTE` value also contains the literal text `END:VCARD`; the
        // injection must land before the real (here lowercase) terminator line,
        // never inside the note value.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:x@oxidav\r\n\
                   NOTE:see END:VCARD here\r\nend:vcard\r\n";

        let injected = inject_property(raw, "FN", "X");

        assert_eq!(
            injected,
            "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:x@oxidav\r\n\
             NOTE:see END:VCARD here\r\nFN:X\r\nend:vcard\r\n"
        );
    }

    #[test]
    fn injects_appends_when_no_terminator_is_present() {
        let injected = inject_property("BEGIN:VCARD\r\nVERSION:4.0\r\n", "FN", "X");
        assert_eq!(injected, "BEGIN:VCARD\r\nVERSION:4.0\r\n\r\nFN:X\r\n");
    }

    #[test]
    fn rejects_a_card_without_uid_or_derivable_name() {
        let raw = card("FN:Name only\r\n"); // FN present but no UID
        assert_eq!(parse_vcf(&raw), Err(VcfError::MissingUid));

        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nNOTE:nothing usable\r\nEND:VCARD\r\n";
        assert_eq!(parse_vcf(raw), Err(VcfError::MissingUid));
    }

    #[test]
    fn multibyte_input_never_panics() {
        // Multibyte characters in the property name, the UID/N values and in an
        // otherwise invalid line must yield a value or an error, never a panic.
        let raw = card("UID:é\r\nN:é;é;;;\r\nEMAIL:correo@é.es\r\n");
        let parsed = parse_vcf(&raw).expect("parse");
        assert_eq!(parsed.fn_, "é é");
        assert_eq!(parsed.email.as_deref(), Some("correo@é.es"));

        // No colon at all (so no property name): treated as neither N nor UID.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nééé\r\nEND:VCARD\r\n";
        assert_eq!(parse_vcf(raw), Err(VcfError::MissingUid));

        // A malformed-but-enveloped body must come back as an error.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\n\x07\x01\x02\r\nFN:ok\r\nUID:u\r\nEND:VCARD\r\n";
        let _ = parse_vcf(raw); // may parse or error, must not panic
    }

    #[test]
    fn folded_fn_line_is_detected() {
        // The continuation line must not be mistaken for a new property.
        let raw = card("UID:fold@oxidav\r\nFN:Very Long\r\n Name\r\n");
        let parsed = parse_vcf(&raw).expect("parse");
        assert_eq!(parsed.fn_, "Very LongName");
    }

    #[test]
    fn format_structured_name_handles_missing_parts() {
        assert_eq!(
            format_structured_name("Doe;John;;;").as_deref(),
            Some("John Doe")
        );
        assert_eq!(format_structured_name("Doe;;;;").as_deref(), Some("Doe"));
        assert_eq!(format_structured_name(";John;;;").as_deref(), Some("John"));
        assert_eq!(format_structured_name(";;;;"), None);
    }

    #[test]
    fn build_vcf_round_trips_all_fields() {
        let fields = CardFields {
            uid: "composed@oxidav".to_string(),
            fn_: "Ada Lovelace".to_string(),
            email: Some("ada@example.com".to_string()),
            phone: Some("+34600000000".to_string()),
            organization: Some("Analytical Engines Ltd".to_string()),
            notes: Some("first programmer".to_string()),
        };

        let raw = build_vcf(&fields);
        let parsed = parse_vcf(&raw).expect("composed payload must parse");

        assert_eq!(parsed.uid, fields.uid);
        assert_eq!(parsed.fn_, fields.fn_);
        assert_eq!(parsed.email, fields.email);
        assert_eq!(parsed.phone, fields.phone);
        assert_eq!(parsed.organization, fields.organization);
        assert_eq!(parsed.notes, fields.notes);
    }

    #[test]
    fn build_vcf_escapes_reserved_characters() {
        let fields = CardFields {
            uid: "uid-b@x".to_string(),
            fn_: "a\\b;c,d".to_string(),
            email: None,
            phone: None,
            organization: None,
            notes: None,
        };

        let raw = build_vcf(&fields);
        assert!(raw.contains("FN:a\\\\b\\;c\\,d"), "{raw}");
        let parsed = parse_vcf(&raw).expect("parse");
        assert_eq!(parsed.fn_, "a\\b;c,d");
    }

    #[test]
    fn build_vcf_neutralises_a_uid_with_line_breaks() {
        // Regression (M1): a stored uid carrying a real newline must never inject
        // a content line into the composed payload.
        let fields = CardFields {
            uid: "evil\r\nX-INJECTED:1".to_string(),
            fn_: "Ada".to_string(),
            email: None,
            phone: None,
            organization: None,
            notes: None,
        };

        let raw = build_vcf(&fields);
        assert!(
            !raw.contains("\r\nX-INJECTED"),
            "the uid injected a content line: {raw}"
        );
        let uid_lines = raw
            .split("\r\n")
            .filter(|line| line.starts_with("UID:"))
            .count();
        assert_eq!(uid_lines, 1, "the uid must stay on a single line: {raw}");
        // A newline and the Unicode line separator are both neutralised.
        assert_eq!(safe_uid("a\nb\u{2028}c\rd"), "abcd");
        // The sanitiser leaves a well-formed uid untouched.
        assert_eq!(safe_uid("card-1@oxidav"), "card-1@oxidav");
    }

    #[test]
    fn build_vcf_omits_absent_optional_fields() {
        let fields = CardFields {
            uid: "minimal@oxidav".to_string(),
            fn_: "Only Name".to_string(),
            email: None,
            phone: None,
            organization: None,
            notes: None,
        };

        let raw = build_vcf(&fields);
        assert!(!raw.contains("EMAIL:"));
        assert!(!raw.contains("TEL:"));
        assert!(!raw.contains("ORG:"));
        assert!(!raw.contains("NOTE:"));
        let parsed = parse_vcf(&raw).expect("parse");
        assert_eq!(parsed.uid, fields.uid);
        assert_eq!(parsed.email, None);
    }

    #[test]
    fn build_vcf_folds_long_lines_without_splitting_characters() {
        let fn_ = "é".repeat(100);
        let fields = CardFields {
            uid: "long@oxidav".to_string(),
            fn_: fn_.clone(),
            email: None,
            phone: None,
            organization: None,
            notes: None,
        };

        let raw = build_vcf(&fields);
        assert!(raw.contains("\r\n "), "long line must be folded");
        for line in raw.split("\r\n") {
            assert!(line.len() <= 75, "physical line exceeds 75 octets: {line}");
        }
        let parsed = parse_vcf(&raw).expect("folded payload must parse");
        assert_eq!(parsed.fn_, fn_);
    }

    // -----------------------------------------------------------------------
    // F22 — full vCard projection (contract 3 of PLAN-023). RED: these assert
    // on the projection that GREEN must implement.
    // -----------------------------------------------------------------------

    /// The value of the entry with the lowest `PREF`, else the first
    /// non-empty one — mirroring [`parse_vcf`]'s denormalisation.
    fn lowest_pref(entries: &[MultiValue]) -> Option<String> {
        entries
            .iter()
            .filter(|entry| !entry.value.trim().is_empty())
            .min_by_key(|entry| entry.pref.unwrap_or(u8::MAX))
            .map(|entry| entry.value.clone())
    }

    fn sample_card() -> String {
        card(
            "UID:ada@oxidav\r\nFN:Ada Lovelace\r\nN:Lovelace;Ada;Byron;Countess;\r\n\
             NICKNAME:Ada\r\n\
             PHOTO;ENCODING=b;TYPE=JPEG:/9j/4AAQSkZJRgABAQAAAQ==\r\n\
             PHOTO:https://example.com/a.jpg\r\n\
             BDAY:19850115\r\nANNIVERSARY:20100101\r\nGENDER:M;woman\r\n\
             TEL;TYPE=CELL;PREF=1:+34600000000\r\nTEL;TYPE=voice:+34900000000\r\n\
             TEL:+34800000000\r\n\
             EMAIL;TYPE=home:ada@example.com\r\nEMAIL;TYPE=work;PREF=2:ada@work.example\r\n\
             IMPP;TYPE=work:xmpp:ada@example.com\r\n\
             ADR;TYPE=home:;;123 Main St;Springfield;IL;62704;USA\r\n\
             ADR;TYPE=work;LABEL=Work:;;456 Oak Ave;Shelbyville;IL;62705;USA\r\n\
             GEO:geo:37.386013,-122.082932\r\nTZ:America/New_York\r\n\
             TITLE:Engineer\r\nROLE:Developer\r\nORG:Analytical Engines;Research\r\n\
             LOGO:https://example.com/logo.png\r\nNOTE:first programmer\r\n\
             CATEGORIES:Friends,Family\r\nURL:https://example.com/ada\r\n\
             SOUND:https://example.com/a.mp3\r\n\
             item1.TEL:item-tel\r\nitem1.X-ABLabel:Home\r\n\
             X-SOCIALPROFILE;TYPE=home:https://example.com/ada\r\n\
             PRODID:-//Apple Inc.//iOS 17.0//EN\r\nKIND:individual\r\n\
             REV:20240115T101530Z\r\n",
        )
    }

    #[test]
    fn projects_a_realistic_card_without_losing_anything() {
        let projection = project_card(&sample_card()).expect("project");

        // Identification and scalar fields.
        assert_eq!(projection.version, "4.0");
        assert_eq!(projection.uid, "ada@oxidav");
        assert_eq!(projection.fn_.as_deref(), Some("Ada Lovelace"));
        // `REV`/`BDAY`/`ANNIVERSARY` are read verbatim from the raw lines (the
        // typed model normalises them), so these match what the client wrote.
        assert_eq!(projection.rev.as_deref(), Some("20240115T101530Z"));

        assert_eq!(
            projection.name,
            Some(StructuredName {
                family: Some("Lovelace".to_string()),
                given: Some("Ada".to_string()),
                additional: vec!["Byron".to_string()],
                prefix: vec!["Countess".to_string()],
                suffix: Vec::new(),
            })
        );
        assert_eq!(projection.nicknames, vec!["Ada".to_string()]);
        assert_eq!(projection.birthday.as_deref(), Some("19850115"));
        assert_eq!(projection.anniversary.as_deref(), Some("20100101"));
        assert_eq!(
            projection.gender,
            Some(Gender {
                sex: Some("M".to_string()),
                identity: Some("woman".to_string()),
            })
        );

        // PHOTO: Apple's embedded base64 plus an external URI.
        assert_eq!(
            projection.photos,
            vec![
                MediaValue::Data {
                    media_type: Some("image/jpeg".to_string()),
                    data_uri: "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQAAAQ==".to_string(),
                },
                MediaValue::Uri("https://example.com/a.jpg".to_string()),
            ]
        );

        // Multivalue communications, in order, with their TYPE/PREF. The
        // grouped `item1.TEL` is still a `TEL`, so it appears here too.
        assert_eq!(
            projection.tels,
            vec![
                MultiValue {
                    value: "+34600000000".to_string(),
                    types: vec!["cell".to_string()],
                    pref: Some(1),
                },
                MultiValue {
                    value: "+34900000000".to_string(),
                    types: vec!["voice".to_string()],
                    pref: None,
                },
                MultiValue {
                    value: "+34800000000".to_string(),
                    types: Vec::new(),
                    pref: None,
                },
                MultiValue {
                    value: "item-tel".to_string(),
                    types: Vec::new(),
                    pref: None,
                },
            ]
        );
        assert_eq!(
            projection.emails,
            vec![
                MultiValue {
                    value: "ada@example.com".to_string(),
                    types: vec!["home".to_string()],
                    pref: None,
                },
                MultiValue {
                    value: "ada@work.example".to_string(),
                    types: vec!["work".to_string()],
                    pref: Some(2),
                },
            ]
        );
        assert_eq!(
            projection.impps,
            vec![MultiValue {
                value: "xmpp:ada@example.com".to_string(),
                types: vec!["work".to_string()],
                pref: None,
            }]
        );

        // Addresses: the seven components plus LABEL and TYPE.
        assert_eq!(
            projection.addresses,
            vec![
                Address {
                    po_box: None,
                    extended: None,
                    street: Some("123 Main St".to_string()),
                    locality: Some("Springfield".to_string()),
                    region: Some("IL".to_string()),
                    postal_code: Some("62704".to_string()),
                    country: Some("USA".to_string()),
                    label: None,
                    types: vec!["home".to_string()],
                    pref: None,
                },
                Address {
                    po_box: None,
                    extended: None,
                    street: Some("456 Oak Ave".to_string()),
                    locality: Some("Shelbyville".to_string()),
                    region: Some("IL".to_string()),
                    postal_code: Some("62705".to_string()),
                    country: Some("USA".to_string()),
                    label: Some("Work".to_string()),
                    types: vec!["work".to_string()],
                    pref: None,
                },
            ]
        );

        assert_eq!(
            projection.geos,
            vec!["geo:37.386013,-122.082932".to_string()]
        );
        assert_eq!(projection.timezones, vec!["America/New_York".to_string()]);
        assert_eq!(projection.titles, vec!["Engineer".to_string()]);
        assert_eq!(projection.roles, vec!["Developer".to_string()]);
        assert_eq!(
            projection.organizations,
            vec![vec![
                "Analytical Engines".to_string(),
                "Research".to_string()
            ]]
        );
        assert_eq!(
            projection.logos,
            vec![MediaValue::Uri("https://example.com/logo.png".to_string())]
        );
        assert_eq!(projection.notes, vec!["first programmer".to_string()]);
        assert_eq!(
            projection.categories,
            vec![vec!["Friends".to_string(), "Family".to_string()]]
        );
        assert_eq!(projection.urls, vec!["https://example.com/ada".to_string()]);
        assert_eq!(
            projection.sounds,
            vec![MediaValue::Uri("https://example.com/a.mp3".to_string())]
        );

        // `others`: everything outside the groups above. Order is not part of
        // the contract, so membership is asserted.
        assert_eq!(
            projection.others.len(),
            4,
            "others: {:?}",
            projection.others
        );
        assert!(projection.others.contains(&RawProperty {
            name: "X-ABLabel".to_string(),
            group: Some("item1".to_string()),
            parameters: Vec::new(),
            value: "Home".to_string(),
        }));
        assert!(projection.others.contains(&RawProperty {
            name: "X-SOCIALPROFILE".to_string(),
            group: None,
            parameters: vec![("TYPE".to_string(), "home".to_string())],
            value: "https://example.com/ada".to_string(),
        }));
        assert!(projection.others.contains(&RawProperty {
            name: "PRODID".to_string(),
            group: None,
            parameters: Vec::new(),
            value: "-//Apple Inc.//iOS 17.0//EN".to_string(),
        }));
        assert!(projection.others.contains(&RawProperty {
            name: "KIND".to_string(),
            group: None,
            parameters: Vec::new(),
            value: "individual".to_string(),
        }));
    }

    #[test]
    fn projects_a_minimal_card() {
        let raw = card("UID:minimal@oxidav\r\nFN:Minimal\r\n");

        let projection = project_card(&raw).expect("project");

        assert_eq!(projection.version, "4.0");
        assert_eq!(projection.uid, "minimal@oxidav");
        assert_eq!(projection.fn_.as_deref(), Some("Minimal"));
        assert_eq!(projection.rev, None);
        assert_eq!(projection.name, None);
        assert!(projection.nicknames.is_empty());
        assert!(projection.photos.is_empty());
        assert_eq!(projection.birthday, None);
        assert_eq!(projection.anniversary, None);
        assert_eq!(projection.gender, None);
        assert!(projection.tels.is_empty());
        assert!(projection.emails.is_empty());
        assert!(projection.impps.is_empty());
        assert!(projection.addresses.is_empty());
        assert!(projection.geos.is_empty());
        assert!(projection.timezones.is_empty());
        assert!(projection.titles.is_empty());
        assert!(projection.roles.is_empty());
        assert!(projection.organizations.is_empty());
        assert!(projection.logos.is_empty());
        assert!(projection.notes.is_empty());
        assert!(projection.categories.is_empty());
        assert!(projection.urls.is_empty());
        assert!(projection.sounds.is_empty());
        assert!(projection.others.is_empty());
    }

    #[test]
    fn projects_verbatim_scalar_text() {
        // R1: `VERSION`, `REV`, `BDAY` y `ANNIVERSARY` se leen de las **líneas
        // crudas** porque el modelo tipado normaliza —o descarta— el texto
        // original. `BDAY:--0203` sin año se modela como `0000-02-03` (una
        // mentira), y `REV` en forma con guiones lo descarta `parse_loose`.
        let raw = card(
            "UID:verbatim@oxidav\r\nFN:Verbatim\r\n\
             BDAY:--0203\r\nANNIVERSARY:2010-01-01\r\nREV:2024-01-15T10:15:30Z\r\n",
        );

        let projection = project_card(&raw).expect("project");

        assert_eq!(projection.version, "4.0");
        assert_eq!(projection.birthday.as_deref(), Some("--0203"));
        assert_eq!(projection.anniversary.as_deref(), Some("2010-01-01"));
        assert_eq!(projection.rev.as_deref(), Some("2024-01-15T10:15:30Z"));
    }

    #[test]
    fn projects_type_tokens_without_the_extension_prefix() {
        // R2: la crate guarda el token desconocido en `TypeParameter::Extension`
        // y su `Display` lo imprime con `X-`. La proyección muestra el token
        // crudo tal cual lo escribió el cliente (la crate preserva su caja).
        let raw = card(
            "UID:types@oxidav\r\nFN:Types\r\n\
             TEL;TYPE=internet:+1\r\nEMAIL;TYPE=INTERNET:e@example.com\r\n",
        );

        let projection = project_card(&raw).expect("project");

        assert_eq!(projection.tels[0].types, vec!["internet".to_string()]);
        assert_eq!(projection.emails[0].types, vec!["INTERNET".to_string()]);
    }

    #[test]
    fn projects_known_media_types_from_the_raw_extension_token() {
        // El media type sale del token crudo de `TypeParameter::Extension`
        // (`PNG`), no de su `Display` (que imprimiría `X-PNG`).
        let raw = card(
            "UID:media@oxidav\r\nFN:Media\r\n\
             PHOTO;ENCODING=b;TYPE=PNG:iVBORw0KGgo=\r\n\
             PHOTO;ENCODING=b;TYPE=gif:R0lGODlh\r\n",
        );

        let projection = project_card(&raw).expect("project");

        assert_eq!(
            projection.photos,
            vec![
                MediaValue::Data {
                    media_type: Some("image/png".to_string()),
                    data_uri: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                },
                MediaValue::Data {
                    media_type: Some("image/gif".to_string()),
                    data_uri: "data:image/gif;base64,R0lGODlh".to_string(),
                },
            ]
        );
    }

    #[test]
    fn projects_an_embedded_media_with_an_unknown_type_as_octet_stream() {
        // Un `TYPE` que no es un media type conocido no puede quedar en un
        // `data:` URI con media type vacío: por RFC 2397 un `data:;base64,…`
        // significa `text/plain`, y la UI creería que la imagen es texto.
        let raw = card(
            "UID:media-unknown@oxidav\r\nFN:Media\r\n\
             PHOTO;ENCODING=b;TYPE=qqq:/9j/4AAQSkZJRgABAQAAAQ==\r\n",
        );

        let projection = project_card(&raw).expect("project");

        assert_eq!(
            projection.photos,
            vec![MediaValue::Data {
                media_type: Some("application/octet-stream".to_string()),
                data_uri: "data:application/octet-stream;base64,/9j/4AAQSkZJRgABAQAAAQ=="
                    .to_string(),
            }]
        );
    }

    #[test]
    fn projects_an_embedded_media_without_a_type_as_octet_stream() {
        // Ausencia de `TYPE`: mismo caso desconocido, mismo fallback.
        let raw = card(
            "UID:media-notype@oxidav\r\nFN:Media\r\n\
             PHOTO;ENCODING=b:/9j/4AAQSkZJRgABAQAAAQ==\r\n",
        );

        let projection = project_card(&raw).expect("project");

        assert_eq!(
            projection.photos,
            vec![MediaValue::Data {
                media_type: Some("application/octet-stream".to_string()),
                data_uri: "data:application/octet-stream;base64,/9j/4AAQSkZJRgABAQAAAQ=="
                    .to_string(),
            }]
        );
    }

    #[test]
    fn rejects_the_same_payloads_as_parse_vcf() {
        let payloads = [
            "",
            "   \r\n  ",
            "garbage",
            "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:x\r\n",
            "BEGIN:VCARD\r\nVERSION:4.0\r\nNOTE:nothing usable\r\nEND:VCARD\r\n",
        ];

        for payload in payloads {
            assert_eq!(
                project_card(payload).err(),
                parse_vcf(payload).err(),
                "payload: {payload:?}"
            );
        }

        // The projection shares `parse_vcf`'s success path as well.
        let raw = card("UID:ok@oxidav\r\nFN:Ok\r\n");
        assert!(project_card(&raw).is_ok());
    }

    #[test]
    fn projection_agrees_with_the_denormalized_parse() {
        let payloads = [
            card("UID:a@oxidav\r\nFN:Ada\r\n"),
            card(
                "UID:b@oxidav\r\nFN:Bob\r\nEMAIL:e@example.com\r\nTEL:+34\r\n\
                 ORG:Acme;R&D\r\nNOTE:hi\r\n",
            ),
            card(
                "UID:c@oxidav\r\nFN:Carl\r\nEMAIL:x@a.com\r\nEMAIL;PREF=1:y@a.com\r\n\
                 TEL:111\r\nTEL;PREF=2:222\r\nORG:Org\r\nNOTE:n1\r\nNOTE:n2\r\n",
            ),
            card("UID:d@oxidav\r\nFN:Dora\r\nN:Doe;Dora;;;\r\nTEL:9\r\n"),
            "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:e@oxidav\r\nFN:Eve\r\n\
             EMAIL;TYPE=INTERNET:eve@example.com\r\nTEL;TYPE=CELL:8\r\n\
             ORG:The Navy\r\nEND:VCARD\r\n"
                .to_string(),
            card(
                "UID:f@oxidav\r\nFN:Fox\r\nEMAIL:first@example.com\r\nEMAIL:second@example.com\r\n",
            ),
            sample_card(),
        ];

        for raw in payloads {
            let parsed = parse_vcf(&raw).expect("parse");
            let projection = project_card(&raw).expect("project");

            assert_eq!(projection.uid, parsed.uid, "uid: {raw:?}");
            assert_eq!(
                projection.fn_.as_deref(),
                Some(parsed.fn_.as_str()),
                "fn: {raw:?}"
            );

            // `email`/`phone` are the value of the lowest-`PREF` entry.
            assert_eq!(
                parsed.email,
                lowest_pref(&projection.emails),
                "email: {raw:?}"
            );
            assert_eq!(
                parsed.phone,
                lowest_pref(&projection.tels),
                "phone: {raw:?}"
            );

            // `organization` is `organizations[0]` joined with `"; "`.
            let expected_org = projection
                .organizations
                .first()
                .map(|components| components.join("; "))
                .filter(|value| !value.trim().is_empty());
            assert_eq!(parsed.organization, expected_org, "org: {raw:?}");

            // `notes` is the first `NOTE`.
            let expected_note = projection
                .notes
                .first()
                .cloned()
                .filter(|value| !value.trim().is_empty());
            assert_eq!(parsed.notes, expected_note, "note: {raw:?}");
        }
    }
}
