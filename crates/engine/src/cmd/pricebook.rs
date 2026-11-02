//! The material price book: recent, dated prices CADaddy holds and keeps up to date.
//!
//! Two layers, each row a price key (`emt34`) with its unit price, pack, catalog, vendor, URL,
//! date and source type (`WEB` a public distributor page, `QUOTE` a supplier's quote, or the
//! owner's own: a licensed book, a bid's unit price):
//! - **shipped** (`assets/costbook/pricebook.csv`): public distributor prices for common material,
//!   each with its page and the date it was read;
//! - **this machine's** (`<settings>/pricebook/`): prices imported or set here, never shipped or
//!   committed (a licensed book's, a job's). A key here replaces the shipped one unless older.
//!
//! The map (`pricebook_map.csv`, and the machine's `map.csv`) ties a bill-of-materials item and
//! size to one or more keys (a fixture and its trim add), or excludes it with a reason. Every
//! replaced price goes to the machine's `history.csv`, so a change is never lost.
//!
//! `estimate` prices a line from the price book after the drawing's own prices and before the
//! starter cost book, the line saying which key, vendor and date.
//!
//! - `pricebook.list {search?, stale_days?, folder?}`
//! - `pricebook.set {key, unit_price | pack_price + pack_qty, unit?, description?, catalog?,
//!   vendor, url?, date?, source_type?, note?, folder?}`
//! - `pricebook.map {item, size?, keys | exclude, note?, folder?}`
//! - `pricebook.import {path (.xlsx with Price List / Item Map sheets, or a .csv), folder?}`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const SHIPPED_CSV: &str = include_str!("../../../../assets/costbook/pricebook.csv");
const SHIPPED_MAP: &str = include_str!("../../../../assets/costbook/pricebook_map.csv");
/// Rows read from one file, at most (hostile files).
const MAX_ROWS: usize = 50_000;
/// A price older than this many days is stale (list's default).
const STALE_DAYS: i64 = 180;
const COLUMNS: [&str; 12] =
    ["key", "description", "catalog", "unit", "pack", "pack_price", "unit_price", "source_type", "vendor", "url", "date", "note"];
const MAP_COLUMNS: [&str; 5] = ["item", "size", "action", "keys", "note"];

/// One price.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Price {
    pub key: String,
    pub description: String,
    pub catalog: String,
    pub unit: String,
    pub pack: String,
    pub pack_price: Option<f64>,
    pub unit_price: f64,
    pub source_type: String,
    pub vendor: String,
    pub url: String,
    pub date: String,
    pub note: String,
    /// `shipped` or `machine`.
    pub origin: &'static str,
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

#[derive(Clone, Debug, Default)]
pub(crate) struct Book {
    pub prices: BTreeMap<String, Price>,
    pub map: Vec<MapRow>,
    pub folder: Option<PathBuf>,
}

/// This machine's price book folder: `folder` given, else `<settings>/pricebook`.
fn machine_folder(folder: Option<&str>) -> Option<PathBuf> {
    match folder.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => Some(PathBuf::from(f)),
        None => crate::control_auth::config_dir().map(|d| d.join("pricebook")),
    }
}

fn rows(text: &str) -> Vec<BTreeMap<String, String>> {
    super::estimate::csv_rows(text).into_iter().take(MAX_ROWS).collect()
}

fn num(v: Option<&String>) -> Option<f64> {
    v.and_then(|t| t.trim().trim_start_matches('$').replace(',', "").parse::<f64>().ok()).filter(|x| x.is_finite() && *x >= 0.0)
}

