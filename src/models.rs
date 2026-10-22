//! Domänenmodelle.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Book,
    Audiobook,
}

impl MediaType {
    pub const ALL: [MediaType; 2] = [MediaType::Book, MediaType::Audiobook];

    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Book => "book",
            MediaType::Audiobook => "audiobook",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "book" => Some(MediaType::Book),
            "audiobook" => Some(MediaType::Audiobook),
            _ => None,
        }
    }

    pub fn is_audiobook(self) -> bool {
        self == MediaType::Audiobook
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Gelesen bzw. gehört.
    Done,
    /// Möchte ich lesen bzw. hören.
    Wanted,
}

impl Status {
    pub const ALL: [Status; 2] = [Status::Done, Status::Wanted];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Done => "done",
            Status::Wanted => "wanted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "done" => Some(Status::Done),
            "wanted" => Some(Status::Wanted),
            _ => None,
        }
    }

    /// Schlüssel in der Sprachdatei, abhängig vom Medientyp.
    pub fn label_key(self, t: MediaType) -> &'static str {
        match (self, t) {
            (Status::Done, MediaType::Book) => "status-done-book",
            (Status::Done, MediaType::Audiobook) => "status-done-audiobook",
            (Status::Wanted, MediaType::Book) => "status-wanted-book",
            (Status::Wanted, MediaType::Audiobook) => "status-wanted-audiobook",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub id: i64,
    pub sub: String,
    pub name: String,
    pub email: String,
    pub is_admin: bool,
}

/// Ein gespeichertes Medium.
#[derive(Clone, Debug, Serialize)]
pub struct Media {
    pub id: i64,
    pub media_type: MediaType,
    pub title: String,
    pub subtitle: Option<String>,
    pub authors: Vec<String>,
    pub narrators: Vec<String>,
    pub description: Option<String>,
    pub isbn10: Option<String>,
    pub isbn13: Option<String>,
    pub asin: Option<String>,
    pub published_year: Option<i64>,
    pub series: Option<String>,
    pub series_index: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub cover_file: Option<String>,
    pub source: String,
    pub external_id: Option<String>,
    pub created_by: Option<i64>,
}

/// Metadaten eines (noch nicht gespeicherten) Mediums, z. B. aus einem Provider
/// oder aus dem Formular „Manuell anlegen“.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct NewMedia {
    pub media_type: Option<MediaType>,
    pub title: String,
    pub subtitle: Option<String>,
    pub authors: Vec<String>,
    pub narrators: Vec<String>,
    pub description: Option<String>,
    pub isbn10: Option<String>,
    pub isbn13: Option<String>,
    pub asin: Option<String>,
    pub published_year: Option<i64>,
    pub series: Option<String>,
    pub series_index: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub cover_url: Option<String>,
    pub source: String,
    pub external_id: Option<String>,
}

impl NewMedia {
    pub fn kind(&self) -> MediaType {
        self.media_type.unwrap_or(MediaType::Book)
    }
}

/// Persönlicher Eintrag eines Benutzers zu einem Medium.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub status: Status,
    pub status_changed_at: DateTime<Utc>,
    pub added_at: DateTime<Utc>,
}

/// Normalisiert Text für die einfache Volltextsuche (Kleinbuchstaben, Umlaute vereinheitlicht).
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'ä' | 'á' | 'à' | 'â' => out.push('a'),
            'ö' | 'ó' | 'ò' | 'ô' => out.push('o'),
            'ü' | 'ú' | 'ù' | 'û' => out.push('u'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'ß' => out.push_str("ss"),
            c if c.is_alphanumeric() => out.push(c),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Bereinigt eine ISBN (nur Ziffern und X) und gibt sie zurück, wenn sie 10 oder 13 Stellen hat.
pub fn clean_isbn(s: &str) -> Option<String> {
    let c: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    matches!(c.len(), 10 | 13).then_some(c)
}

/// Wandelt eine ISBN-10 in eine ISBN-13 um.
pub fn isbn10_to_13(isbn10: &str) -> Option<String> {
    if isbn10.len() != 10 {
        return None;
    }
    let core = format!("978{}", &isbn10[..9]);
    let sum: u32 = core
        .chars()
        .enumerate()
        .map(|(i, c)| c.to_digit(10).unwrap_or(0) * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    let check = (10 - sum % 10) % 10;
    Some(format!("{core}{check}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_umlauts() {
        assert_eq!(normalize("Die  Straße – Über Bücher!"), "die strasse uber bucher");
    }

    #[test]
    fn isbn() {
        assert_eq!(clean_isbn("978-0-306-40615-7").as_deref(), Some("9780306406157"));
        assert_eq!(clean_isbn("123"), None);
        assert_eq!(isbn10_to_13("0306406152").as_deref(), Some("9780306406157"));
    }
}
