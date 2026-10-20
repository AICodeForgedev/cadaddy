//! The synthetic drawing corpus: large, varied, made-up floor plans, seeded. Walls (closed
//! polylines and partitions), doors and furniture (blocks), room tags (a block with attributes),
//! dimensions, hatched floor areas, notes, a door schedule (a table), an external reference and
//! two layouts with viewports. Built through the commands' JSON forms, as an agent would, then
//! tiled by COPY to the size asked for. No real job, place or person: the names are generic.
//!
//! Shared by `tests/corpus_roundtrip.rs` (small sizes, in CI) and `examples/corpus.rs` (10k–500k
//! objects, `cargo xtask corpus`).

#![allow(dead_code)]

use std::time::Instant;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Space};
use serde_json::{Value, json};

/// A small, fast, seeded generator (SplitMix64).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x5DEE_CE66_D1CE_4E5B)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    /// A value in `lo..hi`, rounded to a quarter.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        let u = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        ((lo + (hi - lo) * u) * 4.0).round() / 4.0
    }
}

const ROOMS: &[&str] = &["Office", "Storage", "Lab", "Lobby", "Break Room", "Copy", "Conference", "Mechanical", "Workroom", "Training"];
const LAYERS: &[&str] = &["A-WALL", "A-DOOR", "A-FURN", "A-AREA-PATT", "A-ANNO-DIMS", "A-ANNO-TEXT", "A-ANNO-TAGS", "A-ANNO-SCHD"];

/// A JSON call that must work: the generator's own mistakes fail loudly.
fn run(s: &mut Session, id: &str, params: Value) -> Value {
    match s.execute(id, &params) {
        Ok(v) => v,
        Err(e) => panic!("corpus: {id} {params}: {e}"),
    }
}

fn layer(s: &mut Session, name: &str) {
    run(s, "setvar", json!({ "name": "CLAYER", "value": name }));
}

/// The object a command made: `{handle}` or the first of `{handles}`.
fn handle(v: &Value) -> String {
    v["handle"].as_str().or_else(|| v["handles"][0].as_str()).map(str::to_string).unwrap_or_else(|| panic!("corpus: no handle in {v}"))
}

/// The blocks every plan uses: a door (leaf and swing), a desk and a room tag with NAME and NUMBER
/// attributes.
fn blocks(s: &mut Session) {
    layer(s, "0");
    let leaf = handle(&run(s, "line", json!({ "points": [[0, 0], [0, 0.9]] })));
    let swing = handle(&run(s, "arc", json!({ "center": [0, 0], "radius": 0.9, "start": 0, "end": 90 })));
    run(s, "block", json!({ "name": "DOOR-900", "base": [0, 0], "handles": [leaf, swing] }));
    let top = handle(&run(s, "pline", json!({ "vertices": [[0, 0], [1.6, 0], [1.6, 0.8], [0, 0.8]], "closed": true })));
    let chair = handle(&run(s, "circle", json!({ "center": [0.8, 1.1], "radius": 0.25 })));
    run(s, "block", json!({ "name": "DESK", "base": [0, 0], "handles": [top, chair] }));
    let ring = handle(&run(s, "circle", json!({ "center": [0, 0], "radius": 0.6 })));
    let name = handle(&run(s, "attdef", json!({ "tag": "NAME", "prompt": "Room name", "default": "Room", "at": [-0.45, 0.05], "height": 0.15 })));
    let number =
        handle(&run(s, "attdef", json!({ "tag": "NUMBER", "prompt": "Room number", "default": "000", "at": [-0.2, -0.25], "height": 0.15 })));
    run(s, "block", json!({ "name": "ROOMTAG", "base": [0, 0], "handles": [ring, name, number] }));
}

