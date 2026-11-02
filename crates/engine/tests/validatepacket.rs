//! The evidence packet (Q6115) on a made-up print set: an architectural plan, a power plan with a
//! receptacle and a panel, a mechanical plan with an exhaust fan and a duct, a reflected ceiling
//! plan and a schedule sheet, side by side in one drawing; the model built on the architectural
//! plan. `validate.areas` lists the rooms (a long corridor in stretches) and `validate.packet`
//! writes the crops, the model pictures, the objects, the rules and the prompt.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Mesh, MeshFace, RawTag, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// The architectural plan's origin on the drawing (inches): the level's base.
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
/// Where each sheet starts along x.
const A101: f64 = 0.0;
const E101: f64 = 4000.0;
const M101: f64 = 8000.0;
const A121: f64 = 12000.0;
/// The schedule sheet sits above the A plan; the elevations and the partition legend beside it.
const E601_Y: f64 = 3000.0;
const A501: f64 = 4000.0;
const A601: f64 = 8000.0;
/// The enlarged plan sheet: its view prints the plan at twice the scale.
const A401: f64 = 16000.0;
/// The enlarged view's fit onto the plan: project = 0.5 · sheet + T.
const ENL_TX: f64 = OX - 0.5 * (A401 + 400.0);
const ENL_TY: f64 = OY - 0.5 * 400.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A point of the building (feet) on the sheet starting at `sx`.
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

/// A print symbol: a cross and a ring at a sheet point (drawing inches).
fn symbol(s: &mut Session, p: [f64; 2]) {
    s.execute("line", &json!({"points": [[p[0] - 12.0, p[1]], [p[0] + 12.0, p[1]]]})).unwrap();
    s.execute("line", &json!({"points": [[p[0], p[1] - 12.0], [p[0], p[1] + 12.0]]})).unwrap();
    s.execute("circle", &json!({"center": p, "radius": 6})).unwrap();
}

/// The building's walls on a sheet: a 100' × 12' shell of 6" walls, partitions at 15' and 30'.
fn walls(s: &mut Session, sx: f64) {
    rect(s, sx, [0.0, 0.0], [100.0, 12.0]);
    rect(s, sx, [0.5, 0.5], [99.5, 11.5]);
    line(s, sx, [15.0, 0.5], [15.0, 11.5]);
    line(s, sx, [30.0, 0.5], [30.0, 11.5]);
}

/// Block references at building points (feet) on the A plan, on `layer` with a TYPE attribute.
fn place(s: Session, block: &str, layer: &str, pts: &[[f64; 2]], ty: &str) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key(block) {
        let mut b = Block::new(block);
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert(block.into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for p in pts {
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
        let q = at(A101, *p);
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
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
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
            "PNL-A": {"ceiling": false, "aff_ft": 4.0},
            "EF-1": {"ceiling": true}
        }},
        "sheets": [
            sheet("A-101", "FLOOR PLAN - LEVEL 1", "A", Some("L1"), A101, 0.0, Some(0.0)),
            sheet("E-101", "POWER PLAN - LEVEL 1", "E", Some("L1"), E101, 0.0, Some(-E101)),
            sheet("M-101", "MECHANICAL PLAN - LEVEL 1", "M", Some("L1"), M101, 0.0, Some(-M101)),
            sheet("A-121", "REFLECTED CEILING PLAN - LEVEL 1", "A", Some("L1"), A121, 0.0, Some(-A121)),
            sheet("E-601", "ELECTRICAL SCHEDULES", "E", None, 0.0, E601_Y, None),
            sheet("A-501", "INTERIOR ELEVATIONS", "A", None, A501, E601_Y, None),
            sheet("A-601", "PARTITION TYPES", "A", None, A601, E601_Y, None),
            sheet("A-401", "ENLARGED PLANS", "A", None, A401, 0.0, None),
            {"number": "A-401", "title": "ENLARGED PLANS", "discipline": "A", "level": "L1", "source": "model",
             "rect": [A401, 0.0, A401 + SW, SH],
             "view": {"title": "ENLARGED PLAN - OFFICE", "rect": [A401 + 200.0, 200.0, A401 + 2400.0, 1400.0]},
             "registration": {"reference": "A-101", "method": "view_fit", "a": 0.5, "b": 0.0, "tx": ENL_TX, "ty": ENL_TY, "matched": 0}}
        ]
    })
}

/// Where the print symbols are, drawing inches.
fn recep_on_e() -> [f64; 2] {
    at(E101, [6.0, 0.5])
}
fn fan_on_m() -> [f64; 2] {
    at(M101, [10.0, 8.0])
}
/// A building point (feet) on the enlarged view (twice the scale).
fn on_enlarged(p: [f64; 2]) -> [f64; 2] {
    [(OX + p[0] * 12.0 - ENL_TX) / 0.5, (OY + p[1] * 12.0 - ENL_TY) / 0.5]
}

struct Job {
    s: Session,
    recep: String,
    panel: String,
    fan: String,
    built: String,
}

