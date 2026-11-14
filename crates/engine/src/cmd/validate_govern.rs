//! The document authority the job's own notes state, for the prescreen (Q6563): where a trade
//! sheet's general note sends its devices' locations or mounting heights to the architecture's
//! drawings (`SEE ARCHITECTURAL UNIT PLANS AND ELEVATIONS FOR DEVICE LOCATIONS AND HEIGHTS`,
//! [`super::archauthority::read`]), that sheet's spots and heights are reference only and the
//! architecture's enlarged unit plan of each typical room governs.
//!
//! What is read here is the governing documents themselves, never the model's record of them:
//!
//! - **the deferring sheets**: every sheet but the architecture's whose numbered notes send
//!   something to the architecture (what, and the note as printed);
//! - **the governing spots**: each typical room's unit type's plan views on the architecture's
//!   sheets: their device symbols (round shapes) and the AFF notes on them. A note goes to the
//!   symbol it labels: the one its leader ends on first, else the nearest one, one note to one
//!   symbol, the nearest pairs first (a switch's note set beside the receptacle under it is
//!   still the switch's when its leader says so). A symbol of the noted ones' size with no note
//!   is a spot too.
//! - **laid into the room** by the turn and shift its own lines measure on the room's model walls
//!   ([`super::validate_register::view_turn`], only when that is certain): a view whose turn can't
//!   be told gives no spot (said why);
//! - **paired one to one** with the room's wall elements read from a deferring sheet, the nearest
//!   pairs first, on the same wall first; a device ganged with a paired one (at its spot) shares
//!   it. The spot is where the element goes and the note its height: G-HEIGHT measures against
//!   the note before anything else, G-LAYOUT against the spot, and a noted spot no element took
//!   is reported by G-PRESENT.
//!
//! What a note names (a kind, an appliance) is read through the rules data (`kind_in`,
//! `notes.appliance_words`), never a job's words. Every scan is capped; nothing here panics.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::{Drawing, Entity, EntityKind, EntityStore};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

use super::archauthority::{self, Deferral};
use super::validate_height::{self as vh, Datum};
use super::validate_packet::{inside, intersect, norm_rect, point_in, r2, seg_dist};
use super::validate_prescreen::UnitOf;
use super::validate_register as vreg;

/// Sheets whose notes are read, at most.
const MAX_SHEETS: usize = 10_000;
/// Governing views read, at most (each typical unit type's few).
const MAX_VIEWS: usize = 400;
/// Views of one unit type looked at, at most.
const MAX_TYPE_VIEWS: usize = 40;
/// Objects of one view read, at most.
const MAX_VIEW_ENTITIES: usize = 60_000;
/// Texts of one view read, at most.
const MAX_VIEW_TEXTS: usize = 20_000;
/// Straight pieces of one view looked at for leaders, at most.
const MAX_LEADER_PIECES: usize = 40_000;
/// Notes and symbols of one view, at most.
const MAX_SPOTS: usize = 400;
/// Rooms looked at, at most.
const MAX_ROOMS: usize = 50_000;
/// A height note is a short note: at most this many characters.
const MAX_NOTE_CHARS: usize = 60;
/// A device symbol's radius, feet at the view's scale: from this to [`MAX_SYMBOL_FT`].
const MIN_SYMBOL_FT: f64 = 0.05;
const MAX_SYMBOL_FT: f64 = 1.0;
/// A note's symbol is within this of its leader's end, feet at the view's scale.
const LEADER_TIP_FT: f64 = 1.25;
/// With no leader to one: the symbol within this of the note.
const NOTE_SYMBOL_FT: f64 = 2.5;
/// A letter is about this share of its text height wide (a note's box along its line).
const CHAR_W: f64 = 0.7;
/// A leader starts within this many text heights of its note.
const LEADER_START_H: f64 = 1.5;
/// A label line (`MICROWAVE` over `65" AFF`) stands up to this many text heights over its note.
const LABEL_GAP_H: f64 = 2.5;
/// A governing spot in the room: inside it, or this close outside its outline (a wall device),
/// feet.
const IN_ROOM_FT: f64 = 1.0;
/// Two views showing a spot within this (feet) show one spot.
const SAME_SPOT_FT: f64 = 0.5;
/// An element and a governing spot further apart than this (feet) are not paired: the spot is
/// some other thing's.
pub(crate) const PAIR_FT: f64 = 6.0;
/// On one wall: both within this of a wall of the room (feet), on one line within [`WALL_TOL_FT`].
const WALL_FT: f64 = 1.5;
const WALL_TOL_FT: f64 = 0.5;
/// A device this close to a paired one on its wall (feet) is ganged with it: it shares its spot.
const GANG_FT: f64 = 0.8;
/// Loose spots listed, at most.
const MAX_LOOSE: usize = 5_000;

