//! Handover: COBie and schedules. COBie (Construction Operations Building information exchange,
//! the open spreadsheet standard, version 2.4's sheets and columns) from the model's storeys,
//! spaces, elements, types, systems and properties, with the BCF issues as its Issue sheet; and
//! room, door and equipment schedules. Both as XLSX workbooks.
//!
//! - `cobie.export {path, author?, company?, project?, facility?, category?, phase?, currency?,
//!   created_on?, zones?}`: the COBie workbook, checked by [`validate`] before it is written.
//! - `cobie.validate {path}`: a COBie workbook (ours or another program's) checked against the
//!   column rules below: required sheets and columns, required values, unique names, references
//!   between sheets (floors, types, spaces, components, contacts), dates, numbers, pick lists.
//! - `handover.schedules {path, schedules?}`: the room, door and equipment schedules.
//!
//! The model is the drawing's IFC objects (an IFC model opened with `ifc.open`); a model CADaddy
//! built (WALLGEN, `model.build`) goes through its IFC export first, so both read the same way.
//! Lengths are metres, areas square metres.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec3;
use serde_json::{Value, json};

use super::ifcmodel::{self, Element};
use super::*;
use crate::{Result, Session};

/// Most rows one sheet holds (Excel's own limit, less the header).
const MAX_ROWS: usize = 1_048_575;
/// Most problems a validation lists (all are counted).
const MAX_PROBLEMS: usize = 500;
/// Most attribute rows written.
const MAX_ATTRIBUTES: usize = 500_000;
/// The value COBie writes where something doesn't apply or isn't known.
const NA: &str = "n/a";

// ---------------- COBie 2.4's sheets and columns ----------------

pub const CONTACT: &[&str] = &[
    "Email",
    "CreatedBy",
    "CreatedOn",
    "Category",
    "Company",
    "Phone",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "Department",
    "OrganizationCode",
    "GivenName",
    "FamilyName",
    "Street",
    "PostalBox",
    "Town",
    "StateRegion",
    "PostalCode",
    "Country",
];
pub const FACILITY: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "Category",
    "ProjectName",
    "SiteName",
    "LinearUnits",
    "AreaUnits",
    "VolumeUnits",
    "CurrencyUnit",
    "AreaMeasurement",
    "ExternalSystem",
    "ExternalProjectObject",
    "ExternalProjectIdentifier",
    "ExternalSiteObject",
    "ExternalSiteIdentifier",
    "ExternalFacilityObject",
    "ExternalFacilityIdentifier",
    "Description",
    "ProjectDescription",
    "SiteDescription",
    "Phase",
];
pub const FLOOR: &[&str] =
    &["Name", "CreatedBy", "CreatedOn", "Category", "ExtSystem", "ExtObject", "ExtIdentifier", "Description", "Elevation", "Height"];
pub const SPACE: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "Category",
    "FloorName",
    "Description",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "RoomTag",
    "UsableHeight",
    "GrossArea",
    "NetArea",
];
pub const ZONE: &[&str] = &["Name", "CreatedBy", "CreatedOn", "Category", "SpaceNames", "ExtSystem", "ExtObject", "ExtIdentifier", "Description"];
pub const TYPE: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "Category",
    "Description",
    "AssetType",
    "Manufacturer",
    "ModelNumber",
    "WarrantyGuarantorParts",
    "WarrantyDurationParts",
    "WarrantyGuarantorLabor",
    "WarrantyDurationLabor",
    "WarrantyDurationUnit",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "ReplacementCost",
    "ExpectedLife",
    "DurationUnit",
    "WarrantyDescription",
    "NominalLength",
    "NominalWidth",
    "NominalHeight",
    "ModelReference",
    "Shape",
    "Size",
    "Color",
    "Finish",
    "Grade",
    "Material",
    "Constituents",
    "Features",
    "AccessibilityPerformance",
    "CodePerformance",
    "SustainabilityPerformance",
];
pub const COMPONENT: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "TypeName",
    "Space",
    "Description",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "SerialNumber",
    "InstallationDate",
    "WarrantyStartDate",
    "TagNumber",
    "BarCode",
    "AssetIdentifier",
];
pub const SYSTEM: &[&str] =
    &["Name", "CreatedBy", "CreatedOn", "Category", "ComponentNames", "ExtSystem", "ExtObject", "ExtIdentifier", "Description"];
pub const ASSEMBLY: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "SheetName",
    "ParentName",
    "ChildNames",
    "AssemblyType",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "Description",
];
pub const CONNECTION: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "ConnectionType",
    "SheetName",
    "RowName1",
    "RowName2",
    "RealizingElement",
    "PortName1",
    "PortName2",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "Description",
];
pub const SPARE: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "Category",
    "TypeName",
    "Suppliers",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "Description",
    "SetNumber",
    "PartNumber",
];
pub const RESOURCE: &[&str] = &["Name", "CreatedBy", "CreatedOn", "Category", "ExtSystem", "ExtObject", "ExtIdentifier", "Description"];
pub const JOB: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "Category",
    "Status",
    "TypeName",
    "Description",
    "Duration",
    "DurationUnit",
    "Start",
    "TaskStartUnit",
    "Frequency",
    "FrequencyUnit",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "TaskNumber",
    "Priors",
    "ResourceNames",
];
pub const IMPACT: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "ImpactType",
    "ImpactStage",
    "SheetName",
    "RowName",
    "Value",
    "ImpactUnit",
    "LeadInTime",
    "Duration",
    "LeadOutTime",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "Description",
];
pub const DOCUMENT: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "Category",
    "ApprovalBy",
    "Stage",
    "SheetName",
    "RowName",
    "Directory",
    "File",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "Description",
    "Reference",
];
pub const ATTRIBUTE: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "Category",
    "SheetName",
    "RowName",
    "Value",
    "Unit",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "Description",
    "AllowedValues",
];
pub const COORDINATE: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "Category",
    "SheetName",
    "RowName",
    "CoordinateXAxis",
    "CoordinateYAxis",
    "CoordinateZAxis",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
    "ClockwiseRotation",
    "ElevationalRotation",
    "YawRotation",
];
pub const ISSUE: &[&str] = &[
    "Name",
    "CreatedBy",
    "CreatedOn",
    "Type",
    "Risk",
    "Chance",
    "Impact",
    "SheetName1",
    "RowName1",
    "SheetName2",
    "RowName2",
    "Description",
    "Owner",
    "Mitigation",
    "ExtSystem",
    "ExtObject",
    "ExtIdentifier",
];

/// The sheets in COBie's order, with their columns.
pub const SHEETS: &[(&str, &[&str])] = &[
    ("Contact", CONTACT),
    ("Facility", FACILITY),
    ("Floor", FLOOR),
    ("Space", SPACE),
    ("Zone", ZONE),
    ("Type", TYPE),
    ("Component", COMPONENT),
    ("System", SYSTEM),
    ("Assembly", ASSEMBLY),
    ("Connection", CONNECTION),
    ("Spare", SPARE),
    ("Resource", RESOURCE),
    ("Job", JOB),
    ("Impact", IMPACT),
    ("Document", DOCUMENT),
    ("Attribute", ATTRIBUTE),
    ("Coordinate", COORDINATE),
    ("Issue", ISSUE),
];