fn job() -> Job {
    let mut s = Session::new();
    // The architectural plan, with the room tags; the backgrounds of the other sheets.
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
    // Sheet frames.
    layer(&mut s, "G-FRAME");
    for (x, y) in [(A101, 0.0), (E101, 0.0), (M101, 0.0), (A121, 0.0), (A401, 0.0), (0.0, E601_Y), (A501, E601_Y), (A601, E601_Y)] {
        s.execute("pline", &json!({"vertices": [[x, y], [x + SW, y], [x + SW, y + SH], [x, y + SH]], "closed": true})).unwrap();
    }
    // What the trades' prints show.
    layer(&mut s, "E-SYM");
    symbol(&mut s, recep_on_e());
    symbol(&mut s, at(E101, [1.0, 6.0]));
    text(&mut s, [E101 + 2800.0, 2000.0], "GENERAL NOTES");
    text(&mut s, [E101 + 2800.0, 1980.0], "1. MOUNT RECEPTACLES AT 18 IN AFF.");
    layer(&mut s, "M-SYM");
    symbol(&mut s, fan_on_m());
    line(&mut s, M101, [10.0, 8.0], [10.0, 11.5]);
    layer(&mut s, "E-LITE-SYM");
    symbol(&mut s, at(A121, [7.0, 6.0]));
    // The enlarged plan of the office (walls and the receptacle at twice the scale), the room's
    // interior elevation title, the partition legend's exterior wall type.
    layer(&mut s, "A-ENLG");
    for (a, b) in [([0.5, 0.5], [15.0, 0.5]), ([15.0, 0.5], [15.0, 11.5]), ([15.0, 11.5], [0.5, 11.5]), ([0.5, 11.5], [0.5, 0.5])] {
        s.execute("line", &json!({"points": [on_enlarged(a), on_enlarged(b)]})).unwrap();
    }
    symbol(&mut s, on_enlarged([6.0, 0.5]));
    text(&mut s, [A501 + 1000.0, E601_Y + 600.0], "OFFICE 101 - NORTH");
    text(&mut s, [A601 + 1500.0, E601_Y + 1200.0], "EXTERIOR");
    layer(&mut s, "E-SCHED");
    text(&mut s, [200.0, E601_Y + 1800.0], "PANEL SCHEDULE");
    text(&mut s, [200.0, E601_Y + 1700.0], "PNL-A");
    text(&mut s, [600.0, E601_Y + 1700.0], "120/208V 3PH 4W SURFACE");
    // The model on the A plan: a receptacle, a panel, an exhaust fan and its duct.
    let (s, recep) = place(s, "RECEP", "E-POWR", &[[6.0, 0.5]], "DUPLEX");
    let (s, panel) = place(s, "PANEL", "E-POWR", &[[0.5, 6.0]], "PNL-A");
    let (mut s, fan) = place(s, "FAN", "M-EQPM", &[[10.0, 8.0]], "EF-1");
    layer(&mut s, "M-DUCT");
    line(&mut s, A101, [10.0, 8.0], [10.0, 11.5]);
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s.execute(
        "bim.set_type",
        &json!({"mark": "DUPLEX", "category": "wiring device", "fields": {"description": "duplex receptacle", "catalogNumber": "REC-20"}}),
    )
    .unwrap();
    let (recep, panel, fan) = (recep[0].clone(), panel[0].clone(), fan[0].clone());
    for (h, kind, src) in
        [(&recep, "receptacle", "E-101 at 6',0.5'"), (&panel, "panelboard", "E-101 at 0.5',6'"), (&fan, "exhaust_fan", "M-101 at 10',8'")]
    {
        s.execute("bim.source", &json!({"handles": [h], "status": "fixed", "source": src, "kind": kind})).unwrap();
    }
    // The duct is typed as one; a duct piece the build made (in the office's ceiling space).
    let mut d = (*s.doc().unwrap()).clone();
    let duct = d.model.iter().find(|e| e.common.layer == "M-DUCT" && matches!(e.kind, EntityKind::Line(_))).map(|e| e.handle).unwrap();
    d.modify_entity(duct, |e| e.common.xdata.extend(cadaddy_engine::doc::bimspec::type_mark_xdata("D1"))).unwrap();
    let (x0, y0) = (OX + 3.0 * 12.0, OY + 9.0 * 12.0);
    let (z0, z1) = (8.5 * 12.0, 9.5 * 12.0);
    let vertices = [(0.0, 0.0), (12.0, 0.0), (12.0, 12.0), (0.0, 12.0)]
        .iter()
        .flat_map(|(dx, dy)| [Vec3::new(x0 + dx, y0 + dy, z0), Vec3::new(x0 + dx, y0 + dy, z1)])
        .collect::<Vec<_>>();
    let faces = vec![MeshFace { v: vec![0, 2, 4, 6], hidden: 0 }, MeshFace { v: vec![1, 3, 5, 7], hidden: 0 }];
    let h = d.new_handle();
    let mut piece = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, kind: Default::default() }));
    let tag = |v: &str| RawTag { code: 1000, value: v.into() };
    piece.common.xdata =
        vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }, tag("mep"), tag("L1"), tag("0"), tag("kind:duct"), tag("part:segment")].into();
    d.model.push(piece);
    let built = h.hex();
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Job", None);
    s.execute("bim.set_type", &json!({"mark": "D1", "category": "duct", "fields": {"size": "10x8"}})).unwrap();
    Job { s, recep, panel, fan, built }
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-vpacket-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn areas(s: &mut Session) -> Vec<Value> {
    let r = s.execute("validate.areas", &json!({})).unwrap();
    r["areas"].as_array().cloned().unwrap_or_default()
}

fn office(s: &mut Session) -> String {
    let all = areas(s);
    let a =
        all.iter().find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("office"))).unwrap_or_else(|| panic!("no office in {all:?}"));
    a["id"].as_str().unwrap().to_string()
}

