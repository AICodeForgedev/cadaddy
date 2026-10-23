//! Undo/redo invariants under random edits: seeded sequences of 20–200 commands (drawing,
//! modifying, layers, blocks, dimensions, hatches, tables, styles, system variables, 3D solids),
//! with UNDO BEgin/End groups and UNDO Marks among them, on a made-up drawing. Checked:
//!
//! - a command that is refused leaves the drawing and the undo stack as they were;
//! - a command that changes the drawing adds exactly one undo step (none when it changes nothing);
//! - a BEgin … End group is one undo step;
//! - UNDO Back returns to the last mark exactly;
//! - undoing everything gives the start drawing exactly (handles, tables, header variables), and
//!   redoing everything gives the final drawing exactly.
//!
//! A failing sequence is shrunk to the fewest operations that still fail and printed as a list
//! ready to paste into a test (`replay` below). Seeded: `UNDOFUZZ_SEED` (default 1),
//! `UNDOFUZZ_SEQS` (sequences, default 200), `UNDOFUZZ_MAX` (longest sequence, default 200).

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, EntityKind, Handle};
use serde_json::{Value, json};

/// A synthetic xref DXF written to a temp directory for the xref operations in the fuzz.
fn xref_path() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("undofuzz_xref");
    let path = dir.join("Fixture.dxf");
    if !path.exists() {
        std::fs::create_dir_all(&dir).ok();
        // Build a small made-up drawing via the engine, then write it as DXF.
        let mut s = Session::new();
        s.execute("rectang", &json!({ "p1": [0, 0], "p2": [10, 8] })).unwrap();
        s.execute("text", &json!({ "at": [2, 3], "text": "Ref Drawing", "height": 1 })).unwrap();
        let dxf = cadaddy_io::write_dxf(s.doc().unwrap());
        std::fs::write(&path, dxf).expect("write xref fixture");
    }
    path
}

/// A small, fast, seeded generator (SplitMix64).
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
    /// A coordinate on the made-up drawing (0..100), on a 0.5 grid.
    fn c(&mut self) -> f64 {
        self.below(200) as f64 / 2.0
    }
    fn pt(&mut self) -> [f64; 2] {
        [self.c(), self.c()]
    }
}

/// The operations, by kind; each runs with parameters drawn from its own seed and the drawing as
/// it is when it runs (so a shrunk sequence still makes sense).
const KINDS: &[&str] = &[
    "line",
    "circle",
    "arc",
    "pline",
    "rectang",
    "text",
    "mtext",
    "point",
    "ellipse",
    "spline",
    "hatch",
    "dimlinear",
    "mleader",
    "move",
    "copy",
    "rotate",
    "scale",
    "mirror",
    "erase",
    "offset",
    "explode",
    "arrayrect",
    "properties.set",
    "layer.new",
    "layer.set",
    "layer.current",
    "block",
    "insert",
    "attdef",
    "setvar",
    "box",
    "cylinder",
    "extrude",
    "table",
    "fillet",
    "group",
    "draworder",
    "donut",
    "polygon",
    "style",
    "rename",
    "break",
    "join",
    "stretch",
    "layer.delete",
    "layout",
    "layout.set",
    "mview",
    "purge",
    "dimstyle",
    "UNDO BEgin",
    "UNDO End",
    "UNDO Mark",
    "xattach",
    "-xref detach",
    "-xref reload",
    "-xref unload",
    "-xref bind",
];

#[derive(Clone, Copy, Debug)]
struct Op {
    kind: usize,
    seed: u64,
}

/// One of `xs`, by `r`.
fn one<T: Copy>(r: &mut Rng, xs: &[T]) -> T {
    xs[r.below(xs.len())]
}

fn handles(s: &Session) -> Vec<Handle> {
    let space = s.space();
    s.doc().ok().and_then(|d| d.space(&space)).map(|st| st.iter().map(|e| e.handle).collect()).unwrap_or_default()
}

