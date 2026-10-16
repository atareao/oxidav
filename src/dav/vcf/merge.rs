//! F22 — merge quirúrgico de propiedades sobre el `raw_payload` de una vCard.
//!
//! A diferencia de recomponer la tarjeta desde cero (lo que hacía `build_vcf` a
//! partir de los seis campos desnormalizados), este módulo sustituye **solo** las
//! líneas de las propiedades que el cliente ha editado y deja el resto del
//! payload **byte a byte** intacto: mismo terminador de línea, mismo orden,
//! mismos parámetros y las propiedades que el servidor no modela tal y como las
//! escribió el cliente.
//!
//! # Contrato operativo
//!
//! * Se trabajan **líneas lógicas** (RFC 6350 §3.2: una línea física que empieza
//!   por espacio o tab continúa la anterior) y se clasifican por el **nombre de
//!   propiedad**: lo que hay antes del primer `:`, quitando el prefijo de grupo
//!   (`item1.TEL`) y los parámetros (`;TYPE=…`), comparado **sin distinguir
//!   mayúsculas**.
//! * Se eliminan **todas** las líneas lógicas cuyo nombre coincida, incluidas sus
//!   continuaciones de folding.
//! * Las líneas nuevas se insertan **antes del último `END:VCARD` lógico** (nunca
//!   dentro de un valor que contenga ese texto, y admitiendo `end:vcard` en
//!   minúsculas). Por tanto una propiedad editada se **mueve** al final del
//!   cuerpo, justo antes del terminador; el resto de líneas conserva su posición.
//! * `edits` vacío → el payload se devuelve **idéntico**.
//! * Es **idempotente**: aplicar dos veces los mismos `edits` da el mismo
//!   resultado que aplicarlos una vez.
//! * El payload resultante sigue parseando ([`super::parse_vcf`]).
//!
//! # Renderizado de líneas
//!
//! Los helpers [`render_text_property`], [`render_structured_property`],
//! [`render_uri_property`] y [`render_list_property`] preparan las líneas de
//! contenido que llevan los [`PropertyEdit`]. Cada propiedad usa su encoding
//! (decisión 6 del plan):
//!
//! * **Texto** ([`render_text_property`]): se escapan `\`, `,` y `;`, y el salto
//!   de línea se convierte en la secuencia `\n` (RFC 6350 §3.4).
//! * **Estructurado** ([`render_structured_property`]): los componentes se unen
//!   con `;` **sin escapar**, pero cada componente se escapa por dentro como si
//!   fuera texto (RFC 6350 §3.4: `\`, `,` y salto de línea), de modo que un `;`
//!   interno tampoco rompe el campo. [`render_structured_property_preescaped`]
//!   es la variante para componentes **ya escapados** (la usa el `N`, cuyas
//!   sub-listas se unen con `,` sin escapar).
//! * **URI** ([`render_uri_property`]): el valor se emite **verbatim** (escapar
//!   una URI la corrompe).
//! * **Lista** ([`render_list_property`]): los elementos se unen con `,` sin
//!   escapar (el separador) y cada elemento se escapa por dentro.
//!
//! Las **cuatro** devuelven cadena vacía si algún valor contiene un carácter de
//! control (incluidos CR/LF) o U+2028/U+2029: la vía del merge **rechaza** esos
//! valores (decisión 8, «rechazar, no sanear»), así que un valor multilínea no
//! se acepta por aquí. No se pierde nada: el payload que escribió el cliente se
//! conserva verbatim y no se re-renderiza.
//!
//! Las líneas insertadas se **pliegan** al vuelo a 75 octetos (RFC 6350 §3.2)
//! con [`super::super::common::fold_content_line`], respetando el terminador del
//! fichero (`CRLF` o `LF`); por eso un `NOTE`/`URL` largo no emite una línea
//! física por encima de los 75 octetos (igual que hacía [`super::build_vcf`]).
//!
//! Los parámetros se emiten como `;TYPE=a,b` y `;PREF=n` (en ese orden). Se
//! validan aquí con alfabeto acotado (decisión 5): un `TYPE` fuera de `[A-Za-z0-9-]`
//! o de más de 32 caracteres, más de 8 `TYPE`, o un `PREF` fuera de `1..=100`
//! hacen que la función devuelva **cadena vacía** (rechazar, no sanear).
//!
//! # Limitación conocida: prefijos de grupo
//!
//! Al sustituir una propiedad **con prefijo de grupo** (`item1.TEL`) se
//! **descarta el grupo**: la línea de reemplazo se emite sin prefijo. El
//! `item1.X-ABLabel` que un cliente Apple asocia a ese `item1.TEL` queda así
//! **huérfano** (sin TEL al que etiquetar) pero se **conserva** byte a byte: el
//! merge nunca borra lo que no se le pidió borrar. La clasificación sigue
//! reconociendo la propiedad por su nombre sin grupo, de modo que el `TEL` sí se
//! reemplaza. Este comportamiento lo fija el test
//! [`tests::merge_matches_group_prefixed_properties`].

/// Una propiedad a sustituir en el payload: **todas** sus apariciones se
/// reemplazan por `lines` (vacío = borrar la propiedad por completo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyEdit {
    /// Nombre de la propiedad (`TEL`, `EMAIL`, `ADR`, `N`, `X-ABLabel`…).
    pub name: String,
    /// Líneas de contenido ya renderizadas (sin CRLF, sin folding).
    pub lines: Vec<String>,
}

