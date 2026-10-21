//! BIM element types and spec records.
//!
//! Every BIM element (device, luminaire, conduit, box, panel, duct, pipe, equipment, casework,
//! plumbing fixture) carries a typed spec record, separate from its geometry. Types are shared:
//! change type "H" once and every H updates. Each field records its source (sheet + note, or
//! spec section + page) and confidence (stated / inferred / assumed).
//!
//! Drawing-level: [`BimTypes`] in [`crate::Drawing::bim_types`], persisted as a dictionary
//! XRECORD (`CADADDY_BIMTYPES`).
//!
//! Entity-level: the type mark in XDATA (`CADADDY_BIMSPEC`, one `1000` string = the mark).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum number of BIM types in a drawing (hostile files).
pub const MAX_BIM_TYPES: usize = 50_000;
/// Maximum number of fields per type.
pub const MAX_FIELDS: usize = 200;

// ── confidence ──────────────────────────────────────────────────────────────

/// How certain a spec value is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Value explicitly given on the drawing or in a schedule/spec.
    #[default]
    Stated,
    /// Deduced from context (e.g. "same as the other outlets on this circuit").
    Inferred,
    /// A reasonable default assumed by the user or agent.
    Assumed,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stated => "stated",
            Self::Inferred => "inferred",
            Self::Assumed => "assumed",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "inferred" => Self::Inferred,
            "assumed" => Self::Assumed,
            _ => Self::Stated,
        }
    }
}

// ── spec field ──────────────────────────────────────────────────────────────

/// One field of a spec: value, source and confidence.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecField {
    /// The value as stated (free text: "PVC Sch 40", "#8 THHN", "26 27 26").
    pub value: String,
    /// Where this came from: sheet + note/schedule row, or spec section + page.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// How certain this is.
    #[serde(default, skip_serializing_if = "is_stated")]
    pub confidence: Confidence,
}

fn is_stated(c: &Confidence) -> bool {
    *c == Confidence::Stated
}

impl SpecField {
    pub fn new(value: impl Into<String>) -> Self {
        Self { value: value.into(), source: String::new(), confidence: Confidence::Stated }
    }
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }
    pub fn with_confidence(mut self, confidence: Confidence) -> Self {
        self.confidence = confidence;
        self
    }
}

// ── well-known field keys ───────────────────────────────────────────────────

/// Standard BIM spec field names. Values are free text; these names are conventions the
/// Properties panel, IFC export and MCP tools recognise.
pub mod keys {
    // Identity
    pub const DESCRIPTION: &str = "description";
    pub const MANUFACTURER: &str = "manufacturer";
    pub const CATALOG_NUMBER: &str = "catalogNumber";
    pub const SPEC_SECTION: &str = "specSection";

    // Material / finish
    pub const MATERIAL: &str = "material";
    pub const FINISH: &str = "finish";
    pub const COLOUR: &str = "colour";

    // Ratings
    pub const AMPS: &str = "amps";
    pub const VOLTS: &str = "volts";
    pub const NEMA_CONFIG: &str = "nemaConfig";
    pub const IP_RATING: &str = "ipRating";
    pub const WET_DAMP: &str = "wetDamp";
    pub const FIRE_RATING: &str = "fireRating";

    // Mounting
    pub const MOUNTING: &str = "mounting";
    pub const MOUNTING_HEIGHT: &str = "mountingHeight";

    // Raceway
    pub const RACEWAY_MATERIAL: &str = "racewayMaterial";
    pub const RACEWAY_SCHEDULE: &str = "racewaySchedule";
    pub const TRADE_SIZE: &str = "tradeSize";

    // Cable
    pub const CABLE_TYPE: &str = "cableType";
    pub const CONDUCTOR_COUNT: &str = "conductorCount";
    pub const CONDUCTOR_SIZE: &str = "conductorSize";
    pub const CONDUCTOR_MATERIAL: &str = "conductorMaterial";

