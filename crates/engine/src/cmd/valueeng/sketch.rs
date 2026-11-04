//! An RFI's sketch (Q6154 (e)): the level's plan cropped to the runs the alternative changes,
//! the rooms in grey, the other routed runs thin, the changed ones marked up in red with their
//! labels, and a title with the question. SVG, written beside the drawing.

use std::fmt::Write as _;

use cadaddy_doc::Drawing;
use cadaddy_walk3d::BuildingSpec;

use super::scope::Item;

/// Rooms, runs and labels drawn at most.
const MAX_ROOMS: usize = 3_000;
const MAX_RUNS: usize = 30_000;
const MAX_LABELS: usize = 200;
/// The sketch's width in pixels; its margin round the runs, feet.
const WIDTH: f64 = 1600.0;
const MARGIN_FT: f64 = 15.0;

fn esc(t: &str) -> String {
    t.chars().take(300).collect::<String>().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The sketch of `items` on `level` into `path`.
pub(crate) fn write(
    d: &Drawing,
    sp: &BuildingSpec,
    items: &[Item],
    level: &str,
    title: &str,
    question: &str,
    path: &str,
    upf: f64,
) -> std::io::Result<()> {
    let svg = render(d, sp, items, level, title, question, upf);
    if let Some(dir) = std::path::Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, svg)
}

/// The sketch as SVG text.
pub(crate) fn render(d: &Drawing, sp: &BuildingSpec, items: &[Item], level: &str, title: &str, question: &str, upf: f64) -> String {
    let on_level = |l: &str| l.eq_ignore_ascii_case(level);
    let mine: Vec<&Item> = items.iter().filter(|i| on_level(&i.level)).collect();
    let mine_runs: std::collections::HashSet<usize> = mine.iter().flat_map(|i| i.runs.iter().copied()).collect();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for r in mine_runs.iter().filter_map(|i| sp.mep.runs.get(*i)) {
        for p in r.points.iter().filter(|p| p.iter().all(|v| v.is_finite())) {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
    }
    if !(lo[0].is_finite() && hi[0].is_finite()) {
        lo = [0.0, 0.0];
        hi = [upf * 10.0, upf * 10.0];
    }
    let m = MARGIN_FT * upf;
    let (x0, y0, x1, y1) = (lo[0] - m, lo[1] - m, hi[0] + m, hi[1] + m);
    let w = (x1 - x0).max(1.0);
    let h = (y1 - y0).max(1.0);
    let k = WIDTH / w;
    let head = 110.0;
    let height = (h * k).clamp(200.0, 20_000.0) + head;
    let px = |x: f64| (x - x0) * k;
    let py = |y: f64| head + (y1 - y) * k;
    let inside = |p: &[f64; 2]| p[0] >= x0 && p[0] <= x1 && p[1] >= y0 && p[1] <= y1;
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH:.0}" height="{height:.0}" viewBox="0 0 {WIDTH:.0} {height:.0}" font-family="sans-serif">"#
    );
    let _ = writeln!(s, r##"<rect x="0" y="0" width="{WIDTH:.0}" height="{height:.0}" fill="#ffffff"/>"##);
    // The rooms.
    for r in super::super::bspec::room_elements(d).into_iter().filter(|r| on_level(&r.level) && r.outline.len() >= 3).take(MAX_ROOMS) {
        if !r.outline.iter().any(|p| inside(&[p.x, p.y])) {
            continue;
        }
        let pts: Vec<String> = r.outline.iter().take(400).map(|p| format!("{:.1},{:.1}", px(p.x), py(p.y))).collect();
        let _ = writeln!(s, r##"<polygon points="{}" fill="#f4f4f4" stroke="#9a9a9a" stroke-width="1"/>"##, pts.join(" "));
        let n = r.outline.len() as f64;
        let c = r.outline.iter().fold((0.0, 0.0), |a, p| (a.0 + p.x, a.1 + p.y));
        let label = r.number.clone().filter(|x| !x.trim().is_empty()).unwrap_or_else(|| r.name.clone());
        let _ = writeln!(
            s,
            r##"<text x="{:.1}" y="{:.1}" font-size="10" fill="#777" text-anchor="middle">{}</text>"##,
            px(c.0 / n),
            py(c.1 / n),
            esc(&label)
        );
    }
    // The other routed runs, thin; the changed ones marked up.
    let path_of = |r: &cadaddy_walk3d::MepRun| -> String {
        r.points.iter().filter(|p| p.iter().all(|v| v.is_finite())).map(|p| format!("{:.1},{:.1}", px(p[0]), py(p[1]))).collect::<Vec<_>>().join(" ")
    };
    for (i, r) in sp.mep.runs.iter().enumerate().take(MAX_RUNS) {
        if mine_runs.contains(&i) || !on_level(&r.level) || r.points.len() < 2 || !r.points.iter().any(inside) {
            continue;
        }
        if r.system != super::super::conduitroute::BRANCH_SYSTEM && r.system != super::super::gearbuild::FEEDER_SYSTEM {
            continue;
        }
        let _ = writeln!(s, r##"<polyline points="{}" fill="none" stroke="#b0b0b0" stroke-width="1"/>"##, path_of(r));
    }
    let mut labels = 0usize;
    for it in &mine {
        for r in it.runs.iter().filter_map(|i| sp.mep.runs.get(*i)) {
            if r.points.len() >= 2 {
                let _ = writeln!(s, r##"<polyline points="{}" fill="none" stroke="#d01010" stroke-width="3"/>"##, path_of(r));
            } else if let Some(p) = r.points.first() {
                let _ = writeln!(s, r##"<circle cx="{:.1}" cy="{:.1}" r="5" fill="none" stroke="#d01010" stroke-width="2"/>"##, px(p[0]), py(p[1]));
            }
        }
        if labels < MAX_LABELS
            && let Some(p) = it.runs.iter().filter_map(|i| sp.mep.runs.get(*i)).find_map(|r| r.points.first())
        {
            labels += 1;
            let _ =
                writeln!(s, r##"<text x="{:.1}" y="{:.1}" font-size="11" fill="#d01010">{}</text>"##, px(p[0]) + 4.0, py(p[1]) - 4.0, esc(&it.label));
        }
    }
    // The title, the question, the legend and a scale.
    let _ = writeln!(s, r##"<rect x="0" y="0" width="{WIDTH:.0}" height="{head:.0}" fill="#ffffff" stroke="#333" stroke-width="1"/>"##);
    let _ = writeln!(s, r##"<text x="12" y="26" font-size="18" font-weight="bold" fill="#111">{}</text>"##, esc(title));
    let _ = writeln!(s, r##"<text x="12" y="52" font-size="13" fill="#111">{}</text>"##, esc(question));
    let _ = writeln!(
        s,
        r##"<text x="12" y="76" font-size="12" fill="#333">{} — red: the runs the alternative changes ({} on this level); grey: other routed runs, kept as designed</text>"##,
        esc(level),
        mine.len()
    );
    let bar_ft = [10.0, 20.0, 50.0, 100.0].into_iter().find(|f| f * upf * k >= 80.0).unwrap_or(100.0);
    let bar = bar_ft * upf * k;
    let _ = writeln!(
        s,
        r##"<line x1="12" y1="96" x2="{:.1}" y2="96" stroke="#111" stroke-width="2"/><text x="{:.1}" y="100" font-size="11" fill="#111">{bar_ft:.0} ft</text>"##,
        12.0 + bar,
        18.0 + bar
    );
    s.push_str("</svg>\n");
    s
}
