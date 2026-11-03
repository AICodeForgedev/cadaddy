//! What the prints don't say: the gaps and conflicts a building spec was built over, as an RFI
//! list (request for information). Each item says where it is (level, plan point, the sheets
//! involved), what is missing or conflicting, what was assumed meanwhile and how sure that is.
//!
//! Two sources fill the list:
//! - the print readers, which know the sheets (a tag the legend doesn't have, the same wall
//!   typed differently on two sheets, a callout to a detail that isn't there, sleeves with no
//!   route): they go into [`BuildingSpec::rfis`];
//! - [`check`], from the spec alone: walls whose type was guessed, rated walls stopping at a
//!   ceiling, walls stopping at a ceiling beside an open room or under a two-layer ceiling,
//!   heads of wall not given, ceilings and soffits whose type was guessed, soffit details no
//!   soffit uses.
//!
//! [`list`] puts both together (the readers' items first; a check item that says the same thing
//! is dropped) and numbers them.

use crate::spec::{BuildingSpec, CeilingArea, HeightRule, WallMark};
use serde::{Deserialize, Serialize};

/// Items kept in a spec at most (hostile specs).
pub const MAX_RFIS: usize = 5_000;

/// What an item is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfiKind {
    /// A wall with no type tag (its type was guessed).
    UntaggedWall,
    /// A wall tag the wall type legend doesn't have.
    UnknownTag,
    /// The same wall shown with different types on two sheets.
    SheetConflict,
    /// A wall that stops at the ceiling beside a room open to the structure.
    HeightVsCeiling,
    /// A rated wall that stops at (or just above) the ceiling.
    RatedToCeiling,
    /// A two-layer ceiling over a wall that stops below it.
    DoubleCeilingShortWall,
    /// A wall type with no head of wall detail.
    HeadUnspecified,
    /// A room whose ceiling type or height the prints don't give.
    CeilingUnspecified,
    /// A soffit on the RCP with no detail or section.
    SoffitNoDetail,
    /// A callout to a detail the details sheet doesn't have.
    DetailMissing,
    /// A soffit detail no soffit on the RCPs uses.
    DetailUnused,
    /// A sleeve or pathway given only a size: no route, destination or method.
    SleeveNoRoute,
    /// A sleeve or raceway that won't fit the framing where it is drawn.
    SleeveNoFit,
    /// Gear whose lugs don't take the conductors that land on them (count, wire range, CU-only
    /// lugs on aluminium, neutral or ground bar short), or lugs to order with it.
    GearLugs,
    /// Gear the field can't install as submitted: entry plates, gutter, bending space.
    GearInstall,
    /// Labels that need data the documents don't give yet (an arc flash study's results for a
    /// piece of gear): a document request, so the labels are ordered in time.
    LabelData,
    /// Grounding or bonding the drawings call for that isn't in the model, can't be reached, or
    /// falls short of its note (a missed concrete-encased electrode).
    Grounding,
    /// Floor- or grade-mounted equipment with no pad decision, or a pad nobody owns.
    PadUndecided,
    /// An item that must be in or under a pour before concrete (the Ufer electrode, stub-ups,
    /// sleeves, anchor bolts) not confirmed by the pour date.
    PourHold,
    /// Rebar, conduits, sleeves and embeds that clash or lack cover in an element.
    EmbedConflict,
    /// A utility or service entrance the building needs (power, telephone, CATV, fire alarm
    /// monitoring, gas bonding) that the drawings don't show, or show too unclearly to lay out
    /// before the slab.
    ServiceEntrance,
    /// An equipment connection whose approved submittal's electrical data differs from the
    /// drawings: its protection, its short-circuit rating against the fault current, its conductors.
    EquipmentElectrical,
    /// Lighting where the documents disagree: a plan's fixture type with no schedule row, a
    /// submitted luminaire that isn't the schedule's basis of design.
    Lighting,
}

