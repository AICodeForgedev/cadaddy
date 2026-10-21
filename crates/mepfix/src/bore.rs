//! Holes through framing for raceways, sleeves and pipes: the bit for each crossing, the member's
//! allowed hole, and the nearest fitting option when it doesn't fit (one 3" sleeve → several 2").
//!
//! Flags only: these are the usual prescriptive limits (sawn joists: hole ≤ 1/3 of the depth and
//! ≥ 2" from either edge; sawn studs: ≤ 40% of the width when bearing, 60% when not; metal studs:
//! their punchouts; engineered I-joists: the maker's hole chart as data; trusses: no field holes).
//! They are never a structural approval.

use serde::{Deserialize, Serialize};

/// What crosses the member.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "trade_in")]
pub enum Raceway {
    /// Electrical metallic tubing, by trade size (inches).
    Emt(f64),
    /// Rigid PVC conduit, Schedule 40.
    PvcSch40(f64),
    /// Electrical nonmetallic tubing.
    Ent(f64),
    /// Metal-clad cable (bored with a 7/8" paddle bit).
    Mc,
    /// Nonmetallic sheathed cable (7/8" paddle bit).
    Nm,
    /// Anything else by its outside diameter (a pipe, a sleeve).
    Od(f64),
}

/// Trade size → (outside, inside) diameter, inches.
const EMT: [(f64, f64, f64); 10] = [
    (0.5, 0.706, 0.622),
    (0.75, 0.922, 0.824),
    (1.0, 1.163, 1.049),
    (1.25, 1.510, 1.380),
    (1.5, 1.740, 1.610),
    (2.0, 2.197, 2.067),
    (2.5, 2.875, 2.731),
    (3.0, 3.500, 3.356),
    (3.5, 4.000, 3.834),
    (4.0, 4.500, 4.334),
];
const PVC40: [(f64, f64, f64); 10] = [
    (0.5, 0.840, 0.602),
    (0.75, 1.050, 0.804),
    (1.0, 1.315, 1.029),
    (1.25, 1.660, 1.360),
    (1.5, 1.900, 1.590),
    (2.0, 2.375, 2.047),
    (2.5, 2.875, 2.445),
    (3.0, 3.500, 3.042),
    (3.5, 4.000, 3.521),
    (4.0, 4.500, 3.998),
];
/// ENT comes to 2" only; its outside matches Schedule 40 PVC.
const ENT: [(f64, f64, f64); 6] =
    [(0.5, 0.840, 0.560), (0.75, 1.050, 0.760), (1.0, 1.315, 0.980), (1.25, 1.660, 1.300), (1.5, 1.900, 1.530), (2.0, 2.375, 1.950)];

/// Standard hole saw and bit sizes, inches.
pub const STANDARD_BITS: [f64; 26] = [
    0.75, 0.875, 1.0, 1.125, 1.25, 1.375, 1.5, 1.625, 1.75, 2.0, 2.125, 2.25, 2.5, 2.75, 3.0, 3.25, 3.5, 3.75, 4.0, 4.25, 4.5, 4.75, 5.0, 5.5, 6.0,
    6.5,
];

/// The cable paddle bit.
pub const CABLE_BIT_IN: f64 = 0.875;

/// Clearance around a raceway in its hole, inches.
pub const CLEARANCE_IN: f64 = 0.125;

fn table(r: Raceway) -> &'static [(f64, f64, f64)] {
    match r {
        Raceway::Emt(_) => &EMT,
        Raceway::PvcSch40(_) => &PVC40,
        Raceway::Ent(_) => &ENT,
        _ => &[],
    }
}

fn trade(r: Raceway) -> Option<f64> {
    match r {
        Raceway::Emt(t) | Raceway::PvcSch40(t) | Raceway::Ent(t) => Some(t),
        _ => None,
    }
}

fn with_trade(r: Raceway, t: f64) -> Raceway {
    match r {
        Raceway::Emt(_) => Raceway::Emt(t),
        Raceway::PvcSch40(_) => Raceway::PvcSch40(t),
        Raceway::Ent(_) => Raceway::Ent(t),
        other => other,
    }
}

