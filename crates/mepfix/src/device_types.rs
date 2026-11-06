//! Which tags printed beside device symbols are device types: the ones the sheet's legend and
//! fixture/device schedules list, plus the marks every electrical set uses (`S3`, `OS`, `EF`).
//! Notes, sheet numbers, keynote numbers, lowercase switch legs and dimension text never are;
//! a tag the legend doesn't list is an unknown, for the caller to ask about when it repeats.

use crate::sheet_facts::{TextItem, is_type_tag};
use cadaddy_geom::Vec2;
use std::collections::{BTreeMap, BTreeSet};

/// Most legend/schedule headings read on one sheet.
const MAX_TABLES: usize = 40;
/// Most rows read from one table.
const MAX_ROWS: usize = 400;
/// A table's rows reach at most this many heading heights below the heading.
const MAX_DEPTH: f64 = 220.0;
/// A table's text starts at most this many heading heights left of the heading, and ends
/// at most this many right of it.
const LEFT_REACH: f64 = 12.0;
const RIGHT_REACH: f64 = 140.0;
/// A blank band this many text heights tall ends a table.
const ROW_GAP: f64 = 7.0;
/// A table must list at least this many type tags to count (a lone "SEE LEGEND" note doesn't).
const MIN_TAG_ROWS: usize = 2;

/// Marks electrical sets use with or without a legend: switches, sensors, fans, boxes,
/// thermostats, fire-alarm and security devices.
const KNOWN_MARKS: &[&str] =
    &["S", "S2", "S3", "S4", "SD", "SK", "SM", "OS", "VS", "PC", "EF", "J", "JB", "T", "TV", "F", "FA", "FS", "HS", "CR", "DS", "EM", "WAP", "PB"];

/// The device types a sheet lists, and where its legends and schedules sit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Legend {
    /// Type tags from the legends' and schedules' first column (`A`, `B2`, `OS`).
    pub types: BTreeSet<String>,
    /// Each table's extent `[x0, y0, x1, y1]`: symbols drawn there are samples, not devices.
    pub regions: Vec<[f64; 4]>,
    /// Each type's row: its table and what the row says after the type (a type's first row wins).
    pub rows: BTreeMap<String, LegendRow>,
}

/// One legend or schedule row: the table's heading and the row's other texts, left to right
/// (`LIGHT FIXTURE SCHEDULE`, `2X4 LED TROFFER 120`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LegendRow {
    pub table: String,
    pub description: String,
}

/// A row's type sits within this many heading heights of the table's first column.
const TYPE_COLUMN_H: f64 = 3.0;
/// Longest description kept from one row.
const MAX_DESCRIPTION: usize = 200;

impl Legend {
    /// Whether `p` lies inside one of the legends or schedules.
    pub fn covers(&self, p: Vec2) -> bool {
        self.regions.iter().any(|r| r[0] <= p.x && p.x <= r[2] && r[1] <= p.y && p.y <= r[3])
    }

    /// Adds another sheet's legend.
    pub fn merge(&mut self, other: Legend) {
        self.types.extend(other.types);
        self.regions.extend(other.regions);
        for (k, v) in other.rows {
            self.rows.entry(k).or_insert(v);
        }
    }
}

/// What a symbol's tag says about the symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagVerdict {
    /// A device of this type.
    Device(String),
    /// Not a device mark; the reason (`lettering`, `sheet number`).
    NotDevice(&'static str),
    /// Shaped like a type tag but not in the legend: ask when it repeats.
    Unlisted(String),
}

/// Whether a heading names a table of device types: a symbol/electrical legend or a
/// fixture/luminaire/device/equipment schedule. Panel, door, room and wall tables don't.
pub fn is_device_table_heading(t: &str) -> bool {
    let u = t.trim().to_ascii_uppercase();
    if u.len() > 60 || ["PANEL", "DOOR", "WINDOW", "ROOM", "FINISH", "WALL", "PARTITION", "SEE ", "REFER"].iter().any(|w| u.contains(w)) {
        return false;
    }
    if u.contains("LEGEND") {
        return true;
    }
    u.contains("SCHEDULE") && ["FIXTURE", "LUMINAIRE", "LIGHTING", "LIGHT", "DEVICE", "EQUIPMENT", "FAN"].iter().any(|w| u.contains(w))
}

