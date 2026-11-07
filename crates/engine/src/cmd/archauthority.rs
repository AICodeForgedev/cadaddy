//! When the electrical plans send their devices' locations and heights to the architecture's
//! drawings (Q6244): a general note of an electrical sheet such as
//!
//! ```text
//! 2. SEE ARCHITECTURAL PLANS AND INTERIOR ELEVATIONS FOR DEVICE LOCATIONS AND MOUNTING
//!    HEIGHTS. DO NOT SCALE THESE PLANS.
//! ```
//!
//! makes that sheet's device spots reference only: the architecture's enlarged plan of the room
//! (its symbols, the AFF notes on them, the dimension strings they are drawn at) decides where each
//! device goes and how high, before the electrical sheet's own notes and before any typical height.
//!
//! What a note sends where is rule data (`wordrules.json` `notes.defer_to_architecture`), never
//! a job's words. The pieces here are plain: reading the notes ([`read`]), pairing the
//! architecture's noted symbols with the devices one to one, the nearest pairs first ([`pair`]),
//! and telling whether two spots are on the same wall of a room ([`same_wall`]). A sheet whose
//! notes send its locations away, or call its layout reference only, gives no spot Validate
//! measures against ([`reference_only`]). An interior
//! elevation of the room draws its wall devices to scale: their faceplates and its wall
//! luminaires ([`elevation_items`]), each one's height over the floor line, laid along a wall of
//! the room by the devices it shows ([`register_way`]). Every scan is capped; nothing here panics.

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;

/// Texts of a sheet looked at for its notes, at most.
const MAX_TEXTS: usize = 40_000;
/// Numbered notes of a sheet read, at most; lines one note may run to.
const MAX_NOTES: usize = 400;
const MAX_NOTE_LINES: usize = 10;
/// A note's next line starts within this many text heights below the one before it, and within
/// this many to the left or right of the note's first line.
const LINE_GAP: f64 = 2.5;
const LINE_INDENT: f64 = 6.0;
/// The note kept as the reason, characters at most.
const MAX_REASON_CHARS: usize = 240;
/// Pairs looked at when pairing, at most.
const MAX_PAIRS: usize = 200_000;

/// What an electrical sheet's notes send to the architecture's drawings, and the note that says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Deferral {
    /// The devices' mounting heights.
    pub heights: bool,
    /// The devices' locations (the sheet's spots are reference only).
    pub locations: bool,
    /// The note that says so (the first one read), as printed.
    pub note: String,
}

impl Deferral {
    /// Whether the notes send anything.
    pub fn any(&self) -> bool {
        self.heights || self.locations
    }
}

/// Does a line start a numbered note (`1.`, `12)`)?
fn starts_note(s: &str) -> bool {
    let t = s.trim_start();
    let n = t.chars().take_while(char::is_ascii_digit).count();
    n > 0 && n <= 3 && t.chars().nth(n).is_some_and(|c| c == '.' || c == ')')
}

/// Upper case, every run of white space one space.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase()
}

/// The numbered notes of a sheet's texts, each with its lines joined (a note set as one text per
/// line): a numbered line, then each line just under the one before it and starting near it,
/// until the next numbered line. An MTEXT note is its own paragraph.
pub fn paragraphs(texts: &[TextItem]) -> Vec<String> {
    let flat = |t: &TextItem| t.rotation.sin().abs() < 0.05 && t.rotation.cos() > 0.0;
    let lines: Vec<&TextItem> = texts.iter().take(MAX_TEXTS).filter(|t| flat(t) && t.height > 0.0 && t.at.is_finite()).collect();
    let mut out = Vec::new();
    for first in lines.iter().filter(|t| starts_note(&t.value)).take(MAX_NOTES) {
        let h = first.height;
        let mut text = vec![first.value.as_str()];
        let mut cur: &TextItem = first;
        for _ in 1..MAX_NOTE_LINES {
            let below = lines
                .iter()
                .filter(|l| {
                    let dy = cur.at.y - l.at.y;
                    dy > 0.5 * h
                        && dy <= LINE_GAP * h.max(l.height)
                        && (l.at.x - first.at.x).abs() <= LINE_INDENT * h
                        && (l.height - h).abs() <= 0.25 * h
                })
                .min_by(|a, b| {
                    (cur.at.y - a.at.y).total_cmp(&(cur.at.y - b.at.y)).then((a.at.x - first.at.x).abs().total_cmp(&(b.at.x - first.at.x).abs()))
                });
            let Some(next) = below else { break };
            if starts_note(&next.value) {
                break;
            }
            text.push(next.value.as_str());
            cur = next;
        }
        out.push(text.iter().map(|t| t.trim()).collect::<Vec<_>>().join(" "));
    }
    out
}

