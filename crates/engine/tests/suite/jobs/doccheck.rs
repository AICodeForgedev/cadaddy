//! The documents checked against each other (Q6648): a made-up two-level set whose sheets say
//! things that cannot all be built. A plumbing note routes water mains above the corridor
//! ceiling, which hangs 3 in under the corridor's rated lid; the fire protection section puts the
//! second floor 9 in above the architectural sections; the sprinkler design leaves the floor
//! cavity unsprinklered as "fully insulated" while the floor assembly gives 3 1/2" of batt in
//! 16" trusses. `rfi.read` raises each as a conflict that cites both documents, with the
//! corrected value the field builds; a set whose documents agree raises none.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, EntityKind, HAlign, Text, VAlign};
use cadaddy_engine::geom::Vec3;
use cadaddy_engine::walk3d::BuildingSpec;
use serde_json::{Value, json};

/// The set's sheets side by side: number, discipline, level, where.
const SHEETS: [(&str, &str, Option<&str>, f64); 4] =
    [("A-101", "A", Some("L1"), 0.0), ("P-101", "P", Some("L1"), 2000.0), ("A-301", "A", None, 4000.0), ("FP-301", "FP", None, 6000.0)];

fn sheet_x(n: &str) -> f64 {
    SHEETS.iter().find(|s| s.0 == n).map(|s| s.3).unwrap()
}

/// A line of lettering on a sheet, 10 units tall, `row` lines down from its top.
fn text(s: &mut Session, sheet: &str, x: f64, row: f64, value: &str) {
    let at = Vec3::new(sheet_x(sheet) + x, 900.0 - row * 15.0, 0.0);
    s.add_entity(EntityKind::Text(Text {
        insert: at,
        align_pt: None,
        height: 10.0,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    }))
    .unwrap();
}

/// The set: `conflicting` plants the three conflicts, else the documents agree.
fn set(conflicting: bool) -> Session {
    let mut d = Drawing::new_imperial();
    let sheets: Vec<Value> = SHEETS
        .iter()
        .map(|(n, disc, lv, x)| {
            let mut v = json!({"number": n, "discipline": disc, "rect": [x, 0.0, x + 1500.0, 1000.0]});
            if let Some(l) = lv {
                v["level"] = json!(l);
            }
            v
        })
        .collect();
    let sp = BuildingSpec::from_json(
        &json!({
            "version": 1,
            "units_per_foot": 12.0,
            "levels": [
                {"name": "L1", "elevation_ft": 0.0, "height_ft": 10.0},
                {"name": "L2", "elevation_ft": 10.0, "height_ft": 10.0}
            ],
            "rooms": [
                {"name": "CORRIDOR", "number": "1001", "level": "L1", "ceiling_ft": 8.0, "ceiling_source": "A-101 RCP tag 8'-0\""},
                {"name": "OFFICE", "number": "1002", "level": "L1", "ceiling_ft": 9.0}
            ],
            "floor_framing": [
                {"level": "L2", "member": "truss", "spacing_in": 24.0, "depth_in": 16.0, "source": "S-201: 16\" FLOOR TRUSSES @ 24\" O.C."}
            ],
            "sheets": sheets
        })
        .to_string(),
    )
    .unwrap();
    d.building_spec = Some(sp.to_json()).into();
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Building", None);
    // The plumbing plan's note, one text per printed line.
    text(&mut s, "P-101", 10.0, 0.0, "1.");
    text(&mut s, "P-101", 40.0, 0.0, "RUN 2\" DOMESTIC WATER MAINS TO EACH UNIT");
    text(&mut s, "P-101", 40.0, 1.0, "ABOVE THE CORRIDOR CEILING. SEE RISERS.");
    // The architectural plan's lid note: 3 in over the ceiling, or a foot.
    let lid = if conflicting { "8'-3\"" } else { "9'-0\"" };
    text(&mut s, "A-101", 10.0, 0.0, &format!("1 HR RATED CORRIDOR LID AT {lid} A.F.F."));
    // The architectural section: the levels the model is built on, and the floor assembly.
    text(&mut s, "A-301", 10.0, 0.0, "LEVEL 1");
    text(&mut s, "A-301", 10.0, 1.0, "100'-0\"");
    text(&mut s, "A-301", 10.0, 4.0, "LEVEL 2");
    text(&mut s, "A-301", 10.0, 5.0, "110'-0\"");
    let batt = if conflicting { "3 1/2\"" } else { "16\"" };
    text(&mut s, "A-301", 400.0, 10.0, "FLOOR/CEILING ASSEMBLY: 3/4\" SHEATHING ON 16\" FLOOR TRUSSES");
    text(&mut s, "A-301", 400.0, 11.0, &format!("@ 24\" O.C. WITH {batt} SOUND ATTENUATION BATT INSULATION."));
    // The fire protection section: its own levels, and its note on the floor cavity.
    text(&mut s, "FP-301", 10.0, 0.0, "1ST FLOOR F.F.");
    text(&mut s, "FP-301", 10.0, 1.0, "100' - 0\"");
    text(&mut s, "FP-301", 10.0, 4.0, "2ND FLOOR F.F.");
    text(&mut s, "FP-301", 10.0, 5.0, if conflicting { "110' - 9\"" } else { "110' - 0\"" });
    text(&mut s, "FP-301", 400.0, 10.0, "FLOOR CAVITY TO BE");
    text(&mut s, "FP-301", 400.0, 11.0, "FULLY INSULATED BY OTHERS");
    s
}

