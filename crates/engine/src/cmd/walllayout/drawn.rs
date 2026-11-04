//! The sheets' drawn dimensions on the model (Q6178 dimreader): each dimension the reader
//! placed ([`super::super::dimreader`]) is attached at its two witness points to what it
//! measures — a device's centre, a wall face, an opening's edge, a casework edge — and a
//! device whose along-face offset a chain gives gets it documented.
//!
//! A chain is attached as one: the witness points are slid together along the dimension (a
//! registration is good to a few inches, not to the inch) to where most of them fall on an
//! object, then each witness takes the object nearest it. A device's offset is the sum of the
//! chain's values between it and the nearest fixed end of the chain (a face, an opening edge, a
//! casework edge); a chain of devices only fixes no offset.

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::Handle;
use cadaddy_doc::props::Source;
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::super::dimreader::{Drawn, Placed};
use super::{Ctx, Dev, Obj, Ref, dot, ft_in, r2, same_level, span};

/// A witness's object is this near its witness point (inches), plus the registration's error: a
/// device's centre, the span of a face along the witness line, an opening's face, casework.
const REACH_IN: f64 = 12.0;
/// Witness points slide together at most this far: twice the registration's error, at least
/// this (inches).
const SLIDE_MIN_IN: f64 = 2.0;
/// A witness is on an object within this (inches) when the chain is slid, and takes the
/// nearest object within this much at most.
const TIGHT_IN: f64 = 2.0;
const LOOSE_IN: f64 = 12.0;
/// A face runs along the witness line (square to the dimension) within this cosine; an
/// opening's face runs along the dimension within this.
const SQUARE_COS: f64 = 0.2;
const ALONG_COS: f64 = 0.95;
/// Ends listed in the report, at most.
const MAX_LISTED: usize = 40;

/// What a witness point is on.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Hit {
    Device(usize),
    Face(usize),
    Opening { face: usize, edge: f64, kind: &'static str },
    Casework { obj: usize, point: Vec2, side: &'static str },
}

impl Hit {
    fn kind(&self) -> &'static str {
        match self {
            Hit::Device(_) => "device centre",
            Hit::Face(_) => "wall face",
            Hit::Opening { .. } => "opening edge",
            Hit::Casework { .. } => "casework edge",
        }
    }
    /// Which wins a tie: the device, then an opening, casework, a face.
    fn rank(&self) -> u8 {
        match self {
            Hit::Device(_) => 0,
            Hit::Opening { .. } => 1,
            Hit::Casework { .. } => 2,
            Hit::Face(_) => 3,
        }
    }
    fn fixed(&self) -> bool {
        !matches!(self, Hit::Device(_))
    }
}

/// A device's along-face offset as a chain documents it.
#[derive(Clone, Debug)]
pub(super) struct DocOffset {
    pub reference: Ref,
    /// Inches, the drawn value(s).
    pub value_in: f64,
    /// The dimension's direction (model) — the offset is along the device's face only when its
    /// face runs this way.
    pub dir: Vec2,
    pub source: Source,
    pub why: String,
    /// The reference is a casework edge (in front of the wall, not on its line).
    pub casework: bool,
}

/// What attaching found.
#[derive(Default)]
pub(super) struct Attached {
    /// Per placed dimension: what each end is on (or why nothing).
    pub ends: Vec<(Result<Hit, String>, Result<Hit, String>)>,
    /// Device → its documented offset.
    pub docs: HashMap<Handle, DocOffset>,
    /// Devices a chain measures but fixes no offset for, with why.
    pub undocumented: Vec<(Handle, String)>,
}

/// One candidate on a witness line: where it is along the dimension, and what it is.
struct Cand {
    at: f64,
    hit: Hit,
}

