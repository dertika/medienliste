//! Datenbankzugriff.
//!
//! Alle Funktionen, die persönliche Daten betreffen (Modul [`entries`]), verlangen die
//! `user_id` des angemeldeten Benutzers als ersten Parameter und filtern jede Abfrage
//! danach. Die `user_id` stammt immer aus der Sitzung, nie aus der Anfrage.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};

use crate::models::{Entry, Media, MediaType, NewMedia, Status, User, normalize};

pub type DbResult<T> = Result<T, sqlx::Error>;

fn now() -> DateTime<Utc> {
    Utc::now()
}

pub mod users {
    use super::*;

    #[derive(FromRow)]
    struct Row {
        id: i64,
        sub: String,
        name: String,
        email: String,
        is_admin: bool,
    }

    impl From<Row> for User {
        fn from(r: Row) -> Self {
            User { id: r.id, sub: r.sub, name: r.name, email: r.email, is_admin: r.is_admin }
        }
    }

    /// Just-in-time-Provisioning: legt den Benutzer beim ersten Login an und
    /// aktualisiert Name, E-Mail und Admin-Flag bei jedem weiteren Login.
    pub async fn upsert_login(
        pool: &SqlitePool,
        sub: &str,
        name: &str,
        email: &str,
        is_admin: bool,
    ) -> DbResult<User> {
        let ts = now();
        let row: Row = sqlx::query_as(
            "INSERT INTO users (sub, name, email, is_admin, created_at, last_login_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(sub) DO UPDATE SET
                name = excluded.name,
                email = excluded.email,
                is_admin = excluded.is_admin,
                last_login_at = excluded.last_login_at
             RETURNING id, sub, name, email, is_admin",
        )
        .bind(sub)
        .bind(name)
        .bind(email)
        .bind(is_admin)
        .bind(ts)
        .fetch_one(pool)
        .await?;
        Ok(row.into())
    }

    #[derive(FromRow, Debug, Clone, serde::Serialize)]
    pub struct UserOverview {
        pub id: i64,
        pub name: String,
        pub email: String,
        pub is_admin: bool,
        pub last_login_at: DateTime<Utc>,
        pub entries: i64,
    }

    /// Übersicht für Admins: nur Stammdaten und Anzahl der Einträge, keine Inhalte.
    pub async fn overview(pool: &SqlitePool) -> DbResult<Vec<UserOverview>> {
        sqlx::query_as(
            "SELECT u.id, u.name, u.email, u.is_admin, u.last_login_at,
                    (SELECT COUNT(*) FROM user_media um WHERE um.user_id = u.id) AS entries
             FROM users u ORDER BY u.name COLLATE NOCASE",
        )
        .fetch_all(pool)
        .await
    }

    pub(crate) async fn by_session(pool: &SqlitePool, token_hash: &str) -> DbResult<Option<(User, String)>> {
        #[derive(FromRow)]
        struct SessionUserRow {
            id: i64,
            sub: String,
            name: String,
            email: String,
            is_admin: bool,
            csrf_token: String,
        }
        let r: Option<SessionUserRow> = sqlx::query_as(
            "SELECT u.id, u.sub, u.name, u.email, u.is_admin, s.csrf_token
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND s.expires_at > ?2",
        )
        .bind(token_hash)
        .bind(now())
        .fetch_optional(pool)
        .await?;
        Ok(r.map(|r| {
            (User { id: r.id, sub: r.sub, name: r.name, email: r.email, is_admin: r.is_admin }, r.csrf_token)
        }))
    }
}

pub mod sessions {
    use super::*;

