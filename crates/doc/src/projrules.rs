//! Project rules: a versioned, human-readable set of defaults that override built-in values.
//!
//! Every rule carries a **source** (spec section, RFI number, "GC preference", …) and a
//! **source tier** that says where in the precedence order it sits. The tiers, from highest
//! (wins) to lowest:
//!
//! 1. `FieldVerified` — measured or confirmed on site (as-built).
//! 2. `Rfi` — answered RFI, ASI, or bulletin.
//! 3. `Submittal` — approved submittal data.
//! 4. `DrawingRevision` — latest sheet revision note.
//! 5. `Spec` — specification section.
//! 6. `ProjectRule` — the project rules file.
//! 7. `BuiltIn` — CADaddy's built-in defaults (no file).
//!
//! Rule **domains**: mounting heights by device/room type, wiring methods by location,
//! stud/joist layout preferences, clearances, tolerances, naming conventions.
//!
//! Persisted as `CADADDY_PROJRULES` in the named-object dictionary (JSON XRECORD).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Cap: most rules in one drawing.
pub const MAX_RULES: usize = 10_000;
/// Cap: most entries in a single rule's changelog.
pub const MAX_CHANGELOG: usize = 500;

// ── source tier ────────────────────────────────────────────────────────────

/// Where a value came from, ordered from highest priority to lowest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SourceTier {
    /// Measured or confirmed on site (as-built / field-verified).
    FieldVerified = 0,
    /// Answered RFI, ASI, or bulletin.
    Rfi = 1,
    /// Approved submittal data.
    Submittal = 2,
    /// Latest drawing revision note.
    DrawingRevision = 3,
    /// Specification section.
    Spec = 4,
    /// The project rules file itself.
    #[default]
    ProjectRule = 5,
    /// CADaddy's built-in default.
    BuiltIn = 6,
}

impl SourceTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FieldVerified => "field_verified",
            Self::Rfi => "rfi",
            Self::Submittal => "submittal",
            Self::DrawingRevision => "drawing_revision",
            Self::Spec => "spec",
            Self::ProjectRule => "project_rule",
            Self::BuiltIn => "built_in",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FieldVerified => "Field-verified / as-built",
            Self::Rfi => "Answered RFI / ASI / bulletin",
            Self::Submittal => "Approved submittal",
            Self::DrawingRevision => "Drawing revision",
            Self::Spec => "Specification",
            Self::ProjectRule => "Project rule",
            Self::BuiltIn => "Built-in default",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "field_verified" | "fieldverified" | "as_built" | "asbuilt" => Some(Self::FieldVerified),
            "rfi" | "asi" | "bulletin" => Some(Self::Rfi),
            "submittal" => Some(Self::Submittal),
            "drawing_revision" | "drawingrevision" | "revision" => Some(Self::DrawingRevision),
            "spec" | "specification" => Some(Self::Spec),
            "project_rule" | "projectrule" | "rule" => Some(Self::ProjectRule),
            "built_in" | "builtin" | "default" => Some(Self::BuiltIn),
            _ => None,
        }
    }

    /// All tiers in precedence order (highest first).
    pub const ALL: [Self; 7] = [Self::FieldVerified, Self::Rfi, Self::Submittal, Self::DrawingRevision, Self::Spec, Self::ProjectRule, Self::BuiltIn];
}

// ── rule domain ────────────────────────────────────────────────────────────

/// The domain a rule belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDomain {
    /// Mounting heights for device types in room types.
    MountingHeight,
    /// Wiring method for a location or area classification.
    WiringMethod,
    /// Stud layout (spacing, material).
    StudLayout,
    /// Joist layout (spacing, depth, material).
    JoistLayout,
    /// Minimum clearances.
    Clearance,
    /// Tolerances (placement, alignment, dimensional).
    Tolerance,
    /// Naming conventions (layer, block, sheet).
    Naming,
    /// Catch-all for project-specific rules.
    Custom,
}

