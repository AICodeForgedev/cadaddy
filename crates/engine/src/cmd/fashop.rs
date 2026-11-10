//! Fire alarm shop drawings (Q6495): the approved shop drawings in the submittals, not the design
//! symbols, say where each notification appliance goes and what it is.
//!
//! A fire alarm contractor's shop drawings label every notification appliance with its address
//! (`P2.N3.07`: power supply or panel, notification circuit, device) and its candela (`15cd`),
//! and list the system's devices in a device legend (symbol, quantity, description, model,
//! mounting). This reads them:
//! - the device legend's rows, each with its drawn symbol and whether its symbol cell marks it
//!   weatherproof;
//! - each plan view (its title names its floor: `2nd Fl. FA Plan`), its appliances
//!   (each address with its candela beside it, at its symbol: the legend row whose drawn sample
//!   has its shape, else the most common row of its kind, said so) and its room numbers;
//! - the addresses any other page lists (the riser diagram).
//!
//! In a job ([`apply`]) each plan is laid onto its level by its room numbers (the rooms the
//! architecture's plans traced, numbered alike), and the appliances supersede the design
//! plans' notification devices on that level: a design device with a shop appliance near it
//! takes the appliance's place, kind, mounting, candela, circuit and address (its design place
//! and kind kept beside, with why); an appliance with none is added from the shop drawing; a
//! design device with none is left out as a review item. The legend's quantities are checked
//! against what was placed. Nothing here names a job: the words are rule data
//! (`wordrules.json` `fire_alarm.*`, `devices.legend_kind`).

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::{self, TextItem};
use serde_json::{Value, json};

use super::guided::{Shape, shape_of};
use super::readers::{FireAlarmFacts, ReadDevice};
use super::*;
use crate::{Result, Session};

const CMD: &str = "bim.fa_shop";
/// Files and pages read, at most.
const MAX_FILES: usize = 64;
const MAX_PAGES: u32 = 80;
/// The first pages that must name a fire alarm document (its cover, its title sheet).
const FIRST_PAGES: u32 = 3;
/// Appliances, legend rows, rooms per plan, texts looked at per page, at most.
const MAX_APPLIANCES: usize = 5_000;
const MAX_ROWS: usize = 200;
const MAX_ROOMS: usize = 2_000;
const MAX_TEXTS: usize = 50_000;
/// How far (label heights) a candela may sit from its address to be its.
const CANDELA_REACH_H: f64 = 3.0;
/// How far (label heights) an appliance's symbol may be from its label.
const SYMBOL_REACH_H: f64 = 6.0;
/// How far (label heights) a weatherproof mark may be from an appliance's label or symbol.
const WP_REACH_H: f64 = 4.0;
/// A legend symbol is at most this many label heights across.
const SYMBOL_MAX_H: f64 = 5.0;
/// How far apart (feet) a shop appliance and a design device may be and still be one device.
pub(crate) const PAIR_FT: f64 = 12.0;
/// The same for an initiating device (a detector is one to a room or a few feet of corridor).
const INIT_PAIR_FT: f64 = 6.0;
/// A room number's pair dropped from the plan's fit when farther than this (feet) off it.
const FIT_TOL_FT: f64 = 8.0;
/// Pairs the fit needs, and the most it may be off on average (feet).
const FIT_LEAST: usize = 3;
const FIT_RMS_FT: f64 = 5.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new(CMD, "Fire Alarm Shop Drawings", run).params(
        "{paths: [path] (submittal PDFs, or folders of them)} → {docs: [{file, legend: [{qty, description, model, mounting, kind, mount, weatherproof}], plans: [{page, title, level, appliances: [{address, circuit, candela, at, weatherproof, row, row_basis}], rooms}], listed}], unread: [{file, error}]}: reads the fire alarm shop drawings among them (each notification appliance's address, candela and symbol on each floor's plan, and the device legend); a guided job lays them onto the model, superseding the design plans' notification devices",
    )]
}

/// A row of a shop drawing's device legend.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LegendRow {
    pub qty: Option<u32>,
    pub description: String,
    pub model: String,
    pub mounting: String,
    /// Its symbol cell marks it weatherproof (or its words say outdoor).
    pub weatherproof: bool,
}

impl LegendRow {
    fn words(&self) -> String {
        format!("{} {}", self.description, self.mounting).to_ascii_uppercase()
    }
    /// The notification appliance it is (`STROBE`, `HORN/STROBE`), none for another device.
    pub fn appliance(&self) -> Option<&'static str> {
        let u = self.description.to_ascii_uppercase();
        cadaddy_mepfix::wordrules::first_rule("fire_alarm.notification_tags", |w| u.contains(w)).map(|r| r.value)
    }
    /// How it is mounted: `ceiling` or `wall`, when its row says.
    pub fn mount(&self) -> Option<&'static str> {
        let u = self.words();
        cadaddy_mepfix::wordrules::first_rule("fire_alarm.mount_words", |w| u.contains(w)).map(|r| r.value)
    }
    fn outdoor(&self) -> bool {
        let u = self.description.to_ascii_uppercase();
        self.weatherproof || cadaddy_mepfix::wordrules::words("fire_alarm.outdoor_words").iter().any(|w| u.contains(w))
    }
}

/// A notification appliance on a shop drawing's plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Appliance {
    /// `P2.N3.07`, and its circuit `P2.N3`.
    pub address: String,
    pub circuit: String,
    pub candela: Option<u32>,
    /// Where its symbol is (page units).
    pub at: Vec2,
    pub weatherproof: bool,
    /// Its legend row (index), and how it was told.
    pub row: Option<usize>,
    pub row_basis: String,
}

/// One plan view of a shop drawing.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ShopPlan {
    pub page: u32,
    pub title: String,
    /// The level its title names (`1`, `2`), as the classify stage names levels.
    pub level: String,
    pub appliances: Vec<Appliance>,
    /// The room numbers printed on it and where.
    pub rooms: Vec<(String, Vec2)>,
    /// The initiating devices its loop addresses label (`L7.42`: a detector, a pull station, a
    /// module), each at the symbol its label takes: address, loop, where.
    pub initiating: Vec<(String, String, Vec2)>,
}

/// A fire alarm shop drawing submittal as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FaShop {
    /// The file's name.
    pub doc: String,
    pub legend: Vec<LegendRow>,
    pub legend_page: Option<u32>,
    pub plans: Vec<ShopPlan>,
    /// Every appliance address any page lists (the plans and the riser diagram).
    pub listed: BTreeSet<String>,
}

impl FaShop {
    pub fn appliances(&self) -> usize {
        self.plans.iter().map(|p| p.appliances.len()).sum()
    }
    fn source(&self, page: u32) -> String {
        format!("{} p{page}", self.doc)
    }
}

/// `P2.N3.07` → (`P2.N3.07`, `P2.N3`): a panel or power supply, a circuit, a device number.
pub(crate) fn address_of(token: &str) -> Option<(String, String)> {
    let t = token.trim().trim_end_matches([',', ';']);
    let parts: Vec<&str> = t.split('.').collect();
    let [panel, circuit, device] = parts.as_slice() else { return None };
    let alnum = |s: &str, slash: bool| !s.is_empty() && s.len() <= 8 && s.chars().all(|c| c.is_ascii_alphanumeric() || (slash && c == '/'));
    let starts_letter = |s: &str| s.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    let ok = alnum(panel, false)
        && starts_letter(panel)
        && alnum(circuit, true)
        && starts_letter(circuit)
        && (1..=3).contains(&device.len())
        && device.chars().all(|c| c.is_ascii_digit());
    ok.then(|| (t.to_ascii_uppercase(), format!("{panel}.{circuit}").to_ascii_uppercase()))
}

/// `15cd`, `177 cd`, `75CD` → the candela.
pub(crate) fn candela_of(token: &str) -> Option<u32> {
    let u = token.trim().to_ascii_uppercase();
    let n = u.strip_suffix("CD")?.trim();
    let v: u32 = n.parse().ok()?;
    (1..=1_000).contains(&v).then_some(v)
}

/// The level a plan title names (`2nd Fl. FA Plan` → `1`): `Fl.` and `Flr` read as
/// floor.
pub(crate) fn level_of_title(title: &str) -> Option<String> {
    let words: Vec<String> = title
        .split_whitespace()
        .map(|w| {
            let bare = w.trim_end_matches('.').to_ascii_uppercase();
            if matches!(bare.as_str(), "FL" | "FLR") { "FLOOR".to_string() } else { w.to_string() }
        })
        .collect();
    cadaddy_mepfix::sheet_reader::detect_levels(&words.join(" ")).into_iter().next()
}

/// A level name's number (`Level 2`, `2`, `L2`): the digits in it.
fn level_digits(name: &str) -> String {
    name.chars().filter(char::is_ascii_digit).collect()
}

/// Whether a model level is the one a plan's title names.
pub(crate) fn same_level(model: &str, shop: &str) -> bool {
    model.trim().eq_ignore_ascii_case(shop.trim()) || (!level_digits(shop).is_empty() && level_digits(model) == level_digits(shop))
}

