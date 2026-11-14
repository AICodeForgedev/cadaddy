//! Each level's floor is its documented assembly (Q6408): a made-up four-storey building whose
//! plans' notes give level 1 a 4" slab on grade and levels 2 to 4 a gypsum concrete topping on
//! sheathing on I-joists. The floor elements are those thicknesses, the joists stand under the
//! framed floors' decks as their own pieces, and a floor nothing gives is the build's default
//! slab, marked a default (before: four 6" slabs, the framed floors' lower part through the
//! joists).

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// The plate, inches: 60' by 40', 7 1/4" exterior walls.
const LEN: f64 = 720.0;
const WID: f64 = 480.0;
const T: f64 = 7.25;

const SLAB_NOTE: &str = "4\" THICK SLAB ON GRADE";
const FRAMED_NOTE: &str = "3/4\" GYPSUM CONCRETE ON 3/4\" SHEATHING ON 11 7/8\" I-JOISTS";

/// The building; `notes` puts each level's floor note beside its plan.
fn building(notes: bool, slab_ft: Option<f64>) -> (Session, Value) {
    let mut s = Session::new();
    let mut levels = Vec::new();
    for i in 0..4 {
        let ox = f64::from(i) * 2000.0;
        layer(&mut s, "A-WALL");
        rect(&mut s, [ox, 0.0], [ox + LEN, WID]);
        rect(&mut s, [ox + T, T], [ox + LEN - T, WID - T]);
        if notes {
            layer(&mut s, "A-ANNO");
            let text = if i == 0 { SLAB_NOTE } else { FRAMED_NOTE };
            s.execute("text", &json!({"at": [ox, -60.0], "text": text, "height": 6.0})).unwrap();
        }
        levels.push(json!({"name": format!("{}", i + 1), "elevation_ft": f64::from(i) * 10.0, "height_ft": 10, "ceiling_ft": 9,
            "region": [ox - 100.0, -100, ox + 1000.0, 700]}));
    }
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "stack": true,
        "levels": levels,
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [{"name": "X1", "thickness_in": T, "from": "pairs"}],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    let mut p = json!({});
    if let Some(t) = slab_ft {
        p["slabThickness"] = json!(t);
    }
    let r = s.execute("model.build", &p).unwrap();
    (s, r)
}

/// A mesh element's (bottom, top) z, drawing units (inches).
fn z_range(e: &cadaddy_doc::Entity) -> Option<(f64, f64)> {
    let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { return None };
    let lo = m.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
    let hi = m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
    lo.is_finite().then_some((lo, hi))
}

fn level_of(e: &cadaddy_doc::Entity) -> Option<&str> {
    e.common.xdata.get(2).map(|t| t.value.as_str())
}

