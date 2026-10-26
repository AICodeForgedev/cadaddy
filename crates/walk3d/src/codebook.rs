//! Building-code rule book (Q6013): code-adopted rules as data, grouped by code + edition,
//! with jurisdiction amendments layered on top. Consumers (framing, electrical, plumbing,
//! mechanical, building) query rules by topic and the model's context; `code.check` runs the
//! checks and returns findings with rule ids and section citations.
//!
//! ## COPYRIGHT
//! Only rule VALUES and SECTION NUMBERS, written in our own words. Never copied code text or
//! tables reproduced verbatim from ICC/NFPA publications. The starter set is labelled
//! "starter — verify with your AHJ / the adopted code".

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Items in any list, at most (hostile input).
pub const MAX_RULES: usize = 10_000;
/// Amendments per jurisdiction, at most.
pub const MAX_AMENDMENTS: usize = 1_000;
/// Findings from one check run, at most.
pub const MAX_FINDINGS: usize = 20_000;

// ---------------------------------------------------------------- codes and editions

/// A building code family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// International Building Code.
    Ibc,
    /// International Residential Code.
    Irc,
    /// National Electrical Code (NFPA 70).
    Nec,
    /// International Plumbing Code.
    Ipc,
    /// International Mechanical Code.
    Imc,
    /// International Fire Code.
    Ifc,
    /// International Energy Conservation Code.
    Iecc,
}

impl Code {
    pub fn label(self) -> &'static str {
        match self {
            Code::Ibc => "IBC",
            Code::Irc => "IRC",
            Code::Nec => "NEC",
            Code::Ipc => "IPC",
            Code::Imc => "IMC",
            Code::Ifc => "IFC",
            Code::Iecc => "IECC",
        }
    }

    pub fn parse(s: &str) -> Option<Code> {
        match s.to_ascii_uppercase().as_str() {
            "IBC" => Some(Code::Ibc),
            "IRC" => Some(Code::Irc),
            "NEC" | "NFPA70" | "NFPA 70" => Some(Code::Nec),
            "IPC" => Some(Code::Ipc),
            "IMC" => Some(Code::Imc),
            "IFC" => Some(Code::Ifc),
            "IECC" => Some(Code::Iecc),
            _ => None,
        }
    }

    pub fn all() -> &'static [Code] {
        &[Code::Ibc, Code::Irc, Code::Nec, Code::Ipc, Code::Imc, Code::Ifc, Code::Iecc]
    }
}

/// A code edition (year).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Edition {
    pub code: Code,
    /// The edition year (`2021`, `2024`).
    pub year: u16,
}

impl Edition {
    pub fn label(&self) -> String {
        format!("{} {}", self.code.label(), self.year)
    }
}

// ---------------------------------------------------------------- rules

/// A topic a rule applies to (framing, electrical, plumbing, mechanical, building).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// Stud spacing, height limits, headers, fire blocking, top-plate splices, rated assemblies.
    Framing,
    /// Receptacle spacing, GFCI/AFCI, box fill, ampacity, conduit fill, panel clearance.
    Electrical,
    /// Fixture units, pipe sizing, slopes, vent sizing, cleanout spacing, water heater.
    Plumbing,
    /// Duct/terminal guidance, fire/smoke dampers, clearances, outside air.
    Mechanical,
    /// Egress door widths, accessible clearances, rated-door requirements.
    Building,
}

impl Topic {
    pub fn label(self) -> &'static str {
        match self {
            Topic::Framing => "framing",
            Topic::Electrical => "electrical",
            Topic::Plumbing => "plumbing",
            Topic::Mechanical => "mechanical",
            Topic::Building => "building",
        }
    }

    pub fn parse(s: &str) -> Option<Topic> {
        match s.to_ascii_lowercase().as_str() {
            "framing" | "structural" => Some(Topic::Framing),
            "electrical" | "elec" => Some(Topic::Electrical),
            "plumbing" | "plumb" => Some(Topic::Plumbing),
            "mechanical" | "mech" | "hvac" => Some(Topic::Mechanical),
            "building" | "arch" | "egress" => Some(Topic::Building),
            _ => None,
        }
    }

    pub fn all() -> &'static [Topic] {
        &[Topic::Framing, Topic::Electrical, Topic::Plumbing, Topic::Mechanical, Topic::Building]
    }
}

/// Source of a rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    /// Shipped with CADaddy.
    #[default]
    Starter,
    /// User-added or edited.
    User,
    /// A jurisdiction amendment.
    Amendment,
}

