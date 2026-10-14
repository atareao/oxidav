//! F17 — OIDC authentication routes (`/api/auth/*`).
//!
//! Server-side Authorization Code flow, ported from `valet/src/routes/auth.rs`:
//! `login` builds the provider authorization URL and persists a signed, one-time
//! `state`/`nonce` flow cookie; `callback` verifies that cookie, consumes its
//! `state` exactly once, exchanges the code, validates the ID token and
//! establishes the signed session cookie; `me` returns the session claims;
//! `logout` clears the session and returns the provider SSO logout URL.
//!
//! The `id_token` never reaches JavaScript: it is exchanged and validated
//! server-side and only the opaque session cookie is sent to the browser.

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use cookie::time::Duration;
use serde::Serialize;

use crate::auth::session::{
    Claims, SESSION_COOKIE_NAME, SESSION_TTL_SECS, StateStore, build_end_session_url, discover,
    emit_flow_token, emit_session, exchange_code, http_client, validate_flow_token,
    validate_id_token, validate_session,
};
use crate::router::AppState;

/// Signed flow cookie carrying the one-time `state` and `nonce`.
const FLOW_COOKIE: &str = "oxidav_oauth_flow";
/// Lifetime of the flow cookie, in seconds (matches `session::FLOW_TTL`).
const FLOW_TTL_SECS: i64 = 600;

/// Response body of `POST /api/auth/logout`.
///
/// `end_session_url` is `Some` when the session kept a provider ID token, so
/// the frontend can redirect the user to close the SSO session; it is `None`
/// when only the local session was closed.
#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub end_session_url: Option<String>,
}

/// Response body of `GET /api/auth/me`.
#[derive(Debug, Serialize)]
struct MeResponse {
    sub: String,
    email: Option<String>,
    name: Option<String>,
}

/// Process-wide store enforcing single use of each issued `state`.
fn state_store() -> &'static StateStore {
    static STORE: OnceLock<StateStore> = OnceLock::new();
    STORE.get_or_init(StateStore::new)
}

/// `GET /api/auth/login` — build the provider authorization URL and redirect.
///
/// Answers `404` when auth is disabled (the route simply does not exist in dev
/// mode). On success it sets the signed flow cookie and redirects (`302`) to the
/// provider's `authorization_endpoint` with the one-time `state`/`nonce`.
pub async fn login(State(state): State<AppState>) -> Response {
    let Some(config) = state.auth_config.as_ref().filter(|config| config.enabled) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let discovery = match discover(http_client(), &config.issuer_url).await {
        Ok(discovery) => discovery,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };

    let Ok(mut url) = url::Url::parse(&discovery.authorization_endpoint) else {
        return StatusCode::BAD_GATEWAY.into_response();
    };

    // CSPRNG state/nonce: recorded for one-time consumption AND signed into the
    // flow cookie so it cannot be planted by another origin (cookie tossing).
    let (state_value, nonce_value) = state_store().issue();
    let flow = match emit_flow_token(config, &state_value, &nonce_value, FLOW_TTL_SECS as u64) {
        Ok(flow) => flow,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &config.redirect_url)
        .append_pair("scope", "openid profile email")
        .append_pair("state", &state_value)
        .append_pair("nonce", &nonce_value);

    with_cookies(redirect_to(url.as_ref()), [flow_cookie(flow)])
}

/// `GET /api/auth/callback` — validate state/nonce, exchange the code,
/// validate the ID token and establish the session cookie.
///
/// The flow cookie is cleared on **every** outcome (it is one-time). The `state`
/// is consumed exactly once **before** the network exchange, so a replayed
/// callback cannot redeem the code twice.
pub async fn callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(config) = state.auth_config.as_ref().filter(|config| config.enabled) else {
        // Auth disabled: the route does not exist, but the one-time flow cookie
        // must still be cleared so the "cleared on every outcome" contract holds
        // (a leftover flow cookie from a previous deployment must not linger).
        return with_cookies(
            StatusCode::NOT_FOUND.into_response(),
            [clear_cookie(FLOW_COOKIE)],
        );
    };

    // The flow cookie is one-time: whatever happens, clear it.
    let clear = [clear_cookie(FLOW_COOKIE)];

    let state_param = params.get("state");
    let code = params.get("code");
    let flow = jar
        .get(FLOW_COOKIE)
        .and_then(|cookie| validate_flow_token(config, cookie.value()).ok());

    let (Some(state_param), Some(code), Some(flow)) = (state_param, code, flow) else {
        return with_cookies(StatusCode::UNAUTHORIZED.into_response(), clear);
    };

    if flow.state.as_str() != state_param.as_str() {
        return with_cookies(StatusCode::UNAUTHORIZED.into_response(), clear);
    }

    // Consume the state exactly once BEFORE the network exchange, so a replayed
    // callback cannot trigger a second code redemption.
    if state_store().consume(&flow.state).is_none() {
        return with_cookies(StatusCode::UNAUTHORIZED.into_response(), clear);
    }

    let client = http_client();
    let discovery = match discover(client, &config.issuer_url).await {
        Ok(discovery) => discovery,
        Err(_) => return with_cookies(StatusCode::BAD_GATEWAY.into_response(), clear),
    };

    let token = match exchange_code(client, config, &discovery.token_endpoint, code.as_str()).await
    {
        Ok(token) => token,
        Err(_) => return with_cookies(StatusCode::UNAUTHORIZED.into_response(), clear),
    };

    let identity = match validate_id_token(
        client,
        &discovery.jwks_uri,
        &token.id_token,
        &config.issuer_url,
        &config.client_id,
        &flow.nonce,
    )
    .await
    {
        Ok(identity) => identity,
        Err(_) => return with_cookies(StatusCode::UNAUTHORIZED.into_response(), clear),
    };

    let now = unix_now();
    let claims = Claims {
        sub: identity.sub,
        email: identity.email,
        name: identity.name,
        exp: now + SESSION_TTL_SECS as usize,
        iat: now,
        id_token: Some(token.id_token),
    };

    let session = match emit_session(config, &claims) {
        Ok(session) => session,
        Err(_) => return with_cookies(StatusCode::INTERNAL_SERVER_ERROR.into_response(), clear),
    };

    let cookies = [clear_cookie(FLOW_COOKIE), session_cookie(session)];

    with_cookies(redirect_to("/"), cookies)
}

