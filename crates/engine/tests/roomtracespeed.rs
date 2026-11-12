//! Room trace speed on print-like plans (Q6388): a floor of guest rooms off a corridor, drawn as
//! a converted print draws it: walls as line pairs, doors as a leaf and a swing, room names and
//! numbers as text, and the clutter a real sheet adds (furniture, a tiled floor, dashed lines,
//! text drawn as strokes, dimension strings).

use cadaddy_doc::{Common, EntityKind, Line, Space};
use cadaddy_engine::Session;
use cadaddy_geom::Vec3;
use serde_json::json;

/// What a floor holds besides its walls, doors and tags.
#[derive(Clone, Copy)]
struct Clutter {
    furniture: bool,
    tile: bool,
    dashes: bool,
    strokes: bool,
    dims: bool,
    /// Every `open`-th room's door drawn as its swing alone (no leaf: the room stays open to
    /// the corridor), and a name in the corridor, open at its ends. 0: none.
    open: usize,
}

const ALL: Clutter = Clutter { furniture: true, tile: true, dashes: true, strokes: true, dims: true, open: 0 };

fn line(s: &mut Session, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let common = Common { layer: layer.into(), ..Common::default() };
    let l = Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) };
    s.doc_mut().unwrap().add(&Space::Model, common, EntityKind::Line(l)).unwrap();
}

fn rect(s: &mut Session, layer: &str, a: (f64, f64), b: (f64, f64)) {
    line(s, layer, a, (b.0, a.1));
    line(s, layer, (b.0, a.1), b);
    line(s, layer, b, (a.0, b.1));
    line(s, layer, (a.0, b.1), a);
}

/// A swing drawn as chords (a converted print's arc).
fn swing(s: &mut Session, c: (f64, f64), r: f64, from: f64, to: f64) {
    let n = 8;
    let at = |i: usize| {
        let t = from + (to - from) * i as f64 / n as f64;
        (c.0 + r * t.cos(), c.1 + r * t.sin())
    };
    for i in 0..n {
        line(s, "A-DOOR", at(i), at(i + 1));
    }
}

