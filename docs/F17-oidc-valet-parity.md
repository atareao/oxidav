# F17 — OIDC con paridad de valet (cookie de sesión server-side)

> Sustituye el modelo de [F04](./F04-oidc-pocket-id.md) (verificación de JWT `Bearer`
> contra el JWKS por parte del SPA). El detalle del plan está en
> [`plans/PLAN-018-oidc-valet-parity-COMPLETED.md`](../plans/PLAN-018-oidc-valet-parity-COMPLETED.md).

## Qué implementa

El **backend** hace el flujo **Authorization Code de OIDC** con un **cliente
confidencial** (`client_secret`) y establece una **cookie de sesión firmada**. El
`id_token` se intercambia, se valida y se guarda server-side; el SPA **no almacena ni
envía tokens** y **nunca ve el `access_token`** (ni gestiona refresco). La sesión viaja
en una cookie `HttpOnly` opaca.

La **única** materialización de token que llega al cliente es el `id_token_hint`
embebido en el `end_session_url` que devuelve `POST /api/auth/logout`: es el patrón
estándar de cierre de sesión SSO del IdP. El SPA solo usa esa URL para navegar, **nunca
la almacena** y **solo acepta URLs absolutas `https:`**. No se promete que el token
jamás toque el JavaScript —esa afirmación sería falsa—; lo que sí se garantiza es que
**no hay tokens en `localStorage`/`sessionStorage`**, que el cliente **no envía
`Authorization`** y que no custodia el `access_token`.

El resultado es la **paridad de diseño con valet** (`/data/rust/valet`): misma forma de
cliente, mismas cookies y mismos nombres de variables de entorno. Lo que **no** se copia
de valet es su backend criptográfico (`aws_lc_rs`): OxiDav sigue con `jsonwebtoken` en
modo `rust_crypto` para no arrastrar OpenSSL/`aws-lc-sys` y conservar el binario estático
musl. Es paridad de *diseño*, no de *features*.

Desaparece por completo el soporte `Bearer` del backend y la maquinaria PKCE/`refresh`
del frontend. La autenticación DAV (Basic + app tokens, [F05](./F05-app-tokens-basic-auth.md))
se mantiene y, además, acepta la cookie de sesión.

## Flujo paso a paso

1. El usuario llega a la SPA sin sesión. `GET /api/auth/me` responde `401`.
2. La SPA envía al navegador a `GET /api/auth/login`.
3. `login` pide el discovery del IdP (`{issuer}/.well-known/openid-configuration`),
   genera un par `state`/`nonce` por CSPRNG y **registra el `state` en un almacén en
   memoria** para consumirlo una sola vez. Emite la **cookie de flujo** `oxidav_oauth_flow`
   (firmada con `JWT_SECRET`, lleva `state`+`nonce`) y responde `302` al
   `authorization_endpoint` con `response_type=code`, `client_id`, `redirect_uri`,
   `scope=openid profile email`, `state` y `nonce`.
4. El IdP autentica al usuario y lo devuelve a `redirect_uri`
   (`{base}/api/auth/callback`) con `code` y `state`.
5. `callback`: limpia **siempre** la cookie de flujo; valida la firma de la cookie y
   compara `state`; **consume el `state` una sola vez ANTES de ir a la red** (una
   callback reenviada no canjea el código dos veces); descubre el IdP; hace
   `exchange_code` (POST `application/x-www-form-urlencoded` con `client_secret`); valida
   el `id_token` contra el JWKS (RS256, `kid`, `iss`, `aud`, `nonce`); firma la sesión.
6. Responde `302` a `/` con la **cookie de sesión** `oxidav_session` y, a la vez, una
   cookie de limpieza del flujo.
7. Las siguientes peticiones del SPA llevan `credentials: "include"`; el middleware
   valida la cookie y resuelve la identidad. `GET /api/auth/me` devuelve
   `{sub, email, name}`.

En el logout: `POST /api/auth/logout` limpia la cookie de sesión y responde
`{"end_session_url": string|null}`. Si la sesión conservaba el `id_token` y el IdP
anuncia `end_session_endpoint`, la URL lleva `id_token_hint` (y
`post_logout_redirect_uri` si está configurado) para cerrar también la sesión SSO; si no,
es `null` y solo se cierra la sesión local.

> [!note] El `id_token_hint` es la única excepción al «sin tokens en el cliente».
> El `end_session_url` **sí es legible por JavaScript** porque lleva el `id_token` en el
> query. **No se considera un riesgo relevante**: ese `id_token` es del propio usuario,
> en su propio navegador, y solo sirve para que el IdP cierre la sesión SSO (no concede
> acceso a nada de OxiDav). El SPA lo usa exclusivamente como destino de navegación, no
> lo guarda, y **solo navega a URLs absolutas `https:`**. Como defensa frente a un
> `end_session_endpoint` ajeno, el endpoint se valida contra el origen del issuer antes
> de usarse (ver *Decisiones de seguridad*).

