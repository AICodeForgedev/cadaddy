//! The Validate run (Q6116) on a made-up job, judged by a scripted fake agent (no network): the
//! run starts, hands out packets one area at a time, takes the judge's findings back as review
//! items, a person accepts or rejects them, and the status scores the model and the judge. It
//! resumes after save and reopen, refuses bad reports, and on a planted copy says which planted
//! errors the judge caught.
//!
//! The job: a 100' × 12' building of three rooms (an office, a storage room, a corridor) on an
//! architectural plan, a power plan, a mechanical plan and a reflected ceiling plan side by side;
//! six devices (receptacles, a GFCI receptacle, a panel, an exhaust fan) and a duct.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
const A101: f64 = 0.0;
const E101: f64 = 4000.0;
const M101: f64 = 8000.0;
const A121: f64 = 12000.0;
const E601_Y: f64 = 3000.0;

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

fn symbol(s: &mut Session, p: [f64; 2]) {
    s.execute("line", &json!({"points": [[p[0] - 12.0, p[1]], [p[0] + 12.0, p[1]]]})).unwrap();
    s.execute("circle", &json!({"center": p, "radius": 6})).unwrap();
}

fn walls(s: &mut Session, sx: f64) {
    rect(s, sx, [0.0, 0.0], [100.0, 12.0]);
    rect(s, sx, [0.5, 0.5], [99.5, 11.5]);
    line(s, sx, [15.0, 0.5], [15.0, 11.5]);
    line(s, sx, [30.0, 0.5], [30.0, 11.5]);
}

/// A block reference with a TYPE attribute at a building point (feet) on the A plan.
fn place(s: Session, block: &str, layer: &str, p: [f64; 2], ty: &str) -> (Session, String) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key(block) {
        let mut b = Block::new(block);
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert(block.into(), Arc::new(b));
    }
    let attribs = vec![Attrib {
        tag: "TYPE".into(),
        text: Text { value: ty.into(), ..style.clone() },
        invisible: true,
        constant: false,
        prompt: String::new(),
        props: AttribProps::default(),
        multiline: None,
        modes: AttribModes::default(),
    }];
    let q = at(A101, p);
    let ins = Insert {
        block: block.into(),
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
    e.common.layer = layer.into();
    d.model.push(e);
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, h.hex())
}

fn sheet(number: &str, title: &str, discipline: &str, level: Option<&str>, x: f64, y: f64, tx: Option<f64>) -> Value {
    let mut v = json!({"number": number, "title": title, "discipline": discipline, "source": "model", "rect": [x, y, x + SW, y + SH]});
    if let Some(l) = level {
        v["level"] = json!(l);
    }
    if let Some(tx) = tx {
        let method = if tx == 0.0 { "reference" } else { "tags" };
        v["registration"] = json!({"reference": "A-101", "method": method, "a": 1.0, "b": 0.0, "tx": tx, "ty": 0.0, "matched": 4});
    }
    v
}

fn spec() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-POWR", "role": "device"},
            {"layer": "M-EQPM", "role": "device"},
            {"layer": "M-DUCT", "role": "run"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Office", "number": "101"}, {"name": "Storage", "number": "102"}, {"name": "Corridor", "number": "103"}],
        "devices": {"by_type": {
            "DUPLEX": {"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]},
            "GFCI RECEPTACLE": {"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]},
            "PNL-A": {"ceiling": false, "aff_ft": 4.0},
            "EF-1": {"ceiling": true}
        }},
        "sheets": [
            sheet("A-101", "FLOOR PLAN - LEVEL 1", "A", Some("L1"), A101, 0.0, Some(0.0)),
            sheet("E-101", "POWER PLAN - LEVEL 1", "E", Some("L1"), E101, 0.0, Some(-E101)),
            sheet("M-101", "MECHANICAL PLAN - LEVEL 1", "M", Some("L1"), M101, 0.0, Some(-M101)),
            sheet("A-121", "REFLECTED CEILING PLAN - LEVEL 1", "A", Some("L1"), A121, 0.0, Some(-A121)),
            sheet("E-601", "ELECTRICAL SCHEDULES", "E", None, 0.0, E601_Y, None)
        ]
    })
}

/// What the prints say, by model handle: where each device is, its mounting, whether it is GFCI.
#[derive(Clone, Debug)]
struct Truth {
    handle: String,
    kind: &'static str,
    at: [f64; 2],
    mount: Option<&'static str>,
    gfci: bool,
}

struct Job {
    s: Session,
    truth: Vec<Truth>,
    duct: String,
}

