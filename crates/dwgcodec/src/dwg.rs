//! A DWG file's containers: what [`read`] returns and [`write`] takes.

use crate::frame::Framed;
use crate::objects::RawObject;
use crate::{Error, Limits, Result, Version, r2000};

/// A section kept as raw bytes (one this layer does not interpret).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// The section's name (R2004+: as stored, e.g. "AcDb:Preview"; R2000: the name of the
    /// matching R2004+ section).
    pub name: String,
    pub data: Vec<u8>,
}

/// A DWG file's contents, containers decoded, objects raw.
#[derive(Clone, Debug, PartialEq)]
pub struct Dwg {
    pub version: Version,
    /// The header variables section (spec §9): its bit-coded data in `body`.
    pub header: Framed,
    /// The classes section (spec §10): its bit-coded data in `body`.
    pub classes: Framed,
    /// Every object in the object map, in the order they are stored.
    pub objects: Vec<RawObject>,
    /// Sections this layer keeps as they are (preview, summary info, aux header, …).
    pub sections: Vec<Section>,
    pub(crate) layout: Layout,
}

/// How the file was laid out, so it is written back the same way.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Layout {
    R2000(r2000::Layout),
}

impl Dwg {
    /// The raw section called `name`.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// The object with `handle`.
    pub fn object(&self, handle: u64) -> Option<&RawObject> {
        self.objects.iter().find(|o| o.handle == handle)
    }
}

/// Reads a DWG file's containers.
pub fn read(bytes: &[u8]) -> Result<Dwg> {
    read_with(bytes, &Limits::default())
}

/// [`read`] with explicit [`Limits`].
pub fn read_with(bytes: &[u8], limits: &Limits) -> Result<Dwg> {
    if bytes.len() > limits.max_file_bytes {
        return Err(Error::TooLarge("the file"));
    }
    let id = bytes.get(..6).ok_or(Error::NotDwg)?;
    match Version::from_id(id)? {
        Version::R2000 => r2000::read(bytes, limits),
        Version::R2007 => Err(Error::Unsupported("AutoCAD 2007 (AC1021) DWG files are not read yet".into())),
        v => Err(Error::Unsupported(format!("{} DWG files are not read yet", v.id()))),
    }
}

/// Writes a DWG file from its containers.
pub fn write(dwg: &Dwg) -> Result<Vec<u8>> {
    match &dwg.layout {
        Layout::R2000(l) => r2000::write(dwg, l),
    }
}
