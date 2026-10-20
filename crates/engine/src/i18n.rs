//! The engine's messages in other languages: prompts, command-line messages and errors as people
//! read them. The engine itself always speaks English (agents and scripts get English replies);
//! a user interface passes the text it shows through [`tr`] with the interface language.
//!
//! The catalogs (`i18n/<code>.tsv`) have the user interface's format: `context<TAB>English<TAB>
//! translation`, `#` comments. An English text may hold `{}` for a value ("Command: {}"); the
//! translation places the values in order with `{}`, or picks one with `{0}`, `{1}`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The languages with a catalog, by their file names.
pub const LANGUAGES: [&str; 17] = ["ar", "cs", "de", "es", "fr", "hu", "it", "ja", "ko", "pl", "pt_br", "ru", "sv", "tr", "uk", "zh", "zh_tw"];

const SOURCES: [&str; 17] = [
    include_str!("i18n/ar.tsv"),
    include_str!("i18n/cs.tsv"),
    include_str!("i18n/de.tsv"),
    include_str!("i18n/es.tsv"),
    include_str!("i18n/fr.tsv"),
    include_str!("i18n/hu.tsv"),
    include_str!("i18n/it.tsv"),
    include_str!("i18n/ja.tsv"),
    include_str!("i18n/ko.tsv"),
    include_str!("i18n/pl.tsv"),
    include_str!("i18n/pt_br.tsv"),
    include_str!("i18n/ru.tsv"),
    include_str!("i18n/sv.tsv"),
    include_str!("i18n/tr.tsv"),
    include_str!("i18n/uk.tsv"),
    include_str!("i18n/zh.tsv"),
    include_str!("i18n/zh_tw.tsv"),
];

/// Most lines one text is translated in.
const MAX_LINES: usize = 200;

/// One language's catalog: exact texts, and texts with values (their literal pieces), the most
/// specific first.
pub struct Catalog {
    exact: HashMap<&'static str, &'static str>,
    patterns: Vec<(Vec<&'static str>, &'static str)>,
}

/// The rows of a catalog source, and the problems found (malformed rows, duplicate keys, a
/// translation whose values don't match the English).
pub fn parse(source: &'static str) -> (Catalog, Vec<String>) {
    let mut exact = HashMap::new();
    let mut patterns = Vec::new();
    let mut errors = Vec::new();
    for (i, line) in source.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let (Some(""), Some(key), Some(value), None) = (cols.first().copied(), cols.get(1).copied(), cols.get(2).copied(), cols.get(3)) else {
            errors.push(format!("line {}: expected <TAB>English<TAB>translation", i + 1));
            continue;
        };
        if key.is_empty() || value.is_empty() {
            errors.push(format!("line {}: empty text", i + 1));
            continue;
        }
        let holes = key.matches("{}").count();
        if holes != placeholders(value) {
            errors.push(format!("line {}: {key}: the translation has a different number of values", i + 1));
        }
        if holes == 0 {
            if exact.insert(key, value).is_some() {
                errors.push(format!("line {}: duplicate {key}", i + 1));
            }
        } else if patterns.iter().any(|(p, _): &(Vec<&str>, &str)| p.join("{}") == key) {
            errors.push(format!("line {}: duplicate {key}", i + 1));
        } else {
            patterns.push((key.split("{}").collect(), value));
        }
    }
    // Longer literal text first: "{} found, {} total" before "{} found".
    patterns.sort_by_key(|(p, _)| std::cmp::Reverse(p.iter().map(|s| s.len()).sum::<usize>()));
    (Catalog { exact, patterns }, errors)
}

/// The number of values a translation places (`{}` or `{N}`).
fn placeholders(t: &str) -> usize {
    let mut n = 0;
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        let after = rest.get(i + 1..).unwrap_or_default();
        match after.find('}') {
            Some(j) if after.get(..j).is_some_and(|d| d.chars().all(|c| c.is_ascii_digit())) => {
                n += 1;
                rest = after.get(j + 1..).unwrap_or_default();
            }
            _ => rest = after,
        }
    }
    n
}

/// The catalog of language `code` ("uk", "pt-BR", "pt_br", …), if there is one.
pub fn catalog(code: &str) -> Option<&'static Catalog> {
    static CATALOGS: [OnceLock<Catalog>; 17] = [const { OnceLock::new() }; 17];
    let code = code.trim().to_ascii_lowercase().replace('-', "_");
    let i = LANGUAGES.iter().position(|l| *l == code)?;
    let (cell, source) = (CATALOGS.get(i)?, SOURCES.get(i)?);
    Some(cell.get_or_init(|| parse(source).0))
}

/// The values `pieces` (a pattern's literal text) leave in `text`, if it matches.
fn matches<'t>(pieces: &[&str], text: &'t str) -> Option<Vec<&'t str>> {
    let (first, rest) = pieces.split_first()?;
    let (last, middle) = rest.split_last()?;
    let mut t = text.strip_prefix(first)?;
    let mut values = Vec::with_capacity(pieces.len());
    for m in middle {
        let i = if m.is_empty() { 0 } else { t.find(m)? };
        let (v, after) = t.split_at(i);
        values.push(v);
        t = after.get(m.len()..)?;
    }
    let v = t.strip_suffix(last)?;
    values.push(v);
    values.iter().all(|v| !v.is_empty()).then_some(values)
}

