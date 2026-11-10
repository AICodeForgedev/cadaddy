//! The framing members in `bim.clash` (Q6504): the studs, joists, I-joists, trusses, rim boards,
//! headers and beams the framing trade lays out, tested against the MEP that crosses them, each
//! hit judged by the member's hole rules (rules data, `framing.json`; the I-joist web and truss
//! chord rules are the conduit router's).
//!
//! A run that crosses a member through its thin side bores it: the hole (the run's outside size
//! plus the rules' allowance) is allowed where it is within the member's limits (an I-joist's
//! web hole zone and its distance from the bearing, a third of a sawn joist's depth and 2" off
//! its edges, 40% or 60% of a wood stud, a steel stud's punchout, between a truss's chords) and
//! a conflict otherwise. A run along a member, one through its depth (through an I-joist's
//! flanges), anything through a header or a beam, and equipment standing in a member are
//! conflicts. Every verdict carries the rule it was judged by and the rule's source.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec3;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{El, r3};
use crate::cmd::conduitroute::FramingRules;

const RULES: &str = include_str!("framing.json");
/// Vertices and faces of one element looked at for its axes.
const MAX_VERTS: usize = 20_000;
/// The trade name the framing's pieces carry as their system.
pub(crate) const FRAMING: &str = "framing";
/// The tradepractice rule whose words say a wall type bears (the joist bearing rule's).
const BEARING_RULE: &str = "S-FLOOR-JOIST-BEARING";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Bored {
    pub max_frac: f64,
    pub edge_in: f64,
    pub hole_over_in: f64,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct StudRule {
    pub bearing_frac: f64,
    pub non_bearing_frac: f64,
    pub edge_in: f64,
    pub hole_over_in: f64,
    pub unknown_bearing: String,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PunchRule {
    pub punchout_in: f64,
    pub clearance_in: f64,
    pub source: String,
}

/// `framing.json`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct HoleRules {
    pub members: Vec<String>,
    pub members_source: String,
    pub no_holes: Vec<String>,
    pub no_holes_source: String,
    pub runs: Vec<String>,
    pub parallel_deg: f64,
    pub truss_words: Vec<String>,
    pub sawn_words: Vec<String>,
    pub steel_words: Vec<String>,
    pub sawn_joist: Bored,
    pub rim: Bored,
    pub wood_stud: StudRule,
    pub steel_stud: PunchRule,
}

pub(crate) fn rules() -> &'static HoleRules {
    static R: OnceLock<HoleRules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// Is `e` one of the framing members the check tests (as [`super::clash`] names them: their
/// piece's kind as the category, `framing` the system)?
pub(crate) fn is_member(e: &El) -> bool {
    e.trade == "structure" && e.system == FRAMING && rules().members.iter().any(|m| m == &e.category)
}

/// A structural element no field hole is allowed in (a header, a beam, a column).
pub(crate) fn no_holes(e: &El) -> bool {
    e.trade == "structure" && rules().no_holes.iter().any(|m| e.category.eq_ignore_ascii_case(m))
}

/// The MEP elements the framing is tested against: every MEP trade's, except a device or a box
/// set in a wall as its trade sets it there (fixed to the stud beside it; `hosted` tests them too).
pub(crate) fn crosses(e: &El, hosted: bool) -> bool {
    matches!(e.trade, "mechanical" | "plumbing" | "electrical" | "fire" | "lowvoltage")
        && (hosted || !(e.in_wall || e.category == "device" || e.category == "breaker"))
}

/// What a member is, for its hole rules.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Kind {
    IJoist,
    SawnJoist,
    Truss,
    Rim,
    /// A wood stud; whether its wall bears (none: its type says neither).
    WoodStud(Option<bool>),
    SteelStud,
    NoHoles,
}

/// A member: its kind and what the trade layout wrote on it.
#[derive(Clone, Debug)]
pub(crate) struct Member {
    pub kind: Kind,
    pub piece: String,
    pub ty: String,
    pub item: String,
    pub wall_type: String,
}

