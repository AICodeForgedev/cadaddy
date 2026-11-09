//! Lighting from its documents (Q6144, first part): the light fixture schedule read by its grid
//! ([`super::gridtable`]), each luminaire given its type's row (basis of design, lamp, VA, voltage,
//! installation, description) with the schedule as its source, and the documents checked against
//! each other: a plan's fixture type the schedule doesn't list, a scheduled type no plan shows, a
//! submitted luminaire that isn't the schedule's basis of design (maker or catalog) — the schedule's
//! own "no substitutions" note said beside it.

use std::collections::BTreeSet;

use cadaddy_walk3d::FixtureType;

use super::gridtable::GridTable;

/// Whether a text is a luminaire schedule's title (`schedules.luminaire_titles`; on an
/// electrical sheet a plain `FIXTURE SCHEDULE` too), not another trade's or table's
/// (`PLUMBING FIXTURE SCHEDULE`, `LIGHTING CONTROL SCHEDULE`).
pub(crate) fn is_luminaire_title(text: &str, electrical: bool) -> bool {
    use cadaddy_mepfix::wordrules;
    let u = text.trim().to_ascii_uppercase();
    // A sentence's end is a note's, never a title's (`... ON LIGHT FIXTURE SCHEDULE.`).
    if u.len() > 80 || u.ends_with('.') || wordrules::words("schedules.not_luminaire_title_words").iter().any(|w| u.contains(w)) {
        return false;
    }
    let reference = wordrules::words("schedules.title_reference_words");
    let titled = |titles: &[&str]| {
        titles.iter().any(|w| {
            // A title, not a note that names one (`SEE SHEET X FOR FIXTURE SCHEDULE`): a few
            // words beside it at most (`LIGHT FIXTURE SCHEDULE - LEVEL 1`), none a note's.
            u.find(w).is_some_and(|at| {
                let rest = format!("{} {}", u.get(..at).unwrap_or(""), u.get(at + w.len()..).unwrap_or(""));
                let extra: Vec<&str> = rest.split(|c: char| !c.is_ascii_alphanumeric()).filter(|x| !x.is_empty()).collect();
                extra.len() <= MAX_TITLE_EXTRA_WORDS && !extra.iter().any(|x| reference.contains(x))
            })
        })
    };
    titled(wordrules::words("schedules.luminaire_titles")) || (electrical && titled(wordrules::words("schedules.electrical_luminaire_titles")))
}

/// Words a luminaire schedule's title may carry beside its name (`TYPICAL`, `LEVEL 1`).
const MAX_TITLE_EXTRA_WORDS: usize = 3;

/// Whether a cell reads as a type mark: short, one word, a letter in it (`A`, `F1`, `XA`), and
/// no voltage ([`voltage_shaped`]: `MVOLT`, `120V`).
fn mark_shaped(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty() && v.len() <= MAX_MARK && !v.contains(' ') && v.chars().any(|c| c.is_ascii_alphabetic()) && !voltage_shaped(v)
}

/// A type mark's most letters.
const MAX_MARK: usize = 6;

/// Whether a cell is a voltage, never a type mark: a voltage word (`schedules.voltage_words`:
/// `MVOLT`, `UNV`), or a number or a pair of them (`120`, `120/277`, `347-480`) with a volt unit
/// or none (`schedules.voltage_units`: `277V`, `24VDC`).
fn voltage_shaped(v: &str) -> bool {
    use cadaddy_mepfix::wordrules;
    let u = v.trim().to_ascii_uppercase();
    if u.is_empty() || wordrules::words("schedules.voltage_words").contains(&u.as_str()) {
        return !u.is_empty();
    }
    let mut units: Vec<&str> = wordrules::words("schedules.voltage_units").to_vec();
    units.sort_by_key(|w| std::cmp::Reverse(w.len()));
    let number = units.iter().find_map(|w| u.strip_suffix(w)).unwrap_or(&u).trim_end();
    number.chars().any(|c| c.is_ascii_digit()) && number.chars().all(|c| c.is_ascii_digit() || matches!(c, '/' | '-' | '.'))
}

