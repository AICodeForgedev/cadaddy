//! Luminaires built into casework or millwork, laid out by the architecture's casework details
//! (Q6354).
//!
//! An enlarged electrical plan draws such a luminaire only as the junction box it connects at,
//! tagged with its type and its circuit and switch leg (the device census builds it as that
//! luminaire, [`super::readers::ReadDevice::connection`]). The box is diagrammatic: where the
//! light goes along the counter, the bar or the shelf and how high is the architect's, drawn on
//! the casework or millwork detail (an elevation or section of the casework): the fixture called
//! out by its type mark, a dimension string along the casework from its end through each
//! fixture, a mounting height note (`3'-6" AFF`), or notes giving the spacing (`2'-0" O.C.`) and
//! the distance from the casework's end (`6" FROM END`).
//!
//! - **Reading** ([`read_detail`]): on an architectural or interiors sheet, the texts naming the
//!   type (the mark alone, as its tag, or with words that call out a light:
//!   `casework.callout_words`) are the fixture's callouts; a leader starting at one ends on the
//!   fixture. Round them: the mounting height note nearest a callout, a spacing note
//!   (`casework.spacing_words`), an end distance note (`casework.end_words`), and the dimension
//!   chain whose inner witness lines stand at the fixtures (its first and last at the casework's
//!   ends): each fixture's distance from the end, and the run's length.
//! - **Laying out** ([`lay_out`]): the casework luminaires of one type that stand together on a
//!   level (one run of casework) take the detail's height, and are set along the line their boxes
//!   run on, centred on the boxes, at the detail's distances (else its spacing). Each keeps its box's
//!   spot and says what moved it and why (`drawn_at`, `moved`: the field model's correction is
//!   derived from the detail's dimensions, never passed off as drawn). A type no detail
//!   dimensions stays at its box, and says so.
//!
//! What the words are is rules data (`wordrules.json`), never a job's. Every scan is capped;
//! nothing here panics.

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use std::collections::BTreeMap;

/// A detail reaches this many of its callouts' text heights round them (its notes and its
/// dimension strings are drawn within it).
pub(crate) const REGION_H: f64 = 40.0;
/// A note within this many text heights of a callout is that fixture's.
const NOTE_REACH_H: f64 = 12.0;
/// A leader starts within this many text heights of its callout.
const LEADER_START_H: f64 = 1.5;
/// A leader is at most this many text heights long.
const LEADER_MAX_H: f64 = 30.0;
/// A witness line stands at a fixture when it is within this many text heights of it along the
/// dimension line.
const WITNESS_AT_H: f64 = 2.0;
/// Equal stretches of a chain differ by at most this (inches).
const EQUAL_IN: f64 = 0.5;
/// Most callouts, texts, segments and dimension strings looked at.
const MAX_CALLOUTS: usize = 200;
const MAX_TEXTS: usize = 200_000;
const MAX_SEGS: usize = 200_000;
const MAX_STRINGS: usize = 20_000;
/// Most luminaires laid out in one call.
const MAX_LUMINAIRES: usize = 20_000;
/// Casework luminaires of one type within this (feet) of one another stand on one run, when the
/// detail gives no run length.
const RUN_REACH_FT: f64 = 10.0;
/// A mounting height note is believed up to this (inches).
const MAX_HEIGHT_IN: f64 = 240.0;
/// A spacing or end distance is believed up to this (inches: 50 ft).
const MAX_LENGTH_IN: f64 = 600.0;

/// What one casework detail says of a luminaire type.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CaseworkRead {
    /// The sheet the detail is on and the type it calls out.
    pub sheet: String,
    pub mark: String,
    /// Its mounting height over the floor (inches) and the note giving it.
    pub height: Option<(f64, String)>,
    /// Each fixture's distance from the casework's end (inches, in order), as its dimension
    /// string gives it; and the run's whole length.
    pub offsets_in: Vec<f64>,
    pub run_in: Option<f64>,
    /// The fixtures' spacing and their first one's distance from the end (inches), and where
    /// each comes from (a note's words or the dimension string's values).
    pub spacing: Option<(f64, String)>,
    pub end: Option<(f64, String)>,
}

impl CaseworkRead {
    /// Whether the detail says anything the layout can use.
    pub fn any(&self) -> bool {
        self.height.is_some() || self.offsets_in.len() >= 2 || self.spacing.is_some()
    }
}

