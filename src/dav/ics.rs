//! F07 — iCalendar (`.ics`) parsing for CalDAV.
//!
//! [`parse_ics`] turns a raw `text/calendar` payload into the denormalised
//! [`ParsedEvent`] that F03's `calendar_objects` table stores (plus the raw
//! payload, which the caller keeps verbatim in `raw_payload`).
//!
//! # Date handling and documented limitations
//!
//! Dates are expressed as **Unix epoch milliseconds**, consistent with F03.
//! The raw `DTSTART`/`DTEND` property value is parsed directly so we control the
//! interpretation:
//!
//! * `VALUE=DATE` (all-day, `YYYYMMDD`): interpreted as **midnight UTC** of the
//!   given day. `DTEND` is exclusive per RFC 5545. Local visualisations can be
//!   off by ±1 day; this is a documented limitation.
//! * `DATE-TIME` with a `Z` suffix (UTC) is converted exactly.
//! * `DATE-TIME` with an explicit offset (`±HHMM`) is normalised to UTC. This
//!   form is not RFC 5545, but some clients emit it and the `icalendar` crate
//!   silently drops such a `DTSTART`, so it is handled here.
//! * `DATE-TIME` without a suffix (floating time or `TZID`) is treated **as if
//!   it were UTC**. `TZID`/`VTIMEZONE` resolution is **out of scope for F07**;
//!   the raw payload is preserved so a future plan can resolve it properly.
//! * `RRULE` is stored as opaque text and **not expanded** in F07.
//!
//! The structured fields (`UID`, `SUMMARY`, `DESCRIPTION`) come from the
//! `icalendar` crate; only the time fields are parsed manually.

use icalendar::{Calendar, Component};

/// Seconds in a day.
const SECONDS_PER_DAY: i64 = 86_400;
/// Milliseconds in a second.
const MILLIS_PER_SECOND: i64 = 1_000;

/// Errors produced while parsing an `.ics` payload.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IcsError {
    /// The payload is not a parseable `VCALENDAR`.
    #[error("iCalendar malformado: {0}")]
    Malformed(String),
    /// The calendar contains no `VEVENT` component.
    #[error("el calendario no contiene ningún VEVENT")]
    NoEvent,
    /// The first `VEVENT` has no (or an empty) `UID`.
    #[error("el VEVENT no tiene UID")]
    MissingUid,
    /// The first `VEVENT` has no parseable `DTSTART`.
    #[error("el VEVENT no tiene un DTSTART válido")]
    MissingStart,
}

/// The denormalised fields extracted from a single `VEVENT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    /// `UID`.
    pub uid: String,
    /// `SUMMARY`.
    pub summary: Option<String>,
    /// `DESCRIPTION`.
    pub description: Option<String>,
    /// `DTSTART` as Unix milliseconds (required by the CalDAV contract).
    pub dtstart_ms: i64,
    /// `DTEND` as Unix milliseconds, when present.
    pub dtend_ms: Option<i64>,
    /// Raw `RRULE` text (never expanded).
    pub rrule: Option<String>,
}

/// Parse a raw `.ics` payload and extract the first `VEVENT`.
///
/// # Errors
///
/// [`IcsError::Malformed`] if the payload is not a valid `VCALENDAR`,
/// [`IcsError::NoEvent`] if there is no `VEVENT`, [`IcsError::MissingUid`] if the
/// event has no `UID`, and [`IcsError::MissingStart`] if it has no parseable
/// `DTSTART`.
pub fn parse_ics(raw: &str) -> Result<ParsedEvent, IcsError> {
    let calendar: Calendar = raw
        .parse()
        .map_err(|err: String| IcsError::Malformed(err))?;

    let event = calendar.events().next().ok_or(IcsError::NoEvent)?;

    let uid = event
        .get_uid()
        .filter(|uid| !uid.trim().is_empty())
        .ok_or(IcsError::MissingUid)?
        .to_string();

    let summary = event.get_summary().map(str::to_string);
    let description = event.get_description().map(str::to_string);
    let rrule = event.property_value("RRULE").map(str::to_string);

    let properties = event.properties();
    let dtstart_ms = properties
        .get("DTSTART")
        .and_then(|prop| parse_datetime_ms(prop.value()))
        .ok_or(IcsError::MissingStart)?;
    let dtend_ms = properties
        .get("DTEND")
        .and_then(|prop| parse_datetime_ms(prop.value()));

    Ok(ParsedEvent {
        uid,
        summary,
        description,
        dtstart_ms,
        dtend_ms,
        rrule,
    })
}