impl RuleDomain {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MountingHeight => "mounting_height",
            Self::WiringMethod => "wiring_method",
            Self::StudLayout => "stud_layout",
            Self::JoistLayout => "joist_layout",
            Self::Clearance => "clearance",
            Self::Tolerance => "tolerance",
            Self::Naming => "naming",
            Self::Custom => "custom",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::MountingHeight => "Mounting Heights",
            Self::WiringMethod => "Wiring Methods",
            Self::StudLayout => "Stud Layout",
            Self::JoistLayout => "Joist Layout",
            Self::Clearance => "Clearances",
            Self::Tolerance => "Tolerances",
            Self::Naming => "Naming Conventions",
            Self::Custom => "Custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "mounting_height" | "mountingheight" | "height" => Some(Self::MountingHeight),
            "wiring_method" | "wiringmethod" | "wiring" => Some(Self::WiringMethod),
            "stud_layout" | "studlayout" | "stud" => Some(Self::StudLayout),
            "joist_layout" | "joistlayout" | "joist" => Some(Self::JoistLayout),
            "clearance" => Some(Self::Clearance),
            "tolerance" => Some(Self::Tolerance),
            "naming" => Some(Self::Naming),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    pub const ALL: [Self; 8] =
        [Self::MountingHeight, Self::WiringMethod, Self::StudLayout, Self::JoistLayout, Self::Clearance, Self::Tolerance, Self::Naming, Self::Custom];
}

// ── rule key ───────────────────────────────────────────────────────────────

/// A scoped key that says **what** the rule applies to. The meaning depends on the domain:
///
/// - `MountingHeight`: device type + room type → e.g. `"receptacle/general"`, `"switch/bathroom"`
/// - `WiringMethod`: location → e.g. `"wet_area"`, `"plenum"`, `"general"`
/// - `StudLayout`: wall assembly → e.g. `"exterior"`, `"interior"`
/// - `JoistLayout`: floor/ceiling assembly → e.g. `"floor/main"`, `"ceiling/corridor"`
/// - `Clearance`: situation → e.g. `"panel_front"`, `"working_space"`
/// - `Tolerance`: dimension → e.g. `"placement"`, `"alignment"`
/// - `Naming`: entity kind → e.g. `"layer"`, `"block"`, `"sheet"`
/// - `Custom`: free text key.
pub type RuleKey = String;

// ── rule value ─────────────────────────────────────────────────────────────

/// One rule value with its source provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleValue {
    /// The value as free text (e.g. "48\"", "EMT", "16\" o.c.", "36\"").
    pub value: String,
    /// Where this value came from (e.g. "NEC 210.52", "Spec 26 05 33 p.4", "RFI #23").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// The source tier for precedence.
    #[serde(default)]
    pub tier: SourceTier,
    /// Who set this (e.g. a person's role: "PM", "foreman", "GC", "agent").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub set_by: String,
    /// When this was last changed (ISO 8601, optional).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub changed: String,
}

impl RuleValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self { value: value.into(), source: String::new(), tier: SourceTier::ProjectRule, set_by: String::new(), changed: String::new() }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    pub fn with_tier(mut self, tier: SourceTier) -> Self {
        self.tier = tier;
        self
    }

    pub fn with_set_by(mut self, by: impl Into<String>) -> Self {
        self.set_by = by.into();
        self
    }
}

// ── rule entry ─────────────────────────────────────────────────────────────

/// A single project rule: domain + key + active value + optional overridden values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleEntry {
    /// The domain (mounting height, wiring method, …).
    pub domain: RuleDomain,
    /// The scoped key within the domain.
    pub key: RuleKey,
    /// The active (winning) value. The highest-tier value wins.
    pub active: RuleValue,
    /// Other values that lost: lower-tier entries that the active one overrode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overridden: Vec<RuleValue>,
    /// Change history (newest first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changelog: Vec<ChangeLogEntry>,
}

impl RuleEntry {
    /// Unique identifier for this rule: `"domain/key"`.
    pub fn id(&self) -> String {
        format!("{}/{}", self.domain.as_str(), self.key)
    }

    /// All values (active + overridden) sorted by tier (highest first, using default order).
    pub fn all_values(&self) -> Vec<&RuleValue> {
        let mut all: Vec<&RuleValue> = std::iter::once(&self.active).chain(self.overridden.iter()).collect();
        all.sort_by_key(|v| v.tier);
        all
    }

    /// All values sorted by the given source_order.
    pub fn all_values_ranked(&self, tier_rank: impl Fn(SourceTier) -> usize) -> Vec<&RuleValue> {
        let mut all: Vec<&RuleValue> = std::iter::once(&self.active).chain(self.overridden.iter()).collect();
        all.sort_by_key(|v| tier_rank(v.tier));
        all
    }

