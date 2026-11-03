//! Seiten-Handler. Persönliche Daten werden ausschließlich über `cu.id()`
//! (Benutzer aus der Sitzung) gelesen und geschrieben.

use askama::Template;
use axum::Form;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;

use super::views::{
    Card, Ctx, ListItem, StatusView, cover_url, render, render_status, source_label, type_icon,
};
use super::{AppError, AppState};
use crate::auth::{CurrentUser, require_admin, safe_return_to};
use crate::models::{Media, MediaType, NewMedia, Status, clean_isbn, isbn10_to_13};
use crate::repo::entries::Done;
use crate::repo::{self, entries, media};

const PAGE_SIZE: i64 = 60;
const SEARCH_LIMIT: usize = 20;

/// htmx-Anfrage, die nur ein Fragment erwartet (nicht bei der Wiederherstellung
/// des Verlaufs, dort wird die ganze Seite benötigt).
fn is_htmx(h: &HeaderMap) -> bool {
    h.contains_key("hx-request") && !h.contains_key("hx-history-restore-request")
}

/// Weiterleitung zu einem (geprüften) lokalen Ziel oder zum Fallback.
fn redirect_back(st: &AppState, return_to: &str, fallback: &str) -> Response {
    let r = safe_return_to(return_to);
    let target = if r.is_empty() { fallback.to_string() } else { r };
    Redirect::to(&format!("{}{}", st.cfg.base_path(), target)).into_response()
}

/// Heutiges Datum in der konfigurierten Zeitzone.
fn today(st: &AppState) -> NaiveDate {
    chrono::Utc::now().with_timezone(&st.cfg.timezone).date_naive()
}

fn can_edit(cu: &CurrentUser, m: &Media) -> bool {
    cu.user.is_admin || (m.source == "manual" && m.created_by == Some(cu.id()))
}

pub async fn home(State(st): State<AppState>, _cu: CurrentUser) -> Response {
    Redirect::to(&format!("{}liste", st.cfg.base_path())).into_response()
}

// ----- Suche -----

#[derive(Deserialize, Default)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    typ: String,
}

#[derive(Template)]
#[template(path = "search.html")]
struct SearchPage {
    ctx: Ctx,
    q: String,
    typ: &'static str,
    cards: Vec<Card>,
    searched: bool,
    warning: String,
    /// Quellenhinweis für TMDb (Pflicht laut Nutzungsbedingungen).
    attribution: bool,
}

#[derive(Template)]
#[template(path = "_results.html")]
struct SearchResults {
    ctx: Ctx,
    q: String,
    cards: Vec<Card>,
    searched: bool,
    warning: String,
    /// Quellenhinweis für TMDb (Pflicht laut Nutzungsbedingungen).
    attribution: bool,
}

pub async fn search(
    State(st): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    Query(sq): Query<SearchQuery>,
) -> Result<Response, AppError> {
    let ctx = Ctx::new(&st, Some(&cu), "search");
    let kind = MediaType::parse(&sq.typ).unwrap_or(MediaType::Book);
    let q = sq.q.trim().chars().take(200).collect::<String>();
    let return_to = format!("suche?typ={}&q={}", kind.as_str(), urlencoding::encode(&q));

    let mut cards = Vec::new();
    let mut warning = String::new();
    if !q.is_empty() {
        // 1. Treffer aus der gemeinsamen Bibliothek (inkl. manuell angelegter Medien).
        let local = media::search_local(&st.pool, &q, Some(kind), SEARCH_LIMIT as i64).await?;
        let mut seen: Vec<i64> = Vec::new();
        for m in &local {
            let e = entries::get(&st.pool, cu.id(), m.id).await?;
            seen.push(m.id);
            cards.push(Card::from_media(&ctx, m, e.as_ref(), &return_to));
        }
        // 2. Treffer der Provider, abgeglichen mit der eigenen Liste.
        let outcome = st.providers.search(kind, &q, SEARCH_LIMIT).await;
        for h in &outcome.hits {
            let known = entries::lookup(&st.pool, cu.id(), h).await?;
            if let Some((id, _)) = &known {
                if seen.contains(id) {
                    continue;
                }
                seen.push(*id);
            }
            cards.push(Card::from_hit(&ctx, h, known, &return_to));
        }
        if outcome.no_provider {
            warning = ctx.t("search-no-provider");
        } else if !outcome.failed.is_empty() && outcome.hits.is_empty() {
            warning = ctx.t("search-provider-error");
        }
        // Eigene Einträge zuerst – „Habe ich das schon?“ auf einen Blick.
        cards.sort_by_key(|c| c.status.status.is_none());
    }

    let attribution = kind.is_screen() && st.providers.all().any(|p| p.id().starts_with("tmdb"));
    if is_htmx(&headers) {
        return Ok(render(&SearchResults { ctx, q, cards, searched: true, warning, attribution }));
    }
    let searched = !q.is_empty();
    Ok(render(&SearchPage { ctx, q, typ: kind.as_str(), cards, searched, warning, attribution }))
}