fn text_centre(t: &TextItem) -> Vec2 {
    // A line of text starts at its insertion point; its middle half its width along it.
    let w = 0.6 * t.height * t.value.chars().count() as f64;
    let (s, c) = t.rotation.sin_cos();
    Vec2::new(t.at.x + 0.5 * w * c - 0.5 * t.height * s, t.at.y + 0.5 * w * s + 0.5 * t.height * c)
}

/// The device legend on a page: its QTY | DESCRIPTION table (with MODEL, MOUNTING when it has
/// them), each row with its symbol cell's drawn shape. None when the page has no such table.
pub(crate) fn legend_of(d: &Drawing, texts: &[TextItem]) -> Option<(Vec<LegendRow>, BTreeMap<Shape, Vec<usize>>)> {
    let header = |words: &[&str]| -> Option<&TextItem> {
        texts.iter().find(|t| {
            let u = t.value.trim().to_ascii_uppercase();
            words.iter().any(|w| u == *w || u.starts_with(&format!("{w} ")))
        })
    };
    let qty = header(&["QTY", "QTY.", "QUANTITY"])?;
    let desc = texts.iter().find(|t| t.value.trim().eq_ignore_ascii_case("DESCRIPTION") && (t.at.y - qty.at.y).abs() <= qty.height.max(1e-9))?;
    let h = qty.height.max(1e-9);
    let on_row = |t: &&TextItem| (t.at.y - qty.at.y).abs() <= h;
    let symbol = texts.iter().filter(on_row).find(|t| t.value.trim().eq_ignore_ascii_case("SYMBOL"));
    let model = texts.iter().filter(on_row).find(|t| t.value.trim().to_ascii_uppercase().starts_with("MODEL"));
    let mounting = texts.iter().filter(on_row).find(|t| t.value.trim().to_ascii_uppercase().starts_with("MOUNTING"));
    let backbox = texts.iter().filter(on_row).find(|t| t.value.trim().to_ascii_uppercase().starts_with("BACKBOX"));
    // The columns by their headers' middles: each cell's text goes to the nearest.
    let mut cols: Vec<(&str, f64)> = vec![("qty", text_centre(qty).x), ("desc", text_centre(desc).x)];
    for (name, t) in [("symbol", symbol), ("model", model), ("mounting", mounting), ("backbox", backbox)] {
        if let Some(t) = t {
            cols.push((name, text_centre(t).x));
        }
    }
    cols.sort_by(|a, b| a.1.total_cmp(&b.1));
    let left = cols.first().map_or(qty.at.x, |c| c.1) - 6.0 * h;
    let right = cols.last().map_or(desc.at.x, |c| c.1) + 12.0 * h;
    let col_of = |x: f64| cols.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map(|c| c.0);
    let below: Vec<&TextItem> = texts
        .iter()
        .filter(|t| t.at.y < qty.at.y - 0.5 * h && t.at.x >= left && t.at.x <= right)
        .filter(|t| t.rotation.sin().abs() < 0.2)
        .take(MAX_TEXTS)
        .collect();
    // The rows: each entry of the QTY column, top down; a gap of more than six rows' heights ends
    // the table.
    let mut anchors: Vec<&TextItem> = below.iter().copied().filter(|t| col_of(text_centre(t).x) == Some("qty")).collect();
    anchors.sort_by(|a, b| b.at.y.total_cmp(&a.at.y));
    let mut rows_y: Vec<f64> = Vec::new();
    for a in anchors {
        let last = rows_y.last().copied().unwrap_or(qty.at.y);
        if last - a.at.y > 12.0 * h {
            break;
        }
        if rows_y.last().is_none_or(|y| (y - a.at.y).abs() > 0.8 * h) {
            rows_y.push(a.at.y);
        }
        if rows_y.len() >= MAX_ROWS {
            break;
        }
    }
    if rows_y.is_empty() {
        return None;
    }
    // Each row's band: halfway to its neighbours (the first from the header, the last as far
    // below as the one above it reaches).
    let bands: Vec<(f64, f64)> = rows_y
        .iter()
        .enumerate()
        .map(|(i, y)| {
            let above = if i == 0 { qty.at.y - 0.5 * h } else { (rows_y.get(i - 1).copied().unwrap_or(*y) + y) / 2.0 };
            let below = match rows_y.get(i + 1) {
                Some(n) => (y + n) / 2.0,
                None => y - (above - y).max(2.0 * h),
            };
            (below, above)
        })
        .collect();
    let symbol_right = cols.iter().find(|c| c.0 == "qty").map_or(qty.at.x, |c| c.1) - 0.8 * h;
    let mut rows: Vec<LegendRow> = Vec::new();
    let mut shapes: BTreeMap<Shape, Vec<usize>> = BTreeMap::new();
    let wp_marks = cadaddy_mepfix::wordrules::words("fire_alarm.weatherproof_marks");
    for (i, (lo, hi)) in bands.iter().enumerate() {
        let cell = |name: &str| {
            let mut ts: Vec<&TextItem> =
                below.iter().copied().filter(|t| t.at.y > *lo && t.at.y <= *hi && col_of(text_centre(t).x) == Some(name)).collect();
            ts.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
            ts.iter().map(|t| t.value.trim()).collect::<Vec<_>>().join(" ")
        };
        let q = cell("qty");
        let weatherproof = cell("symbol").split_whitespace().any(|w| wp_marks.iter().any(|m| m.eq_ignore_ascii_case(w)));
        rows.push(LegendRow {
            qty: q.trim().parse().ok(),
            description: cell("desc"),
            model: cell("model"),
            mounting: cell("mounting"),
            weatherproof,
        });
        // Its symbol: the linework left of its quantity, in its band.
        let drawn: Vec<&EntityKind> = d
            .model
            .iter()
            .filter(|e| matches!(e.kind, EntityKind::Line(_) | EntityKind::Circle(_) | EntityKind::Arc(_) | EntityKind::LwPolyline(_)))
            .filter_map(|e| {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                (!b.is_empty() && b.min.x >= left && b.max.x <= symbol_right && b.min.y > *lo && b.max.y < *hi).then_some(&e.kind)
            })
            .take(64)
            .collect();
        if drawn.len() >= 2
            && let Some(shape) = shape_of(drawn.into_iter())
        {
            shapes.entry(shape).or_default().push(i);
        }
    }
    Some((rows, shapes))
}

/// Each address on a page with the candela printed by it (the same line or next to it): the
/// address, its circuit, the candela, where the label is and its height.
fn labels(texts: &[TextItem]) -> Vec<(String, String, Option<u32>, Vec2, f64)> {
    let mut out: Vec<(String, String, Option<u32>, Vec2, f64)> = Vec::new();
    let candelas: Vec<(u32, Vec2)> =
        texts.iter().flat_map(|t| t.value.split_whitespace().filter_map(candela_of).map(|c| (c, text_centre(t))).collect::<Vec<_>>()).collect();
    for t in texts.iter().take(MAX_TEXTS) {
        let words: Vec<&str> = t.value.split_whitespace().collect();
        for (k, w) in words.iter().enumerate() {
            let Some((address, circuit)) = address_of(w) else { continue };
            let h = t.height.max(1e-9);
            let at = text_centre(t);
            let own = words.get(k + 1).and_then(|n| candela_of(n));
            let near = || {
                candelas
                    .iter()
                    .filter(|(_, p)| p.dist(at) <= CANDELA_REACH_H * h)
                    .min_by(|a, b| a.1.dist(at).total_cmp(&b.1.dist(at)))
                    .map(|(c, _)| *c)
            };
            if out.iter().any(|o| o.0 == address) || out.len() >= MAX_APPLIANCES {
                continue;
            }
            out.push((address, circuit, own.or_else(near), at, h));
        }
    }
    out
}