/// Whether column `c`'s header names `w` as a word of its own (`TYPE` in `FIXTURE TYPE`; not the
/// `TAG` in `VOLTAGE`).
fn heads(t: &GridTable, c: usize, w: &str) -> bool {
    t.headers.get(c).is_some_and(|h| h.to_ascii_uppercase().split(|ch: char| !ch.is_ascii_alphanumeric()).any(|x| x == w))
}

/// The schedule's column of type marks: the first column, left to right, headed by a type
/// heading (`schedules.luminaire_type_headings`) and by no word of another field
/// (`schedules.not_mark_heading_words`: a lamp's `TYPE` under its `LAMPS` group, a `VOLTAGE`),
/// whose cells read as marks; else the first column whose cells do; else the first a heading
/// names. The plans' tags (`plan_tags`, upper case; none: no check) are the check: when that
/// column's cells match none of them and another such column's do, that one.
fn mark_column(t: &GridTable, plan_tags: &BTreeSet<String>) -> Option<usize> {
    use cadaddy_mepfix::wordrules;
    let cells = |c: usize| -> Vec<&str> { t.rows.iter().map(|r| t.cell(r, Some(c))).filter(|v| !v.is_empty()).collect() };
    let marks = |c: usize| {
        let v = cells(c);
        !v.is_empty() && 2 * v.iter().filter(|x| mark_shaped(x)).count() > v.len()
    };
    let other_field = |c: usize| wordrules::words("schedules.not_mark_heading_words").iter().any(|w| heads(t, c, w));
    let named: Vec<usize> = (0..t.headers.len())
        .filter(|c| !other_field(*c) && wordrules::words("schedules.luminaire_type_headings").iter().any(|w| heads(t, *c, w)))
        .collect();
    let mut candidates: Vec<usize> = named.iter().copied().filter(|c| marks(*c)).collect();
    candidates.extend((0..t.headers.len()).filter(|c| !other_field(*c) && marks(*c) && !named.contains(c)));
    let on_plans = |c: usize| cells(c).iter().any(|v| plan_tags.contains(&v.to_ascii_uppercase()));
    let first = candidates.first().copied().or_else(|| named.first().copied());
    if plan_tags.is_empty() || first.is_none_or(on_plans) {
        return first;
    }
    candidates.into_iter().find(|c| on_plans(*c)).or(first)
}

