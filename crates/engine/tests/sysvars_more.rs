//! More system variables (gaps U10): each added variable's default, a value it takes, a value it
//! refuses, read-only ones refusing SETVAR, and the ones wired to behaviour (hatch settings,
//! image frames, the multiline style, TILEMODE, AREA/PERIMETER/DISTANCE, CVPORT, CMDACTIVE).

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.new_drawing(false);
    s
}

fn get(s: &mut Session, name: &str) -> Value {
    s.execute("getvar", &json!({ "name": name })).unwrap_or_else(|e| panic!("{name}: {e}"))["value"].clone()
}

fn set(s: &mut Session, name: &str, v: Value) -> bool {
    s.execute("setvar", &json!({ "name": name, "value": v })).is_ok()
}

fn close(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}

/// (name, default, a value it takes, a value it refuses).
fn settable() -> Vec<(&'static str, Value, Value, Value)> {
    let i = |d: i64, ok: i64, bad: i64| (json!(d), json!(ok), json!(bad));
    let mut v: Vec<(&str, (Value, Value, Value))> = vec![
        ("HPASSOC", i(1, 0, 2)),
        ("HPSEPARATE", i(0, 1, 2)),
        ("HPISLANDDETECTION", i(0, 2, 3)),
        ("HPORIGINMODE", i(0, 5, 6)),
        ("IMAGEFRAME", i(1, 2, 3)),
        ("ATTDIA", i(0, 1, 2)),
        ("BLIPMODE", i(0, 1, 2)),
        ("CMDDIA", i(1, 0, 2)),
        ("CMDECHO", i(1, 0, 2)),
        ("COORDS", i(1, 3, 4)),
        ("COPYMODE", i(0, 1, 2)),
        ("DBLCLKEDIT", i(1, 0, 2)),
        ("DELOBJ", i(3, -3, 4)),
        ("DRAGMODE", i(2, 0, 3)),
        ("DRAWORDERCTL", i(3, 0, 4)),
        ("EXPERT", i(0, 5, 6)),
        ("EXPLMODE", i(1, 0, 2)),
        ("FIELDDISPLAY", i(1, 0, 2)),
        ("FIELDEVAL", i(31, 0, 32)),
        ("GRIDDISPLAY", i(2, 15, 16)),
        ("GRIPBLOCK", i(0, 1, 2)),
        ("GRIPCOLOR", i(150, 1, 0)),
        ("GRIPHOT", i(12, 255, 256)),
        ("GRIPHOVER", i(11, 3, 0)),
        ("GRIPTIPS", i(1, 0, 2)),
        ("HALOGAP", i(0, 100, 101)),
        ("HIDEPRECISION", i(0, 1, 2)),
        ("HPBOUND", i(1, 0, 2)),
        ("HPDOUBLE", i(0, 1, 2)),
        ("HPDRAWORDER", i(3, 4, 5)),
        ("HPOBJWARNING", i(10_000, 1, 0)),
        ("ISAVEBAK", i(1, 0, 2)),
        ("ISAVEPERCENT", i(50, 0, 101)),
        ("ISOLINES", i(4, 2047, 2048)),
        ("LIMCHECK", i(0, 1, 2)),
        ("LWUNITS", i(1, 0, 2)),
        ("MAXSORT", i(1000, 0, 32768)),
        ("MBUTTONPAN", i(1, 0, 2)),
        ("MEASUREINIT", i(0, 1, 2)),
        ("MENUECHO", i(0, 15, 16)),
        ("MIRRHATCH", i(0, 1, 2)),
        ("MSLTSCALE", i(1, 0, 2)),
        ("NOMUTT", i(0, 1, 2)),
        ("OFFSETGAPTYPE", i(0, 2, 3)),
        ("OSNAPCOORD", i(2, 0, 3)),
        ("OSNAPZ", i(0, 1, 2)),
        ("PAPERUPDATE", i(0, 1, 2)),
        ("PEDITACCEPT", i(0, 1, 2)),
        ("PELLIPSE", i(0, 1, 2)),
        ("PICKAUTO", i(5, 0, 6)),
        ("PICKDRAG", i(2, 1, 3)),
        ("PLINEGEN", i(0, 1, 2)),
        ("PLINETYPE", i(2, 0, 3)),
        ("PLOTROTMODE", i(2, 1, 3)),
        ("PLQUIET", i(0, 1, 2)),
        ("PROXYNOTICE", i(1, 0, 2)),
        ("PROXYSHOW", i(1, 2, 3)),
        ("PSTYLEPOLICY", i(1, 0, 2)),
        ("QTEXTMODE", i(0, 1, 2)),
        ("REGENMODE", i(1, 0, 2)),
        ("REPORTERROR", i(1, 0, 2)),
        ("ROLLOVERTIPS", i(1, 0, 2)),
        ("SHORTCUTMENU", i(11, 31, 32)),
        ("SKPOLY", i(0, 2, 3)),
        ("SORTENTS", i(127, 0, 128)),
        ("SPLFRAME", i(0, 1, 2)),
        ("SPLINETYPE", i(6, 5, 4)),
        ("SURFTAB1", i(6, 2, 1)),
        ("SURFTAB2", i(6, 32766, 32767)),
        ("TEXTEVAL", i(0, 1, 2)),
        ("TEXTQLTY", i(50, 100, 101)),
        ("TOOLTIPS", i(1, 0, 2)),
        ("TRAYICONS", i(1, 0, 2)),
        ("TREEDEPTH", i(3020, -12000, 12001)),
        ("TSTACKALIGN", i(1, 2, 3)),
        ("TSTACKSIZE", i(70, 25, 24)),
        ("UCSFOLLOW", i(0, 1, 2)),
        ("UNITMODE", i(0, 1, 2)),
        ("VISRETAIN", i(1, 0, 2)),
        ("VPROTATEASSOC", i(1, 0, 2)),
        ("WHIPARC", i(0, 1, 2)),
        ("XEDIT", i(1, 0, 2)),
        ("XFADECTL", i(50, 90, 91)),
        ("XLOADCTL", i(2, 0, 3)),
        ("XREFNOTIFY", i(2, 1, 3)),
        ("XREFTYPE", i(0, 1, 2)),
        ("ZOOMWHEEL", i(0, 1, 2)),
        ("FACETRES", (json!(0.5), json!(2.5), json!(10.5))),
        ("HPSPACE", (json!(1.0), json!(0.25), json!(0))),
        ("LENSLENGTH", (json!(50.0), json!(35), json!(-1))),
        ("MLEADERSCALE", (json!(1.0), json!(0), json!(-2))),
        ("PLINEWID", (json!(0.0), json!(0.5), json!(-0.1))),
        ("PSVPSCALE", (json!(0.0), json!(48), json!(-1))),
        ("SKETCHINC", (json!(0.1), json!(0.05), json!(0))),
        ("CETRANSPARENCY", (json!("ByLayer"), json!("50"), json!([1, 2, 3]))),
        ("HPLAYER", (json!("."), json!("HATCH"), json!({}))),
        ("HPTRANSPARENCY", (json!("use current"), json!("30"), json!(null))),
        ("MTEXTED", (json!("Internal"), json!("notepad"), json!(true))),
    ];
    for n in 1..=5 {
        let ui: &'static str = Box::leak(format!("USERI{n}").into_boxed_str());
        let ur: &'static str = Box::leak(format!("USERR{n}").into_boxed_str());
        let us: &'static str = Box::leak(format!("USERS{n}").into_boxed_str());
        v.push((ui, i(0, -32768, 32768)));
        v.push((ur, (json!(0.0), json!(-2.5), json!("x"))));
        v.push((us, (json!(""), json!("hello"), json!("x".repeat(300)))));
    }
    v.into_iter().map(|(n, (d, ok, bad))| (n, d, ok, bad)).collect()
}