fn price_of(r: &BTreeMap<String, String>, origin: &'static str) -> Option<Price> {
    let g = |k: &str| r.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
    let key = g("key");
    let unit_price = num(r.get("unit_price"))?;
    (!key.is_empty()).then(|| Price {
        key,
        description: g("description"),
        catalog: g("catalog"),
        unit: g("unit"),
        pack: g("pack"),
        pack_price: num(r.get("pack_price")),
        unit_price,
        source_type: g("source_type"),
        vendor: g("vendor"),
        url: g("url"),
        date: g("date"),
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

/// The price book: shipped, with this machine's over it (a key's newer price; a map row of the
/// same item and size).
pub(crate) fn load(folder: Option<&str>) -> Book {
    let mut b = Book { folder: machine_folder(folder), ..Book::default() };
    for p in rows(SHIPPED_CSV).iter().filter_map(|r| price_of(r, "shipped")) {
        b.prices.insert(p.key.clone(), p);
    }
    b.map = rows(SHIPPED_MAP).iter().filter_map(|r| map_of(r, "shipped")).collect();
    if let Some(dir) = b.folder.clone() {
        if let Ok(t) = std::fs::read_to_string(dir.join("prices.csv")) {
            for p in rows(&t).iter().filter_map(|r| price_of(r, "machine")) {
                let older = b.prices.get(&p.key).is_some_and(|s| s.origin == "shipped" && !s.date.is_empty() && p.date < s.date);
                if !older {
                    b.prices.insert(p.key.clone(), p);
                }
            }
        }
        if let Ok(t) = std::fs::read_to_string(dir.join("map.csv")) {
            for m in rows(&t).iter().filter_map(|r| map_of(r, "machine")) {
                b.map.retain(|x| !(x.item.eq_ignore_ascii_case(&m.item) && x.size.eq_ignore_ascii_case(&m.size)));
                b.map.push(m);
            }
        }
    }
    b
}

/// What the price book says of a bill-of-materials line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Found {
    /// Its material per unit, the keys' prices it adds, and the oldest date among them.
    Priced {
        material: f64,
        parts: Vec<Price>,
        date: String,
    },
    Excluded(String),
}

/// The line's price: its map row (item and size; else the item with no size), its keys' prices
/// added. A key with no price, or a unit that isn't the line's, prices nothing.
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
    let parts: Option<Vec<Price>> = row.keys.iter().map(|k| b.prices.get(k).cloned()).collect();
    let parts = parts?;
    let same = |a: &str, b: &str| {
        let n = |u: &str| match u.trim().to_ascii_lowercase().as_str() {
            "ft" | "lf" | "foot" | "feet" => "ft".to_string(),
            "ea" | "each" | "" => "each".to_string(),
            o => o.to_string(),
        };
        n(a) == n(b)
    };
    if !parts.iter().all(|p| same(&p.unit, unit)) {
        return None;
    }
    let date = parts.iter().map(|p| p.date.clone()).filter(|d| !d.is_empty()).min().unwrap_or_default();
    Some(Found::Priced { material: parts.iter().map(|p| p.unit_price).sum(), parts, date })
}

/// How a price reads on an estimate line: `emt34 Elliott Electric 2026-11-02 (WEB)`.
pub(crate) fn source_text(parts: &[Price]) -> String {
    parts.iter().map(|p| format!("{} {} {} ({})", p.key, p.vendor, p.date, p.source_type)).collect::<Vec<_>>().join(" + ")
}

fn csv_cell(v: &str) -> String {
    if v.contains([',', '"', '\n']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() }
}

fn price_line(p: &Price) -> String {
    let f = |x: Option<f64>| x.map(|v| format!("{v}")).unwrap_or_default();
    [
        p.key.clone(),
        p.description.clone(),
        p.catalog.clone(),
        p.unit.clone(),
        p.pack.clone(),
        f(p.pack_price),
        format!("{}", p.unit_price),
        p.source_type.clone(),
        p.vendor.clone(),
        p.url.clone(),
        p.date.clone(),
        p.note.clone(),
    ]
    .iter()
    .map(|c| csv_cell(c))
    .collect::<Vec<_>>()
    .join(",")
}

/// This machine's own rows (not the shipped ones).
fn machine_prices(dir: &Path) -> BTreeMap<String, Price> {
    std::fs::read_to_string(dir.join("prices.csv"))
        .map(|t| rows(&t).iter().filter_map(|r| price_of(r, "machine")).map(|p| (p.key.clone(), p)).collect())
        .unwrap_or_default()
}

fn machine_map(dir: &Path) -> Vec<MapRow> {
    std::fs::read_to_string(dir.join("map.csv")).map(|t| rows(&t).iter().filter_map(|r| map_of(r, "machine")).collect()).unwrap_or_default()
}

fn write_prices(dir: &Path, prices: &BTreeMap<String, Price>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut t = COLUMNS.join(",");
    for p in prices.values() {
        t.push('\n');
        t.push_str(&price_line(p));
    }
    t.push('\n');
    std::fs::write(dir.join("prices.csv"), t)
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
    std::fs::write(dir.join("map.csv"), t)
}

/// A replaced price, kept.
fn to_history(dir: &Path, old: &Price, replaced_on: &str) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let path = dir.join("history.csv");
    let new = !path.exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if new {
        writeln!(f, "{},replaced_on", COLUMNS.join(","))?;
    }
    writeln!(f, "{},{}", price_line(old), csv_cell(replaced_on))
}