/// Read the shop drawing's imported pages (page number, the page as a drawing in sheet units).
/// None when they hold no notification appliance with its candela.
pub(crate) fn read_pages(doc: &str, pages: &[(u32, Drawing)]) -> Option<FaShop> {
    let mut shop = FaShop { doc: doc.to_string(), ..Default::default() };
    let mut legend_shapes: BTreeMap<Shape, Vec<usize>> = BTreeMap::new();
    let page_texts: Vec<Vec<TextItem>> = pages.iter().map(|(_, d)| sheet_facts::texts(d)).collect();
    for ((page, d), texts) in pages.iter().zip(&page_texts) {
        if shop.legend.is_empty()
            && let Some((rows, shapes)) = legend_of(d, texts)
            && rows.iter().any(|r| r.appliance().is_some())
        {
            shop.legend = rows;
            legend_shapes = shapes;
            shop.legend_page = Some(*page);
        }
    }
    // The legend's symbols by their rows (for the shape finder): rows drawing one symbol (an
    // indoor strobe and its weatherproof one, told apart by the WP mark) share it.
    let shape_rows: BTreeMap<Shape, (String, String)> = legend_shapes
        .iter()
        .filter_map(|(s, rows)| {
            // Only the appliances' rows: another device's sample is no appliance's symbol.
            let rows: Vec<String> =
                rows.iter().filter(|r| shop.legend.get(**r).is_some_and(|l| l.appliance().is_some())).map(usize::to_string).collect();
            (!rows.is_empty()).then(|| (s.clone(), (rows.join(","), String::new())))
        })
        .collect();
    let wp_marks = cadaddy_mepfix::wordrules::words("fire_alarm.weatherproof_marks");
    for ((page, d), texts) in pages.iter().zip(&page_texts) {
        let found = labels(texts);
        if found.is_empty() {
            continue;
        }
        shop.listed.extend(found.iter().filter(|l| l.2.is_some()).map(|l| l.0.clone()));
        // The page's plan views and the level each names; a page whose titles are drawn as
        // shapes (no text to read) by its blocks of linework, each block's level told later by
        // its room numbers.
        let mut views: Vec<(sheet_facts::ViewTitle, String)> =
            sheet_facts::view_titles(d, texts).into_iter().filter_map(|v| level_of_title(&v.title).map(|l| (v, l))).collect();
        if views.is_empty() {
            let regions = sheet_facts::view_regions(d);
            let labelled: Vec<[f64; 4]> = regions
                .iter()
                .filter(|(r, _)| found.iter().any(|l| l.2.is_some() && l.3.x >= r[0] && l.3.x <= r[2] && l.3.y >= r[1] && l.3.y <= r[3]))
                .map(|(r, _)| *r)
                .collect();
            views = labelled
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let title = format!("plan {} (untitled)", i + 1);
                    (sheet_facts::ViewTitle { title, region: Some(*r), at: [r[0], r[3]], ..Default::default() }, String::new())
                })
                .collect();
        }
        if views.is_empty() {
            continue;
        }
        let view_of = |p: Vec2| -> Option<usize> {
            let inside = |r: &[f64; 4]| p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3];
            // The smallest region holding it (a view inside a larger block of linework).
            let held = views
                .iter()
                .enumerate()
                .filter(|(_, (v, _))| v.region.as_ref().is_some_and(inside))
                .min_by(|a, b| {
                    let area = |v: &sheet_facts::ViewTitle| v.region.map_or(f64::INFINITY, |r| (r[2] - r[0]) * (r[3] - r[1]));
                    area(&a.1.0).total_cmp(&area(&b.1.0))
                })
                .map(|(i, _)| i);
            held.or_else(|| {
                views
                    .iter()
                    .enumerate()
                    .filter(|(_, (v, _))| v.at[1] >= p.y)
                    .min_by(|a, b| (a.1.0.at[1] - p.y).total_cmp(&(b.1.0.at[1] - p.y)))
                    .map(|(i, _)| i)
            })
        };
        let h = found.iter().map(|l| l.4).fold(f64::INFINITY, f64::min);
        let h = if h.is_finite() { h } else { 1.0 };
        let symbols = super::loosesym::find(d, 0, u64::MAX, &shape_rows, &|_, _| true, SYMBOL_MAX_H * h, &[]);
        // The device symbols (rings: circles, round closed outlines, one per centre), each taken
        // by the nearest device label, one each: an appliance's label doesn't take the detector
        // beside it, whose loop address is nearer it.
        let rings = rings(d, SYMBOL_MAX_H * h);
        let appliance_labels = found.iter().filter(|l| l.2.is_some()).count();
        let loop_labels: Vec<(String, Vec2)> = texts
            .iter()
            .filter_map(|t| t.value.split_whitespace().find(|w| loop_address(w)).map(|w| (w.trim().to_ascii_uppercase(), text_centre(t))))
            .take(MAX_APPLIANCES)
            .collect();
        let device_labels: Vec<Vec2> = found.iter().filter(|l| l.2.is_some()).map(|l| l.3).chain(loop_labels.iter().map(|l| l.1)).collect();
        let taken: BTreeMap<usize, Vec2> = super::archauthority::pair(&device_labels, &rings, SYMBOL_REACH_H * h, |_, _| true)
            .into_iter()
            .filter_map(|(i, j)| rings.get(j).map(|r| (i, *r)))
            .collect();
        let wps: Vec<Vec2> = texts.iter().filter(|t| wp_marks.iter().any(|m| t.value.trim().eq_ignore_ascii_case(m))).map(text_centre).collect();
        let mut plans: Vec<ShopPlan> =
            views.iter().map(|(v, l)| ShopPlan { page: *page, title: v.title.trim().to_string(), level: l.clone(), ..Default::default() }).collect();
        for (k, (address, circuit, candela, at, lh)) in found.into_iter().filter(|l| l.2.is_some()).enumerate() {
            let Some(vi) = view_of(at) else { continue };
            // Its symbol: the ring its label took, else the legend shape nearest its label; its
            // legend rows by that shape when one is there.
            let reach = SYMBOL_REACH_H * lh;
            let spot = taken.get(&k).copied();
            let near = spot.unwrap_or(at);
            let sym = symbols.iter().filter(|f| f.at.dist(near) <= reach).min_by(|a, b| a.at.dist(near).total_cmp(&b.at.dist(near)));
            let spot = spot.or(sym.map(|f| f.at)).unwrap_or(at);
            let rows: Vec<usize> = sym.map(|f| f.row.split(',').filter_map(|r| r.parse::<usize>().ok()).collect()).unwrap_or_default();
            let marked = wps.iter().any(|w| w.dist(spot) <= WP_REACH_H * lh);
            // Of the rows its symbol is, the one weatherproof as it is marked (else the most numerous).
            let row = {
                let of = |want: Option<bool>| {
                    rows.iter()
                        .copied()
                        .filter(|r| want.is_none_or(|w| shop.legend.get(*r).is_some_and(|l| l.outdoor() == w)))
                        .max_by_key(|r| shop.legend.get(*r).and_then(|l| l.qty).unwrap_or(0))
                };
                of(Some(marked)).or_else(|| of(None))
            };
            let weatherproof = marked || row.and_then(|r| shop.legend.get(r)).is_some_and(LegendRow::outdoor);
            let (row, row_basis) = match row {
                Some(r) => (Some(r), "its symbol is the legend row's".to_string()),
                None => match usual_row(&shop.legend, weatherproof) {
                    Some(r) => (
                        Some(r),
                        format!(
                            "its symbol matched no legend sample: the legend's most numerous {} notification appliance taken (inferred)",
                            if weatherproof { "weatherproof" } else { "indoor" }
                        ),
                    ),
                    None => (None, "no notification row in the legend".to_string()),
                },
            };
            if let Some(p) = plans.get_mut(vi) {
                p.appliances.push(Appliance { address, circuit, candela, at: spot, weatherproof, row, row_basis });
            }
        }
        // The room numbers on each plan (numbers and room tags a few characters long).
        for t in texts.iter().filter(|t| t.rotation.sin().abs() < 0.2) {
            let v = t.value.trim();
            if v.is_empty()
                || v.len() > 8
                || !v.chars().any(|c| c.is_ascii_digit())
                || v.contains(' ')
                || address_of(v).is_some()
                || candela_of(v).is_some()
                || loop_address(v)
            {
                continue;
            }
            if let Some(p) = view_of(text_centre(t)).and_then(|vi| plans.get_mut(vi))
                && p.rooms.len() < MAX_ROOMS
            {
                p.rooms.push((v.to_string(), text_centre(t)));
            }
        }
        // The initiating devices, each at the symbol its loop label took.
        for (k, (address, at)) in loop_labels.iter().enumerate() {
            let Some(spot) = taken.get(&(appliance_labels + k)).copied() else { continue };
            let loop_name = address.split('.').next().unwrap_or("").to_string();
            if let Some(p) = view_of(*at).and_then(|vi| plans.get_mut(vi))
                && !p.initiating.iter().any(|i| i.0 == *address)
            {
                p.initiating.push((address.clone(), loop_name, spot));
            }
        }
        shop.plans.extend(plans.into_iter().filter(|p| !p.appliances.is_empty()));
    }
    // A page listing every appliance the other pages' plans show is the system's riser diagram,
    // no floor plan.
    let pages_of: BTreeMap<u32, BTreeSet<String>> = shop.plans.iter().fold(BTreeMap::new(), |mut m, p| {
        m.entry(p.page).or_default().extend(p.appliances.iter().map(|a| a.address.clone()));
        m
    });
    let mut riser: BTreeSet<u32> = pages_of
        .iter()
        .filter(|(page, mine)| {
            let others: BTreeSet<&String> = pages_of.iter().filter(|(p, _)| p != page).flat_map(|(_, a)| a.iter()).collect();
            !others.is_empty() && others.iter().all(|a| mine.contains(*a))
        })
        .map(|(p, _)| *p)
        .collect();
    // Every page lists them all (one floor plan and its riser): the plan is the page with the
    // most room numbers.
    if riser.len() == pages_of.len() {
        let rooms = |page: u32| shop.plans.iter().filter(|p| p.page == page).map(|p| p.rooms.len()).sum::<usize>();
        if let Some(plan) = riser.iter().copied().max_by_key(|p| (rooms(*p), std::cmp::Reverse(*p))) {
            riser.remove(&plan);
        }
    }
    shop.plans.retain(|p| !riser.contains(&p.page));
    (shop.appliances() > 0).then_some(shop)
}

