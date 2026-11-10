//! A window's frame or glazing drawn across a gap in a wall is the window, not a piece of wall
//! (Q6424): the lines lying wholly within the gap's extent and the wall's depth (the frame's
//! outline, its sill and glazing lines) are never paired into a wall bridging the gap, so the gap
//! stays an opening and the window built there has its hole. A wall drawn with a line along its
//! whole length between its faces, or a wall piece in line between two others with nothing
//! drawn inside it, is still a wall.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// A line on the wall layer, inches.
fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x, y, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

/// What the south wall's gap at x 120–192" holds.
#[derive(Clone, Copy, PartialEq, Debug)]
enum InGap {
    /// A closed 72" rectangle flush with both faces and a glazing line along its centre (a
    /// window frame drawn the wall's depth).
    Frame,
    /// Two glazing lines 3" apart, inside the wall's depth, from jamb to jamb.
    Glazing,
    /// A wall piece the wall's thickness, its faces drawn as their own lines and its ends closed,
    /// nothing inside it: a piece of the wall.
    WallPiece,
    /// A frame rectangle as in `Frame`, but the line along its centre runs on the whole wall's
    /// length (a wall drawn with a line between its faces): the wall runs through.
    LineThrough,
    /// The frame's outline as in `Frame`, an inner frame (the sash) 6" deep inset 2 1/2" from
    /// each jamb inside it, a sill line 4" outside the wall's face along the opening and a
    /// stool's outline past its inside face.
    InsetFrame,
}

/// A 40' × 20' building of 14" outside walls (inches). Its south wall stops either side of a 6'
/// gap (x 120–192), its jambs drawn, and the gap holds `in_gap`; a window tag W7 stands outside
/// it. The others run through.
fn building(in_gap: InGap) -> Session {
    let mut d = Drawing::new_imperial();
    let t = 14.0;
    let (w, h) = (480.0, 240.0);
    // South wall: two pieces, the faces of each, and the jambs at the gap.
    for (x0, x1) in [(0.0, 120.0), (192.0, w)] {
        line(&mut d, (x0, 0.0), (x1, 0.0));
        let (i0, i1) = (if x0 == 0.0 { t } else { x0 }, if x1 == w { w - t } else { x1 });
        line(&mut d, (i0, t), (i1, t));
    }
    // The gap's jambs (a frame's ends are drawn on them).
    line(&mut d, (120.0, 0.0), (120.0, t));
    line(&mut d, (192.0, 0.0), (192.0, t));
    match in_gap {
        InGap::Frame | InGap::WallPiece => {
            line(&mut d, (120.0, 0.0), (192.0, 0.0));
            line(&mut d, (120.0, t), (192.0, t));
            if in_gap == InGap::Frame {
                line(&mut d, (120.0, t / 2.0), (192.0, t / 2.0));
            }
        }
        InGap::Glazing => {
            line(&mut d, (120.0, t / 2.0 - 1.5), (192.0, t / 2.0 - 1.5));
            line(&mut d, (120.0, t / 2.0 + 1.5), (192.0, t / 2.0 + 1.5));
        }
        InGap::InsetFrame => {
            line(&mut d, (120.0, 0.0), (192.0, 0.0));
            line(&mut d, (120.0, t), (192.0, t));
            for y in [t / 2.0 - 3.0, t / 2.0 + 3.0] {
                line(&mut d, (122.5, y), (189.5, y));
            }
            line(&mut d, (122.5, t / 2.0 - 3.0), (122.5, t / 2.0 + 3.0));
            line(&mut d, (189.5, t / 2.0 - 3.0), (189.5, t / 2.0 + 3.0));
            line(&mut d, (122.5, t / 2.0), (189.5, t / 2.0));
            line(&mut d, (120.0, -4.0), (192.0, -4.0));
            // A stool's outline 27" long, 3" deep, 1"–4" past the wall's inside face.
            line(&mut d, (142.5, t + 1.0), (169.5, t + 1.0));
            line(&mut d, (142.5, t + 4.0), (169.5, t + 4.0));
        }
        InGap::LineThrough => {
            line(&mut d, (120.0, 0.0), (192.0, 0.0));
            line(&mut d, (120.0, t), (192.0, t));
            line(&mut d, (0.0, t / 2.0), (w, t / 2.0));
        }
    }
    // East, north and west walls, drawn through.
    for (a, b) in [((w, 0.0), (w, h)), ((w, h), (0.0, h)), ((0.0, h), (0.0, 0.0))] {
        line(&mut d, a, b);
    }
    for (a, b) in [((w - t, t), (w - t, h - t)), ((w - t, h - t), (t, h - t)), ((t, h - t), (t, t))] {
        line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).expect("layer");
    s.execute("text", &json!({"at": [150.0, -30.0], "text": "W7", "height": 6})).expect("tag");
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"}],
        "wall_types": [{"name": "Exterior", "thickness_in": 14}],
        "schedule": {"rows": [{"kind": "window", "mark": "W7", "width_in": 72, "height_in": 48, "sill_ft": 3}]},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s
}