fn today() -> String {
    let now = super::coordination::iso_date();
    now.get(..10).unwrap_or(&now).to_string()
}

fn days_between(a: &str, b: &str) -> Option<i64> {
    Some(super::estimate::parse_date(b)? - super::estimate::parse_date(a)?)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("pricebook.list", "Price Book", run_list).params("{search?: text (key, description, catalog, vendor), stale_days?: n (default 180), folder?: this machine's price book folder (default the pricebook folder in the settings folder)} → {prices: [{key, description, catalog, unit, pack, pack_price, unit_price, source_type, vendor, url, date, age_days, stale, origin: shipped|machine}], map: [{item, size, keys | exclude, note, origin}], stale: n, folder}: the material price book, shipped public prices with this machine's over them").menu(&["Tools", "Estimate", "Price Book"]),
        CommandSpec::new("pricebook.set", "Set Material Price", run_set).params("{key, unit_price?: $ per unit | pack_price + pack_qty, unit?, description?, catalog?, vendor, url?, date? (default today), source_type? (QUOTE default; WEB, BOOK, BID…), note?, folder?} → {key, unit_price, replaced?: the price it replaced (kept in history.csv)}: a price on this machine's book (a newer quote replaces the old)"),
        CommandSpec::new("pricebook.map", "Map Item to Price", run_map).params("{item: bill-of-materials item, size?, keys?: [price key] (added: a fixture and its trim), exclude?: reason, note?, folder?} → {item, size, keys | exclude}: how a bill-of-materials line is priced from the price book"),
        CommandSpec::new("pricebook.import", "Import Price Book", run_import).params("{path: .xlsx (sheets \"Price List\": key, description, catalog / basis, unit, pack, pack price $, unit price $, source type, vendor / book, URL, price date, note; \"Item Map\": CADaddy item, size, action, price key 1, price key 2, note) or .csv (the price columns), folder?} → {prices: {added, updated, kept_newer}, map: n, history: n replaced, folder}: prices into this machine's book; a price older than the one held is kept out"),
    ]
}

fn folder_param(p: &Value) -> Option<&str> {
    str_param(p, "folder")
}