#[derive(Deserialize)]
pub struct AddForm {
    #[serde(default)]
    csrf: String,
    source: String,
    external_id: String,
    status: String,
    #[serde(default)]
    return_to: String,
    /// Gesetzt auf der Vorschau-Detailseite: danach zur echten Detailseite.
    #[serde(default)]
    show_remove: String,
}

#[derive(Template)]
#[template(path = "_status.html")]
struct StatusPartial {
    ctx: Ctx,
    s: StatusView,
}

async fn status_response(
    st: &AppState,
    cu: &CurrentUser,
    headers: &HeaderMap,
    media_id: i64,
    return_to: &str,
    show_remove: bool,
) -> Result<Response, AppError> {
    if !is_htmx(headers) {
        return Ok(redirect_back(st, return_to, &format!("medium/{media_id}")));
    }
    let m = media::get(&st.pool, media_id).await?.ok_or(AppError::NotFound)?;
    let ctx = Ctx::new(st, Some(cu), "");
    let entry = entries::get(&st.pool, cu.id(), media_id).await?;
    let mut s = StatusView::new(&ctx, Some(media_id), m.media_type, entry.as_ref(), return_to);
    s.show_remove = show_remove;
    Ok(render(&StatusPartial { ctx, s }))
}

/// Legt ein Medium aus Provider-Metadaten an (oder findet das vorhandene).
async fn ensure_media(st: &AppState, hit: &NewMedia, user_id: i64) -> Result<i64, AppError> {
    if let Some(id) = media::find_existing(&st.pool, hit).await? {
        return Ok(id);
    }
    let cover = match &hit.cover_url {
        Some(u) => match st.covers.download(u).await {
            Ok(name) => Some(name),
            Err(e) => {
                tracing::info!("Cover konnte nicht geladen werden: {e:#}");
                None
            }
        },
        None => None,
    };
    match media::insert(&st.pool, hit, cover.as_deref(), Some(user_id)).await {
        Ok(id) => Ok(id),
        // Gleichzeitig angelegt: vorhandenes verwenden.
        Err(e) => match media::find_existing(&st.pool, hit).await? {
            Some(id) => Ok(id),
            None => Err(e.into()),
        },
    }
}

pub async fn add_from_search(
    State(st): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    Form(f): Form<AddForm>,
) -> Result<Response, AppError> {
    cu.check_csrf(&f.csrf)?;
    let status = Status::parse(&f.status).ok_or(AppError::BadRequest("error-badrequest-text"))?;
    let hit = st
        .providers
        .resolve(&f.source, &f.external_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or(AppError::NotFound)?;
    let media_id = ensure_media(&st, &hit, cu.id()).await?;
    entries::set_status(&st.pool, cu.id(), media_id, status, today(&st)).await?;
    if !f.show_remove.is_empty() {
        let target = format!("{}medium/{media_id}", st.cfg.base_path());
        if is_htmx(&headers) {
            return Ok(([("hx-redirect", target)], StatusCode::OK).into_response());
        }
        return Ok(Redirect::to(&target).into_response());
    }
    status_response(&st, &cu, &headers, media_id, &f.return_to, false).await
}

// ----- Status ändern / entfernen -----

#[derive(Deserialize)]
pub struct StatusForm {
    #[serde(default)]
    csrf: String,
    status: String,
    #[serde(default)]
    return_to: String,
    #[serde(default)]
    show_remove: String,
}

pub async fn set_status(
    State(st): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(f): Form<StatusForm>,
) -> Result<Response, AppError> {
    cu.check_csrf(&f.csrf)?;
    let status = Status::parse(&f.status).ok_or(AppError::BadRequest("error-badrequest-text"))?;
    media::get(&st.pool, id).await?.ok_or(AppError::NotFound)?;
    entries::set_status(&st.pool, cu.id(), id, status, today(&st)).await?;
    status_response(&st, &cu, &headers, id, &f.return_to, !f.show_remove.is_empty()).await
}

#[derive(Deserialize)]
pub struct ReturnForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    return_to: String,
    #[serde(default)]
    show_remove: String,
}