fn by_check<'a>(r: &'a Value, check: &str) -> &'a Value {
    r["documents"]["by_check"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["check"] == check)
        .unwrap_or_else(|| panic!("no check {check}: {}", r["documents"]))
}

fn conflicts<'a>(r: &'a Value, check: &str) -> Vec<&'a Value> {
    r["items"].as_array().unwrap().iter().filter(|i| i["topic"] == format!("documents: {check}")).collect()
}

#[test]
fn documents_that_disagree_raise_one_conflict_each_with_its_correction() {
    let mut s = set(true);
    let r = s.execute("rfi.read", &json!({})).unwrap();
    assert_eq!(r["built"], true, "{r}");
    // (1) The mains need 2" + 1" insulation + 1 1/2" hanger; the lid leaves 3".
    let c = conflicts(&r, "ceiling space");
    assert_eq!(c.len(), 1, "{r}");
    let item = c[0];
    assert_eq!(item["kind"], "conflict");
    assert_eq!(item["level"], "L1");
    let issue = item["issue"].as_str().unwrap();
    assert!(issue.contains("P-101") && issue.contains("corridor") && issue.contains("3\" free"), "{issue}");
    assert!(item["assumed"].as_str().unwrap().contains("7'-10 1/2\""), "the corrected ceiling: {item}");
    let sides: Vec<&str> = item["sides"].as_array().unwrap().iter().map(|x| x["document"].as_str().unwrap()).collect();
    assert!(sides.contains(&"P-101") && sides.iter().any(|d| d.starts_with("A-101")), "{sides:?}");
    assert_eq!(by_check(&r, "ceiling space")["raised"], 1);
    // (2) The fire protection section's second floor, 9" above the architectural one.
    let c = conflicts(&r, "level datums");
    assert_eq!(c.len(), 1, "{r}");
    assert_eq!(c[0]["level"], "L2");
    let issue = c[0]["issue"].as_str().unwrap();
    assert!(issue.contains("FP") && issue.contains("FP-301") && issue.contains("9\" above"), "{issue}");
    assert!(c[0]["assumed"].as_str().unwrap().contains("10'-0\""), "built at the architectural level: {}", c[0]);
    // The architectural section agrees with the model: only FP's levels conflict; both levels compared.
    assert!(by_check(&r, "level datums")["compared"].as_u64().unwrap() >= 4, "{}", r["documents"]);
    // (3) A cavity "fully insulated" against 3 1/2" of batt in 16" trusses.
    let c = conflicts(&r, "design conditions");
    assert_eq!(c.len(), 1, "{r}");
    let issue = c[0]["issue"].as_str().unwrap();
    assert!(issue.contains("FP-301") && issue.contains("3 1/2\"") && issue.contains("16\""), "{issue}");
    assert!(c[0]["assumed"].as_str().unwrap().contains("12 1/2\" more insulation"), "{}", c[0]);
    assert!(c[0]["sides"].as_array().unwrap().iter().any(|x| x["document"].as_str().unwrap().starts_with("NFPA 13")), "{}", c[0]);
    // Apply keeps them in the spec with stable ids; a second read finds the same ones.
    let a = s.execute("rfi.read", &json!({"apply": true})).unwrap();
    assert_eq!(a["applied_documents"], 3, "{a}");
    let listed = s.execute("rfi.list", &json!({})).unwrap();
    let text = listed.to_string();
    for check in ["ceiling space", "level datums", "design conditions"] {
        assert!(text.contains(&format!("documents: {check}")), "{check} not kept: {text}");
    }
    let again = s.execute("rfi.read", &json!({})).unwrap();
    let ids = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["topic"].as_str().is_some_and(|t| t.starts_with("documents: ")))
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids(&r), ids(&again));
    assert_eq!(again["total"], 3, "nothing raised twice: {again}");
}

#[test]
fn documents_that_agree_raise_no_conflict() {
    let mut s = set(false);
    let r = s.execute("rfi.read", &json!({})).unwrap();
    let docs: Vec<&Value> =
        r["items"].as_array().unwrap().iter().filter(|i| i["topic"].as_str().is_some_and(|t| t.starts_with("documents: "))).collect();
    assert!(docs.is_empty(), "{docs:?}");
    // Each check still ran over what the documents give: measured, not a quiet zero.
    for check in ["ceiling space", "level datums", "design conditions"] {
        let c = by_check(&r, check);
        assert!(c["compared"].as_u64().unwrap() >= 1, "{check}: {}", r["documents"]);
        assert_eq!(c["raised"], 0, "{check}: {}", r["documents"]);
    }
}
