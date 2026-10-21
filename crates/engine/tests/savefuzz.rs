//! Save/reopen differential fuzzing (Q239): seeded random drawings — every entity and table
//! type, with odd but valid values (huge and tiny coordinates, unicode names and text, empty
//! blocks, blocks nested deep) — saved to every writer (DXF R12 and 2000–2018 as ASCII and
//! binary, DWG 2000–2018 with both engines, .cdy) and read back. Each copy is compared with the
//! original strictly, object by object (by handle) and record by record (by name); every
//! difference must be one the exchange notes report ([`kept::losses_between`], what saving tells
//! the user). One the notes don't report is a bug in a reader, a writer or the notes, and fails
//! the test unless it is in [`KNOWN`]. SVG and PDF are written and read back too (they keep
//! geometry only): neither may fail or come back empty.
//!
//! `SAVEFUZZ_SEEDS=n` runs seeds 0..n (default 4) and `SAVEFUZZ_SEEDS=a..b` seeds a..b, `SAVEFUZZ_SEED=s` only seed s, and
//! `SAVEFUZZ_REPORT=1` prints every difference found (`target/tmp/savefuzz.md`).
#![cfg(not(target_arch = "wasm32"))]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cadaddy_color::{Color, Rgb};
use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::kept::{Loss, MAX_LISTED, losses_between};
use cadaddy_engine::doc::geom::{Vec2, Vec3};
use cadaddy_engine::doc::{Block, Common, Drawing, Entity, EntityKind, Face3d, Insert, Line, Lineweight, Solid, Space, Transparency};
use cadaddy_io::{DwgEngine, SaveOptions, Version};
use serde_json::{Value, json};

/// Differences the notes don't report that are not bugs, or are bugs not fixed yet:
/// `format|what|path` (format `*` for all). Each needs its reason.
const KNOWN: &[&str] = &[
    // DWG doesn't store a viewport's number: readers number them by their order in the layout
    // (the notes leave the number out on purpose, `kept::content`).
    "DWG|layout viewport|kind.id",
    // A hatch dense enough for a million dashes (the most a hatch draws, as AutoCAD's HPMAXLINES)
    // is a million objects when its SVG is imported: more than the SVG reader's limit for
    // hostile files.
    "SVG|file|error: read: SVG: more than*",
];

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Names: unicode, spaces, the characters symbol names may hold.
const NAMES: &[&str] = &["Ébauche", "層-1", "Слой 2", "A$B-C_D", "x", "Δ fläche", "ط-٣", "N° 7", "Zoë's", "plan (old)"];
/// Text: unicode, MTEXT codes, DXF % codes, a CJK run and a long line.
const TEXTS: &[&str] = &[
    "PLAN",
    "Ø 25 %%c25 %%d %%p",
    "日本語のテキスト",
    "Grüße — «ça»",
    "a{b}c",
    "τ = 1/2",
    "mixed ascii 123 and 中文",
    "x²+y³",
    "emoji 🙂 here",
    "ﬁ ligature",
    "  spaced at both ends  ",
    "%%uunderlined%%u and %%o",
    r"caret ^ and backslash \ here",
    "a long line of text, long enough to need more than one DXF group when it is written out as a value: 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 end",
];

