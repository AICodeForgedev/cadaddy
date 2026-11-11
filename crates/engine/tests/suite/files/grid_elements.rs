//! The structural grid is part of the model (Q6407): `model.build` builds the building's grid
//! lines and labels as model elements, one grid per building read from the lowest storey's plan,
//! so the model tree lists them and IFC export writes them as an `IfcGrid` in that storey. Two
//! made-up plans drawn differently: plain circles at both ends of solid lines, and the parts
//! library's hexagon bubbles labelled by attribute at one end of dashed grid lines.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Entity, EntityKind};
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 8})).unwrap();
}

const LETTERS: [&str; 5] = ["A", "B", "C", "D", "E"];
const NUMBERS: [&str; 4] = ["1", "2", "3", "4"];
/// Bays: 25' across the lettered grids, 20' along the numbered ones (inches).
const BAY_X: f64 = 300.0;
const BAY_Y: f64 = 240.0;

/// The outside walls round the grid, on A-WALL.
fn walls(s: &mut Session, p: &dyn Fn(f64, f64) -> [f64; 2]) {
    layer(s, "A-WALL");
    let (w, h) = (4.0 * BAY_X, 3.0 * BAY_Y);
    for (a, b) in [((0.0, 0.0), (w, 0.0)), ((w, 0.0), (w, h)), ((w, h), (0.0, h)), ((0.0, h), (0.0, 0.0))] {
        line(s, p(a.0, a.1), p(b.0, b.1));
    }
}

/// A 5 x 4 grid at `o`: solid lines from bubble to bubble, a plain circle with its label at both
/// ends of each.
fn plain_floor(s: &mut Session, o: [f64; 2]) {
    let p = |x: f64, y: f64| [o[0] + x, o[1] + y];
    layer(s, "S-GRID");
    let top = 3.0 * BAY_Y;
    for (i, label) in LETTERS.iter().enumerate() {
        let x = i as f64 * BAY_X;
        line(s, p(x, -78.0), p(x, top + 78.0));
        for y in [-96.0, top + 96.0] {
            s.execute("circle", &json!({"center": p(x, y), "radius": 18.0})).unwrap();
            text(s, p(x - 4.0, y - 4.0), label);
        }
    }
    let right = 4.0 * BAY_X;
    for (j, label) in NUMBERS.iter().enumerate() {
        let y = j as f64 * BAY_Y;
        line(s, p(-78.0, y), p(right + 78.0, y));
        for x in [-96.0, right + 96.0] {
            s.execute("circle", &json!({"center": p(x, y), "radius": 18.0})).unwrap();
            text(s, p(x - 4.0, y - 4.0), label);
        }
    }
    walls(s, &p);
}

/// The same grid drawn another way: hexagon bubbles (the parts library's block, labelled by its
/// attribute) at the top and right only, each grid line dashed (pieces 6' long with 2' gaps, too
/// short to be read as its line).
fn dashed_floor(s: &mut Session, o: [f64; 2]) {
    let p = |x: f64, y: f64| [o[0] + x, o[1] + y];
    layer(s, "S-GRID");
    let bubble = |s: &mut Session, at: [f64; 2], label: &str| {
        s.execute("part", &json!({"name": "G_GRID_BUBBLE_HEX", "at": at, "scale": 72.0, "attribs": {"GRID": label}, "layer": "current"})).unwrap();
    };
    let dashes = |s: &mut Session, a: [f64; 2], b: [f64; 2]| {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let mut t = 0.0;
        while t < len {
            let e = (t + 72.0).min(len);
            line(s, [a[0] + u[0] * t, a[1] + u[1] * t], [a[0] + u[0] * e, a[1] + u[1] * e]);
            t = e + 24.0;
        }
    };
    let (top, right) = (3.0 * BAY_Y, 4.0 * BAY_X);
    for (i, label) in LETTERS.iter().enumerate() {
        let x = i as f64 * BAY_X;
        dashes(s, p(x, -60.0), p(x, top + 78.0));
        bubble(s, p(x, top + 96.0), label);
    }
    for (j, label) in NUMBERS.iter().enumerate() {
        let y = j as f64 * BAY_Y;
        dashes(s, p(-60.0, y), p(right + 78.0, y));
        bubble(s, p(right + 96.0, y), label);
    }
    walls(s, &p);
}

