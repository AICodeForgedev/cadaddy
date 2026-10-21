//! Gaps and conflicts planted in a set on purpose, for the RFI list ([`crate::rfi`]) to find:
//! - a wall tag the legend doesn't have;
//! - the same wall typed differently on the floor plan and on an enlarged plan (`A-401`, added to
//!   the set);
//! - a rated wall type made to stop just above the ceiling;
//! - a wall type left out of every head of wall detail;
//! - a ceiling-height wall beside a room open to the structure, and one under a two-layer
//!   ceiling;
//! - a room whose RCP tag and finish schedule leave its ceiling height out;
//! - a soffit callout to a detail the details sheet doesn't have, and a soffit detail no soffit
//!   uses;
//! - sleeve notes: one with only a size, one with a destination but no method, one complete, and
//!   one cored through the slab onto a beam.
//!
//! Each is recorded in [`Truth::planted`]. They come from a random stream of their own, after the
//! walls and ceilings, and only [`crate::generate_with_gaps`] plants them, so every other set is
//! unchanged.

use crate::rng::Rng;
use crate::truth::*;
use crate::walls::{deck_ft, rule_height};
use cadaddy_walk3d::RfiKind;

/// The enlarged plan's sheet number and title.
pub const ENLARGED_SHEET: &str = "A-401";
pub const ENLARGED_TITLE: &str = "ENLARGED TOILET ROOM PLAN - LEVEL 1";

fn plant_gap(t: &mut Truth, kind: RfiKind, about: &str) {
    t.planted.push(PlantedGap { kind: kind.name().into(), about: about.into() });
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Distance from `p` to the segment `a`–`b`.
pub fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    if l2 <= 1e-12 {
        return dist(p, a);
    }
    let s = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + d[0] * s, a[1] + d[1] * s])
}

fn room<'a>(t: &'a Truth, level: &str, number: &str) -> Option<&'a Room> {
    t.building.rooms.iter().find(|r| r.level == level && r.number == number)
}

/// A wall's height again after its type (or the ceilings) changed.
fn rewall(t: &mut Truth, i: usize) {
    let Some(w) = t.plan_walls.get(i) else { return };
    let rule = t.wall_types.iter().find(|x| x.tag == w.wall_type).map(|x| x.height.clone()).unwrap_or_default();
    let ceiling = w.rooms.iter().filter_map(|n| room(t, &w.level, n)).filter_map(|r| r.ceiling.as_ref().map(|c| c.height_ft)).reduce(f64::max);
    let h = rule_height(&rule, ceiling, deck_ft(t, &w.level));
    if let Some(w) = t.plan_walls.get_mut(i) {
        w.height_ft = (h * 1e6).round() / 1e6;
    }
}

fn wet(name: &str) -> bool {
    ["TOILET", "MEN", "WOMEN", "RESTROOM"].iter().any(|w| name.split_whitespace().any(|x| x == *w))
}

/// A wall tag in the legend's style that the legend doesn't have.
fn stray_tag(t: &Truth) -> String {
    let first = t.wall_types.first().map(|w| w.tag.clone()).unwrap_or_else(|| "A".into());
    let stem: String = first.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
    for n in [9, 8, 0] {
        let cand = if stem.is_empty() { format!("{n}X") } else { format!("{stem}{n}") };
        if !t.wall_types.iter().any(|w| w.tag == cand) && cand.chars().count() <= 4 {
            return cand;
        }
    }
    "Q9".into()
}

/// The point a wall's tag lands on (where its leader ends), feet.
fn tag_point(w: &PlanWall, tag: &WallTag) -> [f64; 2] {
    let len = dist(w.a_ft, w.b_ft).max(1e-9);
    let u = [(w.b_ft[0] - w.a_ft[0]) / len, (w.b_ft[1] - w.a_ft[1]) / len];
    [w.a_ft[0] + u[0] * tag.s_ft, w.a_ft[1] + u[1] * tag.s_ft]
}

fn inside(p: [f64; 2], lo: [f64; 2], hi: [f64; 2], margin: f64) -> bool {
    p[0] > lo[0] + margin && p[0] < hi[0] - margin && p[1] > lo[1] + margin && p[1] < hi[1] - margin
}

