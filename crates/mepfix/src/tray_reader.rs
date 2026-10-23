//! Cable tray and large conduit banks from the electrical power plans, for coordination (Q266):
//! where they run, their size and how high they hang. No circuiting.
//!
//! - **Tray**: a tag `W x D CT` (`12x4 CT`) beside a pair of long parallel lines `W` inches apart
//!   (the ladder's sides; its rungs are short and left alone). The tray runs where both sides do.
//! - **Conduit banks**: a tag `(N) D"C` beside `N` long parallel lines of the same extent, a
//!   conduit's width apart; each line is one conduit.
//! - **Heights**: the tag's second line, `BOT @ 9'-8"` (bottom of tray) or `BOC @ 9'-8"` (bottom of
//!   conduit), above the floor.
//! - **Riser banks**: small circles of the riser symbol's size beside a note `(N) D"C UP …` /
//!   `… FROM …`.
//!
//! Positions come from Q255's sheet map (each sheet aligned by its grid), not from the truth.

use crate::score::FoundRun;
use crate::sheet_reader::{Alignment, RecognisedSheet, SheetMap, building_from_model};
use crate::truth::Truth;
use cadaddy_doc::{Drawing, EntityKind};

/// Most lines and texts read from one sheet (hostile drawings).
const MAX_ITEMS: usize = 100_000;
/// Lines shorter than this (feet) aren't runs (rungs, symbols, tag leaders).
const MIN_RUN_FT: f64 = 4.0;

/// A conduit riser bank found on a plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundRiserBank {
    pub level: String,
    pub conduits: usize,
    pub size_in: f64,
    /// `up` or `down` (from the note).
    pub direction: String,
    pub at_ft: Vec<[f64; 2]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayResult {
    /// Trays (`kind: tray`, size [width, depth]) and conduits (`kind: conduit`).
    pub runs: Vec<FoundRun>,
    pub risers: Vec<FoundRiserBank>,
}

/// A nominal size as printed: `2"`, `1-1/4"`, `3/4` (inches).
fn parse_pipe_size(s: &str) -> Option<f64> {
    let t = s.trim().trim_end_matches('"').trim();
    if t.is_empty() || t.len() > 8 {
        return None;
    }
    let frac = |f: &str| -> Option<f64> {
        let (a, b) = f.split_once('/')?;
        let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
        (b > 0.0).then(|| a / b)
    };
    let v = if let Some((w, f)) = t.split_once('-') {
        w.parse::<f64>().ok()? + frac(f)?
    } else if t.contains('/') {
        frac(t)?
    } else {
        t.parse::<f64>().ok()?
    };
    (v > 0.0 && v <= 12.0).then_some(v)
}

/// Feet and inches (`9'-8"`, `10'-7 1/2"`) → feet.
fn parse_feet_inches(s: &str) -> Option<f64> {
    let (f, rest) = s.trim().split_once('\'')?;
    let feet = f.trim().parse::<f64>().ok()?;
    let mut inches = 0.0;
    for part in rest.trim().trim_start_matches('-').trim().trim_end_matches('"').split_whitespace() {
        inches += match part.split_once('/') {
            Some((a, b)) => {
                let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
                if b == 0.0 {
                    return None;
                }
                a / b
            }
            None => part.parse::<f64>().ok()?,
        };
    }
    Some(feet + inches / 12.0)
}

/// `12x4 CT` → (12, 4).
pub fn parse_tray_tag(s: &str) -> Option<(f64, f64)> {
    let t = s.trim().to_uppercase();
    let size = t.strip_suffix("CT")?.trim();
    let (w, d) = size.split_once('X')?;
    let (w, d) = (w.trim().trim_end_matches('"').parse::<f64>().ok()?, d.trim().trim_end_matches('"').parse::<f64>().ok()?);
    (w > 0.0 && d > 0.0 && w <= 60.0 && d <= 12.0).then_some((w, d))
}

/// `(2) 3"C` → (2, 3.0).
pub fn parse_bank_tag(s: &str) -> Option<(usize, f64)> {
    let t = s.trim().to_uppercase();
    let rest = t.strip_prefix('(')?;
    let (n, rest) = rest.split_once(')')?;
    let n = n.trim().parse::<usize>().ok().filter(|n| (1..=24).contains(n))?;
    let size = rest.split_whitespace().next()?.strip_suffix('C')?;
    Some((n, parse_pipe_size(size)?))
}

/// `BOT @ 9'-8"` / `BOC @ 9'-8"` → feet.
fn parse_bottom(s: &str) -> Option<f64> {
    let t = s.trim().to_uppercase();
    let rest = t.strip_prefix("BOT").or_else(|| t.strip_prefix("BOC"))?;
    parse_feet_inches(rest.trim_start_matches([' ', '@']).trim())
}

