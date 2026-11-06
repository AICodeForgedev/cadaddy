//! As-built as commands (Q5110): review markups on a printed sheet imported (a preview that keeps
//! nothing, then pending changes, the same file twice adding nothing), changes recorded, reviewed,
//! compared and exported as sheets and IFC, all kept in the drawing; a measured stud entered
//! through the framing and recorded on its wall; hostile text cut safely.

use std::path::PathBuf;
use std::sync::OnceLock;

use cadaddy_doc::markup::{Markup, MarkupKind};
use cadaddy_engine::Session;
use cadaddy_mepfix::asbuilt::{MAX_TEXT, building_to_paper};
use cadaddy_mepfix::truth::{Sheet, Truth};
use serde_json::{Value, json};

/// Seed 1's print set written to a folder (sheets, truth.json), once for every test.
fn set() -> &'static (Truth, PathBuf) {
    static SET: OnceLock<(Truth, PathBuf)> = OnceLock::new();
    SET.get_or_init(|| {
        let set = cadaddy_mepfix::generate(1).expect("the set");
        let dir = std::env::temp_dir().join(format!("cadaddy-asbuilt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        set.write_to(&dir).expect("written");
        (set.truth, dir)
    })
}

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

fn err(s: &mut Session, cmd: &str, p: Value) -> String {
    match s.execute(cmd, &p) {
        Ok(v) => panic!("{cmd} {p} should fail, gave {v}"),
        Err(e) => e.to_string(),
    }
}

/// Level 1's electrical plan.
fn e_sheet(t: &Truth) -> &Sheet {
    t.sheets.iter().find(|s| s.level.as_deref() == Some("1") && s.number.starts_with('E')).expect("an electrical plan of level 1")
}

fn note(at: [f64; 2], kind: MarkupKind, text: &str) -> Markup {
    Markup {
        kind,
        points: vec![at],
        strokes: Vec::new(),
        rect: [at[0] - 8.0, at[1] - 8.0, at[0] + 8.0, at[1] + 8.0],
        text: text.into(),
        author: "Field Reviewer A".into(),
        subject: String::new(),
        color: None,
        date: String::new(),
    }
}

/// The sheet's PDF with a cloud on a receptacle, an arrow from a switch 2' east and a callout on
/// a conduit, saved as a review copy.
fn review_pdf(t: &Truth, dir: &std::path::Path) -> (PathBuf, [String; 3]) {
    let sh = e_sheet(t);
    let pdf = std::fs::read(dir.join(&sh.pdf)).expect("the sheet's PDF");
    let pts = |p: [f64; 2]| {
        let q = building_to_paper(sh, p);
        [q[0] * 72.0, q[1] * 72.0]
    };
    let recep = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "receptacle").expect("a receptacle");
    let switch = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "switch").expect("a switch");
    let cond = t.elec_conduits.iter().find(|c| c.level == "1").expect("a conduit");
    let c = pts([recep.at_ft[0], recep.at_ft[1]]);
    let mut cloud = note(c, MarkupKind::Cloud, "Receptacle removed");
    cloud.points = vec![[c[0] - 9.0, c[1] - 9.0], [c[0] + 9.0, c[1] - 9.0], [c[0] + 9.0, c[1] + 9.0], [c[0] - 9.0, c[1] + 9.0]];
    let (tail, head) = (pts([switch.at_ft[0], switch.at_ft[1]]), pts([switch.at_ft[0] + 2.0, switch.at_ft[1]]));
    let mut arrow = note(tail, MarkupKind::Arrow, "Move switch");
    arrow.points = vec![head, tail];
    let tip = pts([(cond.from_ft[0] + cond.to_ft[0]) * 0.5, (cond.from_ft[1] + cond.to_ft[1]) * 0.5]);
    let mut callout = note(tip, MarkupKind::Callout, "Conduit verified in field");
    callout.points = vec![tip, [tip[0] + 30.0, tip[1] + 30.0]];
    callout.rect = [tip[0] + 30.0, tip[1] + 30.0, tip[0] + 130.0, tip[1] + 60.0];
    let marked = cadaddy_io::pdf_markups::add_markups(&pdf, 1, &[cloud, arrow, callout]).expect("marked up");
    let review = dir.join("review");
    std::fs::create_dir_all(&review).expect("folder");
    let path = review.join(&sh.pdf);
    std::fs::write(&path, marked).expect("written");
    (path, [recep.id.clone(), switch.id.clone(), cond.id.clone()])
}