/// A loop (signalling line circuit) device's address: `L7.42`.
fn loop_address(token: &str) -> bool {
    let Some((a, b)) = token.trim().split_once('.') else { return false };
    let letters = a.chars().take_while(char::is_ascii_alphabetic).count();
    (1..=3).contains(&letters)
        && a.len() > letters
        && a.len() <= letters + 3
        && a.chars().skip(letters).all(|c| c.is_ascii_digit())
        && (1..=3).contains(&b.len())
        && b.chars().all(|c| c.is_ascii_digit())
}

/// The round symbols on a page no wider than `max_size`: circles, and closed outlines whose
/// corners all lie about one distance from their middle (a ring drawn as a filled outline);
/// rings about one centre are one.
fn rings(d: &Drawing, max_size: f64) -> Vec<Vec2> {
    let mut out: Vec<(Vec2, f64)> = Vec::new();
    let mut grid: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    if !(max_size.is_finite() && max_size > 0.0) {
        return Vec::new();
    }
    for e in d.model.iter() {
        if out.len() >= MAX_TEXTS {
            break;
        }
        let ring = match &e.kind {
            EntityKind::Circle(c) if c.radius > 0.0 && 2.0 * c.radius <= max_size => Some((c.center.xy(), c.radius)),
            EntityKind::LwPolyline(p) if p.vertices.len() >= 8 && p.vertices.len() <= 256 => {
                let n = p.vertices.len() as f64;
                let sum = p.vertices.iter().fold(Vec2::new(0.0, 0.0), |a, v| a + v.p);
                let c = Vec2::new(sum.x / n, sum.y / n);
                let (lo, hi) = p.vertices.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), v| (lo.min(v.p.dist(c)), hi.max(v.p.dist(c))));
                let closed = p.closed || p.vertices.first().zip(p.vertices.last()).is_some_and(|(a, b)| a.p.dist(b.p) <= 0.05 * hi);
                (closed && lo > 0.0 && hi <= 1.2 * lo && 2.0 * hi <= max_size).then_some((c, hi))
            }
            _ => None,
        };
        let Some((c, r)) = ring else { continue };
        let cell = |v: f64| (v / max_size).floor() as i64;
        let (cx, cy) = (cell(c.x), cell(c.y));
        let near = (cx - 1..=cx + 1)
            .flat_map(|x| (cy - 1..=cy + 1).map(move |y| (x, y)))
            .any(|k| grid.get(&k).is_some_and(|ix| ix.iter().any(|i| out.get(*i).is_some_and(|(o, or): &(Vec2, f64)| o.dist(c) <= 0.3 * or.max(r)))));
        if !near {
            grid.entry((cx, cy)).or_default().push(out.len());
            out.push((c, r));
        }
    }
    out.into_iter().map(|(c, _)| c).collect()
}

/// The legend row an appliance whose symbol matched none is taken as: the most numerous
/// notification row of its kind (weatherproof or not), else of any.
fn usual_row(legend: &[LegendRow], weatherproof: bool) -> Option<usize> {
    let best = |want: Option<bool>| {
        legend
            .iter()
            .enumerate()
            .filter(|(_, r)| r.appliance().is_some() && want.is_none_or(|w| r.outdoor() == w))
            .max_by_key(|(_, r)| r.qty.unwrap_or(0))
            .map(|(i, _)| i)
    };
    best(Some(weatherproof)).or_else(|| best(None))
}

/// Read one submittal PDF as a fire alarm shop drawing: None when it is none (its first pages
/// name no fire alarm document, or no page labels an appliance with its candela).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read_file(path: &str) -> std::result::Result<Option<FaShop>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let n = cadaddy_io::pdf_read::page_count(&bytes).map_err(|e| e.to_string())?.min(MAX_PAGES);
    let doc_words = cadaddy_mepfix::wordrules::words("fire_alarm.shop_document_words");
    let import = |page: u32| {
        let opts = cadaddy_io::pdf_read::PdfOptions {
            page,
            scale: Some(1.0),
            layout: false,
            markups: false,
            images: false,
            recover_text: false,
            ..Default::default()
        };
        cadaddy_io::pdf_read::read(&bytes, &opts).map(|imp| imp.drawing).map_err(|e| format!("page {page}: {e}"))
    };
    let mut pages: Vec<(u32, Drawing)> = Vec::new();
    let mut fire_alarm = false;
    for page in 1..=n {
        let d = import(page)?;
        if !fire_alarm {
            let up: String = sheet_facts::texts(&d).iter().map(|t| t.value.to_ascii_uppercase()).collect::<Vec<_>>().join(" ");
            fire_alarm = doc_words.iter().any(|w| up.contains(w));
            // A document whose first pages name no fire alarm system is none of these.
            if !fire_alarm && page >= FIRST_PAGES {
                return Ok(None);
            }
        }
        pages.push((page, d));
    }
    if !fire_alarm {
        return Ok(None);
    }
    let name = std::path::Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().to_string());
    Ok(read_pages(&name, &pages))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn read_file(_path: &str) -> std::result::Result<Option<FaShop>, String> {
    Err("reading submittal files is not available on the web".to_string())
}

/// The fire alarm shop drawings among some submittal files (PDFs or folders): those read, and
/// the files that couldn't be read with why.
pub(crate) fn read_files(paths: &[String]) -> (Vec<FaShop>, Vec<(String, String)>) {
    let mut out = Vec::new();
    let mut unread = Vec::new();
    for f in super::submittals::pdf_files(paths).into_iter().take(MAX_FILES) {
        if !f.to_ascii_lowercase().ends_with(".pdf") {
            continue;
        }
        match read_file(&f) {
            Ok(Some(shop)) => out.push(shop),
            Ok(None) => {}
            Err(e) => unread.push((f, e)),
        }
    }
    (out, unread)
}

fn shop_json(shop: &FaShop) -> Value {
    json!({
        "file": shop.doc,
        "legend_page": shop.legend_page,
        "legend": shop.legend.iter().map(|r| json!({
            "qty": r.qty, "description": r.description, "model": r.model, "mounting": r.mounting,
            "kind": r.appliance(), "mount": r.mount(), "weatherproof": r.outdoor(),
        })).collect::<Vec<_>>(),
        "plans": shop.plans.iter().map(|p| json!({
            "page": p.page, "title": p.title, "level": p.level, "rooms": p.rooms.len(),
            "appliances": p.appliances.iter().map(|a| json!({
                "address": a.address, "circuit": a.circuit, "candela": a.candela, "at": [a.at.x, a.at.y],
                "weatherproof": a.weatherproof, "row": a.row.and_then(|r| shop.legend.get(r)).map(|r| r.description.clone()), "row_basis": a.row_basis,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "appliances": shop.appliances(),
        "listed": shop.listed.len(),
    })
}

fn run(_s: &mut Session, p: &Value) -> Result<Value> {
    let paths: Vec<String> = match p.get("paths") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).take(MAX_FILES).collect(),
        Some(Value::String(one)) => vec![one.clone()],
        None | Some(Value::Null) => Vec::new(),
        Some(v) => return Err(bad(CMD, format!("`paths` is a list of paths, not {v}"))),
    };
    if paths.is_empty() {
        return Err(bad(CMD, "give the submittal PDFs, or a folder of them (`paths`)"));
    }
    let (shops, unread) = read_files(&paths);
    Ok(json!({
        "docs": shops.iter().map(shop_json).collect::<Vec<_>>(),
        "unread": unread.iter().map(|(f, e)| json!({"file": f, "error": e})).collect::<Vec<_>>(),
    }))
}

// ─── Onto the model ──────────────────────────────────────────────────────────

/// A similarity (scale, turn, shift) from a shop plan's page onto the model's plan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fit {
    a: f64,
    b: f64,
    tx: f64,
    ty: f64,
}

impl Fit {
    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2::new(self.a * p.x - self.b * p.y + self.tx, self.b * p.x + self.a * p.y + self.ty)
    }
    pub fn scale(&self) -> f64 {
        self.a.hypot(self.b)
    }
}

/// The least-squares similarity taking `from` onto `to`.
fn similarity(pairs: &[(Vec2, Vec2)]) -> Option<Fit> {
    let n = pairs.len() as f64;
    if pairs.len() < 2 {
        return None;
    }
    let (sp, sq) = pairs.iter().fold((Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0)), |(a, b), (p, q)| (a + *p, b + *q));
    let (cp, cq) = (Vec2::new(sp.x / n, sp.y / n), Vec2::new(sq.x / n, sq.y / n));
    let (mut dot, mut cross, mut norm) = (0.0, 0.0, 0.0);
    for (p, q) in pairs {
        let (u, v) = (*p - cp, *q - cq);
        dot += u.x * v.x + u.y * v.y;
        cross += u.x * v.y - u.y * v.x;
        norm += u.x * u.x + u.y * u.y;
    }
    if !(norm.is_finite() && norm > 0.0) {
        return None;
    }
    let (a, b) = (dot / norm, cross / norm);
    let fit = Fit { a, b, tx: cq.x - (a * cp.x - b * cp.y), ty: cq.y - (b * cp.x + a * cp.y) };
    (fit.scale().is_finite() && fit.scale() > 0.0).then_some(fit)
}

/// The fit of a plan's room numbers onto the model's rooms of its level (number → middle):
/// the worst pair dropped while one is farther than `tol` off, then kept when enough pairs
/// agree. The fit and the pairs it kept and its RMS (model units).
pub(crate) fn fit_rooms(plan: &[(String, Vec2)], model: &BTreeMap<String, Vec2>, tol: f64, rms_most: f64) -> Option<(Fit, usize, f64)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (n, _) in plan {
        *counts.entry(n.as_str()).or_default() += 1;
    }
    let mut pairs: Vec<(Vec2, Vec2)> =
        plan.iter().filter(|(n, _)| counts.get(n.as_str()) == Some(&1)).filter_map(|(n, p)| model.get(n).map(|q| (*p, *q))).take(MAX_ROOMS).collect();
    loop {
        let fit = similarity(&pairs)?;
        let worst = pairs.iter().enumerate().map(|(i, (p, q))| (i, fit.apply(*p).dist(*q))).max_by(|a, b| a.1.total_cmp(&b.1));
        match worst {
            Some((i, d)) if d > tol && pairs.len() > FIT_LEAST => {
                pairs.remove(i);
            }
            _ => {
                let rms = (pairs.iter().map(|(p, q)| fit.apply(*p).dist(*q).powi(2)).sum::<f64>() / pairs.len().max(1) as f64).sqrt();
                return (pairs.len() >= FIT_LEAST && rms <= rms_most).then_some((fit, pairs.len(), rms));
            }
        }
    }
}

