//! A framed roof (Q6314): the roof level's elevation is where its framing bears (a section's
//! truss bearing), the members span the roof's short way at their spacing and depth, the deck
//! sits on them, the outside walls carry up round them to the deck (or the parapet), and what
//! stands on the roof stands on the deck. A roof standing over its walls on nothing, or off
//! their outline, is a model check finding.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Mesh};
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

const ROOF_PLAN: [f64; 4] = [1900.0, -100.0, 3000.0, 1000.0];

/// Two storeys of a 40' × 30' box of 6" walls (Level 1 at 0, Level 2 at 10', 8' floor to
/// bearing), each plan on its own part of the sheet, and the roof plan beside them: the roof's
/// edge and a rooftop unit. The roof level at `roof_ft`, its plan's corner moved by `roof_base`
/// (none: where the stack puts it), the exterior's `framing` (bearing at `roof_ft`) and
/// `parapets` as given.
fn building(roof_ft: f64, roof_base: Option<[f64; 2]>, framing: Value, parapets: Value) -> Session {
    building_with_roof(roof_ft, roof_base, framing, parapets, [2000.0, 0.0], [2480.0, 360.0])
}

/// [`building`] with the roof plan's roof outline from `a` to `b`.
fn building_with_roof(roof_ft: f64, roof_base: Option<[f64; 2]>, framing: Value, parapets: Value, a: [f64; 2], b: [f64; 2]) -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    for y in [0.0, 1200.0] {
        rect(&mut s, [0.0, y], [480.0, y + 360.0]);
        rect(&mut s, [6.0, y + 6.0], [474.0, y + 354.0]);
    }
    layer(&mut s, "A-ROOF");
    rect(&mut s, a, b);
    rect(&mut s, [2100.0, 240.0], [2172.0, 288.0]);
    text(&mut s, [2105.0, 260.0], "RTU-1 H=4'-6\"");
    // A framed roof's level is its surface: over the members (24") and the deck (1.5") on them,
    // which bear at `roof_ft`.
    let mut framing = framing;
    let surface = match framing.as_array_mut().and_then(|a| a.first_mut()) {
        Some(f) => {
            f["bearing_ft"] = json!(roof_ft);
            roof_ft + 25.5 / 12.0
        }
        None => roof_ft,
    };
    let mut roof = json!({"name": "Roof", "elevation_ft": surface, "height_ft": 4, "ceiling_ft": 4, "region": [1900, -200, 3100, 1100]});
    if let Some(b) = roof_base {
        roof["base"] = json!(b);
    }
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "units_per_foot": 12,
            "stack": true,
            "levels": [
                {"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]},
                {"name": "Level 2", "elevation_ft": 10, "height_ft": roof_ft - 10.0, "ceiling_ft": 7.5, "region": [-100, 1100, 1000, 1900], "base": [0, 1200]},
                roof
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "exterior": {"framing": framing, "parapets": parapets}
        }}),
    )
    .unwrap();
    s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    s
}

fn trusses() -> Value {
    json!([{"level": "Roof", "member": "truss", "spacing_in": 24, "depth_in": 24, "spacing_status": "read", "depth_status": "read", "source": "S-1: ROOF TRUSSES @ 24\" OC"}])
}

fn made(s: &Session, part: &str) -> Vec<(Mesh, Vec<String>)> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter_map(|e| {
            if let EntityKind::Mesh(m) = &e.kind { Some((m.clone(), e.common.xdata.iter().map(|t| t.value.clone()).collect())) } else { None }
        })
        .collect()
}

fn z(m: &Mesh) -> (f64, f64) {
    m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)))
}

fn xy(m: &Mesh) -> (f64, f64, f64, f64) {
    m.vertices.iter().fold((f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |(a, b, c, d), v| {
        (a.min(v.x), b.min(v.y), c.max(v.x), d.max(v.y))
    })
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn roof_issues(s: &mut Session) -> Vec<String> {
    let r = s.execute("model.issues", &json!({"kind": "roof_off_walls"})).unwrap();
    r["issues"].as_array().unwrap().iter().map(|i| i["message"].as_str().unwrap_or_default().to_string()).collect()
}

#[test]
fn a_framed_roof_bears_on_the_walls_with_its_members_deck_and_roof_walls() {
    // The roof bears at 18' (the top of Level 2's walls): 24" trusses at 24" o.c.
    let mut s = building(18.0, None, trusses(), json!([]));
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["roofs"], json!(1), "{r}");
    // 40' long, 30' across: a truss every 2' along the length, each the 30' across.
    let members = made(&s, "roof-framing");
    assert_eq!(members.len(), 19, "{r}");
    for (m, x) in &members {
        assert_eq!(z(m), (216.0, 240.0), "bears at 18', 24\" deep");
        let (x0, y0, x1, y1) = xy(m);
        assert!(x1 - x0 < 4.0 && near(y1 - y0, 360.0), "{:?}", xy(m));
        assert!(x.iter().any(|t| t == "status:derived") && x.iter().any(|t| t.starts_with("basis:S-ROOF-FRAMING") && t.contains("S-1")), "{x:?}");
    }
    // The deck on them: 1.5" (the rule's deck and roofing), its top at 18' + 24" + 1.5".
    let roof = &made(&s, "roof")[0].0;
    assert_eq!(z(roof), (240.0, 241.5));
    // No parapet: the outside walls carry up round the framing to the deck, inferred by rule.
    let walls = made(&s, "roof-wall");
    assert_eq!(walls.len(), 1);
    assert_eq!(z(&walls[0].0), (216.0, 241.5));
    assert!(walls[0].1.iter().any(|t| t == "status:inferred"), "{:?}", walls[0].1);
    // The rooftop unit stands on the deck, 4'-6" high.
    let unit = &made(&s, "equipment")[0].0;
    assert_eq!(z(unit), (241.5, 241.5 + 54.0));
    // Stacked over the walls under it, on them: no finding.
    let w = made(&s, "wall").iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (m, _)| (lo.min(xy(m).0), hi.max(xy(m).2)));
    assert!(near(xy(roof).0, w.0) && near(xy(roof).2, w.1), "{:?} {w:?}", xy(roof));
    assert_eq!(roof_issues(&mut s), Vec::<String>::new());
}