## Cookies

| Cookie | Para qué | `Max-Age` | Atributos |
| --- | --- | --- | --- |
| `oxidav_session` | Sesión firmada (JWT HS256 con `JWT_SECRET`) | **8 h** (`SESSION_TTL_SECS`) | `HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/` |
| `oxidav_oauth_flow` | Transporta el `state`/`nonce` del flujo (JWT HS256) | **10 min** (`FLOW_TTL_SECS`) | `HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/` |

Por qué cada atributo:

- **`HttpOnly`**: ni la sesión ni el `state` son legibles desde JavaScript; un XSS no
  puede exfiltrarlos.
- **`Secure`**: la cookie solo viaja por HTTPS. Junto con la validación fail-closed de
  `AuthConfig::validate()`, es una de las dos razones por las que la autenticación activa
  exige HTTPS (ver *Configuración* y *Limitaciones*).
- **`SameSite=Lax`**: la cookie no se adjunta a mutaciones cross-site, lo que mitiga CSRF
  en las escrituras de la API y del DAV (`PUT`/`DELETE`/`MKCALENDAR`). Sí se envía en
  navegaciones GET de nivel superior. Como `Lax` es *site*-scoped, las mutaciones DAV con
  cookie llevan **además** un guard *same-origin* (ver arriba).
- **`Path=/`**: disponible para toda la aplicación.

Las funciones `emit_session`/`validate_session` usan HS256 con `leeway = 0`: una sesión
expirada se rechaza de inmediato (el contrato de `/api/auth/me` lo exige). La cookie de
flujo se firma para que **no pueda plantarla otro origen/subdominio** (*cookie tossing*)
y, además, su `state` se consume exactamente una vez desde el almacén en memoria.

## Rutas y respuestas

Todas bajo `/api/auth/*` (paridad valet; **no** bajo `/api/v1`).

| Ruta | Respuestas |
| --- | --- |
| `GET /api/auth/login` | `404` si la auth está desactivada · `502` si falla el discovery · `302` + `Set-Cookie` de flujo hacia el IdP |
| `GET /api/auth/callback` | `404` si está desactivada · `401` si falta el `code`/`state`, la cookie de flujo es inválida o el `state` ya se consumió · `502` si falla el discovery · `401` si falla el intercambio o la validación del `id_token` · `302` a `/` + cookie de sesión. **Siempre** limpia la cookie de flujo |
| `GET /api/auth/me` | `200` con `{sub, email, name}` de la sesión · `401` sin cookie válida. Con la auth desactivada, `200` con el usuario dev |
| `POST /api/auth/logout` | `200` con `{"end_session_url": string\|null}`. Limpia la cookie de sesión |

**Middleware de sesión.** Se aplica **globalmente**. Sin `auth_config` (o desactivada) es
*pass-through* e inyecta el usuario dev. Están **exentos** `/healthz`, todo `/api/auth/*`
y cualquier ruta fuera de `/api/`. Cualquier otro `/api/*` (incluido `/api/v1/*`) exige
una cookie de sesión válida; si no, `401`.

**DAV.** `/dav` sigue exigiendo Basic + app tokens y **además** acepta la cookie de
sesión cuando la auth está activa; en ambos casos se resuelve la misma identidad que
consumen las colecciones.

Cuando la autenticación proviene de la **cookie** y el método **muta estado** (todo lo
que no sea `GET`, `HEAD`, `OPTIONS`, `PROPFIND` ni `REPORT`), `/dav` exige evidencia de
*same-origin*: `Sec-Fetch-Site: same-origin` (o `none`) **o** una cabecera `Origin` igual
al origen propio. Si no, responde **403**. El camino `Authorization: Basic` (clientes DAV
nativos) no se ve afectado. Esto cierra el hueco que `SameSite=Lax` deja: `Lax` protege
frente a **sitios** distintos, pero un **subdominio hermano** del mismo dominio
registrable es *same-site* y el navegador **sí adjuntaría** la cookie; con el guard, esa
petición no pasa.

## Configuración

Variables nuevas (los nombres son **idénticos a valet**). Sustituyen a
`OXIDAV_POCKET_ID_ISSUER` y `OXIDAV_OIDC_AUDIENCE`, que **desaparecen**.