#[test]
fn every_added_variable_has_its_default_takes_a_value_and_refuses_a_bad_one() {
    let mut s = session();
    let vars = settable();
    assert!(vars.len() >= 80, "{} variables", vars.len());
    for (name, default, ok, bad) in &vars {
        assert!(close(&get(&mut s, name), default), "{name}: default {} ≠ {default}", get(&mut s, name));
        assert!(set(&mut s, name, ok.clone()), "{name} refused {ok}");
        let back = get(&mut s, name);
        let want = if let Some(t) = ok.as_str() { json!(t) } else { ok.clone() };
        assert!(close(&back, &want), "{name}: set {ok}, read {back}");
        assert!(!set(&mut s, name, bad.clone()), "{name} took {bad}");
        assert!(close(&get(&mut s, name), &want), "{name}: a refused value changed it");
        // Case doesn't matter.
        let _ = get(&mut s, &name.to_ascii_lowercase());
    }
    // They are listed by SETVAR ?.
    let all = s.execute("setvar", &json!({ "name": "?" })).ok();
    if let Some(Value::Object(m)) = all.as_ref().and_then(|v| v.get("variables").cloned().or_else(|| Some(v.clone()))) {
        for (name, ..) in &vars {
            assert!(m.contains_key(*name), "{name} not listed");
        }
    }
}