/// Sheets a COBie file must have (the others may be left out).
const REQUIRED_SHEETS: &[&str] = &["Contact", "Facility", "Floor", "Space", "Type", "Component"];

/// The columns that must hold a value (`n/a` counts) in each sheet.
fn required(sheet: &str) -> &'static [&'static str] {
    match sheet {
        "Contact" => &["Email", "CreatedBy", "CreatedOn", "Category", "Company", "Phone"],
        "Facility" => &[
            "Name",
            "CreatedBy",
            "CreatedOn",
            "Category",
            "ProjectName",
            "SiteName",
            "LinearUnits",
            "AreaUnits",
            "VolumeUnits",
            "CurrencyUnit",
            "AreaMeasurement",
        ],
        "Floor" => &["Name", "CreatedBy", "CreatedOn", "Category"],
        "Space" => &["Name", "CreatedBy", "CreatedOn", "Category", "FloorName", "Description"],
        "Zone" => &["Name", "CreatedBy", "CreatedOn", "Category", "SpaceNames"],
        "Type" => &["Name", "CreatedBy", "CreatedOn", "Category", "Description", "AssetType"],
        "Component" => &["Name", "CreatedBy", "CreatedOn", "TypeName", "Space", "Description"],
        "System" => &["Name", "CreatedBy", "CreatedOn", "Category", "ComponentNames"],
        "Attribute" => &["Name", "CreatedBy", "CreatedOn", "Category", "SheetName", "RowName", "Value"],
        "Coordinate" => {
            &["Name", "CreatedBy", "CreatedOn", "Category", "SheetName", "RowName", "CoordinateXAxis", "CoordinateYAxis", "CoordinateZAxis"]
        }
        "Issue" => {
            &["Name", "CreatedBy", "CreatedOn", "Type", "Risk", "Chance", "Impact", "SheetName1", "RowName1", "Description", "Owner", "Mitigation"]
        }
        _ => &["Name", "CreatedBy", "CreatedOn"],
    }
}

/// Pick lists checked: (sheet, column, allowed values; compared ignoring case).
const PICK_LISTS: &[(&str, &str, &[&str])] = &[
    ("Floor", "Category", &["Site", "Floor", "Roof"]),
    ("Type", "AssetType", &["Fixed", "Moveable", NA]),
    ("Coordinate", "Category", &["point", "line-end-one", "line-end-two", "box-lowerleft", "box-upperright"]),
    ("Facility", "LinearUnits", &["millimeters", "centimeters", "meters", "inches", "feet"]),
    ("Facility", "AreaUnits", &["square millimeters", "square centimeters", "square meters", "square inches", "square feet"]),
    ("Facility", "VolumeUnits", &["cubic millimeters", "cubic centimeters", "cubic meters", "cubic inches", "cubic feet"]),
];

/// Number columns (a number or `n/a`).
const NUMBERS: &[(&str, &str)] = &[
    ("Floor", "Elevation"),
    ("Floor", "Height"),
    ("Space", "UsableHeight"),
    ("Space", "GrossArea"),
    ("Space", "NetArea"),
    ("Type", "NominalLength"),
    ("Type", "NominalWidth"),
    ("Type", "NominalHeight"),
    ("Coordinate", "CoordinateXAxis"),
    ("Coordinate", "CoordinateYAxis"),
    ("Coordinate", "CoordinateZAxis"),
];

// ---------------- a workbook of sheets ----------------

/// A workbook: sheets of rows, each sheet's first row its column names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Book {
    pub sheets: Vec<(String, Vec<Vec<String>>)>,
}

impl Book {
    pub fn sheet(&self, name: &str) -> Option<&Vec<Vec<String>>> {
        self.sheets.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, r)| r)
    }

    pub fn sheet_mut(&mut self, name: &str) -> Option<&mut Vec<Vec<String>>> {
        self.sheets.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, r)| r)
    }

    /// Column `col` of a sheet's data rows (after the header), with their row numbers as Excel
    /// shows them.
    pub fn column(&self, sheet: &str, col: &str) -> Vec<(usize, String)> {
        let Some(rows) = self.sheet(sheet) else { return Vec::new() };
        let Some(i) = rows.first().and_then(|h| h.iter().position(|c| c.trim().eq_ignore_ascii_case(col))) else { return Vec::new() };
        rows.iter().enumerate().skip(1).map(|(r, row)| (r + 1, row.get(i).map(|v| v.trim().to_string()).unwrap_or_default())).collect()
    }

    /// The value of `col` in the data row whose `Name` is `name`.
    pub fn value(&self, sheet: &str, name: &str, col: &str) -> Option<String> {
        let names = self.column(sheet, if sheet == "Contact" { "Email" } else { "Name" });
        let at = names.iter().find(|(_, n)| n == name)?.0;
        self.column(sheet, col).into_iter().find(|(r, _)| *r == at).map(|(_, v)| v)
    }

    /// The workbook as .xlsx bytes, each sheet's first row a header.
    pub fn write(&self) -> std::result::Result<Vec<u8>, String> {
        super::table::xlsx::write_sheets_with_headers(&self.sheets)
    }

    /// Read an .xlsx workbook.
    pub fn read(bytes: &[u8]) -> std::result::Result<Book, String> {
        let wb = super::table::xlsx::open(bytes)?;
        let mut book = Book::default();
        for (name, _) in &wb.sheets {
            book.sheets.push((name.clone(), wb.cells(Some(name), None)?));
        }
        Ok(book)
    }
}

// ---------------- the model ----------------

/// The drawing's IFC objects; a model CADaddy built goes through its IFC export first.
fn model(cmd: &str, s: &Session) -> Result<(Drawing, &'static str)> {
    let d = s.doc()?;
    if !ifcmodel::elements(d).is_empty() {
        return Ok((d.clone(), "the IFC model"));
    }
    let hooks = super::file::io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
    let bytes = (hooks.write)(d, "handover.ifc").map_err(|e| bad(cmd, format!("nothing to hand over: {e}")))?;
    let m = (hooks.read)(&bytes, "handover.ifc").map_err(|e| bad(cmd, format!("the model's IFC couldn't be read back: {e}")))?;
    Ok((m, "the built model (through its IFC export)"))
}

/// Building fabric: not maintained assets, so not COBie components (unless they are in a system).
const FABRIC: &[&str] = &[
    "wall",
    "wallstandardcase",
    "slab",
    "roof",
    "covering",
    "beam",
    "column",
    "member",
    "plate",
    "footing",
    "pile",
    "railing",
    "stair",
    "stairflight",
    "ramp",
    "rampflight",
    "curtainwall",
    "openingelement",
    "space",
    "annotation",
    "grid",
    "site",
    "zone",
    "spatialzone",
    "buildingelementpart",
    "buildingelementproxy",
    "chimney",
    "shadingdevice",
];

fn is_component(e: &Element) -> bool {
    let c = e.tag.class.to_ascii_lowercase();
    if c == "space" {
        return false;
    }
    !e.tag.systems.is_empty() || !(FABRIC.contains(&c.as_str()) || c.ends_with("segment") || c.ends_with("fitting"))
}