/// Errores del merge.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MergeError {
    /// El payload no es una vCard (sin envoltura `BEGIN:VCARD`).
    #[error("el payload no es una vCard: falta BEGIN:VCARD/END:VCARD")]
    NotACard,
    /// El payload está envuelto pero no tiene un `END:VCARD` localizable.
    #[error("el payload no tiene un END:VCARD localizable")]
    MissingTerminator,
}

/// Sustituye las propiedades de `edits` dejando **intacto** el resto del payload.
///
/// # Errors
///
/// [`MergeError::NotACard`] cuando el payload no tiene envoltura, y
/// [`MergeError::MissingTerminator`] cuando está envuelto pero no hay un
/// `END:VCARD` localizable donde insertar.
pub fn merge_properties(raw: &str, edits: &[PropertyEdit]) -> Result<String, MergeError> {
    let spans = logical_spans(raw);

    // La envoltura se valida **completa y antes que nada**: sin `BEGIN:VCARD`
    // no hay vCard, así que `NotACard` gana a `MissingTerminator` aunque haya un
    // `END:VCARD`. El orden importa, además, porque `merge_properties("garbage",
    // &[])` debe ser un error y no un atajo por lista vacía.
    let has_begin = spans
        .iter()
        .any(|span| span.content.trim().eq_ignore_ascii_case("BEGIN:VCARD"));
    if !has_begin {
        return Err(MergeError::NotACard);
    }
    let Some(terminator_idx) = spans
        .iter()
        .rposition(|span| span.content.trim().eq_ignore_ascii_case("END:VCARD"))
    else {
        return Err(MergeError::MissingTerminator);
    };

    if edits.is_empty() {
        return Ok(raw.to_string());
    }

    // Nombre de propiedad normalizado: se compara sin distinguir mayúsculas y
    // sin espacios sobrantes. El `name` es entrada interna de confianza, así que
    // no se valida como si viniera del cliente; solo se normaliza. Las
    // propiedades protegidas (`UID`/`VERSION`/`BEGIN`/`END`, decisión 4) se
    // descartan: un merge nunca las toca.
    let names: Vec<String> = edits
        .iter()
        .map(|edit| edit.name.trim().to_ascii_lowercase())
        .filter(|name| !is_protected_property_name(name))
        .collect();

    // Terminador de línea para las líneas nuevas: el del `END:VCARD`, o CRLF si
    // el terminador no venía seguido de salto de línea.
    let span_terminator = &spans[terminator_idx].terminator;
    let terminator = if span_terminator.is_empty() {
        "\r\n"
    } else {
        span_terminator.as_str()
    };

    let mut out = String::with_capacity(raw.len() + 64);
    for (idx, span) in spans.iter().enumerate() {
        if idx == terminator_idx {
            for edit in edits {
                // Decisión 4: `merge_properties` es `pub`, así que en vez de
                // confiar en el llamante se **ignora** cualquier edición de una
                // propiedad protegida (`UID`/`VERSION`/`BEGIN`/`END`).
                if is_protected_property_name(&edit.name) {
                    continue;
                }
                for line in &edit.lines {
                    // Defensa en profundidad: ni siquiera bajo un nombre no
                    // protegido una línea debe **introducir** una propiedad
                    // protegida (p. ej. `name: "N"` con una línea `UID:…`). Un
                    // llamante equivocado salta aquí en desarrollo/tests.
                    debug_assert!(
                        !super::property_name(line).is_some_and(is_protected_property_name),
                        "merge_properties no debe insertar una propiedad protegida: {line}"
                    );
                    push_folded_line(line, terminator, &mut out);
                }
            }
        }
        let matches = super::property_name(&span.content).is_some_and(|found| {
            names
                .iter()
                .any(|name| found.trim().eq_ignore_ascii_case(name))
        });
        if matches {
            continue;
        }
        out.push_str(&raw[span.start..span.end]);
    }
    Ok(out)
}

/// Whether `name` (case-insensitive, trimmed) is one of the properties a merge
/// must **never** touch: the envelope (`BEGIN`/`END`), the `VERSION` and the
/// identity (`UID`) — decision 4.
fn is_protected_property_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "uid" | "version" | "begin" | "end"
    )
}

/// Inserta `line` en `out` **plegándola** a 75 octetos (RFC 6350 §3.2) sin partir
/// un carácter UTF-8, con el terminador del payload (`terminator`).
///
/// Reutiliza [`super::super::common::fold_content_line`] —la misma rutina que usa
/// [`super::build_vcf`]— pero adapta el terminador: esa función emite siempre
/// `CRLF`, mientras que el merge respeta el terminador del fichero (`CRLF` o
/// `LF`). Como los valores no pueden contener saltos de línea (los `render_*`
/// rechazan los caracteres de control), los `CRLF` de la salida plegada son solo
/// los cortes de plegado y el fin de línea, de modo que sustituirlos por el
/// terminador del fichero es seguro.
fn push_folded_line(line: &str, terminator: &str, out: &mut String) {
    if terminator == "\r\n" {
        super::super::common::fold_content_line(line, out);
    } else {
        let mut folded = String::new();
        super::super::common::fold_content_line(line, &mut folded);
        out.push_str(&folded.replace("\r\n", terminator));
    }
}