/// The objects of the current space `keep` accepts.
fn of(s: &Session, keep: impl Fn(&EntityKind) -> bool) -> Vec<Handle> {
    let space = s.space();
    s.doc().ok().and_then(|d| d.space(&space)).map(|st| st.iter().filter(|e| keep(&e.kind)).map(|e| e.handle).collect()).unwrap_or_default()
}

fn hex(hs: &[Handle]) -> Vec<String> {
    hs.iter().map(|h| h.hex()).collect()
}

/// Up to `n` objects picked from `from`.
fn some(r: &mut Rng, from: &[Handle], n: usize) -> Vec<Handle> {
    let mut v: Vec<Handle> = (0..1 + r.below(n)).filter_map(|_| from.get(r.below(from.len())).copied()).collect();
    v.sort_by_key(|h| h.0);
    v.dedup();
    v
}

fn layer_names(s: &Session) -> Vec<String> {
    s.doc().map(|d| d.layers.iter().map(|l| l.name.clone()).collect()).unwrap_or_default()
}

fn block_names(s: &Session) -> Vec<String> {
    s.doc().map(|d| d.blocks.values().filter(|b| !b.anonymous && !b.name.starts_with('*')).map(|b| b.name.clone()).collect()).unwrap_or_default()
}

/// The command and parameters of `op` on the drawing as it is now.
fn command(s: &Session, op: Op) -> (&'static str, Value) {
    let mut r = Rng(op.seed);
    let all = handles(s);
    let kind = KINDS.get(op.kind).copied().unwrap_or("line");
    let p = match kind {
        "line" => json!({ "points": [r.pt(), r.pt(), r.pt()] }),
        "circle" => json!({ "center": r.pt(), "radius": 1.0 + r.below(20) as f64 }),
        "arc" => json!({ "center": r.pt(), "radius": 1.0 + r.below(10) as f64, "start": r.below(360), "end": r.below(360) }),
        "pline" => json!({ "vertices": [r.pt(), r.pt(), r.pt(), r.pt()], "closed": r.below(2) == 0 }),
        "rectang" => {
            let a = r.pt();
            json!({ "p1": a, "p2": [a[0] + 1.0 + r.below(20) as f64, a[1] + 1.0 + r.below(20) as f64] })
        }
        "text" => json!({ "at": r.pt(), "text": format!("Note {}", r.below(100)), "height": 1 + r.below(3) }),
        "mtext" => json!({ "at": r.pt(), "text": "Room A\\PLine two", "height": 1, "width": 10 + r.below(20) }),
        "point" => json!({ "at": r.pt() }),
        "ellipse" => json!({ "center": r.pt(), "major": [2.0 + r.below(8) as f64, 0.0], "ratio": 0.5 }),
        "spline" => json!({ "fit": [r.pt(), r.pt(), r.pt(), r.pt()] }),
        "hatch" => {
            let closed = of(s, |k| matches!(k, EntityKind::Circle(_)) || matches!(k, EntityKind::LwPolyline(p) if p.closed));
            json!({ "handles": hex(&some(&mut r, &closed, 1)), "pattern": one(&mut r, &["ANSI31", "SOLID", "ANSI37"]) })
        }
        "dimlinear" => {
            let a = r.pt();
            json!({ "p1": a, "p2": [a[0] + 5.0, a[1]], "at": [a[0] + 2.0, a[1] - 3.0] })
        }
        "mleader" => json!({ "points": [r.pt(), r.pt()], "text": "Tag" }),
        "move" => json!({ "handles": hex(&some(&mut r, &all, 4)), "delta": [r.c() - 50.0, r.c() - 50.0] }),
        "copy" => json!({ "handles": hex(&some(&mut r, &all, 4)), "delta": [r.c() - 50.0, r.c() - 50.0] }),
        "rotate" => json!({ "handles": hex(&some(&mut r, &all, 4)), "base": r.pt(), "angle": r.below(360) }),
        "scale" => json!({ "handles": hex(&some(&mut r, &all, 4)), "base": r.pt(), "factor": one(&mut r, &[0.5, 2.0, 1.5]) }),
        "mirror" => json!({ "handles": hex(&some(&mut r, &all, 4)), "p1": r.pt(), "p2": r.pt(), "erase": r.below(2) == 0 }),
        "erase" => json!({ "handles": hex(&some(&mut r, &all, 3)) }),
        "offset" => {
            let curves = of(s, |k| matches!(k, EntityKind::Line(_) | EntityKind::Circle(_) | EntityKind::Arc(_) | EntityKind::LwPolyline(_)));
            json!({ "handle": some(&mut r, &curves, 1).first().map(|h| h.hex()), "distance": 1 + r.below(3), "side": r.pt() })
        }
        "explode" => json!({ "handles": hex(&some(&mut r, &all, 2)) }),
        "arrayrect" => {
            json!({ "handles": hex(&some(&mut r, &all, 2)), "rows": 2, "cols": 1 + r.below(3), "rowSpacing": 5, "colSpacing": 5, "associative": r.below(2) == 0 })
        }
        "properties.set" => {
            let layers = layer_names(s);
            let mut p = json!({ "handles": hex(&some(&mut r, &all, 4)) });
            match r.below(3) {
                0 => p["color"] = json!(1 + r.below(7)),
                1 => p["layer"] = json!(layers.get(r.below(layers.len())).cloned().unwrap_or_else(|| "0".into())),
                _ => p["lineweight"] = if r.below(3) == 0 { json!("ByLayer") } else { json!(one(&mut r, &[0.25, 0.5])) },
            }
            p
        }
        "layer.new" => json!({ "name": format!("Layer {}", r.below(8)), "color": 1 + r.below(7) }),
        "layer.set" => {
            let layers = layer_names(s);
            let name = layers.get(r.below(layers.len())).cloned().unwrap_or_else(|| "0".into());
            match r.below(4) {
                0 => json!({ "name": name, "color": 1 + r.below(7) }),
                1 => json!({ "name": name, "on": r.below(2) == 0 }),
                2 => json!({ "name": name, "frozen": r.below(2) == 0 }),
                _ => json!({ "name": name, "locked": r.below(2) == 0 }),
            }
        }
        "layer.current" => {
            let layers = layer_names(s);
            json!({ "name": layers.get(r.below(layers.len())).cloned().unwrap_or_else(|| "0".into()) })
        }
        "block" => {
            json!({ "name": format!("Part {}", r.below(6)), "base": r.pt(), "handles": hex(&some(&mut r, &all, 3)), "keep": one(&mut r, &["convert", "retain", "delete"]) })
        }
        "insert" => {
            let blocks = block_names(s);
            json!({ "name": blocks.get(r.below(blocks.len())).cloned().unwrap_or_else(|| "Part 0".into()), "at": r.pt(), "scale": one(&mut r, &[1, 2]), "rotation": r.below(4) * 90 })
        }
        "attdef" => json!({ "tag": format!("TAG{}", r.below(4)), "default": "101", "at": r.pt(), "height": 1 }),
        "setvar" => match r.below(4) {
            0 => json!({ "name": "FILLETRAD", "value": r.below(5) }),
            1 => json!({ "name": "TEXTSIZE", "value": 0.5 + r.below(4) as f64 }),
            2 => json!({ "name": "LTSCALE", "value": 1 + r.below(3) }),
            _ => json!({ "name": "PDMODE", "value": one(&mut r, &[0, 3, 34]) }),
        },
        "box" => json!({ "corner": r.pt(), "length": 1 + r.below(10), "width": 1 + r.below(10), "height": 1 + r.below(10) }),
        "cylinder" => json!({ "center": r.pt(), "radius": 1 + r.below(5), "height": 1 + r.below(10) }),
        "extrude" => {
            let closed = of(s, |k| matches!(k, EntityKind::Circle(_)) || matches!(k, EntityKind::LwPolyline(p) if p.closed));
            json!({ "handles": hex(&some(&mut r, &closed, 1)), "height": 1 + r.below(5), "keep": r.below(2) == 0 })
        }
        "table" => json!({ "at": r.pt(), "rows": 1 + r.below(3), "cols": 1 + r.below(3), "title": "Schedule" }),
        "fillet" => {
            let plines = of(s, |k| matches!(k, EntityKind::LwPolyline(_)));
            json!({ "handle": some(&mut r, &plines, 1).first().map(|h| h.hex()), "polyline": true, "radius": 0.5 })
        }
        "group" => json!({ "handles": hex(&some(&mut r, &all, 3)) }),
        "draworder" => json!({ "handles": hex(&some(&mut r, &all, 2)), "order": one(&mut r, &["front", "back"]) }),
        "donut" => json!({ "center": r.pt(), "inside": 1, "outside": 2 }),
        "polygon" => json!({ "sides": 3 + r.below(5), "center": r.pt(), "radius": 1 + r.below(5) }),
        "style" => json!({ "name": format!("Style {}", r.below(3)), "height": 0.0, "widthFactor": one(&mut r, &[1.0, 0.8]) }),
        "rename" => {
            let layers: Vec<String> = layer_names(s).into_iter().filter(|l| l != "0" && !l.eq_ignore_ascii_case("Defpoints")).collect();
            json!({ "table": "layer", "from": layers.get(r.below(layers.len())).cloned().unwrap_or_else(|| "Layer 1".into()), "to": format!("Layer {}", 10 + r.below(5)) })
        }
        "break" => {
            let lines = of(s, |k| matches!(k, EntityKind::Line(_)));
            let h = some(&mut r, &lines, 1).first().copied();
            let (a, b) = match h.and_then(|h| s.doc().ok()?.entity(h).map(|e| e.kind.clone())) {
                Some(EntityKind::Line(l)) => (l.a.xy() + (l.b.xy() - l.a.xy()) * 0.3, l.a.xy() + (l.b.xy() - l.a.xy()) * 0.6),
                _ => (cadaddy_engine::geom::Vec2::ZERO, cadaddy_engine::geom::Vec2::ZERO),
            };
            json!({ "handle": h.map(|h| h.hex()), "p1": [a.x, a.y], "p2": [b.x, b.y] })
        }
        "join" => json!({ "handles": hex(&some(&mut r, &all, 4)) }),
        "stretch" => {
            let a = r.pt();
            json!({ "window": [a, [a[0] + 20.0, a[1] + 20.0]], "delta": [r.c() / 10.0, r.c() / 10.0] })
        }
        "layer.delete" => {
            let layers = layer_names(s);
            json!({ "name": layers.get(r.below(layers.len())).cloned().unwrap_or_else(|| "Layer 1".into()) })
        }
        "layout" => json!({ "option": "new", "name": format!("Sheet {}", r.below(3)) }),
        // Model, or a layout that may not exist (refused then).
        "layout.set" => json!({ "name": one(&mut r, &["Model", "Layout1", "Sheet 0", "Sheet 1"]) }),
        "mview" => {
            let a = r.pt();
            json!({ "layout": "Layout1", "p1": a, "p2": [a[0] + 5.0 + r.below(20) as f64, a[1] + 5.0 + r.below(20) as f64] })
        }
        "purge" => json!({ "type": one(&mut r, &["all", "blocks", "layers", "textstyles"]) }),
        "dimstyle" => json!({ "name": format!("Dims {}", r.below(3)), "DIMASZ": 0.5 + r.below(3) as f64, "current": r.below(2) == 0 }),
        "xattach" => {
            let path = xref_path();
            json!({ "path": path.to_str().unwrap_or(""), "name": format!("Ref{}", r.below(3)) })
        }
        "-xref detach" | "-xref reload" | "-xref unload" | "-xref bind" => {
            let option = kind.strip_prefix("-xref ").unwrap_or("detach");
            json!({ "option": option, "names": format!("Ref{}", r.below(3)) })
        }
        "UNDO BEgin" => json!({ "mode": "begin" }),
        "UNDO End" => json!({ "mode": "end" }),
        _ => json!({ "mode": "mark" }),
    };
    let cmd = match kind {
        k if k.starts_with("UNDO") => "undo",
        k if k.starts_with("-xref") => "-xref",
        _ => kind,
    };
    (cmd, p)
}

