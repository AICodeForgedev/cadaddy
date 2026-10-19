//! The outside from its drawings: a synthetic south elevation gives the levels' heights, the
//! roof and parapet, window sills and heads, a storefront band and a brick band; a roof plan
//! gives a sloped roof with a parapet, a cricket and a rooftop unit; `model.build` builds them.

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
    let mut s = Session::new();
    // The ground floor plan: a 40' × 30' box of 6" walls.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [480.0, 360.0]);
    rect(&mut s, [6.0, 6.0], [474.0, 354.0]);
    // The south elevation: level lines with labels, the silhouette, windows on Level 2, a
    // storefront with mullions on Level 1, a brick base.
    layer(&mut s, "A-ELEV");
    text(&mut s, [200.0, G - 60.0], "SOUTH ELEVATION");
    for (dy, label) in
        [(0.0, "LEVEL 1 EL. 100'-0\""), (168.0, "LEVEL 2 EL. 114'-0\""), (312.0, "T.O. ROOF EL. 126'-0\""), (354.0, "T.O. PARAPET EL. 129'-6\"")]
    {
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
    text(&mut s, [2200.0, 320.0], "PARAPET 3'-6\"");
    line(&mut s, [2100.0, 180.0], [2400.0, 180.0]);
    line(&mut s, [2400.0, 180.0], [2388.0, 186.0]);
    line(&mut s, [2400.0, 180.0], [2388.0, 174.0]);
    text(&mut s, [2240.0, 190.0], "SLOPE 1/4\" / FT");
    poly(&mut s, &[[2440.0, 150.0], [2470.0, 180.0], [2440.0, 210.0], [2410.0, 180.0]]);
    text(&mut s, [2430.0, 178.0], "CRICKET");
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
    // The south walls are named brick.
    let brick = s.doc().unwrap().model.iter().filter(|e| e.common.xdata.iter().any(|t| t.value == "facade:South:BRICK")).count();
    assert!(brick >= 1);
}