/// Parse an iCalendar `DATE` or `DATE-TIME` value into Unix milliseconds.
///
/// Accepts `YYYYMMDD` (midnight UTC), `YYYYMMDDTHHMMSS`, `YYYYMMDDTHHMMSSZ` and
/// `YYYYMMDDTHHMMSS±HHMM`. A suffix-less date-time is treated as UTC (see the
/// module docs). Returns `None` for anything unrecognised.
///
/// Shared with the CalDAV `time-range` filter parser ([`super::xml`]), which uses
/// the same iCalendar date-time syntax per RFC 4791.
#[must_use]
pub(crate) fn parse_datetime_ms(value: &str) -> Option<i64> {
    let value = value.trim();

    // Pure date: YYYYMMDD.
    if value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let (year, month, day) = parse_date(value)?;
        let days = days_from_civil(year, month, day);
        return days
            .checked_mul(SECONDS_PER_DAY)?
            .checked_mul(MILLIS_PER_SECOND);
    }

    // Date-time: YYYYMMDDTHHMMSS[Z|±HHMM].
    //
    // Every slice below goes through `str::get`, which returns `None` on a
    // non-ASCII byte offset instead of panicking on a char boundary. The `index
    // 8 is b'T'` test proves the bytes are ASCII up to the separator, but the
    // time and offset segments may still contain multibyte characters, so they
    // must be sliced fallibly.
    let bytes = value.as_bytes();
    if bytes.len() < 15 || bytes[8] != b'T' {
        return None;
    }
    let (year, month, day) = parse_date(value.get(0..8)?)?;
    let (hour, minute, second) = parse_time(value.get(9..15)?)?;

    let offset_seconds = match value.get(15..) {
        // Floating time / TZID: treated as UTC (documented limitation).
        Some("") | Some("Z") => 0,
        Some(offset) => parse_offset(offset)?,
        // The end of the date-time lands mid-character: not a valid value.
        None => return None,
    };

    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(hour * 3_600 + minute * 60 + second)?
        .checked_sub(offset_seconds)?;
    seconds.checked_mul(MILLIS_PER_SECOND)
}

/// The denormalised fields needed to compose a single-`VEVENT` `.ics`.
///
/// This is the composition-side counterpart of [`ParsedEvent`]: where
/// [`parse_ics`] extracts these fields *from* a payload, [`build_ics`] renders
/// them *into* one. It is the single, shared home of iCalendar serialisation,
/// reused by the REST event endpoints (F11) instead of duplicating `.ics`
/// construction in the HTTP layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFields {
    /// `UID`.
    pub uid: String,
    /// `SUMMARY`.
    pub summary: Option<String>,
    /// `DESCRIPTION`.
    pub description: Option<String>,
    /// `DTSTART` as Unix milliseconds (required).
    pub dtstart_ms: i64,
    /// `DTEND` as Unix milliseconds, when present.
    pub dtend_ms: Option<i64>,
    /// Raw `RRULE` text (never expanded).
    pub rrule: Option<String>,
    /// `DTSTAMP` as Unix milliseconds (the moment of composition).
    pub dtstamp_ms: i64,
}

