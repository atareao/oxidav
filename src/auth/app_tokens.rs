//! F05 — Revocable app tokens for native DAV clients (Basic Auth).
//!
//! Native CalDAV/CardDAV clients (iOS, DAVx⁵, Thunderbird) cannot run an OIDC
//! Authorization Code flow, so OxiDav issues long-lived **app tokens** that they
//! present via HTTP Basic Auth. This module owns the token lifecycle:
//!
//! * [`generate_token`] mints a fresh token with a CSPRNG.
//! * [`hash_secret`] / [`verify_secret`] store and check the secret without ever
//!   persisting it in the clear, comparing digests in **constant time**.
//! * The repositories ([`create_app_token`], [`list_app_tokens`],
//!   [`get_app_token`], [`rename_app_token`], [`revoke_app_token`],
//!   [`touch_last_used`], [`verify_app_token`]) are the only code that touches
//!   the `app_tokens` table.
//!
//! # Token format
//!
//! `oxidav_<id>_<secret>` where
//!
//! * `<id>` is the **public** identifier (16 CSPRNG bytes, lowercase hex). It is
//!   safe to log and is used for an indexed `O(log n)` lookup before the secret
//!   is compared.
//! * `<secret>` is **≥256 bits** of CSPRNG entropy, Base64 URL-safe without
//!   padding. It is never stored; only its digest is.
//!
//! # Hashing
//!
//! The secret is a random 256-bit string, not a human-chosen password, so there
//! is no enumerable candidate space to harden against: a costly KDF (Argon2)
//! would add latency without buying anything. We therefore store
//! `SHA-256(secret)` (hex), or `HMAC-SHA256(pepper, secret)` when the optional
//! server pepper `OXIDAV_TOKEN_PEPPER` is set. Comparison uses
//! [`subtle::ConstantTimeEq`] to avoid timing side channels.
//!
//! **Revisable decision:** if the token format ever moves to low-entropy
//! secrets (e.g. short codes), this must migrate to Argon2id and version the
//! `secret_hash` column (e.g. an `$argon2id$…` prefix).

use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

use crate::auth::oidc::AuthUser;
use crate::db::DbError;

/// Environment variable holding the optional server-side pepper.
///
/// When set (and non-empty) it is used as the HMAC-SHA256 key instead of a bare
/// SHA-256. It is never stored in the database; losing or rotating it invalidates
/// every existing token, so rotation requires either regenerating tokens or
/// keeping the previous pepper during a transition window.
pub const ENV_TOKEN_PEPPER: &str = "OXIDAV_TOKEN_PEPPER";

/// Fixed prefix of every app token.
const TOKEN_PREFIX: &str = "oxidav";
/// CSPRNG bytes used for the public id (hex-encoded → 32 characters).
const ID_BYTES: usize = 16;
/// CSPRNG bytes used for the secret (32 bytes = 256 bits of entropy).
const SECRET_BYTES: usize = 32;

/// A fixed, well-formed digest used as a decoy when the token id is unknown.
///
/// [`verify_app_token`] always compares the presented secret against *some*
/// digest, so an attacker cannot time "unknown id" apart from "known id, wrong
/// secret". Feeding this constant for unknown ids keeps the work identical.
const DECOY_SECRET_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Maximum number of concurrent `last_used_at` background updates.
///
/// Under WAL, concurrent writers serialise; capping the in-flight updates keeps
/// the write path bounded instead of queueing an unbounded number of tiny
/// writes. Updates beyond the cap are dropped (see [`spawn_touch_last_used`]).
const TOUCH_CONCURRENCY: usize = 4;

type HmacSha256 = Hmac<Sha256>;

/// Errors produced while generating, storing or verifying app tokens.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    /// The token does not have the `oxidav_<id>_<secret>` shape.
    #[error("formato de token inválido")]
    Malformed,
    /// No row matches the token's public id.
    #[error("token no encontrado")]
    NotFound,
    /// The token exists but has been revoked.
    #[error("token revocado")]
    Revoked,
    /// The token exists and is active, but the secret digest does not match.
    #[error("secret no coincide")]
    SecretMismatch,
    /// The underlying database operation failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

