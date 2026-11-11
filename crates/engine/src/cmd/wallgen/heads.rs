//! A wall's top by its type's head rule (Q6507): the wall type legend says how high each type
//! goes (to the ceiling, past it, to the deck), and the build stops the wall there. A wall built
//! to the ceiling stops at the ceiling beside it: the reflected ceiling plan's area there, else
//! the room's ceiling the spec gives, else the level's; where the two sides differ, the lower.
//! A height the spec gives as a number (the wall's own, else its type's) still wins.

use cadaddy_geom::Vec2;
use cadaddy_walk3d::{BuildingSpec, CeilingKind, HeightRule, Level, feet};

use super::PlanWall;

/// The build tag of a wall whose top its type's head rule set: `head:<rule>: <height> (<from>)`.
pub(crate) const HEAD_TAG: &str = "head:";
/// Most past the ceiling a wall goes, inches (a hostile legend).
const MAX_ABOVE_IN: f64 = 120.0;
/// How far past a wall's face its sides are looked at for the ceiling there, inches.
const PROBE_IN: f64 = 6.0;
/// Where along a wall its sides are looked at (a wall beside several rooms takes the lowest).
const PROBE_ALONG: [f64; 3] = [0.25, 0.5, 0.75];
/// Most wall types named in one note.
const MAX_NAMED: usize = 8;

/// A traced room's ceiling: its level, outline (sheet units), area, and the ceiling height the
/// spec gives it (feet above its floor) with where that was read.
pub(crate) struct RoomCeiling {
    pub level: String,
    pub outline: Vec<Vec2>,
    pub area: f64,
    pub ceiling_ft: f64,
    pub source: String,
}

/// What one side of a wall has over it.
enum Over {
    /// A ceiling this high above the floor (feet), and where its height was read.
    Ceiling(f64, String),
    /// No ceiling: the structure is exposed there.
    Open,
}

/// An outline with its box, for the look-ups.
struct Area {
    poly: Vec<Vec2>,
    lo: Vec2,
    hi: Vec2,
    size: f64,
    over: Over,
}

impl Area {
    fn new(poly: Vec<Vec2>, size: f64, over: Over) -> Option<Self> {
        if poly.len() < 3 {
            return None;
        }
        let lo = poly.iter().fold(Vec2::new(f64::INFINITY, f64::INFINITY), |m, p| Vec2::new(m.x.min(p.x), m.y.min(p.y)));
        let hi = poly.iter().fold(Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY), |m, p| Vec2::new(m.x.max(p.x), m.y.max(p.y)));
        (lo.x.is_finite() && hi.y.is_finite()).then_some(Area { poly, lo, hi, size, over })
    }

    fn holds(&self, p: Vec2) -> bool {
        p.x >= self.lo.x && p.x <= self.hi.x && p.y >= self.lo.y && p.y <= self.hi.y && cadaddy_geom::point_in_polygon(&self.poly, p)
    }
}

/// The tightest area holding `p` (a room traced twice: its tighter trace).
fn at(areas: &[Area], p: Vec2) -> Option<&Over> {
    areas.iter().filter(|a| a.holds(p)).min_by(|a, b| a.size.total_cmp(&b.size)).map(|a| &a.over)
}

/// A polygon's area (shoelace), absolute.
fn poly_area(poly: &[Vec2]) -> f64 {
    let n = poly.len();
    (0..n).map(|i| poly[i].cross(poly[(i + 1) % n])).sum::<f64>().abs() / 2.0
}

