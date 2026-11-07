//! Provenance, project rules and corrections in Validate (Q6259) on a made-up job: guest room
//! 101 with a lavatory and a standard duplex beside it on panel LP1 circuit 3, which the panel
//! schedule prints without a GFCI breaker; a luminaire whose catalog number the build corrected
//! (the design value kept beside it); a duplex placed by a rule. A project rule read on site
//! says LP1's breakers are AF/GF as built: with it the standard duplex is right (its breaker
//! protects it), without it the same model fails.

use std::path::PathBuf;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Source, SourceKind};
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// The plan's origin on the drawing (inches).
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
const RULES: [&str; 2] = ["G-PRODUCT", "G-PRODUCT-CODE"];

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on the plan.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let v = [at(a), at([b[0], a[1]]), at(b), at([a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [at(a), at(b)]})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": p, "text": t, "height": 6})).unwrap();
}

/// Device blocks at building points (feet), each with its TYPE attribute.
fn place(s: Session, items: &[([f64; 2], &str)]) -> (Session, Vec<String>) {
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
            text: Text { value: (*ty).into(), ..style.clone() },
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
        e.common.layer = "E-POWR".into();
        d.model.push(e);
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
}

/// Store properties on objects, as the job's readers and the build do.
fn set_props(s: Session, sets: &[(&str, &str, Prop)]) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    for (handle, key, prop) in sets {
        let h = cadaddy_engine::doc::Handle::parse_hex(handle).unwrap();
        d.modify_entity(h, |e| {
            let mut store = props::read(&e.common.xdata);
            store.set(key, prop.clone());
            e.common.xdata = props::write(&e.common.xdata, &store).into();
        })
        .unwrap();
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

fn val(v: Value) -> Prop {
    Prop::new(v, Source::computed())
}

/// The schedule marks breaker types (GF on circuit 1); circuit 3, the duplex's, has none.
/// `schedule_codes`: whether the schedule marks types at all (one that marks none says
/// nothing, unless the project says what was built).
fn spec(schedule_codes: bool) -> Value {
    let dev = json!({"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]});
    let lum = json!({"ceiling": true, "aff_ft": 9.0, "size_ft": [0.5, 0.5, 0.3]});
    let fix = json!({"ceiling": false, "aff_ft": 2.8, "size_ft": [1.5, 1.5, 0.5]});
    let panel = json!({"ceiling": false, "aff_ft": 5.0, "size_ft": [1.2, 0.3, 2.0]});
    let first = if schedule_codes {
        json!({"number": "1", "description": "RCPT", "protection": ["GF"]})
    } else {
        json!({"number": "1", "description": "RCPT"})
    };
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-POWR", "role": "device"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Guest Room", "number": "101"}, {"name": "Corridor", "number": "102"}],
        "devices": {"by_type": {"DUPLEX": dev, "F9": lum, "LAV": fix, "LP1": panel}},
        "sheets": [
            {"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
             "registration": {"reference": "A-101", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}}
        ],
        "mep": {"panel_schedules": [{"panel": "LP1", "source": "E-601", "legend": {"GF": "GROUND FAULT CIRCUIT INTERRUPTER", "AF": "ARC FAULT CIRCUIT INTERRUPTER"}, "circuits": [
            first,
            {"number": "3", "description": "RCPT"}
        ]}]},
        "prints": {"version": 1, "submittals": [
            {"mark": "F9", "section": "26 51 19", "fields": {"manufacturer": "Acme Lighting", "catalogNumber": "LX-100"}}
        ]}
    })
}

struct Job {
    s: Session,
    /// The standard duplex beside the lavatory, on LP1-3.
    at_sink: String,
    /// The luminaire, corrected or not.
    lum: String,
    /// The duplex a rule placed.
    by_rule: String,
}