    /// Add or update a value at the given tier. Returns a preview of what would change
    /// (the new active value and what it would override) without modifying self.
    /// `tier_rank` maps each tier to its precedence position (lower = higher priority).
    pub fn preview_set(&self, new: &RuleValue, tier_rank: impl Fn(SourceTier) -> usize) -> ChangePreview {
        let mut all_values: Vec<RuleValue> = std::iter::once(self.active.clone())
            .chain(self.overridden.iter().cloned())
            // Replace any existing value at the same tier, or add.
            .filter(|v| v.tier != new.tier)
            .chain(std::iter::once(new.clone()))
            .collect();
        all_values.sort_by_key(|v| tier_rank(v.tier));

        let new_active = all_values.first().cloned().unwrap_or_else(|| new.clone());
        let new_overridden: Vec<RuleValue> = all_values.into_iter().skip(1).collect();
        let changed = new_active.value != self.active.value;

        ChangePreview {
            old_value: self.active.value.clone(),
            old_tier: self.active.tier,
            new_value: new_active.value.clone(),
            new_tier: new_active.tier,
            changed,
            overridden: new_overridden,
        }
    }

    /// Apply a new value, re-sorting by tier precedence. Returns what changed.
    /// `tier_rank` maps each tier to its precedence position (lower = higher priority).
    pub fn apply_set(&mut self, new: RuleValue, reason: &str, tier_rank: impl Fn(SourceTier) -> usize) -> ChangePreview {
        let preview = self.preview_set(&new, &tier_rank);
        let set_by = new.set_by.clone();

        // Rebuild the values list.
        let new_tier = new.tier;
        let mut all_values: Vec<RuleValue> = std::iter::once(self.active.clone())
            .chain(self.overridden.drain(..))
            .filter(|v| v.tier != new_tier)
            .chain(std::iter::once(new))
            .collect();
        all_values.sort_by_key(|v| tier_rank(v.tier));

        if let Some(winner) = all_values.first().cloned() {
            self.active = winner;
        }
        self.overridden = all_values.into_iter().skip(1).collect();

        // Log every set (including lower-tier sets that don't change the active value).
        if self.changelog.len() < MAX_CHANGELOG {
            self.changelog.insert(
                0,
                ChangeLogEntry {
                    from: preview.old_value.clone(),
                    to: preview.new_value.clone(),
                    reason: reason.to_string(),
                    tier: new_tier,
                    set_by,
                    time: String::new(),
                },
            );
        }

        preview
    }

    /// Remove the value at a tier. If it was the active one, the next-highest takes over.
    /// Returns `(removed_value, entry_is_now_empty)`.
    pub fn remove_tier(&mut self, tier: SourceTier) -> (Option<RuleValue>, bool) {
        if self.active.tier == tier {
            let removed = self.active.clone();
            if let Some(next) = self.overridden.first().cloned() {
                self.active = next;
                self.overridden.remove(0);
                (Some(removed), false)
            } else {
                // This was the only value — entry is now empty.
                (Some(removed), true)
            }
        } else if let Some(pos) = self.overridden.iter().position(|v| v.tier == tier) {
            (Some(self.overridden.remove(pos)), false)
        } else {
            (None, false)
        }
    }
}

// ── change preview ─────────────────────────────────────────────────────────

/// What setting a value would change, before actually doing it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePreview {
    pub old_value: String,
    pub old_tier: SourceTier,
    pub new_value: String,
    pub new_tier: SourceTier,
    /// Whether the active value would actually change.
    pub changed: bool,
    /// What would be overridden after the change.
    pub overridden: Vec<RuleValue>,
}

// ── changelog entry ────────────────────────────────────────────────────────

/// One change in a rule's history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeLogEntry {
    pub from: String,
    pub to: String,
    pub reason: String,
    pub tier: SourceTier,
    /// Who made this change (e.g. "agent", "PM", "foreman").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub set_by: String,
    /// When this change was made (ISO 8601, optional).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub time: String,
}

// ── project rules collection ───────────────────────────────────────────────

/// The drawing's project rules. Versioned, persisted as JSON in a named-object dictionary
/// XRECORD (`CADADDY_PROJRULES`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRules {
    /// Schema version for forward compatibility.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Human label for these rules (e.g. project name, "Mercy Tower Phase 2").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// The source order (tiers, highest first). Defaults to the standard order.
    /// This lets a project swap e.g. "submittal > drawing_revision" if the owner prefers.
    #[serde(default = "default_source_order", skip_serializing_if = "is_default_source_order")]
    pub source_order: Vec<SourceTier>,
    /// All rules keyed by `"domain/key"`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, RuleEntry>,
}