/// A drawing generated from `seed`, and the commands that refused (for the report).
fn drawing(seed: u64, dir: &std::path::Path) -> (Drawing, Vec<String>) {
    let mut r = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0x5EED);
    // One size per drawing: tiny, ordinary, huge.
    let scale = *r.pick(&[1.0, 1e-4, 1e6, 1e9, 1.0, 1e13, 1e-8]);
    let off = if r.chance(30) { scale * 1e3 * (r.unit() - 0.5) } else { 0.0 };
    let mut s = Session::new();
    let mut skipped = Vec::new();
    let mut run = |s: &mut Session, id: &str, p: Value| -> Option<Value> {
        match s.execute(id, &p) {
            Ok(v) => Some(v),
            Err(e) => {
                skipped.push(format!("{id}: {e}"));
                None
            }
        }
    };
    let mut c = |r: &mut Rng| -> f64 { if r.chance(5) { 0.0 } else { off + scale * (r.unit() * 200.0 - 100.0) } };
    let mut layers = vec!["0".to_string()];
    for i in 0..1 + r.below(4) {
        let name = format!("{}{}", r.pick(NAMES), i);
        let color = r.pick(&[json!("red"), json!(30), json!("10,200,30"), json!(255), json!(1)]).clone();
        let lw = *r.pick(&[0.0, 0.05, 0.5, 2.11]);
        if run(&mut s, "layer.new", json!({ "name": name, "color": color, "lineweight": lw })).is_some() {
            layers.push(name);
        }
    }
    run(&mut s, "linetype", json!({ "load": "DASHED" }));
    run(&mut s, "style", json!({ "name": format!("{} style", r.pick(NAMES)), "font": "Arial", "height": 0, "current": false }));
    run(&mut s, "dimstyle", json!({ "name": format!("{}-dim", r.pick(NAMES)), "current": false }));
    if r.chance(50) {
        run(&mut s, "mleaderstyle", json!({ "name": "Callout", "current": false }));
        run(&mut s, "tablestyle", json!({ "name": "Schedule", "current": false }));
        run(&mut s, "mlstyle", json!({ "name": "Double", "current": false }));
    }
    if r.chance(50) {
        let (x, y) = (c(&mut r), c(&mut r));
        run(&mut s, "ucs", json!({ "origin": [x, y], "name": format!("{} ucs", r.pick(NAMES)) }));
        run(&mut s, "ucs", json!({ "world": true }));
    }
    let count = 15 + r.below(25);
    for _ in 0..count {
        let layer = r.pick(&layers).clone();
        run(&mut s, "layer.current", json!({ "name": layer }));
        let p = |r: &mut Rng, c: &mut dyn FnMut(&mut Rng) -> f64| json!([c(r), c(r)]);
        let size = scale * (0.5 + r.unit() * 20.0);
        let ang = *r.pick(&[0.0, 15.0, 90.0, 359.999, -45.0, 180.0, 1e-7]);
        let text = r.pick(TEXTS).to_string();
        match r.below(31) {
            0 => run(&mut s, "line", json!({ "points": [p(&mut r, &mut c), p(&mut r, &mut c)] })),
            1 => run(&mut s, "point", json!({ "at": p(&mut r, &mut c) })),
            2 => run(&mut s, "circle", json!({ "center": p(&mut r, &mut c), "radius": size })),
            3 => run(&mut s, "arc", json!({ "center": p(&mut r, &mut c), "radius": size, "start": ang, "end": ang + 1.0 + r.unit() * 300.0 })),
            4 => run(&mut s, "ellipse", json!({ "center": p(&mut r, &mut c), "major": [size, size * 0.2], "ratio": 0.05 + r.unit() * 0.95 })),
            5 => {
                let a = p(&mut r, &mut c);
                let b = p(&mut r, &mut c);
                let bulge = *r.pick(&[0.0, 0.5, -1.0, 1e-6, 3.0]);
                run(
                    &mut s,
                    "pline",
                    json!({ "vertices": [a, { "p": b, "bulge": bulge, "startWidth": size * 0.01, "endWidth": 0 }, p(&mut r, &mut c)], "closed": r.chance(50) }),
                )
            }
            6 => {
                let z = c(&mut r);
                run(&mut s, "3dpoly", json!({ "points": [[c(&mut r), c(&mut r), z], [c(&mut r), c(&mut r), 0], [c(&mut r), c(&mut r), -z]] }))
            }
            7 => run(&mut s, "spline", json!({ "fit": [p(&mut r, &mut c), p(&mut r, &mut c), p(&mut r, &mut c), p(&mut r, &mut c)] })),
            8 => {
                run(&mut s, "spline", json!({ "control": [p(&mut r, &mut c), p(&mut r, &mut c), p(&mut r, &mut c), p(&mut r, &mut c)], "degree": 3 }))
            }
            9 => run(
                &mut s,
                "helix",
                json!({ "center": p(&mut r, &mut c), "baseRadius": size, "topRadius": size * 0.5, "height": size * 2.0, "turns": 1 + r.below(5) }),
            ),
            10 => run(&mut s, "ray", json!({ "base": [c(&mut r), c(&mut r)], "through": [c(&mut r), c(&mut r)] })),
            11 => run(&mut s, "xline", json!({ "base": p(&mut r, &mut c), "angle": ang })),
            12 => {
                let j = r.pick(&["L", "MC", "TR", "BL", "M"]).to_string();
                run(&mut s, "text", json!({ "at": p(&mut r, &mut c), "text": text, "height": size * 0.1, "rotation": ang, "justify": j }))
            }
            13 => run(
                &mut s,
                "mtext",
                json!({ "at": p(&mut r, &mut c), "text": format!("{{\\fArial|b1;{text}}} line\\Pnext"), "width": size * 3.0, "height": size * 0.1 }),
            ),
            14 => run(&mut s, "dimlinear", json!({ "p1": p(&mut r, &mut c), "p2": p(&mut r, &mut c), "at": p(&mut r, &mut c) })),
            15 => run(&mut s, "dimaligned", json!({ "p1": p(&mut r, &mut c), "p2": p(&mut r, &mut c), "at": p(&mut r, &mut c) })),
            16 => {
                let (x, y) = (c(&mut r), c(&mut r));
                run(
                    &mut s,
                    "dimangular",
                    json!({ "vertex": [x, y], "p1": [x + size, y], "p2": [x + size * 0.5, y + size], "at": [x + size * 0.8, y + size * 0.6] }),
                )
            }
            17 => {
                let (x, y) = (c(&mut r), c(&mut r));
                run(&mut s, "circle", json!({ "center": [x, y], "radius": size }));
                if r.chance(50) {
                    run(&mut s, "dimradius", json!({ "center": [x, y], "point": [x + size, y] }))
                } else {
                    run(&mut s, "dimdiameter", json!({ "center": [x, y], "point": [x, y + size] }))
                }
            }
            18 => run(&mut s, "dimordinate", json!({ "feature": p(&mut r, &mut c), "leader": p(&mut r, &mut c) })),
            19 => run(
                &mut s,
                "mline",
                json!({ "points": [p(&mut r, &mut c), p(&mut r, &mut c), p(&mut r, &mut c)], "scale": size * 0.1, "justification": "zero" }),
            ),
            20 => run(&mut s, "leader", json!({ "points": [p(&mut r, &mut c), p(&mut r, &mut c), p(&mut r, &mut c)], "text": text })),
            21 => run(&mut s, "mleader", json!({ "points": [p(&mut r, &mut c), p(&mut r, &mut c)], "text": text })),
            22 => {
                let (x, y) = (c(&mut r), c(&mut r));
                run(&mut s, "rectang", json!({ "p1": [x, y], "p2": [x + size, y + size] }));
                let pat = r.pick(&["ANSI31", "SOLID", "NET", "DOTS"]).to_string();
                run(&mut s, "hatch", json!({ "points": [[x + size * 0.5, y + size * 0.5]], "pattern": pat, "scale": size * 0.05, "angle": ang }))
            }
            23 => run(
                &mut s,
                "table",
                json!({ "at": p(&mut r, &mut c), "rows": 2, "cols": 2, "cells": [[text, "b"], ["c", "1.5"]], "title": text, "header": ["H1", "H2"] }),
            ),
            24 => run(&mut s, "tolerance", json!({ "at": p(&mut r, &mut c), "symbol": "position", "tol1": "%%c0.1(M)", "datums": ["A", "B"] })),
            25 => {
                let (x, y) = (c(&mut r), c(&mut r));
                run(&mut s, "wipeout", json!({ "points": [[x, y], [x + size, y], [x + size, y + size], [x, y + size]] }))
            }
            26 => {
                let (x, y) = (c(&mut r), c(&mut r));
                run(&mut s, "box", json!({ "corner": [x, y, 0], "length": size, "width": size * 0.5, "height": size * 0.3 }))
            }
            27 => {
                let (x, y) = (c(&mut r), c(&mut r));
                let h = run(&mut s, "circle", json!({ "center": [x, y], "radius": size })).and_then(|v| v["handle"].as_str().map(str::to_string));
                h.and_then(|h| run(&mut s, "region", json!({ "handles": [h] })))
            }
            28 => run(&mut s, "sectionplane", json!({ "from": p(&mut r, &mut c), "to": p(&mut r, &mut c) })),
            29 => run(&mut s, "light", json!({ "kind": "point", "position": [c(&mut r), c(&mut r), c(&mut r)] })),
            _ => run(&mut s, "attdef", json!({ "tag": "ROOM", "prompt": text, "default": "101", "at": p(&mut r, &mut c), "height": size * 0.1 })),
        };
    }
    // A block with an attribute, inserted.
    let (x, y) = (c(&mut r), c(&mut r));
    let def = run(&mut s, "attdef", json!({ "tag": "TAG", "prompt": "Tag", "default": "1", "at": [x, y], "height": scale }))
        .and_then(|v| v["handle"].as_str().map(str::to_string));
    let ring = run(&mut s, "circle", json!({ "center": [x, y], "radius": scale })).and_then(|v| v["handle"].as_str().map(str::to_string));
    let bname = format!("{} blk", r.pick(NAMES));
    if let (Some(def), Some(ring)) = (def, ring)
        && run(&mut s, "block", json!({ "name": bname, "base": [x, y], "handles": [def, ring], "keep": "delete" })).is_some()
    {
        let at = [c(&mut r), c(&mut r)];
        run(
            &mut s,
            "insert",
            json!({ "name": bname, "at": at, "scale": [1.5, -1.5], "rotation": 20, "attribs": { "TAG": r.pick(TEXTS).to_string() } }),
        );
    }
    // Layouts, viewports, views, groups.
    if r.chance(60) {
        let vp = run(&mut s, "mview", json!({ "layout": "Layout1", "p1": [1, 1], "p2": [8, 6] }))
            .and_then(|v| v["viewports"].get(0).and_then(Value::as_str).map(str::to_string));
        // Layers frozen and overridden in the viewport.
        if let Some(vp) = vp
            && layers.len() > 1
        {
            let l = layers[1].clone();
            run(&mut s, "vplayer", json!({ "handle": vp, "freeze": [l.clone()], "colors": { l: "10,20,30" } }));
        }
    }
    if r.chance(50) {
        run(&mut s, "layout", json!({ "option": "new", "name": format!("Sheet {}", r.pick(NAMES)) }));
    }
    let (vx, vy) = (c(&mut r), c(&mut r));
    run(&mut s, "view", json!({ "option": "save", "name": format!("{} view", r.pick(NAMES)), "center": [vx, vy], "height": scale * 40.0 }));
    let first = s.doc().map(|d| d.model.iter().take(2).map(|e| e.handle.hex()).collect::<Vec<_>>()).unwrap_or_default();
    if first.len() == 2 {
        run(&mut s, "group", json!({ "name": format!("{}grp", r.pick(NAMES)), "handles": first }));
    }
    if r.chance(30) {
        let png = dir.join("photo.png");
        if !png.exists() {
            let list = cadaddy_engine::render::DisplayList::default();
            let view = cadaddy_engine::render::raster::View::of_window(
                &cadaddy_engine::geom::Bounds2::new(Vec2::ZERO, Vec2::new(1.0, 1.0)),
                Some(8),
                Some(8),
            );
            if let Some(bytes) = cadaddy_engine::render::raster::render_png(&list, &view, &Default::default()) {
                let _ = std::fs::write(&png, bytes);
            }
        }
        run(&mut s, "imageattach", json!({ "path": png, "at": [c(&mut r), c(&mut r)], "scale": scale * 5.0 }));
    }
    // A block made of some of the objects, inserted.
    let handles: Vec<String> = s
        .doc()
        .map(|d| {
            d.model
                .iter()
                .filter(|e| {
                    matches!(
                        e.kind,
                        EntityKind::Line(_)
                            | EntityKind::Circle(_)
                            | EntityKind::Text(_)
                            | EntityKind::MText(_)
                            | EntityKind::LwPolyline(_)
                            | EntityKind::Hatch(_)
                            | EntityKind::Spline(_)
                    )
                })
                .map(|e| e.handle.hex())
                .collect()
        })
        .unwrap_or_default();
    let some: Vec<&String> = handles.iter().filter(|_| r.chance(30)).take(8).collect();
    let made = format!("{} parts", r.pick(NAMES));
    if !some.is_empty() && run(&mut s, "block", json!({ "name": made, "base": [c(&mut r), c(&mut r)], "handles": some, "keep": "delete" })).is_some()
    {
        let at = [c(&mut r), c(&mut r)];
        let ins = run(&mut s, "insert", json!({ "name": made, "at": at, "scale": [2, 2], "rotation": -30 }))
            .and_then(|v| v["handle"].as_str().map(str::to_string));
        if let Some(h) = ins
            && r.chance(50)
        {
            let k = scale * 5.0;
            run(&mut s, "xclip", json!({ "handles": [h], "boundary": [[at[0] - k, at[1] - k], [at[0] + k, at[1] + k]], "invert": r.chance(30) }));
        }
    }
    // A gradient, a layer state, groups with descriptions and without names.
    if r.chance(40) {
        let (x, y) = (c(&mut r), c(&mut r));
        run(&mut s, "rectang", json!({ "p1": [x, y], "p2": [x + scale, y + scale] }));
        let name = r.pick(&["LINEAR", "CYLINDER", "SPHERICAL", "INVCURVED"]).to_string();
        run(
            &mut s,
            "gradient",
            json!({ "points": [[x + scale * 0.5, y + scale * 0.5]], "name": name, "color1": "red", "color2": "10,200,30", "angle": 30 }),
        );
    }
    if r.chance(40) {
        run(&mut s, "layerstate.save", json!({ "name": format!("{} state", r.pick(NAMES)), "description": r.pick(TEXTS).to_string() }));
    }
    let some: Vec<String> = s.doc().map(|d| d.model.iter().skip(2).take(3).map(|e| e.handle.hex()).collect()).unwrap_or_default();
    if !some.is_empty() && r.chance(50) {
        run(&mut s, "group", json!({ "handles": some, "description": r.pick(TEXTS).to_string(), "selectable": r.chance(50) }));
    }
    // A polyline of thousands of vertices.
    if r.chance(30) {
        let n = 500 + r.below(3000);
        let pts: Vec<[f64; 2]> = (0..n).map(|i| [scale * i as f64 * 0.01, scale * (i as f64 * 0.1).sin()]).collect();
        run(&mut s, "pline", json!({ "vertices": pts, "closed": false }));
    }
    let mut d = s.doc().map_or_else(|_| Drawing::new_metric(), |d| d.clone());
    // What no command makes: solids, traces, 3D faces, an empty block, blocks nested deep,
    // an array insert.
    let lay = r.pick(&layers).clone();
    let common = || Common { layer: lay.clone(), ..Common::default() };
    let q = |x: f64, y: f64, k: f64| [Vec3::new(x, y, 0.0), Vec3::new(x + k, y, 0.0), Vec3::new(x, y + k, 0.0), Vec3::new(x + k, y + k, 0.0)];
    let k = scale * 4.0;
    let (x, y) = (c(&mut r), c(&mut r));
    let _ = d.add(&Space::Model, common(), EntityKind::Solid(Solid { corners: q(x, y, k) }));
    let _ = d.add(&Space::Model, common(), EntityKind::Trace(Solid { corners: q(y, x, k) }));
    let _ =
        d.add(&Space::Model, common(), EntityKind::Face3d(Face3d { corners: q(x, x, k).map(|v| Vec3::new(v.x, v.y, v.x * 0.1)), hidden_edges: 5 }));
    let (x, y) = (c(&mut r), c(&mut r));
    let mesh = cadaddy_engine::doc::Mesh {
        vertices: vec![Vec3::new(x, y, 0.0), Vec3::new(x + k, y, 0.0), Vec3::new(x, y + k, k * 0.1), Vec3::new(x + k, y + k, -k)],
        faces: vec![
            cadaddy_engine::doc::MeshFace { v: vec![0, 1, 2], ..Default::default() },
            cadaddy_engine::doc::MeshFace { v: vec![1, 3, 2], ..Default::default() },
        ],
        ..Default::default()
    };
    let _ = d.add(&Space::Model, common(), EntityKind::Mesh(mesh));
    let empty = format!("Empty {}", r.pick(NAMES));
    d.blocks.insert(empty.clone(), Arc::new(Block::new(&empty)));
    if r.chance(70) {
        let _ = d.add(&Space::Model, common(), insert(&empty, Vec3::new(c(&mut r), c(&mut r), 0.0)));
    }
    let depth = 2 + r.below(9);
    let mut inner = String::new();
    for level in 0..depth {
        let name = format!("Nest{level} {}", r.pick(NAMES));
        let mut b = Block::new(&name);
        b.base = Vec3::new(c(&mut r), c(&mut r), 0.0);
        let h = d.new_handle();
        b.entities.push(Entity {
            handle: h,
            common: Common::default(),
            kind: EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(k, k * 0.5, 0.0) }),
        });
        if !inner.is_empty() {
            let h = d.new_handle();
            let mut ins = insert(&inner, Vec3::new(k, 0.0, 0.0));
            if let EntityKind::Insert(i) = &mut ins {
                i.scale = Vec3::new(0.5, 0.5, 1.0);
                i.rotation = 15f64.to_radians();
            }
            b.entities.push(Entity { handle: h, common: Common::default(), kind: ins });
        }
        d.blocks.insert(name.clone(), Arc::new(b));
        inner = name;
    }
    let mut top = insert(&inner, Vec3::new(c(&mut r), c(&mut r), 0.0));
    if r.chance(40)
        && let EntityKind::Insert(i) = &mut top
    {
        i.cols = 3;
        i.rows = 2;
        i.col_spacing = k * 2.0;
        i.row_spacing = k * 3.0;
    }
    let _ = d.add(&Space::Model, common(), top);
    twist(&mut d, &mut r, scale);
    (d, skipped)
}

