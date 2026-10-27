//! Untagged copies of a symbol the import made. The PDF import makes a device symbol only where
//! a mark sits beside it (its type, its circuit): a plan that tags one fixture of a room and
//! draws its neighbours bare ("typical"), or a receptacle with no label of its own, leaves those
//! copies on the page as loose linework. Each symbol block the import made is a template: the
//! same outline (perimeter and size, so any turn), the same number of strokes round it and the
//! same drawn length is the same device, of the template's type.

use super::{PageDevice, dist, is_circuit_tag};
use crate::device_types::Legend;
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use std::collections::{BTreeMap, BTreeSet};

/// Most loose strokes looked at on a page (hostile files).
const MAX_SHAPES: usize = 400_000;
/// Most strokes in a template (a symbol is a handful).
const MAX_TEMPLATE_STROKES: usize = 64;
/// Most copies found per template, per symbol the import made of it (and a few more): a
/// template that matches the plan's own linework stops there.
const MAX_TWINS_PER_SYMBOL: usize = 12;
/// How far the outline's perimeter and size may differ from the template's (share).
const OUTLINE_TOL: f64 = 0.10;
/// How far the drawn length round it may differ from the template's (share).
const LENGTH_TOL: f64 = 0.12;
/// How much past the template's reach a copy's strokes may lie (share).
const REACH_SLACK: f64 = 1.15;

/// Layers the import keeps lettering on (hidden text, the strokes OCR text was read from).
fn lettering_layer(name: &str) -> bool {
    name.eq_ignore_ascii_case(cadaddy_io::pdf_read::HIDDEN_TEXT_LAYER) || name.eq_ignore_ascii_case(cadaddy_io::pdf_read::SHX_SOURCE_LAYER)
}

/// One stroke: its points, whether it closes, its length and box.
#[derive(Clone, Debug)]
struct Shape {
    pts: Vec<[f64; 2]>,
    /// A circle's centre and radius (its points are then empty).
    circle: Option<([f64; 2], f64)>,
    closed: bool,
    len: f64,
    bbox: [f64; 4],
    handle: String,
}

impl Shape {
    fn of(kind: &EntityKind, handle: String) -> Option<Shape> {
        let (pts, circle, closed_flag) = match kind {
            EntityKind::Line(l) => (vec![[l.a.x, l.a.y], [l.b.x, l.b.y]], None, false),
            EntityKind::LwPolyline(p) if p.vertices.len() >= 2 && p.vertices.len() <= 4_096 => {
                (p.vertices.iter().map(|v| [v.p.x, v.p.y]).collect(), None, p.closed)
            }
            EntityKind::Circle(c) if c.radius.is_finite() && c.radius > 0.0 => (Vec::new(), Some(([c.center.x, c.center.y], c.radius)), true),
            _ => return None,
        };
        if pts.iter().flatten().any(|v| !v.is_finite()) || circle.is_some_and(|(c, _)| c.iter().any(|v| !v.is_finite())) {
            return None;
        }
        let (len, bbox, closed) = match circle {
            Some((c, r)) => (std::f64::consts::TAU * r, [c[0] - r, c[1] - r, c[0] + r, c[1] + r], true),
            None => {
                let mut len: f64 = pts.windows(2).filter_map(|w| Some(dist(*w.first()?, *w.get(1)?))).sum();
                let (first, last) = (pts.first().copied().unwrap_or([0.0; 2]), pts.last().copied().unwrap_or([0.0; 2]));
                if closed_flag {
                    len += dist(first, last);
                }
                let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
                for p in &pts {
                    b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
                }
                let span = (b[2] - b[0]).max(b[3] - b[1]);
                // A loop drawn back to its start closes too.
                let closed = closed_flag || (pts.len() >= 4 && dist(first, last) <= 1e-3 * span.max(1e-12));
                (len, b, closed)
            }
        };
        Some(Shape { pts, circle, closed, len, bbox, handle })
    }