impl RuleSource {
    pub fn label(self) -> &'static str {
        match self {
            RuleSource::Starter => "starter",
            RuleSource::User => "user",
            RuleSource::Amendment => "amendment",
        }
    }
}

/// What a rule constrains.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Applies {
    /// Wall type filter (e.g. `bearing`, `non-bearing`, `exterior`, `interior`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall: Option<String>,
    /// Stud size filter (e.g. `2x4`, `2x6`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stud: Option<String>,
    /// Maximum wall height in feet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_ft_max: Option<f64>,
    /// Room type filter (e.g. `kitchen`, `bathroom`, `garage`, `hallway`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    /// Location filter (e.g. `wet`, `outdoor`, `dwelling`, `commercial`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Conductor material (e.g. `copper`, `aluminum`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conductor: Option<String>,
    /// Pipe material (e.g. `pvc`, `copper`, `cast_iron`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipe: Option<String>,
    /// Minimum size (context-dependent: wire gauge, pipe diameter, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_min: Option<String>,
    /// Occupancy type (e.g. `R-2`, `B`, `A-2`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occupancy: Option<String>,
    /// Load condition (e.g. `gravity`, `lateral`, `combined`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load: Option<String>,
}

impl Applies {
    pub fn is_empty(&self) -> bool {
        self.wall.is_none()
            && self.stud.is_none()
            && self.height_ft_max.is_none()
            && self.room.is_none()
            && self.location.is_none()
            && self.conductor.is_none()
            && self.pipe.is_none()
            && self.size_min.is_none()
            && self.occupancy.is_none()
            && self.load.is_none()
    }

    /// Whether this filter matches a given context. `None` in the filter = matches anything.
    pub fn matches(&self, ctx: &RuleContext) -> bool {
        opt_match(&self.wall, &ctx.wall)
            && opt_match(&self.stud, &ctx.stud)
            && opt_ft_max(self.height_ft_max, ctx.height_ft)
            && opt_match(&self.room, &ctx.room)
            && opt_match(&self.location, &ctx.location)
            && opt_match(&self.conductor, &ctx.conductor)
            && opt_match(&self.pipe, &ctx.pipe)
            && opt_match(&self.occupancy, &ctx.occupancy)
    }
}

fn opt_match(filter: &Option<String>, value: &Option<String>) -> bool {
    match filter {
        None => true,
        Some(f) => match value {
            Some(v) => v.eq_ignore_ascii_case(f),
            None => false,
        },
    }
}

fn opt_ft_max(max: Option<f64>, value: Option<f64>) -> bool {
    match (max, value) {
        (Some(m), Some(v)) if m.is_finite() && v.is_finite() => v <= m,
        (Some(_), None) => false,
        _ => true,
    }
}

/// Context for matching rules against a specific element or situation.
#[derive(Clone, Debug, Default)]
pub struct RuleContext {
    pub wall: Option<String>,
    pub stud: Option<String>,
    pub height_ft: Option<f64>,
    pub room: Option<String>,
    pub location: Option<String>,
    pub conductor: Option<String>,
    pub pipe: Option<String>,
    pub occupancy: Option<String>,
}

/// The value a rule gives.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleValue {
    /// A numeric value (spacing in inches, size in inches, count, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_spacing_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_spacing_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_height_ft: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_size_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fill_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_clearance_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_width_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_span_ft: Option<f64>,
    /// Boolean requirement (e.g. GFCI required, fire blocking required).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// A table of values (header sizes, pipe sizes, etc.) keyed by a parameter.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub table: BTreeMap<String, String>,
    /// Free-form text describing the requirement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

// (Default derived above.)

/// One code rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeRule {
    /// Unique id (e.g. `IRC-R602.3(5)`, `NEC-210.52(A)(1)`).
    pub id: String,
    /// The code and edition this rule comes from.
    pub code: Code,
    /// Edition year.
    pub edition: u16,
    /// Section reference (e.g. `R602.3(5)`, `210.52(A)(1)`).
    pub section: String,
    /// What trade/topic this covers.
    pub topic: Topic,
    /// A short description of the rule, in our own words.
    pub summary: String,
    /// What elements this rule applies to (filters).
    #[serde(default, skip_serializing_if = "Applies::is_empty")]
    pub applies: Applies,
    /// The value(s) the rule prescribes.
    pub value: RuleValue,
    /// Where this rule came from.
    #[serde(default)]
    pub source: RuleSource,
}

