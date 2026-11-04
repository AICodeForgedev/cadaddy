//! G-COVERAGE (Q6183) on a made-up one-storey job: a 40' × 24' shell with a corridor (100) and
//! two rooms (101, 102). Its mechanical plan M-101 carries a legend (`QSD` a combination fire/smoke
//! damper, `QVD` a volume damper, `QXD` a dual duct detector) and an abbreviation list (`QFD`
//! FIRE DAMPER, `AFF` above finished floor); an enlarged plan M-501 of part of the level
//! (registered onto M-101) repeats some of its tags; the power plan E-101 (registered too) has a
//! keynote list whose note powers a motorized damper, with its marker. The codes, sheets and
//! rooms are made up.

use std::path::PathBuf;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Level 1's plan origin on the drawing (inches).
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
/// Where E-101 and M-501 sit on the drawing, beside M-101 (the level's plan).
const E_DX: f64 = 4000.0;
const ENL_DY: f64 = -3000.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on the level's plan, in drawing inches.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

/// The same building point on a sheet offset by `d` (E-101: x; M-501: y).
fn on(p: [f64; 2], d: [f64; 2]) -> [f64; 2] {
    let q = at(p);
    [q[0] + d[0], q[1] + d[1]]
}

const E: [f64; 2] = [E_DX, 0.0];
const ENL: [f64; 2] = [0.0, ENL_DY];
const M: [f64; 2] = [0.0, 0.0];

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let v = [at(a), at([b[0], a[1]]), at(b), at([a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [at(a), at(b)]})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": p, "text": t, "height": h})).unwrap();
}

/// A tag (its code) by a duct, at a building point of a sheet.
fn tag(s: &mut Session, p: [f64; 2], sheet: [f64; 2], code: &str) {
    layer(s, "M-ANNO-TAG");
    text(s, on(p, sheet), code, 6.0);
}

