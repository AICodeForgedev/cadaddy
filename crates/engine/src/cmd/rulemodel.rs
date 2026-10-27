//! Rule-based BIM: declarative rule packs for each trade, evaluated in priority order to
//! place, size and route elements with a confidence score. Elements below threshold, rule
//! conflicts, missing inputs and clashes the rules cannot resolve become worklist findings
//! for the agent, which acts on them through direct model operations.
//!
//! Priority order (highest wins): project rules > spec rules > print facts > code rules >
//! common-sense defaults. Every placed/sized/routed element stores the `rule_id`, the inputs
//! it used and a confidence (0.0–1.0).
//!
//! Commands:
//! - `rules.packs {}` — list loaded/available rule packs.
//! - `rules.evaluate {level?, trade?}` — evaluate rules, produce elements and findings.
//! - `rules.worklist {trade?, status?, cursor?, limit?}` — paged findings worklist.
//! - `rules.action {action, target?, params?}` — agent performs a model operation.
//! - `rules.clashloop {level?}` — build → clashes → auto-resolve → re-check cycle.
//! - `rules.score {trade?}` — accuracy metrics per trade/category.
//! - `rules.report {trade?}` — per-category summary report.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{EntityKind, Handle, RawTag};
use cadaddy_geom::Vec2;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

// ── constants ─────────────────────────────────────────────────────────────────

/// Maximum elements the engine will track.
const MAX_ELEMENTS: usize = 500_000;
/// Maximum findings in the worklist.
const MAX_FINDINGS: usize = 100_000;
/// Default confidence threshold below which an element becomes a finding.
const DEFAULT_THRESHOLD: f64 = 0.7;
/// Page sizes for worklist.
const PAGE_DEFAULT: usize = 20;
const PAGE_MAX: usize = 100;

// ── trade ─────────────────────────────────────────────────────────────────────

/// The MEP/coordination trade a rule pack covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trade {
    Electrical,
    Plumbing,
    Mechanical,
    FireProtection,
    Coordination,
    /// Walls, doors, windows, floors, ceilings, stairs and the rest of the building shell.
    Architectural,
    /// Columns, beams, slabs, footings and grids.
    Structural,
}

impl Trade {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Electrical => "electrical",
            Self::Plumbing => "plumbing",
            Self::Mechanical => "mechanical",
            Self::FireProtection => "fire_protection",
            Self::Coordination => "coordination",
            Self::Architectural => "architectural",
            Self::Structural => "structural",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Electrical => "Electrical",
            Self::Plumbing => "Plumbing",
            Self::Mechanical => "Mechanical",
            Self::FireProtection => "Fire Protection",
            Self::Coordination => "Coordination",
            Self::Architectural => "Architectural",
            Self::Structural => "Structural",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "electrical" | "elec" | "e" => Some(Self::Electrical),
            "plumbing" | "plumb" | "p" => Some(Self::Plumbing),
            "mechanical" | "mech" | "m" => Some(Self::Mechanical),
            "fire_protection" | "fireprotection" | "fp" | "fire" | "sprinkler" => Some(Self::FireProtection),
            "coordination" | "coord" | "c" => Some(Self::Coordination),
            "architectural" | "architecture" | "arch" | "a" => Some(Self::Architectural),
            "structural" | "structure" | "struct" | "s" => Some(Self::Structural),
            _ => None,
        }
    }

    pub const ALL: [Self; 7] =
        [Self::Architectural, Self::Structural, Self::Electrical, Self::Plumbing, Self::Mechanical, Self::FireProtection, Self::Coordination];
}

// ── rule priority ─────────────────────────────────────────────────────────────

/// Which tier a rule comes from (highest wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulePriority {
    /// Project-specific rules set by the owner or from an answered RFI.
    Project = 0,
    /// Specification requirements.
    Spec = 1,
    /// Facts read from the prints (dimensions, tags, notes).
    PrintFact = 2,
    /// Code requirements (NEC, IPC, IMC, NFPA).
    Code = 3,
    /// Common-sense trade defaults.
    Default = 4,
}

impl RulePriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Spec => "spec",
            Self::PrintFact => "print_fact",
            Self::Code => "code",
            Self::Default => "default",
        }
    }
}

// ── rule definition ──────────────────────────────────────────────────────────

/// A single rule: when `condition` matches, apply `action` at the given priority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleDef {
    /// Unique id within the pack (e.g. "elec-support-spacing-emt").
    pub id: String,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Priority tier.
    pub priority: RulePriority,
    /// What this rule checks or matches.
    pub condition: RuleCondition,
    /// What to do when the condition matches.
    pub action: RuleAction,
    /// Source citation (code section, spec page, etc.).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// The confidence this rule assigns (0.0–1.0). 1.0 if explicit in spec/code, lower for
    /// inferred defaults.
    #[serde(default = "one")]
    pub confidence: f64,
}

fn one() -> f64 {
    1.0
}

/// What a rule checks: element category, system, properties, location.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleCondition {
    /// Element category (e.g. "conduit", "box", "device", "duct", "pipe", "head").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// System (e.g. "supply_duct", "chw_pipe", "branch_circuit").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub system: String,
    /// Trade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade: Option<Trade>,
    /// Material match (e.g. "EMT", "MC", "PVC").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub material: String,
    /// Location match (e.g. "exposed", "concealed", "wet", "plenum").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
    /// Size range (inches): `[min, max]`. Empty = any size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_range: Option<[f64; 2]>,
    /// Property matches: key=value pairs that must all be true.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

/// What a rule does when its condition matches.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleAction {
    /// The kind of action: "set_property", "set_size", "set_spacing", "set_height",
    /// "route", "place", "check", "yield_to".
    pub kind: String,
    /// Key for set_property / set_height / set_spacing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// Value to set.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// Numeric value (spacing in feet, size in inches, height in inches, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric: Option<f64>,
    /// Secondary numeric (e.g. max for a range check).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Parameters for complex actions (routing, fill calculations).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Value>,
}

// ── rule pack ─────────────────────────────────────────────────────────────────

/// A versioned collection of rules for one trade.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePack {
    /// Pack identifier (e.g. "electrical-v1").
    pub id: String,
    /// Human label.
    pub label: String,
    /// The trade this pack covers.
    pub trade: Trade,
    /// Schema version.
    #[serde(default = "pack_version")]
    pub version: u32,
    /// The rules, in evaluation order.
    pub rules: Vec<RuleDef>,
}

fn pack_version() -> u32 {
    1
}

// ── placed element ────────────────────────────────────────────────────────────

/// An element placed or sized by the rules engine, with traceability.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacedElement {
    /// Unique element id.
    pub id: String,
    /// Trade.
    pub trade: Trade,
    /// Category (conduit, box, device, duct, pipe, head, etc.).
    pub category: String,
    /// System (branch_circuit, supply_duct, chw_pipe, etc.).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub system: String,
    /// Level.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub level: String,
    /// The rule that determined this element.
    pub rule_id: String,
    /// Inputs the rule used (key → value).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    /// Confidence (0.0–1.0).
    pub confidence: f64,
    /// Properties set by the rule.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
    /// An agent or person decided it (accepted it, or a project rule placed it).
    #[serde(default)]
    pub agent_decided: bool,
}

// ── finding ───────────────────────────────────────────────────────────────────

/// A finding: something for the worklist. Low-confidence elements, rule conflicts, missing
/// inputs, clashes, completeness gaps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Stable id (hash of what it is about).
    pub id: String,
    /// Kind: "low_confidence", "conflict", "missing_input", "clash", "completeness", "judgement".
    pub kind: String,
    /// Trade.
    pub trade: Trade,
    /// Level.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub level: String,
    /// Severity: "high", "medium", "low".
    pub severity: String,
    /// Short title.
    pub title: String,
    /// Detailed description with evidence.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The element(s) involved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<String>,
    /// Rule's proposed answer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proposed: String,
    /// Alternative actions the agent can take.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<ActionOption>,
    /// Status: "open", "resolved", "deferred".
    #[serde(default = "open_status")]
    pub status: String,
    /// How it was resolved, if resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
}

fn open_status() -> String {
    "open".into()
}

/// An action the agent can choose for a finding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionOption {
    pub label: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Value>,
}

/// How a finding was resolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub action: String,
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub citation: String,
    /// Whether this resolution becomes a project rule for future builds.
    #[serde(default)]
    pub save_as_rule: bool,
}

// ── evaluation result ─────────────────────────────────────────────────────────

/// The result of evaluating all rule packs against the building.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalResult {
    /// Elements placed/validated by rules.
    pub elements: Vec<PlacedElement>,
    /// Findings for the worklist.
    pub findings: Vec<Finding>,
    /// Per-trade summary: count, rule-placed %, avg confidence.
    pub summary: BTreeMap<String, TradeSummary>,
}

/// Summary for one trade.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TradeSummary {
    pub trade: String,
    pub total: usize,
    pub rule_placed: usize,
    pub agent_decided: usize,
    pub avg_confidence: f64,
    pub findings_open: usize,
    pub findings_resolved: usize,
}

// ── accuracy score ────────────────────────────────────────────────────────────

/// Accuracy metrics, per trade and overall.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccuracyScore {
    pub overall: f64,
    pub by_trade: BTreeMap<String, TradeScore>,
    pub sample_size: usize,
    pub accepted: usize,
    pub rejected: usize,
}

/// Score for one trade.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TradeScore {
    pub score: f64,
    pub sample_size: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub by_category: BTreeMap<String, f64>,
}

// ── report ────────────────────────────────────────────────────────────────────

/// Per-category report line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportLine {
    pub category: String,
    pub trade: String,
    pub count: usize,
    pub quantity: String,
    pub pct_rule_placed: f64,
    pub pct_agent_decided: f64,
    pub open_assumptions: usize,
    pub score: f64,
}

// ── drawing state ─────────────────────────────────────────────────────────────

/// The rules engine state stored in the drawing (in `Drawing.agent` JSON, under `rulemodel`,
/// beside the review queue's state). The elements themselves are never stored: they are read
/// from the live model each time, so the worklist, score and report always describe the model
/// as it is now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuleModelState {
    /// Owner-edited packs: one with a built-in pack's id replaces it, others are added.
    pub packs: Vec<RulePack>,
    /// Project rules: answers the agent (or a person) saved, highest priority.
    pub project_rules: Vec<RuleDef>,
    /// Worklist findings.
    pub findings: Vec<Finding>,
    /// Confidence threshold (elements below this become findings).
    pub threshold: f64,
    /// Last evaluation timestamp.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_eval: String,
    /// Build count (for tracking changes).
    pub build_count: u64,
    /// Per-build scores for tracking rule changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub score_history: Vec<ScoreEntry>,
    /// Elements an agent or person accepted (by [`ModelElem::key`], which outlives a rebuild):
    /// confidence 1, agent-decided.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted: Vec<String>,
    /// Elements an agent or person rejected in an audit (by key).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<String>,
    /// Properties an agent set on elements (a wall type, a circuit), by key: they apply again
    /// after every rebuild.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, BTreeMap<String, String>>,
}

/// A saved score from a build.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreEntry {
    pub build: u64,
    pub overall: f64,
    pub by_trade: BTreeMap<String, f64>,
}

// ── built-in rule packs ──────────────────────────────────────────────────────

/// Electrical trade rules.
#[allow(clippy::vec_init_then_push)]
pub fn electrical_rules() -> RulePack {
    let mut rules = Vec::new();

    // --- Branch routing: device → box → ceiling → panel ---
    rules.push(RuleDef {
        id: "elec-route-device-box".into(),
        description: "Route from device to nearest junction box".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "device".into(), trade: Some(Trade::Electrical), ..Default::default() },
        action: RuleAction { kind: "route".into(), value: "device_to_box".into(), ..Default::default() },
        source: "NEC 314.16".into(),
        confidence: 0.9,
    });
    rules.push(RuleDef {
        id: "elec-route-box-panel".into(),
        description: "Route from box through ceiling space to panel".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "box".into(), trade: Some(Trade::Electrical), ..Default::default() },
        action: RuleAction { kind: "route".into(), value: "box_to_panel".into(), ..Default::default() },
        source: "NEC 210".into(),
        confidence: 0.85,
    });

    // --- One box per device ---
    rules.push(RuleDef {
        id: "elec-box-per-device".into(),
        description: "Each device requires its own box".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "device".into(), trade: Some(Trade::Electrical), ..Default::default() },
        action: RuleAction { kind: "place".into(), value: "box".into(), ..Default::default() },
        source: "NEC 314.16".into(),
        confidence: 1.0,
    });

    // --- EMT/MC by spec location ---
    rules.push(RuleDef {
        id: "elec-raceway-emt-exposed".into(),
        description: "EMT for exposed locations".into(),
        priority: RulePriority::Spec,
        condition: RuleCondition { category: "conduit".into(), trade: Some(Trade::Electrical), location: "exposed".into(), ..Default::default() },
        action: RuleAction { kind: "set_property".into(), key: "racewayMaterial".into(), value: "EMT".into(), ..Default::default() },
        source: "Spec 26 05 33".into(),
        confidence: 0.95,
    });
    rules.push(RuleDef {
        id: "elec-raceway-mc-concealed".into(),
        description: "MC cable for concealed locations".into(),
        priority: RulePriority::Spec,
        condition: RuleCondition { category: "conduit".into(), trade: Some(Trade::Electrical), location: "concealed".into(), ..Default::default() },
        action: RuleAction { kind: "set_property".into(), key: "racewayMaterial".into(), value: "MC cable".into(), ..Default::default() },
        source: "Spec 26 05 33".into(),
        confidence: 0.95,
    });

    // --- Support spacing: EMT ≤10 ft, ≤3 ft from boxes ---
    rules.push(RuleDef {
        id: "elec-support-spacing-emt".into(),
        description: "EMT support spacing: max 10 ft between supports, max 3 ft from boxes".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "conduit".into(), trade: Some(Trade::Electrical), material: "EMT".into(), ..Default::default() },
        action: RuleAction {
            kind: "set_spacing".into(),
            key: "support_spacing_ft".into(),
            numeric: Some(10.0),
            max: Some(3.0), // max from box
            ..Default::default()
        },
        source: "NEC 358.30".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "elec-support-spacing-mc".into(),
        description: "MC cable support spacing: max 6 ft between supports, max 12 in from boxes".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "conduit".into(), trade: Some(Trade::Electrical), material: "MC".into(), ..Default::default() },
        action: RuleAction { kind: "set_spacing".into(), key: "support_spacing_ft".into(), numeric: Some(6.0), max: Some(1.0), ..Default::default() },
        source: "NEC 330.30".into(),
        confidence: 1.0,
    });

    // --- Conduit fill ---
    rules.push(RuleDef {
        id: "elec-conduit-fill".into(),
        description: "Conduit fill per NEC Chapter 9: 1 wire 53%, 2 wires 31%, 3+ wires 40%".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "conduit".into(), trade: Some(Trade::Electrical), ..Default::default() },
        action: RuleAction { kind: "check".into(), key: "conduit_fill_pct".into(), value: "40".into(), ..Default::default() },
        source: "NEC Chapter 9 Table 1".into(),
        confidence: 1.0,
    });

    // --- Box fill ---
    rules.push(RuleDef {
        id: "elec-box-fill".into(),
        description: "Box fill per NEC 314.16: each conductor, clamp, device, ground counted".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "box".into(), trade: Some(Trade::Electrical), ..Default::default() },
        action: RuleAction { kind: "check".into(), key: "box_fill".into(), ..Default::default() },
        source: "NEC 314.16(B)".into(),
        confidence: 1.0,
    });

    // --- Wire size from breaker ---
    rules.push(RuleDef {
        id: "elec-wire-size-20a".into(),
        description: "#12 AWG minimum for 20A circuits".into(),
        priority: RulePriority::Code,
        condition: RuleCondition {
            category: "conductor".into(),
            trade: Some(Trade::Electrical),
            properties: [("breaker_amps".into(), "20".into())].into(),
            ..Default::default()
        },
        action: RuleAction { kind: "set_property".into(), key: "conductorSize".into(), value: "#12 AWG".into(), ..Default::default() },
        source: "NEC 240.4(D)".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "elec-wire-size-15a".into(),
        description: "#14 AWG minimum for 15A circuits".into(),
        priority: RulePriority::Code,
        condition: RuleCondition {
            category: "conductor".into(),
            trade: Some(Trade::Electrical),
            properties: [("breaker_amps".into(), "15".into())].into(),
            ..Default::default()
        },
        action: RuleAction { kind: "set_property".into(), key: "conductorSize".into(), value: "#14 AWG".into(), ..Default::default() },
        source: "NEC 240.4(D)".into(),
        confidence: 1.0,
    });

    // --- Home-run grouping per circuit tags ---
    rules.push(RuleDef {
        id: "elec-homerun-group".into(),
        description: "Home-run conductors grouped per circuit tag on the panel schedule".into(),
        priority: RulePriority::PrintFact,
        condition: RuleCondition { category: "homerun".into(), trade: Some(Trade::Electrical), ..Default::default() },
        action: RuleAction { kind: "route".into(), value: "group_by_circuit".into(), ..Default::default() },
        source: "Panel schedule".into(),
        confidence: 0.9,
    });

    // --- Mounting heights ---
    rules.push(RuleDef {
        id: "elec-mount-receptacle-general".into(),
        description: "Standard receptacle mounting height 18\" AFF".into(),
        priority: RulePriority::Default,
        condition: RuleCondition {
            category: "device".into(),
            trade: Some(Trade::Electrical),
            properties: [("device_type".into(), "receptacle".into())].into(),
            ..Default::default()
        },
        action: RuleAction {
            kind: "set_height".into(),
            key: "mountingHeight".into(),
            value: "18\"".into(),
            numeric: Some(18.0),
            ..Default::default()
        },
        source: "Common practice".into(),
        confidence: 0.7,
    });
    rules.push(RuleDef {
        id: "elec-mount-switch".into(),
        description: "Switch mounting height 48\" AFF".into(),
        priority: RulePriority::Default,
        condition: RuleCondition {
            category: "device".into(),
            trade: Some(Trade::Electrical),
            properties: [("device_type".into(), "switch".into())].into(),
            ..Default::default()
        },
        action: RuleAction {
            kind: "set_height".into(),
            key: "mountingHeight".into(),
            value: "48\"".into(),
            numeric: Some(48.0),
            ..Default::default()
        },
        source: "ADA / common practice".into(),
        confidence: 0.7,
    });
    rules.push(RuleDef {
        id: "elec-mount-receptacle-counter".into(),
        description: "Counter receptacle mounting height 44\" AFF".into(),
        priority: RulePriority::Default,
        condition: RuleCondition {
            category: "device".into(),
            trade: Some(Trade::Electrical),
            location: "kitchen".into(),
            properties: [("device_type".into(), "receptacle".into())].into(),
            ..Default::default()
        },
        action: RuleAction {
            kind: "set_height".into(),
            key: "mountingHeight".into(),
            value: "44\"".into(),
            numeric: Some(44.0),
            ..Default::default()
        },
        source: "NEC 210.52(C)".into(),
        confidence: 0.8,
    });
    rules.push(RuleDef {
        id: "elec-mount-fa-pull".into(),
        description: "Fire alarm pull station 48\" AFF".into(),
        priority: RulePriority::Code,
        condition: RuleCondition {
            category: "device".into(),
            trade: Some(Trade::Electrical),
            properties: [("device_type".into(), "pull_station".into())].into(),
            ..Default::default()
        },
        action: RuleAction {
            kind: "set_height".into(),
            key: "mountingHeight".into(),
            value: "48\"".into(),
            numeric: Some(48.0),
            ..Default::default()
        },
        source: "NFPA 72 17.14".into(),
        confidence: 1.0,
    });

    RulePack { id: "electrical-v1".into(), label: "Electrical — NEC + trade defaults".into(), trade: Trade::Electrical, version: 1, rules }
}

