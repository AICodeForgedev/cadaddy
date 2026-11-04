//! What a code-analysis sheet says of the building (Q6154 (b)): construction type, sprinklers,
//! occupancies, stories, rated assemblies and the electrical code in force, each with the text
//! it was read from. The labels are rules data (`verules.json` → `code_sheet`); the texts come in
//! reading order ([`super::reading_order`]), a value in the label's own text or a few after it.

use serde::Serialize;

use super::Rules;

/// Texts after a label its value may be in.
const VALUE_REACH: usize = 3;
/// Rated assemblies listed at most.
const MAX_RATED: usize = 40;
/// Texts one sheet gives at most (hostile input).
pub(crate) const MAX_TEXTS: usize = 20_000;

/// One fact and the text it was read from.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Fact<T> {
    pub value: T,
    pub source: String,
}

/// A rated assembly the sheet lists (`FLOOR/ROOF CONSTRUCTION 1 HR`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Rated {
    pub assembly: String,
    pub hours: f64,
    pub source: String,
}

/// The building's facts as its code sheet gives them.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct CodeFacts {
    pub construction_type: Option<Fact<String>>,
    pub sprinklered: Option<Fact<bool>>,
    /// `NFPA 13`, `NFPA 13R`… when the sheet names one.
    pub sprinkler_standard: Option<String>,
    pub occupancies: Vec<Fact<String>>,
    pub stories: Option<Fact<u32>>,
    pub rated: Vec<Rated>,
    /// The electrical code in force (`2017 NATIONAL ELECTRIC CODE` → `NEC 2017`).
    pub electrical_code: Option<Fact<String>>,
    /// The sheet the facts came from, when the caller names it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sheet: String,
}

impl CodeFacts {
    pub(crate) fn is_empty(&self) -> bool {
        self.construction_type.is_none() && self.sprinklered.is_none() && self.occupancies.is_empty() && self.stories.is_none() && self.rated.is_empty()
    }
}

fn clip(t: &str) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 120 { t.chars().take(117).collect::<String>() + "…" } else { t }
}

/// A construction type word (`V-A`, `IIB`, `III`): its normal form.
fn construction_word(w: &str) -> Option<String> {
    let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-').to_ascii_uppercase();
    let (num, letter) = match w.split_once('-') {
        Some((n, l)) => (n.to_string(), Some(l.to_string())),
        None => {
            // `VA`, `IIB` written without the dash.
            match w.strip_suffix('A').or_else(|| w.strip_suffix('B')) {
                Some(n) if ["I", "II", "III", "IV", "V"].contains(&n) && w.len() > 1 => (n.to_string(), w.get(n.len()..).map(str::to_string)),
                _ => (w.clone(), None),
            }
        }
    };
    ["I", "II", "III", "IV", "V"].contains(&num.as_str()).then_some(())?;
    match letter {
        Some(l) if ["A", "B", "HT"].contains(&l.as_str()) => Some(format!("{num}-{l}")),
        None => Some(num),
        _ => None,
    }
}

/// The value after `label` in `texts[i]` or the next few texts: the first `pick` finds.
fn after<T>(texts: &[String], i: usize, label: &str, pick: &dyn Fn(&str) -> Option<T>) -> Option<(T, String)> {
    let first = texts.get(i)?;
    let up = first.to_ascii_uppercase();
    let at = up.find(label)? + label.len();
    let rest = up.get(at..).unwrap_or_default().to_string();
    if let Some(v) = pick(&rest) {
        return Some((v, clip(first)));
    }
    texts.iter().skip(i + 1).take(VALUE_REACH).find_map(|t| pick(&t.to_ascii_uppercase()).map(|v| (v, clip(&format!("{first} {t}")))))
}

/// The construction type: `CONSTRUCTION TYPE V-A`, `TYPE OF CONSTRUCTION: TYPE V-A`.
pub(crate) fn construction(texts: &[String], rules: &Rules) -> Option<Fact<String>> {
    let pick = |x: &str| x.split_whitespace().find_map(construction_word);
    for (i, t) in texts.iter().enumerate() {
        let up = t.to_ascii_uppercase();
        for label in &rules.code_sheet.construction {
            if up.contains(label.as_str())
                && let Some((v, src)) = after(texts, i, label, &pick)
            {
                return Some(Fact { value: v, source: src });
            }
        }
    }
    None
}

