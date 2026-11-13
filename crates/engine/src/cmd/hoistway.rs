//! Elevator hoistways from their documents (Q6608): the shaft, its pit and the overrun.
//!
//! An elevator's hoistway is set by the elevator submittal (its shop drawings), or the enlarged
//! elevator plan: the clear size inside the shaft walls, the pit depth below the lowest landing
//! and the overhead from the top landing to the hoistway's ceiling. A plan traced on its own
//! gets the shaft wrong: the car outline, the counterweight, an enlarged-plan callout's boundary
//! or a rating line drawn inside the shaft read as its walls, and nothing stands above the roof
//! for the overrun the roof equipment there sits on.
//!
//! - `elevator.read {files?, text?}` reads the submittal's text (rules as data in
//!   `hoistway.json`): `HOISTWAY 7'-4" x 5'-11"`, `7'-4" HOISTWAY WIDTH`, `PIT 4'-8"`,
//!   `-4'-8" PIT EL`, `OVERHEAD 13'-10"`, and stores it in the building spec's `elevator_data`.
//!   A shop drawing repeats its sizes and its stroked lettering may be misread: every value read
//!   is kept, the most read first.
//! - `model.build` ([`build`]) sets each traced hoistway (the spec's `circulation`, else the
//!   arch reader's `elevators`) to that clear size where the plan's walls agree with it (the
//!   faces the shaft's walls show), else centred on the trace; a size no document gives is the
//!   pair of wall faces round the trace (at most `max_clear_ft` apart), else the trace. Then:
//!   lines inside the shaft are never walls (a wall built there is removed, one crossing it cut
//!   back to the shaft's face); a side of the shaft with no wall drawn gets one of the rated
//!   type the plan gives its other sides (else the rules' shaft wall); the pit is built below
//!   the lowest landing to its depth, and the overrun above the roof to the overhead, a roofed
//!   shaft the roof equipment over it stands on (its design base kept beside the corrected one).
//!   A value no document gives is the rules' trade practice, said as such on the element.

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Common, Drawing, EntityKind, Handle, Mesh, RawTag, Space};
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_walk3d::{BuildingSpec, CirculationKind, ElevatorData, Made, Part};
use serde::Deserialize;
use serde_json::{Value, json};

use super::wallgen;
use super::*;
use crate::{Result, Session};

const CMD: &str = "elevator.read";
/// Files read in one call, pages of each, lines of text, values kept per size.
const MAX_FILES: usize = 20;
const MAX_PAGES: u32 = 120;
const MAX_LINES: usize = 200_000;
const MAX_VALUES: usize = 6;
/// Hoistways built, wall elements looked at, notes kept.
const MAX_HOISTWAYS: usize = 200;
const MAX_ELEMENTS: usize = 2_000_000;
const MAX_NOTES: usize = 40;
/// The layer the pits and overruns go on.
pub(crate) const LAYER: &str = "3D-Elevators";
/// The parts they are made as.
pub(crate) const PIT: &str = "hoistway-pit";
pub(crate) const OVERRUN: &str = "elevator-overrun";

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("elevator.read", "Read Elevator Hoistway", run_read).params(
        "{files?: path or [path] (elevator submittal PDFs or text files, or folders of submittals: the files named for an elevator or hoistway), text?: string (a submittal's text), apply?: bool (default true: store it in the building spec's elevator_data, which model.build builds the shaft, pit and overrun from)} → {elevator_data: {width_ft: [ft], depth_ft: [ft], pit_ft, overhead_ft, sources}, files: [file], unread: [{file, error}], applied: bool}",
    )]
}

/// The rules as data (`hoistway.json` `hoistway`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub file_words: Vec<String>,
    pub hoistway_words: Vec<String>,
    pub width_words: Vec<String>,
    pub depth_words: Vec<String>,
    pub pit_words: Vec<String>,
    pub overhead_words: Vec<String>,
    pub not_shaft_words: Vec<String>,
    pub clear_range_ft: [f64; 2],
    pub pit_range_ft: [f64; 2],
    pub overhead_range_ft: [f64; 2],
    pub max_clear_ft: f64,
    pub tolerance_in: f64,
    pub default_pit_ft: f64,
    pub default_pit_basis: String,
    pub default_overhead_ft: f64,
    pub default_overhead_basis: String,
    pub shaft_wall_in: f64,
    pub shaft_wall_basis: String,
    pub cap_ft: f64,
    pub pit_slab_ft: f64,
    pub rule: String,
}

#[derive(Default, Deserialize)]
struct File {
    #[serde(default)]
    hoistway: Rules,
}

pub(crate) fn rules() -> Rules {
    serde_json::from_str::<File>(include_str!("hoistway.json")).map(|f| f.hoistway).unwrap_or_default()
}

fn in_range(v: f64, r: [f64; 2]) -> bool {
    v.is_finite() && v >= r[0] && v <= r[1]
}

// ─── Reading ─────────────────────────────────────────────────────────────────

/// What the text says, each value with where it was read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Reading {
    pub widths: Vec<(f64, String)>,
    pub depths: Vec<(f64, String)>,
    pub pits: Vec<(f64, String)>,
    pub overheads: Vec<(f64, String)>,
}

/// A length written on a page: where it starts and ends (char indices) and its feet (a lead
/// minus sign, an elevation below the floor as in `-4'-8" PIT EL`, is left off: a depth).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Len {
    start: usize,
    end: usize,
    ft: f64,
}

/// Every length in `t` (upper case, straight quotes): `7'-4"`, `7' - 4 1/2"`, `12'`, `88"`.
fn lengths(t: &[char]) -> Vec<Len> {
    let mut out = Vec::new();
    let digits = |i: &mut usize| -> Option<f64> {
        let b = *i;
        while t.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        if *i == b || *i - b > 6 {
            return None;
        }
        t.get(b..*i)?.iter().collect::<String>().parse::<f64>().ok()
    };
    let skip = |i: &mut usize| {
        while t.get(*i) == Some(&' ') {
            *i += 1;
        }
    };
    let mut i = 0;
    while i < t.len() && out.len() < 1000 {
        let starts = t.get(i).is_some_and(char::is_ascii_digit)
            && (i == 0 || t.get(i - 1).is_some_and(|c| !c.is_ascii_alphanumeric() && *c != '.' && *c != '/'));
        if !starts {
            i += 1;
            continue;
        }
        let start = i;
        let Some(a) = digits(&mut i) else {
            i += 1;
            continue;
        };
        let mut j = i;
        skip(&mut j);
        match t.get(j) {
            Some('\'') => {
                j += 1;
                let mut k = j;
                skip(&mut k);
                if t.get(k) == Some(&'-') {
                    k += 1;
                    skip(&mut k);
                }
                let mut inches = 0.0;
                let mut end = j;
                if t.get(k).is_some_and(char::is_ascii_digit) {
                    let mut m = k;
                    if let Some(n) = digits(&mut m).filter(|n| *n < 12.0) {
                        inches = n;
                        end = m;
                        // A fraction after the inches (` 1/2`).
                        let mut f = m;
                        skip(&mut f);
                        let mut g = f;
                        if let (Some(num), Some('/')) = (digits(&mut g), t.get(g)) {
                            let mut h = g + 1;
                            if let Some(den) = digits(&mut h).filter(|d| *d > 0.0)
                                && num < den
                            {
                                inches += num / den;
                                end = h;
                            }
                        }
                        let mut q = end;
                        skip(&mut q);
                        if t.get(q) == Some(&'"') {
                            end = q + 1;
                        }
                    }
                }
                let ft = a + inches / 12.0;
                out.push(Len { start, end, ft });
                i = end.max(start + 1);
            }
            Some('"') => {
                out.push(Len { start, end: j + 1, ft: a / 12.0 });
                i = j + 1;
            }
            _ => i = j.max(start + 1),
        }
    }
    out
}

