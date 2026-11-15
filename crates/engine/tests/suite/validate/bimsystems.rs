//! The Systems view (Q6617): the whole building as a coordination model, the same in the 3D
//! view and in `model.render`. The shell a pale ghost in which every floor plate reads, every
//! system solid and coloured by system from the rules' palette (equipment never grey), branch
//! work left out unless its switch is on, objects off the building only with the site switch;
//! `model.render` draws exactly what the BIM view draws, says what it drew, and a `view`
//! parameter picks another without changing the drawing's. A made-up building
//! (`sample::bim_building`).

use std::collections::BTreeSet;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook;
use cadaddy_engine::cmd::bimviews::{self, Rules};
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::{Drawing, Entity, Space};
use serde_json::{Value, json};

fn building() -> Session {
    session_of(cadaddy_engine::sample::bim_building(3, 6))
}

fn session_of(d: Drawing) -> Session {
    let mut s = Session::new();
    s.open_drawing(d, "Sample Building", None);
    s
}

fn view(s: &mut Session, p: Value) -> Value {
    s.execute("bim.view", &p).unwrap_or_else(|e| panic!("bim.view {p}: {e}"))
}

fn shown(s: &Session) -> Drawing {
    bimlook::display(s.doc().unwrap()).expect("the BIM view of a built model")
}

fn label(e: &Entity) -> String {
    let tag = |k: &str| bimlook::made_tag(e, k).unwrap_or("").to_string();
    match made_as(e).as_deref() {
        Some("mep") => format!("mep {} {} {} {} {}", tag("kind"), tag("part"), tag("system"), tag("tag"), tag("size")),
        Some("wall") => format!("wall {}", e.common.layer),
        Some(p) => p.to_string(),
        None => format!("context {}", e.common.layer),
    }
}

fn labels(d: &Drawing) -> Vec<String> {
    d.model.iter().map(|e| label(e)).collect()
}

fn has(ls: &[String], f: impl Fn(&str) -> bool) -> bool {
    ls.iter().any(|l| f(l))
}

fn render(s: &mut Session, extra: Value) -> Value {
    let mut p = json!({"width": 900, "height": 600, "dir": [-1.0, -1.4, 0.9]});
    if let (Some(o), Some(x)) = (p.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            o.insert(k.clone(), v.clone());
        }
    }
    s.execute("model.render", &p).unwrap_or_else(|e| panic!("model.render {p}: {e}"))
}

fn pixels(r: &Value) -> image::RgbaImage {
    let b = cadaddy_engine::cmd::file::base64_decode(r["png"].as_str().expect("a png")).expect("base64");
    image::load_from_memory(&b).expect("a png").to_rgba8()
}

/// Pixels that differ between two pictures of the same size.
fn differ(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels().zip(b.pixels()).filter(|(p, q)| p.0.iter().zip(q.0).any(|(x, y)| x.abs_diff(y) > 6)).count()
}

fn rgb(c: &Color) -> Option<[u8; 3]> {
    match c {
        Color::True(c) => Some([c.0, c.1, c.2]),
        _ => None,
    }
}

/// A grey: its channels within a few steps of each other.
fn grey(c: [u8; 3]) -> bool {
    let (lo, hi) = (c.iter().min().copied().unwrap_or(0), c.iter().max().copied().unwrap_or(0));
    hi - lo < 24
}

