//! DTO → vCard property rendering for the contact handlers.
//!
//! This module turns a normalised [`WritableFields`] view of the request into
//! the [`PropertyEdit`] list the surgical merge consumes: it validates every
//! value (reject, never sanitise) and renders each modelled property through the
//! `render_*` helpers of [`crate::dav::vcf::merge`]. An invalid `TYPE`/`PREF`
//! parameter makes those helpers return an empty string, which is turned into a
//! `400` here.

use crate::dav::vcf::merge::{
    PropertyEdit, escape_text_value, render_list_property, render_structured_property,
    render_structured_property_preescaped, render_text_property, render_uri_property,
};

use super::super::dto::{AddressInput, GenderInput, LabelledValueInput, StructuredNameInput};
use super::super::error::ApiError;
use super::validate::{
    MAX_EMAIL_LEN, MAX_NOTE_LEN, MAX_ORGANIZATION_LEN, MAX_PHONE_LEN, MAX_TEXT_LEN, MAX_URL_LEN,
    enforce_count, gender_is_empty, invalid_parameters, name_is_empty, normalize_optional, opt_str,
    scalar_array_conflict, validate_value,
};

/// A normalised view of the writable fields shared by `POST` and `PATCH`.
///
/// Every field is optional: `None` means "absent" (leave the property alone) and
/// `Some` means "present" (replace — or delete, when a slice is empty). The
/// `fn`/`uid` fields are handled separately because their semantics differ
/// between create (required/optional) and patch (immutable `uid`).
pub(super) struct WritableFields<'a> {
    pub(super) email: Option<&'a str>,
    pub(super) emails: Option<&'a [LabelledValueInput]>,
    pub(super) phone: Option<&'a str>,
    pub(super) tels: Option<&'a [LabelledValueInput]>,
    pub(super) organization: Option<&'a str>,
    pub(super) organizations: Option<&'a [Vec<String>]>,
    pub(super) name: Option<&'a StructuredNameInput>,
    pub(super) nicknames: Option<&'a [String]>,
    pub(super) birthday: Option<&'a str>,
    pub(super) anniversary: Option<&'a str>,
    pub(super) gender: Option<&'a GenderInput>,
    pub(super) impps: Option<&'a [LabelledValueInput]>,
    pub(super) addresses: Option<&'a [AddressInput]>,
    pub(super) geos: Option<&'a [String]>,
    pub(super) timezones: Option<&'a [String]>,
    pub(super) titles: Option<&'a [String]>,
    pub(super) roles: Option<&'a [String]>,
    pub(super) notes: Option<&'a [String]>,
    pub(super) categories: Option<&'a [Vec<String>]>,
    pub(super) urls: Option<&'a [String]>,
}

/// Append a [`PropertyEdit`] for `name` (empty `lines` deletes the property).
pub(super) fn push_edit(edits: &mut Vec<PropertyEdit>, name: &str, lines: Vec<String>) {
    edits.push(PropertyEdit {
        name: name.to_string(),
        lines,
    });
}