/// Where a word or phrase stands whole in `t` (upper case): its char ranges.
fn find_word(t: &[char], word: &str) -> Vec<(usize, usize)> {
    let w: Vec<char> = word.trim().to_ascii_uppercase().chars().collect();
    let mut out = Vec::new();
    if w.is_empty() || w.len() > t.len() {
        return out;
    }
    let edge = |c: Option<&char>| c.is_none_or(|c| !c.is_ascii_alphanumeric());
    for s in 0..=t.len() - w.len() {
        if t.get(s..s + w.len()) == Some(&w[..]) && (s == 0 || edge(t.get(s - 1))) && edge(t.get(s + w.len())) {
            out.push((s, s + w.len()));
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fam {
    Hoistway,
    Width,
    Depth,
    Pit,
    Overhead,
}

/// Read one segment of a line (between column breaks).
fn read_segment(seg: &str, at: &str, r: &Rules, out: &mut Reading) {
    let up: Vec<char> =
        seg.to_ascii_uppercase().replace(['\u{2019}', '\u{2032}', '`'], "'").replace(['\u{201D}', '\u{2033}', '\u{201C}'], "\"").chars().collect();
    let lens = lengths(&up);
    if lens.is_empty() {
        return;
    }
    // The keywords, longest first, each place taken once (`PIT DEPTH` is the pit's).
    let mut words: Vec<(&str, Fam)> = Vec::new();
    for (ws, f) in [
        (&r.hoistway_words, Fam::Hoistway),
        (&r.width_words, Fam::Width),
        (&r.depth_words, Fam::Depth),
        (&r.pit_words, Fam::Pit),
        (&r.overhead_words, Fam::Overhead),
    ] {
        words.extend(ws.iter().map(|w| (w.as_str(), f)));
    }
    words.sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
    let mut taken = vec![false; up.len()];
    let mut keys: Vec<(usize, usize, Fam)> = Vec::new();
    for (w, f) in words {
        for (a, b) in find_word(&up, w) {
            if taken.get(a..b).is_some_and(|x| x.iter().any(|t| *t)) {
                continue;
            }
            for t in taken.iter_mut().take(b).skip(a) {
                *t = true;
            }
            keys.push((a, b, f));
        }
    }
    keys.sort_by_key(|k| k.0);
    let shaft = keys.iter().any(|k| k.2 == Fam::Hoistway) && !r.not_shaft_words.iter().any(|w| !find_word(&up, w).is_empty());
    // The lengths that belong to the keyword at `k`: after it up to the next keyword that stops
    // it, else before it back to the one before.
    let stops = |f: Fam, g: Fam| f != g && !(matches!(f, Fam::Width | Fam::Depth) && g == Fam::Hoistway);
    let near = |k: &(usize, usize, Fam)| -> Vec<Len> {
        let next = keys.iter().filter(|o| o.0 >= k.1 && stops(k.2, o.2)).map(|o| o.0).min().unwrap_or(up.len());
        let prev = keys.iter().filter(|o| o.1 <= k.0 && stops(k.2, o.2)).map(|o| o.1).max().unwrap_or(0);
        let after: Vec<Len> = lens.iter().copied().filter(|l| l.start >= k.1 && l.end <= next).collect();
        if !after.is_empty() {
            return after;
        }
        lens.iter().copied().filter(|l| l.end <= k.0 && l.start >= prev).collect()
    };
    let push = |list: &mut Vec<(f64, String)>, v: f64| {
        if list.len() < 64 {
            list.push((v, at.to_string()));
        }
    };
    for k in &keys {
        let mine = near(k);
        match k.2 {
            Fam::Hoistway if shaft => {
                // A size: two lengths joined by an X.
                for w in mine.windows(2) {
                    let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
                    let between: String = up.get(a.end..b.start).unwrap_or_default().iter().collect();
                    let between = between.trim();
                    if matches!(between, "X" | "\u{D7}" | "*") && in_range(a.ft, r.clear_range_ft) && in_range(b.ft, r.clear_range_ft) {
                        push(&mut out.widths, a.ft);
                        push(&mut out.depths, b.ft);
                        break;
                    }
                }
            }
            Fam::Width | Fam::Depth if shaft && mine.len() == 1 => {
                if let Some(l) = mine.first().filter(|l| in_range(l.ft, r.clear_range_ft)) {
                    push(if k.2 == Fam::Width { &mut out.widths } else { &mut out.depths }, l.ft);
                }
            }
            Fam::Pit => {
                if let Some(l) = mine.first().filter(|l| in_range(l.ft, r.pit_range_ft)) {
                    push(&mut out.pits, l.ft);
                }
            }
            Fam::Overhead => {
                if let Some(l) = mine.first().filter(|l| in_range(l.ft, r.overhead_range_ft)) {
                    push(&mut out.overheads, l.ft);
                }
            }
            _ => {}
        }
    }
}

/// Read a document's text (`--- page N` lines mark its pages), `doc` naming it.
pub(crate) fn read_text(text: &str, doc: &str, r: &Rules, out: &mut Reading) {
    let mut page = String::new();
    for line in text.lines().take(MAX_LINES) {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("--- page ") {
            page = rest.split_whitespace().next().unwrap_or("").to_string();
            continue;
        }
        let at = if page.is_empty() { doc.to_string() } else { format!("{doc} p.{page}") };
        for seg in l.split("   ").filter(|s| !s.trim().is_empty()) {
            read_segment(seg, &at, r, out);
        }
    }
}

/// The values of one size, the most read first (the first read first among equals).
fn ranked(list: &[(f64, String)]) -> Vec<(f64, String, usize)> {
    let mut out: Vec<(f64, String, usize)> = Vec::new();
    for (v, at) in list {
        match out.iter_mut().find(|o| (o.0 - v).abs() < 0.01) {
            Some(o) => o.2 += 1,
            None => out.push((*v, at.clone(), 1)),
        }
    }
    out.sort_by_key(|o| std::cmp::Reverse(o.2));
    out.truncate(MAX_VALUES);
    out
}

impl Reading {
    /// What was read as the spec keeps it; none when nothing was.
    pub(crate) fn data(&self) -> Option<ElevatorData> {
        let (w, d, p, o) = (ranked(&self.widths), ranked(&self.depths), ranked(&self.pits), ranked(&self.overheads));
        if w.is_empty() && d.is_empty() && p.is_empty() && o.is_empty() {
            return None;
        }
        let mut data = ElevatorData {
            width_ft: w.iter().map(|x| x.0).collect(),
            depth_ft: d.iter().map(|x| x.0).collect(),
            pit_ft: p.first().map(|x| x.0),
            overhead_ft: o.first().map(|x| x.0),
            ..ElevatorData::default()
        };
        for (key, list) in [("width", &w), ("depth", &d), ("pit", &p), ("overhead", &o)] {
            if let Some(x) = list.first() {
                data.sources.insert(key.into(), x.1.chars().take(200).collect());
            }
        }
        Some(data)
    }
}

/// The files of `paths` that are an elevator's: named for one (`file_words`), or given by name.
fn elevator_files(paths: &[String], r: &Rules) -> Vec<String> {
    let named: Vec<String> = paths.iter().filter(|p| std::path::Path::new(p.trim()).is_file()).map(|p| p.trim().to_string()).collect();
    super::submittals::pdf_files(paths)
        .into_iter()
        .filter(|f| {
            let name = std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
            let words: Vec<char> = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect();
            named.contains(f) || r.file_words.iter().any(|w| !find_word(&words, w).is_empty())
        })
        .take(MAX_FILES)
        .collect()
}

/// Read the elevator submittals at `paths`: what they say, the files read and those that couldn't be.
pub(crate) fn read_files(paths: &[String]) -> (Reading, Vec<String>, Vec<Value>) {
    let r = rules();
    let mut reading = Reading::default();
    let (mut read, mut unread) = (Vec::new(), Vec::new());
    for f in elevator_files(paths, &r) {
        let name = std::path::Path::new(&f).file_name().map_or_else(|| f.clone(), |n| n.to_string_lossy().to_string());
        let text = if f.to_ascii_lowercase().ends_with(".txt") {
            crate::readfile::read_string(&f).map_err(|e| e.to_string())
        } else {
            crate::readfile::read(&f)
                .map_err(|e| e.to_string())
                .and_then(|b| super::spec_import::pdf_text_pages(&b, &f, MAX_PAGES).map_err(|e| e.to_string()))
        };
        match text {
            Ok(t) => {
                read_text(&t, &name, &r, &mut reading);
                read.push(name);
            }
            Err(e) => unread.push(json!({"file": name, "error": e})),
        }
    }
    (reading, read, unread)
}

fn run_read(s: &mut Session, p: &Value) -> Result<Value> {
    let r = rules();
    let paths: Vec<String> = match p.get("files") {
        Some(Value::String(x)) => vec![x.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).take(200).collect(),
        None | Some(Value::Null) => Vec::new(),
        Some(_) => return Err(bad(CMD, "`files` must be a path or a list of paths")),
    };
    let text = match p.get("text") {
        Some(Value::String(t)) => Some(t.as_str()),
        None | Some(Value::Null) => None,
        Some(_) => return Err(bad(CMD, "`text` must be a string")),
    };
    if paths.is_empty() && text.is_none() {
        return Err(bad(CMD, "give `files` (an elevator submittal, or a folder of submittals) or `text`"));
    }
    let apply = p.get("apply").and_then(Value::as_bool).unwrap_or(true);
    let (mut reading, files, unread) = read_files(&paths);
    if let Some(t) = text {
        read_text(t, "text", &r, &mut reading);
    }
    let data = reading.data();
    let mut applied = false;
    if apply
        && let Some(data) = &data
        && let Ok(Some(mut sp)) = super::bspec::stored(s.doc()?)
    {
        sp.elevator_data = Some(data.clone());
        s.doc_mut()?.building_spec = Some(sp.to_json()).into();
        applied = true;
    }
    let mut out = json!({"elevator_data": data, "files": files, "unread": unread, "applied": applied});
    if data.is_none() {
        out["note"] = json!("no hoistway size, pit depth or overhead was read");
    } else if apply && !applied {
        out["note"] = json!("the drawing has no building spec stored, so nothing was applied (bspec.set first)");
    }
    Ok(out)
}

// ─── Building ────────────────────────────────────────────────────────────────

/// A box of a built wall element of a level: its element, plan bounds and wall type.
#[derive(Clone, Debug)]
struct Wall {
    handle: Handle,
    b: Bounds2,
    wall_type: Option<String>,
}

impl Wall {
    /// Its axis when it is a thin box along x (0) or y (1), and its thickness.
    fn axis(&self, max_t: f64) -> Option<(usize, f64)> {
        let (w, h) = (self.b.width(), self.b.height());
        if w <= h && w <= max_t && h >= 1.5 * w {
            Some((1, w))
        } else if h < w && h <= max_t && w >= 1.5 * h {
            Some((0, h))
        } else {
            None
        }
    }
}

fn lo(b: &Bounds2, axis: usize) -> f64 {
    if axis == 0 { b.min.x } else { b.min.y }
}

fn hi(b: &Bounds2, axis: usize) -> f64 {
    if axis == 0 { b.max.x } else { b.max.y }
}

fn overlap(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.1.min(b.1) - a.0.max(b.0)).max(0.0)
}

/// The faces along `axis` (a wall running across it): `(position, support, wall index)` of
/// the walls lying below a face (`low` true: the face is their high side) or above it, beside
/// the trace's extent across (`span`); support is how far the wall runs beside it and `reach`
/// past it (a shaft's wall runs on past the car's).
fn faces(walls: &[Wall], axis: usize, low: bool, span: (f64, f64), reach: f64, max_t: f64) -> Vec<(f64, f64, usize)> {
    let other = 1 - axis;
    walls
        .iter()
        .enumerate()
        .filter_map(|(i, w)| {
            let (run, _) = w.axis(max_t)?;
            (run == other).then_some(())?;
            let extent = (lo(&w.b, other), hi(&w.b, other));
            let along = overlap(extent, (span.0 - reach, span.1 + reach));
            (overlap(extent, span) > 0.3 * (span.1 - span.0)).then(|| (if low { hi(&w.b, axis) } else { lo(&w.b, axis) }, along, i))
        })
        .collect()
}

/// The shaft's extent along one axis: the size given (`size`, drawing units) placed where the
/// walls' faces agree, else centred on the trace `(r0, r1)`; with no size, the pair of faces
/// round the trace (at most `max` apart), else the trace. Its extent, its support and the wall
/// that supports it best.
fn fit_axis(
    lows: &[(f64, f64, usize)],
    highs: &[(f64, f64, usize)],
    r: (f64, f64),
    size: Option<f64>,
    max: f64,
    tol: f64,
) -> ((f64, f64), f64, Option<usize>) {
    let at = |list: &[(f64, f64, usize)], p: f64| -> (f64, Option<usize>) {
        let mut best: (f64, Option<usize>) = (0.0, None);
        let mut sum = 0.0;
        for f in list.iter().filter(|f| (f.0 - p).abs() <= tol) {
            sum += f.1;
            if best.1.is_none() || f.1 > best.0 {
                best = (f.1, Some(f.2));
            }
        }
        (sum, best.1)
    };
    let score = |a: f64, b: f64| -> (f64, Option<usize>) {
        let (sa, wa) = at(lows, a);
        let (sb, wb) = at(highs, b);
        let both = if sa > 0.0 && sb > 0.0 { sa.min(sb) } else { 0.0 };
        (sa + sb + both, wa.or(wb))
    };
    let mut best: Option<((f64, f64), f64, Option<usize>)> = None;
    let mut consider = |a: f64, b: f64| {
        if !(a.is_finite() && b.is_finite()) || a > r.0 + tol || b < r.1 - tol || b - a > max + tol {
            return;
        }
        let (sc, w) = score(a, b);
        let better = match &best {
            None => sc > 0.0,
            Some(x) => sc > x.1 + 1e-9 || ((sc - x.1).abs() <= 1e-9 && (b - a) > (x.0.1 - x.0.0)),
        };
        if better {
            best = Some(((a, b), sc, w));
        }
    };
    match size {
        Some(s) => {
            for f in lows {
                consider(f.0, f.0 + s);
            }
            for f in highs {
                consider(f.0 - s, f.0);
            }
            best.unwrap_or_else(|| {
                let c = (r.0 + r.1) / 2.0;
                ((c - s / 2.0, c + s / 2.0), 0.0, None)
            })
        }
        None => {
            for a in lows {
                for b in highs {
                    if at(lows, a.0).0 > 0.0 && at(highs, b.0).0 > 0.0 {
                        consider(a.0, b.0);
                    }
                }
            }
            best.filter(|b| b.1 > 0.0).unwrap_or((r, 0.0, None))
        }
    }
}

/// A wall mesh's boxes (the build lays a wall as boxes of 8 corners and 6 faces: its pieces
/// beside its openings, a closed shaft's four sides): each box's plan bounds, height, and
/// whether it stands square to the axes. A mesh of another make is one piece, not square.
fn pieces(m: &Mesh) -> Vec<(Bounds2, (f64, f64), bool, usize)> {
    let n = m.vertices.len() / 8;
    let boxes = m.vertices.len().is_multiple_of(8) && m.faces.len() == 6 * n && n > 0;
    let one = |vs: &[cadaddy_geom::Vec3]| {
        let b = Bounds2::from_points(vs.iter().map(|v| Vec2::new(v.x, v.y)));
        let z = vs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |z, v| (z.0.min(v.z), z.1.max(v.z)));
        let eps = 1e-6 * (1.0 + b.width().abs() + b.height().abs());
        let square = vs
            .iter()
            .all(|v| ((v.x - b.min.x).abs() < eps || (v.x - b.max.x).abs() < eps) && ((v.y - b.min.y).abs() < eps || (v.y - b.max.y).abs() < eps));
        (b, z, square)
    };
    if !boxes {
        let (b, z, _) = one(&m.vertices);
        return vec![(b, z, false, usize::MAX)];
    }
    (0..n).filter_map(|k| m.vertices.get(8 * k..8 * k + 8).map(one).map(|(b, z, sq)| (b, z, sq, k))).collect()
}

/// The level's built walls, a [`Wall`] per box.
fn level_walls(d: &Drawing, level: &str) -> Vec<Wall> {
    let mut out = Vec::new();
    for e in d
        .model
        .iter()
        .take(MAX_ELEMENTS)
        .filter(|e| wallgen::made_as(e).as_deref() == Some("wall") && super::bspec::made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(level)))
    {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let wall_type = super::bimlook::made_tag(e, "type").map(str::to_string);
        for (b, z, _, _) in pieces(m) {
            if !b.is_empty() && z.0.is_finite() && z.1 > z.0 && out.len() < MAX_ELEMENTS {
                out.push(Wall { handle: e.handle, b, wall_type: wall_type.clone() });
            }
        }
    }
    out
}