/// The sheet number of a sheet name (upper case, no view suffix).
pub(crate) fn sheet_key(s: &str) -> String {
    s.split('/').next().unwrap_or(s).trim().to_ascii_uppercase()
}

/// An AFF note on a governing plan, as the symbol it labels carries it.
#[derive(Clone, Debug)]
pub(crate) struct Note {
    pub inches: f64,
    /// The note (and its label line), as printed.
    pub text: String,
    pub datum: Datum,
    /// Its point on the sheet (model coordinates).
    pub at: [f64; 2],
    /// How it was tied to its symbol: `leader` or `nearest`.
    pub by: &'static str,
}

/// The governing spot of one element.
#[derive(Clone, Debug)]
pub(crate) struct Spot {
    /// Where the governing plan puts it, plan feet.
    pub at: [f64; 2],
    /// The governing sheet and view (`A-801 UNIT PLAN`).
    pub sheet: String,
    pub view: String,
    /// The symbol's point on its sheet.
    pub sheet_at: [f64; 2],
    pub note: Option<Note>,
    /// How far the element is from it, feet.
    pub off_ft: f64,
    /// The element's own sheet (the deferring one) and what its notes send away.
    pub from_sheet: String,
    pub defer: Deferral,
    /// Shared with the device it is ganged with.
    pub ganged: bool,
}

impl Spot {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "sheet": self.sheet,
            "view": self.view,
            "at": [r2(self.at[0]), r2(self.at[1])],
            "sheet_at": [r2(self.sheet_at[0]), r2(self.sheet_at[1])],
            "note": self.note.as_ref().map(|n| json!({"text": n.text, "in": r2(n.inches), "to": n.datum.name(), "by": n.by})),
            "off_in": r2(self.off_ft * 12.0),
            "reference_sheet": self.from_sheet,
            "reference_note": self.defer.note,
            "ganged": self.ganged,
        })
    }

    /// Why this plan governs, in words.
    pub(crate) fn why(&self) -> String {
        format!("{} sends its devices' {} to the architecture's drawings: \"{}\"", self.from_sheet, sends(&self.defer), self.defer.note)
    }
}

/// What a deferral sends, in words.
fn sends(d: &Deferral) -> &'static str {
    match (d.locations, d.heights) {
        (true, true) => "locations and heights",
        (true, false) => "locations",
        _ => "heights",
    }
}

/// A noted governing spot no element took.
#[derive(Clone, Debug)]
pub(crate) struct Loose {
    /// The room's area id (no stretch suffix).
    pub room: String,
    pub at: [f64; 2],
    pub sheet: String,
    pub view: String,
    pub sheet_at: [f64; 2],
    pub note: Note,
    /// The deferring sheets of the room's elements (what sends them here).
    pub from: Vec<String>,
}

/// An element the governing plans may place: a wall item read from a sheet.
pub(crate) struct Candidate<'a> {
    pub handle: &'a str,
    pub kind: &'a str,
    pub at: [f64; 2],
    /// The sheet it was read from (as its record names it).
    pub sheet: String,
}

/// A typical room in scope with the elements in it.
pub(crate) struct RoomIn<'a> {
    pub id: String,
    pub level: String,
    pub unit: &'a UnitOf,
    /// Its parts' outlines (feet) and their box.
    pub outlines: Vec<Vec<[f64; 2]>>,
    pub bbox: [f64; 4],
    /// Indexes into the candidates.
    pub members: Vec<usize>,
}

/// What the governing documents say, element by element.
#[derive(Clone, Debug, Default)]
pub(crate) struct Governing {
    /// Each deferring sheet (number) and what its notes send away.
    pub defers: BTreeMap<String, Deferral>,
    /// Element handle (upper case) → its governing spot.
    pub spots: HashMap<String, Spot>,
    /// Noted governing spots no element took.
    pub loose: Vec<Loose>,
    /// Each governing view laid into a room: how, and what it gave.
    pub views: Vec<Value>,
    /// Typical rooms with an element of a deferring sheet, and those a governing view was laid in.
    pub rooms: usize,
    pub rooms_laid: usize,
}

