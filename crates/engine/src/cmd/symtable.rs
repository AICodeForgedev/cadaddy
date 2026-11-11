//! The print set's symbol table (Q6484): every symbol any plan of the set draws is an object of a
//! kind, found wherever it is drawn.
//!
//! A SHARED SYMBOL LIBRARY ships with the app ([`symlib.json`], rules data in paper inches: the
//! symbols every set draws the same way — duplex and quad receptacles, a floor box, a data outlet,
//! a junction box, a smoke detector, a thermostat, the switches' letters). A file of the same
//! shape in the settings folder's `symbols/` folder replaces a symbol of the same name or adds
//! one; `learned.json` there holds the drawings promoted from sets ([`promote`]).
//!
//! Each set's table, kept in the drawing, STARTS AS THE LIBRARY and is changed per set:
//! - by its own legends ([`calibrate`]): each legend row's drawing is an entry of the row's kind
//!   (documented, its sheet and row the source). A row that draws a kind its own way, or gives a
//!   library drawing another meaning, turns the library's drawing of it off for this set (said and
//!   logged);
//! - by the answers to its questions (a drawing that repeats on its plans and that nothing
//!   explains is asked about, once for all its copies) and by `bim.symbol.set` (add a drawing,
//!   change an entry's kind, turn one off or on). Every change is logged with who and why and can
//!   be reverted (`bim.symbol.revert`); `bim.symbol.promote` writes an entry into the shared
//!   library. Nothing goes into the library without that step.
//!
//! The job looks for every entry on every plan of the set (`guided`), whatever the set's legend
//! says: a library entry's copies are derived devices (`symbol library: <name>`), a legend row's
//! documented ones. A plain outline (a triangle, a square) counts on a plan only with a second
//! cue: the lettering its row draws inside it, a circuit or designation printed by it, the wall
//! it sits on, or an answer that confirmed it. Each object of a kind the library draws is SHOWN
//! with the library's symbol (`symbol_shown_as`), with the drawing the print used kept beside it
//! (`symbol_as_drawn`); a kind the library doesn't draw keeps the set's own drawing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use cadaddy_doc::Drawing;
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::unitmarks::{Piece, Template};
use super::*;
use crate::{EngineError, Result, Session};

const LIST: &str = "bim.symbols";
const SET: &str = "bim.symbol.set";
const LOG: &str = "bim.symbol.log";
const REVERT: &str = "bim.symbol.revert";
const PROMOTE: &str = "bim.symbol.promote";

/// Where the drawing keeps its symbol table.
const FILE: &str = "symbol-table.json";
/// The shipped library.
const SHIPPED: &str = include_str!("symlib.json");
/// The settings folder's file of drawings promoted from sets.
pub(crate) const LEARNED_FILE: &str = "learned.json";
/// The lettering height of a set's notes on paper, inches (3/32"): what sizes a library symbol on
/// a plan whose paper scale is not known, from the plan's lettering.
pub(crate) const PAPER_TEXT_IN: f64 = 0.093_75;
/// Who the job's own changes are by.
pub(crate) const JOB: &str = "the job";

/// Entries, strokes per entry, log entries kept; library files and their size read, at most.
const MAX_ENTRIES: usize = 2_000;
const MAX_STROKES: usize = 64;
const MAX_LOG: usize = 5_000;
const MAX_LIB_FILES: usize = 32;
const MAX_LIB_FILE: u64 = 1024 * 1024;
const MAX_TEXT: usize = 200;
/// Hits on one sheet, at most; pieces filed per hit as its linework.
const MAX_SHEET_HITS: usize = 20_000;
const MAX_HIT_HANDLES: usize = 64;
/// A straight piece this many times a symbol's size or longer, this near it (a share of its size),
/// is the wall it is on.
const HOST_LEN: f64 = 3.0;
const HOST_NEAR: f64 = 0.75;
/// A wall's two faces: this many symbol sizes apart at most.
const WALL_PAIR: f64 = 1.5;
/// A circuit or designation printed this many symbol sizes from it is its.
const TAG_REACH: f64 = 2.5;
/// Lettering inside a symbol: its middle this share of the symbol's size from the symbol's.
const LETTER_IN: f64 = 0.45;
/// An unexplained drawing: at most this big on paper (inches) and of this many pieces.
const MAX_CLUSTER_IN: f64 = 0.35;
const MIN_CLUSTER_PIECES: usize = 2;
const MAX_CLUSTER_PIECES: usize = 40;
/// Clusters kept per sheet, at most.
const MAX_SHEET_CLUSTERS: usize = 5_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(LIST, "Symbol Table", run_list)
            .params("{library?: bool (the shared symbol library instead of this set's table), kind?} → {entries: [{id, name, kind, origin: library|learned|legend|answer|user, status: documented|derived, source, confirmed, off, off_reason?, shown_as?, strokes, letters}], library: bool, folder?}: the set's symbol table (it starts as the shared library and the job calibrates it from the set's legends and answers)")
            .noundo(),
        CommandSpec::new(SET, "Change Symbol Table", run_set)
            .params("{action: add | kind | off | on | remove, id? (the entry; add makes one), kind? (add, kind), name?, handles? (add: the linework that draws the symbol; default the selection), letters?: [text] (add: lettering drawn inside it), reason?, author?} → {entry, change, next_step}: one change to this set's symbol table, logged and revertible; the job's next build (job.start {refresh: \"bim_build\"}) finds every copy again")
            ,
        CommandSpec::new(LOG, "Symbol Table Log", run_log)
            .params("{id?, limit? (default 50)} → {changes: [{id, at, who, why, entry, action, reverted}], total}")
            .noundo(),
        CommandSpec::new(REVERT, "Revert Symbol Table Change", run_revert)
            .params("{change: id, author?} → {reverted, entry}: the entry as it was before that change"),
        CommandSpec::new(PROMOTE, "Promote Symbol to Library", run_promote)
            .params("{id, folder? (default: the settings folder's symbols/)} → {path, name, kind, sets}: this set's entry written into the shared library (learned.json), in paper inches; a drawing the library holds already counts one more set"),
    ]
}

// ─── Strokes, the library ────────────────────────────────────────────────────

/// One stroke of a symbol's drawing: a straight piece (its two ends) or a circle (its centre and
/// radius).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stroke {
    Seg([f64; 4]),
    Round([f64; 3]),
}

impl Stroke {
    pub(crate) fn piece(self) -> Option<Piece> {
        match self {
            Stroke::Seg([x0, y0, x1, y1]) if [x0, y0, x1, y1].iter().all(|x| x.is_finite()) => Some(Piece::Seg(Vec2::new(x0, y0), Vec2::new(x1, y1))),
            Stroke::Round([x, y, r]) if [x, y, r].iter().all(|v| v.is_finite()) && r > 0.0 => Some(Piece::Round(Vec2::new(x, y), r)),
            _ => None,
        }
    }

    pub(crate) fn of(p: &Piece) -> Stroke {
        match *p {
            Piece::Seg(a, b) => Stroke::Seg([a.x, a.y, b.x, b.y]),
            Piece::Round(c, r) => Stroke::Round([c.x, c.y, r]),
        }
    }

    fn scaled(self, k: f64) -> Stroke {
        match self {
            Stroke::Seg(v) => Stroke::Seg(v.map(|x| x * k)),
            Stroke::Round(v) => Stroke::Round(v.map(|x| x * k)),
        }
    }

    /// Turned `k` quarters counter-clockwise about the origin.
    pub(crate) fn turned(self, k: u8) -> Stroke {
        let q = |x: f64, y: f64| match k % 4 {
            1 => (-y, x),
            2 => (-x, -y),
            3 => (y, -x),
            _ => (x, y),
        };
        match self {
            Stroke::Seg([x0, y0, x1, y1]) => {
                let (a, b) = q(x0, y0);
                let (c, d) = q(x1, y1);
                Stroke::Seg([a, b, c, d])
            }
            Stroke::Round([x, y, r]) => {
                let (a, b) = q(x, y);
                Stroke::Round([a, b, r])
            }
        }
    }
}

fn yes() -> bool {
    true
}

