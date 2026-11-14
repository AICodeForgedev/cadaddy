//! Q6613 fix-wall-list-cannot-move-walls: a wall entry in the spec is an edit the build honours.
//! Moving its centreline moves the wall, deleting it removes the wall built there, adding one
//! builds a wall of its type with no line pair under it, and these survive a rebuild. A built
//! wall can be moved, resized or deleted by command (`wall.edit`), kept by the next rebuild.
//! Made-up plans and wall types only: a masonry shaft drawn as face lines with an outline 4"
//! inside one face (read as a thin wall inside the shaft), in two drawing styles.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line, LwPolyline};
use cadaddy_engine::geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drawn {
    /// Every face a loose line.
    Lines,
    /// Each face loop a closed polyline.
    Outlines,
}

fn push(d: &mut Drawing, kind: EntityKind, layer: &str) {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, kind);
    e.common.layer = layer.into();
    d.model.push(e);
}

/// A line (feet).
fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    push(d, EntityKind::Line(Line { a: p(a), b: p(b) }), layer);
}

/// A rectangle (feet): four lines, or a closed polyline.
fn rect(d: &mut Drawing, drawn: Drawn, layer: &str, (x0, y0): (f64, f64), (x1, y1): (f64, f64)) {
    if drawn == Drawn::Outlines {
        let vertices = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
            .iter()
            .map(|&(x, y)| PolyVertex { p: Vec2::new(x * 12.0, y * 12.0), bulge: 0.0, start_width: 0.0, end_width: 0.0 })
            .collect();
        push(d, EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }), layer);
    } else {
        for (a, b) in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))] {
            line(d, layer, a, b);
        }
    }
}

/// A 10' × 10' shaft of 9" masonry walls (outside faces 0–10', inside 0.75–9.25'), offset by
/// `(ox, oy)` feet, with an outline 4" inside its south face (a car's or a rating line), and a
/// 20' × 16' room of 6" walls east of it. The wall list types the walls as a plan reader does:
/// every wall the plan builds, by its thickness.
fn plan(drawn: Drawn, (ox, oy): (f64, f64)) -> Session {
    let w = "A-WALL";
    let mut d = Drawing::new_imperial();
    let o = |x: f64, y: f64| (x + ox, y + oy);
    rect(&mut d, drawn, w, o(0.0, 0.0), o(10.0, 10.0));
    rect(&mut d, drawn, w, o(0.75, 0.75), o(9.25, 9.25));
    line(&mut d, w, o(0.75, 4.0 / 12.0), o(9.25, 4.0 / 12.0));
    // The room east of it, sharing no wall.
    rect(&mut d, drawn, w, o(14.0, 0.0), o(34.0, 16.0));
    rect(&mut d, drawn, w, o(14.5, 0.5), o(33.5, 15.5));
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": w, "role": "wall"}],
        "wall_types": [{"name": "MX9", "thickness_in": 9}, {"name": "PX6", "thickness_in": 6}, {"name": "PX4", "thickness_in": 4}],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    // The list as a reader writes it: every planned wall, typed by its thickness.
    let walls = planned(&mut s);
    let list: Vec<Value> = walls
        .iter()
        .map(|w| {
            let ty = if w.t > 7.5 {
                "MX9"
            } else if w.t > 5.0 {
                "PX6"
            } else {
                "PX4"
            };
            json!({"level": "L1", "a": [w.a.x, w.a.y], "b": [w.b.x, w.b.y], "wall_type": ty, "source": "A-100", "status": "drawing"})
        })
        .collect();
    let mut sp: Value = s.execute("bspec.get", &json!({})).expect("get")["spec"].clone();
    sp["walls"] = json!(list);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("list");
    s
}

#[derive(Clone, Debug)]
struct W {
    a: Vec2,
    b: Vec2,
    t: f64,
}

/// The walls the plan is read as (inches).
fn planned(s: &mut Session) -> Vec<W> {
    let plan = s.execute("wallgen.plan", &json!({"level": "L1"})).expect("plan");
    plan["walls"]
        .as_array()
        .expect("walls")
        .iter()
        .map(|w| {
            let f = |k: &str, i: usize| w[k][i].as_f64().expect("coordinate");
            W { a: Vec2::new(f("a", 0), f("a", 1)), b: Vec2::new(f("b", 0), f("b", 1)), t: w["thickness"].as_f64().expect("thickness") }
        })
        .collect()
}