/// Commands run, and those refused, over all sequences (the summary line).
static RAN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static REFUSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The made-up drawing every sequence starts from.
fn start() -> Session {
    let mut s = Session::new();
    for (cmd, p) in [
        ("rectang", json!({ "p1": [0, 0], "p2": [40, 30] })),
        ("line", json!({ "points": [[0, 15], [40, 15]] })),
        ("circle", json!({ "center": [60, 20], "radius": 8 })),
        ("text", json!({ "at": [5, 5], "text": "Sample Floor Plan", "height": 2 })),
        ("layer.new", json!({ "name": "Walls", "color": 3 })),
    ] {
        s.execute(cmd, &p).unwrap();
    }
    s
}

fn undo_len(s: &Session) -> usize {
    s.state().map(|st| st.undo.len()).unwrap_or(0)
}

fn doc(s: &Session) -> Drawing {
    s.doc().cloned().unwrap_or_default()
}

/// Where two drawings first differ: the line of their pretty `Debug` form, with the field path.
fn what_changed(a: &Drawing, b: &Drawing) -> String {
    let (a, b) = (format!("{a:#?}"), format!("{b:#?}"));
    let mut path: Vec<String> = Vec::new();
    for (x, y) in a.lines().zip(b.lines()) {
        if x != y {
            return format!("at {} — {} → {}", path.join("."), x.trim(), y.trim());
        }
        let depth = x.len() - x.trim_start().len();
        let name = x.trim().split([':', ' ']).next().unwrap_or_default().to_string();
        path.truncate(depth / 4);
        if x.trim_end().ends_with(['{', '[', '(']) {
            path.push(name);
        }
    }
    "in length".into()
}