#[test]
fn a_model_with_mep_opens_in_the_systems_view() {
    let mut s = building();
    let r = view(&mut s, json!({}));
    assert_eq!(r["view"], "systems", "{r}");
    let offered: BTreeSet<&str> = r["offered"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for k in ["branches", "interior", "site", "m", "e", "p"] {
        assert!(offered.contains(k), "{k} offered: {offered:?}");
    }
    for k in ["branches", "interior", "site"] {
        assert_eq!(r["switches"][k], false, "{k} is off by default");
    }
    // Far fewer than the whole model.
    assert!(r["drawn"].as_u64().unwrap() * 3 < r["total"].as_u64().unwrap(), "{r}");
    // A model with only its shell opens in Architectural (no systems to show).
    let mut d = cadaddy_engine::sample::bim_building(2, 2);
    let mep: Vec<_> = d.model.iter().filter(|e| made_as(e).as_deref() == Some("mep")).map(|e| e.handle).collect();
    for h in mep {
        d.remove_entity(h);
    }
    let mut a = session_of(d);
    assert_eq!(view(&mut a, json!({}))["view"], "architectural");
}

#[test]
fn systems_draws_ducts_mains_feeders_and_all_equipment_never_branch_work() {
    let mut s = building();
    let ls = labels(&shown(&s));
    for want in [
        "mep duct segment SA  24x12",
        "mep pipe segment SAN  4",
        "mep pipe segment CW  2",
        "mep pipe segment CW  3",
        "mep pipe segment FIRE MAIN  4",
        "mep pipe segment FIRE STANDPIPE  4",
        "mep tray segment TRAY  12x4",
        "mep conduit segment E-FEEDER  2",
        "mep conduit segment E-FEEDER  0.75",
        "mep equipment equipment:switchboard  SWB-1 ",
        "mep equipment equipment:panel  U ",
        "mep equipment equipment:panel  H2 ",
        "mep equipment equipment:doas  DOAS-1 ",
        "mep equipment equipment:fire_pump  FP-1 ",
    ] {
        assert!(has(&ls, |l| l == want), "{want} not drawn: {:?}", ls.iter().filter(|l| l.starts_with("mep")).collect::<BTreeSet<_>>());
    }
    assert!(has(&ls, |l| l.contains("equipment:exhaust_fan")), "every piece of equipment, small ones too");
    // The ghost shell: floors and the exterior walls, no interior walls, doors or rooms.
    assert!(has(&ls, |l| l == "floor"));
    assert!(has(&ls, |l| l == "wall A-WALL-EXTR"), "the exterior walls");
    for not in ["wall A-WALL", "door", "window", "room", "ceiling", "column", "stair", "casework", "furniture", "device", "light"] {
        assert!(!has(&ls, |l| l == not), "{not} in the Systems view");
    }
    // No branch conduit, its pieces, boxes, wire, devices, heads, terminals or small pipe.
    for not in ["atom:", "E-BRANCH", "terminal:", "CW  0.75", "SPRINKLER BRANCH", "C-2  0.5"] {
        assert!(!has(&ls, |l| l.contains(not)), "{not} drawn at building scale");
    }
    // The branches switch adds them; the interior switch the interior walls.
    view(&mut s, json!({"switches": {"branches": true, "interior": true}}));
    let ls = labels(&shown(&s));
    for want in ["atom:conduit", "atom:box", "terminal:", "CW  0.75", "E-BRANCH"] {
        assert!(has(&ls, |l| l.contains(want)), "{want} with the branches switch on");
    }
    assert!(has(&ls, |l| l == "wall A-WALL"), "interior walls with their switch on");
    let d = shown(&s);
    let interior = d.model.iter().find(|e| e.common.layer == "A-WALL" && made_as(e).as_deref() == Some("wall")).unwrap();
    assert_eq!(interior.common.material.as_deref(), Some(bimviews::SHELL_INTERIOR), "interior walls fainter");
}

#[test]
fn every_system_and_equipment_takes_a_palette_colour_never_grey() {
    let s = building();
    let d = shown(&s);
    let r = Rules::defaults();
    let palette: BTreeSet<[u8; 3]> = r.systems_palette.iter().map(|p| p.rgb).chain([r.systems_other_rgb]).collect();
    assert!(palette.iter().all(|c| !grey(*c)), "the palette has no grey");
    let mut equipment = 0;
    for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some("mep")) {
        let c = rgb(&e.common.color).unwrap_or_else(|| panic!("{} has no colour of its own", label(e)));
        assert!(palette.contains(&c), "{} is {c:?}, not a palette colour", label(e));
        assert!(e.common.material.as_deref().is_none_or(str::is_empty), "{} keeps a material: {:?}", label(e), e.common.material);
        if bimlook::made_tag(e, "kind") == Some("equipment") {
            equipment += 1;
        }
    }
    assert!(equipment >= 20, "{equipment}");
    // Equipment takes its system's colour: the switchboard electrical power's, the DOAS the
    // mechanical equipment's, the fire pump fire protection's.
    let color_of = |tag: &str| {
        d.model.iter().find(|e| bimlook::made_tag(e, "tag") == Some(tag)).and_then(|e| rgb(&e.common.color)).unwrap_or_else(|| panic!("{tag}"))
    };
    let row = |k: &str| r.systems_palette.iter().find(|p| p.key == k).map(|p| p.rgb).unwrap();
    assert_eq!(color_of("SWB-1"), row("electrical_power"));
    assert_eq!(color_of("DOAS-1"), row("mechanical_equipment"));
    assert_eq!(color_of("FP-1"), row("fire_protection"));
    // The shell: see-through floor plates and exterior walls in the shell's colours.
    let floor = d.model.iter().find(|e| made_as(e).as_deref() == Some("floor")).unwrap();
    assert_eq!(floor.common.material.as_deref(), Some(bimviews::SHELL_FLOOR));
    let m = d.materials.iter().find(|m| m.name == bimviews::SHELL_FLOOR).unwrap();
    assert!(m.opacity < 0.9 && m.opacity >= 0.2, "{}", m.opacity);
    // The palette is the project's: bim.view.rules changes a row's colour.
    let mut s = building();
    let mut rows = serde_json::to_value(&r.systems_palette).unwrap();
    for row in rows.as_array_mut().unwrap() {
        if row["key"] == "electrical_power" {
            row["rgb"] = json!([10, 200, 30]);
        }
    }
    s.execute("bim.view.rules", &json!({"rules": {"systems_palette": rows}})).unwrap();
    let d = shown(&s);
    let swb = d.model.iter().find(|e| bimlook::made_tag(e, "tag") == Some("SWB-1")).unwrap();
    assert_eq!(rgb(&swb.common.color), Some([10, 200, 30]));
}