/// Public view of an app token. **Never** includes `secret_hash`.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AppToken {
    /// Public identifier (the `oxidav_<id>_…` segment).
    pub id: String,
    /// Pocket ID `sub` claim of the owner.
    pub user_id: String,
    /// Human-readable label ("iPhone", "DAVx5", …).
    pub name: String,
    /// Creation time, Unix seconds.
    pub created_at: i64,
    /// Last successful use, Unix seconds; `None` if never used.
    pub last_used_at: Option<i64>,
    /// Revocation time, Unix seconds; `None` while active.
    pub revoked_at: Option<i64>,
}

/// Generate a new token with the system CSPRNG.
///
/// Returns `(token, id, secret)` where `token == "oxidav_<id>_<secret>"`.
/// `id` is the public lookup key and `secret` is the ≥256-bit secret; the
/// caller must hand `secret` to [`hash_secret`] and then discard it.
#[must_use]
pub fn generate_token() -> (String, String, String) {
    let id = hex_encode(&random_bytes(ID_BYTES));
    let secret = URL_SAFE_NO_PAD.encode(random_bytes(SECRET_BYTES));
    let token = format!("{TOKEN_PREFIX}_{id}_{secret}");
    (token, id, secret)
}

/// Split `"oxidav_<id>_<secret>"` into `(id, secret)`.
///
/// The split is strict: exactly three `_`-separated segments, the literal
/// `oxidav` prefix, a valid `id` and a non-empty `secret`. A valid `id` is
/// exactly [`ID_BYTES`]·2 ASCII hex characters (the shape [`generate_token`]
/// mints); anything else is rejected before it can reach a lookup. Because
/// `<id>` is hex (no `_`) and only the first two separators are consumed, a
/// `<secret>` that happens to contain `_` is preserved intact.
///
/// # Errors
///
/// Returns [`TokenError::Malformed`] when the shape is not recognised.
pub fn parse_token(token: &str) -> Result<(String, String), TokenError> {
    let mut segments = token.splitn(3, '_');
    let prefix = segments.next().unwrap_or_default();
    let id = segments.next().ok_or(TokenError::Malformed)?;
    let secret = segments.next().ok_or(TokenError::Malformed)?;

    if prefix != TOKEN_PREFIX || !is_valid_id(id) || secret.is_empty() {
        return Err(TokenError::Malformed);
    }

    Ok((id.to_string(), secret.to_string()))
}