impl RfiKind {
    pub const ALL: [RfiKind; 23] = [
        RfiKind::UntaggedWall,
        RfiKind::UnknownTag,
        RfiKind::SheetConflict,
        RfiKind::HeightVsCeiling,
        RfiKind::RatedToCeiling,
        RfiKind::DoubleCeilingShortWall,
        RfiKind::HeadUnspecified,
        RfiKind::CeilingUnspecified,
        RfiKind::SoffitNoDetail,
        RfiKind::DetailMissing,
        RfiKind::DetailUnused,
        RfiKind::SleeveNoRoute,
        RfiKind::SleeveNoFit,
        RfiKind::GearLugs,
        RfiKind::GearInstall,
        RfiKind::LabelData,
        RfiKind::Grounding,
        RfiKind::PadUndecided,
        RfiKind::PourHold,
        RfiKind::EmbedConflict,
        RfiKind::ServiceEntrance,
        RfiKind::EquipmentElectrical,
        RfiKind::Lighting,
    ];

    /// The snake_case name (as in JSON).
    pub fn name(self) -> &'static str {
        match self {
            RfiKind::UntaggedWall => "untagged_wall",
            RfiKind::UnknownTag => "unknown_tag",
            RfiKind::SheetConflict => "sheet_conflict",
            RfiKind::HeightVsCeiling => "height_vs_ceiling",
            RfiKind::RatedToCeiling => "rated_to_ceiling",
            RfiKind::DoubleCeilingShortWall => "double_ceiling_short_wall",
            RfiKind::HeadUnspecified => "head_unspecified",
            RfiKind::CeilingUnspecified => "ceiling_unspecified",
            RfiKind::SoffitNoDetail => "soffit_no_detail",
            RfiKind::DetailMissing => "detail_missing",
            RfiKind::DetailUnused => "detail_unused",
            RfiKind::SleeveNoRoute => "sleeve_no_route",
            RfiKind::SleeveNoFit => "sleeve_no_fit",
            RfiKind::GearLugs => "gear_lugs",
            RfiKind::GearInstall => "gear_install",
            RfiKind::LabelData => "label_data",
            RfiKind::Grounding => "grounding",
            RfiKind::PadUndecided => "pad_undecided",
            RfiKind::PourHold => "pour_hold",
            RfiKind::EmbedConflict => "embed_conflict",
            RfiKind::ServiceEntrance => "service_entrance",
            RfiKind::EquipmentElectrical => "equipment_electrical",
            RfiKind::Lighting => "lighting",
        }
    }

    pub fn parse(s: &str) -> Option<RfiKind> {
        RfiKind::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s.trim()))
    }

    /// In plain words, for a list's heading.
    pub fn title(self) -> &'static str {
        match self {
            RfiKind::UntaggedWall => "Wall with no type tag",
            RfiKind::UnknownTag => "Wall tag not in the legend",
            RfiKind::SheetConflict => "Wall typed differently on two sheets",
            RfiKind::HeightVsCeiling => "Wall height rule vs the ceiling it meets",
            RfiKind::RatedToCeiling => "Rated wall stops at the ceiling",
            RfiKind::DoubleCeilingShortWall => "Two-layer ceiling over a wall that stops below it",
            RfiKind::HeadUnspecified => "Head of wall not specified",
            RfiKind::CeilingUnspecified => "Ceiling with no type or height",
            RfiKind::SoffitNoDetail => "Soffit with no section or detail",
            RfiKind::DetailMissing => "Callout to a detail that isn't there",
            RfiKind::DetailUnused => "Soffit detail no soffit uses",
            RfiKind::SleeveNoRoute => "Sleeve with no route or method",
            RfiKind::SleeveNoFit => "Sleeve won't fit the framing",
            RfiKind::GearLugs => "Gear lugs don't take the feeders",
            RfiKind::GearInstall => "Gear can't be installed as submitted",
            RfiKind::LabelData => "Labels need data the documents don't give",
            RfiKind::Grounding => "Grounding or bonding missing, unreachable or short",
            RfiKind::PadUndecided => "Equipment pad not decided or not owned",
            RfiKind::PourHold => "Item to cast in not confirmed before the pour",
            RfiKind::EmbedConflict => "Rebar and embeds clash or lack cover",
            RfiKind::ServiceEntrance => "Service entrance not shown or unclear",
            RfiKind::EquipmentElectrical => "Equipment's submittal differs from the drawings (protection, SCCR, conductors)",
            RfiKind::Lighting => "Lighting documents disagree (plans, fixture schedule, submittal)",
        }
    }

    /// How much it matters before work goes ahead.
    pub fn severity(self) -> Severity {
        match self {
            RfiKind::UnknownTag
            | RfiKind::SheetConflict
            | RfiKind::RatedToCeiling
            | RfiKind::SleeveNoRoute
            | RfiKind::SleeveNoFit
            | RfiKind::GearLugs
            | RfiKind::GearInstall
            | RfiKind::Grounding
            | RfiKind::PadUndecided
            | RfiKind::PourHold
            | RfiKind::EmbedConflict
            | RfiKind::ServiceEntrance
            | RfiKind::EquipmentElectrical => Severity::High,
            RfiKind::DetailUnused => Severity::Low,
            _ => Severity::Medium,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Wrong work or a code problem if built as assumed: ask before building.
    High,
    #[default]
    Medium,
    /// Worth confirming; the assumption is usually right.
    Low,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        }
    }

    pub fn parse(s: &str) -> Option<Severity> {
        [Severity::High, Severity::Medium, Severity::Low].into_iter().find(|k| k.name().eq_ignore_ascii_case(s.trim()))
    }
}