/// A level's rooms by number (a number two rooms carry is none).
fn level_rooms(rooms: &[ModelRoom], level: &str) -> BTreeMap<String, Vec2> {
    let mut m: BTreeMap<String, Vec2> = BTreeMap::new();
    let mut twice: BTreeSet<String> = BTreeSet::new();
    for r in rooms.iter().filter(|r| r.level == level && !r.number.trim().is_empty()) {
        let n = r.number.trim().to_string();
        if m.insert(n.clone(), r.at).is_some() {
            twice.insert(n);
        }
    }
    m.retain(|n, _| !twice.contains(n));
    m
}

/// A model room with its number, level and middle.
pub(crate) struct ModelRoom {
    pub number: String,
    pub level: String,
    pub at: Vec2,
}

/// What [`apply`] did.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub(crate) struct Applied {
    pub docs: Vec<String>,
    /// Plans laid onto their levels: `doc p4 2nd Fl. …: level 2, 31 rooms fit (0.9 ft)`.
    pub plans: Vec<String>,
    /// Plans that couldn't be: why.
    pub not_fit: Vec<String>,
    /// Design devices the shop drawing's appliance took the place of; appliances added; design
    /// devices left out (no appliance near them: review items).
    pub superseded: usize,
    pub added: usize,
    pub left_out: Vec<(String, String)>,
    /// The legend's quantity against what was placed or read, where they differ.
    pub quantities: Vec<String>,
    pub by_kind: BTreeMap<String, usize>,
    /// The design's initiating devices that took a shop drawing's loop address (and its spot),
    /// and the loop-labelled symbols no design device was near.
    pub addressed: usize,
    pub initiating_unmatched: usize,
}

/// Whether a read device is a notification appliance (`devices.legend_kind`): its mark, kind
/// and legend row read together.
pub(crate) fn notification(dv: &ReadDevice) -> bool {
    let words = format!("{} {} {}", dv.tag, dv.kind, dv.kind_source);
    super::guided::legend_kind(&words).is_some_and(|(k, _)| k == "fire_alarm_notification") || dv.kind == "fire_alarm_notification"
}