/// One storey drawn by `floor` at the origin, or two (`L2`'s plan beside `L1`'s, stacked).
fn job(floor: fn(&mut Session, [f64; 2]), storeys: usize) -> Session {
    let mut s = Session::new();
    floor(&mut s, [0.0, 0.0]);
    let mut levels = vec![json!({"name": "L1", "elevation_ft": 0, "height_ft": 12, "region": [-400, -400, 1800, 1400], "base": [0, 0]})];
    if storeys > 1 {
        floor(&mut s, [3000.0, 120.0]);
        levels.push(json!({"name": "L2", "elevation_ft": 12, "height_ft": 12, "region": [2600, -400, 4800, 1400], "base": [3000, 120]}));
    }
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"levels": levels, "stack": storeys > 1, "layers": [{"layer": "A-WALL", "role": "wall"}]}}),
    )
    .unwrap();
    s
}

fn grid_lines(s: &Session) -> Vec<Entity> {
    s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("structural-grid")).map(|e| (**e).clone()).collect()
}

fn tag(e: &Entity, key: &str) -> Option<String> {
    e.common.xdata.iter().find_map(|t| t.value.strip_prefix(&format!("{key}:")).map(str::to_string))
}

/// The labels of the built grid lines, sorted.
fn labels(s: &Session) -> Vec<String> {
    let mut v: Vec<String> = grid_lines(s).iter().filter_map(|e| tag(e, "grid")).collect();
    v.sort();
    v
}

fn all_labels() -> Vec<String> {
    let mut v: Vec<String> = NUMBERS.iter().chain(LETTERS.iter()).map(|l| l.to_string()).collect();
    v.sort();
    v
}

/// The model tree's Structure ▸ Grids branch: its count and its types.
fn tree_grids(s: &mut Session) -> (u64, Vec<String>) {
    let t = s.execute("bim.tree", &json!({"category": "Grids"})).unwrap();
    let mut types = Vec::new();
    let mut n = 0;
    for lv in t["levels"].as_array().unwrap() {
        for tr in lv["trades"].as_array().unwrap() {
            assert_eq!(tr["name"], json!("Structure"), "{t}");
            for c in tr["categories"].as_array().unwrap() {
                n += c["count"].as_u64().unwrap();
                types.extend(c["types"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap().to_string()));
            }
        }
    }
    types.sort();
    (n, types)
}

/// The IFC text's instances: number → (entity name, arguments as written).
fn instances(ifc: &str) -> std::collections::BTreeMap<usize, (String, String)> {
    ifc.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (num, rest) = l.strip_prefix('#')?.split_once('=')?;
            let (name, args) = rest.split_once('(')?;
            Some((num.parse().ok()?, (name.to_string(), args.trim_end_matches(';').trim_end_matches(')').to_string())))
        })
        .collect()
}

fn refs_in(args: &str) -> Vec<usize> {
    args.split(|c: char| !(c.is_ascii_digit() || c == '#')).filter_map(|w| w.strip_prefix('#')?.parse().ok()).collect()
}