impl Member {
    pub(crate) fn json(&self) -> Value {
        let mut v = json!({"kind": self.piece, "type": self.ty, "item": self.item});
        if !self.wall_type.is_empty() {
            v["wall_type"] = json!(self.wall_type);
        }
        v
    }
}

/// A wall type's bearing, by the joist bearing rule's words in its name, tag, description and
/// framing layers: `Some(false)` non-bearing, `Some(true)` bearing, none when it says neither.
pub(crate) fn wall_bearing(sp: &cadaddy_walk3d::BuildingSpec) -> BTreeMap<String, Option<bool>> {
    let rule = crate::cmd::provenance::rule_data(BEARING_RULE);
    let words = |k: &str| -> Vec<String> {
        rule.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_ascii_uppercase).take(64).collect())
            .unwrap_or_default()
    };
    let (bearing, non) = (words("bearing_words"), words("non_bearing_words"));
    let mut out = BTreeMap::new();
    for t in sp.wall_types.iter().take(10_000) {
        let mut x = t.name.to_ascii_uppercase();
        if let Some(a) = &t.assembly {
            x.push(' ');
            x.push_str(&a.tag.to_ascii_uppercase());
            x.push(' ');
            x.push_str(&a.description.to_ascii_uppercase());
            for l in a.layers.iter().take(cadaddy_walk3d::MAX_ASSEMBLY_LAYERS).filter(|l| l.category.eq_ignore_ascii_case("framing")) {
                x.push(' ');
                x.push_str(&l.material.to_ascii_uppercase());
            }
        }
        let v = if non.iter().any(|w| x.contains(w.as_str())) {
            Some(false)
        } else if bearing.iter().any(|w| x.contains(w.as_str())) {
            Some(true)
        } else {
            None
        };
        out.insert(t.name.to_ascii_uppercase(), v);
    }
    out
}

/// The member `e` is (its piece's type and item; a stud's wall's bearing from `bearing`).
pub(crate) fn member(d: &Drawing, e: &El, bearing: &BTreeMap<String, Option<bool>>) -> Member {
    let rl = rules();
    let (ty, item, wall_type) = d
        .entity(e.handle)
        .map(|x| {
            let ps = cadaddy_doc::props::read(&x.common.xdata);
            let t = |k: &str| ps.get(k).map(cadaddy_doc::props::Prop::text).unwrap_or_default();
            (t("type"), t("item"), t("wall_type"))
        })
        .unwrap_or_default();
    let text = format!(" {} {} ", ty, item).to_ascii_uppercase();
    let has = |ws: &[String]| ws.iter().any(|w| text.contains(w.to_ascii_uppercase().as_str()));
    let kind = match e.category.as_str() {
        _ if no_holes(e) => Kind::NoHoles,
        "truss" => Kind::Truss,
        "joist" if has(&rl.truss_words) => Kind::Truss,
        "joist" if has(&rl.sawn_words) => Kind::SawnJoist,
        "joist" => Kind::IJoist,
        "rim" => Kind::Rim,
        "stud" if has(&rl.steel_words) => Kind::SteelStud,
        "stud" => Kind::WoodStud(bearing.get(&wall_type.to_ascii_uppercase()).copied().flatten()),
        _ => Kind::NoHoles,
    };
    Member { kind, piece: e.category.clone(), ty, item, wall_type }
}

/// An element's own axes: `u` along its length, `d` across its deeper side, `t` across its thin
/// side, with its centre and its extents along each (drawing units).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Axes {
    pub c: Vec3,
    pub u: Vec3,
    pub d: Vec3,
    pub t: Vec3,
    pub len: f64,
    pub depth: f64,
    pub thick: f64,
}

impl Axes {
    pub(crate) fn shifted(self, by: Vec3) -> Axes {
        Axes { c: self.c + by, ..self }
    }
}

