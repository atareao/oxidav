//! F18 — `VTIMEZONE` derivation from the embedded IANA tzdb (`chrono-tz`).
//!
//! RFC 5545 §3.2.19 requires that a calendar carrying `DTSTART;TZID=<zone>`
//! **also** define `<zone>` with a `VTIMEZONE` component; the `TZID` parameter
//! alone is not enough, because without the definition a client cannot know the
//! zone's UTC offsets and DST rules. This module turns a `chrono_tz::Tz` into the
//! body of that `VTIMEZONE` (the `TZID` line and the `STANDARD`/`DAYLIGHT`
//! observances that follow), so [`super::ics::build_ics`] can embed it whenever an
//! event is anchored to a zone.
//!
//! # Why observances and not a `TZID` token
//!
//! A `VTIMEZONE` is a *historical* list of offset changes, not a rule: each
//! `STANDARD`/`DAYLIGHT` sub-component records one onset (`DTSTART`), the offset
//! in force **immediately before** that onset (`TZOFFSETFROM`), the offset from
//! that onset onwards (`TZOFFSETTO`) and, optionally, the abbreviation in force
//! (`TZNAME`). We therefore enumerate the zone's real transitions inside a finite
//! window and emit one sub-component per transition.
//!
//! Per RFC 5545 §3.6.5 an onset `DTSTART` is expressed in the **local time of
//! `TZOFFSETFROM`** (the offset *before* the change), which is why the onset epoch
//! is shifted by the previous offset rather than the new one.
//!
//! # Classification of observances
//!
//! A span whose `dst_offset` is non-zero is a `DAYLIGHT` observance and anything
//! else a `STANDARD` one. This is deliberately **not** "the offset gets larger":
//! tzdb models `Europe/Dublin` with *negative* DST (base `IST` +01:00, winter
//! `GMT` +00:00 as a `-01:00` DST offset), so a naive "summer = DAYLIGHT" rule
//! would mislabel both of Dublin's observances. Testing the DST component
//! directly classifies every zone, including the negative-DST ones, correctly.
//!
//! # How transitions are obtained
//!
//! `chrono-tz` 0.10.4 does **not** expose its raw transition tables: the internal
//! `TimeSpans`/`FixedTimespanSet` types are private, so `Tz::timespans()` is not
//! callable from a downstream crate. What *is* public is the `chrono::TimeZone`
//! implementation for `Tz` (and the `chrono_tz::{OffsetComponents, OffsetName}`
//! traits), so we recover the transitions **behaviourally**: sample the offset on
//! a fixed grid and, wherever two consecutive samples disagree, binary-search the
//! interval down to the exact second of the change. The zone's offset is a step
//! function (piecewise constant), so this reconstructs the same transitions the
//! tzdb tables describe, to the second.
//!
//! The reconstruction's correctness rests on a single invariant: the grid step
//! ([`SAMPLE_STEP_SECONDS`]) must be **strictly shorter than the separation
//! between two consecutive transitions** of any real zone inside the window. If
//! two transitions were closer together than the step, a whole interval could
//! hide between two equal samples and be lost. It is *not* "every offset lasts
//! months": the **measured** minimum separation inside the modern window
//! `[2023, 2030)` is **9 days** (`America/Asuncion`, 2024-10-06 → 2024-10-15),
//! so a 7-day step is safe — but the margin is only **2 days**, not weeks. If a
//! future tzdb introduced transitions less than 7 days apart, this
//! reconstruction would silently drop them, which is why the step is a
//! parameter carrying that constraint rather than an incidental constant.
//!
//! # Finite window (documented limitation)
//!
//! Emitting a zone's *entire* transition history would produce hundreds of
//! observances for zones with a long tzdb record (millions of seconds of clocks
//! since 1880). The window is therefore bounded to the years
//! `reference_year - 1 ..= reference_year + 5` (seven years, see
//! [`window_bounds`]): enough to cover an event and several of its annual
//! recurrences around the reference date. Onsets **outside** this window are not
//! emitted, so a client expanding a recurrence far beyond `reference_year + 5`
//! may compute an offset that the zone no longer changes at exactly the right
//! instant. This limitation is documented in `docs/F18-timezone-support.md`.
//!
//! # Zone with no transitions in the window
//!
//! A zone that never changes offset (e.g. `Asia/Tokyo`, or the trivial `UTC`)
//! yields no transitions at all. RFC 5545 requires a `VTIMEZONE` to carry **at
//! least one** observance, so a single `STANDARD` observance pinning the constant
//! offset is emitted instead.
//!
//! [`super::ics::build_ics`]: crate::dav::ics::build_ics

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::{OffsetComponents, OffsetName, Tz, TzOffset};