/// Odd but valid values set straight on the drawing: table records' flags and descriptions,
/// objects' properties, xdata, and the fields of text and polylines no command parameter sets.
fn twist(d: &mut Drawing, r: &mut Rng, scale: f64) {
    for l in d.layers.iter_mut().filter(|l| l.name != "0") {
        l.on = r.chance(70);
        l.frozen = r.chance(20);
        l.locked = r.chance(20);
        l.plot = r.chance(70);
        l.transparency = if r.chance(30) { r.below(91) as u8 } else { 0 };
        if r.chance(50) {
            l.description = r.pick(TEXTS).to_string();
        }
    }
    for t in d.text_styles.iter_mut().filter(|t| !t.name.eq_ignore_ascii_case("Standard")) {
        t.width_factor = *r.pick(&[1.0, 0.01, 0.75, 100.0]);
        t.oblique = *r.pick(&[0.0, 15.0, -30.0]);
        t.backwards = r.chance(20);
        t.upside_down = r.chance(20);
    }
    let names: Vec<String> = d.blocks.iter().filter(|(_, b)| !b.anonymous).map(|(n, _)| n.clone()).collect();
    for n in names {
        if let Some(b) = d.blocks.get_mut(&n) {
            let b = Arc::make_mut(b);
            if r.chance(40) {
                b.description = r.pick(TEXTS).to_string();
            }
            if r.chance(30) {
                b.units = r.below(21) as u8;
            }
            if r.chance(20) {
                b.explodable = false;
            }
        }
    }
    for ds in d.dim_styles.iter_mut().filter(|s| !s.name.eq_ignore_ascii_case("Standard")) {
        ds.scale = *r.pick(&[1.0, 0.001, 48.0]);
        ds.arrow_size = scale * 0.2;
        ds.text_height = scale * 0.25;
        ds.linear_factor = *r.pick(&[1.0, 25.4, 0.5]);
        ds.round = *r.pick(&[0.0, 0.25, 5.0]);
        ds.tol_plus = scale * 0.01;
        ds.tol_minus = scale * 0.02;
    }
    // Copies of some objects on a layout's paper.
    if let Some(l) = d.layouts.first().map(|l| l.name.clone()) {
        let copies: Vec<(Common, EntityKind)> = d
            .model
            .iter()
            .filter(|e| !matches!(e.kind, EntityKind::Viewport(_)) && r.chance(15))
            .map(|e| (e.common.clone(), e.kind.clone()))
            .collect();
        for (common, kind) in copies {
            let _ = d.add(&Space::Paper(l.clone()), common, kind);
        }
    }
    // Objects on a layout's paper, not only its viewports.
    if let Some(l) = d.layouts.first().map(|l| l.name.clone()) {
        let k = scale.min(1e3);
        let _ = d.add(&Space::Paper(l.clone()), Common::default(), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(k * 10.0, k * 5.0, 0.0) }));
        let _ = d.add(
            &Space::Paper(l),
            Common::default(),
            EntityKind::Text(cadaddy_engine::doc::Text {
                insert: Vec3::new(k, k, 0.0),
                height: k * 0.25,
                value: r.pick(TEXTS).to_string(),
                align_pt: None,
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: Default::default(),
                valign: Default::default(),
            }),
        );
    }
    // Odd properties on a third of the objects.
    let all: Vec<Entity> = d.model.iter().map(|e| (**e).clone()).collect();
    for mut e in all {
        if r.chance(20) {
            let s = r.pick(TEXTS).to_string();
            let n = format!("{}", (r.unit() - 0.5) * scale);
            e.common.xdata = [(1001, "FUZZ_APP"), (1000, s.as_str()), (1040, n.as_str()), (1070, "-7"), (1071, "123456")]
                .into_iter()
                .map(|(code, value)| cadaddy_engine::doc::RawTag { code, value: value.to_string() })
                .collect();
        }
        match &mut e.kind {
            EntityKind::Text(t) if r.chance(50) => {
                t.width_factor = *r.pick(&[0.5, 2.0, 0.01]);
                t.oblique = *r.pick(&[10.0, -20.0]);
                // Text is flat: its alignment point is at its elevation too.
                t.insert.z = scale * (r.unit() - 0.5);
                if let Some(a) = &mut t.align_pt {
                    a.z = t.insert.z;
                }
            }
            EntityKind::MText(m) if r.chance(50) => {
                m.attach = 1 + r.below(9) as u8;
                m.line_spacing = *r.pick(&[0.25, 1.0, 4.0]);
                m.line_spacing_exact = r.chance(50);
                m.rotation = *r.pick(&[0.0, 1.0, -2.5]);
                m.contents = format!("{}\\P{}", m.contents, "long line ".repeat(40));
            }
            EntityKind::LwPolyline(p) if r.chance(50) => {
                p.elevation = scale * (r.unit() - 0.5);
                p.plinegen = r.chance(50);
                if p.vertices.iter().all(|v| v.start_width == 0.0 && v.end_width == 0.0) {
                    p.const_width = scale * 0.01;
                }
            }
            _ => {}
        }
        d.model.replace(e);
    }
    let all: Vec<Entity> = d.model.iter().map(|e| (**e).clone()).collect();
    for mut e in all {
        if !r.chance(33) {
            continue;
        }
        let c = &mut e.common;
        match r.below(7) {
            0 => c.color = Color::True(Rgb(r.below(256) as u8, r.below(256) as u8, r.below(256) as u8)),
            1 => c.transparency = Transparency::Percent(r.below(91) as u8),
            2 => c.thickness = scale * (r.unit() - 0.5),
            3 => c.ltscale = *r.pick(&[0.01, 7.5, 1e3]),
            4 => c.lineweight = *r.pick(&[Lineweight::Mm100(0), Lineweight::Mm100(211), Lineweight::ByBlock, Lineweight::Default]),
            5 => c.visible = false,
            _ if matches!(e.kind, EntityKind::Circle(_) | EntityKind::Arc(_)) => c.extrusion = Vec3::new(0.0, 0.0, -1.0),
            _ => c.color = Color::Index(1 + r.below(255) as u8),
        }
        d.model.replace(e);
    }
    let _ = scale;
}