/// Words of a heading whose table, a legend or a schedule all the same, lists no device types:
/// the site, planting, material, code and structural tables (`PLANT LEGEND`, `OCCUPANT LOAD
/// FACTOR LEGEND`, `COLUMN FOUNDATION SCHEDULE`), whose marks a device's mark can share.
const NOT_DEVICE_TYPES: &[&str] = &[
    "PLANT",
    "EROSION",
    "MATERIAL",
    "OCCUPAN",
    "PAVING",
    "GRADING",
    "HATCH",
    "COLUMN",
    "FOUNDATION",
    "FOOTING",
    "SHEAR",
    "ANCHOR",
    "PEDESTAL",
    "HOLD DOWN",
    "LINTEL",
];

/// Whether a heading's table lists device types ([`read_legend`]): a device table
/// ([`is_device_table_heading`]) that isn't another trade's ([`NOT_DEVICE_TYPES`]).
pub fn lists_device_types(t: &str) -> bool {
    let u = t.trim().to_ascii_uppercase();
    is_device_table_heading(&u) && !NOT_DEVICE_TYPES.iter().any(|w| u.contains(w))
}

/// Most designations kept from one set.
const MAX_DESIGNATIONS: usize = 200;

/// A legend's designations: its `KEY=MEANING` lines (`3=3-WAY`, `D=DIMMER- REFER TO LIGHTING
/// CONTROLS SCHEDULE`, `OS2=WALL MOUNTED OCCUPANCY SENSOR WSXA-PDT-D`), the meaning by its key
/// (one to three capitals or digits). A key's first line wins.
pub fn designations(texts: &[TextItem]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for t in texts {
        if out.len() >= MAX_DESIGNATIONS {
            break;
        }
        let Some((k, v)) = t.value.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let key = (1..=3).contains(&k.len()) && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if key && v.len() <= 80 && v.chars().filter(char::is_ascii_alphabetic).count() >= 3 {
            out.entry(k.to_string()).or_insert_with(|| v.to_string());
        }
    }
    out
}