/// Run `ops` from the start drawing: `None` when every invariant held, else what broke (the
/// first word names the kind of failure, which shrinking keeps).
fn check(ops: &[Op]) -> Option<String> {
    let mut s = start();
    let start_doc = doc(&s);
    let base = undo_len(&s);
    let mut mark: Option<Drawing> = None;
    let mut group: Option<(usize, Drawing)> = None;
    for (i, op) in ops.iter().enumerate() {
        let (cmd, p) = command(&s, *op);
        let (before, steps) = (doc(&s), undo_len(&s));
        let r = s.execute(cmd, &p);
        let (after, steps2) = (doc(&s), undo_len(&s));
        let at = format!("op {i} ({cmd} {p})");
        if cmd == "undo" {
            match p["mode"].as_str() {
                // A mark ends an open group (the group is one step before it).
                Some("mark") => {
                    mark = Some(after.clone());
                    group = None;
                }
                Some("begin") => group = Some((steps2, after.clone())),
                Some("end") => {
                    // One step for the group (none, or an empty one, when nothing changed).
                    if let Some((at_begin, d)) = group.take() {
                        let want = if after == d { at_begin..=at_begin + 1 } else { at_begin + 1..=at_begin + 1 };
                        if !want.contains(&steps2) {
                            return Some(format!("group: {at}: a BEgin…End group left {} undo step(s)", steps2 as i64 - at_begin as i64));
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        RAN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if r.is_err() {
            REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if after != before {
                return Some(format!(
                    "refused: {at} was refused ({}) but changed the drawing {}",
                    r.err().map(|e| e.to_string()).unwrap_or_default(),
                    what_changed(&before, &after)
                ));
            }
            if steps2 != steps {
                return Some(format!("refused: {at} was refused but added {} undo step(s)", steps2 as i64 - steps as i64));
            }
            continue;
        }
        // Inside a group the steps are merged when the group ends.
        if group.is_none() {
            let want = if after == before { steps..=steps + 1 } else { steps + 1..=steps + 1 };
            if !want.contains(&steps2) {
                return Some(format!(
                    "steps: {at} added {} undo step(s) for a {} drawing",
                    steps2 as i64 - steps as i64,
                    if after == before { "same" } else { "changed" }
                ));
            }
        }
    }
    // An open group ends with the sequence.
    if group.is_some() {
        let _ = s.execute("undo", &json!({ "mode": "end" }));
    }
    let end_doc = doc(&s);
    if let Some(m) = mark {
        let _ = s.execute("undo", &json!({ "mode": "back" }));
        let d = doc(&s);
        if d != m {
            return Some(format!("back: UNDO Back didn't return to the mark: {}", what_changed(&m, &d)));
        }
        let _ = s.execute("mredo", &json!({ "all": true }));
        let d = doc(&s);
        if d != end_doc {
            return Some(format!("redo-back: redoing after UNDO Back didn't give the final drawing: {}", what_changed(&end_doc, &d)));
        }
    }
    let mut guard = 0;
    while undo_len(&s) > base && guard < 2000 {
        if s.execute("undo", &json!({})).is_err() {
            break;
        }
        guard += 1;
    }
    let d = doc(&s);
    if d != start_doc {
        return Some(format!("undo: undoing everything didn't give the start drawing: {}", what_changed(&start_doc, &d)));
    }
    let _ = s.execute("mredo", &json!({ "all": true }));
    let d = doc(&s);
    if d != end_doc {
        return Some(format!("redo: redoing everything didn't give the final drawing: {}", what_changed(&end_doc, &d)));
    }
    None
}

/// The kind of a failure (its first word), which a shrunk sequence must keep.
fn kind(f: &str) -> &str {
    f.split(':').next().unwrap_or(f)
}

/// The fewest operations of `ops` that still fail the same way.
fn shrink(mut ops: Vec<Op>, failure: &str) -> (Vec<Op>, String) {
    let want = kind(failure).to_string();
    let mut last = failure.to_string();
    // Halves first, then one at a time, until nothing more can go.
    let mut chunk = ops.len() / 2;
    while chunk >= 1 {
        let mut i = 0;
        let mut removed = false;
        while i < ops.len() {
            let mut t = ops.clone();
            t.drain(i..(i + chunk).min(t.len()));
            match check(&t) {
                Some(f) if kind(&f) == want => {
                    ops = t;
                    last = f;
                    removed = true;
                }
                _ => i += chunk,
            }
        }
        if !removed {
            chunk /= 2;
        }
    }
    (ops, last)
}

fn sequence(seed: u64, max: usize) -> Vec<Op> {
    let mut r = Rng(seed);
    let n = 20 + r.below(max.saturating_sub(19).max(1));
    (0..n).map(|_| Op { kind: r.below(KINDS.len()), seed: r.next() }).collect()
}

/// A shrunk failure as a test can replay it.
fn replay(ops: &[Op]) -> String {
    ops.iter().map(|o| format!("({}, {:#x})", o.kind, o.seed)).collect::<Vec<_>>().join(", ")
}

#[test]
fn undo_and_redo_are_exact_under_random_edits() {
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    let seed = env("UNDOFUZZ_SEED").unwrap_or(1);
    let seqs = env("UNDOFUZZ_SEQS").unwrap_or(200);
    let max = env("UNDOFUZZ_MAX").unwrap_or(200) as usize;
    let started = std::time::Instant::now();
    let next = std::sync::atomic::AtomicU64::new(0);
    let found = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if k >= seqs {
                        break;
                    }
                    let ops = sequence(seed.wrapping_add(k), max);
                    if let Some(f) = check(&ops) {
                        let (small, why) = shrink(ops, &f);
                        found.lock().unwrap().push(format!(
                            "seed {}: {why}\n      shrunk to {} op(s): [{}]\n      ({})",
                            seed.wrapping_add(k),
                            small.len(),
                            replay(&small),
                            small.iter().map(|o| KINDS.get(o.kind).copied().unwrap_or("?")).collect::<Vec<_>>().join(", ")
                        ));
                    }
                }
            });
        }
    });
    let mut found = found.into_inner().unwrap();
    found.sort();
    let ran = RAN.load(std::sync::atomic::Ordering::Relaxed);
    let refused = REFUSED.load(std::sync::atomic::Ordering::Relaxed);
    eprintln!(
        "undofuzz: {seqs} sequences of 20–{max} operations, seed {seed}: {ran} commands ({refused} refused) in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    assert!(found.is_empty(), "{} failing sequence(s):\n  {}", found.len(), found.join("\n  "));
    // A mix that is mostly refused would pass trivially; guard the ratio so a parameter
    // rename can't silently turn the fuzz into a no-op.
    assert!(ran > 0 && refused * 2 < ran, "too many commands were refused ({refused} refused / {ran} ran); the mix may not be exercising commands");
}