    // Conductors in raceway
    pub const INSULATION: &str = "insulation";
    pub const GROUND_SIZE: &str = "groundSize";

    // Spec / submittal
    pub const OR_EQUAL: &str = "orEqual";
    pub const CCT: &str = "cct";
    pub const LUMENS: &str = "lumens";
    pub const WATTAGE: &str = "wattage";
    pub const SIZE: &str = "size";
    pub const WEIGHT: &str = "weight";
    pub const DIMENSIONS: &str = "dimensions";
    pub const REVIEW_STATUS: &str = "reviewStatus";
    pub const SUBMITTAL_SOURCE: &str = "submittalSource";
}

// ── BIM type ────────────────────────────────────────────────────────────────

/// A shared BIM element type. Every entity with the same `mark` shares these spec values.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BimType {
    /// The type mark (e.g. "H", "A", "J2").
    pub mark: String,
    /// Category of element (e.g. "luminaire", "receptacle", "conduit", "duct").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// Spec fields, keyed by the well-known names in [`keys`] or any custom key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, SpecField>,
}

impl BimType {
    pub fn new(mark: impl Into<String>) -> Self {
        Self { mark: mark.into(), category: String::new(), fields: BTreeMap::new() }
    }

    /// Set or update one field.
    pub fn set_field(&mut self, key: impl Into<String>, field: SpecField) {
        let k = key.into();
        if self.fields.len() < MAX_FIELDS || self.fields.contains_key(&k) {
            self.fields.insert(k, field);
        }
    }

    /// Get one field's value.
    pub fn get_field(&self, key: &str) -> Option<&SpecField> {
        self.fields.get(key)
    }

    /// Remove a field.
    pub fn remove_field(&mut self, key: &str) -> Option<SpecField> {
        self.fields.remove(key)
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

// ── BIM types collection ────────────────────────────────────────────────────

/// The drawing's BIM type definitions, keyed by mark (case-sensitive).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BimTypes {
    #[serde(default)]
    pub types: Vec<BimType>,
}

impl BimTypes {
    pub fn new() -> Self {
        Self { types: Vec::new() }
    }

    /// Find a type by mark (case-insensitive).
    pub fn get(&self, mark: &str) -> Option<&BimType> {
        self.types.iter().find(|t| t.mark.eq_ignore_ascii_case(mark))
    }

    /// Find a type by mark (case-insensitive, mutable).
    pub fn get_mut(&mut self, mark: &str) -> Option<&mut BimType> {
        self.types.iter_mut().find(|t| t.mark.eq_ignore_ascii_case(mark))
    }

    /// Insert or replace a type. Returns the old one if replaced.
    pub fn set(&mut self, ty: BimType) -> Option<BimType> {
        if let Some(existing) = self.types.iter_mut().find(|t| t.mark.eq_ignore_ascii_case(&ty.mark)) {
            let old = std::mem::replace(existing, ty);
            Some(old)
        } else if self.types.len() < MAX_BIM_TYPES {
            self.types.push(ty);
            None
        } else {
            None
        }
    }

