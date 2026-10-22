//! HTTP-Schicht: Router, Middleware, statische Dateien, Health-Endpoints.

mod auth_routes;
mod pages;
pub mod views;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use askama::Template;
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, FromRef, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use ipnet::IpNet;
use rust_embed::RustEmbed;
use sqlx::SqlitePool;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::auth::oidc::OidcService;
use crate::auth::{AuthError, AuthState};
use crate::config::Config;
use crate::covers::CoverStore;
use crate::providers::Registry;
use views::{Ctx, render_status};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub pool: SqlitePool,
    pub providers: Arc<Registry>,
    pub covers: CoverStore,
    pub oidc: Arc<OidcService>,
}

impl FromRef<AppState> for AuthState {
    fn from_ref(s: &AppState) -> Self {
        AuthState { pool: s.pool.clone(), cfg: s.cfg.clone() }
    }
}

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub fn router(state: AppState) -> Router {
    let app = Router::new()
        .route("/", get(pages::home))
        .route("/suche", get(pages::search))
        .route("/suche/hinzufuegen", post(pages::add_from_search))
        .route("/liste", get(pages::list))
        .route("/medium/{id}", get(pages::detail))
        .route("/medium/{id}/status", post(pages::set_status))
        .route("/medium/{id}/entfernen", post(pages::remove_entry))
        .route("/medium/{id}/jahr", post(pages::set_year))
        .route("/medium/{id}/bearbeiten", get(pages::edit_form).post(pages::edit_save))
        .route("/medium/{id}/loeschen", post(pages::delete_media))
        .route("/neu", get(pages::new_form).post(pages::new_save))
        .route("/statistik", get(pages::stats))
        .route("/admin", get(pages::admin))
        .route("/covers/{file}", get(pages::cover))
        .route("/auth/login", get(auth_routes::login))
        .route("/auth/callback", get(auth_routes::callback))
        .route("/auth/logout", post(auth_routes::logout))
        .route("/auth/abgemeldet", get(auth_routes::logged_out))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/static/{*path}", get(static_file))
        .route("/sw.js", get(service_worker))
        .route("/manifest.webmanifest", get(manifest))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), origin_check))
        .layer(middleware::from_fn_with_state(state.clone(), client_ip))
        .layer(RequestBodyLimitLayer::new(8 * 1024 * 1024))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' https: data:; style-src 'self' 'unsafe-inline'; \
                 script-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; \
                 form-action 'self'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ))
        .with_state(state.clone());

    let base = state.cfg.base_path().trim_end_matches('/').to_string();
    if base.is_empty() { app } else { Router::new().nest(&base, app) }
}

// ----- Fehler -----

/// Fehler in Handlern; wird als Fehlerseite gerendert.
#[derive(Debug)]
pub enum AppError {
    NotFound,
    BadRequest(&'static str),
    Auth(AuthError),
    Internal(anyhow::Error),
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Internal(e.into())
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Internal(e)
    }
}

impl From<AuthError> for AppError {
    fn from(e: AuthError) -> Self {
        AppError::Auth(e)
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    ctx: Ctx,
    title: String,
    message: String,
}

fn error_page(status: StatusCode, title_key: &str, message_key: &str) -> Response {
    let ctx = Ctx {
        lang: crate::i18n::DEFAULT_LANG,
        base: BASE_PATH.get().cloned().unwrap_or_else(|| "/".into()),
        user: None,
        csrf: String::new(),
        nav: "",
        version: env!("CARGO_PKG_VERSION"),
        tz: chrono_tz::Europe::Berlin,
    };
    let page = ErrorPage { title: ctx.t(title_key), message: ctx.t(message_key), ctx };
    render_status(status, &page)
}

/// Basis-Pfad für Fehlerseiten, die keinen Zugriff auf den State haben.
static BASE_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::NotFound => {
                error_page(StatusCode::NOT_FOUND, "error-notfound-title", "error-notfound-text")
            }
            AppError::BadRequest(key) => error_page(StatusCode::BAD_REQUEST, "error-badrequest-title", key),
            AppError::Auth(AuthError::Forbidden) => {
                error_page(StatusCode::FORBIDDEN, "error-forbidden-title", "error-forbidden-text")
            }
            AppError::Auth(AuthError::Csrf) => {
                error_page(StatusCode::FORBIDDEN, "error-forbidden-title", "error-csrf-text")
            }
            AppError::Auth(e) => e.into_response(),
            AppError::Internal(e) => {
                tracing::error!("Interner Fehler: {e:#}");
                error_page(StatusCode::INTERNAL_SERVER_ERROR, "error-internal-title", "error-internal-text")
            }
        }
    }
}

async fn not_found() -> AppError {
    AppError::NotFound
}

// ----- Middleware -----