fn run_list(_s: &mut Session, p: &Value) -> Result<Value> {
    let b = load(folder_param(p));
    let stale_days = p.get("stale_days").and_then(Value::as_i64).filter(|d| *d > 0).unwrap_or(STALE_DAYS);
    let search = str_param(p, "search").map(str::to_ascii_lowercase).unwrap_or_default();
    let now = today();
    let mut stale = 0usize;
    let prices: Vec<Value> = b
        .prices
        .values()
        .filter(|x| search.is_empty() || format!("{} {} {} {}", x.key, x.description, x.catalog, x.vendor).to_ascii_lowercase().contains(&search))
        .map(|x| {
            let age = days_between(&x.date, &now);
            let old = age.is_none_or(|a| a > stale_days);
            stale += usize::from(old);
            json!({"key": x.key, "description": x.description, "catalog": x.catalog, "unit": x.unit, "pack": x.pack, "pack_price": x.pack_price,
                   "unit_price": x.unit_price, "source_type": x.source_type, "vendor": x.vendor, "url": x.url, "date": x.date, "note": x.note,
                   "age_days": age, "stale": old, "origin": x.origin})
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
    Ok(json!({"prices": prices, "map": map, "stale": stale, "stale_days": stale_days, "folder": b.folder.map(|f| f.to_string_lossy().to_string())}))
}

fn run_set(_s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "pricebook.set";
    let dir = machine_folder(folder_param(p)).ok_or_else(|| bad(C, "no settings folder for the price book: give `folder`"))?;
    let key = str_param(p, "key").map(str::trim).filter(|k| !k.is_empty() && k.len() <= 64).ok_or_else(|| bad(C, "`key` is required"))?.to_string();
    let f = |k: &str| p.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0);
    let pack_qty = f("pack_qty").filter(|q| *q > 0.0);
    let unit_price = f("unit_price")
        .or_else(|| f("pack_price").zip(pack_qty).map(|(pp, q)| pp / q))
        .ok_or_else(|| bad(C, "`unit_price`, or `pack_price` with `pack_qty`, is required"))?;
    let vendor =
        str_param(p, "vendor").map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| bad(C, "`vendor` (where the price is from) is required"))?;
    let mut prices = machine_prices(&dir);
    let held = load(Some(&dir.to_string_lossy())).prices.get(&key).cloned();
    let s = |k: &str, default: &str| str_param(p, k).map(str::trim).filter(|v| !v.is_empty()).map_or_else(|| default.to_string(), str::to_string);
    let base = held.clone().unwrap_or_default();
    let new = Price {
        key: key.clone(),
        description: s("description", &base.description),
        catalog: s("catalog", &base.catalog),
        unit: s("unit", if base.unit.is_empty() { "each" } else { &base.unit }),
        pack: pack_qty.map_or_else(|| s("pack", &base.pack), |q| format!("{q}")),
        pack_price: f("pack_price"),
        unit_price,
        source_type: s("source_type", "QUOTE"),
        vendor: vendor.to_string(),
        url: s("url", ""),
        date: s("date", &today()),
        note: s("note", ""),
        origin: "machine",
    };
    if super::estimate::parse_date(&new.date).is_none() {
        return Err(bad(C, "`date` is YYYY-MM-DD"));
    }
    if let Some(old) = &held {
        to_history(&dir, old, &today()).map_err(|e| bad(C, format!("history: {e}")))?;
    }
    prices.insert(key.clone(), new);
    write_prices(&dir, &prices).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    Ok(
        json!({"key": key, "unit_price": unit_price, "replaced": held.map(|h| json!({"unit_price": h.unit_price, "date": h.date, "vendor": h.vendor, "origin": h.origin})), "folder": dir.to_string_lossy()}),
    )
}

