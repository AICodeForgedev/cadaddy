//! Q6497: a building whose fire protection is a deferred submittal has no FP sheets in its design
//! set; the sprinkler shop drawings among its submittals are read like FP sheets. The made-up set
//! here: two levels of architectural plans, and a shop drawing of the second floor drawn at
//! another scale and place on its own page (the floor's background in grey, its room numbers;
//! heads typed by the page's legend, hatched residential pendents and plain sidewalls; branch line
//! and main as filled bars of two weights with size labels; a head table that disagrees on one
//! row). The job puts the page onto its floor by the room numbers, builds the heads and the pipe,
//! and says what the table disagrees on. With no FP source at all, the trade is said missing.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Hatch, HatchLoop, Space, Text};
use cadaddy_engine::geom::{PolyVertex, Vec2, Vec3};
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn real_pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn folder(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_fpshop_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(dir.join("Drawings")).expect("folder");
    dir
}

/// The page is the floor plan's sheet scaled by this and moved by [`SHIFT`] (paper inches).
const K: f64 = 0.8;
const SHIFT: [f64; 2] = [3.0, 2.0];
fn tr(p: [f64; 2]) -> [f64; 2] {
    [p[0] * K + SHIFT[0], p[1] * K + SHIFT[1]]
}

fn add(d: &mut Drawing, colour: u8, kind: EntityKind) {
    let c = Common { color: cadaddy_engine::doc::color::Color::Index(colour), ..Common::default() };
    d.add(&Space::Model, c, kind).expect("add");
}
fn line(d: &mut Drawing, colour: u8, a: [f64; 2], b: [f64; 2]) {
    add(d, colour, EntityKind::Line(cadaddy_engine::doc::Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }));
}
fn text(d: &mut Drawing, at: [f64; 2], t: &str, h: f64) {
    add(
        d,
        7,
        EntityKind::Text(Text {
            insert: Vec3::new(at[0], at[1], 0.0),
            align_pt: None,
            height: h,
            value: t.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        }),
    );
}
/// A filled bar `w` wide from `a` to `b`, in red.
fn bar(d: &mut Drawing, a: [f64; 2], b: [f64; 2], w: f64) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx.hypot(dy);
    let (nx, ny) = (-dy / l * w / 2.0, dx / l * w / 2.0);
    let pts = [[a[0] + nx, a[1] + ny], [b[0] + nx, b[1] + ny], [b[0] - nx, b[1] - ny], [a[0] - nx, a[1] - ny]];
    let vertices = pts.iter().map(|p| PolyVertex { p: Vec2::new(p[0], p[1]), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
    let h = Hatch {
        pattern: "SOLID".into(),
        solid: true,
        loops: vec![HatchLoop { vertices, outer: true, sources: Vec::new(), curves: Vec::new() }],
        scale: 1.0,
        angle: 0.0,
        associative: false,
        style: 0,
        elevation: 0.0,
        gradient: None,
        origin: Vec2::ZERO,
        background: None,
        pattern_lines: Vec::new(),
    };
    add(d, 1, EntityKind::Hatch(Box::new(h)));
}
const R: f64 = 0.06;
/// A head: a red circle, hatched (three strokes) for a residential pendent.
fn head(d: &mut Drawing, c: [f64; 2], hatched: bool) {
    add(d, 1, EntityKind::Circle(cadaddy_engine::doc::Circle { center: Vec3::new(c[0], c[1], 0.0), radius: R }));
    if hatched {
        for off in [-0.5, 0.0, 0.5] {
            let o = off * R;
            line(d, 1, [c[0] - 0.6 * R + o, c[1] - 0.6 * R - o], [c[0] + 0.6 * R + o, c[1] + 0.6 * R - o]);
        }
    }
}

/// The room numbers a sheet prints (three or four digits) at their middles, each once.
fn room_numbers(d: &Drawing) -> Vec<(String, [f64; 2], f64, [f64; 2])> {
    let mut all: Vec<(String, [f64; 2], f64, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if let EntityKind::Text(t) = &e.kind
            && let Some(k) = cadaddy_mepfix::tagid::tag_key(&t.value)
        {
            let w = 0.6 * t.height * t.value.trim().chars().count() as f64;
            all.push((k, [t.insert.x, t.insert.y], t.height, [t.insert.x + w / 2.0, t.insert.y + t.height / 2.0]));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    all.retain(|a| seen.insert(a.0.clone()));
    all
}

/// The set (`A-101`, `A-102`) as one PDF in `Drawings/`, and `A-102` read back at paper size with
/// its printed scale (paper inches a foot).
fn design_set(dir: &Path) -> (Drawing, f64) {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| ["A-101", "A-102"].contains(&s.number.as_str()));
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Drawings").join("Sample Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    let second = set.truth.sheets.iter().find(|s| s.number == "A-102").expect("A-102");
    let bytes = set.file(&second.pdf).expect("pdf").to_vec();
    let scale = second.scale_text.as_deref().and_then(cadaddy_mepfix::sheet_reader::parse_scale).expect("A-102 prints its scale");
    (cadaddy_io::pdf_read::read_json(&bytes, &json!({"scale": 1, "layout": false, "images": false})).expect("read A-102").0, scale)
}

/// What the shop drawing draws, in the floor plan's paper inches (before [`tr`]).
struct Drawn {
    heads: Vec<[f64; 2]>,
    main: ([f64; 2], [f64; 2]),
}

/// The second floor's sprinkler shop drawing as a PDF: its background in grey and its room
/// numbers, both carried by [`tr`]; five heads below five room numbers (four hatched pendents
/// and a plain sidewall), the branch line head to head, a main teed into it, labels, the legend,
/// a head table (4 pendents, right; 2 sidewalls, wrong) and a title naming no level.
fn shop_drawing(plan: &Drawing) -> (Vec<u8>, Drawn) {
    let mut d = Drawing::new_imperial();
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for e in plan.model.iter() {
        let pieces: Vec<([f64; 2], [f64; 2])> = match &e.kind {
            EntityKind::Line(l) => vec![([l.a.x, l.a.y], [l.b.x, l.b.y])],
            EntityKind::LwPolyline(pl) => {
                let mut v: Vec<[f64; 2]> = pl.vertices.iter().map(|x| [x.p.x, x.p.y]).collect();
                if pl.closed
                    && let Some(f) = v.first().copied()
                {
                    v.push(f);
                }
                v.windows(2).map(|w| (w[0], w[1])).collect()
            }
            _ => Vec::new(),
        };
        for (a, b) in pieces {
            for p in [a, b] {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
            line(&mut d, 8, tr(a), tr(b));
        }
    }
    let tags = room_numbers(plan);
    assert!(tags.len() >= 6, "the made-up plan prints room numbers: {tags:?}");
    for (k, at, h, _) in &tags {
        text(&mut d, tr(*at), k, h * K);
    }
    // Five heads, each a little below a room number, along the sheet left to right.
    let mut picked: Vec<[f64; 2]> = tags.iter().map(|t| t.3).collect();
    picked.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let step = picked.len() / 5;
    let heads: Vec<[f64; 2]> = (0..5).filter_map(|i| picked.get(i * step.max(1))).map(|c| [c[0], c[1] - 0.3]).collect();
    assert_eq!(heads.len(), 5);
    for (i, h) in heads.iter().enumerate() {
        head(&mut d, tr(*h), i < 4);
    }
    for w in heads.windows(2) {
        bar(&mut d, tr(w[0]), tr(w[1]), 0.02);
    }
    // The main: under the first head, a nipple up to it.
    let first = heads[0];
    let m0 = [first[0], first[1] - 0.8];
    let m1 = [m0[0] + 2.5, m0[1]];
    bar(&mut d, tr(m0), tr(first), 0.02);
    bar(&mut d, tr([m0[0] - 0.03, m0[1]]), tr(m1), 0.05);
    let lm = tr([m0[0] + 1.0, m0[1] + 0.04]);
    text(&mut d, lm, "2-1/2\"", 0.07);
    let mid = tr([(heads[0][0] + heads[1][0]) / 2.0, (heads[0][1] + heads[1][1]) / 2.0]);
    text(&mut d, [mid[0], mid[1] + 0.03], "1\"", 0.07);
    // Legend left of the plan, table below it, title.
    let (lx, ly) = (tr(lo)[0] - 5.0, tr(hi)[1] - 0.5);
    text(&mut d, [lx, ly], "SPRINKLER LEGEND", 0.12);
    head(&mut d, [lx + 0.1, ly - 0.45], true);
    text(&mut d, [lx + 0.4, ly - 0.5], "QUICK RESPONSE RESIDENTIAL PENDENT", 0.1);
    head(&mut d, [lx + 0.1, ly - 0.75], false);
    text(&mut d, [lx + 0.4, ly - 0.8], "HORIZONTAL SIDEWALL", 0.1);
    let (tx, ty) = (tr(lo)[0], tr(lo)[1] - 1.0);
    text(&mut d, [tx, ty], "SPRINKLER HEAD SCHEDULE", 0.12);
    text(&mut d, [tx, ty - 0.35], "TYPE", 0.1);
    text(&mut d, [tx + 5.0, ty - 0.35], "TEMP", 0.1);
    text(&mut d, [tx + 6.0, ty - 0.35], "QTY", 0.1);
    for (i, (label, n)) in [("QUICK RESPONSE RESIDENTIAL PENDENT", "4"), ("HORIZONTAL SIDEWALL", "2")].iter().enumerate() {
        let y = ty - 0.65 - 0.3 * i as f64;
        text(&mut d, [tx, y], label, 0.1);
        text(&mut d, [tx + 5.0, y], "155", 0.1);
        text(&mut d, [tx + 6.0, y], n, 0.1);
    }
    text(&mut d, [tr(hi)[0] - 6.0, tr(lo)[1] - 1.0], "FIRE SPRINKLER SHOP DRAWING", 0.15);
    let opts =
        json!({"layers": false, "fit": false, "center": true, "scale": 1.0, "width": 864, "height": 1118, "landscape": true, "lineweights": true});
    let pdf = cadaddy_io::pdf::plot(&d, &Space::Model, &opts).expect("plot");
    (pdf, Drawn { heads, main: (m0, m1) })
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).as_deref().unwrap_or("{}")).expect("job state")
}

fn stage<'a>(job: &'a Value, name: &str) -> &'a Value {
    job["guided_job"]["stages"].as_array().and_then(|a| a.iter().find(|s| s["name"] == name)).unwrap_or_else(|| panic!("stage {name}"))
}

