# F18 — Soporte de zonas horarias (`TZID`) extremo a extremo con `VTIMEZONE`

> El plan de origen es [`plans/PLAN-019-timezone-support-COMPLETED.md`](../plans/PLAN-019-timezone-support-COMPLETED.md).
> Levanta la limitación de zonas horarias de [F07](./F07-caldav-core.md).

## Qué implementa

Un evento puede quedar **anclado a una zona horaria IANA** de punta a punta:
el `TZID` viaja por el parseo DAV, se guarda en el esquema, determina cómo se
compone el `.ics` y se expone en la API REST. El `.ics` resultante incluye el
**bloque `VTIMEZONE` completo** derivado de la tzdb embebida en `chrono-tz`
(RFC 5545 §3.2.19), no solo el parámetro `TZID=`.

El objetivo funcional es que una recurrencia (`RRULE`) se repita en **hora
local** y no derive una hora en los cambios de horario de verano.

## El problema de partida

`calendar_objects.dtstart`/`dtend` guardan un **epoch en milisegundos UTC**, y
el evento **no recordaba** qué hora local era. Como `build_ics` emitía siempre
`…Z`, un evento a las 10:00 en `Europe/Madrid` se almacenaba sin zona: una
`RRULE` quedaba anclada a un instante UTC fijo y **derivaba una hora** al
entrar o salir el horario de verano. El protocolo no puede representar una hora
local inequívoca sin `TZID` **y** su bloque `VTIMEZONE`, así que ambos faltaban.

## El bug latente corregido

`parse_datetime_ms` recibía solo el **valor** de la propiedad, no sus
parámetros, así que `DTSTART;TZID=Europe/Madrid:20240315T100000` se leía como si
fuera UTC. En `Europe/Madrid` (CET, UTC+1) el instante real es
`1_710_493_200_000` (= 09:00 UTC), pero el parser guardaba
`1_710_496_800_000` (= 10:00 UTC): **exactamente una hora** de desfase.

El `raw_payload` se conservaba verbatim, así que **no había pérdida de datos**,
pero las consultas por ventana temporal, el orden y el DTO expuesto por REST
estaban mal por una hora. La aserción del test
`keeps_rrule_as_text_without_expanding` **consagraba el bug** (afirmaba el valor
de 10:00 UTC) y se ha corregido junto con el parser.

## Qué se implementó

| Capa | Cambio |
| --- | --- |
| Esquema | `migrations/0004_timezone.sql`: columnas **nullable** `dtstart_tzid` y `dtend_tzid` en `calendar_objects` (`ALTER TABLE … ADD COLUMN`). `NULL` = sin zona (comportamiento anterior). |
| Derivación | `src/dav/vtimezone.rs` (nuevo): deriva y renderiza el bloque `VTIMEZONE` de un `chrono_tz::Tz`. |
| Parseo / composición | `src/dav/ics.rs`: el `TZID` llega hasta el parseo de fechas; `build_ics` compone `DTSTART;TZID=…` y añade el `VTIMEZONE`. |
| Persistencia DAV | `src/dav/caldav.rs` (`put_event`): pasa el tzid parseado a `NewCalendarObject`/`CalendarObjectUpdate`. |
| API REST | `src/api/events.rs` (`validate_tzid`, `create`, `update`) y `src/api/dto.rs`: campo opcional `tzid` en las peticiones y `dtstart_tzid`/`dtend_tzid` en `EventDto`. |
| SPA | `web/src/pages/Calendar.tsx`: al **crear** envía `tzid` con la zona del navegador (`Intl.DateTimeFormat().resolvedOptions().timeZone`); al **editar** conserva el ancla existente del evento. Ver el porqué más abajo. |

**Ancla de zona en el SPA.** Al **crear**, envía `tzid` con la zona del
navegador; si el runtime no la resuelve, **omite** el campo (la API rechaza `""`
con **400**). Al **editar**, en cambio, **conserva el ancla existente del
evento** (`editing.dtstart_tzid`); si el evento **no** tiene zona, **omite**
`tzid` en lugar de inventar una. El porqué es deliberado y no obvio: como
`dtstart` es un instante fijo en ms, **re-anclar** al editar cambiaría en
silencio la **hora local de su recurrencia** (p. ej. un evento de
`Europe/Madrid` editado desde otra zona). El SPA **no tiene selector de zona**:
la zona se fija al crear (ver *Limitaciones*).

**Retrocompatibilidad:** el instante en ms sigue siendo la única fuente de
verdad. Un evento **sin** `tzid` produce una salida de `build_ics`
**byte-idéntica** a la anterior (`…Z`, sin `VTIMEZONE`), y las columnas nuevas
quedan `NULL` → **ninguna fila existente cambia**.

## Regla de resolución local→UTC (contrato)

Cuando `DTSTART`/`DTEND` lleva un `TZID` y una hora local **sin sufijo**, la
hora se resuelve a UTC según RFC 5545 §3.3.5:

