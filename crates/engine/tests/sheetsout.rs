//! A built model makes its drawing set (Q6016): a made-up two-level building whose elements
//! carry their properties → `sheetset.from_model` → layouts (cover and index, plans per level per
//! discipline, reflected ceiling and roof plans, elevations, sections, enlarged plans, door and
//! panel schedules, the 3D view) → one PDF with a page per sheet at its paper size and scale,
//! PDF layers per drawing layer and searchable text; read back, the pages hold what the model
//! has (walls at scale, door and room tags, circuits, schedule rows).

use std::collections::BTreeMap;

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Props, Source};
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn io() {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

/// An element as the build makes them: a box (feet; the drawing is in inches) with the build's
/// tag `[part, level, source]` and its property store.
fn el(d: &mut Drawing, part: &str, level: &str, layer: &str, lo: [f64; 3], hi: [f64; 3], ps: &[(&str, Value)]) -> Handle {
    let v = |x: f64, y: f64, z: f64| Vec3 { x: x * 12.0, y: y * 12.0, z: z * 12.0 };
    let vertices = vec![
        v(lo[0], lo[1], lo[2]),
        v(hi[0], lo[1], lo[2]),
        v(hi[0], hi[1], lo[2]),
        v(lo[0], hi[1], lo[2]),
        v(lo[0], lo[1], hi[2]),
        v(hi[0], lo[1], hi[2]),
        v(hi[0], hi[1], hi[2]),
        v(lo[0], hi[1], hi[2]),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }));
    e.common.layer = layer.into();
    let x = vec![
        RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
        RawTag { code: 1000, value: part.into() },
        RawTag { code: 1000, value: level.into() },
        RawTag { code: 1000, value: "0".into() },
    ];
    let mut store = Props::default();
    for (k, val) in ps {
        store.set(k, Prop::new(val.clone(), Source::print("Sample Sheet A-101", None)));
    }
    e.common.xdata = props::write(&x, &store).into();
    d.ensure_layer(layer);
    d.model.push(e);
    h
}

/// The walls of one level at `z` (feet): a 40' × 30' box of 8" walls and a wall at x = 20.
fn level_walls(d: &mut Drawing, level: &str, z: f64) {
    let t = 0.33;
    let w = |d: &mut Drawing, lo: [f64; 2], hi: [f64; 2], ty: &str, rating: Option<&str>| {
        let mut ps = vec![("type", json!(ty))];
        if let Some(r) = rating {
            ps.push(("fire_rating", json!(r)));
        }
        el(d, "wall", level, "A-WALL", [lo[0], lo[1], z], [hi[0], hi[1], z + 10.0], &ps);
    };
    w(d, [0.0, 0.0], [40.0, t], "W1", None);
    w(d, [0.0, 30.0 - t], [40.0, 30.0], "W1", None);
    w(d, [0.0, t], [t, 30.0 - t], "W1", None);
    w(d, [40.0 - t, t], [40.0, 30.0 - t], "W1", None);
    w(d, [20.0 - t / 2.0, t], [20.0 + t / 2.0, 30.0 - t], "W2", Some("1 HR"));
}

