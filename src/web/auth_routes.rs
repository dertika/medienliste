//! Login über OIDC, Callback und Logout.

use askama::Template;
use axum::Form;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::views::{Ctx, render, render_status};
use super::{AppError, AppState};
use crate::auth::{self, CurrentUser, SESSION_COOKIE, cookie_value, hash_token, safe_return_to};
use crate::repo;

#[derive(Deserialize)]
pub struct LoginQuery {
    #[serde(default)]
    return_to: String,
}

pub async fn login(State(st): State<AppState>, Query(q): Query<LoginQuery>) -> Result<Response, AppError> {
    let req = match st.oidc.begin().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("OIDC nicht verfügbar: {e:#}");
            return Ok(login_error(&st, "login-error-provider"));
        }
    };
    repo::sessions::save_login_state(
        &st.pool,
        &req.state,
        &req.pkce_verifier,
        &req.nonce,
        &safe_return_to(&q.return_to),
    )
    .await?;
    Ok(Redirect::to(req.url.as_str()).into_response())
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Template)]
#[template(path = "login_error.html")]
struct LoginErrorPage {
    ctx: Ctx,
    message: String,
}

fn login_error(st: &AppState, key: &str) -> Response {
    let ctx = Ctx::new(st, None, "");
    let message = ctx.t(key);
    render_status(StatusCode::BAD_REQUEST, &LoginErrorPage { ctx, message })
}

pub async fn callback(
    State(st): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    if let Some(err) = q.error {
        tracing::warn!("OIDC-Fehler vom Provider: {err} {}", q.error_description.unwrap_or_default());
        return Ok(login_error(&st, "login-error-denied"));
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return Ok(login_error(&st, "login-error-invalid"));
    };
    let Some((verifier, nonce, return_to)) = repo::sessions::take_login_state(&st.pool, &state).await? else {
        return Ok(login_error(&st, "login-error-expired"));
    };
    let claims = match st.oidc.finish(&code, &verifier, &nonce).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Anmeldung fehlgeschlagen: {e:#}");
            return Ok(login_error(&st, "login-error-invalid"));
        }
    };
    if !st.oidc.is_allowed(&claims) {
        // Kein Anlegen/Aktualisieren des Benutzers; bestehende Sitzungen werden beendet.
        let ended = repo::sessions::delete_for_sub(&st.pool, &claims.sub).await?;
        tracing::warn!(sub = %claims.sub, sessions_ended = ended, "Anmeldung abgelehnt: Pflichtgruppe fehlt");
        let ctx = Ctx::new(&st, None, "");
        let message = ctx.t("login-error-not-allowed");
        return Ok(render_status(StatusCode::FORBIDDEN, &LoginErrorPage { ctx, message }));
    }
    let is_admin = st.oidc.is_admin(&claims);
    let user =
        repo::users::upsert_login(&st.pool, &claims.sub, &claims.name, &claims.email, is_admin).await?;
    tracing::info!(user_id = user.id, admin = is_admin, "Benutzer angemeldet");
    let cookie = auth::create_session(&st.pool, &st.cfg, user.id).await?;
    let target = format!("{}{}", st.cfg.base_path(), safe_return_to(&return_to));
    Ok(([(header::SET_COOKIE, cookie)], Redirect::to(&target)).into_response())
}

#[derive(Deserialize)]
pub struct CsrfForm {
    #[serde(default)]
    csrf: String,
}

pub async fn logout(
    State(st): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    cu.check_csrf(&f.csrf)?;
    if let Some(token) = cookie_value(&headers, SESSION_COOKIE) {
        repo::sessions::delete(&st.pool, &hash_token(token)).await?;
    }
    let clear = auth::session_cookie(&st.cfg, "", 0);
    // Die Sitzung beim Identity Provider (z. B. Authelia) bleibt bestehen; ist eine
    // Logout-URL konfiguriert, geht es dorthin, sonst auf die eigene Abmeldeseite.
    let target = match &st.cfg.logout_url {
        Some(u) => u.to_string(),
        None => format!("{}auth/abgemeldet", st.cfg.base_path()),
    };
    Ok(([(header::SET_COOKIE, clear)], Redirect::to(&target)).into_response())
}

#[derive(Template)]
#[template(path = "logged_out.html")]
struct LoggedOutPage {
    ctx: Ctx,
}

pub async fn logged_out(State(st): State<AppState>) -> Response {
    render(&LoggedOutPage { ctx: Ctx::new(&st, None, "") })
}
