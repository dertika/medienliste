//! Hörbücher: Suche über den Audible-Katalog (Marktplatz, Standard .de),
//! Details bevorzugt über Audnexus (Sprecher, Serie, großes Cover).

use std::collections::HashMap;

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::Deserialize;

use super::{Provider, non_empty, parse_year, strip_html};
use crate::models::{MediaType, NewMedia};

const RESPONSE_GROUPS: &str = "contributors,media,product_attrs,product_desc,product_extended_attrs,series";

pub struct Audible {
    http: reqwest::Client,
    catalog_url: String,
    audnexus_url: String,
    region: String,
}

/// Marktplatz-Kürzel → (API-Host, Audnexus-Region).
fn marketplace(code: &str) -> (&'static str, &'static str) {
    match code {
        "us" | "com" => ("api.audible.com", "us"),
        "uk" | "co.uk" => ("api.audible.co.uk", "uk"),
        "fr" => ("api.audible.fr", "fr"),
        "it" => ("api.audible.it", "it"),
        "es" => ("api.audible.es", "es"),
        "ca" => ("api.audible.ca", "ca"),
        "au" | "com.au" => ("api.audible.com.au", "au"),
        "in" => ("api.audible.in", "in"),
        "jp" | "co.jp" => ("api.audible.co.jp", "jp"),
        _ => ("api.audible.de", "de"),
    }
}

impl Audible {
    pub fn new(http: reqwest::Client, marketplace_code: &str) -> Self {
        let (host, region) = marketplace(marketplace_code);
        Self::with_base_urls(http, &format!("https://{host}"), "https://api.audnex.us", region)
    }

    pub fn with_base_urls(
        http: reqwest::Client,
        catalog_url: &str,
        audnexus_url: &str,
        region: &str,
    ) -> Self {
        Self {
            http,
            catalog_url: catalog_url.trim_end_matches('/').to_string(),
            audnexus_url: audnexus_url.trim_end_matches('/').to_string(),
            region: region.to_string(),
        }
    }

    async fn fetch_audnexus(&self, asin: &str) -> Result<Option<NewMedia>> {
        let resp = self
            .http
            .get(format!("{}/books/{asin}", self.audnexus_url))
            .query(&[("region", &self.region)])
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("Audnexus antwortet mit {}", resp.status());
        }
        let b: AudnexusBook = resp.json().await?;
        Ok(b.into_new_media())
    }

    async fn fetch_catalog(&self, asin: &str) -> Result<Option<NewMedia>> {
        let resp = self
            .http
            .get(format!("{}/1.0/catalog/products/{asin}", self.catalog_url))
            .query(&[("response_groups", RESPONSE_GROUPS), ("image_sizes", "500")])
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("Audible antwortet mit {}", resp.status());
        }
        let r: ProductResponse = resp.json().await?;
        Ok(r.product.and_then(Product::into_new_media))
    }
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    products: Vec<Product>,
}

#[derive(Deserialize)]
struct ProductResponse {
    product: Option<Product>,
}

#[derive(Deserialize)]
struct Product {
    asin: String,
    title: Option<String>,
    subtitle: Option<String>,
    #[serde(default)]
    authors: Vec<Named>,
    #[serde(default)]
    narrators: Vec<Named>,
    #[serde(default)]
    product_images: HashMap<String, String>,
    release_date: Option<String>,
    issue_date: Option<String>,
    #[serde(default)]
    series: Vec<SeriesRef>,
    merchandising_summary: Option<String>,
    publisher_summary: Option<String>,
    publisher_name: Option<String>,
    language: Option<String>,
    content_delivery_type: Option<String>,
}

#[derive(Deserialize)]
struct SeriesRef {
    title: Option<String>,
    sequence: Option<String>,
}

fn lang_code(l: Option<String>) -> Option<String> {
    let l = l?.to_lowercase();
    Some(match l.as_str() {
        "german" | "deutsch" => "de".into(),
        "english" => "en".into(),
        "french" => "fr".into(),
        "spanish" => "es".into(),
        "italian" => "it".into(),
        _ => l,
    })
}