/// Seconds in a day.
const SECONDS_PER_DAY: i64 = 86_400;
/// Milliseconds in a second.
const MILLIS_PER_SECOND: i64 = 1_000;
/// Sample grid used to locate offset changes, in seconds (one week).
///
/// See the module docs: it must be **strictly shorter** than the shortest gap
/// between two consecutive transitions of a real zone in the window. The
/// measured minimum in `[2023, 2030)` is 9 days (`America/Asuncion`), which
/// leaves a 2-day margin; a future tzdb with transitions < 7 days apart would be
/// reconstructed incorrectly. This step is the single knob the reconstruction's
/// correctness depends on.
const SAMPLE_STEP_SECONDS: i64 = 7 * SECONDS_PER_DAY;
/// Inclusive lower bound for the adjustable `reference_year`.
///
/// Keeps the four-digit `YYYY` of a rendered onset valid and the epoch inside
/// `chrono`'s representable range.
const MIN_REFERENCE_YEAR: i32 = 2;
/// Inclusive upper bound for the adjustable `reference_year`.
const MAX_REFERENCE_YEAR: i32 = 9_000;

/// Resolve an IANA time-zone name to a [`Tz`], if it is a known zone.
///
/// This is the single validation entry point shared with the REST layer (F18
/// Phase 5): a `tzid` that does not resolve here is rejected by the API.
#[must_use]
pub fn parse_tzid(name: &str) -> Option<Tz> {
    name.parse::<Tz>().ok()
}

/// Whether `name` is a known IANA time-zone identifier.
#[must_use]
pub fn is_valid_tzid(name: &str) -> bool {
    parse_tzid(name).is_some()
}

/// The canonical IANA name of `tz` (e.g. `Europe/Madrid`).
///
/// Emitted verbatim as the block's `TZID:` and reused as the `TZID=` parameter of
/// `DTSTART`/`DTEND`, so the two can never disagree. Aliases resolve to a
/// distinct `Tz` variant with its own name, and that name — not whatever string a
/// client sent — is what gets written.
#[must_use]
pub fn canonical_tzid(tz: Tz) -> &'static str {
    tz.name()
}

/// Render the Unix-millisecond instant `ms` as the **local** wall-clock time of
/// `tz`, formatted `YYYYMMDDTHHMMSS` **without** the `Z` suffix.
///
/// This is the composition counterpart of the `TZID`-aware parse in
/// [`super::ics`]: `DTSTART;TZID=<zone>` carries a local time, so the instant
/// stored in the database (UTC ms) has to be projected back into `tz` for the
/// `.ics` to round-trip. The offset in force is taken at the instant itself
/// (via the UTC view), so a zone change is handled correctly.
#[must_use]
pub fn format_local_datetime_ms(tz: Tz, ms: i64) -> String {
    let utc_seconds = ms.div_euclid(MILLIS_PER_SECOND);
    let offset = total_offset(&offset_at(tz, utc_seconds));
    format_epoch(utc_seconds + i64::from(offset))
}

