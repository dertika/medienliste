use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use medienliste::{config::Config, db, repo, web};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "Verwendung: medienliste [serve | backup [ZIELDATEI] | migrate | healthcheck]";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("MEDIENLISTE_LOG").unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("serve") {
        "serve" => serve().await,
        "backup" => backup(args.get(1).map(PathBuf::from)).await,
        "migrate" => {
            let cfg = Config::from_env()?;
            medienliste::create_private_dir(&cfg.data_dir)?;
            let pool = db::connect(&cfg.db_path()).await?;
            db::migrate(&pool).await?;
            println!("Migrationen angewendet.");
            Ok(())
        }
        "healthcheck" => healthcheck().await,
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => bail!("Unbekannter Befehl: {other}\n{USAGE}"),
    }
}

async fn serve() -> Result<()> {
    let cfg = Config::from_env()?;
    let listen = cfg.listen;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        base_url = %cfg.base_url,
        data_dir = %cfg.data_dir.display(),
        "Medienliste startet"
    );
    let state = medienliste::build_state(cfg).await?;

    // Hintergrundaufgaben: abgelaufene Sitzungen aufräumen, regelmäßige Backups.
    {
        let pool = state.pool.clone();
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_secs(3600));
            loop {
                iv.tick().await;
                if let Err(e) = repo::sessions::purge_expired(&pool).await {
                    tracing::warn!("Aufräumen der Sitzungen fehlgeschlagen: {e}");
                }
            }
        });
    }
    if state.cfg.backup_interval_hours > 0 {
        let pool = state.pool.clone();
        let dir = state.cfg.backups_dir();
        let keep = state.cfg.backup_keep.max(1);
        let every = Duration::from_secs(state.cfg.backup_interval_hours * 3600);
        tokio::spawn(async move {
            let mut iv =
                tokio::time::interval_at(tokio::time::Instant::now() + Duration::from_secs(60), every);
            loop {
                iv.tick().await;
                match db::backup_rotating(&pool, &dir, keep).await {
                    Ok(p) => tracing::info!("Backup erstellt: {}", p.display()),
                    Err(e) => tracing::error!("Backup fehlgeschlagen: {e:#}"),
                }
            }
        });
    }

    let app = web::router(state);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("Kann nicht auf {listen} lauschen"))?;
    tracing::info!("Lausche auf {listen}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("Beende …");
}

/// Konsistentes Backup der laufenden Datenbank (VACUUM INTO).
async fn backup(dest: Option<PathBuf>) -> Result<()> {
    let cfg = Config::from_env_for_tools()?;
    let pool = db::connect(&cfg.db_path()).await?;
    let path = match dest {
        Some(d) => {
            db::backup(&pool, &d).await?;
            d
        }
        None => db::backup_rotating(&pool, &cfg.backups_dir(), cfg.backup_keep.max(1)).await?,
    };
    println!("Backup erstellt: {}", path.display());
    Ok(())
}

/// Für HEALTHCHECK im Container (kein curl im Image nötig).
async fn healthcheck() -> Result<()> {
    let listen: SocketAddr =
        std::env::var("MEDIENLISTE_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".into()).parse()?;
    let base = std::env::var("MEDIENLISTE_BASE_URL")
        .ok()
        .and_then(|u| url::Url::parse(&u).ok())
        .map(|u| u.path().trim_end_matches('/').to_string())
        .unwrap_or_default();
    let url = format!("http://127.0.0.1:{}{base}/readyz", listen.port());
    let resp = reqwest::Client::new().get(&url).timeout(Duration::from_secs(5)).send().await?;
    if !resp.status().is_success() {
        bail!("ungesund: {}", resp.status());
    }
    Ok(())
}