fn model() -> Session {
    let mut d = Drawing::new_imperial();
    for (level, z) in [("L1", 0.0), ("L2", 10.0)] {
        level_walls(&mut d, level, z);
        let n = if level == "L1" { 1 } else { 2 };
        let (a, b) = if n == 1 { ("OFFICE", "TOILET") } else { ("GUEST ROOM", "BATH") };
        el(
            &mut d,
            "room",
            level,
            "A-AREA",
            [0.33, 0.33, z],
            [19.8, 29.67, z + 9.0],
            &[("number", json!(format!("{n}01"))), ("name", json!(a)), ("area", json!(574)), ("floor_finish", json!("VCT-1"))],
        );
        el(&mut d, "room", level, "A-AREA", [20.2, 0.33, z], [39.67, 29.67, z + 9.0], &[("number", json!(format!("{n}02"))), ("name", json!(b))]);
        // Doors: one into each room through the south wall, one in the rated wall.
        el(
            &mut d,
            "door",
            level,
            "A-DOOR",
            [8.0, -0.1, z],
            [11.0, 0.43, z + 7.0],
            &[("number", json!(format!("{n}01A"))), ("width", json!("3'-0\"")), ("height", json!("7'-0\"")), ("hardware_set", json!("HW-1"))],
        );
        el(
            &mut d,
            "door",
            level,
            "A-DOOR",
            [19.73, 12.0, z],
            [20.27, 15.0, z + 7.0],
            &[("number", json!(format!("{n}02A"))), ("width", json!("3'-0\"")), ("fire_rating", json!("45 MIN"))],
        );
        el(
            &mut d,
            "window",
            level,
            "A-GLAZ",
            [28.0, 29.6, z + 3.0],
            [32.0, 30.1, z + 7.0],
            &[("type_mark", json!("WA")), ("width", json!("4'-0\""))],
        );
        el(&mut d, "ceiling", level, "A-CLNG", [0.33, 0.33, z + 9.0], [19.8, 29.67, z + 9.1], &[("type", json!("ACT-1"))]);
        // The panel and its devices.
        let panel = format!("LP{n}");
        el(&mut d, "equipment", level, "E-EQPM", [0.5, 1.0, z], [1.5, 1.5, z + 6.0], &[("tag", json!(panel)), ("type", json!("Panelboard"))]);
        for (i, c) in [(0, "1"), (1, "1"), (2, "3")] {
            let x = 4.0 + 8.0 * f64::from(i);
            el(
                &mut d,
                "device",
                level,
                "E-POWER",
                [x, 0.4, z + 1.5],
                [x + 0.33, 0.57, z + 1.83],
                &[
                    ("type", json!("Duplex receptacle")),
                    ("type_mark", json!("DR")),
                    ("circuit", json!(format!("{panel}-{c}"))),
                    ("load_va", json!(180)),
                ],
            );
        }
        for i in 0..2 {
            let x = 6.0 + 8.0 * f64::from(i);
            el(
                &mut d,
                "light",
                level,
                "E-LIGHTING",
                [x, 14.0, z + 8.8],
                [x + 2.0, 18.0, z + 9.0],
                &[("type_mark", json!("A")), ("type", json!("2x4 troffer")), ("circuit", json!(format!("{panel}-5"))), ("load_va", json!(40))],
            );
        }
        el(
            &mut d,
            "device",
            level,
            "E-SYSTEMS",
            [10.0, 29.4, z + 1.5],
            [10.3, 29.6, z + 1.8],
            &[("type", json!("Data outlet")), ("type_mark", json!("D"))],
        );
        // Mechanical: a supply duct and two diffusers.
        el(&mut d, "duct", level, "M-DUCT", [2.0, 20.0, z + 9.2], [38.0, 21.0, z + 9.9], &[("size", json!("12x8")), ("system", json!("Supply Air"))]);
        for x in [6.0, 26.0] {
            el(
                &mut d,
                "diffuser",
                level,
                "M-DIFF",
                [x, 22.0, z + 9.0],
                [x + 2.0, 24.0, z + 9.2],
                &[("type_mark", json!("SD-1")), ("cfm", json!(250))],
            );
        }
        // Plumbing: a lavatory and a water closet on a 2" sanitary line.
        el(&mut d, "pipe", level, "P-SANR", [21.0, 2.0, z], [38.0, 2.17, z + 0.17], &[("size", json!("2\"")), ("system", json!("SAN"))]);
        el(&mut d, "fixture", level, "P-FIXT", [22.0, 3.0, z], [23.5, 4.5, z + 3.0], &[("type", json!("Lavatory")), ("type_mark", json!("L-1"))]);
        el(
            &mut d,
            "fixture",
            level,
            "P-FIXT",
            [25.0, 3.0, z],
            [26.5, 5.0, z + 2.5],
            &[("type", json!("Water closet")), ("type_mark", json!("WC-1"))],
        );
        // Fire protection: two sprinkler heads.
        for x in [10.0, 30.0] {
            el(&mut d, "device", level, "F-SPRINK", [x, 10.0, z + 8.9], [x + 0.2, 10.2, z + 9.0], &[("type", json!("Sprinkler head"))]);
        }
    }
    el(&mut d, "roof", "L2", "A-ROOF", [0.0, 0.0, 20.0], [40.0, 30.0, 21.0], &[("slope", json!("1/4\"/FT"))]);
    el(
        &mut d,
        "equipment",
        "L2",
        "M-EQPM",
        [30.0, 20.0, 21.0],
        [36.0, 26.0, 25.0],
        &[("tag", json!("RTU-1")), ("type", json!("Rooftop unit")), ("cfm", json!(1000))],
    );
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Building", None);
    s
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-sheetsout-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sheet<'a>(r: &'a Value, number: &str) -> &'a Value {
    r["sheets"].as_array().unwrap().iter().find(|s| s["number"] == number).unwrap_or_else(|| panic!("no sheet {number}: {}", r["index"]))
}