#[test]
fn model_render_draws_the_bim_view_exactly_and_says_what_it_drew() {
    let mut s = building();
    let all = render(&mut s, json!({"style": "shaded"}));
    let sys = render(&mut s, json!({}));
    assert_eq!(sys["view"], "systems", "{sys}");
    let (n_all, n_sys) = (all["triangles"].as_u64().unwrap(), sys["triangles"].as_u64().unwrap());
    assert!(n_sys * 2 < n_all, "the Systems view draws far less than the whole model: {n_sys} of {n_all}");
    // The same drawing the 3D view builds: as many triangles.
    let d = shown(&s);
    let ext = d.extents(&Space::Model);
    let tol = ext.width().max(ext.height()) / 1500.0;
    let (p, t) = (std::sync::atomic::AtomicUsize::new(0), std::sync::atomic::AtomicUsize::new(0));
    let m3 = cadaddy_engine::render::model3::build(&d, &Space::Model, tol, cadaddy_color::Rgb(255, 255, 255), &p, &t);
    assert_eq!(m3.flatten().tris.len() as u64, n_sys, "model.render draws what the window's view draws");
    // It says what it drew: the view, the elements, the switches, the legend.
    let v = view(&mut s, json!({}));
    assert_eq!(sys["drawn"], v["drawn"]);
    assert_eq!(sys["switches"]["branches"], false);
    let legend: BTreeSet<&str> = sys["legend"].as_array().unwrap().iter().filter_map(|l| l["key"].as_str()).collect();
    for k in ["supply_air", "sanitary_vent", "domestic_water", "fire_protection", "electrical_power", "mechanical_equipment"] {
        assert!(legend.contains(k), "{k} in the legend: {legend:?}");
    }
    assert_eq!(sys["legend"], v["legend"], "bim.view and model.render give one legend");
    for l in sys["legend"].as_array().unwrap() {
        assert!(l["count"].as_u64().unwrap() > 0 && l["color"].as_str().is_some_and(|c| c.starts_with('#')), "{l}");
    }
}