impl Raceway {
    /// Outside diameter, inches (cables: none; they take the paddle bit).
    pub fn od_in(self) -> Option<f64> {
        match self {
            Raceway::Od(d) => (d.is_finite() && d > 0.0).then_some(d),
            Raceway::Mc | Raceway::Nm => None,
            r => trade(r).and_then(|t| table(r).iter().find(|e| (e.0 - t).abs() < 1e-6)).map(|e| e.1),
        }
    }

    /// Inside diameter, inches (what it carries).
    pub fn id_in(self) -> Option<f64> {
        match self {
            Raceway::Od(d) => (d.is_finite() && d > 0.0).then_some(d),
            Raceway::Mc | Raceway::Nm => None,
            r => trade(r).and_then(|t| table(r).iter().find(|e| (e.0 - t).abs() < 1e-6)).map(|e| e.2),
        }
    }

    /// `3" EMT`, `2" ENT`, `MC cable`.
    pub fn words(self) -> String {
        let t = |v: f64| crate::walls::frac(v, true);
        match self {
            Raceway::Emt(v) => format!("{}\" EMT", t(v).trim_end_matches('"')),
            Raceway::PvcSch40(v) => format!("{}\" PVC Sch 40", t(v).trim_end_matches('"')),
            Raceway::Ent(v) => format!("{}\" ENT", t(v).trim_end_matches('"')),
            Raceway::Mc => "MC cable".into(),
            Raceway::Nm => "NM cable".into(),
            Raceway::Od(d) => format!("{} OD", t(d)),
        }
    }
}

/// How a hole is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BitKind {
    /// Carbide-tipped hole saw (wood).
    HoleSaw,
    /// Paddle (spade) bit for cables.
    Paddle,
    /// Bimetal hole saw or step bit (field-cut metal studs).
    MetalHoleSaw,
    /// No bit: through the stud's punchout.
    Punchout,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bit {
    pub dia_in: f64,
    pub kind: BitKind,
}

/// The smallest standard bit at least the raceway's outside diameter plus `clearance_in`
/// (cables: the 7/8" paddle bit). None when nothing in the standard list is big enough.
pub fn pick_bit(r: Raceway, clearance_in: f64) -> Option<Bit> {
    if matches!(r, Raceway::Mc | Raceway::Nm) {
        return Some(Bit { dia_in: CABLE_BIT_IN, kind: BitKind::Paddle });
    }
    let need = r.od_in()? + clearance_in.clamp(0.0, 2.0);
    STANDARD_BITS.iter().copied().find(|b| *b + 1e-9 >= need).map(|dia_in| Bit { dia_in, kind: BitKind::HoleSaw })
}

/// One row of an engineered joist maker's hole chart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoleChartRow {
    pub depth_in: f64,
    /// Largest round hole in the web, inches.
    pub max_round_in: f64,
    /// Least distance from the hole to a support, feet.
    pub min_from_support_ft: f64,
}

/// A maker's hole chart (data, loaded from JSON; never built in as the truth).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HoleChart {
    pub maker: String,
    pub rows: Vec<HoleChartRow>,
}

impl HoleChart {
    /// A generic example chart for tests and demonstrations only: real limits come from the
    /// maker's literature for the actual product.
    pub fn example() -> HoleChart {
        let row = |depth_in, max_round_in, min_from_support_ft| HoleChartRow { depth_in, max_round_in, min_from_support_ft };
        HoleChart {
            maker: "Example I-joist (not a real product)".into(),
            rows: vec![row(9.5, 6.0, 2.0), row(11.875, 8.0, 3.0), row(14.0, 10.0, 4.0), row(16.0, 12.0, 5.0)],
        }
    }

    fn row(&self, depth_in: f64) -> Option<&HoleChartRow> {
        self.rows.iter().find(|r| (r.depth_in - depth_in).abs() < 0.07)
    }
}