/// A dashed line: each dash its own line (as a print's linetype comes in).
fn dashed(s: &mut Session, a: (f64, f64), b: (f64, f64), dash: f64, gap: f64) {
    let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let (ux, uy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
    let mut t = 0.0;
    while t < len {
        let e = (t + dash).min(len);
        line(s, "A-HIDDEN", (a.0 + ux * t, a.1 + uy * t), (a.0 + ux * e, a.1 + uy * e));
        t += dash + gap;
    }
}

/// Text drawn as strokes too (a print's lettering): three short strokes a letter.
fn strokes(s: &mut Session, at: (f64, f64), t: &str, h: f64) {
    for (i, _) in t.chars().enumerate() {
        let x = at.0 + i as f64 * h * 0.8;
        line(s, "A-TEXT", (x, at.1), (x + 0.3 * h, at.1 + h));
        line(s, "A-TEXT", (x + 0.3 * h, at.1 + h), (x + 0.6 * h, at.1));
        line(s, "A-TEXT", (x + 0.15 * h, at.1 + 0.5 * h), (x + 0.45 * h, at.1 + 0.5 * h));
    }
}

fn text(s: &mut Session, at: (f64, f64), t: &str, h: f64) {
    s.execute("text", &json!({"at": [at.0, at.1], "text": t, "height": h})).unwrap();
}

/// `rooms` guest rooms (13' × 30', inches), half each side of a 6' corridor, at `origin`.
fn floor(s: &mut Session, origin: (f64, f64), rooms: usize, level: usize, c: Clutter) {
    let (w, d, t, corr) = (156.0, 360.0, 4.5, 72.0);
    let per_side = rooms.div_ceil(2);
    let (ox, oy) = origin;
    let len = per_side as f64 * w;
    // Corridor walls with a 36" door gap per room, room walls between.
    for side in 0..2 {
        let y = if side == 0 { oy + d } else { oy + d + corr };
        let (face, back) = if side == 0 { (y, y - t) } else { (y, y + t) };
        let far = if side == 0 { oy } else { oy + 2.0 * d + corr };
        for i in 0..per_side {
            let x0 = ox + i as f64 * w;
            let n = level * 100 + side * per_side + i + 1;
            if side * per_side + i >= rooms {
                break;
            }
            let (g0, g1) = (x0 + 12.0, x0 + 48.0);
            line(s, "A-WALL", (x0, face), (g0, face));
            line(s, "A-WALL", (g1, face), (x0 + w, face));
            line(s, "A-WALL", (x0, back), (g0, back));
            line(s, "A-WALL", (g1, back), (x0 + w, back));
            line(s, "A-WALL", (g0, face), (g0, back));
            line(s, "A-WALL", (g1, face), (g1, back));
            // Party wall (two faces) and the outer wall; the end wall at the first room.
            if i == 0 {
                line(s, "A-WALL", (x0, back), (x0, far));
                line(s, "A-WALL", (x0 - 8.0, face), (x0 - 8.0, if side == 0 { far - 8.0 } else { far + 8.0 }));
            }
            line(s, "A-WALL", (x0 + w - t / 2.0, back), (x0 + w - t / 2.0, far));
            line(s, "A-WALL", (x0 + w + t / 2.0, back), (x0 + w + t / 2.0, far));
            line(s, "A-WALL", (x0, far), (x0 + w, far));
            let out = if side == 0 { far - 8.0 } else { far + 8.0 };
            line(s, "A-WALL", (x0, out), (x0 + w, out));
            // The door: leaf and swing, into the room.
            let inward = if side == 0 { -1.0 } else { 1.0 };
            if c.open == 0 || !(side * per_side + i).is_multiple_of(c.open) {
                line(s, "A-DOOR", (g0, back), (g0, back + inward * 36.0));
            }
            let (a0, a1) = if side == 0 { (-std::f64::consts::FRAC_PI_2, 0.0) } else { (0.0, std::f64::consts::FRAC_PI_2) };
            swing(s, (g0, back), 36.0, a0, a1);
            // The bath: 8' × 8' in the corner by the corridor, its door 30".
            let by = if side == 0 { back - 96.0 } else { back + 96.0 };
            let bx = x0 + w - t / 2.0 - 96.0;
            line(s, "A-WALL", (bx, back), (bx, by));
            line(s, "A-WALL", (bx - t, back), (bx - t, by - inward * t));
            line(s, "A-WALL", (bx, by), (bx + 60.0, by));
            line(s, "A-WALL", (bx + 90.0, by), (x0 + w - t / 2.0, by));
            line(s, "A-WALL", (bx - t, by - inward * t), (bx + 60.0, by - inward * t));
            line(s, "A-WALL", (bx + 90.0, by - inward * t), (x0 + w - t / 2.0, by - inward * t));
            line(s, "A-DOOR", (bx + 60.0, by), (bx + 60.0, by + inward * 30.0));
            // Tags.
            let ty = if side == 0 { oy + 150.0 } else { far - 150.0 };
            text(s, (x0 + 30.0, ty), "GUEST ROOM", 6.0);
            text(s, (x0 + 40.0, ty - 12.0), &n.to_string(), 6.0);
            text(s, (bx + 20.0, (back + by) / 2.0), "BATH", 4.0);
            if c.strokes {
                strokes(s, (x0 + 30.0, ty + 40.0), "BED", 4.0);
                strokes(s, (x0 + 20.0, ty - 60.0), "PROVIDE BLOCKING AT TV", 3.0);
                strokes(s, (bx + 10.0, (back + by) / 2.0 + 20.0), "TILE FLOOR", 3.0);
            }
            if c.furniture {
                let fy = if side == 0 { oy + 40.0 } else { far - 120.0 };
                rect(s, "A-FURN", (x0 + 30.0, fy), (x0 + 106.0, fy + 80.0));
                rect(s, "A-FURN", (x0 + 30.0, fy + 70.0), (x0 + 106.0, fy + 80.0));
                rect(s, "A-FURN", (x0 + 8.0, fy), (x0 + 26.0, fy + 18.0));
                rect(s, "A-FURN", (x0 + 110.0, fy), (x0 + 128.0, fy + 18.0));
                rect(s, "A-FURN", (x0 + 8.0, ty - 100.0), (x0 + 60.0, ty - 76.0));
                rect(s, "A-FURN", (bx + 4.0, by - inward * 4.0 + if side == 0 { 0.0 } else { -30.0 }), (bx + 60.0, by - inward * 34.0));
            }
            if c.tile {
                // A 12" tiled bath floor.
                let (y0, y1) = if side == 0 { (by, back) } else { (back, by) };
                let mut x = bx + 12.0;
                while x < x0 + w - t / 2.0 {
                    line(s, "A-FLOR", (x, y0 + 1.0), (x, y1 - 1.0));
                    x += 12.0;
                }
                let mut y = y0 + 12.0;
                while y < y1 {
                    line(s, "A-FLOR", (bx + 1.0, y), (x0 + w - t / 2.0 - 1.0, y));
                    y += 12.0;
                }
            }
            if c.dashes {
                // A soffit above, dashed.
                let sy = if side == 0 { back - 24.0 } else { back + 24.0 };
                dashed(s, (x0 + 4.0, sy), (bx - 8.0, sy), 6.0, 3.0);
            }
        }
    }
    if c.open > 0 {
        text(s, (ox + len / 2.0, oy + d + corr / 2.0 - 3.0), "CORRIDOR", 6.0);
        text(s, (ox + len / 2.0 + 80.0, oy + d + corr / 2.0 - 3.0), &format!("{level}50"), 6.0);
    }
    if c.dashes {
        // Column grid lines across the floor, dashed (long dash, short gap).
        let mut x = ox;
        while x <= ox + len {
            dashed(s, (x, oy - 60.0), (x, oy + 2.0 * d + corr + 60.0), 24.0, 6.0);
            x += 2.0 * w;
        }
    }
    if c.dims {
        // A dimension string along each side: line, ticks, values.
        for y in [oy - 30.0, oy + 2.0 * d + corr + 30.0] {
            line(s, "A-DIMS", (ox, y), (ox + len, y));
            for i in 0..=per_side {
                let x = ox + i as f64 * w;
                line(s, "A-DIMS", (x - 3.0, y - 3.0), (x + 3.0, y + 3.0));
                line(s, "A-DIMS", (x, y - 8.0), (x, y + 8.0));
            }
        }
    }
}

/// A set of `levels` floors of `rooms` each side by side, and their plan rectangles.
fn set(levels: usize, rooms: usize, c: Clutter) -> (Session, Vec<[f64; 4]>) {
    let mut s = Session::new();
    for l in ["A-WALL", "A-DOOR", "A-ANNO", "A-FURN", "A-FLOR", "A-HIDDEN", "A-TEXT", "A-DIMS"] {
        s.execute("layer.new", &json!({"name": l})).unwrap();
    }
    let span = rooms.div_ceil(2) as f64 * 156.0 + 600.0;
    let mut regions = Vec::new();
    for l in 0..levels {
        let ox = l as f64 * span;
        floor(&mut s, (ox, 0.0), rooms, l + 1, c);
        regions.push([ox - 240.0, -240.0, ox + span - 360.0, 2.0 * 360.0 + 72.0 + 240.0]);
    }
    (s, regions)
}

fn trace(s: &mut Session, regions: &[[f64; 4]]) -> (usize, f64) {
    let t = std::time::Instant::now();
    let r = s.execute("rooms.list", &json!({"outline": false, "regions": regions})).unwrap();
    let n = r["rooms"].as_array().map_or(0, Vec::len);
    if std::env::var("SHOW").is_ok() {
        let mut v = r.clone();
        v["rooms"] = json!(r["rooms"].as_array().map(|a| a.iter().take(3).cloned().collect::<Vec<_>>()));
        eprintln!("{v}");
    }
    (n, t.elapsed().as_secs_f64())
}

/// A floor of 160 guest rooms with a print's clutter, every fourth room open to the corridor
/// (its door drawn as the swing alone): their names and the corridor's search out to the trace's
/// reach from each of their points. That takes about as long as the same floor with every room
/// closed, not many times longer (the open names' windows a wing wide were searched again for
/// each point: 20 s a floor here, the minutes a print set's trace took, Q6388). The rooms found
/// are the closed ones either way, and a long trace is never silent.
#[test]
fn rooms_open_to_their_corridor_trace_about_as_fast_as_closed_ones() {
    LINES.lock().unwrap().clear();
    cadaddy_engine::cmd::guided::set_progress_sink(Some(|l| LINES.lock().unwrap().push((std::time::Instant::now(), l.to_string()))));
    let (mut s, regions) = set(1, 160, ALL);
    let (closed, closed_s) = trace(&mut s, &regions);
    let (mut s, regions) = set(1, 160, Clutter { open: 4, ..ALL });
    let started = std::time::Instant::now();
    let (open, open_s) = trace(&mut s, &regions);
    cadaddy_engine::cmd::guided::set_progress_sink(None);
    eprintln!("closed: {closed} rooms in {closed_s:.2} s; open: {open} rooms in {open_s:.2} s");
    // Every fourth guest room open: the others found.
    assert!(closed >= 120 && open >= closed - 40 && open < closed, "{closed} closed, {open} open");
    assert!(open_s <= 12.0 * closed_s.max(0.25), "open rooms took {open_s:.2} s, closed {closed_s:.2} s");
    // No more than 20 s without a line (a minute on the slowest machines' jobs).
    let lines = LINES.lock().unwrap();
    let mut last = started;
    for (at, _) in lines.iter().filter(|(at, _)| *at >= started) {
        assert!(at.duration_since(last).as_secs_f64() <= 25.0, "silent for {:.0} s", at.duration_since(last).as_secs_f64());
        last = *at;
    }
}

static LINES: std::sync::Mutex<Vec<(std::time::Instant, String)>> = std::sync::Mutex::new(Vec::new());

#[test]
#[ignore = "timing probe: cargo test --test roomtracespeed -- --ignored --nocapture"]
fn probe() {
    let rooms: usize = std::env::var("ROOMS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let levels: usize = std::env::var("LEVELS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let only = std::env::var("ONLY").unwrap_or_default();
    cadaddy_engine::cmd::guided::set_progress_sink(Some(|l| eprintln!("{l}")));
    let open: usize = std::env::var("OPEN").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    let cases: [(&str, Clutter); 9] = [
        ("open", Clutter { open, ..Clutter { furniture: false, tile: false, dashes: false, strokes: false, dims: false, open: 0 } }),
        ("allopen", Clutter { open, ..ALL }),
        ("bare", Clutter { furniture: false, tile: false, dashes: false, strokes: false, dims: false, open: 0 }),
        ("furniture", Clutter { furniture: true, tile: false, dashes: false, strokes: false, dims: false, open: 0 }),
        ("tile", Clutter { furniture: false, tile: true, dashes: false, strokes: false, dims: false, open: 0 }),
        ("dashes", Clutter { furniture: false, tile: false, dashes: true, strokes: false, dims: false, open: 0 }),
        ("strokes", Clutter { furniture: false, tile: false, dashes: false, strokes: true, dims: false, open: 0 }),
        ("dims", Clutter { furniture: false, tile: false, dashes: false, strokes: false, dims: true, open: 0 }),
        ("all", ALL),
    ];
    for (name, c) in cases {
        if !only.is_empty() && !only.split(',').any(|o| o == name) {
            continue;
        }
        let (mut s, regions) = set(levels, rooms, c);
        let objects = s.doc().unwrap().model.iter().count();
        let (n, secs) = trace(&mut s, &regions);
        eprintln!("PROBE {name}: {levels} x {rooms} rooms, {objects} objects -> {n} rooms in {secs:.2} s");
    }
}