/// The axes of `e`: its mesh's longest edge is its length; its longest edge square to that, its
/// depth (a box: its sides, longest first).
pub(crate) fn axes(d: &Drawing, e: &El) -> Axes {
    let mesh = d.entity(e.handle).and_then(|x| if let EntityKind::Mesh(m) = &x.kind { Some(m) } else { None });
    let verts: Vec<Vec3> = mesh.map(|m| m.vertices.iter().take(MAX_VERTS).copied().collect()).unwrap_or_default();
    let mut edges: Vec<Vec3> = Vec::new();
    if let Some(m) = mesh {
        for f in m.faces.iter().take(MAX_VERTS) {
            for (k, a) in f.v.iter().enumerate() {
                let b = f.v.get((k + 1) % f.v.len().max(1)).copied().unwrap_or(*a);
                if let (Some(p), Some(q)) = (verts.get(*a as usize), verts.get(b as usize)) {
                    let v = *q - *p;
                    if v.is_finite() && v.len() > 0.0 {
                        edges.push(v);
                    }
                }
            }
        }
    }
    let longest = |vs: &mut dyn Iterator<Item = Vec3>| vs.max_by(|a, b| a.len().total_cmp(&b.len()));
    let from_box = || {
        let s = e.hi - e.lo;
        let mut ax = [(s.x, Vec3::new(1.0, 0.0, 0.0)), (s.y, Vec3::new(0.0, 1.0, 0.0)), (s.z, Vec3::Z)];
        ax.sort_by(|a, b| b.0.total_cmp(&a.0));
        (ax[0].1, ax[1].1)
    };
    let (u, dd) = match longest(&mut edges.iter().copied()) {
        Some(l) if verts.len() >= 4 => {
            let u = l.normalized();
            let perp = longest(&mut edges.iter().copied().filter(|v| (v.normalized().dot(u)).abs() < 0.2)).map(Vec3::normalized);
            let dd = perp.unwrap_or(if u.z.abs() > 0.9 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::Z });
            (u, dd)
        }
        _ => from_box(),
    };
    let t = u.cross(dd).normalized();
    let dd = t.cross(u).normalized();
    // A depth axis pointing up (or along +x / +y): "from the bottom" reads the same either way.
    let dd = if dd.z < -1e-9 || (dd.z.abs() <= 1e-9 && dd.x + dd.y < 0.0) { -dd } else { dd };
    let verts: Vec<Vec3> = verts.into_iter().filter(|v| v.is_finite()).collect();
    let corners: Vec<Vec3> = if verts.len() >= 4 {
        verts
    } else {
        (0..8u8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { e.lo.x } else { e.hi.x },
                    if i & 2 == 0 { e.lo.y } else { e.hi.y },
                    if i & 4 == 0 { e.lo.z } else { e.hi.z },
                )
            })
            .collect()
    };
    let span = |a: Vec3| corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.dot(a)), hi.max(p.dot(a))));
    let (su, sd, st) = (span(u), span(dd), span(t));
    let c = u * ((su.0 + su.1) / 2.0) + dd * ((sd.0 + sd.1) / 2.0) + t * ((st.0 + st.1) / 2.0);
    let ok = |s: (f64, f64)| if (s.1 - s.0).is_finite() { (s.1 - s.0).max(0.0) } else { 0.0 };
    Axes { c, u, d: dd, t, len: ok(su), depth: ok(sd), thick: ok(st) }
}

/// One hit's verdict.
#[derive(Clone, Debug)]
pub(crate) struct Verdict {
    pub allowed: bool,
    pub why: String,
    pub rule: String,
    pub source: String,
    pub hole_in: Option<f64>,
    pub max_hole_in: Option<f64>,
    pub from_end_ft: Option<f64>,
    /// Where the run crosses the member's middle.
    pub at: Vec3,
}