/// A keynote marker: its number in a hexagon.
fn marker(s: &mut Session, p: [f64; 2], sheet: [f64; 2], n: &str) {
    let q = on(p, sheet);
    layer(s, "E-ANNO-KEYN");
    text(s, q, n, 6.0);
    let c = [q[0] + 2.0, q[1] + 3.0];
    let v: Vec<[f64; 2]> = (0..6).map(|i| f64::from(i) * std::f64::consts::PI / 3.0).map(|t| [c[0] + 9.0 * t.cos(), c[1] + 9.0 * t.sin()]).collect();
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

/// M-101's legend and abbreviation list, beside the plan.
fn mechanical_legend(s: &mut Session) {
    layer(s, "M-ANNO-TEXT");
    let o = at([50.0, 22.0]);
    text(s, o, "MECHANICAL SYMBOL LEGEND", 9.0);
    for (i, (code, words)) in
        [("QSD", "COMBINATION FIRE/SMOKE DAMPER, UL 555S"), ("QVD", "MANUAL VOLUME DAMPER"), ("QXD", "DUAL DUCT DETECTOR")].iter().enumerate()
    {
        let y = o[1] - 16.0 - 12.0 * i as f64;
        text(s, [o[0], y], code, 6.0);
        text(s, [o[0] + 60.0, y], words, 6.0);
    }
    let a = at([50.0, 6.0]);
    text(s, a, "ABBREVIATIONS", 9.0);
    for (i, (code, words)) in [("AFF", "ABOVE FINISHED FLOOR"), ("QFD", "FIRE DAMPER"), ("TYP", "TYPICAL")].iter().enumerate() {
        let y = a[1] - 16.0 - 12.0 * i as f64;
        text(s, [a[0], y], code, 6.0);
        text(s, [a[0] + 40.0, y], words, 6.0);
    }
}

/// E-101's keynote list: note 1 powers a motorized damper (no sheet's legend names one).
fn electrical_keynotes(s: &mut Session) {
    layer(s, "E-ANNO-TEXT");
    let o = on([50.0, 22.0], E);
    text(s, o, "ELECTRICAL KEYNOTES:", 9.0);
    let lines = [
        (0.0, "1. PROVIDE 120V POWER CONNECTION TO MOTORIZED DAMPER"),
        (14.0, "ACTUATOR ABOVE THE CEILING."),
        (0.0, "2. RECEPTACLE AT 48\" AFF."),
    ];
    for (i, (dx, t)) in lines.iter().enumerate() {
        text(s, [o[0] + dx, o[1] - 14.0 - 10.0 * i as f64], t, 6.0);
    }
}

/// Device blocks of a kind at building points (feet) of the level's plan.
fn devices(s: Session, items: &[([f64; 2], &str)]) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key("DEV") {
        let mut b = Block::new("DEV");
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert("DEV".into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for (p, ty) in items {
        let attribs = vec![Attrib {
            tag: "TYPE".into(),
            text: Text { value: ty.to_string(), ..style.clone() },
            invisible: true,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        }];
        let q = at(*p);
        let ins = Insert {
            block: "DEV".into(),
            insert: Vec3::new(q[0], q[1], 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs,
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Insert(ins));
        e.common.layer = "M-DEVICE".into();
        d.model.push(e);
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
}

/// The drawing's print inventory, as the job's BIM build keeps it.
fn with_inventory(s: Session, marks: Value) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    let inv = json!({"marks": marks});
    d.embedded.insert("print-inventory.json".into(), Arc::new(serde_json::to_vec(&inv).unwrap()));
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

fn spec() -> Value {
    let dev = json!({"ceiling": true, "aff_ft": 9.5, "size_ft": [1.0, 1.0, 0.5]});
    let reg = |reference: &str, tx: f64, ty: f64| json!({"reference": reference, "method": "reference", "a": 1.0, "b": 0.0, "tx": tx, "ty": ty, "matched": 4});
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "M-DEVICE", "role": "device"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [
            {"name": "Corridor", "number": "100", "level": "L1"},
            {"name": "Storage", "number": "101", "level": "L1"},
            {"name": "Office", "number": "102", "level": "L1"}
        ],
        "devices": {"by_type": {"QSD": dev, "QFD": dev}},
        "sheets": [
            {"number": "M-101", "title": "MECHANICAL PLAN - L1", "discipline": "M", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
             "registration": reg("M-101", 0.0, 0.0)},
            {"number": "M-501", "title": "ENLARGED MECHANICAL PLAN - L1", "discipline": "M", "level": "L1", "source": "model",
             "rect": [0.0, ENL_DY, SW, ENL_DY + SH], "registration": reg("M-101", 0.0, -ENL_DY)},
            {"number": "E-101", "title": "POWER PLAN - L1", "discipline": "E", "level": "L1", "source": "model", "rect": [E_DX, 0.0, E_DX + SW, SH],
             "registration": reg("M-101", -E_DX, 0.0)}
        ]
    })
}

/// The job: the shell and rooms, M-101's legend and tags (three QSD, two QFD, four QXD),
/// M-501 repeating two of the QSD (and nothing else), E-101's keynote and its marker, and the
/// model's dampers (`qsd` of the QSD kind, `qfd` of the QFD kind).
fn job(qsd: usize, qfd: usize) -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [40.0, 24.0]);
    rect(&mut s, [0.5, 0.5], [39.5, 23.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [0.5, 12.0], [39.5, 12.0]);
    line(&mut s, [20.0, 12.0], [20.0, 23.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([30.0, 6.0]), "100", 6.0);
    text(&mut s, at([10.0, 18.0]), "101", 6.0);
    text(&mut s, at([30.0, 18.0]), "102", 6.0);
    mechanical_legend(&mut s);
    // M-101: three fire/smoke dampers, two fire dampers, four duct detectors.
    for p in [[5.0, 12.0], [15.0, 12.0], [25.0, 12.0]] {
        tag(&mut s, p, M, "QSD");
    }
    tag(&mut s, [20.0, 16.0], M, "QFD-1");
    tag(&mut s, [20.0, 20.0], M, "QFD-2");
    for p in [[8.0, 4.0], [16.0, 4.0], [24.0, 4.0], [32.0, 4.0]] {
        tag(&mut s, p, M, "QXD");
    }
    // M-501 (the enlarged plan) shows two of the same fire/smoke dampers.
    tag(&mut s, [5.0, 12.0], ENL, "QSD");
    tag(&mut s, [15.2, 12.1], ENL, "QSD");
    // E-101: the keynote powering a motorized damper, and its marker in room 102.
    electrical_keynotes(&mut s);
    marker(&mut s, [30.0, 20.0], E, "1");
    let mut items: Vec<([f64; 2], &str)> = Vec::new();
    for p in [[5.0, 12.3], [15.0, 12.3], [25.0, 12.3]].iter().take(qsd) {
        items.push((*p, "QSD"));
    }
    for p in [[20.3, 16.0], [20.3, 20.0]].iter().take(qfd) {
        items.push((*p, "QFD"));
    }
    let (s, hs) = if items.is_empty() { (s, Vec::new()) } else { devices(s, &items) };
    let mut s = s;
    let r = s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    for (h, (_, ty)) in hs.iter().zip(&items) {
        let kind = if *ty == "QSD" { "fire_smoke_damper" } else { "fire_damper" };
        s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "M-101", "kind": kind})).unwrap();
    }
    s
}