/// Renderiza una propiedad de texto (`FN`, `NICKNAME`, `TITLE`, `NOTE`,
/// `CATEGORIES`…), escapando `\`, `,` y `;` y convirtiendo el salto de línea en
/// `\n` (RFC 6350 §3.4).
///
/// Devuelve cadena vacía cuando los parámetros son inválidos **o** cuando el
/// valor contiene un carácter de control (incluidos CR/LF) o U+2028/U+2029: la
/// vía del merge **rechaza** esos valores (decisión 8, «rechazar, no sanear»), de
/// modo que un valor multilínea no se acepta por aquí. No es una pérdida de
/// datos: el payload que escribió el cliente se conserva verbatim y el merge no
/// lo re-renderiza.
#[must_use]
pub fn render_text_property(name: &str, value: &str, types: &[String], pref: Option<u8>) -> String {
    let Some(params) = render_parameters(types, pref) else {
        return String::new();
    };
    if value.chars().any(super::is_forbidden_char) {
        return String::new();
    }
    format!("{}{}:{}", name.trim(), params, escape_text(value))
}

/// Renderiza un valor estructurado con componentes separados por `;` (`N`, `ADR`,
/// `ORG`).
///
/// Los componentes se unen con `;` sin escapar; dentro de cada componente se
/// aplica el escape de texto completo (RFC 6350 §3.4: `\`, `,` y salto de
/// línea), de modo que un `;` interno tampoco rompe el campo. Devuelve cadena
/// vacía cuando los parámetros son inválidos **o** algún componente contiene un
/// carácter de control o U+2028/U+2029 (ver [`render_text_property`]).
#[must_use]
pub fn render_structured_property(
    name: &str,
    components: &[String],
    types: &[String],
    pref: Option<u8>,
) -> String {
    let Some(params) = render_parameters(types, pref) else {
        return String::new();
    };
    if components
        .iter()
        .any(|component| component.chars().any(super::is_forbidden_char))
    {
        return String::new();
    }
    let value = components
        .iter()
        .map(|component| escape_text(component))
        .collect::<Vec<_>>()
        .join(";");
    format!("{}{}:{}", name.trim(), params, value)
}

/// Renderiza una propiedad estructurada con componentes **ya escapados**.
///
/// Como [`render_structured_property`], pero los componentes se unen con `;`
/// **verbatim** (no se vuelven a escapar). Se usa para el `N`: sus componentes
/// `additional`/`prefix`/`suffix` son **listas separadas por `,`** (RFC 6350
/// §6.2.2), de modo que el separador de la lista no debe escaparse y volver a
/// escapar el componente entero lo rompería. Cada componente debe venir ya
/// pasado por [`escape_text_value`] (o por el mismo escape).
///
/// Devuelve cadena vacía cuando los parámetros son inválidos.
#[must_use]
pub fn render_structured_property_preescaped(
    name: &str,
    components: &[String],
    types: &[String],
    pref: Option<u8>,
) -> String {
    let Some(params) = render_parameters(types, pref) else {
        return String::new();
    };
    format!("{}{}:{}", name.trim(), params, components.join(";"))
}

/// Renderiza una propiedad de tipo URI (`URL`, `IMPP`, `GEO`, `TZ`, `PHOTO`).
///
/// La URI se emite **verbatim** (no se escapan `,` ni `;`); se le añaden los
/// parámetros igual que a las propiedades de texto. Devuelve cadena vacía cuando
/// los parámetros son inválidos **o** la URI contiene un carácter de control o
/// U+2028/U+2029 (ver [`render_text_property`]).
#[must_use]
pub fn render_uri_property(name: &str, value: &str, types: &[String], pref: Option<u8>) -> String {
    let Some(params) = render_parameters(types, pref) else {
        return String::new();
    };
    if value.chars().any(super::is_forbidden_char) {
        return String::new();
    }
    format!("{}{}:{}", name.trim(), params, value)
}

/// Renderiza una propiedad de **lista** (`CATEGORIES`): los elementos se unen con
/// `,` **sin escapar** (el separador de la lista), pero dentro de cada elemento se
/// aplica el escape de texto de RFC 6350 §3.4 (`\`, `,` y `;`), de modo que una
/// coma interna no se confunda con un separador. Devuelve cadena vacía cuando los
/// parámetros son inválidos **o** algún elemento contiene un carácter de control
/// o U+2028/U+2029 (ver [`render_text_property`]).
#[must_use]
pub fn render_list_property(
    name: &str,
    values: &[String],
    types: &[String],
    pref: Option<u8>,
) -> String {
    let Some(params) = render_parameters(types, pref) else {
        return String::new();
    };
    if values
        .iter()
        .any(|value| value.chars().any(super::is_forbidden_char))
    {
        return String::new();
    }
    let value = values
        .iter()
        .map(|element| escape_text(element))
        .collect::<Vec<_>>()
        .join(",");
    format!("{}{}:{}", name.trim(), params, value)
}

/// Escapa un valor de texto (RFC 6350 §3.4) para un componente estructurado.
///
/// Es la **misma** regla que aplican los `render_*` sobre cada componente; se
/// expone para construir los componentes del `N`, cuyas sub-listas
/// `additional`/`prefix`/`suffix` se escapan **elemento a elemento** y se unen
/// con `,` sin escapar (RFC 6350 §6.2.2).
#[must_use]
pub fn escape_text_value(value: &str) -> String {
    escape_text(value)
}