fn job() -> Job {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, A101, [0.0, 0.0], [100.0, 12.0]);
    rect(&mut s, A101, [0.5, 0.5], [99.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, A101, [15.0, 0.5], [15.0, 11.5]);
    line(&mut s, A101, [30.0, 0.5], [30.0, 11.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at(A101, [5.0, 6.0]), "OFFICE");
    text(&mut s, at(A101, [20.0, 6.0]), "STORAGE");
    text(&mut s, at(A101, [60.0, 6.0]), "CORRIDOR");
    layer(&mut s, "G-BKGD");
    for sx in [E101, M101, A121] {
        walls(&mut s, sx);
    }
    layer(&mut s, "G-FRAME");
    for (x, y) in [(A101, 0.0), (E101, 0.0), (M101, 0.0), (A121, 0.0), (0.0, E601_Y)] {
        s.execute("pline", &json!({"vertices": [[x, y], [x + SW, y], [x + SW, y + SH], [x, y + SH]], "closed": true})).unwrap();
    }
    // The power plan's symbols where the prints put the devices (the storage receptacle at 22').
    layer(&mut s, "E-SYM");
    for p in [[6.0, 0.5], [22.0, 0.5], [20.0, 11.5], [0.5, 6.0], [50.0, 0.5]] {
        symbol(&mut s, at(E101, p));
    }
    layer(&mut s, "M-SYM");
    symbol(&mut s, at(M101, [10.0, 8.0]));
    line(&mut s, M101, [10.0, 8.0], [10.0, 11.5]);
    layer(&mut s, "E-SCHED");
    text(&mut s, [200.0, E601_Y + 1800.0], "PANEL SCHEDULE");
    text(&mut s, [200.0, E601_Y + 1700.0], "PNL-A");
    text(&mut s, [600.0, E601_Y + 1700.0], "120/208V 3PH 4W RECESSED");
    // The model: the storage receptacle was put 2 ft off the print (a real error).
    let (s, recep) = place(s, "RECEP", "E-POWR", [6.0, 0.5], "DUPLEX");
    let (s, recep2) = place(s, "RECEP", "E-POWR", [24.0, 0.5], "DUPLEX");
    let (s, gfci) = place(s, "RECEP", "E-POWR", [20.0, 11.5], "GFCI RECEPTACLE");
    let (s, corridor) = place(s, "RECEP", "E-POWR", [50.0, 0.5], "DUPLEX");
    let (s, panel) = place(s, "PANEL", "E-POWR", [0.5, 6.0], "PNL-A");
    let (mut s, fan) = place(s, "FAN", "M-EQPM", [10.0, 8.0], "EF-1");
    layer(&mut s, "M-DUCT");
    line(&mut s, A101, [10.0, 8.0], [10.0, 11.5]);
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    for (h, kind) in [(&recep, "receptacle"), (&recep2, "receptacle"), (&corridor, "receptacle"), (&panel, "panelboard"), (&fan, "exhaust_fan")] {
        s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": "E-101", "kind": kind})).unwrap();
    }
    let mut d = (*s.doc().unwrap()).clone();
    // The panel's submittal says recessed.
    use cadaddy_engine::doc::props;
    d.modify_entity(cadaddy_engine::doc::Handle::parse_hex(&panel).unwrap(), |e| {
        let mut p = props::read(&e.common.xdata);
        p.set("mounting", props::Prop::new("recessed", props::Source::from_doc(props::SourceKind::Submittal, "Panels.pdf p.2")));
        e.common.xdata = props::write(&e.common.xdata, &p).into();
    })
    .unwrap();
    let duct = d.model.iter().find(|e| e.common.layer == "M-DUCT" && matches!(e.kind, EntityKind::Line(_))).map(|e| e.handle).unwrap();
    d.modify_entity(duct, |e| e.common.xdata.extend(cadaddy_engine::doc::bimspec::type_mark_xdata("D1"))).unwrap();
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Job", None);
    s.execute("bim.set_type", &json!({"mark": "D1", "category": "duct", "fields": {"size": "10x8"}})).unwrap();
    // The prints' symbols for the deterministic pre-screen (bim.validate).
    let sym = |kind: &str, at: [f64; 2], sheet: &str| json!({"sheet": sheet, "level": "L1", "kind": kind, "at": at});
    let prints = json!({
        "sheets": [{"number": "E-101", "title": "POWER PLAN - LEVEL 1", "level": "L1"}, {"number": "M-101", "title": "MECHANICAL PLAN - LEVEL 1", "level": "L1"}],
        "symbols": [sym("receptacle", [6.0, 0.5], "E-101"), sym("receptacle", [22.0, 0.5], "E-101"), sym("receptacle", [20.0, 11.5], "E-101"),
                    sym("receptacle", [50.0, 0.5], "E-101"), sym("panelboard", [0.5, 6.0], "E-101"), sym("exhaust_fan", [10.0, 8.0], "M-101")],
    });
    s.execute("bim.prints", &json!({"prints": prints})).unwrap();
    let t = |handle: &String, kind, at, mount, gfci| Truth { handle: handle.clone(), kind, at, mount, gfci };
    let truth = vec![
        t(&recep, "receptacle", [6.0, 0.5], None, false),
        t(&recep2, "receptacle", [22.0, 0.5], None, false),
        t(&gfci, "receptacle", [20.0, 11.5], None, true),
        t(&corridor, "receptacle", [50.0, 0.5], None, false),
        t(&panel, "panelboard", [0.5, 6.0], Some("recessed"), false),
        t(&fan, "exhaust_fan", [10.0, 8.0], None, false),
    ];
    Job { s, truth, duct: duct.hex() }
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-vagent-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

/// What the fake judge checks.
#[derive(Clone, Copy)]
struct Judge {
    /// Compare duct sizes (off: a judge that misses them).
    sizes: bool,
    /// Raise a low-confidence duct-height alarm (a false alarm: the model is right).
    noisy: bool,
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn ft(v: &Value) -> [f64; 2] {
    [v[0].as_f64().unwrap(), v[1].as_f64().unwrap()]
}

/// The scripted agent: reads the packet the way an agent would and judges it against the truth.
fn judge(packet: &Path, truth: &[Truth], j: Judge) -> Vec<Value> {
    let manifest = read_json(&packet.join("manifest.json"));
    let objects = read_json(&packet.join("objects.json"));
    let rules: Vec<String> =
        read_json(&packet.join("rules.json"))["rules"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    let has = |r: &str| rules.iter().any(|x| x == r);
    let print =
        manifest["prints"].as_array().unwrap().first().map(|c| (c["file"].as_str().unwrap().to_string(), c["sheet"].as_str().unwrap().to_string()));
    let model = manifest["model"].as_array().unwrap().first().map(|c| c["file"].as_str().unwrap().to_string()).unwrap();
    let ev = |what: &str| match &print {
        Some((f, sh)) => json!([{"image": f, "sheet": sh, "what": what}]),
        None => json!([{"image": model, "sheet": "", "what": what}]),
    };
    // With no print in the packet a judge can't fail anything: it is unsure.
    let verdict = |ok: bool| {
        if ok {
            "pass"
        } else if print.is_some() {
            "fail"
        } else {
            "unsure"
        }
    };
    let mut out = Vec::new();
    let elements: Vec<&Value> = objects["elements"].as_array().unwrap().iter().filter(|e| e["built"] != json!(true)).collect();
    for e in &elements {
        let h = e["handle"].as_str().unwrap();
        let Some(t) = truth.iter().find(|t| t.handle.eq_ignore_ascii_case(h)) else { continue };
        let at = ft(&e["at_ft"]);
        if has("G-PLACE") {
            let ok = dist(at, t.at) <= 0.5;
            out.push(
                json!({"rule": "G-PLACE", "handle": h, "verdict": verdict(ok), "issue": if ok { "" } else { "not where the power plan shows it" },
                "expected": format!("{:?}", t.at), "found": format!("{at:?}"), "evidence": ev("the symbol on the plan"), "confidence": 0.9}),
            );
        }
        if let Some(m) = t.mount
            && has("E-PANEL-MOUNT")
        {
            let found = e["mounting"].as_str().unwrap_or_default().to_ascii_lowercase();
            let ok = found.contains(m);
            out.push(json!({"rule": "E-PANEL-MOUNT", "handle": h, "verdict": verdict(ok), "issue": if ok { "" } else { "mounting differs from the panel schedule" },
                "expected": m, "found": found, "evidence": ev("the panel schedule row"), "confidence": 0.8}));
        }
        if t.gfci && has("E-RECEPT-TYPE") {
            let mark = e["mark"].as_str().unwrap_or_default().to_ascii_uppercase();
            let ok = mark.contains("GFCI");
            out.push(json!({"rule": "E-RECEPT-TYPE", "handle": h, "verdict": verdict(ok), "issue": if ok { "" } else { "the plan tags it GFCI" },
                "expected": "GFCI", "found": mark, "evidence": ev("the GFCI tag at the symbol"), "confidence": 0.85}));
        }
    }
    for r in objects["runs"].as_array().unwrap() {
        let h = r["handle"].as_str().unwrap();
        if j.sizes && has("M-DUCT-SIZE") {
            let size = r["size"].as_str().unwrap_or_default();
            let ok = size == "10x8";
            out.push(json!({"rule": "M-DUCT-SIZE", "handle": h, "verdict": verdict(ok), "issue": if ok { "" } else { "duct size differs from the plan's tag" },
                "expected": "10x8", "found": size, "evidence": ev("the size tag on the duct"), "confidence": 0.95}));
        }
        if j.noisy && has("M-DUCT-HEIGHT") && print.is_some() {
            out.push(json!({"rule": "M-DUCT-HEIGHT", "handle": h, "verdict": "fail", "issue": "the duct may sit below the ceiling",
                "expected": "above the ceiling", "found": "unclear", "evidence": ev("the duct on the plan"), "confidence": 0.4, "severity": "low"}));
        }
    }
    // What the prints show here that the model doesn't have (else: nothing is missing).
    let missing_before = out.len();
    let b: Vec<f64> = manifest["area"]["box"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    for t in truth.iter().filter(|t| t.at[0] >= b[0] && t.at[0] <= b[2] && t.at[1] >= b[1] && t.at[1] <= b[3]) {
        if print.is_some() && has("G-PRESENT") && !elements.iter().any(|e| dist(ft(&e["at_ft"]), t.at) <= 1.0) {
            out.push(json!({"rule": "G-PRESENT", "verdict": "missing", "issue": format!("the plan shows a {} here", t.kind),
                "expected": format!("{} at {:?}", t.kind, t.at), "found": "nothing", "evidence": ev("the symbol"), "confidence": 0.9}));
        }
    }
    if has("G-PRESENT") && !out[missing_before..].iter().any(|f| f["verdict"] == "missing") {
        out.push(json!({"rule": "G-PRESENT", "verdict": verdict(true), "issue": "", "expected": "what the plans show here", "found": "all of it",
            "evidence": ev("the area on the plan"), "confidence": 0.8}));
    }
    out
}

/// Loop validate.next → judge → validate.report until the run is done; the areas judged.
fn drive(s: &mut Session, run: &str, truth: &[Truth], j: Judge, limit: usize) -> Vec<String> {
    let mut done = Vec::new();
    for _ in 0..limit {
        let next = s.execute("validate.next", &json!({"run": run})).unwrap();
        if next["done"] == json!(true) {
            break;
        }
        let area = next["area"]["id"].as_str().unwrap().to_string();
        let packet = PathBuf::from(next["packet"]["dir"].as_str().unwrap());
        assert!(packet.join("prompt.md").exists() && next["rules"].is_array(), "{next}");
        let findings = judge(&packet, truth, j);
        let r = s.execute("validate.report", &json!({"run": run, "area": area, "findings": findings})).unwrap_or_else(|e| panic!("{area}: {e}"));
        assert_eq!(r["stored"].as_u64().unwrap() as usize, findings.len(), "{r}");
        done.push(area);
    }
    done
}

fn start(s: &mut Session, out: &Path) -> Value {
    s.execute("validate", &json!({"scope": "building", "out_dir": out.to_string_lossy()})).unwrap()
}

#[test]
fn a_fake_agent_runs_a_validation_and_people_grade_it() {
    let mut j = job();
    let out = dir("full");
    j.s.events_subscribe(true);
    let r = start(&mut j.s, &out);
    let run = r["run"].as_str().unwrap().to_string();
    assert!(run.starts_with("VR-"), "{r}");
    assert_eq!(r["areas_total"], 4, "office, storage, two corridor stretches: {r}");
    assert_eq!(r["next"], "validate.next");
    assert_eq!(r["prescreen"]["call"], "bim.validate", "{r}");
    assert!(r.get("waiting").is_none(), "an agent watches: {r}");
    let events = j.s.events_poll(0);
    assert!(events["events"].as_array().unwrap().iter().any(|e| e["type"] == "validate.started" && e["run"] == json!(run)), "{events}");

    let judged = drive(&mut j.s, &run, &j.truth, Judge { sizes: true, noisy: true }, 10);
    assert_eq!(judged.len(), 4, "{judged:?}");
    let last = j.s.execute("validate.next", &json!({"run": run})).unwrap();
    assert_eq!(last["done"], true, "{last}");

    // The review queue: the misplaced storage receptacle (fail + missing) and the duct-height alarm.
    let list = j.s.execute("review.list", &json!({"kind": "validation", "limit": 50})).unwrap();
    let items: Vec<&Value> = list["items"].as_array().unwrap().iter().filter(|i| i["id"].as_str().unwrap().starts_with("RV-J")).collect();
    assert_eq!(items.len(), 3, "{list}");
    // Ranked by severity × confidence: the low, 0.4 alarm comes last of the judged items.
    let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    let alarm = items.iter().find(|i| i["title"].as_str().unwrap().starts_with("M-DUCT-HEIGHT")).unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(*ids.last().unwrap(), alarm, "{ids:?}");
    let place = items.iter().find(|i| i["title"].as_str().unwrap().starts_with("G-PLACE fail")).unwrap()["id"].as_str().unwrap().to_string();
    let miss = items.iter().find(|i| i["title"].as_str().unwrap().starts_with("G-PRESENT missing")).unwrap()["id"].as_str().unwrap().to_string();
    // The evidence side by side: the print crop and the model drawn over it, and where to zoom.
    let got = review_item(&mut j.s, &place);
    let ev = &got["detail"]["evidence"][0];
    assert!(Path::new(ev["image"].as_str().unwrap()).exists(), "{got}");
    assert!(Path::new(ev["model"].as_str().unwrap()).exists(), "the model over the same crop: {got}");
    assert_eq!(got["detail"]["zoom"]["level"], "L1");
    assert_eq!(got["detail"]["expected"], "[22.0, 0.5]");
    assert_eq!(got["needs_citation"], false);

    // Only a person grades the judge.
    let e = j.s.execute("review.resolve", &json!({"id": place, "answer": "accept"})).unwrap_err().to_string();
    assert!(e.contains("person"), "{e}");
    let e = j.s.execute("review.resolve", &json!({"id": place, "answer": "maybe", "author": "user"})).unwrap_err().to_string();
    assert!(e.contains("accept") && e.contains("reject"), "{e}");
    j.s.execute("review.resolve", &json!({"id": place, "answer": "accept", "author": "user"})).unwrap();
    j.s.execute("review.resolve", &json!({"id": miss, "answer": "accept", "author": "user"})).unwrap();
    j.s.execute("review.resolve", &json!({"id": alarm, "answer": "reject", "author": "user"})).unwrap();

    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    println!("STATUS {}", serde_json::to_string_pretty(&st).unwrap());
    assert_eq!(st["areas"]["done"], 4, "{st}");
    assert_eq!(st["review"]["accepted"], 2, "{st}");
    assert_eq!(st["review"]["rejected"], 1, "{st}");
    assert_eq!(st["review"]["false_alarm_rate"], 0.333, "{st}");
    let by_rule = &st["findings"]["by_rule"];
    assert_eq!(by_rule["G-PLACE"]["fail"], 1, "{st}");
    assert!(by_rule["G-PLACE"]["pass"].as_u64().unwrap() >= 5, "{st}");
    let place_acc = st["accuracy"]["by_rule"]["G-PLACE"].as_f64().unwrap();
    let (p, f) = (by_rule["G-PLACE"]["pass"].as_f64().unwrap(), by_rule["G-PLACE"]["fail"].as_f64().unwrap());
    assert!((place_acc - (p / (p + f) * 1000.0).round() / 1000.0).abs() < 1e-9, "{st}");
    assert_eq!(st["accuracy"]["by_rule"]["M-DUCT-SIZE"], 1.0, "{st}");
    assert!(st["accuracy"]["by_trade"]["electrical"].as_f64().unwrap() < 1.0, "{st}");
    assert_eq!(st["accuracy"]["by_trade"]["mechanical"], 0.667, "fan placed, size pass, height (rejected) fail: {st}");
    assert!(st["prescreen"]["failures"].as_u64().unwrap() >= 1, "bim.validate ran first: {st}");
    // A graded item leaves the open queue; the grade is kept.
    let open = j.s.execute("review.list", &json!({"kind": "validation", "limit": 50})).unwrap();
    assert!(!open["items"].as_array().unwrap().iter().any(|i| i["id"] == json!(place)), "{open}");

    // The same run again: nothing changed, nothing to judge again; force judges all again.
    let again = start(&mut j.s, &out);
    assert_eq!(again["run"], json!(run));
    assert_eq!(again["pending"], 0, "{again}");
    assert_eq!(again["unchanged"], 4, "{again}");
    let forced = j.s.execute("validate", &json!({"scope": "building", "out_dir": out.to_string_lossy(), "force": true})).unwrap();
    assert_eq!(forced["pending"], 4, "{forced}");
    let _ = std::fs::remove_dir_all(&out);
}

fn review_item(s: &mut Session, id: &str) -> Value {
    s.execute("review.next", &json!({"id": id})).unwrap()["item"].clone()
}

#[test]
fn a_run_resumes_after_save_and_reopen() {
    let mut j = job();
    let out = dir("resume");
    let r = start(&mut j.s, &out);
    let run = r["run"].as_str().unwrap().to_string();
    // Started in the app with no agent watching: the status bar waits for one.
    assert_eq!(r["waiting"], "Validate: waiting for an agent", "{r}");
    assert_eq!(cadaddy_engine::cmd::validate_run::status_text(&j.s), Some("Validate: waiting for an agent"));
    let first = drive(&mut j.s, &run, &j.truth, Judge { sizes: true, noisy: false }, 1);
    assert_eq!(first.len(), 1);
    // An area handed out but not reported yet comes again first after the reopen.
    let second = j.s.execute("validate.next", &json!({"run": run})).unwrap()["area"]["id"].as_str().unwrap().to_string();
    assert_ne!(second, first[0]);

    let bytes = cadaddy_io::write(j.s.doc().unwrap(), "Sample Job.cdy").unwrap();
    let d = cadaddy_io::read(&bytes, "Sample Job.cdy").unwrap();
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Job", None);
    let st = s.execute("validate.status", &json!({})).unwrap();
    assert_eq!(st["run"], json!(run), "the latest run by default: {st}");
    assert_eq!(st["areas"]["done"], 1, "{st}");
    assert_eq!(st["areas"]["issued"], 1, "{st}");
    // The pending areas come first; the one handed out and never reported comes again only when
    // none is pending, and says so.
    let mut order: Vec<String> = Vec::new();
    for _ in 0..10 {
        let n = s.execute("validate.next", &json!({"run": run})).unwrap();
        if n["done"] == json!(true) {
            break;
        }
        let id = n["area"]["id"].as_str().unwrap().to_string();
        if id == second {
            assert!(n["again"] == json!(true) && n["note"].as_str().unwrap().contains("handed out before"), "{n}");
        } else {
            assert!(n.get("again").is_none(), "a pending area is no repeat: {n}");
        }
        let findings = judge(Path::new(n["packet"]["dir"].as_str().unwrap()), &j.truth, Judge { sizes: true, noisy: false });
        s.execute("validate.report", &json!({"run": run, "area": id, "findings": findings})).unwrap();
        order.push(id);
    }
    assert_eq!(order.len(), 3, "{order:?}");
    assert_eq!(order.last(), Some(&second), "the unreported area last: {order:?}");
    assert_eq!(order.iter().collect::<BTreeSet<_>>().len(), 3, "each area once: {order:?}");
    let st = s.execute("validate.status", &json!({"run": run})).unwrap();
    assert_eq!(st["areas"]["done"], 4, "{st}");
    assert_eq!(cadaddy_engine::cmd::validate_run::status_text(&s), None, "nothing left to wait for");

    // A changed area is judged again; the others keep their findings.
    let mut d = (*s.doc().unwrap()).clone();
    let h = cadaddy_engine::doc::Handle::parse_hex(&j.truth[3].handle).unwrap();
    d.modify_entity(h, |e| {
        if let EntityKind::Insert(i) = &mut e.kind {
            i.insert.x += 24.0;
        }
    })
    .unwrap();
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Job", None);
    let again = start(&mut s, &out);
    assert_eq!(again["run"], json!(run));
    assert_eq!(again["unchanged"], 3, "only the corridor stretch with the moved receptacle: {again}");
    assert_eq!(again["pending"], 1, "{again}");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn bad_reports_are_refused_whole_with_a_clear_message() {
    let mut j = job();
    let out = dir("bad");
    let r = start(&mut j.s, &out);
    let run = r["run"].as_str().unwrap().to_string();
    let e = j.s.execute("validate.report", &json!({"run": run, "area": "L1/101", "findings": []})).unwrap_err().to_string();
    assert!(e.contains("no packet yet") && e.contains("validate.next"), "{e}");
    let n = j.s.execute("validate.next", &json!({"run": run})).unwrap();
    let area = n["area"]["id"].as_str().unwrap().to_string();
    let packet = PathBuf::from(n["packet"]["dir"].as_str().unwrap());
    let good = judge(&packet, &j.truth, Judge { sizes: true, noisy: false });
    let img = good[0]["evidence"][0]["image"].as_str().unwrap().to_string();
    let model_img = read_json(&packet.join("manifest.json"))["model"][0]["file"].as_str().unwrap().to_string();
    let base = good[0].clone();
    let with = |k: &str, v: Value| {
        let mut f = base.clone();
        f[k] = v;
        f
    };
    let cases: Vec<(Value, &str)> = vec![
        (json!("nope"), "`findings` is a list"),
        (json!([]), "a pass is a finding too"),
        (json!([with("rule", json!("X-NOPE"))]), "not one of this packet's rules"),
        (json!([with("verdict", json!("maybe"))]), "`verdict` is pass, fail"),
        (json!([with("handle", json!("FFFFFF"))]), "not an object of the drawing"),
        (json!([with("confidence", json!(2))]), "`confidence` is a number from 0 to 1"),
        (json!([with("evidence", json!([{"image": "prints/none.png", "what": "x"}]))]), "not a picture of this packet"),
        (json!([with("evidence", json!([]))]), "`evidence` lists the pictures"),
        (json!([with("evidence", json!([{"image": img, "what": ""}]))]), "`what` says"),
        (
            json!([with("verdict", json!("fail"))
                .as_object()
                .map(|o| {
                    let mut o = o.clone();
                    o.insert("evidence".into(), json!([{"image": model_img, "what": "model"}]));
                    Value::Object(o)
                })
                .unwrap()]),
            "cites at least one print picture",
        ),
        (json!([with("verdict", json!("missing"))]), "a `missing` finding has no handle"),
        (
            json!([{"rule": "G-SOURCED", "verdict": "extra", "issue": "x", "evidence": [{"image": img, "what": "x"}], "confidence": 0.5}]),
            "an `extra` finding names",
        ),
        (json!([with("severity", json!("huge"))]), "`severity` is high, medium or low"),
    ];
    for (findings, want) in cases {
        let e = j.s.execute("validate.report", &json!({"run": run, "area": area, "findings": findings})).unwrap_err().to_string();
        assert!(e.contains(want), "{findings}: {e}");
    }
    // Several problems at once are all said; nothing was kept.
    let e =
        j.s.execute("validate.report", &json!({"run": run, "area": area, "findings": [with("rule", json!("X-ONE")), with("confidence", json!(-1))]}))
            .unwrap_err()
            .to_string();
    assert!(e.contains("finding 1") && e.contains("finding 2") && e.contains("nothing kept"), "{e}");
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    assert_eq!(st["areas"]["done"], 0, "{st}");
    // Unknown run, area, scope, trade, rule.
    for (cmd, p, want) in [
        ("validate.next", json!({"run": "VR-0"}), "no validate run VR-0"),
        ("validate.report", json!({"run": run, "area": "L9/1", "findings": []}), "not in run"),
        ("validate", json!({"scope": "galaxy"}), "unknown scope"),
        ("validate", json!({"scope": "level", "level": "L9", "out_dir": out.to_string_lossy()}), "no level named L9"),
        ("validate", json!({"trades": ["cooking"], "out_dir": out.to_string_lossy()}), "unknown trade"),
        ("validate", json!({"rules": ["X-NOPE"], "out_dir": out.to_string_lossy()}), "unknown rule"),
        ("validate", json!({"scope": "selection", "out_dir": out.to_string_lossy()}), "nothing is selected"),
        ("validate.plant", json!({"count": 0}), "`count` is a number from 1"),
        ("validate.plant", json!({"kinds": ["paint"]}), "unknown kind"),
    ] {
        let e = j.s.execute(cmd, &p).unwrap_err().to_string();
        assert!(e.contains(want), "{cmd} {p}: {e}");
    }
    // The good report goes in.
    let ok = j.s.execute("validate.report", &json!({"run": run, "area": area, "findings": good})).unwrap();
    assert_eq!(ok["area"], json!(area));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn scopes_pick_their_areas_and_rules() {
    let mut j = job();
    let out = dir("scopes");
    let room = j.s.execute("validate", &json!({"scope": "room", "room": "102", "out_dir": out.to_string_lossy()})).unwrap();
    assert_eq!(room["areas_total"], 1, "{room}");
    let corridor = j.s.execute("validate", &json!({"scope": "room", "room": "Corridor", "out_dir": out.to_string_lossy()})).unwrap();
    assert_eq!(corridor["areas_total"], 2, "both stretches: {corridor}");
    let level = j.s.execute("validate", &json!({"scope": "level", "level": "L1", "out_dir": out.to_string_lossy()})).unwrap();
    assert_eq!(level["areas_total"], 4);
    let win =
        j.s.execute("validate", &json!({"scope": "area", "area": {"area": [0, 0, 16, 12], "level": "L1"}, "out_dir": out.to_string_lossy()}))
            .unwrap();
    assert_eq!(win["areas_total"], 1);
    // The selection: the panel is in the office.
    let panel = j.truth[4].handle.clone();
    j.s.execute("select", &json!({"handles": [panel]})).unwrap();
    let sel = j.s.execute("validate", &json!({"scope": "selection", "out_dir": out.to_string_lossy()})).unwrap();
    assert_eq!(sel["areas"][0]["id"], "L1/101", "{sel}");
    // Trades limit the rules a packet gives the judge.
    let mech = j.s.execute("validate", &json!({"scope": "room", "room": "101", "trades": ["mechanical"], "out_dir": out.to_string_lossy()})).unwrap();
    let n = j.s.execute("validate.next", &json!({"run": mech["run"]})).unwrap();
    let rules: Vec<&str> = n["rules"].as_array().unwrap().iter().map(|r| r.as_str().unwrap()).collect();
    assert!(rules.contains(&"M-EF") && rules.contains(&"G-PLACE") && !rules.iter().any(|r| r.starts_with("E-")), "{rules:?}");
    let only = j.s.execute("validate", &json!({"scope": "room", "room": "101", "rules": ["g-place"], "out_dir": out.to_string_lossy()})).unwrap();
    let n = j.s.execute("validate.next", &json!({"run": only["run"]})).unwrap();
    assert_eq!(n["rules"], json!(["G-PLACE"]), "{n}");
    let st = j.s.execute("validate.status", &json!({})).unwrap();
    assert!(st["runs"].as_array().unwrap().len() >= 6, "{st}");
    let _ = std::fs::remove_dir_all(&out);
}

fn plant_record(s: &Session) -> Value {
    let a: Value = serde_json::from_str(s.doc().unwrap().agent.as_deref().unwrap()).unwrap();
    a["validate_plant"].clone()
}

#[test]
fn planted_errors_are_deterministic_and_scored() {
    let mut j = job();
    let original = (*j.s.doc().unwrap()).clone();
    let p = j.s.execute("validate.plant", &json!({"count": 6, "seed": 3})).unwrap();
    println!("PLANT {p}");
    assert_eq!(p["planted"], 6, "{p}");
    assert!(p.get("errors").is_none() && !p.to_string().contains(&j.truth[0].handle), "the answer doesn't say what was planted: {p}");
    assert_eq!(j.s.docs.len(), 2, "the copy opens beside the drawing");
    let rec = plant_record(&j.s);
    assert_eq!(rec["errors"].as_array().unwrap().len(), 6);
    let kinds: Vec<&str> = rec["errors"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    for k in ["move", "delete", "duct_size", "panel_mount", "fan_room", "drop_gfci"] {
        assert!(kinds.contains(&k), "{k} in {kinds:?}");
    }
    // The original is untouched.
    j.s.execute("switch", &json!({"index": 0})).ok();
    assert_eq!(*j.s.docs[0].doc, original, "the original drawing is unchanged");

    // The same seed plants the same errors; another seed others.
    let mut again = job();
    again.s.execute("validate.plant", &json!({"count": 6, "seed": 3})).unwrap();
    assert_eq!(plant_record(&again.s), rec);
    let mut other = job();
    other.s.execute("validate.plant", &json!({"count": 6, "seed": 4})).unwrap();
    assert_ne!(plant_record(&other.s)["errors"], rec["errors"]);

    // A run over the copy with a judge that doesn't compare duct sizes: that one is missed.
    let out = dir("plant");
    let mut s = again.s;
    let r = start(&mut s, &out);
    let run = r["run"].as_str().unwrap().to_string();
    drive(&mut s, &run, &again.truth, Judge { sizes: false, noisy: false }, 10);
    let st = s.execute("validate.status", &json!({"run": run})).unwrap();
    println!("PLANTED STATUS {}", serde_json::to_string_pretty(&st["planted"]).unwrap());
    let pl = &st["planted"];
    assert_eq!(pl["total"], 6, "{st}");
    assert_eq!(pl["caught"], 5, "{pl}");
    assert_eq!(pl["missed"], 1, "{pl}");
    assert_eq!(pl["recall"], 0.833, "{pl}");
    let errors = pl["errors"].as_array().unwrap();
    let status: BTreeMap<&str, &str> = errors.iter().map(|e| (e["kind"].as_str().unwrap(), e["status"].as_str().unwrap())).collect();
    assert_eq!(status["duct_size"], "missed", "{pl}");
    for k in ["move", "delete", "panel_mount", "fan_room", "drop_gfci"] {
        assert_eq!(status[k], "caught", "{k}: {pl}");
    }
    let duct = errors.iter().find(|e| e["kind"] == "duct_size").unwrap();
    assert_eq!(duct["handle"].as_str().unwrap().to_ascii_uppercase(), again.duct.to_ascii_uppercase());
    assert_eq!(duct["from"], "10x8");
    assert_eq!(duct["to"], "14x10");
    let _ = std::fs::remove_dir_all(&out);
}

/// The Validate panel's Cancel (Q6117): the run stops handing out areas and stops waiting for an
/// agent; what was judged stays; `validate` with the same scope resumes it.
#[test]
fn a_cancelled_run_stops_waiting_and_resumes() {
    let mut j = job();
    let out = dir("cancel");
    let r = start(&mut j.s, &out);
    let run = r["run"].as_str().unwrap().to_string();
    assert_eq!(r["waiting"], "Validate: waiting for an agent", "started by a person, nobody watching: {r}");
    let judged = drive(&mut j.s, &run, &j.truth, Judge { sizes: true, noisy: false }, 1);
    assert_eq!(judged.len(), 1);
    assert!(cadaddy_engine::cmd::validate_run::status_text(&j.s).is_some());
    let c = j.s.execute("validate.cancel", &json!({"run": run})).unwrap();
    assert_eq!(c["cancelled"], true, "{c}");
    assert_eq!(c["changed"], true, "{c}");
    assert_eq!(cadaddy_engine::cmd::validate_run::status_text(&j.s), None, "a cancelled run doesn't wait for an agent");
    let e = j.s.execute("validate.next", &json!({"run": run})).unwrap_err().to_string();
    assert!(e.contains("cancelled") && e.contains("resumes"), "{e}");
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    assert_eq!(st["cancelled"], true, "{st}");
    assert_eq!(st["areas"]["done"], 1, "the judged area stays: {st}");
    assert!(st.get("waiting").is_none(), "{st}");
    // Again: nothing changes.
    assert_eq!(j.s.execute("validate.cancel", &json!({})).unwrap()["changed"], false);
    // Resumed by the same scope: the judged area is kept, the rest are handed out again.
    let again = start(&mut j.s, &out);
    assert_eq!(again["run"], json!(run));
    assert_eq!(again["unchanged"], 1, "{again}");
    assert_eq!(again["pending"], 3, "{again}");
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    assert!(st.get("cancelled").is_none(), "{st}");
    assert!(j.s.execute("validate.next", &json!({"run": run})).unwrap()["area"]["id"].is_string());
    let _ = std::fs::remove_dir_all(&out);
}

/// Q6131: `validate.next` hands out every pending area before an issued, unreported one, and
/// says when it repeats one.
#[test]
fn next_hands_out_the_pending_areas_before_a_repeat() {
    let mut j = job();
    let out = dir("order");
    let run = start(&mut j.s, &out)["run"].as_str().unwrap().to_string();
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..4 {
        let n = j.s.execute("validate.next", &json!({"run": run})).unwrap();
        assert!(n.get("again").is_none(), "a pending area first: {n}");
        let id = n["area"]["id"].as_str().unwrap().to_string();
        assert!(!seen.contains(&id), "{id} twice before the others: {seen:?}");
        seen.push(id);
    }
    // None pending: the first one handed out and not reported comes again, and says so.
    let n = j.s.execute("validate.next", &json!({"run": run})).unwrap();
    assert_eq!(n["area"]["id"], json!(seen[0]), "{n}");
    assert_eq!(n["again"], true, "{n}");
    assert!(n["note"].as_str().unwrap().contains("handed out before"), "{n}");
    // Reporting the second still works after the others were handed out.
    let p = PathBuf::from(j.s.execute("validate.next", &json!({"run": run, "area": seen[1]})).unwrap()["packet"]["dir"].as_str().unwrap());
    let findings = judge(&p, &j.truth, Judge { sizes: true, noisy: false });
    j.s.execute("validate.report", &json!({"run": run, "area": seen[1], "findings": findings})).unwrap();
    let st = j.s.execute("validate.status", &json!({"run": run})).unwrap();
    assert_eq!((st["areas"]["done"].as_u64(), st["areas"]["issued"].as_u64()), (Some(1), Some(3)), "{st}");
    let _ = std::fs::remove_dir_all(&out);
}

/// A finding whose rule does not apply to its element's kind, or that names an element beside
/// the area or outside it, is refused with the problem named; a rule for every element is not.
#[test]
fn a_finding_must_fit_its_element_and_its_area() {
    let j = job();
    // A device just inside the storage room, by the wall it shares with the office.
    let (mut s, beside) = place(j.s, "RECEP", "E-POWR", [15.6, 6.0], "DUPLEX");
    let out = dir("fit");
    let run = start(&mut s, &out)["run"].as_str().unwrap().to_string();
    let n = s.execute("validate.next", &json!({"run": run, "area": "L1/101"})).unwrap();
    let packet = PathBuf::from(n["packet"]["dir"].as_str().unwrap());
    let objects = read_json(&packet.join("objects.json"));
    let own: Vec<&str> = objects["elements"].as_array().unwrap().iter().filter_map(|e| e["handle"].as_str()).collect();
    let near = objects["nearby"].as_array().unwrap();
    let up = |h: &str| h.to_ascii_uppercase();
    // The storage device is the office's neighbour, not its own: listed apart, with its room.
    assert!(!own.iter().any(|h| up(h) == up(&beside)), "{own:?}");
    let it = near.iter().find(|e| e["handle"].as_str().is_some_and(|h| up(h) == up(&beside))).unwrap_or_else(|| panic!("{near:?}"));
    assert!(it["room"].as_str().unwrap().contains("102"), "{it}");
    assert!(it["why"].as_str().unwrap().contains("not here"), "{it}");
    assert!(std::fs::read_to_string(packet.join("prompt.md")).unwrap().contains("`nearby`"));
    // Nothing of the storage room is judged here: none of the office packet's rules came from it alone.
    let manifest = read_json(&packet.join("manifest.json"));
    let print = manifest["prints"][0].clone();
    let ev = json!([{"image": print["file"], "sheet": print["sheet"], "what": "the symbol on the plan"}]);
    let f = |rule: &str, h: &str, verdict: &str| json!({"rule": rule, "handle": h, "verdict": verdict, "issue": "x", "expected": "x", "found": "x", "evidence": ev, "confidence": 0.8});
    let report = |s: &mut Session, list: Vec<Value>| s.execute("validate.report", &json!({"run": run, "area": "L1/101", "findings": list}));
    let duct = j.duct.clone();
    let fan = j.truth[5].handle.clone();
    let recep = j.truth[0].handle.clone();
    assert!(
        own.iter().any(|h| up(h) == up(&fan)) && objects["runs"].as_array().unwrap().iter().any(|r| r["handle"].as_str().map(up) == Some(up(&duct)))
    );
    for (finding, want) in [
        (f("G-HOST", &duct, "fail"), "does not apply"),
        (f("E-RECEPT-TYPE", &fan, "fail"), "does not apply"),
        (f("M-EF", &recep, "pass"), "does not apply"),
        (f("G-PLACE", &beside, "fail"), "beside this area"),
        (f("G-PLACE", &j.truth[3].handle, "pass"), "not one of this area's objects"),
    ] {
        let e = report(&mut s, vec![finding.clone()]).unwrap_err().to_string();
        assert!(e.contains(want), "{finding}: {e}");
    }
    let e = report(&mut s, vec![f("G-HOST", &duct, "fail")]).unwrap_err().to_string();
    assert!(e.contains("duct") && e.contains("receptacle"), "the element's kind and the rule's kinds are named: {e}");
    // A rule for every element fits any of the area's own; a rule of the element's kind fits it.
    let ok = report(&mut s, vec![f("G-PLACE", &duct, "pass"), f("M-EF", &fan, "pass"), f("E-RECEPT-TYPE", &recep, "pass")]).unwrap();
    assert_eq!(ok["stored"], 3, "{ok}");
    // The storage packet holds the device as its own.
    let n = s.execute("validate.next", &json!({"run": run, "area": "L1/102"})).unwrap();
    let objects = read_json(&PathBuf::from(n["packet"]["dir"].as_str().unwrap()).join("objects.json"));
    assert!(objects["elements"].as_array().unwrap().iter().any(|e| e["handle"].as_str().map(up) == Some(up(&beside))), "{objects}");
    let _ = std::fs::remove_dir_all(&out);
}