/// The member a hole goes through.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Member {
    /// Sawn lumber joist or rafter: actual depth, inches.
    SawnJoist { depth_in: f64 },
    /// Sawn lumber stud: actual width through which it is bored (3 1/2", 5 1/2").
    SawnStud { depth_in: f64, bearing: bool },
    /// Cold-formed steel stud: web depth and punchout width.
    MetalStud { depth_in: f64, punchout_in: f64 },
    /// Engineered I-joist with its maker's chart.
    IJoist { depth_in: f64, chart: HoleChart },
    /// Metal-plate-connected wood truss or open-web joist: no field holes.
    Truss,
}

impl Member {
    pub fn words(&self) -> String {
        let f = |v: f64| crate::walls::frac(v, true);
        match self {
            Member::SawnJoist { depth_in } => format!("{} sawn joist", f(*depth_in)),
            Member::SawnStud { depth_in, bearing } => format!("{} {} stud", f(*depth_in), if *bearing { "bearing" } else { "non-bearing" }),
            Member::MetalStud { depth_in, .. } => format!("{} metal stud", f(*depth_in)),
            Member::IJoist { depth_in, .. } => format!("{} I-joist", f(*depth_in)),
            Member::Truss => "truss".into(),
        }
    }

    /// The largest hole allowed (inches) and the rule it comes from; none = no field holes.
    pub fn max_hole_in(&self) -> (Option<f64>, String) {
        match self {
            Member::SawnJoist { depth_in } => (Some(depth_in / 3.0), "sawn joist: hole ≤ 1/3 of the depth".into()),
            Member::SawnStud { depth_in, bearing: true } => (Some(depth_in * 0.4), "bearing stud: hole ≤ 40% of the width".into()),
            Member::SawnStud { depth_in, bearing: false } => (Some(depth_in * 0.6), "non-bearing stud: hole ≤ 60% of the width".into()),
            Member::MetalStud { punchout_in, .. } => (Some(*punchout_in), "metal stud: through its punchouts".into()),
            Member::IJoist { depth_in, chart } => match chart.row(*depth_in) {
                Some(r) => (Some(r.max_round_in), format!("{} hole chart", chart.maker)),
                None => (None, format!("{} hole chart has no {}\" row", chart.maker, depth_in)),
            },
            Member::Truss => (None, "trusses: no field holes without the truss designer's approval".into()),
        }
    }

    fn depth(&self) -> Option<f64> {
        match self {
            Member::SawnJoist { depth_in }
            | Member::SawnStud { depth_in, .. }
            | Member::MetalStud { depth_in, .. }
            | Member::IJoist { depth_in, .. } => Some(*depth_in),
            Member::Truss => None,
        }
    }

    fn wood(&self) -> bool {
        !matches!(self, Member::MetalStud { .. })
    }
}

/// Where along/through the member the hole is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HoleAt {
    /// From the member's top (joists) or face (studs) to the hole's centre, inches; none =
    /// centred.
    pub centre_from_edge_in: Option<f64>,
    /// From the nearest support, feet (joists); none = unknown.
    pub from_support_ft: Option<f64>,
}

/// One raceway through one member.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoreCheck {
    pub raceway: Raceway,
    pub member: String,
    /// The bit to use (none: nothing standard is big enough).
    pub bit: Option<Bit>,
    pub max_hole_in: Option<f64>,
    pub fits: bool,
    pub flags: Vec<String>,
    /// The nearest fitting option when it doesn't fit: `count` × `raceway`.
    pub proposal: Option<(usize, Raceway)>,
}