/// A built wall: its handle, centreline and thickness (from its mesh), type and tags.
#[derive(Clone, Debug)]
struct Built {
    handle: String,
    a: Vec2,
    b: Vec2,
    t: f64,
    top: f64,
    tags: Vec<String>,
}

impl Built {
    fn ty(&self) -> Option<&str> {
        self.tags.iter().find_map(|t| t.strip_prefix("type:"))
    }
    /// Whether it runs east–west with its centreline at `y` inches (± 0.25").
    fn along_x_at(&self, y: f64) -> bool {
        (self.a.y - self.b.y).abs() < 0.5 && ((self.a.y + self.b.y) / 2.0 - y).abs() < 0.25
    }
}

/// A wall mesh's centreline and thickness: the long axis of its plan points.
fn mesh_line(m: &cadaddy_engine::doc::Mesh) -> ([f64; 2], [f64; 2], f64) {
    let pts: Vec<Vec2> = m.vertices.iter().map(|v| Vec2::new(v.x, v.y)).collect();
    let n = pts.len() as f64;
    let c = pts.iter().fold(Vec2::new(0.0, 0.0), |s, p| s + *p) * (1.0 / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in &pts {
        let d = *p - c;
        sxx += d.x * d.x;
        syy += d.y * d.y;
        sxy += d.x * d.y;
    }
    let ang = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let u = Vec2::new(ang.cos(), ang.sin());
    let nn = u.perp();
    let s: Vec<f64> = pts.iter().map(|p| u.dot(*p - c)).collect();
    let t: Vec<f64> = pts.iter().map(|p| nn.dot(*p - c)).collect();
    let (s0, s1) = (s.iter().copied().fold(f64::MAX, f64::min), s.iter().copied().fold(f64::MIN, f64::max));
    let (t0, t1) = (t.iter().copied().fold(f64::MAX, f64::min), t.iter().copied().fold(f64::MIN, f64::max));
    let mid = c + nn * ((t0 + t1) / 2.0);
    let (a, b) = (mid + u * s0, mid + u * s1);
    ([a.x, a.y], [b.x, b.y], t1 - t0)
}

fn built(s: &Session) -> Vec<Built> {
    let d = s.doc().expect("drawing");
    d.model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("wall"))
        .map(|e| {
            let EntityKind::Mesh(m) = &e.kind else { unreachable!("a wall is a mesh") };
            let (a, b, t) = mesh_line(m);
            let top = m.vertices.iter().map(|v| v.z).fold(f64::MIN, f64::max);
            Built {
                handle: e.handle.hex(),
                a: Vec2::new(a[0], a[1]),
                b: Vec2::new(b[0], b[1]),
                t,
                top,
                tags: e.common.xdata.iter().map(|x| x.value.clone()).collect(),
            }
        })
        .collect()
}

/// The shaft's south side (y within its 9" wall, its middle inside the shaft's 10').
fn south(walls: &[Built], (ox, oy): (f64, f64)) -> Vec<Built> {
    walls
        .iter()
        .filter(|w| {
            let mid = (w.a + w.b) * 0.5;
            (w.a.y - w.b.y).abs() < 0.5
                && mid.y > oy * 12.0 - 0.5
                && mid.y < oy * 12.0 + 9.5
                && mid.x > ox * 12.0
                && mid.x < (ox + 10.0) * 12.0
                && w.a.dist(w.b) > 24.0
        })
        .cloned()
        .collect()
}

/// Whether the shaft's south side still has a misread wall in it (thinner than its 9").
fn misread(walls: &[Built], o: (f64, f64)) -> bool {
    south(walls, o).iter().any(|x| x.t < 8.5)
}

fn list_of(s: &mut Session) -> Vec<Value> {
    s.execute("bspec.get", &json!({})).expect("get")["spec"]["walls"].as_array().cloned().unwrap_or_default()
}

/// The misread thin walls' entries in the list (the shaft's south side read as thin walls), by
/// index.
fn thin_entries(list: &[Value], (ox, oy): (f64, f64)) -> Vec<usize> {
    let v: Vec<usize> = list
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            let f = |k: &str, i: usize| m[k][i].as_f64().unwrap_or(-1e9);
            let mid = (f("a", 0) + f("b", 0)) / 2.0;
            (f("a", 1) - f("b", 1)).abs() < 0.5
                && (f("a", 1) - oy * 12.0) > 0.5
                && (f("a", 1) - oy * 12.0) < 8.5
                && mid > ox * 12.0
                && mid < (ox + 10.0) * 12.0
                && m["wall_type"] == "PX4"
        })
        .map(|(i, _)| i)
        .collect();
    assert!(!v.is_empty(), "the thin walls' entries: {list:?}");
    v
}