/// The planned walls with any part over the gap's middle stretch (x 130–182") in the south
/// wall's depth.
fn walls_in_gap(s: &mut Session) -> Vec<Value> {
    let plan = s.execute("wallgen.plan", &json!({"level": "L1"})).expect("plan");
    plan["walls"]
        .as_array()
        .expect("walls")
        .iter()
        .filter(|w| {
            let (ax, ay, bx, by) = (w["a"][0].as_f64(), w["a"][1].as_f64(), w["b"][0].as_f64(), w["b"][1].as_f64());
            let (Some(ax), Some(ay), Some(bx), Some(by)) = (ax, ay, bx, by) else { return false };
            ay.max(by) <= 14.0 && ay.min(by) >= 0.0 && ax.min(bx) < 182.0 && ax.max(bx) > 130.0
        })
        .cloned()
        .collect()
}

/// The windows built, and whether a built wall stands solid at the gap's middle at window height
/// (x 156", y 7", 5' up: in the window's hole).
fn built(s: &mut Session) -> (Value, usize, bool) {
    let r = s.execute("model.build", &json!({})).expect("build");
    let d = s.doc().expect("doc");
    let windows = d.model.iter().filter(|e| made_as(e).as_deref() == Some("window")).count();
    let solid = d.model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).any(|e| match &e.kind {
        EntityKind::Mesh(m) => inside(m, Vec3 { x: 156.0, y: 7.0, z: 60.0 }),
        _ => false,
    });
    (r, windows, solid)
}

/// `p` inside the closed mesh `m`: a ray straight up from it crosses its faces an odd number of
/// times.
fn inside(m: &cadaddy_engine::doc::Mesh, p: Vec3) -> bool {
    let mut crossings = 0;
    for f in &m.faces {
        let vs: Vec<Vec3> = f.v.iter().filter_map(|&i| m.vertices.get(i as usize).copied()).collect();
        for k in 1..vs.len().saturating_sub(1) {
            let (a, b, c) = (vs[0], vs[k], vs[k + 1]);
            let det = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
            if det.abs() < 1e-9 {
                continue;
            }
            let u = ((p.x - a.x) * (c.y - a.y) - (c.x - a.x) * (p.y - a.y)) / det;
            let v = ((b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y)) / det;
            if u >= 0.0 && v >= 0.0 && u + v < 1.0 && a.z + u * (b.z - a.z) + v * (c.z - a.z) > p.z {
                crossings += 1;
            }
        }
    }
    crossings % 2 == 1
}

#[test]
fn a_window_frame_drawn_the_walls_depth_across_a_gap_is_no_wall() {
    let mut s = building(InGap::Frame);
    let bridged = walls_in_gap(&mut s);
    assert!(bridged.is_empty(), "the frame's lines are not paired into a wall across the gap: {bridged:?}");
    let (r, windows, solid) = built(&mut s);
    assert_eq!(windows, 1, "the window in the gap: {r}");
    assert!(!solid, "the window has its hole: no wall solid through it: {r}");
    assert_eq!(r["window_tags"][0]["by_basis"], json!({"tag": 1}), "the tag's window is the gap's, not one cut into a wall drawn through: {r}");
    let plan = s.execute("wallgen.plan", &json!({"level": "L1"})).expect("plan");
    assert!(plan["notes"].as_array().expect("notes").iter().any(|n| n.as_str().is_some_and(|n| n.contains("window frames"))), "{plan}");
}

#[test]
fn glazing_lines_between_two_wall_ends_are_no_wall() {
    let mut s = building(InGap::Glazing);
    let bridged = walls_in_gap(&mut s);
    assert!(bridged.is_empty(), "the glazing lines are not paired into a wall across the gap: {bridged:?}");
    let (r, windows, solid) = built(&mut s);
    assert_eq!(windows, 1, "the window in the gap: {r}");
    assert!(!solid, "the window has its hole: {r}");
    assert_eq!(r["window_tags"][0]["by_basis"], json!({"tag": 1}), "{r}");
}