/// One unit of plan at `(ox, oy)`: its outline and partitions, doors, a desk or two per room, room
/// tags, a hatched floor area, dimensions and a note. About 30–45 objects.
fn module(s: &mut Session, r: &mut Rng, ox: f64, oy: f64, first_room: &mut usize) {
    let (w, d) = (r.range(9.0, 15.0), r.range(6.0, 10.0));
    let rooms = 2 + r.below(3);
    layer(s, "A-WALL");
    let outline = [[ox, oy], [ox + w, oy], [ox + w, oy + d], [ox, oy + d]];
    run(s, "pline", json!({ "vertices": outline, "closed": true }));
    let t = 0.2;
    run(s, "pline", json!({ "vertices": [[ox + t, oy + t], [ox + w - t, oy + t], [ox + w - t, oy + d - t], [ox + t, oy + d - t]], "closed": true }));
    let mut xs = vec![ox];
    for i in 1..rooms {
        let x = ox + w * i as f64 / rooms as f64;
        run(s, "line", json!({ "points": [[x, oy + t], [x, oy + d - t]] }));
        xs.push(x);
    }
    xs.push(ox + w);
    for pair in xs.windows(2) {
        let [a, b] = pair else { continue };
        let (cx, cy) = ((a + b) / 2.0, oy + d / 2.0);
        layer(s, "A-DOOR");
        let swing = [0, 90, 180, 270][r.below(4)];
        run(s, "insert", json!({ "name": "DOOR-900", "at": [a + 0.4, oy + t], "rotation": swing }));
        layer(s, "A-FURN");
        for _ in 0..1 + r.below(2) {
            let (x, y, turn) = (r.range(a + 0.5, b - 2.2), r.range(oy + 1.0, oy + d - 2.0), [0, 90][r.below(2)]);
            run(s, "insert", json!({ "name": "DESK", "at": [x, y], "rotation": turn }));
        }
        layer(s, "A-ANNO-TAGS");
        *first_room += 1;
        let name = ROOMS[r.below(ROOMS.len())];
        run(s, "insert", json!({ "name": "ROOMTAG", "at": [cx, cy], "attribs": { "NAME": name, "NUMBER": format!("{:03}", *first_room) } }));
    }
    layer(s, "A-AREA-PATT");
    let (a, b) = (xs[0] + t, xs[1] - t * 0.5);
    let floor = handle(&run(s, "pline", json!({ "vertices": [[a, oy + t], [b, oy + t], [b, oy + d - t], [a, oy + d - t]], "closed": true })));
    // Scaled as a floor finish is drawn: some tens of pattern lines across a room.
    let (pattern, scale) = [("ANSI31", 3.0), ("ANSI37", 4.0), ("SOLID", 1.0), ("DOTS", 6.0)][r.below(4)];
    run(s, "hatch", json!({ "handles": [floor], "pattern": pattern, "scale": scale }));
    layer(s, "A-ANNO-DIMS");
    run(s, "dimlinear", json!({ "p1": [ox, oy], "p2": [ox + w, oy], "at": [ox + w / 2.0, oy - 1.0] }));
    run(s, "dimlinear", json!({ "p1": [ox + w, oy], "p2": [ox + w, oy + d], "at": [ox + w + 1.0, oy + d / 2.0] }));
    layer(s, "A-ANNO-TEXT");
    run(
        s,
        "mtext",
        json!({ "at": [ox + 0.3, oy - 1.6], "text": format!("UNIT {}\\PFINISH: {pattern}", *first_room / 10 + 1), "height": 0.2, "width": 4 }),
    );
}

/// What was made, for the report.
pub struct Made {
    pub drawing: Drawing,
    pub objects: usize,
    pub seconds: f64,
}