/// Where an RFI stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfiStatus {
    #[default]
    Open,
    /// Sent to the design team.
    Sent,
    Answered,
    /// Answered and built in, or not needed.
    Closed,
}

impl RfiStatus {
    pub fn name(self) -> &'static str {
        match self {
            RfiStatus::Open => "open",
            RfiStatus::Sent => "sent",
            RfiStatus::Answered => "answered",
            RfiStatus::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<RfiStatus> {
        [RfiStatus::Open, RfiStatus::Sent, RfiStatus::Answered, RfiStatus::Closed].into_iter().find(|k| k.name().eq_ignore_ascii_case(s.trim()))
    }
}

/// One gap or conflict.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rfi {
    /// `RFI-001`…, given by [`list`].
    #[serde(default)]
    pub id: String,
    pub kind: Option<RfiKind>,
    #[serde(default)]
    pub severity: Severity,
    /// The level it is on (empty: the whole set, e.g. a legend entry).
    #[serde(default)]
    pub level: String,
    /// Where on the level's plan (plan drawing units in a spec).
    #[serde(default)]
    pub at: [f64; 2],
    /// The area to show round it, when it has one: min and max corners.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extent: Option<[[f64; 2]; 2]>,
    /// The sheets involved (`A-101`, `A-501`…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheets: Vec<String>,
    /// What it is about: a wall, a wall type, a room, a soffit, a sleeve.
    #[serde(default)]
    pub about: String,
    /// What is missing or conflicting.
    pub issue: String,
    /// What was assumed meanwhile.
    #[serde(default)]
    pub assumed: String,
    /// A proposed fix to send with the question, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<String>,
    #[serde(default)]
    pub status: RfiStatus,
    /// The design team's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

impl Rfi {
    pub fn new(kind: RfiKind, level: &str, at: [f64; 2], about: impl Into<String>, issue: impl Into<String>, assumed: impl Into<String>) -> Rfi {
        Rfi {
            kind: Some(kind),
            severity: kind.severity(),
            level: level.into(),
            at,
            about: about.into(),
            issue: issue.into(),
            assumed: assumed.into(),
            ..Rfi::default()
        }
    }

    pub fn kind_name(&self) -> &'static str {
        self.kind.map_or("other", RfiKind::name)
    }