/// Render the `VTIMEZONE` block for `tz`, covering the years
/// `reference_year - 1 ..= reference_year + 5`.
///
/// The returned string is CRLF-terminated and folded per RFC 5545 §3.1, ready to
/// be spliced into a `VCALENDAR` between `BEGIN:VCALENDAR` and its `VEVENT`. See
/// the module docs for the window, classification and reconstruction rationale.
#[must_use]
pub fn render_vtimezone(tz: Tz, reference_year: i32) -> String {
    let (lo, hi) = window_bounds(reference_year);
    let transitions = transitions_in_window(tz, lo, hi);

    let mut lines: Vec<String> = Vec::with_capacity(6 + transitions.len() * 6);
    lines.push("BEGIN:VTIMEZONE".to_string());
    lines.push(format!("TZID:{}", canonical_tzid(tz)));

    // The offset in force at the window start is the `TZOFFSETFROM` of the first
    // onset, so the very first `DAYLIGHT`/`STANDARD` carries the real preceding
    // span (not the new one).
    let initial = offset_at(tz, lo);
    let mut previous = initial;
    let mut emitted = 0usize;
    for &(onset, span) in &transitions {
        // A transition that does **not** change the total UTC offset (it only
        // reshuffles the base/dst decomposition) leaves the wall clock untouched,
        // so no observance is declared for it: emitting one would produce a
        // `TZOFFSETFROM == TZOFFSETTO` block that some clients reject. The span
        // is still carried forward, so the next real onset reports the offset
        // actually in force right before it (including its abbreviation).
        if total_offset(&previous) != total_offset(&span) {
            let local_onset = onset + i64::from(total_offset(&previous));
            push_observance(&mut lines, local_onset, &previous, &span);
            emitted += 1;
        }
        previous = span;
    }

    if emitted == 0 {
        // No wall-clock change in the window — a zone that never shifts (e.g.
        // `Asia/Tokyo`, or the trivial `UTC`) or one whose only tzdb edits are
        // base/dst reshuffles. RFC 5545 requires **at least one** observance, so
        // pin the constant offset as a single `STANDARD`.
        push_observance(
            &mut lines,
            lo + i64::from(total_offset(&initial)),
            &initial,
            &initial,
        );
    }

    lines.push("END:VTIMEZONE".to_string());

    let mut out = String::with_capacity(256);
    for line in &lines {
        super::common::fold_content_line(line, &mut out);
    }
    out
}

/// Append the lines of one `STANDARD`/`DAYLIGHT` observance.
///
/// `local_onset` is the onset instant already expressed in the local time of
/// `from` (RFC 5545 §3.6.5); `from` is the offset in force before the change and
/// `to` the one from the onset onwards.
fn push_observance(lines: &mut Vec<String>, local_onset: i64, from: &TzOffset, to: &TzOffset) {
    let kind = if to.dst_offset().num_seconds() != 0 {
        "DAYLIGHT"
    } else {
        "STANDARD"
    };
    lines.push(format!("BEGIN:{kind}"));
    lines.push(format!("DTSTART:{}", format_epoch(local_onset)));
    lines.push(format!(
        "TZOFFSETFROM:{}",
        format_offset(total_offset(from))
    ));
    lines.push(format!("TZOFFSETTO:{}", format_offset(total_offset(to))));
    if let Some(name) = to.abbreviation() {
        lines.push(format!("TZNAME:{name}"));
    }
    lines.push(format!("END:{kind}"));
}

/// `[lo, hi)` epoch-second bounds of the observance window for `reference_year`.
///
/// Seven whole years, `reference_year - 1 ..= reference_year + 5`. The year is
/// clamped to a sane range so the four-digit `YYYY` of every onset stays valid.
fn window_bounds(reference_year: i32) -> (i64, i64) {
    let year = reference_year.clamp(MIN_REFERENCE_YEAR, MAX_REFERENCE_YEAR);
    let lo = days_from_civil(i64::from(year - 1), 1, 1) * SECONDS_PER_DAY;
    let hi = days_from_civil(i64::from(year + 6), 1, 1) * SECONDS_PER_DAY;
    (lo, hi)
}

/// All `(onset_epoch_seconds, new_span)` transitions inside `[lo, hi)`.
///
/// The new span is the offset in force **from** the onset onwards. Results are in
/// chronological order.
fn transitions_in_window(tz: Tz, lo: i64, hi: i64) -> Vec<(i64, TzOffset)> {
    transitions_with_step(tz, lo, hi, SAMPLE_STEP_SECONDS)
}

