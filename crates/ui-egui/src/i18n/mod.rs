//! Display-time localization, following the sibling Craft apps' TSV convention.
//! English strings remain lookup keys; command IDs, drawing data and automation stay unchanged.
//! Add a catalog, a `LANGUAGES` row and a preference variant to introduce another language.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preference {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "uk")]
    Ukrainian,
    #[serde(rename = "pt-BR")]
    BrazilianPortuguese,
    #[serde(rename = "de")]
    German,
    #[serde(rename = "es")]
    Spanish,
    #[serde(rename = "cs")]
    Czech,
    #[serde(rename = "ru")]
    Russian,
    #[serde(rename = "fr")]
    French,
    #[serde(rename = "zh")]
    Chinese,
    #[serde(rename = "it")]
    Italian,
    #[serde(rename = "ko")]
    Korean,
    #[serde(rename = "sv")]
    Swedish,
    #[serde(rename = "ar")]
    Arabic,
    #[serde(rename = "ja")]
    Japanese,
    #[serde(rename = "zh-TW")]
    TraditionalChinese,
    #[serde(rename = "pl")]
    Polish,
    #[serde(rename = "hu")]
    Hungarian,
    #[serde(rename = "tr")]
    Turkish,
    #[serde(rename = "nl")]
    Dutch,
    #[serde(rename = "da")]
    Danish,
    #[serde(rename = "fi")]
    Finnish,
    #[serde(rename = "nb")]
    NorwegianBokmal,
    #[serde(rename = "el")]
    Greek,
    #[serde(rename = "ro")]
    Romanian,
    #[serde(rename = "hi")]
    Hindi,
    #[serde(rename = "id")]
    Indonesian,
    #[serde(rename = "vi")]
    Vietnamese,
    #[serde(rename = "th")]
    Thai,
    #[serde(rename = "he")]
    Hebrew,
    #[serde(rename = "fa")]
    Persian,
}

impl Preference {
    pub const ALL: [Self; 31] = [
        Self::Auto,
        Self::English,
        Self::Ukrainian,
        Self::BrazilianPortuguese,
        Self::German,
        Self::Spanish,
        Self::Czech,
        Self::Russian,
        Self::French,
        Self::Chinese,
        Self::Italian,
        Self::Korean,
        Self::Swedish,
        Self::Arabic,
        Self::Japanese,
        Self::TraditionalChinese,
        Self::Polish,
        Self::Hungarian,
        Self::Turkish,
        Self::Dutch,
        Self::Danish,
        Self::Finnish,
        Self::NorwegianBokmal,
        Self::Greek,
        Self::Romanian,
        Self::Hindi,
        Self::Indonesian,
        Self::Vietnamese,
        Self::Thai,
        Self::Hebrew,
        Self::Persian,
    ];

