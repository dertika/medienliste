//! Google Books API (bevorzugt deutsche Ausgaben).

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::Deserialize;

use super::{Provider, non_empty, parse_year, strip_html};
use crate::models::{MediaType, NewMedia, clean_isbn};

pub struct GoogleBooks {
    http: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
}

impl GoogleBooks {
    pub fn new(http: reqwest::Client, api_key: Option<String>) -> Self {
        Self::with_base_url(http, api_key, "https://www.googleapis.com/books/v1")
    }

    pub fn with_base_url(http: reqwest::Client, api_key: Option<String>, base_url: &str) -> Self {
        Self { http, api_key, base_url: base_url.trim_end_matches('/').to_string() }
    }

    async fn query(&self, q: &str, limit: usize, german_only: bool) -> Result<Vec<Item>> {
        let mut params = vec![
            ("q", q.to_string()),
            ("maxResults", limit.clamp(1, 40).to_string()),
            ("printType", "books".into()),
            ("hl", "de".into()),
        ];
        if german_only {
            params.push(("langRestrict", "de".into()));
        }
        if let Some(k) = &self.api_key {
            params.push(("key", k.clone()));
        }
        let resp = self.http.get(format!("{}/volumes", self.base_url)).query(&params).send().await?;
        if !resp.status().is_success() {
            bail!("Google Books antwortet mit {}", resp.status());
        }
        let r: SearchResponse = resp.json().await?;
        Ok(r.items)
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    id: String,
    #[serde(rename = "volumeInfo")]
    info: VolumeInfo,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct VolumeInfo {
    title: Option<String>,
    subtitle: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    publisher: Option<String>,
    published_date: Option<String>,
    description: Option<String>,
    #[serde(default)]
    industry_identifiers: Vec<Identifier>,
    image_links: Option<ImageLinks>,
    language: Option<String>,
}

#[derive(Deserialize)]
struct Identifier {
    #[serde(rename = "type")]
    kind: String,
    identifier: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageLinks {
    thumbnail: Option<String>,
    small_thumbnail: Option<String>,
}

fn cover_url(links: &Option<ImageLinks>) -> Option<String> {
    let l = links.as_ref()?;
    let u = l.thumbnail.clone().or_else(|| l.small_thumbnail.clone())?;
    Some(u.replacen("http://", "https://", 1).replace("&edge=curl", ""))
}

fn convert(item: Item) -> Option<NewMedia> {
    let i = item.info;
    let title = non_empty(i.title)?;
    let mut isbn10 = None;
    let mut isbn13 = None;
    for id in &i.industry_identifiers {
        match id.kind.as_str() {
            "ISBN_13" => isbn13 = clean_isbn(&id.identifier),
            "ISBN_10" => isbn10 = clean_isbn(&id.identifier),
            _ => {}
        }
    }
    Some(NewMedia {
        media_type: Some(MediaType::Book),
        title,
        subtitle: non_empty(i.subtitle),
        authors: i.authors,
        narrators: vec![],
        description: i.description.map(|d| strip_html(&d)).filter(|d| !d.is_empty()),
        isbn10,
        isbn13,
        asin: None,
        published_year: i.published_date.as_deref().and_then(parse_year),
        series: None,
        series_index: None,
        language: i.language,
        publisher: non_empty(i.publisher),
        cover_url: cover_url(&i.image_links),
        source: "googlebooks".into(),
        external_id: Some(item.id),
    })
}

#[async_trait]
impl Provider for GoogleBooks {
    fn id(&self) -> &'static str {
        "googlebooks"
    }

    fn media_type(&self) -> MediaType {
        MediaType::Book
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<NewMedia>> {
        let q = match clean_isbn(query) {
            Some(isbn) => format!("isbn:{isbn}"),
            None => query.to_string(),
        };
        // Zuerst deutsche Ausgaben; reicht das nicht, ergänzen um alle Sprachen.
        let mut items = self.query(&q, limit, true).await?;
        if items.len() < limit / 2 {
            let more = self.query(&q, limit, false).await?;
            for it in more {
                if !items.iter().any(|x| x.id == it.id) {
                    items.push(it);
                }
            }
        }
        items.truncate(limit);
        Ok(items.into_iter().filter_map(convert).collect())
    }

    async fn fetch(&self, external_id: &str) -> Result<Option<NewMedia>> {
        let mut req =
            self.http.get(format!("{}/volumes/{}", self.base_url, urlencoding::encode(external_id)));
        if let Some(k) = &self.api_key {
            req = req.query(&[("key", k)]);
        }
        let resp = req.send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("Google Books antwortet mit {}", resp.status());
        }
        Ok(convert(resp.json().await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SAMPLE: &str = r#"{"items":[{"id":"abc","volumeInfo":{"title":"Der Schwarm","authors":["Frank Schätzing"],
        "publishedDate":"2005-03-01","description":"<p>Ein Thriller</p>","language":"de",
        "industryIdentifiers":[{"type":"ISBN_13","identifier":"9783596164530"},{"type":"ISBN_10","identifier":"3596164532"}],
        "imageLinks":{"thumbnail":"http://books.google.com/x?id=abc&zoom=1&edge=curl"}}}]}"#;

    #[tokio::test]
    async fn search_parses_and_prefers_german() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/volumes"))
            .and(query_param("langRestrict", "de"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(SAMPLE, "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        let gb = GoogleBooks::with_base_url(reqwest::Client::new(), None, &server.uri());
        let hits = gb.search("schwarm", 2).await.unwrap();
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!(h.title, "Der Schwarm");
        assert_eq!(h.authors, vec!["Frank Schätzing"]);
        assert_eq!(h.isbn13.as_deref(), Some("9783596164530"));
        assert_eq!(h.published_year, Some(2005));
        assert_eq!(h.description.as_deref(), Some("Ein Thriller"));
        assert_eq!(h.cover_url.as_deref(), Some("https://books.google.com/x?id=abc&zoom=1"));
        assert_eq!(h.external_id.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn error_status_is_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(429)).mount(&server).await;
        let gb = GoogleBooks::with_base_url(reqwest::Client::new(), None, &server.uri());
        assert!(gb.search("x", 10).await.is_err());
    }
}
