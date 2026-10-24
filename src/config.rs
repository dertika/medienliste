//! Konfiguration aus Umgebungsvariablen.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use chrono_tz::Tz;
use ipnet::IpNet;
use url::Url;

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: SocketAddr,
    pub base_url: Url,
    /// Ziel nach dem Abmelden (z. B. eine Portalseite oder der Logout des Identity Providers).
    pub logout_url: Option<Url>,
    pub data_dir: PathBuf,
    pub timezone: Tz,
    pub oidc: OidcConfig,
    pub google_books_api_key: Option<String>,
    pub tmdb_api_key: Option<String>,
    pub audible_marketplace: String,
    pub trusted_proxies: Vec<IpNet>,
    pub session_days: i64,
    pub backup_interval_hours: u64,
    pub backup_keep: usize,
}

#[derive(Clone, Debug)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    pub admin_group: Option<String>,
    /// Nur Mitglieder dieser Gruppe (oder der Admin-Gruppe) dürfen sich anmelden.
    pub required_group: Option<String>,
}

impl Config {
    /// Liest die Konfiguration aus der Prozessumgebung.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Liest die Konfiguration über eine beliebige Lookup-Funktion (testbar).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let required = |k: &str| get(k).ok_or_else(|| anyhow!("Umgebungsvariable {k} fehlt"));

        let mut base_url: Url = required("MEDIENLISTE_BASE_URL")?
            .parse()
            .context("MEDIENLISTE_BASE_URL ist keine gültige URL")?;
        if !base_url.path().ends_with('/') {
            let p = format!("{}/", base_url.path());
            base_url.set_path(&p);
        }

        let logout_url = match get("MEDIENLISTE_LOGOUT_URL") {
            Some(u) => {
                let u: Url = u.parse().context("MEDIENLISTE_LOGOUT_URL ist keine gültige URL")?;
                if !matches!(u.scheme(), "https" | "http") {
                    bail!("MEDIENLISTE_LOGOUT_URL muss mit https:// oder http:// beginnen");
                }
                Some(u)
            }
            None => None,
        };

        let listen = get("MEDIENLISTE_LISTEN")
            .unwrap_or_else(|| "0.0.0.0:8080".into())
            .parse()
            .context("MEDIENLISTE_LISTEN muss die Form host:port haben")?;

        let redirect_url = match get("MEDIENLISTE_OIDC_REDIRECT_URL") {
            Some(u) => u,
            None => base_url.join("auth/callback")?.to_string(),
        };