fn coverage(s: &mut Session) -> (Value, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"all": true, "limit": 50, "rules": ["G-COVERAGE"]})).unwrap();
    let fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    assert!(fs.iter().all(|f| f["rule"] == "G-COVERAGE"), "{r}");
    (r, fs)
}

fn kind<'a>(fs: &'a [Value], key: &str) -> &'a Value {
    fs.iter().find(|f| f["kind"] == key).unwrap_or_else(|| panic!("no G-COVERAGE finding for {key}: {}", Value::Array(fs.to_vec())))
}

#[test]
fn the_rule_is_built_in_with_its_count_tolerances() {
    let mut s = Session::new();
    s.execute("new", &json!({})).unwrap();
    let r = s.execute("validate.rules", &json!({"id": "G-COVERAGE"})).unwrap();
    let rule = &r["rules"][0];
    assert_eq!(rule["enabled"], true, "{r}");
    assert_eq!(rule["severity"], "high");
    assert_eq!(rule["tolerance"]["count_abs"], 1.0, "{r}");
    assert_eq!(rule["tolerance"]["count_share"], 0.1, "{r}");
    assert_eq!(rule["tolerance"]["dedupe_ft"], 3.0, "{r}");
    assert_eq!(rule["tolerance"]["repeat_sheets"], 2.0, "{r}");
}

/// (1) A legend-defined kind tagged on the plans with no model element fails, ranked first; so
/// does an abbreviation-defined one. An abbreviation that names no object (AFF) is no kind.
#[test]
fn a_legend_kind_tagged_on_the_plans_with_no_model_elements_fails_first() {
    let mut s = job(0, 0);
    let (r, fs) = coverage(&mut s);
    let qsd = kind(&fs, "combination fire smoke damper");
    assert_eq!(qsd["verdict"], "fail", "{qsd}");
    assert_eq!(qsd["severity"], "high");
    assert_eq!(qsd["measured"]["model"], 0);
    assert_eq!(qsd["measured"]["plan_marks"], 3, "{qsd}");
    assert!(qsd["issue"].as_str().unwrap().contains("3 marks on M-101 (3), M-501 (2), 0 in the model"), "{qsd}");
    assert_eq!(qsd["measured"]["defined_by"][0]["from"], "legend", "{qsd}");
    assert_eq!(qsd["measured"]["defined_by"][0]["sheet"], "M-101");
    let qfd = kind(&fs, "fire damper");
    assert_eq!(qfd["verdict"], "fail", "{qfd}");
    assert_eq!(qfd["measured"]["plan_marks"], 2, "numbered tags QFD-1, QFD-2: {qfd}");
    assert_eq!(qfd["measured"]["defined_by"][0]["from"], "abbreviation", "{qfd}");
    assert!(fs.iter().all(|f| f["kind"] != "above finished floor"), "AFF is no object");
    // Zero-element kinds come before any other finding.
    let first_pass = fs.iter().position(|f| f["verdict"] == "pass").unwrap_or(fs.len());
    let last_zero = fs.iter().rposition(|f| f["verdict"] == "fail" && f["measured"]["model"] == 0).unwrap();
    assert!(last_zero < first_pass, "{r}");
    assert_eq!(fs[0]["measured"]["model"], 0, "{}", fs[0]);
    // The answer's table and counts.
    let cov = &r["coverage"];
    assert!(cov["zero_in_model"].as_u64().unwrap() >= 3, "{cov}");
    assert!(r["checked"]["G-COVERAGE"]["fail"].as_u64().unwrap() >= 3, "{r}");
}

/// (2) Counts that match pass; a count off by more than the tolerance fails with both counts.
#[test]
fn counts_that_match_pass_and_a_short_count_fails_with_both() {
    let mut s = job(3, 2);
    let (_, fs) = coverage(&mut s);
    let qsd = kind(&fs, "combination fire smoke damper");
    assert_eq!(qsd["verdict"], "pass", "{qsd}");
    assert_eq!((qsd["measured"]["plan_marks"].as_u64(), qsd["measured"]["model"].as_u64()), (Some(3), Some(3)), "{qsd}");
    let qfd = kind(&fs, "fire damper");
    assert_eq!(qfd["verdict"], "pass", "{qfd}");
    // One QSD in the model for three on the plans: off by two, more than one.
    let mut s = job(1, 2);
    let (_, fs) = coverage(&mut s);
    let qsd = kind(&fs, "combination fire smoke damper");
    assert_eq!(qsd["verdict"], "fail", "{qsd}");
    assert_eq!(qsd["severity"], "medium");
    assert!(qsd["issue"].as_str().unwrap().contains("3 marks on the plans (M-101 (3), M-501 (2)), 1 in the model"), "{qsd}");
}

