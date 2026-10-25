//! Testumgebung: App mit In-Memory-Datenbank, Stub-Provider und Testbenutzern.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use medienliste::auth::{self, hash_token};
use medienliste::config::Config;
use medienliste::covers::CoverStore;
use medienliste::models::{MediaType, NewMedia};
use medienliste::providers::{Provider, Registry};
use medienliste::web::{self, AppState};
use medienliste::{db, repo};
use sqlx::SqlitePool;
use tower::ServiceExt;

pub const ORIGIN: &str = "https://medienliste.test";

pub struct Harness {
    pub app: Router,
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    _dir: tempfile::TempDir,
}

pub struct TestUser {
    pub id: i64,
    pub cookie: String,
    pub csrf: String,
}

/// Liefert feste Treffer, damit Tests ohne Netzwerk laufen.
pub struct StubProvider {
    pub kind: MediaType,
    pub hits: Vec<NewMedia>,
}

#[async_trait]
impl Provider for StubProvider {
    fn id(&self) -> &'static str {
        match self.kind {
            MediaType::Book => "stubbook",
            MediaType::Audiobook => "stubaudio",
            MediaType::Movie => "stubmovie",
            MediaType::Series => "stubseries",
        }
    }
    fn media_type(&self) -> MediaType {
        self.kind
    }
    async fn search(&self, q: &str, _limit: usize) -> anyhow::Result<Vec<NewMedia>> {
        let q = q.to_lowercase();
        Ok(self.hits.iter().filter(|h| h.title.to_lowercase().contains(&q)).cloned().collect())
    }
    async fn fetch(&self, id: &str) -> anyhow::Result<Option<NewMedia>> {
        Ok(self.hits.iter().find(|h| h.external_id.as_deref() == Some(id)).cloned())
    }
}

pub fn book_hit(title: &str, id: &str, isbn13: &str) -> NewMedia {
    NewMedia {
        media_type: Some(MediaType::Book),
        title: title.into(),
        authors: vec!["Testautor".into()],
        isbn13: Some(isbn13.into()),
        source: "stubbook".into(),
        external_id: Some(id.into()),
        ..Default::default()
    }
}

pub fn movie_hit(title: &str, id: &str) -> NewMedia {
    NewMedia {
        media_type: Some(MediaType::Movie),
        title: title.into(),
        authors: vec!["Testregie".into()],
        narrators: vec!["Testdarstellerin".into()],
        source: "stubmovie".into(),
        external_id: Some(id.into()),
        ..Default::default()
    }
}

pub fn series_hit(title: &str, id: &str) -> NewMedia {
    NewMedia {
        media_type: Some(MediaType::Series),
        title: title.into(),
        authors: vec!["Testidee".into()],
        narrators: vec!["Testdarsteller".into()],
        publisher: Some("Testsender".into()),
        source: "stubseries".into(),
        external_id: Some(id.into()),
        ..Default::default()
    }
}

pub fn audio_hit(title: &str, asin: &str) -> NewMedia {
    NewMedia {
        media_type: Some(MediaType::Audiobook),
        title: title.into(),
        authors: vec!["Testautorin".into()],
        narrators: vec!["Testsprecher".into()],
        asin: Some(asin.into()),
        source: "stubaudio".into(),
        external_id: Some(asin.into()),
        ..Default::default()
    }
}

pub fn config(dir: &std::path::Path, issuer: &str) -> Config {
    let mut m: HashMap<&str, String> = HashMap::new();
    m.insert("MEDIENLISTE_BASE_URL", ORIGIN.into());
    m.insert("MEDIENLISTE_DATA_DIR", dir.display().to_string());
    m.insert("MEDIENLISTE_OIDC_ISSUER", issuer.into());
    m.insert("MEDIENLISTE_OIDC_CLIENT_ID", "medienliste".into());
    m.insert("MEDIENLISTE_OIDC_CLIENT_SECRET", "geheim".into());
    m.insert("MEDIENLISTE_OIDC_ADMIN_GROUP", "medienliste-admins".into());
    m.insert("MEDIENLISTE_OIDC_REQUIRED_GROUP", "medienliste-users".into());
    m.insert("MEDIENLISTE_BACKUP_INTERVAL_HOURS", "0".into());
    Config::from_lookup(|k| m.get(k).cloned()).unwrap()
}

pub async fn harness() -> Harness {
    harness_with("https://idp.invalid", default_providers()).await
}

pub fn default_providers() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(StubProvider {
            kind: MediaType::Book,
            hits: vec![
                book_hit("Der Schwarm", "b1", "9783596164530"),
                book_hit("Die Vermessung der Welt", "b2", "9783499241000"),
                book_hit("Momo", "b3", "9783522202107"),
            ],
        }),
        Arc::new(StubProvider {
            kind: MediaType::Audiobook,
            hits: vec![audio_hit("Der Schwarm", "B004V0L2VG")],
        }),
        Arc::new(StubProvider { kind: MediaType::Movie, hits: vec![movie_hit("Lola rennt", "104")] }),
        Arc::new(StubProvider { kind: MediaType::Series, hits: vec![series_hit("Dark", "70523")] }),
    ]
}

