//! Equipment and fixture symbols drawn on the plan are no walls (Q6423): a through-wall unit
//! under a window (its outline and grille), a tub (its front and basin) and a tag's frame round
//! its number pair up like walls' faces. Walls join other walls or the shell; a symbol's lines
//! are left out, and the window in the gap the unit stands in stays centred in it. A wall stub
//! that joins its wall, fill and all, and a free-standing wall with nothing in it stay walls.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line, LwPolyline, Solid};
use cadaddy_engine::geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

/// How the plan is drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Drawn {
    /// As a print comes in: every line on the wall layer, each symbol's outline in loose lines.
    Print,
    /// As a drafted plan: symbols as closed polylines, the glazing on its own layer.
    Drafted,
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

/// A closed outline through `pts` (feet, with each vertex's bulge).
fn outline(d: &mut Drawing, layer: &str, pts: &[(f64, f64, f64)]) {
    let vertices =
        pts.iter().map(|&(x, y, bulge)| PolyVertex { p: Vec2::new(x * 12.0, y * 12.0), bulge, start_width: 0.0, end_width: 0.0 }).collect();
    push(d, EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }), layer);
}

/// A rectangle `x0..x1` × `y0..y1` (feet): a closed polyline, or four lines in a print.
fn rect(d: &mut Drawing, drawn: Drawn, layer: &str, (x0, y0): (f64, f64), (x1, y1): (f64, f64)) {
    if drawn == Drawn::Drafted {
        outline(d, layer, &[(x0, y0, 0.0), (x1, y0, 0.0), (x1, y1, 0.0), (x0, y1, 0.0)]);
    } else {
        for (a, b) in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))] {
            line(d, layer, a, b);
        }
    }
}

/// A 24' × 14' guest room of 6" walls (feet), a 6'-0" window gap in its south wall (x 9–15) with
/// its jambs and glazing; under the window a through-wall unit (3'-6" × 9", its grille drawn), a
/// 5' × 2'-6" tub against the north wall (its basin a rounded outline, a drain), a room tag's
/// 3' × 1'-3" frame round its number; a 4" closet partition from the north wall (filled), and a
/// 4' free-standing wall. With `symbols` false, the room without the unit, the tub and the tag.
fn room(drawn: Drawn, symbols: bool) -> Session {
    let w = "A-WALL";
    let glaz = if drawn == Drawn::Print { w } else { "A-GLAZ" };
    let mut d = Drawing::new_imperial();
    // The shell: outside faces, inside faces, the gap's jambs and glazing.
    for (a, b) in [((0., 0.), (9., 0.)), ((15., 0.), (24., 0.)), ((24., 0.), (24., 14.)), ((24., 14.), (0., 14.)), ((0., 14.), (0., 0.))] {
        line(&mut d, w, a, b);
    }
    for (a, b) in
        [((0.5, 0.5), (9., 0.5)), ((15., 0.5), (23.5, 0.5)), ((23.5, 0.5), (23.5, 13.5)), ((23.5, 13.5), (0.5, 13.5)), ((0.5, 13.5), (0.5, 0.5))]
    {
        line(&mut d, w, a, b);
    }
    line(&mut d, w, (9., 0.), (9., 0.5));
    line(&mut d, w, (15., 0.), (15., 0.5));
    for y in [0.2, 0.3] {
        line(&mut d, glaz, (9., y), (15., y));
    }
    // A 4" closet partition running from the north wall, its end closed, filled solid.
    for (a, b) in [((8., 13.5), (8., 9.)), ((8. + 4. / 12., 13.5), (8. + 4. / 12., 9.)), ((8., 9.), (8. + 4. / 12., 9.))] {
        line(&mut d, w, a, b);
    }
    let c = |x: f64, y: f64| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    push(&mut d, EntityKind::Solid(Solid { corners: [c(8., 9.), c(8. + 4. / 12., 9.), c(8., 13.5), c(8. + 4. / 12., 13.5)] }), w);
    // A free-standing 6" wall 4' long, nothing in it.
    rect(&mut d, drawn, w, (12., 7.), (16., 7.5));
    if symbols {
        // The through-wall unit against the inside of the gap, its grille a row of openings (each
        // a closed outline, as a print's come in too).
        rect(&mut d, drawn, w, (10.25, 0.5), (13.75, 1.25));
        for i in 0..8 {
            let x = 10.75 + 0.3 * f64::from(i);
            rect(&mut d, Drawn::Drafted, w, (x, 0.8), (x + 0.25, 0.9));
        }
        // The tub: its outline against the north wall, its basin rounded at the corners (4"), a
        // drain.
        rect(&mut d, drawn, w, (17., 11.), (22., 13.5));
        let (x0, y0, x1, y1, r) = (17.25, 11.25, 21.75, 13.25, 4. / 12.);
        let q = (std::f64::consts::FRAC_PI_8).tan();
        outline(
            &mut d,
            w,
            &[
                (x0 + r, y0, 0.0),
                (x1 - r, y0, q),
                (x1, y0 + r, 0.0),
                (x1, y1 - r, q),
                (x1 - r, y1, 0.0),
                (x0 + r, y1, q),
                (x0, y1 - r, 0.0),
                (x0, y0 + r, q),
            ],
        );
        push(&mut d, EntityKind::Circle(cadaddy_engine::doc::Circle { center: c(17.75, 12.25), radius: 1.5 }), w);
        // The room tag's frame.
        rect(&mut d, drawn, w, (3., 6.), (6., 7.25));
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    if symbols {
        s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).expect("layer");
        s.execute("text", &json!({"at": [4.5 * 12.0 - 9.0, 6.4 * 12.0], "text": "905", "height": 6})).expect("tag");
    }
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-GLAZ", "role": "window"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s
}

