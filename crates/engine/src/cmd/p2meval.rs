//! A benchmark for plans-to-model agents: synthetic plan sets with their ground truth, run
//! through the same commands an agent calls, and scored.
//!
//! **Cases** (`cases()`), each a drawing and its truth (wall length, doors and windows, room
//! areas, level elevations and heights, stair risers):
//! - `single`: one storey on NCS layers, two rooms, a door and a window block;
//! - `multi`: two storeys drawn side by side (titled `LEVEL 1 PLAN`, `LEVEL 2 PLAN`, with a
//!   `FLOOR TO FLOOR` note) and a stair between them;
//! - `messy`: the single storey on meaningless layers (`Layer 7`, `walls_NEW`, `0`), a door
//!   swing instead of a block, dimensions and furniture in the way;
//! - `pdf`: the messy storey plotted to PDF and read back with no layers (needs the app's PDF
//!   hooks; skipped without them).
//!
//! **Pipeline** (`p2m.bench`): what an agent does, with the heuristic drafter in the agent's
//! place: `plan.layers`, `plan.walls`, `plan.rooms`, `plan.notes` → `bspec.draft` (or
//! `plan.classify {draft}` for a PDF) → levels from the plan titles and notes → `stairs.detect`
//! → `model.build` → `model.issues`, `model.counts`. Each step's result and time are reported.
//!
//! **Scorer** (`score`): each measure from 0 to 1 — wall length (built walls along their run),
//! openings (built doors and windows), rooms (built room ceilings' areas, by name), heights
//! (levels' elevations and floor to floor), stairs (risers) — and their mean.
//!
//! **With a live agent**: give the agent the MCP server (`cadaddy-cli mcp`, or the app with
//! `--control`), open a case's drawing (`p2m.case {name, path}` writes it as DXF), let it work
//! with the `plan.*`, `bspec.*`, `stairs.*`, `model.*` tools, then call `p2m.score {case}` on
//! its drawing to get the same scores as the benchmark.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Mesh, Space};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};
use std::sync::Arc;

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("p2m.bench", "Plans to Model Benchmark", run_bench)
            .params("{cases?: [single|multi|messy|pdf]} → {cases: [{name, steps: [{command, ok, ms, error?}], scores: {walls, openings, rooms, heights, stairs?}, total, issues}], mean}")
            .noundo(),
        CommandSpec::new("p2m.case", "Plans to Model Case", run_case_cmd)
            .params("{name, path?: DXF file to write} → {truth}: a benchmark case opened as a new drawing (or written to a file) for a live agent")
            .noundo(),
        CommandSpec::new("p2m.score", "Plans to Model Score", run_score)
            .params("{case: name} → {scores, total}: the active drawing's built model scored against the case's truth")
            .noundo(),
    ]
}

/// What a case's model should have.
#[derive(Clone, Debug, PartialEq)]
pub struct Truth {
    /// Wall run length on the ground floor, feet (centrelines, through openings).
    pub walls_ft: f64,
    pub doors: usize,
    pub windows: usize,
    /// Room name → floor area, square feet.
    pub rooms: Vec<(String, f64)>,
    /// Level → (elevation, floor to floor), feet.
    pub levels: Vec<(String, f64, f64)>,
    /// Risers of each stair.
    pub stairs: Vec<u32>,
}

pub struct Case {
    pub name: &'static str,
    pub drawing: Drawing,
    pub truth: Truth,
    /// Read back through PDF before the pipeline.
    pub pdf: bool,
    /// A correct spec for the case (the scorer's perfect model).
    pub spec: Value,
}

// ---------------- the cases ----------------

fn add(d: &mut Drawing, layer: &str, lw: Option<u16>, kind: EntityKind) {
    if d.layer(layer).is_none() {
        d.layers.push(cadaddy_doc::Layer { name: layer.into(), ..cadaddy_doc::Layer::default() });
    }
    let mut c = Common { layer: layer.into(), ..Common::default() };
    if let Some(w) = lw {
        c.lineweight = cadaddy_doc::Lineweight::Mm100(w);
    }
    let _ = d.add(&Space::Model, c, kind);
}