| Variable | Descripción | Default |
| --- | --- | --- |
| `AUTH_ENABLED` | Activa la autenticación. Solo `true`/`1` la encienden | `false` |
| `AUTH_ISSUER_URL` | Issuer OIDC (Pocket ID) | — (vacío) |
| `AUTH_CLIENT_ID` | Client id del cliente **confidencial** | — (vacío) |
| `AUTH_CLIENT_SECRET` | Client secret del cliente confidencial | — (vacío) |
| `AUTH_REDIRECT_URL` | `redirect_uri` = `{base}/api/auth/callback` | — (vacío) |
| `AUTH_POST_LOGOUT_REDIRECT_URL` | `post_logout_redirect_uri` opcional para el cierre SSO | — (vacío) |
| `JWT_SECRET` | Secreto HS256 que firma las cookies de sesión y de flujo | — (vacío) |

Con `AUTH_ENABLED=true`, `AuthConfig::validate()` es **fail-closed**: si `AUTH_ISSUER_URL`,
`AUTH_CLIENT_ID`, `AUTH_CLIENT_SECRET`, `AUTH_REDIRECT_URL` o `JWT_SECRET` están vacíos,
**aborta el arranque** (un `jwt_secret` vacío permitiría forjar cookies con una clave HMAC
vacía). Con `AUTH_ENABLED=false` no se exige nada.

Además, con la autenticación activada `AUTH_ISSUER_URL` y `AUTH_REDIRECT_URL` deben ser
URLs **absolutas `https:`**; si no, el arranque **aborta** nombrando la variable. La única
excepción son los hosts **loopback** (`localhost`, `127.0.0.1`, `::1`), donde se admite
`http` para pruebas locales. El porqué es doble: el `client_secret` viaja en el `POST` del
token (`token_endpoint`) y el `code` vuelve por `redirect_url`, así que ambos canales deben
ir cifrados.

> [!warning] Fail-closed, no un simple aviso.
> Un `AUTH_ISSUER_URL`/`AUTH_REDIRECT_URL` en `http:` (fuera de loopback) **no** es una
> advertencia en el log: **impide arrancar**. Configurar HTTPS solo por la cookie `Secure`
> ya no basta como recomendación: ahora es un requisito validado.

## Decisiones de seguridad

- **RS256 exclusivo**: `validate_id_token` fija `Algorithm::RS256`; se rechazan `alg=none`
  y la **confusión de algoritmo** (un HS256 firmado con el módulo RSA público).
- **`kid` obligatorio**: un `id_token` sin `kid` se rechaza antes de tocar la red.
- **Validación de reclamos**: `iss == AUTH_ISSUER_URL`, `aud == AUTH_CLIENT_ID`,
  `nonce ==` el esperado y `exp` con una tolerancia de 5 s (`ID_TOKEN_LEEWAY_SECS`).
- **`same_origin` antes de usar cada endpoint**: el `token_endpoint`, el
  `authorization_endpoint` y el `end_session_endpoint` deben compartir el *origin* del
  issuer (RFC 8414); el `jwks_uri` se vuelve a comprobar antes de descargarlo. Un
  `end_session_endpoint` de otro origen se **descarta** (el logout degrada a local). Así
  un discovery comprometido no puede redirigir el `POST` del `client_secret` ni el cierre
  de sesión —con el `id_token` en el query— a un host ajeno. Es un endurecimiento **por
  encima de valet** (que solo comprobaba `jwks_uri`).
- **HTTPS obligatorio con la auth activa (fail-closed)**: `AuthConfig::validate()` exige
  que `AUTH_ISSUER_URL` y `AUTH_REDIRECT_URL` sean URLs absolutas `https:` —con la
  excepción de hosts loopback para desarrollo— o el arranque **aborta**. El `client_secret`
  viaja en el `POST` al `token_endpoint` y el `code` vuelve por el `redirect_url`: ambos
  canales deben ir cifrados. Además, la cookie `Secure` no se guarda sobre `http://`.
- **Guard de *same-origin* en `/dav` para mutaciones con cookie**: como `SameSite=Lax` es
  *site*-scoped, un subdominio hermano (*same-site*, pero no *same-origin*) aún recibiría
  la cookie en un `PUT`/`DELETE`/`MKCALENDAR`. Cuando el método muta estado y la identidad
  viene de la cookie, se exige `Sec-Fetch-Site: same-origin`/`none` o una cabecera `Origin`
  propia; si no, **403**. El camino `Authorization: Basic` no se ve afectado.
- **Cota de tamaño** de las respuestas del IdP (`MAX_BODY_BYTES = 64 KiB`), aplicada tanto
  por `content-length` como sumando los chunks del stream; un cuerpo sobredimensionado se
  rechaza sin bufferizarlo.
- **`state` de un solo uso**: se consume exactamente una vez y **antes** del intercambio
  de red.
- **Sin seguir redirecciones**: el cliente HTTP (`reqwest`) usa
  `redirect::Policy::none()`, para que un `307`/`308` del token endpoint no reenvíe el
  `client_secret` a otro host. Timeout de 10 s.
