//! The material price book: recent, dated quotes CADaddy holds and keeps up to date, outside
//! the program (a price changes without a new CADaddy).
//!
//! Each price key (`dupTR`: a 20 A tamper-resistant duplex) holds any number of **quotes**: a
//! unit price from one vendor at one location (a branch, a store, a web page), with its pack,
//! catalog number, URL, date and source type (`WEB` a public page, `QUOTE` a supplier's quote,
//! `BOOK` a licensed price book, `BID` a bid's unit price). The book picks one per key by its rule
//! (`settings.json` `pick`: `lowest` current quote (default), `latest`, `median`, or a `vendor`'s),
//! a quote older than `stale_days` counting only when no current one is left.
//!
//! Where it lives:
//! - **shipped** (`assets/costbook/pricebook.csv`): public web prices for common material, so a new
//!   install prices something;
//! - **the local price database**: a folder of plain CSV files (`quotes.csv`, `map.csv`,
//!   `history.csv`, `settings.json`) in the settings folder (`pricebook/`), or `CADADDY_PRICEBOOK`
//!   (one shared folder for every machine), or a `folder` given. Editable in a spreadsheet; never
//!   shipped or committed (a licensed book's prices, a job's). A quote here from the same vendor and
//!   location as a shipped one replaces it unless older.
//!
//! The map (`pricebook_map.csv`, and the local `map.csv`) ties a bill-of-materials item and size to
//! one or more keys (a fixture and its trim add), or excludes it with a reason. A replaced or removed
//! quote goes to `history.csv`: a price change is never lost.
//!
//! `estimate` prices a line from the price book after the drawing's own prices and before the
//! starter cost book, the line saying which quote it took and the spread of the others.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const SHIPPED_CSV: &str = include_str!("../../../../assets/costbook/pricebook.csv");
const SHIPPED_MAP: &str = include_str!("../../../../assets/costbook/pricebook_map.csv");
/// Rows read from one file, at most (hostile files).
const MAX_ROWS: usize = 50_000;
/// A quote older than this many days is stale (the default).
const STALE_DAYS: i64 = 180;
const COLUMNS: [&str; 13] =
    ["key", "description", "catalog", "unit", "pack", "pack_price", "unit_price", "source_type", "vendor", "location", "url", "date", "note"];
const MAP_COLUMNS: [&str; 5] = ["item", "size", "action", "keys", "note"];
const PICKS: [&str; 4] = ["lowest", "latest", "median", "vendor"];

/// One quote: a unit price from one vendor at one location on a date.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Quote {
    pub key: String,
    pub description: String,
    pub catalog: String,
    pub unit: String,
    pub pack: String,
    pub pack_price: Option<f64>,
    pub unit_price: f64,
    pub source_type: String,
    pub vendor: String,
    pub location: String,
    pub url: String,
    pub date: String,
    pub note: String,
    /// `shipped` or `local`.
    pub origin: &'static str,
}

impl Quote {
    /// The same vendor and location (a newer quote of theirs replaces this one).
    fn same_source(&self, o: &Quote) -> bool {
        self.key.eq_ignore_ascii_case(&o.key) && self.vendor.eq_ignore_ascii_case(&o.vendor) && self.location.eq_ignore_ascii_case(&o.location)
    }

    fn json(&self, today: &str, stale_days: i64) -> Value {
        let age = days_between(&self.date, today);
        json!({"vendor": self.vendor, "location": self.location, "unit_price": self.unit_price, "unit": self.unit, "pack": self.pack,
               "pack_price": self.pack_price, "catalog": self.catalog, "source_type": self.source_type, "url": self.url, "date": self.date,
               "note": self.note, "age_days": age, "stale": age.is_none_or(|a| a > stale_days), "origin": self.origin})
    }
}

/// A bill-of-materials item and size, priced by keys or excluded.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MapRow {
    pub item: String,
    pub size: String,
    pub exclude: bool,
    pub keys: Vec<String>,
    pub note: String,
    pub origin: &'static str,
}

/// How the book picks a key's price among its quotes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rule {
    pub pick: String,
    pub vendor: String,
    pub stale_days: i64,
}

impl Default for Rule {
    fn default() -> Self {
        Rule { pick: "lowest".into(), vendor: String::new(), stale_days: STALE_DAYS }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Book {
    /// Each key's quotes.
    pub quotes: BTreeMap<String, Vec<Quote>>,
    pub map: Vec<MapRow>,
    pub rule: Rule,
    pub folder: Option<PathBuf>,
    /// The date the book is read on (YYYY-MM-DD), for which quotes are current.
    pub today: String,
}

/// The quote a key's price is, and the others beside it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Chosen {
    pub quote: Quote,
    pub quotes: usize,
    pub current: usize,
    pub low: f64,
    pub high: f64,
    pub why: String,
}

/// The local price database's folder: `folder` given, else `CADADDY_PRICEBOOK`, else
/// `<settings>/pricebook`.
fn local_folder(folder: Option<&str>) -> Option<PathBuf> {
    if let Some(f) = folder.map(str::trim).filter(|f| !f.is_empty()) {
        return Some(PathBuf::from(f));
    }
    if let Some(f) = std::env::var_os("CADADDY_PRICEBOOK").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(f));
    }
    crate::control_auth::config_dir().map(|d| d.join("pricebook"))
}

fn rows(text: &str) -> Vec<BTreeMap<String, String>> {
    super::estimate::csv_rows(text).into_iter().take(MAX_ROWS).collect()
}

fn num(v: Option<&String>) -> Option<f64> {
    v.and_then(|t| t.trim().trim_start_matches('$').replace(',', "").parse::<f64>().ok()).filter(|x| x.is_finite() && *x >= 0.0)
}

/// A pack's count (`10 ft`, `25 pk`, `500 ft`, `each`).
fn pack_count(pack: &str) -> Option<f64> {
    let p = pack.trim().to_ascii_lowercase();
    if p == "each" || p == "ea" {
        return Some(1.0);
    }
    let n: String = p.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let rest = p.get(n.len()..).unwrap_or_default().trim();
    n.parse::<f64>().ok().filter(|v| *v > 0.0 && v.is_finite() && !rest.starts_with('+'))
}