/// `translation` with `values` in its places.
fn fill(translation: &str, values: &[&str]) -> String {
    let mut out = String::with_capacity(translation.len() + 16);
    let mut next = 0;
    let mut rest = translation;
    while let Some(i) = rest.find('{') {
        out.push_str(rest.get(..i).unwrap_or_default());
        let after = rest.get(i + 1..).unwrap_or_default();
        match after.find('}') {
            Some(j) if after.get(..j).is_some_and(|d| d.chars().all(|c| c.is_ascii_digit())) => {
                let digits = after.get(..j).unwrap_or_default();
                let k = if digits.is_empty() {
                    next += 1;
                    next - 1
                } else {
                    digits.parse().unwrap_or(usize::MAX)
                };
                out.push_str(values.get(k).copied().unwrap_or_default());
                rest = after.get(j + 1..).unwrap_or_default();
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn line<'a>(c: &Catalog, text: &'a str) -> Cow<'a, str> {
    if let Some(t) = c.exact.get(text) {
        return Cow::Borrowed(t);
    }
    for (pieces, translation) in &c.patterns {
        if let Some(values) = matches(pieces, text) {
            return Cow::Owned(fill(translation, &values));
        }
    }
    Cow::Borrowed(text)
}

/// `text` in language `code` (each line on its own); unchanged for English, a language without a
/// catalog, or a text the catalog doesn't have.
pub fn tr<'a>(code: &str, text: &'a str) -> Cow<'a, str> {
    let Some(c) = catalog(code) else { return Cow::Borrowed(text) };
    if !text.contains('\n') {
        return line(c, text);
    }
    let lines: Vec<Cow<str>> = text.split('\n').take(MAX_LINES).map(|l| line(c, l)).collect();
    if lines.iter().all(|l| matches!(l, Cow::Borrowed(_))) && text.split('\n').count() <= MAX_LINES {
        return Cow::Borrowed(text);
    }
    Cow::Owned(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_parses_and_has_the_same_texts() {
        let keys = |s: &'static str| -> Vec<String> {
            s.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).filter_map(|l| l.split('\t').nth(1).map(str::to_string)).collect()
        };
        let first = keys(SOURCES[0]);
        assert!(first.len() >= 190, "{}", first.len());
        for (code, source) in LANGUAGES.iter().zip(SOURCES) {
            let (_, errors) = parse(source);
            assert!(errors.is_empty(), "{code}: {errors:?}");
            assert_eq!(keys(source), first, "{code} lists the same texts in the same order");
        }
    }

    #[test]
    fn texts_and_values_are_translated() {
        assert_eq!(tr("uk", "Command: LINE"), "Команда: LINE");
        assert_eq!(tr("de", "Invalid option keyword."), "Ungültiges Optionsschlüsselwort.");
        assert_eq!(tr("ja", "Select hatch object"), "ハッチング オブジェクトを選択");
        assert_eq!(tr("de", "3 found, 5 total"), "3 gefunden, 5 gesamt");
        assert_eq!(tr("de", "3 found"), "3 gefunden");
        // A translation may put the values in another order.
        assert_eq!(tr("cs", "4 object(s) added to group Doors."), "Do skupiny Doors přidány objekty: 4.");
        assert_eq!(tr("pt-BR", "Requires a point."), "Requer um ponto.");
        // Each line on its own.
        assert_eq!(
            tr("de", "Distance = 5.0000,  Angle in XY Plane = 53,  Angle from XY Plane = 0\nDelta X = 3.0000,  Delta Y = 4.0000,   Delta Z = 0.0000"),
            "Abstand = 5.0000,  Winkel in XY-Ebene = 53,  Winkel von XY-Ebene = 0\nDelta X = 3.0000,  Delta Y = 4.0000,   Delta Z = 0.0000"
        );
        // English, unknown languages and texts stay as they are.
        assert_eq!(tr("en", "Command: LINE"), "Command: LINE");
        assert_eq!(tr("xx", "Command: LINE"), "Command: LINE");
        assert_eq!(tr("uk", "Something else entirely"), "Something else entirely");
        assert_eq!(tr("uk", "Command: "), "Command: ", "a value can't be empty");
    }

    #[test]
    fn hostile_patterns_and_texts_do_not_panic() {
        assert_eq!(fill("{9} {} {x {", &["a"]), " a {x {");
        assert_eq!(placeholders("{0}{1}{}{a}"), 3);
        assert_eq!(matches(&["a", "b", "c"], "aXbYc"), Some(vec!["X", "Y"]));
        assert_eq!(matches(&["a", "", "c"], "aXc"), None);
        let long = "x\n".repeat(1000);
        let _ = tr("uk", &long);
        let _ = tr("uk", "{}{}{}");
    }
}
