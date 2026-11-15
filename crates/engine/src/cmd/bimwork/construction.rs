//! Normal construction (Q6621): what the trades install inside or through the building's
//! assemblies on purpose is not a clash.
//!
//! - A run (duct, pipe, conduit, tray, cable, wire, a fitting of one) wholly inside a wall, floor
//!   or slab is in its cavity: a drop down the wall its device is in, a stack in its wall, a run
//!   in a floor's bays. A run too fat for the assembly (some of its section outside) stays a clash.
//! - A run through an assembly (both ends outside, its axis through the faces), or one that ends
//!   in it (into a wall for its drop), is a penetration: listed on its own with its sleeve and its
//!   seal (a firestop where the wall's type is rated or the floor separates storeys), never a clash.
//! - Gear recessed in its wall by its own data (its schedule's MOUNTING: recessed, flush) is in
//!   the wall; a can deeper than the wall comes out of the far face and stays a clash with the
//!   reason. A through-wall unit (its words: a PTAC, a sidewall fan, a louver) is a wall opening.
//! - A breaker is inside its panel (the panel's clash speaks for it); a support is fastened to the
//!   building; a device or box in casework is hosted by it.
//!
//! The words and the code sections are data (`construction.json`); the framing members keep their
//! own hole rules ([`super::framing`]). `bim.clash {hosted: true}` reports all of it as clashes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use cadaddy_doc::{Drawing, Handle};
use cadaddy_geom::Vec3;
use serde::Deserialize;
use serde_json::{Value, json};

use super::clash::{Tri, inside, seg_tri, triangles};
use super::{El, framing, r3};
use crate::Session;

const RULES: &str = include_str!("construction.json");
/// Most rated walls looked up (hostile drawings).
const MAX_RATED: usize = 200_000;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub source: String,
    pub runs: Vec<String>,
    pub supports: Vec<String>,
    pub gear_pieces: Vec<String>,
    pub hosts: Vec<String>,
    pub cavity_hosts: Vec<String>,
    pub walls: Vec<String>,
    pub casework: Vec<String>,
    pub casework_guests: Vec<String>,
    pub through_wall_words: Vec<String>,
    pub penetrations: Vec<PenRule>,
}

/// How a penetration of one kind of assembly is closed.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PenRule {
    pub host: Vec<String>,
    /// When it needs a firestop: `rated` (the wall's type has a fire rating), `between_storeys`
    /// (a floor with a storey under it), `always`, `never`.
    pub firestop: String,
    pub seal: String,
    pub seal_firestop: String,
    pub sleeve: String,
    pub by_category: BTreeMap<String, String>,
    pub source: String,
}

pub(crate) fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

fn is(list: &[String], e: &El) -> bool {
    list.iter().any(|c| c.eq_ignore_ascii_case(&e.category))
}

/// What a pair the geometry found is, when it is not a clash to report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Normal {
    /// A run inside the cavity of a wall, floor or slab.
    InCavity,
    /// Gear set into its wall as its data says.
    Recessed,
    /// A piece inside its gear (a breaker in its panel).
    InGear,
    /// A device or box in casework.
    Hosted,
    /// A support fastened to the building.
    Attached,
    /// Through an assembly (both ends outside).
    Through,
    /// Into an assembly (one end inside: a drop's entry, a stub into a slab).
    Into,
    /// A through-wall unit in its wall opening.
    Unit,
}

impl Normal {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Normal::InCavity => "in_cavity",
            Normal::Recessed => "recessed",
            Normal::InGear => "in_gear",
            Normal::Hosted => "hosted",
            Normal::Attached => "attached",
            Normal::Through => "through",
            Normal::Into => "into",
            Normal::Unit => "unit",
        }
    }
    pub(crate) fn is_penetration(self) -> bool {
        matches!(self, Normal::Through | Normal::Into | Normal::Unit)
    }
}