#[derive(Deserialize)]
pub struct YearForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    year: String,
    #[serde(default)]
    return_to: String,
    #[serde(default)]
    show_remove: String,
}

/// Prüft Abschlussdatum und -jahr aus einem Formular. Ein Datum hat Vorrang;
/// beides leer = unbekannt.
fn parse_done(date: &str, year: &str, today: NaiveDate) -> Result<Done, AppError> {
    let bad = AppError::BadRequest("form-error-done-year");
    let (date, year) = (date.trim(), year.trim());
    if !date.is_empty() {
        return NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .ok()
            .filter(|d| d.year() >= 1900 && *d <= today)
            .map(Done::Date)
            .ok_or(bad);
    }
    if year.is_empty() {
        return Ok(Done::Unknown);
    }
    year.parse::<i32>().ok().filter(|y| (1900..=today.year()).contains(y)).map(Done::Year).ok_or(bad)
}

/// Ändert das Jahr, in dem der Benutzer das Medium gelesen/gehört/gesehen hat.
pub async fn set_year(
    State(st): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(f): Form<YearForm>,
) -> Result<Response, AppError> {
    cu.check_csrf(&f.csrf)?;
    let done = parse_done(&f.date, &f.year, today(&st))?;
    if !entries::set_done(&st.pool, cu.id(), id, done).await? {
        return Err(AppError::NotFound);
    }
    status_response(&st, &cu, &headers, id, &f.return_to, !f.show_remove.is_empty()).await
}

pub async fn remove_entry(
    State(st): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(f): Form<ReturnForm>,
) -> Result<Response, AppError> {
    cu.check_csrf(&f.csrf)?;
    media::get(&st.pool, id).await?.ok_or(AppError::NotFound)?;
    entries::remove(&st.pool, cu.id(), id).await?;
    status_response(&st, &cu, &headers, id, &f.return_to, !f.show_remove.is_empty()).await
}

// ----- Liste -----

#[derive(Deserialize, Default)]
pub struct ListQuery {
    #[serde(default)]
    typ: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    sort: String,
    #[serde(default)]
    seite: Option<i64>,
}

#[derive(Template)]
#[template(path = "list.html")]
struct ListPage {
    ctx: Ctx,
    items: Vec<ListItem>,
    count_text: String,
    typ: String,
    status: String,
    q: String,
    sort: &'static str,
    page: i64,
    pages: i64,
    prev_url: String,
    next_url: String,
}

pub async fn list(
    State(st): State<AppState>,
    cu: CurrentUser,
    Query(lq): Query<ListQuery>,
) -> Result<Response, AppError> {
    let ctx = Ctx::new(&st, Some(&cu), "list");
    let media_type = MediaType::parse(&lq.typ);
    let status = Status::parse(&lq.status);
    let sort = entries::Sort::parse(&lq.sort);
    let page = lq.seite.unwrap_or(1).max(1);
    let q = lq.q.trim().to_string();
    let filter = entries::ListFilter {
        media_type,
        status,
        q: (!q.is_empty()).then(|| q.clone()),
        sort,
        limit: PAGE_SIZE,
        offset: (page - 1) * PAGE_SIZE,
    };
    let (rows, total) = entries::list(&st.pool, cu.id(), &filter).await?;
    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);

    let typ = media_type.map(|t| t.as_str()).unwrap_or("").to_string();
    let status_s = status.map(|s| s.as_str()).unwrap_or("").to_string();
    let base_query =
        format!("liste?typ={typ}&status={status_s}&q={}&sort={}", urlencoding::encode(&q), sort.as_str());
    let items = rows.iter().map(|(m, e)| ListItem::new(&ctx, m, e)).collect();
    let count_text = ctx.t_count("list-count", total);
    Ok(render(&ListPage {
        items,
        count_text,
        typ,
        status: status_s,
        q,
        sort: sort.as_str(),
        page,
        pages,
        prev_url: if page > 1 {
            format!("{}{base_query}&seite={}", ctx.base, page - 1)
        } else {
            String::new()
        },
        next_url: if page < pages {
            format!("{}{base_query}&seite={}", ctx.base, page + 1)
        } else {
            String::new()
        },
        ctx,
    }))
}

