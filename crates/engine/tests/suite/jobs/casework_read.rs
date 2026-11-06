//! Q301 rework, MUST 5: `casework.read` on an architectural floor plan of the made-up office:
//! every casework item and plumbing fixture with its tag, kind, family, height, SOURCE (the
//! sheet, the schedule) and STATUS, as a compact paged answer; `place` makes each item with a 3D
//! library family that model element, tagged with its id, source and status, once.

use cadaddy_engine::Session;
use cadaddy_mepfix::layout::Kind;
use serde_json::{Value, json};

/// The office seed's floor plan of `level`, drawn and read back from DXF, open in a session.
fn plan(seed: u64, level: &str) -> (Session, cadaddy_mepfix::truth::Truth, String) {
    let (t, st, kinds) = cadaddy_mepfix::office_planned(seed);
    let sh = t.sheets.iter().find(|s| s.number.starts_with("A-10") && s.level.as_deref() == Some(level)).expect("floor plan").clone();
    let kind: &Kind = &kinds.iter().find(|(n, _)| *n == sh.number).expect("kind").1;
    let (d, _, _) = cadaddy_mepfix::draw::draw(&t, &st, &sh, kind);
    let d = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&d).as_bytes()).expect("dxf");
    let mut s = Session::new();
    s.open_drawing(d, "plan.dxf", None);
    (s, t, sh.number)
}

fn items(v: &Value) -> Vec<Value> {
    v["items"].as_array().cloned().unwrap_or_default()
}

#[test]
fn casework_read_lists_items_with_source_and_status() {
    let (mut s, t, sheet) = plan(1, "2");
    let r = s.execute("casework.read", &json!({"limit": 200})).expect("casework.read");
    let level = "2";
    let truth_cw: Vec<_> = t.casework.iter().filter(|c| c.level == level).collect();
    let truth_fx = cadaddy_mepfix::casework_reader::truth_fixtures(&t, level);
    assert_eq!(r["count"].as_u64(), Some((truth_cw.len() + truth_fx.len()) as u64), "{r}");
    assert_eq!(r["sheet"], json!(sheet), "the source sheet from the title block");
    assert_eq!(r["schedule"]["title"], json!("CASEWORK AND PLUMBING FIXTURE SCHEDULE"));
    assert_eq!(r["unscheduled"], json!([]));
    let all = items(&r);
    assert_eq!(all.len(), truth_cw.len() + truth_fx.len());
    for it in &all {
        assert_eq!(it["status"], "schedule", "{it}");
        assert_eq!(it["source"]["sheet"], json!(sheet), "{it}");
        assert_eq!(it["source"]["schedule"], json!("CASEWORK AND PLUMBING FIXTURE SCHEDULE"), "{it}");
        assert!(it["id"].as_str().is_some_and(|id| id.starts_with(&format!("{sheet}:"))), "{it}");
    }
    // Every truth item is listed with its tag, kind and height (casework) or kind (fixtures).
    for c in &truth_cw {
        assert!(
            all.iter().any(|i| i["tag"] == json!(c.tag)
                && i["kind"] == json!(c.kind)
                && i["height_in"].as_f64() == Some(c.height_in)
                && i["category"] == "casework"),
            "{} missing",
            c.tag
        );
    }
    for (kind, tag, _) in &truth_fx {
        assert!(all.iter().any(|i| i["tag"] == json!(tag) && i["kind"] == json!(kind) && i["category"] == "fixture"), "{tag} missing");
    }
    let vanity = all.iter().find(|i| i["kind"] == "vanity").expect("a vanity");
    assert_eq!(vanity["family"], "M3_CABINET_BASE");
    assert_eq!(vanity["spec"], "06 41 16");
    assert_eq!(r["by_status"]["schedule"].as_u64(), Some(all.len() as u64));
    // Paged: five at a time, `next` to the following page.
    let p1 = s.execute("casework.read", &json!({"limit": 5})).expect("page 1");
    assert_eq!((items(&p1).len(), p1["next"].as_u64()), (5, Some(5)));
    let p2 = s.execute("casework.read", &json!({"limit": 5, "offset": 5})).expect("page 2");
    assert_eq!(items(&p2).first().map(|i| i["id"].clone()), all.get(5).map(|i| i["id"].clone()));
    // A kind filter.
    let wc = s.execute("casework.read", &json!({"kind": "water_closet"})).expect("kind");
    assert!(items(&wc).iter().all(|i| i["kind"] == "water_closet") && wc["count"].as_u64() > Some(0));
}

#[test]
fn casework_read_places_families_once() {
    let (mut s, _, sheet) = plan(1, "2");
    let r = s.execute("casework.read", &json!({"place": true, "limit": 200})).expect("place");
    let all = items(&r);
    let with_family = all.iter().filter(|i| !i["family"].is_null()).count();
    assert!(with_family >= 6, "{r}");
    assert_eq!(r["placed"].as_u64(), Some(with_family as u64));
    // Each placed model: the family's block, tagged with the item's id, source and status.
    let d = s.doc().expect("doc");
    for it in all.iter().filter(|i| !i["family"].is_null()) {
        let h = cadaddy_doc::Handle::parse_hex(it["handle"].as_str().expect("a handle")).expect("hex");
        let e = d.entity(h).expect("placed");
        let cadaddy_doc::EntityKind::Insert(ins) = &e.kind else { panic!("an insert") };
        assert_eq!(json!(ins.block), it["family"]);
        let x: Vec<&str> = e.common.xdata.iter().map(|t| t.value.as_str()).collect();
        assert_eq!(
            x,
            vec![
                "CADADDY_CASEWORK",
                it["id"].as_str().unwrap_or_default(),
                it["tag"].as_str().unwrap_or_default(),
                it["kind"].as_str().unwrap_or_default(),
                "schedule",
                sheet.as_str()
            ],
            "{it}"
        );
        // Casework stretched along its run: its width (24" base, 30" wall cabinet) to the run's
        // long side, its depth to the item's.
        if it["category"] == "casework" {
            let size: Vec<f64> = it["size"].as_array().expect("size").iter().filter_map(Value::as_f64).collect();
            let (long, short) = (size.iter().copied().fold(0.0, f64::max), size.iter().copied().fold(f64::MAX, f64::min));
            let (mw, md) = if ins.block == "M3_CABINET_WALL" { (30.0, 12.0) } else { (24.0, 25.0) };
            assert!((ins.scale.x * mw - long).abs() < 0.01 && (ins.scale.y * md - short).abs() < 0.01, "{it}: scale {:?}", ins.scale);
        }
    }
    // Again: nothing new to place.
    let again = s.execute("casework.read", &json!({"place": true})).expect("again");
    assert_eq!(again["placed"], json!(0));
}

#[test]
fn casework_read_on_an_empty_drawing_says_what_it_needs() {
    let mut s = Session::new();
    let r = s.execute("casework.read", &json!({})).expect("no error");
    assert_eq!(r["count"], json!(0));
    assert!(r["message"].as_str().is_some_and(|m| m.contains("CASEWORK")), "{r}");
    assert!(s.execute("casework.read", &json!({"limit": "ten"})).is_err());
}
