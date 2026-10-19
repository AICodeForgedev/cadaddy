//! Saving a .cdy file, in full or incrementally.
//!
//! A full save writes every chunk. An incremental save appends only the chunks whose content
//! changed since the file was last written (or read), then a new index and trailer. The new index
//! lists the unchanged chunks where they already are. A part counts as unchanged when:
//! - it is the very same object store chunk, block or kept file data the file was written from
//!   (see [`cadaddy_doc::ChunkRef`]: while the save state holds them, an edit copies what it
//!   touches);
//! - or, for the small parts (header, tables, layouts, manifest), it encodes to the same bytes.
//!
//! The superseded chunks stay in the file as waste; once waste would pass [`MAX_WASTE`] of the
//! file (and [`MIN_WASTE`] bytes), the save is a full one instead (compaction). A file appended to is never cut: a save that
//! stops partway leaves the previous save's index as the last complete one, which readers use
//! ([`super::container::chunks`]).

use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_doc::foreign::Foreign;
use cadaddy_doc::{Block, ChunkRef, Drawing};

use super::container::{self, CHUNK_HEADER_LEN, HEADER_LEN, IndexEntry, TRAILER_LEN, Writer};
use super::model;

/// The share of a file that may be superseded chunks before a save compacts it…
pub const MAX_WASTE: f64 = 0.3;
/// …once they are at least this many bytes (a small file is saved in full quickly anyway).
pub const MIN_WASTE: u64 = 64 * 1024;
const V1: u16 = 1;

/// What a part of the file was written from, to tell whether it changed.
#[derive(Clone)]
enum Source {
    /// A chunk of the model space object store.
    Objects(ChunkRef),
    /// A block definition, by name.
    Block(String, Arc<Block>),
    Foreign(Arc<Foreign>),
    /// A small part, by tag and the CRC-32 and length of its payload (the manifest, header,
    /// tables, a layout).
    Bytes([u8; 4], u32, u64),
    Thumbnail,
    /// A part this version doesn't know (a newer CADaddy's): kept as it is.
    Other,
}

#[derive(Clone)]
struct Saved {
    source: Source,
    entry: IndexEntry,
}

/// What a .cdy file holds and where, as last written or read: what the next save compares with.
#[derive(Clone)]
pub struct SaveState {
    /// The file's length when it was written (or read).
    pub len: u64,
    saved: Vec<Saved>,
}

impl std::fmt::Debug for SaveState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaveState").field("len", &self.len).field("chunks", &self.saved.len()).finish()
    }
}

/// How to store a save.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    /// A whole new file (write it atomically: a temporary file renamed over the old one).
    Full(Vec<u8>),
    /// Bytes to append to the file, which must still be `from` bytes long. The chunks come first
    /// and the index and trailer from `index_at` on: make the chunks durable before writing those.
    Append { from: u64, bytes: Vec<u8>, index_at: usize },
}

/// One part of the drawing as the file holds it, in file order.
enum Part<'a> {
    Manifest,
    Header,
    Tables,
    Block(&'a str, &'a Arc<Block>),
    Objects(ChunkRef),
    Layout(&'a cadaddy_doc::Layout),
    Foreign(&'a Arc<Foreign>),
}

fn parts(d: &Drawing) -> Vec<Part<'_>> {
    let mut out = vec![Part::Manifest, Part::Header, Part::Tables];
    out.extend(d.blocks.iter().map(|(n, b)| Part::Block(n, b)));
    out.extend(d.model.chunk_refs().into_iter().map(Part::Objects));
    out.extend(d.layouts.iter().map(Part::Layout));
    if !d.foreign.is_empty() {
        out.push(Part::Foreign(&d.foreign));
    }
    out
}

/// The chunks of an earlier save, looked up by what they were written from.
#[derive(Default)]
struct Earlier<'a> {
    objects: HashMap<usize, (&'a ChunkRef, &'a IndexEntry)>,
    blocks: HashMap<&'a str, (&'a Arc<Block>, &'a IndexEntry)>,
    foreign: Option<(&'a Arc<Foreign>, &'a IndexEntry)>,
    bytes: HashMap<([u8; 4], u32, u64), &'a IndexEntry>,
    thumbnail: Option<&'a IndexEntry>,
    other: Vec<&'a IndexEntry>,
}