fn rect(d: &mut Drawing, layer: &str, lw: Option<u16>, a: (f64, f64), b: (f64, f64)) {
    let v = |x: f64, y: f64| cadaddy_geom::PolyVertex { p: Vec2::new(x, y), bulge: 0.0, start_width: 0.0, end_width: 0.0 };
    let vertices = vec![v(a.0, a.1), v(b.0, a.1), v(b.0, b.1), v(a.0, b.1)];
    add(d, layer, lw, EntityKind::LwPolyline(cadaddy_doc::LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }));
}

fn line(d: &mut Drawing, layer: &str, lw: Option<u16>, a: (f64, f64), b: (f64, f64)) {
    add(d, layer, lw, EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) }));
}

fn text(d: &mut Drawing, layer: &str, at: (f64, f64), value: &str, h: f64) {
    let t = cadaddy_doc::Text {
        insert: Vec3::new(at.0, at.1, 0.0),
        align_pt: None,
        height: h,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: cadaddy_doc::HAlign::Left,
        valign: cadaddy_doc::VAlign::Baseline,
    };
    add(d, layer, None, EntityKind::Text(t));
}

fn block(d: &mut Drawing, name: &str, w: f64) {
    if d.blocks.contains_key(name) {
        return;
    }
    let mut b = cadaddy_doc::Block::new(name);
    let h = d.new_handle();
    b.entities.push(Entity::new(h, EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(-w / 2.0, 0.0, 0.0), b: Vec3::new(w / 2.0, 0.0, 0.0) })));
    d.blocks.insert(name.into(), Arc::new(b));
}

