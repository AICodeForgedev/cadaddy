//! Price and manpower (Q6154 (d)): a takeoff's lines priced from a cost DB whose labor unit is
//! DOLLARS (man-hours = labor $ ÷ the job's manpower rate), else through the estimate's prices
//! (hours per unit there). A line no price matches is listed unpriced, never guessed.
//!
//! The cost DB is a private export beside the drawing, never in the repo: a CSV
//! (`item, size?, unit, material_unit, labor_unit`, labor in dollars per unit) whose `#` lines
//! carry what the DB reports of itself (`# price_date: 2026-10-01`, `# escalation: …`,
//! `# manpower_rate: 65`, `# manpower_rate_source: …`), or the same as JSON
//! (`{price_date, escalation, manpower_rate, manpower_rate_source, items: [...]}`).

use serde_json::{Value, json};

use super::super::*;
use crate::{Result, Session};

/// Rows one cost DB gives at most (hostile input).
const MAX_ROWS: usize = 200_000;
/// Characters one cost DB file gives at most.
const MAX_FILE: usize = 64 << 20;
/// Lines one `estimate.change` call takes.
const CHANGE_CHUNK: usize = 200;

/// One cost DB row: material and labor per unit, labor in dollars.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CostRow {
    pub item: String,
    pub size: String,
    pub unit: String,
    pub material: f64,
    pub labor_dollars: f64,
}

/// A cost DB and what it reports of itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CostDb {
    pub rows: Vec<CostRow>,
    pub price_date: String,
    pub escalation: String,
    /// Dollars per man-hour, and where the DB says so.
    pub manpower_rate: Option<f64>,
    pub manpower_rate_source: String,
    pub path: String,
}

fn num(s: &str) -> Option<f64> {
    s.trim().trim_matches('"').trim_start_matches('$').replace(',', "").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

/// One CSV line's cells: a cell that starts with a quote may hold commas (`""` a quote in it);
/// a quote elsewhere is an inch mark (`3/4"`).
fn cells(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' if quoted => quoted = false,
            '"' if cur.trim().is_empty() => {
                cur.clear();
                quoted = true;
            }
            ',' if !quoted => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// A cost DB from its text (`csv` or `json`).
pub(crate) fn parse_cost_db(text: &str, json_form: bool, cmd: &str) -> Result<CostDb> {
    let mut db = CostDb::default();
    if json_form {
        let v: Value = serde_json::from_str(text).map_err(|e| bad(cmd, format!("`cost_db` isn't JSON: {e}")))?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().chars().take(200).collect::<String>();
        db.price_date = s("price_date");
        db.escalation = s("escalation");
        db.manpower_rate = v.get("manpower_rate").and_then(Value::as_f64).filter(|r| r.is_finite() && *r > 0.0);
        db.manpower_rate_source = s("manpower_rate_source");
        for r in v.get("items").and_then(Value::as_array).into_iter().flatten().take(MAX_ROWS) {
            let t = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
            let n = |k: &str| r.get(k).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(num))).filter(|x| x.is_finite() && *x >= 0.0);
            if t("item").is_empty() {
                continue;
            }
            db.rows.push(CostRow {
                item: t("item"),
                size: t("size"),
                unit: t("unit"),
                material: n("material_unit").unwrap_or(0.0),
                labor_dollars: n("labor_unit").unwrap_or(0.0),
            });
        }
    } else {
        let mut head: Option<Vec<String>> = None;
        for l in text.lines().take(MAX_ROWS + 50) {
            let l = l.trim_start_matches('\u{feff}');
            if let Some(meta) = l.trim_start().strip_prefix('#') {
                if let Some((k, v)) = meta.split_once(':') {
                    let v: String = v.trim().chars().take(200).collect();
                    match k.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
                        "price_date" => db.price_date = v,
                        "escalation" => db.escalation = v,
                        "manpower_rate" => db.manpower_rate = num(&v).filter(|r| *r > 0.0),
                        "manpower_rate_source" => db.manpower_rate_source = v,
                        _ => {}
                    }
                }
                continue;
            }
            if l.trim().is_empty() {
                continue;
            }
            let f = cells(l);
            let Some(h) = &head else {
                head = Some(f.iter().map(|x| x.to_ascii_lowercase()).collect());
                continue;
            };
            let col = |names: &[&str]| h.iter().position(|x| names.contains(&x.as_str()));
            let (Some(ci), Some(cm), Some(cl)) =
                (col(&["item", "description", "name"]), col(&["material_unit", "material"]), col(&["labor_unit", "labor"]))
            else {
                return Err(bad(cmd, "`cost_db` needs columns item, material_unit and labor_unit (labor in dollars per unit)"));
            };
            let get = |i: Option<usize>| i.and_then(|i| f.get(i)).cloned().unwrap_or_default();
            let item = get(Some(ci));
            if item.is_empty() || db.rows.len() >= MAX_ROWS {
                continue;
            }
            db.rows.push(CostRow {
                item,
                size: get(col(&["size"])),
                unit: get(col(&["unit", "uom"])),
                material: num(&get(Some(cm))).unwrap_or(0.0),
                labor_dollars: num(&get(Some(cl))).unwrap_or(0.0),
            });
        }
    }
    Ok(db)
}