    fn mid(&self) -> [f64; 2] {
        [(self.bbox[0] + self.bbox[2]) / 2.0, (self.bbox[1] + self.bbox[3]) / 2.0]
    }

    fn size(&self) -> f64 {
        (self.bbox[2] - self.bbox[0]).max(self.bbox[3] - self.bbox[1])
    }

    /// The farthest the stroke reaches from `c`.
    fn reach_from(&self, c: [f64; 2]) -> f64 {
        match self.circle {
            Some((m, r)) => dist(m, c) + r,
            None => self.pts.iter().map(|p| dist(*p, c)).fold(0.0, f64::max),
        }
    }
}

/// A symbol block as a template: its outline's perimeter and size, its strokes' count and
/// length, how far they reach from the outline's middle, and the device it stands for.
struct Template {
    perimeter: f64,
    size: f64,
    strokes: usize,
    len: f64,
    reach: f64,
    device: PageDevice,
    /// Symbols the import made of it on the page.
    made: usize,
}

fn template(d: &Drawing, block: &str, device: PageDevice, made: usize) -> Option<Template> {
    let b = d.blocks.get(block)?;
    let shapes: Vec<Shape> = b.entities.iter().filter_map(|e| Shape::of(&e.kind, String::new())).collect();
    if shapes.len() < 2 || shapes.len() > MAX_TEMPLATE_STROKES {
        return None;
    }
    let outline = shapes.iter().filter(|s| s.closed).max_by(|a, b| a.size().total_cmp(&b.size()))?;
    let c = outline.mid();
    let reach = shapes.iter().map(|s| s.reach_from(c)).fold(0.0, f64::max);
    let len: f64 = shapes.iter().map(|s| s.len).sum();
    (outline.size() > 0.0 && reach > 0.0 && len > 0.0).then(|| Template {
        perimeter: outline.len,
        size: outline.size(),
        strokes: shapes.len(),
        len,
        reach,
        device,
        made,
    })
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs()
}

