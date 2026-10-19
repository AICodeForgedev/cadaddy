//! CADaddy's own drawing format, .cdy: a chunked container (each chunk tagged, versioned,
//! checksummed and compressed on its own) holding the drawing in a frozen keyed encoding, with a
//! human-readable JSON manifest and a PNG preview. The format is described in
//! `docs/cdy-format.md`.

mod container;
mod meshbuf;
mod model;
mod save;
mod tokens;

use cadaddy_doc::{Drawing, Space};

pub use container::{FORMAT_VERSION, is_cdy};
pub use model::{ENTITY_CHUNK, ReadReport};
pub use save::{MAX_WASTE, Output, SaveState, save, seed};

use crate::{IoError, Result};

/// Width and height of the embedded preview, in pixels.
pub const THUMBNAIL_SIZE: (u32, u32) = (256, 192);

/// The preview a .cdy file embeds: model space as a PNG, `None` when it is empty.
pub fn preview(d: &Drawing) -> Option<Vec<u8>> {
    if d.model.is_empty() { None } else { crate::png(d, &Space::Model, THUMBNAIL_SIZE.0, THUMBNAIL_SIZE.1).ok() }
}

/// The drawing as a .cdy file, with a preview of model space.
pub fn write_cdy(d: &Drawing) -> Result<Vec<u8>> {
    model::write(d, preview(d).as_deref()).map_err(IoError::Format)
}

/// The drawing in a .cdy file. Damaged parts that were left out are listed in the drawing's
/// `read_notes`; an error means the file isn't a .cdy file this version can read.
pub fn read_cdy(bytes: &[u8]) -> Result<Drawing> {
    let (mut d, rep) = model::read(bytes).map_err(IoError::Format)?;
    d.read_notes.extend(rep.notes);
    Ok(d)
}

/// The drawing and what else the file holds (manifest, preview, notes).
pub fn read_cdy_report(bytes: &[u8]) -> Result<(Drawing, ReadReport)> {
    model::read(bytes).map_err(IoError::Format)
}

/// The manifest of a .cdy file (format and writer versions, units, extents, counts), read
/// without reading the drawing; `None` when it isn't a readable .cdy file.
pub fn manifest(bytes: &[u8]) -> Option<serde_json::Value> {
    serde_json::from_slice(&container::find(bytes, b"MANI")?.data).ok()
}

/// The PNG preview of a .cdy file (for file browsers and the Start page), read without reading
/// the drawing; `None` when there is none.
pub fn thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    container::find(bytes, b"THMB").map(|c| c.data).filter(|png| png.starts_with(b"\x89PNG"))
}

#[cfg(test)]
mod tests;