/// Plumbing trade rules.
#[allow(clippy::vec_init_then_push)]
pub fn plumbing_rules() -> RulePack {
    let mut rules = Vec::new();

    // --- Gravity slope ---
    rules.push(RuleDef {
        id: "plumb-slope-small".into(),
        description: "Gravity drain slope 1/4\"/ft for pipes 3\" and smaller".into(),
        priority: RulePriority::Code,
        condition: RuleCondition {
            category: "pipe".into(),
            trade: Some(Trade::Plumbing),
            system: "sanitary".into(),
            size_range: Some([0.5, 3.0]),
            ..Default::default()
        },
        action: RuleAction { kind: "set_property".into(), key: "slope".into(), value: "1/4\"/ft".into(), numeric: Some(0.25), ..Default::default() },
        source: "IPC 704.1".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "plumb-slope-large".into(),
        description: "Gravity drain slope 1/8\"/ft for pipes 4\" and larger".into(),
        priority: RulePriority::Code,
        condition: RuleCondition {
            category: "pipe".into(),
            trade: Some(Trade::Plumbing),
            system: "sanitary".into(),
            size_range: Some([4.0, 24.0]),
            ..Default::default()
        },
        action: RuleAction { kind: "set_property".into(), key: "slope".into(), value: "1/8\"/ft".into(), numeric: Some(0.125), ..Default::default() },
        source: "IPC 704.1".into(),
        confidence: 1.0,
    });

    // --- Vent pipes ---
    rules.push(RuleDef {
        id: "plumb-vent-required".into(),
        description: "Every fixture trap requires a vent within the developed length".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "fixture".into(), trade: Some(Trade::Plumbing), ..Default::default() },
        action: RuleAction { kind: "check".into(), key: "vent_required".into(), ..Default::default() },
        source: "IPC 906".into(),
        confidence: 1.0,
    });

    // --- Fixture connections: CW/HW/HWR ---
    rules.push(RuleDef {
        id: "plumb-fixture-cw".into(),
        description: "Every plumbing fixture gets a cold water supply connection".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "fixture".into(), trade: Some(Trade::Plumbing), ..Default::default() },
        action: RuleAction {
            kind: "route".into(),
            value: "fixture_to_riser".into(),
            params: [("system".into(), json!("cold_water"))].into(),
            ..Default::default()
        },
        source: "IPC 604".into(),
        confidence: 0.95,
    });
    rules.push(RuleDef {
        id: "plumb-fixture-hw".into(),
        description: "Fixtures requiring hot water get HW and HWR connections".into(),
        priority: RulePriority::Code,
        condition: RuleCondition {
            category: "fixture".into(),
            trade: Some(Trade::Plumbing),
            properties: [("hot_water".into(), "yes".into())].into(),
            ..Default::default()
        },
        action: RuleAction {
            kind: "route".into(),
            value: "fixture_to_riser".into(),
            params: [("system".into(), json!("hot_water"))].into(),
            ..Default::default()
        },
        source: "IPC 607".into(),
        confidence: 0.95,
    });

    // --- Hanger spacing by material/size ---
    rules.push(RuleDef {
        id: "plumb-hanger-copper-small".into(),
        description: "Copper pipe ≤1.5\" hanger spacing 6 ft".into(),
        priority: RulePriority::Code,
        condition: RuleCondition {
            category: "pipe".into(),
            trade: Some(Trade::Plumbing),
            material: "copper".into(),
            size_range: Some([0.5, 1.5]),
            ..Default::default()
        },
        action: RuleAction { kind: "set_spacing".into(), key: "hanger_spacing_ft".into(), numeric: Some(6.0), ..Default::default() },
        source: "IPC Table 308.5".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "plumb-hanger-copper-large".into(),
        description: "Copper pipe 2\"+ hanger spacing 10 ft".into(),
        priority: RulePriority::Code,
        condition: RuleCondition {
            category: "pipe".into(),
            trade: Some(Trade::Plumbing),
            material: "copper".into(),
            size_range: Some([2.0, 12.0]),
            ..Default::default()
        },
        action: RuleAction { kind: "set_spacing".into(), key: "hanger_spacing_ft".into(), numeric: Some(10.0), ..Default::default() },
        source: "IPC Table 308.5".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "plumb-hanger-steel".into(),
        description: "Steel pipe hanger spacing 12 ft".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "pipe".into(), trade: Some(Trade::Plumbing), material: "steel".into(), ..Default::default() },
        action: RuleAction { kind: "set_spacing".into(), key: "hanger_spacing_ft".into(), numeric: Some(12.0), ..Default::default() },
        source: "IPC Table 308.5".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "plumb-hanger-pvc".into(),
        description: "PVC pipe hanger spacing 4 ft".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "pipe".into(), trade: Some(Trade::Plumbing), material: "PVC".into(), ..Default::default() },
        action: RuleAction { kind: "set_spacing".into(), key: "hanger_spacing_ft".into(), numeric: Some(4.0), ..Default::default() },
        source: "IPC Table 308.5".into(),
        confidence: 1.0,
    });

    RulePack { id: "plumbing-v1".into(), label: "Plumbing — IPC + trade defaults".into(), trade: Trade::Plumbing, version: 1, rules }
}

/// Mechanical trade rules.
#[allow(clippy::vec_init_then_push)]
pub fn mechanical_rules() -> RulePack {
    let mut rules = Vec::new();

    // --- Duct mains at tagged sizes ---
    rules.push(RuleDef {
        id: "mech-duct-main-tagged".into(),
        description: "Duct mains use the size tagged on the drawings".into(),
        priority: RulePriority::PrintFact,
        condition: RuleCondition {
            category: "duct".into(),
            trade: Some(Trade::Mechanical),
            system: "supply_duct".into(),
            properties: [("tagged_size".into(), "*".into())].into(),
            ..Default::default()
        },
        action: RuleAction { kind: "set_size".into(), key: "size".into(), value: "from_tag".into(), ..Default::default() },
        source: "Drawing tag".into(),
        confidence: 1.0,
    });

    // --- Branch sizing from CFM ---
    rules.push(RuleDef {
        id: "mech-duct-branch-cfm".into(),
        description: "When untagged, size branch ducts from CFM (round: low-velocity table)".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "duct".into(), trade: Some(Trade::Mechanical), system: "branch_duct".into(), ..Default::default() },
        action: RuleAction { kind: "set_size".into(), key: "size".into(), value: "from_cfm".into(), ..Default::default() },
        source: "SMACNA low velocity sizing".into(),
        confidence: 0.85,
    });

    // --- Flex duct to diffusers ---
    rules.push(RuleDef {
        id: "mech-flex-to-diffuser".into(),
        description: "Flex duct from branch to diffuser, max 6 ft".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "duct".into(), trade: Some(Trade::Mechanical), system: "flex".into(), ..Default::default() },
        action: RuleAction { kind: "check".into(), key: "max_length_ft".into(), numeric: Some(6.0), ..Default::default() },
        source: "SMACNA / energy code".into(),
        confidence: 0.9,
    });

    // --- Equipment clearances ---
    rules.push(RuleDef {
        id: "mech-equip-clearance".into(),
        description: "Mechanical equipment requires service clearance per manufacturer".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "equipment".into(), trade: Some(Trade::Mechanical), ..Default::default() },
        action: RuleAction { kind: "check".into(), key: "equipment_clearance".into(), ..Default::default() },
        source: "Manufacturer data / IMC".into(),
        confidence: 0.8,
    });

    // --- Duct hangers ---
    rules.push(RuleDef {
        id: "mech-hanger-rect-small".into(),
        description: "Rectangular duct ≤30\" wide: hangers every 8 ft".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "duct".into(), trade: Some(Trade::Mechanical), size_range: Some([4.0, 30.0]), ..Default::default() },
        action: RuleAction { kind: "set_spacing".into(), key: "hanger_spacing_ft".into(), numeric: Some(8.0), ..Default::default() },
        source: "SMACNA HVAC Duct Construction Standards".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "mech-hanger-rect-large".into(),
        description: "Rectangular duct >30\" wide: hangers every 4 ft".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "duct".into(), trade: Some(Trade::Mechanical), size_range: Some([31.0, 120.0]), ..Default::default() },
        action: RuleAction { kind: "set_spacing".into(), key: "hanger_spacing_ft".into(), numeric: Some(4.0), ..Default::default() },
        source: "SMACNA HVAC Duct Construction Standards".into(),
        confidence: 1.0,
    });

    RulePack { id: "mechanical-v1".into(), label: "Mechanical — IMC/SMACNA + trade defaults".into(), trade: Trade::Mechanical, version: 1, rules }
}

/// Fire protection trade rules.
#[allow(clippy::vec_init_then_push)]
pub fn fire_protection_rules() -> RulePack {
    let mut rules = Vec::new();

    // --- Mains/branches/heads from FP sheets ---
    rules.push(RuleDef {
        id: "fp-branch-from-schedule".into(),
        description: "Branch pipe size from NFPA pipe schedule by head count".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "pipe".into(), trade: Some(Trade::FireProtection), system: "branch".into(), ..Default::default() },
        action: RuleAction { kind: "set_size".into(), key: "size".into(), value: "from_head_count".into(), ..Default::default() },
        source: "NFPA 13 Table 22.7.5.0.1".into(),
        confidence: 0.95,
    });

    // --- Head spacing ---
    rules.push(RuleDef {
        id: "fp-head-spacing-light".into(),
        description: "Light hazard head spacing max 15 ft, max 225 sq ft per head".into(),
        priority: RulePriority::Code,
        condition: RuleCondition {
            category: "head".into(),
            trade: Some(Trade::FireProtection),
            properties: [("hazard".into(), "light".into())].into(),
            ..Default::default()
        },
        action: RuleAction {
            kind: "check".into(),
            key: "head_spacing_ft".into(),
            numeric: Some(15.0),
            max: Some(225.0), // sq ft per head
            ..Default::default()
        },
        source: "NFPA 13 Table 8.6.2.2.1(a)".into(),
        confidence: 1.0,
    });

    // --- Main size ---
    rules.push(RuleDef {
        id: "fp-main-size".into(),
        description: "FP main pipe sized for total downstream head count".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "pipe".into(), trade: Some(Trade::FireProtection), system: "main".into(), ..Default::default() },
        action: RuleAction { kind: "set_size".into(), key: "size".into(), value: "from_head_count".into(), ..Default::default() },
        source: "NFPA 13 pipe schedule".into(),
        confidence: 0.95,
    });

    RulePack {
        id: "fire-protection-v1".into(),
        label: "Fire Protection — NFPA 13 + trade defaults".into(),
        trade: Trade::FireProtection,
        version: 1,
        rules,
    }
}

/// Coordination rules (elevation bands, yield order, clearances).
#[allow(clippy::vec_init_then_push)]
pub fn coordination_rules() -> RulePack {
    let mut rules = Vec::new();

    // --- Elevation bands per trade ---
    rules.push(RuleDef {
        id: "coord-band-gravity".into(),
        description: "Gravity pipe (sanitary, storm) gets lowest band — it cannot change slope".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "pipe".into(), trade: Some(Trade::Coordination), system: "gravity".into(), ..Default::default() },
        action: RuleAction { kind: "yield_to".into(), key: "elevation_band".into(), value: "1".into(), numeric: Some(1.0), ..Default::default() },
        source: "Coordination practice".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "coord-band-duct".into(),
        description: "Ductwork gets second band (above gravity pipe)".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "duct".into(), trade: Some(Trade::Coordination), ..Default::default() },
        action: RuleAction { kind: "yield_to".into(), key: "elevation_band".into(), value: "2".into(), numeric: Some(2.0), ..Default::default() },
        source: "Coordination practice".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "coord-band-pressure-pipe".into(),
        description: "Pressure pipe gets third band".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "pipe".into(), trade: Some(Trade::Coordination), system: "pressure".into(), ..Default::default() },
        action: RuleAction { kind: "yield_to".into(), key: "elevation_band".into(), value: "3".into(), numeric: Some(3.0), ..Default::default() },
        source: "Coordination practice".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "coord-band-tray".into(),
        description: "Cable tray gets fourth band".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "tray".into(), trade: Some(Trade::Coordination), ..Default::default() },
        action: RuleAction { kind: "yield_to".into(), key: "elevation_band".into(), value: "4".into(), numeric: Some(4.0), ..Default::default() },
        source: "Coordination practice".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "coord-band-conduit".into(),
        description: "Conduit gets fifth (lowest-priority) band".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { category: "conduit".into(), trade: Some(Trade::Coordination), ..Default::default() },
        action: RuleAction { kind: "yield_to".into(), key: "elevation_band".into(), value: "5".into(), numeric: Some(5.0), ..Default::default() },
        source: "Coordination practice".into(),
        confidence: 1.0,
    });

    // --- Clearances to structure/ceiling ---
    rules.push(RuleDef {
        id: "coord-clearance-structure".into(),
        description: "Minimum 2\" clearance between MEP and structure".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { trade: Some(Trade::Coordination), ..Default::default() },
        action: RuleAction { kind: "check".into(), key: "clearance_to_structure_in".into(), numeric: Some(2.0), ..Default::default() },
        source: "Coordination practice".into(),
        confidence: 1.0,
    });
    rules.push(RuleDef {
        id: "coord-clearance-ceiling".into(),
        description: "Minimum 1\" clearance between MEP and ceiling".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { trade: Some(Trade::Coordination), ..Default::default() },
        action: RuleAction { kind: "check".into(), key: "clearance_to_ceiling_in".into(), numeric: Some(1.0), ..Default::default() },
        source: "Coordination practice".into(),
        confidence: 1.0,
    });

    // --- Yield order for clashes ---
    rules.push(RuleDef {
        id: "coord-yield-order".into(),
        description: "Yield order: gravity pipe > duct > pressure pipe > tray > conduit".into(),
        priority: RulePriority::Code,
        condition: RuleCondition { trade: Some(Trade::Coordination), ..Default::default() },
        action: RuleAction {
            kind: "set_property".into(),
            key: "yield_order".into(),
            value: "gravity_pipe,duct,pressure_pipe,tray,conduit".into(),
            ..Default::default()
        },
        source: "Coordination practice".into(),
        confidence: 1.0,
    });

    RulePack {
        id: "coordination-v1".into(),
        label: "Coordination — elevation bands + yield order".into(),
        trade: Trade::Coordination,
        version: 1,
        rules,
    }
}

