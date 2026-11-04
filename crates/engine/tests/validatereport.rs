//! Validate reports that hold a whole room (Q6140): a report may come in parts, a finding may
//! name several elements, a duplicate is merged with a warning (never dropped unsaid),
//! `validate.status` totals every run of the drawing with the accuracy over the element × rule
//! pairs judged and their coverage, and the kinds Validate round 2 found without a rule
//! (wall luminaire, thermostat, fan coil unit) get theirs.
//!
//! The job is made up: a 50' × 20' building of two rooms (a lounge 11 and a workroom 12) drawn
//! once on a floor plan and once on a combined power and mechanical plan beside it. The lounge
//! holds over a hundred receptacles along its walls (more findings than one report carries); the
//! workroom a thermostat, a wall luminaire and a fan coil unit.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

const OX: f64 = 600.0;
const OY: f64 = 500.0;
const SW: f64 = 2400.0;
const SH: f64 = 1600.0;
/// The floor plan sheet's left edge, and the power and mechanical plan's.
const PLAN: f64 = 0.0;
const MEP: f64 = 3000.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn at(sx: f64, p: [f64; 2]) -> [f64; 2] {
    [sx + OX + p[0] * 12.0, OY + p[1] * 12.0]
}

fn rect(s: &mut Session, sx: f64, a: [f64; 2], b: [f64; 2]) {
    let v = [at(sx, a), at(sx, [b[0], a[1]]), at(sx, b), at(sx, [a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn line(s: &mut Session, sx: f64, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [at(sx, a), at(sx, b)]})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": p, "text": t, "height": 6})).unwrap();
}

fn walls(s: &mut Session, sx: f64) {
    rect(s, sx, [0.0, 0.0], [50.0, 20.0]);
    rect(s, sx, [0.5, 0.5], [49.5, 19.5]);
    line(s, sx, [30.0, 0.5], [30.0, 19.5]);
}

/// Block references with a TYPE attribute at building points (feet) on the floor plan, all at
/// once; their handles.
fn place_all(s: Session, items: &[(&str, &str, [f64; 2], &str)]) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    let mut out = Vec::new();
    for (block, layer, p, ty) in items {
        if !d.blocks.contains_key(*block) {
            let mut b = Block::new(block);
            let h = d.new_handle();
            b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-2.0, 0.0, 0.0), b: Vec3::new(2.0, 0.0, 0.0) })));
            d.blocks.insert((*block).into(), Arc::new(b));
        }
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
        let q = at(PLAN, *p);
        let ins = Insert {
            block: (*block).into(),
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
        e.common.layer = (*layer).into();
        d.model.push(e);
        out.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Made-up Job", None);
    (s2, out)
}

fn spec() -> Value {
    let sheet = |number: &str, title: &str, discipline: &str, x: f64, tx: f64| {
        json!({"number": number, "title": title, "discipline": discipline, "source": "model", "level": "L1", "rect": [x, 0, x + SW, SH],
            "registration": {"reference": "X1.1", "method": if tx == 0.0 { "reference" } else { "tags" }, "a": 1.0, "b": 0.0, "tx": tx, "ty": 0.0, "matched": 4}})
    };
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 11, "ceiling_ft": 9.5, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "WALL-OUT", "role": "wall", "wall_type": "Outside"},
            {"layer": "WALL-IN", "role": "wall", "wall_type": "Inside"},
            {"layer": "DEV-PWR", "role": "device"},
            {"layer": "DEV-MECH", "role": "device"},
            {"layer": "ROOM-NAMES", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Outside", "thickness_in": 6},
            {"name": "Inside", "thickness_in": 5, "from": "centerline"}
        ],
        "rooms": [{"name": "Lounge", "number": "11"}, {"name": "Workroom", "number": "12"}],
        "devices": {"by_type": {
            "RCP-2": {"ceiling": false, "aff_ft": 1.25, "size_ft": [0.25, 0.1, 0.33]},
            "TS": {"ceiling": false, "aff_ft": 4.0},
            "SC-1": {"ceiling": false, "aff_ft": 6.5},
            "UNIT-1": {"ceiling": true}
        }},
        "sheets": [
            sheet("X1.1", "FLOOR PLAN", "A", PLAN, 0.0),
            sheet("X2.1", "POWER AND MECHANICAL PLAN", "E", MEP, -MEP),
        ]
    })
}