/// The words of a text, split at spaces, slashes, commas and brackets.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '/' | ',' | '(' | ')' | '[' | ']' | ':' | ';')).filter(|w| !w.is_empty())
}

/// Whether `text` calls out luminaire type `mark`: the mark alone (its tag), or the mark among
/// words that call out a light (`TYPE X3`, `LED STRIP LIGHT X3`).
pub(crate) fn callout(text: &str, mark: &str) -> bool {
    let mark = mark.trim();
    if mark.is_empty() {
        return false;
    }
    let up = text.trim().to_ascii_uppercase();
    if up == mark.to_ascii_uppercase() {
        return true;
    }
    words(&up).any(|w| w.eq_ignore_ascii_case(mark))
        && cadaddy_mepfix::wordrules::words("casework.callout_words").iter().any(|w| words(&up).any(|x| x == *w) || up.contains(&format!("{w} ")))
}

/// The length a note gives when it has one of `list`'s words (`2'-0" O.C.`, `6" FROM END`).
fn length_note(text: &str, list: &[&str]) -> Option<f64> {
    let up = text.to_ascii_uppercase();
    if !list.iter().any(|w| up.contains(w)) {
        return None;
    }
    super::dimreader::values_in(text, false).first().map(|v| v.inches).filter(|v| v.is_finite() && *v > 0.0 && *v <= MAX_LENGTH_IN)
}

/// The middle of a text (its insertion point is its start for left-aligned text).
fn middle(t: &TextItem) -> Vec2 {
    let w = 0.6 * t.height * t.value.chars().count() as f64;
    t.at + Vec2::from_angle(t.rotation) * (w / 2.0)
}

