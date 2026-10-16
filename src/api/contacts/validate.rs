//! Field-level validation for the contact handlers (`/api/v1/.../contacts`).
//!
//! This module owns the reject-on-control boundary and the per-field/per-group
//! caps the handlers and the vCard renderers share. Every helper is pure and
//! returns an [`ApiError`], so a rejected value is a `400` — never a `500`.

use crate::dav::vcf::is_forbidden_char;

use super::super::dto::{GenderInput, StructuredNameInput};
use super::super::error::ApiError;

/// Maximum accepted length of a contact's formatted name (`fn`), in characters.
pub(super) const MAX_FN_LEN: usize = 255;

/// Maximum accepted length of an email address, in characters.
pub(super) const MAX_EMAIL_LEN: usize = 320;

/// Maximum accepted length of a telephone number, in characters.
pub(super) const MAX_PHONE_LEN: usize = 64;

/// Maximum accepted length of an organisation name (`ORG`), in characters.
pub(super) const MAX_ORGANIZATION_LEN: usize = 255;

/// Maximum accepted length of a client-supplied `uid`, in characters.
const MAX_UID_LEN: usize = 255;

/// Maximum number of values accepted in a single multi-value group (so at most
/// this many `TEL`/`EMAIL`/`ADR`/`NOTE`/… lines per request).
const MAX_VALUES: usize = 32;

/// Maximum accepted length, in characters, of a generic free-text value
/// (nickname, title, role, name component, address component, category, `BDAY`,
/// `ANNIVERSARY`…). Chosen well above a realistic value but far below the
/// pathological 1000-character case the contract rejects.
pub(super) const MAX_TEXT_LEN: usize = 255;

/// Maximum accepted length, in characters, of a single `NOTE`.
pub(super) const MAX_NOTE_LEN: usize = 4096;

/// Maximum accepted length, in characters, of a single `URL`.
pub(super) const MAX_URL_LEN: usize = 2048;

