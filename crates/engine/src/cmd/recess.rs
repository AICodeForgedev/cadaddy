//! A cabinet set into a framed wall must fit the wall (Q6560, `E-RECESS-CAVITY`).
//!
//! A panel whose mounting recesses it (its schedule's MOUNTING: recessed, flush) has its front
//! flush with the room face's finish and its back no deeper than the far face's board: the wall
//! holds its framing depth plus the near face's layers (its wall type's), else its built thickness
//! less the far face's layers (a type with no framing), else less one board (the rule's number).
//! A cabinet deeper than that is mounted on the wall's face, as the field would install it, and
//! the design mounting is kept beside it with the reason and the rule (the alternative, a deeper
//! wall, named in the reason). The numbers and the mounting it is corrected to are rule data
//! (`tradepractice.json`).

use std::collections::HashMap;

use cadaddy_doc::{Drawing, Handle};
use cadaddy_walk3d::{BuildingSpec, LayerSide, MepEquipment, RunCorrection, WallType};
use serde::Deserialize;

/// The trade practice rule this check follows.
pub(crate) const RULE: &str = "E-RECESS-CAVITY";
/// Most built walls looked at (hostile drawings).
const MAX_WALLS: usize = 2_000_000;

/// The numbers of [`RULE`] (rule data `tradepractice.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rule {
    /// The board on the far face of a wall whose type gives no layers, inches.
    pub face_board_in: f64,
    /// How much deeper than the wall holds a cabinet may be and still go in, inches.
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

/// What a wall's type says of the depth it holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Holds {
    /// Its framing this deep, and its thinner face's layers (the face a cabinet is set in, its
    /// board cut out for it), inches.
    Studs { stud: f64, near: f64 },
    /// No framing: its thicker face's layers come off the built thickness, inches.
    Faces { far: f64 },
}

/// What a wall type's layers say it holds; none when it gives no layers.
fn of_type(t: &WallType) -> Option<Holds> {
    let layers = &t.assembly.as_ref()?.layers;
    if layers.is_empty() {
        return None;
    }
    let ok = |v: f64| v.is_finite() && v > 0.0;
    let stud = layers.iter().filter(|l| l.category.eq_ignore_ascii_case("framing") && ok(l.thickness_in)).map(|l| l.thickness_in).fold(0.0, f64::max);
    let face = |c: &str| ["board", "sheathing", "furring"].iter().any(|k| c.eq_ignore_ascii_case(k));
    let side = |sd: LayerSide| -> f64 {
        layers.iter().filter(|l| l.side == sd && !l.in_cavity && face(&l.category) && ok(l.thickness_in)).map(|l| l.thickness_in).sum()
    };
    let (a, b) = (side(LayerSide::Side1), side(LayerSide::Side2));
    Some(if stud > 0.0 { Holds::Studs { stud, near: a.min(b) } } else { Holds::Faces { far: a.max(b) } })
}

/// The built walls' depths as their types give them, and the rule.
pub(crate) struct Cavities {
    rule: Rule,
    by_wall: HashMap<Handle, Holds>,
}

/// Whether a recessed cabinet goes in its wall, or on its face with the correction.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fit {
    InWall,
    OnFace(RunCorrection),
}

impl Cavities {
    /// The rule alone: every wall judged by its built thickness less a board on its far face.
    pub(crate) fn none() -> Self {
        Cavities { rule: Rule::load(), by_wall: HashMap::new() }
    }