/// A jurisdiction amendment that overrides a base rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Amendment {
    /// The rule id being overridden.
    pub rule_id: String,
    /// What changes.
    pub value: RuleValue,
    /// The jurisdiction that adopted this amendment.
    pub jurisdiction: String,
    /// Why it differs (in our own words).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The project's code selection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeSelection {
    /// Selected editions (e.g. [IRC 2021, NEC 2023]).
    pub editions: Vec<Edition>,
    /// Jurisdiction name (for amendment layering).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jurisdiction: Option<String>,
}

/// A finding from a code check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeFinding {
    /// The rule that was checked.
    pub rule_id: String,
    /// Section citation (e.g. `IRC R602.3(5)`).
    pub cite: String,
    /// The element or location that failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    /// Level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// What the code requires.
    pub expected: String,
    /// What was found.
    pub found: String,
    /// The topic.
    pub topic: Topic,
    /// The rule's source.
    pub source: RuleSource,
    /// Severity.
    pub severity: FindingSeverity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    High,
    Medium,
    Low,
}

// ---------------------------------------------------------------- the book

/// The full code book: base rules + amendments, with a project selection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeBook {
    pub rules: Vec<CodeRule>,
    pub amendments: Vec<Amendment>,
    pub selection: CodeSelection,
}

impl CodeBook {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Cap lists to safe limits.
    pub fn capped(mut self) -> Self {
        self.rules.truncate(MAX_RULES);
        self.amendments.truncate(MAX_AMENDMENTS);
        self.selection.editions.truncate(64);
        self
    }

    /// Find rules matching a topic filter and context.
    pub fn query(&self, topic: Option<Topic>, ctx: &RuleContext) -> Vec<&CodeRule> {
        self.rules
            .iter()
            .filter(|r| {
                if topic.is_some_and(|t| r.topic != t) {
                    return false;
                }
                if !self.selection.editions.is_empty() && !self.selection.editions.iter().any(|e| e.code == r.code && e.year == r.edition) {
                    return false;
                }
                r.applies.matches(ctx)
            })
            .collect()
    }

    /// Resolve a rule's effective value, applying any jurisdiction amendment.
    pub fn resolve(&self, rule: &CodeRule) -> RuleValue {
        if let Some(j) = &self.selection.jurisdiction {
            for a in &self.amendments {
                if a.rule_id == rule.id && a.jurisdiction.eq_ignore_ascii_case(j) {
                    return a.value.clone();
                }
            }
        }
        rule.value.clone()
    }

    /// Get a rule by id.
    pub fn get_rule(&self, id: &str) -> Option<&CodeRule> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// All topics with rules in the current selection.
    pub fn topics(&self) -> Vec<Topic> {
        let mut seen = std::collections::BTreeSet::new();
        for r in &self.rules {
            if self.selection.editions.is_empty() || self.selection.editions.iter().any(|e| e.code == r.code && e.year == r.edition) {
                seen.insert(r.topic);
            }
        }
        seen.into_iter().collect()
    }

    // ---- checks -----------------------------------------------------------------

    /// Run framing checks against the model data.
    pub fn check_framing(&self, walls: &[WallCheck]) -> Vec<CodeFinding> {
        let mut findings = Vec::new();
        for wall in walls {
            let ctx = RuleContext {
                wall: Some(wall.wall_type.clone()),
                stud: wall.stud_size.clone(),
                height_ft: Some(wall.height_ft),
                ..Default::default()
            };
            for rule in self.query(Some(Topic::Framing), &ctx) {
                let val = self.resolve(rule);
                if let Some(max) = val.max_spacing_in
                    && wall.stud_spacing_in > max + 0.01
                {
                    findings.push(finding(
                        rule,
                        Some(&wall.id),
                        &wall.level,
                        format!("max spacing {max}\""),
                        format!("spacing {}\"", wall.stud_spacing_in),
                        FindingSeverity::High,
                    ));
                }
                if let Some(max_h) = val.max_height_ft
                    && wall.height_ft > max_h + 0.01
                {
                    findings.push(finding(
                        rule,
                        Some(&wall.id),
                        &wall.level,
                        format!("max height {max_h}'"),
                        format!("height {}'", wall.height_ft),
                        FindingSeverity::High,
                    ));
                }
                if let Some(span) = wall.header_span_ft {
                    check_header_table(&val, span, wall, rule, &mut findings);
                }
            }
            if wall.height_ft > 10.0 && !wall.has_fire_blocking {
                let ctx_fb = RuleContext { wall: Some(wall.wall_type.clone()), ..Default::default() };
                for rule in self.query(Some(Topic::Framing), &ctx_fb) {
                    let val = self.resolve(rule);
                    if val.required == Some(true) && rule.id.contains("fire_block") {
                        findings.push(finding(
                            rule,
                            Some(&wall.id),
                            &wall.level,
                            "fire blocking required at concealed spaces > 10'".into(),
                            "no fire blocking".into(),
                            FindingSeverity::High,
                        ));
                    }
                }
            }
        }
        findings.truncate(MAX_FINDINGS);
        findings
    }

