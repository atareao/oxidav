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
//! unvalidated offset, mirroring the hardened slicing of `dav/ics/mod.rs`, so a payload
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

mod build;
pub mod merge;
pub mod projection;

pub use build::{CardFields, build_vcf};
pub use projection::{
    Address, CardProjection, Gender, MediaValue, MultiValue, RawProperty, StructuredName,
    project_card,
};

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
    let (card, _normalized) = parse_normalized(raw)?;
    parsed_card(&card)
}

/// Parse `raw` with the [`vcard4`] crate, returning the first card **and** the
/// exact text that was parsed.
///
/// This is the crate parse shared by [`parse_vcf`] and
/// [`super::projection::project_card`]: both build their view from the *same*
/// [`vcard4::Vcard`], so the crate parses a payload once per request instead of
/// twice. The returned [`Cow`] is the payload unchanged when it already carries
/// an `FN` ([`Cow::Borrowed`]), or a copy with the synthetic `FN` injected
/// before `END:VCARD` ([`Cow::Owned`]) — the normalisation described in the
/// module docs.
///
/// # Errors
///
/// [`VcfError::Empty`] for an empty payload, [`VcfError::NotACard`] when the
/// envelope is missing or no card is produced, [`VcfError::MissingUid`] when no
/// `FN` can be guaranteed, and [`VcfError::Malformed`] when the enveloped
/// payload is not parseable.
fn parse_normalized(raw: &str) -> Result<(vcard4::Vcard, Cow<'_, str>), VcfError> {
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
    let card = cards.into_iter().next().ok_or(VcfError::NotACard)?;

    Ok((card, normalized))
}

/// Build the denormalised [`ParsedCard`] from an already-parsed card.
///
/// Split out of [`parse_vcf`] so [`super::projection::project_card`] can reuse
/// the same `UID`/`FN`/`EMAIL`/`TEL`/`ORG`/`NOTE` extraction — and therefore
/// the same validation errors — without a second crate parse.
fn parsed_card(card: &vcard4::Vcard) -> Result<ParsedCard, VcfError> {
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

/// Whether a character is forbidden in a serialised content line.
///
/// Covers C0/C1 control characters (including CR, LF and TAB) and the Unicode
/// line/paragraph separators U+2028/U+2029, which some vCard parsers treat as
/// line breaks. This is the **single** predicate shared by the REST field
/// validators (`crate::api::contacts`) and the `merge` render helpers: rejecting
/// such characters up front is the security boundary against content-line /
/// property injection (decision 8, «rechazar, no sanear»).
#[must_use]
pub(crate) fn is_forbidden_char(c: char) -> bool {
    c.is_control() || c == '\u{2028}' || c == '\u{2029}'
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
}
