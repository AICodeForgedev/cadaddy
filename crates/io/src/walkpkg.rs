//! Walkthrough packages (`.cdywalk`, [`cadaddy_walk3d::package`]) as drawings: reading one opens
//! its drawing; writing one from a drawing stores the drawing, its building spec and its 3D as
//! glTF. `walk.publish` (the engine) adds the built model and the baked lightmap.

use crate::{IoError, Result, cdy, mesh3d_io};
use cadaddy_doc::Drawing;
use cadaddy_walk3d::package::{self, Package};

/// The drawing in a package (its building spec from `spec.json` when the drawing lacks one).
pub fn read(bytes: &[u8]) -> Result<Drawing> {
    let p = Package::decode(bytes).map_err(IoError::Format)?;
    let cdy_bytes = p.get(package::DRAWING).ok_or_else(|| IoError::Format("the walkthrough package has no drawing".into()))?;
    let mut d = cdy::read_cdy(cdy_bytes)?;
    if d.building_spec.is_none()
        && let Some(spec) = p.get(package::SPEC).and_then(|s| std::str::from_utf8(s).ok())
        && serde_json::from_str::<serde_json::Value>(spec).is_ok_and(|v| v.is_object())
    {
        d.building_spec = Some(spec.to_string());
    }
    Ok(d)
}

/// A package of `d`: manifest, drawing, its 3D as glTF and its building spec.
pub fn write(d: &Drawing) -> Result<Vec<u8>> {
    package_of(d)?.encode().map_err(IoError::Format)
}

/// The package [`write`] makes, to add files to.
pub fn package_of(d: &Drawing) -> Result<Package> {
    let mut p = Package::default();
    let manifest = serde_json::json!({
        "format": package::FORMAT,
        "kind": "CADaddy walkthrough",
        "writer": format!("CADaddy {}", env!("CARGO_PKG_VERSION")),
        "open": "Open this file in CADaddy (desktop or web) to walk the building.",
    });
    p.put(package::MANIFEST, serde_json::to_vec_pretty(&manifest).unwrap_or_default());
    p.put(package::DRAWING, cdy::write_cdy(d)?);
    p.put(package::MODEL, mesh3d_io::write_glb(d));
    if let Some(spec) = d.building_spec.as_deref().filter(|s| !s.trim().is_empty()) {
        p.put(package::SPEC, spec.as_bytes().to_vec());
    }
    Ok(p)
}