fn default_version() -> u32 {
    1
}

fn default_source_order() -> Vec<SourceTier> {
    SourceTier::ALL.to_vec()
}

fn is_default_source_order(order: &[SourceTier]) -> bool {
    order == SourceTier::ALL
}

impl ProjectRules {
    pub fn new() -> Self {
        Self { version: 1, label: String::new(), source_order: default_source_order(), rules: BTreeMap::new() }
    }

    /// Position of a tier in the source order (lower = higher priority).
    /// Falls back to the tier's enum order if it's not in the list.
    pub fn tier_rank(&self, tier: SourceTier) -> usize {
        self.source_order.iter().position(|&t| t == tier).unwrap_or(tier as usize + 100)
    }

    /// Look up a rule by domain and key.
    pub fn get(&self, domain: RuleDomain, key: &str) -> Option<&RuleEntry> {
        let id = format!("{}/{}", domain.as_str(), key);
        self.rules.get(&id)
    }

    /// Look up a rule mutably.
    pub fn get_mut(&mut self, domain: RuleDomain, key: &str) -> Option<&mut RuleEntry> {
        let id = format!("{}/{}", domain.as_str(), key);
        self.rules.get_mut(&id)
    }

    /// Set a rule value. Creates the entry if it doesn't exist. Returns the change preview.
    pub fn set(&mut self, domain: RuleDomain, key: &str, value: RuleValue, reason: &str) -> ChangePreview {
        let id = format!("{}/{}", domain.as_str(), key);
        if self.rules.len() >= MAX_RULES && !self.rules.contains_key(&id) {
            return ChangePreview {
                old_value: String::new(),
                old_tier: SourceTier::BuiltIn,
                new_value: value.value.clone(),
                new_tier: value.tier,
                changed: false,
                overridden: Vec::new(),
            };
        }
        if let Some(entry) = self.rules.get_mut(&id) {
            let so = self.source_order.clone();
            let rank = move |t: SourceTier| so.iter().position(|&x| x == t).unwrap_or(t as usize + 100);
            entry.apply_set(value, reason, rank)
        } else {
            // Brand new entry: no phantom placeholder.
            let preview = ChangePreview {
                old_value: String::new(),
                old_tier: SourceTier::BuiltIn,
                new_value: value.value.clone(),
                new_tier: value.tier,
                changed: true,
                overridden: Vec::new(),
            };
            let mut entry = RuleEntry {
                domain,
                key: key.to_string(),
                active: value,
                overridden: Vec::new(),
                changelog: Vec::new(),
            };
            if entry.changelog.len() < MAX_CHANGELOG {
                entry.changelog.push(ChangeLogEntry {
                    from: String::new(),
                    to: entry.active.value.clone(),
                    reason: reason.to_string(),
                    tier: entry.active.tier,
                    set_by: entry.active.set_by.clone(),
                    time: String::new(),
                });
            }
            self.rules.insert(id, entry);
            preview
        }
    }

    /// Preview what setting a value would change, without modifying anything.
    pub fn preview(&self, domain: RuleDomain, key: &str, value: &RuleValue) -> ChangePreview {
        let id = format!("{}/{}", domain.as_str(), key);
        if let Some(entry) = self.rules.get(&id) {
            entry.preview_set(value, |t| self.tier_rank(t))
        } else {
            ChangePreview {
                old_value: String::new(),
                old_tier: SourceTier::BuiltIn,
                new_value: value.value.clone(),
                new_tier: value.tier,
                changed: true,
                overridden: Vec::new(),
            }
        }
    }

    /// Remove a rule entirely.
    pub fn remove(&mut self, domain: RuleDomain, key: &str) -> Option<RuleEntry> {
        let id = format!("{}/{}", domain.as_str(), key);
        self.rules.remove(&id)
    }

    /// Remove the value at a specific tier from a rule. If it was the sole value, the
    /// entire rule is deleted (not left as a phantom entry).
    pub fn remove_tier(&mut self, domain: RuleDomain, key: &str, tier: SourceTier) -> Option<RuleValue> {
        let id = format!("{}/{}", domain.as_str(), key);
        let entry = self.rules.get_mut(&id)?;
        let (removed, empty) = entry.remove_tier(tier);
        if empty {
            self.rules.remove(&id);
        }
        removed
    }