    /// The codes a preference can be given as, for messages ("auto, en or uk").
    pub fn codes() -> String {
        let codes: Vec<&str> = Self::ALL.iter().map(|p| p.code()).collect();
        match codes.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
            _ => codes.join(""),
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en",
            Self::Ukrainian => "uk",
            Self::BrazilianPortuguese => "pt-BR",
            Self::German => "de",
            Self::Spanish => "es",
            Self::Czech => "cs",
            Self::Russian => "ru",
            Self::French => "fr",
            Self::Chinese => "zh",
            Self::Italian => "it",
            Self::Korean => "ko",
            Self::Swedish => "sv",
            Self::Arabic => "ar",
            Self::Japanese => "ja",
            Self::TraditionalChinese => "zh-TW",
            Self::Polish => "pl",
            Self::Hungarian => "hu",
            Self::Turkish => "tr",
            Self::Dutch => "nl",
            Self::Danish => "da",
            Self::Finnish => "fi",
            Self::NorwegianBokmal => "nb",
            Self::Greek => "el",
            Self::Romanian => "ro",
            Self::Hindi => "hi",
            Self::Indonesian => "id",
            Self::Vietnamese => "vi",
            Self::Thai => "th",
            Self::Hebrew => "he",
            Self::Persian => "fa",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => t("Auto (system language)"),
            Self::English => "English",
            Self::Ukrainian => "Українська",
            Self::BrazilianPortuguese => "Português (Brasil)",
            Self::German => "Deutsch",
            Self::Spanish => "Español",
            Self::Czech => "Čeština",
            Self::Russian => "Русский",
            Self::French => "Français",
            Self::Chinese => "简体中文",
            Self::Italian => "Italiano",
            Self::Korean => {
                if crate::theme::shows_hangul(CURRENT.with(Cell::get)) {
                    "한국어"
                } else {
                    "Korean"
                }
            }
            Self::Swedish => "Svenska",
            Self::Arabic => "العربية",
            Self::Japanese => {
                if crate::theme::shows_japanese() {
                    "日本語"
                } else {
                    "Japanese"
                }
            }
            Self::TraditionalChinese => "繁體中文",
            Self::Polish => "Polski",
            Self::Hungarian => "Magyar",
            Self::Turkish => "Türkçe",
            Self::Dutch => "Nederlands",
            Self::Danish => "Dansk",
            Self::Finnish => "Suomi",
            Self::NorwegianBokmal => "Norsk bokmål",
            Self::Greek => "Ελληνικά",
            Self::Romanian => "Română",
            // Its script shows only with its font, loaded in its own interface.
            Self::Hindi => {
                if CURRENT.with(Cell::get) == "hi" && crate::theme::can_show("hi") {
                    shown("हिन्दी")
                } else {
                    "Hindi"
                }
            }
            Self::Indonesian => "Bahasa Indonesia",
            Self::Vietnamese => {
                if crate::theme::can_show("vi") {
                    "Tiếng Việt"
                } else {
                    "Vietnamese"
                }
            }
            // Its script shows only with its font, loaded in its own interface.
            Self::Thai => {
                if CURRENT.with(Cell::get) == "th" && crate::theme::can_show("th") {
                    shown("ไทย")
                } else {
                    "Thai"
                }
            }
            // Its script shows only with its font, loaded in its own interface.
            Self::Hebrew => {
                if CURRENT.with(Cell::get) == "he" && crate::theme::can_show("he") {
                    shown("עברית")
                } else {
                    "Hebrew"
                }
            }
            // Its script shows only with its font, loaded in its own interface.
            Self::Persian => {
                if CURRENT.with(Cell::get) == "fa" && crate::theme::can_show("fa") {
                    shown("فارسی")
                } else {
                    "Persian"
                }
            }
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.code() == code)
    }

    /// An invalid or obsolete saved choice follows the system language.
    pub fn restore(saved: Option<&str>) -> Self {
        saved.and_then(Self::parse).unwrap_or_default()
    }

    pub fn resolve(self, locales: &[String]) -> &'static str {
        match self {
            Self::English => "en",
            Self::Ukrainian => "uk",
            Self::BrazilianPortuguese => "pt-BR",
            Self::German => "de",
            Self::Spanish => "es",
            Self::Czech => "cs",
            Self::Russian => "ru",
            Self::French => "fr",
            Self::Chinese => "zh",
            Self::Italian => "it",
            Self::Korean => "ko",
            Self::Swedish => "sv",
            Self::Arabic => "ar",
            Self::Japanese => "ja",
            Self::TraditionalChinese => "zh-TW",
            Self::Polish => "pl",
            Self::Hungarian => "hu",
            Self::Turkish => "tr",
            Self::Dutch => "nl",
            Self::Danish => "da",
            Self::Finnish => "fi",
            Self::NorwegianBokmal => "nb",
            Self::Greek => "el",
            Self::Romanian => "ro",
            Self::Hindi => "hi",
            Self::Indonesian => "id",
            Self::Vietnamese => "vi",
            Self::Thai => "th",
            Self::Hebrew => "he",
            Self::Persian => "fa",
            Self::Auto => locales.iter().find_map(|s| locale_language(s)).unwrap_or("en"),
        }
    }
}

pub struct Language {
    pub code: &'static str,
    pub name: &'static str,
    pub source: &'static str,
    /// Index of the `@plural` form for an integer count.
    pub plural: fn(u64) -> usize,
    /// Number of `|`-separated forms every `@plural` row must carry.
    pub plural_forms: usize,
    catalog: OnceLock<Catalog<'static>>,
}