/// Lehnt zustandsändernde Anfragen ab, deren `Origin` nicht zur App gehört
/// (zusätzlich zum CSRF-Token in jedem Formular und SameSite=Lax).
async fn origin_check(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        let expected = st.cfg.origin();
        let origin = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok());
        let referer = req.headers().get(header::REFERER).and_then(|v| v.to_str().ok());
        let ok = match (origin, referer) {
            (Some(o), _) => o == expected,
            (None, Some(r)) => r == expected || r.starts_with(&format!("{expected}/")),
            (None, None) => true, // Token-Prüfung im Handler genügt
        };
        if !ok {
            tracing::warn!(origin, referer, "Anfrage mit fremdem Origin abgelehnt");
            return AuthError::Csrf.into_response();
        }
    }
    next.run(req).await
}

/// Die echte Client-IP (aus X-Forwarded-For, aber nur von vertrauenswürdigen Proxies).
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub Option<IpAddr>);

pub fn resolve_client_ip(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &[IpNet]) -> Option<IpAddr> {
    let is_trusted = |ip: &IpAddr| trusted.iter().any(|n| n.contains(ip));
    let peer = peer?;
    if !is_trusted(&peer) {
        return Some(peer);
    }
    // Von rechts lesen: der erste nicht vertrauenswürdige Eintrag ist der Client.
    let chain: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let mut client = peer;
    for ip in chain.into_iter().rev() {
        client = ip;
        if !is_trusted(&ip) {
            break;
        }
    }
    Some(client)
}

async fn client_ip(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
    let ip = resolve_client_ip(peer, req.headers(), &st.cfg.trusted_proxies);
    req.extensions_mut().insert(ClientIp(ip));
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let start = std::time::Instant::now();
    let resp = next.run(req).await;
    if !path.ends_with("/healthz") && !path.ends_with("/readyz") {
        tracing::info!(
            client = ip.map(|i| i.to_string()).unwrap_or_default(),
            %method,
            path,
            status = resp.status().as_u16(),
            ms = start.elapsed().as_millis() as u64,
            "Anfrage"
        );
    }
    resp
}

// ----- Health -----

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz(State(st): State<AppState>) -> Response {
    match sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&st.pool).await {
        Ok(_) => "ok".into_response(),
        Err(e) => {
            tracing::error!("Readiness: Datenbank nicht erreichbar: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "db").into_response()
        }
    }
}

// ----- Statische Dateien -----

fn asset_response(path: &str, cache: &'static str) -> Response {
    match Assets::get(path) {
        Some(f) => {
            let mime = f.metadata.mimetype().to_string();
            Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, cache)
                .body(Body::from(f.data.into_owned()))
                .unwrap()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn static_file(Path(path): Path<String>) -> Response {
    asset_response(&path, "public, max-age=604800")
}

async fn service_worker() -> Response {
    let mut r = asset_response("sw.js", "no-cache");
    r.headers_mut().insert("service-worker-allowed", HeaderValue::from_static("/"));
    r
}

async fn manifest(State(st): State<AppState>) -> Response {
    let base = st.cfg.base_path();
    let i = crate::i18n::get();
    let lang = crate::i18n::DEFAULT_LANG;
    let body = serde_json::json!({
        "name": i.t(lang, "app-name"),
        "short_name": i.t(lang, "app-name"),
        "description": i.t(lang, "app-description"),
        "lang": lang,
        "start_url": format!("{base}suche"),
        "scope": base,
        "display": "standalone",
        "background_color": "#f7f4ee",
        "theme_color": "#2f5d50",
        "icons": [
            {"src": format!("{base}static/icons/icon-192.png"), "sizes": "192x192", "type": "image/png"},
            {"src": format!("{base}static/icons/icon-512.png"), "sizes": "512x512", "type": "image/png"},
            {"src": format!("{base}static/icons/icon-maskable-512.png"), "sizes": "512x512", "type": "image/png", "purpose": "maskable"}
        ]
    });
    ([(header::CONTENT_TYPE, "application/manifest+json")], body.to_string()).into_response()
}

pub fn set_base_path(base: &str) {
    let _ = BASE_PATH.set(base.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ip_only_from_trusted_proxy() {
        let trusted: Vec<IpNet> = vec!["10.0.0.0/8".parse().unwrap()];
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4, 10.0.0.5".parse().unwrap());
        let proxy: IpAddr = "10.0.0.1".parse().unwrap();
        let other: IpAddr = "5.6.7.8".parse().unwrap();
        assert_eq!(resolve_client_ip(Some(proxy), &h, &trusted), Some("1.2.3.4".parse().unwrap()));
        assert_eq!(resolve_client_ip(Some(other), &h, &trusted), Some(other));
        // Gefälschter Eintrag links vom echten Client wird ignoriert.
        h.insert("x-forwarded-for", "9.9.9.9, 1.2.3.4".parse().unwrap());
        assert_eq!(resolve_client_ip(Some(proxy), &h, &trusted), Some("1.2.3.4".parse().unwrap()));
    }
}