const STYLES: [(Drawn, (f64, f64)); 2] = [(Drawn::Lines, (0.0, 0.0)), (Drawn::Outlines, (37.5, -12.25))];

#[test]
fn the_plan_reads_thin_walls_inside_the_shaft() {
    // The reproduction: with the outline 4" inside the shaft's south face, the south side is read
    // as thin walls of their own, not the 9" masonry wall.
    for (drawn, o) in STYLES {
        let mut s = plan(drawn, o);
        s.execute("model.build", &json!({})).expect("build");
        let sw = south(&built(&s), o);
        assert!(misread(&sw, o), "{drawn:?}: thin walls in the shaft's south side: {sw:?}");
        assert!(!sw.iter().any(|x| x.along_x_at(o.1 * 12.0 + 4.5) && (x.t - 9.0).abs() < 0.3), "{drawn:?}: no 9\" wall there: {sw:?}");
    }
}

/// The shaft's true south wall: one 9" wall of its type on the masonry's centreline, 4.5" in.
fn true_south_walls(w: &[Built], o: (f64, f64)) -> Vec<Built> {
    south(w, o).into_iter().filter(|x| x.along_x_at(o.1 * 12.0 + 4.5) && (x.t - 9.0).abs() < 0.3 && x.ty() == Some("MX9")).collect()
}

#[test]
fn a_wall_list_entry_moved_moves_the_wall_and_survives_a_rebuild() {
    for (drawn, o) in STYLES {
        let (ox, oy) = o;
        let mut s = plan(drawn, o);
        s.execute("model.build", &json!({})).expect("build");
        let before = built(&s).len();
        let mut list = list_of(&mut s);
        let thin = thin_entries(&list, o);
        // The first thin wall's entry moved to the true wall (the masonry's centreline, the
        // shaft's width) and given its type; the other misread ones removed.
        let y = oy * 12.0 + 4.5;
        let first = thin[0];
        list[first] = json!({"level": "L1", "a": [ox * 12.0, y], "b": [(ox + 10.0) * 12.0, y], "wall_type": "MX9"});
        for i in thin.iter().skip(1).rev() {
            list.remove(*i);
        }
        let r = s.execute("bspec.set", &json!({"spec": {"walls": list}})).expect("set");
        let built_r = s.execute("model.build", &json!({})).expect("rebuild");
        for round in 0..2 {
            let w = built(&s);
            assert!(!misread(&w, o), "{drawn:?} {round}: the misread walls are gone: {:?}\n{}", south(&w, o), built_r["notes"]);
            let moved = true_south_walls(&w, o);
            assert_eq!(moved.len(), 1, "{drawn:?} {round}: the wall where the entry says, of its type: {:?}", south(&w, o));
            let m = &moved[0];
            assert!(m.tags.iter().any(|t| t == "edit:WE-1"), "{drawn:?} {round}: {m:?}");
            assert!(m.tags.iter().any(|t| t == "status:inferred"), "{drawn:?} {round}: not drawn there: {m:?}");
            assert!(m.tags.iter().any(|t| t.starts_with("basis:moved by wall edit WE-1")), "{drawn:?} {round}: {m:?}");
            assert!(m.tags.iter().any(|t| t.starts_with("edited_from:")), "{drawn:?} {round}: the plan's place kept beside it: {m:?}");
            assert!((m.a.dist(m.b) - 120.0).abs() < 0.5, "{drawn:?} {round}: built to its ends: {m:?}");
            assert_eq!(w.len(), before + 1 - thin.len(), "{drawn:?} {round}: one wall for the misread ones: {w:?}");
            // Rebuilt from scratch: the same.
            s.execute("model.build", &json!({"force": true})).expect("force");
        }
        assert_eq!(r["wallEdits"]["made"].as_array().map(Vec::len), Some(thin.len()), "{drawn:?}: {}", r["wallEdits"]);
        // The edits stay when the list is retyped elsewhere.
        let mut list = list_of(&mut s);
        for m in list.iter_mut().filter(|m| m["wall_type"] == "PX6") {
            m["wall_type"] = json!("PX4");
        }
        let r = s.execute("bspec.set", &json!({"spec": {"walls": list}})).expect("retype");
        assert!(r.get("wallEdits").is_none(), "{drawn:?}: a type change is no edit: {r}");
        s.execute("model.build", &json!({})).expect("build");
        let w = built(&s);
        assert_eq!((true_south_walls(&w, o).len(), misread(&w, o)), (1, false), "{drawn:?}: {:?}", south(&w, o));
    }
}