/// What [`cut_out`] did to a wall element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trim {
    Kept,
    Cut,
    Removed,
}

/// Nothing of wall element `h` stands `inside` the shaft: a box of it there is dropped, one
/// crossing it cut back to its face; the element goes when nothing is left.
fn cut_out(d: &mut Drawing, h: Handle, inside: &Bounds2, tol: f64, max_t: f64) -> Trim {
    let Some(EntityKind::Mesh(m)) = d.model.get(h).map(|e| e.kind.clone()) else { return Trim::Kept };
    let area = |b: &Bounds2| (b.width() * b.height()).max(1e-9);
    let hit = |b: &Bounds2| {
        let ov = b.intersection(inside);
        if ov.is_empty() || ov.width() <= 0.0 || ov.height() <= 0.0 { 0.0 } else { ov.width() * ov.height() / area(b) }
    };
    let ps = pieces(&m);
    if ps.iter().all(|p| hit(&p.0) <= 0.0) {
        return Trim::Kept;
    }
    let mut out = wallgen::empty_mesh();
    let mut changed = false;
    for (b, z, square, k) in ps {
        let share = hit(&b);
        let copy = |out: &mut Mesh| {
            let (Some(vs), Some(fs)) = (m.vertices.get(8 * k..8 * k + 8), m.faces.get(6 * k..6 * k + 6)) else { return };
            let base = u32::try_from(out.vertices.len()).unwrap_or(u32::MAX);
            let from = u32::try_from(8 * k).unwrap_or(u32::MAX);
            out.vertices.extend_from_slice(vs);
            for f in fs {
                out.faces
                    .push(cadaddy_doc::MeshFace { v: f.v.iter().map(|i| i.saturating_sub(from).saturating_add(base)).collect(), hidden: f.hidden });
            }
        };
        if share <= 0.0 && k != usize::MAX {
            copy(&mut out);
            continue;
        }
        changed = true;
        let w = Wall { handle: h, b, wall_type: None };
        match w.axis(max_t).filter(|_| square && k != usize::MAX) {
            Some((run, th)) => {
                let (a0, a1) = (lo(&b, run), hi(&b, run));
                let (i0, i1) = (lo(inside, run), hi(inside, run));
                let mid = if run == 0 { (b.min.y + b.max.y) / 2.0 } else { (b.min.x + b.max.x) / 2.0 };
                let pt = |s: f64| if run == 0 { Vec2::new(s, mid) } else { Vec2::new(mid, s) };
                for (p0, p1) in [(a0, i0.min(a1)), (i1.max(a0), a1)] {
                    if p1 - p0 > tol {
                        wallgen::add_box(&mut out, pt(p0), pt(p1), th, z.0, z.1);
                    }
                }
            }
            None if share < 0.5 => {
                if k == usize::MAX {
                    return Trim::Kept;
                }
                copy(&mut out);
            }
            None => {}
        }
    }
    if !changed {
        return Trim::Kept;
    }
    if out.faces.is_empty() {
        d.model.remove(h);
        return Trim::Removed;
    }
    if d.modify_entity(h, |e| e.kind = EntityKind::Mesh(out)).is_ok() { Trim::Cut } else { Trim::Kept }
}