/// Architectural rules: walls, doors, windows, floors and ceilings of the built model.
pub fn architectural_rules() -> RulePack {
    use RulePriority::{Code, Default, PrintFact, Spec};
    let a = Trade::Architectural;
    let rules = vec![
        rule(
            "arch-wall-typed",
            "Wall type read from the prints (tag, legend or wall type schedule)",
            Spec,
            when("wall", a, &[("type", "*")]),
            classify(),
            "Wall types",
            0.97,
        ),
        rule(
            "arch-wall-traced",
            "Wall traced from the plan's wall lines",
            PrintFact,
            when("wall", a, &[("source", "print")]),
            classify(),
            "Floor plan",
            0.92,
        ),
        rule(
            "arch-wall-type",
            "Every wall needs a wall type (assembly, rating); without one the default partition is assumed",
            Default,
            when("wall", a, &[]),
            require("type"),
            "Common sense",
            0.75,
        ),
        rule(
            "arch-wall-length",
            "A wall shorter than 4 in is a fragment of the trace, not a wall",
            Default,
            when("wall", a, &[]),
            check("length_in", Some(4.0), None),
            "Common sense",
            0.4,
        ),
        rule(
            "arch-wall-thickness",
            "Wall thickness 2 in to 36 in (thinner is a pair of lines that isn't a wall, thicker is two walls or a chase)",
            Default,
            when("wall", a, &[]),
            check("thickness_in", Some(2.0), Some(36.0)),
            "Common sense",
            0.5,
        ),
        rule(
            "arch-wall-height",
            "Wall height 1 ft to 40 ft",
            Default,
            when("wall", a, &[]),
            check("height_in", Some(12.0), Some(480.0)),
            "Common sense",
            0.5,
        ),
        rule(
            "arch-door-traced",
            "Door read from the plan's door symbol",
            PrintFact,
            when("door", a, &[("source", "print")]),
            classify(),
            "Floor plan",
            0.9,
        ),
        rule(
            "arch-door-width",
            "Door leaf 30 in to 8 ft wide (an accessible route needs 32 in clear; closets are exempt)",
            Code,
            when("door", a, &[]),
            check("length_in", Some(30.0), Some(96.0)),
            "Accessible route door clear width",
            0.6,
        ),
        rule(
            "arch-door-height",
            "Door at least 6 ft 6 in high (6 ft 8 in is standard)",
            Code,
            when("door", a, &[]),
            check("height_in", Some(78.0), None),
            "Means of egress door height",
            0.6,
        ),
        rule(
            "arch-window-traced",
            "Window read from the plan's window symbol",
            PrintFact,
            when("window", a, &[("source", "print")]),
            classify(),
            "Floor plan",
            0.9,
        ),
        rule(
            "arch-ceiling-height",
            "Ceiling at least 7 ft above the floor",
            Code,
            when("ceiling", a, &[]),
            check("above_floor_in", Some(84.0), None),
            "Minimum ceiling height",
            0.5,
        ),
        rule("arch-floor", "Floor slab from the level's outline", Default, when("floor", a, &[]), classify(), "Level outline", 0.9),
        rule("arch-default", "Building element from the model build", Default, when("", a, &[]), classify(), "Model build", 0.85),
    ];
    RulePack { id: "architectural-v1".into(), label: "Architectural — walls, doors, windows, ceilings".into(), trade: a, version: 1, rules }
}

/// Structural rules: columns, beams, slabs and footings of the built model.
pub fn structural_rules() -> RulePack {
    use RulePriority::{Default, PrintFact};
    let t = Trade::Structural;
    let rules = vec![
        rule(
            "struct-column-traced",
            "Column traced from the plan's column outline",
            PrintFact,
            when("column", t, &[("source", "print")]),
            classify(),
            "Floor plan",
            0.92,
        ),
        rule(
            "struct-column-height",
            "A column stands at least 1 ft",
            Default,
            when("column", t, &[]),
            check("height_in", Some(12.0), None),
            "Common sense",
            0.5,
        ),
        rule("struct-default", "Structural element from the model build", Default, when("", t, &[]), classify(), "Model build", 0.85),
    ];
    RulePack { id: "structural-v1".into(), label: "Structural — columns, beams, slabs".into(), trade: t, version: 1, rules }
}

/// How sure a trade's run is, from where its size and height came from.
fn basis_rules(t: Trade, prefix: &str) -> Vec<RuleDef> {
    use RulePriority::{Default, PrintFact};
    vec![
        rule(
            &format!("{prefix}-documented"),
            "Size and height read from the prints (tag, schedule or note) or set by a person",
            PrintFact,
            when("", t, &[("basis", "documented")]),
            classify(),
            "Prints",
            0.95,
        ),
        rule(
            &format!("{prefix}-partial"),
            "Size or height read from the prints; the other is the system's default",
            Default,
            when("", t, &[("basis", "partial")]),
            classify(),
            "Prints + system default",
            0.8,
        ),
        rule(
            &format!("{prefix}-assumed"),
            "Size and height are the system's defaults: nothing on the prints says",
            Default,
            when("", t, &[("basis", "assumed")]),
            classify(),
            "System default",
            0.55,
        ),
        rule(&format!("{prefix}-default"), "Built from the prints", Default, when("", t, &[]), classify(), "Model build", 0.85),
    ]
}

/// All built-in rule packs.
pub fn builtin_packs() -> Vec<RulePack> {
    let mut packs = vec![
        architectural_rules(),
        structural_rules(),
        electrical_rules(),
        plumbing_rules(),
        mechanical_rules(),
        fire_protection_rules(),
        coordination_rules(),
    ];
    for pack in &mut packs {
        let prefix = match pack.trade {
            Trade::Electrical => "elec",
            Trade::Plumbing => "plumb",
            Trade::Mechanical => "mech",
            Trade::FireProtection => "fp",
            _ => continue,
        };
        pack.rules.extend(basis_rules(pack.trade, prefix));
    }
    packs
}

/// The packs in force: the built-in ones, each replaced by an owner-edited pack of the same id,
/// and the owner's other packs.
pub fn effective_packs(st: &RuleModelState) -> Vec<RulePack> {
    let mut packs: Vec<RulePack> = builtin_packs().into_iter().map(|b| st.packs.iter().find(|p| p.id == b.id).cloned().unwrap_or(b)).collect();
    for p in &st.packs {
        if !packs.iter().any(|x| x.id == p.id) {
            packs.push(p.clone());
        }
    }
    packs
}

fn rule(id: &str, description: &str, priority: RulePriority, condition: RuleCondition, action: RuleAction, source: &str, confidence: f64) -> RuleDef {
    RuleDef { id: id.into(), description: description.into(), priority, condition, action, source: source.into(), confidence }
}

fn when(category: &str, trade: Trade, props: &[(&str, &str)]) -> RuleCondition {
    RuleCondition {
        category: category.into(),
        trade: Some(trade),
        properties: props.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect(),
        ..Default::default()
    }
}

fn classify() -> RuleAction {
    RuleAction { kind: "classify".into(), ..Default::default() }
}

fn check(key: &str, min: Option<f64>, max: Option<f64>) -> RuleAction {
    RuleAction { kind: "check".into(), key: key.into(), numeric: min, max, ..Default::default() }
}

fn require(key: &str) -> RuleAction {
    RuleAction { kind: "require".into(), key: key.into(), ..Default::default() }
}

// ── state helpers ─────────────────────────────────────────────────────────────

fn load_state(d: &cadaddy_doc::Drawing) -> RuleModelState {
    d.agent
        .as_deref()
        .and_then(|text| {
            // The agent field holds the review queue's state too; ours is under `rulemodel`.
            let v: Value = serde_json::from_str(text).ok()?;
            v.get("rulemodel").and_then(|rm| serde_json::from_value(rm.clone()).ok())
        })
        .unwrap_or_else(|| RuleModelState { threshold: DEFAULT_THRESHOLD, ..Default::default() })
}

fn save_state(s: &mut Session, st: &RuleModelState) -> Result<()> {
    let ours = serde_json::to_value(st).map_err(|e| bad("rules", format!("the rules state can't be saved: {e}")))?;
    let doc = s.doc_mut()?;
    // Merge into the agent JSON: keep the review queue's state, add/update ours.
    let mut root: Value = doc.agent.as_deref().and_then(|t| serde_json::from_str(t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    if let Some(obj) = root.as_object_mut() {
        obj.insert("rulemodel".into(), ours);
    }
    doc.agent = serde_json::to_string(&root).ok();
    Ok(())
}

// ── FNV-1a hash (stable ids) ─────────────────────────────────────────────────

fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h & 0xffff_ffff)
}

// ── rule matching ────────────────────────────────────────────────────────────

/// Check if a rule condition matches an element's properties. A property value `*` needs the
/// key present, `!` needs it absent.
pub fn condition_matches(
    cond: &RuleCondition,
    elem_cat: &str,
    elem_sys: &str,
    elem_trade: Trade,
    elem_loc: &str,
    elem_mat: &str,
    elem_size: f64,
    elem_props: &BTreeMap<String, String>,
) -> bool {
    if !cond.category.is_empty() && !cond.category.eq_ignore_ascii_case(elem_cat) {
        return false;
    }
    if !cond.system.is_empty() && !cond.system.eq_ignore_ascii_case(elem_sys) {
        return false;
    }
    if cond.trade.is_some_and(|t| t != elem_trade && t != Trade::Coordination) {
        return false;
    }
    if !cond.material.is_empty() && !cond.material.eq_ignore_ascii_case(elem_mat) {
        return false;
    }
    if !cond.location.is_empty() && !cond.location.eq_ignore_ascii_case(elem_loc) {
        return false;
    }
    if cond.size_range.is_some_and(|[min, max]| elem_size < min || elem_size > max) {
        return false;
    }
    for (key, val) in &cond.properties {
        match val.as_str() {
            "*" if !elem_props.contains_key(key) => return false,
            "!" if elem_props.contains_key(key) => return false,
            "*" | "!" => {}
            _ => match elem_props.get(key) {
                Some(v) if v.eq_ignore_ascii_case(val) => {}
                _ => return false,
            },
        }
    }
    true
}

/// One element of the built model as the rules see it: what it is, where, and what was
/// measured on it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelElem {
    /// The model object's handle (hex).
    pub id: String,
    /// What the element is known by from one build to the next: the MEP piece's id, else the
    /// part and the plan object it was made from, else (made from nothing on the plan) its
    /// handle.
    pub key: String,
    pub trade: Trade,
    /// `wall`, `door`, `window`, `floor`, `ceiling`, `column`…; for MEP the run's kind (`duct`,
    /// `pipe`, `conduit`, `tray`, `cable`, `equipment`).
    pub category: String,
    pub system: String,
    pub level: String,
    /// Tags: `level`, `part`, `source` (`print` or `generated`), `type`, `material`, `room`,
    /// `system`, `kind`, `piece`, `size`, `run`, `size_source`, `height_source`, `basis`,
    /// `service` (pipes: `gravity` or `pressure`)…
    pub props: BTreeMap<String, String>,
    /// Measured on the object: `length_in`, `thickness_in`, `height_in`, `above_floor_in`,
    /// `size_in`.
    pub measures: BTreeMap<String, f64>,
}

fn elem_matches(cond: &RuleCondition, e: &ModelElem) -> bool {
    // Coordination rules are about the MEP trades.
    if cond.trade == Some(Trade::Coordination) && matches!(e.trade, Trade::Architectural | Trade::Structural) {
        return false;
    }
    let p = |k: &str| e.props.get(k).map_or("", String::as_str);
    // A rule's system may name the service class (`gravity`, `pressure`) instead of a system.
    let sys = if !cond.system.is_empty() && cond.system.eq_ignore_ascii_case(p("service")) { p("service") } else { e.system.as_str() };
    let size = e.measures.get("size_in").copied().unwrap_or(0.0);
    condition_matches(cond, &e.category, sys, e.trade, p("location"), p("material"), size, &e.props)
}

/// How many things a condition names: the more specific rule wins a tie in priority.
fn specificity(c: &RuleCondition) -> usize {
    usize::from(!c.category.is_empty())
        + usize::from(!c.system.is_empty())
        + usize::from(!c.material.is_empty())
        + usize::from(!c.location.is_empty())
        + usize::from(c.size_range.is_some())
        + c.properties.len()
}

// ── reading the model ────────────────────────────────────────────────────────

/// Most vertices of one object measured.
const MEASURE_VERTICES: usize = 4_096;
/// Most tags of one object read.
const MAX_TAGS: usize = 128;