#[test]
fn a_framed_roofs_parapet_rises_from_the_bearing_over_the_deck() {
    // A 3' parapet over the roof's surface: one wall from the bearing to 3' over the deck.
    let mut s = building(18.0, None, trusses(), json!([{"level": "Roof", "height_ft": 3}]));
    s.execute("model.build", &json!({})).unwrap();
    let p = made(&s, "parapet");
    assert_eq!(p.len(), 1);
    assert_eq!(z(&p[0].0), (216.0, 241.5 + 36.0));
    assert!(made(&s, "roof-wall").is_empty());
    assert_eq!(roof_issues(&mut s), Vec::<String>::new());
}

#[test]
fn a_roof_over_its_walls_on_nothing_or_off_them_is_a_finding() {
    // Unframed, a storey's default over the walls: the slab stands 1' over them on nothing.
    let mut s = building(20.0, None, json!([]), json!([]));
    let mut sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    for l in sp["levels"].as_array_mut().unwrap().iter_mut().filter(|l| l["name"] == "Level 2") {
        l["height_ft"] = json!(8);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    assert!(made(&s, "roof-framing").is_empty());
    let found = roof_issues(&mut s);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("over the top of Level 2's walls with nothing between them"), "{found:?}");
    // Framed but its plan registered 10' off along the building: off the walls.
    let mut s = building(18.0, Some([1880.0, 0.0]), trusses(), json!([]));
    s.execute("model.build", &json!({})).unwrap();
    let found = roof_issues(&mut s);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("off Level 2's walls") && found[0].contains("10'-0\""), "{found:?}");
}

#[test]
fn a_roof_plan_outline_short_of_the_storey_is_built_over_it_and_the_design_kept() {
    // The roof plan's outline is 30' long over the 40' building: a roof area, not its edge.
    let mut s = building_with_roof(18.0, None, trusses(), json!([]), [2060.0, 0.0], [2420.0, 360.0]);
    let r = s.execute("model.build", &json!({})).unwrap();
    let roof = &made(&s, "roof")[0].0;
    let (x0, _, x1, _) = xy(roof);
    assert!(near(x1 - x0, 480.0), "built over the whole storey: {:?} {r}", xy(roof));
    assert_eq!(made(&s, "roof-framing").len(), 19);
    assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap_or_default().contains("A-ROOF-COVERS-STOREY")), "{r}");
    assert_eq!(roof_issues(&mut s), Vec::<String>::new());
    // The plan's outline kept beside the built one.
    let p = s.execute("bim.provenance", &json!({"corrections": true, "part": "roof"})).unwrap();
    let listed = p["corrections"]["listed"].as_array().unwrap();
    assert!(listed.iter().any(|c| c["key"] == "outline" && c["design"] == "30'-0\" x 30'-0\"" && c["corrected"] == "40'-0\" x 30'-0\""), "{p}");
}

/// A roof plan has no rooms or doors (the build makes none from it): the model check reports no
/// leak for a label on it, while the same label on a storey's plan outside its walls is one.
#[test]
fn a_roof_plans_labels_are_no_leaking_rooms() {
    let mut s = building(18.0, None, trusses(), json!([]));
    let mut sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    sp["layers"].as_array_mut().unwrap().push(json!({"layer": "A-ROOM-TAG", "role": "room_tag"}));
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
    layer(&mut s, "A-ROOM-TAG");
    // On the roof plan, by the unit: a roof area's label.
    text(&mut s, [2300.0, 100.0], "STORAGE");
    // On Level 1's plan, outside its walls: no closed outline round it.
    text(&mut s, [700.0, 100.0], "STORAGE");
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("model.issues", &json!({"kind": "leak"})).unwrap();
    let levels: Vec<Value> = r["issues"].as_array().unwrap().iter().map(|i| i["level"].clone()).collect();
    assert_eq!(levels, [json!("Level 1")], "{r}");
}