    /// Whether two items say the same thing (same kind, and the same subject or place).
    pub fn same_as(&self, o: &Rfi, tol: f64) -> bool {
        self.kind == o.kind
            && self.level.eq_ignore_ascii_case(&o.level)
            && ((!self.about.is_empty() && self.about == o.about) || ((self.at[0] - o.at[0]).hypot(self.at[1] - o.at[1]) <= tol))
    }
}

/// Every item: the spec's own (from the readers), then [`check`]'s that don't repeat one,
/// numbered `RFI-001`… in that order (an item that has an id keeps it).
pub fn list(spec: &BuildingSpec) -> Vec<Rfi> {
    let tol = spec.units_per_foot.abs().max(1e-9);
    let mut out: Vec<Rfi> = spec.rfis.iter().take(MAX_RFIS).cloned().collect();
    for r in check(spec) {
        if out.len() >= MAX_RFIS {
            break;
        }
        if !out.iter().any(|o| o.same_as(&r, tol)) {
            out.push(r);
        }
    }
    let mut used: Vec<String> = out.iter().map(|r| r.id.clone()).filter(|i| !i.is_empty()).collect();
    let mut n = 0usize;
    for r in out.iter_mut().filter(|r| r.id.is_empty()) {
        loop {
            n += 1;
            let id = format!("RFI-{n:03}");
            if !used.contains(&id) {
                used.push(id.clone());
                r.id = id;
                break;
            }
        }
    }
    out
}

fn mid(m: &WallMark) -> [f64; 2] {
    [(m.a[0] + m.b[0]) / 2.0, (m.a[1] + m.b[1]) / 2.0]
}

fn inside(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                odd = !odd;
            }
        }
    }
    odd
}

/// The ceilings either side of a wall mark (a point just off its middle each side): `None` where
/// no ceiling area holds the point.
fn ceilings_beside<'a>(spec: &'a BuildingSpec, m: &WallMark, th_ft: f64) -> [Option<&'a CeilingArea>; 2] {
    let len = (m.b[0] - m.a[0]).hypot(m.b[1] - m.a[1]);
    if len <= 1e-9 || !len.is_finite() {
        return [None, None];
    }
    let u = spec.units_per_foot.abs().max(1e-9);
    let n = [-(m.b[1] - m.a[1]) / len, (m.b[0] - m.a[0]) / len];
    let off = (th_ft / 2.0 + 0.75) * u;
    let c = mid(m);
    [-1.0, 1.0].map(|s| {
        let p = [c[0] + n[0] * off * s, c[1] + n[1] * off * s];
        spec.rcp.ceilings.iter().filter(|a| a.level == m.level).take(MAX_RFIS).find(|a| inside(p, &a.outline))
    })
}

fn boards(layers: &[crate::spec::AssemblyLayer]) -> usize {
    layers.iter().filter(|l| l.category == "board").count()
}

fn centre(outline: &[[f64; 2]]) -> [f64; 2] {
    let n = outline.len().max(1) as f64;
    let s = outline.iter().fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
    [s[0] / n, s[1] / n]
}

fn extent(outline: &[[f64; 2]]) -> Option<[[f64; 2]; 2]> {
    let mut it = outline.iter();
    let f = it.next()?;
    Some(it.fold([*f, *f], |[lo, hi], p| [[lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]]))
}