/// `true` when `id` has the canonical shape: [`ID_BYTES`] hex characters.
fn is_valid_id(id: &str) -> bool {
    id.len() == ID_BYTES * 2 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Digest of a secret for storage, using the process pepper if configured.
///
/// Returns lowercase hex: `HMAC-SHA256(pepper, secret)` when
/// [`ENV_TOKEN_PEPPER`] is set and non-empty, otherwise `SHA-256(secret)`.
#[must_use]
pub fn hash_secret(secret: &str) -> String {
    hash_secret_with_pepper(secret, pepper().as_deref())
}

/// Verify `secret` against a stored digest in constant time.
///
/// Returns `false` (without panicking) for an empty secret or a malformed /
/// wrong-length `stored_hash`.
#[must_use]
pub fn verify_secret(secret: &str, stored_hash: &str) -> bool {
    let candidate = hash_secret(secret);
    candidate.as_bytes().ct_eq(stored_hash.as_bytes()).into()
}

/// Digest `secret` with an explicit pepper (or `None` for bare SHA-256).
fn hash_secret_with_pepper(secret: &str, pepper: Option<&str>) -> String {
    match pepper {
        Some(pepper) if !pepper.is_empty() => match HmacSha256::new_from_slice(pepper.as_bytes()) {
            Ok(mut mac) => {
                mac.update(secret.as_bytes());
                hex_encode(&mac.finalize().into_bytes())
            }
            // HMAC-SHA256 accepts keys of any length, so this is unreachable.
            // Fall back to a bare SHA-256 rather than panicking.
            Err(_) => hex_encode(&Sha256::digest(secret.as_bytes())),
        },
        _ => hex_encode(&Sha256::digest(secret.as_bytes())),
    }
}

/// Process-wide cache of the optional pepper, read from the environment once.
///
/// The pepper is configuration that does not change at runtime, so reading it on
/// every [`hash_secret`] call (the hot path of [`verify_secret`]) is wasted work
/// and an unnecessary `std::env` lookup. [`OnceLock`] snapshots it on first use
/// and reuses it for the life of the process.
static PEPPER: OnceLock<Option<String>> = OnceLock::new();

/// Read the optional pepper from the environment (unset/blank → `None`).
///
/// The value is cached on first call so subsequent (hot-path) lookups are free.
fn pepper() -> Option<String> {
    PEPPER
        .get_or_init(|| {
            std::env::var(ENV_TOKEN_PEPPER)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .clone()
}

/// Fill a buffer with cryptographically secure random bytes.
fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf
}

/// Lowercase hex encoding (avoids an extra dependency and the `_` character
/// that Base64 URL-safe would introduce, keeping [`parse_token`] unambiguous).
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Current wall-clock time as Unix seconds.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

// ---------------------------------------------------------------------------
// Repository
// ---------------------------------------------------------------------------

/// Create a new app token for `user_id` and return `(id, token)`.
///
/// The returned `token` is shown to the user **once**: only the secret digest is
/// persisted, so it can never be retrieved again.
///
/// # Errors
///
/// Returns [`TokenError::Db`] on any database failure.
pub async fn create_app_token(
    pool: &SqlitePool,
    user_id: &str,
    name: &str,
) -> Result<(String, String), TokenError> {
    let (token, id, secret) = generate_token();
    let secret_hash = hash_secret(&secret);

    sqlx::query(
        "INSERT INTO app_tokens (id, user_id, name, secret_hash, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(name)
    .bind(&secret_hash)
    .bind(now_secs())
    .execute(pool)
    .await
    .map_err(DbError::Query)?;

    Ok((id, token))
}

/// List the public views of `user_id`'s tokens, newest first.
///
/// # Errors
///
/// Returns [`TokenError::Db`] on any database failure.
pub async fn list_app_tokens(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<AppToken>, TokenError> {
    let rows = sqlx::query_as::<_, AppToken>(
        "SELECT id, user_id, name, created_at, last_used_at, revoked_at
           FROM app_tokens
          WHERE user_id = ?
          ORDER BY created_at DESC, id ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)?;

    Ok(rows)
}

/// Fetch a single token of `user_id` by its public id.
///
/// Scoped by `user_id`, so a token owned by another user is reported as absent
/// (`None`) rather than exposed — the caller maps that to a uniform `404`
/// (anti-IDOR). Revoked rows are returned as-is (with their `revoked_at`) so the
/// caller can distinguish "active" from "already revoked".
///
/// # Errors
///
/// Returns [`TokenError::Db`] on any database failure.
pub async fn get_app_token(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> Result<Option<AppToken>, TokenError> {
    let row = sqlx::query_as::<_, AppToken>(
        "SELECT id, user_id, name, created_at, last_used_at, revoked_at
           FROM app_tokens
          WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(DbError::Query)?;

    Ok(row)
}

/// Rename one of `user_id`'s **active** tokens.
///
/// Scoped by `user_id` and restricted to active rows (`revoked_at IS NULL`): a
/// token that does not exist, belongs to another user, or has already been
/// revoked yields [`TokenError::NotFound`], so a foreign token's existence is
/// never leaked (anti-IDOR). Returns the updated public view.
///
/// # Errors
///
/// Returns [`TokenError::NotFound`] when no matching active row exists, or
/// [`TokenError::Db`] on any database failure.
pub async fn rename_app_token(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
    name: &str,
) -> Result<AppToken, TokenError> {
    let result = sqlx::query(
        "UPDATE app_tokens
            SET name = ?
          WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
    )
    .bind(name)
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(DbError::Query)?;

    if result.rows_affected() == 0 {
        return Err(TokenError::NotFound);
    }

    get_app_token(pool, user_id, id)
        .await?
        .ok_or(TokenError::NotFound)
}

/// Revoke one of `user_id`'s tokens (logical delete via `revoked_at`).
///
/// Idempotent: revoking an already-revoked token succeeds and leaves the
/// original `revoked_at` untouched. A token that does not belong to `user_id`
/// (or does not exist) is **not** affected and yields [`TokenError::NotFound`].
///
/// # Errors
///
/// Returns [`TokenError::NotFound`] when no matching row exists, or
/// [`TokenError::Db`] on any database failure.
pub async fn revoke_app_token(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> Result<(), TokenError> {
    let result = sqlx::query(
        "UPDATE app_tokens
            SET revoked_at = COALESCE(revoked_at, ?)
          WHERE id = ? AND user_id = ?",
    )
    .bind(now_secs())
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(DbError::Query)?;

    if result.rows_affected() == 0 {
        return Err(TokenError::NotFound);
    }
    Ok(())
}

/// Update `last_used_at` for `id`.
///
/// Best-effort: a missing `id` is a no-op (not an error), so this never fails a
/// request that has already authenticated successfully.
///
/// # Errors
///
/// Returns [`TokenError::Db`] only on a genuine database failure.
pub async fn touch_last_used(pool: &SqlitePool, id: &str) -> Result<(), TokenError> {
    sqlx::query("UPDATE app_tokens SET last_used_at = ? WHERE id = ?")
        .bind(now_secs())
        .bind(id)
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
    Ok(())
}

/// Process-wide limiter coalescing `last_used_at` background updates.
static TOUCH_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

/// Fire-and-forget `last_used_at` update on a background task.
///
/// Used by the DAV extractor so authentication never blocks on, nor fails
/// because of, this write.
///
/// **Coalescing decision:** a naïve `tokio::spawn` per request can fan out an
/// unbounded number of tiny writes, and under WAL concurrent writers serialise,
/// so a busy deployment would pile up tasks competing for the single writer.
/// Instead a process-wide [`Semaphore`] caps the in-flight updates at
/// [`TOUCH_CONCURRENCY`]; when it is saturated the surplus update is **dropped**
/// rather than queued. The timestamp is coarse-grained and purely informational,
/// so a dropped update only means the token reports a slightly older
/// `last_used_at`; requests are never delayed or failed either way.
pub fn spawn_touch_last_used(pool: SqlitePool, id: String) {
    let semaphore = TOUCH_SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(TOUCH_CONCURRENCY)))
        .clone();

    let permit = match semaphore.try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            tracing::debug!("dropping `last_used_at` update: in-flight limit reached");
            return;
        }
    };

    tokio::spawn(async move {
        let _permit = permit;
        if let Err(err) = touch_last_used(&pool, &id).await {
            tracing::debug!(%err, "best-effort `last_used_at` update failed");
        }
    });
}

/// Resolve a full token to the authenticated identity.
///
/// Parses the token, looks the row up by its public id, rejects revoked tokens
/// and verifies the secret digest in constant time. Callers must map every
/// error to a generic `401` so clients cannot tell "unknown" from "revoked".
///
/// **Timing note:** the secret digest is always computed — against
/// [`DECOY_SECRET_HASH`] when the id is unknown — before the outcome is decided,
/// so response time cannot reveal whether an id exists. Only the error *value*
/// (which callers collapse to a generic `401`) distinguishes the cases.
///
/// # Errors
///
/// [`TokenError::Malformed`], [`TokenError::NotFound`], [`TokenError::Revoked`],
/// [`TokenError::SecretMismatch`] or [`TokenError::Db`].
pub async fn verify_app_token(pool: &SqlitePool, token: &str) -> Result<AuthUser, TokenError> {
    let (id, secret) = parse_token(token)?;

    let row: Option<(String, String, Option<i64>)> =
        sqlx::query_as("SELECT user_id, secret_hash, revoked_at FROM app_tokens WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(DbError::Query)?;

    let known = row.is_some();
    let (user_id, secret_hash, revoked_at) = match row {
        Some((user_id, secret_hash, revoked_at)) => (user_id, secret_hash, revoked_at),
        // Unknown id: digest against a decoy so the comparison work is identical
        // to the known-id path and does not leak existence by timing.
        None => (String::new(), DECOY_SECRET_HASH.to_string(), None),
    };

    // Compute the constant-time comparison unconditionally, before deciding.
    let secret_matches = verify_secret(&secret, &secret_hash);

    if !known {
        return Err(TokenError::NotFound);
    }
    if revoked_at.is_some() {
        return Err(TokenError::Revoked);
    }
    if !secret_matches {
        return Err(TokenError::SecretMismatch);
    }

    Ok(AuthUser { sub: user_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_token_has_expected_shape() {
        let (token, id, secret) = generate_token();

        assert!(token.starts_with("oxidav_"), "token: {token}");
        assert_eq!(token, format!("oxidav_{id}_{secret}"));
        assert_eq!(id.len(), ID_BYTES * 2, "id must be hex of {ID_BYTES} bytes");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "id: {id}");

        let (parsed_id, parsed_secret) = parse_token(&token).expect("token must parse");
        assert_eq!(parsed_id, id);
        assert_eq!(parsed_secret, secret);

        let raw = URL_SAFE_NO_PAD
            .decode(&secret)
            .expect("secret is base64url");
        assert!(
            raw.len() >= 32,
            "secret must carry ≥256 bits, got {} bytes",
            raw.len()
        );
    }

    #[test]
    fn generate_token_is_unique() {
        use std::collections::HashSet;

        let mut ids = HashSet::new();
        let mut tokens = HashSet::new();
        for _ in 0..1000 {
            let (token, id, _secret) = generate_token();
            assert!(ids.insert(id), "duplicate id generated");
            assert!(tokens.insert(token), "duplicate token generated");
        }
    }

    #[test]
    fn hash_secret_is_deterministic_and_hex() {
        let a = hash_secret("s3cret");
        let b = hash_secret("s3cret");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64, "SHA-256 hex is 64 chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, hash_secret("other"));
    }

    #[test]
    fn verify_secret_matches_only_the_right_secret() {
        let hash = hash_secret("correct horse");
        assert!(verify_secret("correct horse", &hash));
        assert!(!verify_secret("battery staple", &hash));
    }

    #[test]
    fn verify_secret_handles_empty_and_malformed_without_panicking() {
        assert!(!verify_secret("", ""));
        assert!(!verify_secret("", &hash_secret("x")));
        assert!(!verify_secret("x", ""));
        assert!(!verify_secret("x", "not-hex"));
        // Wrong length: constant-time comparison must return false, not panic.
        assert!(!verify_secret("x", &"a".repeat(63)));
    }

    #[test]
    fn pepper_changes_the_digest() {
        let bare = hash_secret_with_pepper("s3cret", None);
        let peppered = hash_secret_with_pepper("s3cret", Some("server-pepper"));

        assert_ne!(bare, peppered);
        // Deterministic for the same pepper, and sensitive to the pepper value.
        assert_eq!(
            peppered,
            hash_secret_with_pepper("s3cret", Some("server-pepper"))
        );
        assert_ne!(
            peppered,
            hash_secret_with_pepper("s3cret", Some("other-pepper"))
        );
        // An empty pepper degrades to the bare digest.
        assert_eq!(bare, hash_secret_with_pepper("s3cret", Some("")));
    }

    #[test]
    fn parse_token_accepts_and_rejects() {
        let id = "0123456789abcdef0123456789abcdef"; // 32 hex chars
        assert_eq!(
            parse_token(&format!("oxidav_{id}_secret")).expect("valid"),
            (id.to_string(), "secret".to_string())
        );
        // The secret is the whole trailing segment, underscores included.
        assert_eq!(
            parse_token(&format!("oxidav_{id}_cd_ef")).expect("valid"),
            (id.to_string(), "cd_ef".to_string())
        );

        for bad in [
            "",
            "oxidav",
            "oxidav_",
            "oxidav_id",
            "oxidav_id_",
            "oxidav__secret",
            "other_id_secret",
            "bearer_abc_def",
            // id present but not exactly 32 hex chars:
            "oxidav_abc_def",
            "oxidav_0123456789abcdef0123456789abcde_secret", // 31 chars
            "oxidav_0123456789abcdef0123456789abcdefg_secret", // 33 chars
            "oxidav_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz_secret", // non-hex
        ] {
            assert!(
                matches!(parse_token(bad), Err(TokenError::Malformed)),
                "expected Malformed for {bad:?}"
            );
        }
    }
}