/// The objects a witness point may be on. A device only where the sheet draws a symbol at the
/// witness (a wall's witness has none): `symbol`.
fn candidates(cx: &Ctx, devs: &[Dev], objs: &[Obj], level: &str, w: Vec2, u: Vec2, reach: f64, symbol: bool) -> Vec<Cand> {
    let n = u.perp();
    let lv = Some(level);
    let mut out = Vec::new();
    for (i, dv) in devs.iter().enumerate().filter(|(_, d)| symbol && same_level(d.level.as_deref(), lv)) {
        // A device in a wall that runs along the witness line is that wall's, not the
        // dimension's: the dimension measures along the device's own wall.
        if dv.normal().is_some_and(|nm| dot(nm.normalized(), u).abs() > 0.7) {
            continue;
        }
        let c = dv.c();
        if dot(c - w, n).abs() <= reach && dot(c - w, u).abs() <= reach {
            out.push(Cand { at: dot(c, u), hit: Hit::Device(i) });
        }
    }
    for (k, f) in cx.faces.iter().enumerate().filter(|(_, f)| same_level(f.level.as_deref(), lv)) {
        let cu = dot(f.u, u).abs();
        if cu <= SQUARE_COS {
            // The face runs along the witness line: is the witness point beside its span?
            let t = f.along(w);
            if t >= -reach && t <= f.len + reach && f.depth(w).abs() <= reach {
                out.push(Cand { at: dot(f.a, u), hit: Hit::Face(k) });
            }
        } else if cu >= ALONG_COS && f.depth(w).abs() <= reach {
            let mut edges: Vec<(f64, &'static str)> = f.openings.iter().flat_map(|o| [(o.from, o.kind), (o.to, o.kind)]).collect();
            // An end of the face no square wall meets (a door's jamb where the wall stops, an
            // open bay): an opening's edge too.
            let open = |nb: Option<&String>| nb.and_then(|n| cx.face(f.level.as_deref(), n)).is_none_or(|g| dot(g.u, f.u).abs() >= ALONG_COS);
            if open(f.prev.as_ref()) {
                edges.push((0.0, "opening"));
            }
            if open(f.next.as_ref()) {
                edges.push((f.len, "opening"));
            }
            for (edge, kind) in edges {
                let p = f.at(edge);
                if dot(p - w, u).abs() <= reach {
                    out.push(Cand { at: dot(p, u), hit: Hit::Opening { face: k, edge, kind } });
                }
            }
        }
    }
    for (j, o) in objs.iter().enumerate().filter(|(_, o)| same_level(o.level.as_deref(), lv)) {
        if o.pts.is_empty() {
            continue;
        }
        let (n0, n1) = span(&o.pts, w, n);
        if n0 > reach || n1 < -reach {
            continue;
        }
        let (u0, u1) = span(&o.pts, Vec2::ZERO, u);
        for (at, side) in [(u0, "start"), (u1, "end")] {
            if (at - dot(w, u)).abs() <= reach {
                let p = w + u * (at - dot(w, u));
                out.push(Cand { at, hit: Hit::Casework { obj: j, point: p, side } });
            }
        }
    }
    out
}

/// One chain's witnesses: their points (model), each extension's way, and the values between.
struct Chain<'a> {
    dims: Vec<usize>,
    points: Vec<Vec2>,
    sheet_points: Vec<Vec2>,
    symbols: Vec<bool>,
    values: Vec<f64>,
    texts: Vec<String>,
    to_scale: Vec<bool>,
    p: &'a Placed,
}