fn quote_of(r: &BTreeMap<String, String>, origin: &'static str) -> Option<Quote> {
    let g = |k: &str| r.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
    let key = g("key");
    let pack = g("pack");
    let pack_price = num(r.get("pack_price"));
    // The pack's price over its count when both are given (exact), else the unit price.
    let unit_price = pack_price.zip(pack_count(&pack)).map(|(pp, n)| pp / n).or_else(|| num(r.get("unit_price")))?;
    let unit_price = (unit_price * 1e6).round() / 1e6;
    let mut date = g("date");
    date.truncate(10);
    // A web price with no location is the vendor's online price.
    let source_type = g("source_type");
    let location = match g("location") {
        l if l.is_empty() && source_type.eq_ignore_ascii_case("WEB") => "online".to_string(),
        l => l,
    };
    (!key.is_empty()).then(|| Quote {
        key,
        description: g("description"),
        catalog: g("catalog"),
        unit: g("unit"),
        pack,
        pack_price,
        unit_price,
        source_type,
        vendor: g("vendor"),
        location,
        url: g("url"),
        date,
        note: g("note"),
        origin,
    })
}

fn map_of(r: &BTreeMap<String, String>, origin: &'static str) -> Option<MapRow> {
    let g = |k: &str| r.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
    let item = g("item");
    if item.is_empty() {
        return None;
    }
    let exclude = g("action").eq_ignore_ascii_case("exclude");
    let keys: Vec<String> = g("keys").split(['+', ';']).map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect();
    (exclude || !keys.is_empty()).then(|| MapRow { item, size: g("size"), exclude, keys, note: g("note"), origin })
}

/// What adding a quote did.
#[derive(Clone, Debug, PartialEq)]
enum Added {
    New,
    /// It replaced the same vendor and location's earlier quote.
    Replaced(Box<Quote>),
    /// Older than the same vendor and location's quote held: kept out.
    Older,
}

/// Add a quote: one from the same vendor and location is replaced unless this one is older.
fn add(quotes: &mut Vec<Quote>, q: Quote) -> Added {
    match quotes.iter().position(|o| o.same_source(&q)) {
        Some(i) => {
            let Some(old) = quotes.get(i).cloned() else { return Added::New };
            if !old.date.is_empty() && q.date < old.date {
                return Added::Older;
            }
            if let Some(slot) = quotes.get_mut(i) {
                *slot = q;
            }
            Added::Replaced(Box::new(old))
        }
        None => {
            quotes.push(q);
            Added::New
        }
    }
}

fn local_quotes(dir: &Path) -> Vec<Quote> {
    // `prices.csv`: the first version's file (a quote each row), read until rewritten.
    let mut out = Vec::new();
    for f in ["prices.csv", "quotes.csv"] {
        if let Ok(t) = std::fs::read_to_string(dir.join(f)) {
            for q in rows(&t).iter().filter_map(|r| quote_of(r, "local")) {
                let _ = add(&mut out, q);
            }
        }
    }
    out
}

fn local_map(dir: &Path) -> Vec<MapRow> {
    std::fs::read_to_string(dir.join("map.csv")).map(|t| rows(&t).iter().filter_map(|r| map_of(r, "local")).collect()).unwrap_or_default()
}