/// A drawing of about `target` model-space objects (at least the plan's first rows), from `seed`.
pub fn generate(target: usize, seed: u64) -> Made {
    let started = Instant::now();
    // The engine reads the referenced file through the file formats, as the app gives it them.
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    let mut r = Rng::new(seed);
    let mut s = Session::new();
    // The same seed, the same drawing: its fingerprint from the seed, not the clock.
    if let Ok(d) = s.doc_mut() {
        d.header.set_str("FINGERPRINTGUID", &format!("{{00000000-0000-4000-8000-{:012X}}}", seed & 0xFFFF_FFFF_FFFF));
    }
    for name in LAYERS {
        if let Ok(d) = s.doc_mut() {
            d.ensure_layer(name);
        }
    }
    blocks(&mut s);
    // A first block of plan: a few rows of varied units.
    let mut room = 100;
    let (cols, rows) = (6, 3);
    let (pitch_x, pitch_y) = (18.0, 14.0);
    for row in 0..rows {
        for col in 0..cols {
            module(&mut s, &mut r, col as f64 * pitch_x, row as f64 * pitch_y, &mut room);
        }
    }
    let count = |s: &Session| s.doc().map_or(0, |d| d.model.len());
    // Tiled up to the size asked: COPY of everything, upwards then sideways.
    let block_h = rows as f64 * pitch_y;
    let base = count(&s).max(1);
    let copies_up = target.div_ceil(base).clamp(1, 40);
    if copies_up > 1 {
        let handles = all(&s);
        run(&mut s, "copy", json!({ "handles": handles, "delta": [0, block_h], "count": copies_up - 1 }));
    }
    let column = count(&s).max(1);
    let copies_side = target.div_ceil(column).max(1);
    if copies_side > 1 {
        let handles = all(&s);
        run(&mut s, "copy", json!({ "handles": handles, "delta": [cols as f64 * pitch_x, 0], "count": copies_side - 1 }));
    }
    // A door schedule, an external reference (made-up, not found: kept as a reference) and the
    // sheets.
    layer(&mut s, "A-ANNO-SCHD");
    let cells: Vec<Vec<String>> =
        (0..12).map(|i| vec![format!("D{:02}", i + 1), "900 x 2100".into(), ["Wood", "Hollow metal"][i % 2].into()]).collect();
    run(
        &mut s,
        "table",
        json!({ "at": [-30, 40], "rows": 12, "cols": 3, "title": "DOOR SCHEDULE", "header": ["MARK", "SIZE", "TYPE"], "cells": cells }),
    );
    layer(&mut s, "0");
    let grid = grid_file(seed, cols as f64 * pitch_x * copies_side as f64, block_h * copies_up as f64);
    run(&mut s, "xattach", json!({ "path": grid, "at": [0, 0] }));
    // Loaded now; the drawing keeps where it was (the round trips compare that, not its objects).
    if let Some(dir) = std::path::Path::new(&grid).parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
    for name in ["A-101 Floor Plan", "A-102 Enlarged Plan"] {
        run(&mut s, "layout.new", json!({ "name": name }));
    }
    let objects = count(&s);
    let drawing = s.doc().cloned().unwrap_or_else(|_| Drawing::new_imperial());
    Made { drawing, objects, seconds: started.elapsed().as_secs_f64() }
}

/// Model space's objects counted by kind.
pub fn kinds(d: &Drawing) -> std::collections::BTreeMap<String, usize> {
    let mut m = std::collections::BTreeMap::new();
    for e in d.model.iter() {
        *m.entry(format!("{:?}", e.kind).split(['(', ' ', '{']).next().unwrap_or_default().to_string()).or_insert(0) += 1;
    }
    m
}