/// Sequences the fuzzer found, shrunk: each must hold now. These replay through the generator
/// for compatibility, but each bug also has an explicit-parameter test below.
#[test]
fn sequences_that_failed_hold() {
    let k = |name: &str| KINDS.iter().position(|n| *n == name).unwrap();
    let cases: &[&[(&str, u64)]] = &[
        // A refused command left an empty undo step (copy on write): the next U did nothing.
        &[("layer.delete", 0x23aaef4961e33e31)],
        // UNDO Mark inside an open group was dropped when the group ended: Back undid everything.
        &[("UNDO BEgin", 0xc975618e131bd94), ("hatch", 0xa9025a273cb70411), ("UNDO Mark", 0xaa03cc07f7ed0a85)],
        // Redo lost an associative dimension's links.
        &[("fillet", 0x30b3d1429015ceb8), ("move", 0x7f3b1540ae016363), ("dimlinear", 0x18022a9f42523f16)],
    ];
    for case in cases {
        let ops: Vec<Op> = case.iter().map(|(n, seed)| Op { kind: k(n), seed: *seed }).collect();
        assert_eq!(check(&ops), None, "{case:?}");
    }
}

// ---------------------------------------------------------------------------
// Explicit-parameter regression tests for the three bugs the fuzzer found.
// These do not go through the generator, so they survive parameter changes.
// ---------------------------------------------------------------------------