fn count(sh: &Value, what: &str) -> u64 {
    sh["counts"][what].as_u64().unwrap_or(0)
}

/// The words on a page read back: every text (the searchable copy of stroke lettering too).
fn words(d: &Drawing) -> Vec<String> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some(t.value.clone()),
            EntityKind::MText(t) => Some(t.contents.clone()),
            _ => None,
        })
        .collect()
}

/// Total length of what a page has on `layer` (paper inches).
fn length_on(d: &Drawing, layer: &str) -> f64 {
    let mut total = 0.0;
    for e in d.model.iter().filter(|e| e.common.layer.as_str() == layer) {
        match &e.kind {
            EntityKind::Line(l) => total += ((l.b.x - l.a.x).powi(2) + (l.b.y - l.a.y).powi(2)).sqrt(),
            EntityKind::LwPolyline(p) => {
                let v = &p.vertices;
                let n = v.len();
                let segs = if p.closed { n } else { n.saturating_sub(1) };
                for i in 0..segs {
                    let (a, b) = (v[i].p, v[(i + 1) % n].p);
                    total += ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
                }
            }
            _ => {}
        }
    }
    total
}

/// A circle as a plot brings it back: a circle, or a closed run of points all as far from their
/// middle (not a letter's strokes).
fn round(k: &EntityKind) -> bool {
    let mut pts: Vec<(f64, f64)> = match k {
        EntityKind::Circle(_) => return true,
        // Arcs all the way round.
        EntityKind::LwPolyline(p) if p.vertices.len() >= 2 && p.vertices.iter().all(|v| v.bulge.abs() > 0.3) => return true,
        EntityKind::LwPolyline(p) => p.vertices.iter().map(|v| (v.p.x, v.p.y)).collect(),
        _ => return false,
    };
    // Closed by its last point on its first.
    if let (Some(a), Some(b)) = (pts.first().copied(), pts.last().copied())
        && pts.len() > 2
        && (a.0 - b.0).hypot(a.1 - b.1) < 1e-6
    {
        pts.pop();
    }
    if pts.len() < 8 {
        return false;
    }
    let n = pts.len() as f64;
    let (cx, cy) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
    let r: Vec<f64> = pts.iter().map(|p| (p.0 - cx).hypot(p.1 - cy)).collect();
    let (lo, hi) = (r.iter().copied().fold(f64::INFINITY, f64::min), r.iter().copied().fold(0.0, f64::max));
    hi > 0.0 && (hi - lo) / hi < 0.1
}

