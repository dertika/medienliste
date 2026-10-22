//! Anmeldung, Sitzungen und CSRF-Schutz.

pub mod oidc;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use chrono::Utc;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::config::Config;
use crate::models::User;
use crate::repo;

pub const SESSION_COOKIE: &str = "medienliste_session";

/// Zufälliges Token (256 Bit, hex-kodiert).
pub fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Vergleich in konstanter Zeit.
pub fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

pub fn session_cookie(cfg: &Config, token: &str, max_age_secs: i64) -> String {
    let path = cfg.base_path();
    let secure = if cfg.secure_cookies() { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={token}; Path={path}; Max-Age={max_age_secs}; HttpOnly; SameSite=Lax{secure}")
}

/// Legt eine neue Sitzung an und gibt den Cookie-Wert zurück.
pub async fn create_session(pool: &SqlitePool, cfg: &Config, user_id: i64) -> Result<String, sqlx::Error> {
    let token = random_token();
    let expires = Utc::now() + chrono::Duration::days(cfg.session_days);
    repo::sessions::create(pool, &hash_token(&token), user_id, &random_token(), expires).await?;
    Ok(session_cookie(cfg, &token, cfg.session_days * 86400))
}

/// Der angemeldete Benutzer samt CSRF-Token seiner Sitzung.
#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub user: User,
    pub csrf: String,
}

impl CurrentUser {
    pub fn id(&self) -> i64 {
        self.user.id
    }

    pub fn check_csrf(&self, token: &str) -> Result<(), AuthError> {
        if constant_eq(&self.csrf, token) { Ok(()) } else { Err(AuthError::Csrf) }
    }
}

#[derive(Debug)]
pub enum AuthError {
    /// Nicht angemeldet; bei Seitenaufrufen Weiterleitung zum Login.
    Login {
        base: String,
        return_to: String,
        htmx: bool,
        page: bool,
    },
    Csrf,
    Forbidden,
    Internal,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        match self {
            AuthError::Login { base, return_to, htmx, page } => {
                let target = format!("{base}auth/login?return_to={}", urlencoding::encode(&return_to));
                if htmx {
                    // htmx: komplette Seite neu laden lassen.
                    (StatusCode::UNAUTHORIZED, [("HX-Redirect", target)]).into_response()
                } else if page {
                    Redirect::to(&target).into_response()
                } else {
                    StatusCode::UNAUTHORIZED.into_response()
                }
            }
            AuthError::Csrf => (StatusCode::FORBIDDEN, "CSRF-Prüfung fehlgeschlagen").into_response(),
            AuthError::Forbidden => StatusCode::FORBIDDEN.into_response(),
            AuthError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

/// Zustand, den der Extraktor benötigt.
#[derive(Clone)]
pub struct AuthState {
    pub pool: SqlitePool,
    pub cfg: std::sync::Arc<Config>,
}

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let st = AuthState::from_ref(state);
        let htmx = parts.headers.contains_key("hx-request");
        let page = parts.method == Method::GET && !htmx;
        let login = || AuthError::Login {
            base: st.cfg.base_path().to_string(),
            return_to: local_return_to(
                &st.cfg,
                parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/"),
            ),
            htmx,
            page,
        };
        let Some(token) = cookie_value(&parts.headers, SESSION_COOKIE) else {
            return Err(login());
        };
        match repo::sessions::lookup(&st.pool, &hash_token(token)).await {
            Ok(Some((user, csrf))) => Ok(CurrentUser { user, csrf }),
            Ok(None) => Err(login()),
            Err(e) => {
                tracing::error!("Sitzung konnte nicht geladen werden: {e}");
                Err(AuthError::Internal)
            }
        }
    }
}

/// Wandelt einen Pfad aus der Anfrage in ein lokales Rücksprungziel relativ zur Basis um.
fn local_return_to(cfg: &Config, path_and_query: &str) -> String {
    let base = cfg.base_path();
    path_and_query.strip_prefix(base).unwrap_or(path_and_query.trim_start_matches('/')).to_string()
}

/// Prüft ein Rücksprungziel: nur relative Pfade innerhalb der App sind erlaubt.
pub fn safe_return_to(s: &str) -> String {
    let s = s.trim();
    let ok = !s.is_empty()
        && !s.starts_with('/')
        && !s.starts_with('\\')
        && !s.contains("://")
        && !s.contains('\\')
        && !s.chars().any(|c| c.is_control())
        && !s.starts_with("auth/");
    if ok { s.to_string() } else { String::new() }
}

/// Admin-Pflicht.
pub fn require_admin(u: &CurrentUser) -> Result<(), AuthError> {
    if u.user.is_admin { Ok(()) } else { Err(AuthError::Forbidden) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_to_is_sanitized() {
        assert_eq!(safe_return_to("liste?typ=book"), "liste?typ=book");
        assert_eq!(safe_return_to("//evil.example"), "");
        assert_eq!(safe_return_to("https://evil.example"), "");
        assert_eq!(safe_return_to("/\\evil"), "");
        assert_eq!(safe_return_to("auth/login"), "");
    }

    #[test]
    fn cookies() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "a=1; medienliste_session=abc; b=2".parse().unwrap());
        assert_eq!(cookie_value(&h, SESSION_COOKIE), Some("abc"));
        assert!(constant_eq("abc", "abc"));
        assert!(!constant_eq("abc", "abd"));
    }
}