fn insert(block: &str, at: Vec3) -> EntityKind {
    EntityKind::Insert(Insert {
        block: block.into(),
        insert: at,
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: vec![],
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    })
}

/// A writer: its name, and the drawing written and read back by it.
struct Format {
    name: String,
    /// Whether a strict comparison applies (DXF, DWG, .cdy). R12 has no handles to match by;
    /// SVG and PDF keep geometry only.
    strict: bool,
    run: Box<dyn Fn(&Drawing, &std::path::Path) -> Result<Drawing, String>>,
}

fn formats() -> Vec<Format> {
    let mut v: Vec<Format> = Vec::new();
    let with = |name: &'static str, opts: SaveOptions| {
        Box::new(move |d: &Drawing, _: &std::path::Path| {
            let b = cadaddy_io::write_with(d, name, &opts).map_err(|e| format!("write: {e}"))?;
            cadaddy_io::read(&b, name).map_err(|e| format!("read: {e}"))
        }) as Box<dyn Fn(&Drawing, &std::path::Path) -> Result<Drawing, String>>
    };
    for ver in [Version::R2000, Version::R2004, Version::R2007, Version::R2010, Version::R2013, Version::R2018] {
        let y = ver.year();
        v.push(Format { name: format!("DXF {y}"), strict: true, run: with("x.dxf", SaveOptions { version: Some(ver), binary: false }) });
        v.push(Format { name: format!("DXF {y} binary"), strict: true, run: with("x.dxf", SaveOptions { version: Some(ver), binary: true }) });
        // Each engine's file read by itself and by the other one.
        let engines = [("bridge", DwgEngine::Bridge), ("native", DwgEngine::Native)];
        for (w, writer) in engines {
            for (r, reader) in engines {
                let name = if w == r { format!("DWG {y} {w}") } else { format!("DWG {y} {w}→{r}") };
                v.push(Format {
                    name,
                    strict: true,
                    run: Box::new(move |d, _| {
                        let b = cadaddy_io::write_dwg(d, ver, writer).map_err(|e| format!("write: {e}"))?;
                        cadaddy_io::read_dwg(&b, reader).map_err(|e| format!("read: {e}"))
                    }),
                });
            }
        }
    }
    v.push(Format { name: "CDY".into(), strict: true, run: with("x.cdy", SaveOptions::default()) });
    // Release 12 as SAVEAS writes it (the drawing brought down to what R12 holds first).
    for binary in [false, true] {
        v.push(Format {
            name: format!("DXF R12{}", if binary { " binary" } else { "" }),
            strict: false,
            run: Box::new(move |d, dir| {
                let mut s = Session::new();
                s.open_drawing(d.clone(), "fuzz", None);
                let path = dir.join(format!("r12{}.dxf", if binary { "b" } else { "" }));
                s.execute("saveas", &json!({ "path": path, "version": 12, "binary": binary })).map_err(|e| format!("write: {e}"))?;
                let b = std::fs::read(&path).map_err(|e| e.to_string())?;
                cadaddy_io::read(&b, "x.dxf").map_err(|e| format!("read: {e}"))
            }),
        });
    }
    v.push(Format { name: "SVG".into(), strict: false, run: with("x.svg", SaveOptions::default()) });
    v.push(Format { name: "PDF".into(), strict: false, run: with("x.pdf", SaveOptions::default()) });
    v
}

