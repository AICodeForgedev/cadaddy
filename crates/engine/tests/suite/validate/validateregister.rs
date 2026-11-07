//! Print crops checked against the model's walls (Q6141) on a made-up print set: an
//! architectural plan the model is built on, and trade plans of the same floor whose recorded
//! registrations are off — one shifted 3 ft (walls drawn as lines), one turned 3° (as closed
//! polylines), one drawn 1.1 × as large (as solid fills) — beside a ceiling plan registered truly
//! and a sheet whose lines are not the floor at all. The packet
//! re-fits the first three from their wall lines, keeps the true one, marks the last
//! `misregistered`, and `validate.status` lists the sheets worst first.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// The level's base on the drawing (inches).
const OX: f64 = 600.0;
const OY: f64 = 500.0;
/// One sheet, drawing inches, and where each starts along x.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
const A201: f64 = 0.0;
const E201: f64 = 4000.0;
const M201: f64 = 8000.0;
const P201: f64 = 12000.0;
const A221: f64 = 16000.0;
const F201: f64 = 20000.0;
/// Half a wall's thickness (4 7/8"), feet.
const HALF_FT: f64 = 4.875 / 24.0;

/// The floor's wall centrelines, feet: a 70' × 44' shell, a corridor, rooms of unequal widths.
fn centrelines() -> Vec<([f64; 2], [f64; 2])> {
    let mut w = vec![
        ([0.0, 0.0], [70.0, 0.0]),
        ([70.0, 0.0], [70.0, 44.0]),
        ([70.0, 44.0], [0.0, 44.0]),
        ([0.0, 44.0], [0.0, 0.0]),
        ([0.0, 18.0], [70.0, 18.0]),
        ([0.0, 24.0], [70.0, 24.0]),
    ];
    for x in [13.0, 27.5, 41.0, 58.0] {
        w.push(([x, 0.0], [x, 18.0]));
    }
    for x in [9.0, 24.0, 36.5, 52.0] {
        w.push(([x, 24.0], [x, 44.0]));
    }
    w.push(([41.0, 8.0], [47.0, 8.0]));
    w.push(([36.5, 32.0], [42.0, 32.0]));
    w
}

/// Both faces of each wall.
fn faces() -> Vec<([f64; 2], [f64; 2])> {
    let mut out = Vec::new();
    for (a, b) in centrelines() {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        let (nx, ny) = (-dy / l * HALF_FT, dx / l * HALF_FT);
        out.push(([a[0] + nx, a[1] + ny], [b[0] + nx, b[1] + ny]));
        out.push(([a[0] - nx, a[1] - ny], [b[0] - nx, b[1] - ny]));
    }
    out
}

/// A building point (feet) on the sheet starting at `sx`, as the sheet truly draws it (`truth`).
fn on(sx: f64, truth: &dyn Fn([f64; 2]) -> [f64; 2], p: [f64; 2]) -> [f64; 2] {
    let q = truth(p);
    [sx + OX + q[0] * 12.0, OY + q[1] * 12.0]
}

fn shifted(p: [f64; 2]) -> [f64; 2] {
    [p[0] + 3.0, p[1]]
}
/// Turned 3° about the building point (10, 10).
fn turned(p: [f64; 2]) -> [f64; 2] {
    let t = 3.0f64.to_radians();
    let (x, y) = (p[0] - 10.0, p[1] - 10.0);
    [x * t.cos() - y * t.sin() + 10.0, x * t.sin() + y * t.cos() + 10.0]
}
/// Drawn 1.1 × as large about the building point (20, 20).
fn scaled(p: [f64; 2]) -> [f64; 2] {
    [(p[0] - 20.0) * 1.1 + 20.0, (p[1] - 20.0) * 1.1 + 20.0]
}
fn same(p: [f64; 2]) -> [f64; 2] {
    p
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

/// A plan sheet: the walls' faces as `truth` draws them, a door swing's chord and some lettering.
/// How a sheet draws its walls.
#[derive(Clone, Copy, PartialEq)]
enum Drawn {
    /// Each face a line.
    Lines,
    /// Each wall a closed polyline round it.
    Outlines,
    /// Each wall a solid fill (poche), added by [`fills`].
    Fills,
}

/// A wall's outline on the sheet starting at `sx`, as `truth` draws it.
fn outline(sx: f64, truth: &dyn Fn([f64; 2]) -> [f64; 2], a: [f64; 2], b: [f64; 2]) -> [[f64; 2]; 4] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx.hypot(dy);
    let (nx, ny) = (-dy / l * HALF_FT, dx / l * HALF_FT);
    [[a[0] + nx, a[1] + ny], [b[0] + nx, b[1] + ny], [b[0] - nx, b[1] - ny], [a[0] - nx, a[1] - ny]].map(|p| on(sx, truth, p))
}