impl<'a> Earlier<'a> {
    fn of(s: &'a SaveState) -> Self {
        let mut e = Earlier::default();
        for x in &s.saved {
            match &x.source {
                Source::Objects(c) => {
                    e.objects.insert(c.key(), (c, &x.entry));
                }
                Source::Block(n, b) => {
                    e.blocks.insert(n.as_str(), (b, &x.entry));
                }
                Source::Foreign(f) => e.foreign = Some((f, &x.entry)),
                Source::Bytes(tag, crc, len) => {
                    e.bytes.insert((*tag, *crc, *len), &x.entry);
                }
                Source::Thumbnail => e.thumbnail = Some(&x.entry),
                Source::Other => e.other.push(&x.entry),
            }
        }
        e
    }
}

/// Write `d`'s parts: each unchanged one listed where `earlier` has it, each other one written.
fn write_parts(d: &Drawing, w: &mut Writer, earlier: &Earlier<'_>, thumbnail: bool) -> Result<Vec<Saved>, String> {
    let mut saved = Vec::new();
    let mut put = |w: &mut Writer,
                   source: Source,
                   reused: Option<&IndexEntry>,
                   tag: [u8; 4],
                   data: &dyn Fn() -> Result<Vec<u8>, String>|
     -> Result<(), String> {
        match reused {
            Some(e) => w.reuse(e),
            None => w.chunk(tag, V1, &data()?, true),
        }
        if let Some(entry) = w.entries().last() {
            saved.push(Saved { source, entry: entry.clone() });
        }
        Ok(())
    };
    // A small part: encoded every save, rewritten only when its bytes changed.
    let small =
        |w: &mut Writer,
         put: &mut dyn FnMut(&mut Writer, Source, Option<&IndexEntry>, [u8; 4], &dyn Fn() -> Result<Vec<u8>, String>) -> Result<(), String>,
         tag: [u8; 4],
         bytes: Vec<u8>| {
            let key = (tag, crc32fast::hash(&bytes), bytes.len() as u64);
            let reused = earlier.bytes.get(&key).copied();
            put(w, Source::Bytes(key.0, key.1, key.2), reused, tag, &|| Ok(bytes.clone()))
        };
    for part in parts(d) {
        match part {
            Part::Manifest => small(w, &mut put, *b"MANI", model::manifest(d, thumbnail))?,
            Part::Header => small(w, &mut put, *b"HEAD", model::header(d)?)?,
            Part::Tables => small(w, &mut put, *b"TABL", model::tables(d)?)?,
            Part::Layout(l) => small(w, &mut put, *b"LAYO", model::layout(l)?)?,
            Part::Block(name, b) => {
                let reused = earlier.blocks.get(name).filter(|(old, _)| Arc::ptr_eq(old, b)).map(|(_, e)| *e);
                put(w, Source::Block(name.to_string(), b.clone()), reused, *b"BLKS", &|| model::block(name, b))?;
            }
            Part::Objects(c) => {
                let reused = earlier.objects.get(&c.key()).filter(|(old, _)| old.same(&c)).map(|(_, e)| *e);
                let items = c.clone();
                put(w, Source::Objects(c), reused, *b"ENTS", &|| model::entities(items.items()))?;
            }
            Part::Foreign(f) => {
                let reused = earlier.foreign.filter(|(old, _)| Arc::ptr_eq(old, f)).map(|(_, e)| e);
                put(w, Source::Foreign(f.clone()), reused, *b"FRGN", &|| model::foreign(f))?;
            }
        }
    }
    // Parts a newer CADaddy wrote, kept as they are.
    for e in &earlier.other {
        w.reuse(e);
        saved.push(Saved { source: Source::Other, entry: (*e).clone() });
    }
    Ok(saved)
}

/// Bytes a file needs for these chunks: header, chunks, index and trailer.
fn live(saved: &[Saved]) -> u64 {
    let chunks: u64 = saved.iter().map(|s| s.entry.len.saturating_add(CHUNK_HEADER_LEN as u64)).fold(0, u64::saturating_add);
    // The index: about 30 bytes an entry, then its chunk header and the trailer.
    (HEADER_LEN as u64).saturating_add(chunks).saturating_add(saved.len() as u64 * 30).saturating_add((CHUNK_HEADER_LEN + TRAILER_LEN) as u64)
}