/// What a sheet's notes (its texts) send to the architecture's drawings: every note a row of
/// `notes.defer_to_architecture` matches (`heights`, `locations` or `both`).
pub fn read(texts: &[TextItem]) -> Deferral {
    let mut out = Deferral::default();
    let rules = cadaddy_mepfix::wordrules::rules("notes.defer_to_architecture");
    for p in paragraphs(texts) {
        let u = squash(&p);
        let Some(rule) = rules.iter().find(|r| r.matches(|w| u.contains(&squash(w)))) else { continue };
        let (heights, locations) = match rule.value {
            "both" => (true, true),
            "heights" => (true, false),
            "locations" => (false, true),
            _ => (false, false),
        };
        if (heights || locations) && out.note.is_empty() {
            out.note = p.chars().take(MAX_REASON_CHARS).collect();
        }
        out.heights |= heights;
        out.locations |= locations;
    }
    out
}

/// Whether a sheet's notes make its device spots reference only: a note that sends its
/// locations to the architecture's drawings ([`read`]), or one that says its layout is for
/// reference only or its locations diagrammatic (`notes.reference_only`, a numbered note or a
/// line of its own). The note, as printed.
pub fn reference_only(texts: &[TextItem]) -> Option<String> {
    let d = read(texts);
    if d.locations {
        return Some(d.note);
    }
    let rules = cadaddy_mepfix::wordrules::rules("notes.reference_only");
    let lines = texts.iter().take(MAX_TEXTS).map(|t| t.value.clone());
    paragraphs(texts)
        .into_iter()
        .chain(lines)
        .find(|p| {
            let u = squash(p);
            rules.iter().any(|r| r.matches(|w| u.contains(&squash(w))))
        })
        .map(|p| p.chars().take(MAX_REASON_CHARS).collect())
}