/// The element tags of a part this module makes (`part`, its level, its name), with how it is
/// known and why.
fn tags(part: &str, level: &str, name: &str, made: Made, basis: &str) -> Vec<RawTag> {
    let t = |v: String| RawTag { code: 1000, value: v };
    let mut x = vec![
        RawTag { code: 1001, value: wallgen::APP.into() },
        t(part.into()),
        t(level.chars().take(255).collect()),
        t(Handle(0).hex()),
        t(name.chars().take(240).collect()),
    ];
    x.extend(super::provenance::tags(made, basis));
    x
}

fn with_props(xdata: Vec<RawTag>, values: &[(&str, f64, &str, SourceKind)]) -> Vec<RawTag> {
    let mut ps = Props::default();
    for (k, v, src, kind) in values {
        if v.is_finite() {
            let mut p = Prop::new((v * 100.0).round() / 100.0, Source::from_doc(*kind, src));
            p.unit = Some("ft".into());
            let _ = ps.set(k, p);
        }
    }
    props::write(&xdata, &ps)
}

fn ensure_layer(d: &mut Drawing, layer: &str, color: u8) {
    if d.layer(layer).is_none() {
        d.layers.push(cadaddy_doc::Layer { name: layer.into(), color: cadaddy_doc::color::Color::Index(color), ..cadaddy_doc::Layer::default() });
    }
}