    /// List all rules in a domain.
    pub fn list_domain(&self, domain: RuleDomain) -> Vec<&RuleEntry> {
        self.rules.values().filter(|r| r.domain == domain).collect()
    }

    /// List all rules, optionally filtered by domain.
    pub fn list(&self, domain: Option<RuleDomain>) -> Vec<&RuleEntry> {
        match domain {
            Some(d) => self.rules.values().filter(|r| r.domain == d).collect(),
            None => self.rules.values().collect(),
        }
    }

    /// Resolve a value for an element: find the rule and report which source won.
    pub fn resolve(&self, domain: RuleDomain, key: &str) -> Option<ResolvedValue> {
        let entry = self.get(domain, key)?;
        let overridden_by: Vec<(String, SourceTier)> = entry.overridden.iter().map(|v| (v.value.clone(), v.tier)).collect();
        Some(ResolvedValue {
            value: entry.active.value.clone(),
            tier: entry.active.tier,
            source: entry.active.source.clone(),
            overrides: overridden_by,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.label.is_empty()
    }

    /// Serialise to JSON for the XRECORD.
    pub fn to_json(&self) -> std::result::Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialise from JSON.
    pub fn from_json(s: &str) -> std::result::Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

// ── resolved value ─────────────────────────────────────────────────────────

/// The result of resolving a rule for an element: which source won and what it overrode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedValue {
    /// The winning value.
    pub value: String,
    /// The tier that supplied it.
    pub tier: SourceTier,
    /// The source citation.
    pub source: String,
    /// Values that lost: (value, tier) for each overridden entry.
    pub overrides: Vec<(String, SourceTier)>,
}

// ── built-in defaults ──────────────────────────────────────────────────────

/// Built-in default mounting heights (in inches AFF) by device type and room type.
/// These are the fallback when no project rule is set.
pub fn builtin_mounting_height(device: &str, room: &str) -> Option<&'static str> {
    // Common defaults based on standard practice and NEC.
    match (device.to_ascii_lowercase().as_str(), room.to_ascii_lowercase().as_str()) {
        ("receptacle", "general") | ("receptacle", "office") | ("receptacle", "bedroom") => Some("18\""),
        ("receptacle", "kitchen") | ("receptacle", "counter") => Some("44\""),
        ("receptacle", "bathroom") | ("receptacle", "bath") => Some("44\""),
        ("receptacle", "garage") => Some("48\""),
        ("receptacle", "exterior") | ("receptacle", "outdoor") => Some("18\""),
        ("switch", _) => Some("48\""),
        ("thermostat", _) => Some("48\""),
        ("panel", _) => Some("72\" to top"),
        ("fire_alarm", _) | ("pull_station", _) => Some("48\""),
        ("smoke_detector", _) => Some("ceiling"),
        ("exit_sign", _) => Some("84\" or above door"),
        ("sconce", _) | ("wall_light", _) => Some("72\""),
        _ => None,
    }
}

/// Built-in default wiring methods by location.
pub fn builtin_wiring_method(location: &str) -> Option<&'static str> {
    match location.to_ascii_lowercase().as_str() {
        "general" | "interior" => Some("MC cable"),
        "wet_area" | "wet" | "exterior" => Some("EMT or PVC"),
        "plenum" => Some("MC cable, plenum-rated"),
        "underground" | "below_grade" => Some("PVC Sch 40"),
        "exposed" => Some("EMT"),
        "hazardous" => Some("Rigid metal conduit"),
        "concealed" => Some("MC cable or EMT"),
        _ => None,
    }
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_tier_ordering() {
        assert!(SourceTier::FieldVerified < SourceTier::Rfi);
        assert!(SourceTier::Rfi < SourceTier::Submittal);
        assert!(SourceTier::Submittal < SourceTier::DrawingRevision);
        assert!(SourceTier::DrawingRevision < SourceTier::Spec);
        assert!(SourceTier::Spec < SourceTier::ProjectRule);
        assert!(SourceTier::ProjectRule < SourceTier::BuiltIn);
    }

