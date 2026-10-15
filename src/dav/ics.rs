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
//! * `DATE-TIME` without a suffix is a **floating** time with no zone: absent a
//!   `TZID` parameter it is treated **as if it were UTC** (the F07 behaviour).
//!   When the property *does* carry a `TZID`, the local time is resolved to UTC
//!   through the embedded IANA tzdb (`chrono-tz`) and the canonical zone name is
//!   recorded (F18). This feature **lifts** the F07 limitation that left
//!   `TZID`/`VTIMEZONE` resolution out of scope; an **unknown/unresolvable**
//!   `TZID` (e.g. a Windows zone name) still degrades to the old
//!   treat-the-local-time-as-UTC behaviour with no zone recorded, so a `PUT`
//!   carrying a non-IANA name is never rejected and the raw payload is preserved.
//! * `RRULE` is stored as opaque text and **not expanded** in F07.
//!
//! The structured fields (`UID`, `SUMMARY`, `DESCRIPTION`) come from the
//! `icalendar` crate; only the time fields are parsed manually.

use icalendar::{Calendar, Component, Property};

use super::vtimezone;
use chrono::{DateTime, LocalResult, TimeZone};
use chrono_tz::{GapInfo, OffsetComponents, Tz};

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
    /// `DTSTART`'s canonical IANA `TZID`, resolved from the property's `TZID`
    /// parameter (F18). `None` when the property had no `TZID` or its value was
    /// not a known IANA zone.
    pub dtstart_tzid: Option<String>,
    /// `DTEND`'s canonical IANA `TZID`, when its property carried one.
    pub dtend_tzid: Option<String>,
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
    let (dtstart_ms, dtstart_tzid) = properties
        .get("DTSTART")
        .and_then(parse_property_datetime)
        .ok_or(IcsError::MissingStart)?;
    let (dtend_ms, dtend_tzid) = match properties.get("DTEND").and_then(parse_property_datetime) {
        Some((ms, tzid)) => (Some(ms), tzid),
        None => (None, None),
    };

    Ok(ParsedEvent {
        uid,
        summary,
        description,
        dtstart_ms,
        dtend_ms,
        dtstart_tzid,
        dtend_tzid,
        rrule,
    })
}

/// Parse a `DTSTART`/`DTEND` [`Property`] into Unix milliseconds *and* the
/// canonical IANA zone resolved from its `TZID` parameter (F18).
///
/// The `TZID` parameter travels alongside the value here — this is the whole
/// point of the F18 change: before it, [`parse_datetime_ms`] saw only the value
/// and silently read `;TZID=Europe/Madrid` local times as if they were UTC.
fn parse_property_datetime(prop: &Property) -> Option<(i64, Option<String>)> {
    let tzid = prop.params().get("TZID").map(|param| param.value());
    parse_datetime_with_tzid(prop.value(), tzid)
}

/// Parse an iCalendar `DATE` or `DATE-TIME` value into Unix milliseconds.
///
/// Accepts `YYYYMMDD` (midnight UTC), `YYYYMMDDTHHMMSS`, `YYYYMMDDTHHMMSSZ` and
/// `YYYYMMDDTHHMMSS±HHMM`. A suffix-less date-time is treated as UTC (see the
/// module docs) because this entry point carries no `TZID` parameter; the
/// `TZID`-aware path is [`parse_datetime_with_tzid`].
///
/// Shared with the CalDAV `time-range` filter parser ([`super::xml`]), which uses
/// the same iCalendar date-time syntax per RFC 4791 and never carries a `TZID`.
#[must_use]
pub(crate) fn parse_datetime_ms(value: &str) -> Option<i64> {
    parse_datetime_with_tzid(value, None).map(|(ms, _)| ms)
}