/// The untagged copies on page `d` of the symbols in `syms` (the import's inserts, with the
/// devices they stand for): each comes back as a device of its template's mark, at its
/// outline's middle, its handle the outline's. `inserts` are every symbol insert's block and
/// place (legend samples too); `devices` says which of `syms` count.
pub(super) fn find(
    d: &Drawing,
    inserts: &[(String, [f64; 2], String)],
    syms: &[PageDevice],
    counts: &dyn Fn(&PageDevice) -> bool,
    legend: &Legend,
) -> Vec<PageDevice> {
    // Templates: each block with a counted device, the device its first insert (whose strokes
    // the block holds) stands for.
    let by_handle: BTreeMap<&str, &PageDevice> = syms.iter().map(|dv| (dv.handle.as_str(), dv)).collect();
    let mut first: BTreeMap<&str, &str> = BTreeMap::new();
    let mut made: BTreeMap<&str, usize> = BTreeMap::new();
    for (block, _, handle) in inserts {
        first.entry(block.as_str()).or_insert(handle.as_str());
        *made.entry(block.as_str()).or_default() += 1;
    }
    let mut templates: Vec<Template> = Vec::new();
    for (block, handle) in &first {
        let device = match by_handle.get(handle) {
            Some(dv) if counts(dv) => (*dv).clone(),
            Some(_) => continue,
            // A legend sample draws the block: its mark names the type.
            None => match inserts.iter().find(|(_, _, h)| h == handle) {
                Some(_) => {
                    let Some(tag) = d
                        .model
                        .iter()
                        .find(|e| e.handle.hex() == *handle)
                        .and_then(|e| match &e.kind {
                            EntityKind::Insert(ins) => ins.attribs.iter().find(|a| a.tag == "TAG").map(|a| a.text.value.trim().to_string()),
                            _ => None,
                        })
                        .filter(|t| !t.is_empty() && !is_circuit_tag(t))
                    else {
                        continue;
                    };
                    PageDevice { tag, at: [0.0; 2], handle: String::new(), alt: None, circuit_only: false, twin: false }
                }
                None => continue,
            },
        };
        if let Some(t) = template(d, block, device, made.get(block).copied().unwrap_or(0)) {
            templates.push(t);
        }
    }
    if templates.is_empty() {
        return Vec::new();
    }
    // The richest templates first: a plain circle must not take a receptacle's outline.
    templates.sort_by(|a, b| b.strokes.cmp(&a.strokes).then(b.len.total_cmp(&a.len)));
    let max_reach = templates.iter().map(|t| t.reach).fold(0.0, f64::max) * REACH_SLACK;
    let max_size = 2.0 * max_reach;

    // The page's loose strokes, small enough to be part of a symbol, by cell.
    let off: BTreeSet<String> = d.layers.iter().filter(|l| !l.on || lettering_layer(&l.name)).map(|l| l.name.to_ascii_lowercase()).collect();
    let mut shapes: Vec<Shape> = Vec::new();
    for e in d.model.iter() {
        if shapes.len() >= MAX_SHAPES {
            break;
        }
        if lettering_layer(&e.common.layer) || off.contains(&e.common.layer.to_ascii_lowercase()) {
            continue;
        }
        if let Some(s) = Shape::of(&e.kind, String::new())
            && s.size() <= max_size
        {
            shapes.push(Shape { handle: e.handle.hex(), ..s });
        }
    }
    let cell = max_reach.max(1e-9);
    let key = |p: [f64; 2]| ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64);
    let mut grid: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, s) in shapes.iter().enumerate() {
        grid.entry(key(s.mid())).or_default().push(i);
    }
    // The symbols already made, by cell: a copy is never one of them.
    let mut made_at: BTreeMap<(i64, i64), Vec<[f64; 2]>> = BTreeMap::new();
    for (_, at, _) in inserts {
        made_at.entry(key(*at)).or_default().push(*at);
    }

    let mut used = vec![false; shapes.len()];
    let mut out: Vec<PageDevice> = Vec::new();
    for t in &templates {
        let cap = t.made.saturating_mul(MAX_TWINS_PER_SYMBOL).saturating_add(4);
        let mut found = 0usize;
        let reach = t.reach * REACH_SLACK;
        for i in 0..shapes.len() {
            if found >= cap {
                break;
            }
            let Some(o) = shapes.get(i) else { continue };
            if used.get(i).copied().unwrap_or(true) || !o.closed || !near(o.len, t.perimeter, OUTLINE_TOL) || !near(o.size(), t.size, OUTLINE_TOL) {
                continue;
            }
            let c = o.mid();
            if legend.covers(Vec2::new(c[0], c[1])) {
                continue;
            }
            let (cx, cy) = key(c);
            let cells = || (-1..=1).flat_map(move |dx| (-1..=1).map(move |dy| (cx + dx, cy + dy)));
            if cells().any(|k| made_at.get(&k).is_some_and(|ps| ps.iter().any(|p| dist(*p, c) <= t.reach))) {
                continue;
            }
            // Every unused stroke lying wholly within its reach.
            let mut members: Vec<usize> = Vec::new();
            let mut len = 0.0;
            for k in cells() {
                for &j in grid.get(&k).into_iter().flatten() {
                    if let Some(s) = shapes.get(j)
                        && !used.get(j).copied().unwrap_or(true)
                        && s.reach_from(c) <= reach
                    {
                        members.push(j);
                        len += s.len;
                    }
                }
            }
            let strokes_ok = if t.strokes >= 4 { members.len().abs_diff(t.strokes) <= 1 } else { members.len() == t.strokes };
            if !strokes_ok || !near(len, t.len, LENGTH_TOL) {
                continue;
            }
            for j in members {
                if let Some(u) = used.get_mut(j) {
                    *u = true;
                }
            }
            found += 1;
            out.push(PageDevice { at: c, handle: o.handle.clone(), twin: true, ..t.device.clone() });
        }
    }
    out
}