/// Each level's floor element: (handle, bottom, top).
fn floors(s: &Session) -> Vec<(String, cadaddy_doc::Handle, f64, f64)> {
    let d = s.doc().unwrap();
    let mut out: Vec<(String, cadaddy_doc::Handle, f64, f64)> = d
        .model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("floor"))
        .filter_map(|e| z_range(e).map(|(lo, hi)| (level_of(e).unwrap_or_default().to_string(), e.handle, lo, hi)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A level's joists' (lowest bottom, highest top), inches.
fn joists(s: &Session, level: &str) -> Option<(f64, f64, usize)> {
    let d = s.doc().unwrap();
    let zs: Vec<(f64, f64)> = d
        .model
        .iter()
        .filter(|e| e.common.xdata.iter().any(|t| t.value == "part:atom:joist"))
        .filter(|e| level_of(e) == Some(level))
        .filter_map(|e| z_range(e))
        .collect();
    let lo = zs.iter().map(|z| z.0).fold(f64::INFINITY, f64::min);
    let hi = zs.iter().map(|z| z.1).fold(f64::NEG_INFINITY, f64::max);
    lo.is_finite().then_some((lo, hi, zs.len()))
}

fn props(s: &mut Session, h: cadaddy_doc::Handle) -> Value {
    s.execute("bim.props.get", &json!({"handle": h.hex()})).unwrap()["elements"][0]["props"].clone()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn each_level_is_built_as_its_floor_notes_give_it() {
    let (mut s, r) = building(true, None);
    let fl = floors(&s);
    assert_eq!(fl.len(), 4, "{fl:?}");
    for (level, h, lo, hi) in fl.clone() {
        let top = (level.parse::<f64>().unwrap() - 1.0) * 120.0;
        assert!(near(hi, top), "level {level}: the floor's top at its elevation: {hi} vs {top}");
        let want = if level == "1" { 4.0 } else { 1.5 };
        assert!(near(hi - lo, want), "level {level}: the floor is {}\" thick, not {want}\": {r}", hi - lo);
        let p = props(&mut s, h);
        assert_eq!(p["thickness"]["value"], json!(want), "level {level}: {p}");
        assert_eq!(p["thickness"]["source"]["kind"], json!("note"), "level {level}: documented: {p}");
        let asm = p["assembly"]["value"].as_str().unwrap_or_default().to_string();
        if level == "1" {
            assert!(asm.contains("slab on grade"), "{asm}");
            assert_eq!(p["type"]["value"], json!("slab"));
        } else {
            assert!(asm.contains("gypsum concrete") && asm.contains("sheathing") && asm.contains("joists"), "{asm}");
            assert_eq!(p["type"]["value"], json!("framed floor"));
        }
    }
    // The joists stand under each framed floor's deck, as deep as its note says, never in it.
    for (level, _, lo, _) in fl.iter().filter(|f| f.0 != "1") {
        let (jlo, jhi, n) = joists(&s, level).unwrap_or_else(|| panic!("level {level}: joists laid out"));
        assert!(n > 0);
        assert!(near(jhi, *lo), "level {level}: the joists' tops at the deck's underside: {jhi} vs {lo}");
        assert!(near(jhi - jlo, 11.875), "level {level}: 11 7/8\" joists: {}", jhi - jlo);
    }
    // The build reports each floor's assembly.
    let report = r.to_string();
    assert!(report.contains("its floor is 3/4") && report.contains("documented"), "{report}");
}

#[test]
fn a_floor_nothing_gives_is_the_builds_default_slab_marked_a_default() {
    let (mut s, _) = building(false, None);
    for (level, h, lo, hi) in floors(&s) {
        assert!(near(hi - lo, 6.0), "level {level}: the default 6\" slab: {}", hi - lo);
        let p = props(&mut s, h);
        assert_eq!(p["thickness"]["value"], json!(6.0), "{p}");
        assert_eq!(p["thickness"]["source"]["kind"], json!("default"), "marked a default: {p}");
    }
    // The build's slab thickness is the default; it never overrides a documented floor.
    let (s, _) = building(false, Some(0.75));
    assert!(floors(&s).iter().all(|(_, _, lo, hi)| near(hi - lo, 9.0)));
    let (s, _) = building(true, Some(0.75));
    let fl = floors(&s);
    assert!(fl.iter().all(|(l, _, lo, hi)| near(hi - lo, if l == "1" { 4.0 } else { 1.5 })), "{fl:?}");
}

/// The job's floor notes (read off each level's sheets into the spec) and its floor framing give
/// the floors too, with no note on the plans.
#[test]
fn the_specs_floor_notes_and_framing_give_the_floors() {
    let (mut s, _) = building(false, None);
    let patch = json!({
        "floor_notes": [{"level": "1", "text": "5\" CONCRETE SLAB ON GRADE W/ 15 MIL VAPOR BARRIER", "source": "S-1"}],
        "floor_framing": [
            {"level": "2", "member": "joist", "spacing_in": 24, "depth_in": 16, "depth_status": "read", "spacing_status": "read",
             "source": "S-2: 1 1/2\" GYPCRETE OVER 23/32\" OSB ON 16\" DEEP I-JOISTS @ 24\" OC"},
            {"level": "3", "member": "joist", "spacing_in": 24, "depth_in": 14, "depth_status": "read", "spacing_status": "read",
             "source": "S-3: 14\" DEEP I-JOISTS @ 24\" OC"}
        ]
    });
    s.execute("bspec.set", &json!({"spec": patch})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let fl = floors(&s);
    let t = |l: &str| fl.iter().find(|f| f.0 == l).map(|f| f.3 - f.2).unwrap();
    assert!(near(t("1"), 5.0), "{fl:?}");
    assert!(near(t("2"), 1.5 + 23.0 / 32.0), "topping and deck from the framing note: {fl:?}");
    // Framed, no deck given: the trade rules' subfloor, derived.
    assert!(near(t("3"), 0.75), "{fl:?}");
    let h3 = fl.iter().find(|f| f.0 == "3").map(|f| f.1).unwrap();
    assert_eq!(props(&mut s, h3)["thickness"]["source"]["kind"], json!("rule"));
    // Level 4: nothing gives it.
    assert!(near(t("4"), 6.0), "{fl:?}");
    let (jlo, jhi, _) = joists(&s, "2").unwrap();
    let lo2 = fl.iter().find(|f| f.0 == "2").map(|f| f.2).unwrap();
    assert!(near(jhi, lo2) && near(jhi - jlo, 16.0), "{jlo} {jhi} {lo2}");
}