/// Build the [`PropertyEdit`] list for every field present in `fields`.
///
/// Validation happens here (reject, never sanitise): control characters,
/// over-long values, too many values and invalid parameters all yield a `400`
/// before any write. A legacy scalar sent next to its array counterpart is also
/// a `400` (no silent precedence).
pub(super) fn build_edits(fields: &WritableFields<'_>) -> Result<Vec<PropertyEdit>, ApiError> {
    let mut edits = Vec::new();

    // EMAIL: legacy scalar vs array (mutually exclusive).
    match (fields.email, fields.emails) {
        (Some(_), Some(_)) => return Err(scalar_array_conflict("email", "emails")),
        (Some(raw), None) => {
            let value = normalize_optional(Some(raw), "email", MAX_EMAIL_LEN)?;
            push_edit(
                &mut edits,
                "EMAIL",
                scalar_text_lines("EMAIL", value.as_deref())?,
            );
        }
        (None, Some(values)) => {
            push_edit(
                &mut edits,
                "EMAIL",
                render_labelled("EMAIL", values, false)?,
            );
        }
        (None, None) => {}
    }

    // TEL.
    match (fields.phone, fields.tels) {
        (Some(_), Some(_)) => return Err(scalar_array_conflict("phone", "tels")),
        (Some(raw), None) => {
            let value = normalize_optional(Some(raw), "phone", MAX_PHONE_LEN)?;
            push_edit(
                &mut edits,
                "TEL",
                scalar_text_lines("TEL", value.as_deref())?,
            );
        }
        (None, Some(values)) => {
            push_edit(&mut edits, "TEL", render_labelled("TEL", values, false)?);
        }
        (None, None) => {}
    }

    // ORG: legacy scalar vs array (one entry per line).
    match (fields.organization, fields.organizations) {
        (Some(_), Some(_)) => return Err(scalar_array_conflict("organization", "organizations")),
        (Some(raw), None) => {
            let value = normalize_optional(Some(raw), "organization", MAX_ORGANIZATION_LEN)?;
            let lines = match value {
                Some(value) => vec![structured_line("ORG", &[value.as_str()])?],
                None => Vec::new(),
            };
            push_edit(&mut edits, "ORG", lines);
        }
        (None, Some(rows)) => {
            push_edit(
                &mut edits,
                "ORG",
                render_structured_lines("ORG", rows, MAX_TEXT_LEN)?,
            );
        }
        (None, None) => {}
    }

    if let Some(values) = fields.nicknames {
        push_edit(
            &mut edits,
            "NICKNAME",
            render_text_lines("NICKNAME", values, MAX_TEXT_LEN)?,
        );
    }
    // A present-but-totally-empty structured `N` **deletes** the property,
    // coherently with the empty arrays (`tels: []`) and the legacy scalars
    // (`email: ""`): "present replaces; replacing with nothing deletes".
    if let Some(name) = fields.name {
        let lines = if name_is_empty(name) {
            Vec::new()
        } else {
            vec![render_name(name)?]
        };
        push_edit(&mut edits, "N", lines);
    }
    // `birthday`/`anniversary` are free-text scalars: `""` (blank after trim)
    // deletes, mirroring the legacy scalar contract.
    if let Some(value) = fields.birthday {
        let lines = if value.trim().is_empty() {
            Vec::new()
        } else {
            vec![text_line("BDAY", value, MAX_TEXT_LEN)?]
        };
        push_edit(&mut edits, "BDAY", lines);
    }
    if let Some(value) = fields.anniversary {
        let lines = if value.trim().is_empty() {
            Vec::new()
        } else {
            vec![text_line("ANNIVERSARY", value, MAX_TEXT_LEN)?]
        };
        push_edit(&mut edits, "ANNIVERSARY", lines);
    }
    // A present-but-totally-empty `gender` **deletes** the property; a partially
    // filled one is a normal replacement.
    if let Some(gender) = fields.gender {
        let lines = if gender_is_empty(gender) {
            Vec::new()
        } else {
            vec![render_gender(gender)?]
        };
        push_edit(&mut edits, "GENDER", lines);
    }
    if let Some(values) = fields.impps {
        push_edit(&mut edits, "IMPP", render_labelled("IMPP", values, true)?);
    }
    if let Some(values) = fields.addresses {
        push_edit(&mut edits, "ADR", render_addresses(values)?);
    }
    if let Some(values) = fields.geos {
        push_edit(
            &mut edits,
            "GEO",
            render_uri_lines("GEO", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.timezones {
        push_edit(
            &mut edits,
            "TZ",
            render_uri_lines("TZ", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.titles {
        push_edit(
            &mut edits,
            "TITLE",
            render_text_lines("TITLE", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.roles {
        push_edit(
            &mut edits,
            "ROLE",
            render_text_lines("ROLE", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.notes {
        push_edit(
            &mut edits,
            "NOTE",
            render_text_lines("NOTE", values, MAX_NOTE_LEN)?,
        );
    }
    if let Some(values) = fields.categories {
        push_edit(
            &mut edits,
            "CATEGORIES",
            render_list_lines("CATEGORIES", values, MAX_TEXT_LEN)?,
        );
    }
    if let Some(values) = fields.urls {
        push_edit(
            &mut edits,
            "URL",
            render_uri_lines("URL", values, MAX_URL_LEN)?,
        );
    }

    Ok(edits)
}

/// Render a legacy scalar as a single line, or delete the property when cleared.
fn scalar_text_lines(name: &str, value: Option<&str>) -> Result<Vec<String>, ApiError> {
    match value {
        Some(value) => Ok(vec![text_line(name, value, MAX_TEXT_LEN)?]),
        None => Ok(Vec::new()),
    }
}

/// Render one text line, rejecting forbidden characters and over-long values.
fn text_line(name: &str, value: &str, max: usize) -> Result<String, ApiError> {
    let value = validate_value(value, name, max)?;
    let line = render_text_property(name, &value, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters(name));
    }
    Ok(line)
}

/// Render one URI line, rejecting forbidden characters and over-long values.
fn uri_line(name: &str, value: &str, max: usize) -> Result<String, ApiError> {
    let value = validate_value(value, name, max)?;
    let line = render_uri_property(name, &value, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters(name));
    }
    Ok(line)
}

/// Render a structured line (`N`, `ADR`, `ORG`), issuing no parameters.
fn structured_line(name: &str, components: &[&str]) -> Result<String, ApiError> {
    let components: Vec<String> = components
        .iter()
        .map(|component| validate_value(component, name, MAX_TEXT_LEN))
        .collect::<Result<_, _>>()?;
    let line = render_structured_property(name, &components, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters(name));
    }
    Ok(line)
}

/// Render a slice of text values (one line each), enforcing the group cap.
fn render_text_lines(name: &str, values: &[String], max: usize) -> Result<Vec<String>, ApiError> {
    enforce_count(name, values.len())?;
    values
        .iter()
        .map(|value| text_line(name, value, max))
        .collect()
}

/// Render a slice of URI values (one line each), enforcing the group cap.
fn render_uri_lines(name: &str, values: &[String], max: usize) -> Result<Vec<String>, ApiError> {
    enforce_count(name, values.len())?;
    values
        .iter()
        .map(|value| uri_line(name, value, max))
        .collect()
}

/// Render a slice of structured rows (`ORG`), enforcing the group cap.
fn render_structured_lines(
    name: &str,
    rows: &[Vec<String>],
    max: usize,
) -> Result<Vec<String>, ApiError> {
    enforce_count(name, rows.len())?;
    rows.iter()
        .map(|row| {
            enforce_count(name, row.len())?;
            let components: Vec<String> = row
                .iter()
                .map(|component| validate_value(component, name, max))
                .collect::<Result<_, _>>()?;
            let line = render_structured_property(name, &components, &[], None);
            if line.is_empty() {
                return Err(invalid_parameters(name));
            }
            Ok(line)
        })
        .collect()
}

/// Render a slice of list rows (`CATEGORIES`), enforcing the group cap.
fn render_list_lines(
    name: &str,
    rows: &[Vec<String>],
    max: usize,
) -> Result<Vec<String>, ApiError> {
    enforce_count(name, rows.len())?;
    rows.iter()
        .map(|row| {
            enforce_count(name, row.len())?;
            let values: Vec<String> = row
                .iter()
                .map(|value| validate_value(value, name, max))
                .collect::<Result<_, _>>()?;
            let line = render_list_property(name, &values, &[], None);
            if line.is_empty() {
                return Err(invalid_parameters(name));
            }
            Ok(line)
        })
        .collect()
}

/// Render a slice of labelled values (`TEL`/`EMAIL`/`IMPP`) with `TYPE`/`PREF`.
fn render_labelled(
    name: &str,
    values: &[LabelledValueInput],
    uri: bool,
) -> Result<Vec<String>, ApiError> {
    enforce_count(name, values.len())?;
    values
        .iter()
        .map(|value| {
            let validated = validate_value(&value.value, name, MAX_TEXT_LEN)?;
            let types = value.types.clone().unwrap_or_default();
            let line = if uri {
                render_uri_property(name, &validated, &types, value.pref)
            } else {
                render_text_property(name, &validated, &types, value.pref)
            };
            if line.is_empty() {
                return Err(invalid_parameters(name));
            }
            Ok(line)
        })
        .collect()
}

/// Render the structured `N` from its input.
///
/// RFC 6350 §6.2.2: the components `additional`/`prefix`/`suffix` are
/// **comma-separated lists**, so the comma is a *separator* and must **not** be
/// escaped, while each list item is escaped as text (RFC 6350 §3.4). The
/// family/given components are single values and are escaped whole.
fn render_name(input: &StructuredNameInput) -> Result<String, ApiError> {
    let family = validate_value(&opt_str(&input.family), "name.family", MAX_TEXT_LEN)?;
    let given = validate_value(&opt_str(&input.given), "name.given", MAX_TEXT_LEN)?;
    let components = vec![
        escape_text_value(&family),
        escape_text_value(&given),
        join_name_list(&input.additional, "name.additional")?,
        join_name_list(&input.prefix, "name.prefix")?,
        join_name_list(&input.suffix, "name.suffix")?,
    ];
    let line = render_structured_property_preescaped("N", &components, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters("name"));
    }
    Ok(line)
}

/// Join a comma-separated name sub-list (e.g. `additional`), validating each
/// item.
///
/// Each item is escaped with [`escape_text_value`] and the items are joined with
/// `,` **unescaped**: the comma is the list separator (RFC 6350 §6.2.2), not a
/// value character. Escaping it (as a plain structured component would) would
/// emit `Dr.\,Prof.`, which is not conformant.
fn join_name_list(values: &Option<Vec<String>>, field: &str) -> Result<String, ApiError> {
    let Some(values) = values else {
        return Ok(String::new());
    };
    enforce_count(field, values.len())?;
    let validated: Vec<String> = values
        .iter()
        .map(|value| validate_value(value, field, MAX_TEXT_LEN))
        .collect::<Result<_, _>>()?;
    Ok(validated
        .iter()
        .map(|value| escape_text_value(value))
        .collect::<Vec<_>>()
        .join(","))
}

/// Render `GENDER` (`sex;identity`), rejecting an empty or out-of-range sex.
fn render_gender(input: &GenderInput) -> Result<String, ApiError> {
    let sex = match input.sex.as_deref() {
        Some(sex) => {
            let sex = sex.trim().to_ascii_uppercase();
            if !matches!(sex.as_str(), "M" | "F" | "O" | "N" | "U") {
                return Err(ApiError::Validation(
                    "El sexo de `gender` debe ser M, F, O, N o U".to_string(),
                ));
            }
            sex
        }
        None => String::new(),
    };
    let identity = match input.identity.as_deref() {
        Some(identity) => validate_value(identity, "gender.identity", MAX_TEXT_LEN)?,
        None => String::new(),
    };
    if sex.is_empty() && identity.is_empty() {
        return Err(ApiError::Validation(
            "`gender` no puede estar vacío".to_string(),
        ));
    }
    let mut components: Vec<String> = Vec::new();
    if !sex.is_empty() {
        components.push(sex);
    } else {
        components.push(String::new());
    }
    if !identity.is_empty() {
        components.push(identity);
    }
    let line = render_structured_property("GENDER", &components, &[], None);
    if line.is_empty() {
        return Err(invalid_parameters("gender"));
    }
    Ok(line)
}

/// Render a slice of `ADR` rows, with their `TYPE`/`PREF` and optional `LABEL`.
fn render_addresses(values: &[AddressInput]) -> Result<Vec<String>, ApiError> {
    enforce_count("ADR", values.len())?;
    values
        .iter()
        .map(|input| {
            let components = vec![
                validate_value(&opt_str(&input.po_box), "address.po_box", MAX_TEXT_LEN)?,
                validate_value(&opt_str(&input.extended), "address.extended", MAX_TEXT_LEN)?,
                validate_value(&opt_str(&input.street), "address.street", MAX_TEXT_LEN)?,
                validate_value(&opt_str(&input.locality), "address.locality", MAX_TEXT_LEN)?,
                validate_value(&opt_str(&input.region), "address.region", MAX_TEXT_LEN)?,
                validate_value(
                    &opt_str(&input.postal_code),
                    "address.postal_code",
                    MAX_TEXT_LEN,
                )?,
                validate_value(&opt_str(&input.country), "address.country", MAX_TEXT_LEN)?,
            ];
            let types = input.types.clone().unwrap_or_default();
            let line = render_structured_property("ADR", &components, &types, input.pref);
            if line.is_empty() {
                return Err(invalid_parameters("ADR"));
            }
            match input.label.as_deref() {
                Some(label) => {
                    let label = validate_value(label, "address.label", MAX_TEXT_LEN)?;
                    if label.contains([';', ',', ':', '\\']) {
                        return Err(ApiError::Validation(
                            "El `LABEL` de una dirección no puede contener `;`, `,`, `:` ni `\\`"
                                .to_string(),
                        ));
                    }
                    let (name, rest) = line.split_once(':').unwrap_or((line.as_str(), ""));
                    Ok(format!("{name};LABEL={label}:{rest}"))
                }
                None => Ok(line),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::dav::vcf::project_card;

    // -----------------------------------------------------------------------
    // FIX-1 — sub-listas del `N` (separador `,` sin escapar)
    // -----------------------------------------------------------------------

    #[test]
    fn render_name_escapes_list_items_but_not_the_separator() {
        let input = StructuredNameInput {
            family: Some("Lovelace".to_string()),
            given: Some("Ada".to_string()),
            additional: Some(vec!["Byron".to_string(), "King".to_string()]),
            prefix: Some(vec!["Dr.".to_string(), "Prof.".to_string()]),
            suffix: Some(vec!["Jr.".to_string(), "PhD".to_string()]),
        };

        assert_eq!(
            render_name(&input).expect("render"),
            "N:Lovelace;Ada;Byron,King;Dr.,Prof.;Jr.,PhD"
        );
    }

    #[test]
    fn render_name_escapes_each_list_item_inside() {
        // Un carácter reservado dentro de un ítem sí se escapa (`;` -> `\;`),
        // pero el separador `,` de la sub-lista no.
        let input = StructuredNameInput {
            prefix: Some(vec!["Dr.;X".to_string(), "Prof.".to_string()]),
            ..StructuredNameInput::default()
        };

        assert_eq!(render_name(&input).expect("render"), "N:;;;Dr.\\;X,Prof.;");
    }

    #[test]
    fn render_name_sub_lists_round_trip_through_the_parser() {
        let input = StructuredNameInput {
            family: Some("Lovelace".to_string()),
            given: Some("Ada".to_string()),
            additional: Some(vec!["Byron".to_string(), "King".to_string()]),
            prefix: Some(vec!["Dr.".to_string(), "Prof.".to_string()]),
            suffix: Some(vec!["PhD".to_string()]),
        };
        let line = render_name(&input).expect("render");
        let payload = format!(
            "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u@oxidav\r\nFN:Ada\r\n{line}\r\nEND:VCARD\r\n"
        );

        let projection = project_card(&payload).expect("project");
        let name = projection.name.expect("name");
        assert_eq!(name.additional, vec!["Byron", "King"]);
        assert_eq!(name.prefix, vec!["Dr.", "Prof."]);
        assert_eq!(name.suffix, vec!["PhD"]);
    }
}