#[test]
fn renders_follow_the_view_settings_and_a_view_parameter_changes_nothing() {
    let mut s = building();
    // Before the fix MEP mains and Full MEP rendered byte-identical pictures (every element).
    view(&mut s, json!({"view": "mep"}));
    let mains = render(&mut s, json!({}));
    view(&mut s, json!({"full": true}));
    let full = render(&mut s, json!({}));
    assert_eq!((mains["view"].as_str(), full["view"].as_str()), (Some("mep"), Some("mep")));
    assert!(mains["triangles"].as_u64() < full["triangles"].as_u64(), "mains {} full {}", mains["triangles"], full["triangles"]);
    assert!(differ(&pixels(&mains), &pixels(&full)) > 100, "two views, two pictures");
    // The Systems view with and without its branches.
    view(&mut s, json!({"view": "systems"}));
    let sys = render(&mut s, json!({}));
    view(&mut s, json!({"switches": {"branches": true}}));
    let branches = render(&mut s, json!({}));
    assert!(sys["triangles"].as_u64() < branches["triangles"].as_u64());
    assert!(differ(&pixels(&sys), &pixels(&branches)) > 20);
    view(&mut s, json!({"switches": {"branches": false}}));
    // `view` draws another view without changing the drawing's.
    let arch = render(&mut s, json!({"view": "architectural"}));
    assert_eq!(arch["view"], "architectural");
    assert!(differ(&pixels(&arch), &pixels(&sys)) > 1000);
    assert_eq!(view(&mut s, json!({}))["view"], "systems", "the drawing's view is unchanged");
    let again = render(&mut s, json!({}));
    assert_eq!(again["png"], sys["png"], "the same view, the same picture");
    assert!(s.execute("model.render", &json!({"view": "street"})).is_err());
}

#[test]
fn every_floor_plate_reads_in_the_ghost_and_runs_have_no_dark_edges() {
    let mut s = building();
    let base = pixels(&render(&mut s, json!({})));
    // White ground (reports, sharing).
    assert_eq!(base.get_pixel(1, 1).0, [255, 255, 255, 255]);
    for level in ["L1", "L2", "L3"] {
        s.execute("bim.tree", &json!({"path": [level, "Architecture", "Floors"], "visible": false})).unwrap();
        let without = pixels(&render(&mut s, json!({})));
        s.execute("bim.tree", &json!({"path": [level, "Architecture", "Floors"], "visible": true})).unwrap();
        let n = differ(&base, &without);
        assert!(n > 300, "{level}'s floor plate shows in the ghost: {n} pixels");
    }
    // No black edge lines on the systems: nothing near black in the picture.
    let dark = base.pixels().filter(|p| p.0[0] < 40 && p.0[1] < 40 && p.0[2] < 40).count();
    assert!(dark < 20, "{dark} near-black pixels");
}

#[test]
fn a_thin_run_is_at_least_two_pixels_wide() {
    // A 1/2" conduit feeder 200 ft long: far thinner than a pixel at building scale.
    let mut s = building();
    let r = render(&mut s, json!({"width": 600, "height": 400}));
    let img = pixels(&r);
    let orange = bimviews::Rules::defaults().systems_palette.iter().find(|p| p.key == "electrical_power").unwrap().rgb;
    // Every column the feeders cross holds at least two pixels of their colour in a row.
    let near = |p: &image::Rgba<u8>| p.0.iter().zip(orange).all(|(a, b)| (i32::from(*a) - i32::from(b)).abs() < 70);
    let mut runs = 0;
    for x in 0..img.width() {
        let mut best = 0;
        let mut cur = 0;
        for y in 0..img.height() {
            if near(img.get_pixel(x, y)) {
                cur += 1;
                best = best.max(cur);
            } else {
                cur = 0;
            }
        }
        if best == 1 {
            runs += 1;
        }
    }
    let cols = (0..img.width()).filter(|x| (0..img.height()).any(|y| near(img.get_pixel(*x, y)))).count();
    assert!(cols > 50, "the electrical power runs draw: {cols} columns");
    assert!(runs * 10 < cols, "{runs} of {cols} columns hold a one-pixel run");
}