/// Parse a `DATE`/`DATE-TIME` value into Unix milliseconds, resolving a
/// suffix-less local time through the IANA zone named by `tzid` (F18).
///
/// Returns the instant **and** the canonical zone name that was applied, or
/// `None` as the zone when the value was absolute (`Z`/`±HHMM`/`VALUE=DATE`) or
/// when the time was floating. Resolution rules for a `TZID`-governed local time
/// (RFC 5545 §3.3.5):
///
/// * unambiguous → that instant;
/// * **ambiguous** (the repeated hour of an autumn fall-back) → the **first**
///   interpretation, i.e. the offset in force *before* the transition;
/// * **nonexistent** (the skipped hour of a spring gap) → the offset in force
///   *before* the gap, recovered from [`GapInfo`].
///
/// An **unresolvable** `TZID` (a non-IANA name such as a Windows zone token) is
/// **not** an error: the local time degrades to the old treat-as-UTC behaviour
/// and no zone is recorded. That tolerance is deliberate — it keeps a `PUT` from
/// a client that emits non-IANA names working, and `raw_payload` is stored
/// verbatim, so nothing is lost.
pub(crate) fn parse_datetime_with_tzid(
    value: &str,
    tzid: Option<&str>,
) -> Option<(i64, Option<String>)> {
    let value = value.trim();

    // Pure date: YYYYMMDD. `TZID` does not apply to a `VALUE=DATE` (an all-day
    // date is zone-less, pinned to midnight UTC as documented).
    if value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let (year, month, day) = parse_date(value)?;
        let ms = days_from_civil(year, month, day)
            .checked_mul(SECONDS_PER_DAY)?
            .checked_mul(MILLIS_PER_SECOND)?;
        return Some((ms, None));
    }

    // Date-time: YYYYMMDDTHHMMSS[Z|±HHMM|(local)].
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

    let seconds = days_from_civil(year, month, day)
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(hour * 3_600 + minute * 60 + second)?;

    match value.get(15..) {
        // Explicit UTC.
        Some("Z") => Some((seconds.checked_mul(MILLIS_PER_SECOND)?, None)),
        // Suffix-less: floating, or a local time governed by `TZID`.
        Some("") => resolve_local(seconds, tzid),
        // Explicit `±HHMM` offset: normalise to UTC.
        Some(offset) => {
            let offset_seconds = parse_offset(offset)?;
            Some((
                seconds
                    .checked_sub(offset_seconds)?
                    .checked_mul(MILLIS_PER_SECOND)?,
                None,
            ))
        }
        // The end of the date-time lands mid-character: not a valid value.
        None => None,
    }
}

/// Resolve a suffix-less local time (`naive_seconds`, a wall-clock instant read
/// as if it were UTC) to Unix milliseconds using `tzid` when it names a known
/// IANA zone.
///
/// Falls back to treating the wall-clock as UTC (and records no zone) when
/// `tzid` is absent or unresolvable; see [`parse_datetime_with_tzid`].
fn resolve_local(naive_seconds: i64, tzid: Option<&str>) -> Option<(i64, Option<String>)> {
    match tzid.and_then(vtimezone::parse_tzid) {
        Some(tz) => {
            let ms = local_to_utc_ms(tz, naive_seconds)?;
            Some((ms, Some(vtimezone::canonical_tzid(tz).to_string())))
        }
        None => Some((naive_seconds.checked_mul(MILLIS_PER_SECOND)?, None)),
    }
}

/// Convert a wall-clock time, expressed as the number of seconds since the Unix
/// epoch in the local calendar of `tz`, into Unix milliseconds (F18).
///
/// Follows RFC 5545 §3.3.5 for the two degenerate cases: an **ambiguous** local
/// time (autumn fall-back) resolves to the **earlier** instant (the offset before
/// the transition), and a **nonexistent** local time (spring gap) is interpreted
/// with the offset in force **before** the gap. When the offset cannot be
/// recovered (only possible at the extreme ends of the tzdb record) the wall
/// clock is read as UTC, the same degradation as an unknown `TZID`.
fn local_to_utc_ms(tz: Tz, naive_seconds: i64) -> Option<i64> {
    let naive = DateTime::from_timestamp(naive_seconds, 0)?.naive_utc();
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.timestamp_millis()),
        LocalResult::Ambiguous(first, _second) => Some(first.timestamp_millis()),
        LocalResult::None => {
            // Spring-forward gap: `GapInfo::begin` carries the offset in force
            // immediately before the gap.
            let before = GapInfo::new(&naive, &tz).and_then(|info| info.begin)?;
            let offset_seconds =
                before.1.base_utc_offset().num_seconds() + before.1.dst_offset().num_seconds();
            naive_seconds
                .checked_sub(offset_seconds)?
                .checked_mul(MILLIS_PER_SECOND)
        }
    }
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
    /// IANA zone the event is anchored to (F18): when set, `DTSTART` is emitted
    /// as `DTSTART;TZID=<canonical>` in local time and the zone's `VTIMEZONE`
    /// block is embedded. `None` keeps the pre-F18 UTC output byte-for-byte.
    pub dtstart_tzid: Option<String>,
    /// IANA zone for `DTEND`; when `None` a present `DTEND` **inherits**
    /// `dtstart_tzid` (a `DTEND` without a `TZID` would otherwise be floating).
    pub dtend_tzid: Option<String>,
    /// Raw `RRULE` text (never expanded).
    pub rrule: Option<String>,
    /// `DTSTAMP` as Unix milliseconds (the moment of composition).
    pub dtstamp_ms: i64,
}

