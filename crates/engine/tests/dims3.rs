//! DIMREGEN and the single DIM command's modes.

use cadaddy_doc::{DimKind, Dimension, EntityKind, Handle};
use cadaddy_engine::Session;
use serde_json::{Value, json};

fn h(r: &Value) -> String {
    r.get("handle").or_else(|| r["handles"].get(0)).and_then(Value::as_str).unwrap_or_else(|| panic!("no handle in {r}")).to_string()
}

fn dim(s: &Session, x: &str) -> Dimension {
    match &s.doc().unwrap().entity(Handle::parse_hex(x).unwrap()).unwrap().kind {
        EntityKind::Dimension(d) => d.clone(),
        k => panic!("not a dimension: {k:?}"),
    }
}

fn last_dim(s: &Session) -> (String, Dimension) {
    let e = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Dimension(_))).last().unwrap().clone();
    (
        e.handle.hex(),
        match &e.kind {
            EntityKind::Dimension(d) => d.clone(),
            _ => unreachable!(),
        },
    )
}

#[test]
fn dim_picks_the_kind_from_what_it_is_given() {
    let mut s = Session::new();
    let line = h(&s.execute("line", &json!({"points": [[0, 0], [10, 10]]})).unwrap());
    let circle = h(&s.execute("circle", &json!({"center": [30, 0], "radius": 5})).unwrap());
    let arc = h(&s.execute("arc", &json!({"center": [60, 0], "radius": 4, "start": 0, "end": 90})).unwrap());
    let l2 = h(&s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap());
    s.execute("dim", &json!({"p1": [0, 0], "p2": [10, 0], "at": [5, -3]})).unwrap();
    assert!(matches!(last_dim(&s).1.kind, DimKind::Linear { .. }));
    s.execute("dim", &json!({"handle": line})).unwrap();
    assert!(matches!(last_dim(&s).1.kind, DimKind::Aligned));
    s.execute("dim", &json!({"handle": circle, "at": [36, 3]})).unwrap();
    assert!(matches!(last_dim(&s).1.kind, DimKind::Diameter), "{:?}", last_dim(&s).1.kind);
    s.execute("dim", &json!({"handle": arc, "at": [63, 3]})).unwrap();
    assert!(matches!(last_dim(&s).1.kind, DimKind::Radius), "{:?}", last_dim(&s).1.kind);
    s.execute("dim", &json!({"lines": [line, l2], "at": [8, 3]})).unwrap();
    assert!(matches!(last_dim(&s).1.kind, DimKind::Angular | DimKind::Angular3P), "{:?}", last_dim(&s).1.kind);
    assert!(s.execute("dim", &json!({"mode": "sideways"})).is_err());
    let pt = h(&s.execute("point", &json!({"at": [0, 0]})).unwrap());
    assert!(s.execute("dim", &json!({"handle": pt})).is_err());
}

#[test]
fn dim_baseline_continue_and_ordinate_modes() {
    let mut s = Session::new();
    s.execute("dim", &json!({"p1": [0, 0], "p2": [10, 0], "at": [5, -3]})).unwrap();
    let n = |s: &Session| s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Dimension(_))).count();
    s.execute("dim", &json!({"mode": "continue", "points": [[20, 0], [30, 0]]})).unwrap();
    assert_eq!(n(&s), 3);
    s.execute("dim", &json!({"mode": "baseline", "points": [[40, 0]]})).unwrap();
    assert_eq!(n(&s), 4);
    s.execute("dim", &json!({"mode": "ordinate", "feature": [5, 5], "leader": [5, 15]})).unwrap();
    assert!(matches!(last_dim(&s).1.kind, DimKind::Ordinate { .. }));
}

#[test]
fn dim_align_distribute_and_layer() {
    let mut s = Session::new();
    // Four horizontal dimensions with their lines at y = -2, -3, -7, -12.
    let ds: Vec<String> = [2.0, 3.0, 7.0, 12.0]
        .iter()
        .enumerate()
        .map(|(i, off)| {
            let x = i as f64 * 20.0;
            s.execute("dim", &json!({"p1": [x, 0], "p2": [x + 10.0, 0], "at": [x + 5.0, -off]})).unwrap();
            last_dim(&s).0
        })
        .collect();
    // Distribute equally between the outermost: -2, -5.33, -8.67, -12.
    let r = s.execute("dim", &json!({"mode": "distribute", "handles": ds})).unwrap();
    assert_eq!(r["distributed"], 4);
    let ys: Vec<f64> = ds.iter().map(|d| dim(&s, d).defpt.y).collect();
    for (y, want) in ys.iter().zip([-12.0, -26.0 / 3.0, -16.0 / 3.0, -2.0]) {
        assert!(ys.iter().any(|v| (v - want).abs() < 1e-9), "{want} in {ys:?} ({y})");
    }
    // At an offset of 1.5 from the first one given.
    s.execute("dim", &json!({"mode": "distribute", "handles": ds, "type": "offset", "offset": 1.5})).unwrap();
    let mut gaps: Vec<f64> = ds.iter().map(|d| dim(&s, d).defpt.y).collect();
    gaps.sort_by(|a, b| a.total_cmp(b));
    for w in gaps.windows(2) {
        assert!((w[1] - w[0] - 1.5).abs() < 1e-9, "{gaps:?}");
    }
    // Align onto the first one's line.
    s.execute("dim", &json!({"mode": "align", "base": ds[0], "handles": &ds[1..]})).unwrap();
    let y0 = dim(&s, &ds[0]).defpt.y;
    assert!(ds.iter().all(|d| (dim(&s, d).defpt.y - y0).abs() < 1e-9));
    assert!(s.execute("dim", &json!({"mode": "distribute", "handles": [ds[0]]})).is_err());
    // Layer: new dimensions go on DIMS; "." the current layer again.
    s.execute("dim", &json!({"mode": "layer", "layer": "DIMS"})).unwrap();
    s.execute("dim", &json!({"p1": [0, 20], "p2": [10, 20], "at": [5, 25]})).unwrap();
    let on = |s: &Session| s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Dimension(_))).last().unwrap().common.layer.clone();
    assert_eq!(on(&s), "DIMS");
    assert_eq!(s.execute("getvar", &json!({"name": "DIMLAYER"})).unwrap()["value"], "DIMS");
    s.execute("dim", &json!({"mode": "layer", "layer": "."})).unwrap();
    s.execute("dim", &json!({"p1": [0, 30], "p2": [10, 30], "at": [5, 35]})).unwrap();
    assert_eq!(on(&s), "0");
}