#[test]
fn a_wall_list_entry_deleted_removes_the_wall_and_one_added_builds_one() {
    for (drawn, o) in STYLES {
        let (ox, oy) = o;
        let mut s = plan(drawn, o);
        s.execute("model.build", &json!({})).expect("build");
        let before = built(&s).len();
        let mut list = list_of(&mut s);
        let thin = thin_entries(&list, o);
        for i in thin.iter().rev() {
            list.remove(*i);
        }
        // A wall added across the room east of the shaft, where no line pair is.
        let (x, y0, y1) = ((ox + 24.0) * 12.0, (oy + 0.5) * 12.0, (oy + 15.5) * 12.0);
        list.push(json!({"level": "L1", "a": [x, y0], "b": [x, y1], "wall_type": "PX4", "height_ft": 8}));
        let r = s.execute("bspec.set", &json!({"spec": {"walls": list}})).expect("set");
        for force in [false, true] {
            s.execute("model.build", &json!({"force": force})).expect("build");
            let w = built(&s);
            assert!(!misread(&w, o), "{drawn:?}: deleted, not left untyped: {:?}", south(&w, o));
            let added: Vec<&Built> = w.iter().filter(|b| (b.a.x - x).abs() < 0.3 && (b.b.x - x).abs() < 0.3 && b.a.dist(b.b) > 170.0).collect();
            assert_eq!(added.len(), 1, "{drawn:?}: the added wall: {w:?}");
            let a = added[0];
            assert!((a.t - 4.0).abs() < 0.2 && a.ty() == Some("PX4"), "{drawn:?}: of its type: {a:?}");
            assert!((a.top - 96.0).abs() < 0.5, "{drawn:?}: its height: {a:?}");
            assert!((a.a.dist(a.b) - (y1 - y0)).abs() < 0.5, "{drawn:?}: built to its ends: {a:?}");
            assert!(a.tags.iter().any(|t| t.starts_with("basis:added by wall edit")), "{drawn:?}: {a:?}");
            assert_eq!(w.len(), before + 1 - thin.len(), "{drawn:?}: the misread ones gone, one added: {w:?}");
        }
        assert_eq!(r["wallEdits"]["made"].as_array().map(Vec::len), Some(thin.len() + 1), "{drawn:?}: {}", r["wallEdits"]);
        // Edits are listed, and the deletes taken back put the plan's walls back.
        let edits = s.execute("wall.edits", &json!({})).expect("edits");
        assert_eq!(edits["total"], thin.len() + 1, "{edits}");
        let dels: Vec<Value> = edits["edits"].as_array().into_iter().flatten().filter(|x| x["op"] == "delete").map(|x| x["id"].clone()).collect();
        for id in dels {
            s.execute("wall.edits", &json!({"revert": id})).expect("revert");
        }
        assert!(misread(&built(&s), o), "{drawn:?}: the plan's walls back");
    }
}