fn units_per_foot(sp: &cadaddy_walk3d::BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

fn is_structural(part: &str) -> bool {
    matches!(part, "column" | "beam" | "grid" | "gridcolumn" | "slab" | "joist" | "footing" | "pier" | "pad" | "rebar" | "base" | "encasement")
}

/// A pipe system that drains by gravity (sanitary, waste, vent, storm, condensate).
fn is_gravity(system: &str) -> bool {
    let up = system.to_ascii_uppercase();
    ["SAN", "WASTE", "VENT", "STORM", "DRAIN", "GRAV", "-SW", "SW-", "CONDENS"].iter().any(|k| up.contains(k))
}

fn is_fire(system: &str) -> bool {
    let up = system.to_ascii_uppercase();
    ["FP", "FIRE", "SPRINK", "STANDPIPE"].iter().any(|k| up.contains(k))
}

/// Length and thickness along the principal axes of the points on the plan, and the lowest and
/// highest point.
fn measure(points: &[cadaddy_geom::Vec3]) -> Option<(f64, f64, f64, f64)> {
    let pts: Vec<&cadaddy_geom::Vec3> =
        points.iter().filter(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite()).take(MEASURE_VERTICES).collect();
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as f64;
    let (mx, my) = pts.iter().fold((0.0, 0.0), |(x, y), p| (x + p.x, y + p.y));
    let (mx, my) = (mx / n, my / n);
    let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
    for p in &pts {
        let (dx, dy) = (p.x - mx, p.y - my);
        cxx += dx * dx;
        cyy += dy * dy;
        cxy += dx * dy;
    }
    let angle = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
    let (c, s) = (angle.cos(), angle.sin());
    let (mut u0, mut u1, mut v0, mut v1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    let (mut z0, mut z1) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in &pts {
        let (dx, dy) = (p.x - mx, p.y - my);
        let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
        u0 = u0.min(u);
        u1 = u1.max(u);
        v0 = v0.min(v);
        v1 = v1.max(v);
        z0 = z0.min(p.z);
        z1 = z1.max(p.z);
    }
    let (a, b) = (u1 - u0, v1 - v0);
    let out = (a.max(b), a.min(b), z0, z1);
    (out.0.is_finite() && out.1.is_finite() && z0.is_finite() && z1.is_finite()).then_some(out)
}

/// Every object the model build made (tagged `CADADDY_WALLGEN`), as the rules see it.
pub fn read_model(
    d: &cadaddy_doc::Drawing,
    sp: &cadaddy_walk3d::BuildingSpec,
    overrides: &BTreeMap<String, BTreeMap<String, String>>,
) -> Vec<ModelElem> {
    let upf = units_per_foot(sp);
    let to_in = |v: f64| v / upf * 12.0;
    let runs: BTreeMap<String, &cadaddy_walk3d::MepRun> = sp.mep.runs.iter().map(|r| (super::mep::run_base(r), r)).collect();
    let mut out = Vec::new();
    for e in d.model.iter() {
        if out.len() >= MAX_ELEMENTS {
            break;
        }
        let x = &e.common.xdata;
        let Some(at) = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP)) else { continue };
        let tags: Vec<&str> =
            x.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.as_str()).take(MAX_TAGS).collect();
        let (Some(part), Some(level)) = (tags.first().copied(), tags.get(1).copied()) else { continue };
        let source = tags.get(2).copied().unwrap_or("");
        let mut props: BTreeMap<String, String> = BTreeMap::new();
        props.insert("level".into(), level.into());
        props.insert("part".into(), part.into());
        let traced = !source.trim_start_matches('0').is_empty();
        props.insert("source".into(), if traced { "print" } else { "generated" }.into());
        for t in tags.iter().skip(3) {
            let Some((k, v)) = t.split_once(':') else { continue };
            let k = if k == "part" { "piece".to_string() } else { k.trim().to_ascii_lowercase() };
            if k.is_empty() || k == "level" || k == "source" {
                continue;
            }
            props.entry(k).or_insert_with(|| v.trim().to_string());
        }
        let mut measures: BTreeMap<String, f64> = BTreeMap::new();
        if let EntityKind::Mesh(m) = &e.kind
            && let Some((len, thick, z0, z1)) = measure(&m.vertices)
        {
            measures.insert("length_in".into(), to_in(len));
            measures.insert("thickness_in".into(), to_in(thick));
            measures.insert("height_in".into(), to_in(z1 - z0));
            if let Some(l) = sp.level(level) {
                measures.insert("above_floor_in".into(), to_in(z0) - l.elevation_ft * 12.0);
            }
        }
        let system = props.get("system").cloned().unwrap_or_default();
        let (trade, category) = if part == "mep" {
            let kind = props.get("kind").cloned().unwrap_or_else(|| "equipment".into());
            let trade = match kind.as_str() {
                "duct" => Trade::Mechanical,
                "pipe" if is_fire(&system) => Trade::FireProtection,
                "pipe" => Trade::Plumbing,
                "conduit" | "tray" | "cable" => Trade::Electrical,
                _ if is_fire(&system) => Trade::FireProtection,
                _ => Trade::Mechanical,
            };
            if kind == "pipe" {
                props.insert("service".into(), if is_gravity(&system) { "gravity" } else { "pressure" }.into());
            }
            let base = props.get("mepid").map(|id| id.rsplit_once('/').map_or(id.as_str(), |(b, _)| b).to_string());
            if let Some(r) = base.as_deref().and_then(|b| runs.get(b)) {
                let known = |src: &str| !src.is_empty() && src != "default";
                let (size, height) = (known(&r.size_source), known(&r.height_source));
                props.insert("run".into(), r.id.clone());
                props.insert("size_source".into(), if r.size_source.is_empty() { "default".into() } else { r.size_source.clone() });
                props.insert("height_source".into(), if r.height_source.is_empty() { "default".into() } else { r.height_source.clone() });
                let basis = match (size || r.reviewed, height || r.reviewed) {
                    (true, true) => "documented",
                    (false, false) => "assumed",
                    _ => "partial",
                };
                props.insert("basis".into(), basis.into());
                if !r.edited_by.is_empty() {
                    props.insert("edited_by".into(), r.edited_by.clone());
                }
                if r.size_in[0].is_finite() && r.size_in[0] > 0.0 {
                    measures.insert("size_in".into(), r.size_in[0]);
                }
            }
            (trade, kind)
        } else if part == "device" {
            (Trade::Electrical, part.to_string())
        } else if is_structural(part) {
            (Trade::Structural, part.to_string())
        } else {
            (Trade::Architectural, part.to_string())
        };
        let key = match props.get("mepid") {
            Some(id) => format!("mep:{id}"),
            None if traced => format!("{part}@{}#{}", source.trim_start_matches('0'), level),
            None => e.handle.hex(),
        };
        if let Some(o) = overrides.get(&key) {
            for (k, v) in o {
                props.insert(k.clone(), v.clone());
            }
        }
        out.push(ModelElem { id: e.handle.hex(), key, trade, category, system, level: level.to_string(), props, measures });
    }
    out
}

// ── rule evaluation ──────────────────────────────────────────────────────────

/// Most element ids one finding keeps.
const FINDING_ELEMENTS: usize = 5_000;

/// One finding being collected: the elements one rule raised the same issue about on a level.
struct Group {
    kind: &'static str,
    rule_id: String,
    description: String,
    source: String,
    confidence: f64,
    trade: Trade,
    level: String,
    category: String,
    elements: Vec<String>,
    count: usize,
    examples: Vec<String>,
}

fn add_to_group(groups: &mut BTreeMap<(String, String, String), Group>, kind: &'static str, r: &RuleDef, e: &ModelElem, example: Option<String>) {
    let g = groups.entry((kind.to_string(), r.id.clone(), e.level.clone())).or_insert_with(|| Group {
        kind,
        rule_id: r.id.clone(),
        description: r.description.clone(),
        source: r.source.clone(),
        confidence: r.confidence,
        trade: e.trade,
        level: e.level.clone(),
        category: e.category.clone(),
        elements: Vec::new(),
        count: 0,
        examples: Vec::new(),
    });
    g.count += 1;
    if g.elements.len() < FINDING_ELEMENTS {
        g.elements.push(e.id.clone());
    }
    if let Some(x) = example
        && g.examples.len() < 3
    {
        g.examples.push(x);
    }
}

/// What the rules decided about the model: one placed element per model element, the findings
/// grouped by rule and level. Project rules come first, then each pack's rules by priority
/// (project > spec > print fact > code > default; the more specific first within a tier).
pub fn evaluate_model(
    packs: &[RulePack],
    project: &[RuleDef],
    elems: &[ModelElem],
    threshold: f64,
    accepted: &BTreeSet<String>,
    rejected: &BTreeSet<String>,
) -> EvalResult {
    let mut rules: Vec<&RuleDef> = project.iter().chain(packs.iter().flat_map(|p| p.rules.iter())).collect();
    rules.sort_by_key(|r| (r.priority as u8, std::cmp::Reverse(specificity(&r.condition))));

    let mut groups: BTreeMap<(String, String, String), Group> = BTreeMap::new();
    let mut result = EvalResult::default();
    for e in elems.iter().take(MAX_ELEMENTS) {
        let matched: Vec<&RuleDef> = rules.iter().copied().filter(|r| elem_matches(&r.condition, e)).collect();
        // A `waive` rule (an answer saved as a project rule) switches a rule or a key off.
        let waived: BTreeSet<&str> = matched.iter().filter(|r| r.action.kind == "waive").map(|r| r.action.value.as_str()).collect();
        let live: Vec<&RuleDef> = matched.iter().copied().filter(|r| r.action.kind != "waive" && !waived.contains(r.id.as_str())).collect();
        let base = live.iter().find(|r| r.action.kind == "classify").or_else(|| live.first()).copied();
        let mut confidence = base.map_or(0.5, |r| r.confidence.clamp(0.0, 1.0));
        let mut agent_decided = base.is_some_and(|r| r.priority == RulePriority::Project) || e.props.contains_key("edited_by");
        let mut properties: BTreeMap<String, String> = BTreeMap::new();
        let mut issues: Vec<(&'static str, &RuleDef, Option<String>)> = Vec::new();
        let mut seen_keys: BTreeSet<&str> = BTreeSet::new();
        for r in &live {
            let a = &r.action;
            match a.kind.as_str() {
                "check" | "require" => {
                    // The highest-priority rule for a key decides it.
                    if a.key.is_empty() || !seen_keys.insert(a.key.as_str()) || waived.contains(a.key.as_str()) {
                        continue;
                    }
                    if a.kind == "require" {
                        if !e.props.contains_key(&a.key) {
                            issues.push(("missing_input", r, None));
                        }
                    } else if let Some(v) = e.measures.get(&a.key) {
                        let low = a.numeric.is_some_and(|m| *v < m);
                        let high = a.max.is_some_and(|m| *v > m);
                        if low || high {
                            let bound = if low { format!("min {}", a.numeric.unwrap_or(0.0)) } else { format!("max {}", a.max.unwrap_or(0.0)) };
                            issues.push(("check", r, Some(format!("{} {} {v:.1} ({bound})", e.id, a.key))));
                        }
                    }
                }
                "set_property" | "set_size" | "set_height" | "set_spacing" | "route" | "place" | "yield_to" => {
                    let key = if a.key.is_empty() { a.kind.clone() } else { a.key.clone() };
                    let value = if a.value.is_empty() { a.numeric.map(|n| format!("{n}")).unwrap_or_default() } else { a.value.clone() };
                    properties.entry(key).or_insert(value);
                }
                _ => {}
            }
        }
        for (_, r, _) in &issues {
            confidence = confidence.min(r.confidence.clamp(0.0, 1.0));
        }
        if accepted.contains(&e.key) {
            confidence = 1.0;
            agent_decided = true;
            issues.clear();
        } else if rejected.contains(&e.key) {
            confidence = 0.0;
            agent_decided = true;
        }
        if let Some(b) = base {
            if rejected.contains(&e.key) {
                add_to_group(&mut groups, "rejected", b, e, None);
            } else if issues.is_empty() && confidence < threshold {
                add_to_group(&mut groups, "low_confidence", b, e, None);
            }
        }
        for (kind, r, example) in issues {
            add_to_group(&mut groups, kind, r, e, example);
        }
        let mut inputs: BTreeMap<String, String> = e.measures.iter().map(|(k, v)| (k.clone(), format!("{v:.2}"))).collect();
        for k in ["level", "source", "type", "system", "basis", "size_source", "height_source", "run", "piece"] {
            if let Some(v) = e.props.get(k) {
                inputs.insert(k.into(), v.clone());
            }
        }
        result.elements.push(PlacedElement {
            id: e.id.clone(),
            trade: e.trade,
            category: e.category.clone(),
            system: e.system.clone(),
            level: e.level.clone(),
            rule_id: base.map_or_else(|| "none".to_string(), |r| r.id.clone()),
            inputs,
            confidence,
            properties,
            agent_decided,
        });
    }

    for g in groups.into_values() {
        if result.findings.len() >= MAX_FINDINGS {
            break;
        }
        result.findings.push(group_finding(g));
    }
    completeness(elems, &mut result.findings);
    result.summary = summarize(&result.elements, &result.findings);
    result
}

fn group_finding(g: Group) -> Finding {
    let what = if g.count == 1 { format!("1 {}", g.category) } else { format!("{} {}s", g.count, g.category) };
    let on = if g.level.is_empty() { String::new() } else { format!(" on {}", g.level) };
    let (severity, proposed, alternatives) = match g.kind {
        "check" => (
            if g.confidence < 0.5 { "high" } else { "medium" },
            if g.rule_id == "arch-wall-length" {
                "Delete: a fragment of the trace".to_string()
            } else {
                "Check against the prints and fix the element".to_string()
            },
            vec![
                option("The element is right: accept it", "accept"),
                option("Delete it", "delete"),
                option("Move it", "move"),
                option("Keep it and save a project rule for this", "write_rule"),
            ],
        ),
        "missing_input" => (
            "low",
            format!("Assume the default ({})", g.description),
            vec![option("Set the type from the prints", "swap_type"), option("Accept the default", "accept")],
        ),
        "rejected" => ("high", "Fix or delete the rejected element".to_string(), vec![option("Delete it", "delete"), option("Move it", "move")]),
        _ => (
            "medium",
            "Confirm from the prints".to_string(),
            vec![option("Accept the rule's answer", "accept"), option("Change size or height", "resize"), option("Reject it", "reject")],
        ),
    };
    let mut detail = format!("Rule {} ({}): {}. Source: {}.", g.rule_id, g.kind, g.description, g.source);
    if !g.examples.is_empty() {
        detail.push_str(&format!(" For example {}.", g.examples.join("; ")));
    }
    Finding {
        id: fnv(&format!("{}|{}|{}", g.kind, g.rule_id, g.level.to_ascii_uppercase())),
        kind: g.kind.into(),
        trade: g.trade,
        level: g.level,
        severity: severity.into(),
        title: format!("{what}{on}: {}", g.description),
        detail,
        elements: g.elements,
        proposed,
        alternatives,
        status: "open".into(),
        resolution: None,
    }
}

/// Completeness: a level with walls has doors.
fn completeness(elems: &[ModelElem], findings: &mut Vec<Finding>) {
    let mut per_level: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    for e in elems.iter().filter(|e| e.trade == Trade::Architectural) {
        *per_level.entry(e.level.as_str()).or_default().entry(e.category.as_str()).or_default() += 1;
    }
    for (level, counts) in per_level {
        let walls = counts.get("wall").copied().unwrap_or(0);
        if walls == 0 {
            continue;
        }
        for (missing, why) in [("door", "no doors were read from the plan")] {
            if counts.get(missing).copied().unwrap_or(0) > 0 || findings.len() >= MAX_FINDINGS {
                continue;
            }
            findings.push(Finding {
                id: fnv(&format!("completeness|{missing}|{}", level.to_ascii_uppercase())),
                kind: "completeness".into(),
                trade: Trade::Architectural,
                level: level.into(),
                severity: "medium".into(),
                title: format!("{level}: {walls} walls but {why}"),
                detail: format!(
                    "A level with walls has {missing}s: check the import and the layer roles (bspec.get), or accept that this level has none."
                ),
                elements: Vec::new(),
                proposed: format!("Check the {missing} layer's role and build again"),
                alternatives: vec![
                    option("Accept: this level has none", "accept"),
                    option("Build again after fixing the layer roles", "rerun_rules"),
                ],
                status: "open".into(),
                resolution: None,
            });
        }
    }
}

fn option(label: &str, action: &str) -> ActionOption {
    ActionOption { label: label.into(), action: action.into(), params: BTreeMap::new() }
}

fn summarize(elements: &[PlacedElement], findings: &[Finding]) -> BTreeMap<String, TradeSummary> {
    let mut out: BTreeMap<String, TradeSummary> = BTreeMap::new();
    let mut conf: BTreeMap<String, f64> = BTreeMap::new();
    for e in elements {
        let t = out.entry(e.trade.as_str().into()).or_insert_with(|| TradeSummary { trade: e.trade.as_str().into(), ..Default::default() });
        t.total += 1;
        if e.agent_decided {
            t.agent_decided += 1;
        } else {
            t.rule_placed += 1;
        }
        *conf.entry(e.trade.as_str().into()).or_default() += e.confidence;
    }
    for f in findings {
        let t = out.entry(f.trade.as_str().into()).or_insert_with(|| TradeSummary { trade: f.trade.as_str().into(), ..Default::default() });
        match f.status.as_str() {
            "open" => t.findings_open += 1,
            "resolved" => t.findings_resolved += 1,
            _ => {}
        }
    }
    for (k, t) in &mut out {
        t.avg_confidence = if t.total > 0 { conf.get(k).copied().unwrap_or(0.0) / t.total as f64 } else { 0.0 };
    }
    out
}

fn is_clash(f: &Finding) -> bool {
    f.kind == "clash" || f.kind == "judgement"
}

/// The findings after an evaluation: the new ones (a deferred one stays deferred), then the
/// earlier ones the evaluation didn't make again that still matter: resolved and deferred ones,
/// the clash loop's, and those outside the evaluated scope.
fn merge_findings(prev: &[Finding], new: Vec<Finding>, in_scope: impl Fn(&Finding) -> bool) -> Vec<Finding> {
    let mut out: Vec<Finding> = new
        .into_iter()
        .map(|mut f| {
            // An answer stands while the finding is about nothing new: deferred, or accepted.
            let answered = |p: &&Finding| {
                p.id == f.id
                    && (p.status == "deferred" || (p.status == "resolved" && p.resolution.as_ref().is_some_and(|r| r.action == "accept")))
                    && f.elements.iter().all(|e| p.elements.contains(e))
            };
            if let Some(p) = prev.iter().find(answered) {
                f.status.clone_from(&p.status);
                f.resolution.clone_from(&p.resolution);
            }
            f
        })
        .collect();
    for p in prev {
        if out.len() >= MAX_FINDINGS {
            break;
        }
        if out.iter().any(|f| f.id == p.id) {
            continue;
        }
        if is_clash(p) || p.status != "open" || !in_scope(p) {
            out.push(p.clone());
        } else {
            // Open before and not made again: what it was about was fixed.
            let mut fixed = p.clone();
            fixed.status = "resolved".into();
            fixed.resolution = Some(Resolution { action: "fixed".into(), author: "rules".into(), citation: String::new(), save_as_rule: false });
            out.push(fixed);
        }
    }
    out
}

/// Accuracy: the share of elements the rules (or an agent) placed at or above the threshold.
pub fn compute_score(elements: &[PlacedElement], threshold: f64) -> AccuracyScore {
    if elements.is_empty() {
        return AccuracyScore::default();
    }
    let mut by_trade: BTreeMap<String, TradeScore> = BTreeMap::new();
    let mut per_cat: BTreeMap<(String, String), (usize, usize)> = BTreeMap::new();
    let mut total_accepted = 0usize;
    for elem in elements {
        let ts = by_trade.entry(elem.trade.as_str().into()).or_default();
        ts.sample_size += 1;
        let ok = elem.confidence >= threshold;
        if ok {
            ts.accepted += 1;
            total_accepted += 1;
        } else {
            ts.rejected += 1;
        }
        let c = per_cat.entry((elem.trade.as_str().into(), elem.category.clone())).or_default();
        c.0 += 1;
        c.1 += usize::from(ok);
    }
    for ((trade, cat), (n, ok)) in per_cat {
        if let Some(ts) = by_trade.get_mut(&trade) {
            ts.by_category.insert(cat, if n > 0 { ok as f64 / n as f64 } else { 0.0 });
        }
    }
    for ts in by_trade.values_mut() {
        ts.score = if ts.sample_size > 0 { ts.accepted as f64 / ts.sample_size as f64 } else { 0.0 };
    }
    let total = elements.len();
    AccuracyScore {
        overall: total_accepted as f64 / total as f64,
        by_trade,
        sample_size: total,
        accepted: total_accepted,
        rejected: total - total_accepted,
    }
}

/// Per trade and category: count, quantity, how much the rules and agents placed, open
/// assumptions and the score.
pub fn generate_report(elements: &[PlacedElement], findings: &[Finding], threshold: f64) -> Vec<ReportLine> {
    #[derive(Default)]
    struct Acc {
        count: usize,
        agent: usize,
        ok: usize,
        length_in: f64,
        open: usize,
    }
    let mut by_key: BTreeMap<(String, String), Acc> = BTreeMap::new();
    let mut key_of: BTreeMap<&str, (String, String)> = BTreeMap::new();
    for e in elements {
        let key = (e.trade.as_str().to_string(), e.category.clone());
        let a = by_key.entry(key.clone()).or_default();
        a.count += 1;
        a.agent += usize::from(e.agent_decided);
        a.ok += usize::from(e.confidence >= threshold);
        let linear = matches!(e.category.as_str(), "wall" | "duct" | "pipe" | "conduit" | "tray" | "cable");
        if linear && e.inputs.get("piece").is_none_or(|p| p == "segment") {
            a.length_in += e.inputs.get("length_in").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0);
        }
        key_of.insert(e.id.as_str(), key);
    }
    // Open assumptions: the elements open findings are about.
    for f in findings.iter().filter(|f| f.status == "open" && !is_clash(f)) {
        for id in &f.elements {
            if let Some(a) = key_of.get(id.as_str()).and_then(|k| by_key.get_mut(k)) {
                a.open += 1;
            }
        }
    }
    by_key
        .into_iter()
        .map(|((trade, category), a)| {
            let pct = |n: usize| if a.count > 0 { (n as f64 / a.count as f64 * 1000.0).round() / 10.0 } else { 0.0 };
            ReportLine {
                quantity: {
                    let n = if a.count == 1 { "1 element".to_string() } else { format!("{} elements", a.count) };
                    if a.length_in > 0.0 { format!("{n}, {:.0} ft", a.length_in / 12.0) } else { n }
                },
                category,
                trade,
                count: a.count,
                pct_rule_placed: pct(a.count - a.agent),
                pct_agent_decided: pct(a.agent),
                open_assumptions: a.open,
                score: if a.count > 0 { a.ok as f64 / a.count as f64 } else { 0.0 },
            }
        })
        .collect()
}