fn is_moveable(class: &str) -> bool {
    let c = class.to_ascii_lowercase();
    c.contains("furnish") || c.contains("furniture")
}

/// A property by the end of its name (`Manufacturer` finds `Pset_ManufacturerTypeInformation.Manufacturer`).
fn prop<'a>(e: &'a Element, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|n| {
        e.tag
            .props
            .iter()
            .find(|(k, v)| !v.trim().is_empty() && k.rsplit('.').next().is_some_and(|last| last.eq_ignore_ascii_case(n)))
            .map(|(_, v)| v.trim())
    })
}

fn is_quantity_set(set: &str) -> bool {
    set.starts_with("Qto_") || set.contains("Quantities")
}

fn num(v: f64) -> String {
    if !v.is_finite() {
        return NA.into();
    }
    let r = (v * 1000.0).round() / 1000.0;
    let r = if r == 0.0 { 0.0 } else { r };
    format!("{r}")
}

/// The area of an object's footprint (its lowest level faces), in the drawing's units squared.
fn footprint_area(d: &Drawing, e: &Element) -> f64 {
    let Some(ent) = d.entity(e.handle) else { return 0.0 };
    let EntityKind::Mesh(m) = &ent.kind else { return 0.0 };
    let mut area = 0.0;
    for f in &m.faces {
        let vs: Vec<Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize).copied()).collect();
        if vs.len() < 3 || !vs.iter().all(|v| (v.z - e.min.z).abs() < 1e-6) {
            continue;
        }
        let a: f64 = vs.iter().zip(vs.iter().cycle().skip(1)).map(|(p, q)| p.x * q.y - q.x * p.y).sum::<f64>() / 2.0;
        if a.is_finite() {
            area += a.abs();
        }
    }
    area
}

/// Names made unique: a repeat gets " (2)", " (3)"…
#[derive(Default)]
struct Names {
    used: HashMap<String, usize>,
}

impl Names {
    fn take(&mut self, base: &str) -> String {
        let base = if base.trim().is_empty() { "Unnamed".to_string() } else { base.trim().to_string() };
        let n = self.used.entry(base.to_lowercase()).or_insert(0);
        *n += 1;
        if *n == 1 {
            return base;
        }
        let mut k = *n;
        loop {
            let name = format!("{base} ({k})");
            if let std::collections::hash_map::Entry::Vacant(v) = self.used.entry(name.to_lowercase()) {
                v.insert(1);
                return name;
            }
            k += 1;
        }
    }
}

fn is_email(s: &str) -> bool {
    let s = s.trim();
    s.split_once('@')
        .is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.starts_with('.') && !b.ends_with('.') && !s.contains(char::is_whitespace))
}

/// COBie's date form: `yyyy-mm-ddThh:mm:ss`.
fn is_cobie_date(s: &str) -> bool {
    let b = s.trim().as_bytes();
    b.len() >= 19
        && b.iter().take(19).enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            _ => c.is_ascii_digit(),
        })
}

/// What the export is told: who, for what project, when.
struct Opts {
    author: String,
    company: String,
    project: String,
    facility: String,
    category: String,
    phase: String,
    currency: String,
    created_on: String,
    zones: Vec<(String, String, Vec<String>)>,
}

impl Opts {
    fn from(cmd: &str, p: &Value) -> Result<Opts> {
        let t = |k: &str| str_param(p, k).map(|v| v.trim().chars().take(500).collect::<String>()).filter(|v| !v.is_empty());
        let author = t("author").unwrap_or_else(|| "handover@example.com".into());
        if !is_email(&author) {
            return Err(bad(cmd, "author must be an email address (COBie keys contacts by email)"));
        }
        let created_on = match t("created_on") {
            Some(d) if is_cobie_date(&d) => d.chars().take(19).collect(),
            Some(_) => return Err(bad(cmd, "created_on must be yyyy-mm-ddThh:mm:ss")),
            None => super::bcf::now().chars().take(19).collect(),
        };
        let zones = p
            .get("zones")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .take(10_000)
                    .filter_map(|z| {
                        let name = z.get("name")?.as_str()?.trim().to_string();
                        let cat = z.get("category").and_then(Value::as_str).unwrap_or(NA).to_string();
                        let spaces = z.get("spaces")?.as_array()?.iter().filter_map(Value::as_str).map(str::to_string).take(100_000).collect();
                        (!name.is_empty()).then_some((name, cat, spaces))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Opts {
            author,
            company: t("company").unwrap_or_else(|| NA.into()),
            project: t("project").unwrap_or_default(),
            facility: t("facility").unwrap_or_default(),
            category: t("category").unwrap_or_else(|| NA.into()),
            phase: t("phase").unwrap_or_else(|| NA.into()),
            currency: t("currency").unwrap_or_else(|| NA.into()),
            created_on,
            zones,
        })
    }
}

/// One row of `cols`, from (column, value) pairs; the rest `n/a`.
fn row(cols: &[&str], vals: &[(&str, String)]) -> Vec<String> {
    cols.iter()
        .map(|c| {
            vals.iter().find(|(k, _)| k == c).map_or_else(|| NA.to_string(), |(_, v)| if v.trim().is_empty() { NA.to_string() } else { v.clone() })
        })
        .collect()
}

/// The model's storeys, spaces and components, as the schedules and COBie see them.
struct Model<'a> {
    d: &'a Drawing,
    els: Vec<Element>,
    upm: f64,
    /// Storeys bottom up: name, elevation (m).
    storeys: Vec<(String, f64)>,
    /// Spaces: element index, unique name.
    spaces: Vec<(usize, String)>,
    /// Components: element index, unique name.
    components: Vec<(usize, String)>,
}