#[test]
fn areas_list_the_rooms_with_stable_ids_and_split_the_corridor() {
    let mut j = job();
    let all = areas(&mut j.s);
    let names: Vec<String> = all.iter().filter_map(|a| a["name"].as_str().map(str::to_ascii_lowercase)).collect();
    assert!(names.contains(&"office".to_string()) && names.contains(&"storage".to_string()), "{all:?}");
    let office = all.iter().find(|a| a["name"] == "Office" || a["name"] == "OFFICE").unwrap();
    assert_eq!(office["kind"], "room");
    assert_eq!(office["level"], "L1");
    let b: Vec<f64> = office["box"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!(
        (b[0] - 0.5).abs() < 0.6 && (b[2] - 15.0).abs() < 0.6 && (b[1] - 0.5).abs() < 0.6 && (b[3] - 11.5).abs() < 0.6,
        "the office in building feet: {b:?}"
    );
    // The 69' corridor comes in stretches of at most 40'.
    let stretches: Vec<&Value> = all.iter().filter(|a| a["kind"] == "corridor").collect();
    assert_eq!(stretches.len(), 2, "{all:?}");
    for st in &stretches {
        let b: Vec<f64> = st["box"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!(b[2] - b[0] <= 40.0 + 1e-6, "{st}");
        assert!(st["id"].as_str().unwrap().contains('#'), "{st}");
    }
    // Stable: the same ids again; filters; a custom window.
    let again: Vec<Value> = areas(&mut j.s).iter().map(|a| a["id"].clone()).collect();
    assert_eq!(again, all.iter().map(|a| a["id"].clone()).collect::<Vec<_>>());
    let r = j.s.execute("validate.areas", &json!({"kind": "corridor"})).unwrap();
    assert_eq!(r["total"], 2);
    let r = j.s.execute("validate.areas", &json!({"area": [40, 0, 50, 12], "level": "L1", "kind": "zone"})).unwrap();
    assert_eq!(r["total"], 1, "{r}");
    assert_eq!(r["areas"][0]["id"], "L1/win:40.00,0.00,50.00,12.00");
    let r = j.s.execute("validate.areas", &json!({"limit": 1})).unwrap();
    assert_eq!(r["areas"].as_array().map(Vec::len), Some(1));
    assert_eq!(r["next"], 1);
    assert!(j.s.execute("validate.areas", &json!({"kind": "kitchen"})).is_err());
    assert!(j.s.execute("validate.areas", &json!({"level": "L9"})).is_err());
}

/// A crop of the manifest by sheet and kind.
fn crop<'a>(m: &'a Value, sheet: &str, kind: &str) -> &'a Value {
    m["prints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["sheet"] == sheet && c["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} crop of {sheet} in {m:#}"))
}

/// Where a sheet point is in a crop, pixels.
fn pixel_of(c: &Value, p: [f64; 2]) -> (f64, f64) {
    let r: Vec<f64> = c["sheet_rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let s = c["px_per_unit"].as_f64().unwrap();
    ((p[0] - r[0]) * s, (r[3] - p[1]) * s)
}

/// The darkest pixel within `reach` of (x, y).
fn darkest(img: &image::RgbaImage, x: f64, y: f64, reach: i64) -> u32 {
    let mut best = u32::MAX;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let (px, py) = (x.round() as i64 + dx, y.round() as i64 + dy);
            if px < 0 || py < 0 || px >= i64::from(img.width()) || py >= i64::from(img.height()) {
                continue;
            }
            let c = img.get_pixel(px as u32, py as u32);
            best = best.min(u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2]));
        }
    }
    best
}

