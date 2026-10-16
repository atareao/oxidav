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
}