fn chains(drawn: &Drawn) -> Vec<Chain<'_>> {
    let mut by: BTreeMap<(String, usize), Vec<usize>> = BTreeMap::new();
    for (i, p) in drawn.placed.iter().enumerate() {
        by.entry((p.sheet.clone(), p.s.chain)).or_default().push(i);
    }
    let mut out = Vec::new();
    for (_, mut idx) in by {
        idx.sort_by_key(|i| drawn.placed.get(*i).map_or(0, |p| p.s.index));
        let mut cur: Option<Chain> = None;
        for i in idx {
            let Some(p) = drawn.placed.get(i) else { continue };
            let joins = cur.as_ref().is_some_and(|c| c.sheet_points.last().is_some_and(|q| q.dist(p.s.ends.0.on_line) <= p.s.h * 0.4));
            if !joins {
                if let Some(c) = cur.take() {
                    out.push(c);
                }
                cur = Some(Chain {
                    dims: Vec::new(),
                    points: vec![p.a],
                    sheet_points: vec![p.s.ends.0.on_line],
                    symbols: vec![p.s.ends.0.symbol],
                    values: Vec::new(),
                    texts: Vec::new(),
                    to_scale: Vec::new(),
                    p,
                });
            }
            if let Some(c) = cur.as_mut() {
                c.dims.push(i);
                c.points.push(p.b);
                c.sheet_points.push(p.s.ends.1.on_line);
                c.symbols.push(p.s.ends.1.symbol);
                c.to_scale.push(p.s.to_scale);
                c.values.push(p.s.inches);
                c.texts.push(p.s.text.clone());
            }
        }
        if let Some(c) = cur.take() {
            out.push(c);
        }
    }
    out
}

