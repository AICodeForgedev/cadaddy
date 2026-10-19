//! 3D solids in DXF. Other programs see a solid as the mesh of its surfaces (a MESH from DXF
//! R2010 on, a polyface mesh before), which they can show, shade and measure but not edit as a
//! solid. CADaddy keeps the solid's exact definition in the mesh's `CADADDY` xdata
//! (`1000 SOLID3D`, then its JSON in `1000` strings) and reads the solid back from it. A 3DSOLID
//! read from a file (its ACIS data) is kept untouched as a foreign object and written back as it
//! was.

use cadaddy_doc::Solid3d;
use cadaddy_dxf::Tag;

/// The xdata marker before a solid's definition.
pub(crate) const SOLID_MARK: &str = "SOLID3D";
/// The definition's format version.
const SOLID_V: u32 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    v: u32,
    solid: Solid3d,
}

/// The `1000` strings of a solid's xdata, marker first.
pub(crate) fn solid_xdata(s: &Solid3d) -> Vec<String> {
    let Ok(json) = serde_json::to_string(&Saved { v: SOLID_V, solid: s.clone() }) else { return Vec::new() };
    std::iter::once(SOLID_MARK.to_string()).chain(crate::dxf_table::chunks(&json)).collect()
}

/// The solid a mesh record carries in its xdata, if it is one of ours and usable.
pub(crate) fn read_solid(tags: &[Tag]) -> Option<Solid3d> {
    let json = crate::dxf_table::payload(tags, SOLID_MARK)?;
    let saved: Saved = serde_json::from_str(&json).ok()?;
    (saved.v == SOLID_V && saved.solid.is_valid()).then_some(saved.solid)
}

/// The xdata marker before a section plane's definition.
pub(crate) const SECTION_MARK: &str = "SECTIONPLANE";

/// The `1000` strings of a section plane's xdata, marker first.
pub(crate) fn section_xdata(sp: &cadaddy_doc::SectionPlane) -> Vec<String> {
    let Ok(json) = serde_json::to_string(sp) else { return Vec::new() };
    std::iter::once(SECTION_MARK.to_string()).chain(crate::dxf_table::chunks(&json)).collect()
}

/// The section plane a LINE carries in its xdata, if it is one of ours and usable.
pub(crate) fn read_section(tags: &[Tag]) -> Option<cadaddy_doc::SectionPlane> {
    let json = crate::dxf_table::payload(tags, SECTION_MARK)?;
    let sp: cadaddy_doc::SectionPlane = serde_json::from_str(&json).ok()?;
    sp.is_valid().then_some(sp)
}