#[test]
fn asbuilt_commands_import_record_review_compare_export() {
    let (t, dir) = set();
    let model = dir.display().to_string();
    let (pdf, ids) = review_pdf(t, dir);
    let pdf = pdf.display().to_string();
    let mut s = Session::new();

    // Nothing yet: an empty page, and a compare that says what it needs.
    assert_eq!(ex(&mut s, "asbuilt.list", json!({}))["total"], 0);
    assert!(err(&mut s, "asbuilt.compare", json!({})).contains("`model`"));

    // A preview keeps nothing.
    let r = ex(&mut s, "asbuilt.import_markups", json!({ "pdf": pdf, "model": model }));
    assert_eq!(r["preview"], true);
    assert_eq!(r["sheet"], e_sheet(t).number.as_str());
    let proposed = r["proposed"].as_array().expect("proposed");
    assert_eq!(
        proposed.iter().map(|p| p["element"].as_str().unwrap_or_default()).collect::<Vec<_>>(),
        ids.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(proposed.iter().map(|p| p["change"].as_str().unwrap_or_default()).collect::<Vec<_>>(), ["deleted", "moved", "field_verified"]);
    assert_eq!(ex(&mut s, "asbuilt.list", json!({}))["total"], 0, "a preview changes nothing");
    assert!(!s.doc().expect("doc").embedded.contains_key("asbuilt.json"));

    // Confirmed: three pending changes; the same file again adds nothing.
    let r = ex(&mut s, "asbuilt.import_markups", json!({ "pdf": pdf, "confirm": true }));
    assert_eq!((r["added"].as_u64(), r["already"].as_u64()), (Some(3), Some(0)));
    let r = ex(&mut s, "asbuilt.import_markups", json!({ "pdf": pdf, "confirm": true }));
    assert_eq!((r["added"].as_u64(), r["already"].as_u64()), (Some(0), Some(3)));
    let list = ex(&mut s, "asbuilt.list", json!({}));
    assert_eq!(list["total"], 3);
    let changes = list["changes"].as_array().expect("changes");
    assert_eq!(changes.iter().map(|c| c["id"].as_str().unwrap_or_default()).collect::<Vec<_>>(), ["AB-0001", "AB-0002", "AB-0003"]);
    assert!(changes.iter().all(|c| c["status"] == "pending"));
    assert_eq!(changes[1]["element"], ids[1].as_str());
    assert!(changes[1]["markup"].as_str().is_some_and(|m| m.starts_with("MK-")));

    // Recorded by hand: a move, kept with who and why.
    let dev = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "light_fixture").expect("a light");
    let to = [dev.at_ft[0] + 1.5, dev.at_ft[1] - 0.5];
    let r = ex(
        &mut s,
        "asbuilt.record",
        json!({ "kind": "move", "element": dev.id, "to": to, "status": "field_decision", "author": "Foreman B", "reason": "clear of the beam" }),
    );
    assert_eq!(r["change"]["id"], "AB-0004");
    assert_eq!(r["total"], 4);
    let stored: cadaddy_mepfix::asbuilt::AsBuiltRecord =
        serde_json::from_slice(s.doc().expect("doc").embedded.get("asbuilt.json").expect("kept in the drawing")).expect("the record");
    assert_eq!(stored.changes.len(), 4);
    assert_eq!(stored.markups.len(), 3);
    let c = stored.changes.iter().find(|c| c.id == "AB-0004").expect("the move");
    assert_eq!(
        (c.element_id.as_str(), c.element_kind.as_str(), c.author.as_str(), c.level.as_str()),
        (dev.id.as_str(), "elec_device", "Foreman B", "1")
    );
    assert_eq!(c.kind, cadaddy_mepfix::asbuilt::ChangeKind::Moved { from_ft: dev.at_ft, to_ft: [to[0], to[1], dev.at_ft[2]] });
    assert_eq!(c.status, cadaddy_mepfix::asbuilt::ChangeStatus::FieldDecision);
    // By a point instead of an id: the nearest element of the kind asked for.
    let r = ex(
        &mut s,
        "asbuilt.record",
        json!({ "kind": "delete", "near": [dev.at_ft[0] + 0.1, dev.at_ft[1]], "level": "1", "element_kind": "elec_device" }),
    );
    assert_eq!(r["change"]["element"], dev.id.as_str());
    assert_eq!(r["change"]["status"], "pending");
    // What's missing is said.
    assert!(err(&mut s, "asbuilt.record", json!({ "kind": "move", "element": dev.id })).contains("`to`"));
    assert!(err(&mut s, "asbuilt.record", json!({ "kind": "move", "to": [1, 2] })).contains("`element`"));
    assert!(err(&mut s, "asbuilt.record", json!({ "kind": "fly", "element": dev.id })).contains("unknown kind"));
    assert!(err(&mut s, "asbuilt.record", json!({ "kind": "move", "element": "NO-SUCH", "to": [1, 2] })).contains("no element"));

    // Reviewed.
    let r = ex(&mut s, "asbuilt.set_status", json!({ "ids": ["AB-0002", "NOPE"], "status": "change_order", "reason": "CO-4" }));
    assert_eq!(r["updated"], json!(["AB-0002"]));
    assert_eq!(r["missing"], json!(["NOPE"]));
    assert_eq!(ex(&mut s, "asbuilt.list", json!({ "status": "change_order" }))["total"], 1);
    let page = ex(&mut s, "asbuilt.list", json!({ "limit": 2 }));
    assert_eq!((page["changes"].as_array().map(Vec::len), page["next_offset"].as_u64()), (Some(2), Some(2)));

    // Compared: only reviewed changes count unless asked.
    let r = ex(&mut s, "asbuilt.compare", json!({}));
    assert_eq!(r["include_pending"], false);
    assert_eq!((r["total"].as_u64(), r["pending_not_applied"].as_u64()), (Some(2), Some(3)));
    let moved = r["deviations"].as_array().and_then(|d| d.iter().find(|x| x["change"] == "AB-0004").cloned()).expect("the move");
    assert!((moved["magnitude"].as_f64().unwrap_or(0.0) - 1.5f64.hypot(0.5)).abs() < 0.01);
    assert_eq!(ex(&mut s, "asbuilt.compare", json!({ "include_pending": true }))["total"], 5);

    // Exported: a sheet per level and the IFC, read back.
    let out = dir.join("asbuilt-out");
    let r = ex(&mut s, "asbuilt.export", json!({ "dir": out.display().to_string() }));
    let sheets = r["sheets"].as_array().expect("sheets");
    assert_eq!(sheets.len(), t.building.levels.len());
    let one = sheets.iter().find(|x| x["level"] == "1").expect("level 1");
    assert_eq!(one["marks"], 5, "every change on level 1 is pending or decided: none rejected");
    let back = cadaddy_io::read_dxf(&std::fs::read(one["path"].as_str().expect("path")).expect("written")).expect("reads");
    assert_eq!(back.model.iter().filter(|e| e.common.layer.to_string() == "ASBUILT-CHANGES").count(), 5);
    let ifc = std::fs::read(r["ifc"]["path"].as_str().expect("path")).expect("written");
    let model = cadaddy_io::ifc::read_ifc(&ifc).expect("reads");
    let tags: Vec<String> = model
        .model
        .iter()
        .map(|e| e.common.xdata.iter().map(|x| x.value.clone()).collect::<Vec<_>>())
        .find(|x| x.get(3).is_some_and(|n| n == &ids[1]))
        .expect("the switch, by its id");
    assert!(tags.iter().any(|v| v == "prop:CaDaddy_AsBuilt.AsBuiltStatus=Change Order"), "{tags:?}");
    assert!(tags.iter().any(|v| v == "prop:CaDaddy_AsBuilt.AsBuiltAuthor=Field Reviewer A"), "{tags:?}");
}