/// The verdict on a pair: normal construction (which of the two is the building's), or a real
/// conflict with its reason.
#[derive(Clone, Debug)]
pub(crate) enum Verdict {
    Normal { kind: Normal, host: Handle },
    Conflict(String),
}

/// What the judging needs: the rated walls, the levels on grade, inches in drawing units, the
/// recessed-mounting words.
pub(crate) struct Ctx {
    rated: BTreeMap<Handle, String>,
    ground: BTreeSet<String>,
    inch: f64,
    recessed_words: Vec<String>,
    /// The walls' stud cavities, for a recessed can deeper than its wall's (Q6560).
    cav: crate::cmd::recess::Cavities,
}

impl Ctx {
    pub(crate) fn of(s: &Session, cmd: &str, inch: f64) -> Ctx {
        let sp = crate::cmd::bspec::current(s, cmd).ok().map(|(sp, _)| sp);
        let rated = match (s.doc(), &sp) {
            (Ok(d), Some(sp)) => crate::cmd::conduitroute::rated_walls(d, sp, MAX_RATED).into_iter().map(|w| (w.handle, w.rating)).collect(),
            _ => BTreeMap::new(),
        };
        let ground = sp
            .as_ref()
            .map(|sp| {
                let low = sp.levels.iter().map(|l| l.elevation_ft).filter(|z| z.is_finite()).fold(f64::INFINITY, f64::min);
                sp.levels.iter().filter(|l| (l.elevation_ft - low).abs() < 0.01).map(|l| l.name.trim().to_ascii_uppercase()).collect()
            })
            .unwrap_or_default();
        let recessed_words = cadaddy_mepfix::wordrules::words("equipment.in_wall_mounting_words").iter().map(|w| w.to_ascii_uppercase()).collect();
        let cav = match (s.doc(), &sp) {
            (Ok(d), Some(sp)) => crate::cmd::recess::Cavities::of(d, sp),
            _ => crate::cmd::recess::Cavities::none(),
        };
        Ctx { rated, ground, inch, recessed_words, cav }
    }

    /// The verdict on `a` (moved by `shift`) meeting `b`: none when it is a clash as the
    /// geometry found it.
    pub(crate) fn judge(&self, d: &Drawing, a: &El, b: &El, shift: Vec3, cache: &mut BTreeMap<Handle, (Vec<Tri>, bool)>) -> Option<Verdict> {
        let r = rules();
        if is(&r.gear_pieces, a) || is(&r.gear_pieces, b) {
            let host = if is(&r.gear_pieces, a) { b.handle } else { a.handle };
            return Some(Verdict::Normal { kind: Normal::InGear, host });
        }
        let building = |e: &El| matches!(e.trade, "architecture" | "structure") && !framing::is_member(e) && !framing::no_holes(e);
        let (host, guest, gshift) = match (building(a), building(b)) {
            (false, true) => (b, a, shift),
            (true, false) => (a, b, Vec3::ZERO),
            _ => return None,
        };
        let normal = |kind| Some(Verdict::Normal { kind, host: host.handle });
        if is(&r.casework, host) && (is(&r.casework_guests, guest) || guest.in_wall) {
            return normal(Normal::Hosted);
        }
        if !is(&r.hosts, host) {
            return None;
        }
        if is(&r.supports, guest) {
            return normal(Normal::Attached);
        }
        if is(&r.runs, guest) {
            return self.run_in(d, host, guest, gshift, cache).and_then(normal);
        }
        if guest.category == "equipment" && is(&r.walls, host) {
            return self.gear_in_wall(d, host, guest);
        }
        None
    }