fn local_rule(dir: &Path) -> Rule {
    let v: Value = std::fs::read_to_string(dir.join("settings.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    let d = Rule::default();
    Rule {
        pick: v["pick"].as_str().filter(|p| PICKS.contains(p)).map_or(d.pick, str::to_string),
        vendor: v["vendor"].as_str().unwrap_or_default().to_string(),
        stale_days: v["stale_days"].as_i64().filter(|n| *n > 0).unwrap_or(d.stale_days),
    }
}

/// The price book: the shipped quotes with the local database's beside and over them.
pub(crate) fn load(folder: Option<&str>) -> Book {
    let now = super::coordination::iso_date();
    let mut b = Book { folder: local_folder(folder), today: now.get(..10).unwrap_or(&now).to_string(), ..Book::default() };
    for q in rows(SHIPPED_CSV).iter().filter_map(|r| quote_of(r, "shipped")) {
        let _ = add(b.quotes.entry(q.key.clone()).or_default(), q);
    }
    b.map = rows(SHIPPED_MAP).iter().filter_map(|r| map_of(r, "shipped")).collect();
    if let Some(dir) = b.folder.clone() {
        for q in local_quotes(&dir) {
            let _ = add(b.quotes.entry(q.key.clone()).or_default(), q);
        }
        for m in local_map(&dir) {
            b.map.retain(|x| !(x.item.eq_ignore_ascii_case(&m.item) && x.size.eq_ignore_ascii_case(&m.size)));
            b.map.push(m);
        }
        b.rule = local_rule(&dir);
    }
    b
}

/// A key's price by the book's rule.
pub(crate) fn choose(b: &Book, key: &str) -> Option<Chosen> {
    // `a|b`: either of them, their quotes pooled (a web price and an invoice's for one thing).
    let pooled: Vec<Quote> = key.split('|').filter_map(|k| b.quotes.get(k.trim())).flatten().cloned().collect();
    let all = Some(&pooled).filter(|v| !v.is_empty())?;
    let current: Vec<&Quote> = all.iter().filter(|q| days_between(&q.date, &b.today).is_some_and(|a| a <= b.rule.stale_days)).collect();
    let pool: Vec<&Quote> = if current.is_empty() { all.iter().collect() } else { current.clone() };
    let mut by_price = pool.clone();
    by_price.sort_by(|a, c| a.unit_price.total_cmp(&c.unit_price));
    let lowest = by_price.first().copied();
    let (q, why) = match b.rule.pick.as_str() {
        "latest" => (all.iter().max_by(|a, c| a.date.cmp(&c.date)), "the latest quote".to_string()),
        "median" => (by_price.get(by_price.len() / 2).copied(), "the median quote".to_string()),
        "vendor" => {
            match pool.iter().copied().filter(|q| q.vendor.eq_ignore_ascii_case(&b.rule.vendor)).min_by(|a, c| a.unit_price.total_cmp(&c.unit_price))
            {
                Some(v) => (Some(v), format!("{}'s quote", b.rule.vendor)),
                None => (lowest, format!("the lowest (no quote from {})", b.rule.vendor)),
            }
        }
        _ => (lowest, if current.is_empty() { "the lowest (every quote stale)".to_string() } else { "the lowest current quote".to_string() }),
    };
    let q = q?.clone();
    Some(Chosen {
        quote: q,
        quotes: all.len(),
        current: current.len(),
        low: by_price.first().map_or(0.0, |q| q.unit_price),
        high: by_price.last().map_or(0.0, |q| q.unit_price),
        why,
    })
}

/// What the price book says of a bill-of-materials line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Found {
    /// Its material per unit, the quotes it adds, and the oldest date among them.
    Priced {
        material: f64,
        parts: Vec<Chosen>,
        date: String,
    },
    Excluded(String),
}

fn same_unit(a: &str, b: &str) -> bool {
    let n = |u: &str| match u.trim().to_ascii_lowercase().as_str() {
        "ft" | "lf" | "foot" | "feet" => "ft".to_string(),
        "ea" | "each" | "" => "each".to_string(),
        o => o.to_string(),
    };
    n(a) == n(b)
}

/// The line's price: its map row (item and size; else the item with no size), its keys' chosen
/// quotes added. A key with no quote, or a unit that isn't the line's, prices nothing.
pub(crate) fn lookup(b: &Book, item: &str, size: &str, unit: &str) -> Option<Found> {
    let row = b
        .map
        .iter()
        .rev()
        .find(|m| m.item.eq_ignore_ascii_case(item.trim()) && m.size.eq_ignore_ascii_case(size.trim()))
        .or_else(|| b.map.iter().rev().find(|m| m.item.eq_ignore_ascii_case(item.trim()) && m.size.is_empty()))?;
    if row.exclude {
        return Some(Found::Excluded(if row.note.is_empty() { "excluded in the price book".into() } else { row.note.clone() }));
    }
    let parts: Option<Vec<Chosen>> = row.keys.iter().map(|k| choose(b, k)).collect();
    let parts = parts?;
    if !parts.iter().all(|p| same_unit(&p.quote.unit, unit)) {
        return None;
    }
    let date = parts.iter().map(|p| p.quote.date.clone()).filter(|d| !d.is_empty()).min().unwrap_or_default();
    Some(Found::Priced { material: parts.iter().map(|p| p.quote.unit_price).sum(), parts, date })
}

/// How a price reads on an estimate line: `emt34 $0.946 Elliott Electric (online) 2026-11-02 WEB,
/// the lowest current quote of 3 ($0.903-$1.421)`.
pub(crate) fn source_text(parts: &[Chosen]) -> String {
    parts
        .iter()
        .map(|c| {
            let q = &c.quote;
            let at = if q.location.is_empty() { String::new() } else { format!(" ({})", q.location) };
            let spread = if c.quotes > 1 { format!(" of {} (${:.4}-${:.4})", c.quotes, c.low, c.high) } else { String::new() };
            format!("{} ${:.4} {}{at} {} {}, {}{spread}", q.key, q.unit_price, q.vendor, q.date, q.source_type, c.why)
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn csv_cell(v: &str) -> String {
    if v.contains([',', '"', '\n']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() }
}

fn quote_line(q: &Quote) -> String {
    [
        q.key.clone(),
        q.description.clone(),
        q.catalog.clone(),
        q.unit.clone(),
        q.pack.clone(),
        q.pack_price.map(|v| format!("{v}")).unwrap_or_default(),
        format!("{}", q.unit_price),
        q.source_type.clone(),
        q.vendor.clone(),
        q.location.clone(),
        q.url.clone(),
        q.date.clone(),
        q.note.clone(),
    ]
    .iter()
    .map(|c| csv_cell(c))
    .collect::<Vec<_>>()
    .join(",")
}

fn write_quotes(dir: &Path, quotes: &[Quote]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut sorted: Vec<&Quote> = quotes.iter().collect();
    sorted.sort_by(|a, b| a.key.cmp(&b.key).then(a.vendor.cmp(&b.vendor)).then(a.location.cmp(&b.location)));
    let mut t = COLUMNS.join(",");
    for q in sorted {
        t.push('\n');
        t.push_str(&quote_line(q));
    }
    t.push('\n');
    crate::writefile::write(dir.join("quotes.csv"), t)?;
    // The first version's file, now in quotes.csv.
    let old = dir.join("prices.csv");
    if old.exists() {
        crate::writefile::rename(&old, dir.join("prices.csv.bak"))?;
    }
    Ok(())
}

fn write_map(dir: &Path, map: &[MapRow]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut t = MAP_COLUMNS.join(",");
    for m in map {
        t.push('\n');
        t.push_str(
            &[m.item.clone(), m.size.clone(), (if m.exclude { "exclude" } else { "price" }).to_string(), m.keys.join("+"), m.note.clone()]
                .iter()
                .map(|c| csv_cell(c))
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    t.push('\n');
    crate::writefile::write(dir.join("map.csv"), t)
}

/// A replaced or removed quote, kept.
fn to_history(dir: &Path, old: &Quote, on: &str, why: &str) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let path = dir.join("history.csv");
    let new = !path.exists();
    let mut f = crate::writefile::open_append(path)?;
    if new {
        writeln!(f, "{},changed_on,change", COLUMNS.join(","))?;
    }
    writeln!(f, "{},{},{}", quote_line(old), csv_cell(on), csv_cell(why))
}

fn days_between(a: &str, b: &str) -> Option<i64> {
    Some(super::estimate::parse_date(b)? - super::estimate::parse_date(a)?)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("pricebook.list", "Price Book", run_list)
            .params("{search?: text (key, description, catalog, vendor, location), folder?: the local price database's folder (default CADADDY_PRICEBOOK, else the pricebook folder in the settings folder)} → {keys: [{key, description, unit, catalog, price: the quote the rule picks {vendor, location, unit_price, date, why}, quotes: [{vendor, location, unit_price, unit, pack, pack_price, catalog, source_type, url, date, note, age_days, stale, origin: shipped|local}], low, high, current}], map: [{item, size, keys | exclude, note, origin}], rule: {pick, vendor, stale_days}, stale_keys: n, folder}: the material price book, every key's quotes from each vendor and location")
            .menu(&["Tools", "Estimate", "Price Book"]),
        CommandSpec::new("pricebook.set", "Add Material Quote", run_set).params("{key, unit_price?: $ per unit | pack_price + pack_qty, vendor, location?: branch, store or page (one quote per vendor and location), unit?, description?, catalog?, url?, date? (default today), source_type? (QUOTE default; WEB, BOOK, BID), note?, folder?} → {key, unit_price, quotes: n for the key, replaced?: the same vendor and location's earlier quote (kept in history.csv), price: the quote the rule now picks}"),
        CommandSpec::new("pricebook.remove", "Remove Material Quote", run_remove).params("{key, vendor, location?, folder?} → {removed: n (kept in history.csv)}: a local quote out of the book"),
        CommandSpec::new("pricebook.map", "Map Item to Price", run_map).params("{item: bill-of-materials item, size?, keys?: [price key] (added: a fixture and its trim; `a|b` either of them, their quotes pooled for the rule to pick: a web price and an invoice's), exclude?: reason, note?, folder?} → {item, size, keys | exclude}: how a bill-of-materials line is priced from the price book"),
        CommandSpec::new("pricebook.settings", "Price Book Settings", run_settings).params("{pick?: \"lowest\" (default: the lowest current quote)|\"latest\"|\"median\"|\"vendor\", vendor?: the vendor whose quote `vendor` picks, stale_days?: n (default 180: older quotes count only when no current one is left), folder?} → {pick, vendor, stale_days}: how the book picks a key's price among its quotes"),
        CommandSpec::new("pricebook.import", "Import Price Book", run_import).params("{path: .pdf (a distributor's invoices: each line a quote, maker:product its key, the branch its location) | .xlsx (sheets \"Price List\": key, description, catalog / basis, unit, pack, pack price $, unit price $, source type, vendor / book, location?, URL, price date, note; \"Item Map\": CADaddy item, size, action, price key 1, price key 2, note) or .csv (the quote columns), folder?} → {quotes: {added, updated, kept_newer}, map: n, history: n replaced, folder}: quotes into the local price database (one per key, vendor and location; an older quote than the one held is kept out)"),
    ]
}

fn folder_param(p: &Value) -> Option<&str> {
    str_param(p, "folder")
}

/// The local price database's folder (its own files, updated in place: no overwrite guard).
fn need_folder(_s: &Session, cmd: &str, p: &Value) -> Result<PathBuf> {
    local_folder(folder_param(p)).ok_or_else(|| bad(cmd, "no folder for the local price database: give `folder` (or set CADADDY_PRICEBOOK)"))
}

fn chosen_json(c: &Chosen) -> Value {
    json!({"vendor": c.quote.vendor, "location": c.quote.location, "unit_price": c.quote.unit_price, "date": c.quote.date, "source_type": c.quote.source_type, "why": c.why})
}

fn run_list(_s: &mut Session, p: &Value) -> Result<Value> {
    let b = load(folder_param(p));
    let search = str_param(p, "search").map(str::to_ascii_lowercase).unwrap_or_default();
    let mut stale_keys = 0usize;
    let keys: Vec<Value> = b
        .quotes
        .iter()
        .filter(|(k, qs)| {
            search.is_empty()
                || qs.iter().any(|q| format!("{k} {} {} {} {}", q.description, q.catalog, q.vendor, q.location).to_ascii_lowercase().contains(&search))
        })
        .filter_map(|(k, qs)| {
            let c = choose(&b, k)?;
            stale_keys += usize::from(c.current == 0);
            let first = qs.first()?;
            Some(json!({"key": k, "description": first.description, "unit": first.unit, "catalog": first.catalog, "price": chosen_json(&c),
                        "quotes": qs.iter().map(|q| q.json(&b.today, b.rule.stale_days)).collect::<Vec<_>>(), "low": c.low, "high": c.high, "current": c.current}))
        })
        .collect();
    let map: Vec<Value> = b
        .map
        .iter()
        .map(|m| {
            if m.exclude {
                json!({"item": m.item, "size": m.size, "exclude": m.note, "origin": m.origin})
            } else {
                json!({"item": m.item, "size": m.size, "keys": m.keys, "note": m.note, "origin": m.origin})
            }
        })
        .collect();
    Ok(
        json!({"keys": keys, "map": map, "rule": {"pick": b.rule.pick, "vendor": b.rule.vendor, "stale_days": b.rule.stale_days}, "stale_keys": stale_keys,
              "folder": b.folder.map(|f| f.to_string_lossy().to_string())}),
    )
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "pricebook.set";
    let dir = need_folder(s, C, p)?;
    let key = str_param(p, "key").map(str::trim).filter(|k| !k.is_empty() && k.len() <= 64).ok_or_else(|| bad(C, "`key` is required"))?.to_string();
    let f = |k: &str| p.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0);
    let pack_qty = f("pack_qty").filter(|q| *q > 0.0);
    let unit_price = f("unit_price")
        .or_else(|| f("pack_price").zip(pack_qty).map(|(pp, q)| pp / q))
        .ok_or_else(|| bad(C, "`unit_price`, or `pack_price` with `pack_qty`, is required"))?;
    let vendor =
        str_param(p, "vendor").map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| bad(C, "`vendor` (where the price is from) is required"))?;
    let book = load(Some(&dir.to_string_lossy()));
    // What the key is (its description, unit, catalog) from a quote it already has.
    let base = book.quotes.get(&key).and_then(|v| v.first()).cloned().unwrap_or_default();
    let s = |k: &str, default: &str| str_param(p, k).map(str::trim).filter(|v| !v.is_empty()).map_or_else(|| default.to_string(), str::to_string);
    let q = Quote {
        key: key.clone(),
        description: s("description", &base.description),
        catalog: s("catalog", &base.catalog),
        unit: s("unit", if base.unit.is_empty() { "each" } else { &base.unit }),
        pack: pack_qty.map_or_else(|| s("pack", ""), |n| format!("{n}")),
        pack_price: f("pack_price"),
        unit_price,
        source_type: s("source_type", "QUOTE"),
        vendor: vendor.to_string(),
        location: s("location", ""),
        url: s("url", ""),
        date: s("date", &book.today),
        note: s("note", ""),
        origin: "local",
    };
    if super::estimate::parse_date(&q.date).is_none() {
        return Err(bad(C, "`date` is YYYY-MM-DD"));
    }
    if !base.unit.is_empty() && !same_unit(&base.unit, &q.unit) {
        return Err(bad(C, format!("{key} is priced per {}; this quote is per {}", base.unit, q.unit)));
    }
    let mut mine = local_quotes(&dir);
    // A shipped quote of the same vendor and location is replaced too (kept in the history).
    let replaced = book.quotes.get(&key).and_then(|v| v.iter().find(|o| o.same_source(&q)).cloned());
    if replaced.as_ref().is_some_and(|o| !o.date.is_empty() && q.date < o.date) {
        return Err(bad(C, format!("{vendor} already has a newer quote for {key} ({})", replaced.map(|o| o.date).unwrap_or_default())));
    }
    if let Some(old) = &replaced {
        to_history(&dir, old, &book.today, "replaced").map_err(|e| bad(C, format!("history: {e}")))?;
    }
    let _ = add(&mut mine, q);
    write_quotes(&dir, &mine).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    let after = load(Some(&dir.to_string_lossy()));
    Ok(json!({"key": key, "unit_price": unit_price, "quotes": after.quotes.get(&key).map_or(0, Vec::len),
              "replaced": replaced.map(|h| json!({"unit_price": h.unit_price, "date": h.date, "vendor": h.vendor, "location": h.location, "origin": h.origin})),
              "price": choose(&after, &key).map(|c| chosen_json(&c))}))
}

fn run_remove(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "pricebook.remove";
    let dir = need_folder(s, C, p)?;
    let key = str_param(p, "key").map(str::trim).filter(|k| !k.is_empty()).ok_or_else(|| bad(C, "`key` is required"))?;
    let vendor = str_param(p, "vendor").map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| bad(C, "`vendor` is required"))?;
    let location = str_param(p, "location").map(str::trim);
    let mut mine = local_quotes(&dir);
    let today = load(Some(&dir.to_string_lossy())).today;
    let (gone, kept): (Vec<Quote>, Vec<Quote>) = mine.drain(..).partition(|q| {
        q.key.eq_ignore_ascii_case(key) && q.vendor.eq_ignore_ascii_case(vendor) && location.is_none_or(|l| q.location.eq_ignore_ascii_case(l))
    });
    for q in &gone {
        to_history(&dir, q, &today, "removed").map_err(|e| bad(C, format!("history: {e}")))?;
    }
    if !gone.is_empty() {
        write_quotes(&dir, &kept).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    }
    Ok(json!({"removed": gone.len()}))
}

fn run_settings(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "pricebook.settings";
    let dir = need_folder(s, C, p)?;
    let mut r = local_rule(&dir);
    if let Some(pick) = str_param(p, "pick") {
        if !PICKS.contains(&pick) {
            return Err(bad(C, format!("`pick` is one of {}", PICKS.join(", "))));
        }
        r.pick = pick.to_string();
    }
    if let Some(v) = str_param(p, "vendor") {
        r.vendor = v.trim().to_string();
    }
    if let Some(n) = p.get("stale_days").filter(|v| !v.is_null()) {
        r.stale_days = n.as_i64().filter(|n| (1..=36_500).contains(n)).ok_or_else(|| bad(C, "`stale_days` is a whole number of days, 1 or more"))?;
    }
    if r.pick == "vendor" && r.vendor.is_empty() {
        return Err(bad(C, "`pick: vendor` needs `vendor`"));
    }
    let v = json!({"pick": r.pick, "vendor": r.vendor, "stale_days": r.stale_days});
    std::fs::create_dir_all(&dir)
        .and_then(|()| crate::writefile::write(dir.join("settings.json"), v.to_string()))
        .map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    Ok(v)
}

fn run_map(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "pricebook.map";
    let dir = need_folder(s, C, p)?;
    let item = str_param(p, "item").map(str::trim).filter(|i| !i.is_empty()).ok_or_else(|| bad(C, "`item` is required"))?.to_string();
    let size = str_param(p, "size").map(str::trim).unwrap_or_default().to_string();
    let keys: Vec<String> = p
        .get("keys")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .take(8)
        .collect();
    let exclude = str_param(p, "exclude").map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);
    if keys.is_empty() && exclude.is_none() {
        return Err(bad(C, "give `keys` (the prices it adds) or `exclude` (why it isn't priced)"));
    }
    let book = load(Some(&dir.to_string_lossy()));
    if let Some(k) = keys.iter().flat_map(|k| k.split('|')).map(str::trim).find(|k| !book.quotes.contains_key(*k)) {
        return Err(bad(C, format!("no price key {k} (pricebook.set a quote for it first)")));
    }
    let mut map = local_map(&dir);
    map.retain(|m| !(m.item.eq_ignore_ascii_case(&item) && m.size.eq_ignore_ascii_case(&size)));
    let note = exclude.clone().or_else(|| str_param(p, "note").map(str::to_string)).unwrap_or_default();
    map.push(MapRow { item: item.clone(), size: size.clone(), exclude: exclude.is_some(), keys: keys.clone(), note, origin: "local" });
    write_map(&dir, &map).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    Ok(json!({"item": item, "size": size, "keys": keys, "exclude": exclude}))
}