/// The four walls round `rect` (inner faces), `t` thick, from `z0` to `z1`.
fn ring(rect: &Bounds2, t: f64, z0: f64, z1: f64) -> Mesh {
    let mut m = wallgen::empty_mesh();
    let (a, b) = (rect.min, rect.max);
    let h = t / 2.0;
    wallgen::add_box(&mut m, Vec2::new(a.x - t, a.y - h), Vec2::new(b.x + t, a.y - h), t, z0, z1);
    wallgen::add_box(&mut m, Vec2::new(a.x - t, b.y + h), Vec2::new(b.x + t, b.y + h), t, z0, z1);
    wallgen::add_box(&mut m, Vec2::new(a.x - h, a.y), Vec2::new(a.x - h, b.y), t, z0, z1);
    wallgen::add_box(&mut m, Vec2::new(b.x + h, a.y), Vec2::new(b.x + h, b.y), t, z0, z1);
    m
}

/// A flat slab over `rect` from `z0` to `z1`.
fn slab(m: &mut Mesh, rect: &Bounds2, z0: f64, z1: f64) {
    let y = (rect.min.y + rect.max.y) / 2.0;
    wallgen::add_box(m, Vec2::new(rect.min.x, y), Vec2::new(rect.max.x, y), rect.height(), z0, z1);
}

/// A traced hoistway with the walls round it: the faces across x and across y (below, above).
struct Seen {
    level: cadaddy_walk3d::Level,
    name: String,
    tr: Bounds2,
    walls: Vec<Wall>,
    fx: (Vec<(f64, f64, usize)>, Vec<(f64, f64, usize)>),
    fy: (Vec<(f64, f64, usize)>, Vec<(f64, f64, usize)>),
}

/// One hoistway on one level as built.
#[derive(Clone, Debug)]
struct Shaft {
    level: String,
    name: String,
    rect: Bounds2,
    t: f64,
    /// Which stack (one elevator's shafts on the storeys it serves) it is in.
    stack: usize,
}

