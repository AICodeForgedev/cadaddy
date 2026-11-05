//! `bim.estimate {price_book}` (Q6164): the takeoff priced from a price-book workbook, no person
//! mapping its lines by hand.
//!
//! The workbook has a `Price List` sheet (key, description, catalog, unit, unit price $, source
//! type WEB/MCC/NLK/QUOTE, vendor, URL, price date) and an `Item Map` sheet (CADaddy item, size,
//! action price|exclude, price key 1, price key 2, note). Each takeoff line is priced by, in order:
//! 1. its **catalog number** (the submittal's, carried on the BOM line) in the Price List: the
//!    key of the rows that carry it (its price the key's lowest current quote);
//! 2. its **Item Map** row (item and size, else the item with no size): the keys it names added
//!    (a fixture and its trim), or excluded with the row's reason;
//! 3. else it is **unpriced**, listed with why, never priced at zero.
//!
//! Each priced line says its unit $, extended $, key(s), price date and source type; one priced
//! from a quote older than `stale_days` is flagged `STALE`. Totals go by trade and category, with
//! the tax at `tax_pct` as its own line. The XLSX export keeps the formulas live (unit $ =
//! INDEX/MATCH of the line's keys into the workbook's Price List, written beside it), so a price
//! edited in the workbook reprices the takeoff without CADaddy. Material only: labour is the cost
//! table's (`cost_table`) or the estimate's (`estimate.*`).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::r3;
use crate::Result;
use crate::cmd::pricebook::{self, Book, Chosen};
use crate::cmd::table::xlsx::Cell;
use crate::cmd::{bad, str_param};

const CMD: &str = "bim.estimate";
/// A price older than this many days is stale (the default).
const STALE_DAYS: i64 = 90;
/// Lines priced at most (hostile models).
const MAX_LINES: usize = 50_000;
/// Price keys one line adds at most (the Item Map's columns).
const MAX_KEYS: usize = 3;
const PRICE_SHEET: &str = "Price List";
const TAKEOFF_SHEET: &str = "Priced Takeoff";

/// A catalog number as compared: its letters and digits, upper case (`LDN6-35/15` = `ldn6 35 15`).
fn catalog_key(c: &str) -> String {
    c.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).take(120).collect()
}