/// The legends and device schedules among a sheet's texts: each table's type tags (the first
/// text of each row) and extent.
pub fn read_legend(texts: &[TextItem]) -> Legend {
    let mut out = Legend::default();
    let headings = texts.iter().filter(|t| lists_device_types(&t.value) && t.height > 0.0 && t.height.is_finite()).take(MAX_TABLES);
    for h in headings {
        let hh = h.height;
        let (x0, x1) = (h.at.x - LEFT_REACH * hh, h.at.x + RIGHT_REACH * hh);
        let mut below: Vec<&TextItem> = texts
            .iter()
            .filter(|t| !std::ptr::eq(*t, h) && t.at.x >= x0 && t.at.x <= x1 && t.at.y < h.at.y - 0.2 * hh && t.at.y > h.at.y - MAX_DEPTH * hh)
            .collect();
        below.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        // Rows, top down, until a blank band.
        let mut rows: Vec<Vec<&TextItem>> = Vec::new();
        let mut last_y = h.at.y;
        let mut last_h = hh;
        for t in below {
            let gap = last_y - t.at.y;
            if gap > ROW_GAP * last_h.max(t.height) {
                break;
            }
            match rows.last_mut() {
                Some(row) if gap <= 0.5 * t.height.min(last_h) => row.push(t),
                _ => {
                    if rows.len() >= MAX_ROWS {
                        break;
                    }
                    rows.push(vec![t]);
                }
            }
            last_y = t.at.y;
            last_h = t.height;
        }
        // Its type column: the first column of the rows (a neighbouring table the rows run into,
        // an ABBREVIATIONS list beside a legend, keys its own words: `C` CONDUIT is no type).
        let firsts: Vec<&TextItem> = rows.iter().filter_map(|row| row.iter().copied().min_by(|a, b| a.at.x.total_cmp(&b.at.x))).collect();
        let column = firsts.iter().map(|t| t.at.x).fold(f64::INFINITY, f64::min);
        // An ABBREVIATIONS list among the rows: the keys under its header are words, not types.
        let abbreviations: Vec<Vec2> =
            rows.iter().flatten().filter(|t| t.value.to_ascii_uppercase().contains("ABBREVIATION")).map(|t| t.at).collect();
        let abbreviated = |t: &TextItem| abbreviations.iter().any(|a| t.at.y < a.y && (t.at.x - a.x).abs() <= TYPE_COLUMN_H * hh);
        let mut tags = BTreeSet::new();
        let mut listed = BTreeSet::new();
        let mut described: Vec<(String, LegendRow)> = Vec::new();
        let mut ext = [h.at.x, h.at.y + hh, h.at.x, h.at.y];
        for (row, first) in rows.iter().zip(&firsts) {
            if is_type_tag(&first.value) {
                let tag = first.value.trim().to_string();
                tags.insert(tag.clone());
                if first.at.x - column <= TYPE_COLUMN_H * hh && !abbreviated(first) {
                    let mut rest: Vec<&&TextItem> = row.iter().filter(|t| !std::ptr::eq(**t, *first)).collect();
                    rest.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
                    let mut description = rest.iter().map(|t| t.value.trim()).filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" ");
                    if description.len() > MAX_DESCRIPTION {
                        let cut = (0..=MAX_DESCRIPTION).rev().find(|i| description.is_char_boundary(*i)).unwrap_or(0);
                        description.truncate(cut);
                    }
                    described.push((tag.clone(), LegendRow { table: h.value.trim().to_string(), description }));
                    listed.insert(tag);
                }
            }
            for t in row {
                ext = [ext[0].min(t.at.x), ext[1].min(t.at.y), ext[2].max(t.at.x + t.height * t.value.chars().count() as f64), ext[3].max(t.at.y)];
            }
        }
        if tags.len() < MIN_TAG_ROWS {
            continue;
        }
        // Room for the symbols drawn left of the first column and the last row's height.
        let pad = 2.0 * hh;
        out.regions.push([ext[0] - pad, ext[1] - pad, ext[2] + pad, ext[3] + pad]);
        out.types.extend(listed);
        for (tag, row) in described {
            out.rows.entry(tag).or_insert(row);
        }
    }
    out
}

/// Whether `tag` is a mark electrical sets use without listing it.
pub fn is_known_mark(tag: &str) -> bool {
    KNOWN_MARKS.contains(&tag.trim())
}

/// A sheet number in the forms a tag could print it (`E-1` and `E1`).
pub fn sheet_number_forms(number: &str) -> Vec<String> {
    let n = number.trim();
    if n.is_empty() {
        return Vec::new();
    }
    let plain: String = n.chars().filter(|c| *c != '-').collect();
    if plain == n { vec![n.to_string()] } else { vec![n.to_string(), plain] }
}

/// What `tag` printed by a symbol is. `legend` is every type the loaded legends and schedules
/// list (empty when none was found: then any tag-shaped mark counts); `learned` are tags an
/// answer made device types; `sheet_numbers` are the set's sheet numbers.
pub fn tag_verdict(tag: &str, legend: &BTreeSet<String>, learned: &BTreeSet<String>, sheet_numbers: &BTreeSet<String>) -> TagVerdict {
    tag_verdict_listed(tag, legend, !legend.is_empty(), learned, sheet_numbers)
}