/// Build the hoistways (see the module doc); the report.
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec) -> Value {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let tol = r.tolerance_in.max(0.5) * upf / 12.0;
    let max_t = 2.0 * upf;
    let offsets = super::bspec::stack_offsets(d, sp);
    let off = |level: &str| offsets.iter().find(|(n, _)| n.eq_ignore_ascii_case(level)).map_or(Vec2::ZERO, |(_, o)| *o);
    // The traced hoistways (each level's), else the arch reader's (on its lowest level's plan).
    let mut traced: Vec<(String, String, Bounds2)> = sp
        .circulation
        .iter()
        .filter(|c| c.kind == CirculationKind::Hoistway)
        .filter_map(|c| {
            let b = Bounds2::from_points(c.rects.iter().flat_map(|[a, b]| [Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1])]));
            (!b.is_empty() && b.width() > 0.0 && b.height() > 0.0).then(|| (c.level.clone(), c.name.clone(), b))
        })
        .take(MAX_HOISTWAYS)
        .collect();
    if traced.is_empty() {
        for e in sp.elevators.iter().take(MAX_HOISTWAYS) {
            let Some(first) = e.levels.first() else { continue };
            let b = Bounds2::new(Vec2::new(e.min[0], e.min[1]), Vec2::new(e.max[0], e.max[1]));
            for l in e.levels.iter().take(200) {
                let m = off(first) - off(l);
                traced.push((l.clone(), e.name.clone(), Bounds2::new(b.min + m, b.max + m)));
            }
        }
    }
    traced.retain(|(l, _, b)| sp.level(l).is_some() && !wallgen::roof_level(&sp.levels, l) && b.min.is_finite() && b.max.is_finite());
    if traced.is_empty() {
        return json!({"hoistways": 0});
    }
    let data = sp.elevator_data.clone().unwrap_or_default();
    let size_src = |key: &str| data.sources.get(key).cloned().unwrap_or_else(|| "the elevator submittal".into());
    let widths: Vec<Option<f64>> = if data.width_ft.is_empty() { vec![None] } else { data.width_ft.iter().map(|w| Some(w * upf)).collect() };
    let depths: Vec<Option<f64>> = if data.depth_ft.is_empty() { vec![None] } else { data.depth_ft.iter().map(|w| Some(w * upf)).collect() };
    ensure_layer(d, LAYER, 8);
    let mut notes: Vec<String> = Vec::new();
    let mut shafts: Vec<Shaft> = Vec::new();
    let mut report: Vec<Value> = Vec::new();
    let (mut removed, mut clipped, mut added) = (0usize, 0usize, 0usize);
    // Each traced hoistway with the walls round it.
    let reach = 2.0 * upf;
    let max = r.max_clear_ft * upf;
    let seen: Vec<Seen> = traced
        .iter()
        .filter_map(|(level_name, name, tr)| {
            let level = sp.level(level_name).cloned()?;
            let near = tr.expand(max);
            let walls: Vec<Wall> = level_walls(d, &level.name).into_iter().filter(|w| w.b.intersects(&near)).collect();
            let (xs, ys) = ((tr.min.x, tr.max.x), (tr.min.y, tr.max.y));
            let fx = (faces(&walls, 0, true, ys, reach, max_t), faces(&walls, 0, false, ys, reach, max_t));
            let fy = (faces(&walls, 1, true, xs, reach, max_t), faces(&walls, 1, false, xs, reach, max_t));
            Some(Seen { level, name: name.clone(), tr: *tr, walls, fx, fy })
        })
        .collect();
    // One elevator's shaft on every storey it serves (its name, about one place on the stacked
    // plans): one size for all of them.
    let mut stacks: Vec<Vec<usize>> = Vec::new();
    for (i, x) in seen.iter().enumerate() {
        let at = x.tr.center() + off(&x.level.name);
        let mine = stacks.iter_mut().find(|g| {
            g.first()
                .and_then(|j| seen.get(*j))
                .is_some_and(|f| f.name.eq_ignore_ascii_case(&x.name) && (f.tr.center() + off(&f.level.name)).dist(at) <= max)
        });
        match mine {
            Some(g) => g.push(i),
            None => stacks.push(vec![i]),
        }
    }
    let fit = |x: &Seen, sx: Option<f64>, sy: Option<f64>| {
        let (ax, scx, wx) = fit_axis(&x.fx.0, &x.fx.1, (x.tr.min.x, x.tr.max.x), sx, max, tol);
        let (ay, scy, wy) = fit_axis(&x.fy.0, &x.fy.1, (x.tr.min.y, x.tr.max.y), sy, max, tol);
        (ax, ay, scx + scy, wx.or(wy))
    };
    let mut chosen: Vec<(usize, usize, [Option<f64>; 2], [&'static str; 2])> = Vec::new();
    for (gi, g) in stacks.iter().enumerate() {
        let members: Vec<&Seen> = g.iter().filter_map(|i| seen.get(*i)).collect();
        let Some(first) = members.first() else { continue };
        // What each storey's walls show with no size given: the extents their faces agree on.
        let mut plan: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
        for x in &members {
            for (axis, list, span) in [(0usize, &x.fx, (x.tr.min.x, x.tr.max.x)), (1, &x.fy, (x.tr.min.y, x.tr.max.y))] {
                let (ext, sc, _) = fit_axis(&list.0, &list.1, span, None, max, tol);
                let v = ext.1 - ext.0;
                if let Some(vals) = plan.get_mut(axis)
                    && sc > 0.0
                    && !vals.iter().any(|o| (o - v).abs() <= tol)
                    && vals.len() < MAX_VALUES
                {
                    vals.push(v);
                }
            }
        }
        // Each size read, both ways round (the width across the trace's longer side first); an
        // axis no document sizes takes what the walls show. The one the storeys' walls agree
        // with best, the most read first among equals.
        let wide_x = first.tr.width() >= first.tr.height();
        let opts = |given: Option<f64>, axis: usize| -> Vec<(Option<f64>, &'static str)> {
            match given {
                Some(v) => vec![(Some(v), "submittal")],
                None => plan.get(axis).into_iter().flatten().map(|v| (Some(*v), "plan walls")).chain(std::iter::once((None, "plan walls"))).collect(),
            }
        };
        let mut options: Vec<([Option<f64>; 2], [&'static str; 2])> = Vec::new();
        for w in &widths {
            for dp in &depths {
                let turns = if wide_x { [(*w, *dp), (*dp, *w)] } else { [(*dp, *w), (*w, *dp)] };
                for (sx, sy) in turns {
                    for (vx, fx) in opts(sx, 0) {
                        for (vy, fy) in opts(sy, 1) {
                            if options.len() < 400 {
                                options.push(([vx, vy], [fx, fy]));
                            }
                        }
                    }
                }
            }
        }
        let mut best: Option<(f64, [Option<f64>; 2], [&'static str; 2])> = None;
        for (sizes, from) in options {
            let sc: f64 = members.iter().map(|x| fit(x, sizes[0], sizes[1]).2).sum();
            if best.as_ref().is_none_or(|b| sc > b.0 + 1e-9) {
                best = Some((sc, sizes, from));
            }
        }
        if let Some((_, sizes, from)) = best {
            chosen.extend(g.iter().map(|i| (*i, gi, sizes, from)));
        }
    }
    for (i, stack, sizes, from_doc) in chosen {
        let Some(x) = seen.get(i) else { continue };
        let (level, name, tr, walls) = (&x.level, &x.name, &x.tr, &x.walls);
        let (ax, ay, score, support) = fit(x, sizes[0], sizes[1]);
        let rect = Bounds2::new(Vec2::new(ax.0, ay.0), Vec2::new(ax.1, ay.1));
        if !(rect.width() > 0.0 && rect.height() > 0.0) {
            continue;
        }
        let from_axis = |axis: usize| -> &'static str {
            let f = from_doc.get(axis).copied().unwrap_or("plan trace");
            if f == "plan walls" && score <= 0.0 { "plan trace" } else { f }
        };
        let from = [from_axis(0), from_axis(1)];
        let support_wall = support.and_then(|i| walls.get(i));
        let (t, wall_type) = match support_wall.and_then(|w| w.axis(max_t).map(|a| (a.1, w.wall_type.clone()))) {
            Some((t, ty)) if t > 0.0 => (t, ty),
            _ => (r.shaft_wall_in.max(1.0) * upf / 12.0, None),
        };
        // Nothing inside the shaft is a wall: a wall there is removed, one crossing it cut back.
        let inside = rect.expand(-tol);
        let mut handles: Vec<Handle> = walls.iter().map(|w| w.handle).collect();
        handles.sort_by_key(|h| h.0);
        handles.dedup();
        for h in handles {
            match cut_out(d, h, &inside, tol, max_t) {
                Trim::Removed => removed += 1,
                Trim::Cut => clipped += 1,
                Trim::Kept => {}
            }
        }
        let near = rect.expand(t + 2.0 * tol);
        let kept: Vec<Wall> = level_walls(d, &level.name).into_iter().filter(|w| w.b.intersects(&near)).collect();
        // Doors and windows built inside it (a car's door drawn) go with them.
        let inner: Vec<Handle> = d
            .model
            .iter()
            .take(MAX_ELEMENTS)
            .filter(|e| matches!(wallgen::made_as(e).as_deref(), Some("door" | "window")))
            .filter(|e| super::bspec::made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(&level.name)))
            .filter(|e| inside.contains(cadaddy_doc::entity_bounds(d, e, 0).center()))
            .map(|e| e.handle)
            .collect();
        for h in inner {
            d.model.remove(h);
            removed += 1;
        }
        // Each side the plan draws no wall on (less its door) gets the shaft's wall.
        let doors: Vec<Bounds2> = d
            .model
            .iter()
            .take(MAX_ELEMENTS)
            .filter(|e| {
                wallgen::made_as(e).as_deref() == Some("door") && super::bspec::made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(&level.name))
            })
            .map(|e| cadaddy_doc::entity_bounds(d, e, 0))
            .filter(|b| b.intersects(&rect.expand(t + tol)))
            .collect();
        let z0 = level.elevation_ft * upf;
        let z1 = (level.elevation_ft + level.height_ft.max(0.1)) * upf;
        let basis = format!(
            "elevator hoistway {name}: clear {:.2} x {:.2} ft ({}, {}); the plan draws no wall on this side of the shaft{}",
            rect.width() / upf,
            rect.height() / upf,
            if from[0] == "submittal" { format!("{} across", size_src("width")) } else { format!("{} across", from[0]) },
            if from[1] == "submittal" { format!("{} deep", size_src("depth")) } else { format!("{} deep", from[1]) },
            if wall_type.is_some() { ", of the type its other walls are" } else { "" }
        );
        let wall_basis = if wall_type.is_none() { format!("{basis}; {}", r.shaft_wall_basis) } else { basis.clone() };
        let mut sides_added = 0usize;
        // A side's wall runs corner to corner (outside faces); a gap no longer than a wall's
        // thickness at a corner is the other side's, one ending that close to a corner runs to it.
        let mut laid: Vec<Bounds2> = Vec::new();
        for (axis, at, outward) in [(0usize, rect.min.y, -1.0), (0, rect.max.y, 1.0), (1, rect.min.x, -1.0), (1, rect.max.x, 1.0)] {
            // The side runs along `axis` at `at`; its wall stands outside it.
            let strip_lo = if outward < 0.0 { at - t - tol } else { at - tol };
            let strip_hi = if outward < 0.0 { at + tol } else { at + t + tol };
            let (s0, s1) = if axis == 0 { (rect.min.x - t, rect.max.x + t) } else { (rect.min.y - t, rect.max.y + t) };
            let across = |b: &Bounds2| if axis == 0 { (b.min.y, b.max.y) } else { (b.min.x, b.max.x) };
            let along = |b: &Bounds2| if axis == 0 { (b.min.x, b.max.x) } else { (b.min.y, b.max.y) };
            let mut covered: Vec<(f64, f64)> = kept
                .iter()
                .map(|w| w.b)
                .chain(doors.iter().copied())
                .chain(laid.iter().copied())
                .filter(|b| overlap(across(b), (strip_lo, strip_hi)) > 0.0)
                .map(|b| along(&b))
                .collect();
            covered.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut gaps: Vec<(f64, f64)> = Vec::new();
            let mut s = s0;
            for c in &covered {
                if c.0 > s {
                    gaps.push((s, c.0.min(s1)));
                }
                s = s.max(c.1);
            }
            if s < s1 {
                gaps.push((s, s1));
            }
            let c = at + outward * t / 2.0;
            let corner = t + tol;
            for (g0, g1) in gaps.into_iter().filter(|g| g.1 - g.0 > corner) {
                let g0 = if g0 - s0 <= corner { s0 } else { g0 };
                let g1 = if s1 - g1 <= corner { s1 } else { g1 };
                let (p0, p1) = if axis == 0 { (Vec2::new(g0, c), Vec2::new(g1, c)) } else { (Vec2::new(c, g0), Vec2::new(c, g1)) };
                let mut m = wallgen::empty_mesh();
                wallgen::add_box(&mut m, p0, p1, t, z0, z1);
                laid.push(Bounds2::from_points(m.vertices.iter().map(|v| Vec2::new(v.x, v.y))));
                let mut x = wallgen::tag(Part::Wall, &level.name, Handle(0));
                if let Some(ty) = &wall_type {
                    x.push(RawTag { code: 1000, value: format!("type:{}", ty.chars().take(250).collect::<String>()) });
                }
                x.push(RawTag { code: 1000, value: format!("hoistway:{}", name.chars().take(200).collect::<String>()) });
                x.extend(super::provenance::tags(Made::Derived, &wall_basis));
                if d.add(
                    &Space::Model,
                    Common { layer: wallgen::layer_of(Part::Wall).into(), xdata: x.into(), ..Common::default() },
                    EntityKind::Mesh(m),
                )
                .is_ok()
                {
                    added += 1;
                    sides_added += 1;
                }
            }
        }
        report.push(json!({
            "level": level.name, "name": name,
            "clear_ft": [(rect.width() / upf * 100.0).round() / 100.0, (rect.height() / upf * 100.0).round() / 100.0],
            "traced_ft": [(tr.width() / upf * 100.0).round() / 100.0, (tr.height() / upf * 100.0).round() / 100.0],
            "size_from": from,
            "box": [[rect.min.x, rect.min.y], [rect.max.x, rect.max.y]],
            "wall_in": (t / upf * 12.0 * 10.0).round() / 10.0, "wall_type": wall_type.clone(), "walls_added": sides_added,
        }));
        shafts.push(Shaft { level: level.name.clone(), name: name.clone(), rect, t, stack });
    }
    // The shafts one above the other: each stack's pit under its lowest landing and overrun over
    // its top one.
    let mut built: Vec<Vec<usize>> = Vec::new();
    for (i, x) in shafts.iter().enumerate() {
        match built.iter_mut().find(|g| g.first().and_then(|j| shafts.get(*j)).is_some_and(|f| f.stack == x.stack)) {
            Some(g) => g.push(i),
            None => built.push(vec![i]),
        }
    }
    let elevation = |s: &Shaft| sp.level(&s.level).map_or(0.0, |l| l.elevation_ft);
    let (mut pits, mut overruns, mut lifted) = (0usize, 0usize, 0usize);
    let mut sorted_levels: Vec<&cadaddy_walk3d::Level> = sp.levels.iter().filter(|l| l.sheet.is_none()).collect();
    sorted_levels.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    for g in &built {
        let mut g: Vec<&Shaft> = g.iter().filter_map(|i| shafts.get(*i)).collect();
        g.sort_by(|a, b| elevation(a).total_cmp(&elevation(b)));
        let (Some(low), Some(top)) = (g.first().copied(), g.last().copied()) else { continue };
        // The pit.
        let (pit_ft, pit_src, pit_made) = match data.pit_ft.filter(|p| in_range(*p, r.pit_range_ft)) {
            Some(p) => (p, size_src("pit"), Made::Derived),
            None => match sp.elevators.iter().find_map(|e| e.pit_ft.filter(|p| in_range(*p, r.pit_range_ft)).map(|p| (p, e.source.clone()))) {
                Some((p, src)) => (p, if src.trim().is_empty() { "the plan's PIT note".to_string() } else { src }, Made::Derived),
                None => (r.default_pit_ft, r.default_pit_basis.clone(), Made::Inferred),
            },
        };
        let z0 = elevation(low) * upf;
        let bottom = z0 - pit_ft * upf;
        let mut m = ring(&low.rect, low.t, bottom, z0);
        slab(&mut m, &low.rect.expand(low.t), bottom - r.pit_slab_ft.max(0.1) * upf, bottom);
        let basis = format!("elevator {} pit: {:.2} ft below the lowest landing ({}) from {pit_src}", low.name, pit_ft, low.level);
        let x = with_props(
            tags(PIT, &low.level, &low.name, pit_made, &basis),
            &[("pit_depth", pit_ft, pit_src.as_str(), if pit_made == Made::Derived { SourceKind::Submittal } else { SourceKind::Rule })],
        );
        if d.add(&Space::Model, Common { layer: LAYER.into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            pits += 1;
        }
        // The overrun: from the roof over the top landing up to the overhead.
        let (oh_ft, oh_src, oh_made) = match data.overhead_ft.filter(|o| in_range(*o, r.overhead_range_ft)) {
            Some(o) => (o, size_src("overhead"), Made::Derived),
            None => (r.default_overhead_ft, r.default_overhead_basis.clone(), Made::Inferred),
        };
        let top_ft = elevation(top);
        let above = sorted_levels.iter().find(|l| l.elevation_ft > top_ft + 0.01);
        let roof_ft = above.map_or_else(|| sp.level(&top.level).map_or(top_ft, |l| top_ft + l.height_ft), |l| l.elevation_ft);
        let ceiling_ft = top_ft + oh_ft;
        if ceiling_ft <= roof_ft + 0.25 {
            if notes.len() < MAX_NOTES {
                notes.push(format!("Elevator {}: its overhead ({oh_ft:.2} ft) fits under the roof; no overrun.", top.name));
            }
            continue;
        }
        let (on, shift) = match above.filter(|l| wallgen::roof_level(&sp.levels, &l.name)) {
            Some(l) => (l.name.clone(), off(&top.level) - off(&l.name)),
            None => (top.level.clone(), Vec2::ZERO),
        };
        let rect = Bounds2::new(top.rect.min + shift, top.rect.max + shift);
        let cap = r.cap_ft.max(0.1) * upf;
        let mut m = ring(&rect, top.t, roof_ft * upf, ceiling_ft * upf);
        slab(&mut m, &rect.expand(top.t), ceiling_ft * upf, ceiling_ft * upf + cap);
        let basis = format!(
            "elevator {} overrun: the hoistway {:.2} ft over the top landing ({}) from {oh_src}, {:.2} ft above the roof, roofed",
            top.name,
            oh_ft,
            top.level,
            ceiling_ft - roof_ft
        );
        let x = with_props(
            tags(OVERRUN, &on, &top.name, oh_made, &basis),
            &[
                ("overhead", oh_ft, oh_src.as_str(), if oh_made == Made::Derived { SourceKind::Submittal } else { SourceKind::Rule }),
                ("height_above_roof", ceiling_ft - roof_ft + r.cap_ft.max(0.1), "computed from the overhead and the roof", SourceKind::Computed),
            ],
        );
        if d.add(&Space::Model, Common { layer: LAYER.into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            overruns += 1;
        }
        // What the roof plan stands over the overrun stands on its roof.
        let top_z = ceiling_ft * upf + cap;
        let outer = rect.expand(top.t);
        let over: Vec<(Handle, f64)> = d
            .model
            .iter()
            .take(MAX_ELEMENTS)
            .filter(|e| {
                wallgen::made_as(e).as_deref() == Some("equipment") && super::bspec::made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(&on))
            })
            .filter_map(|e| {
                let EntityKind::Mesh(m) = &e.kind else { return None };
                let b = Bounds2::from_points(m.vertices.iter().map(|v| Vec2::new(v.x, v.y)));
                let z = m.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
                (outer.contains(b.center()) && z.is_finite() && z < top_z - 1e-6).then_some((e.handle, z))
            })
            .collect();
        for (h, z) in over {
            let dz = top_z - z;
            let design = format!("{:.2}", z / upf);
            let corrected = format!("{:.2}", top_z / upf);
            let c = super::provenance::Correction {
                key: "base_elevation_ft".into(),
                design,
                corrected,
                reason: format!(
                    "it stands over elevator {}'s overrun, which rises {:.2} ft above the roof: it sits on the overrun's roof",
                    top.name,
                    (top_z / upf) - roof_ft
                ),
                rule: r.rule.clone(),
            };
            if d.modify_entity(h, |e| {
                if let EntityKind::Mesh(m) = &mut e.kind {
                    for v in m.vertices.iter_mut() {
                        v.z += dz;
                    }
                }
                let mut ps = props::read(&e.common.xdata);
                let _ = super::provenance::correct(&mut ps, &c, Source::from_doc(SourceKind::Print, "the roof plan"));
                e.common.xdata = props::write(&e.common.xdata, &ps).into();
            })
            .is_ok()
            {
                lifted += 1;
            }
        }
    }
    json!({
        "hoistways": shafts.len(), "shafts": report, "shaftWalls": added, "wallsRemoved": removed, "wallsCut": clipped,
        "pits": pits, "overruns": overruns, "equipmentOnOverrun": lifted, "notes": notes,
        "data": sp.elevator_data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(t: &str) -> ElevatorData {
        let mut out = Reading::default();
        read_text(t, "doc", &rules(), &mut out);
        out.data().unwrap_or_default()
    }

    #[test]
    fn a_submittal_line_gives_the_hoistway_its_size_pit_and_overhead() {
        let d = read("--- page 3 of 9 ---\nHOISTWAY 7'-4\" x 5'-11\", PIT 4'-8\", OVERHEAD 13'-10\"\n");
        assert_eq!(d.width_ft.len(), 1);
        assert!((d.width_ft[0] - (7.0 + 4.0 / 12.0)).abs() < 1e-6, "{d:?}");
        assert!((d.depth_ft[0] - (5.0 + 11.0 / 12.0)).abs() < 1e-6, "{d:?}");
        assert!((d.pit_ft.unwrap_or(0.0) - (4.0 + 8.0 / 12.0)).abs() < 1e-6, "{d:?}");
        assert!((d.overhead_ft.unwrap_or(0.0) - (13.0 + 10.0 / 12.0)).abs() < 1e-6, "{d:?}");
        assert_eq!(d.sources.get("pit").map(String::as_str), Some("doc p.3"));
    }

    #[test]
    fn labelled_values_read_before_or_after_their_words_and_the_most_read_comes_first() {
        let d = read(
            "7'-10\" HOISTWAY WIDTH   SEE NOTES\nCLEAR HOISTWAY DEPTH 6'-7 1/2\"\n-4'-9\" PIT EL\n7'-10\" HOISTWAY WIDTH\n9'-10\" HOISTWAY WIDTH\nOVERHEAD 14'-1\"\n",
        );
        assert_eq!(d.width_ft.len(), 2, "{d:?}");
        assert!((d.width_ft[0] - (7.0 + 10.0 / 12.0)).abs() < 1e-6, "the most read first: {d:?}");
        assert!((d.depth_ft[0] - (6.0 + 7.5 / 12.0)).abs() < 1e-6, "{d:?}");
        assert!((d.pit_ft.unwrap_or(0.0) - (4.0 + 9.0 / 12.0)).abs() < 1e-6, "{d:?}");
        assert!((d.overhead_ft.unwrap_or(0.0) - (14.0 + 1.0 / 12.0)).abs() < 1e-6, "{d:?}");
    }

    #[test]
    fn door_sill_and_tolerance_sizes_are_no_hoistway_size() {
        let d = read("3'-2\" CLEAR DOOR OPENING HOISTWAY\nHOISTWAY SILL 5'-10 1/2\"\nMAXIMUM HOISTWAY WIDTH 10'-5\"\nPIT LADDER\n");
        assert!(d.width_ft.is_empty() && d.depth_ft.is_empty() && d.pit_ft.is_none(), "{d:?}");
    }

    #[test]
    fn lengths_read_feet_inches_and_fractions_and_hostile_text_does_not_panic() {
        let t: Vec<char> = "8' - 6 1/2\" AND 12' AND 102\" AND 1/2\" AND 99999999'".chars().collect();
        let l: Vec<f64> = lengths(&t).iter().map(|l| l.ft).collect();
        assert_eq!(l.len(), 3, "{l:?}");
        assert!((l[0] - (8.0 + 6.5 / 12.0)).abs() < 1e-9);
        assert!((l[1] - 12.0).abs() < 1e-9);
        assert!((l[2] - 8.5).abs() < 1e-9);
        for s in ["'", "\"", "1'", "1'-", "1'-1/", "1'-1 1/", "1'-1 1/0\"", "x'''''", "\u{2019}\u{201D}9'9"] {
            let _ = read(s);
        }
    }

    #[test]
    fn a_size_fits_where_the_walls_faces_agree_and_with_none_it_takes_the_outer_faces() {
        // Faces below at 0 (the shaft's) and 20 (a car's), above at 80 (a car's) and 102.
        let lows = vec![(0.0, 60.0, 0), (20.0, 50.0, 1)];
        let highs = vec![(80.0, 50.0, 2), (102.0, 60.0, 3)];
        let (ext, sc, _) = fit_axis(&lows, &highs, (20.0, 80.0), Some(102.0), 144.0, 3.0);
        assert_eq!(ext, (0.0, 102.0));
        assert!(sc > 0.0);
        let (ext, _, _) = fit_axis(&lows, &highs, (20.0, 80.0), None, 144.0, 3.0);
        assert_eq!(ext, (0.0, 102.0));
        // No face agrees: centred on the trace.
        let (ext, sc, _) = fit_axis(&[], &[], (20.0, 80.0), Some(90.0), 144.0, 3.0);
        assert_eq!((ext, sc), ((5.0, 95.0), 0.0));
        let (ext, _, _) = fit_axis(&[], &[], (20.0, 80.0), None, 144.0, 3.0);
        assert_eq!(ext, (20.0, 80.0));
    }

    #[test]
    fn the_rules_load() {
        let r = rules();
        assert!(r.default_pit_ft > 0.0 && r.default_overhead_ft > 0.0 && r.shaft_wall_in > 0.0 && r.max_clear_ft > 0.0);
        assert!(!r.hoistway_words.is_empty() && !r.pit_words.is_empty() && !r.overhead_words.is_empty());
    }
}