| Caso (`chrono::LocalResult`) | Resolución |
| --- | --- |
| `Single` (hora inequívoca) | Ese instante. |
| `Ambiguous` (hora **repetida** en el retroceso de otoño) | **La primera**, es decir el offset en vigor **antes** de la transición. |
| `None` (hora **inexistente** en el salto de primavera) | Se interpreta con el **offset previo al hueco** (`GapInfo`). |

El porqué: es exactamente lo que recomienda RFC 5545 §3.3.5 —que fija la
primera interpretación para una hora ambigua— y evita «inventar» un offset
durante el hueco. El desplazamiento es determinista y queda documentado en el
código (`local_to_utc_ms`). Si esa resolución **falla**, la propiedad **no se
parsea** (no se degrada a UTC): `resolve_local` propaga el `None` con `?`, así
que un `DTSTART` irresoluble se considera inválido y acaba rechazando el `PUT`
por falta de inicio. En la práctica esa rama es **inalcanzable desde el DAV**,
porque el parser acota el año a cuatro dígitos (siempre representable por
`chrono`) y un `LocalResult::None` implica que `GapInfo` sí devuelve el offset
previo al hueco. La degradación a leer la hora como UTC solo ocurre cuando el
`TZID` está **ausente o no es resoluble**, como se explica antes.

## Cómo se deriva el `VTIMEZONE`

`chrono-tz` 0.10.4 **no expone sus tablas de transiciones**: los tipos internos
`TimeSpans`/`FixedTimespanSet` son privados y `Tz::timespans()` no es invocable
desde fuera del crate. Lo que **sí** es público es la implementación de
`chrono::TimeZone` para `Tz`, así que las transiciones se **reconstruyen
conductualmente**:

1. Se muestrea el offset cada **7 días** a lo largo de la ventana.
2. Allí donde dos muestras consecutivas difieren, una **búsqueda binaria**
   afina el segundo exacto del cambio (el offset es una función escalonada).

El invariante que lo hace correcto: **el paso de muestreo debe ser menor que la
separación mínima entre dos transiciones consecutivas** de cualquier zona real
en la ventana. La separación mínima **medida** en la ventana moderna
`[2023, 2030)` es de **9 días** (`America/Asuncion`, 2024-10-06 → 2024-10-15),
dejando un margen de **2 días** para el paso de 7 días. No es un margen de
semanas: **una tzdb futura con dos transiciones separadas por menos de 7 días
las perdería** silenciosamente. Por eso el paso es un parámetro que lleva esa
restricción, no una constante accidental.

La ventana es **finita y deliberada**: `año de referencia − 1 … año + 5` (siete
años). Emitir la historia completa de una zona con un registro largo produciría
cientos de observancias; acotarla cubre el evento y varias recurrencias anuales
a su alrededor. Las transiciones **fuera** de la ventana no se emiten (ver
*Limitaciones*). El año de referencia es el año **local** de `DTSTART` en la
zona ancla.

Para cada transición se emite:

- `TZOFFSETFROM` = offset anterior, `TZOFFSETTO` = offset nuevo (`±HHMM`, o
  `±HHMMSS` cuando llevan segundos).
- `DTSTART` = el onset expresado en la **hora local del `TZOFFSETFROM`** (RFC 5545
  §3.6.5), formateado `YYYYMMDDTHHMMSS` **sin** `Z`.
- `TZNAME` = abreviatura del offset nuevo, cuando existe (opcional en el RFC).

El `TZID:` del bloque coincide **exactamente** con el `TZID=` de `DTSTART`, de
modo que nunca pueden discrepar.

## Clasificación de observancias

Una observancia es `DAYLIGHT` si su `dst_offset != 0` y `STANDARD` en caso
contrario. Es una clasificación fiel al **flag de la tzdb**, no a «el offset
crece»: `Europe/Dublin` modela el DST en **negativo** (base `IST` +01:00 y el
invierno `GMT` +00:00 como un DST de −01:00), así que la regla ingenua
«verano = DAYLIGHT» etiquetaría mal ambas observancias del irlandés. Probar el
componente DST directamente clasifica bien todas las zonas, incluidas las de DST
negativo.

Se **omiten** las transiciones que **no cambian el offset total** (solo
reorganizan el desglose base/DST): no hay cambio de reloj de pared y emitir una
observancia con `TZOFFSETFROM == TZOFFSETTO` haría que algunos clientes la
rechacen. El span se arrastra igualmente, de modo que la siguiente observancia
real informa el offset verdaderamente en vigor justo antes.

Una zona sin transiciones de reloj de pared en la ventana (p. ej. `Asia/Tokyo`
o la trivial `UTC`) recibe **una sola** observancia `STANDARD` que fija el offset
constante, porque RFC 5545 exige **al menos una**.

## Degradación ante `TZID` desconocido