/// A symbol of the shared library: its name, the kind and IFC class of the object it is, its
/// drawing (paper inches about its middle) and the lettering drawn inside it (or the lettering
/// alone: a switch's `S`), whether it is the universal drawing of its kind (objects of the kind
/// are shown with it), and for a promoted drawing how many sets confirmed it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LibSymbol {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub ifc: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strokes: Vec<Stroke>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub letters: Vec<String>,
    #[serde(default = "yes")]
    pub universal: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub sets: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct LibraryFile {
    #[serde(default)]
    source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plan_disciplines: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    not_plan_views: Vec<String>,
    #[serde(default)]
    symbols: Vec<LibSymbol>,
}

/// Whether a view's title says it draws no devices in the building (a detail, a schedule, a
/// diagram: the shipped library's `not_plan_views` words).
pub(crate) fn not_a_plan_view(title: &str) -> bool {
    let up = title.to_ascii_uppercase();
    let words: Vec<&str> = up.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').filter(|w| !w.is_empty()).collect();
    let joined = format!(" {} ", words.join(" "));
    serde_json::from_str::<LibraryFile>(SHIPPED)
        .map(|f| f.not_plan_views.iter().any(|w| joined.contains(&format!(" {} ", w.trim().to_ascii_uppercase()))))
        .unwrap_or(false)
}

/// The sheet disciplines whose plans the set's electrical legends and the library speak for
/// (the shipped library's `plan_disciplines`).
pub(crate) fn plan_disciplines() -> Vec<String> {
    serde_json::from_str::<LibraryFile>(SHIPPED)
        .map(|f| f.plan_disciplines.iter().map(|d| d.trim().to_ascii_uppercase()).filter(|d| !d.is_empty()).take(32).collect())
        .unwrap_or_default()
}

fn parse_library(text: &str, learned: bool) -> Vec<(LibSymbol, bool)> {
    let Ok(f) = serde_json::from_str::<LibraryFile>(text) else { return Vec::new() };
    f.symbols
        .into_iter()
        .take(MAX_ENTRIES)
        .filter_map(|mut s| {
            s.name = cap(&s.name, MAX_TEXT);
            s.kind = cap(&s.kind, MAX_TEXT);
            s.strokes.retain(|st| st.piece().is_some());
            s.strokes.truncate(MAX_STROKES);
            s.letters = clean_letters(&s.letters);
            let drawn = !s.strokes.is_empty() || !s.letters.is_empty();
            (!s.name.is_empty() && !s.kind.is_empty() && drawn).then_some((s, learned))
        })
        .collect()
}

fn clean_letters(l: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in l.iter().take(8) {
        let t = t.trim().to_ascii_uppercase();
        if !t.is_empty() && t.chars().count() <= 6 && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// The settings folder's `symbols/` (`folder` given: that).
fn library_folder(folder: Option<&str>) -> Option<std::path::PathBuf> {
    match folder.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => Some(std::path::PathBuf::from(f)),
        None => crate::control_auth::config_dir().map(|d| d.join("symbols")),
    }
}

/// The shared library: the shipped symbols, each replaced by (or joined by) the symbols of the
/// `*.json` files in `folder` (default the settings folder's `symbols/`). Each with whether it
/// was promoted from a set (`learned.json`).
pub fn library(folder: Option<&std::path::Path>) -> Vec<(LibSymbol, bool)> {
    let mut out = parse_library(SHIPPED, false);
    let dir = match folder {
        Some(f) => Some(f.to_path_buf()),
        None => library_folder(None),
    };
    let Some(dir) = dir else { return out };
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    let mut files: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .take(MAX_LIB_FILES)
        .collect();
    files.sort();
    for p in files {
        if !std::fs::metadata(&p).is_ok_and(|m| m.len() <= MAX_LIB_FILE) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let learned = p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(LEARNED_FILE));
        for (s, l) in parse_library(&text, learned) {
            out.retain(|(x, _)| !x.name.eq_ignore_ascii_case(&s.name));
            if out.len() < MAX_ENTRIES {
                out.push((s, l));
            }
        }
    }
    out
}

/// The library symbol objects of `kind` are shown with: its universal drawing.
pub(crate) fn universal_of<'a>(lib: &'a [(LibSymbol, bool)], kind: &str) -> Option<&'a LibSymbol> {
    lib.iter().map(|(s, _)| s).find(|s| s.universal && !s.strokes.is_empty() && s.kind.eq_ignore_ascii_case(kind))
}

/// A drawing's shape whatever its size, place and quarter turn: its pieces in its box scaled to
/// sixteen, the least of its four turns' sorted lists. Equal for two copies of one symbol.
pub fn signature(strokes: &[Stroke]) -> String {
    if strokes.iter().any(|s| s.piece().is_none()) {
        return String::new();
    }
    let mut best: Option<String> = None;
    for k in 0..4u8 {
        let turned: Vec<Stroke> = strokes.iter().map(|s| s.turned(k)).collect();
        let (lo, hi) = turned.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), s| match *s {
            Stroke::Seg([a, b, c, d]) => ([lo[0].min(a).min(c), lo[1].min(b).min(d)], [hi[0].max(a).max(c), hi[1].max(b).max(d)]),
            Stroke::Round([x, y, r]) => ([lo[0].min(x - r), lo[1].min(y - r)], [hi[0].max(x + r), hi[1].max(y + r)]),
        });
        let size = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        if !(size.is_finite() && size > 0.0) {
            return String::new();
        }
        let q = |v: f64| (v / size * 16.0).round() as i64;
        let mut parts: Vec<String> = turned
            .iter()
            .map(|s| match *s {
                Stroke::Seg([a, b, c, d]) => {
                    let (p, r) = ((q(a - lo[0]), q(b - lo[1])), (q(c - lo[0]), q(d - lo[1])));
                    let (p, r) = if p <= r { (p, r) } else { (r, p) };
                    format!("s{},{},{},{}", p.0, p.1, r.0, r.1)
                }
                Stroke::Round([x, y, r]) => format!("r{},{},{}", q(x - lo[0]), q(y - lo[1]), q(r)),
            })
            .collect();
        parts.sort_unstable();
        parts.dedup();
        let t = parts.join(";");
        if best.as_ref().is_none_or(|b| t < *b) {
            best = Some(t);
        }
    }
    best.unwrap_or_default()
}

// ─── The set's table ─────────────────────────────────────────────────────────

/// One entry of a set's table.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// The object kind its copies are (`receptacle`); `not_device`: a drawing known to be no
    /// object (answered so), found so it is not asked about again.
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ifc: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strokes: Vec<Stroke>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub letters: Vec<String>,
    /// The lettering height of the sheet or legend row it was taken from (library: none), that
    /// sheet's drawing units per paper inch when known (library: 1, paper inches) and its width.
    #[serde(default)]
    pub text_h: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upi: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet_w: Option<f64>,
    /// `library`, `learned` (promoted from another set), `legend`, `answer` or `user`.
    pub origin: String,
    /// `E-001 legend: DUPLEX RECEPTACLE`, `symbol library: duplex receptacle`, `answer Q-bim-3`.
    pub source: String,
    /// How it was confirmed: `legend`, `agent`, `user` or `library`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub confirmed: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub off_reason: String,
    /// Its row is a low voltage device's (on no power circuit).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub low_voltage: bool,
    /// The library symbol its objects are shown with (its kind's universal drawing).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shown_as: String,
    /// Written into the shared library already (an answer's drawing).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub promoted: bool,
}

impl Entry {
    /// `derived` for the library's drawings (no document of this set says so), `documented` for
    /// the set's legend's, an answer's or a person's.
    pub fn status(&self) -> &'static str {
        if matches!(self.origin.as_str(), "library" | "learned") { "derived" } else { "documented" }
    }

    /// Its drawing is in paper inches (the library's).
    fn on_paper(&self) -> bool {
        matches!(self.origin.as_str(), "library" | "learned")
    }

    /// Confirmed by an answer or a person: a plain outline needs no second cue.
    fn confirmed_by_person(&self) -> bool {
        matches!(self.confirmed.as_str(), "agent" | "user")
    }

    fn summary(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "kind": self.kind, "origin": self.origin, "status": self.status(),
            "source": self.source, "confirmed": self.confirmed, "off": self.off,
            "off_reason": if self.off_reason.is_empty() { Value::Null } else { json!(self.off_reason) },
            "shown_as": if self.shown_as.is_empty() { Value::Null } else { json!(self.shown_as) },
            "strokes": self.strokes.len(), "letters": self.letters,
        })
    }
}

