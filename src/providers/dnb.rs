//! Bücher: Deutsche Nationalbibliothek (DNB) über die offene SRU-Schnittstelle (MARC21-XML).
//!
//! Die DNB verzeichnet nahezu alle in Deutschland erschienenen Bücher, liefert aber meist
//! weder Cover noch Klappentext. Cover werden daher per ISBN bei Open Library angefragt.

use anyhow::{Result, bail};
use async_trait::async_trait;

use super::Provider;
use crate::models::{MediaType, NewMedia, clean_isbn, isbn10_to_13, normalize};

const MARC_NS: &str = "http://www.loc.gov/MARC21/slim";

pub struct Dnb {
    http: reqwest::Client,
    base_url: String,
    covers_url: String,
}

impl Dnb {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_base_urls(http, "https://services.dnb.de/sru/dnb", "https://covers.openlibrary.org")
    }

    pub fn with_base_urls(http: reqwest::Client, base_url: &str, covers_url: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            covers_url: covers_url.trim_end_matches('/').to_string(),
        }
    }

    async fn sru(&self, cql: &str, limit: usize) -> Result<Vec<NewMedia>> {
        let resp = self
            .http
            .get(&self.base_url)
            .query(&[
                ("version", "1.1"),
                ("operation", "searchRetrieve"),
                ("recordSchema", "MARC21-xml"),
                ("maximumRecords", &limit.clamp(1, 50).to_string()),
                ("query", cql),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("DNB antwortet mit {}", resp.status());
        }
        let xml = resp.text().await?;
        parse_records(&xml, &self.covers_url)
    }
}

/// CQL-Abfrage: ISBN-Suche oder alle Wörter (UND-verknüpft).
fn build_query(q: &str) -> Option<String> {
    if let Some(isbn) = clean_isbn(q) {
        return Some(format!("num={isbn}"));
    }
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|t| t.chars().filter(|c| c.is_alphanumeric() || *c == '-').collect::<String>())
        .filter(|t| !t.is_empty())
        .take(8)
        .map(|t| format!("woe={t}"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" and "))
}

/// Entfernt DNB-Sortierzeichen (Nichtsortier-Markierungen) und Satzzeichen am Ende.
fn clean_text(s: &str) -> String {
    let s: String = s.chars().filter(|c| !matches!(c, '\u{0098}' | '\u{009C}' | '¬')).collect();
    s.trim().trim_end_matches([' ', '/', ':', ';', ',', '=', '.']).trim().to_string()
}

/// „Schätzing, Frank“ → „Frank Schätzing“.
fn person_name(s: &str) -> String {
    let s = clean_text(s);
    match s.split_once(", ") {
        Some((last, first)) if !first.contains(',') => format!("{first} {last}"),
        _ => s,
    }
}

fn first_year(s: &str) -> Option<i64> {
    let digits: Vec<char> = s.chars().collect();
    digits.windows(4).find(|w| w.iter().all(|c| c.is_ascii_digit())).and_then(|w| {
        let y: String = w.iter().collect();
        y.parse().ok()
    })
}

fn lang_code(l: &str) -> String {
    match l {
        "ger" | "deu" => "de".into(),
        "eng" => "en".into(),
        "fre" | "fra" => "fr".into(),
        "spa" => "es".into(),
        "ita" => "it".into(),
        other => other.into(),
    }
}