fn trade_plan(s: &mut Session, sx: f64, truth: &dyn Fn([f64; 2]) -> [f64; 2], drawn: Drawn) {
    match drawn {
        Drawn::Lines => {
            for (a, b) in faces() {
                line(s, on(sx, truth, a), on(sx, truth, b));
            }
        }
        Drawn::Outlines => {
            for (a, b) in centrelines() {
                s.execute("pline", &json!({"vertices": outline(sx, truth, a, b), "closed": true})).unwrap();
            }
        }
        Drawn::Fills => {}
    }
    line(s, on(sx, truth, [14.0, 3.0]), on(sx, truth, [17.0, 6.0]));
    for k in 0..12 {
        let x = 30.0 + f64::from(k) * 0.9;
        line(s, on(sx, truth, [x, 20.0]), on(sx, truth, [x + 0.5, 21.2]));
    }
}

fn sheet(number: &str, title: &str, discipline: &str, x: f64) -> Value {
    let method = if x == 0.0 { "reference" } else { "tags" };
    json!({
        "number": number, "title": title, "discipline": discipline, "level": "L1", "source": "model",
        "rect": [x, 0.0, x + SW, SH],
        // What the job recorded: drawn alike, `x` along (off for the shifted, turned and scaled sheets).
        "registration": {"reference": "A-201", "method": method, "a": 1.0, "b": 0.0, "tx": -x, "ty": 0.0, "matched": 4},
    })
}

fn spec() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [{"name": "Partition", "thickness_in": 4.875, "from": "centerline"}],
        "rooms": [{"name": "Reading", "number": "B12"}, {"name": "Office", "number": "B10"}],
        "sheets": [
            sheet("A-201", "FLOOR PLAN - LEVEL 1", "A", A201),
            sheet("E-201", "POWER PLAN - LEVEL 1", "E", E201),
            sheet("M-201", "MECHANICAL PLAN - LEVEL 1", "M", M201),
            sheet("P-201", "PLUMBING PLAN - LEVEL 1", "P", P201),
            sheet("A-221", "REFLECTED CEILING PLAN - LEVEL 1", "A", A221),
            sheet("F-201", "FIRE PROTECTION PLAN - LEVEL 1", "F", F201),
        ]
    })
}