/// Attach every placed dimension (see the module docs).
pub(super) fn attach(cx: &Ctx, devs: &[Dev], objs: &[Obj], drawn: &Drawn) -> Attached {
    let inch = cx.inch();
    let mut out = Attached { ends: drawn.placed.iter().map(|_| (Err(String::new()), Err(String::new()))).collect(), ..Attached::default() };
    for ch in chains(drawn) {
        let p = ch.p;
        let u = p.dir;
        let reach = REACH_IN * inch + p.residual;
        let cands: Vec<Vec<Cand>> =
            ch.points.iter().zip(&ch.symbols).map(|(w, sym)| candidates(cx, devs, objs, &p.level, *w, u, reach, *sym)).collect();
        // Slide the chain to where most witnesses fall on an object.
        let slide_max = (SLIDE_MIN_IN * inch).max(2.0 * p.residual);
        let tight = TIGHT_IN * inch;
        let mut shifts: Vec<f64> = vec![0.0];
        for (w, cs) in ch.points.iter().zip(&cands) {
            for c in cs {
                let d = c.at - dot(*w, u);
                if d.abs() <= slide_max {
                    shifts.push(d);
                }
            }
        }
        shifts.truncate(4_000);
        let score =
            |d: f64| -> usize { ch.points.iter().zip(&cands).filter(|(w, cs)| cs.iter().any(|c| (c.at - dot(**w, u) - d).abs() <= tight)).count() };
        let best = shifts.iter().map(|d| (score(*d), *d)).max_by(|a, b| a.0.cmp(&b.0).then(b.1.abs().total_cmp(&a.1.abs()))).map_or(0.0, |x| x.1);
        // Each witness: the nearest object to its slid point.
        let hits: Vec<Result<Hit, String>> = ch
            .points
            .iter()
            .zip(&cands)
            .map(|(w, cs)| {
                let at = dot(*w, u) + best;
                let mut ranked: Vec<(f64, &Cand)> = cs.iter().map(|c| ((c.at - at).abs(), c)).filter(|(d, _)| *d <= LOOSE_IN * inch).collect();
                ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
                let near = ranked.first().map_or(0.0, |x| x.0);
                ranked.iter().filter(|(d, _)| *d <= near + 0.5 * inch).min_by_key(|(_, c)| c.hit.rank()).map(|(_, c)| c.hit.clone()).ok_or_else(
                    || {
                        if cs.is_empty() {
                            format!("nothing on its level within {} of its witness point", ft_in(reach / inch))
                        } else {
                            format!("no object within {} of its witness line", ft_in(LOOSE_IN))
                        }
                    },
                )
            })
            .collect();
        for (k, di) in ch.dims.iter().enumerate() {
            if let Some(e) = out.ends.get_mut(*di) {
                *e = (hits.get(k).cloned().unwrap_or_else(|| Err(String::new())), hits.get(k + 1).cloned().unwrap_or_else(|| Err(String::new())));
            }
        }
        // Each device the chain measures: its offset from the chain's nearest fixed end.
        for (k, h) in hits.iter().enumerate() {
            let Ok(Hit::Device(dev)) = h else { continue };
            let Some(dv) = devs.get(*dev) else { continue };
            let fixed =
                (0..hits.len()).filter(|j| hits.get(*j).is_some_and(|x| x.as_ref().is_ok_and(Hit::fixed))).min_by_key(|j| (j.abs_diff(k), *j));
            let Some(r) = fixed else {
                out.undocumented.push((dv.h, "its chain has no fixed end (a face, an opening or casework edge)".into()));
                continue;
            };
            let (lo, hi) = (r.min(k), r.max(k));
            // A value that doesn't fit its drawn length (not to scale, or not a dimension's at
            // all) documents nothing.
            if ch.to_scale.get(lo..hi).is_none_or(|v| v.iter().any(|ok| !ok)) {
                out.undocumented.push((dv.h, "a value on its way to the fixed end does not fit its drawn length (not to scale)".into()));
                continue;
            }
            let value: f64 = ch.values.get(lo..hi).map_or(0.0, |v| v.iter().sum());
            let text = ch.texts.get(lo..hi).map(|t| t.join(" + ")).unwrap_or_default();
            let c = dv.c();
            let reference = match hits.get(r) {
                Some(Ok(Hit::Face(fk))) => match cx.faces.get(*fk) {
                    Some(f) => Ref::Face { name: f.name.clone(), wall: f.wall, inward: f.inward },
                    None => continue,
                },
                Some(Ok(Hit::Opening { face, edge, kind })) => match cx.faces.get(*face) {
                    Some(f) => {
                        let point = f.at(*edge);
                        Ref::End {
                            name: format!("the {kind} edge at {} of {}", ft_in(edge / inch), f.name),
                            point,
                            sign: if dot(c - point, f.u) >= 0.0 { 1.0 } else { -1.0 },
                        }
                    }
                    None => continue,
                },
                Some(Ok(Hit::Casework { obj, point, side })) => match objs.get(*obj) {
                    Some(o) => {
                        let side_word = if *side == "start" { "near" } else { "far" };
                        Ref::End {
                            name: format!("the {side_word} edge of {}", o.name),
                            point: *point,
                            sign: if dot(c - *point, u) >= 0.0 { 1.0 } else { -1.0 },
                        }
                    }
                    None => continue,
                },
                _ => continue,
            };
            let at_sheet = ch.sheet_points.get(k).copied().unwrap_or(Vec2::ZERO);
            let view = p.view.as_deref().map(|v| format!(" ({v})")).unwrap_or_default();
            let why = format!("a dimension on {}{view}: {} from {} to the device, read off the sheet's line work", p.sheet, text, reference.name());
            let casework = matches!(hits.get(r), Some(Ok(Hit::Casework { .. })));
            let doc = DocOffset {
                reference,
                value_in: value,
                dir: u,
                source: Source::print(&p.sheet, Some([r2(at_sheet.x), r2(at_sheet.y)])),
                why,
                casework,
            };
            // Two chains measuring one device: the shorter offset (the nearer reference).
            match out.docs.get(&dv.h) {
                Some(old) if old.value_in <= doc.value_in => {}
                _ => {
                    out.docs.insert(dv.h, doc);
                }
            }
        }
    }
    out
}