    #[test]
    fn source_tier_parse() {
        assert_eq!(SourceTier::parse("field_verified"), Some(SourceTier::FieldVerified));
        assert_eq!(SourceTier::parse("rfi"), Some(SourceTier::Rfi));
        assert_eq!(SourceTier::parse("submittal"), Some(SourceTier::Submittal));
        assert_eq!(SourceTier::parse("spec"), Some(SourceTier::Spec));
        assert_eq!(SourceTier::parse("built_in"), Some(SourceTier::BuiltIn));
        assert_eq!(SourceTier::parse("nonsense"), None);
    }

    #[test]
    fn rule_domain_parse() {
        assert_eq!(RuleDomain::parse("mounting_height"), Some(RuleDomain::MountingHeight));
        assert_eq!(RuleDomain::parse("wiring"), Some(RuleDomain::WiringMethod));
        assert_eq!(RuleDomain::parse("clearance"), Some(RuleDomain::Clearance));
        assert_eq!(RuleDomain::parse("bogus"), None);
    }

    #[test]
    fn set_and_get_rule() {
        let mut rules = ProjectRules::new();
        let val = RuleValue::new("48\"").with_source("Spec 26 05 33").with_tier(SourceTier::Spec);
        let preview = rules.set(RuleDomain::MountingHeight, "switch/general", val, "initial setup");
        assert!(preview.changed);
        assert_eq!(preview.new_value, "48\"");

        let entry = rules.get(RuleDomain::MountingHeight, "switch/general").unwrap();
        assert_eq!(entry.active.value, "48\"");
        assert_eq!(entry.active.tier, SourceTier::Spec);
    }

    #[test]
    fn precedence_higher_tier_wins() {
        let mut rules = ProjectRules::new();

        // Set a project-rule-tier value.
        rules.set(RuleDomain::MountingHeight, "receptacle/general", RuleValue::new("18\"").with_tier(SourceTier::ProjectRule), "project default");
        assert_eq!(rules.get(RuleDomain::MountingHeight, "receptacle/general").unwrap().active.value, "18\"",);

        // Override with a spec value (higher precedence).
        let preview = rules.set(
            RuleDomain::MountingHeight,
            "receptacle/general",
            RuleValue::new("15\"").with_source("Spec 26 05 33 p.8").with_tier(SourceTier::Spec),
            "spec says 15\"",
        );
        assert!(preview.changed);
        assert_eq!(preview.new_value, "15\"");
        assert_eq!(preview.old_value, "18\"");

        let entry = rules.get(RuleDomain::MountingHeight, "receptacle/general").unwrap();
        assert_eq!(entry.active.value, "15\"");
        assert_eq!(entry.active.tier, SourceTier::Spec);
        assert_eq!(entry.overridden.len(), 1);
        assert_eq!(entry.overridden[0].value, "18\"");
        assert_eq!(entry.overridden[0].tier, SourceTier::ProjectRule);
    }

    #[test]
    fn lower_tier_does_not_override() {
        let mut rules = ProjectRules::new();

        rules.set(RuleDomain::WiringMethod, "general", RuleValue::new("EMT").with_tier(SourceTier::Spec), "spec requirement");

        // Try to set a project-rule value (lower tier); shouldn't change the active.
        let preview =
            rules.set(RuleDomain::WiringMethod, "general", RuleValue::new("MC cable").with_tier(SourceTier::ProjectRule), "project default");
        assert!(!preview.changed);
        assert_eq!(preview.new_value, "EMT"); // spec still wins

        let entry = rules.get(RuleDomain::WiringMethod, "general").unwrap();
        assert_eq!(entry.active.value, "EMT");
        assert_eq!(entry.overridden.len(), 1);
        assert_eq!(entry.overridden[0].value, "MC cable");
    }