#[test]
fn an_inner_frame_inset_from_the_jambs_and_a_sill_are_no_wall_either() {
    let mut s = building(InGap::InsetFrame);
    let bridged = walls_in_gap(&mut s);
    assert!(bridged.is_empty(), "no pair of the frame, the sash or the sill bridges the gap: {bridged:?}");
    let plan = s.execute("wallgen.plan", &json!({"level": "L1"})).expect("plan");
    let stool = plan["walls"].as_array().expect("walls").iter().any(|w| w["a"][1].as_f64().is_some_and(|y| y > 14.0 && y < 19.0));
    assert!(!stool, "the stool is no wall: {plan}");
    let (r, windows, solid) = built(&mut s);
    assert_eq!(windows, 1, "{r}");
    assert!(!solid, "{r}");
}

/// A 40' × 20' building of 14" outside walls whose south wall is a row of 72" windows (each a
/// frame rectangle with glazing along its centre) between 6' piers, each pier drawn with its two
/// faces, its ends and a line between its faces (a veneer's back): x 0–48 wall, 48–120 window,
/// 120–192 pier, 192–264 window, 264–336 pier, 336–408 window, 408–480 wall.
fn facade() -> Session {
    let mut d = Drawing::new_imperial();
    let t = 14.0;
    let (w, h) = (480.0, 240.0);
    let windows = [(48.0, 120.0), (192.0, 264.0), (336.0, 408.0)];
    let mut ends = vec![0.0];
    for (a, b) in windows {
        ends.extend([a, b]);
    }
    ends.push(w);
    for piece in ends.chunks(2) {
        let (x0, x1) = (piece[0], piece[1]);
        line(&mut d, (x0, 0.0), (x1, 0.0));
        let (i0, i1) = (if x0 == 0.0 { t } else { x0 }, if x1 == w { w - t } else { x1 });
        line(&mut d, (i0, t), (i1, t));
        if x0 > 0.0 && x1 < w {
            line(&mut d, (x0, 4.0), (x1, 4.0));
        }
    }
    for (a, b) in windows {
        for x in [a, b] {
            line(&mut d, (x, 0.0), (x, t));
        }
        line(&mut d, (a, 0.0), (b, 0.0));
        line(&mut d, (a, t), (b, t));
        line(&mut d, (a, t / 2.0), (b, t / 2.0));
    }
    for (a, b) in [((w, 0.0), (w, h)), ((w, h), (0.0, h)), ((0.0, h), (0.0, 0.0))] {
        line(&mut d, a, b);
    }
    for (a, b) in [((w - t, t), (w - t, h - t)), ((w - t, h - t), (t, h - t)), ((t, h - t), (t, t))] {
        line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"}],
        "wall_types": [{"name": "Exterior", "thickness_in": 14}],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s
}

#[test]
fn piers_drawn_with_a_line_inside_them_between_windows_stay_walls() {
    let mut s = facade();
    let plan = s.execute("wallgen.plan", &json!({"level": "L1"})).expect("plan");
    let south: Vec<(f64, f64)> = plan["walls"]
        .as_array()
        .expect("walls")
        .iter()
        .filter_map(|w| {
            let (ax, ay, bx, by) = (w["a"][0].as_f64()?, w["a"][1].as_f64()?, w["b"][0].as_f64()?, w["b"][1].as_f64()?);
            (ay.max(by) <= 14.0 && ay.min(by) >= 0.0 && (ay - by).abs() < 1.0).then_some((ax.min(bx), ax.max(bx)))
        })
        .collect();
    // Each pier is wall (whatever the windows beside it are read as).
    for (x0, x1) in [(130.0, 182.0), (274.0, 326.0)] {
        assert!(south.iter().any(|(a, b)| *a <= x0 && *b >= x1), "the pier at {x0}–{x1} is wall: {south:?}");
    }
}

#[test]
fn a_wall_piece_with_nothing_inside_it_is_still_wall() {
    // Nothing drawn inside the piece says window: the wall runs through and the tag's window is
    // cut into it there (inferred), as for any tag by a wall drawn through.
    let mut s = building(InGap::WallPiece);
    assert!(!walls_in_gap(&mut s).is_empty(), "the piece is wall");
}

#[test]
fn a_line_along_the_whole_wall_between_its_faces_is_no_window_frame() {
    let mut s = building(InGap::LineThrough);
    assert!(!walls_in_gap(&mut s).is_empty(), "the line runs on past the gap: the wall runs through");
}
