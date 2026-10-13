# F12 — REST de contactos + Vista Contactos (SPA)

## Qué hace

Añade a OxiDav la API REST de **contactos** (que F06 no cubría) y la **vista
Contactos** de la SPA (React 19 + Antd 6), para que el navegador gestione
tarjetas sin hablar CardDAV.

## Endpoints (backend)

| Método | Ruta | Éxito |
|---|---|---|
| GET | `/api/v1/collections/{id}/contacts?limit=&offset=` | `200` (paginado con clamp) |
| POST | `/api/v1/collections/{id}/contacts` | `201` |
| GET | `/api/v1/collections/{id}/contacts/{uid}` | `200` |
| PATCH | `/api/v1/collections/{id}/contacts/{uid}` | `200` |
| DELETE | `/api/v1/collections/{id}/contacts/{uid}` | `204` |

- Códigos de error: `400/401/404/409`.
- Todos los handlers están bajo `require_auth`.
- La colección destino debe ser de tipo `addressbook`; si no lo es → `404`.

## Aislamiento por usuario (anti-IDOR)

Todos los handlers se scopan por `AuthUser.sub`: la colección debe pertenecer al
usuario o el handler devuelve `404` (no `403`, para no filtrar existencia). Los
tests cubren este aislamiento con **dos usuarios** distintos.

## Composición del `.vcf`

El backend **compone internamente** el `.vcf` (`raw_payload`) a partir de los
campos JSON recibidos, reutilizando `src/dav/vcf.rs` (evita duplicar lógica con
CardDAV). El `raw_payload` no se expone nunca en las respuestas.

## Endurecimiento del input

- Se rechazan caracteres de control/CRLF (y separadores U+2028/U+2029) en los
  campos de texto → `400`. Esto previene la inyección de líneas `.vcf`.
- Además, el `uid` **almacenado** se revalida antes de recomponer el payload en
  `PATCH` (por si vino de CardDAV sin validar).
- `uid` duplicado → `409`.

## Frontend

`web/src/pages/Contacts.tsx`: tabla Antd (`fn`/`email`/`phone`/`organization`)
con búsqueda/orden, `<Drawer>` de detalle accesible (botón "Ver" con teclado),
`<Modal>` + `<Form>`, **import/export `.vcf`** (el export pliega líneas a 75
octetos y recorrió **todas** las páginas; el import aplica límite de tamaño y
reporta Éxito/Fallo), **actualizaciones optimistas `useOptimistic`** con
rollback, `App.useApp()` para mensajes, `confirmLoading` sin doble submit y
protección de respuestas obsoletas (`AbortController`). Servicios en
`web/src/services/contacts.ts`, helpers vCard en `web/src/utils/vcf.ts`.

## DTOs

Contacto: `id`, `uid`, `collection_id`, `fn`, `email`, `phone`, `organization`,
`etag`, `created_at`, `updated_at` (epoch ms). Nunca `raw_payload`.

## Cómo verificar

Backend:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test` (~346 tests, incl. `tests/api.rs`: anti-IDOR con dos usuarios,
  CRUD, validación CRLF, `409`, paginación, `401`)

Frontend (desde `web/`):

- `npx tsc -b`
- `npm run lint`
- `npx vitest run` (26)
- `npm run build`

## Deuda / pendiente

- **OIDC PKCE en frontend sin implementar** (heredado de F11; previsto en F14 o
  feature propia).
- Menores: las fichas de import sin `FN` se descartan (documentado); separadores
  Unicode adicionales en el import.

## Relacionado

- [plans/PLAN-013-contacts-view-COMPLETED.md](../plans/PLAN-013-contacts-view-COMPLETED.md)
- [plans/PLAN-001-general.md](../plans/PLAN-001-general.md)
- [plans/PLAN-012-calendar-view-COMPLETED.md](../plans/PLAN-012-calendar-view-COMPLETED.md)
- [docs/F06-rest-api.md](./F06-rest-api.md)
- [docs/F08-carddav-core.md](./F08-carddav-core.md)
- [web/README.md](../web/README.md)
- [AGENTS.md](../AGENTS.md)