pub static LANGUAGES: [Language; 30] = [
    Language { code: "en", name: "English", source: "", plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    Language {
        code: "uk", name: "Українська", source: include_str!("uk.tsv"), plural: plural_uk, plural_forms: 3, catalog: OnceLock::new()
    },
    Language {
        code: "pt-BR",
        name: "Português (Brasil)",
        source: include_str!("pt_br.tsv"),
        plural: plural_pt,
        plural_forms: 2,
        catalog: OnceLock::new(),
    },
    // German plurals follow the English rule: one (1), other.
    Language { code: "de", name: "Deutsch", source: include_str!("de.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    // Spanish integer plurals follow the English rule too.
    Language { code: "es", name: "Español", source: include_str!("es.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    Language { code: "cs", name: "Čeština", source: include_str!("cs.tsv"), plural: plural_cs, plural_forms: 3, catalog: OnceLock::new() },
    // Russian integer plurals follow the Ukrainian rule: one (1, 21), few (2–4), many.
    Language { code: "ru", name: "Русский", source: include_str!("ru.tsv"), plural: plural_uk, plural_forms: 3, catalog: OnceLock::new() },
    // French integer plurals (CLDR `fr`): one (0, 1), other, the Portuguese rule.
    Language { code: "fr", name: "Français", source: include_str!("fr.tsv"), plural: plural_pt, plural_forms: 2, catalog: OnceLock::new() },
    // Chinese has no grammatical plural: both `@plural` forms read the same. Its glyphs come from
    // an installed CJK font (`theme::cjk_font`); without one the interface stays English.
    Language { code: "zh", name: "简体中文", source: include_str!("zh.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    // Italian integer plurals (CLDR `it`) follow the English rule: one (1), other.
    Language { code: "it", name: "Italiano", source: include_str!("it.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    // Korean has one integer plural form and needs a Hangul-capable interface font.
    Language { code: "ko", name: "한국어", source: include_str!("ko.tsv"), plural: plural_ko, plural_forms: 1, catalog: OnceLock::new() },
    // Swedish plurals follow the English rule: one (1), other.
    Language { code: "sv", name: "Svenska", source: include_str!("sv.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    // العربية (Arabic) is right-to-left; an installed Arabic font (`theme::arabic_font`)
    // covers its glyphs, and the shell keeps egui's left-to-right run order. Six plural
    // forms: zero, one, two, few, many, other.
    Language { code: "ar", name: "العربية", source: include_str!("ar.tsv"), plural: plural_arabic, plural_forms: 6, catalog: OnceLock::new() },
    // Japanese has one integer plural form and needs a font with kana and kanji.
    Language { code: "ja", name: "日本語", source: include_str!("ja.tsv"), plural: plural_ko, plural_forms: 1, catalog: OnceLock::new() },
    // Traditional Chinese (Taiwan): like Simplified, no grammatical plural; the glyphs come from
    // an installed CJK font (a Traditional face first where there is one: `theme::cjk_font`).
    Language { code: "zh-TW", name: "繁體中文", source: include_str!("zh_tw.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    // Polish integer plurals (CLDR `pl`): one (1), few (2–4, 22–24 …), many (0, 5–21, 25 …).
    Language { code: "pl", name: "Polski", source: include_str!("pl.tsv"), plural: plural_pl, plural_forms: 3, catalog: OnceLock::new() },
    // Hungarian and Turkish keep a noun singular after a number: one, other (usually the same).
    Language { code: "hu", name: "Magyar", source: include_str!("hu.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    Language { code: "tr", name: "Türkçe", source: include_str!("tr.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    // Dutch, Danish, Finnish, Norwegian Bokmål and Greek: one (1), other.
    Language { code: "nl", name: "Nederlands", source: include_str!("nl.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    Language { code: "da", name: "Dansk", source: include_str!("da.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    Language { code: "fi", name: "Suomi", source: include_str!("fi.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    Language { code: "nb", name: "Norsk bokmål", source: include_str!("nb.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    Language { code: "el", name: "Ελληνικά", source: include_str!("el.tsv"), plural: plural_en, plural_forms: 2, catalog: OnceLock::new() },
    // Romanian (CLDR `ro`): one (1), few (0, 2–19, 101–119 …), other (20 …, written with "de").
    Language { code: "ro", name: "Română", source: include_str!("ro.tsv"), plural: plural_ro, plural_forms: 3, catalog: OnceLock::new() },
    // Hindi (CLDR `hi`): one (0 and 1), other. Devanagari needs an installed font
    // (`theme::script_font`); egui shapes it (conjuncts, reph, the short-i sign).
    Language { code: "hi", name: "हिन्दी", source: include_str!("hi.tsv"), plural: plural_pt, plural_forms: 2, catalog: OnceLock::new() },
    // Indonesian, Vietnamese and Thai: one count form. Vietnamese tone marks and Thai need an
    // installed font with them (`theme::script_font`).
    Language { code: "id", name: "Bahasa Indonesia", source: include_str!("id.tsv"), plural: plural_ko, plural_forms: 1, catalog: OnceLock::new() },
    Language { code: "vi", name: "Tiếng Việt", source: include_str!("vi.tsv"), plural: plural_ko, plural_forms: 1, catalog: OnceLock::new() },
    Language { code: "th", name: "ไทย", source: include_str!("th.tsv"), plural: plural_ko, plural_forms: 1, catalog: OnceLock::new() },
    // עברית (Hebrew) and فارسی (Persian) are right-to-left like Arabic, with an installed font.
    // Hebrew: one (1), two (2), other. Persian (CLDR `fa`): one (0 and 1), other.
    Language { code: "he", name: "עברית", source: include_str!("he.tsv"), plural: plural_he, plural_forms: 3, catalog: OnceLock::new() },
    Language { code: "fa", name: "فارسی", source: include_str!("fa.tsv"), plural: plural_pt, plural_forms: 2, catalog: OnceLock::new() },
];

/// BCP 47 and POSIX locale tags, including `uk-UA`, `uk_UA.UTF-8` and `pt_BR.UTF-8`.
/// A catalog is chosen by the tag's primary language, so any `pt-*` tag selects `pt-BR`; Chinese
/// in Traditional script (`zh-TW`, `zh-HK`, `zh-MO`, `zh-Hant…`) selects `zh-TW`.
pub fn locale_language(tag: &str) -> Option<&'static str> {
    let mut parts = tag.trim().split(['-', '_', '.', '@']);
    let primary = parts.next()?;
    if primary.eq_ignore_ascii_case("zh") && parts.take(2).any(|p| ["tw", "hk", "mo", "hant"].contains(&p.to_ascii_lowercase().as_str())) {
        return Some("zh-TW");
    }
    // Norwegian (`no`, Nynorsk `nn`) reads the Bokmål catalog; `iw` and `in` are the old codes of
    // Hebrew and Indonesian that some systems still report.
    match primary.to_ascii_lowercase().as_str() {
        "no" | "nn" => return Some("nb"),
        "iw" => return Some("he"),
        "in" => return Some("id"),
        _ => {}
    }
    LANGUAGES.iter().find(|l| l.code.split('-').next().is_some_and(|p| p.eq_ignore_ascii_case(primary))).map(|l| l.code)
}

thread_local! {
    // Scoped to a single app's render. Headless calls and parallel tests default to English.
    static CURRENT: Cell<&'static str> = const { Cell::new("en") };
}

/// The language being drawn: background work started from the interface takes it along
/// ([`with_language`]) so its messages read in the same language.
pub fn current() -> &'static str {
    CURRENT.get()
}

pub fn with_language<R>(language: &'static str, draw: impl FnOnce() -> R) -> R {
    struct Restore(&'static str);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.set(self.0);
        }
    }
    let _restore = Restore(CURRENT.replace(language));
    draw()
}

type Catalog<'a> = HashMap<(&'a str, &'a str), &'a str>;

/// Malformed rows are ignored at runtime and diagnosed by the catalog tests.
fn parse(source: &str) -> (Catalog<'_>, Vec<String>) {
    let mut entries = HashMap::new();
    let mut errors = Vec::new();
    for (i, line) in source.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let columns: Vec<_> = line.split('\t').collect();
        match columns.as_slice() {
            [context, key, value] if !key.is_empty() && !value.is_empty() => {
                if entries.insert((*context, *key), *value).is_some() {
                    errors.push(format!("line {}: duplicate key {key}", i + 1));
                }
            }
            _ => errors.push(format!("line {}: expected context, source, translation", i + 1)),
        }
    }
    (entries, errors)
}

fn catalog(language: &str) -> Option<&'static Catalog<'static>> {
    LANGUAGES.iter().find(|l| l.code == language && !l.source.is_empty()).map(|l| l.catalog.get_or_init(|| parse(l.source).0))
}

pub fn tr<'a>(language: &str, source: &'a str) -> &'a str {
    catalog(language).and_then(|c| c.get(&("", source))).copied().unwrap_or(source)
}

pub fn t(source: &str) -> &str {
    CURRENT.with(|l| shown(tr(l.get(), source)))
}

/// Languages written right to left: their interface is mirrored ([`crate::rtl`]).
pub fn is_rtl(language: &str) -> bool {
    matches!(language, "ar" | "he" | "fa")
}

/// The interface language being drawn is right-to-left.
pub fn rtl() -> bool {
    CURRENT.with(|l| is_rtl(l.get()))
}

/// Right-to-left text in display order for a left-to-right text layout (egui's): Arabic letters
/// in their joined forms, each line's runs in visual order with brackets mirrored, the line read
/// as right-to-left. Text with no right-to-left letters comes back as it is.
pub fn rtl_display(s: &str) -> String {
    if !cadaddy_fonts::bidi::has_rtl(s) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let logical: Vec<char> = line.chars().collect();
        let joined: Vec<char> = cadaddy_fonts::bidi::presentation_forms(&logical, |_| true).into_iter().map(|(c, _)| c).collect();
        for (r, rtl) in cadaddy_fonts::bidi::visual_runs(&joined, Some(true)) {
            let run = joined.get(r).unwrap_or_default();
            if rtl {
                out.extend(run.iter().rev().map(|c| cadaddy_fonts::bidi::mirror(*c)));
            } else {
                out.extend(run.iter());
            }
        }
    }
    out
}

/// Most distinct right-to-left texts kept in display order (the catalogs hold a few thousand).
const MAX_SHOWN: usize = 20_000;

/// `text` as drawn: in a right-to-left interface its display order ([`rtl_display`]), made once
/// per text and kept.
fn shown(text: &str) -> &str {
    if !needs_display(text) {
        return text;
    }
    static SHOWN: std::sync::Mutex<Option<HashMap<String, &'static str>>> = std::sync::Mutex::new(None);
    let mut cache = SHOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = cache.get_or_insert_with(HashMap::new);
    if let Some(s) = map.get(text) {
        return s;
    }
    if map.len() >= MAX_SHOWN {
        return text;
    }
    let s: &'static str = Box::leak(display(text).into_boxed_str());
    map.insert(text.to_string(), s);
    s
}

/// Whether `text` is drawn differently from how it's stored: right to left. (egui shapes
/// complex scripts such as Devanagari and Thai itself; right-to-left text needs its display
/// order.)
fn needs_display(text: &str) -> bool {
    rtl() && cadaddy_fonts::bidi::has_rtl(text)
}

/// `text` in display order for the interface language ([`rtl_display`]).
fn display(text: &str) -> String {
    if rtl() { rtl_display(text) } else { text.to_string() }
}

/// Text the engine wrote (a prompt, a command-line message, an error) in the interface language:
/// this catalog's translation, else the engine's own catalog's ([`cadaddy_engine::i18n`], which
/// also fills in values: "Command: {}"). The engine keeps the English for agents and scripts.
pub fn t_engine(source: &str) -> std::borrow::Cow<'_, str> {
    let ui = t(source);
    if ui != source {
        return std::borrow::Cow::Borrowed(ui);
    }
    let text = CURRENT.with(|l| cadaddy_engine::i18n::tr(l.get(), source));
    if needs_display(&text) { std::borrow::Cow::Owned(display(&text)) } else { text }
}

/// Menu headings can differ from command or object labels (e.g. Czech Kótování / Kóta).
/// Catalogs without a `menu` entry retain their ordinary translation.
pub fn tr_menu<'a>(language: &str, source: &'a str) -> &'a str {
    catalog(language).and_then(|c| c.get(&("menu", source))).copied().unwrap_or_else(|| tr(language, source))
}

pub fn menu(source: &str) -> &str {
    CURRENT.with(|l| shown(tr_menu(l.get(), source)))
}

/// Named placeholders allow translations to reorder values. Inserted values are never looked up
/// or substituted again, so a user's name containing `{n}` is preserved verbatim.
pub fn format(source: &str, values: &[(&str, String)]) -> String {
    // The values go into the logical text; the whole is put in display order after.
    let text = CURRENT.with(|l| tr(l.get(), source));
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, tail)) = rest.split_once('{') {
        result.push_str(before);
        let Some((key, after)) = tail.split_once('}') else {
            result.push('{');
            rest = tail;
            break;
        };
        if let Some((_, value)) = values.iter().find(|(name, _)| *name == key) {
            result.push_str(value);
        } else {
            result.push('{');
            result.push_str(key);
            result.push('}');
        }
        rest = after;
    }
    result.push_str(rest);
    if needs_display(&result) { display(&result) } else { result }
}

/// Ukrainian integer plurals: one (1, 21), few (2–4, 22–24), many (0, 5–20, 25…).
pub fn plural_uk(n: u64) -> usize {
    match (n % 10, n % 100) {
        (1, 11) | (2..=4, 12..=14) => 2,
        (1, _) => 0,
        (2..=4, _) => 1,
        _ => 2,
    }
}

/// Portuguese integer plurals (CLDR `pt`): one (0, 1), other (2 and up).
pub fn plural_pt(n: u64) -> usize {
    usize::from(n > 1)
}

/// Romanian integer plurals (CLDR `ro`): one (1), few (0, 2–19, and 01–19 after the hundreds:
/// 101–119), other (20–100, 120 …).
pub fn plural_ro(n: u64) -> usize {
    match (n, n % 100) {
        (1, _) => 0,
        (0, _) | (_, 1..=19) => 1,
        _ => 2,
    }
}

/// Hebrew integer plurals (CLDR `he`): one (1), two (2), other.
pub fn plural_he(n: u64) -> usize {
    match n {
        1 => 0,
        2 => 1,
        _ => 2,
    }
}

/// Korean (and Japanese) use the same count form for every integer.
fn plural_ko(_n: u64) -> usize {
    0
}

/// Arabic integer plurals (CLDR `ar`): zero (0), one (1), two (2), few (3–10, 103–110),
/// many (11–99) — the same rule pdfcraft uses, so the sibling apps agree.
pub fn plural_arabic(n: u64) -> usize {
    match (n, n % 100) {
        (0, _) => 0,
        (1, _) => 1,
        (2, _) => 2,
        (_, 3..=10) => 3,
        (_, 11..=99) => 4,
        _ => 5,
    }
}

fn plural_en(n: u64) -> usize {
    usize::from(n != 1)
}

/// Polish integer plurals: one (1), few (2–4, 22–24, 32–34 … but not 12–14), many (the rest).
pub fn plural_pl(n: u64) -> usize {
    match (n, n % 10, n % 100) {
        (1, _, _) => 0,
        (_, 2..=4, r) if !(12..=14).contains(&r) => 1,
        _ => 2,
    }
}

/// Czech integer plurals: one (1), few (2–4), other (0 and 5 or more).
pub fn plural_cs(n: u64) -> usize {
    match n {
        1 => 0,
        2..=4 => 1,
        _ => 2,
    }
}

pub fn count(n: u64, one: &str, other: &str) -> String {
    let language = CURRENT.get();
    let source = catalog(language).and_then(|c| c.get(&("@plural", one))).copied();
    let plural = LANGUAGES.iter().find(|l| l.code == language).map_or(plural_en as fn(u64) -> usize, |l| l.plural);
    let form = source.and_then(|s| s.split('|').nth(plural(n))).unwrap_or(if n == 1 { one } else { other });
    format(form, &[("n", n.to_string())])
}

#[macro_export]
macro_rules! tl {
    ($source:literal) => {
        $crate::i18n::t($source)
    };
}

#[macro_export]
macro_rules! tf {
    ($source:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::format($source, &[$((stringify!($name), ($value).to_string())),*])
    };
}

#[cfg(test)]
mod tests;
