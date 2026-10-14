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

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/i18n/strings.json")).expect("valid strings.json")
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
}