fn parse_records(xml: &str, covers_url: &str) -> Result<Vec<NewMedia>> {
    let doc = roxmltree::Document::parse(xml)?;
    let mut out: Vec<NewMedia> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for rec in doc.descendants().filter(|n| n.has_tag_name((MARC_NS, "record"))) {
        let leader =
            rec.children().find(|n| n.has_tag_name((MARC_NS, "leader"))).and_then(|n| n.text()).unwrap_or("");
        // Nur Textwerke (Leader/06 = a); Tonträger, Noten usw. überspringen.
        if leader.chars().nth(6).is_some_and(|c| c != 'a') {
            continue;
        }
        let control = |tag: &str| {
            rec.children()
                .find(|n| n.has_tag_name((MARC_NS, "controlfield")) && n.attribute("tag") == Some(tag))
                .and_then(|n| n.text())
                .map(str::to_string)
        };
        // E-Books (007 beginnt mit „cr“ = Online-Ressource) überspringen.
        if control("007").is_some_and(|v| v.starts_with("cr")) {
            continue;
        }
        let fields = |tag: &'static str| {
            rec.children()
                .filter(move |n| n.has_tag_name((MARC_NS, "datafield")) && n.attribute("tag") == Some(tag))
        };
        let sub = |field: roxmltree::Node, code: &str| -> Option<String> {
            field
                .children()
                .find(|n| n.has_tag_name((MARC_NS, "subfield")) && n.attribute("code") == Some(code))
                .and_then(|n| n.text())
                .map(str::to_string)
        };

        let Some(idn) = control("001") else { continue };
        let Some(title_field) = fields("245").next() else { continue };
        let Some(title) = sub(title_field, "a").map(|t| clean_text(&t)).filter(|t| !t.is_empty()) else {
            continue;
        };
        let subtitle = sub(title_field, "b").map(|t| clean_text(&t)).filter(|t| !t.is_empty());

        let mut authors: Vec<String> =
            fields("100").filter_map(|f| sub(f, "a")).map(|a| person_name(&a)).collect();
        for f in fields("700") {
            let is_author = f
                .children()
                .filter(|n| n.attribute("code") == Some("4"))
                .any(|n| n.text().is_some_and(|t| t.ends_with("aut")));
            if is_author && let Some(a) = sub(f, "a") {
                authors.push(person_name(&a));
            }
        }

        let (mut isbn10, mut isbn13) = (None, None);
        for f in fields("020") {
            let Some(raw) = sub(f, "a").or_else(|| sub(f, "9")) else { continue };
            let first = raw.split([' ', ';', ':', '(']).next().unwrap_or("");
            match clean_isbn(first) {
                Some(i) if i.len() == 13 && isbn13.is_none() => isbn13 = Some(i),
                Some(i) if i.len() == 10 && isbn10.is_none() => isbn10 = Some(i),
                _ => {}
            }
        }
        if isbn13.is_none() {
            isbn13 = isbn10.as_deref().and_then(isbn10_to_13);
        }

        let imprint = fields("264").next().or_else(|| fields("260").next());
        let publisher = imprint.and_then(|f| sub(f, "b")).map(|p| clean_text(&p)).filter(|p| !p.is_empty());
        let published_year = imprint.and_then(|f| sub(f, "c")).as_deref().and_then(first_year);
        let language = fields("041").next().and_then(|f| sub(f, "a")).map(|l| lang_code(&l));
        let description = fields("520").next().and_then(|f| sub(f, "a")).map(|d| clean_text(&d));

        // Mehrere Ausgaben desselben Werks (Hardcover, Taschenbuch …) nur einmal zeigen.
        let key =
            format!("{}|{}", normalize(&title), authors.first().map(|a| normalize(a)).unwrap_or_default());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);

        let cover_url = isbn13.as_ref().map(|i| format!("{covers_url}/b/isbn/{i}-L.jpg?default=false"));
        out.push(NewMedia {
            media_type: Some(MediaType::Book),
            title,
            subtitle,
            authors,
            description,
            isbn10,
            isbn13,
            published_year,
            language,
            publisher,
            cover_url,
            source: "dnb".into(),
            external_id: Some(idn),
            ..Default::default()
        });
    }
    Ok(out)
}