/// One change of a table.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub id: u64,
    pub at: u64,
    pub who: String,
    pub why: String,
    pub entry: String,
    /// `add`, `kind`, `off`, `on`, `remove`, `legend` (a legend row overrode it), `drawing` (the
    /// library's drawing changed), `promote`, `revert`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Entry>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reverted: bool,
}

/// What the drawing keeps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Table {
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub log: Vec<Change>,
    #[serde(default)]
    pub next: u64,
}

fn now() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn cap(t: &str, n: usize) -> String {
    t.trim().chars().take(n).collect()
}

/// Lower-case words joined by `-`, for ids.
fn slug(t: &str) -> String {
    let w: String = t.chars().take(80).map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    w.split('-').filter(|x| !x.is_empty()).collect::<Vec<_>>().join("-")
}

impl Table {
    fn log(&mut self, mut c: Change) -> u64 {
        self.next = self.next.saturating_add(1);
        c.id = self.next;
        c.at = now();
        let id = c.id;
        self.log.push(c);
        if self.log.len() > MAX_LOG {
            let extra = self.log.len() - MAX_LOG;
            self.log.drain(..extra);
        }
        id
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Whether a person changed the entry (its last change not the job's and not reverted): the
    /// job's calibration leaves it as they set it.
    fn touched(&self, id: &str) -> bool {
        self.log.iter().rev().find(|c| c.entry == id && !c.reverted).is_some_and(|c| c.who != JOB)
    }

    fn add(&mut self, e: Entry, who: &str, why: &str) -> bool {
        if self.entries.len() >= MAX_ENTRIES || self.get(&e.id).is_some() {
            return false;
        }
        self.log(Change { who: who.into(), why: why.into(), entry: e.id.clone(), action: "add".into(), after: Some(e.clone()), ..Change::default() });
        self.entries.push(e);
        true
    }

    /// Change one entry by `f`, logged as `action`; false when nothing changed.
    fn change(&mut self, id: &str, who: &str, why: &str, action: &str, f: impl FnOnce(&mut Entry)) -> bool {
        let Some(e) = self.get_mut(id) else { return false };
        let before = e.clone();
        f(e);
        if *e == before {
            return false;
        }
        let after = e.clone();
        self.log(Change {
            who: who.into(),
            why: why.into(),
            entry: id.to_string(),
            action: action.into(),
            before: Some(before),
            after: Some(after),
            ..Change::default()
        });
        true
    }
}

/// The table the drawing keeps (none yet: empty).
pub fn stored(d: &Drawing) -> Table {
    d.embedded.get(FILE).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default()
}

/// Keep `t` in the drawing.
pub(crate) fn keep(d: &mut Drawing, t: &Table) -> std::result::Result<(), String> {
    let bytes = serde_json::to_vec(t).map_err(|e| e.to_string())?;
    d.embedded.insert(FILE.into(), Arc::new(bytes));
    Ok(())
}

fn save(s: &mut Session, t: &Table) -> Result<()> {
    keep(s.doc_mut()?, t).map_err(|e| EngineError::Other(format!("{LIST}: {e}")))?;
    s.touch();
    Ok(())
}

/// A legend row's drawing, for [`calibrate`]: its sheet and words, the kind and IFC class its
/// words name, its pieces about their middle, its lettering height, its sheet's units per paper
/// inch (when known) and width, and whether its table is of low voltage devices.
pub(crate) struct LegendDrawing {
    pub sheet: String,
    pub row: String,
    pub kind: String,
    pub ifc: String,
    pub strokes: Vec<Stroke>,
    pub letters: Vec<String>,
    pub text_h: f64,
    pub upi: Option<f64>,
    pub sheet_w: Option<f64>,
    pub low_voltage: bool,
}

/// The library symbol's entry.
fn library_entry(s: &LibSymbol, learned: bool) -> Entry {
    Entry {
        id: format!("{}:{}", if learned { "learned" } else { "library" }, slug(&s.name)),
        name: s.name.clone(),
        kind: s.kind.clone(),
        ifc: s.ifc.clone(),
        strokes: s.strokes.clone(),
        letters: s.letters.clone(),
        text_h: PAPER_TEXT_IN,
        upi: Some(1.0),
        sheet_w: None,
        origin: if learned { "learned" } else { "library" }.into(),
        source: format!("symbol library: {}", s.name),
        confirmed: "library".into(),
        shown_as: if s.universal && !s.strokes.is_empty() { s.name.clone() } else { String::new() },
        ..Entry::default()
    }
}

/// Make the set's table: the library's symbols (added where the table lacks them, their drawings
/// kept up with the library's unless a person changed them), the set's legend rows (each an
/// entry of its row's kind: a row that draws a kind its own way, or a library drawing with
/// another meaning, turns that library drawing off here), and the answers' drawings. Each entry
/// is shown as its kind's universal library symbol. Returns how many changes it made.
pub(crate) fn calibrate(t: &mut Table, lib: &[(LibSymbol, bool)], legend: &[LegendDrawing], answers: &[Entry]) -> usize {
    let mut n = 0usize;
    for (s, learned) in lib {
        let e = library_entry(s, *learned);
        if t.get(&e.id).is_none() {
            n += usize::from(t.add(e, JOB, "the shared symbol library"));
        } else if !t.touched(&e.id) {
            let (strokes, letters) = (e.strokes.clone(), e.letters.clone());
            n += usize::from(t.change(&e.id, JOB, "the shared symbol library's drawing changed", "drawing", |x| {
                x.strokes = strokes;
                x.letters = letters;
            }));
        }
    }
    for row in legend {
        let id = format!("legend:{}:{}", slug(&row.sheet), slug(&row.row));
        let e = Entry {
            id: id.clone(),
            name: cap(&row.row, MAX_TEXT),
            kind: row.kind.clone(),
            ifc: row.ifc.clone(),
            strokes: row.strokes.iter().take(MAX_STROKES).copied().collect(),
            letters: clean_letters(&row.letters),
            text_h: row.text_h,
            upi: row.upi,
            sheet_w: row.sheet_w,
            origin: "legend".into(),
            source: cap(&format!("{} legend: {}", row.sheet, row.row.trim()), MAX_TEXT),
            confirmed: "legend".into(),
            low_voltage: row.low_voltage,
            ..Entry::default()
        };
        if t.get(&id).is_none() {
            n += usize::from(t.add(e, JOB, "the set's legend"));
        }
    }
    for a in answers {
        if t.get(&a.id).is_none() {
            n += usize::from(t.add(a.clone(), JOB, "an answer"));
        }
    }
    // The set's legend speaks for its kinds and drawings: the library's drawing of a kind the
    // legend draws its own way, or the library drawing it gives another meaning, is off here.
    let legend_rows: Vec<(String, String, String)> =
        t.entries.iter().filter(|e| e.origin == "legend" && !e.off).map(|e| (e.kind.clone(), signature(&e.strokes), e.source.clone())).collect();
    let library_ids: Vec<(String, String, String)> =
        t.entries.iter().filter(|e| e.on_paper() && !e.off).map(|e| (e.id.clone(), e.kind.clone(), signature(&e.strokes))).collect();
    for (id, kind, sig) in library_ids {
        if t.touched(&id) {
            continue;
        }
        let other = legend_rows.iter().find(|(k, s, _)| !sig.is_empty() && *s == sig && !k.eq_ignore_ascii_case(&kind));
        let same_kind: Vec<&(String, String, String)> = legend_rows.iter().filter(|(k, _, _)| k.eq_ignore_ascii_case(&kind)).collect();
        let why = match other {
            Some((_, _, src)) => Some(format!("the set's legend gives this drawing another meaning ({src})")),
            None if !same_kind.is_empty() && same_kind.iter().all(|(_, s, _)| *s != sig) => {
                same_kind.first().map(|(_, _, src)| format!("the set's legend draws {kind} its own way ({src})"))
            }
            None => None,
        };
        if let Some(why) = why {
            n += usize::from(t.change(&id, JOB, &why, "legend", |x| {
                x.off = true;
                x.off_reason.clone_from(&why);
            }));
        }
    }
    // Each entry shown as its kind's universal drawing (a kind the library doesn't draw keeps
    // the set's own).
    let shown: Vec<(String, String)> = t
        .entries
        .iter()
        .map(|e| (e.id.clone(), universal_of(lib, &e.kind).map(|s| s.name.clone()).unwrap_or_default()))
        .filter(|(id, name)| t.get(id).is_some_and(|e| e.shown_as != *name))
        .collect();
    for (id, name) in shown {
        if let Some(e) = t.get_mut(&id) {
            e.shown_as = name;
        }
    }
    n
}

/// Per-row and per-sheet counts of a search, for the job's report.
#[derive(Clone, Debug, Default)]
pub(crate) struct Report {
    /// Entry id → sheet → copies found.
    pub by_row: BTreeMap<String, BTreeMap<String, usize>>,
    /// Sheet → (symbols found, matched as devices, asked about).
    pub by_sheet: BTreeMap<String, [usize; 3]>,
}

// ─── The search on one sheet ─────────────────────────────────────────────────

/// How one sheet's drawing units relate to paper: its units per paper inch when the import knows
/// its scale, the height its lettering is drawn at, its width.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SheetUnits {
    pub upi: Option<f64>,
    pub lettering_h: Option<f64>,
    pub width: f64,
}

impl SheetUnits {
    /// Its units per paper inch: known, else from its lettering's height.
    fn paper(&self) -> Option<f64> {
        self.upi.or_else(|| self.lettering_h.map(|h| h / PAPER_TEXT_IN)).filter(|u| u.is_finite() && *u > 0.0)
    }
}

/// How many of a sheet's units one unit of the entry's drawing is.
pub(crate) fn unit_for(e: &Entry, sh: &SheetUnits) -> Option<f64> {
    let u = if e.on_paper() {
        sh.paper()?
    } else if let (Some(a), Some(b)) = (e.upi, sh.upi) {
        b / a
    } else if let Some(h) = sh.lettering_h.filter(|_| e.text_h > 0.0) {
        h / e.text_h
    } else if let Some(w) = e.sheet_w.filter(|w| *w > 0.0 && sh.width > 0.0) {
        sh.width / w
    } else {
        1.0
    };
    (u.is_finite() && u > 0.0).then_some(u)
}

/// One sheet as the search takes it: its linework (each piece with the handle of the object it
/// came from), its lettering, the part of it holding views, the parts left out (its legend and
/// schedule tables, its title block, its typical-room views), its units.
pub(crate) struct SheetScene {
    pub pieces: Vec<(Piece, u64)>,
    pub texts: Vec<TextItem>,
    pub area: [f64; 4],
    pub exclude: Vec<[f64; 4]>,
    pub units: SheetUnits,
    /// The set's sheet numbers (never a circuit).
    pub sheet_numbers: BTreeSet<String>,
}

/// An entry's copy found on a sheet: the entry, its middle and size, its turn from the entry's
/// drawing, the cue that confirmed it, and the objects that draw it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    pub entry: usize,
    pub at: Vec2,
    pub size: f64,
    pub turn: u8,
    pub cue: &'static str,
    pub handles: Vec<u64>,
    /// The drawing as this sheet draws it (its pieces about its middle).
    pub drawn: Vec<Stroke>,
}