/// Read what the detail on `sheet` (its `texts`, its line work `segs` and its dimension strings
/// `dims`, all in sheet coordinates) says of luminaire type `mark` (see the module docs). None
/// when the sheet calls the type out nowhere.
pub(crate) fn read_detail(
    sheet: &str,
    mark: &str,
    texts: &[TextItem],
    segs: &[(Vec2, Vec2)],
    dims: &[super::dimreader::DimString],
) -> Option<CaseworkRead> {
    let callouts: Vec<&TextItem> =
        texts.iter().take(MAX_TEXTS).filter(|t| t.height.is_finite() && t.height > 0.0 && callout(&t.value, mark)).take(MAX_CALLOUTS).collect();
    let first = callouts.first()?;
    let mut hs: Vec<f64> = callouts.iter().map(|t| t.height).collect();
    hs.sort_by(f64::total_cmp);
    let h = hs.get(hs.len() / 2).copied().unwrap_or(first.height);
    let mut out = CaseworkRead { sheet: sheet.to_string(), mark: mark.trim().to_ascii_uppercase(), ..CaseworkRead::default() };
    // Where each fixture is: its callout, and the far end of a leader starting there.
    let mut spots: Vec<Vec2> = Vec::new();
    for c in &callouts {
        let m = middle(c);
        spots.push(m);
        for (a, b) in segs.iter().take(MAX_SEGS) {
            for (near, far) in [(*a, *b), (*b, *a)] {
                if near.dist(m) <= LEADER_START_H * h.max(c.height) && far.dist(near) > LEADER_START_H * h && far.dist(near) <= LEADER_MAX_H * h {
                    spots.push(far);
                }
            }
        }
    }
    let near_callout = |p: Vec2, reach: f64| callouts.iter().map(|c| middle(c).dist(p)).fold(f64::INFINITY, f64::min) <= reach;
    let dist_to = |p: Vec2| callouts.iter().map(|c| middle(c).dist(p)).fold(f64::INFINITY, f64::min);
    // Its mounting height: the nearest height note, one naming the type first.
    out.height = texts
        .iter()
        .take(MAX_TEXTS)
        .filter(|t| {
            near_callout(t.at, NOTE_REACH_H * h) || (words(&t.value).any(|w| w.eq_ignore_ascii_case(mark)) && near_callout(t.at, REGION_H * h))
        })
        .filter_map(|t| Some((t, super::propfill::mount_height_in(&t.value).filter(|v| v.is_finite() && *v > 0.0 && *v <= MAX_HEIGHT_IN)?)))
        .min_by(|a, b| dist_to(a.0.at).total_cmp(&dist_to(b.0.at)))
        .map(|(t, v)| (v, t.value.trim().to_string()));
    // Its spacing and its distance from the end, as notes.
    let note = |list: &[&str]| {
        texts
            .iter()
            .take(MAX_TEXTS)
            .filter(|t| near_callout(t.at, REGION_H * h))
            .filter_map(|t| Some((t, length_note(&t.value, list)?)))
            .min_by(|a, b| dist_to(a.0.at).total_cmp(&dist_to(b.0.at)))
            .map(|(t, v)| (v, format!("{sheet} note \"{}\"", t.value.trim())))
    };
    out.spacing = note(cadaddy_mepfix::wordrules::words("casework.spacing_words"));
    out.end = note(cadaddy_mepfix::wordrules::words("casework.end_words"));
    // The dimension chain through the fixtures: its stretches in order, each witness's distance
    // from the chain's first (the casework's end).
    let mut chains: BTreeMap<usize, Vec<&super::dimreader::DimString>> = BTreeMap::new();
    for d in dims.iter().take(MAX_STRINGS).filter(|d| d.inches.is_finite() && d.inches > 0.0 && near_callout(d.text_at, REGION_H * h)) {
        chains.entry(d.chain).or_default().push(d);
    }
    let mut best: Option<(usize, f64, Vec<f64>, f64, String)> = None;
    for (_, mut ds) in chains {
        ds.sort_by_key(|d| d.index);
        let Some(d0) = ds.first() else { continue };
        let u = d0.dir;
        if !u.is_finite() || u.len() < 0.5 {
            continue;
        }
        // The witnesses along the line, and their distances from the first.
        let mut along: Vec<(Vec2, f64)> = vec![(d0.ends.0.on_line, 0.0)];
        let mut run = 0.0;
        for d in &ds {
            run += d.inches;
            along.push((d.ends.1.on_line, run));
        }
        let inner: Vec<(Vec2, f64)> = along.iter().copied().skip(1).take(along.len().saturating_sub(2)).collect();
        let at_fixture = |w: Vec2| spots.iter().any(|s| ((*s - w).dot(u)).abs() <= WITNESS_AT_H * h);
        let matched: Vec<f64> = inner.iter().filter(|(w, _)| at_fixture(*w)).map(|(_, t)| *t).collect();
        if matched.is_empty() {
            continue;
        }
        // A chain whose inner stretches are all alike spaces a row of fixtures: every inner
        // witness is one (a type mark tags one of them, `TYP.`).
        let gaps: Vec<f64> = inner.windows(2).filter_map(|w| Some(w.get(1)?.1 - w.first()?.1)).collect();
        let regular = gaps.first().is_some_and(|g0| gaps.iter().all(|g| (g - g0).abs() <= EQUAL_IN));
        let offsets: Vec<f64> = if regular { inner.iter().map(|(_, t)| *t).collect() } else { matched.clone() };
        let near = ds.iter().map(|d| dist_to(d.text_at)).fold(f64::INFINITY, f64::min);
        let values: Vec<String> = ds.iter().map(|d| d.text.trim().to_string()).collect();
        let better = best.as_ref().is_none_or(|(n, bd, _, _, _)| matched.len() > *n || (matched.len() == *n && near < *bd));
        if better {
            best = Some((matched.len(), near, offsets, run, values.join(" + ")));
        }
    }
    if let Some((_, _, offsets, run, values)) = best {
        let basis = format!("{sheet} dimension string {values}");
        if let Some(first) = offsets.first() {
            out.end = Some((*first, basis.clone()));
        }
        let gaps: Vec<f64> = offsets.windows(2).filter_map(|w| Some(w.get(1)? - w.first()?)).collect();
        if let Some(g) = median(gaps) {
            out.spacing = Some((g, basis));
        }
        out.offsets_in = offsets;
        out.run_in = Some(run);
    }
    out.any().then_some(out)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied()
}

/// What laying out the casework luminaires did.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Laid {
    /// Casework luminaires seen; given a height by a detail; moved along their casework by one.
    pub luminaires: usize,
    pub heights: usize,
    pub moved: usize,
    /// Their types no detail dimensions (they stay at their boxes, at the rules' height).
    pub undimensioned: Vec<String>,
}