    /// Run electrical checks against rooms/devices.
    pub fn check_electrical(&self, rooms: &[RoomCheck]) -> Vec<CodeFinding> {
        let mut findings = Vec::new();
        for room in rooms {
            let ctx = RuleContext { room: Some(room.room_type.clone()), location: room.location.clone(), ..Default::default() };
            for rule in self.query(Some(Topic::Electrical), &ctx) {
                let val = self.resolve(rule);
                if let Some(max_sp) = val.max_spacing_in
                    && let Some(actual) = room.receptacle_spacing_in
                    && actual > max_sp + 0.01
                {
                    findings.push(finding(
                        rule,
                        Some(&room.id),
                        &room.level,
                        format!("max receptacle spacing {max_sp}\""),
                        format!("spacing {actual}\""),
                        FindingSeverity::High,
                    ));
                }
                if val.required == Some(true) && rule.id.contains("gfci") && !room.has_gfci {
                    findings.push(finding(
                        rule,
                        Some(&room.id),
                        &room.level,
                        "GFCI protection required".into(),
                        "no GFCI protection".into(),
                        FindingSeverity::High,
                    ));
                }
                if val.required == Some(true) && rule.id.contains("afci") && !room.has_afci {
                    findings.push(finding(
                        rule,
                        Some(&room.id),
                        &room.level,
                        "AFCI protection required".into(),
                        "no AFCI protection".into(),
                        FindingSeverity::High,
                    ));
                }
                if let (Some(min_cl), Some(actual_cl)) = (val.min_clearance_in, room.panel_clearance_in)
                    && actual_cl < min_cl - 0.01
                {
                    findings.push(finding(
                        rule,
                        Some(&room.id),
                        &room.level,
                        format!("min clearance {min_cl}\""),
                        format!("clearance {actual_cl}\""),
                        FindingSeverity::High,
                    ));
                }
            }
        }
        findings.truncate(MAX_FINDINGS);
        findings
    }

    /// Run plumbing checks.
    pub fn check_plumbing(&self, fixtures: &[PlumbingCheck]) -> Vec<CodeFinding> {
        let mut findings = Vec::new();
        for fix in fixtures {
            let ctx = RuleContext { pipe: fix.pipe_material.clone(), ..Default::default() };
            for rule in self.query(Some(Topic::Plumbing), &ctx) {
                let val = self.resolve(rule);
                if let (Some(min_size), Some(actual)) = (val.min_size_in, fix.pipe_size_in)
                    && actual < min_size - 0.01
                {
                    findings.push(finding(
                        rule,
                        Some(&fix.id),
                        &fix.level,
                        format!("min pipe size {min_size}\""),
                        format!("pipe size {actual}\""),
                        FindingSeverity::Medium,
                    ));
                }
            }
        }
        findings.truncate(MAX_FINDINGS);
        findings
    }

    /// Run mechanical checks.
    pub fn check_mechanical(&self, ducts: &[MechCheck]) -> Vec<CodeFinding> {
        let mut findings = Vec::new();
        for duct in ducts {
            let ctx = RuleContext { location: duct.location.clone(), ..Default::default() };
            for rule in self.query(Some(Topic::Mechanical), &ctx) {
                let val = self.resolve(rule);
                if val.required == Some(true) && rule.id.contains("damper") && !duct.has_damper {
                    findings.push(finding(
                        rule,
                        Some(&duct.id),
                        &duct.level,
                        "fire/smoke damper required at rated assembly".into(),
                        "no damper".into(),
                        FindingSeverity::High,
                    ));
                }
                if let (Some(min_cl), Some(actual)) = (val.min_clearance_in, duct.clearance_in)
                    && actual < min_cl - 0.01
                {
                    findings.push(finding(
                        rule,
                        Some(&duct.id),
                        &duct.level,
                        format!("min clearance {min_cl}\""),
                        format!("clearance {actual}\""),
                        FindingSeverity::Medium,
                    ));
                }
            }
        }
        findings.truncate(MAX_FINDINGS);
        findings
    }

