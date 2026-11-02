//! Lighting from its documents (Q6144, first part): the light fixture schedule read by its grid
//! ([`super::gridtable`]), each luminaire given its type's row (basis of design, lamp, VA, voltage,
//! installation, description) with the schedule as its source, and the documents checked against
//! each other: a plan's fixture type the schedule doesn't list, a scheduled type no plan shows, a
//! submitted luminaire that isn't the schedule's basis of design (maker or catalog) — the schedule's
//! own "no substitutions" note said beside it.

use cadaddy_walk3d::FixtureType;

use super::gridtable::GridTable;

/// The schedule's rows (by its columns' names), and whether its notes forbid substitutions.
pub(crate) fn fixture_types(t: &GridTable) -> (Vec<FixtureType>, bool) {
    let tag = t.column(&["TAG"]).or_else(|| t.column(&["TYPE"])).or_else(|| t.column(&["MARK"]));
    let maker = t.column(&["MANUFACTURER"]).or_else(|| t.column(&["BASIS"]));
    let model = t.column(&["MODEL"]).or_else(|| t.column(&["CATALOG"]));
    let lamp = t.column(&["LAMP"]).or_else(|| t.column(&["SOURCE"]));
    let volts = t.column(&["VOLT"]);
    let install = t.column(&["INSTALL"]).or_else(|| t.column(&["MOUNTING"]));
    let desc = t.column(&["DESCRIPTION"]);
    let notes = t.column(&["NOTES"]);
    let mut out = Vec::new();
    let mut no_subs = false;
    for r in &t.rows {
        let all = r.join(" ").to_ascii_uppercase();
        if all.contains("NO SUBSTITU") {
            no_subs = true;
        }
        let tg = t.cell(r, tag);
        // A type is short (`A`, `F1`, `XA`); a note row's text isn't.
        if tg.is_empty() || tg.len() > 6 || tg.contains(' ') {
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
        });
    }
    (out, no_subs)
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
    const GENERIC: [&str; 10] = ["LIGHTING", "LIGHT", "ELECTRIC", "SOLUTIONS", "PRODUCTS", "COMPANY", "CORP", "GROUP", "INTERNATIONAL", "INDUSTRIES"];
    let words = |v: &str| {
        v.to_ascii_uppercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.len() >= 4 && !GENERIC.contains(w))
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
        }
    }

    #[test]
    fn a_fixture_schedule_gives_each_type_and_its_no_substitutions_note() {
        let (types, no_subs) = fixture_types(&table());
        assert!(no_subs);
        assert_eq!(types.len(), 2);
        assert_eq!((types[0].tag.as_str(), types[0].manufacturer.as_str(), types[0].va), ("A", "COOPER", Some(13.0)));
        assert_eq!(types[1].manufacturer, "ILLUMINATION LIGHTING");
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
}