/// The sheets whose numbered notes send their devices' locations or heights to the
/// architecture's drawings: every sheet but the architecture's own (`texts`: the drawing's).
pub(crate) fn deferrals(sp: &BuildingSpec, texts: &[TextItem]) -> BTreeMap<String, Deferral> {
    let mut sheets: Vec<(String, [f64; 4])> = Vec::new();
    for l in sp.sheets.iter().take(MAX_SHEETS) {
        let key = sheet_key(&l.number);
        if key.is_empty() || is_architecture(l.discipline.as_deref(), &key) || sheets.iter().any(|(k, _)| *k == key) {
            continue;
        }
        if let Some(r) = l.rect.filter(|r| r.iter().all(|v| v.is_finite())).map(norm_rect) {
            sheets.push((key, r));
        }
    }
    let mut out = BTreeMap::new();
    if sheets.is_empty() {
        return out;
    }
    let mut per: Vec<Vec<TextItem>> = vec![Vec::new(); sheets.len()];
    for t in texts {
        if let Some(i) = sheets.iter().position(|(_, r)| inside(*r, [t.at.x, t.at.y]))
            && let Some(list) = per.get_mut(i)
        {
            list.push(t.clone());
        }
    }
    for ((key, _), list) in sheets.into_iter().zip(per) {
        let d = archauthority::read(&list);
        if d.any() {
            out.insert(key, d);
        }
    }
    out
}

/// Is a sheet the architecture's (its discipline, else its number's letter)?
fn is_architecture(discipline: Option<&str>, number: &str) -> bool {
    match discipline.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => d.eq_ignore_ascii_case("A"),
        None => cadaddy_mepfix::sheet_facts::discipline_of(number).eq_ignore_ascii_case("A"),
    }
}

/// The deferring sheet of an element's sheet name: the name itself, or its first word (`E-101
/// at 12, 40`).
pub(crate) fn deferring<'a>(defers: &'a BTreeMap<String, Deferral>, sheet: &str) -> Option<(&'a String, &'a Deferral)> {
    let whole = sheet_key(sheet);
    if let Some(x) = defers.get_key_value(&whole) {
        return Some(x);
    }
    let first = sheet.split_whitespace().next().map(|w| sheet_key(w.trim_end_matches([',', ';', ':', '.'])))?;
    defers.get_key_value(&first)
}

/// One plan view of a unit type on the architecture's sheets: its sheet, title and rectangle.
fn governing_views(unit: &UnitOf, level: &str, sp: &BuildingSpec) -> Vec<(String, String, [f64; 4])> {
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let floor = digits(level);
    unit.ty
        .get("views")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_TYPE_VIEWS)
        .filter(|v| {
            let levels: Vec<String> = v.get("levels").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(digits).collect();
            levels.is_empty() || levels.iter().any(|l| !l.is_empty() && *l == floor)
        })
        .filter_map(|v| {
            let sheet = v.get("sheet")?.as_str()?.trim().to_string();
            let title = v.get("title")?.as_str()?.trim().to_string();
            let up = title.to_ascii_uppercase();
            if [" WALL", "ELEVATION", "SECTION", "DETAIL"].iter().any(|w| up.contains(w)) {
                return None;
            }
            let key = sheet_key(&sheet);
            let disc = sp.sheets.iter().find(|l| sheet_key(&l.number) == key).and_then(|l| l.discipline.clone());
            if !is_architecture(disc.as_deref(), &key) {
                return None;
            }
            let mut r = rect4(v.get("rect")?)?;
            if let Some(sr) = v.get("sheet_rect").and_then(rect4) {
                r = intersect(r, sr)?;
            }
            Some((sheet, title, norm_rect(r)))
        })
        .collect()
}

fn rect4(v: &Value) -> Option<[f64; 4]> {
    let a = v.as_array()?;
    let r = [a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?, a.get(3)?.as_f64()?];
    r.iter().all(|x| x.is_finite()).then_some(r)
}

/// A governing view's symbols, each with the note it carries (sheet coordinates).
#[derive(Clone, Debug, Default)]
pub(crate) struct ViewSpots {
    /// The symbol's centre and its note.
    pub spots: Vec<([f64; 2], Option<Note>)>,
    /// Height notes read, and those tied to no symbol.
    pub notes: usize,
    pub untied: usize,
}

/// Does a text read as a short mounting-height note (`48" AFF`, `+18"`), not a numbered note?
fn height_note(t: &TextItem) -> Option<f64> {
    let v = t.value.trim();
    if v.is_empty() || v.chars().count() > MAX_NOTE_CHARS {
        return None;
    }
    let numbered = v.chars().next().is_some_and(|c| c.is_ascii_digit()) && v.contains('.') && !v.contains('"');
    if numbered {
        return None;
    }
    vh::aff_inches(v).filter(|z| z.is_finite() && *z >= 6.0 && *z <= 240.0)
}