/// Compose a minimal, RFC 5545-valid `VCALENDAR` with a single `VEVENT`.
///
/// Every date-time is rendered as UTC (`…Z`) via [`format_datetime_ms`], so the
/// result round-trips exactly through [`parse_ics`] (which normalises a `Z`
/// suffix to the same instant).
///
/// Only genuinely free-text values (`SUMMARY`, `DESCRIPTION`) are escaped per
/// RFC 5545 §3.3.11, since those are the fields the parser unescapes. Structured
/// values (`UID`, `DTSTART`, `DTEND`, `RRULE`) are emitted verbatim: `RRULE` in
/// particular uses `;` and `=` as **separators**, so escaping it would corrupt
/// the rule (`FREQ=WEEKLY;COUNT=3` must stay such, not `FREQ=WEEKLY\;COUNT=3`).
/// Every content line is folded at 75 octets (RFC 5545 §3.1) without splitting
/// a UTF-8 character.
///
/// The output uses CRLF line endings and a stable property order, so a given
/// [`EventFields`] always yields the same bytes.
#[must_use]
pub fn build_ics(fields: &EventFields) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(12);
    lines.push("BEGIN:VCALENDAR".to_string());
    lines.push("VERSION:2.0".to_string());
    lines.push("PRODID:-//oxidav//oxidav//EN".to_string());
    lines.push("BEGIN:VEVENT".to_string());
    lines.push(format!("UID:{}", fields.uid));
    lines.push(format!("DTSTAMP:{}", format_datetime_ms(fields.dtstamp_ms)));
    if let Some(summary) = &fields.summary {
        lines.push(format!("SUMMARY:{}", escape_text(summary)));
    }
    if let Some(description) = &fields.description {
        lines.push(format!("DESCRIPTION:{}", escape_text(description)));
    }
    lines.push(format!("DTSTART:{}", format_datetime_ms(fields.dtstart_ms)));
    if let Some(dtend_ms) = fields.dtend_ms {
        lines.push(format!("DTEND:{}", format_datetime_ms(dtend_ms)));
    }
    if let Some(rrule) = &fields.rrule {
        lines.push(format!("RRULE:{rrule}"));
    }
    lines.push("END:VEVENT".to_string());
    lines.push("END:VCALENDAR".to_string());

    let mut out = String::with_capacity(256);
    for line in &lines {
        super::common::fold_content_line(line, &mut out);
    }
    out
}