    /// Remove a type by mark.
    pub fn remove(&mut self, mark: &str) -> Option<BimType> {
        if let Some(pos) = self.types.iter().position(|t| t.mark.eq_ignore_ascii_case(mark)) {
            Some(self.types.remove(pos))
        } else {
            None
        }
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn len(&self) -> usize {
        self.types.len()
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

// ── spec proposal (from spec_import) ───────────────────────────────────────

/// A proposed BIM type extracted from a spec book, schedule or submittal, not yet applied to the
/// drawing. The agent reviews these and calls `bim.spec_accept` to merge chosen ones.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecProposal {
    /// The type to create or merge.
    #[serde(flatten)]
    pub bim_type: BimType,
    /// Where this was read from: "spec", "schedule", "submittal", "general_notes".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
    /// File or sheet that was the source (e.g. "Project Specs.pdf p.42").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file_source: String,
    /// Wiring-method or location rules extracted alongside this type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub location_rules: Vec<LocationRule>,
}

/// A wiring-method or location rule extracted from spec text or general notes: "MC cable
/// permitted concealed in walls above accessible ceilings", "EMT exposed", "PVC below slab".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationRule {
    /// The rule text as stated in the source.
    pub rule: String,
    /// Where it applies (e.g. "concealed in walls above accessible ceilings", "exposed").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
    /// Material or method (e.g. "MC cable", "EMT", "PVC").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub material: String,
    /// Source citation (e.g. "spec 26 05 33 §2.1", "general note 5 on E-1").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

// ── RFI items ──────────────────────────────────────────────────────────────

/// Maximum RFI items per import.
pub const MAX_RFI_ITEMS: usize = 10_000;

/// A disagreement found between spec, schedule and/or submittal that needs resolution.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RfiItem {
    /// Sequential ID within the import ("RFI-001").
    pub id: String,
    /// The type mark or element this concerns.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mark: String,
    /// Field that disagrees (e.g. "cct", "catalogNumber", "manufacturer").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub field: String,
    /// What the spec says.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spec_value: String,
    /// What the schedule says.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_value: String,
    /// What the submittal says.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub submittal_value: String,
    /// Human-readable description of the disagreement.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Source citations for each side.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// Severity: "high" (safety/code), "medium" (cost/schedule), "low" (cosmetic).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub severity: String,
}

// ── submittal record ───────────────────────────────────────────────────────

/// A submittal document parsed from a PDF, tracking its review status and what it covers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmittalRecord {
    /// Spec section it covers (e.g. "26 51 13").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spec_section: String,
    /// Type marks it covers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_marks: Vec<String>,
    /// Manufacturer from the submittal.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manufacturer: String,
    /// Exact model/catalog number with options.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub catalog_number: String,
    /// Product dimensions (for family sizing).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dimensions: String,
    /// Mounting method.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mounting: String,
    /// Finish.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub finish: String,
    /// Ratings (volts, amps, etc.) as free text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ratings: String,
    /// Review status: "approved", "approved_as_noted", "revise", "rejected", "not_reviewed".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub review_status: String,
    /// Reviewer notes (for "approved as noted").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Source file (e.g. "submittal-luminaires.pdf p.3").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file_source: String,
}

/// The result of a `bim.spec_import` call: proposed types, RFIs, rules, and submittal records.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecImportResult {
    pub proposals: Vec<SpecProposal>,
    pub rfis: Vec<RfiItem>,
    pub location_rules: Vec<LocationRule>,
    pub submittals: Vec<SubmittalRecord>,
    pub notes: Vec<String>,
}

// ── entity-level helpers ────────────────────────────────────────────────────

/// The XDATA application name for BIM spec assignment.
pub const BIM_SPEC_APP: &str = "CADADDY_BIMSPEC";

/// Read the BIM type mark from an entity's xdata.
pub fn entity_type_mark(xdata: &[crate::RawTag]) -> Option<&str> {
    let at = xdata.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(BIM_SPEC_APP))?;
    xdata.get(at + 1).filter(|t| t.code == 1000).map(|t| t.value.as_str())
}

/// Build the XDATA tags to assign a type mark to an entity.
pub fn type_mark_xdata(mark: &str) -> Vec<crate::RawTag> {
    vec![
        crate::RawTag { code: 1001, value: BIM_SPEC_APP.to_string() },
        crate::RawTag { code: 1000, value: mark.to_string() },
    ]
}