/// Numbers equal to 1 part in 10⁹ (relative), as the notes compare them (`kept::kept`).
fn same_number(x: f64, y: f64) -> bool {
    x == y || (x - y).abs() <= 1e-9 * x.abs().max(y.abs())
}

/// The paths where `a` and `b` differ, array indices left out (`kind.vertices[]`).
fn differ(a: &Value, b: &Value, path: &str, out: &mut BTreeMap<String, String>) {
    // Unset before and filled in on reading (a dimension's block and text point, a hatch's
    // pattern lines, an image's clip): derived data, nothing lost.
    if a.is_null() || path.starts_with("kind.textMid.") && a.as_f64() == Some(0.0) {
        return;
    }
    let mut note = |p: &str| {
        let short = |v: &Value| v.to_string().chars().take(70).collect::<String>();
        out.entry(p.to_string()).or_insert_with(|| format!("{} → {}", short(a), short(b)));
    };
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if !same_number(x.as_f64().unwrap_or(f64::NAN), y.as_f64().unwrap_or(f64::NAN)) {
                note(path);
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                differ(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), &p, out);
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                note(&format!("{path}[len]"));
            } else {
                for (i, j) in x.iter().zip(y) {
                    differ(i, j, &format!("{path}[]"), out);
                }
            }
        }
        _ if a != b => note(path),
        _ => {}
    }
}

