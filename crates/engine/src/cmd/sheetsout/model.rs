//! The model as the sheets see it: each element's plan outline, height range, discipline and the
//! property texts its tags print, read once from the drawing.

use std::collections::BTreeMap;

use cadaddy_doc::{Drawing, EntityKind, Handle};

use super::super::propuse::{self, El, Model};

/// Most model elements put on sheets.
pub(super) const MAX_ITEMS: usize = 200_000;
/// Most mesh vertices an outline is made from.
const MAX_HULL_VERTS: usize = 20_000;

/// The drawing set's disciplines, in sheet order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Disc {
    Arch,
    Fire,
    Plumb,
    Mech,
    Elec,
}

impl Disc {
    pub const ALL: [Disc; 5] = [Disc::Arch, Disc::Fire, Disc::Plumb, Disc::Mech, Disc::Elec];
    pub fn code(self) -> &'static str {
        match self {
            Disc::Arch => "A",
            Disc::Fire => "FP",
            Disc::Plumb => "P",
            Disc::Mech => "M",
            Disc::Elec => "E",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Disc::Arch => "architectural",
            Disc::Fire => "fire protection",
            Disc::Plumb => "plumbing",
            Disc::Mech => "mechanical",
            Disc::Elec => "electrical",
        }
    }
    /// The layer prefix (NCS: `F-` for fire protection).
    pub fn prefix(self) -> &'static str {
        match self {
            Disc::Fire => "F",
            d => d.code(),
        }
    }
    pub fn parse(t: &str) -> Option<Disc> {
        let k: String = t.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
        Some(match k.as_str() {
            "a" | "arch" | "architectural" | "architecture" => Disc::Arch,
            "fp" | "f" | "fire" | "fireprotection" | "sprinkler" | "sprinklers" => Disc::Fire,
            "p" | "plumbing" | "plumb" => Disc::Plumb,
            "m" | "mech" | "mechanical" | "hvac" => Disc::Mech,
            "e" | "elec" | "electrical" | "power" | "lighting" => Disc::Elec,
            _ => return None,
        })
    }
}

/// One element as drawn on the sheets.
pub(super) struct Item {
    pub handle: Handle,
    pub kind: String,
    pub level: String,
    /// Plan outline (convex), drawing units, counter-clockwise.
    pub hull: Vec<[f64; 2]>,
    pub z: [f64; 2],
    /// Property texts by lower-case key.
    pub props: BTreeMap<String, String>,
    pub words: String,
    pub disc: Disc,
    /// `LP1-3`: its branch circuit and that circuit's panel.
    pub circuit: Option<(String, String)>,
    pub tag: Option<String>,
    pub panel: bool,
    /// Low-voltage and life-safety systems (data, fire alarm, security…), not power.
    pub systems: bool,
    /// An air terminal (diffuser, grille).
    pub terminal: bool,
    pub sprinkler: bool,
}

impl Item {
    pub fn p(&self, key: &str) -> Option<&str> {
        self.props.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }
    pub fn first(&self, keys: &[&str]) -> Option<&str> {
        keys.iter().find_map(|k| self.p(k))
    }
    pub fn center(&self) -> [f64; 2] {
        let n = self.hull.len().max(1) as f64;
        let (x, y) = self.hull.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
        [x / n, y / n]
    }
    /// Plan box: min, max.
    pub fn bounds(&self) -> ([f64; 2], [f64; 2]) {
        bounds(&self.hull)
    }
    /// The outline's long direction (unit) and its long and short sizes.
    pub fn axis(&self) -> ([f64; 2], f64, f64) {
        let mut best = ([1.0, 0.0], 0.0);
        for (i, a) in self.hull.iter().enumerate() {
            let b = self.hull.get((i + 1) % self.hull.len()).copied().unwrap_or(*a);
            let l = (b[0] - a[0]).hypot(b[1] - a[1]);
            if l > best.1 {
                best = ([(b[0] - a[0]) / l, (b[1] - a[1]) / l], l);
            }
        }
        let (u, _) = best;
        let n = [-u[1], u[0]];
        let span = |dir: [f64; 2]| {
            let (lo, hi) = self.hull.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                let t = p[0] * dir[0] + p[1] * dir[1];
                (lo.min(t), hi.max(t))
            });
            if lo.is_finite() { hi - lo } else { 0.0 }
        };
        (u, span(u), span(n))
    }
}