/// The whole file for `d`, with `thumbnail` (a PNG) as its preview.
pub fn write_full(d: &Drawing, thumbnail: Option<Vec<u8>>) -> Result<(Vec<u8>, SaveState), String> {
    let mut w = Writer::new();
    let mut saved = write_parts(d, &mut w, &Earlier::default(), thumbnail.is_some())?;
    if let Some(png) = thumbnail {
        w.chunk(*b"THMB", V1, &png, false);
        if let Some(e) = w.entries().last() {
            saved.push(Saved { source: Source::Thumbnail, entry: e.clone() });
        }
    }
    let bytes = w.finish();
    let len = bytes.len() as u64;
    Ok((bytes, SaveState { len, saved }))
}

/// Save `d`: appended to the file `prev` describes when that saves enough, else in full (with
/// the preview `thumbnail` makes; an appended save keeps the file's preview).
pub fn save(d: &Drawing, prev: Option<&SaveState>, thumbnail: &dyn Fn() -> Option<Vec<u8>>) -> Result<(Output, SaveState), String> {
    if let Some(prev) = prev {
        let earlier = Earlier::of(prev);
        let mut w = Writer::append(prev.len);
        let mut saved = write_parts(d, &mut w, &earlier, earlier.thumbnail.is_some())?;
        if let Some(e) = earlier.thumbnail {
            w.reuse(e);
            saved.push(Saved { source: Source::Thumbnail, entry: e.clone() });
        }
        let (bytes, index_at) = w.finish_split();
        let len = prev.len.saturating_add(bytes.len() as u64);
        let waste = len.saturating_sub(live(&saved));
        if waste <= MIN_WASTE || waste as f64 <= MAX_WASTE * len as f64 {
            return Ok((Output::Append { from: prev.len, bytes, index_at }, SaveState { len, saved }));
        }
    }
    let (bytes, state) = write_full(d, thumbnail())?;
    Ok((Output::Full(bytes), state))
}

/// The save state of a .cdy file just read as `d`, so that saving it again appends only what
/// changes. `None` when the file can't be matched to the drawing part by part (written by an
/// earlier writer, read with damage, or found by walking): the next save is then a full one.
pub fn seed(d: &Drawing, bytes: &[u8]) -> Option<SaveState> {
    let entries = container::index(bytes)?;
    let objects = d.model.chunk_refs();
    let blocks: Vec<(&String, &Arc<Block>)> = d.blocks.iter().collect();
    let count = |tag: &[u8; 4]| entries.iter().filter(|e| &e.tag == tag).count();
    // The reader makes one store chunk and one block per chunk: anything else isn't this file.
    let known = |e: &IndexEntry| [b"MANI", b"HEAD", b"TABL", b"BLKS", b"ENTS", b"LAYO", b"FRGN", b"THMB"].contains(&&e.tag);
    if count(b"ENTS") != objects.len() || count(b"BLKS") != blocks.len() || entries.iter().any(|e| known(e) && e.version > V1) {
        return None;
    }
    let (mut objects, mut blocks) = (objects.into_iter(), blocks.into_iter());
    let mut saved = Vec::with_capacity(entries.len());
    for e in entries {
        let source = match &e.tag {
            b"ENTS" => Source::Objects(objects.next()?),
            b"BLKS" => {
                let (n, b) = blocks.next()?;
                Source::Block(n.clone(), b.clone())
            }
            b"FRGN" => Source::Foreign(d.foreign.clone()),
            b"THMB" => Source::Thumbnail,
            b"MANI" | b"HEAD" | b"TABL" | b"LAYO" => {
                let (container::Read::Ok(c), _) = container::chunk_at(bytes, usize::try_from(e.offset).ok()?)? else { return None };
                Source::Bytes(e.tag, crc32fast::hash(&c.data), c.data.len() as u64)
            }
            _ => Source::Other,
        };
        saved.push(Saved { source, entry: e });
    }
    Some(SaveState { len: bytes.len() as u64, saved })
}