// ----- Detail -----

#[derive(Template)]
#[template(path = "detail.html")]
struct DetailPage {
    ctx: Ctx,
    m: Media,
    type_label: String,
    type_icon: &'static str,
    authors_prefix: String,
    narrators_label: String,
    volume_label: String,
    series_label: String,
    publisher_label: String,
    cover: Option<String>,
    source_label: String,
    paragraphs: Vec<String>,
    s: StatusView,
    added: String,
    can_edit: bool,
    can_delete: bool,
    /// Noch nicht gespeicherter Suchtreffer.
    preview: bool,
}

fn paragraphs(description: Option<&str>) -> Vec<String> {
    description
        .unwrap_or("")
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

impl DetailPage {
    fn new(ctx: Ctx, m: Media, cover: Option<String>, s: StatusView) -> Self {
        DetailPage {
            type_label: ctx.type_label(m.media_type),
            type_icon: type_icon(m.media_type),
            authors_prefix: ctx.authors_prefix(m.media_type),
            narrators_label: ctx.narrators_label(m.media_type),
            volume_label: ctx.volume_label(m.media_type),
            series_label: ctx.t(if m.media_type.is_movie() { "label-movie-series" } else { "label-series" }),
            publisher_label: ctx.t(match m.media_type {
                MediaType::Movie => "label-studio",
                MediaType::Series => "label-network",
                _ => "label-publisher",
            }),
            source_label: source_label(&ctx, &m.source),
            paragraphs: paragraphs(m.description.as_deref()),
            added: String::new(),
            can_edit: false,
            can_delete: false,
            preview: false,
            cover,
            s,
            m,
            ctx,
        }
    }
}

pub async fn detail(
    State(st): State<AppState>,
    cu: CurrentUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let m = media::get(&st.pool, id).await?.ok_or(AppError::NotFound)?;
    let ctx = Ctx::new(&st, Some(&cu), "");
    let entry = entries::get(&st.pool, cu.id(), id).await?;
    let mut s = StatusView::new(&ctx, Some(id), m.media_type, entry.as_ref(), &format!("medium/{id}"));
    s.show_remove = true;
    let cover = cover_url(&ctx, &m.cover_file);
    let added = entry.as_ref().map(|e| ctx.date(&e.added_at)).unwrap_or_default();
    let can_edit = can_edit(&cu, &m);
    let mut page = DetailPage::new(ctx, m, cover, s);
    page.added = added;
    page.can_edit = can_edit;
    page.can_delete = cu.user.is_admin;
    Ok(render(&page))
}

/// Detailseite eines Suchtreffers, ohne ihn zu speichern. Erst beim Setzen
/// eines Status wird das Medium angelegt.
pub async fn preview(
    State(st): State<AppState>,
    cu: CurrentUser,
    Path((source, external_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let hit = st
        .providers
        .resolve(&source, &external_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or(AppError::NotFound)?;
    if let Some(id) = media::find_existing(&st.pool, &hit).await? {
        return Ok(Redirect::to(&format!("{}medium/{id}", st.cfg.base_path())).into_response());
    }
    let ctx = Ctx::new(&st, Some(&cu), "search");
    let kind = hit.kind();
    let mut s = StatusView::new(&ctx, None, kind, None, "");
    s.source = hit.source.clone();
    s.external_id = hit.external_id.clone().unwrap_or_else(|| external_id.clone());
    s.show_remove = true;
    let cover = hit.cover_url.clone().filter(|u| u.starts_with("https://"));
    let m = Media {
        id: 0,
        media_type: kind,
        title: hit.title,
        subtitle: hit.subtitle,
        authors: hit.authors,
        narrators: hit.narrators,
        description: hit.description,
        isbn10: hit.isbn10,
        isbn13: hit.isbn13,
        asin: hit.asin,
        published_year: hit.published_year,
        series: hit.series,
        series_index: hit.series_index,
        language: hit.language,
        publisher: hit.publisher,
        cover_file: None,
        source: hit.source,
        external_id: hit.external_id,
        created_by: None,
    };
    let mut page = DetailPage::new(ctx, m, cover, s);
    page.preview = true;
    Ok(render(&page))
}

// ----- Manuell anlegen / bearbeiten -----

#[derive(Default, Clone, Debug)]
pub struct FormValues {
    media_type: String,
    title: String,
    subtitle: String,
    authors: String,
    narrators: String,
    isbn: String,
    asin: String,
    year: String,
    series: String,
    series_index: String,
    publisher: String,
    language: String,
    description: String,
    status: String,
    done_year: String,
    done_on: String,
}

#[derive(Template)]
#[template(path = "form.html")]
struct FormPage {
    ctx: Ctx,
    is_edit: bool,
    action: String,
    f: FormValues,
    error: String,
    has_cover: bool,
}

impl FormValues {
    /// Soll ein Element mit `data-types` beim aktuellen Typ verborgen werden?
    pub fn hide(&self, types: &str) -> bool {
        !types.split(' ').any(|t| t == self.media_type)
    }

    fn from_media(m: &Media) -> Self {
        FormValues {
            media_type: m.media_type.as_str().into(),
            title: m.title.clone(),
            subtitle: m.subtitle.clone().unwrap_or_default(),
            authors: m.authors.join(", "),
            narrators: m.narrators.join(", "),
            isbn: m.isbn13.clone().or_else(|| m.isbn10.clone()).unwrap_or_default(),
            asin: m.asin.clone().unwrap_or_default(),
            year: m.published_year.map(|y| y.to_string()).unwrap_or_default(),
            series: m.series.clone().unwrap_or_default(),
            series_index: m.series_index.clone().unwrap_or_default(),
            publisher: m.publisher.clone().unwrap_or_default(),
            language: m.language.clone().unwrap_or_default(),
            description: m.description.clone().unwrap_or_default(),
            status: String::new(),
            done_year: String::new(),
            done_on: String::new(),
        }
    }

    /// Prüft die Eingaben; Fehler als Schlüssel der Sprachdatei.
    fn validate(&self) -> Result<NewMedia, &'static str> {
        fn opt(s: &str, max: usize) -> Option<String> {
            let s = s.trim();
            (!s.is_empty()).then(|| s.chars().take(max).collect())
        }
        fn names(s: &str) -> Vec<String> {
            s.split([',', ';', '\n'])
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(|n| n.chars().take(200).collect())
                .take(30)
                .collect()
        }
        let kind = MediaType::parse(&self.media_type).ok_or("form-error-type")?;
        let title = opt(&self.title, 500).ok_or("form-error-title")?;
        let (mut isbn10, mut isbn13) = (None, None);
        if !kind.is_screen() && !self.isbn.trim().is_empty() {
            let i = clean_isbn(&self.isbn).ok_or("form-error-isbn")?;
            if i.len() == 10 {
                isbn13 = isbn10_to_13(&i);
                isbn10 = Some(i);
            } else {
                isbn13 = Some(i);
            }
        }
        let asin = match opt(&self.asin, 20).filter(|_| kind.is_audiobook()) {
            Some(a) => {
                let a = a.to_ascii_uppercase();
                if a.len() != 10 || !a.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err("form-error-asin");
                }
                Some(a)
            }
            None => None,
        };
        let published_year = match opt(&self.year, 10) {
            Some(y) => {
                Some(y.parse::<i64>().ok().filter(|y| (0..=2100).contains(y)).ok_or("form-error-year")?)
            }
            None => None,
        };
        Ok(NewMedia {
            media_type: Some(kind),
            title,
            subtitle: opt(&self.subtitle, 500),
            authors: names(&self.authors),
            narrators: if kind.has_narrators() { names(&self.narrators) } else { vec![] },
            description: opt(&self.description, 20000),
            isbn10,
            isbn13,
            asin,
            published_year,
            series: opt(&self.series, 300),
            series_index: opt(&self.series_index, 20),
            language: opt(&self.language, 20),
            publisher: opt(&self.publisher, 300),
            cover_url: None,
            source: "manual".into(),
            external_id: None,
        })
    }
}

struct ParsedForm {
    values: FormValues,
    csrf: String,
    cover: Option<Vec<u8>>,
    remove_cover: bool,
}

async fn parse_multipart(mut mp: Multipart) -> Result<ParsedForm, AppError> {
    let mut p =
        ParsedForm { values: FormValues::default(), csrf: String::new(), cover: None, remove_cover: false };
    let bad = |_| AppError::BadRequest("error-badrequest-text");
    while let Some(field) = mp.next_field().await.map_err(bad)? {
        let name = field.name().unwrap_or("").to_string();
        if name == "cover" {
            let bytes = field.bytes().await.map_err(bad)?;
            if !bytes.is_empty() {
                p.cover = Some(bytes.to_vec());
            }
            continue;
        }
        let text = field.text().await.map_err(bad)?;
        let v = &mut p.values;
        match name.as_str() {
            "csrf" => p.csrf = text,
            "remove_cover" => p.remove_cover = !text.is_empty(),
            "media_type" => v.media_type = text,
            "title" => v.title = text,
            "subtitle" => v.subtitle = text,
            "authors" => v.authors = text,
            "narrators" => v.narrators = text,
            "isbn" => v.isbn = text,
            "asin" => v.asin = text,
            "year" => v.year = text,
            "series" => v.series = text,
            "series_index" => v.series_index = text,
            "publisher" => v.publisher = text,
            "language" => v.language = text,
            "description" => v.description = text,
            "status" => v.status = text,
            "done_year" => v.done_year = text,
            "done_on" => v.done_on = text,
            _ => {}
        }
    }
    Ok(p)
}

#[derive(Deserialize, Default)]
pub struct NewQuery {
    #[serde(default)]
    typ: String,
    #[serde(default)]
    titel: String,
}

pub async fn new_form(State(st): State<AppState>, cu: CurrentUser, Query(nq): Query<NewQuery>) -> Response {
    let ctx = Ctx::new(&st, Some(&cu), "new");
    let f = FormValues {
        media_type: MediaType::parse(&nq.typ).unwrap_or(MediaType::Book).as_str().into(),
        title: nq.titel.chars().take(500).collect(),
        status: "wanted".into(),
        done_year: today(&st).year().to_string(),
        ..Default::default()
    };
    render(&FormPage {
        action: format!("{}neu", ctx.base),
        ctx,
        is_edit: false,
        f,
        error: String::new(),
        has_cover: false,
    })
}

pub async fn new_save(
    State(st): State<AppState>,
    cu: CurrentUser,
    mp: Multipart,
) -> Result<Response, AppError> {
    let p = parse_multipart(mp).await?;
    cu.check_csrf(&p.csrf)?;
    let ctx = Ctx::new(&st, Some(&cu), "new");
    let action = format!("{}neu", ctx.base);
    let fail = |ctx: Ctx, f: FormValues, key: &str| {
        let error = ctx.t(key);
        render_status(
            StatusCode::UNPROCESSABLE_ENTITY,
            &FormPage { ctx, is_edit: false, action: action.clone(), f, error, has_cover: false },
        )
    };
    let nm = match p.values.validate() {
        Ok(nm) => nm,
        Err(key) => return Ok(fail(ctx, p.values, key)),
    };
    let status = Status::parse(&p.values.status).unwrap_or(Status::Wanted);
    let cover = match &p.cover {
        Some(b) => match st.covers.store(b).await {
            Ok(n) => Some(n),
            Err(_) => return Ok(fail(ctx, p.values, "form-error-cover")),
        },
        None => None,
    };
    let done = match parse_done(&p.values.done_on, &p.values.done_year, today(&st)) {
        Ok(d) => d,
        Err(_) => return Ok(fail(ctx, p.values, "form-error-done-year")),
    };
    let id = media::insert(&st.pool, &nm, cover.as_deref(), Some(cu.id())).await?;
    entries::set_status(&st.pool, cu.id(), id, status, today(&st)).await?;
    if status == Status::Done {
        entries::set_done(&st.pool, cu.id(), id, done).await?;
    }
    Ok(Redirect::to(&format!("{}medium/{id}", st.cfg.base_path())).into_response())
}

pub async fn edit_form(
    State(st): State<AppState>,
    cu: CurrentUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let m = media::get(&st.pool, id).await?.ok_or(AppError::NotFound)?;
    if !can_edit(&cu, &m) {
        return Err(AppError::Auth(crate::auth::AuthError::Forbidden));
    }
    let ctx = Ctx::new(&st, Some(&cu), "");
    Ok(render(&FormPage {
        action: format!("{}medium/{id}/bearbeiten", ctx.base),
        ctx,
        is_edit: true,
        f: FormValues::from_media(&m),
        error: String::new(),
        has_cover: m.cover_file.is_some(),
    }))
}

async fn drop_cover_if_unused(st: &AppState, file: &Option<String>) {
    if let Some(f) = file
        && let Ok(false) = media::cover_in_use(&st.pool, f).await
    {
        st.covers.remove(f).await;
    }
}

pub async fn edit_save(
    State(st): State<AppState>,
    cu: CurrentUser,
    Path(id): Path<i64>,
    mp: Multipart,
) -> Result<Response, AppError> {
    let p = parse_multipart(mp).await?;
    cu.check_csrf(&p.csrf)?;
    let m = media::get(&st.pool, id).await?.ok_or(AppError::NotFound)?;
    if !can_edit(&cu, &m) {
        return Err(AppError::Auth(crate::auth::AuthError::Forbidden));
    }
    let ctx = Ctx::new(&st, Some(&cu), "");
    let action = format!("{}medium/{id}/bearbeiten", ctx.base);
    let has_cover = m.cover_file.is_some();
    let fail = |ctx: Ctx, f: FormValues, key: &str| {
        let error = ctx.t(key);
        render_status(
            StatusCode::UNPROCESSABLE_ENTITY,
            &FormPage { ctx, is_edit: true, action: action.clone(), f, error, has_cover },
        )
    };
    let mut nm = match p.values.validate() {
        Ok(nm) => nm,
        Err(key) => return Ok(fail(ctx, p.values, key)),
    };
    nm.source = m.source.clone();
    nm.external_id = m.external_id.clone();
    if let Some(b) = &p.cover {
        match st.covers.store(b).await {
            Ok(n) => {
                media::set_cover(&st.pool, id, Some(&n)).await?;
                if m.cover_file.as_deref() != Some(n.as_str()) {
                    drop_cover_if_unused(&st, &m.cover_file).await;
                }
            }
            Err(_) => return Ok(fail(ctx, p.values, "form-error-cover")),
        }
    } else if p.remove_cover {
        media::set_cover(&st.pool, id, None).await?;
        drop_cover_if_unused(&st, &m.cover_file).await;
    }
    media::update(&st.pool, id, &nm).await?;
    Ok(Redirect::to(&format!("{}medium/{id}", st.cfg.base_path())).into_response())
}

#[derive(Deserialize)]
pub struct CsrfOnly {
    #[serde(default)]
    csrf: String,
}

pub async fn delete_media(
    State(st): State<AppState>,
    cu: CurrentUser,
    Path(id): Path<i64>,
    Form(f): Form<CsrfOnly>,
) -> Result<Response, AppError> {
    cu.check_csrf(&f.csrf)?;
    require_admin(&cu)?;
    let m = media::get(&st.pool, id).await?.ok_or(AppError::NotFound)?;
    media::delete(&st.pool, id).await?;
    drop_cover_if_unused(&st, &m.cover_file).await;
    tracing::info!(user_id = cu.id(), media_id = id, "Medium gelöscht");
    Ok(Redirect::to(&format!("{}liste", st.cfg.base_path())).into_response())
}

// ----- Statistik -----

pub struct StatCell {
    count: i64,
    pct: i64,
    class: &'static str,
}

pub struct StatRow {
    year: String,
    cells: Vec<StatCell>,
}

pub struct StatTile {
    num: i64,
    label: String,
    class: &'static str,
    icon: &'static str,
    /// „Meine Liste“ mit dem Filter dieser Kachel (relativ zu `ctx.base`).
    link: String,
}

pub struct StatHead {
    label: String,
    icon: &'static str,
}

#[derive(Template)]
#[template(path = "stats.html")]
struct StatsPage {
    ctx: Ctx,
    tiles: Vec<StatTile>,
    head: Vec<StatHead>,
    rows: Vec<StatRow>,
}

pub async fn stats(State(st): State<AppState>, cu: CurrentUser) -> Result<Response, AppError> {
    let ctx = Ctx::new(&st, Some(&cu), "stats");
    let s = entries::stats(&st.pool, cu.id()).await?;
    let max = s.per_year.iter().flat_map(|(_, c)| c.iter().copied()).max().unwrap_or(1).max(1);
    let types = MediaType::ALL;
    let rows = s
        .per_year
        .iter()
        .map(|(year, counts)| StatRow {
            year: year.map(|y| y.to_string()).unwrap_or_else(|| ctx.t("stats-no-year")),
            cells: types
                .iter()
                .map(|t| StatCell {
                    count: counts[t.index()],
                    pct: counts[t.index()] * 100 / max,
                    class: t.as_str(),
                })
                .collect(),
        })
        .collect();
    let mut tiles = Vec::new();
    for (counts, keys, wanted) in [
        (
            &s.done,
            ["stats-books-read", "stats-audiobooks-heard", "stats-movies-seen", "stats-series-seen"],
            false,
        ),
        (
            &s.wanted,
            ["stats-books-wanted", "stats-audiobooks-wanted", "stats-movies-wanted", "stats-series-wanted"],
            true,
        ),
    ] {
        for t in types {
            tiles.push(StatTile {
                num: counts[t.index()],
                label: ctx.t(keys[t.index()]),
                class: if wanted { "" } else { t.as_str() },
                icon: if wanted { "☆" } else { type_icon(t) },
                link: if wanted {
                    format!("liste?typ={}&status=wanted&sort=added", t.as_str())
                } else {
                    format!("liste?typ={}&status=done&sort=done", t.as_str())
                },
            });
        }
    }
    let head = types
        .iter()
        .map(|t| StatHead {
            label: ctx.t(match t {
                MediaType::Book => "type-books",
                MediaType::Audiobook => "type-audiobooks",
                MediaType::Movie => "type-movies",
                MediaType::Series => "type-series-plural",
            }),
            icon: type_icon(*t),
        })
        .collect();
    Ok(render(&StatsPage { ctx, tiles, head, rows }))
}

// ----- Admin -----

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminPage {
    ctx: Ctx,
    users: Vec<repo::users::UserOverview>,
    media_count: i64,
}

pub async fn admin(State(st): State<AppState>, cu: CurrentUser) -> Result<Response, AppError> {
    require_admin(&cu)?;
    let ctx = Ctx::new(&st, Some(&cu), "admin");
    let users = repo::users::overview(&st.pool).await?;
    let media_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media").fetch_one(&st.pool).await?;
    Ok(render(&AdminPage { ctx, users, media_count }))
}

// ----- Cover -----

pub async fn cover(
    State(st): State<AppState>,
    _cu: CurrentUser,
    Path(file): Path<String>,
) -> Result<Response, AppError> {
    let path = st.covers.path(&file).ok_or(AppError::NotFound)?;
    let bytes = tokio::fs::read(&path).await.map_err(|_| AppError::NotFound)?;
    let mime = mime_guess::from_path(&path).first_or_octet_stream().to_string();
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "private, max-age=31536000, immutable".into()),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_validation() {
        let mut f = FormValues {
            media_type: "audiobook".into(),
            title: "  Momo ".into(),
            authors: "Michael Ende".into(),
            narrators: "Gert Westphal; Anna Thalbach".into(),
            isbn: "0-306-40615-2".into(),
            asin: "b004v0l2vg".into(),
            year: "1973".into(),
            ..Default::default()
        };
        let m = f.validate().unwrap();
        assert_eq!(m.title, "Momo");
        assert_eq!(m.narrators.len(), 2);
        assert_eq!(m.isbn13.as_deref(), Some("9780306406157"));
        assert_eq!(m.asin.as_deref(), Some("B004V0L2VG"));
        f.year = "abc".into();
        assert_eq!(f.validate().unwrap_err(), "form-error-year");
        f.year.clear();
        f.title = " ".into();
        assert_eq!(f.validate().unwrap_err(), "form-error-title");
    }
}