/// The distance from `p` to a text's line of lettering (its anchor along its direction).
fn to_text(t: &TextItem, p: [f64; 2]) -> f64 {
    let h = t.height.max(1e-9);
    let dir = Vec2::from_angle(t.rotation);
    let end = t.at + dir * (CHAR_W * h * t.value.chars().count() as f64);
    seg_dist(p, [t.at.x, t.at.y], [end.x, end.y]).0
}

/// Read one governing view: its round symbols and the height notes on them. `ents`: its objects;
/// `texts`: its texts; `upf`: sheet units a foot at its scale.
pub(crate) fn read_view(ents: Vec<Entity>, texts: &[&TextItem], upf: f64) -> ViewSpots {
    let mut out = ViewSpots::default();
    if !(upf.is_finite() && upf > 0.0) {
        return out;
    }
    // Straight pieces (a leader may be a line or a short open polyline, end to end).
    let mut segs: Vec<[[f64; 2]; 2]> = Vec::new();
    for e in ents.iter() {
        if segs.len() >= MAX_LEADER_PIECES {
            break;
        }
        match &e.kind {
            EntityKind::Line(l) => segs.push([[l.a.x, l.a.y], [l.b.x, l.b.y]]),
            EntityKind::LwPolyline(pl) if !pl.closed && pl.vertices.len() <= 8 => {
                segs.extend(pl.vertices.windows(2).filter_map(|w| Some([[w.first()?.p.x, w.first()?.p.y], [w.get(1)?.p.x, w.get(1)?.p.y]])));
                if pl.vertices.len() > 2
                    && let (Some(a), Some(b)) = (pl.vertices.first(), pl.vertices.last())
                {
                    segs.push([[a.p.x, a.p.y], [b.p.x, b.p.y]]);
                }
            }
            _ => {}
        }
    }
    let mut store = EntityStore::new();
    store.push_many(ents);
    let mini = Drawing { model: store, ..Drawing::default() };
    let rounds: Vec<([f64; 2], f64)> = cadaddy_mepfix::sheet_reader::round_shapes(&mini)
        .into_iter()
        .filter(|(_, r)| *r >= MIN_SYMBOL_FT * upf && *r <= MAX_SYMBOL_FT * upf)
        .take(MAX_SPOTS * 4)
        .collect();
    let notes: Vec<(&TextItem, f64)> = texts.iter().filter_map(|t| Some((*t, height_note(t)?))).take(MAX_SPOTS).collect();
    out.notes = notes.len();
    if rounds.is_empty() || notes.is_empty() {
        out.untied = notes.len();
        return out;
    }
    // Each note's label line just over it (`MICROWAVE` over `65" AFF`): its words go with it.
    let label_of = |t: &TextItem| -> Option<&TextItem> {
        let h = t.height.max(1e-9);
        texts
            .iter()
            .copied()
            .filter(|o| {
                let dy = o.at.y - t.at.y;
                dy > 0.5 * h && dy <= LABEL_GAP_H * h && (o.at.x - t.at.x).abs() <= 1.5 * h && (o.height - h).abs() <= 0.25 * h
            })
            .min_by(|a, b| (a.at.y - t.at.y).total_cmp(&(b.at.y - t.at.y)))
    };
    // Candidate (note, symbol) ties: by leader (0), else nearest (1).
    let mut cands: Vec<(u8, f64, usize, usize)> = Vec::new();
    for (ni, (t, _)) in notes.iter().enumerate() {
        let h = t.height.max(1e-9);
        let label = label_of(t);
        let near_note = |p: [f64; 2]| to_text(t, p) <= LEADER_START_H * h || label.is_some_and(|l| to_text(l, p) <= LEADER_START_H * h);
        // Each leader's far end; a leader's end lands on its symbol.
        let tips: Vec<[f64; 2]> = segs
            .iter()
            .filter_map(|s| {
                let (a, b) = (s[0], s[1]);
                if (a[0] - b[0]).hypot(a[1] - b[1]) < 2.0 * h {
                    return None;
                }
                match (near_note(a), near_note(b)) {
                    (true, false) => Some(b),
                    (false, true) => Some(a),
                    _ => None,
                }
            })
            .collect();
        let mut by_leader = false;
        for tip in &tips {
            for (ri, (c, r)) in rounds.iter().enumerate() {
                let d = (c[0] - tip[0]).hypot(c[1] - tip[1]);
                if d <= LEADER_TIP_FT * upf + r {
                    cands.push((0, d, ni, ri));
                    by_leader = true;
                }
            }
        }
        if by_leader {
            continue;
        }
        for (ri, (c, _)) in rounds.iter().enumerate() {
            let d = to_text(t, *c);
            if d <= NOTE_SYMBOL_FT * upf {
                cands.push((1, d, ni, ri));
            }
        }
    }
    cands.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)).then(a.3.cmp(&b.3)));
    let mut note_of: BTreeMap<usize, (usize, u8)> = BTreeMap::new();
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    for (how, _, ni, ri) in cands {
        if note_of.values().any(|(n, _)| *n == ni) || taken.contains(&ri) {
            continue;
        }
        taken.insert(ri);
        note_of.insert(ri, (ni, how));
    }
    out.untied = notes.len().saturating_sub(note_of.len());
    // The noted symbols, then the others of their size standing alone (not a ring of a burner
    // or a drain, not a row of studs).
    let sizes: Vec<f64> = note_of.keys().filter_map(|i| rounds.get(*i).map(|r| r.1)).collect();
    let lo = sizes.iter().copied().fold(f64::INFINITY, f64::min) * 0.8;
    let hi = sizes.iter().copied().fold(0.0, f64::max) * 1.25;
    let lone = |c: [f64; 2], r: f64| {
        !rounds.iter().any(|(o, s)| {
            let d = (o[0] - c[0]).hypot(o[1] - c[1]);
            d > 1e-9 && (d <= 0.3 * r.max(*s) || ((s - r).abs() <= 0.2 * r && d <= 2.5 * r))
        })
    };
    for (ri, (c, r)) in rounds.iter().enumerate() {
        if out.spots.len() >= MAX_SPOTS {
            break;
        }
        match note_of.get(&ri) {
            Some((ni, how)) => {
                let Some((t, z)) = notes.get(*ni) else { continue };
                let label = label_of(t).map(|l| format!("{} ", l.value.trim())).unwrap_or_default();
                let text = format!("{label}{}", t.value.trim());
                out.spots.push((
                    *c,
                    Some(Note { inches: *z, datum: Datum::of(&text), text, at: [t.at.x, t.at.y], by: if *how == 0 { "leader" } else { "nearest" } }),
                ));
            }
            None if *r >= lo && *r <= hi && lone(*c, *r) => {
                // A ring round a noted symbol (its second circle) is that symbol.
                let twin = note_of.keys().filter_map(|i| rounds.get(*i)).any(|(o, _)| (o[0] - c[0]).hypot(o[1] - c[1]) <= 1.5 * r);
                if !twin {
                    out.spots.push((*c, None));
                }
            }
            None => {}
        }
    }
    out
}