/// Plant the gaps (see the module docs).
pub fn plant(t: &mut Truth) {
    let mut r = Rng::new(t.seed ^ 0x5246_495F_4741_5053).fork(7);
    let mut used: Vec<String> = Vec::new();
    let tagged = |t: &Truth, used: &[String]| -> Vec<usize> {
        t.plan_walls.iter().enumerate().filter(|(_, w)| w.tagged && !w.tags.is_empty() && !used.contains(&w.id)).map(|(i, _)| i).collect()
    };

    // The same wall, another type on the enlarged plan: a tagged level 1 wall whose tags are all
    // on the enlarged plan.
    let (lo, hi) = crate::layout::window(t, "TOILET");
    let in_window: Vec<usize> = tagged(t, &used)
        .into_iter()
        .filter(|i| {
            t.plan_walls.get(*i).is_some_and(|w| {
                w.level == "1"
                    && inside(w.a_ft, lo, hi, -0.01)
                    && inside(w.b_ft, lo, hi, -0.01)
                    && w.tags.iter().all(|tg| inside(tag_point(w, tg), lo, hi, 1.5))
            })
        })
        .collect();
    if !in_window.is_empty() {
        let i = r.pick(&in_window);
        let (id, ty) = t.plan_walls.get(i).map(|w| (w.id.clone(), w.wall_type.clone())).unwrap_or_default();
        let others: Vec<String> = t.wall_types.iter().filter(|x| x.tag != ty && !x.external).map(|x| x.tag.clone()).collect();
        let other = r.pick(&others);
        if !other.is_empty()
            && let Some(w) = t.plan_walls.get_mut(i)
        {
            w.enlarged_tag = Some(other);
            used.push(id.clone());
            plant_gap(t, RfiKind::SheetConflict, &id);
        }
    }

    // A tag the legend doesn't have.
    let cands = tagged(t, &used);
    if !cands.is_empty() {
        let i = r.pick(&cands);
        let tag = stray_tag(t);
        if let Some(w) = t.plan_walls.get_mut(i) {
            w.plan_tag = Some(tag);
            let id = w.id.clone();
            used.push(id.clone());
            plant_gap(t, RfiKind::UnknownTag, &id);
        }
    }

    // A ceiling-height partition beside the open mechanical room, and one beside a room with a
    // two-layer ceiling.
    let partition = t
        .wall_types
        .iter()
        .find(|x| matches!(x.height.as_str(), "ceiling" | "above_ceiling") && x.fire_rating.is_none() && !x.external)
        .map(|x| x.tag.clone());
    if let Some(p) = &partition {
        let boards = |t: &Truth, ct: &str| {
            t.ceiling_types.iter().find(|c| c.tag == ct).map_or(0, |c| c.layers.iter().filter(|l| l.material.starts_with("gyp")).count())
        };
        let two_layer =
            |t: &Truth, lv: &str, n: &str| t.room_ceilings.iter().any(|c| c.level == lv && c.room == n && boards(t, &c.ceiling_type) >= 2);
        let open = |t: &Truth, lv: &str, n: &str| room(t, lv, n).is_some_and(|r| r.ceiling.is_none());
        let with_ceiling = |t: &Truth, lv: &str, n: &str| room(t, lv, n).is_some_and(|r| r.ceiling.is_some());
        let corridor = |t: &Truth, lv: &str, n: &str| room(t, lv, n).is_some_and(|r| r.name == "CORRIDOR");
        for kind in [RfiKind::HeightVsCeiling, RfiKind::DoubleCeilingShortWall] {
            let cands: Vec<usize> = tagged(t, &used)
                .into_iter()
                .filter(|i| {
                    t.plan_walls.get(*i).is_some_and(|w| {
                        let [a, b] = [w.rooms.first(), w.rooms.get(1)];
                        let (Some(a), Some(b)) = (a, b) else { return false };
                        match kind {
                            RfiKind::HeightVsCeiling => {
                                (open(t, &w.level, a) && with_ceiling(t, &w.level, b)) || (open(t, &w.level, b) && with_ceiling(t, &w.level, a))
                            }
                            _ => {
                                (two_layer(t, &w.level, a) || two_layer(t, &w.level, b))
                                    && !open(t, &w.level, a)
                                    && !open(t, &w.level, b)
                                    && !(corridor(t, &w.level, a) || corridor(t, &w.level, b))
                            }
                        }
                    })
                })
                .collect();
            if cands.is_empty() {
                continue;
            }
            let i = r.pick(&cands);
            let Some(w) = t.plan_walls.get_mut(i) else { continue };
            w.wall_type = p.clone();
            let id = w.id.clone();
            used.push(id.clone());
            rewall(t, i);
            plant_gap(t, kind, &id);
        }
    }

    // A rated stud wall type made to stop 6" above the ceiling: its head detail becomes the
    // ceiling one (when there is one).
    let rated = t
        .wall_types
        .iter()
        .find(|x| x.fire_rating.is_some() && x.height == "deck" && !x.external && x.layers.iter().any(|l| l.material == "stud"))
        .map(|x| x.tag.clone());
    if let Some(tag) = rated {
        let ceiling_head = t.head_details.iter().find(|h| h.to == "ceiling").map(|h| h.tag.clone());
        for h in t.head_details.iter_mut() {
            h.wall_types.retain(|x| *x != tag);
        }
        if let Some(ch) = &ceiling_head
            && let Some(h) = t.head_details.iter_mut().find(|h| &h.tag == ch)
        {
            h.wall_types.push(tag.clone());
        }
        if let Some(wt) = t.wall_types.iter_mut().find(|x| x.tag == tag) {
            wt.height = "above_ceiling".into();
            wt.head = ceiling_head;
        }
        for i in 0..t.plan_walls.len() {
            if t.plan_walls.get(i).is_some_and(|w| w.wall_type == tag) {
                rewall(t, i);
            }
        }
        plant_gap(t, RfiKind::RatedToCeiling, &tag);
    }

    // A wall type no head of wall detail covers: one from a detail that covers several.
    let shared: Vec<(usize, String)> = t
        .head_details
        .iter()
        .enumerate()
        .filter(|(_, h)| h.wall_types.len() >= 2)
        .flat_map(|(i, h)| h.wall_types.iter().map(move |x| (i, x.clone())))
        .filter(|(_, x)| t.wall_types.iter().any(|w| &w.tag == x && w.fire_rating.is_none()))
        .collect();
    if !shared.is_empty() {
        let (i, tag) = r.pick(&shared);
        if let Some(h) = t.head_details.get_mut(i) {
            h.wall_types.retain(|x| *x != tag);
        }
        if let Some(wt) = t.wall_types.iter_mut().find(|x| x.tag == tag) {
            wt.head = None;
        }
        plant_gap(t, RfiKind::HeadUnspecified, &tag);
    }

    // A room whose ceiling height the RCP and the schedule leave out: a dry room with a ceiling
    // and no soffit.
    let rooms: Vec<usize> = t
        .building
        .rooms
        .iter()
        .enumerate()
        .filter(|(_, rm)| {
            rm.ceiling.is_some()
                && rm.name != "CORRIDOR"
                && !wet(&rm.name)
                && !t.rcp_soffits.iter().any(|s| s.level == rm.level && s.room == rm.number)
        })
        .map(|(i, _)| i)
        .collect();
    if !rooms.is_empty() {
        let i = r.pick(&rooms);
        if let Some(rm) = t.building.rooms.get_mut(i)
            && let Some(c) = rm.ceiling.as_mut()
        {
            c.height_hidden = true;
            let about = format!("{}/{}", rm.level, rm.number);
            plant_gap(t, RfiKind::CeilingUnspecified, &about);
        }
    }

    // Detail numbers in use on the details sheet.
    let numbers: Vec<u32> =
        t.soffit_types.iter().map(|s| &s.tag).chain(t.head_details.iter().map(|h| &h.tag)).filter_map(|s| s.parse().ok()).collect();
    let next = numbers.iter().copied().max().unwrap_or(0) + 1;

    // A callout to a detail that isn't there.
    let called: Vec<usize> = t.rcp_soffits.iter().enumerate().filter(|(_, s)| s.called_out).map(|(i, _)| i).collect();
    if !called.is_empty() {
        let i = r.pick(&called);
        if let Some(s) = t.rcp_soffits.get_mut(i) {
            s.callout_detail = Some((next + 1).to_string());
            let id = s.id.clone();
            plant_gap(t, RfiKind::DetailMissing, &id);
        }
    }

    // A soffit detail no soffit uses: another hung soffit.
    if let Some(mut st) = t.soffit_types.iter().find(|s| s.hangs_from == "structure").or(t.soffit_types.first()).cloned() {
        st.tag = next.to_string();
        st.name = format!("{} AT FUTURE BULKHEAD", st.name);
        let tag = st.tag.clone();
        t.soffit_types.push(st);
        plant_gap(t, RfiKind::DetailUnused, &tag);
    }

    plant_sleeves(t, &mut r);
}