/// Máximo de *tokens* `TYPE` admitidos por propiedad.
const MAX_TYPES: usize = 8;
/// Longitud máxima de un *token* `TYPE`.
const MAX_TYPE_LEN: usize = 32;

/// Renderiza la parte de parámetros de una línea (`;TYPE=a,b;PREF=n`), o `None`
/// cuando algún parámetro es inválido (rechazar, no sanear).
///
/// `TYPE` debe ser una lista de a lo sumo [`MAX_TYPES`] *tokens* no vacíos, cada
/// uno de a lo sumo [`MAX_TYPE_LEN`] caracteres del alfabeto `[A-Za-z0-9-]`;
/// `PREF` debe estar en `1..=100`.
fn render_parameters(types: &[String], pref: Option<u8>) -> Option<String> {
    if types.len() > MAX_TYPES {
        return None;
    }
    for token in types {
        let valid = !token.is_empty()
            && token.len() <= MAX_TYPE_LEN
            && token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        if !valid {
            return None;
        }
    }
    if let Some(pref) = pref
        && !(1..=100).contains(&pref)
    {
        return None;
    }

    let mut out = String::new();
    if !types.is_empty() {
        out.push_str(";TYPE=");
        out.push_str(&types.join(","));
    }
    if let Some(pref) = pref {
        out.push_str(";PREF=");
        out.push_str(&pref.to_string());
    }
    Some(out)
}

/// Escapa un valor de texto según RFC 6350 §3.4: `\`, `,` y `;` ganan una barra,
/// un salto de línea (`LF`) se vuelve la secuencia `\n` y un `CR` suelto se
/// **descarta**.
///
/// Estas son exactamente las reglas de `escape_text` en [`super`] (la que usa
/// [`super::build_vcf`]): hay **una** semántica de escape de texto en la crate.
/// En la práctica los `render_*` rechazan antes cualquier carácter de control,
/// así que las ramas de `\n`/`\r` solo cubren llamadas internas.
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

/// Una línea lógica (RFC 6350 §3.2) junto con el tramo de bytes que ocupa en el
/// payload original.
///
/// Se implementa localmente en lugar de reutilizar `super::logical_lines`
/// porque el merge necesita los **rangos de bytes** (para borrar exactamente el
/// tramo de cada propiedad, plegado incluido) y el **terminador** de cada línea
/// (para respetar CRLF/LF al insertar). `super::logical_lines` devuelve
/// `Vec<String>` y descarta ambos, así que no basta. Sí se reutiliza
/// `super::property_name` para clasificar el contenido ya desplegado.
struct LogicalSpan {
    /// Contenido desplegado: las continuaciones plegadas se unen sin su espacio
    /// o tab inicial.
    content: String,
    /// Byte donde empieza la línea lógica.
    start: usize,
    /// Byte justo después del final de la línea lógica, terminadores incluidos.
    end: usize,
    /// Terminador de la **última** línea física (`"\r\n"`, `"\n"`, o vacío si el
    /// payload no termina en salto de línea).
    terminator: String,
}