#[test]
fn objects_off_the_building_are_the_site_and_keep_out_of_the_fit() {
    let mut d = cadaddy_engine::sample::bim_building(3, 6);
    // A pad-mounted unit 400 ft off the building: a site object.
    let e = d.model.iter().find(|e| bimlook::made_tag(e, "tag") == Some("SWB-1")).map(|e| (**e).clone()).unwrap();
    let mut far = e.clone();
    far.kind.transform(&cadaddy_engine::geom::Mat3::translate(cadaddy_engine::geom::Vec2::new(400.0 * 12.0, 0.0)));
    let h = d.add(&Space::Model, far.common.clone(), far.kind.clone()).unwrap();
    let mut s = session_of(d);
    let shown_d = shown(&s);
    assert!(shown_d.model.get(h).is_none(), "off the building: drawn only with the site switch");
    let ext = shown_d.extents(&Space::Model);
    assert!(ext.width() < 300.0 * 12.0, "the view fits the building: {}", ext.width() / 12.0);
    view(&mut s, json!({"switches": {"site": true}}));
    assert!(shown(&s).model.get(h).is_some(), "the site switch draws it");
}

#[test]
fn presentation_is_kept_in_the_look() {
    let mut s = building();
    let r = s.execute("bim.look", &json!({"presentation": true})).unwrap();
    assert_eq!(r["presentation"], true);
    assert_eq!(view(&mut s, json!({}))["presentation"], true);
}

/// A floor plate as the build makes one (Q6665): rectangles (feet) facing up, meeting at
/// T-junctions round a shaft, their undersides a foot below `z` (feet).
fn rect_plate(rects: &[[f64; 4]], z: f64) -> cadaddy_engine::doc::Mesh {
    use cadaddy_engine::doc::{Mesh, MeshFace, MeshKind};
    use cadaddy_engine::geom::Vec3;
    let mut m = Mesh { kind: MeshKind::Polyface, ..Mesh::default() };
    for r in rects {
        let [x0, y0, x1, y1] = r.map(|c| c * 12.0);
        for (zz, up) in [(z * 12.0, true), ((z - 1.0) * 12.0, false)] {
            let base = m.vertices.len() as u32;
            m.vertices.extend([Vec3::new(x0, y0, zz), Vec3::new(x1, y0, zz), Vec3::new(x1, y1, zz), Vec3::new(x0, y1, zz)]);
            let mut v = vec![base, base + 1, base + 2, base + 3];
            if !up {
                v.reverse();
            }
            m.faces.push(MeshFace { v, hidden: 0 });
        }
    }
    m
}

/// A closed box from `lo` to `hi` (feet).
fn box_mesh(lo: [f64; 3], hi: [f64; 3]) -> cadaddy_engine::doc::Mesh {
    use cadaddy_engine::doc::{Mesh, MeshFace, MeshKind};
    use cadaddy_engine::geom::Vec3;
    let [a, b] = [lo, hi].map(|p| p.map(|c| c * 12.0));
    let vertices = vec![
        Vec3::new(a[0], a[1], a[2]),
        Vec3::new(b[0], a[1], a[2]),
        Vec3::new(b[0], b[1], a[2]),
        Vec3::new(a[0], b[1], a[2]),
        Vec3::new(a[0], a[1], b[2]),
        Vec3::new(b[0], a[1], b[2]),
        Vec3::new(b[0], b[1], b[2]),
        Vec3::new(a[0], b[1], b[2]),
    ];
    let faces = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]]
        .into_iter()
        .map(|v| MeshFace { v: v.to_vec(), hidden: 0 })
        .collect();
    Mesh { vertices, faces, kind: MeshKind::Polyface }
}

