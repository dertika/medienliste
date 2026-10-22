//! Open Library (Fallback für Bücher).

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::Deserialize;

use super::{Provider, non_empty, parse_year, strip_html};
use crate::models::{MediaType, NewMedia, clean_isbn};

pub struct OpenLibrary {
    http: reqwest::Client,
    base_url: String,
    covers_url: String,
}

impl OpenLibrary {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_base_url(http, "https://openlibrary.org", "https://covers.openlibrary.org")
    }

    pub fn with_base_url(http: reqwest::Client, base_url: &str, covers_url: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            covers_url: covers_url.trim_end_matches('/').to_string(),
        }
    }

    fn cover(&self, id: Option<i64>) -> Option<String> {
        id.filter(|i| *i > 0).map(|i| format!("{}/b/id/{i}-L.jpg", self.covers_url))
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    docs: Vec<Doc>,
}

#[derive(Deserialize)]
struct Doc {
    key: String,
    title: Option<String>,
    subtitle: Option<String>,
    #[serde(default)]
    author_name: Vec<String>,
    first_publish_year: Option<i64>,
    #[serde(default)]
    isbn: Vec<String>,
    cover_i: Option<i64>,
    #[serde(default)]
    language: Vec<String>,
    #[serde(default)]
    publisher: Vec<String>,
}

#[derive(Deserialize)]
struct Work {
    title: Option<String>,
    subtitle: Option<String>,
    description: Option<TextOrValue>,
    #[serde(default)]
    covers: Vec<i64>,
    #[serde(default)]
    authors: Vec<WorkAuthor>,
    first_publish_date: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TextOrValue {
    Text(String),
    Value { value: String },
}

#[derive(Deserialize)]
struct WorkAuthor {
    author: Option<KeyRef>,
}

#[derive(Deserialize)]
struct KeyRef {
    key: String,
}

#[derive(Deserialize)]
struct Author {
    name: Option<String>,
}

/// Wählt bevorzugt eine deutsche ISBN-13 (978-3 / 979-1?), sonst die erste ISBN-13.
fn pick_isbn(list: &[String]) -> (Option<String>, Option<String>) {
    let cleaned: Vec<String> = list.iter().filter_map(|i| clean_isbn(i)).collect();
    let isbn13 = cleaned
        .iter()
        .find(|i| i.len() == 13 && i.starts_with("9783"))
        .or_else(|| cleaned.iter().find(|i| i.len() == 13))
        .cloned();
    let isbn10 = cleaned.iter().find(|i| i.len() == 10).cloned();
    (isbn13, isbn10)
}

#[async_trait]
impl Provider for OpenLibrary {
    fn id(&self) -> &'static str {
        "openlibrary"
    }

