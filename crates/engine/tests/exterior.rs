//! The outside from its drawings: a synthetic south elevation gives the levels' heights, the
//! roof and parapet, window sills and heads, a storefront band and a brick band; a roof plan
//! gives a sloped roof with a parapet, a cricket and a rooftop unit; `model.build` builds them.
//! The parapet height survives either reading order, guesses are marked, and north, east and
//! west elevations put their glass on the right face.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Mesh};
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn poly(s: &mut Session, pts: &[[f64; 2]]) {
    s.execute("pline", &json!({"vertices": pts, "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// G = the elevation's ground line.
const G: f64 = -1800.0;

fn drawing() -> Session {
    drawing_with(Some("PARAPET 3'-6\""), (354.0, "T.O. PARAPET EL. 129'-6\""))
}

/// The fixture with the roof plan's parapet note (none: the inner line alone) and the
/// elevation's parapet line (its height above the ground line and its label) given.
fn drawing_with(parapet_note: Option<&str>, parapet_line: (f64, &str)) -> Session {
    let mut s = Session::new();
    // The ground floor plan: a 40' × 30' box of 6" walls.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [480.0, 360.0]);
    rect(&mut s, [6.0, 6.0], [474.0, 354.0]);
    // The south elevation: level lines with labels, the silhouette, windows on Level 2, a
    // storefront with mullions on Level 1, a brick base.
    layer(&mut s, "A-ELEV");
    text(&mut s, [200.0, G - 60.0], "SOUTH ELEVATION");
    for (dy, label) in [(0.0, "LEVEL 1 EL. 100'-0\""), (168.0, "LEVEL 2 EL. 114'-0\""), (312.0, "T.O. ROOF EL. 126'-0\""), parapet_line] {
        line(&mut s, [-60.0, G + dy], [540.0, G + dy]);
        text(&mut s, [560.0, G + dy + 6.0], label);
    }
    rect(&mut s, [0.0, G], [480.0, G + 354.0]);
    for x in [60.0, 300.0] {
        rect(&mut s, [x, G + 168.0 + 36.0], [x + 36.0, G + 168.0 + 84.0]);
    }
    rect(&mut s, [120.0, G], [360.0, G + 120.0]);
    for x in [180.0, 240.0, 300.0] {
        line(&mut s, [x, G], [x, G + 120.0]);
    }
    line(&mut s, [0.0, G + 36.0], [120.0, G + 36.0]);
    s.execute("hatch", &json!({"points": [[60, G + 10.0]], "pattern": "BRICK"})).unwrap();
    text(&mut s, [30.0, G + 12.0], "BRICK");
    // The roof plan: the roof with a parapet line 8" inside, a slope arrow east at ¼"/ft, a
    // cricket and a rooftop unit.
    layer(&mut s, "A-ROOF");
    rect(&mut s, [2000.0, 0.0], [2480.0, 360.0]);
    rect(&mut s, [2008.0, 8.0], [2472.0, 352.0]);
    if let Some(note) = parapet_note {
        text(&mut s, [2200.0, 320.0], note);
    }
    line(&mut s, [2100.0, 180.0], [2400.0, 180.0]);
    line(&mut s, [2400.0, 180.0], [2388.0, 186.0]);
    line(&mut s, [2400.0, 180.0], [2388.0, 174.0]);
    text(&mut s, [2240.0, 190.0], "SLOPE 1/4\" / FT");
    poly(&mut s, &[[2440.0, 150.0], [2470.0, 180.0], [2440.0, 210.0], [2410.0, 180.0]]);
    text(&mut s, [2430.0, 178.0], "CRICKET 6\"");
    rect(&mut s, [2100.0, 240.0], [2172.0, 288.0]);
    text(&mut s, [2105.0, 260.0], "RTU-1 H=4'-6\"");
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]},
                {"name": "Level 2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "region": [-100, 1100, 1000, 1900]},
                {"name": "Roof", "elevation_ft": 24, "height_ft": 4, "ceiling_ft": 4, "region": [1900, -200, 3100, 1100]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s
}

fn spec(s: &mut Session) -> Value {
    s.execute("bspec.get", &json!({})).unwrap()["spec"].clone()
}

#[test]
fn the_elevation_gives_heights_openings_and_bands() {
    let mut s = drawing();
    let r = s.execute("elev.read", &json!({"region": [-200, G - 100.0, 800, G + 500.0]})).unwrap();
    assert_eq!(r["facade"], json!("South"), "{r}");
    let names: Vec<(&str, f64)> =
        r["levels"].as_array().unwrap().iter().map(|l| (l["name"].as_str().unwrap(), l["elevation_ft"].as_f64().unwrap())).collect();
    assert_eq!(names, vec![("LEVEL 1", 0.0), ("LEVEL 2", 14.0), ("T.O. ROOF", 26.0), ("T.O. PARAPET", 29.5)]);
    assert_eq!(r["levels"][1]["matched"], json!("Level 2"));
    assert_eq!((r["roof_ft"].clone(), r["parapet_ft"].clone()), (json!(26.0), json!(3.5)));
    assert_eq!(r["windows"], json!({"count": 2, "sill_ft": 3.0, "head_ft": 7.0}));
    let bands = r["bands"].as_array().unwrap();
    let glass = bands.iter().find(|b| b["kind"] == "glass").unwrap();
    assert_eq!(
        (glass["from_ft"].clone(), glass["to_ft"].clone(), glass["start_ft"].clone(), glass["end_ft"].clone()),
        (json!(0.0), json!(10.0), json!(10.0), json!(30.0))
    );
    let brick = bands.iter().find(|b| b["kind"] == "material").unwrap();
    assert_eq!((brick["material"].clone(), brick["from_ft"].clone(), brick["to_ft"].clone()), (json!("BRICK"), json!(0.0), json!(3.0)), "{brick}");
    // Applied: the levels' elevations and heights, the window heights, the facade.
    assert_eq!(spec(&mut s)["levels"][1]["elevation_ft"], json!(12.0), "not applied yet");
    s.execute("elev.read", &json!({"region": [-200, G - 100.0, 800, G + 500.0], "apply": true})).unwrap();
    let sp = spec(&mut s);
    let lv = |i: usize| (sp["levels"][i]["elevation_ft"].as_f64().unwrap(), sp["levels"][i]["height_ft"].as_f64().unwrap());
    assert_eq!((lv(0), lv(1), lv(2).0), ((0.0, 14.0), (14.0, 12.0), 26.0));
    assert_eq!((sp["openings"]["window_sill_ft"].clone(), sp["openings"]["window_head_ft"].clone()), (json!(3.0), json!(7.0)));
    assert_eq!(sp["exterior"]["facades"][0]["side"], json!("south"));
    // Nothing to read is an error, as is a bad region.
    assert!(s.execute("elev.read", &json!({"region": [5000, 5000, 5100, 5100]})).is_err());
    assert!(s.execute("elev.read", &json!({"region": [0, 0, 0, 10]})).is_err());
    assert!(s.execute("elev.read", &json!({"region": [-200, G - 100.0, 800, G + 500.0], "facade": "up"})).is_err());
}

#[test]
fn the_roof_plan_gives_slopes_parapets_and_equipment() {
    let mut s = drawing();
    let r = s.execute("roof.read", &json!({"region": [1900, -100, 3000, 1000]})).unwrap();
    assert_eq!(r["level"], json!("Roof"));
    let roofs = r["roofs"].as_array().unwrap();
    assert_eq!(roofs.len(), 1, "the parapet's inner line is not a second roof: {r}");
    let roof = &roofs[0];
    assert_eq!(roof["parapet_ft"], json!(3.5));
    let slope = &roof["slope"];
    assert_eq!(slope["down"], json!([1.0, 0.0]), "downhill to the arrow's head");
    assert!((slope["rise_per_ft"].as_f64().unwrap() - 0.25 / 12.0).abs() < 1e-12);
    assert_eq!(r["crickets"].as_array().unwrap().len(), 1);
    assert_eq!(r["crickets"][0]["height_ft"], json!(0.5));
    assert_eq!(r["equipment"][0]["name"], json!("RTU-1"));
    assert_eq!(r["equipment"][0]["height_ft"], json!(4.5));
    s.execute("roof.read", &json!({"region": [1900, -100, 3000, 1000], "apply": true})).unwrap();
    let parts = s.execute("bspec.get", &json!({})).unwrap()["parts"].clone();
    let roof_part = parts.as_array().unwrap().iter().find(|p| p["key"] == "roofs/Roof 1").unwrap()["text"].clone();
    assert_eq!(roof_part, json!("Roof 1: roof on Roof, sloping 1/4\"/ft, 3'-6\" parapet"));
    assert!(s.execute("roof.read", &json!({"region": [1900, -100, 3000, 1000], "level": "Attic"})).is_err());
}

fn made(s: &Session, part: &str) -> Vec<Mesh> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
        .collect()
}

fn z(m: &Mesh) -> (f64, f64) {
    m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)))
}

