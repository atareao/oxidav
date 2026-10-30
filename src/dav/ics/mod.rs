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

pub mod projection;

pub use projection::{EventProjection, project_event};

use icalendar::{Calendar, Component, EventLike, Property};

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
    /// `LOCATION`, already unescaped by the parser (free text; escaped again on
    /// emission). `None` when the property is absent or blank.
    pub location: Option<String>,
    /// `URL`, a URI emitted **verbatim** (never escaped). `None` when absent or
    /// blank.
    pub url: Option<String>,
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
    // `LOCATION` is free text: the `icalendar` parser already unescapes it
    // (its `ValueType::Text`), exactly like `SUMMARY`/`DESCRIPTION`. `URL` is a
    // URI (`ValueType::Uri`) and is returned verbatim, commas included.
    let location = normalize_ics_text(event.get_location());
    let url = normalize_ics_text(event.get_url());
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
        location,
        url,
        dtstart_ms,
        dtend_ms,
        dtstart_tzid,
        dtend_tzid,
        rrule,
    })
}

/// Normalise an optional iCalendar `LOCATION`/`URL` value.
///
/// A missing property, an empty value and a whitespace-only value all collapse
/// to `None`; any other value is returned **unchanged** (not trimmed), so a
/// URI's significant bytes survive verbatim.
fn normalize_ics_text(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
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
    /// `LOCATION` — free text, escaped per RFC 5545 §3.3.11 when emitted.
    pub location: Option<String>,
    /// `URL` — a URI, emitted **verbatim** (never escaped), like `RRULE`.
    pub url: Option<String>,
    /// `CATEGORIES` — a flat list emitted as a **single** content line, items
    /// joined with `,` and each escaped with [`escape_text`] (so a comma *inside*
    /// a category is written `\,`). Empty means the property is absent and its
    /// lines are removed on [`reconcile_ics`].
    pub categories: Vec<String>,
    /// `ATTACH` — one content line **per value**, each emitted **verbatim** (a
    /// URI, like `URL`; never escaped). Empty means the property is absent and
    /// its lines are removed on [`reconcile_ics`].
    pub attach: Vec<String>,
    /// `STATUS` — verbatim (RFC 5545 §3.8.1.11: `TENTATIVE`, `CONFIRMED`,
    /// `CANCELLED`). `None` means the line is removed on [`reconcile_ics`].
    pub status: Option<String>,
    /// `CLASS` — verbatim (RFC 5545 §3.8.1.3: `PUBLIC`, `PRIVATE`,
    /// `CONFIDENTIAL` or an `x-name`/`iana-token`). `None` means removed.
    pub class_: Option<String>,
    /// `TRANSP` — verbatim (RFC 5545 §3.8.2.7: `OPAQUE`, `TRANSPARENT`). `None`
    /// means removed.
    pub transp: Option<String>,
    /// `DURATION` — opaque text, emitted **verbatim** (RFC 5545 §3.3.6 grammar),
    /// like `RRULE`. Mutually exclusive with `dtend_ms` per RFC 5545; the
    /// exclusion is enforced by the REST layer, not here. `None` means removed.
    pub duration: Option<String>,
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
    /// `EXDATE` — instantes (ms) excluidos de la recurrencia, ordenados y sin
    /// duplicados. `Some(vec![])` **limpia** la propiedad; `None` significa «no
    /// tocar las líneas `EXDATE` del payload» (son **opacas**, fuera del
    /// subconjunto soportado).
    pub exdate: Option<Vec<i64>>,
    /// `RDATE` — instantes (ms) añadidos a la recurrencia. Misma semántica de tres
    /// estados que `exdate`.
    pub rdate: Option<Vec<i64>>,
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
/// Only genuinely free-text values (`SUMMARY`, `DESCRIPTION`, `LOCATION` and each
/// `CATEGORIES` item) are escaped per RFC 5545 §3.3.11, since those are the
/// fields the parser unescapes. Structured values (`UID`, `DTSTART`, `DTEND`,
/// `RRULE`, `STATUS`, `CLASS`, `TRANSP`, `DURATION`) and the URIs (`URL`,
/// `ATTACH`) are emitted verbatim: `RRULE` in particular uses `;` and `=` as
/// **separators**, so escaping it would corrupt the rule
/// (`FREQ=WEEKLY;COUNT=3` must stay such, not `FREQ=WEEKLY\;COUNT=3`), and an
/// `ATTACH` URI's commas and `&`/`=` are equally significant.
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

    let mut body: Vec<String> = Vec::with_capacity(15);
    body.push("BEGIN:VEVENT".to_string());
    body.push(uid_line(&fields.uid));
    body.push(dtstamp_line(fields.dtstamp_ms));
    if let Some(summary) = &fields.summary {
        body.push(summary_line(summary));
    }
    if let Some(description) = &fields.description {
        body.push(description_line(description));
    }
    if let Some(location) = &fields.location {
        body.push(location_line(location));
    }
    if let Some(url) = &fields.url {
        body.push(url_line(url));
    }
    if !fields.categories.is_empty() {
        body.push(categories_line(&fields.categories));
    }
    for uri in &fields.attach {
        body.push(attach_line(uri));
    }
    body.push(dtstart_line(fields.dtstart_ms, dtstart_tz));
    if let Some(dtend_ms) = fields.dtend_ms {
        body.push(dtend_line(dtend_ms, dtend_tz));
    }
    if let Some(duration) = &fields.duration {
        body.push(duration_line(duration));
    }
    if let Some(rrule) = &fields.rrule {
        body.push(rrule_line(rrule));
    }
    if let Some(rdate) = &fields.rdate
        && !rdate.is_empty()
    {
        body.push(recur_date_line("RDATE", rdate));
    }
    if let Some(exdate) = &fields.exdate
        && !exdate.is_empty()
    {
        body.push(recur_date_line("EXDATE", exdate));
    }
    if let Some(status) = &fields.status {
        body.push(status_line(status));
    }
    if let Some(class) = &fields.class_ {
        body.push(class_line(class));
    }
    if let Some(transp) = &fields.transp {
        body.push(transp_line(transp));
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

// ---------------------------------------------------------------------------
// F24 — surgical reconciliation of a stored `.ics` (PLAN-028)
// ---------------------------------------------------------------------------

/// Property names [`reconcile_ics`] knows, in canonical emission order.
///
/// `LOCATION` and `URL` are modelled fields as of F24; `CATEGORIES`, `ATTACH`,
/// `STATUS`, `CLASS`, `TRANSP` and `DURATION` join them in F25, and `RDATE`/
/// `EXDATE` in F27. A pre-existing line of any of them is reconciled in place (or
/// removed when the final value is empty), and they anchor the canonical insertion
/// position of a neighbouring modelled property.
const RECONCILE_ORDER: [&str; 16] = [
    "DTSTAMP",
    "SUMMARY",
    "DESCRIPTION",
    "LOCATION",
    "URL",
    "CATEGORIES",
    "ATTACH",
    "DTSTART",
    "DTEND",
    "DURATION",
    "RRULE",
    "RDATE",
    "EXDATE",
    "STATUS",
    "CLASS",
    "TRANSP",
];

/// One **logical** iCalendar content line: its unfolded text plus the physical
/// lines it was folded into, kept verbatim so a line that is not rewritten can be
/// re-emitted byte for byte.
struct LogicalLine {
    /// The line unfolded: continuation prefixes and terminators removed.
    text: String,
    /// The original physical lines (terminators included), in order.
    physical: Vec<String>,
}

/// Strip a single trailing `\r\n` or `\n` from a physical line.
fn strip_line_terminator(line: &str) -> &str {
    match line.strip_suffix("\r\n") {
        Some(content) => content,
        None => line.strip_suffix('\n').unwrap_or(line),
    }
}

/// Split `raw` into **logical** content lines (RFC 5545 §3.1 unfolding).
///
/// A physical line whose content starts with a space or a tab continues the
/// previous logical line; the leading whitespace is dropped and the remainder
/// appended. The physical lines of every logical line are retained verbatim.
fn split_logical_lines(raw: &str) -> Vec<LogicalLine> {
    let mut lines: Vec<LogicalLine> = Vec::new();
    for physical in raw.split_inclusive('\n') {
        let content = strip_line_terminator(physical);
        let is_continuation = content.starts_with(' ') || content.starts_with('\t');
        if is_continuation && let Some(last) = lines.last_mut() {
            last.text.push_str(&content[1..]);
            last.physical.push(physical.to_string());
            continue;
        }
        lines.push(LogicalLine {
            text: content.to_string(),
            physical: vec![physical.to_string()],
        });
    }
    lines
}

/// Extract a `BEGIN:`/`END:` component name from a logical line, upper-cased.
///
/// Returns `None` for a property line. The keyword comparison is ASCII
/// case-insensitive, as RFC 5545 allows; the component name is normalised to
/// upper case so stack comparisons are exact.
fn component_boundary(text: &str, keyword: &str) -> Option<String> {
    let (head, tail) = text.split_once(':')?;
    if head.eq_ignore_ascii_case(keyword) {
        Some(tail.to_uppercase())
    } else {
        None
    }
}

/// The property name of a logical line: everything before the first `:` or `;`.
fn property_name(text: &str) -> &str {
    text.split_once([':', ';']).map_or(text, |(name, _)| name)
}

/// The property value of a logical line: everything after the first `:`.
fn property_value(text: &str) -> &str {
    text.split_once(':').map_or("", |(_, value)| value)
}

/// Extrae un parámetro del *head* de una línea iCalendar (`NAME=value` tras el
/// primer `;`), insensible a mayúsculas y con las comillas del `value`
/// despojadas. Devuelve la ÚLTIMA ocurrencia si el parámetro se repite.
///
/// El *head* es lo que va antes del primer `:`; el primer trozo es el nombre de
/// la propiedad y se ignora (sólo se examinan los trozos `;`-separados
/// siguientes). Una línea sin `:` no tiene *head* con parámetros y devuelve
/// `None`.
pub(crate) fn property_param<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let (head, _) = text.split_once(':')?;
    let mut found = None;
    for segment in head.split(';').skip(1) {
        if let Some((key, value)) = segment.split_once('=')
            && key.eq_ignore_ascii_case(name)
        {
            found = Some(value.trim_matches('"'));
        }
    }
    found
}

/// Whether a logical line is a property (i.e. neither `BEGIN:` nor `END:`).
fn is_property_line(text: &str) -> bool {
    component_boundary(text, "BEGIN").is_none() && component_boundary(text, "END").is_none()
}

/// Whether a component stack is exactly `[VCALENDAR]`.
fn is_vcalendar_level(stack: &[String]) -> bool {
    stack.len() == 1 && stack[0] == "VCALENDAR"
}

/// Whether a component stack is exactly `[VCALENDAR, VEVENT]`.
fn is_vevent_top_level(stack: &[String]) -> bool {
    stack.len() == 2 && stack[0] == "VCALENDAR" && stack[1] == "VEVENT"
}

/// The canonical rank of a property name, or `None` when it is not modelled.
fn canonical_rank(name: &str) -> Option<usize> {
    RECONCILE_ORDER
        .iter()
        .position(|&known| known.eq_ignore_ascii_case(name))
}

/// The component stack applicable to each logical line.
///
/// A `BEGIN:X` line carries the stack **before** the push, so the target
/// `BEGIN:VEVENT` inside a `VCALENDAR` reports `[VCALENDAR]`; an `END:X` line
/// carries the stack **before** the pop, so its `END:VEVENT` reports
/// `[VCALENDAR, VEVENT]`. Property lines carry the current stack unchanged.
fn component_stacks(lines: &[LogicalLine]) -> Vec<Vec<String>> {
    let mut stacks = Vec::with_capacity(lines.len());
    let mut stack: Vec<String> = Vec::new();
    for line in lines {
        if let Some(name) = component_boundary(&line.text, "BEGIN") {
            stacks.push(stack.clone());
            stack.push(name);
        } else if let Some(name) = component_boundary(&line.text, "END") {
            stacks.push(stack.clone());
            if stack.last().is_some_and(|open| open == &name) {
                stack.pop();
            }
        } else {
            stacks.push(stack.clone());
        }
    }
    stacks
}

/// Whether the calendar already defines a `VTIMEZONE` for `tz`.
fn has_tzid_definition(lines: &[LogicalLine], tz: Tz) -> bool {
    let canonical = vtimezone::canonical_tzid(tz);
    lines.iter().any(|line| {
        property_name(&line.text).eq_ignore_ascii_case("TZID")
            && property_value(&line.text).trim() == canonical
    })
}

/// The logical-line index before which a missing property `name` must be inserted.
///
/// The canonical position is before the first same-level property whose rank is
/// later, else before the first nested component, else before `END:VEVENT`
/// (`target_end`). Anchoring to a component boundary keeps a new property inside
/// the event's property list, never interleaved into a `VALARM`.
fn insertion_index(
    name: &str,
    target: usize,
    target_end: usize,
    lines: &[LogicalLine],
    stacks: &[Vec<String>],
) -> usize {
    let rank = canonical_rank(name).unwrap_or(usize::MAX);
    let region: Vec<(&LogicalLine, &Vec<String>)> = lines[target + 1..target_end]
        .iter()
        .zip(&stacks[target + 1..target_end])
        .collect();

    if let Some(offset) = region.iter().position(|(line, stack)| {
        is_vevent_top_level(stack)
            && is_property_line(&line.text)
            && canonical_rank(property_name(&line.text)).is_some_and(|other| other > rank)
    }) {
        return target + 1 + offset;
    }
    if let Some(offset) = region.iter().position(|(line, stack)| {
        is_vevent_top_level(stack) && component_boundary(&line.text, "BEGIN").is_some()
    }) {
        return target + 1 + offset;
    }
    target_end
}

/// Render the `UID` content line (a structured value, emitted verbatim).
fn uid_line(uid: &str) -> String {
    format!("UID:{uid}")
}

/// Render the `DTSTAMP` content line as a UTC date-time.
fn dtstamp_line(dtstamp_ms: i64) -> String {
    format!("DTSTAMP:{}", format_datetime_ms(dtstamp_ms))
}

/// Render the `SUMMARY` content line, escaping the free-text value.
fn summary_line(summary: &str) -> String {
    format!("SUMMARY:{}", escape_text(summary))
}

/// Render the `DESCRIPTION` content line, escaping the free-text value.
fn description_line(description: &str) -> String {
    format!("DESCRIPTION:{}", escape_text(description))
}

/// Render the `LOCATION` content line, escaping the free-text value.
fn location_line(location: &str) -> String {
    format!("LOCATION:{}", escape_text(location))
}

/// Render the `URL` content line (a URI, emitted verbatim).
fn url_line(url: &str) -> String {
    format!("URL:{url}")
}

/// Render the `CATEGORIES` content line as a **single** line: the items joined
/// with `,`, each escaped with [`escape_text`] so a comma *inside* a category is
/// written `\,`. The caller guarantees `categories` is non-empty.
fn categories_line(categories: &[String]) -> String {
    let joined = categories
        .iter()
        .map(|category| escape_text(category))
        .collect::<Vec<_>>()
        .join(",");
    format!("CATEGORIES:{joined}")
}

/// Render one `ATTACH` content line (a URI, emitted verbatim, like `URL`).
fn attach_line(attach: &str) -> String {
    format!("ATTACH:{attach}")
}

/// Render the `STATUS` content line (a structured value, emitted verbatim).
fn status_line(status: &str) -> String {
    format!("STATUS:{status}")
}

/// Render the `CLASS` content line (a structured value, emitted verbatim).
fn class_line(class: &str) -> String {
    format!("CLASS:{class}")
}

/// Render the `TRANSP` content line (a structured value, emitted verbatim).
fn transp_line(transp: &str) -> String {
    format!("TRANSP:{transp}")
}

/// Render the `DURATION` content line (an opaque value, emitted verbatim, like
/// `RRULE`).
fn duration_line(duration: &str) -> String {
    format!("DURATION:{duration}")
}

/// Render the `DTSTART` content line: `;TZID=<canonical>` with a local time when
/// `tz` is set, otherwise the UTC rendering.
fn dtstart_line(dtstart_ms: i64, tz: Option<Tz>) -> String {
    match tz {
        Some(tz) => format!(
            "DTSTART;TZID={}:{}",
            vtimezone::canonical_tzid(tz),
            vtimezone::format_local_datetime_ms(tz, dtstart_ms)
        ),
        None => format!("DTSTART:{}", format_datetime_ms(dtstart_ms)),
    }
}

/// Render the `DTEND` content line, mirroring [`dtstart_line`].
fn dtend_line(dtend_ms: i64, tz: Option<Tz>) -> String {
    match tz {
        Some(tz) => format!(
            "DTEND;TZID={}:{}",
            vtimezone::canonical_tzid(tz),
            vtimezone::format_local_datetime_ms(tz, dtend_ms)
        ),
        None => format!("DTEND:{}", format_datetime_ms(dtend_ms)),
    }
}

/// Render the `RRULE` content line (a structured value, emitted verbatim).
fn rrule_line(rrule: &str) -> String {
    format!("RRULE:{rrule}")
}

/// Renderiza una línea `EXDATE`/`RDATE` como **una sola** content line,
/// **siempre en UTC**: `NAME:<v1>Z,<v2>Z,…`.
///
/// Los instantes se ordenan ascendente y se **deduplican** antes de emitir, y
/// cada uno se formatea con [`format_datetime_ms`] (sufijo `Z`). **Nunca** se
/// emite `;TZID=` ni una hora local en la zona de `DTSTART`, y esto es
/// deliberado: formatear en la zona (`format_local_datetime_ms`) y re-parsear
/// ([`parse_datetime_with_tzid`]) sólo es *lossless* fuera de la hora **ambigua**
/// del cambio de hora de otoño. Un instante que cae en la **segunda** pasada de
/// la hora repetida se escribiría como hora local y al re-parsearlo
/// [`local_to_utc_ms`] lo resolvería a la **primera** interpretación, moviéndolo
/// −1 h en silencio. La forma UTC hace el *round-trip* **exacto** y es RFC 5545
/// válido: `EXDATE`/`RDATE` admiten valores UTC aunque `DTSTART` lleve `TZID`.
fn recur_date_line(name: &str, instants: &[i64]) -> String {
    let mut sorted = instants.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let joined = sorted
        .iter()
        .map(|&ms| format_datetime_ms(ms))
        .collect::<Vec<_>>()
        .join(",");
    format!("{name}:{joined}")
}

/// Reconcilia las propiedades *modeladas* del primer `VEVENT` de `raw` con
/// `fields`, dejando intacta —byte a byte— cualquier otra línea del payload.
///
/// Sólo se tocan las propiedades del **nivel superior** del primer `VEVENT` que
/// cuelga de un `VCALENDAR` (el mismo que elige [`parse_ics`]); un `SUMMARY`
/// dentro de un `VALARM`, un segundo `VEVENT` o cualquier propiedad no modelada
/// (`ATTENDEE`, `X-*`…) se reemiten **verbatim**, con su plegado original. El
/// conjunto modelado —y por tanto reconciliado— es `SUMMARY`, `DESCRIPTION`,
/// `LOCATION`, `URL`, `CATEGORIES`, `ATTACH`, `DTSTART`, `DTEND`, `DURATION`,
/// `RRULE`, `RDATE`, `EXDATE`, `STATUS`, `CLASS`, `TRANSP` y `DTSTAMP`. `ATTACH`
/// es **multi-valor**:
/// la primera ocurrencia se sustituye por el bloque completo de valores (una
/// línea por cada uno, en orden) y todas las demás se eliminan; `CATEGORIES`,
/// `RDATE` y `EXDATE` son multi-valor en el payload pero se reconcilian como
/// **una** línea (las ocurrencias extra se colapsan). `RDATE`/`EXDATE` sólo se
/// reconcilian cuando `fields.rdate`/`fields.exdate` es `Some`: con `None` la
/// propiedad se considera **opaca**, no se toca y sus líneas quedan intactas.
/// Las únicas líneas regeneradas son las modeladas ausentes o modificadas.
///
/// Si la zona final de `DTSTART`/`DTEND` no está definida en el `VCALENDAR`, se
/// inserta su bloque `VTIMEZONE` justo antes del `BEGIN:VEVENT` objetivo. Un
/// `VTIMEZONE` preexistente **nunca** se elimina.
///
/// # Errores
/// [`IcsError::NoEvent`] si `raw` no contiene ningún `VEVENT` de nivel
/// `VCALENDAR`.
pub fn reconcile_ics(raw: &str, fields: &EventFields) -> Result<String, IcsError> {
    use std::collections::{BTreeMap, BTreeSet};

    let lines = split_logical_lines(raw);
    let stacks = component_stacks(&lines);

    // The first `VEVENT` directly inside a `VCALENDAR`: the one `parse_ics` picks.
    let target = lines
        .iter()
        .enumerate()
        .find(|(index, line)| {
            component_boundary(&line.text, "BEGIN").as_deref() == Some("VEVENT")
                && is_vcalendar_level(&stacks[*index])
        })
        .map(|(index, _)| index)
        .ok_or(IcsError::NoEvent)?;

    // The `END:VEVENT` closing the target, bounding the reconciled region so a
    // following `VEVENT` is never touched.
    let target_end = (target + 1..lines.len())
        .find(|&index| {
            component_boundary(&lines[index].text, "END").as_deref() == Some("VEVENT")
                && is_vevent_top_level(&stacks[index])
        })
        .unwrap_or(lines.len());

    // Top-level property lines of the target event.
    let top_level_props: Vec<usize> = lines[target + 1..target_end]
        .iter()
        .zip(&stacks[target + 1..target_end])
        .enumerate()
        .filter(|(_, (line, stack))| is_vevent_top_level(stack) && is_property_line(&line.text))
        .map(|(offset, _)| target + 1 + offset)
        .collect();

    // Final rendered content of every modelled property: a **block** of one or
    // more lines, or an empty block when the property must be removed. A
    // single-valued property yields 0 or 1 line; `ATTACH` yields one line per
    // value; `CATEGORIES` yields a single joined line.
    let dtstart_tz = fields
        .dtstart_tzid
        .as_deref()
        .and_then(vtimezone::parse_tzid);
    let dtend_tz = fields
        .dtend_tzid
        .as_deref()
        .and_then(vtimezone::parse_tzid)
        .or(dtstart_tz);
    let mut final_values: Vec<(&'static str, Vec<String>)> = vec![
        ("DTSTAMP", vec![dtstamp_line(fields.dtstamp_ms)]),
        (
            "SUMMARY",
            fields
                .summary
                .as_deref()
                .map(summary_line)
                .into_iter()
                .collect(),
        ),
        (
            "DESCRIPTION",
            fields
                .description
                .as_deref()
                .map(description_line)
                .into_iter()
                .collect(),
        ),
        (
            "LOCATION",
            fields
                .location
                .as_deref()
                .map(location_line)
                .into_iter()
                .collect(),
        ),
        (
            "URL",
            fields.url.as_deref().map(url_line).into_iter().collect(),
        ),
        (
            "CATEGORIES",
            if fields.categories.is_empty() {
                Vec::new()
            } else {
                vec![categories_line(&fields.categories)]
            },
        ),
        (
            "ATTACH",
            fields
                .attach
                .iter()
                .map(|attach| attach_line(attach))
                .collect(),
        ),
        ("DTSTART", vec![dtstart_line(fields.dtstart_ms, dtstart_tz)]),
        (
            "DTEND",
            fields
                .dtend_ms
                .map(|ms| dtend_line(ms, dtend_tz))
                .into_iter()
                .collect(),
        ),
        (
            "DURATION",
            fields
                .duration
                .as_deref()
                .map(duration_line)
                .into_iter()
                .collect(),
        ),
        (
            "RRULE",
            fields
                .rrule
                .as_deref()
                .map(rrule_line)
                .into_iter()
                .collect(),
        ),
    ];

    // `RDATE`/`EXDATE` sólo entran cuando el campo es `Some`: con `None` la
    // propiedad sigue siendo **opaca** y ni se incluye, así que sus líneas del
    // payload quedan intactas. `Some(_)` no vacío se emite como una sola línea
    // (que colapsa las ocurrencias extra) y `Some(vec![])` vacía la propiedad.
    // La emisión es **siempre en UTC** (`…Z`, vía [`recur_date_line`]), nunca en
    // la zona de `DTSTART`: ver el *doc* de esa función para el porqué (hora
    // ambigua del fall-back).
    if let Some(rdate) = &fields.rdate {
        final_values.push((
            "RDATE",
            if rdate.is_empty() {
                Vec::new()
            } else {
                vec![recur_date_line("RDATE", rdate)]
            },
        ));
    }
    if let Some(exdate) = &fields.exdate {
        final_values.push((
            "EXDATE",
            if exdate.is_empty() {
                Vec::new()
            } else {
                vec![recur_date_line("EXDATE", exdate)]
            },
        ));
    }

    final_values.extend([
        (
            "STATUS",
            fields
                .status
                .as_deref()
                .map(status_line)
                .into_iter()
                .collect(),
        ),
        (
            "CLASS",
            fields
                .class_
                .as_deref()
                .map(class_line)
                .into_iter()
                .collect(),
        ),
        (
            "TRANSP",
            fields
                .transp
                .as_deref()
                .map(transp_line)
                .into_iter()
                .collect(),
        ),
    ]);

    let mut replace: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut remove: BTreeSet<usize> = BTreeSet::new();
    let mut insertions: BTreeMap<usize, Vec<&'static str>> = BTreeMap::new();

    for (name, values) in &final_values {
        let occurrences: Vec<usize> = top_level_props
            .iter()
            .copied()
            .filter(|&index| property_name(&lines[index].text).eq_ignore_ascii_case(name))
            .collect();
        match values.split_first() {
            Some(_) => match occurrences.split_first() {
                // Replace the first occurrence with the whole block; duplicates
                // are invalid per RFC 5545 and the new value wins, so drop the
                // rest. For `ATTACH` this is the multi-value rule: the first
                // occurrence becomes the full list (one line per value) and
                // every other occurrence is removed.
                Some((&first, rest)) => {
                    replace.insert(first, values.clone());
                    remove.extend(rest);
                }
                // No occurrence: insert at the canonical position.
                None => {
                    let at = insertion_index(name, target, target_end, &lines, &stacks);
                    insertions.entry(at).or_default().push(name);
                }
            },
            // Cleared field: drop every occurrence.
            None => remove.extend(&occurrences),
        }
    }

    // Zones the event needs, in first-seen order (dtstart before dtend).
    let mut zones: Vec<Tz> = Vec::new();
    for tz in [dtstart_tz, dtend_tz].into_iter().flatten() {
        if !zones.contains(&tz) {
            zones.push(tz);
        }
    }
    let missing_zones: Vec<Tz> = zones
        .iter()
        .copied()
        .filter(|&tz| !has_tzid_definition(&lines, tz))
        .collect();
    let rendered_zones: Vec<String> = if missing_zones.is_empty() {
        Vec::new()
    } else {
        let reference = reference_year(fields, &zones);
        missing_zones
            .iter()
            .map(|&tz| vtimezone::render_vtimezone(tz, reference))
            .collect()
    };

    // Insertions anchored before `at`, emitted in canonical order.
    let emit_insertions = |at: usize, out: &mut String| {
        let Some(names) = insertions.get(&at) else {
            return;
        };
        let mut names = names.clone();
        names.sort_by_key(|name| canonical_rank(name).unwrap_or(usize::MAX));
        for name in names {
            if let Some(lines) = final_values
                .iter()
                .find_map(|(known, values)| if *known == name { Some(values) } else { None })
            {
                for line in lines {
                    super::common::fold_content_line(line, out);
                }
            }
        }
    };

    let mut out = String::with_capacity(raw.len() + 256);
    for (index, line) in lines.iter().enumerate() {
        // Splice the missing `VTIMEZONE` blocks immediately before the target.
        if index == target {
            for block in &rendered_zones {
                out.push_str(block);
            }
        }
        emit_insertions(index, &mut out);
        if remove.contains(&index) {
            continue;
        }
        match replace.get(&index) {
            Some(new_lines) => {
                for new_line in new_lines {
                    super::common::fold_content_line(new_line, &mut out);
                }
            }
            None => {
                for physical in &line.physical {
                    out.push_str(physical);
                }
            }
        }
    }
    emit_insertions(lines.len(), &mut out);

    Ok(out)
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
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            rrule: Some("FREQ=WEEKLY;COUNT=3".to_string()),
            dtstart_tzid: None,
            dtend_tzid: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
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
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            rrule: Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=3".to_string()),
            dtstart_tzid: None,
            dtend_tzid: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
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
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            rrule: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
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
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            rrule: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
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
            location: None,
            url: None,
            dtstart_ms: 0,
            dtend_ms: None,
            rrule: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 0,
            exdate: None,
            rdate: None,
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
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
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
            location: None,
            url: None,
            // 09:00 UTC == 10:00 Europe/Madrid (CET).
            dtstart_ms: 1_710_493_200_000,
            // 09:00 UTC the same day == 10:00 Madrid; DTEND inherits the zone.
            dtend_ms: Some(1_710_496_800_000),
            dtstart_tzid: Some("Europe/Madrid".to_string()),
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_493_200_000,
            exdate: None,
            rdate: None,
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
            location: None,
            url: None,
            dtstart_ms: 1_710_493_200_000,     // 10:00 Madrid
            dtend_ms: Some(1_710_500_400_000), // 11:00 UTC == 20:00 Tokyo
            dtstart_tzid: Some("Europe/Madrid".to_string()),
            dtend_tzid: Some("Asia/Tokyo".to_string()),
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_493_200_000,
            exdate: None,
            rdate: None,
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

    // -- F24 Paso 1 RED — `reconcile_ics` must not destroy unmodelled data ----

    /// A rich `.ics` payload — the trap fixture of PLAN-028. It carries a
    /// `VALARM` with **its own** `SUMMARY` (`SUMMARY:Recordatorio`, *not* the
    /// event's), two `ATTENDEE` lines, `CATEGORIES`, `LOCATION` and an `X-`
    /// property. As of F24 `LOCATION` **is** modelled and as of F25 `CATEGORIES`
    /// **is too** (both are reconciled like the other modelled fields);
    /// `ATTENDEE`, `X-*` and `VALARM` are not, so they must survive only by
    /// passing through verbatim.
    const RICH_ICS: &str = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "PRODID:-//Thunderbird//EN\r\n",
        "BEGIN:VEVENT\r\n",
        "UID:rich@oxidav\r\n",
        "DTSTAMP:20240315T090000Z\r\n",
        "SUMMARY:Original\r\n",
        "DESCRIPTION:Una descripcion\r\n",
        "LOCATION:Sala 3\r\n",
        "CATEGORIES:Reunion,Trabajo\r\n",
        "X-CUSTOM-PROP:valor-propio\r\n",
        "ATTENDEE;PARTSTAT=ACCEPTED;ROLE=REQ-PARTICIPANT;RSVP=TRUE:mailto:ana@example.org\r\n",
        "ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:luis@example.org\r\n",
        "DTSTART:20240315T100000Z\r\n",
        "DTEND:20240315T110000Z\r\n",
        "BEGIN:VALARM\r\n",
        "ACTION:DISPLAY\r\n",
        "TRIGGER:-PT15M\r\n",
        "SUMMARY:Recordatorio\r\n",
        "END:VALARM\r\n",
        "END:VEVENT\r\n",
        "END:VCALENDAR\r\n",
    );

    /// The `EventFields` used across the `reconcile_ics` tests: every modelled
    /// value equals the fixture's, except `summary`, which is replaced.
    ///
    /// `LOCATION` is modelled as of F24 and `CATEGORIES` as of F25, and the
    /// fixture carries `LOCATION:Sala 3` and `CATEGORIES:Reunion,Trabajo`, so both
    /// **are** set here: the E1/E2 tests then assert that those lines survive
    /// because the (unchanged) modelled values render them back identically. The
    /// properties that are genuinely unmodelled — and therefore only pass through
    /// verbatim — are `ATTENDEE`, `X-*` and `VALARM`.
    fn reconcile_fields(summary: Option<&str>) -> EventFields {
        EventFields {
            uid: "rich@oxidav".to_string(),
            summary: summary.map(str::to_string),
            description: Some("Una descripcion".to_string()),
            location: Some("Sala 3".to_string()),
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            // F25: la tarea de API los cablea.
            categories: vec!["Reunion".to_string(), "Trabajo".to_string()],
            attach: Vec::new(),
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        }
    }

    #[test]
    fn reconcile_keeps_unmodelled_properties_verbatim() {
        // E1: changing only `summary` must leave every *unmodelled* property and
        // subcomponent byte for byte intact.
        let fields = reconcile_fields(Some("Nuevo titulo"));

        let out = reconcile_ics(RICH_ICS, &fields).expect("reconcile");

        assert!(out.contains("SUMMARY:Nuevo titulo"), "{out}");
        // `LOCATION` is a modelled field as of F24 and `CATEGORIES` as of F25:
        // both survive because the reconciled values render the very same lines
        // back, not by pass-through.
        assert!(out.contains("LOCATION:Sala 3"), "{out}");
        assert!(out.contains("CATEGORIES:Reunion,Trabajo"), "{out}");
        // These are genuinely unmodelled, so they survive only by pass-through:
        // `X-*`, both `ATTENDEE` lines and the `VALARM` below.
        assert!(out.contains("X-CUSTOM-PROP:valor-propio"), "{out}");
        // Both `ATTENDEE` lines, with their parameters, must survive verbatim.
        assert!(
            out.contains(
                "ATTENDEE;PARTSTAT=ACCEPTED;ROLE=REQ-PARTICIPANT;RSVP=TRUE:\
                 mailto:ana@example.org"
            ),
            "{out}"
        );
        assert!(
            out.contains("ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:luis@example.org"),
            "{out}"
        );
        // The nested `VALARM` subcomponent, byte for byte.
        assert!(
            out.contains(concat!(
                "BEGIN:VALARM\r\n",
                "ACTION:DISPLAY\r\n",
                "TRIGGER:-PT15M\r\n",
                "SUMMARY:Recordatorio\r\n",
                "END:VALARM\r\n",
            )),
            "{out}"
        );
        // Exactly two `ATTENDEE` lines: nothing added, nothing lost.
        let attendee_lines = out.lines().filter(|l| l.starts_with("ATTENDEE")).count();
        assert_eq!(attendee_lines, 2, "{out}");

        // And the modelled field round-trips.
        let parsed = parse_ics(&out).expect("reconciled payload must parse");
        assert_eq!(parsed.summary.as_deref(), Some("Nuevo titulo"));
    }

    #[test]
    fn reconcile_does_not_touch_the_alarm_summary() {
        // Depth trap: the `VALARM` carries its own `SUMMARY:Recordatorio`, which
        // must NOT be mistaken for the event's and must keep living inside its
        // `BEGIN:VALARM`…`END:VALARM` block.
        let fields = reconcile_fields(Some("Nuevo titulo"));

        let out = reconcile_ics(RICH_ICS, &fields).expect("reconcile");

        assert!(out.contains("SUMMARY:Nuevo titulo"), "{out}");
        assert!(out.contains("SUMMARY:Recordatorio"), "{out}");

        let alarm_start = out.find("BEGIN:VALARM").expect("a VALARM block");
        let alarm_end = out
            .find("END:VALARM")
            .expect("the VALARM end")
            .checked_add("END:VALARM".len())
            .expect("non-overflowing");
        let alarm_block = &out[alarm_start..alarm_end];
        assert!(
            alarm_block.contains("SUMMARY:Recordatorio"),
            "the alarm summary must stay inside its own block:\n{out}"
        );
    }

    #[test]
    fn reconcile_leaves_a_second_vevent_byte_identical() {
        // E7: only the first `VEVENT` is reconciled; any following one must be
        // emitted byte for byte.
        let first = concat!(
            "BEGIN:VEVENT\r\n",
            "UID:primero@oxidav\r\n",
            "DTSTAMP:20240315T090000Z\r\n",
            "SUMMARY:Original\r\n",
            "DTSTART:20240315T100000Z\r\n",
            "DTEND:20240315T110000Z\r\n",
            "END:VEVENT\r\n",
        );
        let second = concat!(
            "BEGIN:VEVENT\r\n",
            "UID:segundo@oxidav\r\n",
            "DTSTAMP:20240315T090000Z\r\n",
            "SUMMARY:Segundo\r\n",
            "DTSTART:20240316T100000Z\r\n",
            "DTEND:20240316T110000Z\r\n",
            "X-SEGUNDO:no-tocar\r\n",
            "END:VEVENT\r\n",
        );
        let raw = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Thunderbird//EN\r\n\
             {first}{second}END:VCALENDAR\r\n"
        );

        let fields = EventFields {
            uid: "primero@oxidav".to_string(),
            summary: Some("Cambiado".to_string()),
            description: None,
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(out.contains("SUMMARY:Cambiado"), "{out}");
        assert!(
            out.contains(second),
            "the second VEVENT must be byte-identical:\n{out}"
        );
        assert!(out.contains("X-SEGUNDO:no-tocar"), "{out}");
        assert!(out.contains("SUMMARY:Segundo"), "{out}");
    }

    #[test]
    fn reconcile_errors_when_there_is_no_vevent() {
        let raw = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
        let fields = reconcile_fields(Some("Nuevo titulo"));

        assert!(matches!(
            reconcile_ics(raw, &fields),
            Err(IcsError::NoEvent)
        ));
    }

    // -- F24 — `reconcile_ics`: zones, insertion, removal, dedup, folding -----

    #[test]
    fn reconcile_adds_a_missing_timezone_and_keeps_the_old_one() {
        // E6: switching zone adds the new `VTIMEZONE` but never removes the old.
        let madrid = vtimezone::render_vtimezone(Tz::Europe__Madrid, 2024);
        let raw = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Thunderbird//EN\r\n\
             {madrid}\
             BEGIN:VEVENT\r\n\
             UID:zonas@oxidav\r\n\
             DTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Con zona\r\n\
             DTSTART;TZID=Europe/Madrid:20240315T100000\r\n\
             DTEND;TZID=Europe/Madrid:20240315T110000\r\n\
             BEGIN:VALARM\r\n\
             ACTION:DISPLAY\r\n\
             TRIGGER:-PT15M\r\n\
             SUMMARY:Recordatorio\r\n\
             END:VALARM\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        );

        let fields = EventFields {
            uid: "zonas@oxidav".to_string(),
            summary: Some("Con zona".to_string()),
            description: None,
            location: None,
            url: None,
            // 09:00 UTC == 10:00 Europe/Madrid before the change.
            dtstart_ms: 1_710_493_200_000,
            dtend_ms: Some(1_710_496_800_000),
            dtstart_tzid: Some("Asia/Tokyo".to_string()),
            dtend_tzid: Some("Asia/Tokyo".to_string()),
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(out.contains("DTSTART;TZID=Asia/Tokyo:"), "{out}");
        assert!(out.contains("BEGIN:VTIMEZONE"), "{out}");
        assert!(out.contains("TZID:Asia/Tokyo\r\n"), "{out}");
        // The pre-existing Madrid block survives verbatim.
        assert!(
            out.contains(&madrid),
            "the Madrid VTIMEZONE must survive:\n{out}"
        );
        // The alarm survives.
        assert!(out.contains("BEGIN:VALARM"), "{out}");
        assert!(out.contains("SUMMARY:Recordatorio"), "{out}");
    }

    #[test]
    fn reconcile_inserts_a_missing_modelled_property() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:ins@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Con titulo\r\nDTSTART:20240315T100000Z\r\n\
             DTEND:20240315T110000Z\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "ins@oxidav".to_string(),
            summary: Some("Con titulo".to_string()),
            description: Some("nueva".to_string()),
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert_eq!(out.matches("DESCRIPTION:nueva").count(), 1, "{out}");
        let summary = out.find("SUMMARY:Con titulo").expect("a SUMMARY line");
        let description = out.find("DESCRIPTION:nueva").expect("a DESCRIPTION line");
        let dtstart = out.find("DTSTART:").expect("a DTSTART line");
        assert!(
            summary < description && description < dtstart,
            "DESCRIPTION must sit between SUMMARY and DTSTART:\n{out}"
        );
        // The rest of the payload is unchanged.
        assert!(out.contains("UID:ins@oxidav"), "{out}");
        assert!(out.contains("DTEND:20240315T110000Z"), "{out}");
    }

    #[test]
    fn reconcile_removes_a_cleared_modelled_property() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:del@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Con titulo\r\nDESCRIPTION:para borrar\r\n\
             DTSTART:20240315T100000Z\r\nDTEND:20240315T110000Z\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "del@oxidav".to_string(),
            summary: Some("Con titulo".to_string()),
            description: None,
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(!out.contains("DESCRIPTION:"), "{out}");
        // Everything else is present (SUMMARY/DTSTAMP rewritten identically).
        assert!(out.contains("SUMMARY:Con titulo"), "{out}");
        assert!(out.contains("DTSTART:20240315T100000Z"), "{out}");
        assert!(out.contains("DTEND:20240315T110000Z"), "{out}");
        assert!(out.contains("UID:del@oxidav"), "{out}");
    }

    #[test]
    fn reconcile_normalises_a_duplicated_modelled_property() {
        // Two top-level `SUMMARY` lines (invalid per RFC 5545) plus the alarm's
        // own `SUMMARY`, which must stay untouched.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:dup@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Primero\r\nSUMMARY:Segundo\r\nDTSTART:20240315T100000Z\r\n\
             BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\n\
             SUMMARY:Recordatorio\r\nEND:VALARM\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "dup@oxidav".to_string(),
            summary: Some("Unico".to_string()),
            description: None,
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert_eq!(out.matches("SUMMARY:Unico").count(), 1, "{out}");
        assert!(!out.contains("SUMMARY:Primero"), "{out}");
        assert!(!out.contains("SUMMARY:Segundo"), "{out}");
        assert!(out.contains("SUMMARY:Recordatorio"), "{out}");
        // Two `SUMMARY` lines in total: the event's and the alarm's.
        let summaries = out.lines().filter(|l| l.starts_with("SUMMARY:")).count();
        assert_eq!(summaries, 2, "{out}");
    }

    #[test]
    fn reconcile_preserves_the_original_folding_of_untouched_lines() {
        // A non-modelled property folded over two physical lines: the first is
        // exactly 75 octets and the second begins with a single space.
        let first_physical = format!("X-LARGO:{}", "a".repeat(67));
        assert_eq!(first_physical.len(), 75);
        let raw = calendar(&format!(
            "BEGIN:VEVENT\r\nUID:fold@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Original\r\n{first_physical}\r\n bbb\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n"
        ));

        let fields = EventFields {
            uid: "fold@oxidav".to_string(),
            summary: Some("Cambiado".to_string()),
            description: None,
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(out.contains("SUMMARY:Cambiado"), "{out}");
        assert!(
            out.contains(&format!("{first_physical}\r\n bbb\r\n")),
            "the X-LARGO folding must be byte-identical:\n{out}"
        );
    }

    // -- F24 — `LOCATION`/`URL`: parse, compose and reconcile ------------------

    #[test]
    fn parse_ics_unescapes_location() {
        // The open question of PLAN-028: the `icalendar` parser types `LOCATION`
        // as `ValueType::Text` (like `SUMMARY`/`DESCRIPTION`), so it unescapes
        // the wire value itself — this test pins that empirically.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:loc@oxidav\r\nSUMMARY:Con ubicacion\r\n\
             LOCATION:Sala 3\\; planta 2\\, ala \\\\norte\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );

        let event = parse_ics(&raw).expect("parse");

        assert_eq!(
            event.location.as_deref(),
            Some("Sala 3; planta 2, ala \\norte")
        );
    }

    #[test]
    fn parse_ics_reads_url_verbatim() {
        // `URL` is a URI (`ValueType::Uri`): the parser must not touch its
        // commas, so the value comes back byte for byte.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:url@oxidav\r\nSUMMARY:Con url\r\n\
             URL:https://ejemplo.org/a?x=1&y=2,3\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );

        let event = parse_ics(&raw).expect("parse");

        assert_eq!(
            event.url.as_deref(),
            Some("https://ejemplo.org/a?x=1&y=2,3")
        );
    }

    #[test]
    fn build_ics_round_trips_location_and_url() {
        let fields = EventFields {
            uid: "roundtrip@oxidav".to_string(),
            summary: Some("RT".to_string()),
            description: None,
            location: Some("Sala 3; planta 2, ala \\norte".to_string()),
            url: Some("https://ejemplo.org/a?x=1&y=2,3".to_string()),
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let raw = build_ics(&fields);
        let parsed = parse_ics(&raw).expect("composed payload must parse");

        assert_eq!(parsed.location, fields.location);
        assert_eq!(parsed.url, fields.url);
    }

    #[test]
    fn build_ics_emits_url_verbatim_but_escapes_location() {
        let fields = EventFields {
            uid: "verbatim@oxidav".to_string(),
            summary: None,
            description: None,
            location: Some("a;b,c\\d".to_string()),
            url: Some("https://x.org/a,b;c".to_string()),
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let raw = build_ics(&fields);

        // Free text: `;`, `,` and `\` are escaped.
        assert!(raw.contains("LOCATION:a\\;b\\,c\\\\d"), "{raw}");
        // A URI: emitted verbatim, commas included, like `RRULE`.
        assert!(raw.contains("URL:https://x.org/a,b;c"), "{raw}");
    }

    #[test]
    fn build_ics_omits_location_and_url_when_none_is_byte_identical() {
        // E12: with `location` and `url` absent (`None`) the output is exactly
        // the pre-F24 bytes — the hard contract that keeps this change inert for
        // every event that does not use the two new fields.
        let fields = EventFields {
            uid: "f24-identity@oxidav".to_string(),
            summary: Some("Plain".to_string()),
            description: Some("Una descripcion".to_string()),
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let expected = concat!(
            "BEGIN:VCALENDAR\r\n",
            "VERSION:2.0\r\n",
            "PRODID:-//oxidav//oxidav//EN\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:f24-identity@oxidav\r\n",
            "DTSTAMP:20240315T100000Z\r\n",
            "SUMMARY:Plain\r\n",
            "DESCRIPTION:Una descripcion\r\n",
            "DTSTART:20240315T100000Z\r\n",
            "DTEND:20240315T110000Z\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        );

        assert_eq!(build_ics(&fields), expected);
    }

    #[test]
    fn build_ics_places_location_and_url_in_canonical_order() {
        let fields = EventFields {
            uid: "order@oxidav".to_string(),
            summary: Some("Orden".to_string()),
            description: Some("d".to_string()),
            location: Some("Sala".to_string()),
            url: Some("https://x.org".to_string()),
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let raw = build_ics(&fields);

        let description = raw.find("DESCRIPTION:d").expect("a DESCRIPTION line");
        let location = raw.find("LOCATION:Sala").expect("a LOCATION line");
        let url = raw.find("URL:https://x.org").expect("a URL line");
        let dtstart = raw.find("DTSTART:").expect("a DTSTART line");
        assert!(
            description < location && location < url && url < dtstart,
            "expected DESCRIPTION < LOCATION < URL < DTSTART:\n{raw}"
        );
    }

    #[test]
    fn reconcile_inserts_location_and_url_in_canonical_order() {
        // A payload without either property: reconciling with both set inserts
        // them, once each, at the canonical position (after `DESCRIPTION`,
        // before `URL`, and both before `DTSTART`).
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:insloc@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Con titulo\r\nDESCRIPTION:desc\r\nDTSTART:20240315T100000Z\r\n\
             DTEND:20240315T110000Z\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "insloc@oxidav".to_string(),
            summary: Some("Con titulo".to_string()),
            description: Some("desc".to_string()),
            location: Some("Sala 3".to_string()),
            url: Some("https://ejemplo.org/x".to_string()),
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert_eq!(out.matches("LOCATION:").count(), 1, "{out}");
        assert_eq!(out.matches("URL:").count(), 1, "{out}");
        let description = out.find("DESCRIPTION:desc").expect("a DESCRIPTION line");
        let location = out.find("LOCATION:Sala 3").expect("a LOCATION line");
        let url = out.find("URL:https://ejemplo.org/x").expect("a URL line");
        let dtstart = out.find("DTSTART:").expect("a DTSTART line");
        assert!(
            description < location && location < url && url < dtstart,
            "expected DESCRIPTION < LOCATION < URL < DTSTART:\n{out}"
        );
    }

    #[test]
    fn reconcile_replaces_location_and_escapes_reserved_characters() {
        // Part of E4 (write side): the stored `LOCATION:Sala 3` is replaced in
        // place by the escaped new value, single line, and nothing else moves.
        let fields = EventFields {
            uid: "rich@oxidav".to_string(),
            summary: Some("Original".to_string()),
            description: Some("Una descripcion".to_string()),
            location: Some("Sala 3; planta 2, ala \\norte".to_string()),
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            // F25: la tarea de API los cablea.
            categories: vec!["Reunion".to_string(), "Trabajo".to_string()],
            attach: Vec::new(),
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(RICH_ICS, &fields).expect("reconcile");

        assert!(
            out.contains("LOCATION:Sala 3\\; planta 2\\, ala \\\\norte"),
            "{out}"
        );
        // Exactly one `LOCATION`, and the old unescaped line is gone.
        assert_eq!(out.matches("LOCATION:").count(), 1, "{out}");
        assert!(!out.contains("LOCATION:Sala 3\r\n"), "{out}");
        // The rest of the payload is untouched.
        assert!(out.contains("SUMMARY:Original"), "{out}");
        assert!(out.contains("DESCRIPTION:Una descripcion"), "{out}");
        // `CATEGORIES` is modelled as of F25 and survives because the reconciled
        // value renders the very same line back.
        assert!(out.contains("CATEGORIES:Reunion,Trabajo"), "{out}");
        assert!(out.contains("X-CUSTOM-PROP:valor-propio"), "{out}");
        assert!(out.contains("BEGIN:VALARM"), "{out}");
        assert!(out.contains("TRIGGER:-PT15M"), "{out}");
    }

    #[test]
    fn reconcile_clears_location_when_absent() {
        // A `None` final value removes every `LOCATION:` line while leaving every
        // unmodelled property and the `VALARM` intact.
        let fields = EventFields {
            uid: "rich@oxidav".to_string(),
            summary: Some("Original".to_string()),
            description: Some("Una descripcion".to_string()),
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            // F25: la tarea de API los cablea.
            categories: vec!["Reunion".to_string(), "Trabajo".to_string()],
            attach: Vec::new(),
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(RICH_ICS, &fields).expect("reconcile");

        assert!(!out.contains("LOCATION:"), "{out}");
        // The unmodelled content survives.
        assert!(
            out.contains("ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:luis@example.org"),
            "{out}"
        );
        assert!(out.contains("X-CUSTOM-PROP:valor-propio"), "{out}");
        assert!(out.contains("CATEGORIES:Reunion,Trabajo"), "{out}");
        assert!(out.contains("BEGIN:VALARM"), "{out}");
        assert!(out.contains("SUMMARY:Recordatorio"), "{out}");
    }

    #[test]
    fn reconcile_changes_location_but_keeps_url_untouched() {
        // Changing `location` (rewritten in place) must not move or re-escape the
        // `URL`, which stays verbatim where it was.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:keepurl@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Con url\r\nLOCATION:Vieja\r\nURL:https://ejemplo.org/a,b;c\r\n\
             DTSTART:20240315T100000Z\r\nDTEND:20240315T110000Z\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "keepurl@oxidav".to_string(),
            summary: Some("Con url".to_string()),
            description: None,
            location: Some("Nueva".to_string()),
            url: Some("https://ejemplo.org/a,b;c".to_string()),
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            // F25: la tarea de API los cablea.
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(out.contains("LOCATION:Nueva\r\n"), "{out}");
        assert!(!out.contains("LOCATION:Vieja"), "{out}");
        assert!(out.contains("URL:https://ejemplo.org/a,b;c\r\n"), "{out}");
        let location = out.find("LOCATION:Nueva").expect("a LOCATION line");
        let url = out
            .find("URL:https://ejemplo.org/a,b;c")
            .expect("a URL line");
        let dtstart = out.find("DTSTART:").expect("a DTSTART line");
        assert!(location < url && url < dtstart, "{out}");
    }

    // -- F25 — the six new properties: compose, canonical order and multi-value
    // reconciliation (PLAN-029, tasks 4-5) -------------------------------------

    #[test]
    fn build_ics_emits_categories_escaped_and_attach_verbatim() {
        let fields = EventFields {
            uid: "f25-cat-attach@oxidav".to_string(),
            summary: None,
            description: None,
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: vec!["trabajo".to_string(), "con, coma".to_string()],
            attach: vec![
                "https://a.example/x".to_string(),
                "https://b.example/y&z,w".to_string(),
            ],
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let raw = build_ics(&fields);

        // One `CATEGORIES` line, each item escaped: the comma *inside* the second
        // item becomes `\,`, the separating comma stays a bare `,`.
        assert!(raw.contains("CATEGORIES:trabajo,con\\, coma\r\n"), "{raw}");
        assert_eq!(raw.matches("CATEGORIES:").count(), 1, "{raw}");
        // One `ATTACH` line per value, emitted verbatim: the `&`, the `,` and the
        // `=` of the URIs are untouched.
        assert!(raw.contains("ATTACH:https://a.example/x\r\n"), "{raw}");
        assert!(raw.contains("ATTACH:https://b.example/y&z,w\r\n"), "{raw}");
        assert_eq!(raw.matches("ATTACH:").count(), 2, "{raw}");
    }

    #[test]
    fn build_ics_omits_the_new_properties_when_empty_is_byte_identical() {
        let fields = EventFields {
            uid: "f25-identity@oxidav".to_string(),
            summary: Some("Plain".to_string()),
            description: Some("Una descripcion".to_string()),
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        // The hard contract: with the six new fields empty the output is exactly
        // the pre-F25 bytes.
        let expected = concat!(
            "BEGIN:VCALENDAR\r\n",
            "VERSION:2.0\r\n",
            "PRODID:-//oxidav//oxidav//EN\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:f25-identity@oxidav\r\n",
            "DTSTAMP:20240315T100000Z\r\n",
            "SUMMARY:Plain\r\n",
            "DESCRIPTION:Una descripcion\r\n",
            "DTSTART:20240315T100000Z\r\n",
            "DTEND:20240315T110000Z\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        );

        assert_eq!(build_ics(&fields), expected);
    }

    #[test]
    fn build_ics_places_the_new_properties_in_canonical_order() {
        let fields = EventFields {
            uid: "f25-order@oxidav".to_string(),
            summary: None,
            description: None,
            location: None,
            url: Some("https://x.org".to_string()),
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: Some("FREQ=DAILY".to_string()),
            status: Some("CONFIRMED".to_string()),
            class_: Some("PRIVATE".to_string()),
            transp: Some("TRANSPARENT".to_string()),
            duration: None,
            categories: vec!["trabajo".to_string()],
            attach: vec!["https://a.example/x".to_string()],
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let raw = build_ics(&fields);

        let url = raw.find("URL:https://x.org").expect("a URL line");
        let categories = raw.find("CATEGORIES:").expect("a CATEGORIES line");
        let attach = raw.find("ATTACH:").expect("an ATTACH line");
        let dtstart = raw.find("DTSTART:").expect("a DTSTART line");
        assert!(
            url < categories && categories < attach && attach < dtstart,
            "expected URL < CATEGORIES < ATTACH < DTSTART:\n{raw}"
        );

        let rrule = raw.find("RRULE:").expect("an RRULE line");
        let status = raw.find("STATUS:").expect("a STATUS line");
        let class = raw.find("CLASS:").expect("a CLASS line");
        let transp = raw.find("TRANSP:").expect("a TRANSP line");
        assert!(
            rrule < status && status < class && class < transp,
            "expected RRULE < STATUS < CLASS < TRANSP:\n{raw}"
        );
    }

    #[test]
    fn reconcile_replaces_attach_block_and_drops_the_extra_occurrences() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:attach@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Con adjuntos\r\n\
             ATTACH:https://a.example/x\r\n\
             ATTACH:https://b.example/y\r\n\
             ATTACH:https://c.example/z\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "attach@oxidav".to_string(),
            summary: Some("Con adjuntos".to_string()),
            description: None,
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: vec![
                "https://a.example/x".to_string(),
                "https://zz.example/w".to_string(),
            ],
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        // The first occurrence becomes the whole block, in order; the two extra
        // occurrences are gone.
        let attaches: Vec<&str> = out
            .lines()
            .filter(|line| line.starts_with("ATTACH:"))
            .collect();
        assert_eq!(
            attaches,
            vec!["ATTACH:https://a.example/x", "ATTACH:https://zz.example/w"],
            "{out}"
        );
        // Nothing else moves.
        assert!(out.contains("SUMMARY:Con adjuntos\r\n"), "{out}");
        assert!(out.contains("DTSTART:20240315T100000Z\r\n"), "{out}");
        assert!(!out.contains("https://b.example/y"), "{out}");
        assert!(!out.contains("https://c.example/z"), "{out}");
    }

    #[test]
    fn reconcile_clears_attach_and_categories_when_empty() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:clr@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Con extras\r\n\
             CATEGORIES:trabajo,personal\r\n\
             ATTACH:https://a.example/x\r\n\
             ATTACH:https://b.example/y\r\n\
             X-CUSTOM:keep-me\r\n\
             DTSTART:20240315T100000Z\r\n\
             BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\n\
             SUMMARY:Recordatorio\r\nEND:VALARM\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "clr@oxidav".to_string(),
            summary: Some("Con extras".to_string()),
            description: None,
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(!out.contains("CATEGORIES:"), "{out}");
        assert!(!out.contains("ATTACH:"), "{out}");
        // Everything else — the `X-` property and the whole `VALARM` — survives.
        assert!(out.contains("X-CUSTOM:keep-me\r\n"), "{out}");
        assert!(out.contains("BEGIN:VALARM"), "{out}");
        assert!(out.contains("TRIGGER:-PT15M"), "{out}");
        assert!(out.contains("SUMMARY:Recordatorio"), "{out}");
    }

    #[test]
    fn reconcile_replaces_categories_collapsing_two_lines_into_one() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:cat@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Con categorias\r\n\
             CATEGORIES:trabajo\r\n\
             CATEGORIES:personal\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "cat@oxidav".to_string(),
            summary: Some("Con categorias".to_string()),
            description: None,
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: vec!["trabajo".to_string(), "personal".to_string()],
            attach: Vec::new(),
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        // The two legacy `CATEGORIES` lines collapse into a single one.
        assert_eq!(out.matches("CATEGORIES:").count(), 1, "{out}");
        assert!(out.contains("CATEGORIES:trabajo,personal\r\n"), "{out}");
    }

    #[test]
    fn reconcile_keeps_the_new_properties_when_fields_carry_them() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:f25-keep@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Original\r\n\
             DESCRIPTION:Una descripcion\r\n\
             LOCATION:Sala 3\r\n\
             CATEGORIES:Reunion,Trabajo\r\n\
             STATUS:CONFIRMED\r\n\
             CLASS:PRIVATE\r\n\
             TRANSP:TRANSPARENT\r\n\
             DURATION:PT1H\r\n\
             ATTACH:https://a.example/x\r\n\
             ATTACH:https://b.example/y\r\n\
             DTSTART:20240315T100000Z\r\n\
             DTEND:20240315T110000Z\r\n\
             BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\n\
             SUMMARY:Recordatorio\r\nEND:VALARM\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "f25-keep@oxidav".to_string(),
            summary: Some("Nuevo".to_string()),
            description: Some("Una descripcion".to_string()),
            location: Some("Sala 3".to_string()),
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: None,
            status: Some("CONFIRMED".to_string()),
            class_: Some("PRIVATE".to_string()),
            transp: Some("TRANSPARENT".to_string()),
            duration: Some("PT1H".to_string()),
            categories: vec!["Reunion".to_string(), "Trabajo".to_string()],
            attach: vec![
                "https://a.example/x".to_string(),
                "https://b.example/y".to_string(),
            ],
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        // The F24 lesson: when `fields` carries the new values, only `SUMMARY`
        // and `DTSTAMP` differ, line for line.
        let before: Vec<&str> = raw.lines().collect();
        let after: Vec<&str> = out.lines().collect();
        assert_eq!(before.len(), after.len(), "line count changed:\n{out}");
        let diffs: Vec<&str> = before
            .iter()
            .zip(&after)
            .filter(|(before, after)| before != after)
            .map(|(before, _)| *before)
            .collect();
        assert_eq!(diffs.len(), 2, "only SUMMARY/DTSTAMP may change:\n{out}");
        assert!(
            diffs.iter().any(|line| line.starts_with("SUMMARY:")),
            "{diffs:?}"
        );
        assert!(
            diffs.iter().any(|line| line.starts_with("DTSTAMP:")),
            "{diffs:?}"
        );

        // With the six empty in `fields`, those lines disappear — correct, and
        // the reason the REST API inherits them from the payload before
        // reconciling.
        let mut cleared = fields.clone();
        cleared.status = None;
        cleared.class_ = None;
        cleared.transp = None;
        cleared.duration = None;
        cleared.categories = Vec::new();
        cleared.attach = Vec::new();
        let cleared_out = reconcile_ics(&raw, &cleared).expect("reconcile");
        for absent in [
            "STATUS:",
            "CLASS:",
            "TRANSP:",
            "DURATION:",
            "CATEGORIES:",
            "ATTACH:",
        ] {
            assert!(
                !cleared_out.contains(absent),
                "{absent} should be gone:\n{cleared_out}"
            );
        }
    }

    // -- F27 — `EXDATE`/`RDATE`: `property_param`, emisión y reconciliación ----

    /// `EventFields` mínimo con recurrencia, para los tests de `EXDATE`/`RDATE`.
    fn recur_fields() -> EventFields {
        EventFields {
            uid: "recur@oxidav".to_string(),
            summary: Some("Recur".to_string()),
            description: None,
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: None,
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: Some("FREQ=WEEKLY;COUNT=3".to_string()),
            exdate: None,
            rdate: None,
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            dtstamp_ms: 1_710_496_800_000,
        }
    }

    #[test]
    fn property_param_reads_the_head_case_insensitively() {
        // Con parámetro, sin distinguir mayúsculas.
        assert_eq!(
            property_param("EXDATE;TZID=Europe/Madrid:20260315T100000", "TZID"),
            Some("Europe/Madrid")
        );
        assert_eq!(
            property_param("EXDATE;tzid=Europe/Madrid:20260315T100000", "TZID"),
            Some("Europe/Madrid")
        );
        // Comillas despojadas del valor.
        assert_eq!(
            property_param("EXDATE;TZID=\"Europe/Madrid\":20260315T100000", "TZID"),
            Some("Europe/Madrid")
        );
        // Ausente.
        assert_eq!(property_param("EXDATE;VALUE=DATE:20260315", "TZID"), None);
        // Si el parámetro se repite, gana la ÚLTIMA ocurrencia.
        assert_eq!(
            property_param("EXDATE;TZID=America/New_York;TZID=Europe/Madrid:x", "TZID"),
            Some("Europe/Madrid")
        );
        // Una línea sin `:` no tiene head con parámetros.
        assert_eq!(property_param("BEGIN:VEVENT", "TZID"), None);
    }

    #[test]
    fn build_ics_omits_recur_dates_when_none_or_empty() {
        // Escenario 6: `None` y `Some(vec![])` no producen línea alguna y la
        // salida es byte a byte la esperada (sin `EXDATE:`/`RDATE:`).
        let fields = recur_fields();

        let expected = concat!(
            "BEGIN:VCALENDAR\r\n",
            "VERSION:2.0\r\n",
            "PRODID:-//oxidav//oxidav//EN\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:recur@oxidav\r\n",
            "DTSTAMP:20240315T100000Z\r\n",
            "SUMMARY:Recur\r\n",
            "DTSTART:20240315T100000Z\r\n",
            "RRULE:FREQ=WEEKLY;COUNT=3\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        );
        assert_eq!(build_ics(&fields), expected);

        let mut cleared = fields.clone();
        cleared.exdate = Some(Vec::new());
        cleared.rdate = Some(Vec::new());
        let raw = build_ics(&cleared);
        assert!(!raw.contains("EXDATE:"), "{raw}");
        assert!(!raw.contains("RDATE:"), "{raw}");
        assert_eq!(raw, expected, "`Some(vec![])` debe equivaler a `None`");
    }

    #[test]
    fn build_ics_emits_recur_dates_always_in_utc() {
        // Decisión M-2 (F27): aunque `DTSTART` esté anclado a `Europe/Madrid`,
        // `EXDATE`/`RDATE` se emiten **siempre** en UTC (`…Z`), nunca en la zona
        // de `DTSTART`. Emitirlos en la zona es lossless sólo fuera de la hora
        // ambigua del fall-back; el *round-trip* UTC es exacto.
        let a = parse_datetime_with_tzid("20260315T100000", Some("Europe/Madrid"))
            .expect("parse")
            .0;
        let b = parse_datetime_with_tzid("20260322T100000", Some("Europe/Madrid"))
            .expect("parse")
            .0;

        let mut fields = recur_fields();
        fields.dtstart_tzid = Some("Europe/Madrid".to_string());
        fields.exdate = Some(vec![b, a]); // desordenados a propósito
        fields.rdate = Some(vec![a, a]); // con un duplicado a propósito

        let raw = build_ics(&fields);

        // UTC, en una sola línea, ordenada y sin duplicados. Nunca `;TZID=`.
        assert!(
            raw.contains("EXDATE:20260315T090000Z,20260322T090000Z\r\n"),
            "{raw}"
        );
        assert!(raw.contains("RDATE:20260315T090000Z\r\n"), "{raw}");
        assert_eq!(raw.matches("EXDATE").count(), 1, "{raw}");
        assert_eq!(raw.matches("RDATE").count(), 1, "{raw}");
        assert!(!raw.contains("EXDATE;TZID="), "{raw}");
        assert!(!raw.contains("RDATE;TZID="), "{raw}");
        // `DTSTART` sí conserva su zona y su hora local.
        assert!(
            raw.contains("DTSTART;TZID=Europe/Madrid:20240315T110000\r\n"),
            "{raw}"
        );
        // La zona se define antes del evento.
        assert!(
            raw.find("BEGIN:VTIMEZONE").expect("a VTIMEZONE") < raw.find("BEGIN:VEVENT").unwrap(),
            "{raw}"
        );

        // Round-trip **exacto**: cada token UTC vuelve al mismo instante.
        let line = raw
            .lines()
            .find(|line| line.starts_with("EXDATE"))
            .expect("an EXDATE line");
        let round: Vec<i64> = property_value(line)
            .split(',')
            .map(|token| parse_datetime_ms(token).expect("parse token"))
            .collect();
        assert_eq!(round, vec![a, b]);
    }

    #[test]
    fn build_ics_recur_dates_round_trip_exactly_across_the_fall_back_hour() {
        // Un instante que cae en la **segunda** pasada de la hora repetida del
        // fall-back de `Europe/Madrid` (2026-10-25: 03:00 CEST → 02:00 CET).
        // Emitirlo en la zona de `DTSTART` lo escribiría como 02:30 local y al
        // re-parsear se resolvería a la PRIMERA pasada (00:30 UTC), moviéndolo
        // −1 h en silencio. La emisión UTC lo conserva exacto.
        let second_pass = datetime_ms("20261025T013000Z");
        let first_pass = parse_datetime_with_tzid("20261025T023000", Some("Europe/Madrid"))
            .expect("parse")
            .0;
        // Confirmamos el escenario: 02:30 local resuelve a 00:30 UTC, y el
        // instante elegido es exactamente una hora después (segunda pasada).
        assert_eq!(first_pass, datetime_ms("20261025T003000Z"));
        assert_eq!(second_pass, first_pass + 3_600_000);

        let mut fields = recur_fields();
        fields.dtstart_tzid = Some("Europe/Madrid".to_string());
        fields.exdate = Some(vec![second_pass]);

        let raw = build_ics(&fields);

        let line = raw
            .lines()
            .find(|line| line.starts_with("EXDATE"))
            .expect("an EXDATE line");
        let token = property_value(line);
        assert_eq!(token, "20261025T013000Z");
        // Blinda la propiedad: el token vuelve al instante original, NO al de la
        // primera pasada (que es lo que ocurriría con una emisión en la zona).
        assert_eq!(parse_datetime_ms(token), Some(second_pass));
        assert_ne!(parse_datetime_ms(token), Some(first_pass));
    }

    #[test]
    fn build_ics_emits_recur_dates_in_utc_without_zone() {
        // Escenario 7 (sin zona): emisión UTC con sufijo `Z`.
        let a = datetime_ms("20260315T100000Z");
        let b = datetime_ms("20260322T100000Z");

        let mut fields = recur_fields();
        fields.exdate = Some(vec![a, b]);

        let raw = build_ics(&fields);

        assert!(
            raw.contains("EXDATE:20260315T100000Z,20260322T100000Z\r\n"),
            "{raw}"
        );
        assert!(!raw.contains("EXDATE;TZID="), "{raw}");
    }

    #[test]
    fn reconcile_keeps_opaque_exdate_verbatim_when_none() {
        // Escenario 8: con `fields.exdate = None` la línea opaca del payload no se
        // toca (ni se reescribe ni se elimina).
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:opaque@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Recur\r\nEXDATE;VALUE=DATE:20260315\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );
        let fields = recur_fields();

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(out.contains("EXDATE;VALUE=DATE:20260315\r\n"), "{out}");
    }

    #[test]
    fn reconcile_clears_all_recur_date_lines_when_empty() {
        // Escenario 9: `Some(vec![])` elimina TODAS las líneas `EXDATE`.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:clear@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Recur\r\nEXDATE:20260315T100000Z\r\nEXDATE:20260322T100000Z\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );
        let mut fields = recur_fields();
        fields.exdate = Some(Vec::new());

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(!out.contains("EXDATE"), "{out}");
        // El resto del payload sigue intacto.
        assert!(out.contains("SUMMARY:Recur\r\n"), "{out}");
        assert!(out.contains("DTSTART:20240315T100000Z\r\n"), "{out}");
    }

    #[test]
    fn reconcile_collapses_recur_date_lines_into_one() {
        // Escenario 10: varias líneas `EXDATE` se colapsan en UNA sola con los
        // valores unidos por comas, ordenados y deduplicados.
        let a = datetime_ms("20260315T100000Z");
        let b = datetime_ms("20260322T100000Z");
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:collapse@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Recur\r\nEXDATE:20260315T100000Z\r\nEXDATE:20260322T100000Z\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );
        let mut fields = recur_fields();
        fields.exdate = Some(vec![a, b]);

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert_eq!(out.matches("EXDATE").count(), 1, "{out}");
        assert!(
            out.contains("EXDATE:20260315T100000Z,20260322T100000Z\r\n"),
            "{out}"
        );
        assert!(out.contains("SUMMARY:Recur\r\n"), "{out}");
    }

    #[test]
    fn reconcile_inserts_recur_date_after_rrule() {
        // Escenario 11: sin ninguna línea `EXDATE`, se inserta en su posición
        // canónica (tras `RRULE`, dentro del `VEVENT`).
        let a = datetime_ms("20260315T100000Z");
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:insert@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Recur\r\nDTSTART:20240315T100000Z\r\n\
             RRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\n",
        );
        let mut fields = recur_fields();
        fields.exdate = Some(vec![a]);

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert_eq!(out.matches("EXDATE").count(), 1, "{out}");
        assert!(out.contains("EXDATE:20260315T100000Z\r\n"), "{out}");
        let rrule = out.find("RRULE:").expect("an RRULE line");
        let exdate = out.find("EXDATE:").expect("an EXDATE line");
        let end = out.find("END:VEVENT").expect("the VEVENT end");
        assert!(
            rrule < exdate && exdate < end,
            "EXDATE debe quedar entre RRULE y END:VEVENT:\n{out}"
        );
    }

    #[test]
    fn reconcile_keeps_opaque_exdate_byte_for_byte_when_none() {
        // m-3 (F27): con `fields.exdate = None` la línea `EXDATE;VALUE=DATE:…` es
        // opaca y no se toca. Todos los demás campos modelados ya coinciden con
        // el payload, así que la única diferencia potencial sería esa `EXDATE`:
        // la salida debe ser **idéntica**, byte a byte.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:opaque-b@oxidav\r\nDTSTAMP:20240315T100000Z\r\n\
             SUMMARY:Recur\r\nDESCRIPTION:Una descripcion\r\n\
             DTSTART:20240315T100000Z\r\nDTEND:20240315T110000Z\r\n\
             RRULE:FREQ=WEEKLY;COUNT=3\r\n\
             EXDATE;VALUE=DATE:20260315\r\nEND:VEVENT\r\n",
        );
        let fields = EventFields {
            uid: "opaque-b@oxidav".to_string(),
            summary: Some("Recur".to_string()),
            description: Some("Una descripcion".to_string()),
            location: None,
            url: None,
            dtstart_ms: 1_710_496_800_000,
            dtend_ms: Some(1_710_500_400_000),
            dtstart_tzid: None,
            dtend_tzid: None,
            rrule: Some("FREQ=WEEKLY;COUNT=3".to_string()),
            status: None,
            class_: None,
            transp: None,
            duration: None,
            categories: Vec::new(),
            attach: Vec::new(),
            dtstamp_ms: 1_710_496_800_000,
            exdate: None,
            rdate: None,
        };

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert_eq!(out, raw, "la EXDATE opaca no debe tocarse");
    }

    #[test]
    fn reconcile_collapses_rdate_lines_into_one() {
        // m-4 (F27): varias líneas `RDATE` se colapsan en UNA sola, ordenadas y
        // deduplicadas (misma mecánica que `EXDATE`, pero para `RDATE`).
        let a = datetime_ms("20260401T090000Z");
        let b = datetime_ms("20260408T090000Z");
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:rdate@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Recur\r\nRDATE:20260401T090000Z\r\nRDATE:20260408T090000Z\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );
        let mut fields = recur_fields();
        fields.rdate = Some(vec![a, b]);

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert_eq!(out.matches("RDATE").count(), 1, "{out}");
        assert!(
            out.contains("RDATE:20260401T090000Z,20260408T090000Z\r\n"),
            "{out}"
        );
        assert!(out.contains("SUMMARY:Recur\r\n"), "{out}");
    }

    #[test]
    fn reconcile_inserts_rdate_after_rrule_when_absent() {
        // m-4 (F27): sin ninguna línea `RDATE`, se inserta en su posición
        // canónica (tras `RRULE`, dentro del `VEVENT`).
        let r = datetime_ms("20260401T090000Z");
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:insrdate@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Recur\r\nDTSTART:20240315T100000Z\r\n\
             RRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\n",
        );
        let mut fields = recur_fields();
        fields.rdate = Some(vec![r]);

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(out.contains("RDATE:20260401T090000Z\r\n"), "{out}");
        assert!(!out.contains("EXDATE"), "{out}");
        let rrule = out.find("RRULE:").expect("an RRULE line");
        let rdate = out.find("RDATE:").expect("an RDATE line");
        let end = out.find("END:VEVENT").expect("the VEVENT end");
        assert!(
            rrule < rdate && rdate < end,
            "RDATE debe quedar entre RRULE y END:VEVENT:\n{out}"
        );
    }

    #[test]
    fn reconcile_clears_rdate_and_exdate_lines_when_empty() {
        // m-4 (F27): `Some(vec![])` en ambos campos elimina TODAS las líneas
        // `RDATE` y `EXDATE`.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:clearboth@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Recur\r\nRDATE:20260401T090000Z\r\nEXDATE:20260315T100000Z\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );
        let mut fields = recur_fields();
        fields.rdate = Some(Vec::new());
        fields.exdate = Some(Vec::new());

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(!out.contains("RDATE"), "{out}");
        assert!(!out.contains("EXDATE"), "{out}");
        assert!(out.contains("SUMMARY:Recur\r\n"), "{out}");
    }

    #[test]
    fn reconcile_inserts_rdate_and_exdate_in_canonical_order() {
        // m-4 (F27): inserción simultánea de `RDATE` y `EXDATE` en un payload
        // sin ninguna de las dos: el orden canónico es RDATE antes que EXDATE,
        // ambos tras `RRULE`.
        let r = datetime_ms("20260401T090000Z");
        let x = datetime_ms("20260315T100000Z");
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:both@oxidav\r\nDTSTAMP:20240315T090000Z\r\n\
             SUMMARY:Recur\r\nDTSTART:20240315T100000Z\r\n\
             RRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\n",
        );
        let mut fields = recur_fields();
        fields.rdate = Some(vec![r]);
        fields.exdate = Some(vec![x]);

        let out = reconcile_ics(&raw, &fields).expect("reconcile");

        assert!(out.contains("RDATE:20260401T090000Z\r\n"), "{out}");
        assert!(out.contains("EXDATE:20260315T100000Z\r\n"), "{out}");
        let rrule = out.find("RRULE:").expect("an RRULE line");
        let rdate = out.find("RDATE:").expect("an RDATE line");
        let exdate = out.find("EXDATE:").expect("an EXDATE line");
        let end = out.find("END:VEVENT").expect("the VEVENT end");
        assert!(
            rrule < rdate && rdate < exdate && exdate < end,
            "RDATE debe ir antes que EXDATE, ambos tras RRULE:\n{out}"
        );
    }
}