#[test]
fn a_two_level_model_makes_its_drawing_set_and_pdf() {
    io();
    let mut s = model();
    let dir = tmp("set");
    let pdf = dir.join("Sample Set.pdf");
    let dwg = dir.join("Sample Set.dwg");
    let r = s
        .execute(
            "sheetset.from_model",
            &json!({ "pdf": pdf.to_string_lossy(), "dwg": dwg.to_string_lossy(), "project": { "name": "Sample Building", "number": "0001" } }),
        )
        .unwrap();
    let numbers: Vec<&str> = r["sheets"].as_array().unwrap().iter().filter_map(|s| s["number"].as_str()).collect();
    // Cover first; numbered by discipline; plans per level.
    assert_eq!(numbers.first(), Some(&"G001"), "{numbers:?}");
    for n in [
        "A101", "A102", "A103", "A104", "A105", "A201", "A301", "A401", "A601", "A901", "FP101", "FP102", "P101", "P102", "M101", "M102", "E101",
        "E102", "E201", "E202", "E301", "E601",
    ] {
        assert!(numbers.contains(&n), "{n} missing: {numbers:?}");
    }
    let a101 = sheet(&r, "A101");
    assert_eq!(a101["title"], "FLOOR PLAN - L1");
    assert_eq!(a101["scale"], "1/4\" = 1'-0\"");
    assert_eq!(count(a101, "walls"), 5);
    assert_eq!(count(a101, "doors"), 2);
    assert_eq!(count(a101, "door_tags"), 2);
    assert_eq!(count(a101, "room_tags"), 2);
    assert_eq!(count(a101, "wall_tags"), 5);
    assert_eq!(count(a101, "window_tags"), 1);
    assert!(count(a101, "dimensions") >= 6, "{}", a101["counts"]);
    assert!(count(a101, "keynote_tags") >= 2, "the rated wall and door: {}", a101["counts"]);
    assert_eq!(count(a101, "north_arrows"), 1);
    assert_eq!(count(a101, "scale_bars"), 1);
    assert_eq!(sheet(&r, "A103")["title"], "REFLECTED CEILING PLAN - L1");
    assert_eq!(count(sheet(&r, "A103"), "ceiling_tags"), 1);
    assert_eq!(count(sheet(&r, "A103"), "light_fixtures"), 2);
    assert_eq!(sheet(&r, "A105")["title"], "ROOF PLAN - ROOF");
    assert_eq!(count(sheet(&r, "A105"), "roofs"), 1);
    let e101 = sheet(&r, "E101");
    assert_eq!(count(e101, "devices"), 3);
    assert_eq!(count(e101, "home_runs"), 2, "LP1-1 and LP1-3: {}", e101["counts"]);
    assert_eq!(count(e101, "circuit_tags"), 3);
    assert_eq!(count(e101, "panels"), 1);
    assert_eq!(count(sheet(&r, "E201"), "home_runs"), 1);
    assert_eq!(count(sheet(&r, "E301"), "devices"), 1, "the data outlet is a systems device");
    assert_eq!(count(sheet(&r, "M101"), "duct_tags"), 1);
    assert_eq!(count(sheet(&r, "M101"), "terminals"), 2);
    assert_eq!(count(sheet(&r, "P101"), "pipe_tags"), 1);
    assert_eq!(count(sheet(&r, "P101"), "fixtures"), 2);
    assert_eq!(count(sheet(&r, "FP101"), "heads"), 2);
    let a301 = sheet(&r, "A301");
    assert_eq!(count(a301, "views"), 2, "a section each way");
    assert!(count(a301, "cut") >= 6, "{}", a301["counts"]);
    assert!(count(a301, "level_markers") >= 4);
    assert_eq!(count(sheet(&r, "A201"), "views"), 4);
    assert!(count(sheet(&r, "A401"), "views") >= 2, "the toilet and the bath enlarged");
    let a601 = sheet(&r, "A601");
    assert!(count(a601, "schedules") >= 3, "door, window, room finish: {}", a601["counts"]);
    assert_eq!(count(sheet(&r, "E601"), "schedules"), 3, "lighting fixtures and a panel schedule each for LP1 and LP2");
    // Every sheet is a layout with the title block filled in.
    let index = r["index"].as_array().unwrap();
    assert_eq!(index.len(), numbers.len());
    {
        let d = s.doc().unwrap();
        for sh in r["sheets"].as_array().unwrap() {
            let l = d.layout(sh["layout"].as_str().unwrap()).unwrap();
            // Stored short side first, turned landscape.
            assert!(
                (l.page.height_mm - 36.0 * 25.4).abs() < 0.01 && (l.page.width_mm - 24.0 * 25.4).abs() < 0.01 && l.page.landscape,
                "{:?}",
                l.page
            );
            let tb = l
                .entities
                .iter()
                .find_map(|e| match &e.kind {
                    EntityKind::Insert(i) if i.block == "G_TITLE_ARCH_D" => Some(i.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{} has no title block: {}", sh["number"], r["notes"]));
            let no = tb.attribs.iter().find(|a| a.tag == "SHEET_NO").map(|a| a.text.value.clone());
            assert_eq!(no.as_deref(), sh["number"].as_str());
        }
    }
    // The PDF: a page a sheet, 36" × 24", layers per drawing layer, the views at scale.
    let bytes = std::fs::read(&pdf).unwrap();
    // To look at the set: SHEETSOUT_KEEP=<folder> keeps a copy of the PDF there.
    if let Some(keep) = std::env::var_os("SHEETSOUT_KEEP") {
        std::fs::copy(&pdf, std::path::Path::new(&keep).join("sample-set.pdf")).unwrap();
    }
    assert_eq!(cadaddy_io::pdf_read::page_count(&bytes).unwrap() as usize, numbers.len());
    let page = |n: usize| cadaddy_io::pdf_read::read_json(&bytes, &json!({ "page": n, "recoverText": false, "images": false })).unwrap();
    let at = |num: &str| numbers.iter().position(|n| *n == num).unwrap() + 1;
    let (d, rep) = page(at("A101"));
    let sheet_in = rep["sheet"].as_array().unwrap().iter().filter_map(Value::as_f64).collect::<Vec<_>>();
    assert!((sheet_in[0] - 36.0).abs() < 0.05 && (sheet_in[1] - 24.0).abs() < 0.05, "{}", rep["sheet"]);
    let layers: Vec<&str> = rep["layers"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for l in ["A-WALL", "A-DOOR", "A-DOOR-IDEN", "A-AREA-IDEN", "A-ANNO-DIMS", "G-ANNO-TTLB"] {
        assert!(layers.contains(&l), "{l} not a PDF layer: {layers:?}");
    }
    // The reader finds the sheet's 1/4" = 1'-0" and brings the plan back at full size (inches):
    // the five walls' outlines, 2 × (40 + 0.33) × 2 + 2 × (29.34 + 0.33) × 3 feet.
    assert!((rep["scale"].as_f64().unwrap() - 48.0).abs() < 1e-6, "{}", rep["scale"]);
    let walls_ft = 2.0 * (40.0 + 0.33) * 2.0 + 2.0 * (29.34 + 0.33) * 3.0;
    let got = length_on(&d, "A-WALL") / 12.0;
    assert!((got - walls_ft).abs() / walls_ft < 0.01, "wall linework {got:.1} ft, the model's {walls_ft:.1}");
    let w = words(&d);
    for t in ["101A", "102A", "101", "102", "OFFICE", "TOILET", "W1", "W2", "A101", "FLOOR PLAN - L1", "Sample Building"] {
        assert!(w.iter().any(|x| x.contains(t)), "`{t}` not on the A101 page: {w:?}");
    }
    let door_tags = d.model.iter().filter(|e| e.common.layer.as_str() == "A-DOOR-IDEN" && round(&e.kind)).count();
    assert_eq!(door_tags, 2);
    // The power plan's page: the devices, circuit tags and home runs read back.
    let (d, rep) = page(at("E101"));
    let layers: Vec<&str> = rep["layers"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(layers.contains(&"E-POWR") && layers.contains(&"E-POWR-WIRE") && layers.contains(&"A-WALL-BKGD"), "{layers:?}");
    let devices = d.model.iter().filter(|e| e.common.layer.as_str() == "E-POWR" && round(&e.kind)).count();
    assert_eq!(devices, 3);
    let w = words(&d);
    assert!(w.iter().filter(|x| x.contains("LP1-1")).count() >= 3, "{w:?}");
    // The schedules' page: every door's row.
    let (d, _) = page(at("A601"));
    let w = words(&d);
    for t in ["DOOR SCHEDULE", "101A", "102A", "201A", "202A", "HW-1", "45 MIN", "ROOM FINISH SCHEDULE", "VCT-1"] {
        assert!(w.iter().any(|x| x.contains(t)), "`{t}` not in the schedules: {w:?}");
    }
    let (d, _) = page(at("E601"));
    let w = words(&d);
    assert!(w.iter().any(|x| x.contains("PANEL LP1")) && w.iter().any(|x| x.contains("PANEL LP2")), "{w:?}");
    // The cover's index lists every sheet.
    let (d, _) = page(1);
    let w = words(&d);
    for n in &numbers {
        assert!(w.iter().any(|x| x == n || x.contains(n)), "{n} not in the index: {w:?}");
    }
    // The DWG has the same sheets.
    let back = cadaddy_io::read(&std::fs::read(&dwg).unwrap(), "Sample Set.dwg").unwrap();
    for sh in r["sheets"].as_array().unwrap() {
        let name = sh["layout"].as_str().unwrap();
        let l = back.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(name)).unwrap_or_else(|| panic!("{name} not in the DWG"));
        assert!(!l.entities.is_empty(), "{name} empty in the DWG");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn building_again_replaces_the_sheets_and_one_pdf_per_sheet() {
    io();
    let mut s = model();
    let first = s.execute("sheetset.from_model", &json!({})).unwrap();
    let layouts = s.doc().unwrap().layouts.len();
    let entities: BTreeMap<String, usize> = s.doc().unwrap().layouts.iter().map(|l| (l.name.clone(), l.entities.len())).collect();
    let again = s.execute("sheetset.from_model", &json!({})).unwrap();
    assert_eq!(first["index"], again["index"]);
    assert_eq!(s.doc().unwrap().layouts.len(), layouts, "no layout twice");
    let now: BTreeMap<String, usize> = s.doc().unwrap().layouts.iter().map(|l| (l.name.clone(), l.entities.len())).collect();
    assert_eq!(entities, now, "each sheet drawn again, not added to");
    // Electrical only, Level 2 only: the sheets the first set made and this one doesn't go.
    let dir = tmp("split");
    let pdf = dir.join("Sample.pdf");
    let r =
        s.execute("sheetset.from_model", &json!({ "disciplines": ["E"], "levels": ["L2"], "pdf": pdf.to_string_lossy(), "split": true })).unwrap();
    let numbers: Vec<&str> = r["sheets"].as_array().unwrap().iter().filter_map(|s| s["number"].as_str()).collect();
    assert_eq!(numbers, ["G001", "E101", "E201", "E301", "E601"], "{}", r["index"]);
    assert_eq!(sheet(&r, "E101")["level"], "L2");
    assert!(!r["removed"].as_array().unwrap().is_empty());
    let files = r["files"].as_array().unwrap();
    assert_eq!(files.len(), numbers.len());
    for f in files {
        let bytes = std::fs::read(f.as_str().unwrap()).unwrap();
        assert_eq!(cadaddy_io::pdf_read::page_count(&bytes).unwrap(), 1);
    }
    assert!(files.iter().any(|f| f.as_str().unwrap().ends_with("Sample-E101.pdf")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_chosen_scale_and_section_and_bad_asks() {
    let mut s = model();
    // A section through the toilet room only, north-south, at 1/8".
    let r = s
        .execute("sheetset.from_model", &json!({ "scale": "1/8\"=1'-0\"", "disciplines": "A", "sections": [{ "from": [360, -24], "to": [360, 384], "name": "SECTION AT TOILET" }], "views3d": false, "schedules": false }))
        .unwrap();
    assert_eq!(sheet(&r, "A101")["scale"], "1/8\" = 1'-0\"");
    assert!(r["sheets"].as_array().unwrap().iter().all(|s| s["number"] != "A201" && s["number"] != "A901" && s["number"] != "A601"));
    assert_eq!(count(sheet(&r, "A301"), "views"), 1);
    // Bad asks are refused, nothing drawn.
    let layouts = s.doc().unwrap().layouts.len();
    for bad in [
        json!({ "scale": "huge" }),
        json!({ "size": "napkin" }),
        json!({ "disciplines": ["Q"] }),
        json!({ "sections": [{ "from": [0, 0] }] }),
        json!({ "enlarged": [{ "level": "L1", "rect": [10, 10, 0, 0] }] }),
        json!({ "title_block": "NO SUCH BLOCK" }),
    ] {
        assert!(s.execute("sheetset.from_model", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.doc().unwrap().layouts.len(), layouts);
    // A drawing with no model says so.
    let mut empty = Session::empty();
    empty.open_drawing(Drawing::new_imperial(), "Sample Empty", None);
    let e = empty.execute("sheetset.from_model", &json!({})).unwrap_err();
    assert!(e.to_string().contains("no building elements"), "{e}");
}