/// [`transitions_in_window`] with an explicit grid step.
///
/// The step is a parameter so the reconstruction can be checked against a dense
/// (one-day) sampling in tests; production always uses [`SAMPLE_STEP_SECONDS`]
/// (see the module docs for the invariant it must satisfy).
fn transitions_with_step(tz: Tz, lo: i64, hi: i64, step: i64) -> Vec<(i64, TzOffset)> {
    let mut transitions = Vec::new();

    let mut base_sample = lo;
    let mut base_offset = offset_at(tz, lo);
    let mut sample = lo + step;
    while sample < hi {
        let offset = offset_at(tz, sample);
        if offset != base_offset {
            // The grid step guarantees at most one transition in the interval.
            let onset = first_change(tz, base_sample, sample, &base_offset);
            transitions.push((onset, offset_at(tz, onset)));
            base_offset = offset;
        }
        base_sample = sample;
        sample += step;
    }

    // The loop's last sample is strictly below `hi`, so the closing stretch
    // `(base_sample, hi)` is examined here. Without this check a transition in
    // the last days of the window is lost — e.g. Morocco's Ramadan-2030 onset on
    // 2029-12-31, which lies inside the `[2023, 2030)` window of reference year
    // 2024 and must be emitted.
    let tail = hi - 1;
    let tail_offset = offset_at(tz, tail);
    if tail_offset != base_offset {
        let onset = first_change(tz, base_sample, tail, &base_offset);
        transitions.push((onset, offset_at(tz, onset)));
    }

    transitions
}

/// The exact second of the first offset change in `(lo, hi]`.
///
/// Precondition: `offset_at(lo) == *old` and `offset_at(hi) != *old` (the caller
/// only calls this once a change has been bracketed). Because the offset is a
/// left-inclusive step function, the smallest second whose offset differs from
/// `old` is the onset instant.
fn first_change(tz: Tz, lo: i64, hi: i64, old: &TzOffset) -> i64 {
    let mut lo = lo;
    let mut hi = hi;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if &offset_at(tz, mid) == old {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// The offset in force at the UTC instant `epoch`.
///
/// The window is always modern (see [`window_bounds`]), but this function is
/// **also** reached from [`format_local_datetime_ms`] with arbitrary millisecond
/// values that flow straight from the database — and therefore from an
/// unvalidated client `dtstart`. `chrono` cannot represent every `i64` second, so
/// the instant is clamped to its representable range before the conversion: an
/// out-of-range value yields a deterministic offset (the one at the nearest
/// representable instant) instead of panicking.
fn offset_at(tz: Tz, epoch: i64) -> TzOffset {
    let clamped = epoch.clamp(
        DateTime::<Utc>::MIN_UTC.timestamp(),
        DateTime::<Utc>::MAX_UTC.timestamp(),
    );
    let instant =
        DateTime::from_timestamp(clamped, 0).expect("a clamped epoch is always representable");
    tz.offset_from_utc_datetime(&instant.naive_utc())
}

/// Total offset from UTC of `span`, in seconds (`base + dst`).
fn total_offset(span: &TzOffset) -> i32 {
    (span.base_utc_offset() + span.dst_offset()).num_seconds() as i32
}

/// Format an offset as `±HHMM`, or `±HHMMSS` when it carries seconds.
fn format_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let magnitude = seconds.unsigned_abs();
    let hours = magnitude / 3_600;
    let minutes = (magnitude % 3_600) / 60;
    let remaining_seconds = magnitude % 60;
    if remaining_seconds == 0 {
        format!("{sign}{hours:02}{minutes:02}")
    } else {
        format!("{sign}{hours:02}{minutes:02}{remaining_seconds:02}")
    }
}

/// Format an epoch second (already in the target local offset) as
/// `YYYYMMDDTHHMMSS`, **without** the `Z` suffix (a local onset is not UTC).
fn format_epoch(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(SECONDS_PER_DAY);
    let remainder = epoch_seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let hour = remainder / 3_600;
    let minute = (remainder % 3_600) / 60;
    let second = remainder % 60;
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}")
}

