//! Q6660 fix-level-view-clips-storey: a level view is the storey, what stands between the
//! level's floor and the floor above, from every level. A run kept as one element from its
//! bottom level to the roof (a model built before the riser cut, or a riser kept whole) is cut
//! at the floor above, and the piece of another level's run standing in the storey is drawn;
//! the same in the window's Floor view and in `model.render`, which also honours a section box.
//! A made-up building (`sample::bim_building`, 12' storeys) with two whole risers added.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Handle, Mesh, MeshFace, RawTag, Space};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Inches a foot (the sample model is in inches).
const FT: f64 = 12.0;

/// A box (feet) tagged as the build tags an MEP run: `mep`, its level, its source, its system,
/// kind and part.
fn run_box(d: &mut Drawing, level: &str, kind: &str, system: &str, size: &str, lo: [f64; 3], hi: [f64; 3]) -> Handle {
    let v = |x: usize, y: usize, z: usize| Vec3::new([lo[0], hi[0]][x] * FT, [lo[1], hi[1]][y] * FT, [lo[2], hi[2]][z] * FT);
    let vertices = vec![v(0, 0, 0), v(1, 0, 0), v(1, 1, 0), v(0, 1, 0), v(0, 0, 1), v(1, 0, 1), v(1, 1, 1), v(0, 1, 1)];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([2, 3, 7, 6]), f([1, 2, 6, 5]), f([3, 0, 4, 7])];
    let mut x = vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }];
    for t in [
        "mep".to_string(),
        level.to_string(),
        "0".to_string(),
        format!("system:{system}"),
        format!("kind:{kind}"),
        "part:segment".into(),
        format!("size:{size}"),
    ] {
        x.push(RawTag { code: 1000, value: t });
    }
    let layer = format!("3D-MEP-{}", kind.to_ascii_uppercase());
    if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&layer)) {
        d.layers.push(cadaddy_engine::doc::Layer::new(&layer));
    }
    d.add(
        &Space::Model,
        Common { layer: layer.into(), xdata: x.into(), ..Common::default() },
        EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }),
    )
    .expect("added")
}

/// Four storeys (floors at 0', 12', 24', 36'), and two risers each kept as one run: a supply
/// duct shaft given on L1 from 1' to 50' (past the top floor's roof), and a sanitary stack
/// given on L2 from 13' to 50'.
fn building() -> (Session, Handle, Handle) {
    let mut d = cadaddy_engine::sample::bim_building(4, 2);
    let duct = run_box(&mut d, "L1", "duct", "SA", "24x12", [24.0, 30.0, 1.0], [26.0, 31.0, 50.0]);
    let pipe = run_box(&mut d, "L2", "pipe", "SAN", "4", [29.5, 30.5, 13.0], [30.0, 31.0, 50.0]);
    let mut s = Session::new();
    s.open_drawing(d, "Sample Building", None);
    (s, duct, pipe)
}

fn view(s: &mut Session, p: Value) -> Value {
    s.execute("bim.view", &p).unwrap_or_else(|e| panic!("bim.view {p}: {e}"))
}

/// The height (feet) an object of the drawn view spans; none when it isn't drawn.
fn span(d: &Drawing, h: Handle) -> Option<(f64, f64)> {
    let e = d.model.get(h)?;
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let zs = m.faces.iter().flat_map(|f| f.v.iter()).filter_map(|i| m.vertices.get(*i as usize)).map(|v| v.z / FT);
    let (lo, hi) = zs.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), z| (a.min(z), b.max(z)));
    (lo <= hi).then_some((lo, hi))
}

fn near(a: Option<(f64, f64)>, want: (f64, f64)) -> bool {
    a.is_some_and(|(lo, hi)| (lo - want.0).abs() < 1e-6 && (hi - want.1).abs() < 1e-6)
}