impl<'a> Model<'a> {
    fn new(d: &'a Drawing) -> Model<'a> {
        let els = ifcmodel::elements(d);
        let upm = super::ifcref::units_per_metre(d);
        let mut storeys: Vec<(String, f64)> = Vec::new();
        for e in &els {
            if let Some(l) = &e.tag.level
                && !storeys.iter().any(|(n, _)| n == l)
            {
                storeys.push((l.clone(), e.tag.elevation.unwrap_or(e.min.z / upm)));
            }
        }
        storeys.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut names = Names::default();
        let spaces: Vec<(usize, String)> =
            els.iter().enumerate().filter(|(_, e)| e.tag.class.eq_ignore_ascii_case("Space")).map(|(i, e)| (i, names.take(&e.tag.name))).collect();
        let mut names = Names::default();
        let components: Vec<(usize, String)> = els
            .iter()
            .enumerate()
            .filter(|(_, e)| is_component(e))
            .map(|(i, e)| {
                let base = if e.tag.name.trim().is_empty() { e.tag.class.clone() } else { e.tag.name.clone() };
                (i, names.take(&base))
            })
            .collect();
        Model { d, els, upm, storeys, spaces, components }
    }

    fn m(&self, v: f64) -> f64 {
        v / self.upm
    }

    fn el(&self, i: usize) -> Option<&Element> {
        self.els.get(i)
    }

    /// The space an object is in: its tag's, else the smallest space on its storey whose box
    /// holds its centre.
    fn space_of(&self, e: &Element) -> Option<String> {
        if let Some(n) = &e.tag.space
            && let Some((_, name)) = self.spaces.iter().find(|(i, _)| self.el(*i).is_some_and(|s| &s.tag.name == n))
        {
            return Some(name.clone());
        }
        let c = (e.min + e.max) * 0.5;
        let tol = 0.01 * self.upm;
        self.spaces
            .iter()
            .filter_map(|(i, n)| self.el(*i).map(|s| (s, n)))
            .filter(|(s, _)| e.tag.level.is_none() || s.tag.level == e.tag.level)
            .filter(|(s, _)| {
                c.x >= s.min.x - tol
                    && c.x <= s.max.x + tol
                    && c.y >= s.min.y - tol
                    && c.y <= s.max.y + tol
                    && c.z >= s.min.z - tol
                    && c.z <= s.max.z + tol
            })
            .min_by(|a, b| ((a.0.max.x - a.0.min.x) * (a.0.max.y - a.0.min.y)).total_cmp(&((b.0.max.x - b.0.min.x) * (b.0.max.y - b.0.min.y))))
            .map(|(_, n)| n.clone())
    }

    /// The spaces a door (or anything) touches: those whose boxes meet its own (on its storey).
    fn spaces_touching(&self, e: &Element) -> Vec<String> {
        let tol = 0.05 * self.upm;
        self.spaces
            .iter()
            .filter_map(|(i, n)| self.el(*i).map(|s| (s, n)))
            .filter(|(s, _)| e.tag.level.is_none() || s.tag.level == e.tag.level)
            .filter(|(s, _)| {
                e.min.x <= s.max.x + tol
                    && s.min.x <= e.max.x + tol
                    && e.min.y <= s.max.y + tol
                    && s.min.y <= e.max.y + tol
                    && e.min.z <= s.max.z
                    && s.min.z <= e.max.z
            })
            .map(|(_, n)| n.clone())
            .collect()
    }

    fn type_name(e: &Element) -> String {
        e.tag.ty.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| e.tag.class.clone())
    }

    fn area(&self, e: &Element) -> f64 {
        prop(e, &["GrossFloorArea", "NetFloorArea"])
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or_else(|| footprint_area(self.d, e) / (self.upm * self.upm))
    }
}

// ---------------- COBie ----------------

