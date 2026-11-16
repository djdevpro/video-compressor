//! Les langues de l'interface.
//!
//! Le texte source est le français, écrit tel quel dans le code : `tr("Ajouter")` renvoie sa
//! traduction dans la langue courante, `tf("{n} fichiers", &[("n", &n)])` remplace en plus les
//! paramètres nommés.
//!
//! Un fichier par langue, `locales/<code>.json` : le texte français (exactement comme dans le code)
//! associé à sa traduction. Ajouter un texte, c'est ajouter la même clé dans chaque fichier ; un
//! test dit lequel en manque.
//!
//! La langue est celle du système, sauf choix contraire dans les préférences. Elle est posée au
//! démarrage réel de l'application ; dans les tests, c'est le français.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Lang {
    Fr,
    En,
}

impl Lang {
    pub const ALL: [Self; 2] = [Self::Fr, Self::En];

    pub fn code(self) -> &'static str {
        ["fr", "en"][self as usize]
    }

    /// Le nom de la langue, dans la langue elle-même.
    pub fn native(self) -> &'static str {
        ["Français", "English"][self as usize]
    }

    /// `fr-FR`, `en_US.UTF-8`… → la langue, si elle est prise en charge.
    pub fn from_locale(locale: &str) -> Option<Self> {
        let code = locale.split(['-', '_', '.']).next()?.to_lowercase();
        Self::ALL.into_iter().find(|l| l.code() == code)
    }

    /// Le contenu de `locales/<code>.json` (rien pour le français, la langue du code).
    fn file(self) -> &'static str {
        match self {
            Self::Fr => "{}",
            Self::En => include_str!("../locales/en.json"),
        }
    }
}

/// La langue du système (anglais si elle n'est pas prise en charge).
pub fn detect() -> Lang {
    sys_locale::get_locale().as_deref().and_then(Lang::from_locale).unwrap_or(Lang::En)
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::Fr as u8);

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    Lang::ALL[usize::from(CURRENT.load(Ordering::Relaxed)).min(Lang::ALL.len() - 1)]
}

type Table = HashMap<String, String>;

/// Les traductions d'une langue, lues à la première utilisation.
fn table(lang: Lang) -> &'static Table {
    static TABLES: [OnceLock<Table>; 2] = [const { OnceLock::new() }; 2];
    TABLES[lang as usize].get_or_init(|| serde_json::from_str(lang.file()).unwrap_or_else(|e| panic!("locales/{}.json illisible : {e}", lang.code())))
}

/// La traduction d'un texte dans `lang` (le français, si elle manque).
pub fn translate(fr: &str, lang: Lang) -> Option<&'static str> {
    if lang == Lang::Fr {
        return None;
    }
    table(lang).get(fr).map(String::as_str).filter(|t| !t.is_empty())
}

/// Le texte dans la langue courante.
pub fn tr(fr: &str) -> &str {
    translate(fr, current()).unwrap_or(fr)
}

/// Le texte dans la langue courante, avec ses paramètres nommés (`{n}`, `{nom}`…) remplacés.
pub fn tf(fr: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = tr(fr).to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

// ---- nombres, tailles, durées ------------------------------------------------------------------

/// Un nombre à `digits` décimales, avec la virgule en français.
pub fn decimal(value: f64, digits: usize) -> String {
    let text = format!("{value:.digits$}");
    if current() == Lang::Fr { text.replace('.', ",") } else { text }
}

/// Un poids de fichier : « 48,2 Mo », « 1,25 Go », « 640 Ko ».
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    let (value, digits, unit) = if b >= 1e9 {
        (b / 1e9, 2, tr("Go"))
    } else if b >= 1e6 {
        (b / 1e6, 1, tr("Mo"))
    } else if b >= 1e3 {
        (b / 1e3, 0, tr("Ko"))
    } else {
        (b, 0, tr("octets"))
    };
    format!("{} {unit}", decimal(value, digits))
}