/// The IfcGrids written: per grid its storey's name, its U and V axis labels (sorted, each
/// set), and its predefined type.
fn ifc_grids(s: &Session) -> Vec<(String, Vec<String>, Vec<String>, String)> {
    let text = String::from_utf8(cadaddy_io::ifc::write_ifc(s.doc().unwrap(), "Sample Building.ifc").unwrap()).unwrap();
    let inst = instances(&text);
    let mut out = Vec::new();
    for (n, (name, args)) in &inst {
        if name != "IFCGRID" {
            continue;
        }
        // GlobalId, OwnerHistory, Name, Description, ObjectType, ObjectPlacement, Representation,
        // UAxes, VAxes, WAxes, PredefinedType: the axes are the two lists after the representation.
        let lists: Vec<&str> = args.split('(').skip(1).map(|x| x.split(')').next().unwrap()).collect();
        assert_eq!(lists.len(), 2, "UAxes and VAxes: {args}");
        let axis_labels = |list: &str| {
            let mut v: Vec<String> = refs_in(list)
                .iter()
                .map(|r| {
                    let (an, aa) = &inst[r];
                    assert_eq!(an, "IFCGRIDAXIS", "{aa}");
                    // AxisTag, AxisCurve (a 2D polyline), SameSense.
                    let curve = refs_in(aa)[0];
                    assert_eq!(inst[&curve].0, "IFCPOLYLINE");
                    for p in refs_in(&inst[&curve].1) {
                        assert_eq!(inst[&p].1.matches(',').count(), 1, "a 2D point: {:?}", inst[&p]);
                    }
                    aa.split('\'').nth(1).unwrap().to_string()
                })
                .collect();
            v.sort();
            v
        };
        let predefined = args.rsplit('.').nth(1).unwrap().to_string();
        // Contained in its storey.
        let storey = inst
            .values()
            .filter(|(nm, a)| nm == "IFCRELCONTAINEDINSPATIALSTRUCTURE" && refs_in(a.split(")").next().unwrap()).contains(n))
            .map(|(_, a)| *refs_in(a).last().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(storey.len(), 1, "contained once: {storey:?}");
        let (sn, sa) = &inst[&storey[0]];
        assert_eq!(sn, "IFCBUILDINGSTOREY");
        out.push((sa.split('\'').nth(3).unwrap().to_string(), axis_labels(lists[0]), axis_labels(lists[1]), predefined));
    }
    out
}

#[test]
fn a_drawn_grid_is_built_as_model_elements() {
    let mut s = job(plain_floor, 1);
    // Before: the lines are only on the sheet.
    assert!(grid_lines(&s).is_empty());
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["structuralGrid"]["lines"], json!(9), "{}", r["structuralGrid"]);
    assert_eq!(r["structuralGrid"]["level"], json!("L1"));
    assert_eq!(labels(&s), all_labels());
    // Each line on its level's floor, as drawn, read from the plan (drawn, not inferred).
    let lines = grid_lines(&s);
    let a = lines.iter().find(|e| tag(e, "grid").as_deref() == Some("A")).unwrap();
    let EntityKind::Line(l) = &a.kind else { panic!("a grid line: {:?}", a.kind) };
    let (lo, hi) = if l.a.y < l.b.y { (l.a, l.b) } else { (l.b, l.a) };
    assert_eq!((lo.x, lo.y, lo.z, hi.x, hi.y, hi.z), (0.0, -78.0, 0.0, 0.0, 3.0 * BAY_Y + 78.0, 0.0));
    assert!(lines.iter().all(|e| tag(e, "status").as_deref() == Some("drawn") && e.common.layer.as_str() == "3D-Grids"));
    // The model tree: Structure ▸ Grids, one type per label.
    let (n, types) = tree_grids(&mut s);
    assert_eq!(n, 9);
    assert_eq!(types, all_labels().iter().map(|l| format!("Grid {l}")).collect::<Vec<_>>());
    // IFC: one rectangular IfcGrid on L1, the lettered grids one way and the numbered the other.
    let g = ifc_grids(&s);
    assert_eq!(g.len(), 1, "{g:?}");
    let (storey, u, v, pre) = &g[0];
    assert_eq!(storey, "L1");
    let mut uv = [u.clone(), v.clone()];
    uv.sort();
    assert_eq!(uv, [NUMBERS.map(String::from).to_vec(), LETTERS.map(String::from).to_vec()]);
    assert_eq!(pre, "RECTANGULAR");
    // Built again: one grid still, not two.
    s.execute("model.build", &json!({"force": true})).unwrap();
    assert_eq!(grid_lines(&s).len(), 9);
}

#[test]
fn a_dashed_grid_with_block_bubbles_on_two_storeys_is_one_grid() {
    let mut s = job(dashed_floor, 2);
    // A label the drawing repeats in the lettered row (a misread letter between C and D): the row
    // is still a row of grid bubbles, and B one grid.
    s.execute(
        "part",
        &json!({"name": "G_GRID_BUBBLE_HEX", "at": [2.5 * BAY_X, 3.0 * BAY_Y + 96.0], "scale": 72.0, "attribs": {"GRID": "B"}, "layer": "current"}),
    )
    .unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    // Read from the lowest storey's plan only: one grid for the building, not one per storey.
    assert_eq!(r["structuralGrid"]["lines"], json!(9), "{}", r["structuralGrid"]);
    assert_eq!(r["structuralGrid"]["level"], json!("L1"));
    assert_eq!(labels(&s), all_labels());
    let b = grid_lines(&s).into_iter().find(|e| tag(e, "grid").as_deref() == Some("B")).unwrap();
    let EntityKind::Line(bl) = &b.kind else { panic!() };
    assert_eq!((bl.a.x, bl.b.x), (BAY_X, BAY_X), "B is the grid at its own bubble: {bl:?}");
    // A dashed line is laid across its row of bubbles, cut to the grid's extent: grid 1 from its
    // bubble on the right across the lettered grids, not out past the plan.
    let lines = grid_lines(&s);
    let one = lines.iter().find(|e| tag(e, "grid").as_deref() == Some("1")).unwrap();
    let EntityKind::Line(l) = &one.kind else { panic!() };
    let (x0, x1) = (l.a.x.min(l.b.x), l.a.x.max(l.b.x));
    assert!(l.a.y.abs() < 1e-6 && l.b.y.abs() < 1e-6, "{l:?}");
    assert!((-100.0..=0.0).contains(&x0) && (4.0 * BAY_X + 96.0..=4.0 * BAY_X + 140.0).contains(&x1), "{x0} to {x1}");
    let (n, _) = tree_grids(&mut s);
    assert_eq!(n, 9);
    let g = ifc_grids(&s);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0].0, "L1");
    assert_eq!(g[0].1.len() + g[0].2.len(), 9);
}