impl Product {
    fn into_new_media(self) -> Option<NewMedia> {
        if self.content_delivery_type.as_deref().is_some_and(|t| t.starts_with("Podcast")) {
            return None;
        }
        let title = non_empty(self.title)?;
        let series = self.series.into_iter().next();
        let cover =
            self.product_images.get("500").cloned().or_else(|| self.product_images.into_values().next());
        Some(NewMedia {
            media_type: Some(MediaType::Audiobook),
            title,
            subtitle: non_empty(self.subtitle),
            authors: self.authors.into_iter().map(|a| a.name).collect(),
            narrators: self.narrators.into_iter().map(|a| a.name).collect(),
            description: self
                .publisher_summary
                .or(self.merchandising_summary)
                .map(|d| strip_html(&d))
                .filter(|d| !d.is_empty()),
            asin: Some(self.asin.clone()),
            published_year: self.release_date.or(self.issue_date).as_deref().and_then(parse_year),
            series: series.as_ref().and_then(|s| non_empty(s.title.clone())),
            series_index: series.and_then(|s| non_empty(s.sequence)),
            language: lang_code(self.language),
            publisher: non_empty(self.publisher_name),
            cover_url: cover,
            source: "audible".into(),
            external_id: Some(self.asin),
            ..Default::default()
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudnexusBook {
    asin: String,
    title: Option<String>,
    subtitle: Option<String>,
    #[serde(default)]
    authors: Vec<Named>,
    #[serde(default)]
    narrators: Vec<Named>,
    image: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    release_date: Option<String>,
    series_primary: Option<AudnexusSeries>,
    language: Option<String>,
    publisher_name: Option<String>,
    isbn: Option<String>,
}

#[derive(Deserialize)]
struct AudnexusSeries {
    name: Option<String>,
    position: Option<String>,
}

impl AudnexusBook {
    fn into_new_media(self) -> Option<NewMedia> {
        let title = non_empty(self.title)?;
        Some(NewMedia {
            media_type: Some(MediaType::Audiobook),
            title,
            subtitle: non_empty(self.subtitle),
            authors: self.authors.into_iter().map(|a| a.name).collect(),
            narrators: self.narrators.into_iter().map(|a| a.name).collect(),
            description: self.summary.or(self.description).map(|d| strip_html(&d)).filter(|d| !d.is_empty()),
            isbn13: self.isbn.as_deref().and_then(crate::models::clean_isbn).filter(|i| i.len() == 13),
            asin: Some(self.asin.clone()),
            published_year: self.release_date.as_deref().and_then(parse_year),
            series: self.series_primary.as_ref().and_then(|s| non_empty(s.name.clone())),
            series_index: self.series_primary.and_then(|s| non_empty(s.position)),
            language: lang_code(self.language),
            publisher: non_empty(self.publisher_name),
            cover_url: self.image,
            source: "audible".into(),
            external_id: Some(self.asin),
            ..Default::default()
        })
    }
}

fn valid_asin(s: &str) -> bool {
    s.len() == 10 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

#[async_trait]
impl Provider for Audible {
    fn id(&self) -> &'static str {
        "audible"
    }

    fn media_type(&self) -> MediaType {
        MediaType::Audiobook
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<NewMedia>> {
        let resp = self
            .http
            .get(format!("{}/1.0/catalog/products", self.catalog_url))
            .query(&[
                ("keywords", query),
                ("num_results", &limit.clamp(1, 50).to_string()),
                ("products_sort_by", "Relevance"),
                ("response_groups", RESPONSE_GROUPS),
                ("image_sizes", "500"),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("Audible antwortet mit {}", resp.status());
        }
        let r: SearchResponse = resp.json().await?;
        Ok(r.products.into_iter().filter_map(Product::into_new_media).collect())
    }

    async fn fetch(&self, asin: &str) -> Result<Option<NewMedia>> {
        let asin = asin.to_ascii_uppercase();
        if !valid_asin(&asin) {
            return Ok(None);
        }
        match self.fetch_audnexus(&asin).await {
            Ok(Some(m)) => Ok(Some(m)),
            Ok(None) => self.fetch_catalog(&asin).await,
            Err(e) => {
                tracing::info!("Audnexus nicht verfügbar ({e:#}), nutze Audible-Katalog");
                self.fetch_catalog(&asin).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn search_catalog() {
        let server = MockServer::start().await;
        let body = r#"{"products":[
          {"asin":"B004V0L2VG","title":"Der Schwarm","authors":[{"asin":"x","name":"Frank Schätzing"}],
           "narrators":[{"name":"Ulrich Pleitgen"}],"product_images":{"500":"https://m.media-amazon.com/x.jpg"},
           "release_date":"2011-04-01","series":[{"asin":"s","title":"Schwarm-Reihe","sequence":"1"}],
           "publisher_summary":"<p>Spannend</p>","publisher_name":"Argon","language":"german","content_delivery_type":"SinglePartBook"},
          {"asin":"B000000000","title":"Ein Podcast","content_delivery_type":"PodcastParent"}
        ]}"#;
        Mock::given(method("GET"))
            .and(path("/1.0/catalog/products"))
            .and(query_param("keywords", "schwarm"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let a = Audible::with_base_urls(reqwest::Client::new(), &server.uri(), &server.uri(), "de");
        let hits = a.search("schwarm", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!(h.media_type, Some(MediaType::Audiobook));
        assert_eq!(h.narrators, vec!["Ulrich Pleitgen"]);
        assert_eq!(h.series.as_deref(), Some("Schwarm-Reihe"));
        assert_eq!(h.series_index.as_deref(), Some("1"));
        assert_eq!(h.language.as_deref(), Some("de"));
        assert_eq!(h.asin.as_deref(), Some("B004V0L2VG"));
        assert_eq!(h.description.as_deref(), Some("Spannend"));
    }

    #[tokio::test]
    async fn fetch_prefers_audnexus_and_falls_back() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/books/B004V0L2VG"))
            .and(query_param("region", "de"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"asin":"B004V0L2VG","title":"Der Schwarm","narrators":[{"name":"Ulrich Pleitgen"}],
                    "image":"https://img/x.jpg","releaseDate":"2011-04-01T00:00:00.000Z",
                    "seriesPrimary":{"name":"Reihe","position":"2"},"language":"german"}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/books/B000000001"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/1.0/catalog/products/B000000001"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    r#"{"product":{"asin":"B000000001","title":"Fallback"}}"#,
                    "application/json",
                ),
            )
            .mount(&server)
            .await;
        let a = Audible::with_base_urls(reqwest::Client::new(), &server.uri(), &server.uri(), "de");
        let m = a.fetch("B004V0L2VG").await.unwrap().unwrap();
        assert_eq!(m.series_index.as_deref(), Some("2"));
        assert_eq!(m.published_year, Some(2011));
        assert_eq!(m.cover_url.as_deref(), Some("https://img/x.jpg"));
        let f = a.fetch("b000000001").await.unwrap().unwrap();
        assert_eq!(f.title, "Fallback");
        assert!(a.fetch("../x").await.unwrap().is_none());
    }
}