/// The COBie workbook of the model in `d`, its BCF topics as issues.
fn build(d: &Drawing, topics: &super::bcf::Topics, o: &Opts, drawing_name: &str) -> Book {
    let m = Model::new(d);
    let by = o.author.clone();
    let on = o.created_on.clone();
    let base = |name: String| -> Vec<(&'static str, String)> { vec![("Name", name), ("CreatedBy", by.clone()), ("CreatedOn", on.clone())] };
    let mut book = Book::default();
    let mut sheet = |name: &str, cols: &[&str], rows: Vec<Vec<String>>| {
        let mut all = vec![cols.iter().map(|c| c.to_string()).collect::<Vec<_>>()];
        all.extend(rows.into_iter().take(MAX_ROWS));
        book.sheets.push((name.to_string(), all));
    };
    // Contacts: the author, and everyone the issues name by email.
    let mut contacts: BTreeSet<String> = BTreeSet::new();
    contacts.insert(o.author.clone());
    for t in &topics.topics {
        for who in [&t.author, &t.assignee] {
            if is_email(who) {
                contacts.insert(who.trim().to_string());
            }
        }
    }
    let contact_rows = contacts
        .iter()
        .map(|e| {
            let company = if *e == o.author { o.company.clone() } else { NA.into() };
            row(
                CONTACT,
                &[("Email", e.clone()), ("CreatedBy", by.clone()), ("CreatedOn", on.clone()), ("Company", company), ("ExtSystem", "CADaddy".into())],
            )
        })
        .collect();
    sheet("Contact", CONTACT, contact_rows);
    // Facility.
    let building = m.els.iter().find_map(|e| e.tag.building.clone());
    let site = m.els.iter().find_map(|e| e.tag.site.clone());
    let fname = if !o.facility.is_empty() {
        o.facility.clone()
    } else {
        building.clone().unwrap_or_else(|| if drawing_name.is_empty() { "Facility".into() } else { drawing_name.to_string() })
    };
    let mut f = base(fname.clone());
    f.extend([
        ("Category", o.category.clone()),
        ("ProjectName", if o.project.is_empty() { fname.clone() } else { o.project.clone() }),
        ("SiteName", site.unwrap_or_else(|| NA.into())),
        ("LinearUnits", "meters".into()),
        ("AreaUnits", "square meters".into()),
        ("VolumeUnits", "cubic meters".into()),
        ("CurrencyUnit", o.currency.clone()),
        ("ExternalSystem", "CADaddy".into()),
        ("ExternalProjectObject", "IfcProject".into()),
        ("ExternalSiteObject", "IfcSite".into()),
        ("ExternalFacilityObject", "IfcBuilding".into()),
        ("Description", fname.clone()),
        ("Phase", o.phase.clone()),
    ]);
    sheet("Facility", FACILITY, vec![row(FACILITY, &f)]);
    // Floors.
    let floors = m
        .storeys
        .iter()
        .enumerate()
        .map(|(i, (n, z))| {
            let mut v = base(n.clone());
            let height = m.storeys.get(i + 1).map_or_else(|| NA.to_string(), |(_, nz)| num(nz - z));
            v.extend([
                ("Category", "Floor".into()),
                ("ExtSystem", "CADaddy".into()),
                ("ExtObject", "IfcBuildingStorey".into()),
                ("Description", n.clone()),
                ("Elevation", num(*z)),
                ("Height", height),
            ]);
            row(FLOOR, &v)
        })
        .collect();
    sheet("Floor", FLOOR, floors);
    // Spaces.
    let spaces = m
        .spaces
        .iter()
        .filter_map(|(i, name)| m.el(*i).map(|e| (e, name)))
        .map(|(e, name)| {
            let mut v = base(name.clone());
            let gross = m.area(e);
            let net = prop(e, &["NetFloorArea"]).and_then(|x| x.parse::<f64>().ok()).filter(|x| x.is_finite()).unwrap_or(gross);
            v.extend([
                ("Category", prop(e, &["Category", "OccupancyType"]).unwrap_or(NA).to_string()),
                ("FloorName", e.tag.level.clone().unwrap_or_else(|| NA.into())),
                ("Description", prop(e, &["Reference", "LongName"]).map_or_else(|| name.clone(), str::to_string)),
                ("ExtSystem", "CADaddy".into()),
                ("ExtObject", "IfcSpace".into()),
                ("ExtIdentifier", e.tag.guid.clone()),
                ("RoomTag", e.tag.name.clone()),
                ("UsableHeight", num(m.m(e.max.z - e.min.z))),
                ("GrossArea", num(gross)),
                ("NetArea", num(net)),
            ]);
            row(SPACE, &v)
        })
        .collect();
    sheet("Space", SPACE, spaces);
    // Zones (as told).
    let zones = o
        .zones
        .iter()
        .map(|(n, cat, sp)| {
            let mut v = base(n.clone());
            v.extend([("Category", cat.clone()), ("SpaceNames", sp.join(",")), ("ExtSystem", "CADaddy".into()), ("Description", n.clone())]);
            row(ZONE, &v)
        })
        .collect();
    sheet("Zone", ZONE, zones);
    // Types: one per type name the components use, its first component's facts.
    let mut types: BTreeMap<String, &Element> = BTreeMap::new();
    for (i, _) in &m.components {
        if let Some(e) = m.el(*i) {
            types.entry(Model::type_name(e)).or_insert(e);
        }
    }
    let type_rows = types
        .iter()
        .map(|(n, e)| {
            let mut v = base(n.clone());
            let size = e.max - e.min;
            v.extend([
                ("Category", prop(e, &["OmniClass", "Category"]).unwrap_or(NA).to_string()),
                ("Description", if *n == e.tag.class { n.clone() } else { format!("{} {n}", e.tag.class) }),
                ("AssetType", if is_moveable(&e.tag.class) { "Moveable" } else { "Fixed" }.into()),
                ("Manufacturer", prop(e, &["Manufacturer"]).unwrap_or(NA).to_string()),
                ("ModelNumber", prop(e, &["ModelLabel", "ModelNumber", "ModelReference"]).unwrap_or(NA).to_string()),
                ("WarrantyDurationUnit", "year".into()),
                ("ExtSystem", "CADaddy".into()),
                ("ExtObject", format!("Ifc{}Type", e.tag.class)),
                ("DurationUnit", "year".into()),
                ("NominalLength", num(m.m(size.x))),
                ("NominalWidth", num(m.m(size.y))),
                ("NominalHeight", num(m.m(size.z))),
                ("ModelReference", prop(e, &["ModelReference"]).unwrap_or(NA).to_string()),
                ("Color", prop(e, &["Color", "Colour"]).unwrap_or(NA).to_string()),
                ("Material", prop(e, &["Material"]).unwrap_or(NA).to_string()),
                ("CodePerformance", prop(e, &["FireRating"]).unwrap_or(NA).to_string()),
            ]);
            row(TYPE, &v)
        })
        .collect();
    sheet("Type", TYPE, type_rows);
    // Components.
    let mut guid_names: HashMap<String, (&str, String)> = HashMap::new();
    let comp_rows = m
        .components
        .iter()
        .filter_map(|(i, n)| m.el(*i).map(|e| (e, n)))
        .map(|(e, n)| {
            guid_names.insert(e.tag.guid.clone(), ("Component", n.clone()));
            let mut v = base(n.clone());
            v.extend([
                ("TypeName", Model::type_name(e)),
                // Its room; a door, the rooms it opens between.
                ("Space", m.space_of(e).or_else(|| Some(m.spaces_touching(e).join(",")).filter(|x| !x.is_empty())).unwrap_or_else(|| NA.into())),
                ("Description", format!("{} {}", e.tag.class, e.tag.name).trim().to_string()),
                ("ExtSystem", "CADaddy".into()),
                ("ExtObject", format!("Ifc{}", e.tag.class)),
                ("ExtIdentifier", e.tag.guid.clone()),
                ("SerialNumber", prop(e, &["SerialNumber"]).unwrap_or(NA).to_string()),
                ("TagNumber", prop(e, &["Tag", "TagNumber", "Mark"]).map_or_else(|| e.tag.name.clone(), str::to_string)),
                ("BarCode", prop(e, &["BarCode"]).unwrap_or(NA).to_string()),
                ("AssetIdentifier", e.tag.guid.clone()),
            ]);
            row(COMPONENT, &v)
        })
        .collect();
    sheet("Component", COMPONENT, comp_rows);
    for (i, n) in &m.spaces {
        if let Some(e) = m.el(*i) {
            guid_names.insert(e.tag.guid.clone(), ("Space", n.clone()));
        }
    }
    // Systems.
    let mut systems: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (i, n) in &m.components {
        for sy in m.el(*i).map(|e| e.tag.systems.clone()).unwrap_or_default() {
            systems.entry(sy).or_default().push(n.clone());
        }
    }
    let sys_rows = systems
        .iter()
        .map(|(n, comps)| {
            let mut v = base(n.clone());
            v.extend([
                ("Category", NA.into()),
                ("ComponentNames", comps.join(",")),
                ("ExtSystem", "CADaddy".into()),
                ("ExtObject", "IfcSystem".into()),
                ("Description", n.clone()),
            ]);
            row(SYSTEM, &v)
        })
        .collect();
    sheet("System", SYSTEM, sys_rows);
    for (name, cols) in [
        ("Assembly", ASSEMBLY),
        ("Connection", CONNECTION),
        ("Spare", SPARE),
        ("Resource", RESOURCE),
        ("Job", JOB),
        ("Impact", IMPACT),
        ("Document", DOCUMENT),
    ] {
        sheet(name, cols, Vec::new());
    }
    // Attributes: the property sets (not the quantities) of spaces and components.
    let mut attrs = Vec::new();
    let owners = m.spaces.iter().map(|(i, n)| ("Space", *i, n)).chain(m.components.iter().map(|(i, n)| ("Component", *i, n)));
    'all: for (sheet_name, i, rn) in owners {
        let Some(e) = m.el(i) else { continue };
        for (k, val) in &e.tag.props {
            let (set, pname) = k.split_once('.').unwrap_or(("", k.as_str()));
            if is_quantity_set(set) || val.trim().is_empty() {
                continue;
            }
            if attrs.len() >= MAX_ATTRIBUTES {
                break 'all;
            }
            let mut v = base(pname.to_string());
            v.extend([
                ("Category", "Submitted".into()),
                ("SheetName", sheet_name.into()),
                ("RowName", rn.clone()),
                ("Value", val.clone()),
                ("ExtSystem", "CADaddy".into()),
                ("ExtObject", if set.is_empty() { NA.into() } else { set.to_string() }),
                ("Description", k.clone()),
            ]);
            attrs.push(row(ATTRIBUTE, &v));
        }
    }
    sheet("Attribute", ATTRIBUTE, attrs);
    // Coordinates: each space's and component's box.
    let mut coords = Vec::new();
    let owners = m.spaces.iter().map(|(i, n)| ("Space", *i, n)).chain(m.components.iter().map(|(i, n)| ("Component", *i, n)));
    for (sheet_name, i, rn) in owners {
        let Some(e) = m.el(i) else { continue };
        for (cat, p) in [("box-lowerleft", e.min), ("box-upperright", e.max)] {
            let mut v = base(rn.clone());
            v.extend([
                ("Category", cat.into()),
                ("SheetName", sheet_name.into()),
                ("RowName", rn.clone()),
                ("CoordinateXAxis", num(m.m(p.x))),
                ("CoordinateYAxis", num(m.m(p.y))),
                ("CoordinateZAxis", num(m.m(p.z))),
                ("ExtSystem", "CADaddy".into()),
                ("ExtObject", format!("Ifc{}", e.tag.class)),
                ("ExtIdentifier", e.tag.guid.clone()),
                ("ClockwiseRotation", "0".into()),
                ("ElevationalRotation", "0".into()),
                ("YawRotation", "0".into()),
            ]);
            coords.push(row(COORDINATE, &v));
        }
    }
    sheet("Coordinate", COORDINATE, coords);
    // Issues: the BCF topics.
    let mut names = Names::default();
    let issues = topics
        .topics
        .iter()
        .map(|t| {
            let who = if is_email(&t.author) { t.author.trim().to_string() } else { by.clone() };
            let when = if is_cobie_date(&t.created) { t.created.chars().take(19).collect() } else { on.clone() };
            let about = t.viewpoints.first().and_then(|v| v.selection.iter().find_map(|g| guid_names.get(g)));
            let mut desc = t.title.clone();
            if !t.description.is_empty() {
                desc = format!("{desc}. {}", t.description);
            }
            if !t.status.is_empty() {
                desc = format!("{desc} (status: {})", t.status);
            }
            row(
                ISSUE,
                &[
                    ("Name", names.take(&t.title)),
                    ("CreatedBy", who.clone()),
                    ("CreatedOn", when),
                    ("Type", if t.topic_type.is_empty() { NA.into() } else { t.topic_type.clone() }),
                    ("Risk", if t.priority.is_empty() { NA.into() } else { t.priority.clone() }),
                    ("SheetName1", about.map_or_else(|| NA.into(), |(s, _)| (*s).to_string())),
                    ("RowName1", about.map_or_else(|| NA.into(), |(_, n)| n.clone())),
                    ("Description", desc),
                    ("Owner", if is_email(&t.assignee) { t.assignee.trim().to_string() } else { who }),
                    ("Mitigation", t.comments.last().map_or_else(|| NA.into(), |c| c.text.clone())),
                    ("ExtSystem", "BCF".into()),
                    ("ExtObject", "Topic".into()),
                    ("ExtIdentifier", t.guid.clone()),
                ],
            )
        })
        .collect();
    sheet("Issue", ISSUE, issues);
    book
}

