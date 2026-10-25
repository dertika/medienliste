//! Austauschbare Metadaten-Quellen.
//!
//! Eine neue Quelle implementiert [`Provider`] und wird in [`Registry::from_config`]
//! eingetragen. Pro Medientyp werden die Provider in der eingetragenen Reihenfolge
//! gefragt; der erste mit Treffern gewinnt (spätere dienen als Fallback).

pub mod audible;
pub mod dnb;
pub mod googlebooks;
pub mod openlibrary;
pub mod tmdb;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;

use crate::config::Config;
use crate::models::{MediaType, NewMedia};

#[async_trait]
pub trait Provider: Send + Sync {
    /// Kennung, die als `source` gespeichert wird.
    fn id(&self) -> &'static str;
    /// Für welchen Medientyp dieser Provider sucht.
    fn media_type(&self) -> MediaType;
    /// Freitextsuche (Titel, Autor, ISBN/ASIN).
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<NewMedia>>;
    /// Details zu einer externen ID.
    async fn fetch(&self, external_id: &str) -> Result<Option<NewMedia>>;
    /// Liefert die Suche nur Kurzdaten (z. B. ohne Regie), sodass beim Hinzufügen
    /// immer die vollständigen Details geholt werden sollen?
    fn fetch_on_add(&self) -> bool {
        false
    }
}

#[derive(Debug, Default)]
pub struct SearchOutcome {
    pub hits: Vec<NewMedia>,
    /// Provider, die mit Fehler geantwortet haben.
    pub failed: Vec<&'static str>,
    /// Für diesen Medientyp ist gar kein Provider konfiguriert.
    pub no_provider: bool,
}

const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const CACHE_MAX: usize = 5000;

pub struct Registry {
    providers: Vec<Arc<dyn Provider>>,
    /// Zuletzt gesehene Suchtreffer, damit „Hinzufügen“ nicht erneut abfragen muss.
    cache: Mutex<HashMap<(String, String), (Instant, NewMedia)>>,
}

/// Gemeinsamer HTTP-Client für Provider.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(crate::USER_AGENT)
        .build()
        .expect("HTTP-Client")
}

impl Registry {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self { providers, cache: Mutex::new(HashMap::new()) }
    }

    pub fn from_config(cfg: &Config, http: reqwest::Client) -> Self {
        let mut providers: Vec<Arc<dyn Provider>> = Vec::new();
        // Bücher: Google Books nur mit API-Key (das anonyme Kontingent ist meist erschöpft),
        // dann die Deutsche Nationalbibliothek, zuletzt Open Library.
        if let Some(key) = &cfg.google_books_api_key {
            providers.push(Arc::new(googlebooks::GoogleBooks::new(http.clone(), Some(key.clone()))));
        }
        providers.push(Arc::new(dnb::Dnb::new(http.clone())));
        providers.push(Arc::new(openlibrary::OpenLibrary::new(http.clone())));
        providers.push(Arc::new(audible::Audible::new(http.clone(), &cfg.audible_marketplace)));
        // Filme nur mit API-Key (TMDb erlaubt keine anonyme Suche).
        if let Some(key) = &cfg.tmdb_api_key {
            providers.push(Arc::new(tmdb::Tmdb::new(http.clone(), key.clone())));
            providers.push(Arc::new(tmdb::TmdbTv::new(http.clone(), key.clone())));
        }
        Self::new(providers)
    }

    pub fn all(&self) -> impl Iterator<Item = &Arc<dyn Provider>> {
        self.providers.iter()
    }

    pub fn provider(&self, id: &str) -> Option<&Arc<dyn Provider>> {
        self.providers.iter().find(|p| p.id() == id)
    }

    pub async fn search(&self, kind: MediaType, query: &str, limit: usize) -> SearchOutcome {
        let mut out = SearchOutcome {
            no_provider: !self.providers.iter().any(|p| p.media_type() == kind),
            ..Default::default()
        };
        for p in self.providers.iter().filter(|p| p.media_type() == kind) {
            match p.search(query, limit).await {
                Ok(hits) if !hits.is_empty() => {
                    self.remember(&hits);
                    out.hits = hits;
                    return out;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(provider = p.id(), "Suche fehlgeschlagen: {e:#}");
                    out.failed.push(p.id());
                }
            }
        }
        out
    }

    /// Liefert die Metadaten zu einem Treffer (aus dem Cache oder frisch vom Provider).
    /// Die Daten stammen dadurch immer vom Provider, nie aus der Anfrage des Clients.
    pub async fn resolve(&self, source: &str, external_id: &str) -> Result<Option<NewMedia>> {
        let key = (source.to_string(), external_id.to_string());
        let cached = self.cache.lock().unwrap().get(&key).filter(|(at, _)| at.elapsed() < CACHE_TTL).cloned();
        let Some(p) = self.provider(source) else {
            return Ok(None);
        };
        if let Some((_, m)) = &cached
            && !p.fetch_on_add()
        {
            return Ok(Some(m.clone()));
        }
        let m = match p.fetch(external_id).await {
            Ok(m) => m,
            // Details nicht abrufbar: auf die Kurzdaten aus der Suche zurückfallen.
            Err(e) if cached.is_some() => {
                tracing::info!(provider = p.id(), "Details nicht abrufbar, nutze Suchtreffer: {e:#}");
                cached.map(|(_, m)| m)
            }
            Err(e) => return Err(e),
        };
        if let Some(m) = &m {
            self.remember(std::slice::from_ref(m));
        }
        Ok(m)
    }

    fn remember(&self, hits: &[NewMedia]) {
        let mut c = self.cache.lock().unwrap();
        if c.len() + hits.len() > CACHE_MAX {
            c.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
            if c.len() + hits.len() > CACHE_MAX {
                c.clear();
            }
        }
        let now = Instant::now();
        for h in hits {
            if let Some(id) = &h.external_id {
                c.insert((h.source.clone(), id.clone()), (now, h.clone()));
            }
        }
    }
}