/// A distributor invoice's lines as quotes: each line's maker and product code (`PVC:400`), its
/// description, its price per unit (its per-code: E each, C per hundred, M per thousand), the
/// invoice's vendor, branch (its city) and date. A line whose price and quantity don't give its
/// extension is left out (a back order's zero extension keeps its price).
pub(crate) fn invoice_quotes(text: &str) -> Vec<Quote> {
    let mut out: Vec<Quote> = Vec::new();
    let num = |v: &str| v.trim().replace(',', "").parse::<f64>().ok().filter(|x| x.is_finite());
    let (mut vendor, mut branch, mut date, mut invoice) = (String::new(), String::new(), String::new(), String::new());
    for page in text.split("--- page ") {
        let lines: Vec<&str> = page.lines().collect();
        // The header: `<vendor>` alone on a line before `<no>   <mm/dd/yy>`, the branch city after.
        for (i, l) in lines.iter().enumerate() {
            let cells: Vec<&str> = l.split("   ").map(str::trim).filter(|c| !c.is_empty()).collect();
            // `<no>   <date>`, or `<vendor's last word>   <no>   <date>`.
            let (lead, no, d) = match cells.as_slice() {
                [no, d] => ("", *no, *d),
                [lead, no, d] => (*lead, *no, *d),
                _ => continue,
            };
            let is_no = no.len() >= 6 && no.contains('-') && no.chars().all(|c| c.is_ascii_digit() || c == '-');
            let parts: Vec<&str> = d.split('/').collect();
            let [m, dd, y] = parts.as_slice() else { continue };
            if !is_no || d.len() != 8 {
                continue;
            }
            invoice = no.to_string();
            date = format!("20{y}-{m}-{dd}");
            if let Some(v) = i
                .checked_sub(1)
                .and_then(|j| lines.get(j))
                .map(|v| v.trim())
                .filter(|v| !v.is_empty() && v.len() <= 40 && v.chars().all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '&'))
            {
                vendor = if lead.is_empty() { v.to_string() } else { format!("{v} {lead}") };
            }
            // The branch: the first `City, ST 12345` after it.
            let city = lines.iter().skip(i + 1).take(4).flat_map(|l| l.split("   ")).find_map(|c| {
                let (city, rest) = c.trim().split_once(", ")?;
                let mut w = rest.split_whitespace();
                let (st, zip) = (w.next()?, w.next()?);
                (st.len() == 2 && st.chars().all(|x| x.is_ascii_uppercase()) && zip.len() >= 5 && zip.chars().take(5).all(|x| x.is_ascii_digit()))
                    .then(|| format!("{city}, {st}"))
            });
            if let Some(c) = city {
                branch = c;
            }
        }
        let mut lot: Option<Quote> = None;
        for l in &lines {
            let cells: Vec<&str> = l.split("   ").map(str::trim).filter(|c| !c.is_empty()).collect();
            // A lot's contents: its designations and its unpriced lines, until the next priced line.
            if let Some(q) = lot.as_mut() {
                let t = l.trim();
                if let Some(d) = t.strip_prefix("TYPE/DESIGNATION:") {
                    q.description.push_str(&format!("; {}", d.trim()));
                    continue;
                }
                if cells.len() >= 6 && cells.contains(&"0.00") {
                    q.note.push_str(&format!("; {} {}", cells.first().unwrap_or(&""), cells.get(3).unwrap_or(&"")));
                    continue;
                }
            }
            if l.contains("LOT BILLING") {
                if let Some(q) = lot.take() {
                    let _ = add(&mut out, q);
                }
                let price = cells.iter().find(|c| c.contains('.') && c.contains(|ch: char| ch.is_ascii_digit())).and_then(|c| num(c));
                if let Some(price) = price.filter(|p| *p > 0.0) {
                    lot = Some(Quote {
                        key: format!("LOT:{invoice}"),
                        description: "lot".into(),
                        catalog: String::new(),
                        unit: "lot".into(),
                        pack: String::new(),
                        pack_price: None,
                        unit_price: price,
                        source_type: "INVOICE".into(),
                        vendor: if vendor.is_empty() { "distributor".into() } else { vendor.clone() },
                        location: branch.clone(),
                        url: String::new(),
                        date: date.clone(),
                        note: format!("invoice {invoice}; consists of"),
                        origin: "local",
                    });
                }
                continue;
            }
            // `qty  maker  product  description  ship  price  per  tax  extension  disc ...`
            let Some(p) = (3..cells.len().saturating_sub(3)).find(|&k| {
                cells.get(k).is_some_and(|c| c.contains('.') && num(c).is_some()) && matches!(cells.get(k + 1).copied(), Some("E" | "C" | "M"))
            }) else {
                continue;
            };
            let (Some(price), Some(per)) = (cells.get(p).and_then(|c| num(c)), cells.get(p + 1)) else { continue };
            let div = match *per {
                "C" => 100.0,
                "M" => 1000.0,
                _ => 1.0,
            };
            let ship = cells.get(p - 1).and_then(|c| num(c)).unwrap_or(0.0);
            let ext = cells.get(p + 3).and_then(|c| num(c)).unwrap_or(0.0);
            if price <= 0.0 || (ext > 0.0 && (price * ship / div - ext).abs() > 0.02f64.max(ext * 0.005)) {
                continue;
            }
            let (Some(maker), Some(product)) = (cells.get(1), cells.get(2)) else { continue };
            let description = cells.get(3..p - 1).map(|d| d.join(" ")).unwrap_or_default();
            let up = format!(" {} {} ", description, product).to_ascii_uppercase();
            // By the foot: conduit, wire, cable, strut; a price said per foot (`#.106/FT`); an NM / MC /
            // AC cable's code (`NM12/2WG`).
            let code = product.to_ascii_uppercase();
            let cable = ["NM", "MC", "AC"].iter().any(|c| {
                code.starts_with(c) && code.get(c.len()..).is_some_and(|r| r.starts_with(|ch: char| ch.is_ascii_digit()) && r.contains('/'))
            });
            let length = cable
                || up.contains("/FT")
                || ["CONDUIT", "EMT", "SCH 40", "SCH 80", "/LIFT", "THHN", "XHHW", "WIRE", "CABLE", "STRUT", " MC "].iter().any(|w| up.contains(w));
            let fitting = [
                "ELL",
                "ELBOW",
                "COUP",
                "CONN",
                "ADAPT",
                "BODY",
                "STRAP",
                "BOX",
                "CLAMP",
                "BUSHING",
                "LOCKNUT",
                "CAP",
                "TAPE",
                "PULL LINE",
                "TERMINATOR",
                "FITTING",
                "CPLG",
            ]
            .iter()
            .any(|w| up.contains(w));
            let unit = if (div > 1.0 || cable) && length && !fitting { "ft" } else { "each" };
            let q = Quote {
                key: format!("{maker}:{product}").to_ascii_uppercase(),
                description,
                catalog: (*product).to_string(),
                unit: unit.into(),
                pack: if div > 1.0 { format!("per {div}") } else { String::new() },
                pack_price: (div > 1.0).then_some(price),
                unit_price: (price / div * 1e6).round() / 1e6,
                source_type: "INVOICE".into(),
                vendor: if vendor.is_empty() { "distributor".into() } else { vendor.clone() },
                location: branch.clone(),
                url: String::new(),
                date: date.clone(),
                note: format!("invoice {invoice}, {ship} shipped"),
                origin: "local",
            };
            if let Some(lq) = lot.take() {
                let _ = add(&mut out, lq);
            }
            let _ = add(&mut out, q);
        }
        if let Some(q) = lot.take() {
            let _ = add(&mut out, q);
        }
    }
    out
}