/// The highest point (feet) of any built object the view draws.
fn top_of(d: &Drawing) -> f64 {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Mesh(m) => m.vertices.iter().map(|v| v.z / FT).reduce(f64::max),
            _ => None,
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

#[test]
fn the_floor_view_of_a_level_is_its_storey() {
    let (mut s, duct, pipe) = building();
    // L2 (12' to 24'): its own stack cut at L3's floor; L1's shaft's piece in the storey.
    view(&mut s, json!({"view": "floor", "level": "L2"}));
    let d = bimlook::display(s.doc().unwrap()).expect("the BIM view of a built model");
    let (p, u) = (span(&d, pipe), span(&d, duct));
    eprintln!("L2 floor view: L2 stack {p:?}, L1 shaft {u:?}, highest point {:.2}'", top_of(&d));
    assert!(near(p, (13.0, 24.0)), "the L2 stack is cut at the floor above: {p:?}");
    assert!(near(u, (12.0, 24.0)), "the L1 shaft's piece standing in L2 is drawn: {u:?}");
    assert!(top_of(&d) <= 24.0 + 1e-6, "nothing over the floor above: {}", top_of(&d));
    // The top storey has no floor above: what stands over it stays.
    view(&mut s, json!({"view": "floor", "level": "L4"}));
    let d = bimlook::display(s.doc().unwrap()).unwrap();
    assert!(near(span(&d, duct), (36.0, 50.0)), "{:?}", span(&d, duct));
    assert!(near(span(&d, pipe), (36.0, 50.0)), "{:?}", span(&d, pipe));
    // L1: its shaft cut at L2's floor; L2's stack is not in L1's storey.
    view(&mut s, json!({"view": "floor", "level": "L1"}));
    let d = bimlook::display(s.doc().unwrap()).unwrap();
    assert!(near(span(&d, duct), (1.0, 12.0)), "{:?}", span(&d, duct));
    assert_eq!(span(&d, pipe), None, "L2's stack stands over L1's storey");
    // The other storeys' own objects are left out (L3's mains run at 24' + 9.5').
    assert!(top_of(&d) <= 12.0 + 1e-6, "{}", top_of(&d));
}

fn render(s: &mut Session, extra: Value) -> Value {
    let mut p = json!({"width": 800, "height": 600, "dir": [0.0, -1.0, 0.02]});
    if let (Some(o), Some(x)) = (p.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            o.insert(k.clone(), v.clone());
        }
    }
    s.execute("model.render", &p).unwrap_or_else(|e| panic!("model.render {p}: {e}"))
}

/// The drawn part of a render (any pixel that isn't white): its height over its width.
fn tallness(r: &Value) -> f64 {
    let b = cadaddy_engine::cmd::file::base64_decode(r["png"].as_str().expect("a png")).expect("base64");
    let img = image::load_from_memory(&b).expect("a png").to_rgba8();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[..3].iter().any(|c| *c < 245) {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    assert!(x1 >= x0 && y1 >= y0, "something is drawn");
    f64::from(y1 - y0 + 1) / f64::from(x1 - x0 + 1)
}

#[test]
fn model_render_of_a_level_is_its_storey() {
    let (mut s, _, _) = building();
    // Seen from the south, L2's storey (12' and its slab under it, 34' long) is far wider than
    // tall; the whole risers to 50' would make the picture taller than wide.
    let mut wrong = Vec::new();
    for extra in [
        json!({"level": "L2"}),
        json!({"level": "L2", "view": "systems"}),
        json!({"level": "L2", "view": "floor"}),
        json!({"level": "L2", "style": "shaded"}),
    ] {
        let r = render(&mut s, extra.clone());
        let t = tallness(&r);
        eprintln!("model.render {extra}: drawn part height/width {t:.3}, storey {}", r["storey"]);
        if t >= 0.6 {
            wrong.push(format!("{extra}: the picture is the risers to the roof, not the storey ({t:.3})"));
        }
        if r["storey"]["top_ft"] != json!(24.0) {
            wrong.push(format!("{extra}: storey {}", r["storey"]));
        }
    }
    // Only round L1's shaft on plan: L2's picture holds the shaft's piece in its storey (more
    // triangles than the same building without the shaft).
    let mut without = Session::new();
    without.open_drawing(cadaddy_engine::sample::bim_building(4, 2), "Sample Building", None);
    for extra in [json!({"level": "L2", "style": "shaded"}), json!({"level": "L2"}), json!({"level": "L2", "view": "floor"})] {
        let mut p = extra.clone();
        p["around"] = json!([23.0 * FT, 29.0 * FT, 27.0 * FT, 32.0 * FT]);
        let (with, bare) = (render(&mut s, p.clone())["triangles"].as_u64().unwrap_or(0), render(&mut without, p)["triangles"].as_u64().unwrap_or(0));
        eprintln!("model.render {extra} round the L1 shaft: {with} triangles, {bare} without the shaft");
        if with <= bare {
            wrong.push(format!("{extra}: L1's shaft is not drawn in L2's storey ({with} triangles, {bare} without it)"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn model_render_honours_a_section_box() {
    let (mut s, _, _) = building();
    let whole = tallness(&render(&mut s, json!({"style": "shaded"})));
    let sbox = json!({"min": [-1000.0, -1000.0, 12.0 * FT], "max": [10000.0, 10000.0, 24.0 * FT]});
    let cut = tallness(&render(&mut s, json!({"style": "shaded", "section": sbox})));
    eprintln!("whole building {whole:.3}, in a 12' section box {cut:.3}");
    assert!(whole > 1.0 && cut < 0.6, "whole {whole:.3}, section {cut:.3}");
    // The coordination section box, while it is on; `section: false` draws without it.
    s.execute("coordination.sectionbox", &json!({"on": true, "min": [-1000.0, -1000.0, 12.0 * FT], "max": [10000.0, 10000.0, 24.0 * FT]})).unwrap();
    assert!(tallness(&render(&mut s, json!({"style": "shaded"}))) < 0.6);
    assert!(tallness(&render(&mut s, json!({"style": "shaded", "section": false}))) > 1.0);
    assert!(s.execute("model.render", &json!({"section": {"min": [0, 0]}})).is_err(), "a box needs both corners");
}