/// A small drawing of loose linework nothing explained: its middle, size, shape and pieces.
#[derive(Clone, Debug)]
pub(crate) struct Cluster {
    pub at: Vec2,
    pub size: f64,
    pub signature: String,
    pub strokes: Vec<Stroke>,
    pub handles: Vec<u64>,
}

/// What the search found on one sheet: the hits, the plain outlines found with no second cue
/// (left out), and the small drawings nothing explained.
#[derive(Default)]
pub(crate) struct SheetFound {
    pub hits: Vec<Hit>,
    pub uncued: usize,
    pub clusters: Vec<Cluster>,
}

fn inside(r: &[f64; 4], p: Vec2) -> bool {
    p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3]
}

fn middle(p: &Piece) -> Vec2 {
    match *p {
        Piece::Seg(a, b) => (a + b) * 0.5,
        Piece::Round(c, _) => c,
    }
}

fn piece_box(p: &Piece) -> [f64; 4] {
    match *p {
        Piece::Seg(a, b) => [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)],
        Piece::Round(c, r) => [c.x - r, c.y - r, c.x + r, c.y + r],
    }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.len2();
    if l2 <= 0.0 {
        return p.dist(a);
    }
    let u = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(a + ab * u)
}

/// The middle of a text's lettering (its point is the start of its baseline).
fn text_middle(t: &TextItem) -> Vec2 {
    let n = t.value.trim().chars().count() as f64;
    t.at + Vec2::from_angle(t.rotation) * (0.3 * t.height * n) + Vec2::from_angle(t.rotation + std::f64::consts::FRAC_PI_2) * (0.5 * t.height)
}

/// Whether a text is a circuit or a device designation printed by a symbol.
fn tag_text(t: &str, sheet_numbers: &BTreeSet<String>) -> bool {
    let t = t.trim();
    !t.is_empty()
        && (cadaddy_mepfix::sheet_facts::is_circuit_label(t)
            || super::guided::circuit_text(t, sheet_numbers)
            || super::readers::switch_designation(t).is_some())
}

/// Boxes filed by cells, to find the ones about a point.
struct Grid {
    cell: f64,
    map: HashMap<(i64, i64), Vec<usize>>,
}

/// Cells one box is filed under, at most (a long wall line is found from those along it).
const MAX_BOX_CELLS: i64 = 400;

impl Grid {
    fn new(boxes: impl Iterator<Item = [f64; 4]>, cell: f64) -> Grid {
        let cell = if cell.is_finite() && cell > 0.0 { cell } else { 1.0 };
        let mut map: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, b) in boxes.enumerate() {
            if !b.iter().all(|v| v.is_finite()) {
                continue;
            }
            let (x0, y0, x1, y1) =
                ((b[0] / cell).floor() as i64, (b[1] / cell).floor() as i64, (b[2] / cell).floor() as i64, (b[3] / cell).floor() as i64);
            if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > MAX_BOX_CELLS {
                // A long piece: filed along its diagonal, the cells it runs through.
                let n = (x1 - x0).abs().max((y1 - y0).abs()).clamp(1, MAX_BOX_CELLS);
                for k in 0..=n {
                    let f = k as f64 / n as f64;
                    let (x, y) = (b[0] + (b[2] - b[0]) * f, b[1] + (b[3] - b[1]) * f);
                    let c = ((x / cell).floor() as i64, (y / cell).floor() as i64);
                    let v = map.entry(c).or_default();
                    if v.last() != Some(&i) {
                        v.push(i);
                    }
                }
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    map.entry((x, y)).or_default().push(i);
                }
            }
        }
        Grid { cell, map }
    }

    /// The boxes filed within `r` of `p` (each once).
    fn near(&self, p: Vec2, r: f64) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        let (x0, y0, x1, y1) = (
            ((p.x - r) / self.cell).floor() as i64,
            ((p.y - r) / self.cell).floor() as i64,
            ((p.x + r) / self.cell).floor() as i64,
            ((p.y + r) / self.cell).floor() as i64,
        );
        if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > MAX_BOX_CELLS {
            return out;
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.map.get(&(x, y)) {
                    out.extend(v.iter().copied());
                }
            }
        }
        out
    }
}