/// Inverse of [`days_from_civil`] (Howard Hinnant's `civil_from_days`).
///
/// Exact for the proleptic Gregorian calendar over the whole `i64` range.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// Days from `1970-01-01` to `year-month-day` (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap a rendered `VTIMEZONE` in a minimal `VCALENDAR` so the `icalendar`
    /// crate can parse it the way a client would.
    fn wrap_in_calendar(vtimezone: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//oxidav//EN\r\n{vtimezone}END:VCALENDAR\r\n"
        )
    }

    fn count_lines(body: &str, prefix: &str) -> usize {
        body.lines().filter(|line| line.starts_with(prefix)).count()
    }

    // -- F18 Paso 1 — `Tz: FromStr` round-trip ------------------------------

    #[test]
    fn tz_parses_known_iana_names_only() {
        assert_eq!(parse_tzid("Europe/Madrid"), Some(Tz::Europe__Madrid));
        assert!("Europe/Madrid".parse::<Tz>().is_ok());
        assert!("Mars/Olympus".parse::<Tz>().is_err());
        assert_eq!(parse_tzid("Mars/Olympus"), None);
        assert!(!is_valid_tzid("Mars/Olympus"));
        assert!(is_valid_tzid("UTC"));
    }

    // -- F18 Paso 4 — `Europe/Madrid` has two observances per year ----------

    #[test]
    fn madrid_renders_two_observances_per_year_without_z_suffix() {
        let body = render_vtimezone(Tz::Europe__Madrid, 2024);

        assert!(body.contains("BEGIN:VTIMEZONE\r\n"), "{body}");
        assert!(body.contains("TZID:Europe/Madrid\r\n"), "{body}");

        // Seven years (2023..=2029) × two transitions per year.
        assert_eq!(count_lines(&body, "BEGIN:DAYLIGHT"), 7, "{body}");
        assert_eq!(count_lines(&body, "BEGIN:STANDARD"), 7, "{body}");
        assert_eq!(
            count_lines(&body, "BEGIN:DAYLIGHT") + count_lines(&body, "BEGIN:STANDARD"),
            14
        );

        // Spring: CET (+01:00) -> CEST (+02:00); autumn: the reverse.
        assert!(body.contains("TZOFFSETFROM:+0100\r\n"), "{body}");
        assert!(body.contains("TZOFFSETTO:+0200\r\n"), "{body}");
        assert!(body.contains("TZOFFSETFROM:+0200\r\n"), "{body}");
        assert!(body.contains("TZOFFSETTO:+0100\r\n"), "{body}");

        // The 2023 spring onset is 2023-03-26 01:00 UTC expressed in the local
        // time of the previous offset (CET, +01:00) => 02:00 local, no `Z`.
        assert!(body.contains("DTSTART:20230326T020000\r\n"), "{body}");

        // Every `DTSTART` is a local time: none may carry the `Z` suffix.
        for line in body.lines().filter(|l| l.starts_with("DTSTART:")) {
            assert!(!line.ends_with('Z'), "local onset must not be UTC: {line}");
        }
    }

    // -- F18 Paso 5 — classification across zones ---------------------------

    #[test]
    fn dublin_negative_dst_is_classified_by_its_dst_component() {
        let body = render_vtimezone(Tz::Europe__Dublin, 2024);

        // Dublin uses negative DST: winter `GMT` carries `dst_offset = -1h`.
        // Classifying by the DST component (not by "offset grew") still yields
        // one DAYLIGHT (winter, GMT) and one STANDARD (summer, IST) per year.
        assert_eq!(count_lines(&body, "BEGIN:DAYLIGHT"), 7, "{body}");
        assert_eq!(count_lines(&body, "BEGIN:STANDARD"), 7, "{body}");

        // A DAYLIGHT block must be the winter one: offsetto +00:00, named GMT.
        let daylight_start = body.find("BEGIN:DAYLIGHT").expect("a DAYLIGHT block");
        let daylight_block = &body[daylight_start..];
        let daylight_end = daylight_block.find("END:DAYLIGHT").expect("END:DAYLIGHT");
        let daylight = &daylight_block[..daylight_end];
        assert!(daylight.contains("TZOFFSETTO:+0000\r\n"), "{daylight}");
        assert!(daylight.contains("TZNAME:GMT\r\n"), "{daylight}");

        // ...and the STANDARD block is the summer one: +01:00, IST.
        let standard_start = body.find("BEGIN:STANDARD").expect("a STANDARD block");
        let standard_block = &body[standard_start..];
        let standard_end = standard_block.find("END:STANDARD").expect("END:STANDARD");
        let standard = &standard_block[..standard_end];
        assert!(standard.contains("TZOFFSETTO:+0100\r\n"), "{standard}");
        assert!(standard.contains("TZNAME:IST\r\n"), "{standard}");
    }

    #[test]
    fn tokyo_without_dst_has_a_single_standard_observance() {
        let body = render_vtimezone(Tz::Asia__Tokyo, 2024);

        assert!(body.contains("TZID:Asia/Tokyo\r\n"), "{body}");
        assert_eq!(count_lines(&body, "BEGIN:DAYLIGHT"), 0, "{body}");
        assert_eq!(count_lines(&body, "BEGIN:STANDARD"), 1, "{body}");
        assert!(body.contains("TZOFFSETFROM:+0900\r\n"), "{body}");
        assert!(body.contains("TZOFFSETTO:+0900\r\n"), "{body}");
        assert!(body.contains("TZNAME:JST\r\n"), "{body}");
    }

    #[test]
    fn utc_is_the_trivial_single_standard_case() {
        let body = render_vtimezone(Tz::UTC, 2024);

        assert!(body.contains("TZID:UTC\r\n"), "{body}");
        assert_eq!(count_lines(&body, "BEGIN:STANDARD"), 1, "{body}");
        assert_eq!(count_lines(&body, "BEGIN:DAYLIGHT"), 0, "{body}");
        assert!(body.contains("TZOFFSETFROM:+0000\r\n"), "{body}");
        assert!(body.contains("TZOFFSETTO:+0000\r\n"), "{body}");
        assert!(body.contains("TZNAME:UTC\r\n"), "{body}");
    }

    // -- F18 Paso 6 — every zone re-parses ----------------------------------

    #[test]
    fn every_zone_renders_a_reparseable_vtimezone() {
        for &tz in chrono_tz::TZ_VARIANTS.iter() {
            let body = render_vtimezone(tz, 2024);
            assert!(body.starts_with("BEGIN:VTIMEZONE"), "{tz}: {body}");
            assert!(body.trim_end().ends_with("END:VTIMEZONE"), "{tz}: {body}");
            assert!(
                body.contains(&format!("TZID:{}\r\n", canonical_tzid(tz))),
                "{tz}: missing canonical TZID\n{body}"
            );
            // RFC 5545 requires at least one observance.
            assert!(
                count_lines(&body, "BEGIN:STANDARD") + count_lines(&body, "BEGIN:DAYLIGHT") >= 1,
                "{tz}: no observance\n{body}"
            );

            let ics = wrap_in_calendar(&body);
            ics.parse::<icalendar::Calendar>()
                .unwrap_or_else(|err| panic!("{tz}: VTIMEZONE did not re-parse: {err}\n{ics}"));
        }
    }

    // -- F18 review — regressions --------------------------------------------

    /// Regression for the window-tail bug: the sampling loop used to examine only
    /// `[lo, last_sample)`, silently dropping any transition in the final stretch
    /// `(last_sample, hi)`. With reference year 2024 the window is `[2023, 2030)`,
    /// and Morocco's Ramadan-2030 onset sits exactly there. Both zones must
    /// therefore emit 15 observances and include that onset.
    ///
    /// The pinned `chrono-tz` 0.10.4 places the transition at **2029-12-30
    /// 02:00 UTC** (epoch `1893290400`), i.e. the local onset `20291230T030000`
    /// (TZOFFSETFROM `+0100`). Some newer tzdata snapshots date it 2029-12-31;
    /// the assertion follows the tzdb this crate actually bundles.
    #[test]
    fn morocco_ramadan_2030_onset_is_not_lost() {
        for tz in [Tz::Africa__Casablanca, Tz::Africa__El_Aaiun] {
            let body = render_vtimezone(tz, 2024);
            let count = count_lines(&body, "BEGIN:DAYLIGHT") + count_lines(&body, "BEGIN:STANDARD");
            assert_eq!(count, 15, "{tz}: expected 15 observances\n{body}");
            assert!(
                body.contains("DTSTART:20291230T030000\r\n"),
                "{tz}: missing the 2029-12-30 onset\n{body}"
            );
        }
    }

    /// Regression for the spurious-observance bug: a transition that does not
    /// change the total offset (a base/dst reshuffle only) must emit nothing, so
    /// no observance may carry `TZOFFSETFROM == TZOFFSETTO`. The zones below are
    /// the ones the review found affected (`Asuncion`, `Coyhaique`,
    /// `Scoresbysund`) plus the previously exotic ones, to keep them honest.
    #[test]
    fn no_observance_declares_an_unchanged_offset() {
        for tz in [
            Tz::Africa__Casablanca,
            Tz::Africa__El_Aaiun,
            Tz::America__Asuncion,
            Tz::America__Coyhaique,
            Tz::America__Scoresbysund,
            Tz::Europe__Dublin,
        ] {
            let body = render_vtimezone(tz, 2024);
            let pairs = offset_pairs(&body);
            assert!(!pairs.is_empty(), "{tz}: expected at least one observance");
            for (from, to) in pairs {
                assert_ne!(
                    from, to,
                    "{tz}: spurious observance with equal offsets\n{body}"
                );
            }
        }
    }

    /// The 7-day fast path must reconstruct exactly the same onset set as a dense
    /// 1-day sampling, across the zones with the tightest / most irregular
    /// transition spacing.
    #[test]
    fn weekly_grid_matches_daily_grid_for_troublesome_zones() {
        let (lo, hi) = window_bounds(2024);
        for tz in [
            Tz::Africa__Casablanca,
            Tz::Africa__El_Aaiun,
            Tz::America__Asuncion,
            Tz::America__Coyhaique,
            Tz::America__Scoresbysund,
            Tz::Europe__Dublin,
            Tz::Asia__Tokyo,
            Tz::UTC,
        ] {
            let onsets = |step: i64| -> Vec<i64> {
                transitions_with_step(tz, lo, hi, step)
                    .into_iter()
                    .map(|(onset, _)| onset)
                    .collect()
            };
            assert_eq!(
                onsets(SAMPLE_STEP_SECONDS),
                onsets(SECONDS_PER_DAY),
                "{tz}: the 7-day grid diverged from the 1-day grid"
            );
        }
    }

    /// Regression for the panic vector: `format_local_datetime_ms` forwards an
    /// arbitrary millisecond value (straight from an unvalidated client `dtstart`)
    /// to `offset_at`. Extreme values must not panic and must be deterministic.
    #[test]
    fn format_local_datetime_ms_never_panics_on_extreme_ms() {
        for tz in [Tz::Europe__Madrid, Tz::America__Asuncion, Tz::UTC] {
            for ms in [i64::MAX, i64::MIN, 0] {
                let first = format_local_datetime_ms(tz, ms);
                let second = format_local_datetime_ms(tz, ms);
                assert_eq!(first, second, "{tz}@{ms}: not deterministic");
            }
        }
    }

    // -- helpers ------------------------------------------------------------

    /// Extract `(TZOFFSETFROM, TZOFFSETTO)` pairs from a rendered `VTIMEZONE`.
    fn offset_pairs(body: &str) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        let mut from: Option<String> = None;
        for line in body.lines() {
            if let Some(value) = line.strip_prefix("TZOFFSETFROM:") {
                from = Some(value.to_string());
            } else if let Some(to) = line.strip_prefix("TZOFFSETTO:") {
                let from = from
                    .take()
                    .expect("TZOFFSETTO without a preceding TZOFFSETFROM");
                pairs.push((from, to.to_string()));
            }
        }
        pairs
    }

    #[test]
    fn offset_formatting_handles_minutes_and_seconds() {
        assert_eq!(format_offset(3_600), "+0100");
        assert_eq!(format_offset(-18_000), "-0500");
        assert_eq!(format_offset(0), "+0000");
        // Second-resolution offsets appear in historical tzdb data.
        assert_eq!(format_offset(-968), "-001608");
        assert_eq!(format_offset(5_400), "+0130");
    }

    #[test]
    fn epoch_formatting_is_suffix_free() {
        // 2024-01-01T00:00:00 UTC as an epoch second.
        assert_eq!(format_epoch(1_704_067_200), "20240101T000000");
        // A pre-1970 instant must round-trip through the civil-date maths.
        assert_eq!(format_epoch(-1), "19691231T235959");
    }
}