// ── the live model ────────────────────────────────────────────────────────────

/// The building spec (or an empty one) and the model's elements as the rules see them.
fn live_model(s: &Session, overrides: &BTreeMap<String, BTreeMap<String, String>>) -> Result<(cadaddy_walk3d::BuildingSpec, Vec<ModelElem>)> {
    let (sp, _) = super::bspec::current(s, "rules")?;
    let elems = read_model(s.doc()?, &sp, overrides);
    Ok((sp, elems))
}

fn id_set(v: &[String]) -> BTreeSet<String> {
    v.iter().cloned().collect()
}

/// Evaluate the live model under the state's packs and project rules, optionally only one
/// level or trade.
fn evaluate_live(s: &Session, st: &RuleModelState, level: Option<&str>, trade: Option<Trade>) -> Result<(Vec<ModelElem>, EvalResult)> {
    let (_, mut elems) = live_model(s, &st.overrides)?;
    elems.retain(|e| level.is_none_or(|l| e.level.eq_ignore_ascii_case(l)) && trade.is_none_or(|t| e.trade == t));
    let packs = effective_packs(st);
    let threshold = if st.threshold.is_finite() && st.threshold > 0.0 { st.threshold } else { DEFAULT_THRESHOLD };
    let result = evaluate_model(&packs, &st.project_rules, &elems, threshold, &id_set(&st.accepted), &id_set(&st.rejected));
    Ok((elems, result))
}

/// Evaluate and keep the findings in the drawing: what `rules.evaluate` and every model action do.
fn reevaluate(s: &mut Session, st: &mut RuleModelState, level: Option<&str>, trade: Option<Trade>) -> Result<EvalResult> {
    let (_, result) = evaluate_live(s, st, level, trade)?;
    let in_scope = |f: &Finding| level.is_none_or(|l| f.level.eq_ignore_ascii_case(l)) && trade.is_none_or(|t| f.trade == t);
    st.findings = merge_findings(&st.findings, result.findings.clone(), in_scope);
    Ok(result)
}

fn open_count(st: &RuleModelState) -> usize {
    st.findings.iter().filter(|f| f.status == "open").count()
}

fn severity_rank(s: &str) -> u8 {
    match s {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    }
}

/// A finding as the worklist shows it: the first few element ids and how many there are.
fn finding_json(f: &Finding) -> Value {
    json!({
        "id": f.id,
        "kind": f.kind,
        "trade": f.trade,
        "level": f.level,
        "severity": f.severity,
        "title": f.title,
        "detail": f.detail,
        "count": f.elements.len(),
        "elements": f.elements.iter().take(10).collect::<Vec<_>>(),
        "proposed": f.proposed,
        "alternatives": f.alternatives,
        "status": f.status,
    })
}

// ── clash loop ────────────────────────────────────────────────────────────────

/// Clearances the clash loop keeps, feet: between runs, below structure, above the ceiling.
const CLEAR_RUN_FT: f64 = 1.0 / 12.0;
const CLEAR_STRUCTURE_FT: f64 = 2.0 / 12.0;
const CLEAR_CEILING_FT: f64 = 1.0 / 12.0;
/// The floor structure's depth the clash detector assumes, feet.
const SLAB_FT: f64 = 0.5;
/// Most clashes the loop turns into findings.
const MAX_CLASH_FINDINGS: usize = 500;

/// A system's elevation band from the yield rules (1 = gravity pipe keeps its place … 5 =
/// conduit yields to everything); none when no rule gives one.
fn system_band(rules: &[&RuleDef], sp: &cadaddy_walk3d::BuildingSpec, system: &str) -> Option<f64> {
    let sy = sp.mep.systems.iter().find(|x| x.name.eq_ignore_ascii_case(system))?;
    let kind = match sy.kind {
        cadaddy_walk3d::MepKind::Duct => "duct",
        cadaddy_walk3d::MepKind::Pipe => "pipe",
        cadaddy_walk3d::MepKind::Conduit => "conduit",
        cadaddy_walk3d::MepKind::Tray => "tray",
        cadaddy_walk3d::MepKind::Cable => "conduit",
    };
    let trade = match kind {
        "duct" => Trade::Mechanical,
        "pipe" if is_fire(system) => Trade::FireProtection,
        "pipe" => Trade::Plumbing,
        _ => Trade::Electrical,
    };
    let mut props = BTreeMap::new();
    if kind == "pipe" {
        props.insert("service".to_string(), if is_gravity(system) { "gravity" } else { "pressure" }.to_string());
    }
    let e = ModelElem {
        id: String::new(),
        key: String::new(),
        trade,
        category: kind.into(),
        system: system.into(),
        level: String::new(),
        props,
        measures: BTreeMap::new(),
    };
    rules.iter().find(|r| r.action.kind == "yield_to" && elem_matches(&r.condition, &e)).and_then(|r| r.action.numeric).filter(|n| n.is_finite())
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * t)
}

/// The run of `system` on `level` nearest the clash point.
fn nearest_run(sp: &cadaddy_walk3d::BuildingSpec, system: &str, level: &str, at: Vec2) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (i, r) in sp.mep.runs.iter().enumerate() {
        if !r.system.eq_ignore_ascii_case(system) || !r.level.eq_ignore_ascii_case(level) || r.top_ft.is_some() {
            continue;
        }
        let pts: Vec<Vec2> = r.points.iter().map(|p| Vec2::new(p[0], p[1])).filter(|p| p.is_finite()).collect();
        let d = match pts.len() {
            0 => continue,
            1 => pts.first().map_or(f64::INFINITY, |p| p.dist(at)),
            _ => pts.windows(2).filter_map(|w| Some(seg_dist(at, *w.first()?, *w.get(1)?))).fold(f64::INFINITY, f64::min),
        };
        if d.is_finite() && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

fn run_height_ft(r: &cadaddy_walk3d::MepRun) -> f64 {
    let h = if r.size_in[1] > 0.0 { r.size_in[1] } else { r.size_in[0] };
    if h.is_finite() && h > 0.0 { h / 12.0 } else { 0.0 }
}

fn run_bottom_ft(r: &cadaddy_walk3d::MepRun) -> Option<f64> {
    r.bottoms_ft.iter().copied().filter(|b| b.is_finite()).reduce(f64::min)
}

/// The fix the coordination rules make for one clash: the run to move and its new bottom (feet
/// above its floor). None when the rules can't decide (equal bands, columns, walls, attached
/// models) or no height fits.
fn plan_fix(sp: &cadaddy_walk3d::BuildingSpec, rules: &[&RuleDef], c: &Value, moved: &BTreeSet<usize>) -> Option<(usize, f64, String)> {
    let kind = c.get("kind").and_then(Value::as_str)?;
    let level_name = c.get("level").and_then(Value::as_str)?;
    let level = sp.level(level_name)?;
    let at =
        c.get("at").and_then(Value::as_array).and_then(|a| Some(Vec2::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?))).filter(|p| p.is_finite())?;
    let systems: Vec<&str> = c.get("systems").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let by_ft = c.get("by_in").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0) / 12.0;
    let ceiling = level.ceiling_ft;
    let structure = level.height_ft - SLAB_FT;
    let fits = |bottom: f64, h: f64| bottom >= ceiling + CLEAR_CEILING_FT - 1e-9 && bottom + h <= structure - CLEAR_STRUCTURE_FT + 1e-9;
    match kind {
        "crossing" => {
            let (a, b) = (*systems.first()?, *systems.get(1)?);
            let (ba, bb) = (system_band(rules, sp, a)?, system_band(rules, sp, b)?);
            if (ba - bb).abs() < 1e-9 {
                return None;
            }
            let (yields, keeps) = if ba > bb { (a, b) } else { (b, a) };
            let yi = nearest_run(sp, yields, level_name, at)?;
            let ki = nearest_run(sp, keeps, level_name, at)?;
            if moved.contains(&yi) {
                return None;
            }
            let (y, k) = (sp.mep.runs.get(yi)?, sp.mep.runs.get(ki)?);
            let (yh, kb, kh) = (run_height_ft(y), run_bottom_ft(k)?, run_height_ft(k));
            let below = kb - CLEAR_RUN_FT - yh;
            let above = kb + kh + CLEAR_RUN_FT;
            let why = |to: &str| format!("{yields} yields to {keeps} (yield order): moved {to} it");
            if fits(below, yh) {
                Some((yi, below, why("below")))
            } else if fits(above, yh) {
                Some((yi, above, why("above")))
            } else {
                None
            }
        }
        "below_ceiling" => {
            let i = nearest_run(sp, systems.first()?, level_name, at)?;
            let r = sp.mep.runs.get(i)?;
            let new = run_bottom_ft(r)? + by_ft + CLEAR_CEILING_FT;
            (!moved.contains(&i) && new + run_height_ft(r) <= structure - CLEAR_STRUCTURE_FT + 1e-9)
                .then(|| (i, new, format!("{} raised above the ceiling (clearance to ceiling)", r.system)))
        }
        "into_structure" => {
            let i = nearest_run(sp, systems.first()?, level_name, at)?;
            let r = sp.mep.runs.get(i)?;
            let top = r.bottoms_ft.iter().copied().filter(|b| b.is_finite()).fold(f64::NEG_INFINITY, f64::max);
            let new = top - by_ft - CLEAR_STRUCTURE_FT;
            (!moved.contains(&i) && new.is_finite() && new >= ceiling + CLEAR_CEILING_FT - 1e-9)
                .then(|| (i, new, format!("{} lowered below the structure (clearance to structure)", r.system)))
        }
        _ => None,
    }
}

/// A clash nothing in the rules resolves, as a finding: a judgement call for the agent.
fn clash_finding(c: &Value) -> Finding {
    let kind = c.get("kind").and_then(Value::as_str).unwrap_or("clash");
    let level = c.get("level").and_then(Value::as_str).unwrap_or("").to_string();
    let message = c.get("message").and_then(Value::as_str).unwrap_or("clash").to_string();
    let at = c.get("at").cloned().unwrap_or(Value::Null);
    let systems: Vec<String> =
        c.get("systems").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    let trade = if kind == "walls_overlap" { Trade::Architectural } else { Trade::Coordination };
    let key = super::mep::clash_match_key(c);
    let mut param = BTreeMap::new();
    param.insert("clash".to_string(), json!(key));
    let (title, alternatives) = if kind == "walls_overlap" {
        (
            format!("Walls overlap{}: {message}", if level.is_empty() { String::new() } else { format!(" on {level}") }),
            vec![option("Delete the duplicate wall", "delete"), option("Keep both: accept", "accept")],
        )
    } else {
        (
            format!("Judgement call{}: {message}", if level.is_empty() { String::new() } else { format!(" on {level}") }),
            vec![
                ActionOption { label: "Drop one run below the other".into(), action: "change_elevation".into(), params: param.clone() },
                ActionOption { label: "Reroute around".into(), action: "reroute".into(), params: param.clone() },
                ActionOption { label: "Resize to make room".into(), action: "resize".into(), params: param.clone() },
                ActionOption { label: "Accept the clash (field coordination)".into(), action: "accept".into(), params: param },
            ],
        )
    };
    Finding {
        id: fnv(&format!("clash|{key}|{at}")),
        kind: if kind == "walls_overlap" { "clash" } else { "judgement" }.into(),
        trade,
        level,
        severity: "high".into(),
        title,
        detail: format!(
            "{kind} clash between {}: the yield and clearance rules can't settle it (equal bands, a column, a wall or no height that fits).",
            systems.join(" and ")
        ),
        elements: c.get("handles").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).take(20).collect(),
        proposed: if kind == "walls_overlap" { "Delete the duplicate".into() } else { "Drop the smaller run below the other".into() },
        alternatives,
        status: "open".into(),
        resolution: None,
    }
}

/// Clash match keys project rules allow (an accepted clash saved as a rule).
fn allowed_clashes(st: &RuleModelState) -> BTreeSet<String> {
    st.project_rules.iter().filter(|r| r.action.kind == "allow_clash").map(|r| r.action.value.clone()).collect()
}

