//! Filme: The Movie Database (TMDb), deutsche Titel und Beschreibungen bevorzugt.
//!
//! Benötigt einen API-Key (v3-Key oder v4-Lesetoken). Hinweis laut TMDb-Nutzungsbedingungen:
//! „Dieses Produkt nutzt die TMDB-API, ist aber nicht von TMDB unterstützt oder zertifiziert.“

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::Deserialize;

use super::{Provider, non_empty, parse_year};
use crate::models::{MediaType, NewMedia};

pub struct Tmdb {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    image_base: String,
}

impl Tmdb {
    pub fn new(http: reqwest::Client, api_key: String) -> Self {
        Self::with_base_urls(http, api_key, "https://api.themoviedb.org/3", "https://image.tmdb.org/t/p")
    }

    pub fn with_base_urls(http: reqwest::Client, api_key: String, base_url: &str, image_base: &str) -> Self {
        Self {
            http,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            image_base: image_base.trim_end_matches('/').to_string(),
        }
    }

    /// v4-Lesetokens (JWT) gehen als Bearer-Header, v3-Keys als Query-Parameter.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        let req = self.http.get(format!("{}{path}", self.base_url));
        if self.api_key.starts_with("eyJ") {
            req.bearer_auth(&self.api_key)
        } else {
            req.query(&[("api_key", &self.api_key)])
        }
    }

    fn poster(&self, path: &Option<String>) -> Option<String> {
        path.as_deref().filter(|p| p.starts_with('/')).map(|p| format!("{}/w500{p}", self.image_base))
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<Movie>,
}

#[derive(Deserialize)]
struct Movie {
    id: i64,
    title: Option<String>,
    original_title: Option<String>,
    release_date: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    original_language: Option<String>,
    belongs_to_collection: Option<Collection>,
    #[serde(default)]
    production_companies: Vec<Named>,
    credits: Option<Credits>,
}

#[derive(Deserialize)]
struct Collection {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct Credits {
    #[serde(default)]
    cast: Vec<Named>,
    #[serde(default)]
    crew: Vec<CrewMember>,
}

#[derive(Deserialize)]
struct CrewMember {
    name: String,
    job: Option<String>,
}

impl Tmdb {
    fn convert(&self, m: Movie) -> Option<NewMedia> {
        let title = non_empty(m.title).or_else(|| non_empty(m.original_title.clone()))?;
        let original = non_empty(m.original_title).filter(|o| *o != title);
        let (directors, cast) = match m.credits {
            Some(c) => (
                c.crew.into_iter().filter(|p| p.job.as_deref() == Some("Director")).map(|p| p.name).collect(),
                c.cast.into_iter().take(6).map(|p| p.name).collect(),
            ),
            None => (vec![], vec![]),
        };
        Some(NewMedia {
            media_type: Some(MediaType::Movie),
            title,
            subtitle: original,
            authors: directors,
            narrators: cast,
            description: non_empty(m.overview),
            published_year: m.release_date.as_deref().and_then(parse_year),
            series: m.belongs_to_collection.and_then(|c| non_empty(c.name)),
            language: non_empty(m.original_language),
            publisher: m.production_companies.into_iter().next().map(|c| c.name),
            cover_url: self.poster(&m.poster_path),
            source: "tmdb".into(),
            external_id: Some(m.id.to_string()),
            ..Default::default()
        })
    }
}

#[async_trait]
impl Provider for Tmdb {
    fn id(&self) -> &'static str {
        "tmdb"
    }

    fn media_type(&self) -> MediaType {
        MediaType::Movie
    }

    fn fetch_on_add(&self) -> bool {
        // Die Suche liefert keine Regie/Besetzung.
        true
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<NewMedia>> {
        let resp = self
            .get("/search/movie")
            .query(&[("query", query), ("language", "de-DE"), ("region", "DE"), ("include_adult", "false")])
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("TMDb antwortet mit {}", resp.status());
        }
        let r: SearchResponse = resp.json().await?;
        Ok(r.results.into_iter().take(limit).filter_map(|m| self.convert(m)).collect())
    }

    async fn fetch(&self, external_id: &str) -> Result<Option<NewMedia>> {
        if external_id.is_empty() || !external_id.chars().all(|c| c.is_ascii_digit()) {
            return Ok(None);
        }
        let resp = self
            .get(&format!("/movie/{external_id}"))
            .query(&[("language", "de-DE"), ("append_to_response", "credits")])
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("TMDb antwortet mit {}", resp.status());
        }
        Ok(self.convert(resp.json().await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn search_with_v3_key() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/movie"))
            .and(query_param("api_key", "k3"))
            .and(query_param("language", "de-DE"))
            .and(query_param("query", "Lola rennt"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"results":[{"id":104,"title":"Lola rennt","original_title":"Lola rennt",
                    "release_date":"1998-08-20","overview":"Lola muss Geld besorgen.","poster_path":"/abc.jpg",
                    "original_language":"de"}]}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let t = Tmdb::with_base_urls(reqwest::Client::new(), "k3".into(), &server.uri(), "https://img");
        let hits = t.search("Lola rennt", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!(h.media_type, Some(MediaType::Movie));
        assert_eq!(h.title, "Lola rennt");
        assert_eq!(h.subtitle, None, "Originaltitel nur, wenn er abweicht");
        assert_eq!(h.published_year, Some(1998));
        assert_eq!(h.cover_url.as_deref(), Some("https://img/w500/abc.jpg"));
        assert_eq!(h.external_id.as_deref(), Some("104"));
        assert_eq!(h.source, "tmdb");
    }

    #[tokio::test]
    async fn fetch_with_bearer_token_and_credits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/27205"))
            .and(header("authorization", "Bearer eyJtoken"))
            .and(query_param("append_to_response", "credits"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"id":27205,"title":"Inception","original_title":"Inception","release_date":"2010-07-15",
                    "belongs_to_collection":null,"production_companies":[{"name":"Legendary Pictures"}],
                    "credits":{"cast":[{"name":"Leonardo DiCaprio"},{"name":"Elliot Page"}],
                               "crew":[{"name":"Christopher Nolan","job":"Director"},{"name":"Hans Zimmer","job":"Original Music Composer"}]}}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/movie/1"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let t = Tmdb::with_base_urls(reqwest::Client::new(), "eyJtoken".into(), &server.uri(), "https://img");
        let m = t.fetch("27205").await.unwrap().unwrap();
        assert_eq!(m.authors, vec!["Christopher Nolan"]);
        assert_eq!(m.narrators, vec!["Leonardo DiCaprio", "Elliot Page"]);
        assert_eq!(m.publisher.as_deref(), Some("Legendary Pictures"));
        assert!(t.fetch("1").await.unwrap().is_none());
        assert!(t.fetch("../x").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn error_status_is_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
        let t = Tmdb::with_base_urls(reqwest::Client::new(), "falsch".into(), &server.uri(), "https://img");
        assert!(t.search("x", 5).await.is_err());
    }
}