/// Compose a minimal, RFC 5545-valid `VCALENDAR` with a single `VEVENT`.
///
/// With no `tzid` every date-time is rendered as UTC (`…Z`) via
/// [`format_datetime_ms`], so the result round-trips exactly through
/// [`parse_ics`] (which normalises a `Z` suffix to the same instant). That path
/// is **byte-for-byte identical to the pre-F18 output** and is a hard contract.
///
/// When `dtstart_tzid`/`dtend_tzid` are set (F18), `DTSTART`/`DTEND` are emitted
/// as `;TZID=<canonical>:<local wall-clock>` (no `Z`) and the zone definition —
/// a `VTIMEZONE` block per distinct zone, from [`vtimezone::render_vtimezone`] —
/// is spliced in between `PRODID` and `BEGIN:VEVENT`, as RFC 5545 §3.2.19
/// requires. The zone's reference year is that of `DTSTART`.
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
    let dtstart_tz = fields
        .dtstart_tzid
        .as_deref()
        .and_then(vtimezone::parse_tzid);
    // A `DTEND` with no zone of its own inherits `DTSTART`'s, so it is never
    // emitted as a floating (zone-less) time next to a zoned `DTSTART`.
    let dtend_tz = fields
        .dtend_tzid
        .as_deref()
        .and_then(vtimezone::parse_tzid)
        .or(dtstart_tz);

    // Distinct zones to define, in first-seen order (dtstart before dtend).
    let mut zones: Vec<Tz> = Vec::new();
    for tz in [dtstart_tz, dtend_tz].into_iter().flatten() {
        if !zones.contains(&tz) {
            zones.push(tz);
        }
    }

    let mut body: Vec<String> = Vec::with_capacity(9);
    body.push("BEGIN:VEVENT".to_string());
    body.push(format!("UID:{}", fields.uid));
    body.push(format!("DTSTAMP:{}", format_datetime_ms(fields.dtstamp_ms)));
    if let Some(summary) = &fields.summary {
        body.push(format!("SUMMARY:{}", escape_text(summary)));
    }
    if let Some(description) = &fields.description {
        body.push(format!("DESCRIPTION:{}", escape_text(description)));
    }
    body.push(match dtstart_tz {
        Some(tz) => format!(
            "DTSTART;TZID={}:{}",
            vtimezone::canonical_tzid(tz),
            vtimezone::format_local_datetime_ms(tz, fields.dtstart_ms)
        ),
        None => format!("DTSTART:{}", format_datetime_ms(fields.dtstart_ms)),
    });
    if let Some(dtend_ms) = fields.dtend_ms {
        body.push(match dtend_tz {
            Some(tz) => format!(
                "DTEND;TZID={}:{}",
                vtimezone::canonical_tzid(tz),
                vtimezone::format_local_datetime_ms(tz, dtend_ms)
            ),
            None => format!("DTEND:{}", format_datetime_ms(dtend_ms)),
        });
    }
    if let Some(rrule) = &fields.rrule {
        body.push(format!("RRULE:{rrule}"));
    }
    body.push("END:VEVENT".to_string());
    body.push("END:VCALENDAR".to_string());

    let mut out = String::with_capacity(256);
    super::common::fold_content_line("BEGIN:VCALENDAR", &mut out);
    super::common::fold_content_line("VERSION:2.0", &mut out);
    super::common::fold_content_line("PRODID:-//oxidav//oxidav//EN", &mut out);
    if !zones.is_empty() {
        let reference_year = reference_year(fields, &zones);
        for &tz in &zones {
            out.push_str(&vtimezone::render_vtimezone(tz, reference_year));
        }
    }
    for line in &body {
        super::common::fold_content_line(line, &mut out);
    }
    out
}

