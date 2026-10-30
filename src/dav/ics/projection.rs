//! F25 — proyección de un `.ics` a las propiedades del `VEVENT` que la SPA edita.
//!
//! [`EventProjection`] es la vista tipada del **detalle** de un evento, derivada
//! en tiempo de lectura de `raw_payload` (la única fuente de verdad); nada se
//! persiste. Es el espejo de
//! [`CardProjection`](crate::dav::vcf::projection::CardProjection) para los
//! contactos (PLAN-023).
//!
//! Los valores de texto (`UID`, `SUMMARY`, `DESCRIPTION`, `LOCATION`) y los
//! tiempos se toman de [`parse_ics`] para que el detalle y la lista (las
//! columnas) **coincidan** exactamente, sin divergir en payloads raros. Las
//! propiedades que la lista no modela (`CATEGORIES`, `ATTACH`, `STATUS`,
//! `CLASS`, `TRANSP`, `DURATION`) se leen de las **líneas crudas** del primer
//! `VEVENT` de nivel `VCALENDAR`, verbatim, sin entrar nunca en componentes
//! anidados: el `SUMMARY` de un `VALARM` no es el del evento.

use super::{
    IcsError, LogicalLine, component_boundary, component_stacks, is_property_line,
    is_vcalendar_level, is_vevent_top_level, normalize_ics_text, parse_datetime_with_tzid,
    parse_ics, property_name, property_param, property_value, split_logical_lines,
};

/// Proyección de un `.ics` a las propiedades del `VEVENT` que la SPA edita.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventProjection {
    /// `UID`.
    pub uid: String,
    /// `SUMMARY`.
    pub summary: Option<String>,
    /// `DESCRIPTION`.
    pub description: Option<String>,
    /// `LOCATION`.
    pub location: Option<String>,
    /// `URL`, verbatim (un URI no se desescapa).
    pub url: Option<String>,
    /// Una entrada por **línea** `CATEGORIES`, y dentro los ítems ya separados
    /// (espejo de `CardProjection::categories`).
    pub categories: Vec<Vec<String>>,
    /// Solo el URI, una entrada por ocurrencia; los parámetros (`FMTTYPE`, …) se
    /// descartan.
    pub attach: Vec<String>,
    /// `STATUS`, verbatim.
    pub status: Option<String>,
    /// `CLASS`, verbatim.
    pub class: Option<String>,
    /// `TRANSP`, verbatim.
    pub transp: Option<String>,
    /// Texto opaco (la gramática la valida la API, no esta capa).
    pub duration: Option<String>,
    /// `DTSTART` como milisegundos Unix.
    pub dtstart_ms: i64,
    /// `DTEND` como milisegundos Unix, cuando está presente. **Nunca** se deriva
    /// de `DURATION`.
    pub dtend_ms: Option<i64>,
    /// `DTSTART`'s `TZID` canónico, cuando lo llevaba.
    pub dtstart_tzid: Option<String>,
    /// `DTEND`'s `TZID` canónico, cuando lo llevaba.
    pub dtend_tzid: Option<String>,
    /// `RRULE` como texto opaco (nunca expandida).
    pub rrule: Option<String>,
    /// Instantes (ms) de todas las líneas `EXDATE` **del subconjunto soportado**,
    /// ordenados ascendente y sin duplicados. Vacío también cuando la propiedad
    /// es opaca.
    pub exdate: Vec<i64>,
    /// Texto **verbatim** de las líneas `EXDATE` cuando la propiedad **no**
    /// encaja en el subconjunto soportado (informativo; su preservación la
    /// garantiza `reconcile_ics` al no tocar las líneas). `None` si es modelable
    /// o ausente.
    pub exdate_opaque: Option<String>,
    /// Igual que `exdate`.
    pub rdate: Vec<i64>,
    /// Igual que `exdate_opaque`.
    pub rdate_opaque: Option<String>,
}