fn clashes_now(s: &mut Session, allowed: &BTreeSet<String>) -> Result<Vec<Value>> {
    let r = s.execute("model.clashes", &json!({}))?;
    Ok(r.get("clashes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| !allowed.contains(&super::mep::clash_match_key(c)))
        .cloned()
        .collect())
}

// ── agent actions ─────────────────────────────────────────────────────────────

/// The actions an agent can perform on the model.
pub fn available_actions() -> Vec<(&'static str, &'static str)> {
    vec![
        ("select", "Select/query elements by trade, level, category, rule or confidence"),
        ("move", "Move model objects by dx, dy, dz feet"),
        ("resize", "Change a run's size (size_in)"),
        ("reroute", "Give a run a new path (points)"),
        ("change_elevation", "Move a run to a new bottom height (bottom_ft) or objects by dz"),
        ("split", "Split a run at a vertex"),
        ("join", "Join two runs of one system end to end"),
        ("swap_type", "Set the type of model objects (a wall type, a family)"),
        ("add", "Add a run (system, level, points, size_in, bottom_ft)"),
        ("delete", "Delete model objects or a run"),
        ("assign_circuit", "Assign model objects to a circuit"),
        ("rerun_rules", "Re-evaluate the rules (all, a level or a trade)"),
        ("teach_symbol", "Teach a symbol (not available yet: use the symbol commands)"),
        ("write_rule", "Save a decision as a project rule"),
        ("accept", "Accept a finding or elements (with save_as_rule: keep it as a project rule)"),
        ("reject", "Reject elements in an audit"),
    ]
}

/// Most objects one action touches, and most points of a run.
const ACTION_MAX: usize = 10_000;
const RUN_POINTS_MAX: usize = 10_000;

fn handles_of(p: &Value, target: &str) -> Vec<Handle> {
    let mut out: Vec<Handle> = target.split(',').filter_map(|t| Handle::parse_hex(t.trim())).collect();
    for v in p.get("ids").and_then(Value::as_array).into_iter().flatten().take(ACTION_MAX) {
        if let Some(h) = v.as_str().and_then(Handle::parse_hex) {
            out.push(h);
        }
    }
    out.truncate(ACTION_MAX);
    out
}

fn num(p: &Value, key: &str) -> Option<f64> {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

/// Change the building spec and build the model again.
fn edit_spec(s: &mut Session, cmd: &str, f: impl FnOnce(&mut cadaddy_walk3d::BuildingSpec) -> Result<Value>) -> Result<Value> {
    let (mut sp, stored) = super::bspec::current(s, cmd)?;
    if !stored {
        return Err(bad(cmd, "there is no building spec yet: call bspec.draft {} or printset.open {path} first"));
    }
    let out = f(&mut sp)?;
    s.doc_mut()?.building_spec = Some(sp.to_json());
    s.execute("model.build", &json!({}))?;
    Ok(out)
}

fn run_index(sp: &cadaddy_walk3d::BuildingSpec, cmd: &str, id: &str) -> Result<usize> {
    sp.mep
        .runs
        .iter()
        .position(|r| r.id == id)
        .ok_or_else(|| bad(cmd, format!("no run with id `{}` (mep.list lists them)", id.chars().take(40).collect::<String>())))
}

fn points_of(p: &Value, cmd: &str) -> Result<Vec<[f64; 2]>> {
    let pts: Vec<[f64; 2]> = p
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(cmd, "`points` is required: [[x, y], …] in drawing units"))?
        .iter()
        .take(RUN_POINTS_MAX + 1)
        .filter_map(|q| Some([q.get(0)?.as_f64()?, q.get(1)?.as_f64()?]))
        .filter(|q| q[0].is_finite() && q[1].is_finite())
        .collect();
    if pts.len() < 2 || pts.len() > RUN_POINTS_MAX {
        return Err(bad(cmd, format!("`points` needs 2 to {RUN_POINTS_MAX} finite points")));
    }
    Ok(pts)
}

/// Set a property on model objects (a `key:value` tag now, and an override by element key so it
/// applies again after a rebuild). Returns how many it set.
fn set_prop(s: &mut Session, st: &mut RuleModelState, handles: &[Handle], key: &str, value: &str) -> Result<usize> {
    let value: String = value.chars().take(240).collect();
    let (_, elems) = live_model(s, &BTreeMap::new())?;
    let wanted: BTreeSet<String> = handles.iter().map(|h| h.hex()).collect();
    let keys: Vec<(Handle, String)> =
        elems.iter().filter(|e| wanted.contains(&e.id)).filter_map(|e| Handle::parse_hex(&e.id).map(|h| (h, e.key.clone()))).collect();
    let tag = format!("{key}:{value}");
    let prefix = format!("{key}:");
    let d = s.doc_mut()?;
    let mut n = 0;
    for (h, k) in keys {
        if st.overrides.len() >= MAX_ELEMENTS && !st.overrides.contains_key(&k) {
            break;
        }
        let _ = d.modify_entity(h, |e| {
            e.common.xdata.retain(|t| !(t.code == 1000 && t.value.starts_with(&prefix)));
            e.common.xdata.push(RawTag { code: 1000, value: tag.clone() });
        });
        st.overrides.entry(k).or_default().insert(key.to_string(), value.clone());
        n += 1;
    }
    Ok(n)
}

/// The keys of model objects by handle (an accept or reject outlives a rebuild).
fn keys_of(s: &Session, ids: &[String]) -> Result<Vec<String>> {
    let (_, elems) = live_model(s, &BTreeMap::new())?;
    let wanted: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    Ok(elems.into_iter().filter(|e| wanted.contains(e.id.as_str())).map(|e| e.key).collect())
}

/// Apply an agent action to the model. Returns what it changed.
fn apply_action(s: &mut Session, st: &mut RuleModelState, action: &str, target: &str, p: &Value, finding: Option<&Finding>) -> Result<Value> {
    const CMD: &str = "rules.action";
    match action {
        "select" => {
            let (_, r) = evaluate_live(s, st, str_param(p, "level"), str_param(p, "trade").and_then(Trade::parse))?;
            let category = str_param(p, "category");
            let rule_id = str_param(p, "rule");
            let below = num(p, "below");
            let ids: BTreeSet<&str> =
                p.get("ids").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).take(ACTION_MAX).collect();
            let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(PAGE_DEFAULT as u64).clamp(1, PAGE_MAX as u64) as usize;
            let hits: Vec<&PlacedElement> = r
                .elements
                .iter()
                .filter(|e| {
                    category.is_none_or(|c| e.category.eq_ignore_ascii_case(c))
                        && rule_id.is_none_or(|x| e.rule_id == x)
                        && below.is_none_or(|b| e.confidence < b)
                        && (ids.is_empty() || ids.contains(e.id.as_str()))
                        && (target.is_empty() || target.split(',').any(|t| t.trim().eq_ignore_ascii_case(&e.id)))
                })
                .collect();
            Ok(json!({"total": hits.len(), "items": hits.iter().take(limit).collect::<Vec<_>>()}))
        }
        "move" | "change_elevation"
            if !target.is_empty() && Handle::parse_hex(target.split(',').next().unwrap_or("").trim()).is_some() && num(p, "bottom_ft").is_none() =>
        {
            let (sp, _) = super::bspec::current(s, CMD)?;
            let upf = units_per_foot(&sp);
            let (dx, dy, dz) = (num(p, "dx").unwrap_or(0.0), num(p, "dy").unwrap_or(0.0), num(p, "dz").unwrap_or(0.0));
            if action == "change_elevation" && (dx != 0.0 || dy != 0.0) {
                return Err(bad(CMD, "change_elevation moves up or down only: give dz (feet), or bottom_ft with a run id"));
            }
            if [dx, dy, dz].iter().any(|v| v.abs() > 10_000.0) {
                return Err(bad(CMD, "a move is at most 10,000 ft"));
            }
            let m = cadaddy_geom::Mat4 { m: [[1.0, 0.0, 0.0, dx * upf], [0.0, 1.0, 0.0, dy * upf], [0.0, 0.0, 1.0, dz * upf], [0.0, 0.0, 0.0, 1.0]] };
            let handles = handles_of(p, target);
            let d = s.doc_mut()?;
            let mut moved = 0;
            for h in &handles {
                if d.modify_entity(*h, |e| {
                    let _ = e.kind.transform3(&m);
                })
                .is_ok()
                {
                    moved += 1;
                }
            }
            Ok(json!({"moved": moved, "by_ft": [dx, dy, dz]}))
        }
        "change_elevation" | "move" => {
            let bottom = num(p, "bottom_ft")
                .ok_or_else(|| bad(CMD, "give a run id as target and bottom_ft (feet above its floor), or object handles and dz"))?;
            if !(-100.0..=1_000.0).contains(&bottom) {
                return Err(bad(CMD, "bottom_ft must be from -100 to 1,000 ft"));
            }
            edit_spec(s, CMD, |sp| {
                let i = run_index(sp, CMD, target)?;
                let r = sp.mep.runs.get_mut(i).ok_or_else(|| bad(CMD, "the run is gone"))?;
                r.bottoms_ft.fill(bottom);
                r.height_source = "override".into();
                r.edited_by = "agent".into();
                Ok(json!({"run": target, "bottom_ft": bottom}))
            })
        }
        "resize" => {
            let size = p
                .get("size_in")
                .and_then(Value::as_array)
                .ok_or_else(|| bad(CMD, "`size_in` is required: [width, height] inches (height 0 for round)"))?;
            let w = size.first().and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0 && *v <= 240.0);
            let h = size.get(1).and_then(Value::as_f64).unwrap_or(0.0);
            let (Some(w), true) = (w, h.is_finite() && (0.0..=240.0).contains(&h)) else {
                return Err(bad(CMD, "`size_in` sizes are 0 to 240 inches"));
            };
            edit_spec(s, CMD, |sp| {
                let i = run_index(sp, CMD, target)?;
                let r = sp.mep.runs.get_mut(i).ok_or_else(|| bad(CMD, "the run is gone"))?;
                r.size_in = [w, h];
                r.size_source = "override".into();
                r.edited_by = "agent".into();
                Ok(json!({"run": target, "size_in": [w, h]}))
            })
        }
        "reroute" => {
            let pts = points_of(p, CMD)?;
            edit_spec(s, CMD, |sp| {
                let i = run_index(sp, CMD, target)?;
                let r = sp.mep.runs.get_mut(i).ok_or_else(|| bad(CMD, "the run is gone"))?;
                let b = run_bottom_ft(r).unwrap_or(0.0);
                r.bottoms_ft = vec![b; pts.len().saturating_sub(1).max(1)];
                r.points = pts;
                r.edited_by = "agent".into();
                Ok(json!({"run": target, "points": r.points.len()}))
            })
        }
        "split" => {
            let at = p.get("at").and_then(Value::as_u64).ok_or_else(|| bad(CMD, "`at` is required: the vertex index to split at"))? as usize;
            edit_spec(s, CMD, |sp| {
                let i = run_index(sp, CMD, target)?;
                let r = sp.mep.runs.get(i).ok_or_else(|| bad(CMD, "the run is gone"))?;
                if at == 0 || at + 1 >= r.points.len() {
                    return Err(bad(CMD, format!("`at` must be an inner vertex: 1 to {}", r.points.len().saturating_sub(2))));
                }
                let mut second = r.clone();
                let mut first = r.clone();
                first.points.truncate(at + 1);
                first.bottoms_ft.truncate(at.max(1));
                second.points = r.points.iter().skip(at).copied().collect();
                second.bottoms_ft = r.bottoms_ft.iter().skip(at).copied().collect();
                if second.bottoms_ft.is_empty() {
                    second.bottoms_ft = vec![run_bottom_ft(r).unwrap_or(0.0)];
                }
                let n = sp.mep.runs.len() + 1;
                let mut k = n;
                while sp.mep.runs.iter().any(|x| x.id == format!("R{k}")) && k < n + 100_000 {
                    k += 1;
                }
                second.id = format!("R{k}");
                second.edited_by = "agent".into();
                first.edited_by = "agent".into();
                let new_id = second.id.clone();
                if let Some(slot) = sp.mep.runs.get_mut(i) {
                    *slot = first;
                }
                sp.mep.runs.push(second);
                Ok(json!({"run": target, "new_run": new_id}))
            })
        }
        "join" => {
            let other = str_param(p, "with").ok_or_else(|| bad(CMD, "`with` is required: the run id to join"))?.to_string();
            edit_spec(s, CMD, |sp| {
                let (i, j) = (run_index(sp, CMD, target)?, run_index(sp, CMD, &other)?);
                let (a, b) =
                    (sp.mep.runs.get(i).ok_or_else(|| bad(CMD, "the run is gone"))?, sp.mep.runs.get(j).ok_or_else(|| bad(CMD, "the run is gone"))?);
                if i == j || a.system != b.system || a.level != b.level {
                    return Err(bad(CMD, "join needs two different runs of one system on one level"));
                }
                let (Some(ae), Some(bs)) = (a.points.last(), b.points.first()) else { return Err(bad(CMD, "a run has no points")) };
                let tol = units_per_foot(sp) / 12.0;
                if (ae[0] - bs[0]).hypot(ae[1] - bs[1]) > tol {
                    return Err(bad(CMD, "the first run's end must meet the second run's start (within 1 in)"));
                }
                let mut joined = a.clone();
                joined.points.extend(b.points.iter().skip(1).copied());
                joined.bottoms_ft.extend(b.bottoms_ft.iter().copied());
                joined.objects.extend(b.objects.iter().cloned());
                joined.edited_by = "agent".into();
                if joined.points.len() > RUN_POINTS_MAX {
                    return Err(bad(CMD, "the joined run would be too long"));
                }
                if let Some(slot) = sp.mep.runs.get_mut(i) {
                    *slot = joined;
                }
                sp.mep.runs.remove(j);
                Ok(json!({"run": target, "joined": other}))
            })
        }
        "add" => {
            let system = str_param(p, "system").ok_or_else(|| bad(CMD, "`system` is required"))?.to_string();
            let level = str_param(p, "level").ok_or_else(|| bad(CMD, "`level` is required"))?.to_string();
            let pts = points_of(p, CMD)?;
            edit_spec(s, CMD, |sp| {
                let sy =
                    sp.mep.systems.iter().find(|x| x.name.eq_ignore_ascii_case(&system)).ok_or_else(|| bad(CMD, format!("no system `{system}`")))?;
                if sp.level(&level).is_none() {
                    return Err(bad(CMD, format!("no level `{level}`")));
                }
                let size = p.get("size_in").and_then(Value::as_array).map_or(sy.size_in, |a| {
                    [a.first().and_then(Value::as_f64).unwrap_or(sy.size_in[0]), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)]
                });
                if !size.iter().all(|v| v.is_finite() && (0.0..=240.0).contains(v)) || size[0] <= 0.0 {
                    return Err(bad(CMD, "`size_in` sizes are 0 to 240 inches"));
                }
                let bottom = num(p, "bottom_ft").unwrap_or(sy.bottom_ft).clamp(-100.0, 1_000.0);
                let mut k = sp.mep.runs.len() + 1;
                while sp.mep.runs.iter().any(|x| x.id == format!("R{k}")) && k < sp.mep.runs.len() + 100_000 {
                    k += 1;
                }
                let run = cadaddy_walk3d::MepRun {
                    system: sy.name.clone(),
                    level: level.clone(),
                    bottoms_ft: vec![bottom; pts.len().saturating_sub(1).max(1)],
                    points: pts,
                    size_in: size,
                    id: format!("R{k}"),
                    size_source: "override".into(),
                    height_source: "override".into(),
                    edited_by: "agent".into(),
                    objects: Vec::new(),
                    encased: false,
                    reviewed: false,
                    top_ft: None,
                    source_sheet: String::new(),
                };
                let id = run.id.clone();
                sp.mep.runs.push(run);
                Ok(json!({"run": id}))
            })
        }
        "delete" => {
            let handles = handles_of(p, target);
            if handles.is_empty() {
                // A run id: take it out of the spec.
                return edit_spec(s, CMD, |sp| {
                    let i = run_index(sp, CMD, target)?;
                    sp.mep.runs.remove(i);
                    Ok(json!({"run": target, "deleted": true}))
                });
            }
            let d = s.doc_mut()?;
            let mut n = 0;
            for h in handles {
                if d.entity(h).is_some_and(|e| super::wallgen::made_as(e).is_some()) && d.remove_entity(h).is_some() {
                    n += 1;
                }
            }
            Ok(json!({"deleted": n}))
        }
        "swap_type" | "assign_circuit" => {
            let (key, param) = if action == "swap_type" { ("type", "type") } else { ("circuit", "circuit") };
            let value =
                str_param(p, param).map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| bad(CMD, format!("`{param}` is required")))?.to_string();
            let mut handles = handles_of(p, target);
            if handles.is_empty()
                && let Some(f) = finding
            {
                handles = f.elements.iter().filter_map(|e| Handle::parse_hex(e)).take(ACTION_MAX).collect();
            }
            let n = set_prop(s, st, &handles, key, &value)?;
            Ok(json!({"set": n, key: value}))
        }
        "rerun_rules" => {
            let r = reevaluate(s, st, str_param(p, "level"), str_param(p, "trade").and_then(Trade::parse))?;
            Ok(json!({"elements": r.elements.len(), "findings": r.findings.len()}))
        }
        "teach_symbol" => {
            Err(bad(CMD, "teach_symbol is not available yet: use the symbol and device commands (mep.read_devices, blocks) and build again"))
        }
        "write_rule" => {
            let raw = p.get("rule").ok_or_else(|| bad(CMD, "`rule` is required: {id, description?, condition, action, source?, confidence?}"))?;
            if raw.to_string().len() > 16_000 {
                return Err(bad(CMD, "the rule is too large"));
            }
            let mut r: RuleDef = serde_json::from_value(json!({"priority": "project", "confidence": 1.0}).as_object().map_or(raw.clone(), |d| {
                let mut v = d.clone();
                if let Some(o) = raw.as_object() {
                    v.extend(o.clone());
                }
                Value::Object(v)
            }))
            .map_err(|e| bad(CMD, format!("the rule doesn't read: {e}")))?;
            r.priority = RulePriority::Project;
            r.confidence = if r.confidence.is_finite() { r.confidence.clamp(0.0, 1.0) } else { 1.0 };
            if r.id.trim().is_empty() || r.id.len() > 120 {
                return Err(bad(CMD, "the rule's `id` must be 1 to 120 characters"));
            }
            let id = r.id.clone();
            save_project_rule(st, r)?;
            Ok(json!({"rule": id, "projectRules": st.project_rules.len()}))
        }
        "accept" | "reject" => {
            let mut ids: Vec<String> = handles_of(p, target).iter().map(|h| h.hex()).collect();
            if ids.is_empty()
                && let Some(f) = finding
            {
                ids = if is_clash(f) { Vec::new() } else { f.elements.clone() };
            }
            let ids = keys_of(s, &ids)?;
            let accept = action == "accept";
            let (list, other) = if accept { (&mut st.accepted, &mut st.rejected) } else { (&mut st.rejected, &mut st.accepted) };
            let mut have: BTreeSet<String> = list.iter().cloned().collect();
            let mut n = 0;
            for id in ids {
                if list.len() >= MAX_ELEMENTS {
                    break;
                }
                if have.insert(id.clone()) {
                    list.push(id);
                    n += 1;
                }
            }
            other.retain(|x| !have.contains(x));
            let key = if accept { "accepted" } else { "rejected" };
            Ok(json!({ key: n }))
        }
        _ => Err(bad(
            CMD,
            format!(
                "unknown action `{}`; the actions are {}",
                action.chars().take(40).collect::<String>(),
                available_actions().iter().map(|(a, _)| *a).collect::<Vec<_>>().join(", ")
            ),
        )),
    }
}

