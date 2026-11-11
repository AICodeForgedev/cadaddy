//! CADaddy's own labor book (Q6526): the hours per unit every estimate route gives a takeoff
//! line when the user's data has none. One rule for `bim.bid`, `bim.estimate` and the
//! `estimate.*` commands, so one model gives one labor total whichever is used:
//! 1. the starter cost book's row for the item (and size, else the item's row with no size);
//! 2. else the labor book's first family rule that fits (`assets/costbook/labor_book.csv`: the
//!    trade and category it applies to, the words the item holds, a size or size range: trade
//!    size in inches, wire gauge, amps, poles or gangs, and the unit), with its basis;
//! 3. else the starter book's word-family row, never of another trade;
//! 4. else the labor book's row for the line's category, else for its trade.
//!
//! Every row is a placeholder until a company's own labor units replace it, and says so.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde_json::Value;

const BOOK_CSV: &str = include_str!("../../../../../assets/costbook/labor_book.csv");
/// Where the labor book's hours come from.
pub(crate) const BOOK_SOURCE: &str = "CADaddy labor book";
/// What every CADaddy-book source carries until a company's units replace it.
pub(crate) const PLACEHOLDER: &str = "(placeholder hours: replace with your own)";
/// Family rows read, at most (the book is shipped data, but stays bounded).
const MAX_BOOK_ROWS: usize = 5_000;

/// A size's measure: what a size range compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dim {
    /// Trade size or box size in inches.
    Inch,
    /// Conductor area in kcmil (gauges converted).
    Wire,
    Amps,
    Poles,
    Gang,
}

#[derive(Clone, Debug, PartialEq)]
enum SizeRule {
    Any,
    Range(Dim, f64, f64),
    /// A size the book can't measure: compared as written.
    Text(String),
}

#[derive(Clone, Debug)]
struct Row {
    trade: String,
    category: String,
    words: BTreeSet<String>,
    size: SizeRule,
    unit: String,
    hours: f64,
    family: String,
    basis: String,
}

/// A line's labor unit from CADaddy's books.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    pub(crate) hours: f64,
    /// The book, as the line's labor source.
    pub(crate) source: String,
    /// The row it came from and why it fits.
    pub(crate) basis: String,
    /// `item` (the starter book's row for the item), `family` (a labor-book or starter family
    /// row) or `category` (the labor book's row for the line's category or trade).
    pub(crate) how: &'static str,
}