/// Proyecta el primer `VEVENT` de un payload `.ics` sobre [`EventProjection`].
///
/// # Errores
///
/// Los mismos que [`parse_ics`] cuando el payload no sirve:
/// [`IcsError::NoEvent`] si no hay ningún `VEVENT` de nivel `VCALENDAR`,
/// [`IcsError::MissingUid`] si el evento no tiene `UID`, [`IcsError::MissingStart`]
/// si no tiene un `DTSTART` parseable, y [`IcsError::Malformed`] si el payload no
/// es un `VCALENDAR` parseable.
pub fn project_event(raw: &str) -> Result<EventProjection, IcsError> {
    let lines = split_logical_lines(raw);
    let stacks = component_stacks(&lines);

    // The first `VEVENT` directly inside a `VCALENDAR`: the same one `parse_ics`
    // and `reconcile_ics` pick. Resolved on the raw lines so the `NoEvent` error
    // does not depend on whether the crate accepts the payload.
    let target = lines
        .iter()
        .enumerate()
        .find(|(index, line)| {
            component_boundary(&line.text, "BEGIN").as_deref() == Some("VEVENT")
                && is_vcalendar_level(&stacks[*index])
        })
        .map(|(index, _)| index)
        .ok_or(IcsError::NoEvent)?;

    // The `END:VEVENT` closing the target bounds the region, so a following
    // `VEVENT` is never read.
    let target_end = (target + 1..lines.len())
        .find(|&index| {
            component_boundary(&lines[index].text, "END").as_deref() == Some("VEVENT")
                && is_vevent_top_level(&stacks[index])
        })
        .unwrap_or(lines.len());

    // Text and times: exactly what the list (columns) sees. `parse_ics` unescapes
    // the text via the crate accessors and parses the times with the same parser,
    // so the detail never diverges from the columns.
    let parsed = parse_ics(raw)?;

    // `LOCATION` is already normalised by `parse_ics`; `SUMMARY`/`DESCRIPTION`
    // are collapsed here so a blank value is `None` (as the DTO expects).
    let categories: Vec<Vec<String>> =
        top_level_values(&lines, &stacks, target, target_end, "CATEGORIES")
            .into_iter()
            .map(split_categories)
            .collect();
    let attach: Vec<String> = top_level_values(&lines, &stacks, target, target_end, "ATTACH")
        .into_iter()
        .filter(|uri| !uri.trim().is_empty())
        .map(str::to_string)
        .collect();

    let (exdate, exdate_opaque) =
        project_recur_dates(&lines, &stacks, target, target_end, "EXDATE");
    let (rdate, rdate_opaque) = project_recur_dates(&lines, &stacks, target, target_end, "RDATE");

    Ok(EventProjection {
        uid: parsed.uid,
        summary: normalize_ics_text(parsed.summary.as_deref()),
        description: normalize_ics_text(parsed.description.as_deref()),
        location: parsed.location,
        url: normalize_ics_text(first_value(&lines, &stacks, target, target_end, "URL")),
        categories,
        attach,
        status: normalize_ics_text(first_value(&lines, &stacks, target, target_end, "STATUS")),
        class: normalize_ics_text(first_value(&lines, &stacks, target, target_end, "CLASS")),
        transp: normalize_ics_text(first_value(&lines, &stacks, target, target_end, "TRANSP")),
        duration: normalize_ics_text(first_value(&lines, &stacks, target, target_end, "DURATION")),
        dtstart_ms: parsed.dtstart_ms,
        dtend_ms: parsed.dtend_ms,
        dtstart_tzid: parsed.dtstart_tzid,
        dtend_tzid: parsed.dtend_tzid,
        rrule: parsed.rrule,
        exdate,
        exdate_opaque,
        rdate,
        rdate_opaque,
    })
}

/// The values of every top-level `name` property of the target `VEVENT`, in
/// order. Nested components (`VALARM`, …) are skipped by the level check, and
/// property parameters are ignored by [`property_value`].
fn top_level_values<'a>(
    lines: &'a [LogicalLine],
    stacks: &[Vec<String>],
    target: usize,
    target_end: usize,
    name: &str,
) -> Vec<&'a str> {
    lines[target + 1..target_end]
        .iter()
        .zip(&stacks[target + 1..target_end])
        .filter(|(line, stack)| is_vevent_top_level(stack) && is_property_line(&line.text))
        .filter(|(line, _)| property_name(&line.text).eq_ignore_ascii_case(name))
        .map(|(line, _)| property_value(&line.text))
        .collect()
}

/// The value of the first top-level `name` property, or `None` when absent.
fn first_value<'a>(
    lines: &'a [LogicalLine],
    stacks: &[Vec<String>],
    target: usize,
    target_end: usize,
    name: &str,
) -> Option<&'a str> {
    top_level_values(lines, stacks, target, target_end, name)
        .into_iter()
        .next()
}