    /// A run against an assembly: in its cavity, through it, into it, or (none) a clash.
    fn run_in(&self, d: &Drawing, host: &El, run: &El, shift: Vec3, cache: &mut BTreeMap<Handle, (Vec<Tri>, bool)>) -> Option<Normal> {
        let ax = framing::axes(d, run).shifted(shift);
        let (tris, closed) = cache.entry(host.handle).or_insert_with(|| triangles(d, host.handle, host.lo, host.hi));
        // A mesh that closes no volume (a wall drawn as bare faces) has no inside: only its faces
        // can be passed through.
        let closed = *closed;
        let tol = 0.25 * self.inch;
        let far = (host.hi - host.lo).len() + ax.len + 1.0;
        let within = |p: Vec3| closed && inside(p, tris, far);
        let half = (ax.len / 2.0 - tol).max(0.0);
        let (p, q) = (ax.c - ax.u * half, ax.c + ax.u * half);
        let (pin, qin) = (within(p), within(q));
        if pin && qin {
            // Its whole section inside too: it fits in the cavity.
            let (rd, rt) = ((ax.depth / 2.0 - tol).max(0.0), (ax.thick / 2.0 - tol).max(0.0));
            let fits = [p, ax.c, q].iter().all(|o| [*o + ax.d * rd, *o - ax.d * rd, *o + ax.t * rt, *o - ax.t * rt, *o].iter().all(|x| within(*x)));
            return (fits && is(&rules().cavity_hosts, host)).then_some(Normal::InCavity);
        }
        if pin != qin {
            // A piece ending on the assembly's face is cut there (a riser cut at each floor): the
            // run goes on through it in its next piece.
            let (end, out) = if pin { (ax.c - ax.u * (ax.len / 2.0), -ax.u) } else { (ax.c + ax.u * (ax.len / 2.0), ax.u) };
            return Some(if within(end + out * (2.0 * tol)) { Normal::Into } else { Normal::Through });
        }
        tris.iter().any(|t| seg_tri(p, q, t)).then_some(Normal::Through)
    }

    /// Gear against its wall: recessed (its data says so and it stays inside the wall's far
    /// face), a through-wall unit, or a conflict with its reason.
    fn gear_in_wall(&self, d: &Drawing, wall: &El, gear: &El) -> Option<Verdict> {
        let wa = framing::axes(d, wall);
        let tol = 0.25 * self.inch;
        let half = wa.thick / 2.0;
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for i in 0..8u8 {
            let c = Vec3::new(
                if i & 1 == 0 { gear.lo.x } else { gear.hi.x },
                if i & 2 == 0 { gear.lo.y } else { gear.hi.y },
                if i & 4 == 0 { gear.lo.z } else { gear.hi.z },
            );
            let s = (c - wa.c).dot(wa.t);
            lo = lo.min(s);
            hi = hi.max(s);
        }
        if !lo.is_finite() || !hi.is_finite() {
            return None;
        }
        let words = gear_words(d, gear.handle);
        let says = |list: &[String]| list.iter().any(|w| !w.is_empty() && words.contains(w.as_str()));
        let through = lo < -half - tol && hi > half + tol;
        // Out of the wall's far face: the face away from the one it is set on (its HOST_FACE),
        // else deeper than the wall.
        let pokes_far = match host_face(d, gear.handle).map(|o| o.dot(wa.t)).filter(|v| v.abs() > 0.3) {
            Some(o) if o > 0.0 => lo < -half - tol,
            Some(_) => hi > half + tol,
            None => hi - lo > wa.thick + tol,
        };
        let normal = |kind| Some(Verdict::Normal { kind, host: wall.handle });
        if says(&rules().through_wall_words) && through {
            return normal(Normal::Unit);
        }
        if says(&self.recessed_words) {
            if pokes_far {
                return Some(Verdict::Conflict(format!(
                    "recessed {}\" deep in a {}\" wall, deeper than the wall: it comes out of the far face (a deeper wall or chase, or mounted on the face)",
                    r3((hi - lo) / self.inch),
                    r3(wa.thick / self.inch)
                )));
            }
            // In the wall but deeper than its stud cavity: the studs and the far face's board are
            // where its back is.
            if let Some(why) = self.cav.too_deep(wall.handle, wa.thick / self.inch, (hi - lo) / self.inch) {
                return Some(Verdict::Conflict(format!(
                    "recessed: {why}, so its back is in the far face's framing and board (mounted on the face, or the wall framed deeper) ({})",
                    crate::cmd::recess::RULE
                )));
            }
            return normal(Normal::Recessed);
        }
        None
    }