fn norm(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// A size as compared when it can't be measured: `#2/0` = `2/0`, spaces dropped.
fn size_text(t: &str) -> String {
    t.trim().trim_start_matches('#').chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase()
}

/// Conductor area in kcmil of an AWG gauge (`0` = 1/0, `-1` = 2/0 …).
fn awg_kcmil(n: f64) -> f64 {
    let d = 0.005 * 92f64.powf((36.0 - n) / 39.0);
    d * d * 1000.0
}

/// `3/4`, `1-1/4`, `1 1/4`, `4`, `2.125` as inches.
fn inches(t: &str) -> Option<f64> {
    let t = t.trim().trim_start_matches(['-', ' ']).trim();
    let frac = |f: &str| -> Option<f64> {
        let (n, d) = f.split_once('/')?;
        let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
        (d > 0.0).then_some(n / d)
    };
    let v = match t.rsplit_once(['-', ' ']) {
        Some((whole, f)) if f.contains('/') => whole.trim().parse::<f64>().ok()? + frac(f)?,
        _ if t.contains('/') => frac(t)?,
        _ => t.parse::<f64>().ok()?,
    };
    (v.is_finite() && v > 0.0).then_some(v)
}

/// The measures a size text holds: `3/4"`, `#12`, `2/0`, `250 kcmil`, `20A 1P`, `2 gang, 1/2"
/// deep`. The first of each kind.
fn measures(t: &str) -> Vec<(Dim, f64)> {
    let s: Vec<char> = t.to_lowercase().chars().take(200).collect();
    let mut out: Vec<(Dim, f64)> = Vec::new();
    let mut put = |d: Dim, v: f64| {
        if v.is_finite() && v >= 0.0 && !out.iter().any(|(k, _)| *k == d) {
            out.push((d, v));
        }
    };
    let text: String = s.iter().collect();
    // Wire: `#12`, `#2/0`, `1/0`, `250 kcmil`.
    let gauge = |g: &str| -> Option<f64> {
        let g = g.trim();
        match g.split_once('/') {
            Some((k, "0")) => k.parse::<f64>().ok().filter(|k| (1.0..=4.0).contains(k)).map(|k| awg_kcmil(1.0 - k)),
            Some(_) => None,
            None => g.parse::<f64>().ok().filter(|n| (0.0..=40.0).contains(n)).map(awg_kcmil),
        }
    };
    let digits_at = |i: usize| -> String { s.iter().skip(i).take_while(|c| c.is_ascii_digit() || **c == '/').collect() };
    for (i, c) in s.iter().enumerate() {
        if *c == '#'
            && let Some(v) = gauge(&digits_at(i + 1))
        {
            put(Dim::Wire, v);
        }
    }
    if let Some(i) = text.find("kcmil").or_else(|| text.find("mcm")) {
        let before: String = text.get(..i).unwrap_or_default().trim_end().chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if let Ok(v) = before.chars().rev().collect::<String>().parse::<f64>() {
            put(Dim::Wire, v);
        }
    }
    // Inches: the number before each `"`.
    for (i, c) in s.iter().enumerate() {
        if *c == '"' {
            let back: String = s.iter().take(i).rev().take_while(|c| c.is_ascii_digit() || matches!(c, '/' | '.' | '-' | ' ')).collect();
            if let Some(v) = inches(&back.chars().rev().collect::<String>()) {
                put(Dim::Inch, v);
            }
        }
    }
    // Numbers followed by a unit word: `20A`, `2000 A`, `1P`, `2 gang`, and a bare `2/0`.
    let mut i = 0;
    while i < s.len() {
        let starts = s.get(i).is_some_and(char::is_ascii_digit)
            && (i == 0 || s.get(i - 1).is_some_and(|c| !c.is_ascii_alphanumeric() && *c != '#' && *c != '/' && *c != '.'));
        if !starts {
            i += 1;
            continue;
        }
        let num: String = s.iter().skip(i).take_while(|c| c.is_ascii_digit() || **c == '.').collect();
        let mut j = i + num.chars().count();
        let v = num.parse::<f64>().ok();
        if s.get(j) == Some(&'/') && s.get(j + 1) == Some(&'0') && s.get(j + 2).is_none_or(|c| !c.is_ascii_digit() && *c != '"') {
            if let Some(w) = gauge(&format!("{num}/0")) {
                put(Dim::Wire, w);
            }
            i = j + 2;
            continue;
        }
        while s.get(j) == Some(&' ') {
            j += 1;
        }
        let word: String = s.iter().skip(j).take_while(|c| c.is_ascii_alphabetic()).collect();
        if let Some(v) = v {
            match word.as_str() {
                "a" | "amp" | "amps" => put(Dim::Amps, v),
                "p" | "pole" | "poles" => put(Dim::Poles, v),
                "gang" | "g" => put(Dim::Gang, v),
                _ => {}
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// A row's size: empty (any), one size, or `lo..hi` (either end may be left open).
fn size_rule(t: &str) -> SizeRule {
    let t = t.trim();
    if t.is_empty() {
        return SizeRule::Any;
    }
    let first = |x: &str| measures(x).into_iter().next();
    match t.split_once("..") {
        Some((lo, hi)) => {
            let (l, h) = (first(lo), first(hi));
            match (l, h) {
                (Some((dl, a)), Some((dh, b))) if dl == dh => SizeRule::Range(dl, a.min(b), a.max(b)),
                (None, Some((d, b))) if lo.trim().is_empty() => SizeRule::Range(d, f64::NEG_INFINITY, b),
                (Some((d, a)), None) if hi.trim().is_empty() => SizeRule::Range(d, a, f64::INFINITY),
                _ => SizeRule::Text(size_text(t)),
            }
        }
        None => match first(t) {
            Some((d, v)) => SizeRule::Range(d, v, v),
            None => SizeRule::Text(size_text(t)),
        },
    }
}

fn size_fits(rule: &SizeRule, size: &str) -> bool {
    match rule {
        SizeRule::Any => true,
        SizeRule::Text(t) => *t == size_text(size),
        SizeRule::Range(d, lo, hi) => measures(size)
            .iter()
            .find(|(k, _)| k == d)
            .is_some_and(|(_, v)| *v >= lo - 1e-6 * lo.abs().max(1.0) && *v <= hi + 1e-6 * hi.abs().max(1.0)),
    }
}

fn book() -> &'static [Row] {
    static BOOK: OnceLock<Vec<Row>> = OnceLock::new();
    BOOK.get_or_init(|| {
        super::csv_rows(BOOK_CSV)
            .iter()
            .take(MAX_BOOK_ROWS)
            .filter_map(|r| {
                let g = |k: &str| r.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
                let hours = g("hours").parse::<f64>().ok().filter(|h| h.is_finite() && *h >= 0.0)?;
                let unit = g("unit");
                super::measure_class(&unit)?;
                Some(Row {
                    trade: norm(&g("trade")),
                    category: norm(&g("category")),
                    words: super::price_words(&g("words")),
                    size: size_rule(&g("size")),
                    unit,
                    hours,
                    family: g("family"),
                    basis: g("basis"),
                })
            })
            .collect()
    })
}

/// Does a family row of `row_trade` apply to a line of `line_trade`? Never across trades; a
/// row or line naming no trade (or the starter book's `general`) applies to any.
pub(crate) fn same_trade(row_trade: &str, line_trade: &str) -> bool {
    let (r, l) = (norm(row_trade), norm(line_trade));
    r.is_empty() || l.is_empty() || r == "general" || l == "general" || r == l
}

fn same_measure(a: &str, b: &str) -> bool {
    crate::cmd::pricebook::same_unit(a, b) || super::measure_class(a).is_some_and(|m| Some(m) == super::measure_class(b))
}

/// The hours per unit CADaddy's books give a takeoff or BOM line (`item`, `size`, `unit`,
/// `category`, `trade`), and where they come from.
pub(crate) fn labor_for(line: &Value) -> Option<Hit> {
    let s = |k: &str| line[k].as_str().unwrap_or_default();
    let (item, size, unit, category, trade) = (s("item"), s("size"), s("unit"), norm(s("category")), norm(s("trade")));
    let starter = starter();
    let starter_source = format!("{} {PLACEHOLDER}", super::STARTER_SOURCE);
    // 1. The starter book's row for the item.
    let named = |sz: &str| {
        starter.iter().find(|p| {
            norm(p["item"].as_str().unwrap_or_default()) == norm(item)
                && size_text(p["size"].as_str().unwrap_or_default()) == size_text(sz)
                && same_measure(p["unit"].as_str().unwrap_or_default(), unit)
        })
    };
    if let Some((p, h)) = named(size).or_else(|| named("")).and_then(|p| p["labor_hours"].as_f64().map(|h| (p, h))) {
        let row = p["item"].as_str().unwrap_or_default();
        return Some(Hit { hours: h, source: starter_source, basis: format!("the starter book's row {row}"), how: "item" });
    }
    // 2. The labor book's family rules (words or a size), first fit.
    let words = super::price_words(item);
    let fits = |r: &&Row| {
        same_measure(&r.unit, unit)
            && (r.trade.is_empty() || r.trade == trade)
            && (r.category.is_empty() || r.category == category)
            && r.words.iter().all(|w| words.contains(w))
            && size_fits(&r.size, size)
    };
    let catch_all = |r: &Row| r.words.is_empty() && matches!(r.size, SizeRule::Any);
    let from_book = |r: &Row, how: &'static str| Hit {
        hours: r.hours,
        source: format!("{BOOK_SOURCE} {PLACEHOLDER}"),
        basis: format!("{}: {}", r.family, r.basis),
        how,
    };
    if let Some(r) = book().iter().filter(|r| !catch_all(r)).find(fits) {
        return Some(from_book(r, "family"));
    }
    // 3. The starter book's word family, of the line's trade.
    if let Some(p) = super::find_family_match(starter, item, unit, &category, s("system"), &trade).and_then(|i| starter.get(i))
        && let Some(h) = p["labor_hours"].as_f64()
    {
        let row = p["item"].as_str().unwrap_or_default();
        return Some(Hit { hours: h, source: starter_source, basis: format!("the starter book's family row {row}"), how: "family" });
    }
    // 4. The labor book's row for the line's category, else its trade.
    book().iter().filter(|r| catch_all(r)).find(fits).map(|r| from_book(r, "category"))
}

fn starter() -> &'static [Value] {
    static STARTER: OnceLock<Vec<Value>> = OnceLock::new();
    STARTER.get_or_init(super::starter_prices)
}

/// The lines no book gives labor, by trade and category: `[{trade, category, lines, qty, unit}]`
/// (`qty` per unit when a category counts in more than one).
pub(crate) fn uncovered<'a>(lines: impl Iterator<Item = (&'a Value, f64)>) -> Vec<Value> {
    let mut by: std::collections::BTreeMap<(String, String, String), (usize, f64)> = std::collections::BTreeMap::new();
    for (l, qty) in lines {
        let s = |k: &str| l[k].as_str().unwrap_or_default().to_string();
        let unit = if crate::cmd::pricebook::same_unit(&s("unit"), "each") { "each".to_string() } else { s("unit") };
        let e = by.entry((s("trade"), s("category"), unit)).or_default();
        e.0 += 1;
        e.1 += qty;
    }
    by.into_iter()
        .map(|((trade, category, unit), (n, q))| serde_json::json!({"trade": trade, "category": category, "lines": n, "qty": (q * 1000.0).round() / 1000.0, "unit": unit}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hit(trade: &str, category: &str, item: &str, size: &str, unit: &str) -> Option<Hit> {
        labor_for(&json!({"trade": trade, "category": category, "item": item, "size": size, "unit": unit}))
    }

    #[test]
    fn sizes_are_measured_in_their_own_kind() {
        let m = |t: &str| measures(t);
        assert_eq!(m("3/4\""), vec![(Dim::Inch, 0.75)]);
        assert_eq!(m("1-1/4\""), vec![(Dim::Inch, 1.25)]);
        assert_eq!(m("1 gang, 1 1/4\" deep"), vec![(Dim::Inch, 1.25), (Dim::Gang, 1.0)]);
        assert_eq!(m("20A 1P"), vec![(Dim::Amps, 20.0), (Dim::Poles, 1.0)]);
        assert_eq!(m("2000 A"), vec![(Dim::Amps, 2000.0)]);
        assert_eq!(m("250 kcmil"), vec![(Dim::Wire, 250.0)]);
        let w = |t: &str| m(t).first().map(|x| x.1).unwrap_or(f64::NAN);
        assert!(
            (w("#12") - 6.53).abs() < 0.01 && (w("#1/0") - 105.6).abs() < 0.2 && (w("2/0") - 133.1).abs() < 0.2,
            "{} {} {}",
            w("#12"),
            w("#1/0"),
            w("2/0")
        );
        assert!(w("#14") < w("#12") && w("#12") < w("#10") && w("#1") < w("#1/0") && w("#4/0") < w("250 kcmil"));
        assert!(m("1'-8\" x 1'-0\" x 4'-0\" H").iter().all(|(d, _)| *d == Dim::Inch), "dimensions are no rating");
        assert_eq!(size_rule("..60A"), SizeRule::Range(Dim::Amps, f64::NEG_INFINITY, 60.0));
        assert_eq!(size_rule("odd"), SizeRule::Text("odd".into()));
    }

    #[test]
    fn every_book_row_reads_and_has_its_basis() {
        let rows = csv_rows_count();
        assert_eq!(book().len(), rows, "a labor-book row did not read (hours, unit)");
        for r in book() {
            assert!(!r.family.is_empty() && !r.basis.is_empty(), "{r:?}");
            assert!(!matches!(r.size, SizeRule::Text(_)), "a size the book can't measure: {r:?}");
        }
    }

    fn csv_rows_count() -> usize {
        super::super::csv_rows(BOOK_CSV).len()
    }

    #[test]
    fn electrical_kinds_get_labor_by_family_and_size() {
        let h = |t: &str, c: &str, i: &str, s: &str, u: &str| hit(t, c, i, s, u).map(|h| h.hours);
        // Wire by gauge, whatever the item calls it.
        assert_eq!(h("electrical", "wire", "THHN copper", "#12", "ft"), Some(0.01));
        assert_eq!(h("electrical", "wire", "Copper ground, green", "#8", "ft"), Some(0.02));
        assert_eq!(h("electrical", "wire", "XHHW aluminum", "500 kcmil", "ft"), Some(0.08));
        // Raceway by its material and trade size.
        assert_eq!(h("electrical", "conduit", "PVC Sch 40 conduit", "4\"", "ft"), Some(0.1));
        assert_eq!(h("electrical", "conduit", "EMT conduit", "4\"", "ft"), Some(0.22));
        assert_eq!(h("electrical", "conduit fittings", "RMC 90° elbow", "5\"", "each"), Some(0.9));
        assert_eq!(h("electrical", "conduit fittings", "Locknut", "1/2\"", "each"), Some(0.01));
        // Devices by their words, breakers by poles, equipment by kind (tags and marks ignored).
        assert_eq!(h("electrical", "devices", "switch, 3-way, dimmer, 0-10V", "", "each"), Some(0.45));
        assert_eq!(h("electrical", "devices", "luminaire X7", "", "each"), Some(0.75));
        assert_eq!(h("electrical", "breakers", "Circuit breaker, AFCI, panel Q9", "20A 1P", "each"), Some(0.2));
        assert_eq!(h("electrical", "equipment", "switchboard MSB-9", "2'-4\" x 4'-9\" x 7'-6\" H", "each"), Some(24.0));
        assert_eq!(h("electrical", "equipment", "Disconnect switch DS-9", "2000 A", "each"), Some(12.0));
        // A tool is bought, not installed.
        assert_eq!(h("electrical", "grounding", "Exothermic weld mould, cable to rod (tool)", "", "each"), Some(0.0));
        // The starter book's own rows first.
        let r = hit("electrical", "conduit", "EMT conduit", "3/4\"", "ft");
        assert_eq!(r.as_ref().map(|h| (h.hours, h.how)), Some((0.05, "item")), "{r:?}");
        // Any other electrical kind still has its category's (or the trade's) hours, said so.
        let r = hit("electrical", "lug kits", "Mechanical lug kit", "", "each");
        assert_eq!(r.as_ref().map(|h| h.how), Some("category"), "{r:?}");
        assert!(r.is_some_and(|h| h.source.contains(PLACEHOLDER) && !h.basis.is_empty()));
    }

    #[test]
    fn uncovered_lines_are_grouped_by_trade_and_category() {
        let a = json!({"trade": "mechanical", "category": "fittings", "item": "CW elbow 90°", "unit": "ea"});
        let b = json!({"trade": "mechanical", "category": "fittings", "item": "HW elbow 90°", "unit": "each"});
        let c = json!({"trade": "job", "category": "required", "item": "Fish tape", "unit": "each"});
        let by = uncovered([(&a, 4.0), (&b, 2.5), (&c, 1.0)].into_iter());
        assert_eq!(
            json!(by),
            json!([
                {"trade": "job", "category": "required", "lines": 1, "qty": 1.0, "unit": "each"},
                {"trade": "mechanical", "category": "fittings", "lines": 2, "qty": 6.5, "unit": "each"}
            ])
        );
    }

    #[test]
    fn a_family_match_never_crosses_trades() {
        // A pipe elbow is no EMT elbow, a duct elbow no conduit elbow.
        for (cat, item, size) in [("fittings", "CW elbow 90°", "1\""), ("fittings", "SA elbow 90°", "12\"")] {
            let r = hit("mechanical", cat, item, size, "each");
            assert!(r.as_ref().is_none_or(|h| !h.basis.contains("EMT") && !h.basis.contains("Raceway")), "{item}: {r:?}");
        }
        assert!(same_trade("general", "drywall") && same_trade("", "electrical") && !same_trade("electrical", "mechanical"));
        // A trade's catch-all row comes after the starter book's own family for the kind.
        let r = hit("mechanical", "hangers", "Hanger, conduit", "3/4\"", "each");
        assert_eq!(r.as_ref().map(|h| (h.hours, h.how)), Some((0.15, "family")), "{r:?}");
    }
}