/// Render Unix milliseconds as an iCalendar UTC date-time (`YYYYMMDDTHHMMSSZ`).
///
/// The exact inverse of [`parse_datetime_ms`] for UTC values: for any `ms`,
/// `parse_datetime_ms(&format_datetime_ms(ms)) == Some(ms)`. Uses floor division
/// so negative (pre-1970) instants stay consistent.
#[must_use]
pub fn format_datetime_ms(ms: i64) -> String {
    let total_seconds = ms.div_euclid(MILLIS_PER_SECOND);
    let days = total_seconds.div_euclid(SECONDS_PER_DAY);
    let seconds_of_day = total_seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// Escape a free-text value for a content line (RFC 5545 §3.3.11).
///
/// This is applied **only** to free-text properties (`SUMMARY`, `DESCRIPTION`)
/// whose values the parser unescapes on the way back. It must **not** be used
/// for structured values such as `RRULE`, whose `;`/`,`/`=` are separators, nor
/// for `UID`/`DTSTART`/`DTEND` which are emitted verbatim.
///
/// Backslash, comma and semicolon gain a leading backslash; CR/LF become the
/// two-character sequence `\n`. A bare `\r` is dropped so CRLF input collapses
/// to a single logical newline, matching [`parse_ics`]'s unescaping.
fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Civil date (proleptic Gregorian) from days since the Unix epoch.
///
/// Howard Hinnant's `civil_from_days` algorithm; the exact inverse of
/// [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

/// Parse `YYYYMMDD`, validating month/day ranges (including leap years).
fn parse_date(value: &str) -> Option<(i64, u32, u32)> {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year: i64 = value[0..4].parse().ok()?;
    let month: u32 = value[4..6].parse().ok()?;
    let day: u32 = value[6..8].parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some((year, month, day))
}

/// Parse `HHMMSS`, allowing `60` for a leap second.
fn parse_time(value: &str) -> Option<(i64, i64, i64)> {
    if value.len() != 6 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour: i64 = value[0..2].parse().ok()?;
    let minute: i64 = value[2..4].parse().ok()?;
    let second: i64 = value[4..6].parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some((hour, minute, second))
}

/// Parse a `±HHMM` UTC offset into seconds.
///
/// The sign byte at index 0 and the four `HHMM` digits must all be ASCII; that
/// is validated with `is_ascii_digit` *before* any slice, so a value carrying a
/// multibyte character (e.g. `+1é2`) returns `None` rather than panicking on a
/// char boundary.
fn parse_offset(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 5 {
        return None;
    }
    let sign = match bytes[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    // `bytes[1..]` is a byte-slice index (never panics on UTF-8); requiring all
    // four remaining bytes to be ASCII digits guarantees the `value[1..3]` /
    // `value[3..5]` slices below sit on char boundaries.
    if !bytes[1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let hour: i64 = value.get(1..3)?.parse().ok()?;
    let minute: i64 = value.get(3..5)?.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(sign * (hour * 3_600 + minute * 60))
}

/// Number of days in `month` for the given `year` (proleptic Gregorian).
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Whether `year` is a leap year in the proleptic Gregorian calendar.
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days since the Unix epoch (1970-01-01) for a civil date.
///
/// Howard Hinnant's `days_from_civil` algorithm; exact for the proleptic
/// Gregorian calendar.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const VCAL_PROLOG: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n";
    const VCAL_EPILOG: &str = "END:VCALENDAR\r\n";

    fn calendar(body: &str) -> String {
        format!("{VCAL_PROLOG}{body}{VCAL_EPILOG}")
    }

    fn datetime_ms(value: &str) -> i64 {
        parse_datetime_ms(value).expect("valid date-time")
    }

    #[test]
    fn parses_a_simple_utc_event() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:simple@oxidav\r\nSUMMARY:Reunión\r\n\
             DESCRIPTION:Line one\\nLine two\r\nDTSTART:20240315T100000Z\r\n\
             DTEND:20240315T110000Z\r\nEND:VEVENT\r\n",
        );

        let event = parse_ics(&raw).expect("parse");

        assert_eq!(event.uid, "simple@oxidav");
        assert_eq!(event.summary.as_deref(), Some("Reunión"));
        assert_eq!(event.description.as_deref(), Some("Line one\nLine two"));
        assert_eq!(event.dtstart_ms, 1_710_496_800_000);
        assert_eq!(event.dtend_ms, Some(1_710_500_400_000));
        assert_eq!(event.rrule, None);
    }

    #[test]
    fn keeps_rrule_as_text_without_expanding() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:repeat@oxidav\r\nSUMMARY:Weekly\r\n\
             DTSTART;TZID=Europe/Madrid:20240315T100000\r\n\
             RRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\n",
        );

        let event = parse_ics(&raw).expect("parse");

        assert_eq!(event.rrule.as_deref(), Some("FREQ=WEEKLY;COUNT=3"));
        // TZID is not resolved in F07: the naive time is treated as UTC.
        assert_eq!(event.dtstart_ms, 1_710_496_800_000);
    }

    #[test]
    fn parses_all_day_value_date_as_utc_midnight() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:allday@oxidav\r\nSUMMARY:Holiday\r\n\
             DTSTART;VALUE=DATE:20240315\r\nDTEND;VALUE=DATE:20240316\r\nEND:VEVENT\r\n",
        );

        let event = parse_ics(&raw).expect("parse");

        assert_eq!(event.dtstart_ms, 1_710_460_800_000);
        // DTEND is exclusive: next day at midnight UTC.
        assert_eq!(event.dtend_ms, Some(1_710_547_200_000));
    }

    #[test]
    fn parses_a_datetime_with_explicit_offset() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:offset@oxidav\r\nSUMMARY:Offset\r\n\
             DTSTART:20240315T100000+0100\r\nDTEND:20240315T110000-0530\r\nEND:VEVENT\r\n",
        );

        let event = parse_ics(&raw).expect("parse");

        // 10:00+01:00 == 09:00Z.
        assert_eq!(event.dtstart_ms, 1_710_493_200_000);
        // 11:00-05:30 == 16:30Z.
        assert_eq!(event.dtend_ms, Some(1_710_520_200_000));
    }

    #[test]
    fn keeps_utc_datetime_without_suffix_as_utc() {
        assert_eq!(datetime_ms("20240315T100000"), 1_710_496_800_000);
        assert_eq!(datetime_ms("20240315T100000Z"), 1_710_496_800_000);
    }

    #[test]
    fn accepts_optional_seconds_and_leap_second() {
        assert_eq!(datetime_ms("20240315T100000Z"), 1_710_496_800_000);
        assert!(parse_datetime_ms("20240315T100060Z").is_some());
    }

    #[test]
    fn rejects_invalid_datetimes() {
        assert!(parse_datetime_ms("not-a-date").is_none());
        assert!(parse_datetime_ms("20241315T100000Z").is_none()); // month 13
        assert!(parse_datetime_ms("20240230T100000Z").is_none()); // feb 30
        assert!(parse_datetime_ms("20240315T250000Z").is_none()); // hour 25
        assert!(parse_datetime_ms("20240315T100000+9900").is_none()); // bad offset
        assert!(parse_datetime_ms("20240315T100000+01").is_none()); // short offset
        assert!(parse_datetime_ms("20240315").is_some()); // date is valid
        assert!(parse_datetime_ms("2024031").is_none()); // short date
    }

    #[test]
    fn multibyte_input_never_panics_on_a_slice() {
        // Regression for the byte-slicing panic (B1): a multibyte character in
        // the field separator, the time segment or the UTC offset used to abort
        // the process with "byte index N is not a char boundary". Every case
        // must yield `None`, never panic.
        //
        // Multibyte character straddling the `15` boundary: `é` occupies bytes
        // 14..16, so `value[9..15]` would have cut it in half.
        assert_eq!(parse_datetime_ms("20240315T12345é"), None);
        // Multibyte character inside the time segment.
        assert_eq!(parse_datetime_ms("20240315T1é3456"), None);
        // Multibyte character at the start of the offset segment.
        assert_eq!(parse_datetime_ms("20240315T10000éZ"), None);
        // Multibyte characters inside an otherwise well-sized offset.
        assert_eq!(parse_datetime_ms("20240315T100000+1é2"), None);
        assert_eq!(parse_datetime_ms("20240315T100000+é12"), None);
        assert_eq!(parse_datetime_ms("20240315T100000-1é2"), None);
        // A multibyte character in the date segment is rejected up front.
        assert_eq!(parse_datetime_ms("2024é315T100000Z"), None);
        // The offset helper on its own must also be panic-free.
        assert_eq!(parse_offset("+1é2"), None);
        assert_eq!(parse_offset("é1234"), None);
        assert_eq!(parse_offset("+éééé"), None);
    }

    #[test]
    fn leap_years_are_validated() {
        assert!(parse_datetime_ms("20240229").is_some());
        assert!(parse_datetime_ms("20230229").is_none());
        assert!(parse_datetime_ms("19000229").is_none());
        assert!(parse_datetime_ms("20000229").is_some());
    }

    #[test]
    fn rejects_malformed_calendars() {
        assert!(matches!(parse_ics("garbage"), Err(IcsError::Malformed(_))));
    }

    #[test]
    fn rejects_a_calendar_without_events() {
        let raw = calendar("");
        assert!(matches!(parse_ics(&raw), Err(IcsError::NoEvent)));
    }

    #[test]
    fn rejects_an_event_without_uid() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nSUMMARY:No uid\r\nDTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );
        assert!(matches!(parse_ics(&raw), Err(IcsError::MissingUid)));
    }

    #[test]
    fn rejects_an_event_without_dtstart() {
        let raw = calendar("BEGIN:VEVENT\r\nUID:nostart@oxidav\r\nSUMMARY:x\r\nEND:VEVENT\r\n");
        assert!(matches!(parse_ics(&raw), Err(IcsError::MissingStart)));
    }

    #[test]
    fn format_datetime_ms_round_trips_through_parse() {
        for ms in [
            0,
            1_710_496_800_000,
            1_700_000_000_000,
            -86_400_000,
            1_710_496_859_000,
        ] {
            let rendered = format_datetime_ms(ms);
            assert_eq!(
                parse_datetime_ms(&rendered),
                Some(ms),
                "round-trip failed for {ms} -> {rendered}"
            );
        }
    }

    #[test]
    fn format_datetime_ms_matches_known_instants() {
        assert_eq!(format_datetime_ms(0), "19700101T000000Z");
        assert_eq!(format_datetime_ms(1_710_496_800_000), "20240315T100000Z");
    }

    #[test]
    fn build_ics_round_trips_all_fields() {
        let fields = EventFields {
            uid: "composed@oxidav".to_string(),
            summary: Some("Reunión: equipo".to_string()),
            description: Some("Line one\nLine two; with, punctuation".to_string()),
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            rrule: Some("FREQ=WEEKLY;COUNT=3".to_string()),
            dtstamp_ms: 1_710_496_800_000,
        };

        let raw = build_ics(&fields);
        let parsed = parse_ics(&raw).expect("composed payload must parse");

        assert_eq!(parsed.uid, fields.uid);
        assert_eq!(parsed.summary, fields.summary);
        assert_eq!(parsed.description, fields.description);
        assert_eq!(parsed.dtstart_ms, fields.dtstart_ms);
        assert_eq!(parsed.dtend_ms, fields.dtend_ms);
        assert_eq!(parsed.rrule, fields.rrule);
    }

    #[test]
    fn build_ics_keeps_rrule_verbatim_but_escapes_text() {
        // RRULE is a structured value: `;` and `=` are separators and must not
        // be escaped. Free-text fields must still escape `;`, `,`, `\` and
        // newlines, and the whole thing must round-trip through `parse_ics`.
        let fields = EventFields {
            uid: "structured@oxidav".to_string(),
            summary: Some("A;B,C\\D".to_string()),
            description: Some("line1\nline2; end, back\\slash".to_string()),
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            rrule: Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=3".to_string()),
            dtstamp_ms: 1_710_496_800_000,
        };

        let raw = build_ics(&fields);

        // The rule is emitted exactly as supplied, separators intact.
        assert!(
            raw.contains("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=3"),
            "{raw}"
        );
        // Text fields are escaped on the wire.
        assert!(raw.contains("SUMMARY:A\\;B\\,C\\\\D"), "{raw}");

        let parsed = parse_ics(&raw).expect("composed payload must parse");
        assert_eq!(parsed.uid, fields.uid);
        assert_eq!(parsed.rrule, fields.rrule);
        assert_eq!(parsed.summary, fields.summary);
        assert_eq!(parsed.description, fields.description);
    }

    #[test]
    fn build_ics_omits_absent_optional_fields() {
        let fields = EventFields {
            uid: "minimal@oxidav".to_string(),
            summary: None,
            description: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            rrule: None,
            dtstamp_ms: 1_710_496_800_000,
        };

        let raw = build_ics(&fields);
        assert!(!raw.contains("SUMMARY:"));
        assert!(!raw.contains("DESCRIPTION:"));
        assert!(!raw.contains("DTEND:"));
        assert!(!raw.contains("RRULE:"));

        let parsed = parse_ics(&raw).expect("parse");
        assert_eq!(parsed.uid, fields.uid);
        assert_eq!(parsed.dtend_ms, None);
    }

    #[test]
    fn build_ics_folds_long_lines_without_splitting_characters() {
        // 100 characters guarantees at least one fold; the multi-byte characters
        // must stay intact and the payload must still parse.
        let summary = "é".repeat(100);
        let fields = EventFields {
            uid: "long@oxidav".to_string(),
            summary: Some(summary.clone()),
            description: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            rrule: None,
            dtstamp_ms: 1_710_496_800_000,
        };

        let raw = build_ics(&fields);
        assert!(raw.contains("\r\n "), "long line must be folded");
        for line in raw.split("\r\n") {
            assert!(line.len() <= 75, "physical line exceeds 75 octets: {line}");
        }
        let parsed = parse_ics(&raw).expect("folded payload must parse");
        assert_eq!(parsed.summary, Some(summary));
    }

    #[test]
    fn build_ics_escapes_reserved_characters() {
        let fields = EventFields {
            uid: "uid-b@x".to_string(),
            summary: Some("a\\b;c,d".to_string()),
            description: None,
            dtstart_ms: 0,
            dtend_ms: None,
            rrule: None,
            dtstamp_ms: 0,
        };

        let raw = build_ics(&fields);
        assert!(raw.contains("SUMMARY:a\\\\b\\;c\\,d"), "{raw}");
        let parsed = parse_ics(&raw).expect("parse");
        assert_eq!(parsed.summary.as_deref(), Some("a\\b;c,d"));
    }
}