/// Keep a project rule (one id replaces the earlier rule of that id).
fn save_project_rule(st: &mut RuleModelState, r: RuleDef) -> Result<()> {
    if let Some(slot) = st.project_rules.iter_mut().find(|x| x.id == r.id) {
        *slot = r;
        return Ok(());
    }
    if st.project_rules.len() >= MAX_PROJECT_RULES {
        return Err(bad("rules.action", format!("a drawing keeps at most {MAX_PROJECT_RULES} project rules")));
    }
    st.project_rules.push(r);
    Ok(())
}

/// Most project rules a drawing keeps.
const MAX_PROJECT_RULES: usize = 10_000;

/// The project rule an answer to a finding becomes, so the same question isn't asked again.
fn rule_from_answer(f: &Finding, action: &str, packs: &[RulePack]) -> Option<RuleDef> {
    if is_clash(f) {
        let key = f.alternatives.iter().find_map(|a| a.params.get("clash")).and_then(Value::as_str).map(str::to_string)?;
        return Some(RuleDef {
            id: format!("project-allow-{}", f.id),
            description: format!("Accepted clash: {}", f.title),
            priority: RulePriority::Project,
            condition: RuleCondition::default(),
            action: RuleAction { kind: "allow_clash".into(), value: key, ..Default::default() },
            source: "Agent answer".into(),
            confidence: 1.0,
        });
    }
    if action != "accept" {
        return None;
    }
    // The rule the finding is about, from its detail ("Rule <id> (<kind>)…").
    let rule_id = f.detail.strip_prefix("Rule ").and_then(|t| t.split_whitespace().next())?.to_string();
    let original = packs.iter().flat_map(|p| p.rules.iter()).find(|r| r.id == rule_id);
    let mut condition = original.map(|r| r.condition.clone()).unwrap_or_default();
    if !f.level.is_empty() {
        condition.properties.insert("level".into(), f.level.clone());
    }
    let action = if f.kind == "low_confidence" {
        RuleAction { kind: "classify".into(), ..Default::default() }
    } else {
        RuleAction { kind: "waive".into(), value: rule_id.clone(), ..Default::default() }
    };
    Some(RuleDef {
        id: format!("project-{}", f.id),
        description: format!("Answered: {}", f.title.chars().take(200).collect::<String>()),
        priority: RulePriority::Project,
        condition,
        action,
        source: "Agent answer".into(),
        confidence: 0.95,
    })
}

// ── commands ──────────────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rules.packs", "Rule Packs", run_packs).params(
            "{pack?: {id, label, trade, version?, rules: [{id, description?, priority: project|spec|print_fact|code|default, condition: {category?, system?, trade?, material?, location?, sizeRange?, properties?}, action: {kind: classify|check|require|waive|set_property|set_size|set_height|set_spacing|route|place|yield_to, key?, value?, numeric?, max?}, source?, confidence?}]} (add one, or replace the built-in pack of that id), remove?: pack id} → {loaded: [{id, label, trade, ruleCount, edited}], projectRules}: the rule packs in force",
        ),
        CommandSpec::new("rules.evaluate", "Evaluate Rules", run_evaluate).params(
            "{level?, trade?, threshold?: 0-1 (0.7)} → {elements, findings, open, byKind, summary, score, threshold, build, top: [finding]}: evaluate the rules against every element of the built model",
        ),
        CommandSpec::new("rules.worklist", "Rules Worklist", run_worklist)
            .params("{trade?, kind?, level?, status?: open|resolved|deferred|all (open), severity?, cursor?, limit?: 1-100 (20)} → {total, items, next_cursor?}: the findings, most important first")
            .noundo(),
        CommandSpec::new("rules.action", "Rules Action", run_action).params(
            "{action, target?: handle(s) or run id, params?: {…}, finding?: id (the worklist finding this answers), citation?: text (drawing, spec page, code section), save_as_rule?: bool (keep the answer as a project rule), defer?: bool} → {delta: {action result, before, after}}: select, move {dx,dy,dz ft}, resize {size_in}, reroute {points}, change_elevation {bottom_ft | dz}, split {at}, join {with}, swap_type {type}, add {system, level, points, size_in?, bottom_ft?}, delete, assign_circuit {circuit}, rerun_rules, teach_symbol, write_rule {rule}, accept, reject",
        ),
        CommandSpec::new("rules.clashloop", "Clash Resolution Loop", run_clashloop).params(
            "{max_iterations?: 1-10 (3)} → {iterations, before, auto_resolved, remaining, moves, findings}: model.clashes → move what the yield and clearance rules decide → build → check again; what's left becomes judgement findings",
        ),
        CommandSpec::new("rules.score", "Accuracy Score", run_score)
            .params("{trade?, sample?: per category (20)} → {overall, byTrade, sampleSize, accepted, rejected, audit: {elements, lowConfidence}, history}: the share of elements placed at or above the threshold, and an audit sample")
            .noundo(),
        CommandSpec::new("rules.report", "Rules Report", run_report)
            .params("{trade?} → {elements, score, lines: [{category, trade, count, quantity, pctRulePlaced, pctAgentDecided, openAssumptions, score}], openFindings}: per-category summary of the live model")
            .noundo(),
    ]
}

fn pack_json(p: &RulePack, edited: bool) -> Value {
    json!({"id": p.id, "label": p.label, "trade": p.trade, "version": p.version, "ruleCount": p.rules.len(), "edited": edited})
}

fn run_packs(s: &mut Session, p: &Value) -> Result<Value> {
    let mut st = load_state(s.doc()?);
    let mut changed = false;
    if let Some(raw) = p.get("pack").filter(|v| !v.is_null()) {
        if raw.to_string().len() > 1_000_000 {
            return Err(bad("rules.packs", "the pack is too large (1 MB at most)"));
        }
        let pack: RulePack = serde_json::from_value(raw.clone()).map_err(|e| bad("rules.packs", format!("the pack doesn't read: {e}")))?;
        if pack.id.trim().is_empty() || pack.rules.is_empty() || pack.rules.len() > 10_000 {
            return Err(bad("rules.packs", "a pack needs an id and 1 to 10,000 rules"));
        }
        if let Some(r) = pack.rules.iter().find(|r| r.id.trim().is_empty() || !r.confidence.is_finite() || !(0.0..=1.0).contains(&r.confidence)) {
            return Err(bad("rules.packs", format!("rule `{}` needs an id and a confidence from 0 to 1", r.id)));
        }
        st.packs.retain(|x| x.id != pack.id);
        st.packs.push(pack);
        changed = true;
    }
    if let Some(id) = str_param(p, "remove") {
        let n = st.packs.len();
        st.packs.retain(|x| x.id != id);
        if st.packs.len() == n {
            return Err(bad("rules.packs", format!("no edited pack `{id}` (a built-in pack can be replaced, not removed)")));
        }
        changed = true;
    }
    if changed {
        save_state(s, &st)?;
    }
    let loaded: Vec<Value> = effective_packs(&st).iter().map(|x| pack_json(x, st.packs.iter().any(|e| e.id == x.id))).collect();
    Ok(json!({"loaded": loaded, "projectRules": st.project_rules.len()}))
}

fn by_kind(findings: &[Finding]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for f in findings.iter().filter(|f| f.status == "open") {
        *m.entry(f.kind.clone()).or_insert(0) += 1;
    }
    m
}

fn ranked(findings: &[Finding]) -> Vec<&Finding> {
    let mut v: Vec<&Finding> = findings.iter().collect();
    v.sort_by(|a, b| severity_rank(&a.severity).cmp(&severity_rank(&b.severity)).then(b.elements.len().cmp(&a.elements.len())).then(a.id.cmp(&b.id)));
    v
}

fn run_evaluate(s: &mut Session, p: &Value) -> Result<Value> {
    let mut st = load_state(s.doc()?);
    if p.get("threshold").is_some() {
        st.threshold = f64_or(p, "threshold", DEFAULT_THRESHOLD).clamp(0.0, 1.0);
    }
    if !st.threshold.is_finite() || st.threshold <= 0.0 {
        st.threshold = DEFAULT_THRESHOLD;
    }
    let level = str_param(p, "level");
    let trade = str_param(p, "trade").and_then(Trade::parse);
    let result = reevaluate(s, &mut st, level, trade)?;
    st.build_count += 1;
    let score = compute_score(&result.elements, st.threshold);
    if st.score_history.len() >= 1000 {
        st.score_history.remove(0);
    }
    st.score_history.push(ScoreEntry {
        build: st.build_count,
        overall: score.overall,
        by_trade: score.by_trade.iter().map(|(k, v)| (k.clone(), v.score)).collect(),
    });
    save_state(s, &st)?;
    // With the answers kept: what is still open in the evaluated scope.
    let in_scope = |f: &&Finding| level.is_none_or(|l| f.level.eq_ignore_ascii_case(l)) && trade.is_none_or(|t| f.trade == t);
    let scoped: Vec<Finding> = st.findings.iter().filter(in_scope).cloned().collect();
    let open: Vec<&Finding> = ranked(&scoped).into_iter().filter(|f| f.status == "open").collect();
    Ok(json!({
        "elements": result.elements.len(),
        "findings": result.findings.len(),
        "open": open.len(),
        "byKind": by_kind(&scoped),
        "summary": summarize(&result.elements, &scoped),
        "threshold": st.threshold,
        "build": st.build_count,
        "score": score.overall,
        "top": open.iter().take(5).map(|f| json!({"id": f.id, "kind": f.kind, "severity": f.severity, "title": f.title, "count": f.elements.len()})).collect::<Vec<_>>(),
    }))
}

fn run_worklist(s: &mut Session, p: &Value) -> Result<Value> {
    let st = load_state(s.doc()?);
    let trade_filter = str_param(p, "trade").and_then(Trade::parse);
    let status_filter = str_param(p, "status").unwrap_or("open");
    let severity_filter = str_param(p, "severity");
    let kind_filter = str_param(p, "kind");
    let level_filter = str_param(p, "level");
    let cursor = p.get("cursor").and_then(Value::as_u64).unwrap_or(0).min(MAX_FINDINGS as u64) as usize;
    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(PAGE_DEFAULT as u64).clamp(1, PAGE_MAX as u64) as usize;
    let filtered: Vec<&Finding> = ranked(&st.findings)
        .into_iter()
        .filter(|f| {
            (status_filter == "all" || f.status == status_filter)
                && trade_filter.is_none_or(|t| f.trade == t)
                && severity_filter.is_none_or(|sev| f.severity == sev)
                && kind_filter.is_none_or(|k| f.kind == k)
                && level_filter.is_none_or(|l| f.level.eq_ignore_ascii_case(l))
        })
        .collect();
    let total = filtered.len();
    let page: Vec<Value> = filtered.into_iter().skip(cursor).take(limit).map(finding_json).collect();
    let next_cursor = if cursor + limit < total { Some(cursor + limit) } else { None };
    Ok(json!({"total": total, "items": page, "next_cursor": next_cursor}))
}

/// What the model is: element count, open findings and lengths, for an action's delta.
fn snapshot(s: &Session, st: &RuleModelState) -> Value {
    let open = open_count(st);
    match live_model(s, &st.overrides) {
        Ok((sp, elems)) => {
            let upf = units_per_foot(&sp);
            let wall_ft: f64 = elems.iter().filter(|e| e.category == "wall").filter_map(|e| e.measures.get("length_in")).sum::<f64>() / 12.0;
            json!({"elements": elems.len(), "open": open, "wall_ft": (wall_ft * 10.0).round() / 10.0, "runs": sp.mep.runs.len(), "units_per_foot": upf})
        }
        Err(_) => json!({"open": open}),
    }
}

fn run_action(s: &mut Session, p: &Value) -> Result<Value> {
    let action = str_param(p, "action").ok_or_else(|| bad("rules.action", "`action` is required"))?.to_string();
    let target = str_param(p, "target").unwrap_or("").to_string();
    let params = p.get("params").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
    let mut st = load_state(s.doc()?);
    let finding_id = str_param(p, "finding").map(str::to_string);
    let finding =
        match &finding_id {
            Some(id) => Some(st.findings.iter().find(|f| f.id == *id).cloned().ok_or_else(|| {
                bad("rules.action", format!("no finding `{}` (rules.worklist lists them)", id.chars().take(40).collect::<String>()))
            })?),
            None => None,
        };
    let before = snapshot(s, &st);
    let result = apply_action(s, &mut st, &action, &target, &params, finding.as_ref())?;
    // The answer to a finding: resolved (or deferred), and kept as a project rule if asked.
    if let Some(f) = &finding {
        let save = bool_or(p, "save_as_rule", false);
        if save && let Some(r) = rule_from_answer(f, &action, &effective_packs(&st)) {
            save_project_rule(&mut st, r)?;
        }
        let status = if bool_or(p, "defer", false) { "deferred" } else { "resolved" };
        if let Some(x) = st.findings.iter_mut().find(|x| x.id == f.id) {
            x.status = status.into();
            x.resolution = Some(Resolution {
                action: action.clone(),
                author: "agent".into(),
                citation: str_param(p, "citation").unwrap_or("").chars().take(300).collect(),
                save_as_rule: save,
            });
        }
    }
    // Model actions re-check: the worklist re-ranks after each one.
    let model_changed = !matches!(action.as_str(), "select" | "rerun_rules");
    if model_changed {
        reevaluate(s, &mut st, None, None)?;
    }
    save_state(s, &st)?;
    let after = snapshot(s, &st);
    Ok(json!({"delta": {"action": action, "result": result, "before": before, "after": after}}))
}