#[test]
fn hostile_text_is_cut_on_character_boundaries() {
    let (t, dir) = set();
    let model = dir.display().to_string();
    let mut s = Session::new();
    let long = "°".repeat(3_000);
    let dev = t.elec_devices.iter().find(|d| d.level == "1").expect("a device");
    let r = ex(&mut s, "asbuilt.record", json!({ "kind": "note", "element": dev.id, "reason": long, "author": long, "model": model }));
    assert!(r["change"]["reason"].as_str().is_some_and(|x| x.chars().all(|c| c == '°')));
    // Through a PDF: a note with the same text.
    let sh = e_sheet(t);
    let pdf = std::fs::read(dir.join(&sh.pdf)).expect("the sheet's PDF");
    let q = building_to_paper(sh, [dev.at_ft[0], dev.at_ft[1]]);
    let mut m = note([q[0] * 72.0, q[1] * 72.0], MarkupKind::Note, &long);
    m.author = long.clone();
    let marked = cadaddy_io::pdf_markups::add_markups(&pdf, 1, &[m]).expect("marked");
    let path = dir.join(format!("hostile-{}.pdf", std::process::id()));
    std::fs::write(&path, marked).expect("written");
    let r = ex(&mut s, "asbuilt.import_markups", json!({ "pdf": path.display().to_string(), "sheet": sh.number, "confirm": true }));
    assert_eq!(r["added"], 1);
    let rec: cadaddy_mepfix::asbuilt::AsBuiltRecord =
        serde_json::from_slice(s.doc().expect("doc").embedded.get("asbuilt.json").expect("kept")).expect("record");
    for c in &rec.changes {
        assert!(c.reason.len() <= MAX_TEXT && c.author.len() <= MAX_TEXT, "{} {}", c.reason.len(), c.author.len());
    }
    let mk = rec.markups.first().expect("the markup");
    assert!(mk.text.len() <= MAX_TEXT && mk.author.len() <= MAX_TEXT);
    assert_eq!(mk.text.chars().count(), MAX_TEXT / 2);
}