/// Split a raw `CATEGORIES` value into items, then unescape each one.
///
/// The structure *is* the separator: the value is split on the commas that are
/// **not** escaped (`\,` is a literal comma), and each item is unescaped
/// (RFC 5545 §3.3.11). An empty value yields no items.
fn split_categories(value: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        match ch {
            // Keep the escape sequence whole so a `\,` does not split here; it
            // is unescaped per item at the end.
            '\\' => {
                current.push('\\');
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ',' => items.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    items.push(current);

    items
        .into_iter()
        .filter(|item| !item.is_empty())
        .map(|item| unescape_text(&item))
        .collect()
}

/// Unescape a `CATEGORIES` item (RFC 5545 §3.3.11).
///
/// Recognises `\\`, `\,`, `\;` and `\n`/`\N`; any other `\x` resolves to `x`,
/// the tolerant behaviour of the common parsers.
fn unescape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(';') => out.push(';'),
            Some(',') => out.push(','),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Proyecta las líneas de nivel superior `EXDATE`/`RDATE` del `VEVENT` objetivo
/// al subconjunto soportado (`DATE-TIME` con `TZID` opcional).
///
/// Devuelve `(instantes, opaco)`:
/// - sin líneas, o cuando todas encajan → `(instantes ordenados y deduplicados, None)`;
/// - si **alguna** línea lleva `VALUE=DATE`/`VALUE=PERIOD` o algún token de su
///   valor no parsea → `(vec![], Some(texto))`, donde `texto` es la
///   concatenación de los textos de línea completos unidos con `"; "` (la regla
///   de conjunto: no se modela a medias).
///
/// El filtro de nivel usa el mismo criterio que [`top_level_values`], pero itera
/// el **texto completo** de cada línea para poder leer sus parámetros.
fn project_recur_dates(
    lines: &[LogicalLine],
    stacks: &[Vec<String>],
    target: usize,
    target_end: usize,
    name: &str,
) -> (Vec<i64>, Option<String>) {
    let mut instants: Vec<i64> = Vec::new();
    let mut texts: Vec<&str> = Vec::new();
    let mut any_opaque = false;

    for (line, stack) in lines[target + 1..target_end]
        .iter()
        .zip(&stacks[target + 1..target_end])
    {
        if !is_vevent_top_level(stack) || !is_property_line(&line.text) {
            continue;
        }
        if !property_name(&line.text).eq_ignore_ascii_case(name) {
            continue;
        }

        texts.push(&line.text);

        // `VALUE=DATE`/`VALUE=PERIOD` quedan fuera del subconjunto soportado.
        if let Some(value) = property_param(&line.text, "VALUE")
            && (value.eq_ignore_ascii_case("DATE") || value.eq_ignore_ascii_case("PERIOD"))
        {
            any_opaque = true;
            continue;
        }

        let tzid = property_param(&line.text, "TZID");
        let raw_value = property_value(&line.text);
        if raw_value.is_empty() {
            any_opaque = true;
            continue;
        }

        for token in raw_value.split(',') {
            match parse_datetime_with_tzid(token, tzid) {
                Some((ms, _)) => instants.push(ms),
                None => {
                    any_opaque = true;
                    break;
                }
            }
        }
    }

    if any_opaque {
        return (Vec::new(), Some(texts.join("; ")));
    }

    instants.sort_unstable();
    instants.dedup();
    (instants, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VCAL_PROLOG: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//oxidav//test//EN\r\n";
    const VCAL_EPILOG: &str = "END:VCALENDAR\r\n";

    fn calendar(body: &str) -> String {
        format!("{VCAL_PROLOG}{body}{VCAL_EPILOG}")
    }

    #[test]
    fn projects_every_new_property() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:full@oxidav\r\nSUMMARY:Evento\r\n\
             DTSTART:20240315T100000Z\r\nDTEND:20240315T110000Z\r\n\
             STATUS:CONFIRMED\r\nCLASS:PRIVATE\r\nTRANSP:TRANSPARENT\r\n\
             CATEGORIES:trabajo,personal\r\n\
             ATTACH:https://x.example/a.pdf\r\nATTACH:https://x.example/b.pdf\r\n\
             DURATION:PT1H\r\nEND:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert_eq!(projection.status.as_deref(), Some("CONFIRMED"));
        assert_eq!(projection.class.as_deref(), Some("PRIVATE"));
        assert_eq!(projection.transp.as_deref(), Some("TRANSPARENT"));
        assert_eq!(projection.duration.as_deref(), Some("PT1H"));
        assert_eq!(
            projection.categories,
            vec![vec!["trabajo".to_string(), "personal".to_string()]]
        );
        assert_eq!(
            projection.attach,
            vec![
                "https://x.example/a.pdf".to_string(),
                "https://x.example/b.pdf".to_string(),
            ]
        );
    }

    #[test]
    fn projects_categories_per_line_and_splits_escaped_commas() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:cats@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             CATEGORIES:trabajo,personal\r\nCATEGORIES:trabajo,con\\, coma\r\n\
             END:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert_eq!(
            projection.categories,
            vec![
                vec!["trabajo".to_string(), "personal".to_string()],
                vec!["trabajo".to_string(), "con, coma".to_string()],
            ]
        );
    }

    #[test]
    fn projects_attach_uri_only_dropping_parameters() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:attach@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             ATTACH;FMTTYPE=application/pdf:https://x.example/doc.pdf\r\n\
             ATTACH:https://x.example/second\r\nEND:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert_eq!(
            projection.attach,
            vec![
                "https://x.example/doc.pdf".to_string(),
                "https://x.example/second".to_string(),
            ]
        );
    }

    #[test]
    fn projects_values_verbatim() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:verbatim@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             URL:https://x.example/a,b;c\r\nCLASS:x-custom-client\r\n\
             DURATION:P1DT2H\r\nEND:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert_eq!(projection.url.as_deref(), Some("https://x.example/a,b;c"));
        assert_eq!(projection.class.as_deref(), Some("x-custom-client"));
        assert_eq!(projection.duration.as_deref(), Some("P1DT2H"));
    }

    #[test]
    fn projects_the_event_summary_not_the_alarm_one() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:alarm@oxidav\r\nSUMMARY:Evento\r\n\
             DTSTART:20240315T100000Z\r\n\
             BEGIN:VALARM\r\nACTION:DISPLAY\r\nSUMMARY:Recordatorio\r\n\
             TRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert_eq!(projection.summary.as_deref(), Some("Evento"));
    }

    #[test]
    fn projects_absent_properties_as_none_and_empty() {
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:minimal@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert_eq!(projection.status, None);
        assert_eq!(projection.class, None);
        assert_eq!(projection.transp, None);
        assert_eq!(projection.duration, None);
        assert_eq!(projection.url, None);
        assert_eq!(projection.location, None);
        assert!(projection.categories.is_empty());
        assert!(projection.attach.is_empty());
    }

    #[test]
    fn errors_without_a_vevent() {
        assert_eq!(
            project_event("BEGIN:VCALENDAREND:VCALENDAR"),
            Err(IcsError::NoEvent)
        );
    }

    // -- F27 — proyección de `EXDATE`/`RDATE` ---------------------------------

    #[test]
    fn projects_absent_recur_dates_as_empty_and_none() {
        // Escenario 1: sin `EXDATE`/`RDATE` → listas vacías y opacos `None`.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:none@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\nEND:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert!(projection.exdate.is_empty());
        assert!(projection.rdate.is_empty());
        assert_eq!(projection.exdate_opaque, None);
        assert_eq!(projection.rdate_opaque, None);
    }

    #[test]
    fn projects_modelable_exdate_with_tzid() {
        // Escenario 2: `DATE-TIME` con `TZID`, varios tokens → ordenados.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:exdate@oxidav\r\nSUMMARY:x\r\n\
             DTSTART;TZID=Europe/Madrid:20260315T100000\r\n\
             EXDATE;TZID=Europe/Madrid:20260315T100000,20260322T100000\r\n\
             END:VEVENT\r\n",
        );

        let a = parse_datetime_with_tzid("20260315T100000", Some("Europe/Madrid"))
            .expect("parse")
            .0;
        let b = parse_datetime_with_tzid("20260322T100000", Some("Europe/Madrid"))
            .expect("parse")
            .0;

        let projection = project_event(&raw).expect("project");

        assert_eq!(projection.exdate, vec![a, b]);
        assert_eq!(projection.exdate_opaque, None);
        assert!(projection.rdate.is_empty());
    }

    #[test]
    fn projects_modelable_rdate_in_utc() {
        // Escenario 3: `RDATE` UTC sin `TZID`.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:rdate@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             RDATE:20260401T090000Z\r\nEND:VEVENT\r\n",
        );

        let expected = parse_datetime_with_tzid("20260401T090000Z", None)
            .expect("parse")
            .0;

        let projection = project_event(&raw).expect("project");

        assert_eq!(projection.rdate, vec![expected]);
        assert_eq!(projection.rdate_opaque, None);
    }

    #[test]
    fn projects_opaque_exdate_value_date() {
        // Escenario 4: `VALUE=DATE` fuera del subconjunto → opaco.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:opaque@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             EXDATE;VALUE=DATE:20260315\r\nEND:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert!(projection.exdate.is_empty());
        assert_eq!(
            projection.exdate_opaque.as_deref(),
            Some("EXDATE;VALUE=DATE:20260315")
        );
    }

    #[test]
    fn projects_mixed_exdate_entirely_opaque() {
        // Escenario 5: una línea modelable y otra `VALUE=DATE` → toda opaca.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:mixed@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             EXDATE:20260315T100000Z\r\nEXDATE;VALUE=DATE:20260322\r\n\
             END:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert!(projection.exdate.is_empty());
        assert_eq!(
            projection.exdate_opaque.as_deref(),
            Some("EXDATE:20260315T100000Z; EXDATE;VALUE=DATE:20260322")
        );
    }

    #[test]
    fn projects_exdate_and_rdate_independently() {
        // Extra: `EXDATE` modelable y `RDATE` opaco a la vez.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:indep@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             EXDATE:20260315T100000Z\r\n\
             RDATE;VALUE=PERIOD:20260401T090000Z/PT1H\r\n\
             RDATE:20260402T090000Z\r\nEND:VEVENT\r\n",
        );

        let expected = parse_datetime_with_tzid("20260315T100000Z", None)
            .expect("parse")
            .0;

        let projection = project_event(&raw).expect("project");

        assert_eq!(projection.exdate, vec![expected]);
        assert_eq!(projection.exdate_opaque, None);
        assert!(projection.rdate.is_empty());
        assert_eq!(
            projection.rdate_opaque.as_deref(),
            Some("RDATE;VALUE=PERIOD:20260401T090000Z/PT1H; RDATE:20260402T090000Z")
        );
    }

    #[test]
    fn projects_dedups_instants_across_exdate_lines() {
        // n-7: el mismo instante repetido en dos líneas `EXDATE` distintas debe
        // colapsar a una sola entrada.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:dedup@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             EXDATE:20260315T100000Z\r\nEXDATE:20260315T100000Z\r\n\
             END:VEVENT\r\n",
        );
        let expected = parse_datetime_with_tzid("20260315T100000Z", None)
            .expect("parse")
            .0;

        let projection = project_event(&raw).expect("project");

        assert_eq!(projection.exdate, vec![expected]);
        assert_eq!(projection.exdate_opaque, None);
    }

    #[test]
    fn projects_rdate_from_multiple_lines_ordered() {
        // n-7: un `RDATE` repartido en varias líneas modelables aporta todas sus
        // entradas, ordenadas ascendente.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:rdate-multi@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             RDATE:20260408T090000Z\r\nRDATE:20260401T090000Z\r\n\
             END:VEVENT\r\n",
        );
        let a = parse_datetime_with_tzid("20260401T090000Z", None)
            .expect("parse")
            .0;
        let b = parse_datetime_with_tzid("20260408T090000Z", None)
            .expect("parse")
            .0;

        let projection = project_event(&raw).expect("project");

        assert_eq!(projection.rdate, vec![a, b]);
        assert_eq!(projection.rdate_opaque, None);
    }

    #[test]
    fn projects_recur_values_ignoring_nested_components() {
        // El `EXDATE` de un `VALARM` no es el del evento.
        let raw = calendar(
            "BEGIN:VEVENT\r\nUID:nested@oxidav\r\nSUMMARY:x\r\n\
             DTSTART:20240315T100000Z\r\n\
             BEGIN:VALARM\r\nACTION:DISPLAY\r\nEXDATE:20260315T100000Z\r\n\
             END:VALARM\r\nEND:VEVENT\r\n",
        );

        let projection = project_event(&raw).expect("project");

        assert!(projection.exdate.is_empty());
        assert_eq!(projection.exdate_opaque, None);
    }
}