#[test]
fn model_build_makes_the_outside() {
    let mut s = drawing();
    s.execute("elev.read", &json!({"region": [-200, G - 100.0, 800, G + 500.0], "apply": true})).unwrap();
    s.execute("roof.read", &json!({"region": [1900, -100, 3000, 1000], "apply": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(
        [&r["roofs"], &r["parapets"], &r["crickets"], &r["equipment"], &r["facadeGlass"]].map(|v| v.as_u64().unwrap()),
        [1, 1, 1, 1, 1],
        "{r}"
    );
    // The roof: on the Roof level at 26', 10" higher at its west end (40' at ¼"/ft).
    let roof = &made(&s, "roof")[0];
    let top = |x: f64| roof.vertices.iter().filter(|v| (v.x - x).abs() < 1e-6).map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
    assert!((top(2000.0) - (312.0 + 10.0)).abs() < 1e-6 && (top(2480.0) - 312.0).abs() < 1e-6, "{} {}", top(2000.0), top(2480.0));
    // The parapet 3'-6" over the high point; the unit stands on the roof, 4'-6" high.
    assert!((z(&made(&s, "parapet")[0]).1 - (322.0 + 42.0)).abs() < 1e-6);
    let unit = z(&made(&s, "equipment")[0]);
    let under = 312.0 + (2480.0 - 2136.0) * 0.25 / 12.0;
    assert!((unit.0 - under).abs() < 1e-6 && (unit.1 - (under + 54.0)).abs() < 1e-6, "{unit:?}");
    assert!((z(&made(&s, "cricket")[0]).1 - (z(&made(&s, "cricket")[0]).0 + 6.0)).abs() < 1.0);
    // The storefront glass on the south face, 0 to 10' up, outside the wall.
    let glass = &made(&s, "facade-glass")[0];
    assert_eq!(z(glass), (0.0, 120.0));
    assert!(glass.vertices.iter().all(|v| v.y < 0.0));
    // 10' to 30' from the left as seen from the south: x 120 to 360.
    assert_near(extent(glass, |v| v.x), (120.0, 360.0));
    // The south walls are named brick.
    let brick = s.doc().unwrap().model.iter().filter(|e| e.common.xdata.iter().any(|t| t.value == "facade:South:BRICK")).count();
    assert!(brick >= 1);
}

/// When no explicit roof plan was read (`roof.read`), but the spec has a ROOF datum, model.build
/// auto-generates a flat roof slab from the top level's wall bounding rectangle.
#[test]
fn auto_roof_from_datum_without_explicit_roof_plan() {
    let mut s = Session::new();
    // A two-level building with walls on L1. The ROOF datum says the roof is at 24'.
    layer(&mut s, "A-WALL");
    // Level 1 plan: a 40' × 30' rectangle of walls (outer face).
    rect(&mut s, [0.0, 0.0], [480.0, 360.0]);
    rect(&mut s, [6.0, 6.0], [474.0, 354.0]);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "version": 1,
            "units_per_foot": 12,
            "levels": [
                {"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 600, 500], "base": [0, 0]},
                {"name": "Level 2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 600, 500]}
            ],
            "datums": [{"name": "ROOF", "elevation_ft": 24, "kind": "roof", "source": "estimated"}],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    // Build without reading a roof plan. No roof.read was called, so sp.exterior.roofs is empty.
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["roofs"].as_u64().unwrap_or(0), 1, "expected auto-roof: {r}");
    // The auto roof should be tagged as "roof" and sit at 24' × 12 = 288 drawing units.
    let roofs = made(&s, "roof");
    assert_eq!(roofs.len(), 1, "expected one roof mesh");
    let (lo, hi) = z(&roofs[0]);
    let expected_z = 24.0 * 12.0; // 288
    assert!((hi - expected_z).abs() < 1.0, "roof top {hi} should be near {expected_z}");
    assert!(hi > lo, "roof slab should have thickness");
    // The roof should appear in the model takeoff as an architecture element.
    let t = s.execute("bim.takeoff", &json!({})).unwrap();
    let has_roof = t["lines"].as_array().unwrap().iter().any(|l| l["category"].as_str() == Some("roofs"));
    assert!(has_roof, "roof should appear in takeoff: {t}");
}

/// When there is also a PARAPET datum, the auto-roof includes a parapet.
#[test]
fn auto_roof_includes_parapet_from_datum() {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [480.0, 360.0]);
    rect(&mut s, [6.0, 6.0], [474.0, 354.0]);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "version": 1,
            "units_per_foot": 12,
            "levels": [
                {"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 600, 500], "base": [0, 0]}
            ],
            "datums": [
                {"name": "ROOF", "elevation_ft": 12, "kind": "roof", "source": "estimated"},
                {"name": "PARAPET", "elevation_ft": 15, "kind": "parapet", "source": "estimated"}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["roofs"].as_u64().unwrap_or(0), 1, "expected auto-roof: {r}");
    assert_eq!(r["parapets"].as_u64().unwrap_or(0), 1, "expected auto-parapet: {r}");
    let parapets = made(&s, "parapet");
    assert_eq!(parapets.len(), 1, "expected one parapet mesh");
    let (_, parapet_hi) = z(&parapets[0]);
    let expected_parapet_z = 15.0 * 12.0; // 180
    assert!((parapet_hi - expected_parapet_z).abs() < 1.0, "parapet top {parapet_hi} should be near {expected_parapet_z}");
}

fn extent(m: &Mesh, f: impl Fn(&cadaddy_engine::geom::Vec3) -> f64) -> (f64, f64) {
    m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(f(v)), hi.max(f(v))))
}

/// Plan coordinates, to rounding.
#[track_caller]
fn assert_near(got: (f64, f64), want: (f64, f64)) {
    assert!((got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9, "{got:?} != {want:?}");
}

const ELEVATION: [f64; 4] = [-200.0, G - 100.0, 800.0, G + 500.0];
const ROOF_PLAN: [f64; 4] = [1900.0, -100.0, 3000.0, 1000.0];

/// The parapet's top once built (the roof's high point is 322: 26' + 10").
fn built_parapet_top(s: &mut Session) -> f64 {
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["parapets"], json!(1), "{r}");
    z(&made(s, "parapet")[0]).1
}

fn roof_parapet(sp: &Value) -> (Value, Value, Value) {
    let r = &sp["exterior"]["roofs"][0];
    (r["parapet_ft"].clone(), r["parapet_status"].clone(), r["parapet_from"].clone())
}

#[test]
fn the_elevation_parapet_beats_the_default_in_either_order() {
    // The roof plan draws the parapet (its inner line) with no height note; the elevation
    // gives 3'-6" (T.O. PARAPET 129'-6" over T.O. ROOF 126'-0").
    let line = (354.0, "T.O. PARAPET EL. 129'-6\"");
    let read = (json!(3.5), json!("read"), json!("elevation"));
    // (a) The elevation first: there is no roof yet, the height is kept for the roof level.
    let mut s = drawing_with(None, line);
    let e = s.execute("elev.read", &json!({"region": ELEVATION, "apply": true})).unwrap();
    assert_eq!(e["parapet"], json!({"level": "Roof", "height_ft": 3.5, "applied": true, "roofs": [], "kept": []}), "{e}");
    assert_eq!(spec(&mut s)["exterior"]["parapets"], json!([{"level": "Roof", "height_ft": 3.5, "facade": "south"}]));
    let r = s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    assert_eq!(r["roofs"][0]["parapet_ft"], json!(3.5), "{r}");
    assert_eq!(r["assumed"], json!([]), "the elevation gave the height: nothing assumed");
    assert_eq!(roof_parapet(&spec(&mut s)), read);
    let a = built_parapet_top(&mut s);
    assert!((a - (322.0 + 42.0)).abs() < 1e-9, "{a}");
    // (b) The roof plan first: a 3'-0" default, marked; the elevation then replaces it.
    let mut s = drawing_with(None, line);
    let r = s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    assert_eq!(r["roofs"][0]["parapet_ft"], json!(3.0));
    assert_eq!(r["assumed"][0]["item"], json!("Roof 1"));
    assert_eq!(r["assumed"][0]["value"], json!("parapet_ft"));
    assert_eq!(roof_parapet(&spec(&mut s)), (json!(3.0), json!("default"), json!("default")));
    let e = s.execute("elev.read", &json!({"region": ELEVATION, "apply": true})).unwrap();
    assert_eq!(e["parapet"], json!({"level": "Roof", "height_ft": 3.5, "applied": true, "roofs": ["Roof 1"], "kept": []}), "{e}");
    assert_eq!(e["conflicts"], json!([]));
    assert_eq!(roof_parapet(&spec(&mut s)), read);
    let b = built_parapet_top(&mut s);
    assert!((b - (322.0 + 42.0)).abs() < 1e-9, "{b}");
    assert_eq!(a, b, "the same either way");
    // A preview says what it would do and changes nothing.
    let mut s = drawing_with(None, line);
    let e = s.execute("elev.read", &json!({"region": ELEVATION})).unwrap();
    assert_eq!(e["parapet"]["applied"], json!(false));
    assert!(spec(&mut s)["exterior"]["parapets"].is_null());
}

#[test]
fn a_roof_plan_note_is_kept_and_a_disagreement_reported() {
    // The roof plan notes 3'-6"; the elevation shows 4'-0" (130'-0" over 126'-0").
    let line = (360.0, "T.O. PARAPET EL. 130'-0\"");
    let conflict = |c: &Value| (c["roof"].clone(), c["roof_plan_ft"].clone(), c["elevation_ft"].clone(), c["kept_ft"].clone());
    let want = (json!("Roof 1"), json!(3.5), json!(4.0), json!(3.5));
    // Roof plan, then elevation: the elevation's answer reports it.
    let mut s = drawing_with(Some("PARAPET 3'-6\""), line);
    s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    let e = s.execute("elev.read", &json!({"region": ELEVATION, "apply": true})).unwrap();
    assert_eq!(e["parapet_ft"], json!(4.0));
    assert_eq!((e["parapet"]["roofs"].clone(), e["parapet"]["kept"].clone()), (json!([]), json!(["Roof 1"])), "{e}");
    assert_eq!(e["conflicts"].as_array().unwrap().len(), 1, "{e}");
    assert_eq!(conflict(&e["conflicts"][0]), want);
    assert_eq!(roof_parapet(&spec(&mut s)), (json!(3.5), json!("read"), json!("roof plan")));
    let top = built_parapet_top(&mut s);
    assert!((top - (322.0 + 42.0)).abs() < 1e-9, "the roof plan's note is kept: {top}");
    // Elevation, then roof plan: the roof plan's answer reports it.
    let mut s = drawing_with(Some("PARAPET 3'-6\""), line);
    let e = s.execute("elev.read", &json!({"region": ELEVATION, "apply": true})).unwrap();
    assert_eq!(e["conflicts"], json!([]), "no roof yet");
    let r = s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    assert_eq!(r["conflicts"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(conflict(&r["conflicts"][0]), want);
    assert_eq!(roof_parapet(&spec(&mut s)), (json!(3.5), json!("read"), json!("roof plan")));
}

#[test]
fn defaulted_values_are_marked() {
    let mut s = drawing();
    // The existing fixture: every value is on the drawings.
    let r = s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    assert_eq!(r["assumed"], json!([]), "{r}");
    let e = s.execute("elev.read", &json!({"region": ELEVATION})).unwrap();
    assert_eq!(e["assumed"], json!([]), "{e}");
    let sp = spec(&mut s);
    let x = &sp["exterior"];
    assert_eq!((x["roofs"][0]["parapet_status"].clone(), x["roofs"][0]["slope"]["direction_status"].clone()), (json!("read"), json!("read")));
    assert_eq!((x["crickets"][0]["height_status"].clone(), x["equipment"][0]["height_status"].clone()), (json!("read"), json!("read")));
    // A second roof plan: a parapet line with no note, a unit without H=, a cricket without a
    // height, and a slope arrow with no head.
    layer(&mut s, "A-ROOF");
    rect(&mut s, [4000.0, 0.0], [4480.0, 360.0]);
    rect(&mut s, [4008.0, 8.0], [4472.0, 352.0]);
    line(&mut s, [4100.0, 180.0], [4400.0, 180.0]);
    text(&mut s, [4240.0, 190.0], "SLOPE 1/4\" / FT");
    poly(&mut s, &[[4440.0, 150.0], [4470.0, 180.0], [4440.0, 210.0], [4410.0, 180.0]]);
    text(&mut s, [4430.0, 178.0], "CRICKET");
    rect(&mut s, [4100.0, 240.0], [4172.0, 288.0]);
    text(&mut s, [4105.0, 260.0], "RTU-2");
    let r = s.execute("roof.read", &json!({"region": [3900, -100, 5000, 1000], "apply": true})).unwrap();
    let mut got: Vec<(String, String)> = r["assumed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["item"].as_str().unwrap().to_string(), a["value"].as_str().unwrap().to_string()))
        .collect();
    got.sort();
    let want: Vec<(String, String)> = [("Cricket 1", "height_ft"), ("RTU-2", "height_ft"), ("Roof 1", "parapet_ft"), ("Roof 1", "slope.down")]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    assert_eq!(got, want, "{r}");
    let value =
        |item: &str, v: &str| r["assumed"].as_array().unwrap().iter().find(|a| a["item"] == item && a["value"] == v).unwrap()["assumed"].clone();
    assert_eq!((value("Roof 1", "parapet_ft"), value("RTU-2", "height_ft"), value("Cricket 1", "height_ft")), (json!(3.0), json!(4.0), json!(0.5)));
    assert_eq!(value("Roof 1", "slope.down"), json!([1.0, 0.0]), "to the end drawn last");
    // Stored as defaults, and bspec.get shows them.
    let got = s.execute("bspec.get", &json!({})).unwrap();
    let x = &got["spec"]["exterior"];
    assert_eq!(roof_parapet(&got["spec"]), (json!(3.0), json!("default"), json!("default")));
    assert_eq!(x["roofs"][0]["slope"]["direction_status"], json!("default"));
    assert_eq!((x["crickets"][0]["height_status"].clone(), x["equipment"][0]["height_status"].clone()), (json!("default"), json!("default")));
    let part = |key: &str| got["parts"].as_array().unwrap().iter().find(|p| p["key"] == key).unwrap()["text"].clone();
    assert_eq!(part("roofs/Roof 1"), json!("Roof 1: roof on Roof, sloping 1/4\"/ft (direction assumed), 3'-0\" parapet (assumed)"));
    assert_eq!(part("equipment/RTU-2"), json!("RTU-2: rooftop equipment on Roof, 4'-0\" high (assumed)"));
}

/// An elevation of the 40' (480) or 30' (360) wide face whose ground line is at `y`, titled
/// `title`: the level lines and a storefront 10' to 30' from its left end, 10' high.
fn elevation(s: &mut Session, y: f64, width: f64, title: &str) {
    layer(s, "A-ELEV");
    text(s, [100.0, y - 60.0], title);
    for (dy, label) in [(0.0, "LEVEL 1 EL. 100'-0\""), (168.0, "LEVEL 2 EL. 114'-0\""), (312.0, "T.O. ROOF EL. 126'-0\"")] {
        line(s, [-60.0, y + dy], [width + 60.0, y + dy]);
        text(s, [width + 80.0, y + dy + 6.0], label);
    }
    rect(s, [0.0, y], [width, y + 312.0]);
    rect(s, [120.0, y], [360.0, y + 120.0]);
    for x in [180.0, 240.0, 300.0] {
        line(s, [x, y], [x, y + 120.0]);
    }
}

#[test]
fn north_east_and_west_glass_lands_on_its_own_face() {
    let mut s = drawing();
    let sides = [("North", -3000.0, 480.0), ("East", -4000.0, 360.0), ("West", -5000.0, 360.0)];
    for (side, y, width) in sides {
        elevation(&mut s, y, width, &format!("{} ELEVATION", side.to_ascii_uppercase()));
        let r = s.execute("elev.read", &json!({"region": [-100.0, y - 100.0, 700.0, y + 450.0], "apply": true})).unwrap();
        assert_eq!(r["facade"], json!(side), "{r}");
        let glass = r["bands"].as_array().unwrap().iter().find(|b| b["kind"] == "glass").cloned().unwrap();
        assert_eq!((glass["start_ft"].clone(), glass["end_ft"].clone()), (json!(10.0), json!(30.0)), "{side}: {r}");
    }
    // A facade given by name reads the same as one named by its title.
    let r = s.execute("elev.read", &json!({"region": [-100.0, -5100.0, 700.0, -4550.0], "facade": "west"})).unwrap();
    assert_eq!(r["facade"], json!("West"));
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["facadeGlass"], json!(3), "{r}");
    let on = |side: &str| -> Mesh {
        s.doc()
            .unwrap()
            .model
            .iter()
            .filter(|e| made_as(e).as_deref() == Some("facade-glass") && e.common.xdata.iter().any(|t| t.value == side))
            .find_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
            .unwrap()
    };
    // North, seen from the north: its left end is the east end, so 10'-30' is x 360 down to
    // 120, outside the y = 360 face.
    let n = on("North");
    assert_near(extent(&n, |v| v.x), (120.0, 360.0));
    assert_eq!(z(&n), (0.0, 120.0));
    assert!(n.vertices.iter().all(|v| v.y >= 360.0) && n.vertices.iter().any(|v| v.y > 360.0), "north glass outside the north face");
    // East: left end is the south end, so y 120 to 360, outside the x = 480 face.
    let e = on("East");
    assert_near(extent(&e, |v| v.y), (120.0, 360.0));
    assert_eq!(z(&e), (0.0, 120.0));
    assert!(e.vertices.iter().all(|v| v.x >= 480.0) && e.vertices.iter().any(|v| v.x > 480.0), "east glass outside the east face");
    // West: left end is the north end, so y 240 down to 0, outside the x = 0 face.
    let w = on("West");
    assert_near(extent(&w, |v| v.y), (0.0, 240.0));
    assert_eq!(z(&w), (0.0, 120.0));
    assert!(w.vertices.iter().all(|v| v.x <= 0.0) && w.vertices.iter().any(|v| v.x < 0.0), "west glass outside the west face");
}
