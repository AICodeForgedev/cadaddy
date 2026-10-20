//! A DWG file's containers: what [`read`] returns and [`write`] takes.

use crate::frame::Framed;
use crate::objects::RawObject;
use crate::{Error, Limits, Result, Version, r2000, r2004, r2007};

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
    R2004(Box<r2004::Layout>),
    R2007(Box<r2007::Layout>),
}

impl Dwg {
    /// A new file of `version` (R2004 and later) holding `header`, `classes` and `objects`; the
    /// containers, maps and checks are made by [`write`].
    pub fn new(version: Version, header: &crate::header::HeaderVars, classes: &crate::classes::Classes, objects: Vec<RawObject>) -> Result<Dwg> {
        if version < Version::R2000 {
            return Err(Error::Unsupported(format!("writing new {} files", version.id())));
        }
        let hi = (version >= Version::R2004 && version != Version::R2007 && r2004::fresh_has_hi(version)).then_some(0);
        let layout = match version {
            Version::R2000 => Layout::R2000(r2000::fresh()),
            Version::R2007 => Layout::R2007(Box::new(r2007::fresh())),
            _ => Layout::R2004(Box::new(r2004::fresh(version))),
        };
        // The section after the classes' end sentinel holds 8 more bytes from R2004 (kept as
        // read); a new file writes none.
        Ok(Dwg {
            version,
            header: Framed { body: header.encode(), hi, trailer: Vec::new() },
            classes: Framed { body: classes.encode(), hi, trailer: Vec::new() },
            objects,
            sections: Vec::new(),
            layout,
        })
    }

    /// The sections a new file has besides its header, classes and objects (spec §16, §21, §27,
    /// §14.2): the template with `measurement` (0 imperial, 1 metric), the object free-space data
    /// for `objects` objects, the auxiliary header with the handle seed, and an empty preview.
    pub fn new_file_sections(version: Version, objects: usize, handseed: u64, measurement: u16) -> Vec<Section> {
        let mut template = vec![0, 0];
        template.extend_from_slice(&measurement.to_le_bytes());
        let mut free = Vec::with_capacity(53);
        free.extend_from_slice(&0u32.to_le_bytes());
        free.extend_from_slice(&u32::try_from(objects).unwrap_or(u32::MAX).to_le_bytes());
        free.extend_from_slice(&[0; 8]);
        // The objects' address (R2000: filled in when written).
        let start = if version < Version::R2004 { r2000::FRESH_OBJECTS_START } else { 0 };
        free.extend_from_slice(&start.to_le_bytes());
        free.push(4);
        for v in [0x32u32, 0, 0x64, 0, 0x200, 0, u32::MAX, 0] {
            free.extend_from_slice(&v.to_le_bytes());
        }
        // The DWG version code the auxiliary header repeats.
        let code: u16 = match version {
            Version::R2000 => 0x17,
            Version::R2004 => 0x19,
            Version::R2007 => 0x1B,
            Version::R2010 => 0x1D,
            Version::R2013 => 0x1F,
            _ => 0x21,
        };
        let mut aux = vec![0xFF, 0x77, 0x01];
        let rs = |a: &mut Vec<u8>, v: u16| a.extend_from_slice(&v.to_le_bytes());
        let rl = |a: &mut Vec<u8>, v: u32| a.extend_from_slice(&v.to_le_bytes());
        rs(&mut aux, code);
        rs(&mut aux, 0);
        rl(&mut aux, 1);
        rl(&mut aux, u32::MAX);
        rs(&mut aux, 1);
        rs(&mut aux, 0);
        rl(&mut aux, 0);
        for _ in 0..2 {
            rs(&mut aux, code);
            rs(&mut aux, 0);
        }
        for _ in 0..2 {
            rs(&mut aux, 0x0005);
            rs(&mut aux, 0x0893);
        }
        rs(&mut aux, 0);
        rs(&mut aux, 1);
        // Five zero longs, the creation and update dates (none).
        aux.extend_from_slice(&[0; 20 + 16]);
        rl(&mut aux, u32::try_from(handseed).unwrap_or(0));
        rl(&mut aux, 0);
        rs(&mut aux, 0);
        rs(&mut aux, 1);
        rs(&mut aux, 0);
        rl(&mut aux, 0);
        rl(&mut aux, 1);
        aux.resize(123, 0);
        let mut preview = r2000::PREVIEW_START.to_vec();
        preview.extend_from_slice(&1u32.to_le_bytes());
        preview.push(0);
        preview.extend_from_slice(&r2000::PREVIEW_END);
        vec![
            Section { name: "AcDb:Template".into(), data: template },
            Section { name: "AcDb:ObjFreeSpace".into(), data: free },
            Section { name: "AcDb:AuxHeader".into(), data: aux },
            Section { name: r2000::PREVIEW.into(), data: preview },
        ]
    }

    /// The drawing's code page number (the file header's, byte 0x13; 30 is ANSI 1252).
    pub fn code_page(&self) -> u16 {
        match &self.layout {
            Layout::R2000(l) => l.code_page(),
            Layout::R2004(l) => l.code_page(),
            Layout::R2007(l) => l.code_page(),
        }
    }

    /// The raw section called `name`.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// The header variables, decoded (spec §9).
    pub fn header_vars(&self) -> Result<crate::header::HeaderVars> {
        crate::header::HeaderVars::decode(&self.header.body, self.version)
    }

    /// Replaces the header variables section's data with `vars` encoded.
    pub fn set_header_vars(&mut self, vars: &crate::header::HeaderVars) {
        self.header.body = vars.encode();
    }

    /// The classes, decoded (spec §10).
    pub fn classes(&self) -> Result<crate::classes::Classes> {
        crate::classes::Classes::decode(&self.classes.body, self.version)
    }

    /// Replaces the classes section's data with `classes` encoded.
    pub fn set_classes(&mut self, classes: &crate::classes::Classes) {
        self.classes.body = classes.encode();
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
        Version::R2007 => r2007::read(bytes, limits),
        v => r2004::read(bytes, v, limits),
    }
}

/// Writes a DWG file from its containers.
pub fn write(dwg: &Dwg) -> Result<Vec<u8>> {
    match &dwg.layout {
        Layout::R2000(l) => r2000::write(dwg, l),
        Layout::R2004(l) => r2004::write(dwg, l),
        Layout::R2007(l) => r2007::write(dwg, l),
    }
}