/// The job; `corrected`: the luminaire's catalog number corrected for the field (the build's
/// mark: the design value kept, the rule and the reason).
fn job(corrected: bool, schedule_codes: bool) -> Job {
    let mut s = Session::new();
    // A 40' × 12' shell; a partition at 20'.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [40.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [39.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [20.0, 0.5], [20.0, 11.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([10.0, 6.0]), "101");
    text(&mut s, at([30.0, 6.0]), "102");
    let (s, h) = place(
        s,
        &[
            ([7.0, 10.5], "LAV"),    // 0 lavatory
            ([8.5, 11.5], "DUPLEX"), // 1 at the sink
            ([1.0, 6.0], "LP1"),     // 2 panel
            ([10.0, 6.0], "F9"),     // 3 luminaire
            ([15.0, 0.5], "DUPLEX"), // 4 placed by a rule
        ],
    );
    let mut s = s;
    s.execute("bspec.set", &json!({"spec": spec(schedule_codes), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let kind = |s: &mut Session, hs: &[&String], k: &str, status: &str, source: &str| {
        s.execute("bim.source", &json!({"handles": hs, "status": status, "source": source, "kind": k})).unwrap();
    };
    kind(&mut s, &[&h[1]], "receptacle", "fixed", "E-101");
    kind(&mut s, &[&h[4]], "receptacle", "rule", "a receptacle every 12 ft of wall (made-up spacing rule)");
    kind(&mut s, &[&h[3]], "luminaire", "fixed", "E-201");
    kind(&mut s, &[&h[0]], "lavatory", "fixed", "P-101");
    kind(&mut s, &[&h[2]], "panel", "fixed", "E-101");
    let mut sets: Vec<(&str, &str, Prop)> = vec![
        (&h[1], "panel", val(json!("LP1"))),
        (&h[1], "circuit", val(json!("LP1-3"))),
        (&h[1], "receptacle_type", val(json!("standard"))),
        (&h[4], "panel", val(json!("LP1"))),
        (&h[4], "circuit", val(json!("LP1-1"))),
        (&h[3], "manufacturer", val(json!("Acme Lighting"))),
        (&h[3], "catalog_number", val(json!("LX-200"))),
    ];
    if corrected {
        sets.push((&h[3], "design:catalog_number", Prop::new(json!("LX-100"), Source::from_doc(SourceKind::Submittal, "submittal 26 51 19"))));
        sets.push((
            &h[3],
            "correction:catalog_number",
            Prop::new(json!("G-DOC-ORDER: the approved resubmittal replaces LX-100 with LX-200"), Source::from_doc(SourceKind::Rule, "G-DOC-ORDER")),
        ));
    }
    let s = set_props(s, &sets);
    Job { s, at_sink: h[1].clone(), lum: h[3].clone(), by_rule: h[4].clone() }
}

/// The field-verified rule: LP1's breakers are AF/GF as built.
fn as_built(s: &mut Session, tier: &str) {
    s.execute(
        "project.rules.set",
        &json!({"domain": "custom", "key": "breaker_protection.LP1", "value": "AF/GF except PTAC", "tier": tier,
                "source": "panel LP1 breakers read on site (made-up walk)", "reason": "the breakers installed differ from the schedule"}),
    )
    .unwrap();
}

fn screen(s: &mut Session) -> (Value, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 200, "rules": RULES})).unwrap();
    let fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    (r, fs)
}

fn of<'a>(fs: &'a [Value], handle: &str, rule: &str, check: &str) -> &'a Value {
    fs.iter()
        .find(|f| f["rule"] == rule && f["check"] == check && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(handle)))
        .unwrap_or_else(|| panic!("no {rule} {check} finding on {handle}: {}", json!(fs)))
}

#[test]
fn a_field_verified_breaker_rule_outranks_the_schedule_and_without_it_the_same_model_fails() {
    // Without the rule: the standard duplex at the sink on a breaker the schedule prints
    // standard fails.
    let mut j = job(false, true);
    let (_, fs) = screen(&mut j.s);
    let f = of(&fs, &j.at_sink, "G-PRODUCT-CODE", "gfci");
    assert_eq!(f["verdict"], "fail", "{f}");
    // With it (and in any tier above the drawings): it passes, the rule cited with its tier and
    // source beside the row it overrides.
    for tier in ["field_verified", "rfi"] {
        let mut j = job(false, true);
        as_built(&mut j.s, tier);
        let (_, fs) = screen(&mut j.s);
        let f = of(&fs, &j.at_sink, "G-PRODUCT-CODE", "gfci");
        assert_eq!(f["verdict"], "pass", "{tier}: {f}");
        let row = f["measured"]["breaker_row"].as_str().unwrap();
        assert!(row.contains(&format!("{tier} project rule breaker_protection.LP1")), "{row}");
        assert!(row.contains("read on site") && row.contains("E-601") && row.contains("circuit 3"), "{row}");
    }
    // A schedule that marks no breaker types says nothing: unsure without the rule, decided
    // by it.
    let mut j = job(false, false);
    let (_, fs) = screen(&mut j.s);
    assert_eq!(of(&fs, &j.at_sink, "G-PRODUCT-CODE", "gfci")["verdict"], "unsure");
    as_built(&mut j.s, "field_verified");
    let (_, fs) = screen(&mut j.s);
    assert_eq!(of(&fs, &j.at_sink, "G-PRODUCT-CODE", "gfci")["verdict"], "pass");
}