/// Whether the building is sprinklered, and the standard when named.
fn sprinklers(texts: &[String], rules: &Rules) -> (Option<Fact<bool>>, Option<String>) {
    let cs = &rules.code_sheet;
    let mut found: Option<Fact<bool>> = None;
    let mut standard: Option<String> = None;
    for t in texts {
        let up = t.to_ascii_uppercase().split_whitespace().collect::<Vec<_>>().join(" ");
        if found.is_none() {
            if cs.sprinklered_no.iter().any(|w| up.contains(w.as_str())) {
                found = Some(Fact { value: false, source: clip(t) });
            } else if cs.sprinklered_yes.iter().any(|w| up.contains(w.as_str())) {
                found = Some(Fact { value: true, source: clip(t) });
            }
        }
        if standard.is_none() {
            standard = cs.sprinkler_standards.iter().find(|w| up.contains(w.as_str())).cloned();
        }
        if found.is_some() && standard.is_some() {
            break;
        }
    }
    (found, standard)
}

/// The occupancies (`R-1`, `B`) named beside an occupancy or group word.
fn occupancies(texts: &[String], rules: &Rules) -> Vec<Fact<String>> {
    let cs = &rules.code_sheet;
    let mut out: Vec<Fact<String>> = Vec::new();
    for t in texts {
        let up = t.to_ascii_uppercase();
        if !cs.occupancy_words.iter().any(|w| up.contains(w.as_str())) {
            continue;
        }
        // `R-1 OCCUPANCY`, `GROUP R-1`, `(R-1)`, `B OCCUPANCY`: a word that is one of the groups.
        let words: Vec<String> =
            up.split(|c: char| c.is_whitespace() || matches!(c, ',' | '(' | ')' | ':' | '/' | ';')).filter(|w| !w.is_empty()).map(str::to_string).collect();
        for (k, w) in words.iter().enumerate() {
            let w = w.trim_matches('.');
            if !cs.occupancies.iter().any(|o| o == w) {
                continue;
            }
            // A one-letter group needs its own word beside it (`B OCCUPANCY`, `GROUP B`).
            if w.len() == 1 {
                let near = |j: Option<usize>| j.and_then(|j| words.get(j)).is_some_and(|x| cs.occupancy_words.iter().any(|o| x.starts_with(o.as_str())));
                if !near(k.checked_sub(1)) && !near(Some(k + 1)) {
                    continue;
                }
            }
            if !out.iter().any(|o| o.value == w) && out.len() < 20 {
                out.push(Fact { value: w.to_string(), source: clip(t) });
            }
        }
    }
    out
}

/// The building's stories: `4 STORIES` (the actual, not the allowable), `NUMBER OF FLOORS: 4`.
fn stories(texts: &[String], rules: &Rules) -> Option<Fact<u32>> {
    let cs = &rules.code_sheet;
    let num_before = |up: &str, word: &str| -> Option<u32> {
        let at = up.find(word)?;
        up.get(..at)?.split(|c: char| c.is_whitespace() || c == '/').rev().find(|w| !w.is_empty())?.parse::<u32>().ok().filter(|n| (1..=200).contains(n))
    };
    let num_in = |up: &str| -> Option<u32> { up.split_whitespace().find_map(|w| w.trim_matches(|c: char| !c.is_ascii_digit()).parse::<u32>().ok()).filter(|n| (1..=200).contains(n)) };
    let mut allowable: Option<Fact<u32>> = None;
    for (i, t) in texts.iter().enumerate() {
        let up = t.to_ascii_uppercase();
        let Some(word) = cs.stories_words.iter().find(|w| up.contains(w.as_str())) else { continue };
        // A label the value follows (`NUMBER OF FLOORS:` then `4`), else the count before the word.
        let n = num_before(&up, word).or_else(|| up.ends_with(':').then(|| texts.get(i + 1).and_then(|x| num_in(x))).flatten());
        let Some(n) = n else { continue };
        // The allowable is the code's limit, not the building's: the actual wins.
        let prev = texts.get(i.wrapping_sub(1)).map(|x| x.to_ascii_uppercase()).unwrap_or_default();
        let ctx = format!("{prev} {up}");
        let fact = Fact { value: n, source: clip(t) };
        if ctx.contains("ALLOW") && !up.contains("ACTUAL") {
            allowable.get_or_insert(fact);
            continue;
        }
        return Some(fact);
    }
    allowable
}