/// Look for every entry of the table that is on (and has a drawing or lettering) on one sheet.
pub(crate) fn search(t: &Table, sc: &SheetScene) -> SheetFound {
    let mut out = SheetFound::default();
    let keep = |p: Vec2| inside(&sc.area, p) && !sc.exclude.iter().any(|r| inside(r, p));
    let pieces: Vec<(Piece, u64)> = sc.pieces.iter().filter(|(p, _)| keep(middle(p))).copied().collect();
    let texts: Vec<&TextItem> = sc.texts.iter().filter(|t| keep(text_middle(t))).collect();
    // The templates, each at this sheet's size.
    let mut templates: Vec<Template> = Vec::new();
    let mut of: Vec<usize> = Vec::new();
    for (i, e) in t.entries.iter().enumerate().filter(|(_, e)| !e.off && !e.strokes.is_empty()) {
        let drawn: Vec<Piece> = e.strokes.iter().filter_map(|s| s.piece()).collect();
        let Some(tp) = Template::drawing(&e.source, "", &e.kind, &e.ifc, &drawn, if e.text_h > 0.0 { e.text_h } else { 1.0 }, &e.letters) else {
            continue;
        };
        let Some(tp) = unit_for(e, &sc.units).and_then(|u| tp.at_unit(u)) else { continue };
        templates.push(tp);
        of.push(i);
    }
    let sizes: Vec<f64> = templates.iter().map(|tp| tp.size() / tp.text_h()).filter(|s| s.is_finite() && *s > 0.0).collect();
    let largest = sizes.iter().copied().fold(0.0, f64::max);
    // The smallest symbol looked for (a template, or a letter of a lettering-only entry).
    let lettering_only = t.entries.iter().any(|e| !e.off && e.strokes.is_empty() && !e.letters.is_empty());
    let smallest = sizes
        .iter()
        .copied()
        .chain(texts.iter().filter(|_| lettering_only).map(|t| 2.0 * t.height))
        .filter(|v| v.is_finite() && *v > 0.0)
        .fold(f64::INFINITY, f64::min);
    let cell = largest.max(if smallest.is_finite() { smallest } else { 0.0 }).max(1e-6) * 1.5;
    let long: Vec<(Vec2, Vec2)> = pieces
        .iter()
        .filter_map(|(p, _)| if let Piece::Seg(a, b) = *p { Some((a, b)) } else { None })
        .filter(|(a, b)| smallest.is_finite() && smallest * HOST_LEN <= a.dist(*b))
        .collect();
    let long_grid = Grid::new(long.iter().map(|(a, b)| [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)]), cell);
    let piece_grid = Grid::new(pieces.iter().map(|(p, _)| piece_box(p)), cell);
    let text_grid = Grid::new(texts.iter().map(|t| text_middle(t)).map(|m| [m.x, m.y, m.x, m.y]), cell);
    // A wall: a long line by the symbol with a parallel one a wall's thickness off it (a section
    // mark's cut line or a door's leaf is one line alone).
    let hosted = |at: Vec2, size: f64| {
        let near: Vec<&(Vec2, Vec2)> = long_grid.near(at, (HOST_NEAR + WALL_PAIR) * size).into_iter().filter_map(|i| long.get(i)).collect();
        near.iter().any(|(a, b)| {
            a.dist(*b) >= HOST_LEN * size
                && seg_dist(at, *a, *b) <= HOST_NEAR * size
                && near.iter().any(|(c, d)| {
                    let (u, v) = (*b - *a, *d - *c);
                    let (lu, lv) = (u.len(), v.len());
                    let off = seg_dist((*c + *d) * 0.5, *a, *b);
                    lu > 0.0
                        && lv > 0.0
                        && (u.cross(v) / (lu * lv)).abs() <= 0.05
                        && off > 1e-9 * lu
                        && off <= WALL_PAIR * size
                        && seg_dist(at, *c, *d) <= (HOST_NEAR + WALL_PAIR) * size
                })
        })
    };
    let texts_near = |at: Vec2, r: f64| text_grid.near(at, r).into_iter().filter_map(|i| texts.get(i).copied()).collect::<Vec<&TextItem>>();
    let tagged = |at: Vec2, size: f64| {
        texts_near(at, TAG_REACH * size * 2.0)
            .iter()
            .any(|t| text_middle(t).dist(at) <= TAG_REACH * size.max(t.height) && tag_text(&t.value, &sc.sheet_numbers))
    };
    let lettered = |at: Vec2, size: f64, letters: &[String]| {
        texts_near(at, LETTER_IN * size)
            .iter()
            .any(|t| text_middle(t).dist(at) <= LETTER_IN * size && letters.iter().any(|l| t.value.trim().eq_ignore_ascii_case(l)))
    };
    let mut used: BTreeSet<usize> = BTreeSet::new();
    if !templates.is_empty() {
        let scene = super::unitmarks::Scene::new(pieces.iter().map(|(p, _)| *p).collect(), cell);
        let found = super::unitmarks::find_symbols_apart(&templates, &scene, sc.area, 1.0);
        for f in found.into_iter().take(MAX_SHEET_HITS) {
            let (Some(&ei), Some(tp)) = (of.get(f.template), templates.get(f.template)) else { continue };
            let Some(e) = t.entries.get(ei) else { continue };
            if sc.exclude.iter().any(|r| inside(r, f.at)) {
                continue;
            }
            // A drawing known by its lettering too: only with that lettering inside.
            if !e.letters.is_empty() && !lettered(f.at, f.size, &e.letters) {
                continue;
            }
            let cue = if !e.letters.is_empty() {
                "its lettering inside"
            } else if !tp.simple {
                "its drawing"
            } else if e.confirmed_by_person() {
                "an answer confirmed its drawing"
            } else if tagged(f.at, f.size) {
                "a circuit or designation by it"
            } else if hosted(f.at, f.size) {
                "the wall it is on"
            } else {
                out.uncued += 1;
                continue;
            };
            // The objects that draw it, and its drawing as this sheet draws it.
            let r = 0.6 * f.size;
            let mut handles: Vec<u64> = Vec::new();
            let mut drawn: Vec<Stroke> = Vec::new();
            for k in piece_grid.near(f.at, r) {
                let Some((p, h)) = pieces.get(k) else { continue };
                let b = piece_box(p);
                if b[0] >= f.at.x - r && b[2] <= f.at.x + r && b[1] >= f.at.y - r && b[3] <= f.at.y + r {
                    used.insert(k);
                    if handles.len() < MAX_HIT_HANDLES && !handles.contains(h) {
                        handles.push(*h);
                    }
                    if drawn.len() < MAX_STROKES {
                        let s = Stroke::of(p);
                        drawn.push(match s {
                            Stroke::Seg([a, b, c, d]) => Stroke::Seg([a - f.at.x, b - f.at.y, c - f.at.x, d - f.at.y]),
                            Stroke::Round([x, y, rr]) => Stroke::Round([x - f.at.x, y - f.at.y, rr]),
                        });
                    }
                }
            }
            out.hits.push(Hit { entry: ei, at: f.at, size: f.size, turn: f.turn, cue, handles, drawn });
        }
    }
    // The lettering-only entries (a switch's `S`): each such lettering on the wall it is by.
    for (i, e) in t.entries.iter().enumerate().filter(|(_, e)| !e.off && e.strokes.is_empty() && !e.letters.is_empty()) {
        for tx in &texts {
            if out.hits.len() >= MAX_SHEET_HITS {
                break;
            }
            let v = tx.value.trim();
            let sized = tx.height.is_finite() && tx.height > 0.0;
            if !sized || !e.letters.iter().any(|l| v.eq_ignore_ascii_case(l)) {
                continue;
            }
            let at = text_middle(tx);
            let size = 2.0 * tx.height;
            // Inside another hit's drawing (a smoke detector's `S`): that one's.
            if out.hits.iter().any(|h| h.at.dist(at) <= 0.6 * h.size) {
                continue;
            }
            if !hosted(at, size) {
                out.uncued += 1;
                continue;
            }
            out.hits.push(Hit { entry: i, at, size, turn: 0, cue: "its lettering by the wall it is on", handles: Vec::new(), drawn: Vec::new() });
        }
    }
    // The small drawings left: grouped where they touch, each a cluster to ask about when it
    // repeats.
    if let Some(paper) = sc.units.paper() {
        out.clusters = clusters(&pieces, &used, MAX_CLUSTER_IN * paper, &out.hits);
    }
    out
}