/// Lay the casework luminaires among `devices` out by `reads` (their type's detail, by upper-case
/// mark), `upi` drawing units per inch (see the module docs).
pub(crate) fn lay_out(devices: &mut [super::readers::ReadDevice], reads: &BTreeMap<String, CaseworkRead>, upi: f64) -> Laid {
    let mut out = Laid::default();
    if !(upi.is_finite() && upi > 0.0) {
        return out;
    }
    let ids: Vec<usize> = devices
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.connection.trim().is_empty() && d.kind.contains("luminaire") && d.at.iter().all(|v| v.is_finite()))
        .map(|(i, _)| i)
        .take(MAX_LUMINAIRES)
        .collect();
    out.luminaires = ids.len();
    // One run each: a type's luminaires on a level within reach of one another.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut seen = vec![false; ids.len()];
    for (k, &i) in ids.iter().enumerate() {
        if seen.get(k).copied().unwrap_or(true) {
            continue;
        }
        let Some(di) = devices.get(i) else { continue };
        let mark = di.tag.trim().to_ascii_uppercase();
        let read = reads.get(&mark);
        let reach = read.and_then(|r| r.run_in).map_or(RUN_REACH_FT * 12.0, |r| r.max(RUN_REACH_FT * 12.0)) * upi;
        if let Some(s) = seen.get_mut(k) {
            *s = true;
        }
        let mut group = vec![i];
        let mut j = 0;
        while let Some(&g) = group.get(j) {
            j += 1;
            let Some(dg) = devices.get(g) else { continue };
            let p = Vec2::new(dg.at[0], dg.at[1]);
            for (m, &o) in ids.iter().enumerate() {
                if seen.get(m).copied().unwrap_or(true) {
                    continue;
                }
                let Some(dd) = devices.get(o) else { continue };
                if dd.tag.trim().eq_ignore_ascii_case(&mark) && dd.level == di.level && Vec2::new(dd.at[0], dd.at[1]).dist(p) <= reach {
                    if let Some(s) = seen.get_mut(m) {
                        *s = true;
                    }
                    group.push(o);
                }
            }
        }
        groups.push(group);
    }
    for group in groups {
        let Some(mark) = group.first().and_then(|i| devices.get(*i)).map(|d| d.tag.trim().to_ascii_uppercase()) else { continue };
        let Some(read) = reads.get(&mark) else {
            if !out.undimensioned.contains(&mark) {
                out.undimensioned.push(mark);
            }
            continue;
        };
        // Its height, unless its own documents printed one by it.
        if let Some((inches, note)) = &read.height {
            for i in &group {
                let Some(d) = devices.get_mut(*i) else { continue };
                if d.aff_in.is_none() {
                    d.aff_in = Some(*inches);
                    d.aff_note = format!("{} casework detail, luminaire {mark}: {note}", read.sheet);
                    out.heights += 1;
                }
            }
        }
        // Along the line the boxes run on, centred on them, at the detail's distances.
        let n = group.len();
        if n < 2 {
            continue;
        }
        let offsets: Vec<f64> = if read.offsets_in.len() == n {
            read.offsets_in.clone()
        } else if let Some((s, _)) = &read.spacing {
            (0..n).map(|k| k as f64 * s).collect()
        } else {
            continue;
        };
        let pts: Vec<Vec2> = group.iter().filter_map(|i| devices.get(*i)).map(|d| Vec2::new(d.at[0], d.at[1])).collect();
        let Some(u) = principal_axis(&pts) else { continue };
        let c = pts.iter().fold(Vec2::ZERO, |a, p| a + *p) / n as f64;
        let mean = offsets.iter().sum::<f64>() / n as f64;
        let mut order: Vec<(usize, f64)> = group.iter().copied().zip(pts.iter().map(|p| (*p - c).dot(u))).collect();
        order.sort_by(|a, b| a.1.total_cmp(&b.1));
        let how = if read.offsets_in.len() == n {
            format!(
                "{} from the casework's end{}",
                read.offsets_in.iter().map(|v| super::walltypes::frac(*v)).collect::<Vec<_>>().join(", "),
                read.end.as_ref().map_or_else(String::new, |(_, b)| format!(" ({b})"))
            )
        } else {
            read.spacing.as_ref().map_or_else(String::new, |(s, b)| format!("{} on centres ({b})", super::walltypes::frac(*s)))
        };
        let end = read
            .end
            .as_ref()
            .filter(|_| read.offsets_in.len() != n)
            .map_or_else(String::new, |(e, b)| format!("; its first {} from the casework's end ({b})", super::walltypes::frac(*e)));
        for ((i, _), off) in order.iter().zip(&offsets) {
            let Some(d) = devices.get_mut(*i) else { continue };
            let to = c + u * ((off - mean) * upi);
            if !to.is_finite() {
                continue;
            }
            let from = d.at;
            d.at = [to.x, to.y];
            d.drawn_at = Some(from);
            d.moved = format!(
                "laid along its casework by {}'s casework detail of luminaire {mark}: {how}{end}; centred on the junction boxes the electrical plan draws (diagrammatic)",
                read.sheet
            );
            out.moved += 1;
        }
    }
    out
}