/// Bug 1: A refused command (layer.delete of layer "0") left an empty undo step; the next
/// U then did nothing. Fixed: a refused command whose drawing equals the one before gets the
/// original `Arc<Drawing>` back, so no step is created.
#[test]
fn refused_layer_delete_leaves_no_undo_step() {
    let mut s = start();
    let steps_before = undo_len(&s);
    // layer "0" can never be deleted — the command is refused.
    let r = s.execute("layer.delete", &json!({ "name": "0" }));
    assert!(r.is_err(), "deleting layer 0 should be refused");
    assert_eq!(undo_len(&s), steps_before, "a refused layer.delete must not add an undo step");
    // Also: U after the refusal should undo the previous real command, not a phantom step.
    let doc_now = doc(&s);
    s.execute("undo", &json!({})).unwrap();
    assert_ne!(doc(&s), doc_now, "U should undo the previous real command");
}

/// Bug 2: UNDO Mark inside an open BEgin group was dropped when the group ended; UNDO Back
/// then found no mark and undid everything. Fixed: Mark ends the open group first.
#[test]
fn undo_mark_inside_begin_group_survives() {
    let mut s = start();
    // Draw something so there is an undo step to go back past.
    s.execute("line", &json!({ "points": [[0, 0], [5, 5]] })).unwrap();
    s.execute("undo", &json!({ "mode": "mark" })).unwrap();
    // Open a group, do work inside it, then mark inside the group.
    s.execute("undo", &json!({ "mode": "begin" })).unwrap();
    s.execute("circle", &json!({ "center": [10, 10], "radius": 3 })).unwrap();
    let mid_drawing = doc(&s);
    // Mark inside the open group should end the group and set the mark.
    s.execute("undo", &json!({ "mode": "mark" })).unwrap();
    s.execute("line", &json!({ "points": [[20, 20], [25, 25]] })).unwrap();
    // UNDO Back should go to the second mark (after the circle), not undo everything.
    s.execute("undo", &json!({ "mode": "back" })).unwrap();
    assert_eq!(doc(&s), mid_drawing, "UNDO Back should return to the mark set inside the group");
}