/// Each typed wall of `level` whose top no number sets: its top by its type's head rule. To the
/// ceiling (or `above_ceiling_in` past it) where a ceiling height is known beside it, capped at
/// the storey; to the deck or full height, the storey's top (`level_top`, sheet units). The
/// walls whose rule can't be settled (no ceiling height known, or no ceiling beside it) keep the
/// storey's top and are named in `notes`.
#[allow(clippy::too_many_arguments)]
pub(super) fn head_walls(
    sp: &BuildingSpec,
    level: &Level,
    rooms: &[RoomCeiling],
    z0: f64,
    level_top: f64,
    upf: f64,
    walls: &mut [PlanWall],
    notes: &mut Vec<String>,
) {
    let ruled = |w: &PlanWall| {
        let t = w.wall_type.as_deref().and_then(|n| sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(n)))?;
        let a = t.assembly.as_ref()?;
        Some((a.height?, a.above_ceiling_in))
    };
    if !walls.iter().any(|w| !w.fixed_top && matches!(ruled(w), Some((HeightRule::Ceiling | HeightRule::AboveCeiling, _)))) {
        return;
    }
    let k = upf / 12.0;
    // The reflected ceiling plans' areas first (what the ceiling there is), then the rooms'.
    let rcp: Vec<Area> = sp
        .rcp
        .ceilings
        .iter()
        .filter(|c| c.level.eq_ignore_ascii_case(&level.name))
        .filter_map(|c| {
            let poly: Vec<Vec2> = c.outline.iter().take(10_000).map(|p| Vec2::new(p[0], p[1])).collect();
            let open = c
                .ceiling_type
                .as_ref()
                .and_then(|t| sp.ceiling_types.iter().find(|ct| ct.name.eq_ignore_ascii_case(t)))
                .is_some_and(|ct| ct.kind == CeilingKind::Open);
            let over = if open {
                Over::Open
            } else if c.height_ft.is_finite() && c.height_ft > 0.0 {
                let from =
                    c.source.as_deref().map_or_else(|| "the reflected ceiling plan".to_string(), |s| format!("the reflected ceiling plan, {s}"));
                Over::Ceiling(c.height_ft, from)
            } else {
                return None;
            };
            let size = poly_area(&poly);
            Area::new(poly, size, over)
        })
        .collect();
    let traced: Vec<Area> = rooms
        .iter()
        .filter(|r| r.level.eq_ignore_ascii_case(&level.name))
        .filter_map(|r| Area::new(r.outline.iter().take(10_000).copied().collect(), r.area, Over::Ceiling(r.ceiling_ft, r.source.clone())))
        .collect();
    let level_ceiling = (level.ceiling_ft.is_finite() && level.ceiling_ft > 0.0).then_some(level.ceiling_ft);
    let (mut to_ceiling, mut no_ceiling, mut unknown) = (0usize, Vec::new(), Vec::new());
    for w in walls.iter_mut().filter(|w| !w.fixed_top) {
        let Some((rule @ (HeightRule::Ceiling | HeightRule::AboveCeiling), above_in)) = ruled(w) else { continue };
        let len = w.a.dist(w.b);
        if len.is_nan() || len <= 1e-9 {
            continue;
        }
        let u = (w.b - w.a) * (1.0 / len);
        let off = w.thickness / 2.0 + PROBE_IN * k;
        let (mut low, mut open): (Option<(f64, String)>, bool) = (None, false);
        for t in PROBE_ALONG {
            let p = w.a + u * (len * t);
            for side in [u.perp() * off, u.perp() * -off] {
                match at(&rcp, p + side).or_else(|| at(&traced, p + side)) {
                    Some(Over::Ceiling(c, from)) => {
                        if low.as_ref().is_none_or(|(l, _)| c < l) {
                            low = Some((*c, from.clone()));
                        }
                    }
                    Some(Over::Open) => open = true,
                    None => {}
                }
            }
        }
        let name = w.wall_type.clone().unwrap_or_default();
        let ceiling = match low {
            Some(c) => c,
            // Exposed structure each side it's looked at: no ceiling for it to stop at.
            None if open => {
                no_ceiling.push(name);
                w.head = Some(format!("{}: no ceiling beside it, to the deck", rule.text(above_in)));
                continue;
            }
            None => match level_ceiling {
                Some(c) => (c, "the level's ceiling".to_string()),
                None => {
                    unknown.push(name);
                    continue;
                }
            },
        };
        let past = if rule == HeightRule::AboveCeiling { above_in.clamp(0.0, MAX_ABOVE_IN) / 12.0 } else { 0.0 };
        let top_ft = ceiling.0 + past;
        let top = (z0 + top_ft * upf).min(level_top);
        if top < level_top {
            w.top = top;
            to_ceiling += 1;
            w.head = Some(format!("{}: {} ({})", rule.text(above_in), feet(top_ft), ceiling.1));
        } else {
            w.head = Some(format!("{}: the ceiling ({}, {}) is at the deck, to the deck", rule.text(above_in), feet(ceiling.0), ceiling.1));
        }
    }
    let named = |v: &mut Vec<String>| {
        v.sort();
        v.dedup();
        let more = v.len().saturating_sub(MAX_NAMED);
        let mut s = v.iter().take(MAX_NAMED).cloned().collect::<Vec<_>>().join(", ");
        if more > 0 {
            s.push_str(&format!(" and {more} more"));
        }
        s
    };
    if to_ceiling > 0 {
        notes
            .push(format!("{}: {to_ceiling} walls stop at the ceiling beside them, as their wall type's head rule says (not the deck).", level.name));
    }
    if !no_ceiling.is_empty() {
        notes.push(format!(
            "{}: {} walls of a type built to the ceiling have no ceiling beside them (structure exposed): built to the deck (types {}).",
            level.name,
            no_ceiling.len(),
            named(&mut no_ceiling)
        ));
    }
    if !unknown.is_empty() {
        notes.push(format!(
            "{}: {} walls of a type built to the ceiling have no ceiling height known beside them: built to the deck until one is given (types {}).",
            level.name,
            unknown.len(),
            named(&mut unknown)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use serde_json::{Value, json};

    fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
        s.execute("line", &json!({"points": [a, b]})).unwrap();
    }

    fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
        line(s, a, [b[0], a[1]]);
        line(s, [b[0], a[1]], b);
        line(s, b, [a[0], b[1]]);
        line(s, [a[0], b[1]], a);
    }

    /// A made-up 30' × 20' floor (inches) on two storeys of 10': 6" outside walls on layer
    /// A-WALL (type X, full height) and a 4" partition on A-PART at x = 180" splitting it into
    /// two rooms, `WEST` and `EAST` tagged on A-AREA-IDEN; the partition's type P with the head
    /// rule `rule`, the spec patched by `patch`, then built.
    fn built(rule: Option<&str>, patch: impl FnOnce(&mut Value)) -> (Session, Value) {
        let mut s = Session::new();
        for l in ["A-WALL", "A-PART", "A-AREA-IDEN"] {
            s.execute("layer.new", &json!({"name": l})).unwrap();
        }
        s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
        rect(&mut s, [0.0, 0.0], [360.0, 240.0]);
        rect(&mut s, [6.0, 6.0], [354.0, 234.0]);
        s.execute("layer.current", &json!({"name": "A-PART"})).unwrap();
        line(&mut s, [178.0, 6.0], [178.0, 234.0]);
        line(&mut s, [182.0, 6.0], [182.0, 234.0]);
        s.execute("layer.current", &json!({"name": "A-AREA-IDEN"})).unwrap();
        for (t, x) in [("WEST", 60.0), ("EAST", 260.0)] {
            s.execute("text", &json!({"at": [x, 120.0], "text": t, "height": 6.0})).unwrap();
        }
        let levels = json!([
            {"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 8.0, "region": [-100.0, -100.0, 500.0, 400.0]},
            {"name": "Level 2", "elevation_ft": 10.0, "height_ft": 10.0, "ceiling_ft": 8.0, "region": [1000.0, 1000.0, 1500.0, 1400.0]}
        ]);
        s.execute("bspec.draft", &json!({"store": true, "levels": levels})).unwrap();
        let mut spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
        let mut assembly = json!({"tag": "P", "layers": []});
        if let Some(r) = rule {
            assembly["height"] = json!(r);
        }
        spec["layers"] = json!([
            {"layer": "A-WALL", "role": "wall", "wall_type": "X"},
            {"layer": "A-PART", "role": "wall", "wall_type": "P"},
            {"layer": "A-AREA-IDEN", "role": "room_tag"}
        ]);
        spec["wall_types"] = json!([
            {"name": "X", "thickness_in": 6, "from": "pairs", "assembly": {"tag": "X", "layers": [], "height": "deck"}},
            {"name": "P", "thickness_in": 4, "from": "pairs", "assembly": assembly}
        ]);
        spec["rooms"] = json!([{"name": "WEST"}, {"name": "EAST"}]);
        patch(&mut spec);
        s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
        let r = s.execute("model.build", &json!({})).unwrap();
        (s, r)
    }

    /// The built walls of `ty`: each one's top (feet) and its `head:` tag.
    fn tops(s: &Session, ty: &str) -> Vec<(f64, Option<String>)> {
        let d = s.doc().unwrap();
        d.model
            .iter()
            .filter(|e| super::super::made_as(e).as_deref() == Some("wall"))
            .filter_map(|e| {
                let tags: Vec<&str> = e.common.xdata.iter().map(|t| t.value.as_str()).collect();
                if !tags.iter().any(|t| t.strip_prefix("type:") == Some(ty)) {
                    return None;
                }
                let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { return None };
                let top = m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max) / 12.0;
                Some((top, tags.iter().find_map(|t| t.strip_prefix(HEAD_TAG)).map(str::to_string)))
            })
            .collect()
    }

    fn notes(r: &Value) -> String {
        r["notes"].as_array().map(|n| n.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" | ")).unwrap_or_default()
    }

    #[test]
    fn a_partition_built_to_the_ceiling_stops_at_the_levels_ceiling_and_past_it_by_the_legends_inches() {
        // To the ceiling: 8' on the 10' storey (it was 10': the deck), its studs and boards with it.
        let (s, r) = built(Some("ceiling"), |_| {});
        let p = tops(&s, "P");
        assert_eq!(p.len(), 1, "{p:?} {}", notes(&r));
        assert!((p[0].0 - 8.0).abs() < 1e-6, "{p:?}");
        assert!(
            p[0].1.as_deref().is_some_and(|h| h.contains("to the ceiling") && h.contains("8'-0\"") && h.contains("the level's ceiling")),
            "{p:?}"
        );
        assert!(notes(&r).contains("1 walls stop at the ceiling"), "{}", notes(&r));
        // The outside walls go to the deck (just under the next floor).
        assert!(tops(&s, "X").iter().all(|(t, h)| *t > 9.8 && h.is_none()), "{:?}", tops(&s, "X"));
        // Past the ceiling: 6" by default, else the legend's.
        let (s, _) = built(Some("above_ceiling"), |_| {});
        assert!(tops(&s, "P").iter().all(|(t, _)| (t - 8.5).abs() < 1e-6), "{:?}", tops(&s, "P"));
        let (s, _) = built(Some("above_ceiling"), |sp| sp["wall_types"][1]["assembly"]["above_ceiling_in"] = json!(12));
        assert!(tops(&s, "P").iter().all(|(t, _)| (t - 9.0).abs() < 1e-6), "{:?}", tops(&s, "P"));
        // No head rule, or deck / full: floor to floor, as before.
        for rule in [None, Some("deck"), Some("full")] {
            let (s, _) = built(rule, |_| {});
            assert!(tops(&s, "P").iter().all(|(t, h)| *t > 9.8 && h.is_none()), "{rule:?}: {:?}", tops(&s, "P"));
        }
    }

    #[test]
    fn a_wall_between_two_ceilings_stops_at_the_lower_and_a_number_still_wins() {
        // The rooms each side give their own ceilings: the lower of the two.
        let (s, _) = built(Some("ceiling"), |sp| {
            sp["rooms"] = json!([{"name": "WEST", "ceiling_ft": 9.0}, {"name": "EAST", "ceiling_ft": 7.5, "ceiling_source": "a made-up RCP note"}]);
        });
        let p = tops(&s, "P");
        assert!(p.iter().all(|(t, h)| (t - 7.5).abs() < 1e-6 && h.as_deref().is_some_and(|h| h.contains("a made-up RCP note"))), "{p:?}");
        // The reflected ceiling plan's area over a side outranks the room's.
        let (s, _) = built(Some("ceiling"), |sp| {
            sp["rooms"] = json!([{"name": "WEST", "ceiling_ft": 9.0}, {"name": "EAST", "ceiling_ft": 9.0}]);
            sp["rcp"] = json!({"ceilings": [{"level": "Level 1", "outline": [[182, 6], [354, 6], [354, 234], [182, 234]], "height_ft": 8.25}]});
        });
        assert!(tops(&s, "P").iter().all(|(t, _)| (t - 8.25).abs() < 1e-6), "{:?}", tops(&s, "P"));
        // A ceiling at or over the storey: the wall goes to the deck.
        let (s, _) =
            built(Some("above_ceiling"), |sp| sp["rooms"] = json!([{"name": "WEST", "ceiling_ft": 9.8}, {"name": "EAST", "ceiling_ft": 9.8}]));
        assert!(tops(&s, "P").iter().all(|(t, h)| *t > 9.8 && h.as_deref().is_some_and(|h| h.contains("to the deck"))), "{:?}", tops(&s, "P"));
        // The wall's own height, and its type's, are numbers the rule doesn't move.
        let (s, _) = built(Some("ceiling"), |sp| {
            sp["walls"] = json!([{"level": "Level 1", "a": [180, 6], "b": [180, 234], "wall_type": "P", "height_ft": 9.0}])
        });
        assert!(tops(&s, "P").iter().all(|(t, h)| (t - 9.0).abs() < 1e-6 && h.is_none()), "{:?}", tops(&s, "P"));
        let (s, _) = built(Some("ceiling"), |sp| sp["wall_types"][1]["height_ft"] = json!(9.25));
        assert!(tops(&s, "P").iter().all(|(t, h)| (t - 9.25).abs() < 1e-6 && h.is_none()), "{:?}", tops(&s, "P"));
    }

    #[test]
    fn a_wall_with_no_ceiling_beside_it_goes_to_the_deck_and_is_reported() {
        // Exposed structure both sides (an open ceiling type over the whole floor).
        let (s, r) = built(Some("ceiling"), |sp| {
            sp["ceiling_types"] = json!([{"name": "OPEN-1", "kind": "open"}]);
            sp["rcp"] = json!({"ceilings": [{"level": "Level 1", "outline": [[0, 0], [360, 0], [360, 240], [0, 240]], "height_ft": 8.0, "ceiling_type": "OPEN-1"}]});
        });
        assert!(
            tops(&s, "P").iter().all(|(t, h)| *t > 9.8 && h.as_deref().is_some_and(|h| h.contains("no ceiling beside it"))),
            "{:?}",
            tops(&s, "P")
        );
        assert!(notes(&r).contains("no ceiling beside them") && notes(&r).contains("types P"), "{}", notes(&r));
    }

    #[test]
    fn no_ceiling_height_known_is_reported_and_the_wall_kept_floor_to_floor() {
        let level =
            Level { name: "L1".into(), elevation_ft: 0.0, height_ft: 10.0, ceiling_ft: f64::NAN, region: None, base: None, attr: None, sheet: None };
        let sp: BuildingSpec = serde_json::from_value(json!({
            "units_per_foot": 12, "levels": [],
            "wall_types": [{"name": "P", "thickness_in": 4, "assembly": {"tag": "P", "layers": [], "height": "ceiling"}}]
        }))
        .unwrap();
        let mut walls = vec![PlanWall {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(120.0, 0.0),
            thickness: 4.0,
            top: 118.8,
            source: cadaddy_doc::Handle(1),
            cuts: Vec::new(),
            extra: Vec::new(),
            wall_type: Some("P".into()),
            fixed_top: false,
            head: None,
            basis: None,
        }];
        let mut notes = Vec::new();
        head_walls(&sp, &level, &[], 0.0, 118.8, 12.0, &mut walls, &mut notes);
        assert!((walls[0].top - 118.8).abs() < 1e-9 && walls[0].head.is_none(), "{:?}", walls[0]);
        assert!(notes.iter().any(|n| n.contains("no ceiling height known") && n.contains("types P")), "{notes:?}");
        // A hostile legend's inches past the ceiling are capped.
        let mut sp = sp;
        if let Some(a) = sp.wall_types[0].assembly.as_mut() {
            a.height = Some(HeightRule::AboveCeiling);
            a.above_ceiling_in = 1e12;
        }
        let level = Level { ceiling_ft: 2.0, height_ft: 40.0, ..level };
        walls[0].top = 40.0 * 12.0;
        head_walls(&sp, &level, &[], 0.0, 40.0 * 12.0, 12.0, &mut walls, &mut Vec::new());
        assert!((walls[0].top - 12.0 * 12.0).abs() < 1e-6, "{:?}", walls[0]);
    }
}