struct Job {
    s: Session,
    /// The lounge's receptacles.
    receps: Vec<String>,
    thermostat: String,
    sconce: String,
    unit: String,
}

fn job() -> Job {
    let mut s = Session::new();
    layer(&mut s, "WALL-OUT");
    rect(&mut s, PLAN, [0.0, 0.0], [50.0, 20.0]);
    rect(&mut s, PLAN, [0.5, 0.5], [49.5, 19.5]);
    layer(&mut s, "WALL-IN");
    line(&mut s, PLAN, [30.0, 0.5], [30.0, 19.5]);
    layer(&mut s, "ROOM-NAMES");
    text(&mut s, at(PLAN, [12.0, 10.0]), "LOUNGE");
    text(&mut s, at(PLAN, [40.0, 10.0]), "WORKROOM");
    layer(&mut s, "BACKGROUND");
    walls(&mut s, MEP);
    layer(&mut s, "FRAMES");
    for x in [PLAN, MEP] {
        s.execute("pline", &json!({"vertices": [[x, 0], [x + SW, 0], [x + SW, SH], [x, SH]], "closed": true})).unwrap();
    }
    // The lounge's receptacles: along its south and north walls every half foot.
    let mut items: Vec<(&str, &str, [f64; 2], &str)> = Vec::new();
    for k in 0..56 {
        let x = 1.5 + f64::from(k) * 0.5;
        items.push(("OUTLET", "DEV-PWR", [x, 0.6], "RCP-2"));
        items.push(("OUTLET", "DEV-PWR", [x, 19.4], "RCP-2"));
    }
    items.push(("TSTAT", "DEV-MECH", [30.4, 8.0], "TS"));
    items.push(("SCONCE", "DEV-PWR", [49.4, 10.0], "SC-1"));
    items.push(("UNIT", "DEV-MECH", [40.0, 15.0], "UNIT-1"));
    let (mut s, hs) = place_all(s, &items);
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let n = hs.len();
    let receps: Vec<String> = hs.iter().take(n - 3).cloned().collect();
    let (thermostat, sconce, unit) = (hs[n - 3].clone(), hs[n - 2].clone(), hs[n - 1].clone());
    s.execute("bim.source", &json!({"handles": receps, "status": "fixed", "source": "X2.1", "kind": "receptacle"})).unwrap();
    for (h, kind) in [(&thermostat, "thermostat"), (&sconce, "wall_luminaire"), (&unit, "fan_coil_unit")] {
        s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "X2.1", "kind": kind})).unwrap();
    }
    Job { s, receps, thermostat, sconce, unit }
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-vreport-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

/// What a packet holds: its print picture, its rules and its own elements (handle, kind).
struct Packet {
    print: (String, String),
    rules: Vec<String>,
    elements: Vec<(String, String)>,
}

