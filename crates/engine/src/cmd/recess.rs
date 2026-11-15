//! A cabinet set into a framed wall sits in the wall's stud cavity (Q6560, `E-RECESS-CAVITY`).
//!
//! A panel whose mounting recesses it (its schedule's MOUNTING: recessed, flush) goes in its wall
//! only when the wall can hold it: its box no deeper than the wall's cavity, which is the framing
//! depth its wall type gives, else the wall's built thickness less the type's face layers, else
//! less one board on each face (the rule's number). A cabinet deeper than that is mounted on the
//! wall's face, as the field would install it, and the design mounting is kept beside it with the
//! reason and the rule (the alternative, a deeper wall, named in the reason). The numbers and the
//! mounting it is corrected to are rule data (`tradepractice.json`).

use std::collections::HashMap;

use cadaddy_doc::{Drawing, Handle};
use cadaddy_walk3d::{BuildingSpec, MepEquipment, RunCorrection, WallType};
use serde::Deserialize;

/// The trade practice rule this check follows.
pub(crate) const RULE: &str = "E-RECESS-CAVITY";
/// Most built walls looked at (hostile drawings).
const MAX_WALLS: usize = 2_000_000;

/// The numbers of [`RULE`] (rule data `tradepractice.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rule {
    /// The board on each face of a wall whose type gives no layers, inches.
    pub face_board_in: f64,
    /// How much deeper than the cavity a cabinet may be and still go in, inches.
    pub tol_in: f64,
    /// The mounting a cabinet the wall can't hold is corrected to.
    pub on_face: String,
}

impl Rule {
    /// The rule as its data has it.
    pub(crate) fn load() -> Self {
        serde_json::from_value(super::provenance::rule_data(RULE)).unwrap_or_default()
    }
}

/// What a wall's type says of its cavity.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cavity {
    /// Its framing is this deep, inches.
    Studs(f64),
    /// Its face layers (board, sheathing, furring) add up to this, inches: the rest is the cavity.
    Faces(f64),
}

/// The cavity of a wall type by its layers: its framing's depth, else its face layers; none when
/// it gives no layers.
fn of_type(t: &WallType) -> Option<Cavity> {
    let layers = &t.assembly.as_ref()?.layers;
    let ok = |v: f64| v.is_finite() && v > 0.0;
    let stud = layers.iter().filter(|l| l.category.eq_ignore_ascii_case("framing") && ok(l.thickness_in)).map(|l| l.thickness_in).fold(0.0, f64::max);
    if stud > 0.0 {
        return Some(Cavity::Studs(stud));
    }
    let face = |c: &str| ["board", "sheathing", "furring"].iter().any(|k| c.eq_ignore_ascii_case(k));
    let faces: f64 = layers.iter().filter(|l| !l.in_cavity && face(&l.category) && ok(l.thickness_in)).map(|l| l.thickness_in).sum();
    (!layers.is_empty()).then_some(Cavity::Faces(faces))
}

/// The built walls' cavities as their types give them, and the rule.
pub(crate) struct Cavities {
    rule: Rule,
    by_wall: HashMap<Handle, Cavity>,
}

/// Whether a recessed cabinet goes in its wall, or on its face with the correction.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fit {
    InWall,
    OnFace(RunCorrection),
}

impl Cavities {
    /// The rule alone: every wall judged by its built thickness less a board on each face.
    pub(crate) fn none() -> Self {
        Cavities { rule: Rule::load(), by_wall: HashMap::new() }
    }

    /// The built walls of `d` whose type (`type:` build tag, a wall type of `sp`) gives layers.
    pub(crate) fn of(d: &Drawing, sp: &BuildingSpec) -> Self {
        let mut out = Self::none();
        let types: HashMap<String, Cavity> = sp.wall_types.iter().filter_map(|t| Some((t.name.trim().to_ascii_lowercase(), of_type(t)?))).collect();
        if types.is_empty() {
            return out;
        }
        for e in d.model.iter().take(MAX_WALLS) {
            if super::wallgen::made_as(e).as_deref() != Some("wall") {
                continue;
            }
            if let Some(c) = super::bimlook::made_tag(e, "type").and_then(|t| types.get(&t.trim().to_ascii_lowercase())) {
                out.by_wall.insert(e.handle, *c);
            }
        }
        out
    }

    /// The stud cavity of wall `wall`, `thick_in` thick as built, inches.
    pub(crate) fn cavity_in(&self, wall: Handle, thick_in: f64) -> f64 {
        let thick = if thick_in.is_finite() { thick_in.max(0.0) } else { 0.0 };
        match self.by_wall.get(&wall) {
            Some(Cavity::Studs(s)) => *s,
            Some(Cavity::Faces(f)) => (thick - f).max(0.0),
            None => (thick - 2.0 * self.rule.face_board_in.max(0.0)).max(0.0),
        }
    }