/// The gaps the spec itself shows (see the module docs).
pub fn check(spec: &BuildingSpec) -> Vec<Rfi> {
    let mut out = Vec::new();
    let wall_type = |n: &str| spec.wall_types.iter().find(|t| t.name == n);
    let marks: Vec<&WallMark> = spec.walls.iter().take(MAX_RFIS).collect();
    for m in &marks {
        let at = mid(m);
        let ext = Some([[m.a[0].min(m.b[0]), m.a[1].min(m.b[1])], [m.a[0].max(m.b[0]), m.a[1].max(m.b[1])]]);
        let Some(wt) = wall_type(&m.wall_type) else {
            let mut r = Rfi::new(
                RfiKind::UnknownTag,
                &m.level,
                at,
                format!("wall type {}", m.wall_type),
                format!("The wall is typed {}, which the wall types don't have.", m.wall_type),
                "Built with its layer's wall type.",
            );
            r.extent = ext;
            out.push(r);
            continue;
        };
        if let Some(why) = &m.guess {
            let mut r = Rfi::new(RfiKind::UntaggedWall, &m.level, at, "", "No wall type tag on this wall.", format!("Type {} ({why}).", m.wall_type));
            r.extent = ext;
            out.push(r);
        }
        let Some(a) = &wt.assembly else { continue };
        if !matches!(a.height, Some(HeightRule::Ceiling | HeightRule::AboveCeiling)) || a.external {
            continue;
        }
        let rule = a.height.map(|h| h.text(a.above_ceiling_in)).unwrap_or_default();
        let sides = ceilings_beside(spec, m, wt.thickness_in / 12.0);
        let open = sides.iter().filter(|c| c.is_none()).count();
        if open == 1 {
            let mut r = Rfi::new(
                RfiKind::HeightVsCeiling,
                &m.level,
                at,
                "",
                format!("Type {} goes {rule}, but one side has no ceiling (open to the structure).", m.wall_type),
                "Built to the ceiling on the other side; the open side is left as drawn.",
            );
            r.extent = ext;
            r.proposed = Some("Extend this wall to the deck, or confirm it stops at the ceiling.".into());
            out.push(r);
        }
        let double = sides.iter().flatten().find_map(|c| {
            let t = spec.ceiling_types.iter().find(|t| Some(&t.name) == c.ceiling_type.as_ref())?;
            (boards(&t.layers) >= 2).then_some(t)
        });
        if let Some(t) = double {
            let mut r = Rfi::new(
                RfiKind::DoubleCeilingShortWall,
                &m.level,
                at,
                "",
                format!("Ceiling {} has {} board layers over a type {} wall that goes {rule}.", t.name, boards(&t.layers), m.wall_type),
                "The wall stops at its rule; the ceiling runs over it.",
            );
            r.extent = ext;
            r.proposed = Some("Confirm whether the ceiling runs continuous over the wall or the wall goes to the deck.".into());
            out.push(r);
        }
    }
    let has_heads = spec.wall_types.iter().any(|t| t.assembly.as_ref().is_some_and(|a| a.head.is_some()));
    for wt in spec.wall_types.iter().take(MAX_RFIS) {
        let Some(a) = &wt.assembly else { continue };
        let first = marks.iter().find(|m| m.wall_type == wt.name);
        let (level, at) = first.map_or((String::new(), [0.0, 0.0]), |m| (m.level.clone(), mid(m)));
        if a.fire_rating.is_some() && matches!(a.height, Some(HeightRule::Ceiling | HeightRule::AboveCeiling)) {
            let rating = a.fire_rating.clone().unwrap_or_default();
            let rule = a.height.map(|h| h.text(a.above_ceiling_in)).unwrap_or_default();
            let mut r = Rfi::new(
                RfiKind::RatedToCeiling,
                &level,
                at,
                format!("wall type {}", wt.name),
                format!("Type {} is rated {rating} but goes {rule}.", wt.name),
                format!("Built {rule} as the legend says."),
            );
            r.proposed = Some("A rated wall normally runs to the deck: confirm the height or a rated ceiling assembly.".into());
            out.push(r);
        }
        if has_heads && a.head.is_none() {
            out.push(Rfi::new(
                RfiKind::HeadUnspecified,
                &level,
                at,
                format!("wall type {}", wt.name),
                format!("No head of wall detail covers type {}.", wt.name),
                "A single top track at its height rule.",
            ));
        }
    }
    for c in spec.rcp.ceilings.iter().take(MAX_RFIS) {
        let missing = c.ceiling_type.is_none() || c.guess.is_some();
        if !missing {
            continue;
        }
        let what = c.room.clone().unwrap_or_default();
        let mut r = Rfi::new(
            RfiKind::CeilingUnspecified,
            &c.level,
            centre(&c.outline),
            if what.is_empty() { String::new() } else { format!("room {what}") },
            "The prints don't give this ceiling's type.",
            match (&c.ceiling_type, &c.guess) {
                (Some(t), Some(why)) => format!("Type {t} ({why})."),
                (Some(t), None) => format!("Type {t}."),
                (None, _) => "No type: built as a plain ceiling.".into(),
            },
        );
        r.extent = extent(&c.outline);
        out.push(r);
    }
    for s in spec.rcp.soffits.iter().take(MAX_RFIS) {
        if s.soffit_type.is_some() && s.guess.is_none() {
            continue;
        }
        let mut r = Rfi::new(
            RfiKind::SoffitNoDetail,
            &s.level,
            centre(&s.outline),
            "",
            "No section or detail shows how this soffit is built.",
            match (&s.soffit_type, &s.guess) {
                (Some(t), Some(why)) => format!("Detail {t} ({why})."),
                (Some(t), None) => format!("Detail {t}."),
                (None, _) => "No detail: built as a plain box.".into(),
            },
        );
        r.extent = extent(&s.outline);
        out.push(r);
    }
    for t in spec.soffit_types.iter().take(MAX_RFIS) {
        if !spec.rcp.soffits.iter().any(|s| s.soffit_type.as_ref() == Some(&t.name)) {
            out.push(Rfi::new(
                RfiKind::DetailUnused,
                "",
                [0.0, 0.0],
                format!("soffit detail {}", t.name),
                format!("Soffit detail {} is on the details sheet, but no soffit on the RCPs uses it.", t.name),
                "Not built anywhere.",
            ));
        }
    }
    out.truncate(MAX_RFIS);
    out
}

