//! Inferred junction boxes stay accessible (Q6562; NEC 314.29, trade practice `E-JBOX-ACCESS` in
//! `tradepractice.json`).
//!
//! The build infers a junction box where three or more runs meet, at the runs' height. Over a
//! suspended (grid) ceiling it is reached by lifting a tile, and over an open ceiling it can be
//! seen. Over a gypsum (hard-lid) ceiling nothing reaches it. The field sets a flush access panel
//! in the ceiling under such a box, and the build does the same. The panel is an element of its
//! own (the takeoff counts it), connected to its box. The box keeps the design value beside the
//! correction: `design:access` (none), `access` (the panel) and `correction:access` (the rule and
//! the reason).
//!
//! The ceiling under a box is found in this order:
//! - a gypsum soffit of the reflected ceiling plans over the point;
//! - else the smallest RCP ceiling area over it: a grid, or a grid or open type, is accessible;
//!   gypsum, or no type and no grid, is hard (as Validate reads it);
//! - else the smallest built ceiling over it: a grid over the point is accessible, a ceiling typed
//!   gypsum is hard, an untyped one is not known and is left as it is (counted).
//!
//! Nothing here panics; every scan is capped.

use cadaddy_geom::Vec2;
use cadaddy_walk3d::{BuildingSpec, CeilingKind};

use super::super::propfill::BuiltCeiling;
use super::super::provenance;

/// The trade practice rule this module follows.
pub(crate) const RULE: &str = "E-JBOX-ACCESS";
/// Most ceiling areas and soffits looked at (hostile specs).
const MAX_AREAS: usize = 50_000;

/// The rule's item and size (`tradepractice.json`).
#[derive(Clone, Debug)]
pub(crate) struct Rule {
    /// What the panel is bought as.
    pub panel_item: String,
    /// Its size as text.
    pub panel_size: String,
    /// Its side, inches (a square panel).
    pub panel_in: f64,
    /// The basis the panel and the correction carry: the rule's id and its reason.
    pub basis: String,
}

impl Rule {
    pub(crate) fn load() -> Rule {
        let v = provenance::rule_data(RULE);
        let text = |k: &str, or: &str| v[k].as_str().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(or).to_string();
        Rule {
            panel_item: text("panel_item", "Access panel, ceiling"),
            panel_size: text("panel_size", ""),
            panel_in: v["panel_in"].as_f64().filter(|x| x.is_finite() && (6.0..=48.0).contains(x)).unwrap_or(12.0),
            basis: provenance::rule(RULE).basis_for(""),
        }
    }
}

/// A hard ceiling under a point: its underside (drawing units, the model's height) and what it is
/// in words.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hard {
    pub z: f64,
    pub what: String,
}

/// What is under a point of a level, above the room.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Under {
    /// A gypsum ceiling or soffit: nothing reaches what is over it.
    Hard(Hard),
    /// A grid (its tiles lift out) or no ceiling at all.
    Accessible,
    /// No ceiling the documents type covers the point.
    Unknown,
}

fn area_of(o: &[[f64; 2]]) -> f64 {
    let n = o.len();
    (0..n).filter_map(|i| Some((o.get(i)?, o.get((i + 1) % n)?))).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum::<f64>().abs() / 2.0
}

fn inside(o: &[[f64; 2]], p: Vec2) -> bool {
    if o.len() < 3 {
        return false;
    }
    let poly: Vec<Vec2> = o.iter().map(|q| Vec2::new(q[0], q[1])).collect();
    cadaddy_geom::point_in_polygon(&poly, p)
}

/// A height in feet as `9'-0"`.
fn ft_in(ft: f64) -> String {
    let inches = (ft * 12.0).round().max(0.0);
    format!("{}'-{}\"", (inches / 12.0).floor(), inches % 12.0)
}