/// Check a raceway through a member: the bit, the member's limit, edge distances, and an option
/// that fits when it doesn't.
pub fn check(r: Raceway, m: &Member, at: HoleAt, clearance_in: f64) -> BoreCheck {
    let mut flags = Vec::new();
    let (max, rule) = m.max_hole_in();
    let mut bit = pick_bit(r, clearance_in);
    if !m.wood()
        && let Some(b) = bit.as_mut()
    {
        let od = r.od_in().unwrap_or(b.dia_in);
        b.kind = if max.is_some_and(|p| od + clearance_in <= p + 1e-9) { BitKind::Punchout } else { BitKind::MetalHoleSaw };
    }
    let hole = bit.map(|b| if b.kind == BitKind::Punchout { r.od_in().unwrap_or(b.dia_in) + clearance_in } else { b.dia_in });
    let mut fits = match (hole, max) {
        (Some(h), Some(mx)) => h <= mx + 1e-9,
        _ => false,
    };
    if bit.is_none() {
        flags.push(format!("no standard bit is big enough for {}", r.words()));
    }
    if let (Some(h), Some(mx)) = (hole, max)
        && h > mx + 1e-9
    {
        flags.push(format!("{} hole is over the {} limit ({}: {})", crate::walls::frac(h, true), m.words(), crate::walls::frac(mx, true), rule));
    }
    if max.is_none() {
        flags.push(rule);
    }
    if let (Some(h), Some(d)) = (hole, m.depth()) {
        let c = at.centre_from_edge_in.unwrap_or(d / 2.0);
        let edge = (c - h / 2.0).min(d - c - h / 2.0);
        let need = match m {
            Member::SawnJoist { .. } => Some(2.0),
            Member::SawnStud { .. } => Some(0.625),
            _ => None,
        };
        if let Some(n) = need
            && edge < n - 1e-9
        {
            fits = false;
            flags.push(format!(
                "hole edge {} from the member's edge (needs {})",
                crate::walls::frac(edge.max(0.0), true),
                crate::walls::frac(n, true)
            ));
        }
    }
    if let (Member::IJoist { depth_in, chart }, Some(s)) = (m, at.from_support_ft)
        && let Some(row) = chart.row(*depth_in)
        && s < row.min_from_support_ft
    {
        fits = false;
        flags.push(format!("{s:.1} ft from the support; the chart needs {:.1} ft", row.min_from_support_ft));
    }
    let proposal = if fits { None } else { propose(r, m, clearance_in) };
    BoreCheck { raceway: r, member: m.words(), bit, max_hole_in: max, fits, flags, proposal }
}

/// The nearest option that fits: the largest smaller trade size of the same raceway (or ENT for
/// a plain sleeve) whose hole is allowed, as many as carry the same inside area.
pub fn propose(r: Raceway, m: &Member, clearance_in: f64) -> Option<(usize, Raceway)> {
    let (max, _) = m.max_hole_in();
    let max = max?;
    let need_area = r.id_in()?.powi(2);
    let base = if trade(r).is_some() { r } else { Raceway::Ent(2.0) };
    let mut sizes: Vec<f64> = table(base).iter().map(|e| e.0).filter(|t| trade(r).is_none_or(|rt| *t < rt - 1e-9)).collect();
    sizes.sort_by(|a, b| b.total_cmp(a));
    for t in sizes {
        let c = with_trade(base, t);
        let ok = pick_bit(c, clearance_in).is_some_and(|b| {
            let h = if m.wood() { b.dia_in } else { c.od_in().unwrap_or(b.dia_in) + clearance_in };
            h <= max + 1e-9
        });
        if ok && let Some(id) = c.id_in() {
            let n = (need_area / (id * id)).ceil().max(1.0);
            return Some((n.min(64.0) as usize, c));
        }
    }
    None
}

/// The bits and hole counts for the tool takeoff.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BitLine {
    pub kind: BitKind,
    pub dia_in: f64,
    pub holes: usize,
}

pub fn bit_takeoff<'a>(checks: impl IntoIterator<Item = &'a BoreCheck>) -> Vec<BitLine> {
    let mut out: Vec<BitLine> = Vec::new();
    for c in checks {
        let Some(b) = c.bit else { continue };
        if b.kind == BitKind::Punchout {
            continue;
        }
        match out.iter_mut().find(|l| l.kind == b.kind && (l.dia_in - b.dia_in).abs() < 1e-9) {
            Some(l) => l.holes += 1,
            None => out.push(BitLine { kind: b.kind, dia_in: b.dia_in, holes: 1 }),
        }
    }
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.dia_in.total_cmp(&b.dia_in)));
    out
}