#[async_trait]
impl Provider for Dnb {
    fn id(&self) -> &'static str {
        "dnb"
    }

    fn media_type(&self) -> MediaType {
        MediaType::Book
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<NewMedia>> {
        let Some(cql) = build_query(query) else { return Ok(vec![]) };
        // Mehr Datensätze holen, weil mehrere Ausgaben zusammengefasst werden.
        let mut hits = self.sru(&cql, limit * 2).await?;
        hits.truncate(limit);
        Ok(hits)
    }

    async fn fetch(&self, external_id: &str) -> Result<Option<NewMedia>> {
        if external_id.is_empty() || !external_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Ok(None);
        }
        Ok(self.sru(&format!("idn={external_id}"), 1).await?.into_iter().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn record(idn: &str, leader6: char, extra_007: &str, title: &str, isbn: &str) -> String {
        format!(
            r#"<record xmlns="http://www.loc.gov/MARC21/slim" type="Bibliographic">
  <leader>00000n{leader6}m  2200000 c 4500</leader>
  <controlfield tag="001">{idn}</controlfield>
  {extra_007}
  <datafield tag="020" ind1=" " ind2=" "><subfield code="a">{isbn}</subfield><subfield code="c">kart. : EUR 12.00</subfield></datafield>
  <datafield tag="041" ind1=" " ind2=" "><subfield code="a">ger</subfield></datafield>
  <datafield tag="100" ind1="1" ind2=" "><subfield code="a">Schätzing, Frank</subfield><subfield code="4">aut</subfield></datafield>
  <datafield tag="245" ind1="1" ind2="0"><subfield code="a">{title} :</subfield><subfield code="b">Roman /</subfield></datafield>
  <datafield tag="264" ind1=" " ind2="1"><subfield code="a">Frankfurt am Main</subfield><subfield code="b">Fischer Taschenbuch,</subfield><subfield code="c">[2005]</subfield></datafield>
  <datafield tag="700" ind1="1" ind2=" "><subfield code="a">Muster, Erika</subfield><subfield code="4">https://d-nb.info/standards/elementset/gnd#aut</subfield></datafield>
  <datafield tag="700" ind1="1" ind2=" "><subfield code="a">Übersetzer, Ute</subfield><subfield code="4">trl</subfield></datafield>
</record>"#
        )
    }

    fn response(records: &[String]) -> String {
        let recs: String = records
            .iter()
            .map(|r| {
                format!(
                    "<record><recordSchema>MARC21-xml</recordSchema><recordData>{r}</recordData></record>"
                )
            })
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><searchRetrieveResponse xmlns="http://www.loc.gov/zing/srw/"><version>1.1</version><numberOfRecords>{}</numberOfRecords><records>{recs}</records></searchRetrieveResponse>"#,
            records.len()
        )
    }

    #[test]
    fn query_building() {
        assert_eq!(build_query("Der Schwarm").as_deref(), Some("woe=Der and woe=Schwarm"));
        assert_eq!(build_query("978-3-596-16453-0").as_deref(), Some("num=9783596164530"));
        assert_eq!(build_query("\"; drop").as_deref(), Some("woe=drop"));
        assert_eq!(build_query("  ").as_deref(), None);
    }

    #[test]
    fn names_and_titles() {
        assert_eq!(person_name("Schätzing, Frank"), "Frank Schätzing");
        assert_eq!(person_name("Goethe"), "Goethe");
        assert_eq!(clean_text("\u{98}Der\u{9c} Schwarm :"), "Der Schwarm");
        assert_eq!(clean_text("¬Die¬ Vermessung der Welt /"), "Die Vermessung der Welt");
    }

    #[tokio::test]
    async fn search_parses_dedupes_and_filters() {
        let server = MockServer::start().await;
        let body = response(&[
            record("974434868", 'a', "", "\u{98}Der\u{9c} Schwarm", "978-3-596-16453-0"),
            // zweite Ausgabe desselben Werks → zusammengefasst
            record("1001", 'a', "", "Der Schwarm", "3-462-03374-X"),
            // E-Book → übersprungen
            record(
                "1002",
                'a',
                r#"<controlfield tag="007">cr||||||||||||</controlfield>"#,
                "Limit",
                "9783462000001",
            ),
            // Tonträger → übersprungen
            record("1003", 'i', "", "Hörspiel", "9783462000002"),
        ]);
        Mock::given(method("GET"))
            .and(path("/sru"))
            .and(query_param("query", "woe=schwarm"))
            .and(query_param("recordSchema", "MARC21-xml"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/xml"))
            .mount(&server)
            .await;
        let dnb = Dnb::with_base_urls(reqwest::Client::new(), &format!("{}/sru", server.uri()), "https://c");
        let hits = dnb.search("schwarm", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "{hits:#?}");
        let h = &hits[0];
        assert_eq!(h.title, "Der Schwarm");
        assert_eq!(h.subtitle.as_deref(), Some("Roman"));
        assert_eq!(h.authors, vec!["Frank Schätzing", "Erika Muster"], "Übersetzer nicht als Autor");
        assert_eq!(h.isbn13.as_deref(), Some("9783596164530"));
        assert_eq!(h.published_year, Some(2005));
        assert_eq!(h.publisher.as_deref(), Some("Fischer Taschenbuch"));
        assert_eq!(h.language.as_deref(), Some("de"));
        assert_eq!(h.external_id.as_deref(), Some("974434868"));
        assert_eq!(h.source, "dnb");
        assert_eq!(h.cover_url.as_deref(), Some("https://c/b/isbn/9783596164530-L.jpg?default=false"));
    }

    #[tokio::test]
    async fn fetch_by_idn_and_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("query", "idn=974434868"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                response(&[record("974434868", 'a', "", "Der Schwarm", "9783596164530")]),
                "application/xml",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("query", "woe=kaputt"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let dnb = Dnb::with_base_urls(reqwest::Client::new(), &server.uri(), "https://c");
        assert_eq!(dnb.fetch("974434868").await.unwrap().unwrap().title, "Der Schwarm");
        assert!(dnb.fetch("../x").await.unwrap().is_none());
        assert!(dnb.search("kaputt", 5).await.is_err());
    }
}
