//! The Systems view's weights (Q6663): at building scale almost every run is thinner than a
//! pixel, so each draws at its role's minimum from the view rules: mains, feeders and risers
//! strong, branch work thin, devices and equipment their own; a palette row's weight makes one
//! trade read stronger; sizes above the minimum stay true to scale. A made-up three-storey
//! building (`sample::bim_building`) with its MEP replaced by one feeder, ten branch conduits,
//! a pipe stack and a duct riser.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook;
use cadaddy_engine::cmd::bimviews::Rules;
use cadaddy_engine::cmd::wallgen::{APP, made_as};
use cadaddy_engine::doc::Lineweight;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_engine::geom::{Vec2, Vec3};
use serde_json::json;

/// Inches a foot (the sample is in inches).
const FT: f64 = 12.0;

/// A box on the centreline `a`–`b` (feet), `t` inches thick, from `z0` to `z1` (feet).
fn run_box(a: [f64; 2], b: [f64; 2], t: f64, z0: f64, z1: f64) -> Mesh {
    let (a, b) = (Vec2::new(a[0] * FT, a[1] * FT), Vec2::new(b[0] * FT, b[1] * FT));
    let b = if a.dist(b) < 1e-9 { a + Vec2::new(t, 0.0) } else { b };
    let u = (b - a) * (1.0 / a.dist(b));
    let n = u.perp() * (t / 2.0);
    let mut vertices = Vec::new();
    for z in [z0 * FT, z1 * FT] {
        for p in [a - n, b - n, b + n, a + n] {
            vertices.push(Vec3::new(p.x, p.y, z));
        }
    }
    let f = |v: [u32; 4]| MeshFace { v: v.to_vec(), hidden: 0 };
    let faces = vec![f([3, 2, 1, 0]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    Mesh { vertices, faces, kind: MeshKind::Polyface }
}

fn mep(d: &mut Drawing, level: &str, kind: &str, system: &str, size: &str, more: &[&str], m: Mesh) {
    let mut x = vec![RawTag { code: 1001, value: APP.into() }];
    for v in ["mep", level, "0", &format!("system:{system}"), &format!("kind:{kind}"), "part:segment", &format!("size:{size}")]
        .into_iter()
        .chain(more.iter().copied())
    {
        x.push(RawTag { code: 1000, value: v.to_string() });
    }
    let layer = format!("3D-MEP-{}", kind.to_ascii_uppercase());
    if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(&layer)) {
        d.layers.push(cadaddy_engine::doc::Layer::new(&layer));
    }
    d.add(&Space::Model, Common { layer: layer.as_str().into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap();
}

/// Where the runs stand (feet): the feeder at y 50, the branches at y 8 … 44 (4 ft apart),
/// all from x 2 to 30 on the top storey; the stack and the riser through every storey.
const FEEDER_Y: f64 = 50.0;
const STACK: [f64; 2] = [5.0, 58.0];
const RISER: [f64; 2] = [25.0, 58.0];

/// The three-storey sample with only these systems: a 4 in feeder, ten 1/2 in branch
/// conduits, a 4 in waste stack and a 4 in exhaust duct riser.
fn building() -> Session {
    let mut d = cadaddy_engine::sample::bim_building(3, 2);
    let old: Vec<_> = d.model.iter().filter(|e| made_as(e).as_deref() == Some("mep")).map(|e| e.handle).collect();
    for h in old {
        d.remove_entity(h);
    }
    let z = 24.0;
    mep(&mut d, "L3", "conduit", "E-FEEDER", "4", &[], run_box([2.0, FEEDER_Y], [30.0, FEEDER_Y], 4.0, z + 10.0, z + 10.33));
    for k in 0..10 {
        let y = 8.0 + 4.0 * f64::from(k);
        mep(&mut d, "L3", "conduit", "E-BRANCH", "0.5", &[], run_box([2.0, y], [30.0, y], 0.5, z + 10.0, z + 10.04));
    }
    mep(&mut d, "L1", "pipe", "SAN", "4", &[], run_box(STACK, STACK, 4.0, -1.0, 37.0));
    mep(&mut d, "L1", "duct", "EA", "4", &["duct_type:exhaust"], run_box(RISER, RISER, 4.0, 1.0, 37.0));
    let mut s = Session::new();
    s.open_drawing(d, "Sample Building", None);
    s.execute("bim.view", &json!({"view": "systems", "switches": {"branches": true}})).unwrap();
    s
}

/// The picture's size: about 2.9 in of building a pixel (a 4 in run is under one and a half
/// pixels, a 1/2 in one a fifth of a pixel), the scale of a whole building on a screen.
const SIZE: u32 = 260;

/// The building from above at `px` square.
fn top(s: &mut Session, px: u32) -> image::RgbaImage {
    let r = s.execute("model.render", &json!({"width": px, "height": px, "dir": [0.0, 0.0, 1.0], "ghost": true})).unwrap();
    let b = cadaddy_engine::cmd::file::base64_decode(r["png"].as_str().expect("a png")).expect("base64");
    image::load_from_memory(&b).expect("a png").to_rgba8()
}

/// How strongly coloured a pixel is (a system against the pale shell and the white page).
fn chroma(p: &image::Rgba<u8>) -> u8 {
    let c = &p.0[..3];
    c.iter().max().copied().unwrap_or(0) - c.iter().min().copied().unwrap_or(0)
}

/// The bands of system colour along a line of pixels: each band's strong pixels (at least
/// 60 % of the band's strongest colour: a half-covered fringe doesn't count as width).
fn bands(line: &[image::Rgba<u8>]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < line.len() {
        if chroma(&line[k]) < 50 {
            k += 1;
            continue;
        }
        let start = k;
        while k < line.len() && chroma(&line[k]) >= 50 {
            k += 1;
        }
        let band = &line[start..k];
        let peak = band.iter().map(chroma).max().unwrap_or(0);
        out.push(band.iter().filter(|p| f64::from(chroma(p)) >= 0.6 * f64::from(peak)).count());
    }
    out
}

/// The picture's column at `fx` of its width, top to bottom.
fn column(img: &image::RgbaImage, fx: f64) -> Vec<image::Rgba<u8>> {
    let x = ((f64::from(img.width()) * fx) as u32).min(img.width() - 1);
    (0..img.height()).map(|y| *img.get_pixel(x, y)).collect()
}

/// The widest band of system colour on any row through the strip `y0..y1` of the picture
/// (fractions of its height) in its columns `x0..x1` (fractions of its width).
fn widest_across(img: &image::RgbaImage, x0: f64, x1: f64, y0: f64, y1: f64) -> usize {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    let (xa, xb) = ((w * x0) as u32, ((w * x1) as u32).min(img.width()));
    ((h * y0) as u32..((h * y1) as u32).min(img.height()))
        .map(|y| {
            let row: Vec<image::Rgba<u8>> = (xa..xb).map(|x| *img.get_pixel(x, y)).collect();
            bands(&row).into_iter().max().unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}

/// Where the building's plan is in the picture: the fractions of its width and height (top
/// down) of plan point (x, y) in feet, from where the coloured runs are drawn.
fn plan_frac(img: &image::RgbaImage, at: [f64; 2]) -> (f64, f64) {
    // The runs span x 2 … 30 ft and the branches' and feeder's y 8 … 50 ft: find them.
    let (w, h) = (img.width(), img.height());
    let coloured = |x: u32, y: u32| chroma(img.get_pixel(x, y)) >= 50;
    let rows: Vec<u32> = (0..h).filter(|y| (0..w).filter(|x| coloured(*x, *y)).count() > (w as usize) / 8).collect();
    let cols: Vec<u32> = (0..w).filter(|x| rows.iter().filter(|y| coloured(*x, **y)).count() >= 6).collect();
    let (ylo, yhi) = (*rows.first().unwrap() as f64, *rows.last().unwrap() as f64);
    let (xlo, xhi) = (*cols.first().unwrap() as f64, *cols.last().unwrap() as f64);
    // y up the plan is up the picture: y 50 at the top row, 8 at the bottom.
    let fx = (xlo + (at[0] - 2.0) / 28.0 * (xhi - xlo)) / f64::from(w);
    let fy = (yhi - (at[1] - 8.0) / 42.0 * (yhi - ylo)) / f64::from(h);
    (fx, fy)
}

/// The finding's reproduction: a 4 in feeder, 1/2 in branch conduits, a 4 in waste stack and
/// a 4 in duct riser, all thinner than a pixel at this scale, drew at one width. Now each
/// draws at its role's minimum: the feeder, the stack and the riser at the mains' (4 px), the
/// branches at the branches' (1.5 px: one strong pixel and a soft fringe).
#[test]
fn mains_feeders_and_risers_draw_stronger_than_branch_work() {
    let mut s = building();
    let img = top(&mut s, SIZE);
    let found = bands(&column(&img, 0.5));
    // The stack and the riser (upright, seen end-on).
    let ends: Vec<usize> = [STACK, RISER]
        .into_iter()
        .map(|at| {
            let (fx, fy) = plan_frac(&img, at);
            widest_across(&img, fx - 0.04, fx + 0.04, fy - 0.04, fy + 0.04)
        })
        .collect();
    eprintln!("feeder, branches: {found:?}; stack, riser: {ends:?}");
    assert_eq!(found.len(), 11, "the feeder and ten branches across the middle: {found:?}");
    // The feeder at y 50 is the top band.
    let feeder = found[0];
    let branches = &found[1..];
    assert!(feeder >= 4, "the feeder at the mains' weight: {found:?}");
    assert!(branches.iter().all(|b| *b <= 2), "branch work thin: {found:?}");
    assert!(feeder >= branches.iter().max().unwrap() + 2, "mains fat, branches thin: {found:?}");
    assert!(ends.iter().all(|w| *w >= 4), "the stack and the riser at the mains' weight: {ends:?}");
}

/// The weights are the project's (bim.view.rules): a role's minimum, and a palette row's
/// weight that makes one trade read stronger than the others.
#[test]
fn role_and_palette_row_weights_are_project_rules() {
    let r = Rules::defaults();
    for k in ["main", "branch", "device", "equipment"] {
        assert!(r.systems_weights.contains_key(k), "{k} has a weight: {:?}", r.systems_weights);
    }
    assert!(r.systems_weights["main"] > r.systems_weights["branch"], "{:?}", r.systems_weights);
    // A thicker branch minimum: the branches thicken, the feeder keeps the built-in main
    // weight (a project names only the roles it changes).
    let mut s = building();
    let before = bands(&column(&top(&mut s, SIZE), 0.5));
    let r2 = s.execute("bim.view.rules", &json!({"rules": {"systems_weights": {"branch": 3}}})).unwrap();
    assert_eq!(r2["rules"]["systems_weights"]["main"], json!(4.0), "{r2}");
    let after = bands(&column(&top(&mut s, SIZE), 0.5));
    assert_eq!(after.len(), 11, "{after:?}");
    assert!(after[1..].iter().all(|b| *b >= 3), "branches at 3 px: {before:?} → {after:?}");
    assert_eq!(after[0], before[0], "the feeder as it was: {before:?} → {after:?}");
    assert!(s.execute("bim.view.rules", &json!({"rules": {"systems_weights": {"branch": "thin"}}})).is_err());
    // An agent sets them too (the doc names the map with its object note).
    let r3 = s.execute_agent("bim.view.rules", &json!({"rules": {"systems_weights": {"device": 3}}})).unwrap();
    assert_eq!(r3["rules"]["systems_weights"]["device"], json!(3.0), "{r3}");
    // The electrical power row twice as strong: its branches double.
    let mut s = building();
    let mut rows = serde_json::to_value(&r.systems_palette).unwrap();
    for row in rows.as_array_mut().unwrap() {
        if row["key"] == "electrical_power" {
            row["weight"] = json!(2.0);
        }
    }
    s.execute("bim.view.rules", &json!({"rules": {"systems_palette": rows}})).unwrap();
    let strong = bands(&column(&top(&mut s, SIZE), 0.5));
    assert!(strong[1..].iter().all(|b| *b >= 3), "electrical power read stronger: {before:?} → {strong:?}");
    assert!(strong[0] >= 7, "the feeder doubled too: {strong:?}");
}

/// What carries a run's weight: the painted element's lineweight (CAD's own display width,
/// 0.2 mm a pixel), set by its role and its palette row; the shell keeps none.
#[test]
fn each_drawn_system_carries_its_role_weight() {
    let s = building();
    let d = bimlook::display(s.doc().unwrap()).expect("the Systems view");
    let r = Rules::defaults();
    let px = |e: &cadaddy_engine::doc::Entity| match e.common.lineweight {
        Lineweight::Mm100(v) => Some(f64::from(v) / cadaddy_engine::render::bimview::SYSTEMS_MM100_PER_PX),
        _ => None,
    };
    let near = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 0.06);
    let mut seen = 0;
    for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some("mep")) {
        let system = bimlook::made_tag(e, "system").unwrap_or("");
        let want = if system == "E-BRANCH" { r.systems_weights["branch"] } else { r.systems_weights["main"] };
        assert!(near(px(e), want), "{system}: {:?} px, want {want}", px(e));
        seen += 1;
    }
    assert_eq!(seen, 13);
    for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some("floor")) {
        assert!(px(e).is_none(), "the shell has no weight");
    }
}