/// (3) A floor plan's tag and the enlarged plan's tag of the same damper (registered onto the
/// floor plan) count once: five tags printed, three dampers.
#[test]
fn a_floor_plan_and_an_enlarged_plan_tag_count_once() {
    let mut s = job(3, 2);
    let (_, fs) = coverage(&mut s);
    let qsd = kind(&fs, "combination fire smoke damper");
    assert_eq!(qsd["measured"]["raw_marks"], 5, "{qsd}");
    assert_eq!(qsd["measured"]["plan_marks"], 3, "{qsd}");
    assert_eq!(qsd["measured"]["by_level"]["L1"], 3, "{qsd}");
    assert_eq!(qsd["measured"]["by_sheet"]["M-501"], 2, "{qsd}");
}

/// (4) The print inventory's unexplained marks repeated on several plan sheets are listed as a
/// likely missing kind, with examples; one sheet's repeats, a defined code's and lettering are
/// not. Its legend rows define kinds too.
#[test]
fn an_unexplained_tag_repeated_across_sheets_is_listed() {
    let s = job(3, 2);
    let m = |id: usize, sheet: &str, text: &str, p: [f64; 2], d: [f64; 2]| {
        json!({"id": id, "sheet": sheet, "trade": &sheet[..1], "what": "tag", "text": text, "at": on(p, d), "level": "L1", "status": "unexplained", "reason": "a mark no legend or schedule lists"})
    };
    let mut marks = vec![
        m(0, "M-101", "QZ", [3.0, 3.0], M),
        m(1, "M-101", "QZ", [9.0, 3.0], M),
        m(2, "M-101", "QZ", [14.0, 3.0], M),
        m(3, "E-101", "QZ", [3.0, 20.0], E),
        m(4, "E-101", "QZ", [9.0, 20.0], E),
        m(5, "M-101", "QW", [1.0, 1.0], M),
        m(6, "M-101", "QW", [2.0, 1.0], M),
        m(7, "M-101", "QW", [3.0, 1.0], M),
        m(8, "M-101", "QW", [4.0, 1.0], M),
        m(9, "M-101", "QSD", [5.0, 12.0], M),
        m(10, "E-101", "QSD", [5.0, 12.0], E),
        m(11, "M-101", "QSD", [6.0, 12.0], M),
        m(12, "E-101", "QSD", [6.0, 12.0], E),
    ];
    // A legend row the inventory read on E-101 (a night glow marker, its own kind) and two of its
    // tags on the plan: no model element.
    marks.push(json!({"id": 13, "sheet": "E-101", "trade": "E", "what": "legend_row", "text": "NG", "at": on([60.0, 2.0], E), "status": "not_object",
        "reason": "legend or schedule row", "kind": "legend:night glow marker", "source": "E-101 ELECTRICAL LEGEND: NG NIGHT GLOW MARKER"}));
    let mut s = with_inventory(s, json!(marks));
    {
        let mut d = (*s.doc().unwrap()).clone();
        let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
        for p in [[12.0, 8.0], [28.0, 8.0]] {
            let q = on(p, E);
            let h = d.new_handle();
            d.model.push(Entity::new(h, EntityKind::Text(Text { value: "NG".into(), insert: Vec3::new(q[0], q[1], 0.0), height: 6.0, ..style.clone() })));
        }
        let mut s2 = Session::empty();
        s2.open_drawing(d, "Sample Job", None);
        s = s2;
    }
    let (r, fs) = coverage(&mut s);
    let groups: Vec<&Value> = fs.iter().filter(|f| f["verdict"] == "unsure").collect();
    assert_eq!(groups.len(), 1, "{}", Value::Array(fs.clone()));
    let g = groups[0];
    assert_eq!(g["measured"]["text"], "QZ", "{g}");
    assert_eq!(g["measured"]["marks"], 5);
    assert_eq!(g["measured"]["sheets"], json!(["E-101", "M-101"]));
    assert!(g["issue"].as_str().unwrap().contains("likely a kind of object missing from the model"), "{g}");
    let ex = g["measured"]["examples"].as_array().unwrap();
    assert!(!ex.is_empty() && ex[0]["sheet"].is_string() && ex[0]["at"].is_array() && ex[0]["text"] == "QZ", "{g}");
    assert_eq!(r["coverage"]["unexplained_groups"], 1, "{r}");
    // The inventory's legend row is a kind: its two tags, nothing in the model.
    let ng = kind(&fs, "night glow marker");
    assert_eq!(ng["verdict"], "fail", "{ng}");
    assert_eq!(ng["measured"]["plan_marks"], 2, "{ng}");
    assert_eq!(ng["measured"]["defined_by"][0]["from"], "inventory_row", "{ng}");
}

