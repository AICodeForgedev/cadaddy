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

/// A check of a .cdy file: every chunk's checksum and compression (each save's, the superseded
/// ones too), the index and trailer, and whether the drawing reads without leaving anything out.
/// `ok` is true only when all of that is sound. Errors when it isn't a .cdy file at all.
///
/// ```json
/// {"ok", "format", "bytes", "index": "sound"|"damaged", "chunks": [{tag, version, offset, stored,
///  raw, deflated, live, damage?}], "unreadable": [[from, to]], "sizes": {TAG: {count, stored, raw}},
///  "waste": bytes of superseded chunks, "notes": [what reading left out]}
/// ```
pub fn verify(bytes: &[u8]) -> std::result::Result<serde_json::Value, String> {
    use serde_json::{Map, json};
    let format = container::header(bytes)?;
    let index = container::index(bytes);
    let live: std::collections::HashSet<u64> = index.iter().flatten().map(|e| e.offset).collect();
    let (walked, lost) = container::walk(bytes);
    let mut sizes = Map::new();
    let (mut waste, mut damaged) = (0u64, 0usize);
    let chunks: Vec<_> = walked
        .iter()
        .filter(|w| &w.tag != b"INDX")
        .map(|w| {
            let tag = String::from_utf8_lossy(&w.tag).into_owned();
            let is_live = live.contains(&w.offset) || index.is_none();
            if is_live {
                let e = sizes.entry(tag.clone()).or_insert_with(|| json!({"count": 0, "stored": 0, "raw": 0}));
                for (k, n) in [("count", 1), ("stored", w.stored), ("raw", w.raw)] {
                    e[k] = json!(e[k].as_u64().unwrap_or(0).saturating_add(n));
                }
            } else {
                waste = waste.saturating_add(w.stored).saturating_add(container::CHUNK_HEADER_LEN as u64);
            }
            if w.damage.is_some() && is_live {
                damaged += 1;
            }
            let mut c = json!({"tag": tag, "version": w.version, "offset": w.offset, "stored": w.stored, "raw": w.raw, "deflated": w.deflated, "live": is_live});
            if let Some(why) = &w.damage {
                c["damage"] = json!(why);
            }
            c
        })
        .collect();
    let notes = match model::read(bytes) {
        Ok((_, rep)) => rep.notes,
        Err(e) => vec![e],
    };
    let ok = index.is_some() && damaged == 0 && lost.is_empty() && notes.is_empty();
    Ok(json!({
        "ok": ok,
        "format": format,
        "bytes": bytes.len(),
        "index": if index.is_some() { "sound" } else { "damaged" },
        "chunks": chunks,
        "unreadable": lost,
        "sizes": sizes,
        "waste": waste,
        "notes": notes,
    }))
}

/// The PNG preview of a .cdy file (for file browsers and the Start page), read without reading
/// the drawing; `None` when there is none.
pub fn thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    container::find(bytes, b"THMB").map(|c| c.data).filter(|png| png.starts_with(b"\x89PNG"))
}

#[cfg(test)]
mod tests;