fn run_clashloop(s: &mut Session, p: &Value) -> Result<Value> {
    let max_iter = p.get("max_iterations").and_then(Value::as_u64).unwrap_or(3).clamp(1, 10) as usize;
    let mut st = load_state(s.doc()?);
    let packs = effective_packs(&st);
    let mut rules: Vec<&RuleDef> = st.project_rules.iter().chain(packs.iter().flat_map(|p| p.rules.iter())).collect();
    rules.sort_by_key(|r| (r.priority as u8, std::cmp::Reverse(specificity(&r.condition))));
    let rules: Vec<RuleDef> = rules.into_iter().cloned().collect();
    let rule_refs: Vec<&RuleDef> = rules.iter().collect();
    let allowed = allowed_clashes(&st);

    let mut clashes = clashes_now(s, &allowed)?;
    let before = clashes.len();
    let mut iterations = 1;
    let mut auto = 0usize;
    let mut moves: Vec<Value> = Vec::new();
    for _ in 1..max_iter {
        let (mut sp, stored) = super::bspec::current(s, "rules.clashloop")?;
        if !stored {
            break;
        }
        let mut moved: BTreeSet<usize> = BTreeSet::new();
        for c in &clashes {
            if let Some((i, bottom, why)) = plan_fix(&sp, &rule_refs, c, &moved)
                && let Some(r) = sp.mep.runs.get_mut(i)
            {
                r.bottoms_ft.fill(bottom);
                moved.insert(i);
                auto += 1;
                if moves.len() < 50 {
                    moves.push(
                        json!({"run": r.id, "system": r.system, "level": r.level, "bottom_ft": (bottom * 1000.0).round() / 1000.0, "why": why}),
                    );
                }
            }
        }
        if moved.is_empty() {
            break;
        }
        s.doc_mut()?.building_spec = Some(sp.to_json());
        s.execute("model.build", &json!({}))?;
        clashes = clashes_now(s, &allowed)?;
        iterations += 1;
    }

    // What's left: judgement calls for the agent (a clash answered before keeps its answer).
    let mut new: Vec<Finding> = Vec::new();
    for c in clashes.iter().take(MAX_CLASH_FINDINGS) {
        let mut f = clash_finding(c);
        if new.iter().any(|x| x.id == f.id) {
            continue;
        }
        if let Some(prev) = st.findings.iter().find(|x| x.id == f.id && x.status != "open") {
            f.status.clone_from(&prev.status);
            f.resolution.clone_from(&prev.resolution);
        }
        new.push(f);
    }
    let remaining = new.iter().filter(|f| f.status == "open").count();
    let mut findings: Vec<Finding> =
        st.findings.iter().filter(|f| !is_clash(f) || (f.status != "open" && !new.iter().any(|n| n.id == f.id))).cloned().collect();
    findings.extend(new.iter().cloned());
    findings.truncate(MAX_FINDINGS);
    st.findings = findings;
    save_state(s, &st)?;
    Ok(json!({
        "iterations": iterations,
        "before": before,
        "auto_resolved": auto,
        "remaining": remaining,
        "moves": moves,
        "findings": new.iter().filter(|f| f.status == "open").take(10).map(finding_json).collect::<Vec<_>>(),
    }))
}

/// The audit sample: per trade and category the first `n` elements in a stable shuffled order
/// (by a hash of the id), and every low-confidence element (capped).
fn audit_sample(elements: &[PlacedElement], n: usize, threshold: f64) -> (Vec<String>, Vec<String>) {
    let mut by_cat: BTreeMap<(String, String), Vec<(String, &str)>> = BTreeMap::new();
    for e in elements {
        by_cat.entry((e.trade.as_str().into(), e.category.clone())).or_default().push((fnv(&e.id), e.id.as_str()));
    }
    let mut sample = Vec::new();
    for v in by_cat.values_mut() {
        v.sort();
        sample.extend(v.iter().take(n).map(|(_, id)| (*id).to_string()));
    }
    sample.truncate(2_000);
    let low: Vec<String> = elements.iter().filter(|e| e.confidence < threshold).take(500).map(|e| e.id.clone()).collect();
    (sample, low)
}

fn run_score(s: &mut Session, p: &Value) -> Result<Value> {
    let st = load_state(s.doc()?);
    let trade = str_param(p, "trade").and_then(Trade::parse);
    let (_, r) = evaluate_live(s, &st, None, trade)?;
    let threshold = if st.threshold.is_finite() && st.threshold > 0.0 { st.threshold } else { DEFAULT_THRESHOLD };
    let score = compute_score(&r.elements, threshold);
    let n = p.get("sample").and_then(Value::as_u64).unwrap_or(20).clamp(1, 200) as usize;
    let (sample, low) = audit_sample(&r.elements, n, threshold);
    let mut out = serde_json::to_value(&score).map_err(|e| bad("rules.score", e.to_string()))?;
    if let Some(o) = out.as_object_mut() {
        o.insert("audit".into(), json!({"elements": sample.iter().take(200).collect::<Vec<_>>(), "sampleTotal": sample.len(), "lowConfidence": low.iter().take(200).collect::<Vec<_>>()}));
        o.insert("audited".into(), json!({"accepted": st.accepted.len(), "rejected": st.rejected.len()}));
        o.insert("history".into(), json!(st.score_history.iter().rev().take(10).collect::<Vec<_>>()));
    }
    Ok(out)
}

fn run_report(s: &mut Session, p: &Value) -> Result<Value> {
    let st = load_state(s.doc()?);
    let trade = str_param(p, "trade").and_then(Trade::parse);
    let (_, r) = evaluate_live(s, &st, None, trade)?;
    let threshold = if st.threshold.is_finite() && st.threshold > 0.0 { st.threshold } else { DEFAULT_THRESHOLD };
    // Open findings: the live evaluation's, with the agent's answers kept.
    let findings = merge_findings(&st.findings, r.findings, |_| true);
    let lines = generate_report(&r.elements, &findings, threshold);
    let score = compute_score(&r.elements, threshold);
    Ok(json!({
        "elements": r.elements.len(),
        "score": score.overall,
        "openFindings": findings.iter().filter(|f| f.status == "open").count(),
        "lines": lines.iter().take(200).collect::<Vec<_>>(),
    }))
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn elem(id: &str, trade: Trade, category: &str, level: &str, props: &[(&str, &str)], measures: &[(&str, f64)]) -> ModelElem {
        let mut p: BTreeMap<String, String> = props.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
        p.insert("level".into(), level.into());
        ModelElem {
            id: id.into(),
            key: format!("k-{id}"),
            trade,
            category: category.into(),
            system: String::new(),
            level: level.into(),
            props: p,
            measures: measures.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
        }
    }

    fn wall(id: &str, typed: bool, length_in: f64) -> ModelElem {
        let mut props = vec![("source", "print")];
        if typed {
            props.push(("type", "Partition"));
        }
        elem(id, Trade::Architectural, "wall", "L1", &props, &[("length_in", length_in), ("thickness_in", 6.0), ("height_in", 120.0)])
    }

    fn eval(elems: &[ModelElem]) -> EvalResult {
        evaluate_model(&builtin_packs(), &[], elems, DEFAULT_THRESHOLD, &BTreeSet::new(), &BTreeSet::new())
    }

    #[test]
    fn trade_parse() {
        assert_eq!(Trade::parse("electrical"), Some(Trade::Electrical));
        assert_eq!(Trade::parse("FP"), Some(Trade::FireProtection));
        assert_eq!(Trade::parse("arch"), Some(Trade::Architectural));
        assert_eq!(Trade::parse("structure"), Some(Trade::Structural));
        assert_eq!(Trade::parse("bogus"), None);
    }

    #[test]
    fn builtin_packs_valid() {
        let packs = builtin_packs();
        assert_eq!(packs.len(), 7);
        let mut ids = BTreeSet::new();
        for pack in &packs {
            assert!(!pack.rules.is_empty());
            for r in &pack.rules {
                assert!(ids.insert(r.id.clone()), "duplicate rule id {}", r.id);
                assert!((0.0..=1.0).contains(&r.confidence));
            }
        }
        // Every trade has a catch-all classify rule, so no element goes unplaced.
        for t in [Trade::Architectural, Trade::Structural, Trade::Electrical, Trade::Plumbing, Trade::Mechanical, Trade::FireProtection] {
            assert!(
                packs
                    .iter()
                    .flat_map(|p| &p.rules)
                    .any(|r| r.action.kind == "classify" && r.condition.trade == Some(t) && r.condition.category.is_empty())
            );
        }
    }

    #[test]
    fn condition_present_absent_and_values() {
        let props: BTreeMap<String, String> = [("type".to_string(), "Partition".to_string())].into();
        let c = |k: &str, v: &str| RuleCondition { properties: [(k.to_string(), v.to_string())].into(), ..Default::default() };
        assert!(condition_matches(&c("type", "*"), "wall", "", Trade::Architectural, "", "", 0.0, &props));
        assert!(!condition_matches(&c("type", "!"), "wall", "", Trade::Architectural, "", "", 0.0, &props));
        assert!(condition_matches(&c("room", "!"), "wall", "", Trade::Architectural, "", "", 0.0, &props));
        assert!(condition_matches(&c("type", "partition"), "wall", "", Trade::Architectural, "", "", 0.0, &props));
        assert!(!condition_matches(&c("type", "Shaft"), "wall", "", Trade::Architectural, "", "", 0.0, &props));
        let sized = RuleCondition { size_range: Some([2.0, 4.0]), ..Default::default() };
        assert!(condition_matches(&sized, "pipe", "", Trade::Plumbing, "", "", 3.0, &props));
        assert!(!condition_matches(&sized, "pipe", "", Trade::Plumbing, "", "", 5.0, &props));
    }

    #[test]
    fn coordination_rules_leave_architecture_alone() {
        let r = eval(&[wall("A", true, 120.0)]);
        let e = r.elements.first().unwrap();
        assert!(!e.properties.contains_key("yield_order"), "{e:?}");
        let duct = elem("D", Trade::Mechanical, "duct", "L1", &[("basis", "documented")], &[]);
        let r = eval(&[duct]);
        assert_eq!(r.elements.first().unwrap().properties.get("elevation_band").map(String::as_str), Some("2"));
    }

    #[test]
    fn measure_finds_length_thickness_and_heights_at_any_angle() {
        let a = 0.6f64;
        let (c, s) = (a.cos(), a.sin());
        let mut pts = Vec::new();
        for (u, v) in [(0.0, 0.0), (100.0, 0.0), (100.0, 6.0), (0.0, 6.0)] {
            for z in [10.0, 130.0] {
                pts.push(cadaddy_geom::Vec3::new(u * c - v * s, u * s + v * c, z));
            }
        }
        let (len, thick, z0, z1) = measure(&pts).unwrap();
        assert!((len - 100.0).abs() < 1e-6 && (thick - 6.0).abs() < 1e-6, "{len} {thick}");
        assert!((z0 - 10.0).abs() < 1e-9 && (z1 - 130.0).abs() < 1e-9);
        assert!(measure(&[]).is_none());
        assert!(measure(&[cadaddy_geom::Vec3::new(f64::NAN, 0.0, 0.0)]).is_none());
    }

    #[test]
    fn priority_order_decides_and_checks_raise_grouped_findings() {
        let elems = vec![wall("A", true, 120.0), wall("B", false, 120.0), wall("C", true, 2.0), wall("D", true, 1.0)];
        let r = eval(&elems);
        let rule_of = |id: &str| r.elements.iter().find(|e| e.id == id).unwrap().rule_id.clone();
        // A typed wall: the spec tier beats the print tier.
        assert_eq!(rule_of("A"), "arch-wall-typed");
        assert_eq!(rule_of("B"), "arch-wall-traced");
        let fr = |kind: &str| r.findings.iter().filter(|f| f.kind == kind).collect::<Vec<_>>();
        assert_eq!(fr("missing_input").len(), 1);
        let short = fr("check");
        assert_eq!(short.len(), 1, "two fragments, one finding: {short:?}");
        assert_eq!(short.first().unwrap().elements, vec!["C".to_string(), "D".to_string()]);
        assert!(r.elements.iter().find(|e| e.id == "C").unwrap().confidence < DEFAULT_THRESHOLD);
        // A project rule waives the check; another raises the bar.
        let waive = RuleDef {
            id: "p1".into(),
            description: String::new(),
            priority: RulePriority::Project,
            condition: when("wall", Trade::Architectural, &[]),
            action: RuleAction { kind: "waive".into(), value: "arch-wall-length".into(), ..Default::default() },
            source: String::new(),
            confidence: 1.0,
        };
        let r2 = evaluate_model(&builtin_packs(), &[waive], &elems, DEFAULT_THRESHOLD, &BTreeSet::new(), &BTreeSet::new());
        assert!(!r2.findings.iter().any(|f| f.kind == "check"));
        let stricter = RuleDef {
            id: "p2".into(),
            description: "Walls at least 10 ft".into(),
            priority: RulePriority::Project,
            condition: when("wall", Trade::Architectural, &[]),
            action: check("length_in", Some(130.0), None),
            source: String::new(),
            confidence: 0.3,
        };
        let r3 = evaluate_model(&builtin_packs(), &[stricter], &elems, DEFAULT_THRESHOLD, &BTreeSet::new(), &BTreeSet::new());
        let f = r3.findings.iter().find(|f| f.kind == "check").unwrap();
        assert_eq!(f.elements.len(), 4, "the project rule decides the key: {f:?}");
    }

    #[test]
    fn accepted_elements_are_agent_decided_and_rejected_ones_are_raised() {
        let elems = vec![wall("C", true, 2.0), wall("A", true, 120.0)];
        let acc: BTreeSet<String> = ["k-C".to_string()].into();
        let rej: BTreeSet<String> = ["k-A".to_string()].into();
        let r = evaluate_model(&builtin_packs(), &[], &elems, DEFAULT_THRESHOLD, &acc, &rej);
        let c = r.elements.iter().find(|e| e.id == "C").unwrap();
        assert!(c.agent_decided && (c.confidence - 1.0).abs() < 1e-12);
        assert!(r.findings.iter().any(|f| f.kind == "rejected" && f.elements == vec!["A".to_string()]));
        let score = compute_score(&r.elements, DEFAULT_THRESHOLD);
        assert_eq!((score.accepted, score.rejected), (1, 1));
    }

    #[test]
    fn completeness_flags_a_level_without_doors() {
        let r = eval(&[wall("A", true, 120.0)]);
        assert!(r.findings.iter().any(|f| f.kind == "completeness" && f.level == "L1"));
        let door = elem("X", Trade::Architectural, "door", "L1", &[("source", "print")], &[("length_in", 36.0), ("height_in", 84.0)]);
        let r = eval(&[wall("A", true, 120.0), door]);
        assert!(!r.findings.iter().any(|f| f.kind == "completeness"));
        assert!(!r.findings.iter().any(|f| f.kind == "check"), "a 3 ft by 7 ft door passes: {:?}", r.findings);
    }

    #[test]
    fn answers_stand_while_nothing_new_and_fixed_findings_are_resolved() {
        let mut a = Finding {
            id: "f1".into(),
            kind: "check".into(),
            trade: Trade::Architectural,
            level: "L1".into(),
            severity: "high".into(),
            title: String::new(),
            detail: String::new(),
            elements: vec!["1".into()],
            proposed: String::new(),
            alternatives: Vec::new(),
            status: "resolved".into(),
            resolution: Some(Resolution { action: "accept".into(), author: "agent".into(), citation: String::new(), save_as_rule: false }),
        };
        let mut again = a.clone();
        again.status = "open".into();
        again.resolution = None;
        let out = merge_findings(std::slice::from_ref(&a), vec![again.clone()], |_| true);
        assert_eq!(out.first().unwrap().status, "resolved");
        // Something new: asked again.
        again.elements.push("2".into());
        let out = merge_findings(std::slice::from_ref(&a), vec![again], |_| true);
        assert_eq!(out.first().unwrap().status, "open");
        // Open and gone: fixed.
        a.status = "open".into();
        let out = merge_findings(&[a], Vec::new(), |_| true);
        assert_eq!(out.first().unwrap().status, "resolved");
        assert_eq!(out.first().unwrap().resolution.as_ref().unwrap().action, "fixed");
    }

    #[test]
    fn answers_become_rules() {
        let packs = builtin_packs();
        let r = eval(&[wall("C", true, 2.0)]);
        let f = r.findings.iter().find(|f| f.kind == "check").unwrap();
        let rule = rule_from_answer(f, "accept", &packs).unwrap();
        assert_eq!(rule.action.kind, "waive");
        assert_eq!(rule.action.value, "arch-wall-length");
        assert_eq!(rule.condition.properties.get("level").map(String::as_str), Some("L1"));
        assert!(rule_from_answer(f, "delete", &packs).is_none());
    }

    #[test]
    fn report_and_score() {
        let r = eval(&[wall("A", true, 120.0), wall("B", true, 240.0), wall("C", true, 2.0)]);
        let lines = generate_report(&r.elements, &r.findings, DEFAULT_THRESHOLD);
        let w = lines.iter().find(|l| l.category == "wall").unwrap();
        assert_eq!(w.count, 3);
        assert_eq!(w.quantity, "3 elements, 30 ft");
        assert_eq!(w.open_assumptions, 1);
        let s = compute_score(&r.elements, DEFAULT_THRESHOLD);
        assert!((s.overall - 2.0 / 3.0).abs() < 1e-9);
        assert!((s.by_trade.get("architectural").unwrap().by_category.get("wall").unwrap() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(compute_score(&[], DEFAULT_THRESHOLD).sample_size, 0);
    }

    #[test]
    fn gravity_and_fire_systems() {
        assert!(is_gravity("P-SAN") && is_gravity("Storm drain") && !is_gravity("P-DCW"));
        assert!(is_fire("FP-MAIN") && is_fire("Sprinkler") && !is_fire("M-SA"));
    }

    #[test]
    fn rule_pack_json_round_trip() {
        let pack = architectural_rules();
        let back: RulePack = serde_json::from_str(&serde_json::to_string(&pack).unwrap()).unwrap();
        assert_eq!(pack, back);
    }
}