    /// The built walls of `d` whose type (`type:` build tag, a wall type of `sp`) gives layers.
    pub(crate) fn of(d: &Drawing, sp: &BuildingSpec) -> Self {
        let mut out = Self::none();
        let types: HashMap<String, Holds> = sp.wall_types.iter().filter_map(|t| Some((t.name.trim().to_ascii_lowercase(), of_type(t)?))).collect();
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

    /// How deep a cabinet set into wall `wall` (`thick_in` thick as built) may be, its front
    /// flush with the room face: inches.
    pub(crate) fn holds_in(&self, wall: Handle, thick_in: f64) -> f64 {
        let thick = if thick_in.is_finite() { thick_in.max(0.0) } else { 0.0 };
        match self.by_wall.get(&wall) {
            Some(Holds::Studs { stud, near }) => stud + near,
            Some(Holds::Faces { far }) => (thick - far).max(0.0),
            None => (thick - self.rule.face_board_in.max(0.0)).max(0.0),
        }
    }

    /// Why a cabinet `depth_in` deep can't go into wall `wall` (`thick_in` thick as built): the
    /// wall holds less (none: it fits).
    pub(crate) fn too_deep(&self, wall: Handle, thick_in: f64, depth_in: f64) -> Option<String> {
        let holds = self.holds_in(wall, thick_in);
        if !depth_in.is_finite() || depth_in <= holds + self.rule.tol_in.max(0.0) {
            return None;
        }
        let by = match self.by_wall.get(&wall) {
            Some(Holds::Studs { stud, near }) => format!("its type's {}\" framing and {}\" near face", r2(*stud), r2(*near)),
            Some(Holds::Faces { far }) => format!("the wall's {}\" less its type's {}\" far face", r2(thick_in), r2(*far)),
            None => format!("the wall's {}\" less a {}\" board on its far face", r2(thick_in), r2(self.rule.face_board_in)),
        };
        Some(format!("a {}\" deep cabinet can't recess in a wall that holds {}\" behind its face ({by})", r2(depth_in), r2(holds)))
    }

    /// Whether `e`, a cabinet `depth_in` deep whose mounting sets it into wall `wall` (`thick_in`
    /// thick as built), fits the wall; else it goes on the face, with the correction.
    pub(crate) fn fit(&self, e: &MepEquipment, wall: Handle, thick_in: f64, depth_in: f64) -> Fit {
        let on_face = self.rule.on_face.trim();
        let Some(why) = self.too_deep(wall, thick_in, depth_in).filter(|_| !on_face.is_empty()) else { return Fit::InWall };
        let deeper = depth_in - self.holds_in(wall, thick_in);
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

    fn layer(category: &str, t: f64, side: LayerSide) -> AssemblyLayer {
        AssemblyLayer { material: format!("{category} layer"), thickness_in: t, category: category.into(), side, ..AssemblyLayer::default() }
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
    fn a_wall_types_framing_and_faces_give_what_it_holds() {
        let bare = WallType { name: "T".into(), height_ft: None, thickness_in: 6.0, from: WallFrom::Pairs, assembly: None };
        let t = |layers: Vec<AssemblyLayer>| WallType { assembly: Some(Assembly { layers, ..Assembly::default() }), ..bare.clone() };
        let (s1, s2, core) = (LayerSide::Side1, LayerSide::Side2, LayerSide::Core);
        // 3 5/8" studs, two boards on one face and one on the other: the cabinet goes in the thinner.
        let studs = t(vec![layer("board", 0.625, s1), layer("board", 0.625, s1), layer("framing", 3.625, core), layer("board", 0.625, s2)]);
        assert_eq!(of_type(&studs), Some(Holds::Studs { stud: 3.625, near: 0.625 }));
        assert_eq!(of_type(&t(vec![layer("board", 0.625, s1), layer("core", 7.625, core)])), Some(Holds::Faces { far: 0.625 }));
        assert_eq!(of_type(&bare), None, "no layers: the rule's board");
        let c = Cavities::none();
        let board = c.rule.face_board_in;
        assert!((c.holds_in(Handle(1), 6.0) - (6.0 - board)).abs() < 1e-9);
        assert_eq!(c.holds_in(Handle(1), f64::NAN), 0.0);
    }

    #[test]
    fn a_cabinet_deeper_than_its_wall_holds_goes_on_the_face_with_its_design_mounting_kept() {
        let c = Cavities::none();
        let e = panel("recessed, front flush with the wall");
        // A 4" can goes in a 4 7/8" partition (4 1/4" behind the face); a wall 4 1/8" thick holds 3 1/2".
        assert_eq!(c.fit(&e, Handle(1), 4.875, 4.0), Fit::InWall);
        let Fit::OnFace(k) = c.fit(&e, Handle(1), 4.125, 4.0) else { panic!("on the face") };
        assert_eq!((k.key.as_str(), k.design.as_str(), k.rule.as_str()), ("MOUNTING", "recessed, front flush with the wall", RULE));
        assert_eq!(k.corrected, c.rule.on_face);
        assert!(k.reason.contains("holds 3.5\"") && k.reason.contains("alternative"), "{}", k.reason);
        // Its type's framing governs over what was built: 5 1/2" studs hold it however thin it was drawn.
        let mut c = Cavities::none();
        c.by_wall.insert(Handle(2), Holds::Studs { stud: 5.5, near: 0.625 });
        assert_eq!(c.fit(&e, Handle(2), 4.125, 4.0), Fit::InWall);
        c.by_wall.insert(Handle(3), Holds::Studs { stud: 2.5, near: 0.5 });
        assert!(matches!(c.fit(&e, Handle(3), 8.0, 4.0), Fit::OnFace(_)), "2 1/2\" studs and a 1/2\" board can't take a 4\" can");
    }
}