/// The way most of `pts` run (the principal axis), unit; None when they are all at one spot.
fn principal_axis(pts: &[Vec2]) -> Option<Vec2> {
    let n = pts.len() as f64;
    if n < 2.0 {
        return None;
    }
    let c = pts.iter().fold(Vec2::ZERO, |a, p| a + *p) / n;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in pts {
        let q = *p - c;
        sxx += q.x * q.x;
        sxy += q.x * q.y;
        syy += q.y * q.y;
    }
    if sxx + syy <= 1e-12 {
        return None;
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let u = Vec2::from_angle(angle);
    u.is_finite().then_some(u)
}

/// The casework details of the types among `marks` (upper case), read off the architectural and
/// interiors sheets of the job's drawing `d`: `sheets` are each such sheet's number and handle
/// span. A type the sheets call out on several details takes the one that says most.
pub(crate) fn read_details(d: &cadaddy_doc::Drawing, sheets: &[(String, u64, u64)], marks: &[String]) -> BTreeMap<String, CaseworkRead> {
    let mut out: BTreeMap<String, CaseworkRead> = BTreeMap::new();
    if marks.is_empty() || sheets.is_empty() {
        return out;
    }
    // Each sheet's texts, by its handle span (one pass over the model).
    let mut texts: Vec<Vec<TextItem>> = vec![Vec::new(); sheets.len()];
    let mut seen = 0usize;
    for e in d.model.iter() {
        let (value, at, height, rotation) = match &e.kind {
            cadaddy_doc::EntityKind::Text(t) => (t.value.as_str(), t.insert.xy(), t.height, t.rotation),
            cadaddy_doc::EntityKind::MText(t) => (t.contents.as_str(), t.insert.xy(), t.height, t.rotation),
            _ => continue,
        };
        let Some(k) = sheets.iter().position(|(_, lo, hi)| e.handle.0 >= *lo && e.handle.0 < *hi) else { continue };
        let value = value.trim();
        if value.is_empty() || !at.is_finite() || !height.is_finite() || height <= 0.0 {
            continue;
        }
        seen += 1;
        if seen > MAX_TEXTS {
            break;
        }
        if let Some(v) = texts.get_mut(k) {
            v.push(TextItem { value: value.to_string(), at, height, rotation: if rotation.is_finite() { rotation } else { 0.0 } });
        }
    }
    // The regions round each type's callouts, sheet by sheet: their line work and dimensions.
    let mut regions: Vec<(usize, String, [f64; 4])> = Vec::new();
    for (k, ts) in texts.iter().enumerate() {
        for m in marks {
            let cs: Vec<&TextItem> = ts.iter().filter(|t| callout(&t.value, m)).take(MAX_CALLOUTS).collect();
            let Some(h) = median(cs.iter().map(|t| t.height).collect()) else { continue };
            let r = REGION_H * h;
            let b = cs.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, t| {
                [b[0].min(t.at.x - r), b[1].min(t.at.y - r), b[2].max(t.at.x + r), b[3].max(t.at.y + r)]
            });
            if b.iter().all(|v| v.is_finite()) {
                regions.push((k, m.clone(), b));
            }
        }
    }
    if regions.is_empty() {
        return out;
    }
    let rects: Vec<[f64; 4]> = regions.iter().map(|(_, _, r)| *r).collect();
    let items = super::dimreader::items_by_rect(d, &rects);
    for ((k, m, _), it) in regions.iter().zip(items) {
        let Some((number, _, _)) = sheets.get(*k) else { continue };
        let dims = super::dimreader::read(&it, None).strings;
        let ts = texts.get(*k).map_or(&[][..], Vec::as_slice);
        let Some(r) = read_detail(number, m, ts, &it.segs, &dims) else { continue };
        let score = |r: &CaseworkRead| (r.offsets_in.len(), r.height.is_some() as usize + r.spacing.is_some() as usize);
        if out.get(m).is_none_or(|o| score(&r) > score(o)) {
            out.insert(m.clone(), r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::readers::ReadDevice;

    fn t(value: &str, x: f64, y: f64, h: f64) -> TextItem {
        TextItem { value: value.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
    }

    #[test]
    fn a_callout_is_the_mark_alone_or_with_words_that_call_out_a_light() {
        assert!(callout("X3", "X3"));
        assert!(callout("TYPE X3", "X3"));
        assert!(callout("LED STRIP LIGHT, TYPE X3 TYP.", "X3"));
        assert!(!callout("X3 PANEL", "X3"), "no light word");
        assert!(!callout("X30", "X3"));
        assert!(!callout("TYPE X3", ""));
    }

    #[test]
    fn notes_give_the_height_spacing_and_end_distance() {
        let texts = [
            t("TYPE X3", 100.0, 100.0, 2.0),
            t("3'-6\" AFF", 104.0, 92.0, 2.0),
            t("2'-0\" O.C.", 90.0, 80.0, 2.0),
            t("6\" FROM END OF MILLWORK", 90.0, 75.0, 2.0),
            // Far off: another detail's note.
            t("7'-0\" AFF", 900.0, 900.0, 2.0),
        ];
        let r = read_detail("A-50", "X3", &texts, &[], &[]).unwrap_or_else(|| panic!("no read"));
        assert_eq!(r.height.as_ref().map(|h| h.0), Some(42.0), "{r:?}");
        assert_eq!(r.spacing.as_ref().map(|s| s.0), Some(24.0), "{r:?}");
        assert_eq!(r.end.as_ref().map(|s| s.0), Some(6.0), "{r:?}");
        assert!(r.offsets_in.is_empty());
        assert!(read_detail("A-50", "Z9", &texts, &[], &[]).is_none(), "a type it never calls out");
    }

    fn dev(tag: &str, x: f64, y: f64, connection: &str) -> ReadDevice {
        ReadDevice {
            tag: tag.into(),
            at: [x, y],
            kind: "luminaire".into(),
            level: "1".into(),
            connection: connection.into(),
            ..ReadDevice::default()
        }
    }

    #[test]
    fn a_run_is_laid_along_its_boxes_at_the_details_distances() {
        // Three boxes drawn roughly along a counter (diagrammatic: uneven, 1 unit = 1 inch).
        let mut devices = vec![dev("X3", 0.0, 0.0, "box"), dev("X3", 50.0, 1.0, "box"), dev("X3", 70.0, -1.0, "box"), dev("H", 30.0, 0.0, "")];
        let mut reads = BTreeMap::new();
        reads.insert(
            "X3".to_string(),
            CaseworkRead {
                sheet: "A-50".into(),
                mark: "X3".into(),
                height: Some((42.0, "3'-6\" AFF".into())),
                offsets_in: vec![6.0, 30.0, 54.0],
                run_in: Some(60.0),
                spacing: Some((24.0, "chain".into())),
                end: Some((6.0, "chain".into())),
            },
        );
        let laid = lay_out(&mut devices, &reads, 1.0);
        assert_eq!((laid.luminaires, laid.heights, laid.moved), (3, 3, 3), "{laid:?}");
        let mut xs: Vec<f64> = devices.iter().take(3).map(|d| (d.at[0] * 10.0).round() / 10.0).collect();
        xs.sort_by(f64::total_cmp);
        // Centred on the boxes (x 40): 16, 40, 64.
        let gaps: Vec<f64> = xs.windows(2).map(|w| ((w[1] - w[0]) * 10.0).round() / 10.0).collect();
        assert_eq!(gaps, vec![24.0, 24.0], "{xs:?}");
        assert!(((xs[0] + xs[2]) / 2.0 - 40.0).abs() < 0.6, "{xs:?}");
        for d in devices.iter().take(3) {
            assert_eq!(d.aff_in, Some(42.0));
            assert!(d.drawn_at.is_some() && d.moved.contains("A-50") && d.moved.contains("diagrammatic"), "{d:?}");
        }
        // The ceiling luminaire is no casework's.
        assert_eq!(devices[3].at, [30.0, 0.0]);
        assert!(devices[3].aff_in.is_none() && devices[3].moved.is_empty());
    }

    #[test]
    fn a_type_no_detail_dimensions_stays_at_its_box() {
        let mut devices = vec![dev("X4", 0.0, 0.0, "box"), dev("X4", 50.0, 0.0, "box")];
        let laid = lay_out(&mut devices, &BTreeMap::new(), 1.0);
        assert_eq!((laid.luminaires, laid.moved), (2, 0));
        assert_eq!(laid.undimensioned, vec!["X4".to_string()]);
        assert_eq!(devices[1].at, [50.0, 0.0]);
    }

    /// A casework elevation drawn on a sheet: the counter from x 0 to 60 (1 unit = 1 inch), three
    /// fixtures under it at 6, 30 and 54, one tagged `X3` with a leader, the others not; a chain
    /// below dimensioning end, fixtures, end; a height note by the tag.
    #[test]
    fn a_dimension_chain_through_the_fixtures_gives_their_distances_from_the_end() {
        let mut d = cadaddy_doc::Drawing::default();
        let common = || cadaddy_doc::Common { layer: "PDF".into(), ..cadaddy_doc::Common::default() };
        let mut line = |a: (f64, f64), b: (f64, f64)| {
            d.add(
                &cadaddy_doc::Space::Model,
                common(),
                cadaddy_doc::EntityKind::Line(cadaddy_doc::Line {
                    a: cadaddy_geom::Vec3::new(a.0, a.1, 0.0),
                    b: cadaddy_geom::Vec3::new(b.0, b.1, 0.0),
                }),
            )
            .ok();
        };
        let y = -10.0;
        line((-1.0, y), (61.0, y));
        for x in [0.0, 6.0, 30.0, 54.0, 60.0] {
            line((x - 0.5, y - 0.5), (x + 0.5, y + 0.5));
            line((x, y + 1.0), (x, 18.0));
        }
        // The tag's leader, from by the tag down to the middle fixture.
        line((30.5, 37.0), (30.0, 21.0));
        let mut text = |v: &str, x: f64, yy: f64| {
            d.add(
                &cadaddy_doc::Space::Model,
                common(),
                cadaddy_doc::EntityKind::Text(cadaddy_doc::Text {
                    value: v.into(),
                    insert: cadaddy_geom::Vec3::new(x, yy, 0.0),
                    align_pt: None,
                    height: 1.5,
                    rotation: 0.0,
                    width_factor: 1.0,
                    oblique: 0.0,
                    style: "STANDARD".into(),
                    halign: cadaddy_doc::HAlign::default(),
                    valign: cadaddy_doc::VAlign::default(),
                }),
            )
            .ok();
        };
        for (v, x) in [("6\"", 2.0), ("2'-0\"", 16.0), ("2'-0\"", 40.0), ("6\"", 55.5)] {
            text(v, x, y + 0.4);
        }
        text("X3", 30.0, 38.0);
        text("42\" AFF", 33.0, 30.0);
        let hi = d.handseed;
        let reads = read_details(&d, &[("A-50".to_string(), 0, hi.max(1) + 1)], &["X3".to_string()]);
        let r = reads.get("X3").unwrap_or_else(|| panic!("no read: {reads:?}"));
        assert_eq!(r.offsets_in, vec![6.0, 30.0, 54.0], "{r:?}");
        assert_eq!(r.run_in, Some(60.0));
        assert_eq!(r.spacing.as_ref().map(|s| s.0), Some(24.0));
        assert_eq!(r.end.as_ref().map(|s| s.0), Some(6.0));
        assert_eq!(r.height.as_ref().map(|s| s.0), Some(42.0));
        // No other sheet, no read.
        assert!(read_details(&d, &[("A-50".to_string(), 0, 1)], &["X3".to_string()]).is_empty());
    }
}