#[test]
fn a_measured_stud_is_recorded_on_its_wall() {
    let (_, dir) = set();
    let mut s = Session::new();
    ex(&mut s, "framing.build", json!({ "dir": dir.display().to_string() }));
    let walls = ex(&mut s, "framing.list", json!({ "limit": 500 }));
    let w = walls["walls"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|w| {
                    let l = |k: &str, i: usize| w[k][i].as_f64().unwrap_or(0.0);
                    w["stud_in"].as_f64().is_some() && (l("b_ft", 0) - l("a_ft", 0)).hypot(l("b_ft", 1) - l("a_ft", 1)) > 6.0
                })
                .cloned()
        })
        .expect("a stud wall");
    let id = w["id"].as_str().expect("id").to_string();
    let r = ex(
        &mut s,
        "asbuilt.record",
        json!({ "kind": "stud", "wall": id, "tape": { "from": "a", "to_near_edge_in": 15.25 }, "status": "field_decision", "author": "Carpenter A" }),
    );
    assert_eq!(r["change"]["element"], id.as_str());
    assert_eq!(r["change"]["element_kind"], "wall");
    assert_eq!(r["change"]["kind"], "field_verified");
    assert_eq!(r["framing"]["changed"], true);
    let centre = r["framing"]["centre_in"].as_f64().expect("centre");
    let wall = ex(&mut s, "framing.wall", json!({ "wall": id, "members": true, "limit": 1000 }));
    let members = wall["members"].as_array().expect("members");
    assert!(
        members.iter().any(|m| m["status"] == "field_verified" && (m["s_in"].as_f64().unwrap_or(f64::NAN) - centre).abs() < 0.05),
        "the framing reports the verified stud at {centre}: {wall}"
    );
    let list = ex(&mut s, "asbuilt.list", json!({ "element": id }));
    assert_eq!(list["total"], 1);
    // A stud the wall can't hold is refused and nothing is recorded.
    let e = err(&mut s, "asbuilt.record", json!({ "kind": "stud", "wall": id, "centre_in": -4 }));
    assert!(e.contains("not on the wall"), "{e}");
    assert_eq!(ex(&mut s, "asbuilt.list", json!({}))["total"], 1);
}
