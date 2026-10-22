//! Übersetzungen auf Basis von Fluent (`locales/<sprache>.ftl`).
//!
//! Weitere Sprachen: Datei `locales/<code>.ftl` anlegen und in [`LOCALES`] eintragen.

use std::collections::HashMap;

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

const LOCALES: &[(&str, &str)] = &[("de", include_str!("../locales/de.ftl"))];

pub const DEFAULT_LANG: &str = "de";

static INSTANCE: std::sync::LazyLock<I18n> = std::sync::LazyLock::new(I18n::load);

/// Globale, beim ersten Zugriff geladene Übersetzungen.
pub fn get() -> &'static I18n {
    &INSTANCE
}

pub struct I18n {
    bundles: HashMap<&'static str, FluentBundle<FluentResource>>,
}

impl I18n {
    pub fn load() -> Self {
        let mut bundles = HashMap::new();
        for (code, src) in LOCALES {
            let res = FluentResource::try_new(src.to_string())
                .unwrap_or_else(|(_, errs)| panic!("Sprachdatei {code} fehlerhaft: {errs:?}"));
            let lang: LanguageIdentifier = code.parse().expect("gültiger Sprachcode");
            let mut b = FluentBundle::new_concurrent(vec![lang]);
            // Keine Unicode-Isolationszeichen in HTML-Ausgabe.
            b.set_use_isolating(false);
            b.add_resource(res).expect("keine doppelten Schlüssel");
            bundles.insert(*code, b);
        }
        Self { bundles }
    }

    fn bundle(&self, lang: &str) -> &FluentBundle<FluentResource> {
        self.bundles.get(lang).unwrap_or_else(|| &self.bundles[DEFAULT_LANG])
    }

    pub fn t(&self, lang: &str, key: &str) -> String {
        self.format(lang, key, None)
    }

    pub fn t_args(&self, lang: &str, key: &str, args: &[(&str, FluentValue<'_>)]) -> String {
        let mut fa = FluentArgs::new();
        for (k, v) in args {
            fa.set(*k, v.clone());
        }
        self.format(lang, key, Some(&fa))
    }

    fn format(&self, lang: &str, key: &str, args: Option<&FluentArgs>) -> String {
        let b = self.bundle(lang);
        let Some(msg) = b.get_message(key).and_then(|m| m.value()) else {
            tracing::warn!("Übersetzung fehlt: {key}");
            return key.to_string();
        };
        let mut errors = vec![];
        let s = b.format_pattern(msg, args, &mut errors);
        if !errors.is_empty() {
            tracing::warn!("Fehler in Übersetzung {key}: {errors:?}");
        }
        s.into_owned()
    }

    /// Alle Schlüssel einer Sprache (für Tests).
    pub fn has(&self, lang: &str, key: &str) -> bool {
        self.bundle(lang).has_message(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_plural() {
        let i = I18n::load();
        assert_eq!(i.t("de", "nav-search"), "Suche");
        assert_eq!(i.t_args("de", "list-count", &[("count", 1.into())]), "1 Eintrag");
        assert_eq!(i.t_args("de", "list-count", &[("count", 3.into())]), "3 Einträge");
    }

    /// Jeder in Templates/Code verwendete Schlüssel muss existieren.
    #[test]
    fn all_used_keys_exist() {
        let i = I18n::load();
        let re_sources = [("templates", "html"), ("src/web", "rs")];
        let mut missing = vec![];
        for st in crate::models::Status::ALL {
            for mt in crate::models::MediaType::ALL {
                if !i.has("de", st.label_key(mt)) {
                    missing.push(st.label_key(mt).to_string());
                }
            }
        }
        for (dir, ext) in re_sources {
            for path in walk(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir), ext) {
                let text = std::fs::read_to_string(&path).unwrap();
                for key in extract_keys(&text) {
                    if !i.has("de", &key) {
                        missing.push(format!("{}: {key}", path.display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "fehlende Übersetzungen: {missing:#?}");
    }

    fn walk(dir: std::path::PathBuf, ext: &str) -> Vec<std::path::PathBuf> {
        let mut out = vec![];
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.extend(walk(p, ext));
            } else if p.extension().is_some_and(|x| x == ext) {
                out.push(p);
            }
        }
        out
    }

    /// Findet `t("schluessel"` bzw. `t_args("schluessel"` sowie `label_key`-Rückgabewerte.
    fn extract_keys(text: &str) -> Vec<String> {
        let mut keys = vec![];
        for marker in [".t(\"", ".t_args(\""] {
            let mut rest = text;
            while let Some(pos) = rest.find(marker) {
                rest = &rest[pos + marker.len()..];
                if let Some(end) = rest.find('"') {
                    let k = &rest[..end];
                    if !k.is_empty()
                        && k.chars().all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
                    {
                        keys.push(k.to_string());
                    }
                }
            }
        }
        keys
    }
}