    /// Run building (egress/accessibility) checks.
    pub fn check_building(&self, openings: &[EgressCheck]) -> Vec<CodeFinding> {
        let mut findings = Vec::new();
        for op in openings {
            let ctx = RuleContext { occupancy: op.occupancy.clone(), ..Default::default() };
            for rule in self.query(Some(Topic::Building), &ctx) {
                let val = self.resolve(rule);
                if let (Some(min_w), Some(actual)) = (val.min_width_in, op.width_in)
                    && actual < min_w - 0.01
                {
                    findings.push(finding(
                        rule,
                        Some(&op.id),
                        &op.level,
                        format!("min width {min_w}\""),
                        format!("width {actual}\""),
                        FindingSeverity::High,
                    ));
                }
                if let (Some(min_cl), Some(actual)) = (val.min_clearance_in, op.clearance_in)
                    && actual < min_cl - 0.01
                {
                    findings.push(finding(
                        rule,
                        Some(&op.id),
                        &op.level,
                        format!("min clearance {min_cl}\""),
                        format!("clearance {actual}\""),
                        FindingSeverity::High,
                    ));
                }
            }
        }
        findings.truncate(MAX_FINDINGS);
        findings
    }
}

fn finding(
    rule: &CodeRule,
    element: Option<&str>,
    level: &Option<String>,
    expected: String,
    found: String,
    severity: FindingSeverity,
) -> CodeFinding {
    CodeFinding {
        rule_id: rule.id.clone(),
        cite: format!("{} {}: {}", rule.code.label(), rule.section, rule.summary),
        element: element.map(String::from),
        level: level.clone(),
        expected,
        found,
        topic: rule.topic,
        source: rule.source,
        severity,
    }
}

fn check_header_table(val: &RuleValue, span_ft: f64, wall: &WallCheck, rule: &CodeRule, findings: &mut Vec<CodeFinding>) {
    // The table maps span ranges to header sizes.
    if val.table.is_empty() {
        return;
    }
    // Find the applicable row: keys are "<=N" where N is the max span in feet.
    let mut required_header: Option<&str> = None;
    let mut best_max = f64::MAX;
    for (key, header) in &val.table {
        if let Some(max_str) = key.strip_prefix("<=")
            && let Ok(max) = max_str.trim().parse::<f64>()
            && span_ft <= max + 0.01
            && max < best_max
        {
            best_max = max;
            required_header = Some(header.as_str());
        }
    }
    if let (Some(required), Some(actual)) = (required_header, &wall.header_size)
        && !actual.eq_ignore_ascii_case(required)
    {
        findings.push(finding(
            rule,
            Some(&wall.id),
            &wall.level,
            format!("header {required} for span {span_ft}'"),
            format!("header {actual}"),
            FindingSeverity::High,
        ));
    }
}

// ---------------------------------------------------------------- check inputs

/// Data for checking a wall against framing rules.
#[derive(Clone, Debug, Default)]
pub struct WallCheck {
    pub id: String,
    pub level: Option<String>,
    pub wall_type: String,
    pub stud_size: Option<String>,
    pub stud_spacing_in: f64,
    pub height_ft: f64,
    pub has_fire_blocking: bool,
    /// Header opening span, if there is one.
    pub header_span_ft: Option<f64>,
    pub header_size: Option<String>,
}

/// Data for checking a room against electrical rules.
#[derive(Clone, Debug, Default)]
pub struct RoomCheck {
    pub id: String,
    pub level: Option<String>,
    pub room_type: String,
    pub location: Option<String>,
    pub receptacle_spacing_in: Option<f64>,
    pub has_gfci: bool,
    pub has_afci: bool,
    pub panel_clearance_in: Option<f64>,
}

/// Data for checking plumbing fixtures.
#[derive(Clone, Debug, Default)]
pub struct PlumbingCheck {
    pub id: String,
    pub level: Option<String>,
    pub pipe_material: Option<String>,
    pub pipe_size_in: Option<f64>,
    pub fixture_units: Option<f64>,
}