/// [`tag_verdict`] for a tag on one sheet: `listed` are the types the legends that speak for
/// that sheet list (its own, its trade's); `any_listed`, whether the set has a legend at all
/// (with none, any tag-shaped mark counts; with some, a sheet whose legends list nothing has
/// only the known and learned marks).
pub fn tag_verdict_listed(
    tag: &str,
    listed: &BTreeSet<String>,
    any_listed: bool,
    learned: &BTreeSet<String>,
    sheet_numbers: &BTreeSet<String>,
) -> TagVerdict {
    let t = tag.trim();
    if !is_type_tag(t) {
        // Lowercase switch legs, keynote numbers, dimensions, notes, long sheet numbers.
        return TagVerdict::NotDevice("lettering");
    }
    if sheet_numbers.contains(t) {
        return TagVerdict::NotDevice("sheet number");
    }
    if learned.contains(t) || !any_listed || listed.contains(t) || is_known_mark(t) {
        return TagVerdict::Device(t.to_string());
    }
    TagVerdict::Unlisted(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(value: &str, x: f64, y: f64, h: f64) -> TextItem {
        TextItem { value: value.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
    }

    #[test]
    fn a_legends_designations_are_its_key_equals_meaning_lines() {
        let texts = [
            t("2=2 POLE", 0.0, 10.0, 1.0),
            t("3=3-WAY", 0.0, 9.0, 1.0),
            t("OS2=WALL MOUNTED OCCUPANCY SENSOR WSXA-PDT-D", 0.0, 8.0, 1.0),
            t("3=SOMETHING ELSE", 0.0, 7.0, 1.0),
            t("1/4\" = 1'-0\"", 0.0, 6.0, 1.0),
            t("+60\" = MOUNT AT 60\"AFF", 0.0, 5.0, 1.0),
            t("X=1", 0.0, 4.0, 1.0),
        ];
        let d = designations(&texts);
        assert_eq!(d.len(), 3, "{d:?}");
        assert_eq!(d["3"], "3-WAY", "a key's first line wins");
        assert_eq!(d["OS2"], "WALL MOUNTED OCCUPANCY SENSOR WSXA-PDT-D");
    }

    /// A made-up lighting fixture schedule and symbol legend in a sheet's right margin, a note
    /// that mentions the legend, and plan lettering below a blank band.
    fn sheet() -> Vec<TextItem> {
        let mut v = vec![
            t("LIGHTING FIXTURE SCHEDULE", 1500.0, 1200.0, 8.0),
            t("TYPE", 1500.0, 1185.0, 5.0),
            t("DESCRIPTION", 1530.0, 1185.0, 5.0),
            t("VOLTS", 1700.0, 1185.0, 5.0),
        ];
        for (i, (ty, d)) in
            [("A", "2X4 LED TROFFER"), ("B2", "6\" LED DOWNLIGHT"), ("XA", "EXIT SIGN, EDGE LIT"), ("L4", "LINEAR LED")].iter().enumerate()
        {
            let y = 1172.0 - 12.0 * i as f64;
            v.push(t(ty, 1500.0, y, 5.0));
            v.push(t(d, 1530.0, y, 5.0));
            v.push(t("120", 1700.0, y, 5.0));
        }
        v.push(t("ELECTRICAL SYMBOL LEGEND", 1500.0, 1000.0, 8.0));
        for (i, (sym, d)) in [("OS", "OCCUPANCY SENSOR"), ("S3", "THREE-WAY SWITCH"), ("WP", "WEATHERPROOF RECEPTACLE")].iter().enumerate() {
            let y = 985.0 - 12.0 * i as f64;
            v.push(t(sym, 1505.0, y, 5.0));
            v.push(t(d, 1530.0, y, 5.0));
        }
        // a note naming the legend, and plan lettering far below the tables
        v.push(t("SEE LEGEND ON SHEET Q-0.1", 200.0, 1300.0, 5.0));
        v.push(t("Q", 1505.0, 600.0, 5.0));
        v.push(t("ZZ", 1510.0, 590.0, 5.0));
        v
    }

    #[test]
    fn the_legend_and_schedule_list_the_types() {
        let l = read_legend(&sheet());
        let want: BTreeSet<String> = ["A", "B2", "XA", "L4", "OS", "S3", "WP"].iter().map(|s| s.to_string()).collect();
        assert_eq!(l.types, want);
        assert_eq!(l.regions.len(), 2, "{:?}", l.regions);
        assert!(l.covers(Vec2::new(1495.0, 1160.0)), "a sample symbol beside the schedule");
        assert!(!l.covers(Vec2::new(1505.0, 600.0)), "plan below the blank band");
        assert!(!l.covers(Vec2::new(200.0, 1300.0)), "the note");
    }

    #[test]
    fn notes_and_other_tables_are_not_legends() {
        for h in ["SEE LEGEND ON SHEET E-0.1", "PANEL SCHEDULE LP1", "DOOR SCHEDULE", "WALL LEGEND", "GENERAL NOTES", "KEYNOTES"] {
            assert!(!is_device_table_heading(h), "{h}");
        }
        for h in ["LIGHTING FIXTURE SCHEDULE", "LUMINAIRE SCHEDULE", "ELECTRICAL SYMBOL LEGEND", "Legend", "FAN SCHEDULE"] {
            assert!(is_device_table_heading(h), "{h}");
        }
        // A heading with only one tag row below it is a note, not a table.
        let l = read_legend(&[t("LEGEND", 0.0, 100.0, 5.0), t("A", 0.0, 90.0, 5.0), t("FIXTURE", 20.0, 90.0, 5.0)]);
        assert!(l.types.is_empty() && l.regions.is_empty());
    }

    #[test]
    fn a_row_keeps_its_table_and_description() {
        let l = read_legend(&sheet());
        let a = l.rows.get("A").unwrap_or_else(|| panic!("{:?}", l.rows));
        assert_eq!((a.table.as_str(), a.description.as_str()), ("LIGHTING FIXTURE SCHEDULE", "2X4 LED TROFFER 120"));
        assert_eq!(l.rows.get("OS").map(|r| r.description.as_str()), Some("OCCUPANCY SENSOR"));
        let mut merged = Legend::default();
        merged.merge(l.clone());
        merged.merge(read_legend(&[t("LEGEND", 0.0, 100.0, 5.0), t("A", 0.0, 90.0, 5.0), t("OTHER", 20.0, 90.0, 5.0), t("Q", 0.0, 80.0, 5.0)]));
        assert_eq!(merged.rows["A"].description, "2X4 LED TROFFER 120", "a type's first row wins");
    }

    /// Q6165: a legend whose rows run into the ABBREVIATIONS list beside it (`C` CONDUIT, `P`
    /// POLE) lists only its own types; site, planting and structural tables list none.
    #[test]
    fn abbreviations_and_other_trades_tables_are_not_device_types() {
        let mut v = vec![
            t("ELECTRICAL LEGEND", 1000.0, 1400.0, 7.0),
            t("DESCRIPTION- LOW VOLTAGE DEVICES", 970.0, 1387.0, 5.0),
            t("ABBREVIATIONS", 1230.0, 1387.0, 5.0),
        ];
        for (i, (d, k, m)) in [("TELEPHONE OUTLET", "A", "AMPERE"), ("", "C", "CONDUIT"), ("WIRELESS ACCESS POINT", "G", "GROUND"), ("", "P", "POLE")]
            .iter()
            .enumerate()
        {
            let y = 1373.0 - 7.0 * i as f64;
            if !d.is_empty() {
                v.push(t(d, 970.0, y, 5.0));
            }
            v.push(t(k, 1235.0, y, 5.0));
            v.push(t(m, 1267.0, y, 5.0));
        }
        let l = read_legend(&v);
        assert!(l.types.is_empty() && l.rows.is_empty(), "{:?}", l.types);
        assert_eq!(l.regions.len(), 1, "its symbols are still samples");
        // Its descriptions printed left of where the heading's table reaches: the list's own
        // header says what its keys are.
        let far: Vec<TextItem> = v.iter().map(|x| if x.at.x < 1000.0 { t(&x.value, 850.0, x.at.y, x.height) } else { x.clone() }).collect();
        let l = read_legend(&far);
        assert!(l.types.is_empty() && l.rows.is_empty(), "{:?}", l.types);
        // A legend with its ABBREVIATIONS list under it, in its own column: its types stay its.
        let mut stacked = vec![t("LEGEND", 1000.0, 1400.0, 7.0)];
        for (i, (k, d)) in [("A", "2X4 LED TROFFER"), ("B", "LED DOWNLIGHT"), ("EM", "EMERGENCY BATTERY UNIT")].iter().enumerate() {
            let y = 1386.0 - 9.0 * i as f64;
            stacked.extend([t(k, 1000.0, y, 5.0), t(d, 1030.0, y, 5.0)]);
        }
        stacked.extend([
            t("ABBREVIATIONS", 1000.0, 1350.0, 7.0),
            t("AFF", 1000.0, 1338.0, 5.0),
            t("ABOVE FINISHED FLOOR", 1030.0, 1338.0, 5.0),
            t("GYP", 1000.0, 1329.0, 5.0),
            t("GYPSUM BOARD", 1030.0, 1329.0, 5.0),
        ]);
        let l = read_legend(&stacked);
        let want: BTreeSet<String> = ["A", "B", "EM"].iter().map(|s| s.to_string()).collect();
        assert_eq!(l.types, want, "{:?}", l.rows);
        for h in [
            "PLANT LEGEND",
            "EROSION CONTROL LEGEND",
            "COLUMN FOUNDATION SCHEDULE",
            "WOOD COLUMN SCHEDULE",
            "OCCUPANT LOAD FACTOR LEGEND",
            "MATERIAL LEGEND",
        ] {
            assert!(!lists_device_types(h), "{h}");
        }
        assert!(lists_device_types("LIFE SAFETY LEGEND"), "its pull stations and extinguishers are");
        assert!(is_device_table_heading("PLANT LEGEND"), "still a legend (its rows aren't notes), of no device types");
    }

    /// Q6165: a sheet's tags are its own legends' types: with a legend in the set, a sheet whose
    /// legends list nothing has only the known and learned marks.
    #[test]
    fn a_sheet_lists_its_own_types() {
        let none = BTreeSet::new();
        let sheets = BTreeSet::new();
        let listed: BTreeSet<String> = ["A".to_string()].into();
        assert_eq!(tag_verdict_listed("A", &listed, true, &none, &sheets), TagVerdict::Device("A".into()));
        assert_eq!(tag_verdict_listed("A", &none, true, &none, &sheets), TagVerdict::Unlisted("A".into()));
        assert_eq!(tag_verdict_listed("EF", &none, true, &none, &sheets), TagVerdict::Device("EF".into()), "a known mark");
        assert_eq!(tag_verdict_listed("A", &none, false, &none, &sheets), TagVerdict::Device("A".into()), "no legend in the set");
    }

    #[test]
    fn verdicts() {
        let legend: BTreeSet<String> = ["A", "B2", "XA"].iter().map(|s| s.to_string()).collect();
        let none = BTreeSet::new();
        let sheets: BTreeSet<String> = sheet_number_forms("Q-7").into_iter().chain(sheet_number_forms("E1")).collect();
        assert_eq!(sheets, ["E1", "Q-7", "Q7"].iter().map(|s| s.to_string()).collect());
        // listed, known and learned marks are devices
        for tag in ["A", "B2", "XA", "OS", "S3", "EF"] {
            assert_eq!(tag_verdict(tag, &legend, &none, &sheets), TagVerdict::Device(tag.into()), "{tag}");
        }
        let learned: BTreeSet<String> = ["QQ".to_string()].into();
        assert_eq!(tag_verdict("QQ", &legend, &learned, &sheets), TagVerdict::Device("QQ".into()));
        // never devices: lowercase switch legs, keynote numbers, dimensions, notes, sheet numbers
        for tag in ["s", "t", "a1", "12", "3A", "4'-0\"", "TYP", "GFCI", "S2004", "E-4", "X"] {
            assert_eq!(tag_verdict(tag, &legend, &none, &sheets), TagVerdict::NotDevice("lettering"), "{tag}");
        }
        assert_eq!(tag_verdict("Q7", &legend, &none, &sheets), TagVerdict::NotDevice("sheet number"));
        assert_eq!(tag_verdict("E1", &none, &none, &sheets), TagVerdict::NotDevice("sheet number"));
        // shaped like a tag but unlisted
        assert_eq!(tag_verdict("ZZ", &legend, &none, &sheets), TagVerdict::Unlisted("ZZ".into()));
        // with no legend loaded, the tag's shape is all there is to go on
        assert_eq!(tag_verdict("ZZ", &none, &none, &sheets), TagVerdict::Device("ZZ".into()));
    }
}