/// Bug 3: Redo dropped an associative dimension's links. The after-command follow-ups
/// (associative dimensions) ran on the restored drawing, taking restored objects for moved
/// ones. Fixed: undo/redo commands skip the follow-ups.
#[test]
fn redo_keeps_associative_dimension_links() {
    let mut s = Session::new();
    // A polyline, fillet it to round a corner, place a dimension.
    s.execute("pline", &json!({ "vertices": [[0, 0], [10, 0], [10, 10]], "closed": false })).unwrap();
    let pline = s.doc().unwrap().model.last().unwrap().handle;
    s.execute("fillet", &json!({ "handle": pline.hex(), "polyline": true, "radius": 0.5 })).unwrap();
    s.execute("move", &json!({ "handles": [pline.hex()], "delta": [5, 0] })).unwrap();
    s.execute("dimlinear", &json!({ "p1": [5, 0], "p2": [15, 0], "at": [10, -3] })).unwrap();
    let final_doc = doc(&s);
    // Undo everything, then redo everything: the drawing must match exactly.
    while undo_len(&s) > 0 {
        if s.execute("undo", &json!({})).is_err() {
            break;
        }
    }
    s.execute("mredo", &json!({ "all": true })).unwrap();
    assert_eq!(doc(&s), final_doc, "redo should preserve the drawing exactly (including assoc dim links)");
}