/// The header's columns by name (`unit price $` → `unit_price`).
fn header_key(h: &str) -> Option<&'static str> {
    let u = h.trim().to_ascii_lowercase();
    Some(match u.as_str() {
        "key" | "price key" => "key",
        "description" => "description",
        "catalog / basis" | "catalog" | "catalog number" => "catalog",
        "unit" => "unit",
        "pack" => "pack",
        "pack price $" | "pack price" | "pack_price" => "pack_price",
        "unit price $" | "unit price" | "unit_price" => "unit_price",
        "source type" | "source_type" => "source_type",
        "vendor / book" | "vendor" => "vendor",
        "location" | "branch" | "store" => "location",
        "url" => "url",
        "price date" | "date" => "date",
        "note" | "notes" => "note",
        "cadaddy item" | "item" => "item",
        "size" => "size",
        "action" => "action",
        "price key 1" => "key1",
        "price key 2" => "key2",
        "price key 3" => "key3",
        "keys" => "keys",
        _ => return None,
    })
}

/// A table as rows by column name; the header is the first row naming `want`. The price columns
/// come from `raw` (the same table unformatted) when given.
fn table_rows(cells: &[Vec<String>], raw: Option<&[Vec<String>]>, want: &str) -> Vec<BTreeMap<String, String>> {
    let Some(hi) = cells.iter().take(20).position(|r| r.iter().any(|c| header_key(c) == Some(want))) else { return Vec::new() };
    let cols: Vec<Option<&'static str>> = cells.get(hi).map(|r| r.iter().map(|c| header_key(c)).collect()).unwrap_or_default();
    cells
        .iter()
        .enumerate()
        .skip(hi + 1)
        .take(MAX_ROWS)
        .map(|(ri, r)| {
            let mut m: BTreeMap<String, String> = BTreeMap::new();
            for (ci, (c, v)) in cols.iter().zip(r).enumerate() {
                if let Some(c) = c {
                    let exact = matches!(*c, "unit_price" | "pack_price").then(|| raw.and_then(|t| t.get(ri)).and_then(|row| row.get(ci))).flatten();
                    m.insert((*c).to_string(), exact.unwrap_or(v).trim().to_string());
                }
            }
            if !m.contains_key("keys") {
                let keys: Vec<String> = ["key1", "key2", "key3"].iter().filter_map(|k| m.get(*k).filter(|v| !v.is_empty()).cloned()).collect();
                m.insert("keys".into(), keys.join("+"));
            }
            m
        })
        .collect()
}

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "pricebook.import";
    let path =
        str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(C, "`path` (a .xlsx or .csv price book) is required"))?;
    let dir = need_folder(s, C, p)?;
    let bytes = crate::readfile::read(path).map_err(|e| bad(C, format!("{path}: {e}")))?;
    // A distributor's invoices (a PDF): each line a quote.
    let invoice_rows: Vec<Quote> = if path.to_ascii_lowercase().ends_with(".pdf") {
        let text = super::spec_import::pdf_text(&bytes, path).map_err(|e| bad(C, format!("{path}: {e}")))?;
        let q = invoice_quotes(&text);
        if q.is_empty() {
            return Err(bad(C, format!("{path}: no invoice lines read (a PDF is read as a distributor's invoices)")));
        }
        q
    } else {
        Vec::new()
    };
    let (quote_rows, map_rows) = if !invoice_rows.is_empty() {
        (Vec::new(), Vec::new())
    } else if path.to_ascii_lowercase().ends_with(".xlsx") {
        let wb = super::table::xlsx::open(&bytes).map_err(|e| bad(C, format!("{path}: {e}")))?;
        let read = |name: Option<&str>| (wb.cells(name, None).unwrap_or_default(), wb.values(name, None).unwrap_or_default());
        let (shown, raw) = read(Some("Price List"));
        let mut quotes = table_rows(&shown, Some(&raw), "unit_price");
        if quotes.is_empty() {
            let (shown, raw) = read(None);
            quotes = table_rows(&shown, Some(&raw), "unit_price");
        }
        let (map, _) = read(Some("Item Map"));
        (quotes, table_rows(&map, None, "item"))
    } else {
        let text = String::from_utf8(bytes).map_err(|e| bad(C, format!("{path}: {e}")))?;
        let cells = super::estimate::csv_cells(&text);
        (table_rows(&cells, None, "unit_price"), table_rows(&cells, None, "item"))
    };
    let book = load(Some(&dir.to_string_lossy()));
    let mut mine = local_quotes(&dir);
    let (mut added, mut updated, mut kept_newer, mut history) = (0usize, 0usize, 0usize, 0usize);
    let read: Vec<Quote> = quote_rows.iter().filter_map(|r| quote_of(r, "local")).chain(invoice_rows).collect();
    for q in read {
        let held = book.quotes.get(&q.key).and_then(|v| v.iter().find(|o| o.same_source(&q)).cloned());
        match held {
            Some(old) if !old.date.is_empty() && q.date < old.date => kept_newer += 1,
            Some(old) if (old.unit_price - q.unit_price).abs() < 1e-9 && old.date == q.date && old.unit == q.unit && old.origin == "local" => {}
            Some(old) => {
                to_history(&dir, &old, &book.today, "replaced by import").map_err(|e| bad(C, format!("history: {e}")))?;
                history += 1;
                updated += 1;
                let _ = add(&mut mine, q);
            }
            None => {
                added += 1;
                let _ = add(&mut mine, q);
            }
        }
    }
    write_quotes(&dir, &mine).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    let mut map = local_map(&dir);
    let mut mapped = 0usize;
    for r in &map_rows {
        let Some(m) = map_of(r, "local") else { continue };
        map.retain(|x| !(x.item.eq_ignore_ascii_case(&m.item) && x.size.eq_ignore_ascii_case(&m.size)));
        map.push(m);
        mapped += 1;
    }
    if mapped > 0 {
        write_map(&dir, &map).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    }
    Ok(
        json!({"quotes": {"added": added, "updated": updated, "kept_newer": kept_newer}, "map": mapped, "history": history, "folder": dir.to_string_lossy()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(vendor: &str, location: &str, price: f64, date: &str) -> Quote {
        Quote {
            key: "dupTR".into(),
            unit: "each".into(),
            unit_price: price,
            vendor: vendor.into(),
            location: location.into(),
            date: date.into(),
            source_type: "QUOTE".into(),
            origin: "local",
            ..Quote::default()
        }
    }

    #[test]
    fn the_shipped_book_prices_common_material_with_its_source_and_date() {
        let b = load(Some("/nonexistent-pricebook-folder"));
        let emt = b.quotes.get("emt34").and_then(|v| v.first()).expect("3/4 EMT");
        assert_eq!(emt.unit, "ft");
        // The pack's price over its count: exact, not the sheet's rounding.
        assert!((emt.unit_price - 0.946).abs() < 1e-9, "{emt:?}");
        assert_eq!(emt.source_type, "WEB");
        assert!(!emt.url.is_empty() && !emt.date.is_empty());
        // Only public prices are shipped.
        assert!(b.quotes.values().flatten().all(|p| p.source_type == "WEB"));
        match lookup(&b, "EMT conduit", "3/4\"", "ft") {
            Some(Found::Priced { material, parts, .. }) => {
                assert!((material - emt.unit_price).abs() < 1e-9);
                assert!(source_text(&parts).starts_with("emt34 $0.9460 Elliott Electric"), "{}", source_text(&parts));
            }
            other => panic!("{other:?}"),
        }
        // Per foot, not per each.
        assert_eq!(lookup(&b, "EMT conduit", "3/4\"", "each"), None);
        assert_eq!(lookup(&b, "EMT conduit", "4\"", "ft"), None);
    }

    #[test]
    fn a_key_holds_each_vendors_and_locations_quote_and_the_rule_picks_one() {
        let mut b = Book { today: "2026-11-02".into(), ..Book::default() };
        let mut v = Vec::new();
        for x in [
            q("Supply A", "North branch", 8.08, "2026-11-01"),
            q("Supply A", "South branch", 7.90, "2026-10-20"),
            q("Big Box", "", 9.25, "2026-11-02"),
            q("Old Co", "", 5.00, "2025-01-01"),
        ] {
            assert_eq!(add(&mut v, x), Added::New);
        }
        // The same vendor and location again replaces its quote; an older one is kept out.
        assert!(matches!(add(&mut v, q("Supply A", "North branch", 8.20, "2026-11-02")), Added::Replaced(o) if o.unit_price == 8.08));
        assert_eq!(add(&mut v, q("Supply A", "North branch", 6.00, "2026-01-01")), Added::Older);
        b.quotes.insert("dupTR".into(), v);
        let c = choose(&b, "dupTR").unwrap();
        // The lowest current quote (Old Co's is stale), the spread of the current ones.
        assert_eq!((c.quote.vendor.as_str(), c.quote.location.as_str(), c.quote.unit_price), ("Supply A", "South branch", 7.90));
        assert_eq!((c.quotes, c.current), (4, 3));
        assert_eq!((c.low, c.high), (7.90, 9.25));
        b.rule = Rule { pick: "latest".into(), ..Rule::default() };
        assert_eq!(choose(&b, "dupTR").unwrap().quote.vendor, "Big Box");
        b.rule = Rule { pick: "vendor".into(), vendor: "supply a".into(), ..Rule::default() };
        assert_eq!(choose(&b, "dupTR").unwrap().quote.unit_price, 7.90);
        b.rule = Rule { pick: "median".into(), ..Rule::default() };
        assert_eq!(choose(&b, "dupTR").unwrap().quote.unit_price, 8.20);
        // Either of two keys: their quotes pooled.
        b.rule = Rule::default();
        b.quotes.insert(
            "LEV:TBR20".into(),
            vec![Quote {
                key: "LEV:TBR20".into(),
                unit: "each".into(),
                unit_price: 6.75,
                vendor: "CED".into(),
                location: "Ponder, TX".into(),
                date: "2026-10-01".into(),
                source_type: "INVOICE".into(),
                origin: "local",
                ..Quote::default()
            }],
        );
        let c = choose(&b, "dupTR|LEV:TBR20").unwrap();
        assert_eq!((c.quote.vendor.as_str(), c.quote.unit_price, c.quotes), ("CED", 6.75, 5));
    }

    #[test]
    fn a_distributors_invoice_lines_are_quotes_at_their_branch() {
        let text = "--- page 1 of 2 ---\nInvoice No.   Invoice Date\nACME\n1234-5678901   02/14/26\n100 Main St   ** COPY **\nSpringfield, IL 62701\n\
                    ORDER QTY   PRODUCT   CODE   DESCRIPTION   CODE   SHIP QTY   PRICE PER   DISC   EXTENSION   C/D   TAX   TY\n\
                    500   ALLIED   34EMT   3/4 EMT CONDUIT 10FT   500   95.00   C   N   475.00   0.0 %   Y   S\n\
                    10   BRIDGE   231   3/4 EMT SS CONNECTOR   10   48.00   C   N   4.80   0.0 %   Y   S\n\
                    250   WIRE   NM12/2WG   250' ROLL   250   0.61   E   N   152.50   0.0 %   Y   S\n\
                    4   LEV   TBR20   20A TR DUPLEX   0   8.10   E   N   0.00   0.0 %   Y   B\n\
                    2   BAD   X1   WRONG EXTENSION   2   10.00   E   N   99.00   0.0 %   Y   S\n";
        let q = invoice_quotes(text);
        assert_eq!(q.len(), 4, "{q:#?}");
        assert_eq!(q.iter().find(|x| x.key == "WIRE:NM12/2WG").map(|x| x.unit.as_str()), Some("ft"));
        let emt = q.iter().find(|x| x.key == "ALLIED:34EMT").expect("emt");
        assert_eq!(
            (emt.unit.as_str(), emt.unit_price, emt.vendor.as_str(), emt.location.as_str(), emt.date.as_str()),
            ("ft", 0.95, "ACME", "Springfield, IL", "2026-02-14")
        );
        assert_eq!(emt.source_type, "INVOICE");
        assert_eq!(q.iter().find(|x| x.key == "BRIDGE:231").map(|x| (x.unit.as_str(), x.unit_price)), Some(("each", 0.48)));
        // A back order keeps its price.
        assert_eq!(q.iter().find(|x| x.key == "LEV:TBR20").map(|x| x.unit_price), Some(8.1));
        // A lot: its price, its designations and contents.
        let lots = "--- page 1 of 1 ---\nInvoice No.   Invoice Date\nBIG ELECTRIC\nSUPPLY   9999-0000001   03/01/26\nTown, TX 75001\n\
                    1   MFR   LOT BILLING   1   3,149.00   E   N   3,149.00   0.0 %   Y   D\nCONSISTS OF\nTYPE/DESIGNATION: 26020 B\n\
                    1   MFR   PB_INTERIOR   INTERIOR -P1   1   0.00   E   N   0.00   0.0 %   N   D\n";
        let q = invoice_quotes(lots);
        let l = q.iter().find(|x| x.unit == "lot").expect("lot");
        assert_eq!((l.unit_price, l.vendor.as_str(), l.location.as_str()), (3149.0, "BIG ELECTRIC SUPPLY", "Town, TX"));
        assert!(l.description.contains("26020 B") && l.note.contains("INTERIOR -P1"), "{l:?}");
    }

    #[test]
    fn a_packs_count_reads() {
        assert_eq!(pack_count("10 ft"), Some(10.0));
        assert_eq!(pack_count("25 pk"), Some(25.0));
        assert_eq!(pack_count("each"), Some(1.0));
        assert_eq!(pack_count("50+"), None);
        assert_eq!(pack_count(""), None);
    }
}