#[test]
fn a_fail_that_is_only_the_correction_is_judged_on_its_basis() {
    // Not corrected: the catalog number against the submittal fails.
    let mut j = job(false, true);
    let (_, fs) = screen(&mut j.s);
    let f = of(&fs, &j.lum, "G-PRODUCT", "catalog");
    assert_eq!(f["verdict"], "fail", "{f}");
    assert!(f.get("corrections").is_none(), "{f}");
    // Corrected for the field: the same disagreement is the correction itself, unsure with its
    // basis to judge, the tallies moved with it.
    let mut j = job(true, true);
    let (r, fs) = screen(&mut j.s);
    let f = of(&fs, &j.lum, "G-PRODUCT", "catalog");
    assert_eq!(f["verdict"], "unsure", "{f}");
    assert_eq!(f["judge_on_basis"]["design"], "LX-100", "{f}");
    assert_eq!(f["judge_on_basis"]["corrected"], "LX-200", "{f}");
    assert_eq!(f["judge_on_basis"]["rule"], "G-DOC-ORDER", "{f}");
    assert!(f["issue"].as_str().unwrap().starts_with("corrected for the field"), "{f}");
    assert!(r["checked"]["G-PRODUCT"]["unsure"].as_u64().unwrap() >= 1, "{}", r["checked"]);
    // A finding on an element a rule placed says so.
    if let Some(f) = fs.iter().find(|f| f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.by_rule))) {
        assert_eq!(f["provenance"]["made"], "inferred", "{f}");
    }
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-validatebasis-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The first file named `name` under `dir`.
fn walk(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = walk(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}

#[test]
fn a_packet_carries_provenance_corrections_and_the_project_rules() {
    let mut j = job(true, true);
    as_built(&mut j.s, "field_verified");
    let areas = j.s.execute("validate.areas", &json!({})).unwrap();
    let id = areas["areas"].as_array().unwrap().iter().find(|a| a["number"] == "101").unwrap()["id"].as_str().unwrap().to_string();
    let out = dir("packet");
    let r = j.s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "max_images": 1})).unwrap();
    let objects = walk(&out, "objects.json").unwrap_or_else(|| panic!("no objects.json under {}: {r}", out.display()));
    let o: Value = serde_json::from_str(&std::fs::read_to_string(&objects).unwrap()).unwrap();
    // Every element and run says what it is by the documents.
    let els = o["elements"].as_array().unwrap();
    assert!(!els.is_empty(), "{o}");
    for e in els.iter().chain(o["runs"].as_array().unwrap()) {
        let m = e["provenance"]["made"].as_str().unwrap_or_else(|| panic!("no provenance: {e}"));
        assert!(["drawn", "derived", "inferred"].contains(&m), "{e}");
        assert!(m == "drawn" || e["provenance"]["basis"].as_str().is_some_and(|b| !b.is_empty()), "{e}");
    }
    let el =
        |h: &str| els.iter().find(|e| e["handle"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(h))).unwrap_or_else(|| panic!("{h} not in {o}"));
    let ruled = el(&j.by_rule);
    assert_eq!(ruled["provenance"]["made"], "inferred", "{ruled}");
    assert!(ruled["provenance"]["basis"].as_str().unwrap().contains("spacing rule"), "{ruled}");
    assert_eq!(el(&j.at_sink)["provenance"]["made"], "drawn");
    // The corrected value beside its design value, with the rule and the reason.
    let lum = el(&j.lum);
    let c = &lum["corrections"][0];
    assert_eq!((c["key"].as_str(), c["design"].as_str(), c["corrected"].as_str()), (Some("catalog_number"), Some("LX-100"), Some("LX-200")), "{lum}");
    assert_eq!(c["rule"], "G-DOC-ORDER", "{c}");
    assert!(c["reason"].as_str().unwrap().contains("resubmittal"), "{c}");
    // The project's rules with tier and source.
    let rules = o["project_rules"].as_array().unwrap();
    let br = rules.iter().find(|r| r["key"] == "breaker_protection.LP1").unwrap_or_else(|| panic!("{rules:?}"));
    assert_eq!((br["tier"].as_str(), br["outranks_drawings"].as_bool()), (Some("field_verified"), Some(true)), "{br}");
    assert!(br["source"].as_str().unwrap().contains("read on site"), "{br}");
    assert_eq!(o["counts"]["provenance"]["corrected"], 1, "{}", o["counts"]);
    // The prescreen in the packet follows the rule: the duplex passes on its breaker as built.
    let pre = o["prescreen"]["findings"].as_array().unwrap();
    let g = pre
        .iter()
        .find(|f| f["rule"] == "G-PRODUCT-CODE" && f["check"] == "gfci" && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.at_sink)))
        .unwrap_or_else(|| panic!("{pre:?}"));
    assert_eq!(g["verdict"], "pass", "{g}");
    // The judge is told to judge a correction on its basis and sees the rule.
    let prompt = std::fs::read_to_string(walk(&out, "prompt.md").unwrap()).unwrap();
    assert!(prompt.contains("A correction with a stated basis is judged on that basis"), "{prompt}");
    assert!(prompt.contains("is not a fail by itself"), "{prompt}");
    assert!(prompt.contains("`custom/breaker_protection.LP1` = AF/GF except PTAC (field_verified: panel LP1 breakers read on site"), "{prompt}");
    let basis = prompt.find("## What the documents draw").unwrap();
    assert!(basis < prompt.find("## Rules to check").unwrap(), "the section comes before the rules");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(walk(&out, "manifest.json").unwrap()).unwrap()).unwrap();
    assert_eq!(manifest["counts"]["project_rules"], 1, "{}", manifest["counts"]);
    // Byte-identical when made again.
    let again = dir("again");
    j.s.execute("validate.packet", &json!({"area": id, "out_dir": again.to_string_lossy(), "max_images": 1})).unwrap();
    for f in ["objects.json", "prompt.md", "manifest.json"] {
        assert_eq!(std::fs::read(walk(&out, f).unwrap()).unwrap(), std::fs::read(walk(&again, f).unwrap()).unwrap(), "{f}");
    }
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&again);
}