/// The small drawings among `pieces` (but the `used` ones): pieces shorter than `max` grouped
/// where their boxes touch; each group of a few pieces no bigger than `max`, away from every hit.
fn clusters(pieces: &[(Piece, u64)], used: &BTreeSet<usize>, max: f64, hits: &[Hit]) -> Vec<Cluster> {
    if !(max.is_finite() && max > 0.0) {
        return Vec::new();
    }
    let small: Vec<usize> = (0..pieces.len())
        .filter(|k| !used.contains(k))
        .filter(|k| {
            pieces.get(*k).is_some_and(|(p, _)| {
                let b = piece_box(p);
                (b[2] - b[0]).max(b[3] - b[1]) <= max
            })
        })
        .collect();
    let tol = 0.02 * max;
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let cell = max;
    let key = |x: f64, y: f64| ((x / cell).floor() as i64, (y / cell).floor() as i64);
    for (n, k) in small.iter().enumerate() {
        if let Some((p, _)) = pieces.get(*k) {
            let m = middle(p);
            grid.entry(key(m.x, m.y)).or_default().push(n);
        }
    }
    let mut parent: Vec<usize> = (0..small.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        let mut steps = 0;
        while let Some(&p) = parent.get(i) {
            if p == i || steps > 10_000 {
                break;
            }
            if let Some(&pp) = parent.get(p)
                && let Some(slot) = parent.get_mut(i)
            {
                *slot = pp;
            }
            i = p;
            steps += 1;
        }
        i
    }
    let boxes: Vec<[f64; 4]> = small.iter().filter_map(|k| pieces.get(*k).map(|(p, _)| piece_box(p))).collect();
    for (n, b) in boxes.iter().enumerate() {
        let (cx, cy) = key((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(list) = grid.get(&(cx + dx, cy + dy)) else { continue };
                for &m in list {
                    if m <= n {
                        continue;
                    }
                    let Some(o) = boxes.get(m) else { continue };
                    if b[0] - tol <= o[2] && o[0] - tol <= b[2] && b[1] - tol <= o[3] && o[1] - tol <= b[3] {
                        let (ra, rb) = (root(&mut parent, n), root(&mut parent, m));
                        if ra != rb
                            && let Some(slot) = parent.get_mut(ra)
                        {
                            *slot = rb;
                        }
                    }
                }
            }
        }
    }
    // A piece inside a group's outline (a circle in a square) is that group's too.
    let mut group_box: BTreeMap<usize, [f64; 4]> = BTreeMap::new();
    for (n, b) in boxes.iter().enumerate() {
        let r = root(&mut parent, n);
        let g = group_box.entry(r).or_insert(*b);
        *g = [g[0].min(b[0]), g[1].min(b[1]), g[2].max(b[2]), g[3].max(b[3])];
    }
    for (r, g) in group_box {
        let size = (g[2] - g[0]).max(g[3] - g[1]);
        if !(size > 0.0 && size <= max) {
            continue;
        }
        let (cx, cy) = key((g[0] + g[2]) / 2.0, (g[1] + g[3]) / 2.0);
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(list) = grid.get(&(cx + dx, cy + dy)) else { continue };
                for &m in list {
                    let Some(o) = boxes.get(m) else { continue };
                    if o[0] >= g[0] - tol && o[2] <= g[2] + tol && o[1] >= g[1] - tol && o[3] <= g[3] + tol {
                        let (ra, rb) = (root(&mut parent, m), root(&mut parent, r));
                        if ra != rb
                            && let Some(slot) = parent.get_mut(ra)
                        {
                            *slot = rb;
                        }
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for n in 0..small.len() {
        let r = root(&mut parent, n);
        groups.entry(r).or_default().push(n);
    }
    let mut out = Vec::new();
    for members in groups.values() {
        if !(MIN_CLUSTER_PIECES..=MAX_CLUSTER_PIECES).contains(&members.len()) || out.len() >= MAX_SHEET_CLUSTERS {
            continue;
        }
        let b = members.iter().filter_map(|n| boxes.get(*n)).fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |a, q| {
            [a[0].min(q[0]), a[1].min(q[1]), a[2].max(q[2]), a[3].max(q[3])]
        });
        let size = (b[2] - b[0]).max(b[3] - b[1]);
        if !(size.is_finite() && size > 0.0 && size <= max) {
            continue;
        }
        let at = Vec2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        if hits.iter().any(|h| h.at.dist(at) <= h.size.max(size)) {
            continue;
        }
        let strokes: Vec<Stroke> = members
            .iter()
            .filter_map(|n| small.get(*n).and_then(|k| pieces.get(*k)))
            .map(|(p, _)| match Stroke::of(p) {
                Stroke::Seg([a, b2, c, d]) => Stroke::Seg([a - at.x, b2 - at.y, c - at.x, d - at.y]),
                Stroke::Round([x, y, r]) => Stroke::Round([x - at.x, y - at.y, r]),
            })
            .collect();
        let mut handles: Vec<u64> = members.iter().filter_map(|n| small.get(*n).and_then(|k| pieces.get(*k)).map(|(_, h)| *h)).collect();
        handles.sort_unstable();
        handles.dedup();
        handles.truncate(MAX_HIT_HANDLES);
        out.push(Cluster { at, size, signature: signature(&strokes), strokes, handles });
    }
    out
}

// ─── Commands ────────────────────────────────────────────────────────────────

fn author_of(p: &Value) -> String {
    str_param(p, "author").map(|a| cap(a, 64)).filter(|a| !a.is_empty()).unwrap_or_else(|| "user".into())
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = str_param(p, "kind").map(str::trim).filter(|k| !k.is_empty());
    let want = |k: &str| kind.is_none_or(|w| w.eq_ignore_ascii_case(k));
    if p.get("library").and_then(Value::as_bool) == Some(true) {
        let lib = library(None);
        let entries: Vec<Value> = lib
            .iter()
            .filter(|(x, _)| want(&x.kind))
            .map(|(x, learned)| {
                json!({"name": x.name, "kind": x.kind, "ifc": x.ifc, "origin": if *learned { "learned" } else { "library" }, "universal": x.universal, "sets": x.sets, "strokes": x.strokes, "letters": x.letters})
            })
            .collect();
        return Ok(json!({"entries": entries, "library": true, "folder": library_folder(None).map(|d| d.display().to_string())}));
    }
    let t = stored(s.doc()?);
    let entries: Vec<Value> = t.entries.iter().filter(|e| want(&e.kind)).map(Entry::summary).collect();
    let mut out = json!({"entries": entries, "library": false});
    if t.entries.is_empty() {
        out["next_step"] = json!(
            "the job makes the set's table from the shared library and the set's legends at its build (job.start); bim.symbols {library: true} shows the library"
        );
    }
    Ok(out)
}

/// A drawing made of the given objects: their pieces about the middle of their box, and the
/// height of the lettering round it.
fn drawing_of(s: &Session, handles: &[cadaddy_doc::Handle]) -> Result<(Vec<Stroke>, f64)> {
    let d = s.doc()?;
    let mut pieces: Vec<Piece> = Vec::new();
    for h in handles.iter().take(MAX_STROKES) {
        let Some(e) = d.model.get(*h) else { return Err(bad(SET, format!("there is no object {} in the model", h.hex()))) };
        match &e.kind {
            cadaddy_doc::EntityKind::Insert(_) => {
                for part in super::modify::explode_kind(d, e).unwrap_or_default().iter().take(MAX_STROKES) {
                    super::unitmarks::pieces(&part.kind, &mut pieces);
                }
            }
            k => super::unitmarks::pieces(k, &mut pieces),
        }
    }
    pieces.truncate(MAX_STROKES);
    let b = pieces.iter().map(piece_box).fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |a, q| {
        [a[0].min(q[0]), a[1].min(q[1]), a[2].max(q[2]), a[3].max(q[3])]
    });
    let size = (b[2] - b[0]).max(b[3] - b[1]);
    if pieces.is_empty() || !(size.is_finite() && size > 0.0) {
        return Err(bad(SET, "the objects given draw no lines, circles or arcs"));
    }
    let mid = Vec2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let strokes = pieces
        .iter()
        .map(|p| match *p {
            Piece::Seg(a, c) => Stroke::Seg([a.x - mid.x, a.y - mid.y, c.x - mid.x, c.y - mid.y]),
            Piece::Round(c, r) => Stroke::Round([c.x - mid.x, c.y - mid.y, r]),
        })
        .collect();
    // The lettering round it: the middle height of the texts within a few sizes of it.
    let mut hs: Vec<f64> = d
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Text(t) if t.height.is_finite() && t.height > 0.0 && t.insert.xy().dist(mid) <= 20.0 * size => Some(t.height),
            _ => None,
        })
        .take(2_000)
        .collect();
    hs.sort_by(f64::total_cmp);
    let text_h = hs.get(hs.len() / 2).copied().unwrap_or(size / 2.0);
    Ok((strokes, text_h))
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let action = str_param(p, "action").map(str::trim).unwrap_or("").to_ascii_lowercase();
    let who = author_of(p);
    let reason = str_param(p, "reason").map(|r| cap(r, MAX_TEXT)).unwrap_or_default();
    let mut t = stored(s.doc()?);
    let id = str_param(p, "id").map(|i| cap(i, MAX_TEXT)).unwrap_or_default();
    let kind = str_param(p, "kind").map(|k| cap(k, MAX_TEXT)).filter(|k| !k.is_empty());
    let change = match action.as_str() {
        "add" => {
            let kind = kind.ok_or_else(|| bad(SET, "`kind` is required to add a symbol"))?;
            let hs = targets(s, p)?;
            if hs.is_empty() {
                return Err(bad(SET, "give `handles` (the linework that draws the symbol) or select it"));
            }
            let (strokes, text_h) = drawing_of(s, &hs)?;
            let letters = p
                .get("letters")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
                .unwrap_or_default();
            let name = str_param(p, "name").map(|n| cap(n, MAX_TEXT)).filter(|n| !n.is_empty()).unwrap_or_else(|| kind.replace('_', " "));
            let sig = signature(&strokes);
            let id = if id.is_empty() { format!("user:{}", slug(&format!("{name} {}", &sig.chars().take(24).collect::<String>()))) } else { id };
            if t.get(&id).is_some() {
                return Err(bad(SET, format!("the table has an entry {id} already: change it with action kind/off/on")));
            }
            let e = Entry {
                id: id.clone(),
                name,
                kind,
                strokes,
                letters: clean_letters(&letters),
                text_h,
                origin: "user".into(),
                source: format!("added by {who}"),
                confirmed: "user".into(),
                ..Entry::default()
            };
            if !t.add(e, &who, if reason.is_empty() { "bim.symbol.set add" } else { &reason }) {
                return Err(bad(SET, format!("the table holds {MAX_ENTRIES} entries, the most it keeps")));
            }
            id
        }
        "kind" => {
            let kind = kind.ok_or_else(|| bad(SET, "`kind` is required"))?;
            if t.get(&id).is_none() {
                return Err(bad(SET, format!("no entry {id} in the table (bim.symbols lists them)")));
            }
            t.change(&id, &who, if reason.is_empty() { "bim.symbol.set kind" } else { &reason }, "kind", |e| {
                e.kind = kind;
                e.confirmed = "user".into();
            });
            id
        }
        "off" | "on" => {
            if t.get(&id).is_none() {
                return Err(bad(SET, format!("no entry {id} in the table (bim.symbols lists them)")));
            }
            let off = action == "off";
            let why = if reason.is_empty() { format!("bim.symbol.set {action}") } else { reason.clone() };
            t.change(&id, &who, &why, &action, |e| {
                e.off = off;
                e.off_reason = if off { why.clone() } else { String::new() };
            });
            id
        }
        "remove" => {
            let Some(e) = t.get(&id).cloned() else { return Err(bad(SET, format!("no entry {id} in the table"))) };
            if e.origin != "user" && e.origin != "answer" {
                return Err(bad(SET, format!("{id} is the {}'s: turn it off instead (action off)", e.origin)));
            }
            t.entries.retain(|x| x.id != id);
            t.log(Change { who: who.clone(), why: reason.clone(), entry: id.clone(), action: "remove".into(), before: Some(e), ..Change::default() });
            id
        }
        _ => return Err(bad(SET, "`action` is add, kind, off, on or remove")),
    };
    let entry = t.get(&change).map(Entry::summary).unwrap_or(Value::Null);
    let last = t.log.last().map_or(0, |c| c.id);
    save(s, &t)?;
    Ok(
        json!({"entry": entry, "change": last, "next_step": "the job's next build finds every copy on the set again: job.start {pdfs, refresh: \"bim_build\"}"}),
    )
}

