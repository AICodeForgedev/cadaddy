//! Ceiling, soffit and wall track takeoff from the building spec's assemblies.
//!
//! - **Ceilings** (each RCP ceiling with a type): board or tile area per layer, less the soffits
//!   hung from the structure in it (the ceiling stops at them; soffits built under a ceiling
//!   leave it whole); grid tee length; framing (channels, runners, furring) length from its
//!   spacing.
//! - **Soffits** (each with a type): bottom board area per layer; side board area per layer (the
//!   sides that show, from the soffit's bottom up to the ceiling round it, past it for soffits
//!   hung from the structure); stud length on the sides and channel length across the bottom.
//! - **Walls** (the plan's wall marks whose type has a head): top track by kind (a double track
//!   and head angles count both runs) and bottom track.
//!
//! Areas are square feet, lengths feet.

use crate::spec::{AssemblyLayer, BuildingSpec, CeilingKind, Hang, HeadTrack, MAX_ASSEMBLY_LAYERS, MAX_ITEMS};
use std::fmt::Write as _;

/// One line of a takeoff.
#[derive(Clone, Debug, PartialEq)]
pub struct TakeoffRow {
    pub level: String,
    /// `ceiling`, `ceiling framing`, `soffit bottom`, `soffit sides`, `soffit framing`,
    /// `top track`, `bottom track`.
    pub part: String,
    /// The ceiling, soffit or wall type.
    pub assembly: String,
    pub material: String,
    /// Board layers from the room face, 1 first; 0 for framing and track.
    pub layer: usize,
    pub quantity: f64,
    /// `sf` or `lf`.
    pub unit: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Takeoff {
    pub rows: Vec<TakeoffRow>,
}

impl Takeoff {
    fn add(&mut self, level: &str, part: &str, assembly: &str, material: &str, layer: usize, quantity: f64, unit: &'static str) {
        if !(quantity.is_finite() && quantity > 0.0) {
            return;
        }
        match self
            .rows
            .iter_mut()
            .find(|r| r.level == level && r.part == part && r.assembly == assembly && r.material == material && r.layer == layer)
        {
            Some(r) => r.quantity += quantity,
            None => self.rows.push(TakeoffRow {
                level: level.into(),
                part: part.into(),
                assembly: assembly.into(),
                material: material.into(),
                layer,
                quantity,
                unit,
            }),
        }
    }

    /// The total of a part (and material, when given).
    pub fn total(&self, part: &str, material: Option<&str>) -> f64 {
        self.rows.iter().filter(|r| r.part == part && material.is_none_or(|m| r.material == m)).map(|r| r.quantity).sum()
    }