/// A cost DB from its file.
pub(crate) fn read_cost_db(path: &str, cmd: &str) -> Result<CostDb> {
    let bytes = crate::readfile::read(std::path::Path::new(path)).map_err(|e| bad(cmd, format!("`cost_db` {path}: {e}")))?;
    if bytes.len() > MAX_FILE {
        return Err(bad(cmd, format!("`cost_db` {path} is larger than {} MB", MAX_FILE >> 20)));
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut db = parse_cost_db(&text, path.to_ascii_lowercase().ends_with(".json"), cmd)?;
    db.path = path.to_string();
    Ok(db)
}

/// An item or size for matching: lower case, letters, digits and `/`, `-`.
pub(crate) fn norm(s: &str) -> String {
    s.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '/' || *c == '-').collect()
}

impl CostDb {
    /// The row for an item and size: the item's own name and its size (a row with no size takes
    /// any).
    pub(crate) fn find(&self, item: &str, size: &str) -> Option<&CostRow> {
        let (i, s) = (norm(item), norm(size));
        self.rows
            .iter()
            .find(|r| norm(&r.item) == i && !r.size.is_empty() && norm(&r.size) == s)
            .or_else(|| self.rows.iter().find(|r| norm(&r.item) == i && r.size.is_empty()))
    }
}

/// A takeoff line to price.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Line {
    pub kind: String,
    pub item: String,
    pub size: String,
    pub unit: String,
    pub qty: f64,
}

/// The manpower basis: dollars per man-hour and where it comes from; a crew.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Manpower {
    pub rate: Option<f64>,
    pub rate_source: String,
    pub crew_size: f64,
    pub hours_per_day: f64,
    pub crew_source: String,
}

impl Manpower {
    /// Crew-days for man-hours.
    pub(crate) fn crew_days(&self, man_hours: f64) -> f64 {
        let per_day = self.crew_size * self.hours_per_day;
        if per_day > 0.0 && man_hours.is_finite() { man_hours / per_day } else { 0.0 }
    }

    pub(crate) fn json(&self) -> Value {
        json!({
            "manpower_rate": self.rate, "manpower_rate_source": self.rate_source,
            "crew_size": self.crew_size, "hours_per_day": self.hours_per_day, "crew_source": self.crew_source,
        })
    }
}