    pub async fn create(
        pool: &SqlitePool,
        token_hash: &str,
        user_id: i64,
        csrf_token: &str,
        expires_at: DateTime<Utc>,
    ) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, csrf_token, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(token_hash)
        .bind(user_id)
        .bind(csrf_token)
        .bind(now())
        .bind(expires_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn lookup(pool: &SqlitePool, token_hash: &str) -> DbResult<Option<(User, String)>> {
        super::users::by_session(pool, token_hash).await
    }

    pub async fn delete(pool: &SqlitePool, token_hash: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?1").bind(token_hash).execute(pool).await?;
        Ok(())
    }

    /// Beendet alle Sitzungen des Benutzers mit diesem `sub` (z. B. nach Entzug der Berechtigung).
    pub async fn delete_for_sub(pool: &SqlitePool, sub: &str) -> DbResult<u64> {
        let r = sqlx::query("DELETE FROM sessions WHERE user_id IN (SELECT id FROM users WHERE sub = ?1)")
            .bind(sub)
            .execute(pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// Entfernt abgelaufene Sitzungen und alte Login-Zustände.
    pub async fn purge_expired(pool: &SqlitePool) -> DbResult<()> {
        sqlx::query("DELETE FROM sessions WHERE expires_at <= ?1").bind(now()).execute(pool).await?;
        sqlx::query("DELETE FROM login_states WHERE created_at <= ?1")
            .bind(now() - chrono::Duration::minutes(15))
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn save_login_state(
        pool: &SqlitePool,
        state: &str,
        pkce_verifier: &str,
        nonce: &str,
        return_to: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO login_states (state, pkce_verifier, nonce, return_to, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(state)
        .bind(pkce_verifier)
        .bind(nonce)
        .bind(return_to)
        .bind(now())
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Holt und löscht einen Login-Zustand (einmalig verwendbar, max. 15 Minuten alt).
    pub async fn take_login_state(
        pool: &SqlitePool,
        state: &str,
    ) -> DbResult<Option<(String, String, String)>> {
        sqlx::query_as(
            "DELETE FROM login_states WHERE state = ?1 AND created_at > ?2
             RETURNING pkce_verifier, nonce, return_to",
        )
        .bind(state)
        .bind(now() - chrono::Duration::minutes(15))
        .fetch_optional(pool)
        .await
    }
}

pub mod media {
    use super::*;

    pub(crate) const COLUMNS: &str = "m.id, m.media_type, m.title, m.subtitle, m.authors, m.narrators,
        m.description, m.isbn10, m.isbn13, m.asin, m.published_year, m.series, m.series_index,
        m.language, m.publisher, m.cover_file, m.source, m.external_id, m.created_by";

    #[derive(FromRow)]
    pub(crate) struct Row {
        id: i64,
        media_type: String,
        title: String,
        subtitle: Option<String>,
        authors: String,
        narrators: String,
        description: Option<String>,
        isbn10: Option<String>,
        isbn13: Option<String>,
        asin: Option<String>,
        published_year: Option<i64>,
        series: Option<String>,
        series_index: Option<String>,
        language: Option<String>,
        publisher: Option<String>,
        cover_file: Option<String>,
        source: String,
        external_id: Option<String>,
        created_by: Option<i64>,
    }

    impl From<Row> for Media {
        fn from(r: Row) -> Self {
            Media {
                id: r.id,
                media_type: MediaType::parse(&r.media_type).unwrap_or(MediaType::Book),
                title: r.title,
                subtitle: r.subtitle,
                authors: serde_json::from_str(&r.authors).unwrap_or_default(),
                narrators: serde_json::from_str(&r.narrators).unwrap_or_default(),
                description: r.description,
                isbn10: r.isbn10,
                isbn13: r.isbn13,
                asin: r.asin,
                published_year: r.published_year,
                series: r.series,
                series_index: r.series_index,
                language: r.language,
                publisher: r.publisher,
                cover_file: r.cover_file,
                source: r.source,
                external_id: r.external_id,
                created_by: r.created_by,
            }
        }
    }

    fn search_text(m: &NewMedia) -> String {
        let mut s = m.title.clone();
        for p in [&m.subtitle, &m.series, &m.isbn13, &m.isbn10, &m.asin].into_iter().flatten() {
            s.push(' ');
            s.push_str(p);
        }
        for a in m.authors.iter().chain(m.narrators.iter()) {
            s.push(' ');
            s.push_str(a);
        }
        normalize(&s)
    }

    pub async fn get(pool: &SqlitePool, id: i64) -> DbResult<Option<Media>> {
        let r: Option<Row> = sqlx::query_as(&format!("SELECT {COLUMNS} FROM media m WHERE m.id = ?1"))
            .bind(id)
            .fetch_optional(pool)
            .await?;
        Ok(r.map(Into::into))
    }

    /// Sucht ein bereits gespeichertes Medium mit gleicher Quelle/ID, ISBN-13 oder ASIN.
    pub async fn find_existing(pool: &SqlitePool, m: &NewMedia) -> DbResult<Option<i64>> {
        if let Some(ext) = &m.external_id {
            let id: Option<i64> =
                sqlx::query_scalar("SELECT id FROM media WHERE source = ?1 AND external_id = ?2")
                    .bind(&m.source)
                    .bind(ext)
                    .fetch_optional(pool)
                    .await?;
            if id.is_some() {
                return Ok(id);
            }
        }
        let kind = m.kind().as_str();
        if let Some(isbn) = &m.isbn13 {
            let id: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM media WHERE isbn13 = ?1 AND media_type = ?2 ORDER BY id LIMIT 1",
            )
            .bind(isbn)
            .bind(kind)
            .fetch_optional(pool)
            .await?;
            if id.is_some() {
                return Ok(id);
            }
        }
        if let Some(asin) = &m.asin {
            let id: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM media WHERE asin = ?1 AND media_type = ?2 ORDER BY id LIMIT 1",
            )
            .bind(asin)
            .bind(kind)
            .fetch_optional(pool)
            .await?;
            if id.is_some() {
                return Ok(id);
            }
        }
        Ok(None)
    }

    pub async fn insert(
        pool: &SqlitePool,
        m: &NewMedia,
        cover_file: Option<&str>,
        created_by: Option<i64>,
    ) -> DbResult<i64> {
        let ts = now();
        sqlx::query_scalar(
            "INSERT INTO media (media_type, title, subtitle, authors, narrators, description,
                isbn10, isbn13, asin, published_year, series, series_index, language, publisher,
                cover_file, cover_source_url, source, external_id, search_text, created_by,
                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                ?18, ?19, ?20, ?21, ?21)
             RETURNING id",
        )
        .bind(m.kind().as_str())
        .bind(&m.title)
        .bind(&m.subtitle)
        .bind(serde_json::to_string(&m.authors).unwrap_or_else(|_| "[]".into()))
        .bind(serde_json::to_string(&m.narrators).unwrap_or_else(|_| "[]".into()))
        .bind(&m.description)
        .bind(&m.isbn10)
        .bind(&m.isbn13)
        .bind(&m.asin)
        .bind(m.published_year)
        .bind(&m.series)
        .bind(&m.series_index)
        .bind(&m.language)
        .bind(&m.publisher)
        .bind(cover_file)
        .bind(&m.cover_url)
        .bind(&m.source)
        .bind(&m.external_id)
        .bind(search_text(m))
        .bind(created_by)
        .bind(ts)
        .fetch_one(pool)
        .await
    }

    /// Aktualisiert die Metadaten (Quelle und externe ID bleiben unverändert).
    pub async fn update(pool: &SqlitePool, id: i64, m: &NewMedia) -> DbResult<()> {
        sqlx::query(
            "UPDATE media SET media_type = ?2, title = ?3, subtitle = ?4, authors = ?5,
                narrators = ?6, description = ?7, isbn10 = ?8, isbn13 = ?9, asin = ?10,
                published_year = ?11, series = ?12, series_index = ?13, language = ?14,
                publisher = ?15, search_text = ?16, updated_at = ?17
             WHERE id = ?1",
        )
        .bind(id)
        .bind(m.kind().as_str())
        .bind(&m.title)
        .bind(&m.subtitle)
        .bind(serde_json::to_string(&m.authors).unwrap_or_else(|_| "[]".into()))
        .bind(serde_json::to_string(&m.narrators).unwrap_or_else(|_| "[]".into()))
        .bind(&m.description)
        .bind(&m.isbn10)
        .bind(&m.isbn13)
        .bind(&m.asin)
        .bind(m.published_year)
        .bind(&m.series)
        .bind(&m.series_index)
        .bind(&m.language)
        .bind(&m.publisher)
        .bind(search_text(m))
        .bind(now())
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn set_cover(pool: &SqlitePool, id: i64, cover_file: Option<&str>) -> DbResult<()> {
        sqlx::query("UPDATE media SET cover_file = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(cover_file)
            .bind(now())
            .execute(pool)
            .await?;
        Ok(())
    }

    pub async fn delete(pool: &SqlitePool, id: i64) -> DbResult<bool> {
        let r = sqlx::query("DELETE FROM media WHERE id = ?1").bind(id).execute(pool).await?;
        Ok(r.rows_affected() > 0)
    }

    /// Wird eine Cover-Datei noch von einem anderen Medium verwendet?
    pub async fn cover_in_use(pool: &SqlitePool, cover_file: &str) -> DbResult<bool> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE cover_file = ?1")
            .bind(cover_file)
            .fetch_one(pool)
            .await?;
        Ok(n > 0)
    }

    /// Durchsucht die globale Medienbibliothek (alle Terme müssen vorkommen).
    pub async fn search_local(
        pool: &SqlitePool,
        q: &str,
        kind: Option<MediaType>,
        limit: i64,
    ) -> DbResult<Vec<Media>> {
        let terms: Vec<String> =
            normalize(q).split(' ').filter(|t| !t.is_empty()).map(String::from).collect();
        if terms.is_empty() {
            return Ok(vec![]);
        }
        let mut sql = format!("SELECT {COLUMNS} FROM media m WHERE 1=1");
        for _ in &terms {
            sql.push_str(" AND instr(m.search_text, ?) > 0");
        }
        if kind.is_some() {
            sql.push_str(" AND m.media_type = ?");
        }
        sql.push_str(" ORDER BY m.title COLLATE NOCASE LIMIT ?");
        let mut query = sqlx::query_as::<_, Row>(&sql);
        for t in &terms {
            query = query.bind(t);
        }
        if let Some(k) = kind {
            query = query.bind(k.as_str());
        }
        let rows = query.bind(limit).fetch_all(pool).await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}

pub mod entries {
    //! Persönliche Einträge. Jede Funktion ist auf `user_id` beschränkt.
    use super::*;
    use std::collections::BTreeMap;

    #[derive(FromRow)]
    struct EntryRow {
        status: String,
        status_changed_at: DateTime<Utc>,
        added_at: DateTime<Utc>,
        done_year: Option<i32>,
    }

    impl From<EntryRow> for Entry {
        fn from(r: EntryRow) -> Self {
            Entry {
                status: Status::parse(&r.status).unwrap_or(Status::Wanted),
                status_changed_at: r.status_changed_at,
                added_at: r.added_at,
                done_year: r.done_year,
            }
        }
    }

    pub async fn get(pool: &SqlitePool, user_id: i64, media_id: i64) -> DbResult<Option<Entry>> {
        let r: Option<EntryRow> = sqlx::query_as(
            "SELECT status, status_changed_at, added_at, done_year FROM user_media
             WHERE user_id = ?1 AND media_id = ?2",
        )
        .bind(user_id)
        .bind(media_id)
        .fetch_optional(pool)
        .await?;
        Ok(r.map(Into::into))
    }

    /// Setzt den Status. Änderungsdatum und Jahr werden nur aktualisiert, wenn sich der
    /// Status ändert: Beim Wechsel auf „done“ wird `current_year` als Jahr vorbelegt,
    /// bei „wanted“ wird das Jahr entfernt.
    pub async fn set_status(
        pool: &SqlitePool,
        user_id: i64,
        media_id: i64,
        status: Status,
        current_year: i32,
    ) -> DbResult<()> {
        let year = (status == Status::Done).then_some(current_year);
        sqlx::query(
            "INSERT INTO user_media (user_id, media_id, status, status_changed_at, added_at, done_year)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)
             ON CONFLICT(user_id, media_id) DO UPDATE SET
                status_changed_at = CASE WHEN user_media.status = excluded.status
                                         THEN user_media.status_changed_at
                                         ELSE excluded.status_changed_at END,
                done_year = CASE WHEN user_media.status = excluded.status
                                 THEN user_media.done_year
                                 ELSE excluded.done_year END,
                status = excluded.status",
        )
        .bind(user_id)
        .bind(media_id)
        .bind(status.as_str())
        .bind(now())
        .bind(year)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Ändert das Jahr eines eigenen, abgeschlossenen Eintrags. `false`, wenn es keinen gibt.
    pub async fn set_done_year(
        pool: &SqlitePool,
        user_id: i64,
        media_id: i64,
        year: Option<i32>,
    ) -> DbResult<bool> {
        let r = sqlx::query(
            "UPDATE user_media SET done_year = ?3
             WHERE user_id = ?1 AND media_id = ?2 AND status = 'done'",
        )
        .bind(user_id)
        .bind(media_id)
        .bind(year)
        .execute(pool)
        .await?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn remove(pool: &SqlitePool, user_id: i64, media_id: i64) -> DbResult<bool> {
        let r = sqlx::query("DELETE FROM user_media WHERE user_id = ?1 AND media_id = ?2")
            .bind(user_id)
            .bind(media_id)
            .execute(pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Sort {
        #[default]
        Changed,
        Title,
        Author,
        Added,
        /// Nach dem Jahr „gelesen/gehört/gesehen“, neueste zuerst; Merkliste danach.
        DoneYear,
    }

    impl Sort {
        pub fn parse(s: &str) -> Self {
            match s {
                "title" => Sort::Title,
                "author" => Sort::Author,
                "added" => Sort::Added,
                "done" => Sort::DoneYear,
                _ => Sort::Changed,
            }
        }
        pub fn as_str(self) -> &'static str {
            match self {
                Sort::Changed => "changed",
                Sort::Title => "title",
                Sort::Author => "author",
                Sort::Added => "added",
                Sort::DoneYear => "done",
            }
        }
        fn order_by(self) -> &'static str {
            match self {
                Sort::Changed => "um.status_changed_at DESC, m.title COLLATE NOCASE",
                Sort::Title => "m.title COLLATE NOCASE, m.id",
                Sort::Author => {
                    "m.authors COLLATE NOCASE, m.series COLLATE NOCASE, \
                                 CAST(m.series_index AS REAL), m.title COLLATE NOCASE"
                }
                Sort::Added => "um.added_at DESC, m.id DESC",
                Sort::DoneYear => {
                    "um.status <> 'done', um.done_year IS NULL, um.done_year DESC, \
                                 um.status_changed_at DESC, m.title COLLATE NOCASE"
                }
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct ListFilter {
        pub media_type: Option<MediaType>,
        pub status: Option<Status>,
        pub q: Option<String>,
        pub sort: Sort,
        pub limit: i64,
        pub offset: i64,
    }

    /// Liefert die Einträge des Benutzers samt Medium und die Gesamtanzahl für die Filter.
    pub async fn list(
        pool: &SqlitePool,
        user_id: i64,
        f: &ListFilter,
    ) -> DbResult<(Vec<(Media, Entry)>, i64)> {
        let terms: Vec<String> =
            f.q.as_deref()
                .map(normalize)
                .unwrap_or_default()
                .split(' ')
                .filter(|t| !t.is_empty())
                .map(String::from)
                .collect();

        let mut where_sql = String::from("um.user_id = ?");
        if f.media_type.is_some() {
            where_sql.push_str(" AND m.media_type = ?");
        }
        if f.status.is_some() {
            where_sql.push_str(" AND um.status = ?");
        }
        for _ in &terms {
            where_sql.push_str(" AND instr(m.search_text, ?) > 0");
        }

        macro_rules! bind_filters {
            ($q:expr) => {{
                let mut q = $q.bind(user_id);
                if let Some(t) = f.media_type {
                    q = q.bind(t.as_str());
                }
                if let Some(s) = f.status {
                    q = q.bind(s.as_str());
                }
                for t in &terms {
                    q = q.bind(t.clone());
                }
                q
            }};
        }

        let count_sql = format!(
            "SELECT COUNT(*) FROM user_media um JOIN media m ON m.id = um.media_id WHERE {where_sql}"
        );
        let total: i64 = bind_filters!(sqlx::query_scalar(&count_sql)).fetch_one(pool).await?;

        let sql = format!(
            "SELECT {cols}, um.status AS e_status, um.status_changed_at AS e_changed,
                    um.added_at AS e_added, um.done_year AS e_year
             FROM user_media um JOIN media m ON m.id = um.media_id
             WHERE {where_sql} ORDER BY {order} LIMIT ? OFFSET ?",
            cols = media::COLUMNS,
            order = f.sort.order_by()
        );
        let rows = bind_filters!(sqlx::query(&sql))
            .bind(if f.limit > 0 { f.limit } else { -1 })
            .bind(f.offset)
            .fetch_all(pool)
            .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            use sqlx::Row as _;
            let m: media::Row = media::Row::from_row(&row)?;
            let e = Entry {
                status: Status::parse(row.try_get::<String, _>("e_status")?.as_str())
                    .unwrap_or(Status::Wanted),
                status_changed_at: row.try_get("e_changed")?,
                added_at: row.try_get("e_added")?,
                done_year: row.try_get("e_year")?,
            };
            out.push((m.into(), e));
        }
        Ok((out, total))
    }

    /// Prüft für ein Suchergebnis, ob der Benutzer es schon in seiner Liste hat.
    pub async fn lookup(
        pool: &SqlitePool,
        user_id: i64,
        m: &NewMedia,
    ) -> DbResult<Option<(i64, Option<Entry>)>> {
        let Some(media_id) = media::find_existing(pool, m).await? else {
            return Ok(None);
        };
        Ok(Some((media_id, get(pool, user_id, media_id).await?)))
    }

    /// Alle Einträge (für Statistik) als (Typ, Status, Änderungsdatum).
    /// Alle Einträge (für Statistik) als (Typ, Status, Jahr).
    pub async fn all_for_stats(
        pool: &SqlitePool,
        user_id: i64,
    ) -> DbResult<Vec<(MediaType, Status, Option<i32>)>> {
        let rows: Vec<(String, String, Option<i32>)> = sqlx::query_as(
            "SELECT m.media_type, um.status, um.done_year
             FROM user_media um JOIN media m ON m.id = um.media_id
             WHERE um.user_id = ?1",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(t, s, y)| Some((MediaType::parse(&t)?, Status::parse(&s)?, y)))
            .collect())
    }

    /// Zähler pro Medientyp, Reihenfolge wie [`MediaType::ALL`].
    pub type PerType = [i64; MediaType::ALL.len()];

    /// Statistik: abgeschlossene Medien pro Jahr (nach dem hinterlegten Jahr) und offene Wünsche.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct Stats {
        /// Jahr → Anzahl pro Typ, absteigend sortiert; `None` = ohne Jahr (am Ende).
        pub per_year: Vec<(Option<i32>, PerType)>,
        pub done: PerType,
        pub wanted: PerType,
    }

    pub async fn stats(pool: &SqlitePool, user_id: i64) -> DbResult<Stats> {
        let mut per_year: BTreeMap<std::cmp::Reverse<Option<i32>>, PerType> = BTreeMap::new();
        let mut s = Stats::default();
        for (t, st, year) in all_for_stats(pool, user_id).await? {
            match st {
                Status::Done => {
                    s.done[t.index()] += 1;
                    per_year.entry(std::cmp::Reverse(year)).or_default()[t.index()] += 1;
                }
                Status::Wanted => s.wanted[t.index()] += 1,
            }
        }
        // Reverse sortiert Some(Jahr) absteigend und None (kleinster Wert) ans Ende.
        s.per_year = per_year.into_iter().map(|(y, c)| (y.0, c)).collect();
        Ok(s)
    }
}
