//! Template-Kontext und View-Modelle. Alle Texte kommen aus der Sprachdatei.

use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use fluent_bundle::FluentValue;

use crate::auth::CurrentUser;
use crate::i18n::{self, DEFAULT_LANG};
use crate::models::{Entry, Media, MediaType, NewMedia, Status, User};
use crate::web::AppState;

/// Gemeinsamer Kontext jeder Seite.
pub struct Ctx {
    pub lang: &'static str,
    pub base: String,
    pub user: Option<User>,
    pub csrf: String,
    pub nav: &'static str,
    pub version: &'static str,
    pub tz: Tz,
}

impl Ctx {
    pub fn new(st: &AppState, cu: Option<&CurrentUser>, nav: &'static str) -> Self {
        Ctx {
            lang: DEFAULT_LANG,
            base: st.cfg.base_path().to_string(),
            user: cu.map(|c| c.user.clone()),
            csrf: cu.map(|c| c.csrf.clone()).unwrap_or_default(),
            nav,
            version: crate::web::asset_version(),
            tz: st.cfg.timezone,
        }
    }

    pub fn t(&self, key: &str) -> String {
        i18n::get().t(self.lang, key)
    }

    pub fn t_args(&self, key: &str, args: &[(&str, FluentValue<'_>)]) -> String {
        i18n::get().t_args(self.lang, key, args)
    }

    pub fn t_count<N: std::borrow::Borrow<i64>>(&self, key: &str, count: N) -> String {
        self.t_args(key, &[("count", (*count.borrow()).into())])
    }

    pub fn is_admin(&self) -> bool {
        self.user.as_ref().is_some_and(|u| u.is_admin)
    }

    pub fn date(&self, d: &DateTime<Utc>) -> String {
        d.with_timezone(&self.tz).format("%d.%m.%Y").to_string()
    }

    pub fn type_label(&self, t: MediaType) -> String {
        self.t(match t {
            MediaType::Book => "type-book",
            MediaType::Audiobook => "type-audiobook",
            MediaType::Movie => "type-movie",
        })
    }

    /// Vorsatz vor der Urheber-Zeile auf der Detailseite (bei Filmen „Regie:“).
    pub fn authors_prefix(&self, t: MediaType) -> String {
        if t.is_movie() { format!("{} ", self.t("label-directed-by")) } else { String::new() }
    }

    /// Vorsatz vor Sprecher bzw. Darstellern in Karten.
    pub fn narrators_prefix(&self, t: MediaType) -> String {
        self.t(if t.is_movie() { "label-starring" } else { "label-narrated-by" })
    }

    /// Bezeichnung des Feldes `narrators`: Sprecher bzw. Darsteller.
    pub fn narrators_label(&self, t: MediaType) -> String {
        self.t(if t.is_movie() { "label-cast" } else { "label-narrators" })
    }

    /// „Band“ bzw. bei Filmen „Teil“.
    pub fn volume_label(&self, t: MediaType) -> String {
        self.t(if t.is_movie() { "label-part" } else { "label-volume" })
    }

    pub fn status_label(&self, s: Status, t: MediaType) -> String {
        self.t(s.label_key(t))
    }

    pub fn nav_class(&self, item: &str) -> &'static str {
        if self.nav == item { "active" } else { "" }
    }
}

pub fn render<T: Template>(t: &T) -> Response {
    render_status(StatusCode::OK, t)
}

pub fn render_status<T: Template>(status: StatusCode, t: &T) -> Response {
    match t.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(e) => {
            tracing::error!("Template-Fehler: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Status-Anzeige und -Schaltflächen für ein Medium (Suchtreffer, Liste, Detail).
#[derive(Clone, Debug)]
pub struct StatusView {
    /// Gespeichertes Medium, falls schon in der Bibliothek.
    pub media_id: Option<i64>,
    /// Für noch nicht gespeicherte Suchtreffer.
    pub source: String,
    pub external_id: String,
    pub status: Option<&'static str>,
    pub status_text: String,
    /// Zusatz zum Status: Jahr bei „done“, „seit <Datum>“ bei „wanted“.
    pub status_when: String,
    /// Jahr als Formularwert (leer = unbekannt).
    pub done_year: String,
    pub label_done: String,
    pub label_wanted: String,
    /// Zielseite für Formulare ohne JavaScript.
    pub return_to: String,
    pub show_remove: bool,
}

impl StatusView {
    pub fn new(
        ctx: &Ctx,
        media_id: Option<i64>,
        kind: MediaType,
        entry: Option<&Entry>,
        return_to: &str,
    ) -> Self {
        StatusView {
            media_id,
            source: String::new(),
            external_id: String::new(),
            status: entry.map(|e| e.status.as_str()),
            status_text: entry.map(|e| ctx.status_label(e.status, kind)).unwrap_or_default(),
            status_when: entry
                .map(|e| match e.status {
                    Status::Done => e.done_year.map(|y| y.to_string()).unwrap_or_default(),
                    Status::Wanted => format!("{} {}", ctx.t("status-since"), ctx.date(&e.status_changed_at)),
                })
                .unwrap_or_default(),
            done_year: entry.and_then(|e| e.done_year).map(|y| y.to_string()).unwrap_or_default(),
            label_done: ctx.status_label(Status::Done, kind),
            label_wanted: ctx.status_label(Status::Wanted, kind),
            return_to: return_to.to_string(),
            show_remove: false,
        }
    }

    pub fn is(&self, s: &str) -> bool {
        self.status == Some(s)
    }
}

/// Karte für Listen und Suchergebnisse.
#[derive(Clone, Debug)]
pub struct Card {
    pub media_id: Option<i64>,
    pub title: String,
    pub subtitle: String,
    pub authors: String,
    pub narrators: String,
    /// „gelesen von“ bzw. „mit“ (Darsteller).
    pub narrators_label: String,
    pub year: String,
    pub series: String,
    pub cover: Option<String>,
    pub type_label: String,
    pub type_class: &'static str,
    pub type_icon: &'static str,
    pub source_label: String,
    pub status: StatusView,
}

/// Kompakter Eintrag für „Meine Liste“: Titel, Serie/Band, Jahr, Autor, Kategorie, Status.
#[derive(Clone, Debug)]
pub struct ListItem {
    pub media_id: i64,
    pub title: String,
    pub series: String,
    pub authors: String,
    pub year: String,
    pub cover: Option<String>,
    pub type_label: String,
    pub type_class: &'static str,
    pub icon_id: &'static str,
    pub status_class: &'static str,
    pub status_text: String,
}

impl ListItem {
    pub fn new(ctx: &Ctx, m: &Media, e: &Entry) -> Self {
        let series = match (&m.series, &m.series_index) {
            (Some(s), Some(i)) => format!("{s} · {} {i}", ctx.volume_label(m.media_type)),
            (Some(s), None) => s.clone(),
            (None, Some(i)) => format!("{} {i}", ctx.volume_label(m.media_type)),
            (None, None) => String::new(),
        };
        ListItem {
            media_id: m.id,
            title: m.title.clone(),
            series,
            authors: m.authors.join(", "),
            year: m.published_year.map(|y| y.to_string()).unwrap_or_default(),
            cover: cover_url(ctx, &m.cover_file),
            type_label: ctx.type_label(m.media_type),
            type_class: type_class(m.media_type),
            icon_id: icon_id(m.media_type),
            status_class: e.status.as_str(),
            status_text: match e.done_year {
                Some(y) if e.status == Status::Done => {
                    format!("{} {y}", ctx.status_label(e.status, m.media_type))
                }
                _ => ctx.status_label(e.status, m.media_type),
            },
        }
    }
}

fn series_line(series: &Option<String>, index: &Option<String>) -> String {
    match (series, index) {
        (Some(s), Some(i)) => format!("{s} · {i}"),
        (Some(s), None) => s.clone(),
        _ => String::new(),
    }
}

fn type_class(t: MediaType) -> &'static str {
    match t {
        MediaType::Book => "book",
        MediaType::Audiobook => "audiobook",
        MediaType::Movie => "movie",
    }
}

/// Symbol-ID im SVG-Sprite (`static/icons/sprite.svg`).
pub fn icon_id(t: MediaType) -> &'static str {
    match t {
        MediaType::Book => "book",
        MediaType::Audiobook => "headphones",
        MediaType::Movie => "film",
    }
}

pub fn type_icon(t: MediaType) -> &'static str {
    match t {
        MediaType::Book => "📖",
        MediaType::Audiobook => "🎧",
        MediaType::Movie => "🎬",
    }
}

pub fn cover_url(ctx: &Ctx, cover_file: &Option<String>) -> Option<String> {
    cover_file.as_ref().map(|f| format!("{}covers/{f}", ctx.base))
}

pub fn source_label(ctx: &Ctx, source: &str) -> String {
    ctx.t(match source {
        "googlebooks" => "source-googlebooks",
        "openlibrary" => "source-openlibrary",
        "audible" => "source-audible",
        "tmdb" => "source-tmdb",
        "manual" => "source-manual",
        _ => "source-other",
    })
}

impl Card {
    pub fn from_media(ctx: &Ctx, m: &Media, entry: Option<&Entry>, return_to: &str) -> Self {
        Card {
            media_id: Some(m.id),
            title: m.title.clone(),
            subtitle: m.subtitle.clone().unwrap_or_default(),
            authors: m.authors.join(", "),
            narrators: m.narrators.join(", "),
            narrators_label: ctx.narrators_prefix(m.media_type),
            year: m.published_year.map(|y| y.to_string()).unwrap_or_default(),
            series: series_line(&m.series, &m.series_index),
            cover: cover_url(ctx, &m.cover_file),
            type_label: ctx.type_label(m.media_type),
            type_class: type_class(m.media_type),
            type_icon: type_icon(m.media_type),
            source_label: source_label(ctx, &m.source),
            status: StatusView::new(ctx, Some(m.id), m.media_type, entry, return_to),
        }
    }

    /// Suchtreffer eines Providers, ggf. mit Abgleich gegen die eigene Liste.
    pub fn from_hit(ctx: &Ctx, h: &NewMedia, known: Option<(i64, Option<Entry>)>, return_to: &str) -> Self {
        let kind = h.kind();
        let (media_id, entry) = match known {
            Some((id, e)) => (Some(id), e),
            None => (None, None),
        };
        let mut status = StatusView::new(ctx, media_id, kind, entry.as_ref(), return_to);
        status.source = h.source.clone();
        status.external_id = h.external_id.clone().unwrap_or_default();
        Card {
            media_id,
            title: h.title.clone(),
            subtitle: h.subtitle.clone().unwrap_or_default(),
            authors: h.authors.join(", "),
            narrators: h.narrators.join(", "),
            narrators_label: ctx.narrators_prefix(kind),
            year: h.published_year.map(|y| y.to_string()).unwrap_or_default(),
            series: series_line(&h.series, &h.series_index),
            cover: h.cover_url.clone().filter(|u| u.starts_with("https://")),
            type_label: ctx.type_label(kind),
            type_class: type_class(kind),
            type_icon: type_icon(kind),
            source_label: source_label(ctx, &h.source),
            status,
        }
    }
}