#[test]
fn the_systems_view_draws_the_same_elements_every_time_and_a_shaft_is_on_site() {
    use cadaddy_engine::doc::{Common, EntityKind, RawTag};
    // Two storeys 62' square, each slab cut into rectangles round a 6' shaft, as the build cuts
    // a floor plate: one rectangle's side runs past two others'.
    const PLATE: [[f64; 4]; 7] = [
        [0.0, 0.0, 30.0, 62.0],
        [30.0, 0.0, 36.0, 20.0],
        [30.0, 26.0, 36.0, 62.0],
        [36.0, 0.0, 62.0, 15.0],
        [36.0, 15.0, 50.0, 40.0],
        [50.0, 15.0, 62.0, 40.0],
        [36.0, 40.0, 62.0, 62.0],
    ];
    let mut d = cadaddy_engine::sample::bim_building(2, 6);
    let floors: Vec<_> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("floor"))
        .map(|e| (e.handle, cadaddy_engine::cmd::bspec::made_level(e).unwrap_or_default()))
        .collect();
    assert_eq!(floors.len(), 2);
    for (h, level) in &floors {
        let z = if level == "L1" { 0.0 } else { 12.0 };
        d.modify_entity(*h, |e| e.kind = EntityKind::Mesh(rect_plate(&PLATE, z))).unwrap();
    }
    // 50 small units spread over each floor, and one standing over the shaft.
    let mut units = BTreeSet::new();
    for (k, level) in ["L1", "L2"].into_iter().enumerate() {
        let z = k as f64 * 12.0;
        let mut at: Vec<[f64; 2]> = (0..50).map(|n| [4.0 + 5.5 * (n % 10) as f64, 6.0 + 11.0 * (n / 10) as f64]).collect();
        at.push([32.5, 22.5]);
        for (n, [x, y]) in at.into_iter().enumerate() {
            let tags = [
                "mep".to_string(),
                level.to_string(),
                "0".to_string(),
                "system:".to_string(),
                "kind:equipment".to_string(),
                "part:equipment:fan_coil".to_string(),
                format!("tag:FC-{level}-{n}"),
                "title:FAN COIL UNIT".to_string(),
            ];
            let mut x_data = vec![RawTag { code: 1001, value: cadaddy_engine::cmd::wallgen::APP.into() }];
            x_data.extend(tags.into_iter().map(|value| RawTag { code: 1000, value }));
            let common = Common { layer: "3D-MEP-EQUIPMENT".into(), xdata: x_data.into(), ..Common::default() };
            let mesh = box_mesh([x, y, z + 8.0], [x + 1.5, y + 1.5, z + 9.0]);
            units.insert(d.add(&Space::Model, common, EntityKind::Mesh(mesh)).unwrap());
        }
    }
    assert_eq!(units.len(), 102);
    let mut s = session_of(d);
    assert_eq!(view(&mut s, json!({}))["view"], "systems");
    let drawn = |s: &Session| -> BTreeSet<cadaddy_engine::doc::Handle> { shown(s).model.iter().map(|e| e.handle).collect() };
    // The same model and view, computed again and again: the same elements, every unit on site.
    let first = drawn(&s);
    let missing: Vec<_> = units.iter().filter(|h| !first.contains(h)).collect();
    assert!(missing.is_empty(), "{} of {} units judged off site", missing.len(), units.len());
    let counts: Vec<Value> = (0..3).map(|_| view(&mut s, json!({}))["drawn"].clone()).collect();
    for _ in 0..5 {
        assert_eq!(drawn(&s), first, "the same input draws the same elements");
    }
    assert!(counts.iter().all(|c| *c == counts[0]), "bim.view drawn {counts:?}");
    let renders: Vec<Value> = (0..3).map(|_| render(&mut s, json!({}))["drawn"].clone()).collect();
    assert!(renders.iter().all(|c| *c == renders[0]), "model.render drawn {renders:?}");
    // The site switch adds nothing of the building: every unit was already drawn.
    view(&mut s, json!({"switches": {"site": true}}));
    let with_site = drawn(&s);
    assert_eq!(units.iter().filter(|h| with_site.contains(h)).count(), units.len());
    assert_eq!(with_site, first, "nothing on the building waits for the site switch");
}