pub async fn harness_with(issuer: &str, providers: Vec<Arc<dyn Provider>>) -> Harness {
    harness_with_config(issuer, providers, |_| {}).await
}

/// Wie [`harness_with`], aber die Konfiguration kann vorher angepasst werden.
pub async fn harness_with_config(
    issuer: &str,
    providers: Vec<Arc<dyn Provider>>,
    adjust: impl FnOnce(&mut Config),
) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), issuer);
    adjust(&mut cfg);
    let cfg = Arc::new(cfg);
    let pool = db::connect_memory().await.unwrap();
    let state = AppState {
        cfg: cfg.clone(),
        pool: pool.clone(),
        providers: Arc::new(Registry::new(providers)),
        covers: CoverStore::new(cfg.covers_dir()),
        oidc: Arc::new(auth::oidc::OidcService::new(cfg.oidc.clone()).unwrap()),
    };
    Harness { app: web::router(state), pool, cfg, _dir: dir }
}

impl Harness {
    pub async fn user(&self, sub: &str, admin: bool) -> TestUser {
        let u = repo::users::upsert_login(
            &self.pool,
            sub,
            &format!("Name {sub}"),
            &format!("{sub}@example.org"),
            admin,
        )
        .await
        .unwrap();
        let set_cookie = auth::create_session(&self.pool, &self.cfg, u.id).await.unwrap();
        let token = set_cookie.split(';').next().unwrap().split_once('=').unwrap().1.to_string();
        let csrf: String = sqlx::query_scalar("SELECT csrf_token FROM sessions WHERE token_hash = ?1")
            .bind(hash_token(&token))
            .fetch_one(&self.pool)
            .await
            .unwrap();
        TestUser { id: u.id, cookie: format!("{}={token}", auth::SESSION_COOKIE), csrf }
    }

    pub async fn request(&self, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8_lossy(&body).to_string())
    }

    pub async fn get(&self, path: &str, u: Option<&TestUser>) -> (StatusCode, HeaderMap, String) {
        let mut b = Request::get(path);
        if let Some(u) = u {
            b = b.header(header::COOKIE, &u.cookie);
        }
        self.request(b.body(Body::empty()).unwrap()).await
    }

    pub async fn get_htmx(&self, path: &str, u: &TestUser) -> (StatusCode, HeaderMap, String) {
        let req = Request::get(path)
            .header(header::COOKIE, &u.cookie)
            .header("hx-request", "true")
            .body(Body::empty())
            .unwrap();
        self.request(req).await
    }

    /// POST eines Formulars; `csrf` wird automatisch mit dem Token des Benutzers ergänzt.
    pub async fn post(
        &self,
        path: &str,
        u: Option<&TestUser>,
        form: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, String) {
        let mut pairs: Vec<(String, String)> =
            form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if let Some(u) = u
            && !form.iter().any(|(k, _)| *k == "csrf")
        {
            pairs.push(("csrf".into(), u.csrf.clone()));
        }
        self.post_raw(path, u.map(|u| u.cookie.as_str()), Some(ORIGIN), &pairs, false).await
    }

    pub async fn post_htmx(
        &self,
        path: &str,
        u: &TestUser,
        form: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, String) {
        let mut pairs: Vec<(String, String)> =
            form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        pairs.push(("csrf".into(), u.csrf.clone()));
        self.post_raw(path, Some(&u.cookie), Some(ORIGIN), &pairs, true).await
    }

    pub async fn post_raw(
        &self,
        path: &str,
        cookie: Option<&str>,
        origin: Option<&str>,
        pairs: &[(String, String)],
        htmx: bool,
    ) -> (StatusCode, HeaderMap, String) {
        let body = pairs
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let mut b = Request::post(path).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        if htmx {
            b = b.header("hx-request", "true");
        }
        self.request(b.body(Body::from(body)).unwrap()).await
    }

    /// Legt ein Medium direkt in der Datenbank an.
    pub async fn media(&self, m: &NewMedia, created_by: Option<i64>) -> i64 {
        repo::media::insert(&self.pool, m, None, created_by).await.unwrap()
    }

    pub async fn entry_status(&self, user_id: i64, media_id: i64) -> Option<(String, String)> {
        sqlx::query_as(
            "SELECT status, status_changed_at FROM user_media WHERE user_id = ?1 AND media_id = ?2",
        )
        .bind(user_id)
        .bind(media_id)
        .fetch_optional(&self.pool)
        .await
        .unwrap()
    }
}