/// A made-up consultant's column grid (lines, bubbles and their labels) as a DXF file in a temporary
/// folder, for the plan to reference: its path.
fn grid_file(seed: u64, w: f64, h: f64) -> String {
    let mut g = Session::new();
    let step = 9.0;
    for (i, x) in (0..).map(|i| f64::from(i) * step).take_while(|x| *x <= w).enumerate() {
        run(&mut g, "line", json!({ "points": [[x, -3.0], [x, h + 3.0]] }));
        run(&mut g, "circle", json!({ "center": [x, h + 4.0], "radius": 1.0 }));
        run(&mut g, "text", json!({ "at": [x, h + 4.0], "text": (i + 1).to_string(), "height": 0.8, "justify": "MC" }));
    }
    for (i, y) in (0..).map(|i| f64::from(i) * step).take_while(|y| *y <= h).enumerate() {
        run(&mut g, "line", json!({ "points": [[-3.0, y], [w + 3.0, y]] }));
        let label = char::from(b'A' + (i % 26) as u8).to_string();
        run(&mut g, "text", json!({ "at": [-4.0, y], "text": label, "height": 0.8, "justify": "MC" }));
    }
    let dir = std::env::temp_dir().join(format!("cadaddy-corpus-{}-{seed}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("consultant-grid.dxf");
    let bytes = g.doc().ok().and_then(|d| cadaddy_io::write(d, "grid.dxf").ok()).unwrap_or_default();
    if let Err(e) = std::fs::write(&path, bytes) {
        panic!("corpus: {}: {e}", path.display());
    }
    path.to_string_lossy().into_owned()
}

fn all(s: &Session) -> Vec<String> {
    s.doc().map(|d| d.model.iter().map(|e| e.handle.hex()).collect()).unwrap_or_default()
}

/// How `b` (read back) differs from `a` (written): per object kind, the first field that differs
/// and how many objects differ that way, with an example; and the tables and layouts.
pub fn differences(a: &Drawing, b: &Drawing) -> Vec<String> {
    use std::collections::BTreeMap;
    let mut out = Vec::new();
    let kind = |e: &cadaddy_engine::doc::Entity| format!("{:?}", e.kind).split(['(', ' ', '{']).next().unwrap_or_default().to_string();
    // Objects by handle, in every space.
    let mut ways: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let spaces: Vec<Space> = std::iter::once(Space::Model).chain(a.layouts.iter().map(|l| Space::Paper(l.name.clone()))).collect();
    for space in &spaces {
        let (Some(sa), sb) = (a.space(space), b.space(space)) else { continue };
        for e in sa.iter() {
            let Some(f) = sb.and_then(|st| st.get(e.handle)) else {
                let entry = ways.entry(format!("{} missing ({space:?})", kind(e))).or_default();
                entry.0 += 1;
                continue;
            };
            let (x, y) = (format!("{:#?}", e), format!("{:#?}", f));
            if x != y {
                let (path, example) = first_difference(&x, &y);
                let entry = ways.entry(format!("{} {path}", kind(e))).or_insert((0, example));
                entry.0 += 1;
            }
        }
        if let (Some(sb), sa) = (sb, a.space(space)) {
            let extra = sb.iter().filter(|f| sa.is_some_and(|st| st.get(f.handle).is_none())).count();
            if extra > 0 {
                out.push(format!("{extra} object(s) more in {space:?} after reading"));
            }
        }
    }
    for (way, (n, example)) in ways {
        out.push(format!("{n} × {way}: {example}"));
    }
    let names = |d: &Drawing| -> Vec<String> {
        let mut v: Vec<String> = d.layers.iter().map(|l| format!("layer {}", l.name)).collect();
        v.extend(d.blocks.keys().map(|k| format!("block {k}")));
        v.extend(d.layouts.iter().map(|l| format!("layout {}", l.name)));
        v
    };
    let (na, nb) = (names(a), names(b));
    for n in na.iter().filter(|n| !nb.contains(n)) {
        out.push(format!("{n} missing after reading"));
    }
    // Named records: the same name, compared whole.
    fn records<T: std::fmt::Debug>(out: &mut Vec<String>, what: &str, a: &[T], b: &[T], name: impl Fn(&T) -> String) {
        for x in a {
            let Some(y) = b.iter().find(|y| name(y) == name(x)) else { continue };
            let (dx, dy) = (format!("{x:#?}"), format!("{y:#?}"));
            if dx != dy {
                let (path, example) = first_difference(&dx, &dy);
                out.push(format!("{what} {} {path}: {example}", name(x)));
            }
        }
    }
    records(&mut out, "layer", &a.layers, &b.layers, |l| l.name.clone());
    records(&mut out, "linetype", &a.linetypes, &b.linetypes, |l| l.name.clone());
    records(&mut out, "text style", &a.text_styles, &b.text_styles, |l| l.name.clone());
    records(&mut out, "dimension style", &a.dim_styles, &b.dim_styles, |l| l.name.clone());
    records(&mut out, "table style", &a.table_styles, &b.table_styles, |l| l.name.clone());
    records(&mut out, "layout", &a.layouts, &b.layouts, |l| l.name.clone());
    // Block definitions: their objects by position (handles inside blocks may be given anew).
    for (name, x) in &a.blocks {
        let Some(y) = b.blocks.get(name) else { continue };
        // An external reference's objects come from its file when the drawing is opened; a file
        // keeps only where it is.
        if x.xref_path.is_some() {
            if x.xref_path != y.xref_path {
                out.push(format!("xref {name}: path {:?} → {:?}", x.xref_path, y.xref_path));
            }
            continue;
        }
        let (ex, ey): (Vec<_>, Vec<_>) = (x.entities.iter().collect(), y.entities.iter().collect());
        if ex.len() != ey.len() {
            out.push(format!("block {name}: {} objects → {}", ex.len(), ey.len()));
            continue;
        }
        for (p, q) in ex.iter().zip(&ey) {
            let (dx, dy) = (format!("{:#?}", p.kind), format!("{:#?}", q.kind));
            if dx != dy {
                let (path, example) = first_difference(&dx, &dy);
                out.push(format!("block {name} {path}: {example}"));
                break;
            }
        }
    }
    // Header variables: changed, lost and added.
    let (ha, hb) = (&a.header.vars, &b.header.vars);
    let changed: Vec<String> =
        ha.iter().filter(|(k, v)| hb.get(*k).is_some_and(|w| w != *v)).map(|(k, v)| format!("{k} {v:?} → {:?}", hb.get(k))).collect();
    let lost: Vec<&String> = ha.keys().filter(|k| !hb.contains_key(*k)).collect();
    let added: Vec<&String> = hb.keys().filter(|k| !ha.contains_key(*k)).collect();
    if !changed.is_empty() {
        out.push(format!("{} header variable(s) changed: {}", changed.len(), changed.iter().take(6).cloned().collect::<Vec<_>>().join("; ")));
    }
    if !lost.is_empty() {
        out.push(format!("{} header variable(s) lost: {:?}", lost.len(), lost.iter().take(12).collect::<Vec<_>>()));
    }
    if !added.is_empty() {
        out.push(format!("{} header variable(s) added: {:?}", added.len(), added.iter().take(12).collect::<Vec<_>>()));
    }
    out
}

/// The field path and the old → new lines where two pretty `Debug` forms first differ.
fn first_difference(a: &str, b: &str) -> (String, String) {
    let mut path: Vec<String> = Vec::new();
    for (x, y) in a.lines().zip(b.lines()) {
        if x != y {
            return (path.join("."), format!("{} → {}", x.trim(), y.trim()));
        }
        let depth = (x.len() - x.trim_start().len()) / 4;
        path.truncate(depth);
        if x.trim_end().ends_with(['{', '[', '(']) {
            path.push(x.trim().split([':', ' ', '(']).next().unwrap_or_default().to_string());
        }
    }
    (String::from("(length)"), format!("{} → {} lines", a.lines().count(), b.lines().count()))
}

/// One format's round trip: (write s, bytes, read s, regen s of what was read, differences).
pub struct Trip {
    pub format: &'static str,
    pub save: f64,
    pub bytes: usize,
    pub open: f64,
    pub regen: f64,
    pub differences: Vec<String>,
}

pub fn round_trip(d: &Drawing, format: &'static str) -> Result<Trip, String> {
    // "dwg-native": DWG with CADaddy's own codec (Q203); "dwg" is the default engine's.
    let native = format == "dwg-native";
    let name = if native { "corpus.dwg".to_string() } else { format!("corpus.{format}") };
    let t = Instant::now();
    let bytes =
        if native { cadaddy_io::write_dwg(d, cadaddy_io::Version::R2018, cadaddy_io::DwgEngine::Native) } else { cadaddy_io::write(d, &name) }
            .map_err(|e| format!("save {format}: {e}"))?;
    let save = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let back = if native { cadaddy_io::read_dwg(&bytes, cadaddy_io::DwgEngine::Native) } else { cadaddy_io::read(&bytes, &name) }
        .map_err(|e| format!("open {format}: {e}"))?;
    let open = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let built = cadaddy_engine::render::build(&back, &Space::Model, &cadaddy_engine::render::Options::default());
    let regen = t.elapsed().as_secs_f64();
    std::hint::black_box(built);
    Ok(Trip { format, save, bytes: bytes.len(), open, regen, differences: differences(d, &back) })
}