    fn media_type(&self) -> MediaType {
        MediaType::Book
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<NewMedia>> {
        let mut params = vec![
            ("limit", limit.clamp(1, 50).to_string()),
            ("lang", "de".into()),
            (
                "fields",
                "key,title,subtitle,author_name,first_publish_year,isbn,cover_i,language,publisher".into(),
            ),
        ];
        match clean_isbn(query) {
            Some(isbn) => params.push(("isbn", isbn)),
            None => params.push(("q", query.to_string())),
        }
        let resp = self.http.get(format!("{}/search.json", self.base_url)).query(&params).send().await?;
        if !resp.status().is_success() {
            bail!("Open Library antwortet mit {}", resp.status());
        }
        let r: SearchResponse = resp.json().await?;
        let mut docs = r.docs;
        // Deutsche Werke nach vorne (stabile Sortierung).
        docs.sort_by_key(|d| !d.language.iter().any(|l| l == "ger"));
        Ok(docs
            .into_iter()
            .filter_map(|d| {
                let title = non_empty(d.title)?;
                let (isbn13, isbn10) = pick_isbn(&d.isbn);
                Some(NewMedia {
                    media_type: Some(MediaType::Book),
                    title,
                    subtitle: non_empty(d.subtitle),
                    authors: d.author_name,
                    published_year: d.first_publish_year,
                    isbn13,
                    isbn10,
                    language: d.language.iter().any(|l| l == "ger").then(|| "de".to_string()),
                    publisher: d.publisher.into_iter().next(),
                    cover_url: self.cover(d.cover_i),
                    source: "openlibrary".into(),
                    external_id: Some(d.key.trim_start_matches("/works/").to_string()),
                    ..Default::default()
                })
            })
            .collect())
    }

    async fn fetch(&self, external_id: &str) -> Result<Option<NewMedia>> {
        if !external_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Ok(None);
        }
        let resp = self.http.get(format!("{}/works/{external_id}.json", self.base_url)).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("Open Library antwortet mit {}", resp.status());
        }
        let w: Work = resp.json().await?;
        let Some(title) = non_empty(w.title) else { return Ok(None) };
        let mut authors = vec![];
        for a in w.authors.iter().filter_map(|a| a.author.as_ref()).take(5) {
            if !a.key.starts_with("/authors/") {
                continue;
            }
            if let Ok(r) = self.http.get(format!("{}{}.json", self.base_url, a.key)).send().await
                && let Ok(Author { name: Some(n) }) = r.json::<Author>().await
            {
                authors.push(n);
            }
        }
        Ok(Some(NewMedia {
            media_type: Some(MediaType::Book),
            title,
            subtitle: non_empty(w.subtitle),
            authors,
            description: w.description.map(|d| match d {
                TextOrValue::Text(t) | TextOrValue::Value { value: t } => strip_html(&t),
            }),
            published_year: w.first_publish_date.as_deref().and_then(|d| {
                d.split(|c: char| !c.is_ascii_digit()).find(|p| p.len() == 4).and_then(parse_year)
            }),
            cover_url: self.cover(w.covers.first().copied()),
            source: "openlibrary".into(),
            external_id: Some(external_id.to_string()),
            ..Default::default()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn search_sorts_german_first() {
        let server = MockServer::start().await;
        let body = r#"{"docs":[
            {"key":"/works/OL1W","title":"The Swarm","author_name":["Frank Schätzing"],"language":["eng"],"isbn":["0060813261"],"cover_i":5},
            {"key":"/works/OL2W","title":"Der Schwarm","author_name":["Frank Schätzing"],"language":["ger"],"isbn":["9780000000002","978-3-596-16453-0"],"first_publish_year":2004}
        ]}"#;
        Mock::given(method("GET"))
            .and(path("/search.json"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let ol = OpenLibrary::with_base_url(reqwest::Client::new(), &server.uri(), "https://c");
        let hits = ol.search("schwarm", 10).await.unwrap();
        assert_eq!(hits[0].title, "Der Schwarm");
        assert_eq!(hits[0].isbn13.as_deref(), Some("9783596164530"));
        assert_eq!(hits[0].external_id.as_deref(), Some("OL2W"));
        assert_eq!(hits[1].cover_url.as_deref(), Some("https://c/b/id/5-L.jpg"));
        assert_eq!(hits[1].isbn10.as_deref(), Some("0060813261"));
    }

    #[tokio::test]
    async fn fetch_work_with_authors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works/OL2W.json"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"title":"Der Schwarm","description":{"value":"Text"},"covers":[7],"authors":[{"author":{"key":"/authors/OL9A"}}]}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/authors/OL9A.json"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(r#"{"name":"Frank Schätzing"}"#, "application/json"),
            )
            .mount(&server)
            .await;
        let ol = OpenLibrary::with_base_url(reqwest::Client::new(), &server.uri(), "https://c");
        let m = ol.fetch("OL2W").await.unwrap().unwrap();
        assert_eq!(m.authors, vec!["Frank Schätzing"]);
        assert_eq!(m.description.as_deref(), Some("Text"));
        assert!(ol.fetch("../etc").await.unwrap().is_none());
    }
}