fn run_log(s: &mut Session, p: &Value) -> Result<Value> {
    let t = stored(s.doc()?);
    let id = str_param(p, "id").map(str::trim).filter(|i| !i.is_empty());
    let limit = p.get("limit").and_then(Value::as_u64).map_or(50, |n| n.clamp(1, 1_000)) as usize;
    let mine: Vec<&Change> = t.log.iter().filter(|c| id.is_none_or(|i| c.entry == i)).collect();
    let changes: Vec<Value> = mine
        .iter()
        .rev()
        .take(limit)
        .map(|c| json!({"id": c.id, "at": c.at, "who": c.who, "why": c.why, "entry": c.entry, "action": c.action, "reverted": c.reverted}))
        .collect();
    Ok(json!({"changes": changes, "total": mine.len()}))
}

fn run_revert(s: &mut Session, p: &Value) -> Result<Value> {
    let change = p.get("change").and_then(Value::as_u64).ok_or_else(|| bad(REVERT, "`change` (an id from bim.symbol.log) is required"))?;
    let who = author_of(p);
    let mut t = stored(s.doc()?);
    let Some(c) = t.log.iter().find(|c| c.id == change).cloned() else { return Err(bad(REVERT, format!("no change {change} in the log"))) };
    if c.reverted {
        return Err(bad(REVERT, format!("change {change} is reverted already")));
    }
    // A later change of the same entry is reverted first.
    if t.log.iter().any(|x| x.entry == c.entry && x.id > c.id && !x.reverted && x.action != "revert") {
        return Err(bad(REVERT, format!("{} changed again after change {change}: revert the later change first", c.entry)));
    }
    match &c.before {
        Some(b) => {
            if let Some(e) = t.get_mut(&c.entry) {
                *e = b.clone();
            } else {
                t.entries.push(b.clone());
            }
        }
        None => t.entries.retain(|e| e.id != c.entry),
    }
    if let Some(x) = t.log.iter_mut().find(|x| x.id == change) {
        x.reverted = true;
    }
    t.log(Change { who, why: format!("reverted change {change}"), entry: c.entry.clone(), action: "revert".into(), ..Change::default() });
    let entry = t.get(&c.entry).map(Entry::summary).unwrap_or(Value::Null);
    save(s, &t)?;
    Ok(json!({"reverted": change, "entry": entry}))
}

/// An entry's drawing in paper inches: the library's as it is; a set's from its sheet's units per
/// paper inch, else its lettering's height.
fn on_paper(e: &Entry) -> Option<Vec<Stroke>> {
    let k = if e.on_paper() {
        1.0
    } else if let Some(u) = e.upi.filter(|u| u.is_finite() && *u > 0.0) {
        1.0 / u
    } else if e.text_h.is_finite() && e.text_h > 0.0 {
        PAPER_TEXT_IN / e.text_h
    } else {
        return None;
    };
    Some(e.strokes.iter().map(|s| s.scaled(k)).collect())
}

/// Write `e` into the shared library's `learned.json` in `dir`: a drawing the library holds
/// already (by its shape and kind) counts one more set; another kind's is replaced. Returns the
/// file and the sets that confirmed it.
pub(crate) fn promote(dir: &std::path::Path, e: &Entry) -> std::result::Result<(std::path::PathBuf, u32), String> {
    if e.kind.is_empty() || (e.strokes.is_empty() && e.letters.is_empty()) {
        return Err(format!("{} has no kind or no drawing", e.id));
    }
    let strokes = on_paper(e).ok_or_else(|| format!("{}: its sheet's paper scale isn't known", e.id))?;
    let path = dir.join(LEARNED_FILE);
    let mut file = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<LibraryFile>(&t).ok()).unwrap_or_else(|| LibraryFile {
        source: "symbols promoted from print sets (bim.symbol.promote, or an answer to a symbol question)".into(),
        plan_disciplines: Vec::new(),
        not_plan_views: Vec::new(),
        symbols: Vec::new(),
    });
    let sig = signature(&strokes);
    let mut sets = 1u32;
    if let Some(x) = file.symbols.iter_mut().find(|x| signature(&x.strokes) == sig && x.letters == e.letters) {
        if x.kind.eq_ignore_ascii_case(&e.kind) {
            x.sets = x.sets.saturating_add(1);
        } else {
            x.kind.clone_from(&e.kind);
            x.sets = 1;
        }
        sets = x.sets;
    } else {
        if file.symbols.len() >= MAX_ENTRIES {
            return Err(format!("{} holds {MAX_ENTRIES} symbols, the most it keeps", path.display()));
        }
        let mut name = cap(&e.name, MAX_TEXT);
        if file.symbols.iter().any(|x| x.name.eq_ignore_ascii_case(&name)) {
            name = format!("{name} {}", file.symbols.len() + 1);
        }
        // A promoted drawing of a kind the library draws already is how some sets draw it, not
        // its universal symbol.
        let universal = parse_library(SHIPPED, false).iter().all(|(s, _)| !s.kind.eq_ignore_ascii_case(&e.kind));
        file.symbols.push(LibSymbol { name, kind: e.kind.clone(), ifc: e.ifc.clone(), strokes, letters: e.letters.clone(), universal, sets: 1 });
    }
    crate::writefile::check(&path).map_err(|x| format!("{}: {x}", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|x| format!("{}: {x}", dir.display()))?;
    let text = serde_json::to_string_pretty(&file).map_err(|x| x.to_string())?;
    crate::writefile::write(&path, text).map_err(|x| format!("{}: {x}", path.display()))?;
    Ok((path, sets))
}