/// Lay the shop drawings' plans onto the model and let their appliances supersede the design
/// plans' notification devices on those levels (see the module's doc). `upf`: model units per
/// foot. The design devices left out are returned by handle with why (for the job's ledger).
pub(crate) fn apply(shops: &[FaShop], rooms: &[ModelRoom], devices: &mut Vec<ReadDevice>, upf: f64) -> Applied {
    let mut out = Applied::default();
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    for shop in shops {
        out.docs.push(shop.doc.clone());
        let mut placed_by_row: BTreeMap<usize, usize> = BTreeMap::new();
        let mut covered: BTreeSet<String> = BTreeSet::new();
        for plan in &shop.plans {
            let label = format!("{} p{} {}", shop.doc, plan.page, plan.title);
            // The model's levels it may be: the one its title names, else each (its room numbers
            // tell which).
            let mut levels: Vec<String> = Vec::new();
            for r in rooms.iter().filter(|r| plan.level.is_empty() || same_level(&r.level, &plan.level)) {
                if !levels.contains(&r.level) {
                    levels.push(r.level.clone());
                }
            }
            if levels.is_empty() {
                out.not_fit.push(format!("{label}: no level {} among the model's rooms", plan.level));
                continue;
            }
            let mut best: Option<(String, Fit, usize, f64)> = None;
            let mut printed = 0usize;
            for level in &levels {
                let model = level_rooms(rooms, level);
                printed = printed.max(plan.rooms.iter().filter(|(r, _)| model.contains_key(r)).count());
                if let Some((fit, n, rms)) = fit_rooms(&plan.rooms, &model, FIT_TOL_FT * upf, FIT_RMS_FT * upf)
                    && best.as_ref().is_none_or(|b| n > b.2 || (n == b.2 && rms < b.3))
                {
                    best = Some((level.clone(), fit, n, rms));
                }
            }
            let Some((level, fit, n, rms)) = best else {
                out.not_fit.push(format!(
                    "{label}: its room numbers don't line up with {} rooms ({} numbers printed, at most {printed} of them a level's)",
                    if plan.level.is_empty() { "any level's".to_string() } else { format!("level {}'s", plan.level) },
                    plan.rooms.len()
                ));
                continue;
            };
            covered.insert(level.clone());
            out.plans.push(format!(
                "{label}: level {level}{}, {n} rooms fit ({:.1} ft)",
                if plan.level.is_empty() { " (by its room numbers: its title isn't text)" } else { "" },
                rms / upf
            ));
            // The design's notification devices on the level, and the shop's appliances there.
            let design: Vec<usize> = devices.iter().enumerate().filter(|(_, dv)| dv.level == level && notification(dv)).map(|(i, _)| i).collect();
            let design_at: Vec<Vec2> = design.iter().filter_map(|i| devices.get(*i)).map(|dv| Vec2::new(dv.at[0], dv.at[1])).collect();
            let shop_at: Vec<Vec2> = plan.appliances.iter().map(|a| fit.apply(a.at)).collect();
            let pairs = super::archauthority::pair(&shop_at, &design_at, PAIR_FT * upf, |_, _| true);
            let paired_design: BTreeSet<usize> = pairs.iter().filter_map(|(_, j)| design.get(*j).copied()).collect();
            let mut by_shop: BTreeMap<usize, usize> = pairs.iter().filter_map(|(i, j)| design.get(*j).map(|d| (*i, *d))).collect();
            let mut added: Vec<ReadDevice> = Vec::new();
            for (i, (a, at)) in plan.appliances.iter().zip(&shop_at).enumerate() {
                let row = a.row.and_then(|r| shop.legend.get(r));
                if let Some(r) = a.row {
                    *placed_by_row.entry(r).or_default() += 1;
                }
                let tag = row.and_then(LegendRow::appliance).unwrap_or("STROBE");
                let facts = FireAlarmFacts {
                    address: a.address.clone(),
                    circuit: a.circuit.clone(),
                    candela: a.candela,
                    mount: row.and_then(LegendRow::mount).unwrap_or_default().to_string(),
                    model: row.map(|r| r.model.clone()).unwrap_or_default(),
                    weatherproof: a.weatherproof,
                    source: format!("{} ({})", shop.source(plan.page), plan.title),
                    row: row.map(|r| r.description.clone()).unwrap_or_default(),
                    row_basis: a.row_basis.clone(),
                    design: String::new(),
                    loop_device: false,
                };
                let kind_source = format!(
                    "{} device legend: {}",
                    shop.source(shop.legend_page.unwrap_or(plan.page)),
                    row.map_or("(no legend row)", |r| r.description.as_str())
                );
                let ifc = row.and_then(|r| super::guided::legend_kind(&r.description)).map(|k| k.1).unwrap_or("IfcAlarm");
                match by_shop.remove(&i).and_then(|d| devices.get_mut(d)) {
                    Some(dv) => {
                        let was = Vec2::new(dv.at[0], dv.at[1]);
                        let moved = was.dist(*at) / upf;
                        let mut facts = facts;
                        facts.design = format!(
                            "the design plan's {} ({}{}) at [{:.1}, {:.1}] ft, {moved:.1} ft away",
                            dv.tag,
                            if dv.sheet.is_empty() { "?" } else { &dv.sheet },
                            if dv.kind_source.is_empty() { String::new() } else { format!("; {}", dv.kind_source) },
                            was.x / upf,
                            was.y / upf
                        );
                        if moved > 0.05 {
                            dv.drawn_at = Some([was.x, was.y]);
                            dv.moved = format!("at its appliance {} on the approved fire alarm shop drawing ({})", a.address, facts.source);
                        }
                        dv.at = [at.x, at.y];
                        dv.tag = tag.to_string();
                        dv.kind = "fire alarm device".to_string();
                        dv.kind_source = kind_source;
                        dv.ifc_class = ifc.to_string();
                        dv.circuit = a.circuit.clone();
                        dv.aff_in = None;
                        dv.fire_alarm = Some(facts);
                        out.superseded += 1;
                    }
                    None => {
                        added.push(ReadDevice {
                            tag: tag.to_string(),
                            at: [at.x, at.y],
                            sheet: shop.source(plan.page),
                            level: level.clone(),
                            circuit: a.circuit.clone(),
                            kind: "fire alarm device".to_string(),
                            kind_source,
                            ifc_class: ifc.to_string(),
                            fire_alarm: Some(facts),
                            ..Default::default()
                        });
                        out.added += 1;
                    }
                }
                *out.by_kind.entry(tag.to_string()).or_default() += 1;
            }
            // The design's devices the shop drawing shows nothing near: left out, for review.
            let gone: BTreeSet<usize> = design.iter().copied().filter(|i| !paired_design.contains(i)).collect();
            for i in &gone {
                if let Some(dv) = devices.get(*i) {
                    out.left_out.push((
                        dv.handle.clone(),
                        format!(
                            "the design plan's {} on {} at [{:.1}, {:.1}] ft: the approved fire alarm shop drawing ({label}) supersedes the design's notification devices on level {level} and shows none within {PAIR_FT} ft; left out, to be reviewed",
                            dv.tag,
                            if dv.sheet.is_empty() { "?" } else { &dv.sheet },
                            dv.at[0] / upf,
                            dv.at[1] / upf
                        ),
                    ));
                }
            }
            let mut k = 0usize;
            devices.retain(|_| {
                let keep = !gone.contains(&k);
                k += 1;
                keep
            });
            devices.extend(added);
            // The design's initiating devices (detectors, pull stations) take the loop address
            // and spot of the labelled symbol near them.
            let init: Vec<usize> = devices
                .iter()
                .enumerate()
                .filter(|(_, dv)| dv.level == level && dv.fire_alarm.is_none() && dv.kind == "fire alarm device" && !notification(dv))
                .map(|(i, _)| i)
                .collect();
            let init_at: Vec<Vec2> = init.iter().filter_map(|i| devices.get(*i)).map(|dv| Vec2::new(dv.at[0], dv.at[1])).collect();
            let shop_init: Vec<Vec2> = plan.initiating.iter().map(|(_, _, p)| fit.apply(*p)).collect();
            let pairs = super::archauthority::pair(&shop_init, &init_at, INIT_PAIR_FT * upf, |_, _| true);
            out.initiating_unmatched += plan.initiating.len().saturating_sub(pairs.len());
            for (i, j) in pairs {
                let (Some((address, lp, _)), Some(at), Some(dv)) =
                    (plan.initiating.get(i), shop_init.get(i), init.get(j).and_then(|d| devices.get_mut(*d)))
                else {
                    continue;
                };
                let was = Vec2::new(dv.at[0], dv.at[1]);
                let moved = was.dist(*at) / upf;
                let source = format!("{} ({})", shop.source(plan.page), plan.title);
                if moved > 0.05 {
                    dv.drawn_at = Some([was.x, was.y]);
                    dv.moved = format!("at its symbol {address} on the approved fire alarm shop drawing ({source})");
                }
                dv.at = [at.x, at.y];
                dv.fire_alarm = Some(FireAlarmFacts {
                    address: address.clone(),
                    circuit: lp.clone(),
                    loop_device: true,
                    source,
                    design: format!("the design plan's {} at [{:.1}, {:.1}] ft, {moved:.1} ft away", dv.tag, was.x / upf, was.y / upf),
                    ..Default::default()
                });
                out.addressed += 1;
            }
        }
        if covered.is_empty() {
            continue;
        }
        // The legend's quantities against what was placed (its appliances) and read (its other
        // devices, the design plans' on every level).
        for (i, r) in shop.legend.iter().enumerate() {
            let Some(qty) = r.qty else { continue };
            if r.appliance().is_some() {
                let n = placed_by_row.get(&i).copied().unwrap_or(0);
                if n != qty as usize {
                    out.quantities.push(format!("{}: the legend lists {qty}, its plans show {n}", r.description));
                }
                continue;
            }
            let Some((kind, _)) = super::guided::legend_kind(&r.description) else { continue };
            if !cadaddy_mepfix::wordrules::words("fire_alarm.counted_kinds").contains(&kind) {
                continue;
            }
            let n = devices
                .iter()
                .filter(|dv| {
                    dv.fire_alarm.is_none()
                        && super::guided::legend_kind(&format!("{} {} {}", dv.tag, dv.kind, dv.kind_source)).is_some_and(|(k, _)| k == kind)
                })
                .count();
            if n != qty as usize {
                out.quantities.push(format!("{}: the legend lists {qty}, the design plans show {n}", r.description));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Circle, Entity, Line, Text};
    use cadaddy_geom::Vec3;

    fn push(d: &mut Drawing, k: EntityKind) {
        let h = cadaddy_doc::Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        d.model.push(Entity::new(h, k));
    }
    fn text(d: &mut Drawing, at: [f64; 2], h: f64, v: &str) {
        push(
            d,
            EntityKind::Text(Text {
                insert: Vec3::new(at[0], at[1], 0.0),
                align_pt: None,
                height: h,
                value: v.into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: Default::default(),
                valign: Default::default(),
            }),
        );
    }
    fn line(d: &mut Drawing, a: [f64; 2], b: [f64; 2]) {
        push(d, EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }));
    }
    fn circle(d: &mut Drawing, c: [f64; 2], r: f64) {
        push(d, EntityKind::Circle(Circle { center: Vec3::new(c[0], c[1], 0.0), radius: r }));
    }
    /// A made-up strobe: a circle with an X in it.
    fn strobe(d: &mut Drawing, c: [f64; 2], r: f64) {
        circle(d, c, r);
        let k = r * std::f64::consts::FRAC_1_SQRT_2;
        line(d, [c[0] - k, c[1] - k], [c[0] + k, c[1] + k]);
        line(d, [c[0] - k, c[1] + k], [c[0] + k, c[1] - k]);
    }
    /// A made-up horn/strobe: the strobe with a horn's box to its left.
    fn horn_strobe(d: &mut Drawing, c: [f64; 2], r: f64) {
        strobe(d, c, r);
        let x = c[0] - r;
        line(d, [x, c[1] - 0.5 * r], [x - r, c[1] - 0.5 * r]);
        line(d, [x - r, c[1] - 0.5 * r], [x - r, c[1] + 0.5 * r]);
        line(d, [x - r, c[1] + 0.5 * r], [x, c[1] + 0.5 * r]);
    }

    /// The legend page: SYMBOL | QTY | DESCRIPTION | MODEL # | MOUNTING, three appliance rows
    /// and a pull station's.
    fn legend_page() -> Drawing {
        let mut d = Drawing::new_imperial();
        let h = 0.1;
        for (x, w) in [(1.0, "SYMBOL"), (2.0, "QTY"), (3.0, "DESCRIPTION"), (6.0, "MODEL #"), (8.0, "MOUNTING")] {
            text(&mut d, [x, 20.0], h, w);
        }
        let rows: [(&str, &str, &str, &str); 4] = [
            ("4", "ACME CEILING MOUNT STROBE", "ZX-1", "CENTER OF TILE"),
            ("1", "ACME CEILING MOUNT HORN STROBE", "ZX-2", "CENTER OF TILE"),
            ("1", "ACME OUTDOOR WALL MOUNT STROBE", "ZX-3", "80\" AFF"),
            ("2", "ACME DUAL ACTION PULL STATION", "ZP-1", "48\" AFF"),
        ];
        for (i, (q, desc, model, mount)) in rows.iter().enumerate() {
            let y = 19.4 - 0.6 * i as f64;
            text(&mut d, [2.0, y], h, q);
            text(&mut d, [3.0, y + 0.08], h, desc);
            text(&mut d, [6.0, y], h, model);
            text(&mut d, [8.0, y], h, mount);
            let c = [1.2, y + 0.05];
            match i {
                0 => strobe(&mut d, c, 0.12),
                1 => horn_strobe(&mut d, c, 0.12),
                2 => {
                    strobe(&mut d, c, 0.12);
                    text(&mut d, [0.8, y + 0.15], 0.06, "WP");
                }
                _ => {
                    line(&mut d, [c[0] - 0.1, c[1] - 0.1], [c[0] + 0.1, c[1] - 0.1]);
                    line(&mut d, [c[0] - 0.1, c[1] - 0.1], [c[0], c[1] + 0.1]);
                }
            }
        }
        d
    }

    /// A plan page at 1/8" = 1': room numbers R-11..R-14 and five appliances.
    fn plan_page() -> Drawing {
        let mut d = Drawing::new_imperial();
        text(&mut d, [10.0, 22.0], 0.3, "1st Fl. FA Plan");
        text(&mut d, [10.0, 21.6], 0.2, "Scale: 1/8\" = 1'");
        // The plan's outline (its view region).
        for (a, b) in [([2.0, 4.0], [30.0, 4.0]), ([30.0, 4.0], [30.0, 20.0]), ([30.0, 20.0], [2.0, 20.0]), ([2.0, 20.0], [2.0, 4.0])] {
            line(&mut d, a, b);
        }
        for (i, n) in ["R-11", "R-12", "R-13", "R-14"].iter().enumerate() {
            text(&mut d, [4.0 + 6.0 * i as f64, 8.0], 0.1, n);
        }
        let appliances: [([f64; 2], &str, &str, u8); 5] = [
            ([4.5, 12.0], "BP7.N1.01", "15cd", 0),
            ([10.5, 12.0], "BP7.N1.02", "15cd", 0),
            ([16.5, 12.0], "BP7.N1.03", "75cd", 1),
            ([22.5, 12.0], "BP7.N2.01", "177cd", 0),
            ([28.5, 12.0], "BP7.N2.02", "30cd", 2),
        ];
        // A detector in room R-12 with its loop address.
        circle(&mut d, [8.0, 14.0], 0.12);
        text(&mut d, [8.2, 14.0], 0.1, "L7.42");
        for (c, addr, cd, kind) in appliances {
            match kind {
                1 => horn_strobe(&mut d, c, 0.12),
                _ => strobe(&mut d, c, 0.12),
            }
            if kind == 2 {
                text(&mut d, [c[0] - 0.35, c[1] + 0.1], 0.06, "WP");
            }
            text(&mut d, [c[0] + 0.25, c[1] + 0.05], 0.1, addr);
            text(&mut d, [c[0] + 0.25, c[1] - 0.12], 0.1, cd);
        }
        d
    }

    #[test]
    fn addresses_and_candelas_are_read_in_the_shop_drawings_format() {
        assert_eq!(address_of("BP7.N1.03"), Some(("BP7.N1.03".to_string(), "BP7.N1".to_string())));
        assert_eq!(address_of("CP.I/O2.04"), Some(("CP.I/O2.04".to_string(), "CP.I/O2".to_string())));
        assert_eq!(address_of("L7.42"), None, "an initiating device's loop address has two parts");
        assert_eq!(address_of("1.2.3"), None);
        assert_eq!(candela_of("15cd"), Some(15));
        assert_eq!(candela_of("177CD"), Some(177));
        assert_eq!(candela_of("cd"), None);
        assert_eq!(level_of_title("1st Fl. FA Plan").as_deref(), Some("1"));
        assert_eq!(level_of_title("Third Floor Fire Alarm Plan").as_deref(), Some("3"));
        assert!(same_level("Level 2", "2") && !same_level("Level 12", "2"));
    }

    #[test]
    fn the_legend_rows_and_each_appliance_by_its_symbol_are_read() {
        let shop = read_pages("Sample FA Shop.pdf", &[(3, legend_page()), (4, plan_page())]).unwrap_or_else(|| panic!("a shop drawing"));
        assert_eq!(shop.legend.len(), 4, "{:#?}", shop.legend);
        let first = shop.legend.first().unwrap_or_else(|| panic!("a row"));
        assert_eq!((first.qty, first.appliance(), first.mount(), first.model.as_str()), (Some(4), Some("STROBE"), Some("ceiling"), "ZX-1"));
        assert_eq!(shop.legend.get(1).and_then(LegendRow::appliance), Some("HORN/STROBE"));
        assert!(shop.legend.get(2).is_some_and(|r| r.outdoor() && r.mount() == Some("wall")));
        assert_eq!(shop.legend.get(3).and_then(LegendRow::appliance), None, "a pull station is no appliance");
        assert_eq!(shop.plans.len(), 1);
        let plan = shop.plans.first().unwrap_or_else(|| panic!("a plan"));
        assert_eq!((plan.level.as_str(), plan.appliances.len(), plan.rooms.len()), ("1", 5, 4), "{plan:#?}");
        let by: BTreeMap<&str, (Option<u32>, Option<usize>, bool)> =
            plan.appliances.iter().map(|a| (a.address.as_str(), (a.candela, a.row, a.weatherproof))).collect();
        assert_eq!(by.get("BP7.N1.01"), Some(&(Some(15), Some(0), false)));
        assert_eq!(by.get("BP7.N1.03"), Some(&(Some(75), Some(1), false)), "the horn/strobe by its symbol");
        assert_eq!(by.get("BP7.N2.01"), Some(&(Some(177), Some(0), false)));
        assert_eq!(by.get("BP7.N2.02").map(|v| v.2), Some(true), "the WP mark by it");
        // Each at its symbol, not its label.
        let a = plan.appliances.iter().find(|a| a.address == "BP7.N1.01").unwrap_or_else(|| panic!("BP7.N1.01"));
        assert!(a.at.dist(Vec2::new(4.5, 12.0)) < 0.05, "{a:?}");
    }

    /// The finding's case (Q6495): an electrical plan with two wall horn/strobes and a shop
    /// drawing with five ceiling strobes labelled with their circuits and candela. Before the shop
    /// drawings were read, the model kept the two design devices, no candela, no circuit.
    #[test]
    fn the_shop_drawings_appliances_supersede_the_design_symbols_with_candela_and_circuit() {
        let shop = read_pages("Sample FA Shop.pdf", &[(3, legend_page()), (4, plan_page())]).unwrap_or_else(|| panic!("a shop drawing"));
        // The model's plan in inches, 1/8" = 1': a sheet inch is 96 model inches, the plan sits
        // elsewhere and is turned a quarter.
        let to_model = |p: Vec2| Vec2::new(5000.0 - 96.0 * p.y, 200.0 + 96.0 * p.x);
        let rooms: Vec<ModelRoom> = ["R-11", "R-12", "R-13", "R-14"]
            .iter()
            .enumerate()
            .map(|(i, n)| ModelRoom { number: n.to_string(), level: "Level 1".into(), at: to_model(Vec2::new(4.09 + 6.0 * i as f64, 8.05)) })
            .chain(std::iter::once(ModelRoom { number: "R-21".into(), level: "Level 2".into(), at: Vec2::new(0.0, 0.0) }))
            .collect();
        let design = |tag: &str, p: Vec2, h: &str| ReadDevice {
            tag: tag.into(),
            at: [p.x, p.y],
            sheet: "E-901".into(),
            level: "Level 1".into(),
            handle: h.into(),
            kind: "fire alarm device".into(),
            kind_source: "E-901 legend: COMBINATION HORN & STROBE".into(),
            aff_in: Some(80.0),
            ..Default::default()
        };
        let mut devices = vec![
            // Near the first appliance (3 ft off), and one in a corridor the shop drawing has nothing near.
            design("HORN/STROBE", to_model(Vec2::new(4.5, 12.0)) + Vec2::new(36.0, 0.0), "A1"),
            design("HORN/STROBE", to_model(Vec2::new(16.0, 18.0)), "A2"),
            // A detector 2 ft from the shop drawing's, and one the shop drawing has nothing near.
            ReadDevice { kind_source: String::new(), ..design("SR", to_model(Vec2::new(8.0, 14.0)) + Vec2::new(0.0, 24.0), "D1") },
            ReadDevice { kind_source: String::new(), ..design("SR", to_model(Vec2::new(26.0, 6.0)), "D2") },
            // A receptacle is no appliance: untouched.
            ReadDevice { tag: "R".into(), at: [10.0, 10.0], level: "Level 1".into(), kind: "receptacle".into(), ..Default::default() },
        ];
        let done = apply(std::slice::from_ref(&shop), &rooms, &mut devices, 12.0);
        assert_eq!((done.superseded, done.added, done.left_out.len()), (1, 4, 1), "{done:#?}");
        assert_eq!(done.left_out.first().map(|l| l.0.as_str()), Some("A2"));
        let fa: Vec<&ReadDevice> = devices.iter().filter(|d| d.fire_alarm.as_ref().is_some_and(|f| !f.loop_device)).collect();
        assert_eq!(fa.len(), 5);
        assert!(devices.iter().any(|d| d.tag == "R"), "the receptacle stays");
        let get =
            |addr: &str| fa.iter().find(|d| d.fire_alarm.as_ref().is_some_and(|f| f.address == addr)).copied().unwrap_or_else(|| panic!("{addr}"));
        // The design device took the first appliance's place: its design spot and kind kept beside.
        let first = get("BP7.N1.01");
        assert_eq!(first.handle, "A1");
        assert_eq!((first.tag.as_str(), first.circuit.as_str(), first.aff_in), ("STROBE", "BP7.N1", None));
        let f = first.fire_alarm.as_ref().unwrap_or_else(|| panic!("facts"));
        assert_eq!((f.candela, f.mount.as_str(), f.model.as_str()), (Some(15), "ceiling", "ZX-1"));
        assert!(f.design.contains("HORN/STROBE") && f.design.contains("3.0 ft away"), "{f:?}");
        assert!(first.drawn_at.is_some() && first.moved.contains("BP7.N1.01"));
        assert!(to_model(Vec2::new(4.5, 12.0)).dist(Vec2::new(first.at[0], first.at[1])) < 6.0, "{:?}", first.at);
        let hs = get("BP7.N1.03");
        assert_eq!((hs.tag.as_str(), hs.level.as_str()), ("HORN/STROBE", "Level 1"));
        assert_eq!(get("BP7.N2.01").fire_alarm.as_ref().and_then(|f| f.candela), Some(177));
        let wp = get("BP7.N2.02").fire_alarm.as_ref().unwrap_or_else(|| panic!("facts"));
        assert!(wp.weatherproof && wp.mount == "wall", "{wp:?}");
        // The detector took its loop address and spot; the other kept its own.
        assert_eq!((done.addressed, done.initiating_unmatched), (1, 0), "{done:#?}");
        let d1 = devices.iter().find(|d| d.handle == "D1").and_then(|d| d.fire_alarm.as_ref()).unwrap_or_else(|| panic!("D1 addressed"));
        assert_eq!((d1.address.as_str(), d1.circuit.as_str(), d1.loop_device, d1.candela), ("L7.42", "L7", true, None));
        assert!(d1.design.contains("SR") && d1.design.contains("ft away"), "{d1:?}");
        let moved = devices
            .iter()
            .find(|d| d.handle == "D1")
            .and_then(|d| d.drawn_at.map(|w| Vec2::new(w[0], w[1]).dist(Vec2::new(d.at[0], d.at[1])) / 12.0));
        assert!(moved.is_some_and(|m| (m - 2.0).abs() < 0.5), "{moved:?}");
        assert!(devices.iter().find(|d| d.handle == "D2").is_some_and(|d| d.fire_alarm.is_none()));
        // The legend's quantities checked: 4 ceiling strobes listed, 3 on the plan.
        assert!(
            done.quantities.iter().any(|q| q.contains("CEILING MOUNT STROBE") && q.contains("lists 4") && q.contains("show 3")),
            "{:?}",
            done.quantities
        );
    }

    /// A plan whose title is drawn as shapes (no text), a detector with its loop address beside
    /// each appliance, and a riser page listing every appliance: the plan is found by its block
    /// of linework, its level by its room numbers; each label takes its own symbol; the riser is
    /// no plan.
    #[test]
    fn an_untitled_plans_level_comes_from_its_room_numbers_and_a_riser_is_no_plan() {
        let mut plan = Drawing::new_imperial();
        for (a, b) in [([2.0, 4.0], [30.0, 4.0]), ([30.0, 4.0], [30.0, 20.0]), ([30.0, 20.0], [2.0, 20.0]), ([2.0, 20.0], [2.0, 4.0])] {
            line(&mut plan, a, b);
        }
        // Its rooms' walls (the outline alone spans the page, a border to the region finder).
        // (drawn in short pieces, as a plan's walls are).
        for k in 0..5 {
            let x = 3.0 + 6.0 * k as f64;
            for j in 0..10 {
                line(&mut plan, [x, 6.0 + j as f64], [x, 7.0 + j as f64]);
            }
        }
        for j in 0..24 {
            for y in [6.0, 16.0] {
                line(&mut plan, [3.0 + j as f64, y], [4.0 + j as f64, y]);
            }
        }
        for (i, n) in ["R-21", "R-22", "R-23", "R-24"].iter().enumerate() {
            text(&mut plan, [4.0 + 6.0 * i as f64, 8.0], 0.1, n);
        }
        let mut riser = Drawing::new_imperial();
        for (i, (addr, cd)) in [("BP8.N1.01", "15cd"), ("BP8.N1.02", "30cd")].iter().enumerate() {
            let c = [6.0 + 8.0 * i as f64, 12.0];
            // The appliance, and a detector beside it with its loop address on its far side.
            circle(&mut plan, c, 0.12);
            circle(&mut plan, [c[0] - 0.4, c[1]], 0.12);
            text(&mut plan, [c[0] - 0.95, c[1] - 0.05], 0.1, &format!("L7.{}", 10 + i));
            text(&mut plan, [c[0] + 0.25, c[1] + 0.05], 0.1, addr);
            text(&mut plan, [c[0] + 0.25, c[1] - 0.12], 0.1, cd);
            text(&mut riser, [3.0, 10.0 - i as f64], 0.1, addr);
            text(&mut riser, [3.0, 9.85 - i as f64], 0.1, cd);
        }
        // The riser's lines between its boxes.
        line(&mut riser, [0.0, 0.0], [20.0, 0.0]);
        line(&mut riser, [20.0, 0.0], [20.0, 14.0]);
        for k in 0..6 {
            let y = 8.0 + 0.4 * k as f64;
            line(&mut riser, [2.5, y], [5.0, y]);
            line(&mut riser, [2.5, y], [2.5, y + 0.3]);
        }
        let shop = read_pages("Sample FA Shop.pdf", &[(3, legend_page()), (4, plan), (5, riser)]).unwrap_or_else(|| panic!("a shop drawing"));
        assert_eq!(shop.plans.len(), 1, "the riser page is no plan: {:#?}", shop.plans);
        let p = shop.plans.first().unwrap_or_else(|| panic!("a plan"));
        assert_eq!((p.page, p.level.as_str(), p.appliances.len()), (4, "", 2));
        for a in &p.appliances {
            let want = if a.address.ends_with("01") { Vec2::new(6.0, 12.0) } else { Vec2::new(14.0, 12.0) };
            assert!(a.at.dist(want) < 0.01, "{} at its own symbol, not the detector's: {:?}", a.address, a.at);
        }
        let to_model = |p: Vec2| Vec2::new(96.0 * p.x, 96.0 * p.y);
        let rooms: Vec<ModelRoom> = ["R-11", "R-12", "R-13", "R-14", "R-21", "R-22", "R-23", "R-24"]
            .iter()
            .enumerate()
            .map(|(i, n)| ModelRoom {
                number: n.to_string(),
                level: if i < 4 { "Level 1".into() } else { "Level 2".into() },
                at: to_model(Vec2::new(4.09 + 6.0 * (i % 4) as f64, 8.05 + if i < 4 { 3.0 } else { 0.0 })),
            })
            .collect();
        let mut devices = Vec::new();
        let done = apply(std::slice::from_ref(&shop), &rooms, &mut devices, 12.0);
        assert_eq!(done.added, 2, "{done:#?}");
        assert!(done.plans.first().is_some_and(|p| p.contains("level Level 2") && p.contains("room numbers")), "{done:#?}");
        assert!(devices.iter().all(|d| d.level == "Level 2"));
    }

    #[test]
    fn a_plan_whose_room_numbers_dont_line_up_changes_nothing() {
        let shop = read_pages("Sample FA Shop.pdf", &[(3, legend_page()), (4, plan_page())]).unwrap_or_else(|| panic!("a shop drawing"));
        let rooms: Vec<ModelRoom> = ["R-11", "R-12", "R-13", "R-14"]
            .iter()
            .enumerate()
            .map(|(i, n)| ModelRoom { number: n.to_string(), level: "1".into(), at: Vec2::new(100.0 * (i % 2) as f64, 900.0 * i as f64 * i as f64) })
            .collect();
        let mut devices = vec![ReadDevice {
            tag: "STROBE".into(),
            level: "1".into(),
            kind: "fire alarm device".into(),
            kind_source: "E-901 legend: WALL STROBE".into(),
            ..Default::default()
        }];
        let done = apply(std::slice::from_ref(&shop), &rooms, &mut devices, 12.0);
        assert_eq!((done.plans.len(), done.not_fit.len(), devices.len()), (0, 1, 1), "{done:#?}");
        assert!(devices.iter().all(|d| d.fire_alarm.is_none()));
    }

    #[test]
    fn a_similarity_drops_the_pair_that_doesnt_agree() {
        let to = |p: Vec2| Vec2::new(10.0 + 2.0 * p.x, -3.0 + 2.0 * p.y);
        let mut plan: Vec<(String, Vec2)> = (0..5).map(|i| (format!("{i}"), Vec2::new(i as f64, (i * i) as f64))).collect();
        let mut model: BTreeMap<String, Vec2> = plan.iter().map(|(n, p)| (n.clone(), to(*p))).collect();
        model.insert("4".into(), Vec2::new(500.0, 500.0));
        plan.push(("4".into(), Vec2::new(9.0, 9.0)));
        let (fit, n, rms) = fit_rooms(&plan, &model, 1.0, 1.0).unwrap_or_else(|| panic!("a fit"));
        // The number printed twice is no pair; the rest agree.
        assert_eq!(n, 4);
        assert!(rms < 1e-6 && (fit.scale() - 2.0).abs() < 1e-9);
    }
}