/// The architecture's noted spots paired with devices, one to one: `ok(note, device)` says
/// whether a pair may be made at all; of the pairs within `reach` the nearest is made first, then
/// the nearest left of the rest. `(note index, device index)`.
pub fn pair(notes: &[Vec2], devices: &[Vec2], reach: f64, ok: impl Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    'all: for (i, n) in notes.iter().enumerate() {
        for (j, d) in devices.iter().enumerate() {
            if pairs.len() >= MAX_PAIRS {
                break 'all;
            }
            let dd = n.dist(*d);
            if dd.is_finite() && dd <= reach && ok(i, j) {
                pairs.push((dd, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let (mut note_used, mut dev_used) = (vec![false; notes.len()], vec![false; devices.len()]);
    let mut out = Vec::new();
    for (_, i, j) in pairs {
        if note_used.get(i).copied().unwrap_or(true) || dev_used.get(j).copied().unwrap_or(true) {
            continue;
        }
        if let Some(u) = note_used.get_mut(i) {
            *u = true;
        }
        if let Some(u) = dev_used.get_mut(j) {
            *u = true;
        }
        out.push((i, j));
    }
    out
}

/// The wall of a room's outline nearest `p`: its piece (two corners), and how far `p` is from it.
/// A traced outline steps in and out by a few inches where the wall's finish or a chase does:
/// pieces shorter than a fifth of the outline's longest are no wall of their own (unless every
/// piece is that short).
fn nearest_wall(p: Vec2, outline: &[Vec2]) -> Option<(usize, Vec2, Vec2, f64)> {
    let n = outline.len();
    if n < 2 {
        return None;
    }
    let pieces: Vec<(usize, Vec2, Vec2)> = (0..n).filter_map(|i| Some((i, *outline.get(i)?, *outline.get((i + 1) % n)?))).collect();
    let longest = pieces.iter().map(|(_, a, b)| a.dist(*b)).filter(|l| l.is_finite()).fold(0.0, f64::max);
    let near = |min: f64| {
        pieces
            .iter()
            .filter_map(|&(i, a, b)| {
                let ab = b - a;
                let len2 = ab.x * ab.x + ab.y * ab.y;
                if len2 <= 0.0 || !len2.is_finite() || len2.sqrt() < min {
                    return None;
                }
                let u = (((p - a).x * ab.x + (p - a).y * ab.y) / len2).clamp(0.0, 1.0);
                Some((i, a, b, p.dist(a + ab * u)))
            })
            .min_by(|x, y| x.3.total_cmp(&y.3))
    };
    near(longest / 5.0).or_else(|| near(0.0))
}

/// How far `p` is from the nearest wall of a room's outline (`None`: no outline).
pub fn wall_dist(p: Vec2, outline: &[Vec2]) -> Option<f64> {
    nearest_wall(p, outline).map(|w| w.3)
}

/// Whether two spots of a room may be on one wall (`outline`, the room's corners): when both are
/// within `reach` of a wall, the outline pieces nearest each run the same way and lie on one line
/// (within `tol`). A spot further than that from every wall can't be told: a traced outline is
/// rough and a view laid on a room a little off, so it may be on either.
pub fn same_wall(p: Vec2, q: Vec2, outline: &[Vec2], reach: f64, tol: f64) -> bool {
    let (Some((_, a0, a1, ad)), Some((_, b0, b1, bd))) = (nearest_wall(p, outline), nearest_wall(q, outline)) else { return true };
    if ad > reach || bd > reach {
        return true;
    }
    let (da, db) = (a1 - a0, b1 - b0);
    let (la, lb) = (da.x.hypot(da.y), db.x.hypot(db.y));
    if la <= 0.0 || lb <= 0.0 {
        return false;
    }
    let cross = (da.x * db.y - da.y * db.x) / (la * lb);
    if cross.abs() > 0.1 {
        return false;
    }
    // b's piece on a's line: its offset across it.
    let off = |pt: Vec2| ((pt - a0).x * da.y - (pt - a0).y * da.x).abs() / la;
    off(b0) <= tol && off(b1) <= tol
}

/// A device an interior elevation draws: how far along the view (inches at the building's
/// scale, from the view's left), how high its middle is over the floor line (inches), and whether
/// it is a wall luminaire (a bar a tag's leader points at) rather than a faceplate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevItem {
    pub along: f64,
    pub aff: f64,
    pub luminaire: bool,
    /// A receptacle's faceplate (its outlets' faces drawn in it).
    pub receptacle: bool,
}

/// A faceplate's sides (inches): the short one and the long one (a one- to two-gang plate, either
/// way up).
const PLATE_SHORT_IN: (f64, f64) = (2.0, 5.2);
const PLATE_LONG_IN: (f64, f64) = (3.8, 7.6);
/// A wall luminaire's bar (inches): its length and its depth on the elevation.
const BAR_LONG_IN: (f64, f64) = (10.0, 72.0);
const BAR_SHORT_IN: (f64, f64) = (0.75, 10.0);
/// Items of one elevation, at most.
const MAX_ITEMS: usize = 400;

/// The devices an interior elevation draws: of its rectangles (`rects`, `[x0, y0, x1, y1]` in view
/// units), the faceplate-sized ones, and the bars `tagged` says a tag's leader points at; `floor`
/// the floor line's y, `inch` view units per inch at the building's scale, `left` the view's left
/// edge. A faceplate with two of the closed shapes `faces` inside it at two places, each at least
/// a third of its width and a sixth of its height (its outlets, not a switch's toggle or its
/// screws), is a receptacle's. A rectangle drawn twice is one; one in the floor or past the ceiling
/// is none.
pub fn elevation_items(
    rects: &[[f64; 4]],
    faces: &[[f64; 4]],
    tagged: impl Fn(&[f64; 4]) -> bool,
    floor: f64,
    inch: f64,
    left: f64,
) -> Vec<ElevItem> {
    let mut out: Vec<ElevItem> = Vec::new();
    if !(inch.is_finite() && inch > 0.0 && floor.is_finite() && left.is_finite()) {
        return out;
    }
    let within = |v: f64, r: (f64, f64)| v >= r.0 && v <= r.1;
    for r in rects {
        if out.len() >= MAX_ITEMS {
            break;
        }
        let (w, h) = ((r[2] - r[0]).abs() / inch, (r[3] - r[1]).abs() / inch);
        if !(w.is_finite() && h.is_finite()) {
            continue;
        }
        let (short, long) = if w < h { (w, h) } else { (h, w) };
        let plate = within(short, PLATE_SHORT_IN) && within(long, PLATE_LONG_IN);
        let bar = !plate && w > h && within(long, BAR_LONG_IN) && within(short, BAR_SHORT_IN) && tagged(r);
        if !plate && !bar {
            continue;
        }
        let along = ((r[0] + r[2]) / 2.0 - left) / inch;
        let aff = ((r[1] + r[3]) / 2.0 - floor) / inch;
        if !(0.0..=240.0).contains(&aff) {
            continue;
        }
        if out.iter().any(|o| (o.along - along).abs() <= 0.5 && (o.aff - aff).abs() <= 0.5) {
            continue;
        }
        let (pw, ph) = ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
        let outlet = |f: &&[f64; 4]| {
            let (fw, fh) = (f[2] - f[0], f[3] - f[1]);
            f[0] >= r[0].min(r[2])
                && f[2] <= r[0].max(r[2])
                && f[1] >= r[1].min(r[3])
                && f[3] <= r[1].max(r[3])
                && fw >= pw.min(ph) / 3.0
                && fh >= pw.max(ph) / 6.0
                && fw < 0.8 * pw
        };
        // Two outlets at two places (a shape drawn twice is one).
        let mut at: Vec<Vec2> = Vec::new();
        for f in faces.iter().filter(outlet).take(MAX_ITEMS) {
            let c = Vec2::new((f[0] + f[2]) / 2.0, (f[1] + f[3]) / 2.0);
            if !at.iter().any(|o| o.dist(c) <= 0.1 * pw.max(ph)) {
                at.push(c);
            }
        }
        let receptacle = plate && at.len() >= 2;
        out.push(ElevItem { along, aff, luminaire: bar, receptacle });
    }
    out
}

/// How an elevation lies along a wall: the way it runs (`reversed`: its left is the wall's far
/// end), the shift (inches) from its along to the wall's, and the items paired with the wall's
/// devices, `(item, device)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Registration {
    pub reversed: bool,
    pub shift: f64,
    pub pairs: Vec<(usize, usize)>,
}

/// Lay an elevation's items (`items`: inches along it from its left end) on a wall's devices
/// (`wall`: inches along the wall, `length` long) one way (`reversed`: the view's left end on the
/// wall's end): of the shifts one item-device pair gives, the one most items land within `tol` of a
/// device on (one to one, nearest first), then the least spread, then the view's ends nearest the
/// wall's. `None` when fewer than `min` land.
pub fn register_way(items: &[f64], wall: &[f64], length: f64, tol: f64, min: usize, reversed: bool) -> Option<Registration> {
    if items.is_empty() || wall.is_empty() || !(tol.is_finite() && tol > 0.0) {
        return None;
    }
    let (items, wall) = (items.get(..items.len().min(MAX_ITEMS))?, wall.get(..wall.len().min(MAX_ITEMS))?);
    let dir = if reversed { -1.0 } else { 1.0 };
    let expected = if !length.is_finite() {
        0.0
    } else if reversed {
        length
    } else {
        0.0
    };
    let devs: Vec<Vec2> = wall.iter().map(|s| Vec2::new(*s, 0.0)).collect();
    let mut best: Option<(Registration, f64, f64)> = None;
    for u in items {
        for s in wall {
            let shift = s - dir * u;
            let placed: Vec<Vec2> = items.iter().map(|u| Vec2::new(dir * u + shift, 0.0)).collect();
            let pairs = pair(&placed, &devs, tol, |_, _| true);
            let spread = pairs.iter().filter_map(|(i, j)| Some((placed.get(*i)?.x - devs.get(*j)?.x).abs())).sum::<f64>();
            let off = (shift - expected).abs();
            let better = best.as_ref().is_none_or(|(b, bs, bo)| {
                pairs.len() > b.pairs.len() || (pairs.len() == b.pairs.len() && (spread < bs - 1e-9 || ((spread - bs).abs() <= 1e-9 && off < *bo)))
            });
            if pairs.len() >= min && better {
                best = Some((Registration { reversed, shift, pairs }, spread, off));
            }
        }
    }
    best.map(|b| b.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(value: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: value.into(), at: Vec2::new(x, y), height: 1.0, rotation: 0.0 }
    }

    #[test]
    fn a_general_note_over_several_lines_sends_locations_and_heights_to_the_architecture() {
        let texts = vec![
            t("GENERAL NOTES", 0.0, 20.0),
            t("1. SEE ARCHITECTURAL INTERIOR ELEVATIONS FOR EVERY LOCATION AND", 0.0, 18.0),
            t("MOUNTING HEIGHT. DO NOT SCALE THESE PLANS.", 0.4, 16.5),
            t("2. PROVIDE A SEPARATE CIRCUIT FOR THE WATER HEATER.", 0.0, 14.0),
        ];
        let p = paragraphs(&texts);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].ends_with("DO NOT SCALE THESE PLANS."), "{p:?}");
        let d = read(&texts);
        assert!(d.heights && d.locations, "{d:?}");
        assert!(d.note.starts_with("1. SEE ARCHITECTURAL"), "{d:?}");
    }

    #[test]
    fn a_keynote_sends_only_heights_and_an_unrelated_note_sends_nothing() {
        let d = read(&[t("4. GANG THESE DEVICES; MOUNT AT THE HEIGHT SHOWN ON THE ARCHITECTURAL PLANS.", 0.0, 0.0)]);
        assert!(d.heights && !d.locations, "{d:?}");
        let d = read(&[t("3. LOW VOLTAGE LAYOUT IS FOR REFERENCE ONLY.", 0.0, 0.0), t("5. MOUNT PANELS 6'-0\" TO TOP.", 0.0, -5.0)]);
        assert!(!d.any(), "{d:?}");
        // A plain sentence that isn't a numbered note isn't read.
        assert!(!read(&[t("SEE ARCHITECTURAL PLANS FOR LOCATIONS AND HEIGHTS", 0.0, 0.0)]).any());
    }

    #[test]
    fn pairs_are_made_nearest_first_one_to_one() {
        let notes = [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)];
        // Device 0 is nearest note 1, device 1 nearest note 0 after it: note 0 does not take the
        // device note 1 sits on just because it is read first.
        let devices = [Vec2::new(0.9, 0.0), Vec2::new(-1.5, 0.0)];
        assert_eq!(pair(&notes, &devices, 2.0, |_, _| true), vec![(1, 0), (0, 1)]);
        // Out of reach, or refused: no pair.
        assert!(pair(&notes, &devices, 0.05, |_, _| true).is_empty());
        assert_eq!(pair(&notes, &devices, 2.0, |i, _| i == 0), vec![(0, 0)]);
    }

    #[test]
    fn an_elevations_faceplates_and_tagged_bars_give_heights_over_the_floor() {
        // At 1/2" over 3/8" plans: 4/3 view units an inch. Floor at y 100, view's left at x 0.
        let k = 4.0 / 3.0;
        let rect =
            |x: f64, z: f64, w: f64, h: f64| [x * k - w * k / 2.0, 100.0 + z * k - h * k / 2.0, x * k + w * k / 2.0, 100.0 + z * k + h * k / 2.0];
        let rects = [
            rect(30.0, 18.0, 2.75, 4.5),
            rect(30.0, 18.0, 2.75, 4.5),   // drawn twice
            rect(60.0, 44.0, 4.5, 2.75),   // a plate on its side
            rect(90.0, 60.0, 30.0, 2.0),   // a bar a tag points at
            rect(140.0, 30.0, 30.0, 2.0),  // a shelf: no tag
            rect(200.0, 24.0, 20.0, 30.0), // a drawer
        ];
        let tagged = |r: &[f64; 4]| r[0] < 100.0 * k;
        // The first faceplate shows its two outlets.
        let face = |x: f64, z: f64| [x * k - 0.9 * k, 100.0 + z * k - 0.7 * k, x * k + 0.9 * k, 100.0 + z * k + 0.7 * k];
        let faces = [face(30.0, 17.0), face(30.0, 19.0)];
        let items = elevation_items(&rects, &faces, tagged, 100.0, k, 0.0);
        assert!(items[0].receptacle && !items[1].receptacle && !items[2].receptacle, "{items:?}");
        let got: Vec<(i64, i64, bool)> = items.iter().map(|i| (i.along.round() as i64, i.aff.round() as i64, i.luminaire)).collect();
        assert_eq!(got, [(30, 18, false), (60, 44, false), (90, 60, true)]);
    }

    #[test]
    fn an_elevation_lies_along_its_wall_whichever_way_it_runs() {
        // Its faceplates 30, 60 and 100 in from its left; the wall's devices (from its corner)
        // 140, 110 and 70: the view runs the other way, its left at the wall's 170.
        let way = |reversed| register_way(&[30.0, 60.0, 100.0], &[140.0, 110.0, 70.0, 20.0], 170.0, 6.0, 2, reversed);
        assert_eq!(way(false).map(|r| r.pairs.len()), Some(2), "{:?}", way(false));
        let reg = way(true).unwrap_or_else(|| panic!("no registration"));
        assert!(reg.reversed, "{reg:?}");
        assert!((reg.shift - 170.0).abs() < 1e-6, "{reg:?}");
        let mut pairs = reg.pairs.clone();
        pairs.sort_unstable();
        assert_eq!(pairs, [(0, 0), (1, 1), (2, 2)]);
        // One item alone never lays a view on a wall.
        assert!(register_way(&[30.0], &[140.0], 170.0, 6.0, 2, false).is_none());
        // Two faceplates fit both ways (the pairs swapped): each way gives its own shift.
        let fwd = register_way(&[30.0, 100.0], &[114.0, 44.0], 144.0, 6.0, 2, false).unwrap_or_else(|| panic!("no registration"));
        let rev = register_way(&[30.0, 100.0], &[114.0, 44.0], 144.0, 6.0, 2, true).unwrap_or_else(|| panic!("no registration"));
        assert!((fwd.shift - 14.0).abs() < 1e-6 && (rev.shift - 144.0).abs() < 1e-6, "{fwd:?} {rev:?}");
    }

    #[test]
    fn spots_on_one_wall_or_across_a_corner() {
        let room = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 20.0), Vec2::new(0.0, 20.0)];
        // Both by the south wall.
        assert!(same_wall(Vec2::new(2.0, 0.2), Vec2::new(6.0, 0.3), &room, 1.0, 0.5));
        // One by the south wall, one by the east: no.
        assert!(!same_wall(Vec2::new(9.0, 0.2), Vec2::new(9.8, 3.0), &room, 1.0, 0.5));
        // North and south walls run the same way but are not one line.
        assert!(!same_wall(Vec2::new(5.0, 0.2), Vec2::new(5.0, 19.8), &room, 1.0, 0.5));
        // One off every wall (a view laid a little off): it can't be told, so it may be.
        assert!(same_wall(Vec2::new(5.0, 0.2), Vec2::new(5.0, 2.0), &room, 1.0, 0.5));
        // A traced outline stepping in a few inches along its east wall: still one wall.
        let jog = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 8.0),
            Vec2::new(9.75, 8.0),
            Vec2::new(9.75, 9.0),
            Vec2::new(10.0, 9.0),
            Vec2::new(10.0, 20.0),
            Vec2::new(0.0, 20.0),
        ];
        assert!(same_wall(Vec2::new(9.6, 8.5), Vec2::new(9.9, 7.0), &jog, 1.0, 0.5));
        assert!(!same_wall(Vec2::new(9.6, 8.5), Vec2::new(8.0, 0.2), &jog, 1.0, 0.5));
    }
}