// ---------------------------------------------------------------------------
// MUST 1: UNDOHISTORY {action:"to"} must skip follow-ups (assoc dims, groups, tables…)
// on the restored drawing, just as plain undo/redo does.
// ---------------------------------------------------------------------------

/// `undohistory to N` must give exact drawings and keep associative-dimension links intact.
/// This tests the full round trip (to 0 and back) AND partial jumps.
/// Without the `CommandSpec::restores_history` flag on `undohistory`, the after-command
/// follow-ups would corrupt the drawing when jumping between steps where objects moved.
#[test]
fn undohistory_to_keeps_assoc_dims() {
    let mut s = Session::new();
    let empty_doc = doc(&s);
    // Step 1: a polyline and an associative dimlinear on it.
    s.execute("pline", &json!({ "vertices": [[0, 0], [20, 0], [20, 10]], "closed": false })).unwrap();
    let pline_h = s.doc().unwrap().model.last().unwrap().handle;
    s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [20, 0], "at": [10, -3] })).unwrap();
    let before_move = doc(&s);
    let step_before_move = undo_len(&s);
    // Step 2: move the polyline (the dimension follows via assoc).
    s.execute("move", &json!({ "handles": [pline_h.hex()], "delta": [5, 0] })).unwrap();
    let after_move = doc(&s);
    let total = {
        let st = s.state().unwrap();
        st.undo.len() + st.redo.len()
    };
    // Full round trip: jump to 0 then back.
    s.execute("undohistory", &json!({ "action": "to", "step": 0 })).unwrap();
    assert_eq!(doc(&s), empty_doc, "undohistory to 0 should restore the empty drawing exactly");
    s.execute("undohistory", &json!({ "action": "to", "step": total })).unwrap();
    assert_eq!(doc(&s), after_move, "undohistory to <total> should restore the final drawing exactly");
    // Partial jump: from step 3 (after move) back to step 2 (before move). This is the
    // critical case: the polyline moved between before and after, so assoc::after_command
    // would see it as "moved" and try to recompute the dimension.
    s.execute("undohistory", &json!({ "action": "to", "step": step_before_move })).unwrap();
    assert_eq!(doc(&s), before_move, "undohistory to step before move should restore the drawing exactly (no assoc fixup)");
    // And back to the final step.
    s.execute("undohistory", &json!({ "action": "to", "step": total })).unwrap();
    assert_eq!(doc(&s), after_move, "undohistory back to final should restore the drawing exactly (assoc dims intact)");
}