/// The walls the plan is read as: (middle x, middle y, thickness), inches.
fn planned(s: &mut Session) -> (Vec<(f64, f64, f64)>, Value) {
    let plan = s.execute("wallgen.plan", &json!({"level": "L1"})).expect("plan");
    let walls = plan["walls"]
        .as_array()
        .expect("walls")
        .iter()
        .map(|w| {
            let f = |k: &str, i: usize| w[k][i].as_f64().expect("coordinate");
            ((f("a", 0) + f("b", 0)) / 2.0, (f("a", 1) + f("b", 1)) / 2.0, w["thickness"].as_f64().expect("thickness"))
        })
        .collect();
    (walls, plan)
}

/// Whether a wall's middle lies in the box `x0..x1` × `y0..y1` (feet).
fn within(w: &(f64, f64, f64), (x0, y0): (f64, f64), (x1, y1): (f64, f64)) -> bool {
    w.0 >= x0 * 12.0 && w.0 <= x1 * 12.0 && w.1 >= y0 * 12.0 && w.1 <= y1 * 12.0
}

#[test]
fn symbol_outlines_are_no_walls_and_real_walls_stay() {
    for drawn in [Drawn::Drafted, Drawn::Print] {
        let mut s = room(drawn, true);
        let (walls, plan) = planned(&mut s);
        let unit = walls.iter().filter(|w| within(w, (10., 0.55), (14., 1.5))).count();
        let tub = walls.iter().filter(|w| within(w, (16.9, 10.9), (22.1, 13.4))).count();
        let tag = walls.iter().filter(|w| within(w, (2.9, 5.9), (6.1, 7.35))).count();
        assert_eq!((unit, tub, tag), (0, 0, 0), "{drawn:?}: the unit, the tub and the tag are no walls: {walls:?}\n{}", plan["notes"]);
        assert!(plan["notes"].to_string().contains("symbols"), "{drawn:?}: the notes say so: {}", plan["notes"]);
        // The walls that are walls: the partition joining the north wall (filled), the
        // free-standing one (nothing in it), the shell.
        let partition = walls.iter().filter(|w| within(w, (7.9, 9.), (8.5, 14.)) && (w.2 - 4.0).abs() < 0.1).count();
        let free = walls.iter().filter(|w| within(w, (11.9, 7.), (16.1, 7.5)) && (w.2 - 6.0).abs() < 0.1).count();
        assert_eq!((partition, free), (1, 1), "{drawn:?}: {walls:?}");
        // The same as the room without the symbols.
        let (bare, _) = planned(&mut room(drawn, false));
        assert_eq!(walls.len(), bare.len(), "{drawn:?}: {walls:?} vs {bare:?}");
    }
}

#[test]
fn the_window_stays_centred_in_the_gap_the_unit_stands_in() {
    for drawn in [Drawn::Drafted, Drawn::Print] {
        let mut s = room(drawn, true);
        let built = s.execute("model.build", &json!({})).expect("build");
        let d = s.doc().expect("drawing");
        let windows: Vec<(f64, f64)> = d
            .model
            .iter()
            .filter(|e| made_as(e).as_deref() == Some("window"))
            .map(|e| {
                let b = cadaddy_engine::doc::entity_bounds(d, e, 0);
                (b.min.x, b.max.x)
            })
            .collect();
        assert_eq!(windows.len(), 1, "{drawn:?}: {built}");
        let (x0, x1) = windows[0];
        assert!((x0 + x1) / 2.0 > 143.0 && (x0 + x1) / 2.0 < 145.0, "{drawn:?}: centred on the gap (x 12'): {x0}..{x1}");
        assert!(x1 - x0 > 70.0, "{drawn:?}: the gap's width (6'): {x0}..{x1}");
        // No more walls built than in the room without the symbols.
        let bare = room(drawn, false).execute("model.build", &json!({})).expect("bare");
        assert_eq!(built["walls"], bare["walls"], "{drawn:?}: {built} vs {bare}");
    }
}
