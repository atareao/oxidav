# PLAN-001 — Roadmap General OxiDav

## Objetivo

Construir un servidor personal CalDAV/CardDAV/WebDAV en Rust, distribuido como un único binario con una SPA de gestión embebida. El MVP debe permitir a un usuario autenticado con Pocket ID (OIDC) gestionar sus colecciones de calendario y contactos, sincronizarlas con clientes DAV nativos (iOS, DAVx⁵, Thunderbird) mediante app tokens, y buscarlas con SQLite FTS5. El resultado es un binario autocontenido (`oxidav`) desplegable en Docker, sin dependencias de servicios externos más allá del IdP.

## Alcance (MVP)

- Backend Axum + Tokio (edition 2024) y frontend React 19 + Ant Design v6 + Vite + TS en `web/`.
- Persistencia local en SQLite (WAL) con FTS5 `trigram` + ranking `bm25`; sin índice externo.
- Autenticación OIDC con Pocket ID para la SPA (Authorization Code + PKCE → Bearer JWT contra JWKS).
- App tokens revocables para clientes DAV nativos (Basic Auth, hash local en SQLite).
- API REST `/api/v1/...` con DTOs limpios (collections, CRUD, paginación, búsqueda).
- Endpoints DAV en `/dav/...`: CalDAV core, CardDAV core y WebDAV sync-collection (RFC 6578).
- SPA con vistas de Calendario, Contactos, Sincronización/Dispositivos y Ajustes/Perfil.
- Empaquetado en un binario único con assets embebidos (`rust-embed`) y Dockerfile multi-stage.

## No-objetivos (fuera del MVP)

Todo lo listado aquí es **posterior al MVP** y queda explícitamente fuera de este roadmap:

- Compartir calendarios o libretas de direcciones entre usuarios.
- Notificaciones push (Web Push, APNs, etc.).
- Grupos y permisos colectivos.
- Avatares y gestión de imágenes de perfil.
- IMAP/SMTP y cualquier integración de correo.
- Multi-tenant (múltiples organizaciones/dominios aislados).
- Delegación de calendarios (invitaciones, scheduling, iTIP/iMIP).
- WebUI de administración (gestión de usuarios, cuotas, auditoría).
- Búsqueda externa o motores de índice dedicados.
- Soporte de OAuth/IdP alternativos a Pocket ID.

## Hitos (features)

| ID | Feature | Capa | Plan | Estado |
|----|---------|------|------|--------|
| F01 | Scaffolding backend (Cargo deps: axum, tokio, sqlx/rusqlite bundled, quick-xml, tower, tracing; config por env; servidor Axum + /healthz) | backend | PLAN-002 | Completado |
| F02 | Scaffolding frontend (Vite + React 19 + Antd 6 + TS; layout base; rutas) | frontend | PLAN-003 | Completado |
| F03 | Capa de datos (pool SQLite, PRAGMAs, migraciones, esquema collections/calendar_objects/card_objects, FTS5 trigram + triggers) | backend/data | PLAN-004 | Completado |
| F04 | Autenticación OIDC Pocket ID (validación JWT contra JWKS, middleware Bearer para /api/v1) | backend/auth | PLAN-005 | Completado |
| F05 | App tokens + Basic Auth DAV (CRUD tokens revocables, hash en SQLite, middleware Basic para /dav) | backend/auth | PLAN-006 | Completado |
| F06 | API REST base /api/v1 (DTOs, CRUD de collections, paginación, errores) | backend/api | PLAN-007 | Completado |
| F07 | CalDAV core (MKCALENDAR, PROPFIND, PUT/DELETE .ics, REPORT calendar-query, parseo .ics) | backend/dav | PLAN-008 | Completado |
| F08 | CardDAV core (crear addressbook, PROPFIND, PUT/DELETE .vcf, REPORT addressbook-query, parseo .vcf) | backend/dav | PLAN-009 | Completado |
| F09 | WebDAV sync-collection RFC 6578 (sync-token incremental) | backend/dav | PLAN-010 | Completado |
| F10 | API de búsqueda FTS5 (bm25, /api/v1/search para eventos y contactos) | backend | PLAN-011 | Completado |
| F11 | Vista Calendario SPA (rejilla, modales alta/edición, useOptimistic) | frontend | PLAN-012 | Completado |
| F12 | Vista Contactos SPA (tabla/drawer, import/export .vcf) | frontend | PLAN-013 | Completado |
| F13 | Vista Sincronización y Dispositivos SPA (URLs DAV, gestor app tokens) | frontend | PLAN-014 | Completado |
| F14 | Vista Ajustes y Perfil SPA (perfil Pocket ID, tema claro/oscuro, gestión colecciones) | frontend | PLAN-015 | Completado |
| F15 | Empaquetado y despliegue (rust-embed, orden de build, Dockerfile multi-stage, verificación) | infra | PLAN-016 | Completado |
| F16 | Pulido de entrega (publicación en GHCR, code-splitting, licencia MIT, redirect post-login) | infra/frontend | PLAN-017 | Completado |