/// The list as CSV (one row per item, a header first).
pub fn csv(items: &[Rfi]) -> String {
    let mut s = String::new();
    for row in rows(items) {
        let cells: Vec<String> =
            row.iter().map(|c| if c.contains([',', '"', '\n']) { format!("\"{}\"", c.replace('"', "\"\"")) } else { c.clone() }).collect();
        s.push_str(&cells.join(","));
        s.push('\n');
    }
    s
}

/// The list as table rows, a header first: what a spreadsheet or a printed list shows.
pub fn rows(items: &[Rfi]) -> Vec<Vec<String>> {
    let mut out = vec![
        ["RFI", "Severity", "Kind", "Level", "Sheets", "About", "Issue", "Assumed meanwhile", "Proposed", "Status", "Answer"]
            .map(String::from)
            .to_vec(),
    ];
    for r in items {
        out.push(vec![
            r.id.clone(),
            r.severity.name().into(),
            r.kind.map_or("Other", RfiKind::title).into(),
            r.level.clone(),
            r.sheets.join(" "),
            r.about.clone(),
            r.issue.clone(),
            r.assumed.clone(),
            r.proposed.clone().unwrap_or_default(),
            r.status.name().into(),
            r.answer.clone().unwrap_or_default(),
        ]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{Assembly, CeilingType, Rcp, WallType};

    fn wall_type(name: &str, thickness_in: f64, assembly: Assembly) -> WallType {
        WallType { name: name.into(), height_ft: None, thickness_in, from: Default::default(), assembly: Some(assembly) }
    }

    fn area(outline: Vec<[f64; 2]>, ceiling_type: Option<&str>, guess: Option<&str>) -> CeilingArea {
        CeilingArea {
            level: "1".into(),
            room: None,
            outline,
            height_ft: 9.0,
            grid: None,
            ceiling_type: ceiling_type.map(String::from),
            guess: guess.map(String::from),
            source: None,
        }
    }

    fn mark(t: &str, a: [f64; 2], b: [f64; 2], guess: Option<&str>) -> WallMark {
        WallMark {
            level: "1".into(),
            a,
            b,
            wall_type: t.into(),
            height_ft: None,
            guess: guess.map(String::from),
            source: String::new(),
            status: None,
            height_source: String::new(),
        }
    }

    #[test]
    fn spec_checks_and_numbering() {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        let rated =
            Assembly { fire_rating: Some("1 HR".into()), height: Some(HeightRule::AboveCeiling), above_ceiling_in: 6.0, ..Assembly::default() };
        sp.wall_types.push(wall_type("A", 5.0, rated));
        sp.walls.push(mark("A", [0.0, 0.0], [240.0, 0.0], Some("only type that thick")));
        sp.walls.push(mark("Z9", [0.0, 120.0], [240.0, 120.0], None));
        // A two-layer ceiling north of the first wall, nothing south: open on one side.
        sp.ceiling_types.push(CeilingType {
            name: "C2".into(),
            layers: vec![
                crate::spec::AssemblyLayer { category: "board".into(), ..Default::default() },
                crate::spec::AssemblyLayer { category: "board".into(), ..Default::default() },
            ],
            ..CeilingType::default()
        });
        sp.rcp = Rcp {
            ceilings: vec![area(vec![[0.0, 0.0], [240.0, 0.0], [240.0, 120.0], [0.0, 120.0]], Some("C2"), Some("not scheduled"))],
            ..Rcp::default()
        };
        let l = list(&sp);
        let kinds: Vec<RfiKind> = l.iter().filter_map(|r| r.kind).collect();
        for k in [
            RfiKind::UntaggedWall,
            RfiKind::UnknownTag,
            RfiKind::HeightVsCeiling,
            RfiKind::DoubleCeilingShortWall,
            RfiKind::RatedToCeiling,
            RfiKind::CeilingUnspecified,
        ] {
            assert!(kinds.contains(&k), "{k:?} in {kinds:?}");
        }
        assert_eq!(l.first().map(|r| r.id.as_str()), Some("RFI-001"));
        // The readers' own item comes first and keeps its id; a check item saying the same goes.
        let mut own = Rfi::new(RfiKind::UnknownTag, "1", [120.0, 120.0], "wall type Z9", "tag Z9 not in the legend", "guessed");
        own.id = "RFI-007".into();
        own.sheets = vec!["A-101".into()];
        sp.rfis.push(own);
        let l2 = list(&sp);
        assert_eq!(l2.first().map(|r| r.id.as_str()), Some("RFI-007"));
        assert_eq!(l2.iter().filter(|r| r.kind == Some(RfiKind::UnknownTag)).count(), 1);
        assert_eq!(l2.len(), l.len());
        let csv = csv(&l2);
        assert!(csv.starts_with("RFI,Severity,Kind"));
        assert_eq!(csv.lines().count(), l2.len() + 1);
        // JSON round trip.
        let j = serde_json::to_string(&l2).unwrap_or_default();
        let back: Vec<Rfi> = serde_json::from_str(&j).unwrap_or_default();
        assert_eq!(back, l2);
    }

    #[test]
    fn hostile_specs() {
        let mut sp = BuildingSpec { units_per_foot: f64::NAN, ..BuildingSpec::default() };
        sp.walls.push(mark("A", [f64::NAN, 0.0], [f64::INFINITY, 0.0], Some("x")));
        sp.walls.push(mark("A", [0.0, 0.0], [0.0, 0.0], None));
        sp.wall_types.push(wall_type("A", f64::NAN, Assembly { height: Some(HeightRule::Ceiling), ..Assembly::default() }));
        sp.rcp.ceilings.push(area(vec![], None, None));
        for _ in 0..(MAX_RFIS + 10) {
            sp.rfis.push(Rfi::default());
        }
        let l = list(&sp);
        assert!(l.len() <= MAX_RFIS);
    }
}