/// Divide `raw` en líneas lógicas conservando sus rangos de bytes y
/// terminadores.
fn logical_spans(raw: &str) -> Vec<LogicalSpan> {
    let mut spans: Vec<LogicalSpan> = Vec::new();
    let mut offset = 0usize;
    for physical in raw.split_inclusive('\n') {
        let start = offset;
        offset += physical.len();

        let (body, terminator) = if let Some(stripped) = physical.strip_suffix("\r\n") {
            (stripped, "\r\n")
        } else if let Some(stripped) = physical.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (physical, "")
        };

        let is_continuation = body.starts_with(' ') || body.starts_with('\t');
        if is_continuation && !spans.is_empty() {
            // `get(1..)` nunca entra en pánico: el byte 1 sigue a un espacio o
            // tab ASCII, frontera de carácter por construcción.
            let rest = body.get(1..).unwrap_or("");
            if let Some(last) = spans.last_mut() {
                last.content.push_str(rest);
                last.end = offset;
                last.terminator = terminator.to_string();
            }
        } else {
            spans.push(LogicalSpan {
                content: body.to_string(),
                start,
                end: offset,
                terminator: terminator.to_string(),
            });
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Atajo: `&str` → `String`.
    fn s(value: &str) -> String {
        value.to_string()
    }

    /// Atajo: `&[&str]` → `Vec<String>`.
    fn list(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    // -----------------------------------------------------------------------
    // merge_properties
    // -----------------------------------------------------------------------

    #[test]
    fn merge_replaces_an_existing_property_byte_for_byte() {
        // La propiedad editada se elimina de su posición y se reinserta antes del
        // terminador; el resto del payload (aquí `FN` y `NOTE`) queda byte a byte
        // idéntico, en su orden original.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u@oxidav\r\nFN:Ada\r\n\
                   TEL;TYPE=home:+111\r\nNOTE:keep me\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: vec![s("TEL;TYPE=work:+222")],
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u@oxidav\r\nFN:Ada\r\n\
                        NOTE:keep me\r\nTEL;TYPE=work:+222\r\nEND:VCARD\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_deletes_a_property_when_lines_are_empty() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:111\r\n\
                   NOTE:n\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: Vec::new(),
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nNOTE:n\r\nEND:VCARD\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_deletes_every_occurrence_of_a_property() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:a\r\nTEL:b\r\n\
                   NOTE:x\r\nTEL;TYPE=cell:c\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: Vec::new(),
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nNOTE:x\r\nEND:VCARD\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_removes_folded_continuations_of_a_deleted_property() {
        // El `TEL` está partido en dos líneas físicas (la segunda empieza por
        // espacio): ambas deben desaparecer juntas.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:+34\r\n 600000000\r\n\
                   NOTE:keep\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: Vec::new(),
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nNOTE:keep\r\nEND:VCARD\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_preserves_folding_of_untouched_properties() {
        // El `NOTE` está plegado y no se toca: debe conservar sus dos líneas
        // físicas exactamente como estaban (no se despliega).
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:111\r\nNOTE:long\r\n text\r\n\
                   END:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: vec![s("TEL:222")],
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nNOTE:long\r\n text\r\n\
                        TEL:222\r\nEND:VCARD\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_inserts_before_the_last_case_insensitive_terminator() {
        // La `NOTE` contiene el texto literal `END:VCARD`; la inserción debe caer
        // antes del terminador real (aquí en minúsculas), nunca dentro del valor.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\n\
                   NOTE:see END:VCARD inside\r\nend:vcard\r\n";
        let edits = [PropertyEdit {
            name: s("EMAIL"),
            lines: vec![s("EMAIL:a@b")],
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\n\
                        NOTE:see END:VCARD inside\r\nEMAIL:a@b\r\nend:vcard\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_does_not_confuse_property_text_inside_values() {
        // Un valor que contiene el texto `TEL:` (dentro de un `NOTE`) no se
        // confunde con la propiedad `TEL` que sí se borra.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\n\
                   NOTE:call TEL: 555 or END:VCARD\r\nTEL:111\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: Vec::new(),
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\n\
                        NOTE:call TEL: 555 or END:VCARD\r\nEND:VCARD\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_matches_group_prefixed_properties() {
        // El prefijo de grupo (`item1.TEL`) se descarta al clasificar la línea.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nitem1.TEL:111\r\n\
                   item1.X-ABLabel:Home\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: Vec::new(),
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\n\
                        item1.X-ABLabel:Home\r\nEND:VCARD\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_matches_property_names_case_insensitively() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTel:111\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: Vec::new(),
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nEND:VCARD\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_with_no_edits_is_identity() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:111\r\nEND:VCARD\r\n";
        assert_eq!(merge_properties(raw, &[]), Ok(raw.to_string()));
    }

    #[test]
    fn merge_is_idempotent() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:111\r\n\
                   NOTE:n\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: vec![s("TEL:222")],
        }];

        let once = merge_properties(raw, &edits).expect("merge");
        let twice = merge_properties(&once, &edits).expect("merge");

        assert_eq!(once, twice);
    }

    #[test]
    fn merge_preserves_lf_terminator() {
        let raw = "BEGIN:VCARD\nVERSION:4.0\nUID:u\nFN:N\nTEL:111\nEND:VCARD\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: vec![s("TEL:222")],
        }];

        let expected = "BEGIN:VCARD\nVERSION:4.0\nUID:u\nFN:N\nTEL:222\nEND:VCARD\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_preserves_crlf_terminator() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:111\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: vec![s("TEL:222")],
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:222\r\nEND:VCARD\r\n";

        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_rejects_a_payload_without_envelope() {
        assert_eq!(merge_properties("garbage", &[]), Err(MergeError::NotACard));
    }

    #[test]
    fn merge_rejects_a_payload_without_terminator() {
        assert_eq!(
            merge_properties("BEGIN:VCARD\r\nVERSION:4.0\r\nFN:x\r\n", &[]),
            Err(MergeError::MissingTerminator)
        );
    }

    #[test]
    fn merge_rejects_end_without_begin() {
        // La envoltura se valida **completa**: un `END:VCARD` sin su
        // `BEGIN:VCARD` no es una vCard, así que el error es `NotACard` y no
        // `MissingTerminator` (no hay envoltura que completar).
        assert_eq!(
            merge_properties("VERSION:4.0\r\nUID:u\r\nEND:VCARD\r\n", &[]),
            Err(MergeError::NotACard)
        );
    }

    #[test]
    fn merge_result_parses_back() {
        let raw =
            "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u@oxidav\r\nFN:Ada\r\nTEL:111\r\nEND:VCARD\r\n";
        let edits = [
            PropertyEdit {
                name: s("TEL"),
                lines: vec![s("TEL:+34999888777")],
            },
            PropertyEdit {
                name: s("EMAIL"),
                lines: vec![s("EMAIL:new@example.com")],
            },
        ];

        let merged = merge_properties(raw, &edits).expect("merge");
        let parsed = super::super::parse_vcf(&merged).expect("merged payload must parse");

        assert_eq!(parsed.uid, "u@oxidav");
        assert_eq!(parsed.fn_, "Ada");
        assert_eq!(parsed.phone.as_deref(), Some("+34999888777"));
        assert_eq!(parsed.email.as_deref(), Some("new@example.com"));
    }

    // -----------------------------------------------------------------------
    // render_text_property
    // -----------------------------------------------------------------------

    #[test]
    fn render_text_property_basic() {
        assert_eq!(
            render_text_property("FN", "Ada Lovelace", &[], None),
            "FN:Ada Lovelace"
        );
    }

    #[test]
    fn render_text_property_escapes_reserved_characters() {
        // `\`, `,` y `;` ganan una barra.
        let value = "a\\b,c;d";
        assert_eq!(
            render_text_property("NOTE", value, &[], None),
            "NOTE:a\\\\b\\,c\\;d"
        );
    }

    #[test]
    fn render_text_property_rejects_control_characters() {
        // FIX-4 (decisión 8): un carácter de control (incluidos CR/LF) o un
        // separador de línea Unicode hacen que el render **rechace** (cadena
        // vacía), en vez de sanear. Un valor multilínea no se acepta por aquí.
        assert_eq!(render_text_property("NOTE", "a\x07b", &[], None), "");
        assert_eq!(render_text_property("NOTE", "a\u{2028}b", &[], None), "");
        assert_eq!(render_text_property("NOTE", "a\nb", &[], None), "");
        assert_eq!(render_text_property("NOTE", "a\rb", &[], None), "");
        assert_eq!(render_text_property("NOTE", "a\u{2029}b", &[], None), "");
    }

    #[test]
    fn render_text_property_emits_type_and_pref_parameters() {
        assert_eq!(
            render_text_property("TEL", "+34", &list(&["work", "voice"]), Some(1)),
            "TEL;TYPE=work,voice;PREF=1:+34"
        );
    }

    // -----------------------------------------------------------------------
    // render_structured_property
    // -----------------------------------------------------------------------

    #[test]
    fn render_structured_property_joins_components_with_semicolons() {
        assert_eq!(
            render_structured_property("N", &list(&["Doe", "John", "", "", ""]), &[], None),
            "N:Doe;John;;;"
        );
    }

    #[test]
    fn render_structured_property_escapes_internal_semicolons() {
        // El separador de componentes no se escapa, pero un `;` dentro de un
        // componente sí.
        assert_eq!(
            render_structured_property("ORG", &list(&["Acme;Inc", "R&D"]), &[], None),
            "ORG:Acme\\;Inc;R&D"
        );
    }

    #[test]
    fn render_structured_property_escapes_text_inside_components() {
        // RFC 6350 §3.4: dentro de cada componente se aplica el escape de texto
        // **completo** (`\`, `,` y salto de línea), no solo el `;` interno.
        assert_eq!(
            render_structured_property("N", &list(&["Doe,D.", "John"]), &[], None),
            "N:Doe\\,D.;John"
        );
        assert_eq!(
            render_structured_property("ORG", &list(&["A\\B"]), &[], None),
            "ORG:A\\\\B"
        );
    }

    #[test]
    fn render_structured_property_rejects_control_characters() {
        assert_eq!(
            render_structured_property("N", &list(&["a\x07b"]), &[], None),
            ""
        );
        assert_eq!(
            render_structured_property("N", &list(&["a\u{2028}b"]), &[], None),
            ""
        );
        assert_eq!(
            render_structured_property("N", &list(&["a\nb"]), &[], None),
            ""
        );
    }

    // -----------------------------------------------------------------------
    // render_uri_property
    // -----------------------------------------------------------------------

    #[test]
    fn render_uri_property_is_verbatim() {
        assert_eq!(
            render_uri_property("URL", "https://example.com/a,b;c", &[], None),
            "URL:https://example.com/a,b;c"
        );
    }

    #[test]
    fn render_uri_property_emits_parameters() {
        assert_eq!(
            render_uri_property("URL", "https://x/y", &list(&["work"]), Some(1)),
            "URL;TYPE=work;PREF=1:https://x/y"
        );
    }

    #[test]
    fn render_uri_property_rejects_control_and_line_separators() {
        assert_eq!(render_uri_property("URL", "https://x/\x07", &[], None), "");
        assert_eq!(
            render_uri_property("URL", "https://x/\u{2028}", &[], None),
            ""
        );
        assert_eq!(
            render_uri_property("URL", "https://x/\u{2029}", &[], None),
            ""
        );
    }

    // -----------------------------------------------------------------------
    // render_list_property
    // -----------------------------------------------------------------------

    #[test]
    fn render_list_property_joins_elements_with_commas() {
        // El separador de la lista es `,` y **no** se escapa.
        assert_eq!(
            render_list_property(
                "CATEGORIES",
                &list(&["Friends", "Family", "Work"]),
                &[],
                None
            ),
            "CATEGORIES:Friends,Family,Work"
        );
    }

    #[test]
    fn render_list_property_escapes_special_characters_inside_elements() {
        // Dentro de cada elemento se aplica el escape de texto completo (RFC 6350
        // §3.4): `\`, `,` y `;` ganan una barra. Una coma interna así no se
        // confunde con el separador.
        assert_eq!(
            render_list_property("CATEGORIES", &list(&["a,b", "c;d", "e\\f"]), &[], None),
            "CATEGORIES:a\\,b,c\\;d,e\\\\f"
        );
    }

    #[test]
    fn render_list_property_rejects_control_characters() {
        assert_eq!(
            render_list_property("CATEGORIES", &list(&["a\x07b"]), &[], None),
            ""
        );
        assert_eq!(
            render_list_property("CATEGORIES", &list(&["a\u{2028}b"]), &[], None),
            ""
        );
        assert_eq!(
            render_list_property("CATEGORIES", &list(&["a\nb"]), &[], None),
            ""
        );
    }

    #[test]
    fn render_list_property_with_a_single_element_omits_the_separator() {
        assert_eq!(
            render_list_property("CATEGORIES", &list(&["Solo"]), &[], None),
            "CATEGORIES:Solo"
        );
    }

    #[test]
    fn render_list_property_with_an_empty_list_renders_an_empty_value() {
        // Hoy una lista vacía no se rechaza: se emite la propiedad con el valor
        // vacío (`CATEGORIES:`), que es una línea sintácticamente válida que
        // vuelve a parsear como cero elementos. Se fija aquí para no romperlo por
        // accidente.
        assert_eq!(
            render_list_property("CATEGORIES", &[], &[], None),
            "CATEGORIES:"
        );
    }

    #[test]
    fn render_list_property_emits_type_and_pref_parameters() {
        assert_eq!(
            render_list_property("CATEGORIES", &list(&["Friends"]), &list(&["work"]), Some(2)),
            "CATEGORIES;TYPE=work;PREF=2:Friends"
        );
    }

    #[test]
    fn render_list_property_rejects_invalid_parameters() {
        // Mismo contrato que los otros `render_*`: parámetros inválidos → cadena
        // vacía (rechazar, no sanear).
        assert_eq!(
            render_list_property("CATEGORIES", &list(&["x"]), &list(&["a b"]), None),
            ""
        );
        assert_eq!(
            render_list_property("CATEGORIES", &list(&["x"]), &[], Some(0)),
            ""
        );
    }

    #[test]
    fn render_list_property_result_parses_back() {
        // El resultado del merge vuelve a parsear como una `CATEGORIES` de la
        // crate y produce los mismos elementos. Los `;` y `\` internos se
        // deshacen del escape correctamente.
        //
        // No se usa ningún elemento con `,` interna a propósito: el parser de la
        // crate (`vcard4` 0.7.3) **divide primero por `,`** y luego deshace el
        // escape, así que un `\,` no puede round-trip (lo partiría en dos
        // elementos). La emisión sí lo escapa, por RFC 6350 §3.4; es el parser de
        // la crate el que no lo respeta.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u@oxidav\r\nFN:N\r\nEND:VCARD\r\n";
        let line = render_list_property(
            "CATEGORIES",
            &list(&["Friends", "Work;Team", "a\\b"]),
            &[],
            None,
        );
        let edits = [PropertyEdit {
            name: s("CATEGORIES"),
            lines: vec![line],
        }];

        let merged = merge_properties(raw, &edits).expect("merge");
        let projection = super::super::project_card(&merged).expect("project");

        assert_eq!(
            projection.categories,
            vec![list(&["Friends", "Work;Team", "a\\b"])]
        );
    }

    // -----------------------------------------------------------------------
    // Validación de parámetros (decisión 5: rechazar, no sanear)
    // -----------------------------------------------------------------------

    #[test]
    fn render_rejects_invalid_type_tokens() {
        // Alfabeto acotado a `[A-Za-z0-9-]`: espacio y `:` no valen.
        assert_eq!(
            render_text_property("TEL", "x", &list(&["wo rk"]), None),
            ""
        );
        assert_eq!(render_text_property("TEL", "x", &list(&["a:b"]), None), "");
        // Más de 32 caracteres.
        assert_eq!(
            render_text_property("TEL", "x", &[s(&"a".repeat(33))], None),
            ""
        );
    }

    #[test]
    fn render_rejects_too_many_types() {
        let nine: Vec<String> = (0..9).map(|i| format!("t{i}")).collect();
        assert_eq!(render_text_property("TEL", "x", &nine, None), "");
    }

    #[test]
    fn render_rejects_invalid_pref() {
        assert_eq!(render_text_property("TEL", "x", &[], Some(0)), "");
        assert_eq!(render_text_property("TEL", "x", &[], Some(101)), "");
    }

    #[test]
    fn render_accepts_boundary_parameters() {
        let eight: Vec<String> = (0..8).map(|i| format!("t{i}")).collect();
        assert_eq!(
            render_text_property("TEL", "x", &eight, None),
            "TEL;TYPE=t0,t1,t2,t3,t4,t5,t6,t7:x"
        );
        let long = s(&"a".repeat(32));
        assert_eq!(
            render_text_property("TEL", "x", std::slice::from_ref(&long), None),
            format!("TEL;TYPE={long}:x")
        );
        assert_eq!(
            render_text_property("TEL", "x", &[], Some(100)),
            "TEL;PREF=100:x"
        );
    }

    #[test]
    fn render_rejects_invalid_parameters_for_uri_and_structured() {
        assert_eq!(
            render_uri_property("URL", "https://x", &list(&["a b"]), None),
            ""
        );
        assert_eq!(
            render_structured_property("N", &list(&["Doe"]), &[], Some(0)),
            ""
        );
    }

    // -----------------------------------------------------------------------
    // FIX-1 — render_structured_property_preescaped y escape_text_value
    // -----------------------------------------------------------------------

    #[test]
    fn render_structured_property_preescaped_does_not_reescape() {
        // Los componentes ya vienen escapados: se unen con `;` verbatim, de modo
        // que una coma (separador de sub-lista del `N`) sobrevive.
        assert_eq!(
            render_structured_property_preescaped(
                "N",
                &list(&["Lovelace", "Ada", "Byron,King", "Dr.,Prof.", "Jr.,PhD"]),
                &[],
                None
            ),
            "N:Lovelace;Ada;Byron,King;Dr.,Prof.;Jr.,PhD"
        );
    }

    #[test]
    fn escape_text_value_matches_the_render_escaping() {
        assert_eq!(escape_text_value("a\\b,c;d"), "a\\\\b\\,c\\;d");
        assert_eq!(escape_text_value("plain"), "plain");
    }

    // -----------------------------------------------------------------------
    // FIX-5 — propiedades protegidas (decisión 4)
    // -----------------------------------------------------------------------

    #[test]
    fn merge_ignores_protected_property_edits() {
        // El primitivo es `pub`: aunque un llamante pase ediciones de `UID`,
        // `VERSION`, `BEGIN` o `END`, se **ignoran** (decisión 4) y el payload no
        // cambia. El `debug_assert!` interno no salta porque estas líneas nunca
        // llegan a insertarse.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u@oxidav\r\nFN:N\r\nEND:VCARD\r\n";
        let edits = [
            PropertyEdit {
                name: s("UID"),
                lines: vec![s("UID:evil")],
            },
            PropertyEdit {
                name: s("VERSION"),
                lines: vec![s("VERSION:3.0")],
            },
            PropertyEdit {
                name: s("BEGIN"),
                lines: vec![s("BEGIN:VCARD")],
            },
            PropertyEdit {
                name: s("END"),
                lines: vec![s("END:VCARD")],
            },
        ];
        assert_eq!(merge_properties(raw, &edits), Ok(raw.to_string()));
    }

    #[test]
    fn merge_applies_normal_edits_and_ignores_protected_ones() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:1\r\nEND:VCARD\r\n";
        let edits = [
            PropertyEdit {
                name: s("UID"),
                lines: vec![s("UID:evil")],
            },
            PropertyEdit {
                name: s("TEL"),
                lines: vec![s("TEL:2")],
            },
        ];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:2\r\nEND:VCARD\r\n";
        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    // -----------------------------------------------------------------------
    // FIX-6 — plegado de las líneas insertadas
    // -----------------------------------------------------------------------

    #[test]
    fn merge_folds_long_inserted_lines() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u@oxidav\r\nFN:N\r\nEND:VCARD\r\n";
        let value = "x".repeat(300);
        let line = render_text_property("NOTE", &value, &[], None);
        assert!(line.len() > 75, "the rendered line must need folding");
        let edits = [PropertyEdit {
            name: s("NOTE"),
            lines: vec![line.clone()],
        }];

        let merged = merge_properties(raw, &edits).expect("merge");
        for physical in merged.split("\r\n") {
            assert!(
                physical.len() <= 75,
                "physical line exceeds 75 octets: {physical:?}"
            );
        }
        assert!(merged.contains("\r\n "), "the line must be folded");

        let projection = super::super::project_card(&merged).expect("project");
        assert_eq!(projection.notes, vec![value.clone()]);

        // Idempotencia: reaplicar el mismo edit re-pliega igual.
        let twice = merge_properties(&merged, &edits).expect("merge again");
        assert_eq!(merged, twice, "the merge must stay idempotent when folding");
    }

    #[test]
    fn merge_folds_with_the_lf_terminator() {
        let raw = "BEGIN:VCARD\nVERSION:4.0\nUID:u@oxidav\nFN:N\nEND:VCARD\n";
        let value = "y".repeat(300);
        let line = render_text_property("NOTE", &value, &[], None);
        let edits = [PropertyEdit {
            name: s("NOTE"),
            lines: vec![line],
        }];

        let merged = merge_properties(raw, &edits).expect("merge");
        assert!(
            !merged.contains('\r'),
            "an LF payload must stay LF: {merged:?}"
        );
        assert!(merged.contains("\n "), "the line must be folded with LF");
        for physical in merged.split('\n') {
            assert!(
                physical.len() <= 75,
                "physical line exceeds 75 octets: {physical:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // FIX-8 — terminadores y payload sin salto final
    // -----------------------------------------------------------------------

    #[test]
    fn merge_handles_a_payload_without_a_trailing_newline() {
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:1\r\nEND:VCARD";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: vec![s("TEL:2")],
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u\r\nFN:N\r\nTEL:2\r\nEND:VCARD";
        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }

    #[test]
    fn merge_respects_mixed_line_terminators() {
        // CRLF y LF mezclados en el mismo payload: cada línea conserva su
        // terminador y las nuevas se insertan con el del `END:VCARD`.
        let raw = "BEGIN:VCARD\r\nVERSION:4.0\nUID:u\nFN:N\r\nTEL:1\r\nEND:VCARD\r\n";
        let edits = [PropertyEdit {
            name: s("TEL"),
            lines: vec![s("TEL:2")],
        }];

        let expected = "BEGIN:VCARD\r\nVERSION:4.0\nUID:u\nFN:N\r\nTEL:2\r\nEND:VCARD\r\n";
        assert_eq!(merge_properties(raw, &edits), Ok(expected.to_string()));
    }
}