/// The schedule's rows (by its columns' names), and whether its notes forbid substitutions;
/// its column of marks checked against the tags the plans print (`plan_tags`, upper case; none:
/// no check; [`mark_column`]). Marks its notes list as a scheduled type with a suffix
/// (`FIXTURES AE, BE AND CE ARE ON EMERGENCY BATTERY BACKUP` under types A, B, C) are types too:
/// their base type's row, the note beside it.
pub(crate) fn fixture_types(t: &GridTable, plan_tags: &BTreeSet<String>) -> (Vec<FixtureType>, bool) {
    let tag = mark_column(t, plan_tags);
    let maker = t.column(&["MANUFACTURER"]).or_else(|| t.column(&["BASIS"]));
    let model = t.column(&["MODEL"]).or_else(|| t.column(&["CATALOG"])).or_else(|| t.column(&["SERIES"]));
    let lamp = t.column(&["LAMP"]).or_else(|| t.column(&["SOURCE"]));
    let volts = t.column(&["VOLT"]);
    let install = t.column(&["INSTALL"]).or_else(|| t.column(&["MOUNTING"]));
    let desc = t.column(&["DESCRIPTION"]).filter(|c| Some(*c) != tag);
    let notes = t.column(&["NOTES"]).or_else(|| t.column(&["REMARKS"]));
    // A luminaire schedule describes its types: a table with no describing column (a grid of
    // linework round a note, an elevation's dimensions) is none.
    if [maker, model, lamp, volts, install, desc].iter().all(Option::is_none) {
        return (Vec::new(), false);
    }
    let qty = t.column(&["QTY"]).or_else(|| t.column(&["QUANTITY"]));
    let mut out = Vec::new();
    let mut no_subs = false;
    for r in &t.rows {
        let all = r.join(" ").to_ascii_uppercase();
        if all.contains("NO SUBSTITU") {
            no_subs = true;
        }
        let tg = t.cell(r, tag);
        // A type is short (`A`, `F1`, `XA`); a note row's text isn't, nor is a voltage.
        if tg.is_empty() || tg.len() > MAX_MARK || tg.contains(' ') || voltage_shaped(tg) {
            continue;
        }
        let lamp_text = t.cell(r, lamp).to_string();
        let va = lamp_text.split_whitespace().find_map(|w| {
            let u = w.to_ascii_uppercase();
            u.strip_suffix("VA").and_then(|n| n.trim().parse::<f64>().ok())
        });
        // A lamp line in the maker's cell (`LAMP: TECHNICAL CONSUMER PRO.`) isn't the maker.
        let mk = t.cell(r, maker);
        let mk = mk.split(" LAMP:").next().unwrap_or(mk).split(" BULB:").next().unwrap_or(mk).trim().to_string();
        out.push(FixtureType {
            tag: tg.to_ascii_uppercase(),
            manufacturer: mk,
            model: t.cell(r, model).to_string(),
            lamp: lamp_text,
            va,
            voltage: t.cell(r, volts).to_string(),
            installation: t.cell(r, install).to_string(),
            description: t.cell(r, desc).to_string(),
            notes: t.cell(r, notes).to_string(),
            source: format!("{} LIGHT FIXTURE SCHEDULE", t.sheet),
            quantity: qty.and_then(|_| t.cell(r, qty).trim().parse::<u32>().ok()).filter(|q| *q > 0 && *q < 100_000),
        });
    }
    for note in &t.notes {
        if note.to_ascii_uppercase().contains("NO SUBSTITU") {
            no_subs = true;
        }
        let tags: Vec<String> = out.iter().map(|f| f.tag.clone()).collect();
        for (variant, base) in note_variants(note, &tags) {
            if out.iter().any(|f| f.tag == variant) || out.len() >= MAX_TYPES {
                continue;
            }
            if let Some(b) = out.iter().find(|f| f.tag == base).cloned() {
                let notes = if b.notes.is_empty() { note.trim().to_string() } else { format!("{}; {}", b.notes, note.trim()) };
                out.push(FixtureType { tag: variant, notes, quantity: None, ..b });
            }
        }
    }
    (out, no_subs)
}

/// Types read from one schedule, at most.
const MAX_TYPES: usize = 400;

/// The marks a schedule note lists that are a scheduled type with a suffix, with that type:
/// a list of two or more such marks with one suffix (`AE, BE, AND CE`), or one led by a fixture
/// word (`FIXTURE AE IS ...`, `schedules.variant_lead_words`). A word that only happens to
/// start with a type's letter (`ALL`, `TO BE`) is no mark: it is in no such list.
fn note_variants(note: &str, tags: &[String]) -> Vec<(String, String)> {
    let lead = cadaddy_mepfix::wordrules::words("schedules.variant_lead_words");
    let words: Vec<String> = note
        .to_ascii_uppercase()
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '/')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    // A scheduled type and the suffix that makes `w` from it (the longest type first: `AE`
    // over `A` when both are scheduled).
    let split = |w: &str| -> Option<(String, String)> {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if !mark_shaped(w) || tags.iter().any(|t| t == w) {
            return None;
        }
        tags.iter()
            .filter(|t| w.len() > t.len() && w.len() - t.len() <= 2 && w.starts_with(t.as_str()))
            .max_by_key(|t| t.len())
            .and_then(|t| w.get(t.len()..).filter(|s| s.chars().all(|c| c.is_ascii_alphabetic())).map(|s| (t.clone(), s.to_string())))
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        // A run of marks joined by commas, slashes, AND, OR, `&`.
        let mut run: Vec<(String, String, String)> = Vec::new();
        let mut j = i;
        while j < words.len() {
            let w = words.get(j).map_or("", String::as_str);
            if matches!(w, "AND" | "OR" | "&") {
                if run.is_empty() {
                    break;
                }
                j += 1;
                continue;
            }
            // One list has one suffix: a word after it with another (`ARE` after `AE, BE, AND
            // CE`) ends it.
            match split(w) {
                Some((base, suffix)) if run.first().is_none_or(|(_, _, s)| *s == suffix) => {
                    run.push((w.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string(), base, suffix));
                }
                _ => break,
            }
            j += 1;
        }
        let led = i.checked_sub(1).and_then(|k| words.get(k)).is_some_and(|w| lead.contains(&w.as_str()));
        if !run.is_empty() && (run.len() >= 2 || led) {
            out.extend(run.into_iter().map(|(w, base, _)| (w, base)));
        }
        i = j.max(i + 1);
    }
    out
}