    /// A penetration's record: what goes through what, its sleeve and its seal, with the rule.
    pub(crate) fn penetration(&self, d: &Drawing, kind: Normal, host: &El, guest: &El, at: Vec3) -> Value {
        let rl = rules().penetrations.iter().find(|p| p.host.iter().any(|h| h.eq_ignore_ascii_case(&host.category)));
        let rating = self.rated.get(&host.handle);
        let firestop = rl.is_some_and(|p| match p.firestop.as_str() {
            "rated" => rating.is_some(),
            "between_storeys" => !self.ground.contains(&host.level.trim().to_ascii_uppercase()),
            "always" => true,
            _ => false,
        });
        let seal = rl.map_or(String::new(), |p| {
            if !firestop {
                p.seal.clone()
            } else if let Some(r) = rating {
                format!("{} ({r})", p.seal_firestop)
            } else {
                p.seal_firestop.clone()
            }
        });
        let sleeve = rl.map_or(String::new(), |p| p.by_category.get(&guest.category).cloned().unwrap_or_else(|| p.sleeve.clone()));
        let ax = framing::axes(d, guest);
        let level = if host.level.is_empty() { guest.level.clone() } else { host.level.clone() };
        let mut v = json!({
            "kind": kind.key(), "a": guest.json(), "b": host.json(), "level": level, "at": [r3(at.x), r3(at.y), r3(at.z)],
            "size_in": r3(ax.depth.max(ax.thick) / self.inch.max(1e-9)), "firestop": firestop, "seal": seal, "sleeve": sleeve,
            "source": rl.map_or_else(|| rules().source.clone(), |p| p.source.clone()),
        });
        if let Some(r) = rating {
            v["rating"] = json!(r);
        }
        v
    }
}

/// The outward direction of the wall face a piece of equipment is set on (its `HOST_FACE`).
fn host_face(d: &Drawing, h: Handle) -> Option<Vec3> {
    let e = d.entity(h)?;
    let v = super::super::fieldview::wallgen_tags(e).into_iter().find_map(|t| t.strip_prefix("prop:HOST_FACE="))?;
    let mut it = v.split(',').map(|x| x.trim().parse::<f64>().ok().filter(|x| x.is_finite()));
    let o = Vec3::new(it.next()??, it.next()??, 0.0);
    (o.len() > 1e-9).then(|| o.normalized())
}

/// The words a piece of equipment says of itself (its kind, title and properties), upper case.
fn gear_words(d: &Drawing, h: Handle) -> String {
    let Some(e) = d.entity(h) else { return String::new() };
    let mut out = String::new();
    for t in super::super::fieldview::wallgen_tags(e) {
        let v = ["part:equipment:", "title:", "prop:", "tag:"].iter().find_map(|k| t.strip_prefix(k));
        if let Some(v) = v {
            out.push(' ');
            out.push_str(&v.replace('_', " ").to_ascii_uppercase());
        }
        if out.len() > 4_000 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_load_and_every_host_has_its_penetration_rule() {
        let r = rules();
        assert!(!r.runs.is_empty() && !r.hosts.is_empty() && !r.through_wall_words.is_empty());
        for h in &r.hosts {
            assert!(r.penetrations.iter().any(|p| p.host.contains(h)), "{h}");
        }
        for p in &r.penetrations {
            assert!(!p.seal.is_empty() && !p.sleeve.is_empty() && !p.source.is_empty(), "{p:?}");
            assert!(matches!(p.firestop.as_str(), "rated" | "between_storeys" | "always" | "never"), "{p:?}");
        }
    }
}