// ---------------- validation ----------------

/// One thing a COBie workbook gets wrong.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub sheet: String,
    /// The row as Excel numbers it (1: the header); 0 for the whole sheet.
    pub row: usize,
    pub column: String,
    pub rule: &'static str,
    pub message: String,
    pub error: bool,
}

impl Problem {
    fn json(&self) -> Value {
        json!({"sheet": self.sheet, "row": self.row, "column": self.column, "rule": self.rule, "message": self.message, "severity": if self.error { "error" } else { "warning" }})
    }
}

/// `Name` values of a sheet (`Email` for Contact).
fn names_of(b: &Book, sheet: &str) -> BTreeSet<String> {
    b.column(sheet, if sheet == "Contact" { "Email" } else { "Name" }).into_iter().map(|(_, v)| v).filter(|v| !v.is_empty()).collect()
}

/// A COBie workbook checked against COBie 2.4's column rules (see the module docs).
pub fn validate(b: &Book) -> Vec<Problem> {
    let mut out: Vec<Problem> = Vec::new();
    let mut add = |sheet: &str, row: usize, column: &str, rule: &'static str, message: String, error: bool| {
        out.push(Problem { sheet: sheet.into(), row, column: column.into(), rule, message, error });
    };
    for s in REQUIRED_SHEETS {
        if b.sheet(s).is_none() {
            add(s, 0, "", "sheet", format!("the {s} sheet is missing"), true);
        }
    }
    let known: BTreeMap<&str, BTreeSet<String>> = SHEETS.iter().map(|(s, _)| (*s, names_of(b, s))).collect();
    let contacts = known.get("Contact").cloned().unwrap_or_default();
    for (sheet, cols) in SHEETS {
        let Some(rows) = b.sheet(sheet) else { continue };
        // Header: COBie's columns, in order (more may follow).
        let header: Vec<String> = rows.first().map(|h| h.iter().map(|c| c.trim().to_string()).collect()).unwrap_or_default();
        for (i, c) in cols.iter().enumerate() {
            if header.get(i).is_none_or(|h| !h.eq_ignore_ascii_case(c)) {
                add(sheet, 1, c, "header", format!("column {} should be {c}", i + 1), true);
                break;
            }
        }
        // Required values.
        for c in required(sheet) {
            for (r, v) in b.column(sheet, c) {
                if v.is_empty() {
                    add(sheet, r, c, "required", format!("{c} is empty (write n/a where it doesn't apply)"), true);
                }
            }
        }
        // Unique names (attributes: per the row they describe).
        let key_col = if *sheet == "Contact" { "Email" } else { "Name" };
        let keys: Vec<(usize, String)> = if *sheet == "Attribute" || *sheet == "Coordinate" {
            let (n, sn, rn, cat) = (b.column(sheet, "Name"), b.column(sheet, "SheetName"), b.column(sheet, "RowName"), b.column(sheet, "Category"));
            n.iter()
                .map(|(r, v)| {
                    let at = |c: &Vec<(usize, String)>| c.iter().find(|(x, _)| x == r).map(|(_, v)| v.clone()).unwrap_or_default();
                    (*r, format!("{v}|{}|{}|{}", at(&sn), at(&rn), if *sheet == "Coordinate" { at(&cat) } else { String::new() }))
                })
                .collect()
        } else {
            b.column(sheet, key_col)
        };
        let mut seen: HashMap<String, usize> = HashMap::new();
        for (r, k) in keys {
            if k.is_empty() || k.eq_ignore_ascii_case(NA) {
                continue;
            }
            if let Some(first) = seen.insert(k.to_lowercase(), r) {
                add(sheet, r, key_col, "unique", format!("{} repeats row {first}", k.split('|').next().unwrap_or_default()), true);
            }
        }
        // Who and when.
        for (r, v) in b.column(sheet, "CreatedBy") {
            if !v.is_empty() && !contacts.contains(&v) {
                add(sheet, r, "CreatedBy", "contact", format!("{v} isn't on the Contact sheet"), true);
            }
        }
        for (r, v) in b.column(sheet, "CreatedOn") {
            if !v.is_empty() && !is_cobie_date(&v) {
                add(sheet, r, "CreatedOn", "date", format!("{v} isn't a date as yyyy-mm-ddThh:mm:ss"), true);
            }
        }
    }
    for (r, v) in b.column("Contact", "Email") {
        if !v.is_empty() && !is_email(&v) {
            add("Contact", r, "Email", "email", format!("{v} isn't an email address"), true);
        }
    }
    let facilities = b.column("Facility", "Name").len();
    if b.sheet("Facility").is_some() && facilities != 1 {
        add("Facility", 0, "Name", "one facility", format!("the Facility sheet holds {facilities} rows, not one"), true);
    }
    // References between sheets.
    let refs = |out: &mut Vec<Problem>, sheet: &str, col: &str, to: &str, list: bool, na_ok: bool| {
        let targets = known.get(to).cloned().unwrap_or_default();
        for (r, v) in b.column(sheet, col) {
            if v.is_empty() {
                continue;
            }
            if v.eq_ignore_ascii_case(NA) {
                if !na_ok {
                    out.push(Problem {
                        sheet: sheet.into(),
                        row: r,
                        column: col.into(),
                        rule: "reference",
                        message: format!("{col} is n/a: it must name a {to}"),
                        error: false,
                    });
                }
                continue;
            }
            let parts: Vec<&str> = if list { v.split(',').map(str::trim).filter(|x| !x.is_empty()).collect() } else { vec![v.as_str()] };
            for x in parts {
                if !targets.contains(x) {
                    out.push(Problem {
                        sheet: sheet.into(),
                        row: r,
                        column: col.into(),
                        rule: "reference",
                        message: format!("{x} isn't on the {to} sheet"),
                        error: true,
                    });
                }
            }
        }
    };
    refs(&mut out, "Space", "FloorName", "Floor", false, false);
    refs(&mut out, "Component", "TypeName", "Type", false, false);
    refs(&mut out, "Component", "Space", "Space", true, false);
    refs(&mut out, "System", "ComponentNames", "Component", true, false);
    refs(&mut out, "Zone", "SpaceNames", "Space", true, false);
    refs(&mut out, "Spare", "TypeName", "Type", false, true);
    refs(&mut out, "Job", "TypeName", "Type", false, true);
    refs(&mut out, "Issue", "Owner", "Contact", false, true);
    // Rows that describe another sheet's row.
    for (sheet, pairs) in [
        ("Attribute", &[("SheetName", "RowName")][..]),
        ("Coordinate", &[("SheetName", "RowName")]),
        ("Document", &[("SheetName", "RowName")]),
        ("Impact", &[("SheetName", "RowName")]),
        ("Issue", &[("SheetName1", "RowName1"), ("SheetName2", "RowName2")]),
    ] {
        for (sc, rc) in pairs {
            let rows = b.column(sheet, rc);
            for (r, target) in b.column(sheet, sc) {
                if target.is_empty() || target.eq_ignore_ascii_case(NA) {
                    continue;
                }
                let name = rows.iter().find(|(x, _)| *x == r).map(|(_, v)| v.clone()).unwrap_or_default();
                match known.iter().find(|(k, _)| k.eq_ignore_ascii_case(&target)) {
                    None => out.push(Problem {
                        sheet: sheet.into(),
                        row: r,
                        column: (*sc).into(),
                        rule: "reference",
                        message: format!("{target} isn't a COBie sheet"),
                        error: true,
                    }),
                    Some((k, names)) if !names.contains(&name) => out.push(Problem {
                        sheet: sheet.into(),
                        row: r,
                        column: (*rc).into(),
                        rule: "reference",
                        message: format!("{name} isn't on the {k} sheet"),
                        error: true,
                    }),
                    _ => {}
                }
            }
        }
    }
    // Numbers and pick lists.
    for (sheet, col) in NUMBERS {
        for (r, v) in b.column(sheet, col) {
            if !v.is_empty() && !v.eq_ignore_ascii_case(NA) && v.parse::<f64>().map_or(true, |x| !x.is_finite()) {
                out.push(Problem {
                    sheet: (*sheet).into(),
                    row: r,
                    column: (*col).into(),
                    rule: "number",
                    message: format!("{v} isn't a number"),
                    error: true,
                });
            }
        }
    }
    for (sheet, col, allowed) in PICK_LISTS {
        for (r, v) in b.column(sheet, col) {
            if !v.is_empty() && !allowed.iter().any(|a| a.eq_ignore_ascii_case(&v)) {
                out.push(Problem {
                    sheet: (*sheet).into(),
                    row: r,
                    column: (*col).into(),
                    rule: "pick list",
                    message: format!("{v} isn't one of {}", allowed.join(", ")),
                    error: true,
                });
            }
        }
    }
    // Every type is used.
    let used: BTreeSet<String> = b.column("Component", "TypeName").into_iter().map(|(_, v)| v).collect();
    for (r, t) in b.column("Type", "Name") {
        if !t.is_empty() && !used.contains(&t) {
            out.push(Problem {
                sheet: "Type".into(),
                row: r,
                column: "Name".into(),
                rule: "unused type",
                message: format!("no component is a {t}"),
                error: false,
            });
        }
    }
    out
}