/// `GET /api/auth/me` — return the claims of the current session, or 401.
///
/// With auth disabled it answers the synthetic dev user so a development SPA
/// boots without an IdP.
pub async fn me(State(state): State<AppState>, jar: CookieJar) -> Response {
    match state.auth_config.as_ref() {
        Some(config) if config.enabled => {
            let Some(session) = jar.get(SESSION_COOKIE_NAME) else {
                return StatusCode::UNAUTHORIZED.into_response();
            };

            match validate_session(config, session.value()) {
                Ok(claims) => Json(MeResponse {
                    sub: claims.sub,
                    email: claims.email,
                    name: claims.name,
                })
                .into_response(),
                Err(_) => StatusCode::UNAUTHORIZED.into_response(),
            }
        }
        // With OIDC disabled the API operates as a default dev user.
        _ => Json(MeResponse {
            sub: "dev".into(),
            email: None,
            name: Some("Dev User".into()),
        })
        .into_response(),
    }
}

/// `POST /api/auth/logout` — clear the session cookie and return the SSO
/// end-session URL.
///
/// The URL is built only when the session retained an `id_token` (and the
/// provider advertises an `end_session_endpoint`); otherwise only the local
/// session is closed and `end_session_url` is `null`.
pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> Response {
    let end_session_url = match state.auth_config.as_ref() {
        Some(config) if config.enabled => {
            let id_token = jar
                .get(SESSION_COOKIE_NAME)
                .and_then(|cookie| validate_session(config, cookie.value()).ok())
                .and_then(|claims| claims.id_token);

            match id_token {
                Some(id_token) => match discover(http_client(), &config.issuer_url).await {
                    Ok(discovery) => {
                        discovery
                            .end_session_endpoint
                            .as_deref()
                            .and_then(|endpoint| {
                                build_end_session_url(config, endpoint, Some(&id_token))
                            })
                    }
                    Err(_) => None,
                },
                None => None,
            }
        }
        _ => None,
    };

    let response = Json(LogoutResponse { end_session_url }).into_response();
    with_cookies(response, [clear_cookie(SESSION_COOKIE_NAME)])
}

/// Authentication route table, merged into the main router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/login", get(login))
        .route("/api/auth/callback", get(callback))
        .route("/api/auth/me", get(me))
        .route("/api/auth/logout", post(logout))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Session/flow cookies are `HttpOnly` (unreadable from JS), `Secure` (never
/// sent over plain HTTP) and `SameSite=Lax` (not attached to cross-site
/// mutations, mitigating CSRF on DAV/API writes).
fn flow_cookie(value: String) -> Cookie<'static> {
    Cookie::build((FLOW_COOKIE, value))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(Duration::seconds(FLOW_TTL_SECS))
        .build()
}

fn session_cookie(value: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE_NAME, value))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(Duration::seconds(SESSION_TTL_SECS as i64))
        .build()
}

/// Clear a cookie with the same attributes used to set it, so the browser
/// matches and drops it.
fn clear_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, ""))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(Duration::seconds(0))
        .build()
}

/// Build a `302 Found` redirect to `location`. Locations are provider/route
/// internals, not user input, so a malformed value simply omits the header.
fn redirect_to(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

/// Append one `Set-Cookie` header per cookie (never joined into a single line).
fn with_cookies(
    mut response: Response,
    cookies: impl IntoIterator<Item = Cookie<'static>>,
) -> Response {
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

/// Current UNIX timestamp in seconds.
fn unix_now() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as usize)
        .unwrap_or(0)
}