/// Two sets of takeoff lines as one: the same kind, item, size and unit summed.
pub(crate) fn merge(lines: Vec<Line>, more: &[Line]) -> Vec<Line> {
    let mut by: std::collections::BTreeMap<(String, String, String, String), f64> = std::collections::BTreeMap::new();
    for l in lines.into_iter().chain(more.iter().cloned()) {
        *by.entry((l.kind, l.item, l.size, l.unit)).or_default() += l.qty;
    }
    by.into_iter().map(|((kind, item, size, unit), qty)| Line { kind, item, size, unit, qty: (qty * 100.0).round() / 100.0 }).collect()
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Lines priced: each with material, labor dollars, man-hours and its basis; the totals; what
/// no price matched.
pub(crate) fn price(s: &mut Session, lines: &[Line], db: Option<&CostDb>, mp: &Manpower) -> Value {
    let mut out: Vec<Value> = Vec::new();
    let mut unpriced: Vec<String> = Vec::new();
    let (mut tm, mut tl, mut th) = (0.0, 0.0, 0.0);
    let mut via_estimate: Vec<usize> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let mut v = json!({"kind": l.kind, "item": l.item, "size": l.size, "unit": l.unit, "qty": r2(l.qty)});
        match db {
            Some(db) => match db.find(&l.item, &l.size) {
                Some(r) => {
                    let (m, ld) = (r.material * l.qty, r.labor_dollars * l.qty);
                    let mh = mp.rate.map(|rt| ld / rt);
                    v["material"] = json!(r2(m));
                    v["labor_dollars"] = json!(r2(ld));
                    v["man_hours"] = mh.map_or(Value::Null, |h| json!((h * 1000.0).round() / 1000.0));
                    v["via"] = json!(format!(
                        "cost DB: {} {} ${}/{} material, ${}/{} labor (dollars)",
                        r.item,
                        r.size,
                        r.material,
                        unit_of(r),
                        r.labor_dollars,
                        unit_of(r)
                    ));
                    tm += m;
                    tl += ld;
                    th += mh.unwrap_or(0.0);
                }
                None => unpriced.push(format!("{} {}", l.item, l.size).trim().to_string()),
            },
            None => via_estimate.push(i),
        }
        out.push(v);
    }
    if !via_estimate.is_empty() {
        // The estimate's prices (the drawing's own, the price book, the starter book): hours per
        // unit there, dollars by the estimate's labor rate.
        let mut got: Vec<Value> = Vec::new();
        let want: Vec<Value> = via_estimate
            .iter()
            .filter_map(|i| lines.get(*i))
            .map(|l| json!({"item": l.item, "size": l.size, "unit": if l.unit == "ft" { "ft" } else { "each" }, "qty": l.qty}))
            .collect();
        for chunk in want.chunks(CHANGE_CHUNK) {
            let priced = s.execute("estimate.change", &json!({"lines": chunk})).ok();
            let mut part: Vec<Value> = priced.as_ref().and_then(|v| v["lines"].as_array().cloned()).unwrap_or_default();
            part.resize(chunk.len(), Value::Null);
            got.extend(part);
        }
        for (k, i) in via_estimate.iter().enumerate() {
            let (Some(v), Some(l)) = (out.get_mut(*i), lines.get(*i)) else { continue };
            let x = got.get(k).cloned().unwrap_or(Value::Null);
            if x["priced"].as_bool() == Some(true) {
                let (m, h, ld) =
                    (x["material"].as_f64().unwrap_or(0.0), x["labor_hours"].as_f64().unwrap_or(0.0), x["labor_cost"].as_f64().unwrap_or(0.0));
                v["material"] = json!(r2(m));
                v["labor_dollars"] = json!(r2(ld));
                v["man_hours"] = json!((h * 1000.0).round() / 1000.0);
                v["via"] = json!(format!("estimate: {} (labor hours per unit × the estimate's rate)", x["via"].as_str().unwrap_or_default()));
                tm += m;
                tl += ld;
                th += h;
            } else {
                unpriced.push(format!("{} {}", l.item, l.size).trim().to_string());
            }
        }
    }
    unpriced.sort();
    unpriced.dedup();
    let basis = match db {
        Some(db) => format!(
            "cost DB {}: material and labor per unit, labor in DOLLARS; man-hours = labor $ ÷ {}",
            db.path,
            mp.rate.map_or_else(|| "(no manpower rate: man-hours not computed)".to_string(), |r| format!("${r}/h ({})", mp.rate_source))
        ),
        None => "the estimate's prices (drawing, price book, starter book): labor hours per unit, dollars at the estimate's labor rate".to_string(),
    };
    json!({
        "lines": out, "unpriced": unpriced, "basis": basis,
        "material": r2(tm), "labor_dollars": r2(tl), "man_hours": r2(th), "crew_days": r2(mp.crew_days(th)), "cost": r2(tm + tl),
    })
}