    #[test]
    fn remove_tier_promotes_next() {
        let mut rules = ProjectRules::new();

        rules.set(RuleDomain::MountingHeight, "switch/general", RuleValue::new("48\"").with_tier(SourceTier::ProjectRule), "");
        rules.set(
            RuleDomain::MountingHeight,
            "switch/general",
            RuleValue::new("46\"").with_tier(SourceTier::Rfi).with_source("RFI #12"),
            "RFI response",
        );

        assert_eq!(rules.get(RuleDomain::MountingHeight, "switch/general").unwrap().active.value, "46\"",);

        // Remove the RFI tier; project rule should take over.
        let removed = rules.remove_tier(RuleDomain::MountingHeight, "switch/general", SourceTier::Rfi);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().value, "46\"");
        assert_eq!(rules.get(RuleDomain::MountingHeight, "switch/general").unwrap().active.value, "48\"",);
    }

    #[test]
    fn first_set_at_spec_has_no_phantom_override() {
        let mut rules = ProjectRules::new();
        let val = RuleValue::new("36\"").with_source("NEC 110.26").with_tier(SourceTier::Spec);
        rules.set(RuleDomain::Clearance, "panel_front", val, "initial");

        let entry = rules.get(RuleDomain::Clearance, "panel_front").unwrap();
        assert_eq!(entry.active.value, "36\"");
        assert_eq!(entry.active.tier, SourceTier::Spec);
        // No phantom empty ProjectRule override.
        assert!(entry.overridden.is_empty(), "overrides should be empty, got {:?}", entry.overridden);
    }

    #[test]
    fn set_at_built_in_on_new_key_works() {
        let mut rules = ProjectRules::new();
        let val = RuleValue::new("48\"").with_tier(SourceTier::BuiltIn);
        let preview = rules.set(RuleDomain::MountingHeight, "switch/general", val, "builtin default");

        assert!(preview.changed);
        assert_eq!(preview.new_value, "48\"");

        let entry = rules.get(RuleDomain::MountingHeight, "switch/general").unwrap();
        assert_eq!(entry.active.value, "48\"");
        assert_eq!(entry.active.tier, SourceTier::BuiltIn);
    }

    #[test]
    fn source_order_swapped_changes_winner() {
        let mut rules = ProjectRules::new();
        // Swap submittal and drawing_revision in source_order:
        // default: [FV, Rfi, Submittal, DrawingRevision, Spec, ProjectRule, BuiltIn]
        // swapped: [FV, Rfi, DrawingRevision, Submittal, Spec, ProjectRule, BuiltIn]
        rules.source_order = vec![
            SourceTier::FieldVerified,
            SourceTier::Rfi,
            SourceTier::DrawingRevision,
            SourceTier::Submittal,
            SourceTier::Spec,
            SourceTier::ProjectRule,
            SourceTier::BuiltIn,
        ];

        rules.set(
            RuleDomain::MountingHeight,
            "receptacle/kitchen",
            RuleValue::new("44\"").with_tier(SourceTier::Submittal).with_source("Submittal #A-3"),
            "submittal",
        );
        rules.set(
            RuleDomain::MountingHeight,
            "receptacle/kitchen",
            RuleValue::new("42\"").with_tier(SourceTier::DrawingRevision).with_source("Rev 3"),
            "drawing revision",
        );

        // With swapped order, DrawingRevision is now higher priority than Submittal.
        let entry = rules.get(RuleDomain::MountingHeight, "receptacle/kitchen").unwrap();
        assert_eq!(entry.active.value, "42\"", "drawing_revision should win with swapped order");
        assert_eq!(entry.active.tier, SourceTier::DrawingRevision);
        assert_eq!(entry.overridden.len(), 1);
        assert_eq!(entry.overridden[0].value, "44\"");
        assert_eq!(entry.overridden[0].tier, SourceTier::Submittal);
    }

    #[test]
    fn remove_sole_tier_deletes_rule() {
        let mut rules = ProjectRules::new();
        rules.set(RuleDomain::Naming, "layer", RuleValue::new("E-prefix").with_tier(SourceTier::Spec), "spec says");

        let removed = rules.remove_tier(RuleDomain::Naming, "layer", SourceTier::Spec);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().value, "E-prefix");
        // The rule should be fully gone.
        assert!(rules.get(RuleDomain::Naming, "layer").is_none(), "rule should be deleted after removing sole tier");
    }

    #[test]
    fn preview_does_not_modify() {
        let mut rules = ProjectRules::new();
        rules.set(RuleDomain::Clearance, "panel_front", RuleValue::new("36\"").with_tier(SourceTier::Spec).with_source("NEC 110.26"), "");

        let proposed = RuleValue::new("42\"").with_tier(SourceTier::Rfi).with_source("RFI #5");
        let preview = rules.preview(RuleDomain::Clearance, "panel_front", &proposed);
        assert!(preview.changed);
        assert_eq!(preview.new_value, "42\"");

        // The actual value should be unchanged.
        assert_eq!(rules.get(RuleDomain::Clearance, "panel_front").unwrap().active.value, "36\"",);
    }

    #[test]
    fn changelog_recorded() {
        let mut rules = ProjectRules::new();
        rules.set(RuleDomain::Tolerance, "placement", RuleValue::new("1/4\"").with_tier(SourceTier::ProjectRule), "initial");
        rules.set(
            RuleDomain::Tolerance,
            "placement",
            RuleValue::new("1/8\"").with_tier(SourceTier::Spec).with_source("Spec 01 43 00"),
            "tightened by spec",
        );

        let entry = rules.get(RuleDomain::Tolerance, "placement").unwrap();
        // Two changelog entries: the initial set (from "" to "1/4\"") and the spec override.
        assert_eq!(entry.changelog.len(), 2);
        // Newest first.
        assert_eq!(entry.changelog[0].from, "1/4\"");
        assert_eq!(entry.changelog[0].to, "1/8\"");
        assert_eq!(entry.changelog[0].reason, "tightened by spec");
        assert_eq!(entry.changelog[1].from, "");
        assert_eq!(entry.changelog[1].to, "1/4\"");
        assert_eq!(entry.changelog[1].reason, "initial");
    }

    #[test]
    fn resolve_value() {
        let mut rules = ProjectRules::new();
        rules.set(RuleDomain::MountingHeight, "receptacle/kitchen", RuleValue::new("44\"").with_tier(SourceTier::ProjectRule), "");
        rules.set(
            RuleDomain::MountingHeight,
            "receptacle/kitchen",
            RuleValue::new("42\"").with_tier(SourceTier::Submittal).with_source("Submittal #A-3"),
            "submittal says 42\"",
        );

        let resolved = rules.resolve(RuleDomain::MountingHeight, "receptacle/kitchen").unwrap();
        assert_eq!(resolved.value, "42\"");
        assert_eq!(resolved.tier, SourceTier::Submittal);
        assert_eq!(resolved.source, "Submittal #A-3");
        assert_eq!(resolved.overrides.len(), 1);
        assert_eq!(resolved.overrides[0].0, "44\"");
        assert_eq!(resolved.overrides[0].1, SourceTier::ProjectRule);
    }

    #[test]
    fn json_round_trip() {
        let mut rules = ProjectRules::new();
        rules.label = "Sample Project".to_string();
        rules.set(RuleDomain::MountingHeight, "switch/general", RuleValue::new("48\"").with_source("NEC").with_tier(SourceTier::Spec), "initial");
        rules.set(RuleDomain::WiringMethod, "general", RuleValue::new("MC cable").with_tier(SourceTier::ProjectRule), "project default");

        let json = rules.to_json().unwrap();
        let back = ProjectRules::from_json(&json).unwrap();
        assert_eq!(rules, back);
    }

    #[test]
    fn builtin_defaults() {
        assert_eq!(builtin_mounting_height("receptacle", "general"), Some("18\""));
        assert_eq!(builtin_mounting_height("switch", "anything"), Some("48\""));
        assert_eq!(builtin_mounting_height("unknown", "unknown"), None);
        assert_eq!(builtin_wiring_method("general"), Some("MC cable"));
        assert_eq!(builtin_wiring_method("plenum"), Some("MC cable, plenum-rated"));
        assert_eq!(builtin_wiring_method("unknown"), None);
    }

    #[test]
    fn max_rules_cap() {
        let mut rules = ProjectRules::new();
        for i in 0..MAX_RULES + 10 {
            rules.set(RuleDomain::Custom, &format!("rule_{i}"), RuleValue::new("v"), "");
        }
        assert_eq!(rules.rules.len(), MAX_RULES);
    }

    #[test]
    fn list_by_domain() {
        let mut rules = ProjectRules::new();
        rules.set(RuleDomain::MountingHeight, "a", RuleValue::new("1"), "");
        rules.set(RuleDomain::MountingHeight, "b", RuleValue::new("2"), "");
        rules.set(RuleDomain::WiringMethod, "c", RuleValue::new("3"), "");

        assert_eq!(rules.list(Some(RuleDomain::MountingHeight)).len(), 2);
        assert_eq!(rules.list(Some(RuleDomain::WiringMethod)).len(), 1);
        assert_eq!(rules.list(None).len(), 3);
    }

    #[test]
    fn remove_rule() {
        let mut rules = ProjectRules::new();
        rules.set(RuleDomain::Naming, "layer", RuleValue::new("prefix"), "");
        assert!(rules.remove(RuleDomain::Naming, "layer").is_some());
        assert!(rules.get(RuleDomain::Naming, "layer").is_none());
    }
}