fn run_map(_s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "pricebook.map";
    let dir = machine_folder(folder_param(p)).ok_or_else(|| bad(C, "no settings folder for the price book: give `folder`"))?;
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
    if let Some(k) = keys.iter().find(|k| !book.prices.contains_key(*k)) {
        return Err(bad(C, format!("no price key {k} (pricebook.set it first)")));
    }
    let mut map = machine_map(&dir);
    map.retain(|m| !(m.item.eq_ignore_ascii_case(&item) && m.size.eq_ignore_ascii_case(&size)));
    let note = exclude.clone().or_else(|| str_param(p, "note").map(str::to_string)).unwrap_or_default();
    map.push(MapRow { item: item.clone(), size: size.clone(), exclude: exclude.is_some(), keys: keys.clone(), note, origin: "machine" });
    write_map(&dir, &map).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    Ok(json!({"item": item, "size": size, "keys": keys, "exclude": exclude}))
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

/// A table (rows of cells) as rows by column name; the header is the first row naming `want`.
fn table_rows(cells: &[Vec<String>], want: &str) -> Vec<BTreeMap<String, String>> {
    let Some(hi) = cells.iter().take(20).position(|r| r.iter().any(|c| header_key(c) == Some(want))) else { return Vec::new() };
    let cols: Vec<Option<&'static str>> = cells.get(hi).map(|r| r.iter().map(|c| header_key(c)).collect()).unwrap_or_default();
    cells
        .iter()
        .skip(hi + 1)
        .take(MAX_ROWS)
        .map(|r| {
            let mut m: BTreeMap<String, String> = BTreeMap::new();
            for (c, v) in cols.iter().zip(r) {
                if let Some(c) = c {
                    m.insert((*c).to_string(), v.trim().to_string());
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

fn run_import(_s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "pricebook.import";
    let path =
        str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(C, "`path` (a .xlsx or .csv price book) is required"))?;
    let dir = machine_folder(folder_param(p)).ok_or_else(|| bad(C, "no settings folder for the price book: give `folder`"))?;
    let bytes = crate::readfile::read(path).map_err(|e| bad(C, format!("{path}: {e}")))?;
    let (price_rows, map_rows) = if path.to_ascii_lowercase().ends_with(".xlsx") {
        let wb = super::table::xlsx::open(&bytes).map_err(|e| bad(C, format!("{path}: {e}")))?;
        let sheet = |name: &str| wb.cells(Some(name), None).unwrap_or_default();
        let mut prices = table_rows(&sheet("Price List"), "unit_price");
        if prices.is_empty() {
            prices = table_rows(&wb.cells(None, None).unwrap_or_default(), "unit_price");
        }
        (prices, table_rows(&sheet("Item Map"), "item"))
    } else {
        let text = String::from_utf8(bytes).map_err(|e| bad(C, format!("{path}: {e}")))?;
        let cells = super::estimate::csv_cells(&text);
        (table_rows(&cells, "unit_price"), table_rows(&cells, "item"))
    };
    let held = load(Some(&dir.to_string_lossy())).prices;
    let mut mine = machine_prices(&dir);
    let (mut added, mut updated, mut kept_newer, mut history) = (0usize, 0usize, 0usize, 0usize);
    let now = today();
    for r in &price_rows {
        let Some(mut np) = price_of(r, "machine") else { continue };
        if np.date.len() > 10 {
            np.date.truncate(10);
        }
        match held.get(&np.key) {
            Some(old) if !old.date.is_empty() && np.date < old.date => kept_newer += 1,
            Some(old) if old.unit_price == np.unit_price && old.date == np.date => {}
            Some(old) => {
                to_history(&dir, old, &now).map_err(|e| bad(C, format!("history: {e}")))?;
                history += 1;
                updated += 1;
                mine.insert(np.key.clone(), np);
            }
            None => {
                added += 1;
                mine.insert(np.key.clone(), np);
            }
        }
    }
    write_prices(&dir, &mine).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    let mut map = machine_map(&dir);
    let mut mapped = 0usize;
    for r in &map_rows {
        let Some(m) = map_of(r, "machine") else { continue };
        map.retain(|x| !(x.item.eq_ignore_ascii_case(&m.item) && x.size.eq_ignore_ascii_case(&m.size)));
        map.push(m);
        mapped += 1;
    }
    if mapped > 0 {
        write_map(&dir, &map).map_err(|e| bad(C, format!("{}: {e}", dir.display())))?;
    }
    Ok(
        json!({"prices": {"added": added, "updated": updated, "kept_newer": kept_newer}, "map": mapped, "history": history, "folder": dir.to_string_lossy()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_book_prices_common_material_with_its_source_and_date() {
        let b = load(Some("/nonexistent-pricebook-folder"));
        let emt = b.prices.get("emt34").expect("3/4 EMT");
        assert_eq!(emt.unit, "ft");
        assert!(emt.unit_price > 0.1 && emt.unit_price < 5.0);
        assert_eq!(emt.source_type, "WEB");
        assert!(!emt.url.is_empty() && !emt.date.is_empty());
        // Only public prices are shipped.
        assert!(b.prices.values().all(|p| p.source_type == "WEB"), "{:?}", b.prices.values().filter(|p| p.source_type != "WEB").collect::<Vec<_>>());
        match lookup(&b, "EMT conduit", "3/4\"", "ft") {
            Some(Found::Priced { material, parts, .. }) => {
                assert!((material - emt.unit_price).abs() < 1e-9);
                assert!(source_text(&parts).starts_with("emt34 "));
            }
            other => panic!("{other:?}"),
        }
        // Per foot, not per each.
        assert_eq!(lookup(&b, "EMT conduit", "3/4\"", "each"), None);
        assert_eq!(lookup(&b, "EMT conduit", "4\"", "ft"), None);
    }
}