/// Data for checking mechanical systems.
#[derive(Clone, Debug, Default)]
pub struct MechCheck {
    pub id: String,
    pub level: Option<String>,
    pub location: Option<String>,
    pub has_damper: bool,
    pub clearance_in: Option<f64>,
}

/// Data for checking egress/accessibility.
#[derive(Clone, Debug, Default)]
pub struct EgressCheck {
    pub id: String,
    pub level: Option<String>,
    pub occupancy: Option<String>,
    pub width_in: Option<f64>,
    pub clearance_in: Option<f64>,
}

// ---------------------------------------------------------------- starter book

/// Load the built-in starter code book.
pub fn starter_book() -> CodeBook {
    let json = include_str!("codebook_starter.json");
    match serde_json::from_str::<CodeBook>(json) {
        Ok(b) => b.capped(),
        Err(_) => CodeBook::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_loads() {
        let book = starter_book();
        assert!(!book.rules.is_empty(), "starter book has no rules");
        assert!(book.rules.len() >= 20, "starter book has fewer than 20 rules");
    }

    #[test]
    fn framing_header_from_book() {
        let book = starter_book();
        let wall = WallCheck {
            id: "W-1".into(),
            level: Some("Level 1".into()),
            wall_type: "bearing".into(),
            stud_size: Some("2x4".into()),
            stud_spacing_in: 16.0,
            height_ft: 9.0,
            has_fire_blocking: true,
            header_span_ft: Some(4.0),
            header_size: Some("2x6".into()), // wrong for a 4' span
            ..Default::default()
        };
        let findings = book.check_framing(&[wall]);
        // Should find a finding about header size (2x6 is wrong for 4' span on bearing wall).
        let header_findings: Vec<_> = findings.iter().filter(|f| f.expected.contains("header")).collect();
        assert!(!header_findings.is_empty(), "expected a header finding");
    }

    #[test]
    fn framing_stud_spacing_check() {
        let book = starter_book();
        let wall = WallCheck {
            id: "W-2".into(),
            level: Some("Level 1".into()),
            wall_type: "bearing".into(),
            stud_size: Some("2x4".into()),
            stud_spacing_in: 24.0,
            height_ft: 9.0,
            has_fire_blocking: true,
            ..Default::default()
        };
        let findings = book.check_framing(&[wall]);
        let spacing_findings: Vec<_> = findings.iter().filter(|f| f.expected.contains("spacing")).collect();
        // IRC R602.3(5): 2x4 bearing wall at 9' -> max 16" o.c.
        assert!(!spacing_findings.is_empty(), "expected a stud spacing finding for 2x4 bearing at 24\"");
    }

    #[test]
    fn receptacle_spacing_check() {
        let book = starter_book();
        let room = RoomCheck {
            id: "R-1".into(),
            level: Some("Level 1".into()),
            room_type: "dwelling".into(),
            location: Some("dwelling".into()),
            receptacle_spacing_in: Some(160.0), // > 12' (144"), violates NEC 210.52
            has_gfci: true,
            has_afci: true,
            ..Default::default()
        };
        let findings = book.check_electrical(&[room]);
        let recep_findings: Vec<_> = findings.iter().filter(|f| f.expected.contains("receptacle")).collect();
        assert!(!recep_findings.is_empty(), "expected a receptacle spacing finding");
    }

    #[test]
    fn amendment_overrides_base() {
        let mut book = starter_book();
        // Add an amendment that allows 24" stud spacing for bearing 2x4.
        book.amendments.push(Amendment {
            rule_id: "IRC-R602.3(5)-bearing-2x4".into(),
            value: RuleValue { max_spacing_in: Some(24.0), ..Default::default() },
            jurisdiction: "Sample County".into(),
            note: Some("Local amendment allows 24\" for single-storey".into()),
        });
        book.selection.jurisdiction = Some("Sample County".into());

        let wall = WallCheck {
            id: "W-3".into(),
            level: Some("Level 1".into()),
            wall_type: "bearing".into(),
            stud_size: Some("2x4".into()),
            stud_spacing_in: 24.0,
            height_ft: 9.0,
            has_fire_blocking: true,
            ..Default::default()
        };
        let findings = book.check_framing(&[wall]);
        let spacing_findings: Vec<_> = findings.iter().filter(|f| f.expected.contains("spacing")).collect();
        assert!(spacing_findings.is_empty(), "amendment should allow 24\" spacing");
    }
}