/// Is a plan point in the room (inside a part, or within [`IN_ROOM_FT`] of its outline)?
fn in_room(outlines: &[Vec<[f64; 2]>], p: [f64; 2]) -> bool {
    outlines.iter().any(|o| {
        if o.len() < 3 {
            return false;
        }
        if point_in(o, p) {
            return true;
        }
        let n = o.len();
        (0..n).any(|i| match (o.get(i), o.get((i + 1) % n)) {
            (Some(a), Some(b)) => seg_dist(p, *a, *b).0 <= IN_ROOM_FT,
            _ => false,
        })
    })
}

/// May a note's words go to an element of `kind`: a note naming a kind only to that kind, a
/// note naming an appliance only to a receptacle (its connection), any other to any.
fn note_fits(note: Option<&Note>, kind: &str) -> bool {
    let Some(n) = note else { return true };
    if let Some(k) = vh::kind_in(&n.text) {
        let kind_words = kind.replace('_', " ");
        return cadaddy_walk3d::validate_kinds::kind_matches(k, kind) || kind_words.contains(&k.replace('_', " "));
    }
    let up = format!(
        " {} ",
        n.text.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
    );
    let appliance = cadaddy_mepfix::wordrules::words("notes.appliance_words").iter().any(|w| up.contains(&format!(" {} ", w.to_ascii_uppercase())));
    !appliance || kind.to_ascii_lowercase().contains("recept")
}