fn alnum(v: &str) -> String {
    v.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase()
}

/// Lighting brands by the maker that owns them (`lightbrands.json`).
const BRANDS: &str = include_str!("lightbrands.json");

/// The maker family a name belongs to (`Lithonia Lighting` → ACUITY), if the data knows it.
fn family(name: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(BRANDS).ok()?;
    let up = name.to_ascii_uppercase();
    v.get("families")?
        .as_object()?
        .iter()
        .find_map(|(fam, brands)| brands.as_array()?.iter().filter_map(serde_json::Value::as_str).any(|b| up.contains(b)).then(|| fam.clone()))
}

/// Two makers are one when they're brands of one maker (`lightbrands.json`), or a word of four
/// letters or more is in both (`COOPER` / `Cooper Lighting`), or one's letters hold the other's.
pub(crate) fn same_maker(a: &str, b: &str) -> bool {
    if let (Some(fa), Some(fb)) = (family(a), family(b)) {
        return fa == fb;
    }
    // Words every maker's name may carry don't make two makers one.
    let words = |v: &str| {
        v.to_ascii_uppercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.len() >= 4 && !cadaddy_mepfix::wordrules::words("fixtures.generic_maker_words").contains(w))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let (wa, wb) = (words(a), words(b));
    wa.iter().any(|w| wb.contains(w))
        || (!alnum(a).is_empty() && !alnum(b).is_empty() && (alnum(a).contains(&alnum(b)) || alnum(b).contains(&alnum(a))))
}

/// A submitted catalog number is the scheduled model when one holds the other's first piece (the
/// series: `ML4DM10` of `ML4DM10FSL9FS1E-TL40R2GMW`).
pub(crate) fn same_product(model: &str, catalog: &str) -> bool {
    let first = |v: &str| v.split([' ', '-', '/']).map(alnum).find(|w| w.len() >= 3).unwrap_or_default();
    let (m, c) = (alnum(model), alnum(catalog));
    if m.is_empty() || c.is_empty() {
        return true;
    }
    let (fm, fc) = (first(model), first(catalog));
    // One series with other options (`ML4DM10FSL9...` / `ML4DM10LSFL9...`): a common start of
    // six letters or more.
    let common = m.chars().zip(c.chars()).take_while(|(x, y)| x == y).count();
    common >= SERIES_LEN || (!fm.is_empty() && c.contains(&fm)) || (!fc.is_empty() && m.contains(&fc))
}

/// The letters two catalog numbers share from their start for one series.
const SERIES_LEN: usize = 6;

/// A luminaire's type from its mark (`luminaire XA` → `XA`).
pub(crate) fn type_of(mark: &str) -> String {
    let m = mark.trim();
    let m = m.strip_prefix("luminaire ").or_else(|| m.strip_prefix("LUMINAIRE ")).unwrap_or(m);
    m.trim().to_ascii_uppercase()
}

