# F11 — REST de eventos + Vista Calendario (SPA)

## Qué hace

Añade a OxiDav la API REST de **eventos** (que F06 no cubría) y la **vista
Calendario** de la SPA (React 19 + Antd 6), para que el navegador gestione
eventos sin hablar CalDAV.

## Endpoints (backend)

| Método | Ruta | Éxito |
|---|---|---|
| GET | `/api/v1/collections/{id}/events` | `200` |
| POST | `/api/v1/collections/{id}/events` | `201` |
| GET | `/api/v1/collections/{id}/events/{uid}` | `200` |
| PATCH | `/api/v1/collections/{id}/events/{uid}` | `200` |
| DELETE | `/api/v1/collections/{id}/events/{uid}` | `204` |

- El listado admite `?start=&end=` para filtrar por **ventana temporal por
  solape** (no por contención) y `?limit=&offset=` para paginación.
- Códigos de error: `400/401/404/409`.
- Todos los handlers están bajo `require_auth`.
- La colección destino debe ser de tipo `calendar`; si no lo es → `404`.

## Aislamiento por usuario (anti-IDOR)

Todos los handlers se scopan por `AuthUser.sub`: la colección debe pertenecer al
usuario o el handler devuelve `404` (no `403`, para no filtrar existencia). Los
tests cubren este aislamiento con **dos usuarios** distintos.

## Composición del `.ics`

El backend **compone internamente** el `.ics` (`raw_payload`) a partir de los
campos JSON recibidos, reutilizando la serialización de iCalendar de F07 (evita
duplicar lógica con CalDAV). El `raw_payload` no se expone nunca en las
respuestas.

## Endurecimiento del input

- `rrule` se valida: se rechazan caracteres de control (CRLF) y un charset
  no-RECUR → `400`. Esto previene la inyección de líneas iCalendar en
  `raw_payload`.
- `summary`/`description` se escapan como texto; `RRULE`/`UID`/fechas van
  verbatim.

## Frontend

`web/src/pages/Calendar.tsx`: rejilla de calendario, carga de colecciones y
eventos por rango, modales de alta/edición, **actualizaciones optimistas con
`useOptimistic`** (con rollback en error), `App.useApp()` para mensajes (tema
oscuro), guardado con `confirmLoading` (sin doble submit) y protección de
respuestas obsoletas (`AbortController`). Servicios tipados en
`web/src/services/{api,auth,collections,events}.ts`.

## DTOs

Evento: `id`, `uid`, `collection_id`, `summary`, `description`, `dtstart`,
`dtend`, `rrule`, `etag`, `created_at`, `updated_at` (epoch ms). Nunca
`raw_payload`.

## Cómo verificar

Backend:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test` (incl. `tests/api.rs`: anti-IDOR con dos usuarios, CRUD,
  validación de `rrule`, filtro por rango, `401`)

Frontend (desde `web/`):

- `npx tsc -b`
- `npm run lint`
- `npx vitest run`
- `npm run build`

## Deuda / pendiente

- **OIDC PKCE en frontend sin implementar**: F04 fue solo backend;
  `web/src/services/auth.ts` usa un proveedor de token mínimo con `TODO`. Debe
  cerrarse (previsto en F14 Ajustes/Perfil o feature propia).
- Menores de la vista: agrupación visual de eventos multi-día, a11y fina de la
  rejilla.

## Relacionado

- [plans/PLAN-012-calendar-view-COMPLETED.md](../plans/PLAN-012-calendar-view-COMPLETED.md)
- [plans/PLAN-001-general.md](../plans/PLAN-001-general.md)
- [docs/F06-rest-api.md](./F06-rest-api.md)
- [docs/F07-caldav-core.md](./F07-caldav-core.md)
- [web/README.md](../web/README.md)
- [AGENTS.md](../AGENTS.md)