/// `kept::words`: a field name as the notes name it.
fn words(key: &str) -> String {
    let mut out = String::new();
    for c in key.chars() {
        if c == '_' {
            out.push(' ');
        } else if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

fn kind_name(kind: &Value) -> String {
    match kind.get("type").and_then(Value::as_str) {
        Some("unknown") => kind.get("dxfType").and_then(Value::as_str).unwrap_or("unknown").to_ascii_lowercase(),
        Some(t) => t.to_ascii_lowercase(),
        None => "object".into(),
    }
}

/// Whether a loss covers `handle` (listed, or the list was cut at [`MAX_LISTED`]).
fn covers_handle(l: &Loss, handle: &str) -> bool {
    l.handles.iter().any(|h| h.eq_ignore_ascii_case(handle)) || (l.handles.len() >= MAX_LISTED && l.count > l.handles.len())
}

fn covers_name(l: &Loss, name: &str) -> bool {
    l.names.iter().any(|n| n.eq_ignore_ascii_case(name)) || (l.names.len() >= MAX_LISTED && l.count > l.names.len())
}

/// What the notes leave out: (what, path) → example.
type Unreported = BTreeMap<(String, String), String>;

/// The entities of one space or block compared by handle.
fn compare_entities(a: &[Arc<Entity>], b: &[Arc<Entity>], wher: &str, notes: &[Loss], out: &mut Unreported) {
    let back: BTreeMap<_, _> = b.iter().map(|e| (e.handle, e)).collect();
    let ours: BTreeSet<_> = a.iter().map(|e| e.handle).collect();
    let fields = |e: &Entity| (serde_json::to_value(&e.common).unwrap_or_default(), serde_json::to_value(&e.kind).unwrap_or_default());
    for e in a {
        let (ca, ka) = fields(e);
        let kind = kind_name(&ka);
        let h = e.handle.hex();
        let Some(f) = back.get(&e.handle) else {
            if !notes.iter().any(|l| covers_handle(l, &h) && l.feature.starts_with(&format!("{kind} objects"))) {
                out.entry((format!("{wher} {kind}"), "missing".into())).or_insert_with(|| h.clone());
            }
            continue;
        };
        let (cb, kb) = fields(f);
        if kind_name(&kb) != kind {
            if !notes.iter().any(|l| covers_handle(l, &h) && l.feature.starts_with(&format!("{kind} objects (written as"))) {
                out.entry((format!("{wher} {kind}"), format!("became {}", kind_name(&kb)))).or_insert_with(|| h.clone());
            }
            continue;
        }
        let whole = notes.iter().any(|l| covers_handle(l, &h) && l.feature.starts_with(&format!("{kind} objects")));
        let mut paths = BTreeMap::new();
        differ(&ca, &cb, "common", &mut paths);
        differ(&ka, &kb, "kind", &mut paths);
        for (p, ex) in paths {
            let mut parts = p.splitn(3, '.');
            let (top, field) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
            let field = field.trim_end_matches("[len]").trim_end_matches("[]");
            if matches!(field, "handle" | "owner") {
                continue;
            }
            let feature = if top == "common" { format!("object {}", words(field)) } else { format!("{kind} {}", words(field)) };
            if whole || notes.iter().any(|l| l.feature == feature && covers_handle(l, &h)) {
                continue;
            }
            out.entry((format!("{wher} {kind}"), p)).or_insert_with(|| format!("{h}: {ex}"));
        }
    }
    for f in b.iter().filter(|f| !ours.contains(&f.handle)) {
        let kind = kind_name(&serde_json::to_value(&f.kind).unwrap_or_default());
        if !notes.iter().any(|l| l.feature.contains("(written as") && l.feature.contains(&kind)) {
            out.entry((format!("{wher} {kind}"), "appeared".into())).or_insert_with(|| f.handle.hex());
        }
    }
}

/// Table records by (upper-case) name.
fn records<T: serde::Serialize>(t: &T) -> BTreeMap<String, Value> {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|x| Some((x.get("name")?.as_str()?.to_ascii_uppercase(), x)))
        .collect()
}

/// Everything `back` changed of `d` that `notes` don't report.
fn unreported(d: &Drawing, back: &Drawing, notes: &[Loss]) -> Unreported {
    let mut out = Unreported::new();
    let list = |s: &cadaddy_engine::doc::EntityStore| s.iter().cloned().collect::<Vec<_>>();
    compare_entities(&list(&d.model), &list(&back.model), "model", notes, &mut out);
    for l in &d.layouts {
        match back.layouts.iter().find(|x| x.name.eq_ignore_ascii_case(&l.name)) {
            Some(m) => {
                compare_entities(&list(&l.entities), &list(&m.entities), "layout", notes, &mut out);
                let mut paths = BTreeMap::new();
                differ(&serde_json::to_value(l).unwrap_or_default(), &serde_json::to_value(m).unwrap_or_default(), "", &mut paths);
                for (p, ex) in paths {
                    out.entry(("layout".into(), p)).or_insert_with(|| format!("{}: {ex}", l.name));
                }
            }
            None if notes.iter().any(|x| x.feature == "layouts" && covers_name(x, &l.name)) => {}
            None => {
                out.entry(("layout".into(), "missing".into())).or_insert_with(|| l.name.clone());
            }
        }
    }
    for (n, b) in d.blocks.iter().filter(|(_, b)| !b.anonymous) {
        match back.blocks.get(n) {
            Some(m) => {
                compare_entities(&list(&b.entities), &list(&m.entities), "block", notes, &mut out);
                let mut paths = BTreeMap::new();
                differ(&serde_json::to_value(&**b).unwrap_or_default(), &serde_json::to_value(&**m).unwrap_or_default(), "", &mut paths);
                for (p, ex) in paths {
                    let field = p.split('.').next().unwrap_or_default().trim_end_matches("[len]").trim_end_matches("[]");
                    if !notes.iter().any(|l| l.feature == format!("blocks: {}", words(field)) && covers_name(l, n)) {
                        out.entry(("block".into(), p)).or_insert_with(|| format!("{n}: {ex}"));
                    }
                }
            }
            None if notes.iter().any(|x| x.feature == "blocks" && covers_name(x, n)) => {}
            None => {
                out.entry(("block".into(), "missing".into())).or_insert_with(|| n.clone());
            }
        }
    }
    let tables: [(&str, BTreeMap<String, Value>, BTreeMap<String, Value>); 7] = [
        ("layers", records(&d.layers), records(&back.layers)),
        ("linetypes", records(&d.linetypes), records(&back.linetypes)),
        ("text styles", records(&d.text_styles), records(&back.text_styles)),
        ("dimension styles", records(&d.dim_styles), records(&back.dim_styles)),
        ("multileader styles", records(&d.mleader_styles), records(&back.mleader_styles)),
        ("table styles", records(&d.table_styles), records(&back.table_styles)),
        ("multiline styles", records(&d.mline_styles), records(&back.mline_styles)),
    ];
    for (t, a, b) in &tables {
        for (name, rec) in a {
            let Some(r) = b.get(name) else {
                if !notes.iter().any(|l| l.feature == *t && covers_name(l, name)) {
                    out.entry((t.to_string(), "missing".into())).or_insert_with(|| name.clone());
                }
                continue;
            };
            let mut paths = BTreeMap::new();
            differ(rec, r, "", &mut paths);
            for (p, ex) in paths {
                let field = p.split('.').next().unwrap_or_default().trim_end_matches("[len]").trim_end_matches("[]");
                if field == "name" {
                    continue;
                }
                let feature = format!("{t}: {}", words(field));
                if notes.iter().any(|l| (l.feature == feature || l.feature == *t) && covers_name(l, name)) {
                    continue;
                }
                out.entry((t.to_string(), p)).or_insert_with(|| format!("{name}: {ex}"));
            }
        }
        for name in b.keys().filter(|n| !a.contains_key(*n)) {
            out.entry((t.to_string(), "appeared".into())).or_insert_with(|| name.clone());
        }
    }
    let v = |x: &dyn erased::Ser| x.json();
    let whole: [(&str, Value, Value); 9] = [
        ("named views", v(&d.views), v(&back.views)),
        ("UCSs", v(&d.ucss), v(&back.ucss)),
        ("layer states", v(&d.layer_states), v(&back.layer_states)),
        ("groups", v(&d.groups), v(&back.groups)),
        ("plot style tables", v(&d.plot_style_tables), v(&back.plot_style_tables)),
        ("geometric and dimensional constraints", v(&d.constraints), v(&back.constraints)),
        ("parameters", v(&d.parametric), v(&back.parametric)),
        ("the scale list", v(&d.scales), v(&back.scales)),
        ("materials", v(&d.materials), v(&back.materials)),
    ];
    for (part, a, b) in &whole {
        if notes.iter().any(|l| l.feature == *part) {
            continue;
        }
        let mut paths = BTreeMap::new();
        differ(a, b, "", &mut paths);
        for (p, ex) in paths {
            out.entry((part.to_string(), p)).or_insert(ex);
        }
    }
    out
}

/// Serializing behind `&dyn` (the parts have different types).
mod erased {
    pub trait Ser {
        fn json(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn json(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap_or_default()
        }
    }
}

/// Whether a KNOWN line matches `format|what|path`.
fn known(format: &str, what: &str, path: &str) -> bool {
    KNOWN.iter().any(|k| {
        let mut it = k.splitn(3, '|');
        let (f, w, p) = (it.next().unwrap_or_default(), it.next().unwrap_or_default(), it.next().unwrap_or_default());
        (f == "*" || format.starts_with(f))
            && (w == "*" || w == what)
            && (p == "*" || p == path || (p.ends_with('*') && path.starts_with(p.trim_end_matches('*'))))
    })
}

fn count_visible(d: &Drawing) -> usize {
    d.model.len()
}

#[test]
fn save_reopen_differential_fuzz() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-savefuzz-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    let seeds: Vec<u64> = match std::env::var("SAVEFUZZ_SEED").ok().and_then(|s| s.parse().ok()) {
        Some(s) => vec![s],
        // `n` (seeds 0..n) or `a..b`.
        None => {
            let v = std::env::var("SAVEFUZZ_SEEDS").unwrap_or_default();
            let (a, b) = v.split_once("..").map_or((Some(0), v.parse().ok()), |(a, b)| (a.parse().ok(), b.parse().ok()));
            (a.unwrap_or(0)..b.unwrap_or(4)).collect()
        }
    };
    let fmts = formats();
    // (format, what, path) → (seed, example).
    let mut found: BTreeMap<(String, String, String), (u64, String)> = BTreeMap::new();
    let mut refused = BTreeSet::new();
    let mut types: BTreeMap<&'static str, usize> = BTreeMap::new();
    for &seed in &seeds {
        let (d, skipped) = drawing(seed, &dir);
        for e in d.model.iter() {
            *types.entry(e.kind.type_name()).or_default() += 1;
        }
        refused.extend(skipped.into_iter().map(|s| s.chars().take(120).collect::<String>()));
        for f in &fmts {
            let back = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (f.run)(&d, &dir))) {
                Ok(Ok(b)) => b,
                Ok(Err(e)) => {
                    // The error's words (its numbers left out) tell one kind from another.
                    let kind: String = e.chars().filter(|c| !c.is_ascii_digit()).take(60).collect();
                    found.entry((f.name.clone(), "file".into(), format!("error: {kind}"))).or_insert((seed, e));
                    continue;
                }
                Err(_) => {
                    found.entry((f.name.clone(), "file".into(), "panic".into())).or_insert((seed, String::new()));
                    continue;
                }
            };
            if !f.strict {
                if count_visible(&d) > 0 && back.model.is_empty() {
                    found.entry((f.name.clone(), "file".into(), "empty".into())).or_insert((seed, String::new()));
                }
                if f.name.starts_with("DXF R12") {
                    // The notes of a Release 12 file must still be worked out.
                    let _ = losses_between(&d, &back);
                }
                continue;
            }
            let notes = losses_between(&d, &back);
            for ((what, path), ex) in unreported(&d, &back, &notes) {
                found.entry((f.name.clone(), what, path)).or_insert((seed, ex));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let mut md =
        String::from("# Save/reopen differential fuzzing\n\nDifferences the exchange notes don't report (format · what · path: seed, example).\n\n");
    for ((f, w, p), (seed, ex)) in &found {
        let k = if known(f, w, p) { " (known)" } else { "" };
        md.push_str(&format!("- {f} · {w} · {p}{k}: seed {seed}, {ex}\n"));
    }
    md.push_str(&format!("\n## Model-space objects generated ({} seeds)\n\n{types:?}\n", seeds.len()));
    md.push_str(&format!("\n## Commands that refused\n\n- {}\n", refused.into_iter().collect::<Vec<_>>().join("\n- ")));
    let out = option_env!("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from).join("savefuzz.md");
    let _ = std::fs::write(&out, &md);
    if std::env::var("SAVEFUZZ_REPORT").is_ok() {
        println!("{md}\n(report: {})", out.display());
    }
    let new: Vec<String> =
        found.iter().filter(|((f, w, p), _)| !known(f, w, p)).map(|((f, w, p), (s, ex))| format!("{f}|{w}|{p} (seed {s}: {ex})")).collect();
    assert!(new.is_empty(), "{} difference(s) the exchange notes don't report (see {}):\n{}", new.len(), out.display(), new.join("\n"));
}

/// The comparison itself: what the notes leave out (a zero coordinate that became something, a
/// description emptied, a block's base moved) is found; what they report (a coordinate changed)
/// is not.
#[test]
fn the_comparison_finds_what_the_notes_leave_out() {
    let mut d = Drawing::new_metric();
    let line = d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(5.0, 5.0, 0.0) })).expect("a line");
    d.layers.push(cadaddy_engine::doc::Layer { name: "Walls".into(), description: "outer".into(), ..Default::default() });
    let mut b = Block::new("B");
    b.base = Vec3::new(1.0, 2.0, 0.0);
    d.blocks.insert("B".into(), Arc::new(b));
    let mut back = d.clone();
    if let Some(e) = back.model.get(line) {
        let mut e = (**e).clone();
        if let EntityKind::Line(l) = &mut e.kind {
            l.a = Vec3::new(0.0, 3.0, 0.0);
            l.b = Vec3::new(6.0, 5.0, 0.0);
        }
        back.model.replace(e);
    }
    if let Some(l) = back.layers.iter_mut().find(|l| l.name == "Walls") {
        l.description.clear();
    }
    if let Some(b) = back.blocks.get_mut("B") {
        Arc::make_mut(b).base = Vec3::ZERO;
    }
    let notes = losses_between(&d, &back);
    let found = unreported(&d, &back, &notes);
    let paths: BTreeSet<String> = found.keys().map(|(w, p)| format!("{w}|{p}")).collect();
    assert!(paths.contains("model line|kind.a.y"), "the zero that became 3: {paths:?}");
    assert!(!paths.contains("model line|kind.b.x"), "the notes report b: {paths:?} {notes:?}");
    assert!(notes.iter().any(|l| l.feature == "line b"), "{notes:?}");
    // Reported by the notes now (block properties and descriptions): not left out.
    assert!(notes.iter().any(|l| l.feature == "blocks: base"), "{notes:?}");
    assert!(!paths.iter().any(|p| p.starts_with("block|base")), "{paths:?}");
    assert!(notes.iter().any(|l| l.feature == "layers: description"), "{notes:?}");
}

