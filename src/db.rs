//! SQLite-Verbindung, Migrationen und Backup.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Öffnet (bzw. erzeugt) die Datenbank im WAL-Modus.
pub async fn connect(path: &Path) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str("sqlite://")?
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(10));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await
        .with_context(|| format!("Datenbank {} kann nicht geöffnet werden", path.display()))?;
    Ok(pool)
}

/// In-Memory-Datenbank für Tests (eine Verbindung, damit alle dieselbe DB sehen).
pub async fn connect_memory() -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    MIGRATOR.run(pool).await.context("Migration fehlgeschlagen")?;
    Ok(())
}

/// Erstellt eine konsistente Kopie der laufenden Datenbank mit `VACUUM INTO`.
/// Die Datei wird zuerst unter einem temporären Namen geschrieben und dann umbenannt.
pub async fn backup(pool: &SqlitePool, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        crate::create_private_dir(parent)?;
    }
    let tmp = dest.with_extension("tmp");
    let _ = tokio::fs::remove_file(&tmp).await;
    sqlx::query("VACUUM INTO ?")
        .bind(tmp.to_string_lossy().as_ref())
        .execute(pool)
        .await
        .context("VACUUM INTO fehlgeschlagen")?;
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

/// Legt ein Backup mit Zeitstempel im Backup-Verzeichnis an und entfernt alte Backups.
pub async fn backup_rotating(pool: &SqlitePool, dir: &Path, keep: usize) -> Result<PathBuf> {
    let name = format!("medienliste-{}.sqlite3", Utc::now().format("%Y%m%d-%H%M%S"));
    let dest = dir.join(name);
    backup(pool, &dest).await?;

    let mut existing = Vec::new();
    let mut rd = tokio::fs::read_dir(dir).await?;
    while let Some(e) = rd.next_entry().await? {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with("medienliste-") && n.ends_with(".sqlite3") {
            existing.push(e.path());
        }
    }
    existing.sort();
    if existing.len() > keep {
        for old in &existing[..existing.len() - keep] {
            let _ = tokio::fs::remove_file(old).await;
        }
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wal_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("t.sqlite3")).await.unwrap();
        migrate(&pool).await.unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode").fetch_one(&pool).await.unwrap();
        assert_eq!(mode, "wal");

        let bdir = dir.path().join("backups");
        for _ in 0..3 {
            backup_rotating(&pool, &bdir, 2).await.unwrap();
            tokio::time::sleep(Duration::from_millis(1100)).await;
        }
        let count = std::fs::read_dir(&bdir).unwrap().count();
        assert_eq!(count, 2);

        let latest = std::fs::read_dir(&bdir).unwrap().next().unwrap().unwrap().path();
        let copy = connect(&latest).await.unwrap();
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations").fetch_one(&copy).await.unwrap();
        assert!(n >= 1);
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    /// 0002 baut `media` neu auf; bestehende Medien und persönliche Einträge müssen erhalten bleiben.
    #[tokio::test]
    async fn series_migration_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("m.sqlite3")).await.unwrap();
        // Nur die erste Migration anwenden (Stand vor Serien).
        let mut only_first = sqlx::migrate::Migrator::new(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/migrations"
        )))
        .await
        .unwrap();
        only_first.migrations = std::borrow::Cow::Owned(vec![MIGRATOR.migrations[0].clone()]);
        only_first.run(&pool).await.unwrap();

        let now = "2026-01-01T00:00:00+00:00";
        sqlx::query("INSERT INTO users (id, sub, created_at, last_login_at) VALUES (1, 's', ?1, ?1)")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO media (id, media_type, title, source, created_at, updated_at)
             VALUES (7, 'movie', 'Lola rennt', 'manual', ?1, ?1)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO user_media (user_id, media_id, status, status_changed_at, added_at, done_year)
             VALUES (1, 7, 'done', ?1, ?1, 2019)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        migrate(&pool).await.unwrap();

        let (title, year): (String, i64) = sqlx::query_as(
            "SELECT m.title, um.done_year FROM user_media um JOIN media m ON m.id = um.media_id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((title.as_str(), year), ("Lola rennt", 2019));
        // Neuer Typ erlaubt, Fremdschlüssel wieder aktiv und konsistent.
        sqlx::query(
            "INSERT INTO media (media_type, title, source, created_at, updated_at)
             VALUES ('series', 'Dark', 'manual', ?1, ?1)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys").fetch_one(&pool).await.unwrap();
        assert_eq!(fk, 1);
        let violations: Vec<(String,)> =
            sqlx::query_as("PRAGMA foreign_key_check").fetch_all(&pool).await.unwrap();
        assert!(violations.is_empty());
        // Löschen kaskadiert weiterhin.
        sqlx::query("DELETE FROM media WHERE id = 7").execute(&pool).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_media").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 0);
    }
}
