//! Regions in DXF. A REGION entity holds ACIS solid-modeler data, which no public specification
//! describes in enough detail, so CADaddy writes a region as closed LWPOLYLINEs, one per loop,
//! which every DXF reader understands. The first carries `CADADDY` xdata naming it a region
//! and holding its other loops (`1000 REGION`); the others are marked `1000 REGIONPART`. Reading
//! our own file builds the region back from the first and drops the others. Foreign REGIONs are
//! kept as unknown objects.

use cadaddy_doc::{EntityKind, HatchLoop, LwPolyline, MAX_REGION_LOOPS, Region};
use cadaddy_dxf::Tag;
use cadaddy_geom::{LoopEdge, PolyVertex, Spline, Vec2};

use crate::dxf_ext::{APP, xdata, xdata_list};

/// Most vertices of a region's other loops kept in xdata (its size is limited in other
/// programs); past it those loops are written as plain polylines.
const MAX_XDATA_VERTICES: usize = 300;
/// Most vertices read back per region.
const MAX_VERTICES: usize = 1_000_000;
/// Most numbers of exact edges kept in a region's xdata (past it the loops keep their points
/// only), and most read back per region.
const MAX_XDATA_REALS: usize = 1_200;
const MAX_EDGE_REALS: usize = 100_000;

/// One closed LWPOLYLINE: the groups after the common entity groups.
fn polyline_tags(l: &HatchLoop) -> Vec<Tag> {
    let mut t = vec![Tag::i(90, l.vertices.len() as i64), Tag::i(70, 1)];
    for v in &l.vertices {
        t.push(Tag::f(10, v.p.x));
        t.push(Tag::f(20, v.p.y));
        if v.bulge != 0.0 && v.bulge.is_finite() {
            t.push(Tag::f(42, v.bulge));
        }
    }
    t
}

/// The numbers of an exact edge in xdata, after its kind: a line `a, b`, an arc `centre, radius,
/// start, end, ccw`, an elliptical arc `centre, major, ratio, start, end, ccw`, a spline
/// `degree, closed, knots, control points, weights` (counts first).
fn edge_reals(c: &LoopEdge) -> (i64, Vec<f64>) {
    let flag = |b: bool| f64::from(u8::from(b));
    match c {
        LoopEdge::Line { a, b } => (1, vec![a.x, a.y, b.x, b.y]),
        LoopEdge::Arc { center, radius, start, end, ccw } => (2, vec![center.x, center.y, *radius, *start, *end, flag(*ccw)]),
        LoopEdge::Ellipse { center, major, ratio, start, end, ccw } => {
            (3, vec![center.x, center.y, major.x, major.y, *ratio, *start, *end, flag(*ccw)])
        }
        LoopEdge::Spline(sp) => {
            let mut v = vec![sp.degree as f64, flag(sp.closed), sp.knots.len() as f64, sp.control.len() as f64, sp.weights.len() as f64];
            v.extend(&sp.knots);
            v.extend(sp.control.iter().flat_map(|p| [p.x, p.y]));
            v.extend(&sp.weights);
            (4, v)
        }
    }
}

/// The edge records of a loop's exact curves (nothing for a loop of lines and arcs, or one
/// whose curves would pass the `budget` of numbers left).
fn curve_tags(l: &HatchLoop, budget: &mut usize) -> Vec<Tag> {
    let edges: Vec<(i64, Vec<f64>)> = l.curves.iter().map(edge_reals).collect();
    let n: usize = edges.iter().map(|e| e.1.len()).sum();
    if n > *budget {
        return Vec::new();
    }
    *budget -= n;
    let mut t = Vec::new();
    for (kind, reals) in edges {
        t.extend([Tag::s(1000, "EDGE"), Tag::i(1070, kind)]);
        t.extend(reals.into_iter().map(|r| Tag::f(1040, r)));
    }
    t
}

