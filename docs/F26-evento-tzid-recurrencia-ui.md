# F26 — Zona horaria (`TZID`) y recurrencia (`RRULE`) en el diálogo de eventos

Plan: `plans/PLAN-030-evento-tzid-rrule-ui.md` (cerrado como `-COMPLETED`).

## Qué entrega

Dos campos editables nuevos en el diálogo de alta/edición de la SPA: **Zona horaria** (`TZID`, F18) y **Repetición** (`RRULE`, F07). Antes la zona se auto-rellenaba en silencio (zona del navegador al crear, ancla preservada al editar) y la regla de recurrencia era invisible e inalterable.

**Solo frontend**: el backend ya validaba y persistía ambas (`validate_tzid`/`validate_rrule`); no hay cambio de API, esquema ni migración. Dos módulos **puros** nuevos: `web/src/utils/recurrence.ts` y `web/src/utils/timezone.ts`.

## Zona horaria (`TZID`)

- `Select` buscable con la lista IANA de `Intl.supportedValuesOf('timeZone')` (más `UTC`, que esa API no incluye), y el valor actual siempre presente entre las opciones.
- **Alta**: por defecto la zona del navegador; **limpiable** (vacío ⇒ se omite `tzid` ⇒ evento sin zona).
- **Edición**: precargada con `dtstart_tzid`; **no limpiable** (un `PATCH` no puede borrar la zona: `tzid` ausente conserva, `""` da `400`). Se explica con texto de ayuda.
- Cambiar la zona re-ancla `DTSTART` y `DTEND` a la nueva (comportamiento del servidor).

## Recurrencia (`RRULE`)

- **Constructor estructurado**: frecuencia (diaria/semanal/mensual/anual), intervalo («cada N»), días (solo semanal), fin (nunca / N veces / hasta una fecha) y un **resumen legible** en vivo.
- `composeRrule` emite un texto determinista dentro del alfabeto del servidor (`A-Z0-9=;,-`, ≤1024): nunca puede ser rechazado por `validate_rrule`. `UNTIL` se emite en UTC fin de día (`…T235959Z`).
- Reglas que la UI no modela (**opacas**, p. ej. `FREQ=MONTHLY;BYMONTHDAY=15`) se muestran **verbatim** y **no se reescriben**: se preservan salvo que el usuario componga una regla nueva o marque «Quitar recurrencia».

## La disciplina D2 (solo enviar lo que cambió)

- En edición, `rrule` **solo** viaja al `PATCH` si cambió respecto del baseline. La comparación es contra la **forma canónica** del baseline (`composeRrule(parseRrule(baseline))`), de modo que una regla **parseable pero no canónica** (`UNTIL=20261231`, `BYDAY=WE,MO`, `INTERVAL=1`) **no se reenvía** y el `.ics` conserva el texto original tal cual.
- Vaciar una regla parseable ⇒ se envía `""` (el servidor la elimina). Marcar «Quitar recurrencia» sobre una opaca ⇒ `""`. No tocar nada ⇒ se omite la clave y el servidor conserva.
- La **capa optimista** refleja exactamente lo enviado.

## Hallazgos de la auditoría, cerrados

1. **B1 (bloqueante)** — `ruleFromValues` no trataba los `null` de antd v6: borrar «Hasta el día» hacía `null.format(...)` (**crash** de render/handler) y borrar «Número de veces» emitía `COUNT=null` (**400**); los decimales producían `INTERVAL=2.5` (fuera de alfabeto). Ahora normaliza `null`/decimales (`normalizePositiveInt`, entero ≥1; `precision={0}` en la UI) y el fin cae a `never` si falta el valor; `composeRrule` además normaliza defensivamente (defensa en profundidad).
2. **M1 (mayor)** — la comparación D2 era textual: una regla parseable no canónica se reescribía al guardar sin tocarla (en `UNTIL=20261231` esto **cambia el instante de fin**). Ahora se compara contra la forma canónica del baseline.
3. **m1** — fin elegido sin valor: `required` en los campos y fallback a `never` (antes se componía una regla sin fin en silencio).
4. **m2** — cabeceras de test corregidas a F26.
5. **m3** — `composeRrule` refuerza su invariante de alfabeto.

## Límites aceptados

- La zona **no se puede quitar** en edición (limitación del DTO: `PATCH` no distingue ausente de `null`).
- El constructor cubre un **subconjunto** de RFC 5545 (`FREQ`/`INTERVAL`/`COUNT`/`UNTIL`/`BYDAY`). Todo lo demás se preserva verbatim, no se destruye.
- Sin indicadores de recurrencia/zona en la lista o el detalle (decisión: solo en el diálogo).

## Verificación

- Frontend: `npm run test` → **305 pasando / 0 fallando**; `npm run typecheck` sin errores; `npm run lint` sin warnings; `npm run build` OK (chunk `Calendar-*.js` con las etiquetas nuevas).
- Backend: **sin cambios** (no toca `src/` ni `web/src/types/api.ts`).
- Auditoría: `@react-reviewer` → **APROBADO** tras cerrar B1/M1/m1/m2/m3.