        let scopes = get("MEDIENLISTE_OIDC_SCOPES")
            .unwrap_or_else(|| "openid profile email groups".into())
            .split([' ', ','])
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        let trusted_proxies = get("MEDIENLISTE_TRUSTED_PROXIES")
            .unwrap_or_else(|| "127.0.0.0/8,::1/128,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,fc00::/7".into())
            .split([' ', ','])
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<IpNet>()
                    .or_else(|_| s.parse::<std::net::IpAddr>().map(IpNet::from))
                    .with_context(|| format!("MEDIENLISTE_TRUSTED_PROXIES: ungültiger Eintrag {s}"))
            })
            .collect::<Result<Vec<_>>>()?;

        let timezone: Tz = get("MEDIENLISTE_TIMEZONE")
            .unwrap_or_else(|| "Europe/Berlin".into())
            .parse()
            .map_err(|e| anyhow!("MEDIENLISTE_TIMEZONE: {e}"))?;

        let parse_num = |k: &str, default: u64| -> Result<u64> {
            match get(k) {
                Some(v) => v.parse().with_context(|| format!("{k} muss eine Zahl sein")),
                None => Ok(default),
            }
        };

        let session_days = parse_num("MEDIENLISTE_SESSION_DAYS", 30)? as i64;
        if session_days == 0 {
            bail!("MEDIENLISTE_SESSION_DAYS muss größer als 0 sein");
        }

        Ok(Config {
            listen,
            base_url,
            logout_url,
            data_dir: get("MEDIENLISTE_DATA_DIR").unwrap_or_else(|| "/data".into()).into(),
            timezone,
            oidc: OidcConfig {
                issuer: required("MEDIENLISTE_OIDC_ISSUER")?,
                client_id: required("MEDIENLISTE_OIDC_CLIENT_ID")?,
                client_secret: get("MEDIENLISTE_OIDC_CLIENT_SECRET"),
                redirect_url,
                scopes,
                admin_group: get("MEDIENLISTE_OIDC_ADMIN_GROUP"),
                required_group: get("MEDIENLISTE_OIDC_REQUIRED_GROUP"),
            },
            google_books_api_key: get("MEDIENLISTE_GOOGLE_BOOKS_API_KEY"),
            tmdb_api_key: get("MEDIENLISTE_TMDB_API_KEY"),
            audible_marketplace: get("MEDIENLISTE_AUDIBLE_MARKETPLACE").unwrap_or_else(|| "de".into()),
            trusted_proxies,
            session_days,
            backup_interval_hours: parse_num("MEDIENLISTE_BACKUP_INTERVAL_HOURS", 24)?,
            backup_keep: parse_num("MEDIENLISTE_BACKUP_KEEP", 7)? as usize,
        })
    }

    /// Minimal-Konfiguration für Hilfsbefehle (z. B. `backup`), die kein OIDC brauchen.
    pub fn from_env_for_tools() -> Result<Self> {
        Self::from_lookup(|k| {
            std::env::var(k).ok().or_else(|| match k {
                "MEDIENLISTE_BASE_URL" => Some("http://localhost/".into()),
                "MEDIENLISTE_OIDC_ISSUER" | "MEDIENLISTE_OIDC_CLIENT_ID" => Some("unbenutzt".into()),
                _ => None,
            })
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("medienliste.sqlite3")
    }

    pub fn covers_dir(&self) -> PathBuf {
        self.data_dir.join("covers")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    /// Cookies nur mit `Secure`, wenn die öffentliche URL HTTPS ist.
    pub fn secure_cookies(&self) -> bool {
        self.base_url.scheme() == "https"
    }

    /// Absoluter Pfad unterhalb der Basis-URL, z. B. `/` oder `/medienliste/`.
    pub fn base_path(&self) -> &str {
        self.base_url.path()
    }

    /// Erlaubter `Origin`-Header (ohne Pfad) für CSRF-Prüfung.
    pub fn origin(&self) -> String {
        self.base_url.origin().ascii_serialization()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> Result<Config> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Config::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn defaults_and_redirect() {
        let c = cfg(&[
            ("MEDIENLISTE_BASE_URL", "https://medienliste.example.org"),
            ("MEDIENLISTE_OIDC_ISSUER", "https://auth.example.org"),
            ("MEDIENLISTE_OIDC_CLIENT_ID", "medienliste"),
        ])
        .unwrap();
        assert_eq!(c.oidc.redirect_url, "https://medienliste.example.org/auth/callback");
        assert_eq!(c.origin(), "https://medienliste.example.org");
        assert!(c.secure_cookies());
        assert_eq!(c.oidc.scopes, vec!["openid", "profile", "email", "groups"]);
        assert_eq!(c.backup_interval_hours, 24);
    }

    #[test]
    fn logout_url() {
        let base = [
            ("MEDIENLISTE_BASE_URL", "https://medien.example.org"),
            ("MEDIENLISTE_OIDC_ISSUER", "https://auth.example.org"),
            ("MEDIENLISTE_OIDC_CLIENT_ID", "medienliste"),
        ];
        assert!(cfg(&base).unwrap().logout_url.is_none());
        let mut with = base.to_vec();
        with.push(("MEDIENLISTE_LOGOUT_URL", "https://example.org"));
        assert_eq!(cfg(&with).unwrap().logout_url.unwrap().as_str(), "https://example.org/");
        let mut bad = base.to_vec();
        bad.push(("MEDIENLISTE_LOGOUT_URL", "javascript:alert(1)"));
        assert!(cfg(&bad).is_err());
    }

    #[test]
    fn missing_required() {
        assert!(cfg(&[("MEDIENLISTE_BASE_URL", "https://x")]).is_err());
    }
}