struct Line2 {
    a: [f64; 2],
    b: [f64; 2],
}

impl Line2 {
    fn len(&self) -> f64 {
        dist(self.a, self.b)
    }
    fn dir(&self) -> [f64; 2] {
        let l = self.len().max(1e-12);
        [(self.b[0] - self.a[0]) / l, (self.b[1] - self.a[1]) / l]
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Read every power plan's tray, conduit banks and riser banks.
pub fn read_trays(dxf_data: &[(String, Vec<u8>)], map: &SheetMap) -> TrayResult {
    let mut res = TrayResult::default();
    for (name, bytes) in dxf_data {
        let stem = name.trim_end_matches(".dxf");
        let Some(sh) = map.sheets.iter().find(|s| s.number == stem) else { continue };
        if sh.discipline != "E" || !sh.title.to_uppercase().contains("POWER") {
            continue;
        }
        let (Some(level), Some(al)) = (&sh.level, &sh.alignment) else { continue };
        let Ok(d) = cadaddy_io::read_dxf(bytes) else { continue };
        read_sheet(&mut res, sh, &d, level, al);
    }
    res
}

fn read_sheet(res: &mut TrayResult, sh: &RecognisedSheet, d: &Drawing, level: &str, al: &Alignment) {
    let to_b = |x: f64, y: f64| building_from_model(al, [x, y]);
    let k = if sh.scale > 0.0 { 12.0 / sh.scale } else { 96.0 }; // model inches per paper inch
    let mut lines: Vec<Line2> = Vec::new();
    // Short lines: a ladder tray's rungs among them.
    let mut shorts: Vec<Line2> = Vec::new();
    let mut texts: Vec<(String, [f64; 2])> = Vec::new();
    let mut circles: Vec<([f64; 2], f64)> = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        match &e.kind {
            EntityKind::Line(l) => {
                let ln = Line2 { a: to_b(l.a.x, l.a.y), b: to_b(l.b.x, l.b.y) };
                if ln.len() >= MIN_RUN_FT {
                    lines.push(ln);
                } else if ln.len() < 5.0 && shorts.len() < MAX_ITEMS {
                    shorts.push(ln);
                }
            }
            EntityKind::Text(t) => {
                let p = t.align_pt.unwrap_or(t.insert);
                texts.push((t.value.clone(), to_b(p.x, p.y)));
            }
            // Radius in drawing inches.
            EntityKind::Circle(c) => circles.push((to_b(c.center.x, c.center.y), c.radius)),
            _ => {}
        }
    }
    // The height note under a tag: the nearest BOT/BOC text.
    let bottom_near = |p: [f64; 2]| {
        texts
            .iter()
            .filter_map(|(t, q)| parse_bottom(t).map(|b| (b, dist(*q, p))))
            .filter(|(_, dd)| *dd < 6.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0)
    };
    // Parallel lines' offset and overlap.
    let rel = |a: &Line2, b: &Line2| -> Option<(f64, [f64; 2], [f64; 2])> {
        let u = a.dir();
        let v = b.dir();
        if (u[0] * v[1] - u[1] * v[0]).abs() > 1e-3 {
            return None;
        }
        let off = (b.a[0] - a.a[0]) * -u[1] + (b.a[1] - a.a[1]) * u[0];
        let t = |p: [f64; 2]| (p[0] - a.a[0]) * u[0] + (p[1] - a.a[1]) * u[1];
        let (s0, s1) = (t(b.a).min(t(b.b)).max(0.0), t(b.a).max(t(b.b)).min(a.len()));
        (s1 - s0 > MIN_RUN_FT).then_some((off, [a.a[0] + u[0] * s0, a.a[1] + u[1] * s0], [a.a[0] + u[0] * s1, a.a[1] + u[1] * s1]))
    };
    // Perpendicular distance from a point to a line's span (infinite outside it).
    let beside = |l: &Line2, p: [f64; 2]| {
        let u = l.dir();
        let s = (p[0] - l.a[0]) * u[0] + (p[1] - l.a[1]) * u[1];
        if s < 0.0 || s > l.len() { f64::INFINITY } else { ((p[0] - l.a[0]) * -u[1] + (p[1] - l.a[1]) * u[0]).abs() }
    };
    for (t, p) in &texts {
        if let Some((w, dep)) = parse_tray_tag(t) {
            // The pair of sides `w` apart nearest the tag.
            let wf = w / 12.0;
            let mut best: Option<(f64, [f64; 2], [f64; 2])> = None;
            for (i, a) in lines.iter().enumerate() {
                let da = beside(a, *p);
                if da > 6.0 {
                    continue;
                }
                for b in lines.iter().skip(i + 1) {
                    let Some((off, s0, s1)) = rel(a, b) else { continue };
                    if (off.abs() - wf).abs() > wf * 0.05 + 0.02 {
                        continue;
                    }
                    let u = a.dir();
                    let n = [-u[1] * off / 2.0, u[0] * off / 2.0];
                    let c0 = [s0[0] + n[0], s0[1] + n[1]];
                    let c1 = [s1[0] + n[0], s1[1] + n[1]];
                    // A ladder: rungs square across from side to side (two walls a tray's width
                    // apart have none).
                    let mid = Line2 { a: c0, b: c1 };
                    let rungs = shorts
                        .iter()
                        .filter(|r| (r.len() - off.abs()).abs() < 0.02 && beside(&mid, [(r.a[0] + r.b[0]) / 2.0, (r.a[1] + r.b[1]) / 2.0]) < 0.02)
                        .count();
                    if rungs < 3 {
                        continue;
                    }
                    let score = da;
                    if best.as_ref().is_none_or(|bb| score < bb.0) {
                        best = Some((score, c0, c1));
                    }
                }
            }
            if let Some((_, c0, c1)) = best {
                res.runs.push(FoundRun {
                    level: level.into(),
                    kind: "tray".into(),
                    system: "CT".into(),
                    shape: "tray".into(),
                    size_in: [w, dep],
                    from_ft: c0,
                    to_ft: c1,
                    bottom_ft: bottom_near(*p),
                });
            }
        } else if let Some((n, size)) = parse_bank_tag(t) {
            if t.to_uppercase().contains(" UP") || t.to_uppercase().contains(" FROM") {
                // A riser bank's note: the riser circles beside it.
                let r0 = 0.04 * k; // the riser symbol's radius, drawing inches
                let mut at: Vec<[f64; 2]> =
                    circles.iter().filter(|(c, r)| (*r - r0).abs() < r0 * 0.15 && dist(*c, *p) < 8.0).map(|(c, _)| *c).collect();
                at.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
                at.dedup_by(|a, b| dist(*a, *b) < 0.05);
                res.risers.push(FoundRiserBank {
                    level: level.into(),
                    conduits: n,
                    size_in: size,
                    direction: if t.to_uppercase().contains(" UP") { "up".into() } else { "down".into() },
                    at_ft: at,
                });
                continue;
            }
            // `n` parallel lines of one extent nearest the tag, stacked away from it at a regular
            // conduit spacing (a tray's side rail beyond the bank is not one of them).
            let od = crate::design::pipe_od(size) / 12.0;
            let mut best: Option<(f64, Vec<([f64; 2], [f64; 2])>)> = None;
            for a in &lines {
                let da = beside(a, *p);
                if da > 6.0 {
                    continue;
                }
                // Offsets of same-extent parallels, signed away from the tag.
                let away = {
                    let u = a.dir();
                    let side = (p[0] - a.a[0]) * -u[1] + (p[1] - a.a[1]) * u[0];
                    if side > 0.0 { -1.0 } else { 1.0 }
                };
                let mut offs: Vec<(f64, &Line2)> = lines
                    .iter()
                    .filter_map(|b| {
                        let (off, _, _) = rel(a, b)?;
                        let same_extent = (dist(a.a, b.a).min(dist(a.a, b.b)) - off.abs()).abs() < 0.1 && (b.len() - a.len()).abs() < 0.1;
                        (same_extent && off * away > 0.01).then_some((off * away, b))
                    })
                    .collect();
                offs.sort_by(|x, y| x.0.total_cmp(&y.0));
                let Some(&(step, _)) = offs.first() else {
                    if n == 1 && best.as_ref().is_none_or(|b| da < b.0) {
                        best = Some((da, vec![(a.a, a.b)]));
                    }
                    continue;
                };
                if step < od * 0.9 || step > od + 0.2 {
                    continue;
                }
                let mut group = vec![(a.a, a.b)];
                for kk in 1..n {
                    if let Some((_, b)) = offs.iter().find(|(o, _)| (o - kk as f64 * step).abs() < 0.03) {
                        group.push((b.a, b.b));
                    }
                }
                if group.len() == n && best.as_ref().is_none_or(|b| da < b.0) {
                    best = Some((da, group));
                }
            }
            if let Some((_, group)) = best {
                let bottom = bottom_near(*p);
                for (a, b) in group {
                    res.runs.push(FoundRun {
                        level: level.into(),
                        kind: "conduit".into(),
                        system: "EC".into(),
                        shape: "conduit".into(),
                        size_in: [size, size],
                        from_ft: a,
                        to_ft: b,
                        bottom_ft: bottom,
                    });
                }
            }
        }
    }
}

/// Generate a seed's set and read its trays and conduit banks.
pub fn read_fixture_trays(seed: u64) -> Result<(Truth, TrayResult), String> {
    let set = crate::generate(seed)?;
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = crate::sheet_reader::recognise_fixture_set(&dxf);
    Ok((set.truth, read_trays(&dxf, &map)))
}

/// A level's code, spec name and floor elevation (feet).
pub struct LevelInfo<'a> {
    pub code: &'a str,
    pub name: &'a str,
    pub elevation_ft: f64,
}