/// What is under plan point `p` of `level` (sheet units; `upf` a foot): see the module notes.
/// `built`: the model's built ceilings on their plans, grids kept
/// ([`super::super::propfill::built_ceilings_with_grids_on_plans`]).
pub(crate) fn under(sp: &BuildingSpec, built: &[BuiltCeiling], level: &str, p: Vec2, upf: f64) -> Under {
    let Some(lv) = sp.level(level) else { return Under::Unknown };
    let floor = lv.elevation_ft * upf;
    if !floor.is_finite() {
        return Under::Unknown;
    }
    let on_level = |l: &str| l.trim().eq_ignore_ascii_case(level.trim());
    let kind_of = |t: Option<&str>| {
        let t = t.map(str::trim).filter(|t| !t.is_empty())?;
        sp.ceiling_types.iter().take(MAX_AREAS).find(|c| c.name.trim().eq_ignore_ascii_case(t)).map(|c| c.kind)
    };
    // A gypsum soffit over the point.
    if let Some(s) = sp
        .rcp
        .soffits
        .iter()
        .take(MAX_AREAS)
        .filter(|s| on_level(&s.level) && s.height_ft.is_finite() && s.height_ft > 0.0 && inside(&s.outline, p))
        .min_by(|a, b| area_of(&a.outline).total_cmp(&area_of(&b.outline)))
    {
        return Under::Hard(Hard { z: floor + s.height_ft * upf, what: format!("the gypsum soffit ({} above the floor)", ft_in(s.height_ft)) });
    }
    // The reflected ceiling plans' smallest area over it.
    if let Some(c) = sp
        .rcp
        .ceilings
        .iter()
        .take(MAX_AREAS)
        .filter(|c| on_level(&c.level) && c.height_ft.is_finite() && c.height_ft > 0.0 && inside(&c.outline, p))
        .min_by(|a, b| area_of(&a.outline).total_cmp(&area_of(&b.outline)))
    {
        let kind = kind_of(c.ceiling_type.as_deref());
        if c.grid.is_some() || matches!(kind, Some(CeilingKind::Grid | CeilingKind::Open)) {
            return Under::Accessible;
        }
        let of = c.room.as_deref().map(str::trim).filter(|r| !r.is_empty()).map(|r| format!(" of room {r}")).unwrap_or_default();
        let what = match (kind, c.ceiling_type.as_deref().map(str::trim).filter(|t| !t.is_empty())) {
            (Some(_), Some(t)) => format!("the gypsum ceiling{of} (type {t}, {} above the floor)", ft_in(c.height_ft)),
            _ => format!("the gypsum ceiling{of} ({} above the floor; no grid on the reflected ceiling plan)", ft_in(c.height_ft)),
        };
        return Under::Hard(Hard { z: floor + c.height_ft * upf, what });
    }
    // The model's built ceilings over it.
    let over: Vec<&BuiltCeiling> = built.iter().filter(|c| c.level().is_none_or(on_level) && c.covers_at(p)).collect();
    if over.iter().any(|c| c.is_grid()) {
        return Under::Accessible;
    }
    let Some(c) = over.into_iter().min_by(|a, b| a.z_and_area().1.total_cmp(&b.z_and_area().1)) else { return Under::Unknown };
    let z = c.z_and_area().0;
    match kind_of(c.ceiling_type()) {
        Some(CeilingKind::Gypsum) if z.is_finite() => Under::Hard(Hard {
            z,
            what: format!("the gypsum ceiling (type {}, {} above the floor)", c.ceiling_type().unwrap_or_default().trim(), ft_in((z - floor) / upf)),
        }),
        Some(_) => Under::Accessible,
        None => Under::Unknown,
    }
}