pub(super) fn bounds(pts: &[[f64; 2]]) -> ([f64; 2], [f64; 2]) {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in pts.iter().filter(|p| super::pen::ok(**p)) {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    (lo, hi)
}

/// The convex hull, counter-clockwise (monotone chain).
pub(super) fn hull(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    pts.retain(|p| super::pen::ok(*p));
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for p in &pts {
        push_hull(&mut lower, *p);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for p in pts.iter().rev() {
        push_hull(&mut upper, *p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn push_hull(chain: &mut Vec<[f64; 2]>, p: [f64; 2]) {
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    while chain.len() >= 2 {
        let (Some(o), Some(a)) = (chain.get(chain.len() - 2).copied(), chain.last().copied()) else { break };
        if cross(o, a, p) > 0.0 {
            break;
        }
        chain.pop();
    }
    chain.push(p);
}

fn has_any(w: &str, words: &[&str]) -> bool {
    words.iter().any(|x| w.contains(x))
}

const SYSTEM_WORDS: &[&str] = &[
    "data",
    "telephone",
    "voice",
    "fire alarm",
    "smoke detector",
    "heat detector",
    "horn",
    "strobe",
    "pull station",
    "security",
    "camera",
    "card reader",
    "speaker",
    "wireless access",
    "intercom",
    "nurse call",
    "access control",
];

fn item(d: &Drawing, el: &El) -> Item {
    let mut pts = Vec::new();
    if let EntityKind::Mesh(m) = &el.e.kind {
        pts.extend(m.vertices.iter().take(MAX_HULL_VERTS).map(|v| [v.x, v.y]));
    }
    let mut hull = hull(pts);
    if hull.len() < 3 {
        let (lo, hi) = (el.lo, el.hi);
        hull = vec![[lo[0], lo[1]], [hi[0], lo[1]], [hi[0], hi[1]], [lo[0], hi[1]]];
    }
    let _ = d;
    let mut props = BTreeMap::new();
    for (k, p, _) in &el.props {
        if p.has_value() {
            let t = p.text().trim().to_string();
            if !t.is_empty() {
                props.entry(k.to_ascii_lowercase()).or_insert(t);
            }
        }
    }
    let words = el.words();
    let sprinkler = propuse::is_sprinkler(el);
    let fire_pipe = el.kind == "pipe"
        && has_any(&format!("{words} {}", el.text("system").unwrap_or_default().to_ascii_lowercase()), &["fire", "sprink", "standpipe"]);
    let disc = if sprinkler || fire_pipe {
        Disc::Fire
    } else if el.kind == "equipment" && propuse::is_water_heater(el) {
        Disc::Plumb
    } else if el.kind == "equipment" && propuse::is_panel(el) {
        Disc::Elec
    } else if propuse::is_terminal(el) && el.kind != "device" {
        Disc::Mech
    } else {
        match propuse::trade_of(&el.kind) {
            "electrical" => Disc::Elec,
            "mechanical" => Disc::Mech,
            "plumbing" => Disc::Plumb,
            _ => Disc::Arch,
        }
    };
    let circuit = propuse::circuit_of(el).map(|c| (c.key, c.panel));
    let systems = el.kind == "device" && !propuse::is_receptacle(el) && has_any(&words, SYSTEM_WORDS);
    Item {
        handle: el.handle(),
        kind: el.kind.clone(),
        level: el.level(),
        hull,
        z: [el.lo[2], el.hi[2]],
        props,
        words,
        disc,
        circuit,
        tag: el.tag(),
        panel: propuse::is_panel(el),
        systems,
        terminal: propuse::is_terminal(el),
        sprinkler,
    }
}

/// Every element of the drawing's model, with its units per foot.
pub(super) fn items(d: &Drawing) -> (Vec<Item>, f64) {
    let m = Model::new(d);
    let items = m.els.iter().take(MAX_ITEMS).map(|el| item(d, el)).collect();
    (items, m.upf)
}

/// Feet and inches: `12'-6"`.
pub(super) fn ft_in(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let sign = if ft < 0.0 { "-" } else { "" };
    let total = (ft.abs() * 12.0).round() as i64;
    format!("{sign}{}'-{}\"", total / 12, total % 12)
}