/// The reference year for the `VTIMEZONE` observances: the **local** year of
/// `DTSTART` (or, when only `DTEND` carries a zone, of `DTEND`) in the anchor
/// zone `zones[0]`.
fn reference_year(fields: &EventFields, zones: &[Tz]) -> i32 {
    let anchor = zones[0];
    let anchor_ms = if fields.dtstart_tzid.is_some() {
        fields.dtstart_ms
    } else {
        fields.dtend_ms.unwrap_or(fields.dtstart_ms)
    };
    let local = vtimezone::format_local_datetime_ms(anchor, anchor_ms);
    local
        .get(0..4)
        .and_then(|year| year.parse().ok())
        .unwrap_or(2024)
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

/// Smallest calendar year a `DTSTART`/`DTEND` value may name.
///
/// Mirrors [`parse_date`], which reads a four-digit `YYYY`; the REST layer reuses
/// this bound so the API and the DAV parser accept exactly the same instants.
pub const MIN_EVENT_YEAR: i64 = 1;

/// Largest calendar year a `DTSTART`/`DTEND` value may name.
pub const MAX_EVENT_YEAR: i64 = 9_999;

/// The half-open range `[min, max)` of Unix milliseconds whose UTC rendering is a
/// valid iCalendar date-time (a four-digit year in
/// [`MIN_EVENT_YEAR`]..=[`MAX_EVENT_YEAR`]).
///
/// This is the sanity bound the REST API applies to `dtstart`/`dtend`: those
/// fields are arbitrary `i64` milliseconds from the client, and a value such as
/// `i64::MAX` cannot be represented as an iCalendar date-time (nor projected into
/// a zone), so it must be rejected (`400`) rather than reaching composition. The
/// bounds are derived from [`days_from_civil`] — the very helper the parser uses —
/// so both layers agree to the millisecond.
#[must_use]
pub fn supported_instant_range_ms() -> (i64, i64) {
    let lo = days_from_civil(MIN_EVENT_YEAR, 1, 1) * SECONDS_PER_DAY * MILLIS_PER_SECOND;
    let hi = days_from_civil(MAX_EVENT_YEAR + 1, 1, 1) * SECONDS_PER_DAY * MILLIS_PER_SECOND;
    (lo, hi)
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
        // F18: TZID is resolved. 10:00 in Europe/Madrid on 2024-03-15 is CET
        // (UTC+1), i.e. 09:00 UTC. The pre-F18 assertion of 1_710_496_800_000
        // (= 10:00 UTC) was wrong and consigned the one-hour bug.
        assert_eq!(event.dtstart_ms, 1_710_493_200_000);
        assert_eq!(event.dtstart_tzid.as_deref(), Some("Europe/Madrid"));
    }

    // -- F18 Paso 8/9 — `TZID` resolution and DST edge cases -----------------

    #[test]
    fn resolves_tzid_local_time_to_utc() {
        let (ms, tzid) =
            parse_datetime_with_tzid("20240315T100000", Some("Europe/Madrid")).expect("parse");
        assert_eq!(ms, 1_710_493_200_000);
        assert_eq!(tzid.as_deref(), Some("Europe/Madrid"));
    }

    #[test]
    fn spring_forward_gap_uses_the_offset_before_the_gap() {
        // 2024-03-31 02:00 CET -> 03:00 CEST: 02:30 local does not exist. RFC
        // 5545 §3.3.5 says interpret with the offset in force before the gap
        // (CET, +01:00) => 01:30 UTC.
        let (ms, tzid) =
            parse_datetime_with_tzid("20240331T023000", Some("Europe/Madrid")).expect("parse");
        assert_eq!(ms, 1_711_848_600_000);
        assert_eq!(tzid.as_deref(), Some("Europe/Madrid"));
    }

    #[test]
    fn fall_back_ambiguous_time_takes_the_first_interpretation() {
        // 2024-10-27 03:00 CEST -> 02:00 CET: 02:30 local happens twice. RFC
        // 5545 §3.3.5 says take the first, i.e. the offset before the
        // transition (CEST, +02:00) => 00:30 UTC.
        let (ms, tzid) =
            parse_datetime_with_tzid("20241027T023000", Some("Europe/Madrid")).expect("parse");
        assert_eq!(ms, 1_729_989_000_000);
        assert_eq!(tzid.as_deref(), Some("Europe/Madrid"));
    }

    #[test]
    fn unknown_tzid_degrades_to_treating_the_time_as_utc() {
        // A non-IANA (e.g. Windows) zone name must not fail the parse: the local
        // time is read as UTC and no zone is recorded, so a client's `PUT` is
        // tolerated and its `raw_payload` is preserved.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:win@oxidav\r\nSUMMARY:Windows\r\n\
             DTSTART;TZID=W. Europe Standard Time:20240315T100000\r\nEND:VEVENT\r\n",
        );
        let event = parse_ics(&raw).expect("parse");
        assert_eq!(event.dtstart_ms, 1_710_496_800_000); // 10:00 as UTC
        assert_eq!(event.dtstart_tzid, None);
    }

    #[test]
    fn parse_reports_dtend_tzid() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:range@oxidav\r\nSUMMARY:Range\r\n\
             DTSTART;TZID=Europe/Madrid:20240315T100000\r\n\
             DTEND;TZID=Asia/Tokyo:20240315T200000\r\nEND:VEVENT\r\n",
        );
        let event = parse_ics(&raw).expect("parse");
        assert_eq!(event.dtstart_tzid.as_deref(), Some("Europe/Madrid"));
        assert_eq!(event.dtend_tzid.as_deref(), Some("Asia/Tokyo"));
        // 2024-03-15 20:00 JST (UTC+9) == 11:00 UTC.
        assert_eq!(event.dtend_ms, Some(1_710_500_400_000));
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
    fn format_datetime_ms_never_panics_on_extreme_ms() {
        // The non-tzid path must be panic-free for arbitrary `i64` milliseconds:
        // `civil_from_days` is exact over the whole `i64` range, so an absurd
        // instant still renders deterministically.
        for ms in [i64::MAX, i64::MIN, 0] {
            let first = format_datetime_ms(ms);
            let second = format_datetime_ms(ms);
            assert_eq!(first, second, "{ms}: not deterministic");
        }
    }

    #[test]
    fn supported_instant_range_brackets_four_digit_years() {
        let (lo, hi) = supported_instant_range_ms();
        // The bounds are the instants at the start of year 1 and year 10000.
        assert_eq!(format_datetime_ms(lo), "00010101T000000Z");
        assert_eq!(format_datetime_ms(hi), "100000101T000000Z");
        // The bounds are strictly inside the `i64` range and a 2024 instant is
        // comfortably between them.
        assert!(lo < 1_710_496_800_000 && 1_710_496_800_000 < hi);
        assert!(lo > i64::MIN + 1);
        assert!(hi < i64::MAX - 1);
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
            dtstart_tzid: None,
            dtend_tzid: None,
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
            dtstart_tzid: None,
            dtend_tzid: None,
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
            dtstart_tzid: None,
            dtend_tzid: None,
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
            dtstart_tzid: None,
            dtend_tzid: None,
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
            dtstart_tzid: None,
            dtend_tzid: None,
            dtstamp_ms: 0,
        };

        let raw = build_ics(&fields);
        assert!(raw.contains("SUMMARY:a\\\\b\\;c\\,d"), "{raw}");
        let parsed = parse_ics(&raw).expect("parse");
        assert_eq!(parsed.summary.as_deref(), Some("a\\b;c,d"));
    }

    // -- F18 Paso 10 — regression: no tzid is byte-identical ----------------

    #[test]
    fn build_ics_without_tzid_is_byte_identical() {
        let fields = EventFields {
            uid: "regression@oxidav".to_string(),
            summary: Some("Plain".to_string()),
            description: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            dtstamp_ms: 1_710_496_800_000,
        };

        let expected = concat!(
            "BEGIN:VCALENDAR\r\n",
            "VERSION:2.0\r\n",
            "PRODID:-//oxidav//oxidav//EN\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:regression@oxidav\r\n",
            "DTSTAMP:20240315T100000Z\r\n",
            "SUMMARY:Plain\r\n",
            "DTSTART:20240315T100000Z\r\n",
            "DTEND:20240315T110000Z\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        );

        assert_eq!(build_ics(&fields), expected);
    }

    // -- F18 Paso 11/12 — composition with a tzid ---------------------------

    #[test]
    fn build_ics_with_tzid_emits_local_time_and_vtimezone() {
        let fields = EventFields {
            uid: "zoned@oxidav".to_string(),
            summary: Some("Zoned".to_string()),
            description: None,
            // 09:00 UTC == 10:00 Europe/Madrid (CET).
            dtstart_ms: 1_710_493_200_000,
            // 09:00 UTC the same day == 10:00 Madrid; DTEND inherits the zone.
            dtend_ms: Some(1_710_496_800_000),
            dtstart_tzid: Some("Europe/Madrid".to_string()),
            dtend_tzid: None,
            rrule: None,
            dtstamp_ms: 1_710_493_200_000,
        };

        let raw = build_ics(&fields);

        // `DTSTART`/`DTEND` carry the canonical zone and a `Z`-free local time.
        assert!(
            raw.contains("DTSTART;TZID=Europe/Madrid:20240315T100000\r\n"),
            "{raw}"
        );
        assert!(
            raw.contains("DTEND;TZID=Europe/Madrid:20240315T110000\r\n"),
            "{raw}"
        );
        // The zone is defined and the definition precedes the event.
        let vtimezone = raw.find("BEGIN:VTIMEZONE").expect("a VTIMEZONE block");
        let vevent = raw.find("BEGIN:VEVENT").expect("a VEVENT");
        assert!(vtimezone < vevent, "{raw}");
        assert!(raw.contains("TZID:Europe/Madrid\r\n"), "{raw}");
        // No UTC suffix remains on the zoned date-times.
        assert!(
            !raw.contains("DTSTART;TZID=Europe/Madrid:20240315T100000Z"),
            "{raw}"
        );

        // Round-trip: same instant and same canonical zone.
        let parsed = parse_ics(&raw).expect("composed payload must parse");
        assert_eq!(parsed.dtstart_ms, fields.dtstart_ms);
        assert_eq!(parsed.dtend_ms, fields.dtend_ms);
        assert_eq!(parsed.dtstart_tzid.as_deref(), Some("Europe/Madrid"));
        assert_eq!(parsed.dtend_tzid.as_deref(), Some("Europe/Madrid"));
    }

    #[test]
    fn build_ics_dedups_vtimezone_for_two_distinct_zones() {
        let fields = EventFields {
            uid: "two-zones@oxidav".to_string(),
            summary: None,
            description: None,
            dtstart_ms: 1_710_493_200_000,     // 10:00 Madrid
            dtend_ms: Some(1_710_500_400_000), // 11:00 UTC == 20:00 Tokyo
            dtstart_tzid: Some("Europe/Madrid".to_string()),
            dtend_tzid: Some("Asia/Tokyo".to_string()),
            rrule: None,
            dtstamp_ms: 1_710_493_200_000,
        };

        let raw = build_ics(&fields);

        // One `VTIMEZONE` per distinct zone.
        assert_eq!(raw.matches("BEGIN:VTIMEZONE").count(), 2, "{raw}");
        assert!(raw.contains("TZID:Europe/Madrid\r\n"), "{raw}");
        assert!(raw.contains("TZID:Asia/Tokyo\r\n"), "{raw}");

        let parsed = parse_ics(&raw).expect("parse");
        assert_eq!(parsed.dtstart_tzid.as_deref(), Some("Europe/Madrid"));
        assert_eq!(parsed.dtend_tzid.as_deref(), Some("Asia/Tokyo"));
        assert_eq!(parsed.dtend_ms, Some(1_710_500_400_000));
    }
}