/// (5) A kind the electrical sheets power (a keynote connecting a motorized damper) with no model
/// element fails, ranked first, though no mechanical legend names it; its keynote marker is its
/// mark.
#[test]
fn an_electrical_keynote_powering_a_kind_with_no_elements_fails() {
    let mut s = job(3, 2);
    let (_, fs) = coverage(&mut s);
    let md = kind(&fs, "motorized damper");
    assert_eq!(md["verdict"], "fail", "{md}");
    assert_eq!(md["severity"], "high");
    assert_eq!(md["measured"]["model"], 0);
    assert_eq!(md["measured"]["plan_marks"], 1, "the keynote's marker: {md}");
    let c = &md["measured"]["connected"][0];
    assert_eq!((c["sheet"].as_str(), c["via"].as_str()), (Some("E-101"), Some("power")), "{md}");
    assert!(md["issue"].as_str().unwrap().contains("E-101 connects to it (power)"), "{md}");
    // Connected kinds come first of all.
    assert_eq!(fs[0]["kind"], "motorized damper", "{}", fs[0]);
    // The note's receptacle is the electrical sheet's own kind, never another trade's.
    assert!(fs.iter().all(|f| f["kind"] != "receptacle" || f["measured"]["connected"].as_array().is_none_or(|a| a.is_empty())));
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-validatecoverage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// An agent's G-COVERAGE answer that cites no print page is kept as unsure; one citing a print
/// picture keeps its verdict.
#[test]
fn an_agent_answer_citing_no_print_page_is_kept_unsure() {
    let mut s = job(3, 2);
    let out = dir("agent");
    let run = s.execute("validate", &json!({"level": "L1", "out_dir": out.to_string_lossy()})).unwrap();
    let run = run["run"].as_str().unwrap().to_string();
    let area = "L1/101".to_string();
    let n = s.execute("validate.next", &json!({"run": run, "area": area})).unwrap();
    let pdir = PathBuf::from(n["packet"]["dir"].as_str().unwrap());
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(pdir.join("manifest.json")).unwrap()).unwrap();
    let rules: Value = serde_json::from_str(&std::fs::read_to_string(pdir.join("rules.json")).unwrap()).unwrap();
    assert!(rules["rules"].as_array().unwrap().iter().any(|r| r["id"] == "G-COVERAGE"), "{rules}");
    let print = manifest["prints"][0].clone();
    let model = manifest["model"][0]["file"].as_str().map(str::to_string);
    let uncited = json!({"rule": "G-COVERAGE", "verdict": "missing", "issue": "the plan tags fire/smoke dampers the model lacks", "expected": "dampers",
        "found": "none", "evidence": model.iter().map(|m| json!({"image": m, "sheet": "", "what": "the model here"})).collect::<Vec<_>>(), "confidence": 0.8});
    let cited = json!({"rule": "G-COVERAGE", "verdict": "missing", "issue": "a fire damper tag with no damper", "expected": "a fire damper",
        "found": "none", "evidence": [{"image": print["file"], "sheet": print["sheet"], "what": "the tag on the plan"}], "confidence": 0.8});
    let r = s.execute("validate.report", &json!({"run": run, "area": area, "findings": [uncited, cited], "final": true})).unwrap();
    assert_eq!(r["by_verdict"]["unsure"], 1, "{r}");
    assert_eq!(r["by_verdict"]["missing"], 1, "{r}");
    assert!(r["warnings"].as_array().is_some_and(|w| w.iter().any(|x| x.as_str().is_some_and(|t| t.contains("kept as unsure")))), "{r}");
    let _ = std::fs::remove_dir_all(&out);
}

/// Hostile and empty inputs: no sheets, no job, a drawing with a stored inventory of junk.
#[test]
fn no_sheets_or_junk_never_panic() {
    let mut s = Session::new();
    s.execute("new", &json!({})).unwrap();
    let _ = s.execute("validate.prescreen", &json!({"rules": ["G-COVERAGE"]}));
    let s = job(0, 0);
    let mut s = with_inventory(s, json!([{"id": 0, "sheet": "", "what": "tag", "text": "\u{0}\u{1}", "at": [1e308, -1e308], "level": ",,,", "status": "unexplained"}]));
    let (_, fs) = coverage(&mut s);
    assert!(fs.iter().all(|f| f["verdict"] != "unsure"), "{}", Value::Array(fs));
}