/// The `bim.layout.dims` report: per sheet, what was read, placed, attached and documented.
pub(super) fn report(drawn: &Drawn, at: &Attached, devs: &[Dev], cx: &Ctx, conflicts: &[Value], limit: usize) -> Value {
    let inch = cx.inch();
    let mut sheets: BTreeMap<String, Value> = BTreeMap::new();
    for (num, r) in &drawn.by_sheet {
        if r.values == 0 {
            continue;
        }
        sheets.insert(
            num.clone(),
            json!({
                "values": r.values, "read": r.read, "placed": r.placed, "unplaced": r.unplaced, "values_unused": r.unused,
                "stretches_without_value": r.no_value, "chains": r.chains, "baseline_groups": r.baselines, "not_to_scale": r.not_to_scale,
                "scale_in_per_unit": r.scale_in_per_unit, "registration": r.registration,
                "attached": {"both_ends": 0, "one_end": 0, "none": 0}, "ends": {}, "unattached_why": {},
            }),
        );
    }
    let mut listed: Vec<Value> = Vec::new();
    for (i, p) in drawn.placed.iter().enumerate() {
        let Some((e0, e1)) = at.ends.get(i) else { continue };
        let Some(sh) = sheets.get_mut(&p.sheet) else { continue };
        let n_ok = usize::from(e0.is_ok()) + usize::from(e1.is_ok());
        let key = match n_ok {
            2 => "both_ends",
            1 => "one_end",
            _ => "none",
        };
        sh["attached"][key] = json!(sh["attached"][key].as_u64().unwrap_or(0) + 1);
        for e in [e0, e1] {
            match e {
                Ok(h) => {
                    let k = h.kind();
                    sh["ends"][k] = json!(sh["ends"][k].as_u64().unwrap_or(0) + 1);
                }
                Err(why) if !why.is_empty() => {
                    sh["unattached_why"][why.as_str()] = json!(sh["unattached_why"][why.as_str()].as_u64().unwrap_or(0) + 1);
                }
                Err(_) => {}
            }
        }
        if listed.len() < limit.min(MAX_LISTED) {
            let name = |e: &Result<Hit, String>| -> Value {
                match e {
                    Ok(Hit::Device(d)) => json!({"device": devs.get(*d).map(|x| x.h.hex())}),
                    Ok(Hit::Face(f)) => json!({"face": cx.faces.get(*f).map(|x| x.name.clone())}),
                    Ok(Hit::Opening { face, edge, kind }) => {
                        json!({"opening": kind, "face": cx.faces.get(*face).map(|x| x.name.clone()), "edge_in": r2(edge / inch)})
                    }
                    Ok(Hit::Casework { side, .. }) => json!({"casework_edge": side}),
                    Err(w) => json!({"none": w}),
                }
            };
            listed.push(json!({
                "sheet": p.sheet, "view": p.view, "level": p.level, "value": p.s.text, "inches": r2(p.s.inches),
                "on_sheet": [[r2(p.s.ends.0.point.x), r2(p.s.ends.0.point.y)], [r2(p.s.ends.1.point.x), r2(p.s.ends.1.point.y)]],
                "model_ft": [[r2(p.a.x / inch / 12.0), r2(p.a.y / inch / 12.0)], [r2(p.b.x / inch / 12.0), r2(p.b.y / inch / 12.0)]],
                "value_at": [r2(p.s.text_at.x), r2(p.s.text_at.y)], "registration": p.method, "model_units_per_sheet_unit": (p.scale * 1e4).round() / 1e4,
                "marks": [p.s.ends.0.mark.name(), p.s.ends.1.mark.name()],
                "extension_lines": [p.s.ends.0.ext.len() > 0.5, p.s.ends.1.ext.len() > 0.5], "to_scale": p.s.to_scale, "chain": p.s.chain, "baseline": p.s.baseline,
                "ends": [name(e0), name(e1)],
            }));
        }
    }
    let mut undocumented: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, why) in &at.undocumented {
        *undocumented.entry(why.as_str()).or_default() += 1;
    }
    let read: usize = drawn.by_sheet.values().map(|r| r.read).sum();
    json!({
        "read": read,
        "placed": drawn.placed.len(),
        "devices_documented": at.docs.len(),
        "devices_measured_not_fixed": undocumented,
        "conflicts": conflicts.len(),
        "conflicts_first": conflicts.iter().take(limit.min(MAX_LISTED)).collect::<Vec<_>>(),
        "by_sheet": sheets,
        "dimensions_first": listed,
    })
}
