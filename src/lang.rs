// SPDX-License-Identifier: GPL-3.0-or-later
//! Available languages and selection (uses Slint's bundled translations, .po files in `lang/`).

/// (code saved in the settings, Slint locale, native name)
pub const LANGS: &[(&str, &str, &str)] = &[
    ("pt-BR", "pt_BR", "Português (Brasil)"),
    ("en", "en", "English"),
    ("es", "es", "Español"),
    ("de", "de", "Deutsch"),
    ("fr", "fr", "Français"),
    ("it", "it", "Italiano"),
    ("ru", "ru", "Русский"),
    ("zh-CN", "zh_CN", "简体中文"),
    ("ja", "ja", "日本語"),
    ("ko", "ko", "한국어"),
    ("hi", "hi", "हिन्दी"),
    ("ar", "ar", "العربية"),
    ("id", "id", "Bahasa Indonesia"),
    ("nl", "nl", "Nederlands"),
    ("pl", "pl", "Polski"),
];

/// Resolves "auto" or a code to the index in LANGS.
pub fn resolve(code: &str) -> usize {
    if code != "auto" {
        if let Some(i) = LANGS.iter().position(|l| l.0.eq_ignore_ascii_case(code)) {
            return i;
        }
    }
    detect(&crate::sysutil::system_locale())
}

pub fn detect(locale: &str) -> usize {
    let l = locale.to_ascii_lowercase().replace('_', "-");
    let idx = |c: &str| LANGS.iter().position(|x| x.0 == c).unwrap_or(1);
    let primary = l.split('-').next().unwrap_or("");
    match primary {
        "pt" => idx("pt-BR"),
        "zh" => idx("zh-CN"),
        // "in" is the old code for Indonesian
        "in" => idx("id"),
        other => LANGS.iter().position(|x| x.0 == other).unwrap_or(1),
    }
}

pub fn apply(index: usize) {
    let locale = LANGS.get(index).map(|l| l.1).unwrap_or("en");
    if let Err(e) = slint::select_bundled_translation(locale) {
        eprintln!("solua: translation '{locale}' unavailable: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_languages() {
        assert_eq!(LANGS[detect("pt-BR")].0, "pt-BR");
        assert_eq!(LANGS[detect("pt_PT.UTF-8")].0, "pt-BR");
        assert_eq!(LANGS[detect("de-DE")].0, "de");
        assert_eq!(LANGS[detect("es_ES")].0, "es");
        assert_eq!(LANGS[detect("ja-JP")].0, "ja");
        assert_eq!(LANGS[detect("zh_CN.UTF-8")].0, "zh-CN");
        assert_eq!(LANGS[detect("fr_FR")].0, "fr");
        assert_eq!(LANGS[detect("sw-KE")].0, "en");
        assert_eq!(LANGS[detect("")].0, "en");
        assert_eq!(LANGS[resolve("de")].0, "de");
    }
}