/// Validate and normalise a required free-text field (e.g. `fn`).
///
/// The value is trimmed, must be non-blank and bounded, and must be free of
/// forbidden characters (see [`is_forbidden_char`]). Rejecting them up front is
/// a security boundary: [`build_vcf`] escapes the free-text fields, but refusing
/// them keeps an injection attempt a `400` rather than a silently sanitised
/// write.
pub(super) fn validate_required_text(
    raw: &str,
    field: &str,
    max: usize,
) -> Result<String, ApiError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` no puede estar vacío"
        )));
    }
    if value.chars().count() > max {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` no puede superar {max} caracteres"
        )));
    }
    if value.chars().any(is_forbidden_char) {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` contiene caracteres no permitidos"
        )));
    }
    Ok(value.to_string())
}

/// Normalise an optional free-text field: blank values become `None` (so `""`
/// clears the field), the surviving value is stored **trimmed**, over-long
/// values are rejected, and forbidden characters are refused (see
/// [`is_forbidden_char`]). Trimming keeps the persisted value coherent with
/// [`validate_required_text`].
pub(super) fn normalize_optional(
    raw: Option<&str>,
    field: &str,
    max: usize,
) -> Result<Option<String>, ApiError> {
    match raw {
        None => Ok(None),
        Some(raw) => {
            let value = raw.trim();
            if value.is_empty() {
                return Ok(None);
            }
            if value.chars().count() > max {
                return Err(ApiError::Validation(format!(
                    "El campo `{field}` no puede superar {max} caracteres"
                )));
            }
            if value.chars().any(is_forbidden_char) {
                return Err(ApiError::Validation(format!(
                    "El campo `{field}` contiene caracteres no permitidos"
                )));
            }
            Ok(Some(value.to_string()))
        }
    }
}

/// Validate a client-supplied `uid`: non-blank, bounded, and free of forbidden
/// characters or `/` (which would break the resource URL).
pub(super) fn validate_uid(raw: &str) -> Result<String, ApiError> {
    let uid = raw.trim();
    if uid.is_empty() {
        return Err(ApiError::Validation(
            "El `uid` no puede estar vacío".to_string(),
        ));
    }
    if uid.chars().count() > MAX_UID_LEN {
        return Err(ApiError::Validation(format!(
            "El `uid` no puede superar {MAX_UID_LEN} caracteres"
        )));
    }
    if uid.chars().any(|c| is_forbidden_char(c) || c == '/') {
        return Err(ApiError::Validation(
            "El `uid` contiene caracteres no permitidos".to_string(),
        ));
    }
    Ok(uid.to_string())
}

/// Re-validate a `uid` read back from the database before it is used to compose
/// a `.vcf`.
///
/// A stored `uid` can originate from CardDAV: [`parse_vcf`] keeps the client's
/// `UID` and the F03 repositories persist it without passing [`validate_uid`].
/// Composing a payload from such a value could inject a content line into
/// `raw_payload` (a `UID:` value must never carry CR/LF). A stored `uid` that no
/// longer satisfies [`validate_uid`] — or that requires trimming, i.e. already
/// differs from what `build_vcf` would emit — is a server-side data-integrity
/// failure, not a client error: the handler answers `500`
/// ([`ApiError::Internal`]) and leaves the corrupt row untouched rather than
/// persisting an injected payload.
///
/// # Errors
///
/// [`ApiError::Internal`] when `stored` is blank, over-long, carries a forbidden
/// character (see [`is_forbidden_char`]) or `/`, or is not already trimmed.
pub(super) fn validate_stored_uid(stored: &str) -> Result<(), ApiError> {
    match validate_uid(stored) {
        Ok(uid) if uid == stored => Ok(()),
        _ => {
            tracing::error!(
                uid = ?stored,
                "uid almacenado inválido; se rechaza recomponer el .vcf"
            );
            Err(ApiError::Internal)
        }
    }
}

/// A `None` component becomes an empty string.
pub(super) fn opt_str(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

/// Whether a structured `N` input is **totally** empty: every one of its five
/// components is absent, blank, or an empty/blank-only list.
///
/// A totally empty `N` deletes the property; any non-blank component makes it a
/// replacement (decision 7, extended to the structured fields). This is the
/// structured counterpart of the empty-array and blank-scalar deletions.
pub(super) fn name_is_empty(input: &StructuredNameInput) -> bool {
    blank_component(&input.family)
        && blank_component(&input.given)
        && blank_component_list(&input.additional)
        && blank_component_list(&input.prefix)
        && blank_component_list(&input.suffix)
}

/// Whether a `GENDER` input is **totally** empty: neither `sex` nor `identity`
/// carries a non-blank value.
///
/// A totally empty `gender` deletes the property; a partially filled one (a
/// `sex` or an `identity`, or both) is a normal replacement.
pub(super) fn gender_is_empty(input: &GenderInput) -> bool {
    is_blank(input.sex.as_deref()) && is_blank(input.identity.as_deref())
}

/// A single name component is blank when absent or empty after trim.
fn blank_component(value: &Option<String>) -> bool {
    is_blank(value.as_deref())
}

/// A name sub-list is blank when absent, empty, or holds only blank items.
fn blank_component_list(values: &Option<Vec<String>>) -> bool {
    values
        .as_ref()
        .is_none_or(|items| items.iter().all(|item| item.trim().is_empty()))
}

/// A `&str` is blank when it is absent or empty after trim.
fn is_blank(value: Option<&str>) -> bool {
    value.is_none_or(|value| value.trim().is_empty())
}

/// A `400` for a group holding more than [`MAX_VALUES`] values.
pub(super) fn enforce_count(field: &str, count: usize) -> Result<(), ApiError> {
    if count > MAX_VALUES {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` no puede tener más de {MAX_VALUES} valores"
        )));
    }
    Ok(())
}

