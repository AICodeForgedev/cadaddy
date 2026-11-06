//! Framing from the prints as commands (Q5103): a made-up print set's DXF sheets framed by
//! `framing.build`, a known wall's next-stud dimension and member counts checked against the
//! fixture's true framing, a field point surviving a rebuild (and the same entry twice changing
//! nothing), a shop drawing applied and kept, the framing views on `FRAMING-*` layers, and the
//! BOM's framing lines and bits coming from `Framing::takeoff` and the bore checks.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cadaddy_engine::Session;
use cadaddy_mepfix::framing::{self as fr, End, Framing, Status, WallFrame};
use cadaddy_mepfix::truth::Truth;
use serde_json::{Value, json};

const SEED: u64 = 1;

fn io() {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

/// The set's DXF sheets in a folder of their own, and its truth.
fn sheets(tag: &str) -> (Truth, PathBuf) {
    let set = cadaddy_mepfix::generate(SEED).expect("the set");
    let dir = std::env::temp_dir().join(format!("cadaddy-framing-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (n, b) in set.files.iter().filter(|(n, _)| n.ends_with(".dxf")) {
        std::fs::write(dir.join(n), b).expect("write");
    }
    (set.truth, dir)
}

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

/// Every framed wall, page by page.
fn all_walls(s: &mut Session) -> Vec<Value> {
    let mut out = Vec::new();
    let mut offset = 0;
    loop {
        let r = ex(s, "framing.list", json!({"offset": offset, "limit": 500}));
        out.extend(r["walls"].as_array().cloned().unwrap_or_default());
        match r["next_offset"].as_u64() {
            Some(n) => offset = n,
            None => return out,
        }
    }
}

fn near(a: &Value, b: [f64; 2]) -> bool {
    (a[0].as_f64().unwrap_or(f64::NAN) - b[0]).abs() < 0.1 && (a[1].as_f64().unwrap_or(f64::NAN) - b[1]).abs() < 0.1
}

/// A truth wall with studs, field studs under both layouts and the read wall that is it (same
/// ends, same direction): (truth index, read id).
fn known_wall(t: &Truth, open: &Framing, read: &[Value], want_stack: bool) -> (usize, String) {
    for (i, w) in open.walls.iter().enumerate() {
        let tw = t.plan_walls.get(i).expect("the truth wall");
        let both = [End::A, End::B].iter().all(|e| w.members.iter().any(|m| m.layout == Some(*e)));
        let stacked = open.stacks.iter().any(|g| g.contains(&i) && g.len() > 1);
        if w.wall.stud_in.is_none() || !both || w.wall.len_in() < 96.0 || (want_stack && !stacked) || tw.layout_from.is_none() {
            continue;
        }
        if let Some(r) = read.iter().find(|r| near(&r["a_ft"], w.wall.a_ft) && near(&r["b_ft"], w.wall.b_ft) && r["level"] == json!(w.wall.level)) {
            return (i, r["id"].as_str().expect("an id").to_string());
        }
    }
    panic!("no known wall");
}

fn by_status<'a>(members: impl Iterator<Item = &'a fr::Member>) -> BTreeMap<Status, usize> {
    let mut out = BTreeMap::new();
    for m in members {
        *out.entry(m.status).or_default() += 1;
    }
    out
}

fn status_of(v: &str) -> Status {
    match v {
        "fixed" => Status::Fixed,
        "agreed" => Status::Agreed,
        "rule" => Status::Rule,
        "shop_drawing" => Status::ShopDrawing,
        "field_verified" => Status::FieldVerified,
        other => panic!("status {other}"),
    }
}

/// The read wall's members under layout `e`, by status.
fn read_counts(s: &mut Session, id: &str, e: End) -> BTreeMap<Status, usize> {
    let w = ex(s, "framing.wall", json!({"wall": id, "members": true, "limit": 500}));
    let mut out = BTreeMap::new();
    for m in w["members"].as_array().expect("members") {
        if m["layout"].as_str().is_none_or(|l| l == e.letter()) {
            *out.entry(status_of(m["status"].as_str().expect("status"))).or_default() += 1;
        }
    }
    out
}

#[test]
fn a_known_wall_reads_like_the_true_framing_and_a_field_point_survives_a_rebuild() {
    io();
    let (t, dir) = sheets("known");
    let mut s = Session::new();
    let b = ex(&mut s, "framing.build", json!({"dir": dir.display().to_string()}));
    assert!(b["sheets"].as_u64().unwrap_or(0) > 5, "{b}");
    assert!(b["stud_walls"].as_u64().unwrap_or(0) > 20, "{b}");
    assert!(b["by_status"]["fixed"].as_u64().unwrap_or(0) > 300, "{b}");
    assert!(b["rule_layouts"]["rule_max"].as_u64().unwrap_or(0) > 0, "{b}");
    assert!(b["list"]["walls"].as_array().is_some_and(|w| !w.is_empty()), "the first page of walls comes with the build");

    // MUST 1: the same next-stud string and status counts as the truth under the true layout.
    let open = fr::frame(&fr::walls_from_truth(&t), &fr::posts_from_truth(&t), &[], &[]);
    let truth = fr::truth_framing(&t);
    let read = all_walls(&mut s);
    let (i, id) = known_wall(&t, &open, &read, true);
    let e = t.plan_walls.get(i).and_then(|w| w.layout_from.as_deref()).and_then(End::parse).expect("the true layout");
    let ow: &WallFrame = open.walls.get(i).expect("open wall");
    let n = ex(&mut s, "framing.next_stud", json!({"wall": id}));
    assert_eq!(n["from_a"], json!(ow.next_stud_words(End::A)), "{n}");
    assert_eq!(n["from_b"], json!(ow.next_stud_words(End::B)), "{n}");
    assert!(n["from_a"].as_str().is_some_and(|x| x.starts_with("next stud ")), "{n}");
    let tw = truth.walls.get(i).expect("true wall");
    println!("wall {id} (truth {i}, laid out from {}): {n}; true counts {:?}", e.letter(), by_status(tw.members.iter()));
    assert_eq!(read_counts(&mut s, &id, e), by_status(tw.members.iter()), "wall {id} under the true layout ({})", e.letter());
    let w = ex(&mut s, "framing.wall", json!({"wall": id}));
    assert!(w["layouts"]["a"]["field_studs"].as_u64().unwrap_or(0) > 0 && w["layouts"]["b"]["field_studs"].as_u64().unwrap_or(0) > 0, "{w}");
    let stack: Vec<String> = w["stack"].as_array().expect("stack").iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
    assert!(stack.len() > 1 && stack.contains(&id), "{w}");

    // MUST 2: a tape from the a end to the true layout's first stud; the run and its stack
    // snap to field verified, kept in the job, re-applied on a rebuild.
    let first = ow.next_stud_in(End::A)[if e == End::A { 0 } else { 1 }].expect("a first stud");
    let f = ex(&mut s, "framing.field_point", json!({"wall": id, "tape": {"from": "a", "to_near_edge_in": first}, "author": "agent-7"}));
    assert_eq!(f["changed"], json!(true), "{f}");
    assert_eq!(f["matches"], json!(e.letter()), "{f}");
    assert_eq!(f["author"], json!("agent-7"));
    let centre = f["centre_in"].as_f64().expect("centre");
    let agent_before = s.doc().expect("doc").agent.clone();
    assert!(agent_before.as_deref().is_some_and(|a| a.contains("agent-7") && a.contains(&id)), "the entry is in the job");
    let again = ex(&mut s, "framing.field_point", json!({"wall": id, "tape": {"from": "a", "to_near_edge_in": first}, "author": "agent-7"}));
    assert_eq!(again["changed"], json!(false), "{again}");
    assert_eq!(s.doc().expect("doc").agent, agent_before, "the same entry again changes nothing");

    let check = |s: &mut Session| {
        for sid in &stack {
            let w = ex(s, "framing.wall", json!({"wall": sid, "members": true, "limit": 500}));
            assert!(w["settled"].as_str().is_some_and(|x| x.starts_with("field verified")), "{sid}: {}", w["settled"]);
            assert_eq!(w["counts"]["rule_a"], json!(0), "{sid}");
            assert_eq!(w["counts"]["rule_b"], json!(0), "{sid}");
        }
        let w = ex(s, "framing.wall", json!({"wall": id, "members": true, "limit": 500}));
        let verified: Vec<f64> =
            w["members"].as_array().expect("members").iter().filter(|m| m["status"] == "field_verified").filter_map(|m| m["s_in"].as_f64()).collect();
        assert!(verified.iter().any(|v| (v - centre).abs() < 1.0 / 16.0), "a verified stud at {centre}: {verified:?}");
    };
    check(&mut s);
    // Rebuild from the sheets (as after a print revision), and again with the cache gone.
    let b2 = ex(&mut s, "framing.build", json!({}));
    assert_eq!(b2["reapplied"]["field_points"], json!(1), "{b2}");
    assert!(b2["stale"].as_array().is_some_and(Vec::is_empty), "{b2}");
    assert!(b2["by_status"]["field_verified"].as_u64().unwrap_or(0) > 0, "{b2}");
    check(&mut s);
    s.framing = None;
    check(&mut s);
    // A different stud on the same wall replaces the entry (one per wall), not a second one.
    let moved = ex(&mut s, "framing.field_point", json!({"wall": id, "centre_in": centre + 8.0}));
    assert_eq!(moved["changed"], json!(true));
    assert_eq!(moved["matches"], Value::Null, "8\" off both layouts: neither end");
    let agent: Value = serde_json::from_str(s.doc().expect("doc").agent.as_deref().expect("agent")).expect("json");
    assert_eq!(agent["framing"]["field_points"].as_array().map(Vec::len), Some(1), "{}", agent["framing"]);
    // Refused, and nothing kept: off the wall, a wall that isn't there.
    assert!(s.execute("framing.field_point", &json!({"wall": id, "centre_in": -5.0})).is_err());
    assert!(s.execute("framing.field_point", &json!({"wall": "NO-SUCH-WALL", "centre_in": 20.0})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn views_shop_drawings_and_the_bom_come_from_the_framing() {
    io();
    let (t, dir) = sheets("views");
    let mut s = Session::new();
    ex(&mut s, "framing.build", json!({"dir": dir.display().to_string()}));
    let read = all_walls(&mut s);
    let open = fr::frame(&fr::walls_from_truth(&t), &fr::posts_from_truth(&t), &[], &[]);
    let (i, id) = known_wall(&t, &open, &read, false);
    let ow = open.walls.get(i).expect("open wall").clone();
    let level = ow.wall.level.clone();

    // MUST 3: the elevation of a known wall, the level's plan and the Walk boxes.
    let v = ex(&mut s, "framing.view", json!({"wall": id}));
    let layers: Vec<&str> = v["layers"].as_array().expect("layers").iter().filter_map(Value::as_str).collect();
    for l in ["FRAMING-FIXED", "FRAMING-RULE-A", "FRAMING-RULE-B", "FRAMING-DIMS", "FRAMING-WALL"] {
        assert!(layers.contains(&l), "{l} in {layers:?}");
    }
    let texts: Vec<&str> = v["texts"].as_array().expect("texts").iter().filter_map(Value::as_str).collect();
    assert!(texts.contains(&ow.next_stud_words(End::A).as_str()), "the next-stud dimension is drawn: {texts:?}");
    let d = s.doc().expect("doc");
    let dims: Vec<String> = d
        .model
        .iter()
        .filter(|e| e.common.layer == "FRAMING-DIMS")
        .filter_map(|e| if let cadaddy_doc::EntityKind::Text(x) = &e.kind { Some(x.value.clone()) } else { None })
        .collect();
    assert!(dims.contains(&ow.next_stud_words(End::B)), "{dims:?}");
    let rule_layer = d.layer("FRAMING-RULE-A").expect("the rule layer");
    assert!(rule_layer.linetype.eq_ignore_ascii_case("DASHED"), "rule members are ghosted (dashed)");
    let n1 = v["objects"].as_u64().unwrap_or(0);
    let again = ex(&mut s, "framing.view", json!({"wall": id}));
    assert_eq!(again["replaced"].as_u64(), Some(n1), "drawing it again replaces the earlier one");
    let w = ex(&mut s, "framing.view", json!({"level": level, "walk": true}));
    let wl: Vec<&str> = w["layers"].as_array().expect("layers").iter().filter_map(Value::as_str).collect();
    assert!(wl.contains(&"FRAMING-3D-FIXED") && wl.contains(&"FRAMING-3D-RULE-A"), "{wl:?}");
    let meshes = s
        .doc()
        .expect("doc")
        .model
        .iter()
        .filter(|e| e.common.layer.starts_with("FRAMING-3D-") && matches!(e.kind, cadaddy_doc::EntityKind::Mesh(_)))
        .count();
    assert!(meshes > 100, "{meshes} member boxes");

    // A shop drawing (DXF, inches): studs on 16" centres along the known wall from its a end.
    let mut sd = cadaddy_doc::Drawing::new_imperial();
    let mut c = 8.0;
    while c < ow.wall.len_in() - 8.0 {
        let p = ow.wall.at(c);
        let common = cadaddy_doc::Common { layer: "STUD".into(), ..cadaddy_doc::Common::default() };
        let line = cadaddy_doc::Line {
            a: cadaddy_geom::Vec3::new(p[0] * 12.0 - 0.8, p[1] * 12.0, 0.0),
            b: cadaddy_geom::Vec3::new(p[0] * 12.0 + 0.8, p[1] * 12.0, 0.0),
        };
        sd.add(&cadaddy_doc::Space::Model, common, cadaddy_doc::EntityKind::Line(line)).expect("add");
        c += 16.0;
    }
    let shop = dir.join("shop-studs.dxf");
    std::fs::write(&shop, cadaddy_io::write(&sd, "shop-studs.dxf").expect("dxf")).expect("write");
    let sh = ex(&mut s, "framing.shop_drawing", json!({"file": shop.display().to_string(), "level": level, "author": "agent-7"}));
    assert_eq!(sh["changed"], json!(true), "{sh}");
    assert!(sh["studs"].as_u64().unwrap_or(0) > 3 && sh["walls_changed"].as_u64().unwrap_or(0) >= 1, "{sh}");
    let again = ex(&mut s, "framing.shop_drawing", json!({"file": shop.display().to_string(), "level": level}));
    assert_eq!(again["changed"], json!(false), "the same file again changes nothing");
    let rebuilt = ex(&mut s, "framing.build", json!({}));
    assert_eq!(rebuilt["reapplied"]["shop_drawings"], json!(1));
    assert!(rebuilt["by_status"]["shop_drawing"].as_u64().unwrap_or(0) > 3, "{rebuilt}");

    // MUST 4: the BOM's framing lines are the framing's takeoff (rule members at rule_max), and a
    // conduit run inside a framed wall bores its studs: a bit line.
    let built = s.execute("framing.list", &json!({})).expect("list");
    assert!(built["total"].as_u64().unwrap_or(0) > 0);
    // The engine's framing as the BOM sees it: rebuilt here from the same sheets and entries.
    let st: Value = serde_json::from_str(s.doc().expect("doc").agent.as_deref().expect("agent")).expect("json");
    let sheets: Vec<(String, Vec<u8>)> = st["framing"]["sheets"]
        .as_array()
        .expect("sheets")
        .iter()
        .filter_map(Value::as_str)
        .map(|p| (PathBuf::from(p).file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string(), std::fs::read(p).expect("sheet")))
        .collect();
    let mut f: Framing = fr::read_framing(&sheets);
    let drawing: fr::ShopDrawing = serde_json::from_value(st["framing"]["shop_drawings"][0]["drawing"].clone()).expect("shop");
    f.apply_shop_drawing(&level, &drawing);
    let take = f.takeoff();
    // Along the wall, 0.5' in from each end: 2" EMT (bigger than the punchouts).
    let a = ow.wall.at(6.0);
    let b = ow.wall.at(ow.wall.len_in() - 6.0);
    let spec = json!({
        "levels": [{"name": level, "elevation_ft": 0, "height_ft": 12}],
        "mep": {
            "systems": [{"name": "E-BRANCH-COND", "layer": "E-BRANCH-COND", "kind": "conduit", "size_in": [2.0, 0], "bottom_ft": 9}],
            "runs": [{"system": "E-BRANCH-COND", "level": level, "points": [[a[0] * 12.0, a[1] * 12.0], [b[0] * 12.0, b[1] * 12.0]], "size_in": [2.0, 0], "bottoms_ft": [9, 9]}]
        }
    });
    ex(&mut s, "bspec.set", json!({"replace": true, "spec": spec}));
    let bom = ex(&mut s, "bim.bom", json!({}));
    let lines: Vec<&Value> = bom["lines"].as_array().expect("lines").iter().filter(|l| l["category"] == "framing").collect();
    assert!(!lines.is_empty(), "{bom}");
    let sum = |pred: &dyn Fn(&Value) -> bool| lines.iter().filter(|l| pred(l)).map(|l| l["net"].as_f64().unwrap_or(0.0)).sum::<f64>();
    let rule_words = Status::Rule.words();
    for st in [Status::Fixed, Status::Agreed, Status::ShopDrawing, Status::FieldVerified] {
        assert_eq!(sum(&|l| l["size"] == json!(st.words())) as usize, take.by_status(st), "{st:?}");
    }
    assert_eq!(sum(&|l| l["size"] == json!(rule_words)) as usize, take.rule_max(), "rule members at the larger layout");
    let headers = sum(&|l| l["item"] == "Header");
    let true_headers: usize = take.counts.iter().filter(|((_, k), _)| *k == fr::Kind::Header).map(|(_, n)| n).sum();
    assert_eq!(headers as usize, true_headers, "headers");
    assert!(
        !bom["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .any(|l| l["category"] == "drywall" && l["item"].as_str().is_some_and(|i| i.starts_with("Studs "))),
        "framing replaces the drywall stud length"
    );
    let bits = bom["bits"].as_array().expect("bits");
    let holes: u64 = bits.iter().filter_map(|b| b["holes"].as_u64()).sum();
    let crossed = fr::wall_crossings(&f, &level, a, b, cadaddy_mepfix::bore::Raceway::Emt(2.0))
        .into_iter()
        .filter(|x| x.along && x.check.as_ref().is_some_and(|c| c.bit.is_some_and(|b| b.kind != cadaddy_mepfix::bore::BitKind::Punchout)))
        .map(|x| x.studs.iter().copied().max().unwrap_or(0))
        .sum::<usize>();
    println!("framing lines {}, takeoff {:?} rule_max {}, bits {bits:?}", lines.len(), take.counts.len(), take.rule_max());
    assert!(crossed > 3, "the run bores the wall's studs");
    assert_eq!(holes as usize, crossed, "{bits:?}");
    let req = bom["required"].as_array().expect("required");
    assert!(
        req.iter().any(|r| r["item"].as_str().is_some_and(|i| i.contains("hole saw"))
            && r["rule"].as_str().is_some_and(|x| x.starts_with(&format!("{crossed} holes")))),
        "a bit line in the tools: {req:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