fn insert(d: &mut Drawing, layer: &str, name: &str, at: (f64, f64), rotation: f64) {
    let ins = cadaddy_doc::Insert {
        block: name.into(),
        insert: Vec3::new(at.0, at.1, 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    add(d, layer, None, EntityKind::Insert(ins));
}

/// The storey every case is made of, at `o`: 40' × 20', two 19' × 19' rooms in 6" walls with
/// a 12" wall between, a window in the south wall and a door in the middle wall.
fn storey(d: &mut Drawing, o: (f64, f64), messy: bool, lw: bool) {
    let p = |x: f64, y: f64| (o.0 + x, o.1 + y);
    let (walls, doors, glaz, tags) = if messy { ("walls_NEW", "Layer 7", "Layer 7", "0") } else { ("A-WALL", "A-DOOR", "A-GLAZ", "A-AREA-IDEN") };
    let heavy = lw.then_some(70);
    rect(d, walls, heavy, p(0.0, 0.0), p(480.0, 240.0));
    rect(d, walls, heavy, p(6.0, 6.0), p(234.0, 234.0));
    rect(d, walls, heavy, p(246.0, 6.0), p(474.0, 234.0));
    if messy {
        // A door swing in the middle wall's gap side, its leaf, furniture, a dimension.
        let light = lw.then_some(18);
        add(
            d,
            doors,
            light,
            EntityKind::Arc(cadaddy_doc::Arc {
                center: Vec3::new(p(234.0, 60.0).0, p(234.0, 60.0).1, 0.0),
                radius: 36.0,
                start: 90f64.to_radians(),
                end: 180f64.to_radians(),
            }),
        );
        line(d, doors, light, p(234.0, 60.0), p(234.0, 96.0));
        rect(d, doors, light, p(150.0, 150.0), p(210.0, 180.0));
        line(d, "0", light, p(0.0, 280.0), p(480.0, 280.0));
        text(d, "0", p(220.0, 284.0), "40'-0\"", 6.0);
    } else {
        block(d, "DOOR-36", 36.0);
        block(d, "WIN-48", 48.0);
        insert(d, doors, "DOOR-36", p(240.0, 60.0), std::f64::consts::FRAC_PI_2);
        insert(d, glaz, "WIN-48", p(120.0, 3.0), 0.0);
    }
    text(d, tags, p(90.0, 120.0), "OFFICE", 9.0);
    text(d, tags, p(330.0, 120.0), "LOBBY", 9.0);
}

fn base_truth() -> Truth {
    Truth {
        // The perimeter's centreline (474 × 234) and the middle wall's run (228).
        walls_ft: (2.0 * (474.0 + 234.0) + 228.0) / 12.0,
        doors: 1,
        windows: 1,
        rooms: vec![("Office".into(), 361.0), ("Lobby".into(), 361.0)],
        levels: vec![("Level 1".into(), 0.0, 10.0)],
        stairs: Vec::new(),
    }
}

fn perfect_spec(levels: Value, extra: Value) -> Value {
    let mut v = json!({
        "levels": levels,
        "layers": [
            {"layer": "A-WALL", "role": "wall", "wall_type": "Walls"},
            {"layer": "walls_NEW", "role": "wall", "wall_type": "Walls"},
            {"layer": "A-DOOR", "role": "door"},
            {"layer": "A-GLAZ", "role": "window"},
            {"layer": "A-AREA-IDEN", "role": "room_tag"},
            {"layer": "A-FLOR-STRS", "role": "stair"},
            {"layer": "Layer 7", "role": "door"},
            {"layer": "0", "role": "room_tag"}
        ],
        "wall_types": [{"name": "Walls", "thickness_in": 6}],
        "rooms": [{"name": "Office"}, {"name": "Lobby"}],
        "families": [{"name": "Door", "kind": "door", "block": "DOOR*"}, {"name": "Window", "kind": "window", "block": "WIN*"}]
    });
    if let (Some(o), Some(x)) = (v.as_object_mut(), extra.as_object()) {
        o.extend(x.clone());
    }
    v
}

/// The benchmark's cases.
pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    // single
    let mut d = Drawing::new_imperial();
    storey(&mut d, (0.0, 0.0), false, false);
    out.push(Case {
        name: "single",
        drawing: d,
        truth: base_truth(),
        pdf: false,
        spec: perfect_spec(json!([{"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 8}]), json!({})),
    });
    // multi: two storeys side by side, a stair on the first.
    let mut d = Drawing::new_imperial();
    storey(&mut d, (0.0, 0.0), false, false);
    storey(&mut d, (1000.0, 0.0), false, false);
    text(&mut d, "A-ANNO-TTLB", (0.0, -60.0), "LEVEL 1 PLAN", 12.0);
    text(&mut d, "A-ANNO-TTLB", (1000.0, -60.0), "LEVEL 2 PLAN", 12.0);
    text(&mut d, "A-ANNO-NOTE", (0.0, -90.0), "FLOOR TO FLOOR 12'-0\"", 6.0);
    for i in 0..=18 {
        let x = 280.0 + 9.0 * f64::from(i);
        line(&mut d, "A-FLOR-STRS", None, (x, 150.0), (x, 194.0));
    }
    text(&mut d, "A-FLOR-STRS", (262.0, 168.0), "UP", 6.0);
    let mut truth = base_truth();
    truth.levels = vec![("Level 1".into(), 0.0, 12.0), ("Level 2".into(), 12.0, 12.0)];
    truth.stairs = vec![19];
    let spec_levels = json!([
        {"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 8, "region": [-200, -200, 900, 600]},
        {"name": "Level 2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 8, "region": [900, -200, 2000, 600]}
    ]);
    out.push(Case {
        name: "multi",
        drawing: d,
        truth,
        pdf: false,
        spec: perfect_spec(spec_levels, json!({"stairs": [{"name": "S1", "level": "Level 1", "runs": [{"from": [280, 172], "to": [442, 172], "treads": 18}], "width_in": 44, "confirmed": true}]})),
    });
    // messy
    let mut d = Drawing::new_imperial();
    storey(&mut d, (0.0, 0.0), true, false);
    out.push(Case {
        name: "messy",
        drawing: d,
        truth: Truth { windows: 0, ..base_truth() },
        pdf: false,
        spec: perfect_spec(json!([{"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 8}]), json!({})),
    });
    // pdf: the messy storey with pens, through PDF.
    let mut d = Drawing::new_imperial();
    storey(&mut d, (0.0, 0.0), true, true);
    text(&mut d, "0", (0.0, -50.0), "SCALE: 1/4\" = 1'-0\"", 6.0);
    out.push(Case { name: "pdf", drawing: d, truth: Truth { windows: 0, ..base_truth() }, pdf: true, spec: Value::Null });
    out
}

// ---------------- the pipeline ----------------

/// One step: run a command, note its result and time.
fn step(s: &mut Session, steps: &mut Vec<Value>, command: &str, params: Value) -> Option<Value> {
    let t = std::time::Instant::now();
    let r = s.execute(command, &params);
    let ms = (t.elapsed().as_secs_f64() * 1e5).round() / 100.0;
    match r {
        Ok(v) => {
            steps.push(json!({ "command": command, "ok": true, "ms": ms }));
            Some(v)
        }
        Err(e) => {
            steps.push(json!({ "command": command, "ok": false, "ms": ms, "error": e.to_string() }));
            None
        }
    }
}

/// Levels from the plan titles (`LEVEL n PLAN`) and a floor-to-floor note, as an agent would
/// read them: each title's plan is the cluster of objects above it.
fn levels_from_titles(s: &mut Session, steps: &mut Vec<Value>) -> Option<Value> {
    let notes = step(s, steps, "texts", json!({"contains": "PLAN", "limit": 50}))?;
    let mut titles: Vec<(u32, f64, f64)> = Vec::new();
    for t in notes["texts"].as_array().or_else(|| notes["items"].as_array()).into_iter().flatten() {
        let text = t["text"].as_str().unwrap_or_default().to_ascii_uppercase();
        let words: Vec<&str> = text.split_whitespace().collect();
        if let [lv, n, plan, ..] = words[..]
            && lv == "LEVEL"
            && plan == "PLAN"
            && let Ok(n) = n.parse::<u32>()
        {
            let at = &t["at"];
            titles.push((n, at[0].as_f64().unwrap_or(0.0), at[1].as_f64().unwrap_or(0.0)));
        }
    }
    if titles.len() < 2 {
        return None;
    }
    titles.sort_by_key(|t| t.0);
    let f2f = step(s, steps, "texts", json!({"contains": "FLOOR TO FLOOR"}))
        .and_then(|v| {
            v["texts"].as_array().or_else(|| v["items"].as_array()).and_then(|a| a.first()).and_then(|t| t["text"].as_str().map(str::to_string))
        })
        .and_then(|t| super::perceive::feet_inches(&t))
        .map_or(10.0, |i| i / 12.0);
    // Each plan's region: from its title to halfway to the next one.
    let levels: Vec<Value> = titles
        .iter()
        .enumerate()
        .map(|(i, (n, x, _))| {
            let left = if i == 0 { x - 10_000.0 } else { (titles[i - 1].1 + x) / 2.0 };
            let right = titles.get(i + 1).map_or(x + 10_000.0, |t| (t.1 + x) / 2.0);
            json!({"name": format!("Level {n}"), "elevation_ft": f2f * f64::from(n - 1), "height_ft": f2f, "ceiling_ft": 8, "region": [left, -10_000, right, 10_000]})
        })
        .collect();
    Some(json!(levels))
}

/// Run a case through the pipeline (the heuristic drafter as the agent) and score it.
pub fn run(case: &Case) -> Value {
    let mut s = Session::empty();
    let mut steps: Vec<Value> = Vec::new();
    let mut d = case.drawing.clone();
    if case.pdf {
        let hooks = super::file::io();
        let (Some(plot), Some(read)) = (hooks.and_then(|h| h.plot), hooks.and_then(|h| h.read_pdf)) else {
            return json!({ "name": case.name, "skipped": "no PDF plotter or reader in this build" });
        };
        let pdf = match plot(
            &d,
            &Space::Model,
            &json!({"layers": false, "fit": false, "center": true, "scale": 0.25 / 12.0, "paper": "ANSI B", "landscape": true, "lineweights": true}),
        ) {
            Ok(b) => b,
            Err(e) => return json!({ "name": case.name, "error": e }),
        };
        d = match read(&pdf, &json!({"scale": 1, "layers": "object", "layout": false})) {
            Ok((d, _)) => d,
            Err(e) => return json!({ "name": case.name, "error": e }),
        };
    }
    s.open_drawing(d, case.name, None);
    for (c, p) in
        [("plan.layers", json!({})), ("plan.walls", json!({"limit": 50})), ("plan.rooms", json!({"outline": false})), ("plan.notes", json!({}))]
    {
        step(&mut s, &mut steps, c, p);
    }
    if case.pdf {
        step(&mut s, &mut steps, "plan.classify", json!({"draft": true}));
    } else {
        step(&mut s, &mut steps, "bspec.draft", json!({"store": true}));
    }
    if let Some(levels) = levels_from_titles(&mut s, &mut steps) {
        step(&mut s, &mut steps, "bspec.set", json!({"spec": {"levels": levels}}));
    }
    // The drafted level's name for a single storey ("Level 1").
    step(&mut s, &mut steps, "stairs.detect", json!({"store": true}));
    step(&mut s, &mut steps, "model.build", json!({}));
    let issues = step(&mut s, &mut steps, "model.issues", json!({})).map_or(Value::Null, |v| v["byKind"].clone());
    step(&mut s, &mut steps, "model.counts", json!({}));
    let (scores, total) = score(&s, &case.truth);
    json!({ "name": case.name, "steps": steps, "scores": scores, "total": total, "issues": issues })
}

// ---------------- the scorer ----------------

/// How close `got` is to `want`: 1 when equal, falling to 0 at twice the error.
fn closeness(got: f64, want: f64) -> f64 {
    if want.abs() < 1e-9 {
        return if got.abs() < 1e-9 { 1.0 } else { 0.0 };
    }
    (1.0 - (got - want).abs() / want.abs()).clamp(0.0, 1.0)
}

fn meshes<'a>(d: &'a Drawing, part: &'a str) -> impl Iterator<Item = (&'a Arc<Entity>, &'a Mesh)> + 'a {
    d.model
        .iter()
        .filter(move |e| super::wallgen::made_as(e).as_deref() == Some(part))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some((e, m)) } else { None })
}

/// A wall's run: its length along its longest plan direction (through its openings).
fn run_length(m: &Mesh) -> f64 {
    let pts: Vec<Vec2> = m.vertices.iter().map(|v| v.xy()).collect();
    let b = cadaddy_geom::Bounds2::from_points(pts.iter().copied());
    if b.is_empty() {
        return 0.0;
    }
    // Axis-aligned walls (the cases') run along the box's long side; others along the diagonal.
    let (w, h) = (b.width(), b.height());
    if w.min(h) <= 24.0 * w.max(h) / 100.0 + 24.0 { w.max(h) } else { w.hypot(h) }
}

/// Faces' total area (a ceiling's), square drawing units.
fn area(m: &Mesh) -> f64 {
    m.faces
        .iter()
        .map(|f| {
            let p: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
            let mut a = 0.0;
            for k in 1..p.len().saturating_sub(1) {
                let (u, v) = (p[k] - p[0], p[k + 1] - p[0]);
                a += 0.5 * (u.x * v.y - u.y * v.x).abs();
            }
            a
        })
        .sum()
}

/// The built model of a session scored against a case's truth: each measure 0–1, and the mean.
pub fn score(s: &Session, truth: &Truth) -> (Value, f64) {
    let Ok(d) = s.doc() else { return (json!({}), 0.0) };
    let spec = super::bspec::current(s, "p2m.score").map(|x| x.0).ok();
    let upf = spec.as_ref().map_or(12.0, |sp| if sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 });
    let ground = spec.as_ref().and_then(|sp| sp.levels.iter().min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).map(|l| l.name.clone()));
    let on_ground = |e: &Entity| ground.as_ref().is_some_and(|g| super::bspec::made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(g)));
    let mut scores = serde_json::Map::new();
    // Walls: the ground floor's runs, less the corner overlaps (each wall end runs on into the
    // wall it meets by its thickness).
    let walls: f64 = meshes(d, "wall").filter(|(e, _)| on_ground(e)).map(|(_, m)| run_length(m)).sum::<f64>() / upf;
    scores.insert("walls".into(), json!(round(closeness(walls, truth.walls_ft))));
    // Openings.
    let count = |part: &str| meshes(d, part).filter(|(e, _)| on_ground(e)).count();
    let (doors, windows) = (count("door"), count("window"));
    let want = (truth.doors + truth.windows).max(1) as f64;
    let miss = doors.abs_diff(truth.doors) + windows.abs_diff(truth.windows);
    scores.insert("openings".into(), json!(round((1.0 - miss as f64 / want).clamp(0.0, 1.0))));
    // Rooms: each room's built ceiling area, by name.
    if !truth.rooms.is_empty() {
        let mut sum = 0.0;
        for (name, sqft) in &truth.rooms {
            let tag = format!("room:{}", name.to_ascii_lowercase());
            let got = meshes(d, "ceiling")
                .filter(|(e, _)| e.common.xdata.iter().any(|t| t.value.to_ascii_lowercase() == tag))
                .map(|(_, m)| area(m) / (upf * upf))
                .fold(0.0, f64::max);
            sum += closeness(got, *sqft);
        }
        scores.insert("rooms".into(), json!(round(sum / truth.rooms.len() as f64)));
    }
    // Heights: each level's elevation and floor to floor.
    if !truth.levels.is_empty() {
        let mut sum = 0.0;
        for (i, (_, elev, height)) in truth.levels.iter().enumerate() {
            let got = spec.as_ref().and_then(|sp| {
                let mut ls: Vec<&cadaddy_walk3d::Level> = sp.levels.iter().collect();
                ls.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
                ls.get(i).map(|l| (l.elevation_ft, l.height_ft))
            });
            sum += match got {
                Some((e, h)) => 0.5 * (if (e - elev).abs() < 0.05 { 1.0 } else { closeness(e, *elev).min(0.5) }) + 0.5 * closeness(h, *height),
                None => 0.0,
            };
        }
        scores.insert("heights".into(), json!(round(sum / truth.levels.len() as f64)));
    }
    // Stairs: risers.
    if !truth.stairs.is_empty() {
        let got: Vec<u32> =
            spec.as_ref().map(|sp| sp.stairs.iter().filter_map(|st| sp.stair_rule(st)).map(|r| r.risers()).collect()).unwrap_or_default();
        let sum: f64 = truth.stairs.iter().enumerate().map(|(i, want)| got.get(i).map_or(0.0, |g| closeness(f64::from(*g), f64::from(*want)))).sum();
        scores.insert("stairs".into(), json!(round(sum / truth.stairs.len() as f64)));
    }
    let vals: Vec<f64> = scores.values().filter_map(Value::as_f64).collect();
    let total = if vals.is_empty() { 0.0 } else { vals.iter().sum::<f64>() / vals.len() as f64 };
    (Value::Object(scores), round(total))
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn truth_json(t: &Truth) -> Value {
    json!({
        "walls_ft": t.walls_ft, "doors": t.doors, "windows": t.windows,
        "rooms": t.rooms.iter().map(|(n, a)| json!({"name": n, "area_sqft": a})).collect::<Vec<_>>(),
        "levels": t.levels.iter().map(|(n, e, h)| json!({"name": n, "elevation_ft": e, "height_ft": h})).collect::<Vec<_>>(),
        "stairs": t.stairs,
    })
}

fn run_bench(_s: &mut Session, p: &Value) -> Result<Value> {
    let only: Option<Vec<String>> =
        p.get("cases").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let results: Vec<Value> = cases().iter().filter(|c| only.as_ref().is_none_or(|o| o.iter().any(|n| n == c.name))).map(run).collect();
    let totals: Vec<f64> = results.iter().filter_map(|r| r["total"].as_f64()).collect();
    let mean = if totals.is_empty() { 0.0 } else { round(totals.iter().sum::<f64>() / totals.len() as f64) };
    Ok(json!({ "cases": results, "mean": mean }))
}

fn case_named(name: &str) -> Result<Case> {
    cases().into_iter().find(|c| c.name == name).ok_or_else(|| bad("p2m", format!("there is no case named {name} (single, multi, messy, pdf)")))
}

fn run_case_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").ok_or_else(|| bad("p2m.case", "`name` is required"))?;
    let case = case_named(name)?;
    let truth = truth_json(&case.truth);
    match str_param(p, "path") {
        Some(path) => {
            let write = super::file::io().map(|h| h.write).ok_or_else(|| bad("p2m.case", "no file writer in this build"))?;
            let bytes = write(&case.drawing, path).map_err(|e| bad("p2m.case", e))?;
            #[cfg(not(target_arch = "wasm32"))]
            std::fs::write(path, bytes).map_err(|e| bad("p2m.case", format!("{path}: {e}")))?;
            #[cfg(target_arch = "wasm32")]
            let _ = bytes;
        }
        None => {
            s.open_drawing(case.drawing, case.name, None);
        }
    }
    Ok(json!({ "truth": truth }))
}

fn run_score(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "case").ok_or_else(|| bad("p2m.score", "`case` is required"))?;
    let case = case_named(name)?;
    let (scores, total) = score(s, &case.truth);
    Ok(json!({ "scores": scores, "total": total }))
}

/// For the scorer's tests: open a case built from its correct spec.
pub fn perfect(case: &Case) -> Result<Session> {
    let mut s = Session::empty();
    s.open_drawing(case.drawing.clone(), case.name, None);
    s.execute("bspec.set", &json!({"spec": case.spec, "replace": true}))?;
    s.execute("model.build", &json!({}))?;
    Ok(s)
}