fn job() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL-CL");
    for (a, b) in centrelines() {
        line(&mut s, on(A201, &same, a), on(A201, &same, b));
    }
    layer(&mut s, "A-ROOM-TAG");
    for (p, t) in [([44.0, 38.0], "READING"), ([6.0, 9.0], "OFFICE")] {
        s.execute("text", &json!({"at": on(A201, &same, p), "text": t, "height": 6})).unwrap();
    }
    layer(&mut s, "G-BKGD");
    for (a, b) in faces() {
        line(&mut s, on(A201, &same, a), on(A201, &same, b));
    }
    trade_plan(&mut s, E201, &shifted, Drawn::Lines);
    trade_plan(&mut s, M201, &turned, Drawn::Outlines);
    trade_plan(&mut s, P201, &scaled, Drawn::Fills);
    trade_plan(&mut s, A221, &same, Drawn::Lines);
    // A sheet whose lines are not the floor: a coarse grid of long lines.
    for k in 0..12 {
        let v = f64::from(k) * 7.3 - 5.0;
        line(&mut s, [F201 + OX + v * 12.0, OY - 120.0], [F201 + OX + v * 12.0, OY + 700.0]);
    }
    let mut s = fills(s, P201, &scaled);
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// The walls of the sheet starting at `sx` as solid fills, the way a PDF's wall poche imports.
fn fills(s: Session, sx: f64, truth: &dyn Fn([f64; 2]) -> [f64; 2]) -> Session {
    use cadaddy_engine::doc::{Entity, EntityKind, Hatch, HatchLoop};
    use cadaddy_engine::geom::{PolyVertex, Vec2};
    let mut d = (*s.doc().unwrap()).clone();
    for (a, b) in centrelines() {
        let vertices =
            outline(sx, truth, a, b).iter().map(|p| PolyVertex { p: Vec2::new(p[0], p[1]), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
        let hatch = Hatch {
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
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Hatch(Box::new(hatch)));
        e.common.layer = "G-BKGD".into();
        d.model.push(e);
    }
    let mut out = Session::empty();
    out.open_drawing(d, "Sample Job", None);
    out
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-vregister-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn reading_room(s: &mut Session) -> (String, Vec<f64>) {
    let r = s.execute("validate.areas", &json!({})).unwrap();
    let all = r["areas"].as_array().cloned().unwrap_or_default();
    let a = all
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("reading")))
        .unwrap_or_else(|| panic!("no reading room in {all:#?}"));
    (a["id"].as_str().unwrap().to_string(), a["box"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect())
}

fn plan<'a>(m: &'a Value, sheet: &str) -> &'a Value {
    m["prints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["sheet"] == sheet && c["registration"].is_object())
        .unwrap_or_else(|| panic!("no registered crop of {sheet} in {:#}", m["prints"]))
}

fn rect_of(c: &Value) -> Vec<f64> {
    c["sheet_rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
}

fn num(v: &Value, key: &str) -> f64 {
    v[key].as_f64().unwrap_or_else(|| panic!("no {key} in {v}"))
}

#[test]
fn crops_placed_off_the_walls_are_refitted_kept_or_marked() {
    let mut s = job();
    let (id, bx) = reading_room(&mut s);
    let out = dir("packet");
    s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "compare": false})).unwrap();
    let m = read_json(&out.join("manifest.json"));

    // The reference plan and the truly registered ceiling plan: kept.
    for sheet in ["A-201", "A-221"] {
        let r = &plan(&m, sheet)["registration"];
        assert_eq!(r["status"], "ok", "{sheet}: {r}");
        assert!(num(r, "residual_ft") < 0.1 && num(r, "coverage") > 0.8, "{sheet}: {r}");
    }
    let rcp = plan(&m, "A-221");
    let x0 = A221 + OX + (bx[0] - 5.0) * 12.0;
    assert!((rect_of(rcp)[0] - x0).abs() < 1.0, "the true crop stays where it was: {rcp}");

    // The 3 ft shift: found and the crop placed again where the sheet has the room.
    let e = plan(&m, "E-201");
    let r = &e["registration"];
    assert_eq!(r["status"], "refitted", "{r}");
    assert!((num(r, "residual_ft") - 3.0).abs() < 0.15, "{r}");
    assert!((r["shift_ft"][0].as_f64().unwrap() - 3.0).abs() < 0.15 && r["shift_ft"][1].as_f64().unwrap().abs() < 0.15, "{r}");
    assert!(num(r, "fit_ft") < 0.1 && num(r, "coverage") > 0.8, "{r}");
    assert!(r["read"].as_str().unwrap().contains("placed again"), "{r}");
    let want_x0 = E201 + OX + (bx[0] - 5.0 + 3.0) * 12.0;
    let want_y1 = OY + (bx[3] + 5.0) * 12.0;
    let got = rect_of(e);
    assert!((got[0] - want_x0).abs() < 2.0 && (got[3] - want_y1).abs() < 2.0, "{got:?} vs {want_x0}, {want_y1}");

    // The 3° turn and the 1.1 scale: measured and placed again.
    let r = &plan(&m, "M-201")["registration"];
    assert_eq!(r["status"], "refitted", "{r}");
    assert!((num(r, "rotation_deg") - 3.0).abs() < 0.1 && (num(r, "scale") - 1.0).abs() < 0.005, "{r}");
    assert!(num(r, "residual_ft") > 1.0, "{r}");
    let r = &plan(&m, "P-201")["registration"];
    assert_eq!(r["status"], "refitted", "{r}");
    assert!((num(r, "scale") - 1.1).abs() < 0.005 && num(r, "rotation_deg").abs() < 0.1, "{r}");
    assert!(num(r, "residual_ft") > 1.0, "{r}");

    // The sheet whose lines aren't the floor: marked, with the numbers.
    let f = plan(&m, "F-201");
    let r = &f["registration"];
    assert_eq!(r["status"], "misregistered", "{r}");
    assert!(r["why"].as_str().is_some_and(|w| !w.is_empty()) && r["coverage"].is_number() && r["residual_ft"].is_number(), "{r}");
    let fx0 = F201 + OX + (bx[0] - 5.0) * 12.0;
    assert!((rect_of(f)[0] - fx0).abs() < 1.0, "a misregistered crop is not moved: {f}");

    // The manifest's summary, worst first; the prompt names the crops placed off the walls.
    let sm = &m["registration"];
    assert_eq!(sm["threshold_ft"].as_f64(), Some(1.0), "{sm}");
    assert!(sm["threshold_from"].as_str().unwrap().starts_with("default"), "{sm}");
    assert_eq!(sm["counts"]["refitted"], 3, "{sm}");
    assert_eq!(sm["counts"]["misregistered"], 1, "{sm}");
    assert_eq!(sm["counts"]["ok"], 2, "{sm}");
    let order: Vec<&str> = sm["by_residual"].as_array().unwrap().iter().map(|r| r["sheet"].as_str().unwrap()).collect();
    assert_eq!(order.first(), Some(&"F-201"), "{order:?}");
    assert_eq!(order.get(1), Some(&"E-201"), "the 3 ft shift is the largest residual: {sm}");
    assert!(order.ends_with(&["A-201", "A-221"]) || order.ends_with(&["A-221", "A-201"]), "{order:?}");
    let prompt = std::fs::read_to_string(out.join("prompt.md")).unwrap();
    for c in [e, f] {
        assert!(prompt.contains(c["file"].as_str().unwrap()), "{prompt}");
    }

    // The overlay over the refitted crop draws the model's walls on the sheet's walls: the
    // reading room's west wall face (blue) where the shifted sheet has it.
    let ov = m["model"].as_array().unwrap().iter().find(|x| x["of"] == e["file"]).unwrap();
    let img = image::open(out.join(ov["file"].as_str().unwrap())).unwrap().to_rgba8();
    let ppu = num(e, "px_per_unit");
    let blue_at = |p: [f64; 2]| {
        let (x, y) = ((p[0] - got[0]) * ppu, (got[3] - p[1]) * ppu);
        (-2i64..=2).any(|dy| {
            (-2i64..=2).any(|dx| {
                let (px, py) = (x.round() as i64 + dx, y.round() as i64 + dy);
                px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() && {
                    let c = img.get_pixel(px as u32, py as u32);
                    c[2] > 160 && c[0] < 90 && c[1] < 90
                }
            })
        })
    };
    let wall_at = on(E201, &shifted, [36.5 - HALF_FT, 34.0]);
    assert!(blue_at(wall_at), "the model's wall should be drawn over the sheet's at {wall_at:?}");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn the_threshold_is_the_projects_and_status_lists_the_sheets_worst_first() {
    let mut s = job();
    let (id, _) = reading_room(&mut s);
    // A bad value is refused; 4 ft keeps the 3 ft shift as placed.
    for bad in ["-2", "wide", "80"] {
        assert!(
            s.execute("project.rules.set", &json!({"domain": "tolerance", "key": "validate.registration.residual_ft", "value": bad, "reason": "x"}))
                .is_err(),
            "{bad}"
        );
    }
    assert!(
        s.execute("project.rules.set", &json!({"domain": "custom", "key": "validate.registration.residual_ft", "value": "4", "reason": "x"}))
            .is_err()
    );
    s.execute(
        "project.rules.set",
        &json!({"domain": "tolerance", "key": "validate.registration.residual_ft", "value": "4", "reason": "the trade sets are loosely registered"}),
    )
    .unwrap();
    let out = dir("loose");
    s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "compare": false})).unwrap();
    let m = read_json(&out.join("manifest.json"));
    assert_eq!(plan(&m, "E-201")["registration"]["status"], "ok", "3 ft is within 4");
    assert_eq!(m["registration"]["threshold_ft"].as_f64(), Some(4.0));
    assert!(m["registration"]["threshold_from"].as_str().unwrap().contains("validate.registration.residual_ft"), "{}", m["registration"]);
    let _ = std::fs::remove_dir_all(&out);

    // A validate run: the status lists the sheets by their crops' registration.
    s.execute("project.rules.set", &json!({"domain": "tolerance", "key": "validate.registration.residual_ft", "value": "1", "reason": "back"}))
        .unwrap();
    let runs = dir("run");
    let start = s.execute("validate", &json!({"scope": "area", "area": id, "out_dir": runs.to_string_lossy()})).unwrap();
    let run = start["run"].as_str().unwrap().to_string();
    let next = s.execute("validate.next", &json!({"run": run})).unwrap();
    assert!(next["packet"]["manifest"].is_string(), "{next}");
    let st = s.execute("validate.status", &json!({"run": run})).unwrap();
    let reg = &st["registration"];
    let sheets: Vec<&str> = reg["sheets"].as_array().unwrap().iter().map(|r| r["sheet"].as_str().unwrap()).collect();
    assert_eq!(sheets.first(), Some(&"F-201"), "misregistered first: {reg:#}");
    assert_eq!(sheets.get(1), Some(&"E-201"), "then the largest residual: {reg:#}");
    let first = &reg["sheets"][0];
    assert_eq!(first["status"], "misregistered");
    assert_eq!(first["worst"]["area"], json!(id));
    let e = &reg["sheets"][1];
    assert_eq!(e["status"], "refitted");
    assert!((e["residual_ft"].as_f64().unwrap() - 3.0).abs() < 0.15, "{e}");
    let residuals: Vec<f64> = reg["sheets"].as_array().unwrap().iter().skip(1).filter_map(|r| r["residual_ft"].as_f64()).collect();
    assert!(residuals.windows(2).all(|w| w[0] >= w[1]), "largest residual first: {residuals:?}");
    assert_eq!(reg["threshold_ft"].as_f64(), Some(1.0));
    let _ = std::fs::remove_dir_all(&runs);
}
