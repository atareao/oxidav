# PLAN-012 — F11 REST de eventos + Vista Calendario (SPA)

## Objetivo

OxiDav ya cuenta con un backend Rust/Axum que expone una API REST bajo `/api/v1`
(F06: CRUD de **colecciones**, middleware `require_auth` y envelope de error
uniforme), el núcleo CalDAV en `/dav` (F07) y la búsqueda FTS5 (F10). El
frontend es **React 19 + Ant Design v6 + Vite + TypeScript** en `web/` (F02:
scaffolding, layout, rutas incluyendo `/calendar`, `ThemeContext` y proxy de
desarrollo hacia `/api` y `/dav`).

**Gap:** F06 **no** expone endpoints REST de **objetos** (eventos). La SPA no
puede hablar CalDAV directamente para renderizar, por lo que actualmente la
ruta `/calendar` es un placeholder sin datos reales.

Este plan (F11) cierra ese gap añadiendo los **endpoints REST de eventos** sobre
una colección y la **vista Calendario** funcional que los consume.

## Alcance

**Backend** — nuevos endpoints REST de eventos, anidados bajo una colección:

- `GET  /api/v1/collections/{id}/events` — listado de eventos de una colección
  (con filtro opcional por rango temporal).
- `POST /api/v1/collections/{id}/events` — alta de evento.
- `GET    /api/v1/collections/{id}/events/{uid}` — obtener un evento.
- `PATCH  /api/v1/collections/{id}/events/{uid}` — edición parcial.
- `DELETE /api/v1/collections/{id}/events/{uid}` — borrado.

Reglas transversales:

- Todos los endpoints se **scopean por `AuthUser.sub`**: un usuario solo puede
  operar sobre sus propias colecciones/objetos.
- La colección destino debe ser de tipo `calendar`; si no lo es, se rechaza.
- Las respuestas usan DTOs JSON limpios (nunca exponen `raw_payload`):
  `id`, `uid`, `collection_id`, `summary`, `description`, `dtstart`, `dtend`,
  `rrule`, `etag` y timestamps.
- Validación de entrada y **envelope de error de F06** en todos los fallos.

**Frontend** (`web/`) — vista Calendario funcional:

- `web/src/pages/Calendar.tsx` sustituye el placeholder actual.
- Rejilla de calendario (librería a decidir y justificar, p. ej.
  `@schedule-x/react` o similar).
- Carga de colecciones vía `GET /api/v1/collections` y de eventos por rango.
- Modales de alta/edición (`<Modal>`, `<Form>`, `DatePicker`).
- **Actualizaciones optimistas** con `useOptimistic` de React 19.
- Servicios en `web/src/services/` (cliente HTTP con `Authorization: Bearer`).
- Estados de carga y de error visibles para el usuario.

## Decisiones técnicas

- **Identificadores (`uid` vs `id` del objeto):** se define y documenta cuál de
  los dos se expone en la ruta. *(Decisión pendiente de fijar en este plan; ver
  Contratos.)*
- **Composición del `.ics` en backend:** al crear/editar, el backend **acepta
  campos JSON y compone internamente el `.ics`** almacenado en `raw_payload`
  (recomendado). Alternativa descartada de entrada: exigir el `.ics` completo
  desde el cliente. Debe decidirse y documentarse.
- **Librería de calendario del frontend:** elegir una rejilla compatible con
  React 19 y Ant Design v6, y justificar la elección.
- **Gestión de estado / optimistic updates:** uso de `useOptimistic` de React 19
  con rollback explícito ante error del backend.
- **Manejo de errores:** parseo del envelope de error de F06 y mapeo a
  notificaciones de Ant Design.
- **Colección por defecto:** definir el criterio cuando el usuario tiene cero o
  varias colecciones de tipo `calendar`.

## Contratos / Interfaces

**DTOs de evento**

- `EventDto` (response): `id`, `uid`, `collection_id`, `summary`,
  `description`, `dtstart`, `dtend`, `rrule`, `etag`, timestamps.
- `CreateEventRequest` / `UpdateEventRequest` (request): campos de entrada JSON;
  el backend compone el `.ics`.
- El DTO **no** incluye `raw_payload` en ningún caso.

**Endpoints y códigos de estado**

| Método | Ruta | Éxito | Errores |
|---|---|---|---|
| GET | `/api/v1/collections/{id}/events` | `200` | `400`, `401`, `404` |
| POST | `/api/v1/collections/{id}/events` | `201` | `400`, `401`, `404`, `409` |
| GET | `/api/v1/collections/{id}/events/{uid}` | `200` | `401`, `404` |
| PATCH | `/api/v1/collections/{id}/events/{uid}` | `200` | `400`, `401`, `404`, `409` |
| DELETE | `/api/v1/collections/{id}/events/{uid}` | `204` | `401`, `404` |

**Servicios y tipos TypeScript del frontend**

- `web/src/services/` — cliente HTTP con Bearer para eventos y colecciones.
- Tipos TS espejo de los DTOs de evento.
- Hook/servicio de estado optimista para alta/edición/borrado.

## Tareas (TDD)

### Backend

- [x] Tests anti-IDOR: dos usuarios distintos; garantizar que el usuario A no
      puede leer/modificar/borrar eventos de una colección del usuario B.
- [x] Tests CRUD de eventos scopeados por `AuthUser.sub`.
- [x] Tests de validación de entrada (campos inválidos → `400`).
- [x] Test de que `raw_payload` **no** se expone en ninguna respuesta.
- [x] Test del envelope de error de F06 en fallos.
- [x] Test de `401` sin autenticación.
- [x] Implementación mínima hasta poner en verde los tests anteriores.
- [x] `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.

### Frontend

- [x] Tests con Vitest + RTL de la vista Calendario: render, alta/edición con
      `user-event`, y comportamiento de estado optimista (incluido rollback).
- [x] Implementación de servicios HTTP con Bearer y tipos TS.
- [x] Implementación de la vista y modales de alta/edición.
- [x] `tsc -b`, `eslint`, `vitest run`, `npm run build`.
- [x] Compilar el frontend (`web/dist`) **antes** de `cargo build` (orden de
      build por `rust-embed`).

## Criterios de aceptación

- Backend en verde: `cargo test` + `cargo clippy -- -D warnings` +
  `cargo fmt --check`.
- Frontend en verde: `tsc -b`, `eslint`, `vitest run`, `npm run build`.
- Vista Calendario funcional: lista colecciones y eventos, y permite alta,
  edición y borrado con actualizaciones optimistas y rollback.
- En ningún momento se expone `raw_payload` por la API REST de eventos.

## Riesgos

- **Compatibilidad de la librería de calendario con React 19:** algunas
  librerías aún no declaran soporte para React 19; verificar antes de
  comprometerse con una en concreto.
- **Complejidad de las optimistic updates y rollback:** manejar correctamente
  el estado temporal y su reversión ante errores de red o del backend.
- **Coherencia DTO ↔ `.ics`:** asegurar que los campos JSON y el `.ics` generado
  se mantienen sincronizados en altas y ediciones.
- **Reutilización de la capa de datos/CalDAV:** el backend de eventos debe
  reutilizar la capa existente de F07 para evitar lógica duplicada.

## Referencias

- `plans/PLAN-001-general.md` (F11)
- `docs/F06-rest-api.md`
- `docs/F07-caldav-core.md`
- `web/README.md`
- `AGENTS.md`
