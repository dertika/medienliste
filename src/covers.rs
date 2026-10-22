//! Lokaler Cover-Cache unter `<data>/covers`.
//!
//! Dateien werden nach dem SHA-256 ihres Inhalts benannt; das Format wird anhand der
//! Magic Bytes erkannt (nicht anhand des Content-Type).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

pub const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone)]
pub struct CoverStore {
    dir: PathBuf,
    http: reqwest::Client,
}

pub fn detect_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else {
        None
    }
}

/// Erlaubte Cover-Dateinamen: 32 Hex-Zeichen plus Endung.
pub fn valid_file_name(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else { return false };
    stem.len() == 32
        && stem.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        && matches!(ext, "jpg" | "png" | "webp" | "gif")
}

impl CoverStore {
    pub fn new(dir: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent(crate::USER_AGENT)
            .build()
            .expect("HTTP-Client");
        Self { dir, http }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, name: &str) -> Option<PathBuf> {
        valid_file_name(name).then(|| self.dir.join(name))
    }

    /// Speichert Bilddaten und gibt den Dateinamen zurück.
    pub async fn store(&self, bytes: &[u8]) -> Result<String> {
        if bytes.len() > MAX_COVER_BYTES {
            bail!("Cover zu groß");
        }
        let Some(ext) = detect_ext(bytes) else { bail!("kein unterstütztes Bildformat") };
        let hash = hex::encode(Sha256::digest(bytes));
        let name = format!("{}.{ext}", &hash[..32]);
        let path = self.dir.join(&name);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(name);
        }
        crate::create_private_dir(&self.dir)?;
        let tmp = self.dir.join(format!(".{name}.tmp"));
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(name)
    }

    /// Lädt ein Cover von einer Provider-URL herunter und speichert es.
    pub async fn download(&self, url: &str) -> Result<String> {
        let u: url::Url = url.parse()?;
        if !matches!(u.scheme(), "https" | "http") {
            bail!("ungültiges Schema");
        }
        let mut resp = self.http.get(u).send().await?;
        if !resp.status().is_success() {
            bail!("Cover-Download: {}", resp.status());
        }
        if resp.content_length().is_some_and(|l| l as usize > MAX_COVER_BYTES) {
            bail!("Cover zu groß");
        }
        let mut buf = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            buf.extend_from_slice(&chunk);
            if buf.len() > MAX_COVER_BYTES {
                bail!("Cover zu groß");
            }
        }
        self.store(&buf).await
    }

    pub async fn remove(&self, name: &str) {
        if let Some(p) = self.path(name) {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    #[tokio::test]
    async fn store_dedupes_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let s = CoverStore::new(dir.path().join("covers"));
        let a = s.store(PNG).await.unwrap();
        let b = s.store(PNG).await.unwrap();
        assert_eq!(a, b);
        assert!(a.ends_with(".png"));
        assert!(valid_file_name(&a));
        assert!(s.store(b"<svg></svg>").await.is_err());
        assert!(!valid_file_name("../../etc/passwd"));
        assert!(s.path("x.jpg").is_none());
    }
}