#[test]
fn dimregen_puts_associative_dimensions_back_on_their_objects() {
    let mut s = Session::new();
    let l = h(&s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap());
    s.execute("dimlinear", &json!({"p1": [0, 0], "p2": [10, 0], "at": [5, -3]})).unwrap();
    let (d, before) = last_dim(&s);
    assert!(!before.assoc.is_empty(), "placed on the line's ends, the dimension is associative");
    // The line changed without the dimension following (as a file edited elsewhere would).
    s.doc_mut()
        .unwrap()
        .modify_entity(Handle::parse_hex(&l).unwrap(), |e| {
            if let EntityKind::Line(x) = &mut e.kind {
                x.b.x = 16.0;
            }
        })
        .unwrap();
    assert_eq!(dim(&s, &d).p14.x, 10.0);
    let r = s.execute("dimregen", &json!({})).unwrap();
    assert_eq!((r["updated"].as_u64(), r["regenerated"].as_u64()), (Some(1), Some(1)), "{r}");
    let after = dim(&s, &d);
    assert!([after.p13.x, after.p14.x].contains(&16.0), "{after:?}");
    // Nothing to do the second time; one undo puts it back.
    assert_eq!(s.execute("dimregen", &json!({})).unwrap()["updated"], 0);
    s.execute("undo", &json!({})).unwrap();
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(dim(&s, &d).p14.x, 10.0);
}

#[test]
fn dimedit_and_a_style_apply_to_many_dimensions_at_once() {
    let mut s = Session::new();
    s.execute("dim", &json!({"p1": [0, 0], "p2": [10, 0], "at": [5, -3]})).unwrap();
    let a = last_dim(&s).0;
    s.execute("dim", &json!({"p1": [0, 10], "p2": [10, 10], "at": [5, 7]})).unwrap();
    let b = last_dim(&s).0;
    s.execute("dim", &json!({"p1": [0, 20], "p2": [10, 20], "at": [5, 17]})).unwrap();
    let c = last_dim(&s).0;
    s.execute("dimedit", &json!({"handles": [a, b], "mode": "new", "text": "TYP <>"})).unwrap();
    assert!([&a, &b].iter().all(|x| dim(&s, x).text == "TYP <>"));
    assert_ne!(dim(&s, &c).text, "TYP <>");
    s.execute("dimedit", &json!({"handles": [a, b], "mode": "rotate", "angle": 30})).unwrap();
    assert!([&a, &b].iter().all(|x| (dim(&s, x).text_rotation.to_degrees() - 30.0).abs() < 1e-9));
    s.execute("dimedit", &json!({"handles": [a, b], "mode": "oblique", "angle": 60})).unwrap();
    assert!(
        [&a, &b].iter().all(|x| matches!(dim(&s, x).marks.oblique, Some(o) if (o.to_degrees() - 60.0).abs() < 1e-9)),
        "{:?}",
        dim(&s, &a).marks.oblique
    );
    // A style applied to the selected dimensions only (DIMSTYLE Apply / Dimension ▸ Update).
    s.execute("dimstyle", &json!({"name": "BIG", "textHeight": 0.5})).unwrap();
    s.execute("dimstyle.current", &json!({"name": "BIG"})).unwrap();
    s.execute("dimstyle.update", &json!({"handles": [a, c]})).unwrap();
    assert_eq!((dim(&s, &a).style.as_str(), dim(&s, &b).style.as_str(), dim(&s, &c).style.as_str()), ("BIG", "Standard", "BIG"));
}

#[test]
fn typed_dim_offers_its_modes_as_keywords() {
    let mut s = Session::new();
    let count = |s: &Session| s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Dimension(_))).count();
    s.cmdline("dim").unwrap();
    assert!(s.prompt_text().contains("Baseline") && s.prompt_text().contains("LAyer"), "{}", s.prompt_text());
    // LAyer first, then a linear dimension: on that layer.
    s.cmdline("la").unwrap();
    assert!(s.prompt_text().contains("layer name"), "{}", s.prompt_text());
    s.cmdline("NOTES-DIM").unwrap();
    for t in ["0,0", "10,0", "5,-3"] {
        s.cmdline(t).unwrap();
    }
    assert_eq!(count(&s), 1);
    let layer = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Dimension(_))).last().unwrap().common.layer.clone();
    assert_eq!(layer, "NOTES-DIM");
    s.cancel();
    // Continue: DIMCONTINUE's prompts from the last dimension.
    s.cmdline("dim").unwrap();
    s.cmdline("c").unwrap();
    s.cmdline("20,0").unwrap();
    assert_eq!(count(&s), 2, "{}", s.prompt_text());
    s.cancel();
    // Once a point is given, the keywords are the linear dimension's own.
    s.cmdline("dim").unwrap();
    s.cmdline("0,5").unwrap();
    assert!(!s.prompt_text().contains("Baseline"), "{}", s.prompt_text());
    s.cancel();
}