impl Governing {
    /// Read the governing documents for the rooms in scope. `texts`: the drawing's; `defers`:
    /// [`deferrals`]; `measure(level, view, room box)`: how a view lies on a room by the model's
    /// walls.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn of(
        d: &Drawing,
        sp: &BuildingSpec,
        upf: f64,
        texts: &[TextItem],
        defers: BTreeMap<String, Deferral>,
        cands: &[Candidate],
        rooms: &[RoomIn],
        measure: &dyn Fn(&str, [f64; 4], [f64; 4]) -> std::result::Result<vreg::Turned, String>,
    ) -> Governing {
        let mut out = Governing { defers, ..Governing::default() };
        if out.defers.is_empty() || !(upf.is_finite() && upf > 0.0) {
            return out;
        }
        // The rooms holding an element of a deferring sheet, and their governing views.
        let deferred = |i: &usize| cands.get(*i).is_some_and(|c| deferring(&out.defers, &c.sheet).is_some());
        let mut wanted: Vec<(&RoomIn, Vec<(String, String, [f64; 4])>)> = Vec::new();
        for r in rooms.iter().take(MAX_ROOMS) {
            if !r.members.iter().any(deferred) {
                continue;
            }
            out.rooms += 1;
            let views = governing_views(r.unit, &r.level, sp);
            if !views.is_empty() {
                wanted.push((r, views));
            }
        }
        if wanted.is_empty() {
            return out;
        }
        // Each view read once: its objects in one pass over the drawing.
        let mut rects: Vec<[f64; 4]> = Vec::new();
        for (_, vs) in &wanted {
            for (_, _, r) in vs {
                if rects.len() < MAX_VIEWS && !rects.iter().any(|x| x.map(f64::to_bits) == r.map(f64::to_bits)) {
                    rects.push(*r);
                }
            }
        }
        let mut ents: Vec<Vec<Entity>> = vec![Vec::new(); rects.len()];
        for e in d.model.iter().take(super::validate::MAX_ENTITIES) {
            if matches!(e.kind, EntityKind::Text(_) | EntityKind::MText(_) | EntityKind::Insert(_)) {
                continue;
            }
            let b = cadaddy_doc::entity_bounds(d, e, 0);
            if b.is_empty() {
                continue;
            }
            let c = b.center();
            for (i, r) in rects.iter().enumerate() {
                if inside(*r, [c.x, c.y])
                    && let Some(list) = ents.get_mut(i).filter(|l| l.len() < MAX_VIEW_ENTITIES)
                {
                    list.push((**e).clone());
                }
            }
        }
        let read: Vec<ViewSpots> = rects
            .iter()
            .zip(ents)
            .map(|(r, list)| {
                let mine: Vec<&TextItem> = texts.iter().filter(|t| inside(*r, [t.at.x, t.at.y])).take(MAX_VIEW_TEXTS).collect();
                read_view(list, &mine, upf)
            })
            .collect();
        for (room, views) in wanted {
            let mid = [(room.bbox[0] + room.bbox[2]) / 2.0, (room.bbox[1] + room.bbox[3]) / 2.0];
            // The room's spots, each view laid by its own lines on the room's walls.
            let mut spots: Vec<([f64; 2], [f64; 2], Option<Note>, String, String)> = Vec::new();
            let mut laid_any = false;
            for (sheet, title, r) in &views {
                let Some(vs) = rects.iter().position(|x| x.map(f64::to_bits) == r.map(f64::to_bits)).and_then(|i| read.get(i)) else { continue };
                let view = format!("{sheet} {title}");
                let mut info = json!({"room": room.id, "view": view, "symbols": vs.spots.len(), "notes": vs.notes, "notes_untied": vs.untied});
                if vs.spots.is_empty() {
                    info["why"] = json!("no device symbol with a height note on it read in the view");
                    push_view(&mut out.views, info);
                    continue;
                }
                let laid = match measure(&room.level, *r, room.bbox) {
                    Ok(t) if t.certain => t.best,
                    Ok(t) => {
                        info["why"] = json!(format!("its turn on the room's walls is not certain: {}", t.why.unwrap_or_default()));
                        push_view(&mut out.views, info);
                        continue;
                    }
                    Err(why) => {
                        info["why"] = json!(format!("its lines can't be laid on the room's walls: {why}"));
                        push_view(&mut out.views, info);
                        continue;
                    }
                };
                laid_any = true;
                let vmid = [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0];
                let mut kept = 0usize;
                for (p, note) in &vs.spots {
                    let q = laid.to_plan([(p[0] - vmid[0]) / upf, (p[1] - vmid[1]) / upf]);
                    let at = [mid[0] + q[0], mid[1] + q[1]];
                    if !in_room(&room.outlines, at) || spots.iter().any(|s| (s.0[0] - at[0]).hypot(s.0[1] - at[1]) <= SAME_SPOT_FT) {
                        continue;
                    }
                    spots.push((at, *p, note.clone(), sheet.clone(), view.clone()));
                    kept += 1;
                }
                info["laid"] = json!({"rotation_deg": r2(laid.rotation_deg), "mirrored": laid.mirrored, "offset_ft": [r2(laid.offset_ft[0]), r2(laid.offset_ft[1])]});
                info["in_room"] = json!(kept);
                push_view(&mut out.views, info);
            }
            if !laid_any || spots.is_empty() {
                continue;
            }
            out.rooms_laid += 1;
            // The room's elements of deferring sheets, paired one to one with its spots.
            let members: Vec<usize> = room.members.iter().copied().filter(deferred).collect();
            let outline: Vec<Vec2> = room.outlines.first().map(|o| o.iter().map(|p| Vec2::new(p[0], p[1])).collect()).unwrap_or_default();
            let spot_v: Vec<Vec2> = spots.iter().map(|s| Vec2::new(s.0[0], s.0[1])).collect();
            let mem_v: Vec<Vec2> =
                members.iter().map(|i| cands.get(*i).map_or(Vec2::new(f64::NAN, f64::NAN), |c| Vec2::new(c.at[0], c.at[1]))).collect();
            let kind_of = |j: usize| members.get(j).and_then(|i| cands.get(*i)).map_or("", |c| c.kind);
            let fits = |i: usize, j: usize| note_fits(spots.get(i).and_then(|s| s.2.as_ref()), kind_of(j));
            let mut paired: Vec<(usize, usize)> = Vec::new();
            for same_wall in [true, false] {
                let (spot_used, mem_used): (BTreeSet<usize>, BTreeSet<usize>) =
                    (paired.iter().map(|p| p.0).collect(), paired.iter().map(|p| p.1).collect());
                let ok = |i: usize, j: usize| {
                    !spot_used.contains(&i)
                        && !mem_used.contains(&j)
                        && fits(i, j)
                        && (!same_wall
                            || match (spot_v.get(i), mem_v.get(j)) {
                                (Some(p), Some(q)) => archauthority::same_wall(*p, *q, &outline, WALL_FT, WALL_TOL_FT),
                                _ => false,
                            })
                };
                paired.extend(archauthority::pair(&spot_v, &mem_v, PAIR_FT, ok));
            }
            // A device ganged with a paired one shares its spot.
            let mut ganged: Vec<(usize, usize)> = Vec::new();
            for (j, q) in mem_v.iter().enumerate() {
                if paired.iter().any(|p| p.1 == j) {
                    continue;
                }
                let mate = paired
                    .iter()
                    .filter_map(|(i, k)| Some((*i, mem_v.get(*k)?.dist(*q))))
                    .filter(|(_, d)| *d <= GANG_FT)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((i, _)) = mate {
                    ganged.push((i, j));
                }
            }
            for (i, j, gang) in paired.iter().map(|(i, j)| (*i, *j, false)).chain(ganged.iter().map(|(i, j)| (*i, *j, true))) {
                let (Some(s), Some(c)) = (spots.get(i), members.get(j).and_then(|k| cands.get(*k))) else { continue };
                let Some((from, defer)) = deferring(&out.defers, &c.sheet) else { continue };
                out.spots.insert(
                    c.handle.to_ascii_uppercase(),
                    Spot {
                        at: s.0,
                        sheet: s.3.clone(),
                        view: s.4.clone(),
                        sheet_at: s.1,
                        note: s.2.clone(),
                        off_ft: (c.at[0] - s.0[0]).hypot(c.at[1] - s.0[1]),
                        from_sheet: from.clone(),
                        defer: defer.clone(),
                        ganged: gang,
                    },
                );
            }
            // The noted spots no element took: what the governing plan shows that the room lacks.
            let from: Vec<String> = members
                .iter()
                .filter_map(|i| cands.get(*i).and_then(|c| deferring(&out.defers, &c.sheet)).map(|(k, _)| k.clone()))
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect();
            for (i, s) in spots.iter().enumerate() {
                if paired.iter().any(|p| p.0 == i) || out.loose.len() >= MAX_LOOSE {
                    continue;
                }
                if let Some(n) = &s.2 {
                    out.loose.push(Loose {
                        room: room.id.clone(),
                        at: s.0,
                        sheet: s.3.clone(),
                        view: s.4.clone(),
                        sheet_at: s.1,
                        note: n.clone(),
                        from: from.clone(),
                    });
                }
            }
        }
        out
    }

    /// The governing spot of an element.
    pub(crate) fn spot(&self, handle: &str) -> Option<&Spot> {
        self.spots.get(&handle.to_ascii_uppercase())
    }

    /// The height the governing plan's note gives an element (its sheet sends heights there).
    pub(crate) fn height(&self, handle: &str) -> Option<vh::Found> {
        let s = self.spot(handle).filter(|s| s.defer.heights)?;
        let n = s.note.as_ref()?;
        Some(vh::Found {
            inches: n.inches,
            from: "plan_note",
            what: format!("AFF note \"{}\" on the governing plan {} ({}; tied by its {})", n.text, s.view, s.why(), n.by),
            sheet: Some(s.sheet.clone()),
            at: Some(n.at),
            text: Some(n.text.clone()),
            datum: n.datum,
        })
    }

    /// The deferral of an element's sheet (`sheet`: as its record names it).
    pub(crate) fn defer_of(&self, sheet: Option<&str>) -> Option<(&String, &Deferral)> {
        deferring(&self.defers, sheet?)
    }

    /// What was read, for the prescreen's reply.
    pub(crate) fn to_json(&self) -> Value {
        const LISTED: usize = 40;
        json!({
            "deferring_sheets": self.defers.iter().map(|(k, d)| (k.clone(), json!({"sends": sends(d), "note": d.note}))).collect::<serde_json::Map<String, Value>>(),
            "rooms": self.rooms,
            "rooms_laid": self.rooms_laid,
            "elements_placed": self.spots.len(),
            "elements_off": self.spots.values().filter(|s| s.off_ft * 12.0 > 6.0).count(),
            "noted_spots_untaken": self.loose.len(),
            "views": self.views.iter().take(LISTED).cloned().collect::<Vec<_>>(),
        })
    }
}