/// The polylines a region is written as (each the groups after the common entity groups,
/// xdata last). The first takes the region's handle.
pub(crate) fn polylines(r: &Region) -> Vec<Vec<Tag>> {
    let Some((first, rest)) = r.loops.split_first() else { return Vec::new() };
    // The exact curves ride along the points they were flattened to (EDGE records after the
    // loop they belong to), so a region read back keeps its ellipses and splines. A loop kept by
    // its curves needs no points in xdata: they are made from the curves when it is read.
    let mut budget = MAX_XDATA_REALS;
    let first_curves = curve_tags(first, &mut budget);
    let rest_curves: Vec<Vec<Tag>> = rest.iter().map(|l| curve_tags(l, &mut budget)).collect();
    let points_in_xdata = |l: &HatchLoop, c: &[Tag]| if c.is_empty() { l.vertices.len() } else { 0 };
    let in_xdata = rest.iter().zip(&rest_curves).map(|(l, c)| points_in_xdata(l, c)).sum::<usize>() <= MAX_XDATA_VERTICES;
    let mut main = polyline_tags(first);
    main.extend([Tag::s(1001, APP), Tag::s(1000, "REGION"), Tag::s(1002, "{"), Tag::i(1070, i64::from(first.outer))]);
    main.extend(first_curves);
    if in_xdata {
        for (l, curves) in rest.iter().zip(&rest_curves) {
            let n = points_in_xdata(l, curves);
            main.extend([Tag::s(1000, "LOOP"), Tag::i(1070, i64::from(l.outer)), Tag::i(1071, n as i64)]);
            if n > 0 {
                for v in &l.vertices {
                    main.extend([Tag::f(1010, v.p.x), Tag::f(1020, v.p.y), Tag::f(1030, 0.0), Tag::f(1040, v.bulge)]);
                }
            }
            main.extend(curves.iter().cloned());
        }
    }
    main.push(Tag::s(1002, "}"));
    let mut out = vec![main];
    for l in rest {
        let mut t = polyline_tags(l);
        if in_xdata {
            t.extend([Tag::s(1001, APP), Tag::s(1000, "REGIONPART")]);
        }
        out.push(t);
    }
    out
}

/// What a closed LWPOLYLINE read as `pl` stands for: a region CADaddy wrote, nothing (a loop
/// of such a region, read with it), or the polyline itself.
pub(crate) fn from_polyline(tags: &[Tag], pl: LwPolyline) -> Option<EntityKind> {
    let x = xdata(tags, APP);
    if x.iter().any(|t| t.code == 1000 && t.str().trim() == "REGIONPART") {
        return None;
    }
    if !x.iter().any(|t| t.code == 1000 && t.str().trim() == "REGION") {
        return Some(EntityKind::LwPolyline(pl));
    }
    let list = xdata_list(x, "REGION");
    let fin = |v: &PolyVertex| v.p.is_finite() && v.bulge.is_finite();
    let first_outer = list.iter().find(|t| t.code == 1070).is_none_or(|t| t.i64() != 0);
    let mut loops =
        vec![HatchLoop { curves: Vec::new(), vertices: pl.vertices.into_iter().filter(fin).collect(), outer: first_outer, sources: Vec::new() }];
    let mut total = 0usize;
    let mut cur: Option<HatchLoop> = None;
    let mut expect_flag = false;
    // Edge records: the loop they belong to (`None` for the first, read from the polyline),
    // their numbers so far and the kind being read.
    let mut first_curves: Vec<(i64, Vec<f64>)> = Vec::new();
    let mut cur_curves: Vec<(i64, Vec<f64>)> = Vec::new();
    let mut in_edge = false;
    let mut reals = 0usize;
    let mut done: Vec<(HatchLoop, Vec<(i64, Vec<f64>)>)> = Vec::new();
    for t in list {
        match t.code {
            1000 if t.str().trim() == "LOOP" => {
                if let Some(l) = cur.take() {
                    done.push((l, std::mem::take(&mut cur_curves)));
                }
                in_edge = false;
                if done.len() + 1 >= MAX_REGION_LOOPS {
                    break;
                }
                cur = Some(HatchLoop { curves: Vec::new(), vertices: Vec::new(), outer: false, sources: Vec::new() });
                expect_flag = true;
            }
            1000 if t.str().trim() == "EDGE" => {
                in_edge = true;
                expect_flag = false;
                let list = if cur.is_some() { &mut cur_curves } else { &mut first_curves };
                list.push((0, Vec::new()));
            }
            1070 if in_edge => {
                if let Some(e) = (if cur.is_some() { &mut cur_curves } else { &mut first_curves }).last_mut() {
                    e.0 = t.i64();
                }
            }
            1040 if in_edge => {
                if reals < MAX_EDGE_REALS
                    && let Some(e) = (if cur.is_some() { &mut cur_curves } else { &mut first_curves }).last_mut()
                {
                    e.1.push(t.f64());
                    reals += 1;
                }
            }
            1070 if expect_flag => {
                if let Some(l) = cur.as_mut() {
                    l.outer = t.i64() != 0;
                }
                expect_flag = false;
            }
            1010 => {
                if let Some(l) = cur.as_mut()
                    && total < MAX_VERTICES
                {
                    l.vertices.push(PolyVertex::new(Vec2::new(t.f64(), 0.0)));
                    total += 1;
                }
            }
            1020 => {
                if let Some(v) = cur.as_mut().and_then(|l| l.vertices.last_mut()) {
                    v.p.y = t.f64();
                }
            }
            1040 => {
                if let Some(v) = cur.as_mut().and_then(|l| l.vertices.last_mut()) {
                    v.bulge = t.f64();
                }
            }
            _ => {}
        }
    }
    if let Some(l) = cur {
        done.push((l, cur_curves));
    }
    let mut with_curves = vec![first_curves];
    for (l, c) in done {
        loops.push(l);
        with_curves.push(c);
    }
    for (l, c) in loops.iter_mut().zip(with_curves) {
        l.vertices.retain(fin);
        let edges = edges_of(&c);
        if l.vertices.is_empty() {
            // Kept by its curves alone: the points are made from them.
            if let Some(e) = edges.filter(|e| closes(e)) {
                *l = HatchLoop::from_curves(e, l.outer, Vec::new());
            }
        } else {
            // Curves that don't make a closed loop of the points' area aren't ours: points only.
            l.curves = edges.filter(|edges| agrees_with_points(edges, &l.vertices)).unwrap_or_default();
        }
    }
    loops.retain(|l| l.vertices.len() >= 2);
    Some(EntityKind::Region(Region { loops }))
}

