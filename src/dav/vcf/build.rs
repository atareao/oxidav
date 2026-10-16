//! F22 — vCard (`.vcf`) composition.
//!
//! The composition-side counterpart of [`super::parse_vcf`]: [`CardFields`]
//! holds the denormalised fields and [`build_vcf`] renders them into a minimal,
//! RFC 6350-valid `VCARD`. See the parent module docs for the parsing side.
//!
//! Since F22 the REST contact endpoints use [`build_vcf`] only to compose the
//! **skeleton** (`BEGIN`/`VERSION:4.0`/`UID`/`FN`/`END`); the rest of a card is
//! written by the surgical merge in [`super::merge`], which replaces only the
//! properties the client edited and leaves the stored payload otherwise **byte
//! for byte** intact.

/// The denormalised fields needed to compose a single-`VCARD` `.vcf`.
///
/// This is the composition-side counterpart of [`ParsedCard`]: where
/// [`parse_vcf`] extracts these fields *from* a payload, [`build_vcf`] renders
/// them *into* one.
///
/// Since F22 the REST contact endpoints use [`build_vcf`] only to compose the
/// **skeleton** (`BEGIN`/`VERSION:4.0`/`UID`/`FN`/`END`); the rest of a card is
/// written by the surgical merge in [`merge`](crate::dav::vcf::merge), which
/// replaces only the properties the client edited and leaves the stored payload
/// otherwise **byte for byte** intact. This struct is therefore no longer "the
/// single home of vCard serialisation": it is the minimal seed the merge starts
/// from.
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
        super::super::common::fold_content_line(line, &mut out);
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
///
/// The `merge` module's `escape_text` follows the **same** semantics (`\r` is
/// discarded, not turned into `\n`): there is one escape rule for vCard text in
/// this crate.
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
    use crate::dav::vcf::parse_vcf;

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