    /// Why a cabinet `depth_in` deep can't go into wall `wall` (`thick_in` thick as built): its
    /// cavity is shallower (none: it fits).
    pub(crate) fn too_deep(&self, wall: Handle, thick_in: f64, depth_in: f64) -> Option<String> {
        let cavity = self.cavity_in(wall, thick_in);
        if !depth_in.is_finite() || depth_in <= cavity + self.rule.tol_in.max(0.0) {
            return None;
        }
        let by = match self.by_wall.get(&wall) {
            Some(Cavity::Studs(_)) => "its wall type's framing".to_string(),
            Some(Cavity::Faces(_)) => format!("the wall's {}\" less its type's face layers", r2(thick_in)),
            None => format!("the wall's {}\" less a {}\" board on each face", r2(thick_in), r2(self.rule.face_board_in)),
        };
        Some(format!("a {}\" deep cabinet can't recess in a wall whose stud cavity is {}\" ({by})", r2(depth_in), r2(cavity)))
    }

    /// Whether `e`, a cabinet `depth_in` deep whose mounting sets it into wall `wall` (`thick_in`
    /// thick as built), fits the wall's cavity; else it goes on the face, with the correction.
    pub(crate) fn fit(&self, e: &MepEquipment, wall: Handle, thick_in: f64, depth_in: f64) -> Fit {
        let on_face = self.rule.on_face.trim();
        let Some(why) = self.too_deep(wall, thick_in, depth_in).filter(|_| !on_face.is_empty()) else { return Fit::InWall };
        let deeper = depth_in - self.cavity_in(wall, thick_in);
        Fit::OnFace(RunCorrection {
            key: "MOUNTING".into(),
            design: e.properties.get("MOUNTING").cloned().unwrap_or_default(),
            corrected: on_face.to_string(),
            reason: format!("{why}: mounted on the face; alternative: frame the wall {}\" deeper there to take it", r2(deeper)),
            rule: RULE.into(),
        })
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_walk3d::{Assembly, AssemblyLayer, WallFrom};

    fn layer(category: &str, t: f64) -> AssemblyLayer {
        AssemblyLayer { material: format!("{category} layer"), thickness_in: t, category: category.into(), ..AssemblyLayer::default() }
    }

    fn panel(mounting: &str) -> MepEquipment {
        let mut e = MepEquipment { tag: "P1".into(), kind: "panel".into(), ..MepEquipment::default() };
        e.properties.insert("MOUNTING".into(), mounting.into());
        e
    }

    #[test]
    fn the_rule_loads_with_its_numbers() {
        let r = Rule::load();
        assert!(r.face_board_in > 0.0 && r.tol_in >= 0.0 && !r.on_face.is_empty(), "{r:?}");
        assert!(!super::super::provenance::rule(RULE).basis.is_empty());
    }

    #[test]
    fn a_wall_types_framing_or_face_layers_give_its_cavity() {
        let bare = WallType { name: "T".into(), height_ft: None, thickness_in: 6.0, from: WallFrom::Pairs, assembly: None };
        let t = |layers: Vec<AssemblyLayer>| WallType { assembly: Some(Assembly { layers, ..Assembly::default() }), ..bare.clone() };
        assert_eq!(of_type(&t(vec![layer("board", 0.625), layer("framing", 3.625), layer("board", 0.625)])), Some(Cavity::Studs(3.625)));
        assert_eq!(of_type(&t(vec![layer("board", 0.625), layer("core", 7.625)])), Some(Cavity::Faces(0.625)));
        assert_eq!(of_type(&bare), None, "no layers: the rule's boards");
        let c = Cavities::none();
        let board = c.rule.face_board_in;
        assert!((c.cavity_in(Handle(1), 6.0) - (6.0 - 2.0 * board)).abs() < 1e-9);
        assert_eq!(c.cavity_in(Handle(1), f64::NAN), 0.0);
    }

    #[test]
    fn a_cabinet_deeper_than_the_cavity_goes_on_the_face_with_its_design_mounting_kept() {
        let c = Cavities::none();
        let e = panel("recessed, front flush with the wall");
        // A 4" can in a 6" wall (4 3/4" cavity) goes in; in a wall 4 1/8" thick (2 7/8") it can't.
        assert_eq!(c.fit(&e, Handle(1), 6.0, 4.0), Fit::InWall);
        let Fit::OnFace(k) = c.fit(&e, Handle(1), 4.125, 4.0) else { panic!("on the face") };
        assert_eq!((k.key.as_str(), k.design.as_str(), k.rule.as_str()), ("MOUNTING", "recessed, front flush with the wall", RULE));
        assert_eq!(k.corrected, c.rule.on_face);
        assert!(k.reason.contains("2.88\"") && k.reason.contains("alternative"), "{}", k.reason);
        // Its type's framing governs over what was built: 6" studs hold it however thin it was drawn.
        let mut c = Cavities::none();
        c.by_wall.insert(Handle(2), Cavity::Studs(5.5));
        assert_eq!(c.fit(&e, Handle(2), 4.125, 4.0), Fit::InWall);
        c.by_wall.insert(Handle(3), Cavity::Studs(3.5));
        assert!(matches!(c.fit(&e, Handle(3), 8.0, 4.0), Fit::OnFace(_)), "3 1/2\" studs can't take a 4\" can");
    }
}