#[test]
fn read_only_variables_refuse_setvar() {
    let mut s = session();
    for name in [
        "AREA",
        "PERIMETER",
        "DISTANCE",
        "CDATE",
        "DATE",
        "CVPORT",
        "CMDACTIVE",
        "LASTANGLE",
        "ERRNO",
        "DIASTAT",
        "PLATFORM",
        "PRODUCT",
        "PROGRAM",
        "LOCALE",
        "SYSCODEPAGE",
        "WORLDUCS",
        "UCSNAME",
        "UCSORG",
        "UCSXDIR",
        "UCSYDIR",
        "TARGET",
        "VIEWDIR",
        "DWGCODEPAGE",
    ] {
        let before = get(&mut s, name);
        assert!(!before.is_null(), "{name}");
        assert!(!set(&mut s, name, json!(1)), "{name} is read-only");
    }
    assert_eq!(get(&mut s, "PRODUCT"), json!("CADaddy"));
    assert_eq!(get(&mut s, "WORLDUCS"), json!(1));
    assert_eq!(get(&mut s, "VIEWDIR"), json!([0.0, 0.0, 1.0]));
}

#[test]
fn hatch_settings_reach_hatch() {
    let mut s = session();
    let r = s.execute("rectang", &json!({ "p1": [0, 0], "p2": [10, 10] })).unwrap();
    assert!(set(&mut s, "HPASSOC", json!(0)));
    assert!(set(&mut s, "HPORIGIN", json!("2,3")));
    assert!(set(&mut s, "HPORIGINMODE", json!(0)));
    let h = s.execute("hatch", &json!({ "handles": [r["handle"]] })).unwrap();
    let hh = cadaddy_engine::doc::Handle::parse_hex(h["handle"].as_str().unwrap()).unwrap();
    match &s.doc().unwrap().entity(hh).unwrap().kind {
        cadaddy_engine::doc::EntityKind::Hatch(x) => {
            assert!(!x.associative);
            assert!(x.origin.near(cadaddy_engine::geom::Vec2::new(2.0, 3.0), 1e-12), "{:?}", x.origin);
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(get(&mut s, "HPORIGIN"), json!([2.0, 3.0, 0.0]));
    assert!(!set(&mut s, "HPORIGIN", json!(5)));
    assert!(!set(&mut s, "HPORIGIN", json!("nowhere")));
}

#[test]
fn multiline_style_must_exist() {
    let mut s = session();
    assert_eq!(get(&mut s, "CMLSTYLE"), json!("STANDARD"));
    assert!(set(&mut s, "CMLSTYLE", json!("standard")));
    assert!(!set(&mut s, "CMLSTYLE", json!("NoSuchStyle")));
}

#[test]
fn tilemode_switches_tabs() {
    let mut s = session();
    assert_eq!(get(&mut s, "TILEMODE"), json!(1));
    assert_eq!(get(&mut s, "CVPORT"), json!(2));
    assert!(set(&mut s, "TILEMODE", json!(0)));
    assert!(matches!(s.layout_space(), cadaddy_engine::doc::Space::Paper(_)));
    assert_eq!(get(&mut s, "TILEMODE"), json!(0));
    assert_eq!(get(&mut s, "CVPORT"), json!(1));
    assert!(set(&mut s, "TILEMODE", json!(1)));
    assert_eq!(s.layout_space(), cadaddy_engine::doc::Space::Model);
    assert!(!set(&mut s, "TILEMODE", json!(2)));
}

#[test]
fn area_perimeter_and_distance_follow_the_inquiries() {
    let mut s = session();
    s.execute("area", &json!({ "points": [[0, 0], [4, 0], [4, 3]] })).unwrap();
    assert!(close(&get(&mut s, "AREA"), &json!(6.0)));
    assert!(close(&get(&mut s, "PERIMETER"), &json!(12.0)));
    s.execute("dist", &json!({ "p1": [0, 0], "p2": [3, 4] })).unwrap();
    assert!(close(&get(&mut s, "DISTANCE"), &json!(5.0)));
    // Typed DIST too.
    s.cmdline("dist 0,0 6,8").unwrap();
    assert!(close(&get(&mut s, "DISTANCE"), &json!(10.0)));
}

#[test]
fn cmdactive_and_dates() {
    let mut s = session();
    assert_eq!(get(&mut s, "CMDACTIVE"), json!(0));
    s.cmdline("line").unwrap();
    assert_eq!(get(&mut s, "CMDACTIVE"), json!(1));
    s.cancel();
    let cdate = get(&mut s, "CDATE").as_f64().unwrap();
    let date = get(&mut s, "DATE").as_f64().unwrap();
    assert!(cdate > 20_200_101.0 && cdate < 30_000_000.0, "{cdate}");
    assert!(date > 2_458_000.0, "{date}");
}