/// What the notes now report (savefuzz): a thickness of 1e-10 a format drops (rays have none in
/// DXF; the notes compared numbers to an absolute 1e-9), and a block's insert units and
/// explodability in a DWG older than 2007 (the notes didn't compare block definitions' own
/// properties, and took a false that came back true as unchanged).
#[test]
fn notes_report_tiny_numbers_and_block_properties() {
    let mut d = Drawing::new_metric();
    let ray = d
        .add(
            &Space::Model,
            Common { thickness: 1e-10, ..Common::default() },
            EntityKind::Ray(cadaddy_engine::doc::RayLine { base: Vec3::ZERO, dir: Vec3::new(1.0, 0.0, 0.0) }),
        )
        .expect("a ray");
    let mut b = Block::new("Unit block");
    b.units = 4;
    b.explodable = false;
    d.blocks.insert(b.name.clone(), Arc::new(b));
    let opts = |v| SaveOptions { version: Some(v), binary: false };
    let back = cadaddy_io::read(&cadaddy_io::write_with(&d, "x.dxf", &opts(Version::R2018)).expect("DXF"), "x.dxf").expect("read");
    let notes = losses_between(&d, &back);
    assert!(notes.iter().any(|l| l.feature == "object thickness" && l.handles.contains(&ray.hex())), "{notes:?}");
    for engine in [DwgEngine::Bridge, DwgEngine::Native] {
        let old = cadaddy_io::read_dwg(&cadaddy_io::write_dwg(&d, Version::R2000, engine).expect("DWG"), engine).expect("read");
        let notes = losses_between(&d, &old);
        for f in ["blocks: units", "blocks: explodable"] {
            assert!(notes.iter().any(|l| l.feature == f && l.names == ["Unit block"]), "{engine:?} {f}: {notes:?}");
        }
        let new = cadaddy_io::read_dwg(&cadaddy_io::write_dwg(&d, Version::R2018, engine).expect("DWG"), engine).expect("read");
        let notes = losses_between(&d, &new);
        assert!(!notes.iter().any(|l| l.feature.starts_with("blocks")), "{engine:?} 2018 keeps them: {notes:?}");
    }
}