// ----- Hilfsfunktionen für Provider -----

/// Erste vierstellige Jahreszahl in einem Datumstext.
pub(crate) fn parse_year(s: &str) -> Option<i64> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 4 { digits[..4].parse().ok() } else { None }
}

/// Entfernt HTML-Tags und dekodiert die häufigsten Entities; Absätze bleiben erhalten.
pub(crate) fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for c in s.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                let t = tag.trim_start_matches('/').to_ascii_lowercase();
                let name = t.split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or("");
                if matches!(name, "br" | "p" | "div" | "li") && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            (true, c) => tag.push(c),
            (false, c) => out.push(c),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let lines: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines.join("\n\n")
}

pub(crate) fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_is_stripped() {
        assert_eq!(strip_html("<p>Hallo &amp; <b>Welt</b></p><p>Zwei</p>"), "Hallo & Welt\n\nZwei");
    }

    #[test]
    fn years() {
        assert_eq!(parse_year("2019-03-01"), Some(2019));
        assert_eq!(parse_year("19"), None);
    }

    struct Failing;
    #[async_trait]
    impl Provider for Failing {
        fn id(&self) -> &'static str {
            "failing"
        }
        fn media_type(&self) -> MediaType {
            MediaType::Book
        }
        async fn search(&self, _: &str, _: usize) -> Result<Vec<NewMedia>> {
            anyhow::bail!("kaputt")
        }
        async fn fetch(&self, _: &str) -> Result<Option<NewMedia>> {
            Ok(None)
        }
    }

    struct Fixed;
    #[async_trait]
    impl Provider for Fixed {
        fn id(&self) -> &'static str {
            "fixed"
        }
        fn media_type(&self) -> MediaType {
            MediaType::Book
        }
        async fn search(&self, q: &str, _: usize) -> Result<Vec<NewMedia>> {
            Ok(vec![NewMedia {
                title: q.into(),
                source: "fixed".into(),
                external_id: Some("1".into()),
                ..Default::default()
            }])
        }
        async fn fetch(&self, _: &str) -> Result<Option<NewMedia>> {
            anyhow::bail!("sollte aus dem Cache kommen")
        }
    }

    fn ids(cfg: &Config) -> Vec<&'static str> {
        Registry::from_config(cfg, reqwest::Client::new()).all().map(|p| p.id()).collect()
    }

    #[test]
    fn provider_order_depends_on_keys() {
        let base = |extra: &[(&str, &str)]| {
            let mut m: HashMap<String, String> = [
                ("MEDIENLISTE_BASE_URL", "https://m.example.org"),
                ("MEDIENLISTE_OIDC_ISSUER", "https://auth.example.org"),
                ("MEDIENLISTE_OIDC_CLIENT_ID", "m"),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            m.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
            Config::from_lookup(|k| m.get(k).cloned()).unwrap()
        };
        // Ohne Keys: DNB zuerst, kein Google, keine Filme/Serien.
        assert_eq!(ids(&base(&[])), vec!["dnb", "openlibrary", "audible"]);
        // Mit Keys: Google vor DNB, TMDB für Filme und Serien.
        assert_eq!(
            ids(&base(&[("MEDIENLISTE_GOOGLE_BOOKS_API_KEY", "g"), ("MEDIENLISTE_TMDB_API_KEY", "t")])),
            vec!["googlebooks", "dnb", "openlibrary", "audible", "tmdb", "tmdbtv"]
        );
    }

    #[tokio::test]
    async fn falls_back_and_caches() {
        let r = Registry::new(vec![Arc::new(Failing), Arc::new(Fixed)]);
        let out = r.search(MediaType::Book, "Momo", 10).await;
        assert_eq!(out.failed, vec!["failing"]);
        assert_eq!(out.hits[0].title, "Momo");
        assert_eq!(r.resolve("fixed", "1").await.unwrap().unwrap().title, "Momo");
        assert!(r.search(MediaType::Audiobook, "Momo", 10).await.hits.is_empty());
    }
}
