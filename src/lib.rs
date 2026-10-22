//! Medienliste – selbst gehostete Webapp zum Tracken von Büchern, Hörbüchern und Filmen.

pub mod auth;
pub mod config;
pub mod covers;
pub mod db;
pub mod i18n;
pub mod models;
pub mod providers;
pub mod repo;
pub mod web;

pub const USER_AGENT: &str = concat!("Medienliste/", env!("CARGO_PKG_VERSION"), "");

use std::sync::Arc;

/// Legt ein Verzeichnis an, das nur für den eigenen Benutzer (und die Gruppe lesbar) ist –
/// unabhängig von der umask (z. B. bei `podman exec`).
pub fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o750);
    b.create(path)
}

/// Baut den kompletten App-Zustand aus der Konfiguration.
pub async fn build_state(cfg: config::Config) -> anyhow::Result<web::AppState> {
    create_private_dir(&cfg.data_dir)?;
    create_private_dir(&cfg.covers_dir())?;
    let pool = db::connect(&cfg.db_path()).await?;
    db::migrate(&pool).await?;
    let http = providers::http_client();
    web::set_base_path(cfg.base_path());
    Ok(web::AppState {
        providers: Arc::new(providers::Registry::from_config(&cfg, http)),
        covers: covers::CoverStore::new(cfg.covers_dir()),
        oidc: Arc::new(auth::oidc::OidcService::new(cfg.oidc.clone())?),
        pool,
        cfg: Arc::new(cfg),
    })
}