fn run(dir: &Path, submittals: Option<&Path>) -> Session {
    let mut s = Session::new();
    let mut p = json!({"pdfs": dir.join("Drawings").display().to_string(), "goal": "bim", "auto": true, "checkpoint": false});
    if let Some(sub) = submittals {
        p["submittals"] = json!([sub.display().to_string()]);
    }
    let st = s.execute("job.start", &p).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    s
}

/// The built fire protection objects: by part (`part:terminal:sprinkler_pendent`), and their places.
fn built(s: &Session) -> (std::collections::BTreeMap<String, usize>, Vec<[f64; 2]>) {
    let mut parts = std::collections::BTreeMap::new();
    let mut heads = Vec::new();
    for e in s.doc().expect("doc").model.iter() {
        if !e.common.layer.to_string().starts_with("3D-MEP-FP-") {
            continue;
        }
        let Some(part) = e.common.xdata.iter().find(|t| t.value.starts_with("part:")).map(|t| t.value.clone()) else { continue };
        if part.starts_with("part:terminal:") {
            let b = cadaddy_engine::doc::entity_bounds(s.doc().expect("doc"), e, 0);
            heads.push([(b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0]);
        }
        *parts.entry(part).or_default() += 1;
    }
    (parts, heads)
}

fn findings(job: &Value) -> Vec<String> {
    job["guided_job"]["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| format!("{} — {}", f["description"].as_str().unwrap_or(""), f["why"].as_str().unwrap_or("")))
        .collect()
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

#[test]
fn a_deferred_submittals_sprinkler_shop_drawing_is_read_onto_its_floor_and_built() {
    let _g = serial();
    real_pdf_reader();
    let dir = folder("built");
    let (plan, a_in_per_ft) = design_set(&dir);
    let (pdf, drawn) = shop_drawing(&plan);
    let subs = dir.join("Submittals");
    std::fs::create_dir_all(&subs).expect("submittals");
    std::fs::write(subs.join("Fire Sprinkler Shop Drawings.pdf"), pdf).expect("write shop drawing");
    let s = run(&dir, Some(&subs));
    let job = job_state(&s);
    let read = stage(&job, "mep_read")["result"]["fire_protection"].clone();
    eprintln!("fire protection read: {read:#}");
    // Five heads, typed by the legend; the legend and table symbols aren't counted.
    assert_eq!(read["heads"], json!(5), "{read:#}");
    assert_eq!(read["heads_by_type"], json!({"QUICK RESPONSE RESIDENTIAL PENDENT": 4, "HORIZONTAL SIDEWALL": 1}), "{read:#}");
    let src = &read["sources"][0];
    assert_eq!(src["kind"], json!("shop_drawing"), "{read:#}");
    // Put onto the second floor's plan by its room numbers (the title names no level), at the
    // page's own scale: K times the plan's printed scale.
    let placed = &src["placed"];
    assert_eq!(placed["onto"], json!("A-102"), "{placed:#}");
    assert_eq!(src["level"], json!("Level 2"), "{placed:#}");
    assert!(placed["matched"].as_u64().unwrap_or(0) >= 6, "{placed:#}");
    let page_scale = placed["page_in_per_ft"].as_f64().unwrap_or(0.0);
    assert!((page_scale - K * a_in_per_ft).abs() < 0.01 * K * a_in_per_ft, "{placed:#} vs {}", K * a_in_per_ft);
    assert!(placed["rotation_deg"].as_f64().unwrap_or(9.0).abs() < 0.5, "{placed:#}");
    assert!(placed["residual_ft"].as_f64().unwrap_or(9.0) < 1.0, "{placed:#}");
    assert_eq!(placed["walls"]["checked"], json!(true), "the walls checked the fit: {placed:#}");
    assert!(placed["walls"]["coverage"].as_f64().unwrap_or(0.0) > 0.5, "{placed:#}");
    // The branch line (1", labelled) and the main (2-1/2", labelled), their lengths as drawn (the
    // plan's paper inches at its printed scale).
    let ft_per_in = 1.0 / a_in_per_ft;
    let branch_in: f64 = drawn.heads.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>() + 0.8;
    let main_in = dist(drawn.main.0, drawn.main.1) + 0.03;
    let by = &src["pipe_by_size"];
    let one = by["1\""].as_f64().unwrap_or(0.0);
    let main = by["2-1/2\""].as_f64().unwrap_or(0.0);
    assert!((one - branch_in * ft_per_in).abs() <= 0.05 * branch_in * ft_per_in, "{by} vs {}", branch_in * ft_per_in);
    assert!((main - main_in * ft_per_in).abs() <= 0.05 * main_in * ft_per_in, "{by} vs {}", main_in * ft_per_in);
    assert_eq!(by.as_object().map(|o| o.len()), Some(2), "the grey background walls are no pipe: {by}");
    // The head table: pendents agree, the sidewalls don't, and the job says so.
    assert_eq!(src["table_differs"], json!(true), "{src:#}");
    let f = findings(&job);
    assert!(f.iter().any(|x| x.contains("differ from the head table") && x.contains("HORIZONTAL SIDEWALL: table 2, drawn 1")), "{f:#?}");
    assert!(!f.iter().any(|x| x.starts_with("No fire protection")), "{f:#?}");
    // Built: four pendents, a sidewall and the pipe, in the building.
    let (parts, heads) = built(&s);
    assert_eq!(parts.get("part:terminal:sprinkler_pendent"), Some(&4), "{parts:?}");
    assert_eq!(parts.get("part:terminal:sprinkler_sidewall"), Some(&1), "{parts:?}");
    assert!(parts.iter().any(|(p, n)| p == "part:segment" && *n >= 6), "{parts:?}");
    // Rigid: the heads lie as far apart in the model as the page draws them.
    let upf = 12.0;
    for (i, j) in [(0usize, 1usize), (0, 4), (2, 3)] {
        let (Some(a), Some(b), Some(p), Some(q)) = (heads.get(i), heads.get(j), drawn.heads.get(i), drawn.heads.get(j)) else { continue };
        let model_ft = dist(*a, *b) / upf;
        let page_ft = dist(*p, *q) * ft_per_in;
        assert!((model_ft - page_ft).abs() < 1.0, "heads {i}-{j}: model {model_ft:.2} ft, page {page_ft:.2} ft");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shop_drawings_in_a_submittals_folder_beside_the_prints_are_found_and_no_source_is_said_missing() {
    let _g = serial();
    real_pdf_reader();
    // None anywhere: the trade is missing, and the job says so.
    let dir = folder("missing");
    let (plan, _) = design_set(&dir);
    let s = run(&dir, None);
    let job = job_state(&s);
    let read = stage(&job, "mep_read")["result"]["fire_protection"].clone();
    assert_eq!(read["missing"], json!(true), "{read:#}");
    assert_eq!(read["heads"], json!(0));
    let f = findings(&job);
    assert!(f.iter().any(|x| x.starts_with("No fire protection in the documents")), "{f:#?}");
    let (parts, _) = built(&s);
    assert!(parts.is_empty(), "{parts:?}");
    // The same set with the shop drawing in a submittals folder beside the prints, not given.
    let subs = dir.join("Project Submittals").join("21 13 13 Sprinklers");
    std::fs::create_dir_all(&subs).expect("submittals");
    std::fs::write(subs.join("Shop Drawings.pdf"), shop_drawing(&plan).0).expect("write");
    let s = run(&dir, None);
    let read = stage(&job_state(&s), "mep_read")["result"]["fire_protection"].clone();
    assert_eq!(read["heads"], json!(5), "{read:#}");
    assert_eq!(read["missing"], json!(false));
    let _ = std::fs::remove_dir_all(&dir);
}