fn next(s: &mut Session, run: &str, area: &str) -> Packet {
    let n = s.execute("validate.next", &json!({"run": run, "area": area})).unwrap();
    let dir = PathBuf::from(n["packet"]["dir"].as_str().unwrap());
    let manifest = read_json(&dir.join("manifest.json"));
    let objects = read_json(&dir.join("objects.json"));
    let c = &manifest["prints"][0];
    let rules = read_json(&dir.join("rules.json"))["rules"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    let elements = objects["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["handle"].as_str().unwrap().to_ascii_uppercase(), e["kind"].as_str().unwrap_or_default().to_string()))
        .collect();
    Packet { print: (c["file"].as_str().unwrap().into(), c["sheet"].as_str().unwrap_or_default().into()), rules, elements }
}

impl Packet {
    fn finding(&self, rule: &str, handles: &[&str], verdict: &str, issue: &str) -> Value {
        let mut f = json!({"rule": rule, "verdict": verdict, "issue": issue, "expected": "as drawn", "found": "as modelled",
            "evidence": [{"image": self.print.0, "sheet": self.print.1, "what": "the plan at the element"}], "confidence": 0.9});
        match handles {
            [] => {}
            [h] => f["handle"] = json!(h),
            more => f["handles"] = json!(more),
        }
        f
    }

    /// Every element × rule pair the packet asks for, by the rules `validate.rules` gives each
    /// element's kind.
    fn pairs(&self, s: &mut Session) -> Vec<(String, String)> {
        let mut by_kind: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
        let mut out = Vec::new();
        for (h, kind) in &self.elements {
            let ids = by_kind.entry(kind.clone()).or_insert_with(|| {
                let mut ids = Vec::new();
                let mut cursor = 0;
                loop {
                    let r = s.execute("validate.rules", &json!({"kind": kind, "detail": false, "limit": 50, "cursor": cursor})).unwrap();
                    ids.extend(r["rules"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()));
                    match r["next"].as_u64() {
                        Some(n) => cursor = n,
                        None => break,
                    }
                }
                ids
            });
            for r in ids.iter().filter(|r| self.rules.contains(r)) {
                out.push((r.clone(), h.clone()));
            }
        }
        out
    }
}

fn report(s: &mut Session, run: &str, area: &str, extra: Value, findings: Vec<Value>) -> Result<Value, String> {
    let mut p = json!({"run": run, "area": area, "findings": findings});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        p[k] = v;
    }
    s.execute("validate.report", &p).map_err(|e| e.to_string())
}

fn start(s: &mut Session, out: &Path, scope: Value) -> String {
    let mut p = scope;
    p["out_dir"] = json!(out.to_string_lossy());
    s.execute("validate", &p).unwrap()["run"].as_str().unwrap().to_string()
}

const LOUNGE: &str = "L1/11";
const WORKROOM: &str = "L1/12";

/// More findings than one report carries come in parts; the area is done on the final part, and
/// it holds them all (no cap below its elements × rules).
#[test]
fn a_large_area_reports_in_parts() {
    let mut j = job();
    let out = dir("parts");
    let run = start(&mut j.s, &out, json!({"scope": "room", "room": "11"}));
    let pk = next(&mut j.s, &run, LOUNGE);
    assert!(pk.elements.len() >= 112, "the lounge's receptacles are its own: {}", pk.elements.len());
    let pairs = pk.pairs(&mut j.s);
    assert!(pairs.len() > 1_000, "more pairs than one report carries: {}", pairs.len());
    let all: Vec<Value> = pairs.iter().map(|(r, h)| pk.finding(r, &[h], "pass", "")).collect();
    // One call carries at most 1000, and says how to send the rest.
    let e = report(&mut j.s, &run, LOUNGE, json!({}), all.clone()).unwrap_err();
    assert!(e.contains("at most 1000") && e.contains("part"), "{e}");
    // Part 1, then part 2: the area is not done until the final part.
    let (a, b) = all.split_at(700);
    let r1 = report(&mut j.s, &run, LOUNGE, json!({"part": 1}), a.to_vec()).unwrap();
    assert_eq!((r1["stored"].as_u64(), r1["final"].as_bool(), r1["next"].as_str()), (Some(700), Some(false), Some("validate.report")), "{r1}");
    assert!(r1["note"].as_str().unwrap().contains("final: true"), "{r1}");
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    assert_eq!((st["areas"]["done"].as_u64(), st["areas"]["in_parts"].as_u64()), (Some(0), Some(1)), "{st}");
    assert_eq!(st["findings"]["total"], 700, "{st}");
    let r2 = report(&mut j.s, &run, LOUNGE, json!({"part": 2, "final": true}), b.to_vec()).unwrap();
    assert_eq!(r2["final"], true, "{r2}");
    assert_eq!(r2["area_findings"].as_u64().unwrap() as usize, pairs.len(), "{r2}");
    assert_eq!(r2["pairs"]["judged"].as_u64().unwrap() as usize, pairs.len(), "{r2}");
    assert_eq!(r2["pairs"]["coverage"], 1.0, "every pair the packet asks for was judged: {r2}");
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    assert_eq!((st["areas"]["done"].as_u64(), st["areas"]["in_parts"].as_u64()), (Some(1), Some(0)), "{st}");
    assert_eq!(st["findings"]["total"].as_u64().unwrap() as usize, pairs.len(), "{st}");
    assert_eq!(st["accuracy"]["coverage"], 1.0, "{st}");
    // A part after the final one is refused; a whole report replaces the area's findings.
    let e = report(&mut j.s, &run, LOUNGE, json!({"part": 3}), a[..1].to_vec()).unwrap_err();
    assert!(e.contains("final already"), "{e}");
    let whole = report(&mut j.s, &run, LOUNGE, json!({}), all[..900].to_vec()).unwrap();
    assert_eq!((whole["area_findings"].as_u64(), whole["final"].as_bool()), (Some(900), Some(true)), "{whole}");
    // A part with nothing in it is refused unless it is the final one.
    next(&mut j.s, &run, LOUNGE);
    let e = report(&mut j.s, &run, LOUNGE, json!({"part": 1}), vec![]).unwrap_err();
    assert!(e.contains("no findings"), "{e}");
    let e = report(&mut j.s, &run, LOUNGE, json!({"part": 1, "final": true}), vec![]).unwrap_err();
    assert!(e.contains("a pass is a finding too"), "nothing at all is no report: {e}");
    report(&mut j.s, &run, LOUNGE, json!({"part": true}), all[..10].to_vec()).unwrap();
    let closed = report(&mut j.s, &run, LOUNGE, json!({"final": true, "part": 2}), vec![]).unwrap();
    assert_eq!((closed["area_findings"].as_u64(), closed["final"].as_bool()), (Some(10), Some(true)), "{closed}");
    // Handed out again mid-parts: the parts in are dropped, and said.
    let run2 = start(&mut j.s, &out, json!({"scope": "room", "room": "11", "force": true}));
    assert_eq!(run2, run);
    next(&mut j.s, &run, LOUNGE);
    report(&mut j.s, &run, LOUNGE, json!({"part": 1}), all[..5].to_vec()).unwrap();
    let again = j.s.execute("validate.next", &json!({"run": run})).unwrap();
    assert_eq!(again["parts_dropped"]["findings"], 5, "{again}");
    for bad in [json!({"part": 0}), json!({"part": -1}), json!({"part": "one"}), json!({"final": "yes"})] {
        let e = report(&mut j.s, &run, LOUNGE, bad.clone(), all[..1].to_vec()).unwrap_err();
        assert!(e.contains("`part`") || e.contains("`final`"), "{bad}: {e}");
    }
    let _ = std::fs::remove_dir_all(&out);
}

/// One finding may name several elements when the same evidence and verdict cover them; it
/// counts once per element, and its review item names them all.
#[test]
fn one_finding_names_several_elements() {
    let mut j = job();
    let out = dir("handles");
    let run = start(&mut j.s, &out, json!({"scope": "room", "room": "11"}));
    let pk = next(&mut j.s, &run, LOUNGE);
    let hs: Vec<&str> = j.receps.iter().map(String::as_str).take(40).collect();
    let fails: Vec<&str> = j.receps.iter().map(String::as_str).skip(40).take(3).collect();
    let r = report(
        &mut j.s,
        &run,
        LOUNGE,
        json!({}),
        vec![pk.finding("G-PLACE", &hs, "pass", ""), pk.finding("G-PLACE", &fails, "fail", "a foot off the power plan's symbols")],
    )
    .unwrap();
    assert_eq!(r["stored"], 43, "{r}");
    assert_eq!(r["by_verdict"]["pass"], 40, "{r}");
    assert_eq!(r["review_items"], 1, "one finding, one review item: {r}");
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    assert_eq!(st["findings"]["by_rule"]["G-PLACE"]["pass"], 40, "{st}");
    assert_eq!(st["findings"]["by_rule"]["G-PLACE"]["fail"], 3, "{st}");
    assert_eq!(st["accuracy"]["by_rule"]["G-PLACE"], 0.93, "40 / 43 pairs: {st}");
    assert_eq!(st["accuracy"]["judged_pairs"], 43, "{st}");
    let list = j.s.execute("review.list", &json!({"kind": "validation", "limit": 50})).unwrap();
    let item = list["items"].as_array().unwrap().iter().find(|i| i["id"].as_str().unwrap().starts_with("RV-J")).unwrap().clone();
    assert!(item["title"].as_str().unwrap().contains("3 elements"), "{item}");
    let got = j.s.execute("review.next", &json!({"id": item["id"]})).unwrap()["item"].clone();
    let named: Vec<String> = got["detail"]["handles"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_ascii_uppercase()).collect();
    assert_eq!(named, fails.iter().map(|h| h.to_ascii_uppercase()).collect::<Vec<_>>(), "{got}");
    // Each handle is checked like a lone one: a handle beside or outside the area is refused.
    let e = report(&mut j.s, &run, LOUNGE, json!({}), vec![pk.finding("G-PLACE", &[hs[0], &j.unit], "pass", "")]).unwrap_err();
    assert!(e.contains("not one of this area's objects") || e.contains("beside this area"), "{e}");
    let e = report(
        &mut j.s,
        &run,
        LOUNGE,
        json!({}),
        vec![
            pk.finding("G-PRESENT", &[], "missing", "x")
                .as_object()
                .cloned()
                .map(|mut o| {
                    o.insert("handles".into(), json!([hs[0]]));
                    Value::Object(o)
                })
                .unwrap(),
        ],
    )
    .unwrap_err();
    assert!(e.contains("a `missing` finding has no handle"), "{e}");
    let mut f = pk.finding("G-PLACE", &[], "pass", "");
    f["handles"] = json!("not a list");
    let e = report(&mut j.s, &run, LOUNGE, json!({}), vec![f]).unwrap_err();
    assert!(e.contains("`handles` is a list"), "{e}");
    let _ = std::fs::remove_dir_all(&out);
}

/// The same rule, element and verdict twice (in one report or across parts), or the same missing
/// thing twice, is merged into the first and said; two verdicts for one pair are both kept and
/// the pair counts as the worse.
#[test]
fn duplicates_are_merged_with_a_warning_never_dropped_unsaid() {
    let mut j = job();
    let out = dir("dups");
    let run = start(&mut j.s, &out, json!({"scope": "room", "room": "11"}));
    let pk = next(&mut j.s, &run, LOUNGE);
    let (a, b, c) = (j.receps[0].as_str(), j.receps[1].as_str(), j.receps[2].as_str());
    let r = report(
        &mut j.s,
        &run,
        LOUNGE,
        json!({"part": 1}),
        vec![
            pk.finding("G-PLACE", &[a], "pass", ""),
            pk.finding("G-PLACE", &[a], "pass", ""),
            pk.finding("G-PLACE", &[a, b], "pass", ""),
            pk.finding("G-PRESENT", &[], "missing", "the plan shows a floor box by the window"),
            pk.finding("G-PRESENT", &[], "missing", "The plan shows a floor box by the window"),
            pk.finding("G-PRESENT", &[], "missing", "the plan shows a floor box by the door"),
        ],
    )
    .unwrap();
    assert_eq!(r["stored"], 4, "a, b, and two different missing boxes: {r}");
    assert_eq!(r["merged"], 3, "{r}");
    let w = r["warnings"].as_array().unwrap();
    assert_eq!(w.len(), 3, "each merge is said: {r}");
    assert!(w[0].as_str().unwrap().contains("finding 2") && w[0].as_str().unwrap().contains("finding 1"), "{r}");
    assert!(w.iter().any(|x| x.as_str().unwrap().contains("finding 5") && x.as_str().unwrap().contains("missing")), "{r}");
    // Across parts: a repeat of the first part is merged too; a second verdict for a pair is kept, said.
    let r = report(
        &mut j.s,
        &run,
        LOUNGE,
        json!({"part": 2, "final": true}),
        vec![pk.finding("G-PLACE", &[a], "pass", ""), pk.finding("G-PLACE", &[b], "fail", "off the symbol"), pk.finding("G-PLACE", &[c], "pass", "")],
    )
    .unwrap();
    assert_eq!((r["stored"].as_u64(), r["merged"].as_u64(), r["conflicts"].as_u64()), (Some(2), Some(1), Some(1)), "{r}");
    assert!(r["warnings"].as_array().unwrap().iter().any(|x| x.as_str().unwrap().contains("earlier part")), "{r}");
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    // Findings per handle: G-PLACE a, b pass, b fail, c pass; the pairs a pass, b fail, c pass.
    assert_eq!(st["findings"]["by_rule"]["G-PLACE"]["pass"], 3, "{st}");
    assert_eq!(st["findings"]["by_rule"]["G-PLACE"]["fail"], 1, "{st}");
    assert_eq!(st["accuracy"]["by_rule"]["G-PLACE"], 0.667, "{st}");
    assert_eq!(st["findings"]["by_rule"]["G-PRESENT"]["missing"], 2, "{st}");
    let _ = std::fs::remove_dir_all(&out);
}

/// The accuracy is over the pairs judged (an unsure pair is not judged), with the coverage of
/// the pairs the packets asked for beside it.
#[test]
fn accuracy_is_over_the_pairs_judged_with_coverage() {
    let mut j = job();
    let out = dir("coverage");
    let run = start(&mut j.s, &out, json!({"scope": "room", "room": "11"}));
    let pk = next(&mut j.s, &run, LOUNGE);
    let pairs = pk.pairs(&mut j.s);
    let asked = pairs.len();
    // A quarter judged: 3 of every 4 of those pass, the 4th fails; another quarter unsure.
    let quarter = asked / 4;
    let mut list = Vec::new();
    for (k, (r, h)) in pairs.iter().take(quarter).enumerate() {
        list.push(if k % 4 == 3 { pk.finding(r, &[h], "fail", "not as drawn") } else { pk.finding(r, &[h], "pass", "") });
    }
    for (r, h) in pairs.iter().skip(quarter).take(quarter) {
        list.push(pk.finding(r, &[h], "unsure", "the crop does not show it"));
    }
    let fails = (0..quarter).filter(|k| k % 4 == 3).count();
    report(&mut j.s, &run, LOUNGE, json!({}), list).unwrap();
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    let acc = &st["accuracy"];
    assert_eq!(acc["judged_pairs"].as_u64().unwrap() as usize, quarter, "{acc}");
    assert_eq!(acc["unsure_pairs"].as_u64().unwrap() as usize, quarter, "{acc}");
    assert_eq!(acc["pairs_asked"].as_u64().unwrap() as usize, asked, "{acc}");
    let want = ((quarter - fails) as f64 / quarter as f64 * 1000.0).round() / 1000.0;
    assert_eq!(acc["overall"].as_f64().unwrap(), want, "{acc}");
    let cov = (quarter as f64 / asked as f64 * 1000.0).round() / 1000.0;
    assert_eq!(acc["coverage"].as_f64().unwrap(), cov, "{acc}");
    assert!(acc["coverage_by_rule"].as_object().unwrap().len() >= 5, "{acc}");
    let _ = std::fs::remove_dir_all(&out);
}

/// `validate.status {}` totals every run of the drawing (and lists each run's own scores); an
/// area judged in two runs counts once, from the later.
#[test]
fn status_totals_every_run() {
    let mut j = job();
    let out = dir("totals");
    let lounge = start(&mut j.s, &out, json!({"scope": "room", "room": "11"}));
    let pk = next(&mut j.s, &lounge, LOUNGE);
    let hs: Vec<&str> = j.receps.iter().map(String::as_str).take(10).collect();
    report(&mut j.s, &lounge, LOUNGE, json!({}), vec![pk.finding("G-PLACE", &hs, "pass", "")]).unwrap();
    let work = start(&mut j.s, &out, json!({"scope": "room", "room": "12"}));
    assert_ne!(work, lounge);
    let pk = next(&mut j.s, &work, WORKROOM);
    report(&mut j.s, &work, WORKROOM, json!({}), vec![pk.finding("G-PLACE", &[&j.thermostat], "fail", "on the wrong wall")]).unwrap();
    let st = j.s.execute("validate.status", &json!({})).unwrap();
    assert_eq!(st["run"], json!(work), "the latest run's detail: {st}");
    assert_eq!(st["findings"]["total"], 1, "{st}");
    let t = &st["totals"];
    assert_eq!((t["runs"].as_u64(), t["areas_judged"].as_u64()), (Some(2), Some(2)), "{t}");
    assert_eq!(t["findings"]["total"], 11, "{t}");
    assert_eq!(t["accuracy"]["overall"], 0.909, "10 of 11 pairs: {t}");
    let per: Vec<&Value> = st["per_run"].as_array().unwrap().iter().collect();
    assert_eq!(per.len(), 2, "{st}");
    assert_eq!((per[0]["run"].as_str(), per[0]["findings"].as_u64()), (Some(work.as_str()), Some(1)), "{st}");
    assert_eq!((per[1]["run"].as_str(), per[1]["findings"].as_u64()), (Some(lounge.as_str()), Some(10)), "{st}");
    // The whole building judged later: the lounge counts once, from the building run.
    let building = start(&mut j.s, &out, json!({"scope": "building"}));
    let pk = next(&mut j.s, &building, LOUNGE);
    report(&mut j.s, &building, LOUNGE, json!({}), vec![pk.finding("G-PLACE", &hs[..4], "pass", "")]).unwrap();
    let t = j.s.execute("validate.status", &json!({})).unwrap()["totals"].clone();
    assert_eq!((t["runs"].as_u64(), t["areas_judged"].as_u64()), (Some(3), Some(2)), "{t}");
    assert_eq!(t["findings"]["total"], 5, "the lounge's 4 from the building run and the workroom's 1: {t}");
    let _ = std::fs::remove_dir_all(&out);
}

/// A wall luminaire gets the luminaire and circuit rules, a thermostat its own beside host,
/// height and present, a fan coil unit the mechanical equipment rule and not the exhaust fan's.
#[test]
fn leftover_kinds_get_their_rules() {
    let mut j = job();
    let out = dir("kinds");
    let run = start(&mut j.s, &out, json!({"scope": "room", "room": "12"}));
    let pk = next(&mut j.s, &run, WORKROOM);
    let kind_of = |h: &str| {
        pk.elements.iter().find(|(x, _)| x.eq_ignore_ascii_case(h)).map(|(_, k)| k.clone()).unwrap_or_else(|| panic!("{h}: {:?}", pk.elements))
    };
    assert_eq!(kind_of(&j.thermostat), "thermostat");
    assert_eq!(kind_of(&j.sconce), "wall_luminaire");
    assert_eq!(kind_of(&j.unit), "fan_coil_unit");
    for id in ["M-TSTAT", "G-HOST", "G-HEIGHT", "G-PRESENT", "E-LUM", "E-CIRCUIT", "M-EQUIP"] {
        assert!(pk.rules.contains(&id.to_string()), "{id} in {:?}", pk.rules);
    }
    assert!(!pk.rules.contains(&"M-EF".to_string()), "no exhaust fan here: {:?}", pk.rules);
    for (rule, h, want) in [
        ("M-EF", &j.unit, "not one of this packet's rules"),
        ("E-LUM", &j.thermostat, "does not apply"),
        ("M-TSTAT", &j.sconce, "does not apply"),
        ("M-EQUIP", &j.thermostat, "does not apply"),
    ] {
        let e = report(&mut j.s, &run, WORKROOM, json!({}), vec![pk.finding(rule, &[h], "pass", "")]).unwrap_err();
        assert!(e.contains(want), "{rule} on {h}: {e}");
    }
    let ok = report(
        &mut j.s,
        &run,
        WORKROOM,
        json!({}),
        vec![
            pk.finding("M-TSTAT", &[&j.thermostat], "pass", ""),
            pk.finding("G-HOST", &[&j.thermostat], "pass", ""),
            pk.finding("E-LUM", &[&j.sconce], "pass", ""),
            pk.finding("E-CIRCUIT", &[&j.sconce], "pass", ""),
            pk.finding("M-EQUIP", &[&j.unit], "pass", ""),
        ],
    )
    .unwrap();
    assert_eq!(ok["stored"], 5, "{ok}");
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    // A thermostat is the mechanical trade's (its own rule says so): its G-HOST finding too.
    assert_eq!(st["findings"]["by_trade"]["mechanical"]["pass"], 3, "the thermostat's two and the unit's: {st}");
    assert_eq!(st["findings"]["by_trade"]["electrical"]["pass"], 2, "the wall luminaire's: {st}");
    let _ = std::fs::remove_dir_all(&out);
}