fn report(problems: &[Problem]) -> Value {
    let errors = problems.iter().filter(|p| p.error).count();
    json!({
        "ok": errors == 0, "errors": errors, "warnings": problems.len() - errors,
        "problems": problems.iter().take(MAX_PROBLEMS).map(Problem::json).collect::<Vec<_>>(),
    })
}

fn write_out(cmd: &str, path: &str, bytes: &[u8], out: &mut Value) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::writefile::write(path, bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
        out["path"] = json!(path);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (cmd, path);
        out["data"] = json!(super::file::base64_encode(bytes));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = cmd;
    Ok(())
}

fn path_param<'a>(cmd: &str, p: &'a Value) -> Result<&'a str> {
    str_param(p, "path").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "path required (an .xlsx file)"))
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_param("cobie.export", p)?;
    let o = Opts::from("cobie.export", p)?;
    let (d, source) = model("cobie.export", s)?;
    let topics = super::bcf::load(s.doc()?);
    let name = s.state().ok().map(|st| st.title.clone()).unwrap_or_default();
    let book = build(&d, &topics, &o, &name);
    let problems = validate(&book);
    let bytes = book.write().map_err(|e| bad("cobie.export", e))?;
    let rows: serde_json::Map<String, Value> = book.sheets.iter().map(|(n, r)| (n.clone(), json!(r.len().saturating_sub(1)))).collect();
    let mut out = json!({ "source": source, "rows": rows, "validation": report(&problems) });
    write_out("cobie.export", path, &bytes, &mut out)?;
    Ok(out)
}

fn run_validate(_s: &mut Session, p: &Value) -> Result<Value> {
    let book = match (str_param(p, "path"), str_param(p, "data")) {
        (Some(path), _) => {
            let bytes = crate::readfile::read(path.trim()).map_err(|e| bad("cobie.validate", format!("{path}: {e}")))?;
            Book::read(&bytes).map_err(|e| bad("cobie.validate", e))?
        }
        (None, Some(data)) => {
            let bytes = super::file::base64_decode(data).ok_or_else(|| bad("cobie.validate", "data isn't base64"))?;
            Book::read(&bytes).map_err(|e| bad("cobie.validate", e))?
        }
        _ => return Err(bad("cobie.validate", "path or data required (an .xlsx COBie workbook)")),
    };
    let mut out = report(&validate(&book));
    out["sheets"] = json!(book.sheets.iter().map(|(n, r)| json!({"name": n, "rows": r.len().saturating_sub(1)})).collect::<Vec<_>>());
    Ok(out)
}

// ---------------- schedules ----------------

fn is_door(class: &str) -> bool {
    class.to_ascii_lowercase().contains("door")
}

fn is_window(class: &str) -> bool {
    class.to_ascii_lowercase().contains("window")
}

