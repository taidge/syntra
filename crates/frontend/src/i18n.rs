use std::collections::HashMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;

const EN_JSON: &str = include_str!("i18n/en.json");
const ZH_JSON: &str = include_str!("i18n/zh.json");

static DICTIONARIES: Lazy<HashMap<&'static str, HashMap<String, String>>> = Lazy::new(|| {
    let mut m = HashMap::new();
    if let Ok(en) = serde_json::from_str::<HashMap<String, String>>(EN_JSON) {
        m.insert("en", en);
    }
    if let Ok(zh) = serde_json::from_str::<HashMap<String, String>>(ZH_JSON) {
        m.insert("zh", zh);
    }
    m
});

static CURRENT: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new(detect_initial_locale()));

fn detect_initial_locale() -> String {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage()
            && let Ok(Some(v)) = storage.get_item("locale")
            && (v == "zh" || v == "en")
        {
            return v;
        }
        if let Some(lang) = window.navigator().language()
            && lang.to_lowercase().starts_with("zh")
        {
            return "zh".into();
        }
    }
    "en".into()
}

pub fn current_locale() -> String {
    CURRENT
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| "en".into())
}

pub fn set_locale(loc: &str) {
    let normalized = if loc == "zh" { "zh" } else { "en" };
    if let Ok(mut s) = CURRENT.lock() {
        *s = normalized.to_string();
    }
    if let Some(window) = web_sys::window()
        && let Ok(Some(storage)) = window.local_storage()
    {
        let _ = storage.set_item("locale", normalized);
    }
}

pub fn t(key: &str) -> String {
    let loc = current_locale();
    if let Some(dict) = DICTIONARIES.get(loc.as_str())
        && let Some(s) = dict.get(key)
    {
        return s.clone();
    }
    if let Some(en) = DICTIONARIES.get("en")
        && let Some(s) = en.get(key)
    {
        return s.clone();
    }
    key.to_string()
}