/// A luminaire schedule drawn with its grid, for tests: titled `LIGHTING FIXTURE SCHEDULE` at
/// `title_x`, its columns left to right, each its group heading (empty: none), its heading and its
/// cells (a row each). A group's columns share a rule under its heading, their lines between
/// them drawn from that rule down, the heading centred over them.
#[cfg(test)]
pub(crate) fn drawn_schedule(
    title_x: f64,
    cols: &[(&str, &str, &[&str])],
) -> (Vec<cadaddy_mepfix::sheet_facts::TextItem>, Vec<(cadaddy_geom::Vec2, cadaddy_geom::Vec2)>) {
    use cadaddy_geom::Vec2;
    let t =
        |x: f64, y: f64, v: &str, h: f64| cadaddy_mepfix::sheet_facts::TextItem { value: v.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 };
    let w = 2.0;
    let x1 = w * cols.len() as f64;
    let rows = cols.iter().map(|c| c.2.len()).max().unwrap_or(0);
    let bottom = 9.0 - 0.4 * rows as f64;
    let mut texts = vec![t(title_x, 10.0, "LIGHTING FIXTURE SCHEDULE", 0.2)];
    let mut segs = vec![(Vec2::new(0.0, 9.8), Vec2::new(x1, 9.8)), (Vec2::new(x1, 9.8), Vec2::new(x1, bottom))];
    for r in 0..=rows {
        let y = 9.0 - 0.4 * r as f64;
        segs.push((Vec2::new(0.0, y), Vec2::new(x1, y)));
    }
    for (i, (group, head, cells)) in cols.iter().enumerate() {
        let x = w * i as f64;
        let in_group = !group.is_empty() && i > 0 && cols.get(i - 1).is_some_and(|c| c.0 == *group);
        segs.push((Vec2::new(x, if in_group { 9.4 } else { 9.8 }), Vec2::new(x, bottom)));
        if group.is_empty() {
            texts.push(t(x + 0.1, 9.3, head, 0.12));
        } else {
            if !in_group {
                let span = cols.iter().skip(i).take_while(|c| c.0 == *group).count() as f64;
                segs.push((Vec2::new(x, 9.4), Vec2::new(x + w * span, 9.4)));
                texts.push(t(x + 0.5 * w * span - 0.3, 9.55, group, 0.12));
            }
            texts.push(t(x + 0.1, 9.1, head, 0.12));
        }
        for (r, v) in cells.iter().enumerate().filter(|(_, v)| !v.is_empty()) {
            texts.push(t(x + 0.1, 8.75 - 0.4 * r as f64, v, 0.12));
        }
    }
    (texts, segs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> GridTable {
        let row = |c: &[&str]| c.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        GridTable {
            headers: row(&[
                "",
                "FIXTURE TAG",
                "MANUFACTURER/BASIC OF DESIGN",
                "MODEL/SERIES #",
                "LAMP/LED SOURCE",
                "VOLTAGE",
                "INSTALLATION",
                "DESCRIPTION",
                "NOTES",
            ]),
            rows: vec![
                row(&["64", "A", "COOPER", "ML4DM10FSL9FS1E-TL40R2GMW", "LED 13VA", "120", "SURFACE/CEILING", "PUBLIC SPACES", "2,3,5"]),
                row(&[
                    "",
                    "G",
                    "ILLUMINATION LIGHTING LAMP: TECHNICAL CONSUMER PRO.",
                    "D-10413-08 L3G9D2527K x2",
                    "LED 6VA",
                    "120",
                    "SURFACE",
                    "CORRIDOR WALL SCONCE",
                    "",
                ]),
                row(&["", "", "NOTES: 5. NO SUBSTITUIONS OF VENDORS OR PRODUCTS ON LIGHTFIXTURES UNLESS APPROVED", "", "", "", "", "", ""]),
            ],
            sheet: "E-2".into(),
            ..GridTable::default()
        }
    }

    #[test]
    fn a_fixture_schedule_gives_each_type_and_its_no_substitutions_note() {
        let (types, no_subs) = fixture_types(&table(), &BTreeSet::new());
        assert!(no_subs);
        assert_eq!(types.len(), 2);
        assert_eq!((types[0].tag.as_str(), types[0].manufacturer.as_str(), types[0].va), ("A", "COOPER", Some(13.0)));
        assert_eq!(types[1].manufacturer, "ILLUMINATION LIGHTING");
    }

    /// fix-lum-fixture-schedule-not-read: a schedule whose column of marks is headed FIXTURE (no
    /// TAG, TYPE or MARK) gives its types; SERIES is the model, REMARKS the notes; a note listing
    /// a scheduled type with a suffix (`AE, BE, AND CE`) adds those types, a word that only starts
    /// with a type's letter (`ALL`, `TO BE`, `ARE`) doesn't.
    #[test]
    fn marks_under_a_fixture_heading_and_the_variants_a_note_lists_are_types() {
        let row = |c: &[&str]| c.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let t = GridTable {
            headers: row(&["FIXTURE", "DESCRIPTION", "MANUFACTURER", "SERIES", "REMARKS"]),
            rows: vec![
                row(&["A", "LINEAR FIXTURE", "MAKER ONE", "SER-1 8", "MOUNTED TO STRUCTURE"]),
                row(&["B", "RECESSED CAN", "MAKER TWO", "CAN-5", "WET LOCATION"]),
                row(&["C", "WALL PACK", "MAKER THREE", "WP-40", ""]),
            ],
            sheet: "A-2".into(),
            title: "LIGHT FIXTURE SCHEDULE".into(),
            notes: vec!["NOTE:".into(), "1. FIXTURES AE, BE, AND CE ARE ON EMERGENCY BATTERY BACKUP".into(), "2. ALL LIGHTS TO BE LED, TYP.".into()],
            ..GridTable::default()
        };
        let (types, no_subs) = fixture_types(&t, &BTreeSet::new());
        assert!(!no_subs);
        let tags: Vec<&str> = types.iter().map(|f| f.tag.as_str()).collect();
        assert_eq!(tags, ["A", "B", "C", "AE", "BE", "CE"]);
        let a = &types[0];
        assert_eq!(
            (a.description.as_str(), a.manufacturer.as_str(), a.model.as_str(), a.notes.as_str()),
            ("LINEAR FIXTURE", "MAKER ONE", "SER-1 8", "MOUNTED TO STRUCTURE")
        );
        let be = &types[4];
        assert_eq!((be.description.as_str(), be.model.as_str()), ("RECESSED CAN", "CAN-5"));
        assert_eq!(be.notes, "WET LOCATION; 1. FIXTURES AE, BE, AND CE ARE ON EMERGENCY BATTERY BACKUP");
        // One mark led by a fixture word counts; the same mark in passing words doesn't.
        let tags = ["A".to_string(), "B".to_string()];
        assert_eq!(note_variants("TYPE AX IS DIMMED", &tags), vec![("AX".to_string(), "A".to_string())]);
        assert!(note_variants("ALL LIGHTS TO BE LED", &tags).is_empty());
        assert!(note_variants("SEE NOTES AND AREA PLANS", &tags).is_empty());
        // A column headed TYPE that holds lamp types is passed over for the one holding marks.
        let t = GridTable {
            headers: row(&["MARK", "LAMP TYPE", "DESCRIPTION"]),
            rows: vec![row(&["F1", "LED 3500K", "DOWNLIGHT"]), row(&["F2", "LED 4000K", "TROFFER"])],
            ..GridTable::default()
        };
        assert_eq!(fixture_types(&t, &BTreeSet::new()).0.iter().map(|f| f.tag.as_str()).collect::<Vec<_>>(), ["F1", "F2"]);
        // A grid with mark-shaped cells but no describing column is no luminaire schedule.
        let t = GridTable { headers: row(&["", "", ""]), rows: vec![row(&["EQ", "EQ", "8' - 0\""]), row(&["EQ", "", ""])], ..GridTable::default() };
        assert!(fixture_types(&t, &BTreeSet::new()).0.is_empty());
    }

    /// A luminaire schedule's title on any sheet; a plain FIXTURE SCHEDULE on an electrical sheet
    /// only; never another trade's or table's, nor a note that names one.
    #[test]
    fn a_luminaire_schedules_title_is_told_from_notes_and_other_tables() {
        assert!(is_luminaire_title("LIGHT FIXTURE SCHEDULE", false));
        assert!(is_luminaire_title("LUMINAIRE SCHEDULE - LEVEL 2", false));
        assert!(is_luminaire_title("Lighting Fixture Schedule", false));
        assert!(!is_luminaire_title("FIXTURE SCHEDULE", false));
        assert!(is_luminaire_title("FIXTURE SCHEDULE", true));
        assert!(!is_luminaire_title("PLUMBING FIXTURE SCHEDULE", true));
        assert!(!is_luminaire_title("LIGHTING CONTROL SCHEDULE", true));
        assert!(!is_luminaire_title("SWITCH. SEE ARCH SHEET X-2 FOR FIXTURE SCHEDULE.", true));
        assert!(!is_luminaire_title("REFER TO THE LIGHT FIXTURE SCHEDULE ON SHEET X-2 FOR TYPES", false));
        assert!(!is_luminaire_title("ON LIGHT FIXTURE SCHEDULE.", false));
        assert!(!is_luminaire_title("SEE LIGHT FIXTURE SCHEDULE", false));
        assert!(!is_luminaire_title("PER LUMINAIRE SCHEDULE", false));
    }

    #[test]
    fn makers_and_products_compare_by_their_words_and_series() {
        assert!(same_maker("COOPER", "Cooper Lighting Solutions"));
        assert!(!same_maker("COOPER", "Lithonia Lighting"));
        assert!(!same_maker("ILLUMINATION LIGHTING", "Lithonia Lighting"));
        assert!(same_product("ML4DM10FSL9FS1E-TL40R2GMW", "ML4DM10FSL9FS1E"));
        assert!(!same_product("SMX6RLSFS010", "LDN6 40/10"));
        assert!(same_product("ML4DM10FSL9FS1E-TL40R2GMW", "ML4DM10LSFL9FS1E/TL40R2GMW"));
        assert!(!same_product("CS71SD", "CX71SD"));
        assert!(same_maker("LITHONIA", "ACUITY"));
        assert!(!same_maker("SIGNIFY", "COOPER LIGHTING SOLUTIONS"));
        assert_eq!(type_of("luminaire XA"), "XA");
    }

    /// The schedule of fix-lum-schedule-type-column-wrong, its columns in one order or another:
    /// the fixture's TYPE, QTY, DESCRIPTION, MANUFACTURER, the lamp's TYPE and COLOR under a LAMPS
    /// group heading, and VOLTAGE (MVOLT, 120, UNV).
    fn lamps_schedule(order: &[usize]) -> GridTable {
        let cols: [(&str, &str, &[&str]); 7] = [
            ("", "TYPE", &["A", "B", "S"]),
            ("", "QTY", &["6", "4", "2"]),
            ("", "DESCRIPTION", &["RECESSED DOWNLIGHT", "SCONCE", "LINEAR STRIP"]),
            ("", "MANUFACTURER", &["MAKER ONE", "MAKER TWO", "MAKER ONE"]),
            ("LAMPS", "TYPE", &["LED", "LED", "LED"]),
            ("LAMPS", "COLOR", &["3000K", "-", "4000K"]),
            ("", "VOLTAGE", &["MVOLT", "120", "UNV"]),
        ];
        let cols: Vec<(&str, &str, &[&str])> = order.iter().map(|i| cols[*i]).collect();
        let (texts, segs) = drawn_schedule(0.2, &cols);
        let title = texts.first().cloned().unwrap_or_else(|| panic!("no title"));
        super::super::gridtable::read_at(&texts, &segs, &title, "X-6").unwrap_or_else(|| panic!("no table: {texts:?}"))
    }

    /// fix-lum-schedule-type-column-wrong: the marks are the first column headed TYPE that is no
    /// lamp's (under its LAMPS group heading), in either column order; a voltage (MVOLT, 120,
    /// UNV) is never a mark, and the TAG inside VOLTAGE is no heading.
    #[test]
    fn the_marks_are_the_first_type_column_never_a_lamps_type_or_a_voltage() {
        for order in [[0, 1, 2, 3, 4, 5, 6], [4, 5, 6, 1, 0, 2, 3]] {
            let t = lamps_schedule(&order);
            let lamp_type = t.headers.iter().position(|h| h == "LAMPS TYPE");
            assert!(lamp_type.is_some() && t.headers.iter().any(|h| h == "LAMPS COLOR"), "{order:?}: {:?}", t.headers);
            assert_eq!(t.headers.iter().filter(|h| *h == "TYPE").count(), 1, "{order:?}: {:?}", t.headers);
            let (types, _) = fixture_types(&t, &BTreeSet::new());
            let tags: Vec<&str> = types.iter().map(|f| f.tag.as_str()).collect();
            assert_eq!(tags, ["A", "B", "S"], "{order:?}: {t:#?}");
            let a = &types[0];
            assert_eq!(
                (a.quantity, a.voltage.as_str(), a.lamp.as_str(), a.description.as_str()),
                (Some(6), "MVOLT", "LED", "RECESSED DOWNLIGHT"),
                "{order:?}"
            );
        }
        // Read by words: TAG is in VOLTAGE's letters, not one of its words.
        let t = lamps_schedule(&[0, 1, 2, 3, 4, 5, 6]);
        assert!(!heads(&t, 6, "TAG") && heads(&t, 0, "TYPE") && heads(&t, 4, "TYPE"));
        // Headings with no rule under their group: the leftmost TYPE.
        let row = |c: &[&str]| c.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let t = GridTable {
            headers: row(&["VOLTAGE", "TYPE", "DESCRIPTION", "TYPE", "COLOR"]),
            rows: vec![row(&["120V", "F1", "DOWNLIGHT", "LED", "3000K"]), row(&["277V", "F2", "TROFFER", "LED", "4000K"])],
            ..GridTable::default()
        };
        assert_eq!(fixture_types(&t, &BTreeSet::new()).0.iter().map(|f| f.tag.as_str()).collect::<Vec<_>>(), ["F1", "F2"]);
    }

    #[test]
    fn a_voltage_is_never_a_type_mark() {
        for v in ["MVOLT", "mvolt", "UNV", "120", "277V", "120/277", "347-480", "24VDC", "120 V", "208VAC"] {
            assert!(voltage_shaped(v), "{v}");
            assert!(!mark_shaped(v), "{v}");
        }
        for v in ["A", "AE", "F1", "V1", "XA", "S2", "LED"] {
            assert!(!voltage_shaped(v), "{v}");
            assert!(mark_shaped(v), "{v}");
        }
    }

    /// The plans' tags check the column: when the first type column's cells are on no plan and
    /// another column's are, that one; with no tags, or tags that match it, the first.
    #[test]
    fn the_plans_tags_pick_the_column_of_marks() {
        let row = |c: &[&str]| c.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let t = GridTable {
            headers: row(&["TYPE", "FIXTURE", "DESCRIPTION"]),
            rows: vec![row(&["L1", "A", "DOWNLIGHT"]), row(&["L2", "B", "SCONCE"])],
            ..GridTable::default()
        };
        let tags = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<BTreeSet<String>>();
        let marks = |p: &BTreeSet<String>| fixture_types(&t, p).0.into_iter().map(|f| f.tag).collect::<Vec<_>>();
        assert_eq!(marks(&BTreeSet::new()), ["L1", "L2"]);
        assert_eq!(marks(&tags(&["A", "B", "ZZ"])), ["A", "B"]);
        assert_eq!(marks(&tags(&["L2"])), ["L1", "L2"]);
        assert_eq!(marks(&tags(&["QQ"])), ["L1", "L2"], "no column on the plans: the first");
    }
}