/// The room, door and equipment schedules of the model in `d`.
fn schedules(d: &Drawing, which: &[String]) -> Book {
    let m = Model::new(d);
    let mut book = Book::default();
    let want = |k: &str| which.is_empty() || which.iter().any(|w| w.eq_ignore_ascii_case(k));
    let doors: Vec<(&Element, &String)> =
        m.components.iter().filter_map(|(i, n)| m.el(*i).map(|e| (e, n))).filter(|(e, _)| is_door(&e.tag.class)).collect();
    let equipment: Vec<(&Element, &String)> = m
        .components
        .iter()
        .filter_map(|(i, n)| m.el(*i).map(|e| (e, n)))
        .filter(|(e, _)| !is_door(&e.tag.class) && !is_window(&e.tag.class))
        .collect();
    if want("rooms") {
        let mut rows = vec![["Room", "Storey", "Area (m²)", "Height (m)", "Doors", "Equipment", "GlobalId"].map(String::from).to_vec()];
        let mut spaces: Vec<(&Element, &String)> = m.spaces.iter().filter_map(|(i, n)| m.el(*i).map(|e| (e, n))).collect();
        spaces.sort_by(|a, b| a.0.tag.elevation.unwrap_or(0.0).total_cmp(&b.0.tag.elevation.unwrap_or(0.0)).then_with(|| a.1.cmp(b.1)));
        for (e, n) in spaces {
            let n_doors = doors.iter().filter(|(dd, _)| m.spaces_touching(dd).contains(n)).count();
            let n_eq = equipment.iter().filter(|(q, _)| m.space_of(q).as_ref() == Some(n)).count();
            rows.push(vec![
                n.clone(),
                e.tag.level.clone().unwrap_or_default(),
                format!("{:.2}", m.area(e)),
                format!("{:.2}", m.m(e.max.z - e.min.z)),
                n_doors.to_string(),
                n_eq.to_string(),
                e.tag.guid.clone(),
            ]);
        }
        book.sheets.push(("Rooms".into(), rows));
    }
    if want("doors") {
        let mut rows = vec![["Mark", "Type", "Storey", "Rooms", "Width (m)", "Height (m)", "Fire rating", "GlobalId"].map(String::from).to_vec()];
        for (e, n) in &doors {
            let size = e.max - e.min;
            let width = prop(e, &["OverallWidth", "Width"]).and_then(|v| v.parse::<f64>().ok()).unwrap_or(m.m(size.x.max(size.y)));
            let height = prop(e, &["OverallHeight", "Height"]).and_then(|v| v.parse::<f64>().ok()).unwrap_or(m.m(size.z));
            rows.push(vec![
                prop(e, &["Mark", "Tag"]).map_or_else(|| (*n).clone(), str::to_string),
                Model::type_name(e),
                e.tag.level.clone().unwrap_or_default(),
                m.spaces_touching(e).join(" / "),
                format!("{width:.2}"),
                format!("{height:.2}"),
                prop(e, &["FireRating"]).unwrap_or_default().to_string(),
                e.tag.guid.clone(),
            ]);
        }
        book.sheets.push(("Doors".into(), rows));
    }
    if want("equipment") {
        let mut rows = vec![["Tag", "Class", "Type", "Manufacturer", "Model", "Systems", "Room", "Storey", "GlobalId"].map(String::from).to_vec()];
        for (e, n) in &equipment {
            rows.push(vec![
                prop(e, &["Tag", "TagNumber", "Mark"]).map_or_else(|| (*n).clone(), str::to_string),
                e.tag.class.clone(),
                Model::type_name(e),
                prop(e, &["Manufacturer"]).unwrap_or_default().to_string(),
                prop(e, &["ModelLabel", "ModelNumber", "ModelReference"]).unwrap_or_default().to_string(),
                e.tag.systems.join(", "),
                m.space_of(e).unwrap_or_default(),
                e.tag.level.clone().unwrap_or_default(),
                e.tag.guid.clone(),
            ]);
        }
        book.sheets.push(("Equipment".into(), rows));
    }
    book
}

fn run_schedules(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_param("handover.schedules", p)?;
    let which: Vec<String> = match p.get("schedules") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(t)) => t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        Some(_) => return Err(bad("handover.schedules", "schedules is a list of rooms, doors, equipment")),
    };
    if let Some(w) = which.iter().find(|w| !["rooms", "doors", "equipment"].iter().any(|k| k.eq_ignore_ascii_case(w))) {
        return Err(bad("handover.schedules", format!("no {} schedule (rooms, doors, equipment)", w.chars().take(30).collect::<String>())));
    }
    let (d, source) = model("handover.schedules", s)?;
    let book = schedules(&d, &which);
    let bytes = book.write().map_err(|e| bad("handover.schedules", e))?;
    let rows: serde_json::Map<String, Value> = book.sheets.iter().map(|(n, r)| (n.clone(), json!(r.len().saturating_sub(1)))).collect();
    let mut out = json!({ "source": source, "rows": rows });
    write_out("handover.schedules", path, &bytes, &mut out)?;
    Ok(out)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("cobie.export", "Export COBie", run_export)
            .menu(&["File", "Export", "COBie..."])
            .params("{path: .xlsx file, author?: email (default handover@example.com), company?, project?, facility?, category?, phase?, currency?, created_on?: yyyy-mm-ddThh:mm:ss (default now), zones?: [{name, category?, spaces: [space name]}]} → {source, rows: {sheet: count}, validation: {ok, errors, warnings, problems}, path}")
            .noundo(),
        CommandSpec::new("cobie.validate", "Validate COBie", run_validate)
            .enabled(always)
            .params("{path: .xlsx COBie workbook} | {data: base64} → {ok, errors, warnings, problems: [{sheet, row, column, rule, message, severity}], sheets}")
            .noundo(),
        CommandSpec::new("handover.schedules", "Export Handover Schedules", run_schedules)
            .menu(&["File", "Export", "Handover Schedules..."])
            .params("{path: .xlsx file, schedules?: [rooms, doors, equipment] (default all)} → {source, rows: {sheet: count}, path}")
            .noundo(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_dates_and_emails() {
        let mut n = Names::default();
        assert_eq!([n.take("Door"), n.take("door"), n.take("Door (2)"), n.take("")], ["Door", "door (2)", "Door (2) (2)", "Unnamed"]);
        assert!(
            is_cobie_date("2026-10-21T09:30:00")
                && is_cobie_date("2026-10-21T09:30:00Z")
                && !is_cobie_date("21/10/2026")
                && !is_cobie_date("2026-10-21")
        );
        assert!(is_email("a.b@example.com") && !is_email("ab@example") && !is_email("a b@example.com") && !is_email("@example.com"));
        assert_eq!((num(1.23456), num(-0.0001), num(f64::NAN)), ("1.235".to_string(), "0".to_string(), "n/a".to_string()));
    }

    #[test]
    fn every_sheet_has_name_created_by_and_created_on_first() {
        for (s, cols) in SHEETS {
            let key = if *s == "Contact" { "Email" } else { "Name" };
            assert_eq!(cols.first(), Some(&key), "{s}");
            assert!(cols.contains(&"CreatedBy") && cols.contains(&"CreatedOn"), "{s}");
            for r in required(s) {
                assert!(cols.contains(r), "{s}: {r}");
            }
        }
    }
}