/// The edges the records hold, or `None` when any is malformed.
fn edges_of(records: &[(i64, Vec<f64>)]) -> Option<Vec<LoopEdge>> {
    let mut out = Vec::new();
    for (kind, r) in records {
        let p = |i: usize| Some(Vec2::new(*r.get(i)?, *r.get(i + 1)?));
        let flag = |i: usize| r.get(i).map(|v| *v != 0.0);
        let edge = match kind {
            1 => LoopEdge::Line { a: p(0)?, b: p(2)? },
            2 => LoopEdge::Arc { center: p(0)?, radius: *r.get(2)?, start: *r.get(3)?, end: *r.get(4)?, ccw: flag(5)? },
            3 => LoopEdge::Ellipse { center: p(0)?, major: p(2)?, ratio: *r.get(4)?, start: *r.get(5)?, end: *r.get(6)?, ccw: flag(7)? },
            4 => {
                let count = |i: usize| r.get(i).filter(|v| **v >= 0.0 && **v <= MAX_EDGE_REALS as f64).map(|v| *v as usize);
                let (nk, nc, nw) = (count(2)?, count(3)?, count(4)?);
                let knots = r.get(5..5 + nk)?.to_vec();
                let control: Vec<Vec2> =
                    r.get(5 + nk..5 + nk + 2 * nc)?.chunks_exact(2).filter_map(|c| Some(Vec2::new(*c.first()?, *c.get(1)?))).collect();
                let weights = r.get(5 + nk + 2 * nc..5 + nk + 2 * nc + nw)?.to_vec();
                let degree = count(0)?;
                LoopEdge::Spline(Spline { degree, knots, control, weights, closed: flag(1)?, ..Spline::default() })
            }
            _ => return None,
        };
        if !edge.is_valid() {
            return None;
        }
        out.push(edge);
    }
    Some(out)
}

/// Whether `edges` run on from one to the next and back to the first.
fn closes(edges: &[LoopEdge]) -> bool {
    !edges.is_empty() && edges.iter().zip(edges.iter().cycle().skip(1)).all(|(a, b)| a.end_point().near(b.start_point(), 1e-6))
}

/// Whether exact `edges` close up and enclose the area of the loop's `points` (the flattened
/// curves) to within a few percent: edges that don't are not this loop's.
fn agrees_with_points(edges: &[LoopEdge], points: &[PolyVertex]) -> bool {
    let flat = cadaddy_geom::Polyline { vertices: points.to_vec(), closed: true }.area().abs();
    closes(edges) && (cadaddy_geom::loop_area(edges).abs() - flat).abs() <= 0.05 * flat.max(1e-12)
}