## Orden de ejecución y dependencias

El grafo de dependencias es el siguiente:

- **F01** es la raíz: sin el esqueleto Axum + config + `/healthz` no arranca nada. Bloquea F03, F04 y F05.
- **F02** es independiente en su capa: puede ejecutarse en paralelo con F01 (solo comparte el contrato de rutas y el build en `web/dist`).
- **F03** depende de F01 (necesita el runtime y el pool). Bloquea F06, F07, F08, F09 y F10, que necesitan esquema, PRAGMAs y FTS5.
- **F04** y **F05** dependen de F01 (y F05 de F03 para persistir el hash de los app tokens). Son paralelizables entre sí.
- **F06, F07, F08, F09 y F10** dependen de F03; F06/F07/F08/F09 dependen además de F04/F05 para los middlewares de auth de `/api/v1` y `/dav`. F10 depende de F03 (FTS5) y de F06 (routing/DTOs).
- **F06** puede ir en paralelo con F07–F09 una vez listo F03; F10 requiere F03.
- **F07, F08 y F09** son paralelizables entre sí tras F03 + F05. F09 (sync-collection) reutiliza la base de F07/F08, por lo que conviene cerrarlo después de F07 y F08.
- **F11–F15** dependen de que el backend exponga los contratos correspondientes: F11↔F06/F07/F10, F12↔F06/F08/F10, F13↔F05, F14↔F06/F04.
- **F15** es el cierre: depende de F02 (build de `web/dist`) y de todo el backend, por el orden de build de `rust-embed`.
- **F16** es pulido posterior al MVP (publicación en GHCR, code-splitting, licencia MIT y redirect post-login): no forma parte del cierre funcional del MVP.

**Candidate a paralelizar:** F01 ∥ F02; F04 ∥ F05; F07 ∥ F08; F11 ∥ F12 ∥ F13 ∥ F14 (una vez listos sus contratos de backend).

## Riesgos y preguntas abiertas

1. **FTS5 `trigram`**: requiere un build de SQLite con FTS5 habilitado; exige `rusqlite` con feature `bundled`. Hay que verificar que el tokenizer `trigram` existe en la versión empaquetada y que el mínimo de 3 caracteres es aceptable para la UX de búsqueda.
2. **Parseo de `.ics` (RRULE) y `.vcf`**: el parseo fiable de recurrencias y de casos límite de vCard es complejo; hay que decidir librería/estrategia y su cobertura de tests.
3. **Complejidad RFC 6578 (sync-collection)**: mantener el `sync-token` incremental correcto (altas, bajas, cambios y colecciones vacías) es propenso a errores; requiere tests dedicados.
4. **Compatibilidad real con clientes DAV**: iOS, DAVx⁵ y Thunderbird validan comportamientos no siempre documentados; harán falta pruebas manuales con cada cliente.
5. **Validación JWKS offline/caching**: definir la política de caché de claves, rotación y comportamiento ante el IdP no disponible (fail-closed vs. caché con TTL).

## Convenciones de trabajo

- Cada feature se desarrolla siguiendo el flujo **SDD (OpenSpec) + TDD**: spec aprobada antes de escribir código; tests en rojo antes de la implementación.
- Un plan por feature en `plans/`, con nombre `PLAN-XXX-<slug>.md` (XXX = número secuencial). Este documento general es PLAN-001.
- Al completar un plan se renombra a `PLAN-XXX-<slug>-COMPLETED.md` (no se borra: queda como histórico).
- La documentación de cada feature se consolida en `docs/` con el mismo slug que su plan de origen.
- El orden de build importa: compilar `web/dist` **antes** de `cargo build`, porque `rust-embed` lee los assets en tiempo de compilación.