fn unit_of(r: &CostRow) -> &str {
    if r.unit.is_empty() { "unit" } else { &r.unit }
}

/// The difference, design less alternative (what the alternative saves), when both are priced
/// in full; else none (the gaps say why).
pub(crate) fn saves(design: &Value, alt: &Value, mp: &Manpower) -> Value {
    let full = |v: &Value| v["unpriced"].as_array().is_some_and(Vec::is_empty);
    if !full(design) || !full(alt) {
        return Value::Null;
    }
    let n = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    let mh = n(design, "man_hours") - n(alt, "man_hours");
    json!({
        "material": r2(n(design, "material") - n(alt, "material")),
        "labor_dollars": r2(n(design, "labor_dollars") - n(alt, "labor_dollars")),
        "man_hours": r2(mh),
        "crew_days": r2(mp.crew_days(mh)),
        "cost": r2(n(design, "cost") - n(alt, "cost")),
    })
}

/// The rows of `db` the lines use: the private price book the job needs, as CSV (with the DB's
/// own `#` lines first).
pub(crate) fn book_csv(db: &CostDb, lines: &[Line]) -> (String, usize) {
    let mut csv = String::new();
    for (k, v) in [
        ("source", db.path.as_str()),
        ("price_date", db.price_date.as_str()),
        ("escalation", db.escalation.as_str()),
        ("manpower_rate_source", db.manpower_rate_source.as_str()),
    ] {
        if !v.is_empty() {
            csv.push_str(&format!("# {k}: {}\n", v.replace(['\n', '\r'], " ")));
        }
    }
    if let Some(r) = db.manpower_rate {
        csv.push_str(&format!("# manpower_rate: {r}\n"));
    }
    csv.push_str("item,size,unit,material_unit,labor_unit\n");
    let mut seen: Vec<&CostRow> = Vec::new();
    for l in lines {
        if let Some(r) = db.find(&l.item, &l.size)
            && !seen.contains(&r)
        {
            seen.push(r);
        }
    }
    for r in &seen {
        let cell = |t: &str| {
            let t = t.replace('"', "\"\"");
            let risky = t.starts_with(['=', '+', '-', '@']);
            if t.contains([',', '"', '\n']) || risky { format!("\"{}{t}\"", if risky { "'" } else { "" }) } else { t }
        };
        csv.push_str(&format!("{},{},{},{},{}\n", cell(&r.item), cell(&r.size), cell(&r.unit), r.material, r.labor_dollars));
    }
    (csv, seen.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cost_db_reads_its_rows_and_what_it_reports_of_itself() {
        let csv = "# price_date: 2026-09-30\n# escalation: 3% a year to the bid date\n# manpower_rate: 70\n# manpower_rate_source: the job's workbook\nitem,size,unit,material_unit,labor_unit\n\"EMT conduit\",\"3/4\"\"\",ft,0.80,2.10\nNM-B cable,12/2 w/G,ft,0.55,0.95\nCable staple,,ea,0.05,0.40\nEMT coupling,1/2\",ea,0.30,1.10\n";
        let db = parse_cost_db(csv, false, "t").unwrap();
        assert_eq!((db.price_date.as_str(), db.manpower_rate), ("2026-09-30", Some(70.0)));
        assert_eq!(db.escalation, "3% a year to the bid date");
        assert_eq!(db.rows.len(), 4);
        assert_eq!(db.rows[0].size, "3/4\"");
        assert_eq!(db.rows[3].size, "1/2\"", "an inch mark in an unquoted cell");
        assert!(db.find("emt conduit", "3/4\"").is_some());
        assert!(db.find("Cable staple", "anything").is_some(), "a row with no size takes any");
        assert!(db.find("EMT conduit", "1\"").is_none());
        let j = parse_cost_db(r##"{"price_date": "2026-10", "manpower_rate": 65, "items": [{"item": "MC cable", "size": "#12", "unit": "ft", "material_unit": 1.1, "labor_unit": "0.9"}]}"##, true, "t").unwrap();
        assert_eq!((j.rows.len(), j.manpower_rate), (1, Some(65.0)));
        assert!((j.rows[0].labor_dollars - 0.9).abs() < 1e-9);
        assert!(parse_cost_db("a,b\n1,2\n", false, "t").is_err(), "no item/material/labor columns");
        let (book, n) = book_csv(&db, &[Line { item: "NM-B cable".into(), size: "12/2 w/G".into(), ..Line::default() }]);
        assert_eq!(n, 1);
        assert!(book.contains("# manpower_rate: 70") && book.contains("NM-B cable,12/2 w/G,ft,0.55,0.95") && !book.contains("EMT"), "{book}");
    }

    #[test]
    fn labor_dollars_turn_into_man_hours_by_the_manpower_rate_and_crew_days_by_the_crew() {
        let mut s = Session::new();
        let db = parse_cost_db("# manpower_rate: 50\nitem,size,unit,material_unit,labor_unit\nMC cable,#12,ft,1.00,2.00\n", false, "t").unwrap();
        let mp = Manpower { rate: db.manpower_rate, rate_source: "cost DB".into(), crew_size: 2.0, hours_per_day: 8.0, crew_source: "test".into() };
        let lines = vec![
            Line { kind: "cable".into(), item: "MC cable".into(), size: "#12".into(), unit: "ft".into(), qty: 400.0 },
            Line { kind: "strap".into(), item: "Cable strap".into(), size: String::new(), unit: "ea".into(), qty: 10.0 },
        ];
        let v = price(&mut s, &lines, Some(&db), &mp);
        assert_eq!((v["material"].as_f64(), v["labor_dollars"].as_f64(), v["man_hours"].as_f64()), (Some(400.0), Some(800.0), Some(16.0)));
        assert_eq!(v["crew_days"].as_f64(), Some(1.0), "16 man-hours / (2 × 8 h)");
        assert_eq!(v["unpriced"], json!(["Cable strap"]), "never guessed");
        // Saves only when both ways are priced in full.
        assert!(saves(&v, &v, &mp).is_null());
        let full = price(&mut s, &lines[..1], Some(&db), &mp);
        let half = price(&mut s, &[Line { qty: 100.0, ..lines[0].clone() }], Some(&db), &mp);
        let sv = saves(&full, &half, &mp);
        assert_eq!((sv["cost"].as_f64(), sv["man_hours"].as_f64(), sv["crew_days"].as_f64()), (Some(900.0), Some(12.0), Some(0.75)));
    }

    #[test]
    fn hostile_cost_dbs_never_panic() {
        for t in
            ["", "#", "# manpower_rate: NaN\nitem,material,labor\n,,\nx,-1,inf\n", "\"unterminated,a\nitem,material,labor\n\"\"\"\n", "{}", "[1,2]"]
        {
            let _ = parse_cost_db(t, false, "t");
            let _ = parse_cost_db(t, true, "t");
        }
        let big = format!("item,material,labor\n{}", "x,1,1\n".repeat(MAX_ROWS + 10));
        assert_eq!(parse_cost_db(&big, false, "t").unwrap().rows.len(), MAX_ROWS);
    }
}