impl Verdict {
    pub(crate) fn json(&self) -> Value {
        let mut v = json!({
            "verdict": if self.allowed { "allowed" } else { "conflict" }, "why": self.why, "rule": self.rule, "source": self.source,
        });
        for (k, x) in [("hole_in", self.hole_in), ("max_hole_in", self.max_hole_in), ("from_end_ft", self.from_end_ft)] {
            if let Some(x) = x.filter(|x| x.is_finite()) {
                v[k] = json!(r3(x));
            }
        }
        v
    }
}

fn frac(v: f64) -> String {
    cadaddy_mepfix::walls::frac(v, true)
}

/// Judge the run (or element) `r` (its category, its axes) through member `m` (its axes), in a
/// drawing of `inch` units an inch.
pub(crate) fn judge(m: &Member, ma: &Axes, category: &str, ra: &Axes, inch: f64, fr: &FramingRules) -> Verdict {
    let rl = rules();
    let inch = if inch.is_finite() && inch > 0.0 { inch } else { 1.0 };
    let mut v = Verdict {
        allowed: false,
        why: String::new(),
        rule: String::new(),
        source: String::new(),
        hole_in: None,
        max_hole_in: None,
        from_end_ft: None,
        at: ra.c,
    };
    let what = m.item.clone();
    if m.kind == Kind::NoHoles {
        v.why = format!("no field hole is cut in a {}: reroute it under or beside it, or the engineer details a hole", m.piece);
        v.rule = "no field holes".into();
        v.source = rl.no_holes_source.clone();
        return v;
    }
    if !rl.runs.iter().any(|x| x == category) {
        v.why = format!("the {category} stands in the {}: set it in the bay beside the member", m.piece);
        v.rule = "equipment and boxes stand clear of the framing".into();
        v.source = rl.members_source.clone();
        return v;
    }
    let (cu, cd, ct) = (ra.u.dot(ma.u).abs(), ra.u.dot(ma.d).abs(), ra.u.dot(ma.t).abs());
    let parallel = rl.parallel_deg.clamp(0.0, 89.0).to_radians().cos();
    if cu >= parallel {
        v.why = format!("the {category} runs along inside the {}: keep it in the bay beside the member", m.piece);
        v.rule = "a run crosses a member, never along it".into();
        v.source = rl.members_source.clone();
        return v;
    }
    if cd > ct {
        v.why = format!(
            "the {category} goes through the {}'s depth (through its edges{}): cross it through its thin side, or keep to the bay beside it",
            m.piece,
            if m.kind == Kind::IJoist { " and flanges" } else { "" }
        );
        v.rule = "a hole is bored through a member's thin side".into();
        v.source = rl.members_source.clone();
        return v;
    }
    // Where the run's centreline crosses the member's middle plane (square to its thin side).
    let denom = ra.u.dot(ma.t);
    let s = if denom.abs() > 1e-12 { (ma.c - ra.c).dot(ma.t) / denom } else { 0.0 };
    let p = ra.c + ra.u * s;
    v.at = p;
    let depth_in = ma.depth / inch;
    // The hole's centre from the member's low edge (inches), and its distance from the member's
    // nearer end (its bearing), feet.
    let c_in = (p - ma.c).dot(ma.d) / inch + depth_in / 2.0;
    let along = (p - ma.c).dot(ma.u) + ma.len / 2.0;
    let end_ft = (along.min(ma.len - along).max(0.0) / inch / 12.0).max(0.0);
    let size_in = ra.depth.max(ra.thick) / inch;
    let bored = |v: &mut Verdict, hole: f64, max: f64, edge: f64, rule: &str, source: &str| {
        v.hole_in = Some(hole);
        v.max_hole_in = Some(max);
        v.rule = rule.to_string();
        v.source = source.to_string();
        let edges = (c_in - hole / 2.0).min(depth_in - c_in - hole / 2.0);
        if hole > max + 1e-6 {
            v.why = format!("a {} hole is over the {} limit of {} ({rule})", frac(hole), what, frac(max));
        } else if edges < edge - 1e-6 {
            v.why =
                format!("the {} hole's edge is {} from the {}'s edge: it needs {} ({rule})", frac(hole), frac(edges.max(0.0)), m.piece, frac(edge));
        } else {
            v.allowed = true;
            v.why = format!("a {} hole bored through the {} ({rule})", frac(hole), what);
        }
    };
    match &m.kind {
        Kind::IJoist => {
            let hole = size_in + fr.hole_over_in.max(0.0);
            let keep = fr.flange_in.max(0.0) + fr.edge_in.max(0.0);
            v.hole_in = Some(hole);
            v.max_hole_in = Some((depth_in - 2.0 * keep).max(0.0).min(fr.min_end_ft.last().map_or(0.0, |r| r[0])));
            v.from_end_ft = Some(end_ft);
            v.rule = format!(
                "I-joist web hole: {} flanges and {} of web kept, the larger the hole the farther from the bearing",
                frac(fr.flange_in),
                frac(fr.edge_in)
            );
            v.source = fr.source.clone();
            match fr.min_end(hole) {
                _ if c_in - hole / 2.0 < keep - 1e-6 || c_in + hole / 2.0 > depth_in - keep + 1e-6 => {
                    v.why = format!("the {} hole would cut a flange of the {} (outside the web's hole zone)", frac(hole), what);
                }
                None => v.why = format!("a {} hole is bigger than the {}'s web takes", frac(hole), what),
                Some(need) if end_ft + 1e-9 < need => {
                    v.why = format!("the {} hole is {:.1} ft from the {}'s bearing: it needs {:.1} ft", frac(hole), end_ft, what, need);
                }
                Some(_) => {
                    v.allowed = true;
                    v.why = format!("a {} hole in the {}'s web, {:.1} ft from its bearing", frac(hole), what, end_ft);
                }
            }
        }
        Kind::Truss => {
            let chord = fr.truss_chord_in.max(0.0);
            v.rule = format!("truss: through its open web between the chords ({} each), the chords never bored", frac(chord));
            v.source = fr.source.clone();
            v.hole_in = Some(0.0);
            v.max_hole_in = Some((depth_in - 2.0 * chord).max(0.0));
            if c_in - size_in / 2.0 < chord - 1e-6 || c_in + size_in / 2.0 > depth_in - chord + 1e-6 {
                v.why = format!("the {category} would bore a chord of the {}: pass it between the chords", what);
            } else {
                v.allowed = true;
                v.why = format!("through the {}'s open web, between its chords (the web members' places are the truss drawings')", what);
            }
        }
        Kind::SawnJoist => {
            let r = &rl.sawn_joist;
            v.from_end_ft = Some(end_ft);
            bored(
                &mut v,
                size_in + r.hole_over_in.max(0.0),
                depth_in * r.max_frac.max(0.0),
                r.edge_in.max(0.0),
                "sawn joist: hole up to 1/3 of the depth, 2\" off its edges",
                &r.source,
            );
        }
        Kind::Rim => {
            let r = &rl.rim;
            bored(
                &mut v,
                size_in + r.hole_over_in.max(0.0),
                depth_in * r.max_frac.max(0.0),
                r.edge_in.max(0.0),
                "rim board: hole up to 1/3 of its depth",
                &r.source,
            );
        }
        Kind::WoodStud(b) => {
            let r = &rl.wood_stud;
            let bears = b.unwrap_or(!r.unknown_bearing.eq_ignore_ascii_case("non-bearing"));
            let f = if bears { r.bearing_frac } else { r.non_bearing_frac };
            let rule = match b {
                Some(true) => format!("wood stud in a bearing wall: hole up to {}% of its width", r3(f * 100.0)),
                Some(false) => format!("wood stud in a non-bearing wall: hole up to {}% of its width", r3(f * 100.0)),
                None => format!(
                    "wood stud: hole up to {}% of its width (its wall type says neither bearing nor non-bearing: taken as {}; a non-bearing partition allows {}%)",
                    r3(f * 100.0),
                    if bears { "bearing" } else { "non-bearing" },
                    r3(r.non_bearing_frac * 100.0)
                ),
            };
            bored(&mut v, size_in + r.hole_over_in.max(0.0), depth_in * f.max(0.0), r.edge_in.max(0.0), &rule, &r.source);
        }
        Kind::SteelStud => {
            let r = &rl.steel_stud;
            let need = size_in + r.clearance_in.max(0.0);
            let punch = r.punchout_in.max(0.0);
            v.hole_in = Some(need);
            v.max_hole_in = Some(punch);
            v.rule = format!("steel stud: through its {} punchouts in the middle of the web", frac(punch));
            v.source = r.source.clone();
            if need > punch + 1e-6 {
                v.why = format!(
                    "the {category} ({} with its clearance) is over the {}'s {} punchout: a field-cut hole by the stud maker's limits, or reroute",
                    frac(need),
                    what,
                    frac(punch)
                );
            } else if (c_in - depth_in / 2.0).abs() > (punch - need) / 2.0 + 1e-6 {
                v.why = format!("the {category} is off the {}'s punchouts (in the middle of the web): move it to them", what);
            } else {
                v.allowed = true;
                v.why = format!("through the {}'s punchout", what);
            }
        }
        Kind::NoHoles => {}
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(category: &str) -> El {
        El {
            handle: cadaddy_doc::Handle(1),
            trade: "structure",
            category: category.into(),
            system: FRAMING.into(),
            level: String::new(),
            lo: Vec3::ZERO,
            hi: Vec3::ZERO,
            run: String::new(),
            in_wall: false,
        }
    }

    fn member(kind: Kind, piece: &str, item: &str) -> Member {
        Member { kind, piece: piece.into(), ty: String::new(), item: item.into(), wall_type: String::new() }
    }

    /// A joist along y, 2 1/2" wide (x), `depth` deep from z = 0, 20' long from y = 0 (inches).
    fn joist(depth: f64) -> Axes {
        Axes {
            c: Vec3::new(0.0, 120.0, depth / 2.0),
            u: Vec3::new(0.0, 1.0, 0.0),
            d: Vec3::Z,
            t: Vec3::new(1.0, 0.0, 0.0),
            len: 240.0,
            depth,
            thick: 2.5,
        }
    }

    /// A round run along x of `dia` at height `z`, at y.
    fn run_x(dia: f64, y: f64, z: f64) -> Axes {
        Axes { c: Vec3::new(0.0, y, z), u: Vec3::new(1.0, 0.0, 0.0), d: Vec3::Z, t: Vec3::new(0.0, -1.0, 0.0), len: 400.0, depth: dia, thick: dia }
    }

    fn fr() -> FramingRules {
        crate::cmd::conduitroute::rules().framing.clone()
    }

    #[test]
    fn rules_load_with_their_sources() {
        let r = rules();
        assert!(r.members.iter().any(|m| m == "stud") && r.members.iter().any(|m| m == "joist"));
        assert!(r.wood_stud.bearing_frac > 0.0 && r.sawn_joist.max_frac > 0.0 && r.steel_stud.punchout_in > 0.0);
        for s in [&r.sawn_joist.source, &r.rim.source, &r.wood_stud.source, &r.steel_stud.source, &r.no_holes_source, &r.members_source] {
            assert!(s.len() > 20, "{s}");
        }
        assert!(fr().flange_in > 0.0 && !fr().min_end_ft.is_empty(), "the router's I-joist rules");
        assert!(is_member(&el("joist")) && !is_member(&el("sheet")) && !is_member(&El { system: "drywall".into(), ..el("stud") }));
    }

    #[test]
    fn an_i_joist_takes_a_web_hole_but_not_a_flange_one_nor_one_near_its_bearing() {
        let m = member(Kind::IJoist, "joist", "11 7/8\" I-joist");
        let j = joist(11.875);
        let ok = judge(&m, &j, "pipe", &run_x(2.0, 120.0, 5.94), 1.0, &fr());
        assert!(ok.allowed, "{ok:?}");
        assert!((ok.hole_in.unwrap() - 2.375).abs() < 1e-9 && ok.from_end_ft.unwrap() > 9.9, "{ok:?}");
        let flange = judge(&m, &j, "pipe", &run_x(2.0, 120.0, 10.9), 1.0, &fr());
        assert!(!flange.allowed && flange.why.contains("flange"), "{flange:?}");
        let near = judge(&m, &j, "pipe", &run_x(2.0, 6.0, 5.94), 1.0, &fr());
        assert!(!near.allowed && near.why.contains("bearing"), "{near:?}");
        let big = judge(&m, &j, "duct", &run_x(8.0, 120.0, 5.94), 1.0, &fr());
        assert!(!big.allowed, "{big:?}");
        // Along the joist, and down through its flanges: conflicts whatever the size.
        let along = Axes { u: Vec3::new(0.0, 1.0, 0.0), t: Vec3::new(1.0, 0.0, 0.0), ..run_x(1.0, 120.0, 5.94) };
        assert!(!judge(&m, &j, "conduit", &along, 1.0, &fr()).allowed);
        let down = Axes { u: Vec3::Z, d: Vec3::new(1.0, 0.0, 0.0), t: Vec3::new(0.0, 1.0, 0.0), ..run_x(1.0, 120.0, 5.94) };
        let v = judge(&m, &j, "pipe", &down, 1.0, &fr());
        assert!(!v.allowed && v.why.contains("depth"), "{v:?}");
        // Equipment standing in a member is never a bore.
        assert!(!judge(&m, &j, "equipment", &run_x(1.0, 120.0, 5.94), 1.0, &fr()).allowed);
    }

    #[test]
    fn sawn_joists_trusses_studs_and_headers_by_their_own_rules() {
        // A 2x10 (9 1/4"): a 2" pipe's 2 3/8" hole is within 1/3 (3.08"), in the middle.
        let sawn = member(Kind::SawnJoist, "joist", "2x10 joist");
        assert!(judge(&sawn, &joist(9.25), "pipe", &run_x(2.0, 120.0, 4.625), 1.0, &fr()).allowed);
        // 3" is over the third; 1" 1/2" off the top is too near its edge.
        assert!(!judge(&sawn, &joist(9.25), "pipe", &run_x(3.0, 120.0, 4.625), 1.0, &fr()).allowed);
        let edge = judge(&sawn, &joist(9.25), "pipe", &run_x(1.0, 120.0, 8.0), 1.0, &fr());
        assert!(!edge.allowed && edge.why.contains("edge"), "{edge:?}");
        // A truss 24" deep: between the 3 1/2" chords, not through them.
        let truss = member(Kind::Truss, "truss", "roof truss");
        assert!(judge(&truss, &joist(24.0), "duct", &run_x(10.0, 120.0, 12.0), 1.0, &fr()).allowed);
        let chord = judge(&truss, &joist(24.0), "duct", &run_x(10.0, 120.0, 20.0), 1.0, &fr());
        assert!(!chord.allowed && chord.why.contains("chord"), "{chord:?}");
        // A 2x6 stud (vertical, 5 1/2" across the wall along y, 1 1/2" along the wall along x).
        let stud = Axes {
            c: Vec3::new(0.0, 2.75, 48.0),
            u: Vec3::Z,
            d: Vec3::new(0.0, 1.0, 0.0),
            t: Vec3::new(1.0, 0.0, 0.0),
            len: 96.0,
            depth: 5.5,
            thick: 1.5,
        };
        let along_wall = |dia: f64| Axes { c: Vec3::new(0.0, 2.75, 48.0), ..run_x(dia, 2.75, 48.0) };
        let bearing = member(Kind::WoodStud(Some(true)), "stud", "2x6 wood stud");
        let unknown = member(Kind::WoodStud(None), "stud", "2x6 wood stud");
        let partition = member(Kind::WoodStud(Some(false)), "stud", "2x6 wood stud");
        assert!(judge(&bearing, &stud, "conduit", &along_wall(1.0), 1.0, &fr()).allowed);
        // 2 1/2": 2 7/8" hole: over 40% (2.2") but within 60% (3.3").
        assert!(!judge(&bearing, &stud, "conduit", &along_wall(2.5), 1.0, &fr()).allowed);
        let u = judge(&unknown, &stud, "conduit", &along_wall(2.5), 1.0, &fr());
        assert!(!u.allowed && u.rule.contains("taken as bearing"), "{u:?}");
        assert!(judge(&partition, &stud, "conduit", &along_wall(2.5), 1.0, &fr()).allowed);
        // Across the wall, through the stud's depth: a conflict.
        let across =
            Axes { c: Vec3::new(0.0, 2.75, 48.0), u: Vec3::new(0.0, 1.0, 0.0), d: Vec3::Z, t: Vec3::new(1.0, 0.0, 0.0), ..run_x(1.0, 2.75, 48.0) };
        assert!(!judge(&bearing, &stud, "pipe", &across, 1.0, &fr()).allowed);
        // A steel stud: a 1" conduit through its 1 1/2" punchout; a 2" pipe needs a field-cut hole.
        let steel = member(Kind::SteelStud, "stud", "3 5/8\" 20 ga steel stud");
        let st = Axes { depth: 3.625, c: Vec3::new(0.0, 1.8125, 48.0), ..stud };
        let at = |dia: f64| Axes { c: Vec3::new(0.0, 1.8125, 48.0), ..run_x(dia, 1.8125, 48.0) };
        assert!(judge(&steel, &st, "conduit", &at(1.0), 1.0, &fr()).allowed);
        assert!(!judge(&steel, &st, "pipe", &at(2.0), 1.0, &fr()).allowed);
        // Headers and beams: never bored.
        let header = member(Kind::NoHoles, "header", "header");
        let h = judge(&header, &joist(9.25), "conduit", &run_x(0.5, 120.0, 4.6), 1.0, &fr());
        assert!(!h.allowed && h.source.contains("engineer"), "{h:?}");
    }

    #[test]
    fn hostile_geometry_never_panics() {
        let m = member(Kind::IJoist, "joist", "I-joist");
        let nan =
            Axes { c: Vec3::new(f64::NAN, 0.0, 0.0), u: Vec3::ZERO, d: Vec3::ZERO, t: Vec3::ZERO, len: f64::NAN, depth: f64::INFINITY, thick: -1.0 };
        let _ = judge(&m, &nan, "pipe", &nan, f64::NAN, &fr());
        let _ = judge(&m, &joist(0.0), "pipe", &run_x(0.0, 0.0, 0.0), 0.0, &FramingRules::default());
        let mut d = Drawing::new_imperial();
        let _ = axes(&d, &el("joist"));
        let _ = axes(&d, &El { lo: Vec3::new(f64::NAN, 0.0, 0.0), ..el("stud") });
        // A broken mesh: a NaN vertex, an index past the end, an empty face.
        let mesh = cadaddy_doc::Mesh {
            vertices: vec![Vec3::new(f64::NAN, 0.0, 0.0), Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0)],
            faces: vec![cadaddy_doc::MeshFace { v: vec![0, 1, 2, 9], hidden: 0 }, cadaddy_doc::MeshFace { v: Vec::new(), hidden: 0 }],
            ..cadaddy_doc::Mesh::default()
        };
        let h = d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), EntityKind::Mesh(mesh)).unwrap();
        let a = axes(&d, &El { handle: h, ..el("stud") });
        assert!(a.len.is_finite() && a.depth.is_finite() && a.thick.is_finite(), "{a:?}");
    }
}