/// The runs as building spec MEP runs and their systems (`mep.systems` / `mep.runs` for
/// `bspec.set`): drawing inches (building feet × 12). Runs whose height wasn't found are left
/// out. Riser banks matched across levels become vertical conduit runs.
pub fn spec_runs(res: &TrayResult, levels: &[LevelInfo<'_>]) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let systems = vec![
        serde_json::json!({"name": "E-CABL-TRAY", "layer": "E-CABL-TRAY", "kind": "tray", "size_in": [12, 4], "bottom_ft": 9}),
        serde_json::json!({"name": "E-FDR-COND", "layer": "E-FDR-COND", "kind": "conduit", "size_in": [2, 0], "bottom_ft": 9}),
    ];
    let level_name = |code: &str| levels.iter().find(|l| l.code == code).map(|l| l.name);
    let level_elev = |code: &str| levels.iter().find(|l| l.code == code).map(|l| l.elevation_ft);
    let mut runs: Vec<serde_json::Value> = res
        .runs
        .iter()
        .filter_map(|r| {
            let level = level_name(&r.level)?;
            let b = r.bottom_ft?;
            let (system, size) = if r.kind == "tray" { ("E-CABL-TRAY", r.size_in) } else { ("E-FDR-COND", [r.size_in[0], 0.0]) };
            Some(serde_json::json!({
                "system": system,
                "level": level,
                "points": [[r.from_ft[0] * 12.0, r.from_ft[1] * 12.0], [r.to_ft[0] * 12.0, r.to_ft[1] * 12.0]],
                "size_in": size,
                "bottoms_ft": [b],
            }))
        })
        .collect();
    // Match riser banks across levels: an "up" bank on one level paired with a "down" bank on
    // the level above (same conduit count and size, within 2 ft in plan).
    let mut used = vec![false; res.risers.len()];
    for (i, up) in res.risers.iter().enumerate() {
        if up.direction != "up" || used[i] {
            continue;
        }
        let Some(up_elev) = level_elev(&up.level) else { continue };
        let Some(up_name) = level_name(&up.level) else { continue };
        // The level above: the one with the smallest elevation > up_elev.
        let above = levels.iter().filter(|l| l.elevation_ft > up_elev).min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
        let Some(above) = above else { continue };
        // A matching "down" bank on the level above.
        let dn = res.risers.iter().enumerate().find(|(j, d)| {
            !used[*j]
                && d.direction == "down"
                && d.level == above.code
                && d.conduits == up.conduits
                && (d.size_in - up.size_in).abs() < 0.01
        });
        let Some((j, dn)) = dn else { continue };
        used[i] = true;
        used[j] = true;
        // Average the plan positions from both levels.
        let avg = |pts: &[[f64; 2]]| {
            let n = pts.len().max(1) as f64;
            [pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n]
        };
        let up_centre = avg(&up.at_ft);
        let dn_centre = avg(&dn.at_ft);
        let centre = [(up_centre[0] + dn_centre[0]) / 2.0, (up_centre[1] + dn_centre[1]) / 2.0];
        let pt = [centre[0] * 12.0, centre[1] * 12.0];
        // Each conduit in the bank: a vertical run from just above the lower level's floor to just
        // above the upper level's floor (the conduit passes through the slab).
        let bottom_ft = 0.5; // just above the slab on the lower level
        let top_ft = above.elevation_ft - up_elev + 0.5;
        for _ in 0..up.conduits {
            runs.push(serde_json::json!({
                "system": "E-FDR-COND",
                "level": up_name,
                "points": [pt],
                "size_in": [up.size_in, 0],
                "bottoms_ft": [bottom_ft],
                "top_ft": top_ft,
            }));
        }
    }
    (systems, runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_parse() {
        assert_eq!(parse_tray_tag("12x4 CT"), Some((12.0, 4.0)));
        assert_eq!(parse_tray_tag("6x4 ct"), Some((6.0, 4.0)));
        assert_eq!(parse_tray_tag("12x4"), None);
        assert_eq!(parse_bank_tag("(2) 3\"C"), Some((2, 3.0)));
        assert_eq!(parse_bank_tag("(4) 2\"C UP TO LEVEL 2 PANELS"), Some((4, 2.0)));
        assert_eq!(parse_bank_tag("(3) 1-1/4\"C"), Some((3, 1.25)));
        assert_eq!(parse_bank_tag("3/4\"C"), None);
        assert_eq!(parse_bottom("BOT @ 9'-8\""), Some(9.0 + 8.0 / 12.0));
        assert_eq!(parse_bottom("BOC @ 10'-0\""), Some(10.0));
    }
}