/// Four sleeve notes on the top floor (see the module docs).
fn plant_sleeves(t: &mut Truth, r: &mut Rng) {
    let level = t.building.levels.iter().map(|l| l.code.clone()).filter(|c| c != "R").max().unwrap_or_else(|| "1".into());
    let below = t.building.levels.iter().filter(|l| l.code != level && l.code != "R").map(|l| l.code.clone()).max();
    let below_words = below.map_or("THE LEVEL BELOW".to_string(), |b| format!("LEVEL {b}"));
    let beams: Vec<([f64; 2], [f64; 2])> = t.building.beams.iter().filter(|b| b.level == level).map(|b| (b.from_ft, b.to_ft)).collect();
    let walls: Vec<([f64; 2], [f64; 2])> = t.plan_walls.iter().filter(|w| w.level == level).map(|w| (w.a_ft, w.b_ft)).collect();
    // The note runs up and right of its sleeve: keep wall tags out of that box (a tag's leader
    // mustn't meet the note's), the note about 0.07" a letter at the plan's scale.
    let scale = t.sheets.iter().find(|s| s.level.as_deref() == Some(level.as_str()) && s.title.contains("FLOOR PLAN")).map_or(0.125, |s| s.scale);
    let tags: Vec<[f64; 2]> = t.plan_walls.iter().filter(|w| w.level == level).flat_map(|w| w.tags.iter().map(|tg| tag_point(w, tg))).collect();
    // A note's box: the sleeve, its leader and the text, feet.
    let k = 1.0 / scale.max(1e-6);
    let note_box = |p: [f64; 2], chars: usize| [p[0] - 1.0, p[1] - 1.0, p[0] + (0.5 + chars as f64 * 0.07) * k, p[1] + 0.45 * k + 0.5];
    let names: Vec<[f64; 2]> = t
        .building
        .rooms
        .iter()
        .filter(|r| r.level == level)
        .map(|r| [(r.min_ft[0] + r.max_ft[0]) / 2.0, (r.min_ft[1] + r.max_ft[1]) / 2.0])
        .collect();
    let x_max = t.building.rooms.iter().filter(|r| r.level == level).map(|r| r.max_ft[0]).fold(f64::MIN, f64::max);
    let note_clear = |p: [f64; 2], chars: usize, boxes: &[[f64; 4]]| {
        let b = note_box(p, chars);
        let apart = |o: &[f64; 4]| b[2] + 1.0 < o[0] || o[2] + 1.0 < b[0] || b[3] + 1.0 < o[1] || o[3] + 1.0 < b[1];
        b[2] < x_max - 1.0
            && names.iter().all(|c| dist(p, *c) > 4.0)
            && tags.iter().all(|q| !(q[0] > b[0] - 3.0 && q[0] < b[2] + 3.0 && q[1] > b[1] - 3.0 && q[1] < b[3] + 3.0))
            && boxes.iter().all(apart)
    };
    let clear = |p: [f64; 2], boxes: &[[f64; 4]], chars: usize| {
        beams.iter().all(|(a, b)| to_segment(p, *a, *b) > 3.0)
            && walls.iter().all(|(a, b)| to_segment(p, *a, *b) > 2.0)
            && note_clear(p, chars, boxes)
    };
    // Candidates: points across each room (not the corridor) on this level.
    let mut cands: Vec<[f64; 2]> = Vec::new();
    for rm in t.building.rooms.iter().filter(|rm| rm.level == level && rm.name != "CORRIDOR") {
        for fx in [0.25, 0.5, 0.75] {
            for fy in [0.3, 0.5, 0.7] {
                cands.push([rm.min_ft[0] + (rm.max_ft[0] - rm.min_ft[0]) * fx, rm.min_ft[1] + (rm.max_ft[1] - rm.min_ft[1]) * fy]);
            }
        }
    }
    let start = r.below(cands.len().max(1)).min(cands.len());
    cands.rotate_left(start);
    let mut boxes: Vec<[f64; 4]> = Vec::new();
    let notes: [(&str, usize, f64, Option<String>, Option<&str>); 3] = [
        ("3\" SLEEVE", 1, 3.0, None, None),
        ("(2) 2\" SLEEVES TO CORRIDOR CEILING", 2, 2.0, Some("CORRIDOR CEILING".into()), None),
        ("", 2, 2.0, Some(format!("{below_words} CEILING")), Some("CORE THRU SLAB")),
    ];
    let mut n = 0;
    for (text, count, size, route, method) in notes {
        let text = if text.is_empty() { format!("(2) 2\" SLEEVES, CORE THRU SLAB TO {below_words} CEILING") } else { text.to_string() };
        let Some(p) = cands.iter().copied().find(|p| clear(*p, &boxes, text.chars().count())) else { continue };
        boxes.push(note_box(p, text.chars().count()));
        n += 1;
        let id = format!("SL{n:02}");
        let gap = match (&route, method) {
            (Some(_), Some(_)) => None,
            _ => Some(RfiKind::SleeveNoRoute),
        };
        t.sleeve_notes.push(SleeveNote {
            id: id.clone(),
            level: level.clone(),
            at_ft: p,
            text,
            count,
            size_in: size,
            route,
            method: method.map(String::from),
            on_beam: false,
        });
        if let Some(k) = gap {
            plant_gap(t, k, &id);
        }
    }
    // Cored through the slab onto a beam: halfway along a beam inside a room, away from walls.
    let text = format!("4\" SLEEVE, CORE THRU SLAB TO {below_words} CEILING");
    let on: Vec<[f64; 2]> = beams
        .iter()
        .flat_map(|(a, b)| [0.2, 0.3, 0.4, 0.5, 0.6, 0.7].map(|f| [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]))
        .filter(|p| walls.iter().all(|(a, b)| to_segment(*p, *a, *b) > 2.0) && note_clear(*p, text.chars().count(), &boxes))
        .filter(|p| t.building.rooms.iter().any(|rm| rm.level == level && inside(*p, rm.min_ft, rm.max_ft, 1.0)))
        .collect();
    if !on.is_empty() {
        let p = r.pick(&on);
        n += 1;
        let id = format!("SL{n:02}");
        t.sleeve_notes.push(SleeveNote {
            id: id.clone(),
            level: level.clone(),
            at_ft: p,
            text,
            count: 1,
            size_in: 4.0,
            route: Some(format!("{below_words} CEILING")),
            method: Some("CORE THRU SLAB".into()),
            on_beam: true,
        });
        plant_gap(t, RfiKind::SleeveNoFit, &id);
    }
}

/// The enlarged plan's sheet: the enlarged plumbing plan's place on paper (same window, scale
/// and placement), as an architectural sheet.
pub fn enlarged_sheet(t: &Truth) -> Option<Sheet> {
    let p = t.sheets.iter().find(|s| s.number == "P-401")?;
    let mut s = p.clone();
    s.number = ENLARGED_SHEET.into();
    s.title = ENLARGED_TITLE.into();
    s.discipline = "A".into();
    s.pdf = format!("{ENLARGED_SHEET}.pdf");
    s.dxf = format!("{ENLARGED_SHEET}.dxf");
    s.legend = Vec::new();
    s.abbreviations = Vec::new();
    Some(s)
}