#[test]
fn a_built_wall_moved_resized_or_deleted_by_command_stays_so() {
    for (drawn, o) in STYLES {
        let (ox, oy) = o;
        let mut s = plan(drawn, o);
        s.execute("model.build", &json!({})).expect("build");
        let w = built(&s);
        let n = w.len();
        let thin: Vec<Built> = south(&w, o).into_iter().filter(|x| x.t < 8.5).collect();
        // Delete the misread walls, picked in the model (each delete rebuilds: pick the next one
        // again by where it is).
        for t in &thin {
            let now = built(&s).into_iter().find(|x| x.along_x_at((t.a.y + t.b.y) / 2.0) && (x.t - t.t).abs() < 0.1).expect("the thin wall");
            let r = s.execute("wall.edit", &json!({"op": "delete", "handle": now.handle, "reason": "a car outline, not a wall"})).expect("delete");
            assert_eq!((r["rebuilt"].as_bool(), r["edit"]["op"].as_str()), (Some(true), Some("delete")), "{r}");
        }
        assert!(!misread(&built(&s), o), "{drawn:?}: deleted");
        // Add the masonry wall as a 9" wall of its type.
        let y = (oy * 12.0) + 4.5;
        let to = json!([[ox * 12.0, y], [(ox + 10.0) * 12.0, y]]);
        s.execute("wall.edit", &json!({"op": "add", "level": "L1", "to": to, "wall_type": "MX9"})).expect("add");
        // The room's north wall moved 6" north, then its east wall made 2' shorter.
        let w = built(&s);
        let north_y = (oy + 15.75) * 12.0;
        let north = w.iter().find(|x| x.along_x_at(north_y) && x.a.dist(x.b) > 200.0).expect("the room's north wall").clone();
        let r = s.execute("wall.edit", &json!({"op": "move", "handle": north.handle, "delta_ft": [0, 0.5]})).expect("move");
        assert_eq!(r["walls"].as_array().map(Vec::len), Some(1), "{r}");
        let east_x = (ox + 33.75) * 12.0;
        let east = built(&s).into_iter().find(|x| (x.a.x - east_x).abs() < 0.3 && (x.b.x - east_x).abs() < 0.3).expect("east wall");
        let (lo, hi) = (east.a.y.min(east.b.y), east.a.y.max(east.b.y));
        s.execute("wall.edit", &json!({"op": "resize", "handle": east.handle, "to": [[east_x, lo + 24.0], [east_x, hi]]})).expect("resize");
        for force in [false, true] {
            s.execute("model.build", &json!({"force": force})).expect("build");
            let w = built(&s);
            assert!(!misread(&w, o), "{drawn:?}: still deleted");
            assert_eq!(true_south_walls(&w, o).len(), 1, "{drawn:?}: added: {:?}", south(&w, o));
            assert!(!w.iter().any(|x| x.along_x_at(north_y) && x.a.dist(x.b) > 200.0), "{drawn:?}: moved away: {w:?}");
            let moved = w.iter().find(|x| x.along_x_at(north_y + 6.0)).expect("moved north");
            assert!((moved.t - north.t).abs() < 0.2 && moved.tags.iter().any(|t| t.starts_with("edited_from:")), "{moved:?}");
            let e = w.iter().find(|x| (x.a.x - east_x).abs() < 0.3 && (x.b.x - east_x).abs() < 0.3).expect("east");
            assert!((e.a.y.min(e.b.y) - (lo + 24.0)).abs() < 0.5 && (e.a.y.max(e.b.y) - hi).abs() < 0.5, "{drawn:?}: resized: {e:?}");
            assert_eq!(w.len(), n + 1 - thin.len(), "{drawn:?}: {w:?}");
        }
        let edits = s.execute("wall.edits", &json!({})).expect("edits");
        assert_eq!(edits["total"], thin.len() + 3, "{edits}");
        assert!(edits["edits"].as_array().is_some_and(|e| e.iter().all(|x| x["via"] == "wall.edit")), "{edits}");
    }
}

#[test]
fn wall_edit_refuses_what_it_cannot_do() {
    let mut s = plan(Drawn::Lines, (0.0, 0.0));
    s.execute("model.build", &json!({})).expect("build");
    let floor = s.doc().expect("d").model.iter().find(|e| made_as(e).as_deref() == Some("floor")).map(|e| e.handle.hex());
    for p in [
        json!({}),
        json!({"op": "spin"}),
        json!({"op": "move", "level": "L1", "from": [[0, 0], [0, 0]], "to": [[0, 0], [10, 0]]}),
        json!({"op": "delete", "level": "L1", "from": [[5000, 5000], [5100, 5000]]}),
        json!({"op": "add", "level": "Nowhere", "to": [[0, 0], [10, 0]]}),
        json!({"op": "add", "level": "L1", "to": [[0, 0], [10, 0]], "wall_type": "NoSuchType"}),
        json!({"op": "add", "level": "L1", "to": [[0, 0], [1e300, 0]]}),
        json!({"op": "add", "level": "L1", "to": [[0, 0], [10, 0]], "height_ft": -2}),
        json!({"op": "delete", "handle": floor.unwrap_or_default()}),
    ] {
        assert!(s.execute("wall.edit", &p).is_err(), "refused: {p}");
    }
    assert_eq!(s.execute("wall.edits", &json!({})).expect("edits")["total"], 0);
}