Un `TZID` que no sea un nombre IANA conocido **no falla el parseo**: la hora
local se trata como **UTC** y el tzid queda **nulo**. Es deliberado —mantiene la
tolerancia del `PUT` de clientes que emiten nombres no-IANA (p. ej. los de
Windows, como `W. Europe Standard Time`)— y el `raw_payload` sigue guardándose
verbatim, así que no se pierde nada. Nótese que este camino de **parseo DAV** es
distinto del **contrato REST**, donde un `tzid` desconocido sí es un `400` (ver
abajo).

## Contrato REST

- `dtstart`/`dtend` siguen siendo Unix **ms** (instante UTC) y **no cambian**.
- Campo **opcional** `tzid` (nombre IANA) en `POST` y `PATCH`.
- Un `tzid` que no sea un nombre IANA conocido → **`400`** (`ApiError::Validation`).
- Se persiste el nombre **canónico** (un alias se resuelve a su identificador
  canónico), de modo que el `TZID=` almacenado no puede discrepar del bloque.
- El REST aplica **un único `tzid`** a `DTSTART` y a `DTEND` (conveniencia del
  MVP). El DAV, en cambio, admite zonas distintas para `DTSTART` y `DTEND` y las
  expone por separado en el DTO.
- En un `PATCH`, un `tzid` **ausente** significa «no cambiar» y **no se puede
  borrar la zona** (el DTO no distingue «ausente» de `null`, la misma limitación
  de MVP que el resto de campos opcionales).
- `EventDto` gana `dtstart_tzid`/`dtend_tzid` (`Option<String>`).

Ejemplo de petición (evento semanal anclado a Madrid; `1_710_493_200_000` es
2024-03-15 09:00 UTC = 10:00 en Madrid):

```http
POST /api/v1/collections/{id}/events
Content-Type: application/json

{
  "summary": "Reunión",
  "dtstart": 1710493200000,
  "dtend": 1710496800000,
  "tzid": "Europe/Madrid",
  "rrule": "FREQ=WEEKLY;COUNT=10"
}
```

El `.ics` compuesto llevará `DTSTART;TZID=Europe/Madrid:20240315T100000` y su
bloque `VTIMEZONE`, mientras `dtstart` sigue siendo el instante UTC.

### Rango de instantes aceptado

`dtstart`/`dtend` son `i64` arbitrarios que llegan del cliente. La API los acota
a los años **1..9999** (400 fuera de rango): un valor como `i64::MAX` no es
representable como fecha iCalendar ni proyectable a una zona, así que se rechaza
antes de llegar a la composición. El límite se deriva del propio parser de
`.ics`, de modo que ambas capas aceptan exactamente los mismos instantes.

## Limitaciones / fuera de alcance de F18

- **No se acepta hora local de pared (`dtstart_local`) en la entrada REST**: el
  instante en ms sigue siendo la única fuente de verdad.
- **El SPA no tiene selector de zona**: la zona se fija al crear (la del
  navegador) y se **conserva** al editar; no se puede elegir ni cambiar la zona
  desde la interfaz. Limitación de MVP.
- **No se expanden `RRULE`**: se guardan como texto opaco.
- **Un `PATCH` REST recomponiendo el `.ics` no preserva propiedades no
  modeladas** del original (`ATTENDEE`, `VALARM`, propiedades `X-`). Es una
  **limitación preexistente**, documentada pero no arreglada en F18.
- **Ventana finita del `VTIMEZONE`** (`año − 1 … año + 5`): un cliente que
  expanda una recurrencia mucho más allá de `año + 5` puede calcular un offset
  que la zona ya no cambia en el instante exacto.
- **Invariante del muestreo a 7 días**: una tzdb futura con dos transiciones
  separadas por menos de 7 días se reconstruiría mal (ver arriba).

## Cómo verificar

```bash
# Backend
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked

# Frontend (desde web/)
npx tsc -b
npm run lint
npx vitest run
```

Los tests cubren: el parseo correcto de `DTSTART;TZID=Europe/Madrid:…`
(→ `1_710_493_200_000`), las horas ambigua e inexistente de DST, el render del
`VTIMEZONE` (con `DTSTART` sin `Z`), la clasificación por zona
(`Europe/Dublin`, `Asia/Tokyo`, `UTC`), la regresión byte a byte sin tzid, la
validación REST (`Mars/Olympus` → `400`) y la integración REST/DAV con `tzid`.
Un *sanity* renderiza el `VTIMEZONE` de **todas** las `TZ_VARIANTS` y comprueba
que se vuelve a parsear.

## Relacionado

- [`plans/PLAN-019-timezone-support-COMPLETED.md`](../plans/PLAN-019-timezone-support-COMPLETED.md)
- [`docs/F07-caldav-core.md`](./F07-caldav-core.md) — limitación de zonas horarias que F18 levanta
- [`docs/F06-rest-api.md`](./F06-rest-api.md)
- [`docs/F03-data-layer.md`](./F03-data-layer.md)
- [`AGENTS.md`](../AGENTS.md) — restricción del binario musl estático sin OpenSSL