- **El `client_secret` y el `access_token` nunca llegan al navegador**; el flujo es
  server-side y el SPA no custodia tokens. La única excepción documentada es el
  `id_token_hint` del `end_session_url` de `logout` (ver arriba), que no concede acceso.
- **Errores opacos**: los fallos de discovery devuelven el mismo `DiscoveryFailed` sin
  revelar qué endpoint se rechazó.
- **Cota del almacén de `state`**: `/login` no está autenticado, así que el mapa de
  `state` pendientes tiene un tope (`MAX_PENDING_STATES = 1024`) con evicción del más
  antiguo y purga por TTL (defensa contra *memory DoS*).

## Modo desarrollo

Con `AUTH_ENABLED=false` (el **default**) no hace falta IdP:

- La API opera como usuario dev: `sub = "dev"`, `name = "Dev User"`.
- `login` y `callback` responden `404` (las rutas no existen en dev).
- El middleware es *pass-through*.
- `GET /api/auth/me` responde el usuario dev.

Como la cookie es `Secure` y en local se sirve por `http://`, el modo soportado en
desarrollo es, precisamente, la auth desactivada. (`AuthConfig::validate()` admite `http`
para hosts **loopback** —`localhost`, `127.0.0.1`, `::1`— cuando la auth está activa, de
modo que el flujo se puede probar localmente sin relajar el requisito de HTTPS en
producción.) El SPA hace sus peticiones *same-origin* gracias al proxy de
`web/vite.config.ts` (`/api` y `/dav` → `http://localhost:8080`), así que la cookie viaja
sin CORS con credenciales.

## Limitaciones conocidas

- **Sin `refresh_token`**: al expirar la sesión (8 h) se vuelve a pasar por el IdP. No hay
  refresco silencioso.
- **Sin PKCE server-side** (paridad estricta con valet). Es un endurecimiento futuro
  posible: protege de *code injection*/mix-up y no requiere nada que no tengamos.
- **La autenticación activa exige HTTPS**: la cookie `Secure` no se guarda sobre `http://`
  (el login no persiste) y, además, `AuthConfig::validate()` **aborta el arranque** si
  `AUTH_ISSUER_URL`/`AUTH_REDIRECT_URL` no son `https:` absolutas (salvo hosts loopback).
- **El `StateStore` es en proceso**: los `state`/`nonce` de un login se guardan en la
  memoria **del proceso**. En un despliegue con **varias instancias** detrás de un
  balanceador, un `state` emitido por la instancia A no puede consumirse en la B y el
  `callback` falla con `401`. Salida: **sesiones *sticky*** en el balanceador o un
  **almacén compartido** (p. ej. Redis) para los `state` pendientes.
- **El guard de `/dav` con cookie es fail-closed**: depende de que el navegador envíe
  `Sec-Fetch-Site` u `Origin`. Una petición que presente la cookie sin ninguna de esas dos
  cabeceras se rechaza con `403`; es el comportamiento deseado, pero conviene saberlo si
  algún cliente intermedio reescribe cabeceras.
- **`JWT_SECRET` debe ser estable**: rotarlo invalida todas las sesiones vivas.
- **No hay `Bearer`** para acceso programático a la API REST; los app tokens existen para
  el DAV (Basic).
- **No hay migración automática** de despliegues que usaran `OXIDAV_POCKET_ID_ISSUER`.

## Cómo verificar

```bash
# Backend
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                 # unitarios de src/auth/ + integración de tests/auth.rs

# Frontend (desde web/)
npx tsc -b
npm run lint
npx vitest run
```

Los tests de `src/auth/` no usan red real: sirven un JWKS falso con `wiremock` y generan
un par RS256 local. Cubren la ida y vuelta y el rechazo de sesión/flujo, el `StateStore`
(un solo uso, TTL y tope), la validación fail-closed, y el rechazo de `validate_id_token`
ante issuer/audiencia/nonce/signatura incorrectos, `alg=none`, confusión de algoritmo y
`jwks_uri` en otro origen.

## Relacionado

- [`plans/PLAN-018-oidc-valet-parity-COMPLETED.md`](../plans/PLAN-018-oidc-valet-parity-COMPLETED.md)
- [`docs/F04-oidc-pocket-id.md`](./F04-oidc-pocket-id.md) — modelo anterior, superado por F17
- [`docs/F05-app-tokens-basic-auth.md`](./F05-app-tokens-basic-auth.md)
- [`docs/F06-rest-api.md`](./F06-rest-api.md)
- [`docs/F15-packaging-deploy.md`](./F15-packaging-deploy.md)
- [`AGENTS.md`](../AGENTS.md)