#[test]
fn a_plan_with_no_grid_builds_none() {
    let mut s = Session::new();
    walls(&mut s, &|x, y| [x, y]);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}], "layers": [{"layer": "A-WALL", "role": "wall"}]}}),
    )
    .unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["structuralGrid"], json!({"lines": 0}));
    assert!(grid_lines(&s).is_empty());
    assert!(ifc_grids(&s).is_empty());
    let _: Value = r;
}

/// A storey whose region holds two architectural sheets: its floor plan (the whole 5 x 4 grid,
/// the walls) and another plan of it with no walls built from it, repeating its grid and adding a
/// grid between two (C.5), so it shows more grids; and on the floor plan a section callout (a
/// bubble naming its sheet, the cut line through it) and a wall tag in a circle. The grid is the
/// floor plan's, where the model is built: whole, and the callout and the tag are not grids.
#[test]
fn the_grid_is_read_from_the_storeys_own_plan_sheet() {
    let mut s = Session::new();
    plain_floor(&mut s, [0.0, 0.0]);
    // The other plan, its sheet above the floor plan's.
    let o = [0.0, 1600.0];
    let p = |x: f64, y: f64| [o[0] + x, o[1] + y];
    layer(&mut s, "S-GRID");
    let lettered: Vec<(f64, &str)> = LETTERS.iter().enumerate().map(|(i, l)| (i as f64 * BAY_X, *l)).chain([(2.5 * BAY_X, "C.5")]).collect();
    for (x, label) in lettered {
        line(&mut s, p(x, -78.0), p(x, 3.0 * BAY_Y + 78.0));
        s.execute("circle", &json!({"center": p(x, 3.0 * BAY_Y + 96.0), "radius": 18.0})).unwrap();
        text(&mut s, p(x - 4.0, 3.0 * BAY_Y + 92.0), label);
    }
    for (j, label) in NUMBERS.iter().enumerate() {
        let y = j as f64 * BAY_Y;
        line(&mut s, p(-78.0, y), p(4.0 * BAY_X + 78.0, y));
        s.execute("circle", &json!({"center": p(-96.0, y), "radius": 18.0})).unwrap();
        text(&mut s, p(-100.0, y - 4.0), label);
    }
    // On the floor plan: a section cut ending at its callout bubble, and a wall tag.
    layer(&mut s, "A-ANNO");
    line(&mut s, [450.0, -300.0], [450.0, 900.0]);
    s.execute("circle", &json!({"center": [450.0, -300.0 - 18.0], "radius": 18.0})).unwrap();
    text(&mut s, [442.0, -322.0], "Q901");
    s.execute("circle", &json!({"center": [150.0, 120.0], "radius": 12.0})).unwrap();
    text(&mut s, [146.0, 116.0], "T7");
    // A keynote numbered 1 in a circle: the sheet's third bubble labelled 1.
    s.execute("circle", &json!({"center": [450.0, 360.0], "radius": 12.0})).unwrap();
    text(&mut s, [446.0, 356.0], "1");
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "region": [-600, -600, 1800, 2700]}],
            "sheets": [
                {"number": "A-101", "title": "FLOOR PLAN", "discipline": "A", "level": "L1", "rect": [-600, -600, 1800, 1100]},
                {"number": "A-401", "title": "PLAN", "discipline": "A", "level": "L1", "rect": [-600, 1300, 1800, 2700]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    let g = &r["structuralGrid"];
    assert_eq!((g["lines"].clone(), g["level"].clone(), g["plan"].clone()), (json!(9), json!("L1"), json!(["A-101"])), "{g}");
    assert_eq!(labels(&s), all_labels());
    // Every line on the floor plan's sheet, none on the other plan's; grid 1 where its bubbles
    // are, not through the keynote.
    for e in grid_lines(&s) {
        let EntityKind::Line(l) = &e.kind else { panic!() };
        assert!(l.a.y < 1100.0 && l.b.y < 1100.0, "{:?}: {l:?}", tag(&e, "grid"));
        if tag(&e, "grid").as_deref() == Some("1") {
            assert_eq!((l.a.y, l.b.y), (0.0, 0.0), "{l:?}");
        }
    }
}