fn push_view(list: &mut Vec<Value>, v: Value) {
    if list.len() < MAX_VIEWS {
        list.push(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: 4.0, rotation: 0.0 }
    }

    fn ent(kind: EntityKind) -> Entity {
        Entity::new(cadaddy_doc::Handle(1), kind)
    }

    fn circle(x: f64, y: f64) -> Entity {
        ent(EntityKind::Circle(cadaddy_doc::Circle { center: cadaddy_geom::Vec3::new(x, y, 0.0), radius: 3.0 }))
    }

    fn line(a: [f64; 2], b: [f64; 2]) -> Entity {
        ent(EntityKind::Line(cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(a[0], a[1], 0.0), b: cadaddy_geom::Vec3::new(b[0], b[1], 0.0) }))
    }

    #[test]
    fn a_note_goes_to_the_symbol_its_leader_ends_on_not_the_nearest() {
        // Two symbols 18 in apart; the switch's 48" note sits nearer the receptacle, its leader
        // ending on the switch; the receptacle's 18" note further off, its leader on it.
        let ents = vec![circle(36.0, 6.0), circle(54.0, 6.0), line([55.0, 26.0], [36.0, 9.0]), line([72.0, 38.0], [54.0, 9.0])];
        let t = [text("48\" AFF", 55.0, 26.0), text("18\" AFF", 72.0, 38.0)];
        let refs: Vec<&TextItem> = t.iter().collect();
        let v = read_view(ents.clone(), &refs, 12.0);
        let at = |x: f64| v.spots.iter().find(|s| (s.0[0] - x).abs() < 1e-6).and_then(|s| s.1.clone()).map(|n| (n.inches, n.by));
        assert_eq!(at(36.0), Some((48.0, "leader")), "{v:?}");
        assert_eq!(at(54.0), Some((18.0, "leader")), "{v:?}");
        // With no leaders, the nearest pairs first: the 48" note is nearest the receptacle.
        let bare = read_view(ents.into_iter().take(2).collect(), &refs, 12.0);
        let at = |x: f64| bare.spots.iter().find(|s| (s.0[0] - x).abs() < 1e-6).and_then(|s| s.1.clone()).map(|n| n.inches);
        assert_eq!(at(54.0), Some(48.0), "{bare:?}");
    }

    #[test]
    fn a_note_naming_an_appliance_goes_to_a_receptacle_only() {
        let n = Note { inches: 65.0, text: "MICROWAVE 65\" AFF".into(), datum: Datum::Centre, at: [0.0, 0.0], by: "leader" };
        assert!(note_fits(Some(&n), "receptacle"));
        assert!(!note_fits(Some(&n), "data_outlet"));
        assert!(note_fits(None, "data_outlet"));
    }

    #[test]
    fn an_element_sheet_finds_its_deferral_by_number() {
        let mut m = BTreeMap::new();
        m.insert("E-101".to_string(), Deferral { heights: true, locations: true, note: "1. SEE ARCH".into() });
        assert!(deferring(&m, "e-101").is_some());
        assert!(deferring(&m, "E-101 at 12, 40").is_some());
        assert!(deferring(&m, "E-102").is_none());
    }
}