/// Remove the BIM spec xdata from a tag list, returning the rest.
pub fn strip_bim_xdata(xdata: &[crate::RawTag]) -> Vec<crate::RawTag> {
    let mut out = Vec::with_capacity(xdata.len());
    let mut skip = false;
    for t in xdata {
        if t.code == 1001 {
            skip = t.value.trim().eq_ignore_ascii_case(BIM_SPEC_APP);
        }
        if !skip {
            out.push(t.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bim_type_round_trip() {
        let mut ty = BimType::new("H");
        ty.category = "luminaire".into();
        ty.set_field(keys::DESCRIPTION, SpecField::new("recessed 2x4 LED troffer").with_source("E-1, fixture schedule row 3"));
        ty.set_field(keys::MANUFACTURER, SpecField::new("Lithonia"));
        ty.set_field(keys::CATALOG_NUMBER, SpecField::new("2GTL4 4400LM"));
        ty.set_field(keys::SPEC_SECTION, SpecField::new("26 51 13"));
        ty.set_field(keys::MOUNTING, SpecField::new("recessed"));
        ty.set_field(keys::VOLTS, SpecField::new("120/277").with_confidence(Confidence::Inferred));

        let json = ty.to_json().unwrap_or_default();
        let back = BimType::from_json(&json).unwrap_or_default();
        assert_eq!(ty, back);
        assert_eq!(back.get_field(keys::VOLTS).map(|f| f.confidence), Some(Confidence::Inferred));
    }

    #[test]
    fn bim_types_collection() {
        let mut types = BimTypes::new();
        let h = BimType::new("H");
        types.set(h.clone());
        assert_eq!(types.len(), 1);
        assert!(types.get("h").is_some(), "case-insensitive");
        assert!(types.get("H").is_some());

        let mut h2 = BimType::new("H");
        h2.category = "luminaire".into();
        types.set(h2);
        assert_eq!(types.len(), 1);
        assert_eq!(types.get("H").map(|t| t.category.as_str()), Some("luminaire"));

        types.remove("H");
        assert!(types.is_empty());
    }

    #[test]
    fn bim_types_json_round_trip() {
        let mut types = BimTypes::new();
        let mut h = BimType::new("H");
        h.set_field(keys::DESCRIPTION, SpecField::new("troffer"));
        types.set(h);
        let json = types.to_json().unwrap_or_default();
        let back = BimTypes::from_json(&json).unwrap_or_default();
        assert_eq!(types, back);
    }

    #[test]
    fn entity_xdata_round_trip() {
        let tags = type_mark_xdata("H");
        assert_eq!(entity_type_mark(&tags), Some("H"));
        let stripped = strip_bim_xdata(&tags);
        assert!(stripped.is_empty());
        assert_eq!(entity_type_mark(&stripped), None);
    }

    #[test]
    fn xdata_preserves_other_apps() {
        use crate::RawTag;
        let mut tags = vec![
            RawTag { code: 1001, value: "OTHER_APP".into() },
            RawTag { code: 1000, value: "some data".into() },
        ];
        tags.extend(type_mark_xdata("J2"));
        tags.push(RawTag { code: 1001, value: "ANOTHER".into() });
        tags.push(RawTag { code: 1000, value: "more".into() });
        let stripped = strip_bim_xdata(&tags);
        assert_eq!(stripped.len(), 4); // OTHER_APP + its data + ANOTHER + its data
        assert_eq!(entity_type_mark(&stripped), None);
    }

    #[test]
    fn confidence_serde() {
        let field = SpecField::new("PVC Sch 40").with_confidence(Confidence::Assumed);
        let json = serde_json::to_string(&field).unwrap_or_default();
        assert!(json.contains("\"assumed\""));
        let back: SpecField = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(back.confidence, Confidence::Assumed);

        // "stated" is the default and skipped in serialisation
        let stated = SpecField::new("test");
        let json2 = serde_json::to_string(&stated).unwrap_or_default();
        assert!(!json2.contains("confidence"), "stated is default, omitted");
    }

    #[test]
    fn max_fields_cap() {
        let mut ty = BimType::new("X");
        for i in 0..MAX_FIELDS + 10 {
            ty.set_field(format!("field{i}"), SpecField::new("v"));
        }
        assert_eq!(ty.fields.len(), MAX_FIELDS);
    }
}