#[test]
fn a_packet_holds_the_crops_the_model_the_objects_and_the_rules() {
    let mut j = job();
    let id = office(&mut j.s);
    let out = dir("office");
    let r = j.s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy()})).unwrap();
    let m = read_json(&out.join("manifest.json"));
    assert_eq!(r["area"], m["area"], "the answer is the manifest");
    assert!(r["ms"].as_u64().is_some_and(|ms| ms < 10_000), "one area in seconds: {}", r["ms"]);
    // Every plan of the level, the RCP, the schedule row of a mark present, the general notes.
    let a = crop(&m, "A-101", "plan");
    let e = crop(&m, "E-101", "plan");
    let mech = crop(&m, "M-101", "plan");
    let rcp = crop(&m, "A-121", "rcp");
    assert_eq!(a["placed_by"], "reference");
    assert_eq!(e["placed_by"], "registration");
    assert_eq!(rcp["trade"], "architecture");
    let sched = crop(&m, "E-601", "schedule");
    assert_eq!(sched["shows"], "mark PNL-A", "{sched}");
    let notes = crop(&m, "E-101", "general_notes");
    assert_eq!(notes["placed_by"], "text");
    let elev = crop(&m, "A-501", "interior_elevation");
    assert_eq!(elev["shows"], "room 101", "{elev}");
    assert_eq!(m["prints"].as_array().unwrap().iter().filter(|c| c["sheet"] == "A-501").count(), 1, "one crop for one title");
    let legend = crop(&m, "A-601", "partition_legend");
    assert_eq!(legend["shows"], "wall type Exterior", "{legend}");
    // The enlarged plan: the view the job placed, at its own scale.
    let enl = crop(&m, "A-401", "enlarged_plan");
    assert_eq!(enl["placed_by"], "view_placement");
    assert_eq!(enl["view"], "ENLARGED PLAN - OFFICE");
    let img = image::open(out.join(enl["file"].as_str().unwrap())).unwrap().to_rgba8();
    let (x, y) = pixel_of(enl, on_enlarged([6.0, 0.5]));
    assert!(darkest(&img, x, y, 2) < 300, "the enlarged view's receptacle should be at ({x:.1}, {y:.1})");
    let enl_ov = m["model"].as_array().unwrap().iter().find(|x| x["of"] == enl["file"]).unwrap();
    assert_eq!(enl_ov["kind"], "overlay");
    // At least 150 dpi of the printed sheet (a 36" × 24" sheet).
    for c in m["prints"].as_array().unwrap() {
        assert!(c["dpi"].as_f64().unwrap() >= 150.0 - 1e-6, "{c}");
        assert_eq!(c["paper"], "ARCH D 24x36");
    }
    // The crop maps the area: the office's box ± 5' carried onto each sheet.
    let office_box = m["area"]["box"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
    for (c, sx) in [(a, A101), (e, E101), (mech, M101), (rcp, A121)] {
        let r: Vec<f64> = c["sheet_rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let want_x0 = sx + OX + (office_box[0] - 5.0) * 12.0;
        let want_y1 = OY + (office_box[3] + 5.0) * 12.0;
        assert!((r[0] - want_x0).abs() < 0.01 && (r[3] - want_y1).abs() < 0.01, "{} {r:?} vs {want_x0} {want_y1}", c["sheet"]);
        let want_w = (office_box[2] - office_box[0] + 10.0) * 12.0;
        assert!((r[2] - r[0]) >= want_w - 0.01 && (r[2] - r[0]) <= want_w + 1.0, "{r:?}");
    }
    // A symbol at a known sheet point lands where the crop says (± 2 px), and the crop is blank
    // where the print has nothing.
    for (c, p) in [(e, recep_on_e()), (mech, fan_on_m())] {
        let img = image::open(out.join(c["file"].as_str().unwrap())).unwrap().to_rgba8();
        let px = c["px"].as_array().unwrap();
        assert_eq!((u64::from(img.width()), u64::from(img.height())), (px[0].as_u64().unwrap(), px[1].as_u64().unwrap()));
        let (x, y) = pixel_of(c, p);
        assert!(darkest(&img, x, y, 2) < 300, "{}: the symbol at {p:?} should be at ({x:.1}, {y:.1})", c["file"]);
        let (bx, by) = pixel_of(c, [p[0] + 30.0, p[1] + 40.0]);
        assert!(darkest(&img, bx, by, 1) > 700, "{}: nothing is drawn at ({bx:.1}, {by:.1})", c["file"]);
    }
    // The model: an overlay per plan crop, the plan alone, 3D and wall elevations.
    let model = m["model"].as_array().unwrap();
    for f in ["model/plan.png", "model/3d.png"] {
        assert!(model.iter().any(|x| x["file"] == f), "{f} in {model:?}");
        assert!(out.join(f).is_file(), "{f}");
    }
    assert!(model.iter().any(|x| x["kind"] == "overlay" && x["of"] == e["file"]));
    assert!(model.iter().any(|x| x["kind"] == "elevation"), "{model:?}");
    for x in model {
        assert!(out.join(x["file"].as_str().unwrap()).is_file(), "{x}");
    }
    // The overlay draws the model's receptacle (red) where the print has it.
    let ov = model.iter().find(|x| x["kind"] == "overlay" && x["of"] == e["file"]).unwrap();
    let img = image::open(out.join(ov["file"].as_str().unwrap())).unwrap().to_rgba8();
    let (x, y) = pixel_of(e, recep_on_e());
    let mut red = false;
    for dy in -12i64..=12 {
        for dx in -12i64..=12 {
            let (px, py) = ((x.round() as i64 + dx) as u32, (y.round() as i64 + dy) as u32);
            if px < img.width() && py < img.height() {
                let c = img.get_pixel(px, py);
                red |= c[0] > 180 && c[1] < 90 && c[2] < 90;
            }
        }
    }
    assert!(red, "the model's receptacle is drawn in red over the print");
    // objects.json: the placed elements with their source.
    let o = read_json(&out.join("objects.json"));
    let els = o["elements"].as_array().unwrap();
    let by =
        |h: &str| els.iter().find(|x| x["handle"].as_str().is_some_and(|y| y.eq_ignore_ascii_case(h))).unwrap_or_else(|| panic!("{h} in {els:?}"));
    let rc = by(&j.recep);
    assert_eq!(rc["kind"], "receptacle");
    assert_eq!(rc["trade"], "electrical");
    assert_eq!(rc["mark"], "DUPLEX");
    assert_eq!(rc["catalog"], "REC-20");
    assert_eq!(rc["source"]["note"], "E-101 at 6',0.5'");
    assert_eq!(rc["mounting"], "wall");
    assert_eq!(rc["room"], "101");
    assert!(rc["host_wall"]["wall"].is_string(), "{rc}");
    assert_eq!(rc["host_wall"]["faces_area"], true, "{rc}");
    let pn = by(&j.panel);
    assert_eq!(pn["kind"], "panelboard");
    assert_eq!(pn["source"]["note"], "E-101 at 0.5',6'");
    let fan = by(&j.fan);
    assert_eq!(fan["kind"], "exhaust_fan");
    assert_eq!(fan["trade"], "mechanical");
    assert_eq!(fan["mounting"], "ceiling");
    assert_eq!(fan["source"]["note"], "M-101 at 10',8'");
    let pos: Vec<f64> = fan["at_ft"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!((pos[0] - 10.0).abs() < 0.01 && (pos[1] - 8.0).abs() < 0.01, "{pos:?}");
    let runs = o["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "the duct: {runs:?}");
    assert_eq!(runs[0]["kind"], "duct");
    assert_eq!(runs[0]["mark"], "D1");
    assert_eq!(runs[0]["size"], "10x8");
    assert_eq!(runs[0]["trade"], "mechanical");
    // The duct piece the build made: listed with its place and height in the ceiling space.
    let piece = by(&j.built);
    assert_eq!(piece["built"], true);
    assert_eq!(piece["kind"], "duct");
    assert_eq!(piece["height_aff_in"].as_f64(), Some(102.0), "{piece}");
    assert_eq!(piece["top_aff_in"].as_f64(), Some(114.0), "{piece}");
    assert!(!o["walls"].as_array().unwrap().is_empty());
    // rules.json: the rules of the kinds present, with the general ones; not the plumbing ones.
    let rl = read_json(&out.join("rules.json"));
    let ids: Vec<&str> = rl["rules"].as_array().unwrap().iter().filter_map(|r| r["id"].as_str()).collect();
    for want in ["G-PRESENT", "G-HOST", "E-RECEPT-POS", "E-PANEL-POS", "E-PANEL-MOUNT", "M-EF", "M-DUCT-ROUTE", "M-DUCT-SIZE", "M-DUCT-HEIGHT"] {
        assert!(ids.contains(&want), "{want} in {ids:?}");
    }
    assert!(!ids.contains(&"P-FIXTURE") && !ids.contains(&"F-HEAD"), "{ids:?}");
    // prompt.md: from the rules, naming the pictures, the findings shape, UNSURE.
    let pr = std::fs::read_to_string(out.join("prompt.md")).unwrap();
    assert!(pr.contains("### E-RECEPT-POS") && pr.contains("### M-EF"), "{pr}");
    assert!(pr.contains(e["file"].as_str().unwrap()));
    assert!(pr.contains("\"verdict\": \"pass|fail|missing|extra|unsure\""));
    assert!(pr.contains("UNSURE"));
    assert_eq!(m["counts"]["rules"].as_u64(), Some(ids.len() as u64));
    // VALIDATEPACKET_KEEP=1 keeps the packet to look at.
    if std::env::var_os("VALIDATEPACKET_KEEP").is_some() {
        println!("packet kept in {}", out.display());
    } else {
        let _ = std::fs::remove_dir_all(&out);
    }
}

#[test]
fn a_packet_is_deterministic_bounded_and_refuses_to_overwrite() {
    let mut j = job();
    let id = office(&mut j.s);
    let (d1, d2) = (dir("det1"), dir("det2"));
    j.s.execute("validate.packet", &json!({"area": id, "out_dir": d1.to_string_lossy()})).unwrap();
    j.s.execute("validate.packet", &json!({"area": id, "out_dir": d2.to_string_lossy()})).unwrap();
    for f in ["manifest.json", "objects.json", "rules.json", "prompt.md"] {
        assert_eq!(std::fs::read(d1.join(f)).unwrap(), std::fs::read(d2.join(f)).unwrap(), "{f} is byte-identical");
    }
    // Once written, a second packet into the same folder needs force.
    let err = j.s.execute("validate.packet", &json!({"area": id, "out_dir": d1.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("force"), "{err}");
    j.s.execute("validate.packet", &json!({"area": id, "out_dir": d1.to_string_lossy(), "force": true})).unwrap();
    // A cap on the pictures, with what was left out said.
    let d3 = dir("cap");
    let r = j.s.execute("validate.packet", &json!({"area": id, "out_dir": d3.to_string_lossy(), "max_images": 3})).unwrap();
    let m = read_json(&d3.join("manifest.json"));
    assert_eq!(m["counts"]["pictures"], 3, "{r}");
    let left: Vec<&Value> =
        m["left_out"].as_array().unwrap().iter().filter(|x| x["why"].as_str().is_some_and(|w| w.contains("max_images"))).collect();
    assert!(!left.is_empty(), "{m:#}");
    // Only one trade: the architectural background stays, the mechanical plan goes.
    let d4 = dir("trade");
    j.s.execute("validate.packet", &json!({"area": id, "out_dir": d4.to_string_lossy(), "trades": ["electrical"]})).unwrap();
    let m = read_json(&d4.join("manifest.json"));
    let sheets: Vec<&str> = m["prints"].as_array().unwrap().iter().filter_map(|c| c["sheet"].as_str()).collect();
    assert!(sheets.contains(&"E-101") && sheets.contains(&"A-101") && !sheets.contains(&"M-101"), "{sheets:?}");
    // A custom window works the same.
    let d5 = dir("window");
    let r = j.s.execute("validate.packet", &json!({"area": {"area": [40, 0, 50, 12], "level": "L1"}, "out_dir": d5.to_string_lossy()})).unwrap();
    assert_eq!(r["area"]["kind"], "zone");
    for d in [d1, d2, d3, d4, d5] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn bad_packet_calls_are_refused_and_never_panic() {
    let mut j = job();
    let id = office(&mut j.s);
    let out = dir("bad");
    let o = out.to_string_lossy().to_string();
    for (p, why) in [
        (json!({"area": id}), "out_dir"),
        (json!({"area": "L1/999", "out_dir": o}), "no area"),
        (json!({"area": id, "out_dir": o, "trades": ["carpentry"]}), "unknown trade"),
        (json!({"area": id, "out_dir": o, "dpi": 72}), "dpi"),
        (json!({"area": id, "out_dir": o, "max_images": 0}), "max_images"),
        (json!({"area": {"area": [1, 1, 1, 5]}, "out_dir": o}), "area"),
        (json!({"area": 7, "out_dir": o}), "area"),
    ] {
        let e = j.s.execute("validate.packet", &p).unwrap_err().to_string();
        assert!(e.contains(why), "{p}: {e}");
    }
    assert!(!out.join("manifest.json").exists(), "nothing written by a refused call");
    for v in [json!(null), json!([]), json!("x"), json!(1e308), json!({"area": [f64::MAX, 0, 1, 1]}), json!(-1)] {
        for key in ["area", "out_dir", "trades", "margin_ft", "dpi", "max_images", "level", "kind", "cursor", "limit"] {
            let _ = j.s.execute("validate.packet", &json!({key: v.clone()}));
            let _ = j.s.execute("validate.areas", &json!({key: v.clone()}));
        }
    }
    // A drawing with no rooms says how to get some.
    let mut s = Session::new();
    let r = s.execute("validate.areas", &json!({})).unwrap();
    assert_eq!(r["total"], 0);
    assert!(r["note"].as_str().is_some_and(|n| n.contains("model.build")), "{r}");
    assert!(s.execute("validate.packet", &json!({"area": "L1/101", "out_dir": o})).is_err());
}

// ---------------------------------------------------------------- the sheets the elements cite (Q6134)

/// More sheets, side by side past the enlarged plan: a fire alarm sheet and a low-voltage sheet
/// no one placed on the plan, a lighting plan and a power plan titled with the level (no level
/// tag, not placed), a stair enlargement titled with the level, and the power plan listed twice.
const E102: f64 = 20000.0;
const E103: f64 = 24000.0;
const E104: f64 = 28000.0;
const E105: f64 = 32000.0;
const A402: f64 = 36000.0;

/// Where the fire alarm sheet shows the receptacle's symbol, and where the lighting plan tags
/// the office.
fn on_e102() -> [f64; 2] {
    [E102 + 1500.0, 1200.0]
}
fn office_tag_on_e104() -> [f64; 2] {
    [E104 + 1000.0, 1000.0]
}

fn more_sheets(s: Session) -> Session {
    let mut s = s;
    layer(&mut s, "G-MORE");
    symbol(&mut s, on_e102());
    text(&mut s, [E102 + 200.0, 200.0], "FIRE ALARM DEVICES");
    line(&mut s, E103, [0.0, 0.0], [100.0, 12.0]);
    text(&mut s, office_tag_on_e104(), "OFFICE");
    text(&mut s, [office_tag_on_e104()[0], office_tag_on_e104()[1] - 20.0], "101");
    line(&mut s, E105, [0.0, 0.0], [100.0, 0.0]);
    line(&mut s, A402 - OX + 200.0, [0.0, 0.0], [10.0, 10.0]);
    let mut sp = spec();
    let sheets = sp["sheets"].as_array_mut().unwrap();
    let dup = sheets.iter().find(|x| x["number"] == "E-101").unwrap().clone();
    sheets.push(dup);
    sheets.push(sheet("E-102", "FIRE ALARM DEVICES", "E", None, E102, 0.0, None));
    sheets.push(sheet("E-103", "LOW VOLTAGE DEVICES", "E", None, E103, 0.0, None));
    sheets.push(sheet("E-104", "LIGHTING PLAN - LEVEL 1", "E", None, E104, 0.0, None));
    sheets.push(sheet("E-105", "POWER PLAN - FIRST FLOOR", "E", None, E105, 0.0, None));
    // Listed twice (two links of one view): shown once, even in tiles.
    sheets.push(sheet("E-105", "POWER PLAN - FIRST FLOOR", "E", None, E105, 0.0, None));
    sheets.push(json!({"number": "A-402", "title": "STAIR DETAILS", "discipline": "A", "source": "model", "rect": [A402, 0.0, A402 + SW, SH],
        "view": {"title": "ENLARGED PLAN - STAIR - LEVEL 1", "rect": [A402 + 200.0, 200.0, A402 + 700.0, 600.0]}}));
    s.execute("bspec.set", &json!({"spec": sp, "replace": true})).unwrap();
    s
}

/// Records on entities (by handle) as a job would leave them: `key` = value from `source`.
fn record(s: Session, sets: &[(String, &str, &str, cadaddy_engine::doc::props::Source)]) -> Session {
    use cadaddy_engine::doc::props::{self, Prop};
    let mut d = (*s.doc().unwrap()).clone();
    for (h, key, value, src) in sets {
        let h = cadaddy_engine::doc::Handle::parse_hex(h).unwrap();
        d.modify_entity(h, |e| {
            let mut store = props::read(&e.common.xdata);
            store.set(key, Prop::new(value.to_string(), src.clone()));
            e.common.xdata = props::write(&e.common.xdata, &store).into();
        })
        .unwrap();
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

fn print_source(doc: &str, at: [f64; 2]) -> cadaddy_engine::doc::props::Source {
    use cadaddy_engine::doc::props::{Source, SourceKind};
    Source { kind: SourceKind::Print, doc: Some(doc.into()), at: Some(at) }
}

fn rect_of(c: &Value) -> Vec<f64> {
    c["sheet_rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
}

#[test]
fn every_sheet_an_element_was_read_from_and_every_sheet_of_the_level_is_offered() {
    let j = job();
    let s = more_sheets(j.s);
    // The receptacle was read from the fire alarm sheet (at its symbol there); the panel from the
    // low-voltage sheet, at a point of the plan (not on that sheet), as a job may keep it.
    let mut s = record(
        s,
        &[
            (j.recep.clone(), "symbol", "receptacle", print_source("E-102/1", on_e102())),
            (j.panel.clone(), "symbol", "panel", print_source("E-103/1", [OX + 6.0, OY + 72.0])),
        ],
    );
    let id = office(&mut s);
    let out = dir("cited");
    s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "max_images": 200})).unwrap();
    let m = read_json(&out.join("manifest.json"));
    let prints = m["prints"].as_array().unwrap();
    let of = |sheet: &str| prints.iter().filter(|c| c["sheet"] == sheet).collect::<Vec<_>>();
    let left = m["left_out"].as_array().unwrap();
    let says = |what: &str| left.iter().find(|x| x["what"].as_str().is_some_and(|w| w.contains(what)));

    // An element sourced from an unregistered sheet: cropped round its source point, and the
    // symbol there is in the picture.
    let fa = of("E-102");
    assert_eq!(fa.len(), 1, "{fa:?}");
    assert_eq!(fa[0]["placed_by"], "source_point", "{}", fa[0]);
    assert_eq!(fa[0]["cited"], true);
    assert_eq!(fa[0]["unregistered"], true);
    assert!(fa[0]["shows"].as_str().unwrap().contains("1 element(s) here were read from it"), "{}", fa[0]);
    let img = image::open(out.join(fa[0]["file"].as_str().unwrap())).unwrap().to_rgba8();
    let (x, y) = pixel_of(fa[0], on_e102());
    assert!(darkest(&img, x, y, 2) < 300, "the symbol the receptacle was read from at ({x:.1}, {y:.1})");
    // Its source point is not on the sheet and nothing on it names the room: the whole sheet, in
    // at most 2 × 2 tiles, marked unregistered.
    let lv = of("E-103");
    assert!(!lv.is_empty() && lv.len() <= 4, "{lv:?}");
    for c in &lv {
        assert_eq!(c["placed_by"], "unregistered", "{c}");
        assert_eq!(c["cited"], true);
        assert!(c["tile"].is_array(), "{c}");
    }
    // A sheet titled with the level (no level tag, not placed): round the room's tag on it.
    let lt = of("E-104");
    assert_eq!(lt.len(), 1, "{lt:?}");
    assert_eq!(lt[0]["placed_by"], "room_tag", "{}", lt[0]);
    assert!(lt[0]["shows"].as_str().unwrap().contains("101"), "{}", lt[0]);
    assert!(lt[0].get("cited").is_none());
    let r = rect_of(lt[0]);
    let t = office_tag_on_e104();
    assert!(t[0] > r[0] && t[0] < r[2] && t[1] > r[1] && t[1] < r[3], "the tag in the crop: {r:?}");
    // A level-titled sheet that names nothing of the room is still offered whole (it is the
    // level's plan); a partial view of the level that names neither is left out, saying why.
    let pw = of("E-105");
    assert!(!pw.is_empty() && pw.len() <= 4 && pw.iter().all(|c| c["placed_by"] == "unregistered"), "{pw:?}");
    assert!(of("A-402").is_empty());
    let stair = says("A-402").unwrap_or_else(|| panic!("A-402 in {left:?}"));
    assert!(stair["why"].as_str().unwrap().contains("partial view"), "{stair}");
    // None of the level's sheets is left out as merely unplaced.
    assert!(!left.iter().any(|x| x["why"].as_str().is_some_and(|w| w.contains("not placed on its plan yet"))), "{left:?}");
    // No two pictures of one view: the power plan listed twice gives one crop.
    assert_eq!(of("E-101").iter().filter(|c| c["kind"] == "plan").count(), 1);
    for sh in ["E-101", "E-105"] {
        assert!(left.iter().any(|x| x["why"].as_str().is_some_and(|w| w.contains(&format!("the same view of sheet {sh}")))), "{sh}: {left:?}");
    }
    // No sliver: every crop that is not a tile is at least 320 px a side.
    for c in prints.iter().filter(|c| c["tile"].is_null()) {
        let px = c["px"].as_array().unwrap();
        assert!(px.iter().all(|v| v.as_u64().unwrap() >= 320), "{c}");
    }
    // The prompt says which pictures are not placed on the plan.
    let pr = std::fs::read_to_string(out.join("prompt.md")).unwrap();
    assert!(pr.contains("NOT placed on the plan"), "{pr}");
    // The model's wall elevations: one per side direction, each from inside the office.
    let els: Vec<&Value> = m["model"].as_array().unwrap().iter().filter(|x| x["kind"] == "elevation").collect();
    assert_eq!(els.len(), 4, "{els:?}");
    let sides: std::collections::BTreeSet<&str> = els.iter().filter_map(|e| e["what"].as_str()?.split_whitespace().nth(1)).collect();
    assert_eq!(sides.len(), 4, "{els:?}");
    assert!(els.iter().all(|e| e["what"].as_str().unwrap().contains("seen from inside it")), "{els:?}");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_tight_cap_cuts_the_unread_whole_sheets_first_and_says_so() {
    let j = job();
    let mut s = more_sheets(j.s);
    let id = office(&mut s);
    let all = dir("capall");
    s.execute("validate.packet", &json!({"area": id, "out_dir": all.to_string_lossy(), "max_images": 200})).unwrap();
    let full = read_json(&all.join("manifest.json"));
    let n = full["counts"]["pictures"].as_u64().unwrap() as usize;
    let whole: Vec<String> = full["prints"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["placed_by"] == "unregistered" && c.get("cited").is_none())
        .map(|c| c["file"].as_str().unwrap().to_string())
        .collect();
    assert!(!whole.is_empty(), "{full:#}");
    // Room for all but those: they go, the model's pictures stay, and each is listed.
    let out = dir("captight");
    s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "max_images": n - whole.len()})).unwrap();
    let m = read_json(&out.join("manifest.json"));
    let files: Vec<&str> = m["prints"].as_array().unwrap().iter().chain(m["model"].as_array().unwrap()).filter_map(|c| c["file"].as_str()).collect();
    assert!(whole.iter().all(|w| !files.contains(&w.as_str())), "{files:?}");
    assert!(files.iter().any(|f| f.starts_with("model/elevation_")), "{files:?}");
    let left = m["left_out"].as_array().unwrap();
    for w in &whole {
        assert!(left.iter().any(|x| x["what"] == json!(w) && x["why"].as_str().unwrap().contains("max_images")), "{w} in {left:?}");
    }
    for d in [all, out] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn the_packet_offers_the_submittals_of_its_marks_and_names_one_the_job_did_not_list() {
    use cadaddy_engine::doc::props::{Source, SourceKind};
    let j = job();
    // The receptacle carries its submittal page; the fan carries none, but the job applied a fan
    // submittal to another element of its type (mark EF-1) elsewhere.
    let mut s = j.s;
    layer(&mut s, "G-OTHER");
    line(&mut s, A121, [60.0, 2.0], [61.0, 2.0]);
    let other = {
        let d = s.doc().unwrap();
        d.model.iter().filter(|e| e.common.layer == "G-OTHER").last().map(|e| e.handle.hex()).unwrap()
    };
    let sub = |doc: &str| Source::from_doc(SourceKind::Submittal, doc);
    let mut s = record(
        s,
        &[
            (j.recep.clone(), "submittal", "Sample Devices p.2", sub("Sample Devices p.2")),
            (other.clone(), "type_mark", "EF-1", Source::user()),
            (other, "submittal", "Sample Fans p.4", sub("Sample Fans p.4")),
        ],
    );
    let id = office(&mut s);
    let out = dir("subs");
    s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy()})).unwrap();
    let m = read_json(&out.join("manifest.json"));
    let un = m["submittals_unlisted"].as_array().unwrap();
    let by = |name: &str| un.iter().find(|x| x["submittal"] == name).unwrap_or_else(|| panic!("{name} in {un:?}"));
    let dev = by("Sample Devices");
    assert_eq!(dev["pages"], json!([2]));
    assert!(dev["for"].as_array().unwrap().iter().any(|f| f == "mark DUPLEX"), "{dev}");
    let fans = by("Sample Fans");
    assert_eq!(fans["pages"], json!([4]));
    assert_eq!(fans["applied_to"], 1);
    assert_eq!(fans["marks"], json!(["EF-1"]));
    assert!(fans["for"].as_array().unwrap().iter().any(|f| f.as_str().unwrap().starts_with("mark EF-1 (the job's submittal for its type")), "{fans}");
    // left_out says which, and that the job lists none.
    let left = m["left_out"].as_array().unwrap();
    let fl = left.iter().find(|x| x["what"] == "submittal Sample Fans").unwrap_or_else(|| panic!("{left:?}"));
    let why = fl["why"].as_str().unwrap();
    assert!(why.contains("applied it to 1 element(s) (marks EF-1)") && why.contains("the job lists none"), "{why}");
    let _ = std::fs::remove_dir_all(&out);
}

/// A copy of `s`'s drawing with `f` applied, in a new session.
fn edited(s: &Session, f: impl FnOnce(&mut cadaddy_engine::doc::Drawing)) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    f(&mut d);
    let mut out = Session::empty();
    out.open_drawing(d, "Sample Job", None);
    out
}

fn add(d: &mut cadaddy_engine::doc::Drawing, layer: &str, kind: EntityKind) {
    let h = d.new_handle();
    let mut e = Entity::new(h, kind);
    e.common.layer = layer.into();
    d.model.push(e);
}

fn big_text(at: [f64; 2], style: &str, value: &str) -> EntityKind {
    EntityKind::Text(Text {
        insert: Vec3::new(at[0], at[1], 0.0),
        align_pt: None,
        height: 120.0,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: style.into(),
        halign: Default::default(),
        valign: Default::default(),
    })
}

/// Every picture the manifest lists is a file in the packet.
fn all_written(m: &Value, out: &Path) {
    for sec in ["prints", "model", "submittals"] {
        for p in m[sec].as_array().unwrap() {
            let f = out.join(p["file"].as_str().unwrap());
            assert!(std::fs::metadata(&f).is_ok_and(|x| x.len() > 0), "{} is listed but not written", f.display());
        }
    }
}

#[test]
fn a_print_crop_draws_the_lettering_as_printed_and_a_late_mask_hides_nothing() {
    use cadaddy_engine::doc::{Layer, TextStyle, Wipeout};
    use cadaddy_engine::geom::Vec2;
    let j = job();
    // As a PDF import leaves a sheet: lettering read off strokes, the strokes kept on an off
    // layer; the readings at a cluster's size (two sheet inches high); a mask merged over the
    // general notes, later than the notes it once sat behind.
    let stroke_at = at(E101, [8.0, 3.0]);
    let notes = [E101 + 2800.0, 2000.0];
    let readings = |d: &mut cadaddy_engine::doc::Drawing| {
        d.text_styles.push(TextStyle { name: "PDF-SHX-OCR".into(), ..TextStyle::default() });
        add(d, "0", big_text(at(E101, [2.0, 2.0]), "PDF-SHX-OCR", "WWW"));
        add(d, "0", big_text(at(E101, [2.0, 8.0]), "Standard", "MMM"));
    };
    let strokes = |d: &mut cadaddy_engine::doc::Drawing| {
        d.layers.push(Layer { on: false, ..Layer::new("PDF-SHX-SOURCE") });
        for k in 0..3 {
            let x = stroke_at[0] + f64::from(k) * 4.0;
            add(d, "PDF-SHX-SOURCE", EntityKind::Line(Line { a: Vec3::new(x, stroke_at[1], 0.0), b: Vec3::new(x, stroke_at[1] + 6.0, 0.0) }));
        }
        let (x, y) = (notes[0] - 20.0, notes[1] - 40.0);
        let boundary = vec![Vec2::new(x, y), Vec2::new(x + 400.0, y), Vec2::new(x + 400.0, y + 80.0), Vec2::new(x, y + 80.0)];
        add(d, "0", EntityKind::Wipeout(Wipeout { boundary }));
    };
    let mut plain = edited(&j.s, strokes);
    let mut read = edited(&plain, readings);
    let id = office(&mut read);
    let (o1, o2) = (dir("lettering-plain"), dir("lettering-read"));
    plain.execute("validate.packet", &json!({"area": id, "out_dir": o1.to_string_lossy()})).unwrap();
    read.execute("validate.packet", &json!({"area": id, "out_dir": o2.to_string_lossy()})).unwrap();
    let (m1, m2) = (read_json(&o1.join("manifest.json")), read_json(&o2.join("manifest.json")));
    all_written(&m2, &o2);
    // The readings draw nothing: the crop is the one without them, byte for byte.
    let e = crop(&m2, "E-101", "plan");
    let file = e["file"].as_str().unwrap();
    assert_eq!(std::fs::read(o1.join(file)).unwrap(), std::fs::read(o2.join(file)).unwrap(), "the readings show in {file}");
    assert_eq!(crop(&m1, "E-101", "plan")["file"], e["file"]);
    // The strokes the page prints are drawn.
    let img = image::open(o2.join(file)).unwrap().to_rgba8();
    let (x, y) = pixel_of(e, [stroke_at[0] + 4.0, stroke_at[1] + 3.0]);
    assert!(darkest(&img, x, y, 2) < 400, "the lettering's strokes at ({x:.0},{y:.0})");
    // The notes show through the late mask.
    let n = crop(&m2, "E-101", "general_notes");
    let img = image::open(o2.join(n["file"].as_str().unwrap())).unwrap().to_rgba8();
    let (x, y) = pixel_of(n, [notes[0] + 20.0, notes[1] + 2.0]);
    assert!(darkest(&img, x, y, 4) < 400, "the notes under the mask at ({x:.0},{y:.0})");
    // No PDF to compare with: said, per sheet.
    let why = m2["similarity"]["not_compared"].as_array().unwrap();
    assert!(why.iter().any(|w| w["sheet"] == "E-101" && w["why"] == "the job keeps no PDF page for this sheet"), "{why:?}");
}

#[test]
fn each_view_is_labelled_by_the_view_it_shows_on_its_own_sheet() {
    let j = job();
    let mut sp = spec();
    // A-401 prints two views of the office's type side by side: its sanitary and its water plan;
    // the set links the sheet by its sanitary view only.
    let san = [A401 + 200.0, 200.0, A401 + 1600.0, 1400.0];
    let wat = [A401 + 1800.0, 200.0, A401 + 3200.0, 1400.0];
    let sheets: Vec<Value> = sp["sheets"].as_array().unwrap().iter().filter(|l| l["number"] != "A-401").cloned().collect();
    sp["sheets"] = json!(sheets);
    sp["sheets"].as_array_mut().unwrap().push(json!({"number": "A-401", "title": "ENLARGED PLANS", "discipline": "A", "source": "model",
        "rect": [A401, 0.0, A401 + SW, SH], "view": {"title": "OFFICE PLAN - SANITARY", "rect": san}}));
    sp["unit_types"] = json!([{"id": "OFC", "name": "OFFICE TYPE", "views": [
        {"sheet": "A-401", "title": "OFFICE PLAN - SANITARY", "rect": san},
        {"sheet": "A-401", "title": "OFFICE PLAN - WATER", "rect": wat}]}]);
    sp["unit_instances"] = json!([{"number": "101", "level": "L1", "type": "OFC"}]);
    let mut s = edited(&j.s, |_| {});
    s.execute("bspec.set", &json!({"spec": sp, "replace": true})).unwrap();
    let id = office(&mut s);
    let out = dir("views");
    s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy()})).unwrap();
    let m = read_json(&out.join("manifest.json"));
    let unit: Vec<&Value> = m["prints"].as_array().unwrap().iter().filter(|c| c["sheet"] == "A-401" && c["placed_by"] == "unit_type").collect();
    for (part, title) in [("WATER", "OFFICE PLAN - WATER"), ("SANITARY", "OFFICE PLAN - SANITARY")] {
        let c = unit.iter().find(|c| c["shows"].as_str().is_some_and(|x| x.ends_with(part))).unwrap_or_else(|| panic!("no {part} view in {unit:?}"));
        assert_eq!(c["view"], title, "{c}");
    }
    let prompt = std::fs::read_to_string(out.join("prompt.md")).unwrap();
    assert!(prompt.contains("view OFFICE PLAN - WATER"), "{prompt}");
}