/// What pricing a line came to.
enum Outcome {
    Priced { by: &'static str, parts: Vec<Chosen> },
    Excluded(String),
    Unpriced(String),
}

/// A line's size words, for a reason (`` (size 2x4)``).
fn sized(item: &str, size: &str) -> String {
    if size.is_empty() { format!("\"{item}\"") } else { format!("\"{item}\" (size {size})") }
}

/// How the book prices one takeoff line.
fn price(book: &Book, by_catalog: &BTreeMap<String, BTreeSet<String>>, l: &Value) -> Outcome {
    let s = |k: &str| l[k].as_str().unwrap_or_default().trim().to_string();
    let (item, size, unit, catalog) = (s("item"), s("size"), s("unit"), s("catalog"));
    // 1. Its catalog number in the Price List: the cheapest key carrying it in the line's unit.
    let mut catalog_note = String::new();
    if let Some(keys) = by_catalog.get(&catalog_key(&catalog)).filter(|_| !catalog_key(&catalog).is_empty()) {
        let best = keys
            .iter()
            .filter_map(|k| pricebook::choose(book, k))
            .filter(|c| pricebook::same_unit(&c.quote.unit, &unit))
            .min_by(|a, b| a.quote.unit_price.total_cmp(&b.quote.unit_price));
        match best {
            Some(c) => return Outcome::Priced { by: "catalog", parts: vec![c] },
            None => {
                catalog_note =
                    format!("catalog {catalog} is in the Price List ({}) but not per {unit}; ", keys.iter().cloned().collect::<Vec<_>>().join(", "))
            }
        }
    }
    // 2. Its Item Map row: item and size, else the item with no size.
    let words = |t: &str| t.split_whitespace().map(str::to_ascii_lowercase).collect::<Vec<_>>();
    let named = |m: &&pricebook::MapRow| words(&m.item) == words(&item);
    let row = book
        .map
        .iter()
        .rev()
        .filter(named)
        .find(|m| words(&m.size) == words(&size))
        .or_else(|| book.map.iter().rev().filter(named).find(|m| m.size.trim().is_empty()));
    let Some(row) = row else {
        let why = if catalog.is_empty() {
            format!("{catalog_note}no catalog number known and no Item Map row for {}", sized(&item, &size))
        } else if catalog_note.is_empty() {
            format!("catalog {catalog} is in no Price List row and no Item Map row for {}", sized(&item, &size))
        } else {
            format!("{catalog_note}no Item Map row for {}", sized(&item, &size))
        };
        return Outcome::Unpriced(why);
    };
    if row.exclude {
        return Outcome::Excluded(if row.note.trim().is_empty() { "excluded in the Item Map".into() } else { row.note.trim().to_string() });
    }
    let mut parts = Vec::new();
    for k in row.keys.iter().take(MAX_KEYS) {
        let Some(c) = pricebook::choose(book, k) else {
            return Outcome::Unpriced(format!("the Item Map row for {} names key {k}, which has no Price List row", sized(&item, &size)));
        };
        if !pricebook::same_unit(&c.quote.unit, &unit) {
            return Outcome::Unpriced(format!("key {k} is priced per {}, the line is in {unit}", c.quote.unit));
        }
        parts.push(c);
    }
    if row.keys.len() > MAX_KEYS {
        return Outcome::Unpriced(format!("the Item Map row for {} adds more than {MAX_KEYS} keys", sized(&item, &size)));
    }
    Outcome::Priced { by: "item_map", parts }
}

/// The line's quantity bought: its net and the waste its category takes (whole units of counted
/// items, as the BOM does).
fn bought(l: &Value) -> (f64, f64, f64) {
    let qty = l["qty"].as_f64().filter(|v| v.is_finite()).unwrap_or(0.0);
    let pct = l["waste_pct"].as_f64().filter(|w| w.is_finite() && (0.0..=200.0).contains(w)).unwrap_or(0.0);
    let waste = qty * pct / 100.0;
    let waste = if pricebook::same_unit(l["unit"].as_str().unwrap_or_default(), "each") { waste.round() } else { waste };
    (qty, pct, qty + waste)
}

/// Columns of the priced takeoff sheet (and the CSV).
const COLUMNS: [&str; 23] = [
    "Trade",
    "Category",
    "Item",
    "Size",
    "Type mark",
    "Manufacturer",
    "Catalog",
    "Dimming",
    "Product source",
    "Level",
    "Unit",
    "Qty",
    "Status",
    "Matched by",
    "Price key 1",
    "Price key 2",
    "Price key 3",
    "Unit $",
    "Extended $",
    "Price date",
    "Source type",
    "Stale",
    "Note",
];
/// Columns (0-based) the formulas use: qty, the keys, unit $ and extended $.
const QTY_COL: usize = 11;
const KEY_COL: usize = 14;
const UNIT_COL: usize = 17;
const EXT_COL: usize = 18;

fn col(c: usize) -> String {
    let mut s = Vec::new();
    let mut c = c + 1;
    while c > 0 {
        s.push(b'A' + ((c - 1) % 26) as u8);
        c = (c - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// The unit $ formula of row `r` (1-based): each key's price looked up in the Price List, added;
/// no key gives no price (blank, never zero).
fn unit_formula(r: usize) -> String {
    let look = |c: usize| format!("INDEX('{PRICE_SHEET}'!$E:$E,MATCH(${}{r},'{PRICE_SHEET}'!$A:$A,0))", col(c));
    let first = col(KEY_COL);
    let rest: String = (1..MAX_KEYS).map(|i| format!("+IF(${}{r}=\"\",0,{})", col(KEY_COL + i), look(KEY_COL + i))).collect();
    format!("IF(${first}{r}=\"\",\"\",{}{rest})", look(KEY_COL))
}

pub(super) fn run(p: &Value, lines: &[Value]) -> Result<Value> {
    let path = str_param(p, "price_book").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(CMD, "`price_book` is an .xlsx path"))?;
    for other in ["cost_table", "csv", "rows", "revision", "compare_to"] {
        if p.get(other).is_some_and(|v| !v.is_null()) {
            return Err(bad(
                CMD,
                format!("`{other}` doesn't go with `price_book` (the price book prices material only; a cost table prices labor too)"),
            ));
        }
    }
    let stale_days = match p.get("stale_days") {
        None | Some(Value::Null) => STALE_DAYS,
        Some(v) => v.as_i64().filter(|n| (1..=36_500).contains(n)).ok_or_else(|| bad(CMD, "`stale_days` is a whole number of days, 1 or more"))?,
    };
    let tax_pct = match p.get("tax_pct") {
        None | Some(Value::Null) => 0.0,
        Some(v) => v.as_f64().filter(|t| t.is_finite() && (0.0..=100.0).contains(t)).ok_or_else(|| bad(CMD, "`tax_pct` is a percent, 0 to 100"))?,
    };
    let today = match str_param(p, "today").map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => {
            crate::cmd::estimate::parse_date(t).map(|_| t.chars().take(10).collect::<String>()).ok_or_else(|| bad(CMD, "`today` is YYYY-MM-DD"))?
        }
        None => {
            let now = crate::cmd::coordination::iso_date();
            now.get(..10).unwrap_or(&now).to_string()
        }
    };
    let bytes = crate::readfile::read(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    if !bytes.starts_with(b"PK") {
        return Err(bad(CMD, format!("{path}: a price book is an .xlsx workbook (sheets \"{PRICE_SHEET}\" and \"Item Map\")")));
    }
    let book = pricebook::workbook_book(&bytes, &today, stale_days).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    if book.quotes.is_empty() {
        return Err(bad(CMD, format!("{path}: no \"{PRICE_SHEET}\" rows with a key and a unit price")));
    }
    let mut by_catalog: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for q in book.quotes.values().flatten() {
        let c = catalog_key(&q.catalog);
        if !c.is_empty() {
            by_catalog.entry(c).or_default().insert(q.key.clone());
        }
    }

    let mut priced = Vec::new();
    let mut unpriced = Vec::new();
    let mut excluded = Vec::new();
    // Every line with its outcome, in order, for the export.
    let mut rows: Vec<(Value, Option<Vec<Chosen>>)> = Vec::new();
    let mut groups: BTreeMap<(String, String), (usize, f64)> = BTreeMap::new();
    let mut by_category: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();
    let mut stale_lines = 0usize;
    for l in lines.iter().take(MAX_LINES) {
        let (qty, pct, total) = bought(l);
        if qty <= 0.0 {
            continue;
        }
        let mut line = json!({
            "key": l["key"], "level": l["level"], "trade": l["trade"], "category": l["category"], "item": l["item"], "size": l["size"],
            "unit": l["unit"], "qty": r3(qty), "waste_pct": r3(pct), "qty_total": r3(total), "elements": l["element_count"],
        });
        for k in super::takeoff::PRODUCT_FIELDS {
            if let Some(v) = l[k].as_str().filter(|v| !v.is_empty()) {
                line[k] = json!(v);
            }
        }
        let category = l["category"].as_str().unwrap_or_default().to_string();
        let tally = by_category.entry(category.clone()).or_default();
        *tally.entry("lines").or_default() += 1;
        match price(&book, &by_catalog, l) {
            Outcome::Priced { by, parts } => {
                let unit_price: f64 = parts.iter().map(|c| c.quote.unit_price).sum();
                let extended = unit_price * total;
                // The oldest price it adds says how current it is.
                let date = parts.iter().map(|c| c.quote.date.clone()).min().unwrap_or_default();
                let age = pricebook::days_between(&date, &today);
                let stale = age.is_none_or(|a| a > stale_days);
                let types: BTreeSet<&str> = parts.iter().map(|c| c.quote.source_type.as_str()).filter(|t| !t.is_empty()).collect();
                let vendors: BTreeSet<&str> = parts.iter().map(|c| c.quote.vendor.as_str()).filter(|t| !t.is_empty()).collect();
                line["status"] = json!("priced");
                line["matched_by"] = json!(by);
                line["price_keys"] = json!(parts.iter().map(|c| c.quote.key.clone()).collect::<Vec<_>>());
                line["unit_price"] = json!((unit_price * 1e4).round() / 1e4);
                line["extended"] = json!((extended * 100.0).round() / 100.0);
                line["price_date"] = json!(date);
                line["age_days"] = json!(age);
                line["source_type"] = json!(types.into_iter().collect::<Vec<_>>().join(" + "));
                line["vendor"] = json!(vendors.into_iter().collect::<Vec<_>>().join(" + "));
                line["price"] = json!(pricebook::source_text(&parts));
                if stale {
                    stale_lines += 1;
                    line["flag"] = json!("STALE");
                    line["stale_why"] = json!(match age {
                        Some(a) => format!("priced {a} days before {today} (older than {stale_days})"),
                        None => "the price has no date".to_string(),
                    });
                }
                *tally.entry("priced").or_default() += 1;
                *tally.entry(if by == "catalog" { "by_catalog" } else { "by_item_map" }).or_default() += 1;
                let g = groups.entry((l["trade"].as_str().unwrap_or_default().to_string(), category)).or_default();
                g.0 += 1;
                g.1 += extended;
                rows.push((line.clone(), Some(parts)));
                priced.push(line);
            }
            Outcome::Excluded(why) => {
                line["status"] = json!("excluded");
                line["reason"] = json!(why);
                *tally.entry("excluded").or_default() += 1;
                rows.push((line.clone(), None));
                excluded.push(line);
            }
            Outcome::Unpriced(why) => {
                line["status"] = json!("unpriced");
                line["reason"] = json!(why);
                *tally.entry("unpriced").or_default() += 1;
                rows.push((line.clone(), None));
                unpriced.push(line);
            }
        }
    }
    let subtotal: f64 = groups.values().map(|g| g.1).sum();
    let tax = subtotal * tax_pct / 100.0;
    let cents = |v: f64| (v * 100.0).round() / 100.0;
    let by_group: Vec<Value> = groups.iter().map(|((t, c), (n, x))| json!({"trade": t, "category": c, "lines": n, "extended": cents(*x)})).collect();
    let mut warnings = Vec::new();
    if !unpriced.is_empty() {
        warnings.push(json!({"kind": "unpriced_lines", "message": format!("{} lines have no price: they are not in the total", unpriced.len()),
            "fix": "add their catalog numbers to the Price List, or Item Map rows for their items"}));
    }
    if stale_lines > 0 {
        warnings.push(json!({"kind": "stale_prices", "message": format!("{stale_lines} lines are priced from a quote older than {stale_days} days"),
            "fix": "update those Price List rows' prices and dates"}));
    }
    let by_catalog_n = priced.iter().filter(|l| l["matched_by"] == "catalog").count();
    let mut out = json!({
        "price_book": path, "today": today, "stale_days": stale_days,
        "lines": priced, "unpriced": unpriced, "excluded": excluded,
        "counts": {"lines": rows.len(), "priced": rows.len() - excluded.len() - unpriced.len(), "by_catalog": by_catalog_n,
                   "by_item_map": rows.len() - excluded.len() - unpriced.len() - by_catalog_n, "unpriced": unpriced.len(), "excluded": excluded.len(), "stale": stale_lines},
        "by_category": by_category,
        "totals": {"by_group": by_group, "subtotal": cents(subtotal), "tax": {"rate_pct": tax_pct, "amount": cents(tax)}, "total": cents(subtotal + tax)},
        "material_total": cents(subtotal), "total": cents(subtotal + tax),
        "warnings": warnings,
    });
    let sheets = workbook(&book, &rows, &groups, tax_pct);
    out["csv"] = json!(csv(sheets.first().map(|s| s.1.as_slice()).unwrap_or_default()));
    if let Some(file) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
        let bytes = if file.to_ascii_lowercase().ends_with(".xlsx") {
            crate::cmd::table::xlsx::write_cells(&sheets).map_err(|e| bad(CMD, e))?
        } else {
            out["csv"].as_str().unwrap_or_default().as_bytes().to_vec()
        };
        #[cfg(not(target_arch = "wasm32"))]
        crate::writefile::write(file, &bytes).map_err(|e| crate::EngineError::Other(format!("{file}: {e}")))?;
        #[cfg(target_arch = "wasm32")]
        let _ = bytes;
        out["path"] = json!(file);
    }
    Ok(out)
}

/// A sheet's cells as CSV (values as they are now).
fn csv(rows: &[Vec<Cell>]) -> String {
    let cell = |c: &Cell| {
        let t = match c {
            Cell::Empty => String::new(),
            Cell::Text(t) | Cell::FormulaText(_, t) => t.clone(),
            Cell::Number(n) | Cell::Formula(_, n) => format!("{n}"),
        };
        if t.contains([',', '"', '\n']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t }
    };
    rows.iter().map(|r| r.iter().map(cell).collect::<Vec<_>>().join(",") + "\n").collect()
}

/// The priced takeoff workbook: the lines with live price formulas, the totals by group with the
/// tax line, the unpriced and excluded lines, and the price book's own two sheets.
fn workbook(
    book: &Book,
    rows: &[(Value, Option<Vec<Chosen>>)],
    groups: &BTreeMap<(String, String), (usize, f64)>,
    tax_pct: f64,
) -> Vec<(String, Vec<Vec<Cell>>)> {
    let t = |v: &Value| Cell::text(v.as_str().map_or_else(|| if v.is_null() { String::new() } else { v.to_string() }, str::to_string));
    let mut lines: Vec<Vec<Cell>> = vec![COLUMNS.iter().map(|c| Cell::text(*c)).collect()];
    for (i, (l, parts)) in rows.iter().enumerate() {
        let r = i + 2;
        let keys: Vec<String> = parts.as_ref().map(|p| p.iter().map(|c| c.quote.key.clone()).collect()).unwrap_or_default();
        let unit_price = l["unit_price"].as_f64();
        let status = l["status"].as_str().unwrap_or_default().to_ascii_uppercase();
        let mut row = vec![
            t(&l["trade"]),
            t(&l["category"]),
            t(&l["item"]),
            t(&l["size"]),
            t(&l["type_mark"]),
            t(&l["manufacturer"]),
            t(&l["catalog"]),
            t(&l["dimming"]),
            t(&l["product_source"]),
            t(&l["level"]),
            t(&l["unit"]),
            Cell::Number(l["qty_total"].as_f64().unwrap_or(0.0)),
            Cell::text(status),
            t(&l["matched_by"]),
        ];
        row.extend((0..MAX_KEYS).map(|k| keys.get(k).map_or(Cell::Empty, |x| Cell::text(x.clone()))));
        let q = col(QTY_COL);
        let u = col(UNIT_COL);
        row.push(match unit_price {
            Some(v) => Cell::Formula(unit_formula(r), v),
            None => Cell::FormulaText(unit_formula(r), String::new()),
        });
        row.push(match l["extended"].as_f64() {
            Some(v) => Cell::Formula(format!("IF(${u}{r}=\"\",\"\",${q}{r}*${u}{r})"), v),
            None => Cell::FormulaText(format!("IF(${u}{r}=\"\",\"\",${q}{r}*${u}{r})"), String::new()),
        });
        row.push(t(&l["price_date"]));
        row.push(t(&l["source_type"]));
        row.push(t(&l["flag"]));
        row.push(Cell::text(l["reason"].as_str().or_else(|| l["stale_why"].as_str()).unwrap_or_default()));
        lines.push(row);
    }
    // Totals: each group's sum of the takeoff's extended $, the subtotal, the tax at its rate, the total.
    let (ext, trade, cat) = (col(EXT_COL), col(0), col(1));
    let mut totals: Vec<Vec<Cell>> = vec![["Trade", "Category", "Priced lines", "Extended $"].iter().map(|c| Cell::text(*c)).collect()];
    for (i, ((tr, ca), (n, x))) in groups.iter().enumerate() {
        let r = i + 2;
        totals.push(vec![
            Cell::text(tr.clone()),
            Cell::text(ca.clone()),
            Cell::Number(*n as f64),
            Cell::Formula(
                format!("SUMIFS('{TAKEOFF_SHEET}'!${ext}:${ext},'{TAKEOFF_SHEET}'!${trade}:${trade},$A{r},'{TAKEOFF_SHEET}'!${cat}:${cat},$B{r})"),
                *x,
            ),
        ]);
    }
    let last = groups.len() + 1;
    let subtotal: f64 = groups.values().map(|g| g.1).sum();
    let (sub_r, rate_r, tax_r) = (last + 1, last + 2, last + 3);
    let tax = subtotal * tax_pct / 100.0;
    totals.push(vec![Cell::text("Subtotal"), Cell::Empty, Cell::Empty, Cell::Formula(format!("SUM(D2:D{last})"), subtotal)]);
    totals.push(vec![Cell::text("Tax rate %"), Cell::Empty, Cell::Empty, Cell::Number(tax_pct)]);
    totals.push(vec![Cell::text("Tax"), Cell::Empty, Cell::Empty, Cell::Formula(format!("D{sub_r}*D{rate_r}/100"), tax)]);
    totals.push(vec![Cell::text("Total"), Cell::Empty, Cell::Empty, Cell::Formula(format!("D{sub_r}+D{tax_r}"), subtotal + tax)]);
    // What the total leaves out, and why.
    let mut left: Vec<Vec<Cell>> =
        vec![["Status", "Trade", "Category", "Item", "Size", "Type mark", "Catalog", "Unit", "Qty", "Why"].iter().map(|c| Cell::text(*c)).collect()];
    for (l, _) in rows.iter().filter(|(l, _)| l["status"] != "priced") {
        left.push(vec![
            Cell::text(l["status"].as_str().unwrap_or_default().to_ascii_uppercase()),
            t(&l["trade"]),
            t(&l["category"]),
            t(&l["item"]),
            t(&l["size"]),
            t(&l["type_mark"]),
            t(&l["catalog"]),
            t(&l["unit"]),
            Cell::Number(l["qty_total"].as_f64().unwrap_or(0.0)),
            t(&l["reason"]),
        ]);
    }
    // The price book's quotes, each key's chosen quote first (MATCH finds it).
    let mut prices: Vec<Vec<Cell>> = vec![
        ["Key", "Description", "Catalog", "Unit", "Unit price $", "Source type", "Vendor", "URL", "Price date", "Location", "Note"]
            .iter()
            .map(|c| Cell::text(*c))
            .collect(),
    ];
    for (key, quotes) in &book.quotes {
        let chosen = pricebook::choose(book, key).map(|c| c.quote);
        let mut ordered: Vec<&pricebook::Quote> = quotes.iter().collect();
        ordered.sort_by_key(|q| chosen.as_ref() != Some(*q));
        for q in ordered {
            prices.push(vec![
                Cell::text(q.key.clone()),
                Cell::text(q.description.clone()),
                Cell::text(q.catalog.clone()),
                Cell::text(q.unit.clone()),
                Cell::Number(q.unit_price),
                Cell::text(q.source_type.clone()),
                Cell::text(q.vendor.clone()),
                Cell::text(q.url.clone()),
                Cell::text(q.date.clone()),
                Cell::text(q.location.clone()),
                Cell::text(q.note.clone()),
            ]);
        }
    }
    let mut map: Vec<Vec<Cell>> =
        vec![["CADaddy item", "Size", "Action", "Price key 1", "Price key 2", "Price key 3", "Note"].iter().map(|c| Cell::text(*c)).collect()];
    for m in &book.map {
        let mut row = vec![Cell::text(m.item.clone()), Cell::text(m.size.clone()), Cell::text(if m.exclude { "exclude" } else { "price" })];
        row.extend((0..MAX_KEYS).map(|k| m.keys.get(k).map_or(Cell::Empty, |x| Cell::text(x.clone()))));
        row.push(Cell::text(m.note.clone()));
        map.push(row);
    }
    vec![
        (TAKEOFF_SHEET.to_string(), lines),
        ("Totals".to_string(), totals),
        ("Unpriced".to_string(), left),
        (PRICE_SHEET.to_string(), prices),
        ("Item Map".to_string(), map),
    ]
}

#[cfg(test)]
mod tests {
    use crate::Session;
    use crate::cmd::table::xlsx;
    use cadaddy_doc::props::{Prop, Props, Source, SourceKind};
    use serde_json::{Value, json};

    /// A built device of spec tag `devid` with properties.
    fn built(s: &mut Session, devid: &str, props: &[(&str, &str, SourceKind)]) {
        let mut tags = crate::cmd::wallgen::tag(cadaddy_walk3d::Part::Device, "1", cadaddy_doc::Handle(0));
        tags.push(cadaddy_doc::RawTag { code: 1000, value: format!("devid:{devid}") });
        let mut store = Props::default();
        for (k, v, kind) in props {
            store.set(k, Prop::new(*v, Source::from_doc(*kind, "Devices p.2")));
        }
        let xdata = cadaddy_doc::props::write(&tags, &store);
        let mut m = crate::cmd::wallgen::empty_mesh();
        crate::cmd::wallgen::add_box(&mut m, cadaddy_geom::Vec2::new(0.0, 0.0), cadaddy_geom::Vec2::new(4.0, 0.0), 2.0, 46.0, 50.0);
        let common = cadaddy_doc::Common { layer: "3D-Devices".into(), xdata: xdata.into(), ..Default::default() };
        s.doc_mut().unwrap().add(&cadaddy_doc::Space::Model, common, cadaddy_doc::EntityKind::Mesh(m)).unwrap();
    }

    /// A made-up job: two luminaires of type A (their catalog number from the type catalog), one
    /// of type B (none), two receptacles, a GFCI, and dimmers that dim two ways.
    fn job() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        let tag = |id: &str, kind: &str, fields: Value| json!({"id": id, "kind": kind, "level": "1", "at_ft": [1, 1], "fields": fields});
        let lum = |id: &str, ty: &str| tag(id, "luminaire", json!({"type": {"value": ty, "source": "plan"}}));
        let dev = |id: &str, ty: &str| tag(id, "receptacle", json!({"device_type": {"value": ty, "source": "legend"}}));
        let dim = |id: &str| {
            tag(id, "switch", json!({"device_type": {"value": "switch, dimmer", "source": "legend"}, "type": {"value": "XD", "source": "plan"}}))
        };
        let tags = json!([
            lum("L1", "A"),
            lum("L2", "A"),
            lum("L3", "B"),
            dev("R1", "duplex receptacle"),
            dev("R2", "duplex receptacle"),
            dev("G1", "GFCI receptacle"),
            dim("D1"),
            dim("D2"),
            dim("D3")
        ]);
        s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}], "tags": tags}}))
            .unwrap();
        {
            use cadaddy_doc::bimspec::{BimType, SpecField, keys};
            let mut a = BimType::new("A");
            a.category = "luminaire".into();
            a.set_field(keys::MANUFACTURER, SpecField::new("Maker One").with_source("Package p.4"));
            a.set_field(keys::CATALOG_NUMBER, SpecField::new("AB12-L48").with_source("Package p.4"));
            s.doc_mut().unwrap().bim_types.types.push(a);
        }
        for (id, leg, how) in [("D1", "a", "phase"), ("D2", "b", "phase"), ("D3", "c", "0-10V")] {
            built(
                &mut s,
                id,
                &[
                    ("type_mark", "XD", SourceKind::Print),
                    ("type", "switch, dimmer", SourceKind::Legend),
                    ("switch_leg", leg, SourceKind::Print),
                    ("dimming", how, SourceKind::Submittal),
                ],
            );
        }
        s
    }

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|c| (*c).to_string()).collect()
    }

    /// A made-up price book: type A's catalog under key `fxA` (two vendors, one of them stale and
    /// cheaper), an Item Map row that would price type A otherwise, type B as a fixture and its
    /// trim (one price old), each dimmer protocol, and the GFCI excluded.
    fn book(dir: &std::path::Path) -> String {
        let prices = vec![
            row(&["Key", "Description", "Catalog", "Unit", "Unit Price $", "Source Type", "Vendor", "URL", "Price Date"]),
            row(&["fxA", "2x4 troffer", "AB12 L48", "ea", "100", "QUOTE", "Vendor X", "", "2026-10-01"]),
            row(&["fxA", "2x4 troffer", "AB12 L48", "ea", "80", "WEB", "Vendor Y", "https://example.com/a", "2025-01-01"]),
            row(&["generic", "any fixture", "", "ea", "5", "WEB", "Vendor Y", "", "2026-10-01"]),
            row(&["fxB", "downlight", "DL-6", "ea", "40", "MCC", "Book", "", "2026-01-01"]),
            row(&["trimB", "downlight trim", "DL-6T", "ea", "10", "MCC", "Book", "", "2026-10-15"]),
            row(&["dimP", "phase dimmer", "PD-1", "ea", "30", "WEB", "Vendor X", "", "2026-10-20"]),
            row(&["dimZ", "0-10V dimmer", "ZD-1", "ea", "55", "WEB", "Vendor X", "", "2026-10-20"]),
        ];
        let map = vec![
            row(&["CADaddy Item", "Size", "Action", "Price Key 1", "Price Key 2", "Note"]),
            row(&["Luminaire type A", "", "price", "generic", "", ""]),
            row(&["Luminaire type B", "", "price", "fxB", "trimB", "fixture + trim"]),
            row(&["switch, dimmer, phase", "", "price", "dimP", "", ""]),
            row(&["switch, dimmer, 0-10V", "", "price", "dimZ", "", ""]),
            row(&["GFCI receptacle", "", "exclude", "", "", "furnished by the owner"]),
        ];
        let bytes = xlsx::write_sheets(&[("Price List".into(), prices), ("Item Map".into(), map)]).unwrap();
        let path = dir.join("book.xlsx");
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn line<'a>(r: &'a Value, item: &str) -> &'a Value {
        r["lines"].as_array().unwrap().iter().find(|l| l["item"] == json!(item)).unwrap_or(&Value::Null)
    }

    #[test]
    fn a_price_book_prices_by_catalog_then_item_map_and_lists_what_it_cant() {
        let dir = std::env::temp_dir().join(format!("cadaddy-priced-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = book(&dir);
        let mut s = job();
        let out = dir.join("priced.xlsx");
        let r =
            s.execute("bim.estimate", &json!({"price_book": path, "today": "2026-11-04", "tax_pct": 8.25, "file": out.to_string_lossy()})).unwrap();
        // The catalog number beats the Item Map row; the key's lowest CURRENT quote.
        let a = line(&r, "Luminaire type A");
        assert_eq!(
            (a["matched_by"].clone(), a["price_keys"].clone(), a["unit_price"].clone(), a["qty_total"].clone(), a["extended"].clone()),
            (json!("catalog"), json!(["fxA"]), json!(100.0), json!(2.0), json!(200.0)),
            "{r}"
        );
        assert_eq!(
            (a["catalog"].clone(), a["price_date"].clone(), a["source_type"].clone(), a.get("flag")),
            (json!("AB12-L48"), json!("2026-10-01"), json!("QUOTE"), None)
        );
        // Two keys added; the older of them makes it stale.
        let b = line(&r, "Luminaire type B");
        assert_eq!(
            (b["matched_by"].clone(), b["price_keys"].clone(), b["unit_price"].clone()),
            (json!("item_map"), json!(["fxB", "trimB"]), json!(50.0))
        );
        assert_eq!((b["flag"].clone(), b["price_date"].clone(), b["source_type"].clone()), (json!("STALE"), json!("2026-01-01"), json!("MCC")));
        // Dimmers bought by protocol, each its own price.
        assert_eq!(
            (line(&r, "switch, dimmer, phase")["qty"].clone(), line(&r, "switch, dimmer, phase")["unit_price"].clone()),
            (json!(2.0), json!(30.0))
        );
        assert_eq!(
            (line(&r, "switch, dimmer, 0-10V")["dimming"].clone(), line(&r, "switch, dimmer, 0-10V")["unit_price"].clone()),
            (json!("0-10V"), json!(55.0))
        );
        // Unmapped: listed with why, not priced at zero; excluded with its reason.
        let un = r["unpriced"].as_array().unwrap();
        assert!(un.iter().any(|l| l["item"] == json!("duplex receptacle") && l["reason"].as_str().unwrap().contains("no Item Map row")), "{un:?}");
        assert!(line(&r, "duplex receptacle").is_null());
        let ex = r["excluded"].as_array().unwrap();
        assert_eq!((ex[0]["item"].clone(), ex[0]["reason"].clone()), (json!("GFCI receptacle"), json!("furnished by the owner")));
        // Totals by group, the tax its own line.
        let sub: f64 = 200.0 + 50.0 + 60.0 + 55.0;
        assert_eq!(r["totals"]["subtotal"], json!(sub));
        assert_eq!(r["totals"]["tax"], json!({"rate_pct": 8.25, "amount": (sub * 8.25).round() / 100.0}));
        let groups = r["totals"]["by_group"].as_array().unwrap();
        assert!(groups.iter().any(|g| g["category"] == json!("fixtures") && g["extended"] == json!(250.0)), "{groups:?}");
        assert_eq!(
            (r["counts"]["by_catalog"].clone(), r["counts"]["by_item_map"].clone(), r["counts"]["unpriced"].clone(), r["counts"]["stale"].clone()),
            (json!(1), json!(3), json!(4), json!(1))
        );
        // The receptacles' box, ring and plate (rule lines of the BOM) no row prices either.
        let mut un_items: Vec<&str> = un.iter().filter_map(|l| l["item"].as_str()).collect();
        un_items.sort_unstable();
        assert_eq!(un_items, ["Box, 4\" square", "Device plate", "Plaster ring", "duplex receptacle"]);
        // The export: unit $ an INDEX/MATCH of the line's keys into the Price List beside it,
        // each key's chosen quote its first row.
        let wb = xlsx::open(&std::fs::read(&out).unwrap()).unwrap();
        let cells = wb.cells(Some("Priced Takeoff"), None).unwrap();
        let header = &cells[0];
        let at = |name: &str| header.iter().position(|h| h == name).unwrap();
        let a_row = cells.iter().position(|c| c[at("Item")] == "Luminaire type A").unwrap();
        assert_eq!(
            (cells[a_row][at("Unit $")].as_str(), cells[a_row][at("Extended $")].as_str(), cells[a_row][at("Price key 1")].as_str()),
            ("100", "200", "fxA")
        );
        let formulas = wb.formulas(Some("Priced Takeoff")).unwrap();
        let unit_ref = format!("R{}", a_row + 1);
        let f = &formulas.iter().find(|(c, _)| *c == unit_ref).unwrap().1;
        assert!(f.contains(&format!("MATCH($O{},'Price List'!$A:$A,0)", a_row + 1)) && f.contains("INDEX('Price List'!$E:$E"), "{f}");
        let prices = wb.cells(Some("Price List"), None).unwrap();
        let first_a = prices.iter().find(|p| p[0] == "fxA").unwrap();
        assert_eq!((first_a[4].as_str(), first_a[6].as_str()), ("100", "Vendor X"), "the chosen quote first");
        let un_row = cells.iter().find(|c| c[at("Item")] == "duplex receptacle").unwrap();
        assert_eq!((un_row[at("Status")].as_str(), un_row[at("Unit $")].as_str()), ("UNPRICED", ""), "blank, never zero");
        let totals = wb.cells(Some("Totals"), None).unwrap();
        assert!(totals.iter().any(|t| t[0] == "Tax rate %" && t[3] == "8.25"), "{totals:?}");
        assert!(wb.formulas(Some("Totals")).unwrap().iter().any(|(_, f)| f.starts_with("SUMIFS('Priced Takeoff'!$S:$S")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_price_book_is_refused_with_a_cost_table_or_without_prices() {
        let dir = std::env::temp_dir().join(format!("cadaddy-priced-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = book(&dir);
        let mut s = job();
        assert!(s.execute("bim.estimate", &json!({"price_book": path, "cost_table": "x.csv"})).is_err());
        assert!(s.execute("bim.estimate", &json!({"price_book": path, "stale_days": -3})).is_err());
        assert!(s.execute("bim.estimate", &json!({"price_book": path, "tax_pct": "lots"})).is_err());
        let empty = dir.join("empty.xlsx");
        std::fs::write(&empty, xlsx::write_sheets(&[("Price List".into(), vec![row(&["Key", "Unit Price $"])])]).unwrap()).unwrap();
        let e = s.execute("bim.estimate", &json!({"price_book": empty.to_string_lossy()})).unwrap_err().to_string();
        assert!(e.contains("no \"Price List\" rows"), "{e}");
        std::fs::write(dir.join("notes.txt"), "not a workbook").unwrap();
        assert!(s.execute("bim.estimate", &json!({"price_book": dir.join("notes.txt").to_string_lossy()})).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