/// The rated assemblies: `FLOOR/ROOF CONSTRUCTION 1 HR`, `2 HOUR FIRE RATED BARRIER - UL U301`.
fn rated(texts: &[String], rules: &Rules) -> Vec<Rated> {
    let cs = &rules.code_sheet;
    let hours_in = |up: &str| -> Option<(f64, usize, usize)> {
        let words: Vec<&str> = up.split_whitespace().collect();
        for (k, w) in words.iter().enumerate() {
            let unit = words.get(k + 1).map(|x| x.trim_matches(|c: char| !c.is_ascii_alphanumeric()));
            let (num, glued) = match w.find(|c: char| c.is_ascii_alphabetic()) {
                Some(p) => (w.get(..p).unwrap_or_default(), w.get(p..)),
                None => (*w, None),
            };
            let unit_word = glued.or(unit).unwrap_or_default();
            if !cs.rated_words.iter().any(|r| r == unit_word.trim_end_matches('.')) {
                continue;
            }
            let h = match num.trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '/') {
                "1/2" => 0.5,
                "3/4" => 0.75,
                n => match n.parse::<f64>() {
                    Ok(v) if (0.0..=4.0).contains(&v) => v,
                    _ => continue,
                },
            };
            return Some((h, k, if glued.is_some() { k + 1 } else { k + 2 }));
        }
        None
    };
    let mut out: Vec<Rated> = Vec::new();
    for (i, t) in texts.iter().enumerate() {
        if out.len() >= MAX_RATED {
            break;
        }
        let up = t.to_ascii_uppercase().split_whitespace().collect::<Vec<_>>().join(" ");
        if cs.rated_skip.iter().any(|s| up.contains(s.as_str())) {
            continue;
        }
        let Some((h, k, after_k)) = hours_in(&up) else { continue };
        let words: Vec<&str> = up.split_whitespace().collect();
        // The assembly: the words after the rating (`1 HOUR FIRE RATED PARTITION`), else before
        // it, else the text before (a table's label column: `STRUCTURAL FRAME` then `1 HR`).
        let tail = words.get(after_k..).unwrap_or_default().join(" ");
        let head = words.get(..k).unwrap_or_default().join(" ");
        let assembly = if tail.chars().any(char::is_alphabetic) {
            tail
        } else if head.chars().any(char::is_alphabetic) {
            head
        } else {
            match i.checked_sub(1).and_then(|j| texts.get(j)) {
                Some(p) if p.chars().any(char::is_alphabetic) && hours_in(&p.to_ascii_uppercase()).is_none() => p.to_ascii_uppercase(),
                _ => continue,
            }
        };
        let assembly = clip(assembly.trim_matches(|c: char| c == '-' || c == ':' || c.is_whitespace()));
        if assembly.is_empty() || out.iter().any(|r| r.assembly == assembly && r.hours == h) {
            continue;
        }
        out.push(Rated { assembly, hours: h, source: clip(t) });
    }
    out
}

/// The electrical code in force: `2017 NATIONAL ELECTRIC CODE`, `NFPA 70 (2020)`, `NEC 2023`.
fn electrical_code(texts: &[String], rules: &Rules) -> Option<Fact<String>> {
    let year = |s: &str| -> Option<u32> { s.split(|c: char| !c.is_ascii_digit()).filter_map(|w| w.parse::<u32>().ok()).find(|y| (1990..=2100).contains(y)) };
    for t in texts {
        let up = t.to_ascii_uppercase();
        let Some(w) = rules.code_sheet.nec_words.iter().find(|w| up.contains(w.as_str())) else { continue };
        // `NEC` is its own word, not part of another (`CONNECT`).
        if w == "NEC" && !up.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == "NEC") {
            continue;
        }
        let without = up.replace("NFPA 70", "");
        if let Some(y) = year(&without) {
            return Some(Fact { value: format!("NEC {y}"), source: clip(t) });
        }
    }
    None
}

