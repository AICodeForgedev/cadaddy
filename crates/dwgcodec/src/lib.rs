//! CADaddy's own DWG codec, written from the Open Design Alliance's public "Open Design
//! Specification for .dwg files" (cited below as "spec §n"). Nothing here comes from any other
//! DWG implementation.
//!
//! This crate reads and writes the containers of a DWG file: the bit-coded values (spec §2), the
//! R2000 file layout (spec §3), the R2004+ section and page maps and their LZ77 compression
//! (spec §4, §6–§8), the header variables (spec §9), the classes (spec §10) and the object map
//! (spec §23). Objects are kept as raw bytes with their handle and type; decoding entities and
//! objects is a later layer. `docs/dwg-codec.md` has the design and the version matrix.
//!
//! Every input is hostile: sizes, counts and nesting are capped ([`Limits`]), all reads are
//! bounds-checked, and errors are returned, never panics.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod bits;
pub mod classes;
pub mod crc;
mod dwg;
mod error;
pub mod frame;
pub mod header;
pub mod lz77;
pub mod objects;
pub mod objmap;
mod r2000;
mod r2004;

pub use dwg::{Dwg, Section, read, read_with, write};
pub use error::{Error, Result};

/// The DWG versions this codec knows, by the six-byte version id at the start of a file
/// (spec §3.2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Version {
    /// AC1015, AutoCAD 2000–2002.
    R2000,
    /// AC1018, AutoCAD 2004–2006.
    R2004,
    /// AC1021, AutoCAD 2007–2009 (its own container, spec §5: not read yet).
    R2007,
    /// AC1024, AutoCAD 2010–2012.
    R2010,
    /// AC1027, AutoCAD 2013–2017.
    R2013,
    /// AC1032, AutoCAD 2018 and later.
    R2018,
}

impl Version {
    /// The version id written at the start of the file.
    pub fn id(self) -> &'static str {
        match self {
            Version::R2000 => "AC1015",
            Version::R2004 => "AC1018",
            Version::R2007 => "AC1021",
            Version::R2010 => "AC1024",
            Version::R2013 => "AC1027",
            Version::R2018 => "AC1032",
        }
    }

    /// The version a file's first six bytes name.
    pub fn from_id(id: &[u8]) -> Result<Version> {
        Ok(match id {
            b"AC1015" => Version::R2000,
            b"AC1018" => Version::R2004,
            b"AC1021" => Version::R2007,
            b"AC1024" => Version::R2010,
            b"AC1027" => Version::R2013,
            b"AC1032" => Version::R2018,
            b"AC1012" | b"AC1014" => return Err(Error::Unsupported("R13 and R14 DWG files are not read yet".into())),
            _ => return Err(Error::NotDwg),
        })
    }

    /// Strings are UTF-16 (TU) rather than code-page bytes (T) from R2007 on (spec §2).
    pub fn unicode(self) -> bool {
        self >= Version::R2007
    }
}

/// Caps on what a file may make the reader allocate or loop over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest file read at all.
    pub max_file_bytes: usize,
    /// Largest decompressed section.
    pub max_section_bytes: usize,
    /// Most objects in the object map.
    pub max_objects: usize,
    /// Most characters in one string.
    pub max_string_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_file_bytes: 2 << 30, max_section_bytes: 1 << 30, max_objects: 50_000_000, max_string_chars: 1 << 20 }
    }
}