/// Une durée : « 0:07 », « 12:30 », « 1:02:03 ».
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn langue_du_systeme() {
        assert_eq!(Lang::from_locale("fr-FR"), Some(Lang::Fr));
        assert_eq!(Lang::from_locale("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::from_locale("hi-IN"), None, "pas prise en charge : anglais");
    }

    #[test]
    fn traduire_et_parametres() {
        // Sans changer la langue globale : les autres tests tournent en même temps, en français.
        assert_eq!(translate("Ajouter", Lang::En), Some("Add"));
        assert_eq!(translate("Ajouter", Lang::Fr), None);
        assert_eq!(translate("un texte absent des fichiers", Lang::En), None);
        assert_eq!(tr("Ajouter"), "Ajouter");
        assert_eq!(tf("{n} fichiers", &[("n", &3)]), "3 fichiers");
    }

    #[test]
    fn nombres_tailles_et_durees() {
        assert_eq!(decimal(12.345, 1), "12,3");
        assert_eq!((size(48_200_000), size(1_250_000_000), size(640_000), size(12)), ("48,2 Mo".into(), "1,25 Go".into(), "640 Ko".into(), "12 octets".into()));
        assert_eq!((clock(7.2), clock(750.0), clock(3723.0)), ("0:07".into(), "12:30".into(), "1:02:03".into()));
    }

    fn params(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(n, _)| n.to_owned())).collect();
        v.sort();
        v
    }

    /// Les textes passés à `tr(…)` ou `tf(…)` dans le code, avec leur fichier.
    fn keys() -> Vec<(String, String)> {
        let mut keys = Vec::new();
        let mut files = vec![std::path::PathBuf::from("src")];
        while let Some(p) = files.pop() {
            if p.is_dir() {
                files.extend(std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
                continue;
            }
            if p.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            for start in ["tr(\"", "tf(\""] {
                for (i, _) in src.match_indices(start) {
                    // `tr(` seul, pas `str(` ni `attr(`.
                    if src[..i].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let rest = &src[i + start.len()..];
                    let Some(end) = rest.find('"') else { continue };
                    keys.push((p.display().to_string(), rest[..end].replace("\\\"", "\"").replace("\\n", "\n")));
                }
            }
        }
        keys
    }

    /// Chaque fichier se lit, n'a pas de traduction vide, et garde les paramètres du français.
    #[test]
    fn fichiers_de_langue_coherents() {
        for lang in Lang::ALL.into_iter().skip(1) {
            for (fr, text) in table(lang) {
                assert!(!text.trim().is_empty(), "locales/{}.json : traduction vide pour « {fr} »", lang.code());
                assert_eq!(params(text), params(fr), "locales/{}.json : paramètres différents pour « {fr} »", lang.code());
            }
        }
    }

    /// Chaque texte du code est traduit, et aucune traduction ne traîne pour un texte disparu.
    #[test]
    fn tous_les_textes_sont_traduits() {
        let keys = keys();
        let mut problems = Vec::new();
        for lang in Lang::ALL.into_iter().skip(1) {
            for (file, key) in &keys {
                if !table(lang).contains_key(key) {
                    problems.push(format!("locales/{}.json : il manque « {key} » ({file})", lang.code()));
                }
            }
            for key in table(lang).keys() {
                if !keys.iter().any(|(_, k)| k == key) {
                    problems.push(format!("locales/{}.json : « {key} » ne sert plus", lang.code()));
                }
            }
        }
        problems.sort();
        problems.dedup();
        assert!(problems.is_empty(), "{} problèmes :\n{}", problems.len(), problems.join("\n"));
    }

    /// Les règles de rédaction : ni tiret long, ni apostrophe courbe, ni deux-points dans un texte
    /// de l'interface, dans les deux langues. Une règle écrite sans contrôle ne tient pas.
    #[test]
    fn regles_de_redaction() {
        let banned = ['\u{2014}', '\u{2013}', '\u{2019}', ':'];
        let mut texts: Vec<String> = keys().into_iter().map(|(_, k)| k).collect();
        texts.extend(table(Lang::En).values().cloned());
        let faulty: Vec<&String> = texts.iter().filter(|t| t.contains(banned)).collect();
        assert!(faulty.is_empty(), "textes à reformuler :\n{}", faulty.iter().map(|t| t.as_str()).collect::<Vec<_>>().join("\n"));
        // Le README : mêmes signes, sauf les deux-points, qu'un lien ou un bloc de code réclame.
        let readme = std::fs::read_to_string("README.md").unwrap();
        let lines: Vec<&str> = readme.lines().filter(|l| l.contains(&banned[..3])).collect();
        assert!(lines.is_empty(), "README à reformuler :\n{}", lines.join("\n"));
    }
}