/// The hard ceiling a junction box at height `box_z` (drawing units) is over, when it is over one.
pub(crate) fn over_hard(under: &Under, box_z: f64) -> Option<&Hard> {
    match under {
        Under::Hard(h) if box_z.is_finite() && box_z > h.z => Some(h),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(extra: serde_json::Value) -> BuildingSpec {
        let mut v = json!({
            "units_per_foot": 12.0,
            "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12}, {"name": "Level 2", "elevation_ft": 12, "height_ft": 12}],
            "ceiling_types": [
                {"name": "HL-1", "kind": "gypsum"},
                {"name": "AT-1", "kind": "grid"},
                {"name": "EX-1", "kind": "open"}
            ]
        });
        if let (Some(o), Some(x)) = (v.as_object_mut(), extra.as_object()) {
            for (k, val) in x {
                o.insert(k.clone(), val.clone());
            }
        }
        serde_json::from_value(v).expect("spec")
    }

    fn sq(x0: f64, y0: f64, side: f64) -> serde_json::Value {
        json!([[x0, y0], [x0 + side, y0], [x0 + side, y0 + side], [x0, y0 + side]])
    }

    #[test]
    fn the_ceiling_under_a_point_is_read_by_its_kind() {
        let sp = spec(json!({"rcp": {"ceilings": [
            {"level": "Level 1", "room": "A", "outline": sq(0.0, 0.0, 120.0), "height_ft": 9.0, "ceiling_type": "HL-1"},
            {"level": "Level 1", "outline": sq(200.0, 0.0, 120.0), "height_ft": 9.0, "ceiling_type": "AT-1"},
            {"level": "Level 1", "outline": sq(400.0, 0.0, 120.0), "height_ft": 8.0, "grid": {"spacing_in": [24.0, 24.0], "origin": [400.0, 0.0]}},
            {"level": "Level 1", "outline": sq(600.0, 0.0, 120.0), "height_ft": 8.5},
            {"level": "Level 1", "outline": sq(800.0, 0.0, 120.0), "height_ft": 10.0, "ceiling_type": "EX-1"},
            {"level": "Level 2", "outline": sq(0.0, 0.0, 120.0), "height_ft": 8.0, "ceiling_type": "HL-1"}
        ], "soffits": [{"level": "Level 1", "outline": sq(20.0, 20.0, 24.0), "height_ft": 7.5}]}}));
        let at = |level: &str, x: f64, y: f64| under(&sp, &[], level, Vec2::new(x, y), 12.0);
        // A typed gypsum ceiling: hard, its underside at the level's floor plus its height.
        let Under::Hard(h) = at("Level 1", 60.0, 90.0) else { panic!("{:?}", at("Level 1", 60.0, 90.0)) };
        assert_eq!(h.z, 108.0);
        assert!(h.what.contains("room A") && h.what.contains("HL-1") && h.what.contains("9'-0\""), "{}", h.what);
        // A soffit inside it is what is under a point over the soffit.
        let Under::Hard(s) = at("Level 1", 30.0, 30.0) else { panic!("soffit") };
        assert_eq!(s.z, 90.0);
        assert!(s.what.contains("soffit"), "{}", s.what);
        // A grid type, a grid read on the plan, an open ceiling: accessible.
        assert_eq!(at("Level 1", 260.0, 60.0), Under::Accessible);
        assert_eq!(at("Level 1", 460.0, 60.0), Under::Accessible);
        assert_eq!(at("Level 1", 860.0, 60.0), Under::Accessible);
        // No type and no grid on the plan: gypsum, as Validate reads it.
        let Under::Hard(u) = at("Level 1", 660.0, 60.0) else { panic!("untyped") };
        assert!(u.what.contains("no grid"), "{}", u.what);
        // Another level's ceiling, and its floor's height.
        let Under::Hard(l2) = at("Level 2", 60.0, 60.0) else { panic!("level 2") };
        assert_eq!(l2.z, (12.0 + 8.0) * 12.0);
        // Where no ceiling is read: not known.
        assert_eq!(at("Level 1", 60.0, 500.0), Under::Unknown);
        assert_eq!(at("Level 9", 60.0, 60.0), Under::Unknown);
    }

    #[test]
    fn a_box_is_over_a_hard_ceiling_only_above_its_underside() {
        let h = Under::Hard(Hard { z: 108.0, what: "x".into() });
        assert!(over_hard(&h, 110.0).is_some());
        assert!(over_hard(&h, 100.0).is_none(), "a box under the ceiling is in the room");
        assert!(over_hard(&h, f64::NAN).is_none());
        assert!(over_hard(&Under::Accessible, 200.0).is_none());
        assert!(over_hard(&Under::Unknown, 200.0).is_none());
    }

    #[test]
    fn the_rule_has_its_panel() {
        let r = Rule::load();
        assert!(r.panel_item.to_ascii_lowercase().contains("access panel"), "{r:?}");
        assert!(r.panel_in >= 6.0 && r.basis.starts_with(RULE) && r.basis.contains("314.29"), "{r:?}");
    }
}
