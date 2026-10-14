//! UI strings (en/vi). The English table is taken verbatim from the original client's translation table.

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Vi,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Vi => "vi",
        }
    }
    pub fn from_code(code: &str) -> Lang {
        if code == "vi" { Lang::Vi } else { Lang::En }
    }
    pub fn toggled(self) -> Lang {
        match self {
            Lang::En => Lang::Vi,
            Lang::Vi => Lang::En,
        }
    }
}

/// Values are strings, except a few lists such as `joint_names` and `servo_modes`.
type Table = HashMap<String, HashMap<String, serde_json::Value>>;

fn lookup(lang: Lang, key: &str) -> Option<&'static serde_json::Value> {
    table().get(lang.code()).and_then(|m| m.get(key))
}

/// Lab pages keep their strings in separate files so the original client's table stays verbatim.
const LAB_TABLES: &[&str] = &[
    include_str!("../assets/i18n/lab.json"),
    include_str!("../assets/i18n/lab_datasets.json"),
    include_str!("../assets/i18n/lab_record.json"),
    include_str!("../assets/i18n/lab_train.json"),
    include_str!("../assets/i18n/lab_servers.json"),
    include_str!("../assets/i18n/lab_deploy.json"),
];

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        let mut table: Table = serde_json::from_str(include_str!("../assets/i18n/strings.json")).expect("valid strings.json");
        for src in LAB_TABLES {
            let lab: Table = serde_json::from_str(src).expect("valid lab string file");
            for (lang, entries) in lab {
                table.entry(lang).or_default().extend(entries);
            }
        }
        table
    })
}

/// Translated string for `key`, or `key` itself when missing.
pub fn tr(lang: Lang, key: &str) -> String {
    tr_or(lang, key, key)
}

/// Like [`tr`] with an explicit fallback (mirrors `STRINGS[lang].get(key, default)`).
pub fn tr_or(lang: Lang, key: &str, fallback: &str) -> String {
    lookup(lang, key).and_then(|v| v.as_str()).unwrap_or(fallback).to_string()
}

/// List-valued entry such as `joint_names`; empty when missing.
pub fn tr_list(lang: Lang, key: &str) -> Vec<String> {
    lookup(lang, key)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_have_same_keys() {
        let t = table();
        let vi: std::collections::BTreeSet<_> = t["vi"].keys().collect();
        let en: std::collections::BTreeSet<_> = t["en"].keys().collect();
        assert_eq!(vi, en);
        assert_eq!(tr(Lang::En, "window_title"), "NexArm Controller");
        assert_eq!(tr(Lang::Vi, "msg_error"), "Lỗi");
        assert_eq!(tr(Lang::Vi, "missing_key"), "missing_key");
        assert_eq!(tr_list(Lang::En, "joint_names").len(), 6);
    }

    /// Every `lab_*` / `tab_lab_*` key the Lab code mentions must exist in both languages.
    #[test]
    fn lab_keys_used_in_source_are_translated() {
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(std::fs::read_to_string(&path).unwrap());
                }
            }
        }
        let mut sources = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut sources);
        let mut used = std::collections::BTreeSet::new();
        for src in &sources {
            for part in src.split('"').skip(1).step_by(2) {
                let is_key = (part.starts_with("lab_") || part.starts_with("tab_lab_"))
                    && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                if is_key {
                    used.insert(part.to_string());
                }
            }
        }
        assert!(used.len() > 10, "found only {} lab keys", used.len());
        let t = table();
        for key in used {
            for lang in ["en", "vi"] {
                assert!(t[lang].contains_key(&key), "missing {lang} string for {key}");
            }
        }
    }
}