/// Validate a free-text value: bounded and free of forbidden characters.
pub(super) fn validate_value(raw: &str, field: &str, max: usize) -> Result<String, ApiError> {
    if raw.chars().count() > max {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` no puede superar {max} caracteres"
        )));
    }
    if raw.chars().any(is_forbidden_char) {
        return Err(ApiError::Validation(format!(
            "El campo `{field}` contiene caracteres no permitidos"
        )));
    }
    Ok(raw.to_string())
}

/// A `400` for a field whose `TYPE`/`PREF` parameters are invalid.
pub(super) fn invalid_parameters(field: &str) -> ApiError {
    ApiError::Validation(format!("Los parámetros del campo `{field}` no son válidos"))
}

/// A `400` for a legacy scalar sent together with its array counterpart.
pub(super) fn scalar_array_conflict(scalar: &str, array: &str) -> ApiError {
    ApiError::Validation(format!(
        "No puede usar `{scalar}` y `{array}` en la misma petición"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_required_text_rejects_blank_and_trims() {
        assert!(matches!(
            validate_required_text("   ", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert_eq!(
            validate_required_text("  Ada  ", "fn", MAX_FN_LEN).ok(),
            Some("Ada".to_string())
        );
    }

    #[test]
    fn validate_required_text_rejects_control_characters() {
        assert!(matches!(
            validate_required_text("a\r\nb", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_required_text("a\nb", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_required_text("a\tb", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_required_text("a\u{0007}b", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_required_text_rejects_overlong_values() {
        let long = "x".repeat(MAX_FN_LEN + 1);
        assert!(matches!(
            validate_required_text(&long, "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn normalize_optional_drops_blanks_and_trims() {
        assert_eq!(normalize_optional(None, "email", 10).ok(), Some(None));
        assert_eq!(normalize_optional(Some("  "), "email", 10).ok(), Some(None));
        assert_eq!(
            normalize_optional(Some("a@b.com"), "email", 20).ok(),
            Some(Some("a@b.com".to_string()))
        );
        // The stored value is trimmed, coherent with `validate_required_text`.
        assert_eq!(
            normalize_optional(Some("  a@b.com "), "email", 20).ok(),
            Some(Some("a@b.com".to_string()))
        );
    }

    #[test]
    fn normalize_optional_rejects_control_and_overlong_values() {
        assert!(matches!(
            normalize_optional(Some("a\r\nb"), "email", 320),
            Err(ApiError::Validation(_))
        ));
        let long = "x".repeat(11);
        assert!(matches!(
            normalize_optional(Some(&long), "email", 10),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_uid_rejects_blank_and_slashes() {
        assert!(matches!(validate_uid("  "), Err(ApiError::Validation(_))));
        assert!(matches!(validate_uid("a/b"), Err(ApiError::Validation(_))));
        assert!(matches!(validate_uid("a\nb"), Err(ApiError::Validation(_))));
        assert_eq!(validate_uid(" card-1 ").ok(), Some("card-1".to_string()));
    }

    #[test]
    fn validators_reject_unicode_line_separators() {
        // U+2028 (line separator) and U+2029 (paragraph separator) are not
        // `char::is_control`, so they need an explicit refusal in every field.
        assert!(matches!(
            validate_required_text("a\u{2028}b", "fn", MAX_FN_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            normalize_optional(Some("a\u{2029}b"), "email", MAX_EMAIL_LEN),
            Err(ApiError::Validation(_))
        ));
        assert!(matches!(
            validate_uid("a\u{2028}b"),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn validate_stored_uid_accepts_only_a_canonical_uid() {
        assert!(validate_stored_uid("card-1@oxidav").is_ok());
        // A stored uid carrying a real newline (as CardDAV may persist) is a
        // server-side integrity failure, not a client error.
        assert!(matches!(
            validate_stored_uid("evil\nX-INJECTED:1"),
            Err(ApiError::Internal)
        ));
        assert!(matches!(
            validate_stored_uid("a/b"),
            Err(ApiError::Internal)
        ));
        assert!(matches!(
            validate_stored_uid("  spaced  "),
            Err(ApiError::Internal)
        ));
    }
}
