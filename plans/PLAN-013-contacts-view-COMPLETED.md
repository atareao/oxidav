# PLAN-013 — F12 REST de contactos + Vista Contactos (SPA)

## Objetivo

Añadir los endpoints REST de contactos bajo `/api/v1` y la vista Contactos de la SPA, replicando el patrón ya establecido por F11 para eventos.

## Alcance

- **Backend (Rust/Axum):** nuevos endpoints REST de objetos tarjeta scopeados por colección:
  - `GET /api/v1/collections/{id}/contacts` (con `?limit=&offset=`)
  - `POST /api/v1/collections/{id}/contacts` → `201`
  - `GET /api/v1/collections/{id}/contacts/{uid}`
  - `PATCH /api/v1/collections/{id}/contacts/{uid}`
  - `DELETE /api/v1/collections/{id}/contacts/{uid}`
- **Frontend (`web/`):** vista `web/src/pages/Contacts.tsx` (sustituir el placeholder) y servicio `web/src/services/contacts.ts`.
- **Fuera de alcance:** cambios en el protocolo CardDAV (`/dav`, F08), cambios en el índice de búsqueda (F10) más allá de lo heredado, y endpoints REST de colecciones (F06, ya existentes).

## Decisiones técnicas

- **Réplica del patrón F11:** F12 replica para contactos lo que F11 hizo para eventos. F06 no expone endpoints REST de objetos tarjeta, por lo que F12 los añade, igual que F11 los añadió para eventos.
- **Scope por usuario (anti-IDOR):** todos los endpoints van bajo `require_auth` y filtran por `AuthUser.sub`. Si la colección no pertenece al usuario o no es de tipo `addressbook` → `404`.
- **Composición del `.vcf`:** el backend compone internamente el `.vcf` para `raw_payload` **reutilizando `src/dav/vcf.rs`**, sin duplicar lógica de serialización.
- **DTOs JSON sin `raw_payload`:** el cliente nunca recibe ni parsea `.vcf` crudo.
- **Validación y sanitización de entrada:** se eliminan caracteres de control y CRLF que puedan inyectar líneas en el `.vcf` generado.
- **Envelope de errores:** se reutiliza el envelope de F06 `{"error":{"code","message"}}`.
- **Frontend:** React 19 + Ant Design v6 + Vite + TS; componentes Antd, actualizaciones optimistas con `useOptimistic` y rollback, mensajería vía `App.useApp()`, y `confirmLoading` para evitar doble submit.

## Contratos / Interfaces

### Endpoints REST (bajo `require_auth`)

| Método | Ruta | Respuesta |
|---|---|---|
| GET | `/api/v1/collections/{id}/contacts?limit=&offset=` | `200` lista de contactos |
| POST | `/api/v1/collections/{id}/contacts` | `201` contacto creado |
| GET | `/api/v1/collections/{id}/contacts/{uid}` | `200` contacto |
| PATCH | `/api/v1/collections/{id}/contacts/{uid}` | `200` contacto actualizado |
| DELETE | `/api/v1/collections/{id}/contacts/{uid}` | `204` |

Reglas:
- La colección debe ser de tipo `addressbook`; en caso contrario → `404`.
- Recurso no perteneciente a `AuthUser.sub` → `404`.
- Sin autenticación válida → `401`.
- Errores con envelope `{"error":{"code","message"}}`.

### DTO de contacto (JSON, sin `raw_payload`)

```json
{
  "id": 1,
  "uid": "abc123",
  "collection_id": 7,
  "fn": "Ada Lovelace",
  "full_name": "Ada Lovelace",
  "email": "ada@example.com",
  "phone": "+34 600 000 000",
  "organization": "Analytical Engines Ltd",
  "etag": "W/\"...\"",
  "created_at": 1700000000000,
  "updated_at": 1700000000000
}
```

- `created_at` / `updated_at`: epoch en milisegundos.
- `fn` / `full_name`: nombre completo (alias admitidos).

### Frontend

- `web/src/services/contacts.ts`: cliente REST de contactos.
- `web/src/types/api.ts`: tipos TypeScript de contacto.
- `web/src/pages/Contacts.tsx`: vista con `<Table>` Antd (búsqueda/orden), `<Drawer>` de detalle, modales de alta/edición (`<Form>`), import/export `.vcf`.

## Tareas (TDD)

### Backend

- [x] **Tarea 1 — Tests anti-IDOR:** escenario con dos usuarios; el usuario B no puede leer/actualizar/borrar contactos del usuario A (esperado `404`).
- [x] **Tarea 2 — Tests CRUD scopeado:** alta (`201`), listado con `?limit=&offset=`, lectura, PATCH y DELETE, todo scopeado por `AuthUser.sub`.
- [x] **Tarea 3 — Tests validación/sanitización:** entrada con caracteres de control/CRLF se sanea y no inyecta líneas `.vcf`.
- [x] **Tarea 4 — Test colección no-addressbook:** la colección existe pero no es `addressbook` → `404`.
- [x] **Test no exponer `raw_payload`:** ninguna respuesta JSON incluye el campo `raw_payload`.
- [x] **Test envelope de error:** los fallos devuelven `{"error":{"code","message"}}`.
- [x] **Test `401`:** petición sin autenticación válida → `401`.
- [x] **Implementar** los handlers REST reutilizando `src/dav/vcf.rs` para componer `raw_payload`.

### Frontend

- [x] **Tarea 5 — Test de render:** `Contacts.tsx` renderiza la tabla y datos del servicio (Vitest + RTL).
- [x] **Tarea 6 — Test de alta:** creación mediante `user-event` sobre el formulario del modal.
- [x] **Tarea 7 — Test de estado optimista/rollback:** `useOptimistic` aplica el cambio y revierte ante error.
- [x] **Tarea 8 — Test import/export:** export del listado a `.vcf` e import de `.vcf` (POST por ficha).
- [x] **Type check:** `npx tsc -b` sin errores.
- [x] **Lint:** `npm run lint` sin errores.
- [x] **Tests:** `npx vitest run` en verde.
- [x] **Build:** `npm run build` correcto.

## Criterios de aceptación

- Backend en verde, sin romper los **323 tests** existentes.
- Frontend en verde (`vitest run`, `tsc -b`, `eslint`, `npm run build`).
- La vista Contactos es funcional: tabla, detalle, alta/edición y import/export `.vcf`.
- Ninguna respuesta REST expone `raw_payload`.
- Scope por `AuthUser.sub` verificado con tests de dos usuarios.

## Riesgos

- **Duplicación de lógica `.vcf`:** reutilizar `src/dav/vcf.rs` sin reimplementar serialización.
- **Parseo/serialización `.vcf`:** escapado y líneas plegadas pueden divergir entre import y export.
- **Import masivo:** volumen de fichas importadas y manejo de errores parciales.
- **Coherencia DTO ↔ `.vcf`:** el DTO debe reflejar fielmente lo compuesto en `raw_payload`.

## Referencias

- `plans/PLAN-001-general.md` (F12)
- `plans/PLAN-012-calendar-view-COMPLETED.md`
- `docs/F06-rest-api.md`
- `docs/F08-carddav-core.md`
- `web/README.md`
- `AGENTS.md`