/// Mark `id` promoted, logged.
pub(crate) fn promoted(t: &mut Table, id: &str, who: &str, path: &std::path::Path) {
    let why = format!("promoted to the shared library ({})", path.display());
    t.change(id, who, &why, "promote", |e| e.promoted = true);
}

fn run_promote(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_param(p, "id").map(str::trim).filter(|i| !i.is_empty()).ok_or_else(|| bad(PROMOTE, "`id` is required"))?.to_string();
    let mut t = stored(s.doc()?);
    let e = t.get(&id).cloned().ok_or_else(|| bad(PROMOTE, format!("no entry {id} in the table")))?;
    let dir = library_folder(str_param(p, "folder")).ok_or_else(|| bad(PROMOTE, "no settings folder: give `folder`"))?;
    let (path, sets) = promote(&dir, &e).map_err(|x| bad(PROMOTE, x))?;
    promoted(&mut t, &id, &author_of(p), &path);
    save(s, &t)?;
    Ok(json!({"path": path.display().to_string(), "name": e.name, "kind": e.kind, "sets": sets}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(at: [f64; 2], k: f64) -> Vec<Stroke> {
        let [x, y] = at;
        vec![Stroke::Seg([x - k, y - k, x + k, y - k]), Stroke::Seg([x + k, y - k, x, y + k]), Stroke::Seg([x, y + k, x - k, y - k])]
    }

    #[test]
    fn a_drawings_signature_is_the_same_turned_moved_and_scaled() {
        let a = tri([0.0, 0.0], 1.0);
        let b: Vec<Stroke> = tri([0.0, 0.0], 3.0).into_iter().map(|s| s.turned(1)).collect();
        assert_eq!(signature(&a), signature(&b));
        let lib = library(Some(std::path::Path::new("/nonexistent-folder")));
        let duplex = lib.iter().find(|(s, _)| s.name == "duplex receptacle").map(|(s, _)| s.strokes.clone()).unwrap_or_default();
        assert_ne!(signature(&a), signature(&duplex));
    }

    #[test]
    fn the_shipped_library_reads_and_every_symbol_has_a_drawing_or_lettering() {
        let lib = parse_library(SHIPPED, false);
        assert!(lib.len() >= 8, "{lib:?}");
        for (s, _) in &lib {
            assert!(!s.kind.is_empty() && (!s.strokes.is_empty() || !s.letters.is_empty()), "{s:?}");
            let pieces: Vec<Piece> = s.strokes.iter().filter_map(|x| x.piece()).collect();
            if !pieces.is_empty() {
                assert!(Template::drawing(&s.name, "", &s.kind, &s.ifc, &pieces, PAPER_TEXT_IN, &s.letters).is_some(), "{s:?}");
            }
        }
        assert!(universal_of(&lib, "receptacle").is_some_and(|s| s.name == "duplex receptacle"));
        assert!(universal_of(&lib, "camera").is_none());
    }

    #[test]
    fn hostile_library_files_and_tables_are_bounded() {
        assert!(parse_library("{\"symbols\": [{\"name\": \"x\", \"kind\": \"k\", \"strokes\": [{\"round\": [0, 0, -1]}]}]}", false).is_empty());
        assert!(parse_library("not json", false).is_empty());
        let mut t = Table::default();
        for i in 0..(MAX_ENTRIES + 5) {
            t.add(Entry { id: format!("u{i}"), kind: "k".into(), ..Entry::default() }, "a", "b");
        }
        assert_eq!(t.entries.len(), MAX_ENTRIES);
        assert!(t.log.len() <= MAX_LOG);
        assert_eq!(signature(&[]), "");
        assert_eq!(signature(&[Stroke::Seg([f64::NAN, 0.0, 1.0, 1.0])]), "");
    }

    #[test]
    fn a_legend_that_redefines_a_library_drawing_turns_it_off_and_a_person_s_change_holds() {
        let lib = library(Some(std::path::Path::new("/nonexistent-folder")));
        let duplex = lib.iter().find(|(s, _)| s.name == "duplex receptacle").map(|(s, _)| s.strokes.clone()).unwrap_or_default();
        let mut t = Table::default();
        assert!(calibrate(&mut t, &lib, &[], &[]) >= lib.len());
        // Again: nothing more.
        assert_eq!(calibrate(&mut t, &lib, &[], &[]), 0);
        let row = |kind: &str, strokes: Vec<Stroke>| LegendDrawing {
            sheet: "E-0".into(),
            row: format!("{kind} ROW"),
            kind: kind.into(),
            ifc: String::new(),
            strokes,
            letters: Vec::new(),
            text_h: 1.0,
            upi: None,
            sheet_w: None,
            low_voltage: false,
        };
        // The duplex's drawing as this set's TV outlet; the data outlet drawn its own way.
        let legend = [
            row("tv_outlet", duplex.iter().map(|s| s.scaled(40.0)).collect()),
            row("data_outlet", tri([0.0, 0.0], 2.0).into_iter().chain([Stroke::Seg([-1.0, 0.0, 1.0, 0.0])]).collect()),
        ];
        calibrate(&mut t, &lib, &legend, &[]);
        let get = |t: &Table, id: &str| t.get(id).cloned().unwrap_or_default();
        assert!(get(&t, "library:duplex-receptacle").off, "{:?}", get(&t, "library:duplex-receptacle"));
        assert!(get(&t, "library:duplex-receptacle").off_reason.contains("another meaning"));
        assert!(get(&t, "library:data-outlet").off && get(&t, "library:data-outlet").off_reason.contains("its own way"));
        assert!(!get(&t, "library:quadruplex-receptacle").off);
        // The legend's data outlet is shown as the library's; the TV outlet keeps its own.
        assert_eq!(get(&t, "legend:e-0:data-outlet-row").shown_as, "data outlet");
        assert_eq!(get(&t, "legend:e-0:tv-outlet-row").shown_as, "");
        assert_eq!(get(&t, "legend:e-0:tv-outlet-row").status(), "documented");
        assert_eq!(get(&t, "library:quadruplex-receptacle").status(), "derived");
        // A person turns the duplex back on: the next calibration leaves it on.
        t.change("library:duplex-receptacle", "user", "it is a receptacle here too", "on", |e| e.off = false);
        calibrate(&mut t, &lib, &legend, &[]);
        assert!(!get(&t, "library:duplex-receptacle").off);
    }

    #[test]
    fn a_promoted_drawing_counts_its_sets_and_reads_back_as_learned() {
        let dir = std::env::temp_dir().join(format!("cadaddy-symlib-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let e = Entry {
            id: "answer:x".into(),
            name: "bow tie".into(),
            kind: "card_reader".into(),
            strokes: vec![Stroke::Seg([-5.0, -5.0, 5.0, 5.0]), Stroke::Seg([-5.0, 5.0, 5.0, -5.0]), Stroke::Seg([-5.0, -5.0, -5.0, 5.0])],
            text_h: 2.0,
            origin: "answer".into(),
            ..Entry::default()
        };
        let (path, sets) = promote(&dir, &e).unwrap_or_else(|x| panic!("{x}"));
        assert_eq!(sets, 1);
        assert_eq!(promote(&dir, &e).map(|(_, n)| n), Ok(2));
        let lib = library(Some(&dir));
        let got = lib.iter().find(|(s, learned)| *learned && s.kind == "card_reader");
        // In paper inches: its lettering 2 units high is 3/32".
        assert!(
            got.is_some_and(|(s, _)| s.sets == 2
                && s.universal
                && matches!(s.strokes.first(), Some(Stroke::Seg(v)) if (v[2] - 5.0 * PAPER_TEXT_IN / 2.0).abs() < 1e-9)),
            "{got:?}"
        );
        assert!(path.ends_with(LEARNED_FILE));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