/// Everything the sheet's texts say.
pub(crate) fn read(texts: &[String], rules: &Rules) -> CodeFacts {
    let texts = texts.get(..texts.len().min(MAX_TEXTS)).unwrap_or_default();
    let (sprinklered, sprinkler_standard) = sprinklers(texts, rules);
    CodeFacts {
        construction_type: construction(texts, rules),
        sprinklered,
        sprinkler_standard,
        occupancies: occupancies(texts, rules),
        stories: stories(texts, rules),
        rated: rated(texts, rules),
        electrical_code: electrical_code(texts, rules),
        sheet: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_code_sheet_gives_its_facts_in_either_layout() {
        let rules = super::super::rules();
        // A label column and its values beside it, read in rows.
        let a = read(
            &t(&[
                "USE AND OCCUPANCY CLASSIFICATION",
                "RESIDENTIAL GROUP R-2 OCCUPANCY (310.4)",
                "MERCANTILE GROUP M OCCUPANCY",
                "ALLOWABLE:",
                "85 FEET / 5 STORIES",
                "ACTUAL:",
                "52 FEET / 3 STORIES",
                "CONSTRUCTION TYPE            III-B",
                "AUTOMATIC SPRINKLER SYSTEM    PROVIDED (NFPA 13R)",
                "STRUCTURAL FRAME",
                "2 HR",
                "NON-BEARING WALLS INTERIOR",
                "0 HR",
                "1 HOUR FIRE RATED PARTITION - UL U999",
                "2020 NATIONAL ELECTRICAL CODE",
            ]),
            &rules,
        );
        assert_eq!(a.construction_type.as_ref().map(|f| f.value.as_str()), Some("III-B"));
        assert_eq!(a.sprinklered.as_ref().map(|f| f.value), Some(true));
        assert_eq!(a.sprinkler_standard.as_deref(), Some("NFPA 13R"));
        let occ: Vec<&str> = a.occupancies.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(occ, ["R-2", "M"]);
        assert_eq!(a.stories.as_ref().map(|f| f.value), Some(3), "the actual, not the allowable");
        let rated: Vec<(String, f64)> = a.rated.iter().map(|r| (r.assembly.clone(), r.hours)).collect();
        assert!(rated.contains(&("STRUCTURAL FRAME".into(), 2.0)), "{rated:?}");
        assert!(rated.contains(&("NON-BEARING WALLS INTERIOR".into(), 0.0)), "{rated:?}");
        assert!(rated.contains(&("FIRE RATED PARTITION - UL U999".into(), 1.0)), "{rated:?}");
        assert_eq!(a.electrical_code.as_ref().map(|f| f.value.as_str()), Some("NEC 2020"));

        // Labels first, values after (a form), a different wording, no sprinklers.
        let b = read(
            &t(&[
                "TYPE OF CONSTRUCTION:",
                "TYPE IIB",
                "NUMBER OF FLOORS:",
                "2",
                "SPRINKLERED: NO",
                "OCCUPANCY: B",
                "CODES: NFPA 70 (2023)",
            ]),
            &rules,
        );
        assert_eq!(b.construction_type.as_ref().map(|f| f.value.as_str()), Some("II-B"));
        assert_eq!(b.sprinklered.as_ref().map(|f| f.value), Some(false));
        assert_eq!(b.stories.as_ref().map(|f| f.value), Some(2));
        assert_eq!(b.occupancies.iter().map(|o| o.value.as_str()).collect::<Vec<_>>(), ["B"]);
        assert_eq!(b.electrical_code.as_ref().map(|f| f.value.as_str()), Some("NEC 2023"));

        // Nothing to read: nothing claimed.
        let c = read(&t(&["GENERAL NOTES", "1. VERIFY ALL DIMENSIONS", "CONNECT TO EXISTING 2019 SERVICE"]), &rules);
        assert!(c.is_empty() && c.electrical_code.is_none(), "{c:?}");
    }

    #[test]
    fn hostile_code_sheet_texts_never_panic() {
        let rules = super::super::rules();
        let mut texts: Vec<String> = (0..30_000).map(|i| format!("{} HR {}", i % 7, "Ω".repeat(i % 5))).collect();
        texts.push("CONSTRUCTION TYPE".into());
        texts.push(String::new());
        texts.push("STORIES 4294967296".into());
        texts.push("HOUR".into());
        texts.push("-".repeat(10_000));
        let f = read(&texts, &rules);
        assert!(f.rated.len() <= MAX_RATED);
        let _ = read(&[], &rules);
    }
}