    /// As CSV: a header line, then a line per row.
    pub fn csv(&self) -> String {
        let q = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
        let mut out = String::from("level,part,assembly,material,layer,quantity,unit\n");
        for r in &self.rows {
            let _ = writeln!(out, "{},{},{},{},{},{:.2},{}", q(&r.level), q(&r.part), q(&r.assembly), q(&r.material), r.layer, r.quantity, r.unit);
        }
        out
    }
}

fn area(o: &[[f64; 2]]) -> f64 {
    let n = o.len();
    let mut a = 0.0;
    for i in 0..n {
        if let (Some(p), Some(q)) = (o.get(i), o.get((i + 1) % n)) {
            a += p[0] * q[1] - q[0] * p[1];
        }
    }
    (a / 2.0).abs()
}

fn perimeter(o: &[[f64; 2]]) -> f64 {
    let n = o.len();
    (0..n).filter_map(|i| Some((o.get(i)?, o.get((i + 1) % n)?))).map(|(p, q)| (q[0] - p[0]).hypot(q[1] - p[1])).sum()
}

fn centroid(o: &[[f64; 2]]) -> [f64; 2] {
    let n = o.len().max(1) as f64;
    let s = o.iter().fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
    [s[0] / n, s[1] / n]
}

fn inside(o: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = o.len();
    let mut c = false;
    for i in 0..n {
        let (Some(a), Some(b)) = (o.get(i), o.get((i + n - 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

fn boards(ls: &[AssemblyLayer]) -> impl Iterator<Item = (usize, &AssemblyLayer)> {
    ls.iter().take(MAX_ASSEMBLY_LAYERS).filter(|l| matches!(l.category.as_str(), "board" | "tile" | "")).enumerate().map(|(i, l)| (i + 1, l))
}

fn is_stud(l: &AssemblyLayer) -> bool {
    l.material.to_ascii_lowercase().contains("stud")
}

/// The takeoff of a spec's ceilings, soffits and wall tracks.
pub fn takeoff(sp: &BuildingSpec) -> Takeoff {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut t = Takeoff::default();
    let soffit_type = |name: &Option<String>| name.as_ref().and_then(|n| sp.soffit_types.iter().find(|s| s.name.eq_ignore_ascii_case(n)));
    for c in sp.rcp.ceilings.iter().take(MAX_ITEMS) {
        let Some(ct) = c.ceiling_type.as_ref().and_then(|n| sp.ceiling_types.iter().find(|x| x.name.eq_ignore_ascii_case(n))) else { continue };
        if ct.kind == CeilingKind::Open {
            continue;
        }
        // Less the soffits hung from the structure inside it.
        let hung: f64 = sp
            .rcp
            .soffits
            .iter()
            .take(MAX_ITEMS)
            .filter(|s| s.level == c.level && soffit_type(&s.soffit_type).is_some_and(|x| x.hangs_from == Hang::Structure))
            .filter(|s| inside(&c.outline, centroid(&s.outline)))
            .map(|s| area(&s.outline))
            .sum();
        let a = (area(&c.outline) - hung).max(0.0) / (upf * upf);
        for (i, l) in boards(&ct.layers) {
            t.add(&c.level, "ceiling", &ct.name, &l.material, i, a, "sf");
        }
        for l in ct.layers.iter().take(MAX_ASSEMBLY_LAYERS) {
            if l.category == "grid" {
                let [p, q] = ct.grid_in.or(c.grid.map(|g| g.spacing_in)).unwrap_or([24.0, 48.0]);
                if p >= 1.0 && q >= 1.0 {
                    t.add(&c.level, "ceiling framing", &ct.name, &l.material, 0, a * (12.0 / p + 12.0 / q), "lf");
                }
            } else if let Some(s) = l.spacing_in.filter(|s| s.is_finite() && *s >= 1.0) {
                t.add(&c.level, "ceiling framing", &ct.name, &l.material, 0, a * 12.0 / s, "lf");
            }
        }
    }
    for sf in sp.rcp.soffits.iter().take(MAX_ITEMS) {
        let Some(st) = soffit_type(&sf.soffit_type) else { continue };
        let a = area(&sf.outline) / (upf * upf);
        let sides = sf.sides_ft.unwrap_or_else(|| perimeter(&sf.outline) / upf);
        let level_top = sp.level(&sf.level).map(|l| l.ceiling_ft);
        let ceiling =
            sf.top_ft.or_else(|| sp.rcp.ceilings.iter().filter(|c| c.level == sf.level).map(|c| c.height_ft).reduce(f64::max)).or(level_top);
        let past = if st.hangs_from == Hang::Structure { st.above_ceiling_in / 12.0 } else { 0.0 };
        let h = ceiling.map_or(0.0, |c| (c + past - sf.height_ft).max(0.0));
        for (i, l) in boards(&st.bottom) {
            t.add(&sf.level, "soffit bottom", &st.name, &l.material, i, a, "sf");
        }
        for (i, l) in boards(&st.sides) {
            t.add(&sf.level, "soffit sides", &st.name, &l.material, i, sides * h, "sf");
        }
        for l in st.framing.iter().take(MAX_ASSEMBLY_LAYERS) {
            let Some(s) = l.spacing_in.filter(|s| s.is_finite() && *s >= 1.0) else { continue };
            let len = if is_stud(l) { (sides * 12.0 / s).ceil() * h } else { a * 12.0 / s };
            t.add(&sf.level, "soffit framing", &st.name, &l.material, 0, len, "lf");
        }
    }
    for w in sp.walls.iter().take(MAX_ITEMS) {
        let Some(wt) = sp.wall_types.iter().find(|x| x.name.eq_ignore_ascii_case(&w.wall_type)) else { continue };
        let Some(head) = wt.assembly.as_ref().and_then(|a| a.head.as_ref()) else { continue };
        let len = (w.b[0] - w.a[0]).hypot(w.b[1] - w.a[1]) / upf;
        let (material, runs) = match head.track {
            HeadTrack::Single => ("top track", 1.0),
            HeadTrack::Deflection => ("slotted deflection track", 1.0),
            HeadTrack::Double => ("nested double track", 2.0),
            HeadTrack::Angle => ("head angle", 2.0),
        };
        t.add(&w.level, "top track", &wt.name, material, 0, len * runs, "lf");
        if head.bottom_track {
            t.add(&w.level, "bottom track", &wt.name, "bottom track", 0, len, "lf");
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{CeilingArea, CeilingType, Soffit, SoffitType};

    fn layer(material: &str, category: &str, spacing_in: Option<f64>) -> AssemblyLayer {
        AssemblyLayer { material: material.into(), thickness_in: 0.625, category: category.into(), spacing_in, ..AssemblyLayer::default() }
    }

    #[test]
    fn ceilings_soffits_and_hostile_values() {
        let mut sp = BuildingSpec::default();
        let level = sp.levels[0].name.clone();
        sp.units_per_foot = 1.0;
        sp.ceiling_types.push(CeilingType {
            name: "C1".into(),
            layers: vec![layer("gyp", "board", None), layer("gyp x", "board", None), layer("hat channel", "furring", Some(16.0))],
            ..CeilingType::default()
        });
        sp.soffit_types.push(SoffitType {
            name: "3".into(),
            bottom: vec![layer("gyp", "board", None)],
            sides: vec![layer("gyp", "board", None)],
            framing: vec![layer("studs", "framing", Some(16.0))],
            hangs_from: Hang::Structure,
            above_ceiling_in: 6.0,
            ..SoffitType::default()
        });
        let sq = |x0: f64, y0: f64, x1: f64, y1: f64| vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        sp.rcp.ceilings.push(CeilingArea {
            level: level.clone(),
            room: None,
            outline: sq(0.0, 0.0, 20.0, 10.0),
            height_ft: 9.0,
            grid: None,
            ceiling_type: Some("c1".into()),
            guess: None,
        });
        sp.rcp.soffits.push(Soffit {
            level: level.clone(),
            outline: sq(0.0, 8.0, 20.0, 10.0),
            height_ft: 8.0,
            soffit_type: Some("3".into()),
            top_ft: Some(9.0),
            sides_ft: Some(20.0),
            guess: None,
        });
        let t = takeoff(&sp);
        // 200 sf less the 40 sf soffit hung from the structure, two layers.
        assert!((t.total("ceiling", Some("gyp")) - 160.0).abs() < 1e-9);
        assert!((t.total("ceiling", None) - 320.0).abs() < 1e-9);
        assert!((t.total("ceiling framing", None) - 160.0 * 12.0 / 16.0).abs() < 1e-9);
        assert!((t.total("soffit bottom", None) - 40.0).abs() < 1e-9);
        // 20' of side, 1' to the ceiling and 6" past it.
        assert!((t.total("soffit sides", None) - 30.0).abs() < 1e-9);
        assert!((t.total("soffit framing", None) - 15.0 * 1.5).abs() < 1e-9);
        assert_eq!(t.csv().lines().count(), t.rows.len() + 1);
        // Hostile numbers make no rows, and nothing panics.
        sp.rcp.soffits[0].height_ft = f64::NAN;
        sp.rcp.ceilings[0].outline = vec![[f64::INFINITY, 0.0], [1.0, f64::NAN]];
        sp.ceiling_types[0].layers[2].spacing_in = Some(0.0);
        let t = takeoff(&sp);
        assert!(t.rows.iter().all(|r| r.quantity.is_finite() && r.quantity > 0.0));
    }
}
