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
use cadaddy_doc::sheetstore::{SheetRect, StoredChunk, same_rect};
use cadaddy_doc::{Block, ChunkRef, Drawing};

use super::container::{self, CHUNK_HEADER_LEN, HEADER_LEN, IndexEntry, TRAILER_LEN, Writer};
use super::model;
use super::sheets;

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
    /// A chunk of a print set's sheet as stored: parked (out of memory) or decoded and unchanged
    /// since. Written back byte for byte.
    Stored(Arc<StoredChunk>),
    /// One of the pieces a chunk of the object store was written as, split where one sheet's
    /// objects end and the next's begin ([`sheets::pieces`]): its objects `from..to`.
    Piece(ChunkRef, usize, usize),
    /// A block definition, by name.
    Block(String, Arc<Block>),
    Foreign(Arc<Foreign>),
    /// An embedded file, by name.
    Embedded(String, Arc<Vec<u8>>),
    /// A small part, by tag and the CRC-32 and length of its payload (the manifest, header,
    /// tables, a layout).
    Bytes([u8; 4], u32, u64),
    Thumbnail,
    /// A part this version doesn't know (a newer CADaddy's): kept as it is.
    Other,
}

/// A run of an object chunk's objects on one sheet (or none), `(sheet, from, to)`.
type Piece = (Option<u32>, usize, usize);

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
    /// The sheet each object chunk was filed under ([`sheets`]), against the sheets in
    /// `filed_against`: worked out again only for chunks that changed.
    filed: Arc<HashMap<usize, (ChunkRef, Vec<Piece>)>>,
    filed_against: Arc<Vec<SheetRect>>,
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
    /// A parked sheet's chunk, with the sheet (its place in the sheet list) it is filed under.
    Parked(Arc<StoredChunk>, Option<u32>),
    /// The sheet index, after every other part.
    SheetIndex,
    Layout(&'a cadaddy_doc::Layout),
    Foreign(&'a Arc<Foreign>),
    /// One of the text parts ([`model::TEXT_PARTS`]).
    Text([u8; 4], std::borrow::Cow<'a, str>),
    Embedded(&'a str, &'a Arc<Vec<u8>>),
}

/// The sheets a drawing's objects are filed under: the print set's, then any parked sheet the
/// list lacks (a drawing whose records changed keeps the sheets it parked).
fn sheet_list(d: &Drawing) -> Vec<SheetRect> {
    let mut out: Vec<SheetRect> = d.sheet_store.sheets.as_ref().clone();
    for p in d.sheet_store.parked.iter().filter(|p| !p.chunks.is_empty()) {
        if !out.iter().any(|s| p.is(&s.number, &s.rect)) {
            out.push(SheetRect { number: p.number.clone(), rect: p.rect });
        }
    }
    out
}

fn parts<'a>(d: &'a Drawing, sheets: &[SheetRect]) -> Vec<Part<'a>> {
    let mut out = vec![Part::Manifest, Part::Header, Part::Tables];
    out.extend(d.blocks.iter().map(|(n, b)| Part::Block(n, b)));
    // Model space in draw order: the parked sheets' chunks where they go back (their order among
    // the store's chunks), so a file read back has the order the drawing had with every sheet in.
    let mut parked: Vec<(u32, Arc<StoredChunk>, Option<u32>)> = d
        .sheet_store
        .parked
        .iter()
        .flat_map(|p| {
            let k = sheets.iter().position(|s| p.is(&s.number, &s.rect)).and_then(|k| u32::try_from(k).ok());
            p.chunks.iter().map(move |c| (c.order, c.clone(), k))
        })
        .collect();
    parked.sort_by_key(|(order, _, _)| *order);
    let mut parked = parked.into_iter().peekable();
    for c in d.model.chunk_refs() {
        while let Some((_, sc, k)) = parked.next_if(|(order, _, _)| *order < c.order()) {
            out.push(Part::Parked(sc, k));
        }
        out.push(Part::Objects(c));
    }
    out.extend(parked.map(|(_, sc, k)| Part::Parked(sc, k)));
    out.extend(d.layouts.iter().map(Part::Layout));
    if !d.foreign.is_empty() {
        out.push(Part::Foreign(&d.foreign));
    }
    out.extend(model::TEXT_PARTS.iter().filter_map(|(tag, _)| model::text_of(d, tag).map(|t| Part::Text(**tag, t))));
    out.extend(d.embedded.iter().map(|(n, b)| Part::Embedded(n, b)));
    if !sheets.is_empty() {
        out.push(Part::SheetIndex);
    }
    out
}

/// The chunks of an earlier save, looked up by what they were written from.
#[derive(Default)]
struct Earlier<'a> {
    objects: HashMap<usize, (&'a ChunkRef, &'a IndexEntry)>,
    stored: HashMap<usize, &'a IndexEntry>,
    pieces: HashMap<(usize, usize), (&'a ChunkRef, usize, &'a IndexEntry)>,
    filed: Option<(&'a HashMap<usize, (ChunkRef, Vec<Piece>)>, &'a [SheetRect])>,
    blocks: HashMap<&'a str, (&'a Arc<Block>, &'a IndexEntry)>,
    foreign: Option<(&'a Arc<Foreign>, &'a IndexEntry)>,
    embedded: HashMap<&'a str, (&'a Arc<Vec<u8>>, &'a IndexEntry)>,
    bytes: HashMap<([u8; 4], u32, u64), &'a IndexEntry>,
    thumbnail: Option<&'a IndexEntry>,
    other: Vec<&'a IndexEntry>,
}

impl<'a> Earlier<'a> {
    fn of(s: &'a SaveState) -> Self {
        let mut e = Earlier { filed: Some((s.filed.as_ref(), s.filed_against.as_slice())), ..Earlier::default() };
        for x in &s.saved {
            match &x.source {
                Source::Objects(c) => {
                    e.objects.insert(c.key(), (c, &x.entry));
                }
                Source::Stored(c) => {
                    e.stored.insert(Arc::as_ptr(c) as usize, &x.entry);
                }
                Source::Piece(c, from, to) => {
                    e.pieces.insert((c.key(), *from), (c, *to, &x.entry));
                }
                Source::Block(n, b) => {
                    e.blocks.insert(n.as_str(), (b, &x.entry));
                }
                Source::Foreign(f) => e.foreign = Some((f, &x.entry)),
                Source::Embedded(n, b) => {
                    e.embedded.insert(n.as_str(), (b, &x.entry));
                }
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
/// A chunk's payload: as encoded, or already compressed ([`container::pack`]).
enum Payload {
    Raw(u16, Vec<u8>),
    Packed(u16, usize, container::Packed),
    /// A stored chunk, written as it is.
    Stored(Arc<StoredChunk>),
}

/// Threads encoding a save's object chunks at most.
#[cfg(not(target_arch = "wasm32"))]
const MAX_ENCODING_THREADS: usize = 4;

/// What a save wrote besides the chunks: the sheet each object chunk was filed under.
struct Written {
    saved: Vec<Saved>,
    filed: HashMap<usize, (ChunkRef, Vec<Piece>)>,
    sheets: Vec<SheetRect>,
}

/// How each object chunk of `parts` is written ([`sheets::pieces`]: whole, or split where one
/// sheet's objects end and the next's begin, each piece filed under its sheet), from `earlier`
/// for the chunks it filed against the same sheets, else worked out (on all cores).
fn file_chunks(d: &Drawing, parts: &[Part<'_>], sheets: &[SheetRect], earlier: &Earlier<'_>) -> HashMap<usize, (ChunkRef, Vec<Piece>)> {
    let mut out = HashMap::new();
    if sheets.is_empty() {
        return out;
    }
    let same_sheets = earlier.filed.is_some_and(|(_, was)| {
        was.len() == sheets.len() && was.iter().zip(sheets).all(|(a, b)| a.number == b.number && same_rect(&a.rect, &b.rect))
    });
    let mut todo = Vec::new();
    for p in parts {
        let Part::Objects(c) = p else { continue };
        match earlier.filed.filter(|_| same_sheets).and_then(|(f, _)| f.get(&c.key())).filter(|(old, _)| old.same(c)) {
            Some((_, k)) => {
                out.insert(c.key(), (c.clone(), k.clone()));
            }
            None => todo.push(c.clone()),
        }
    }
    let slack = sheets::slack(sheets);
    let found = crate::par_map(&todo, |c| sheets::pieces(d, &slack, c.items()));
    for (c, k) in todo.into_iter().zip(found) {
        out.insert(c.key(), (c, k));
    }
    out
}

fn write_parts(d: &Drawing, w: &mut Writer, earlier: &Earlier<'_>, thumbnail: bool) -> Result<Written, String> {
    let sheet_list = sheet_list(d);
    let parts = parts(d, &sheet_list);
    let filed = file_chunks(d, &parts, &sheet_list, earlier);
    let reused_objects = |c: &ChunkRef| earlier.objects.get(&c.key()).filter(|(old, _)| old.same(c)).map(|(_, e)| *e);
    let reused_piece = |c: &ChunkRef, from: usize, to: usize| {
        earlier.pieces.get(&(c.key(), from)).filter(|(old, end, _)| old.same(c) && *end == to).map(|(_, _, e)| *e)
    };
    // The pieces each object chunk is written as (one, whole, for most).
    let pieces_of = |c: &ChunkRef| -> Vec<Piece> {
        match filed.get(&c.key()) {
            Some((_, p)) if p.len() > 1 => p.clone(),
            Some((_, p)) => vec![(p.first().and_then(|x| x.0), 0, c.items().len())],
            None => vec![(None, 0, c.items().len())],
        }
    };
    let reused_stored = |c: &Arc<StoredChunk>| earlier.stored.get(&(Arc::as_ptr(c) as usize)).copied();
    let origin = |c: &ChunkRef| d.sheet_store.origin_of(c).cloned();
    // The object chunks to write, encoded and compressed on all cores: most of a large drawing's
    // save. Only the compressed bytes are kept until they are written. A chunk decoded from a
    // stored one and not changed since is written as it was stored.
    let fresh: Vec<(&ChunkRef, usize, usize)> = parts
        .iter()
        .filter_map(|p| if let Part::Objects(c) = p { Some(c) } else { None })
        .filter(|c| origin(c).is_none())
        .flat_map(|c| {
            let pieces = pieces_of(c);
            let whole = pieces.len() == 1;
            pieces
                .into_iter()
                .filter(move |(_, from, to)| if whole { reused_objects(c).is_none() } else { reused_piece(c, *from, *to).is_none() })
                .map(move |(_, from, to)| (c, from, to))
        })
        .collect();
    let encode = || -> Vec<Result<((usize, usize), u16, usize, container::Packed), String>> {
        crate::par_map(&fresh, |(c, from, to)| {
            let items = c.items().get(*from..*to).unwrap_or_default();
            model::entities(items).map(|(v, raw)| ((c.key(), *from), v, raw.len(), container::pack(&raw, true)))
        })
    };
    // On a few cores at most: each chunk is its objects as JSON values while it is encoded, and a
    // 10-core machine held ten (a job's checkpoint was its peak memory).
    #[cfg(not(target_arch = "wasm32"))]
    let packed = match rayon::ThreadPoolBuilder::new().num_threads(rayon::current_num_threads().clamp(1, MAX_ENCODING_THREADS)).build() {
        Ok(pool) => pool.install(encode),
        Err(_) => encode(),
    };
    #[cfg(target_arch = "wasm32")]
    let packed = encode();
    let mut packed: HashMap<(usize, usize), (u16, usize, container::Packed)> =
        packed.into_iter().map(|r| r.map(|(k, v, n, p)| (k, (v, n, p)))).collect::<Result<_, _>>()?;
    let mut saved = Vec::new();
    let mut put = |w: &mut Writer,
                   source: Source,
                   reused: Option<&IndexEntry>,
                   tag: [u8; 4],
                   deflate: bool,
                   data: &dyn Fn() -> Result<Payload, String>|
     -> Result<(), String> {
        match reused {
            Some(e) => w.reuse(e),
            None => match data()? {
                Payload::Raw(version, bytes) => w.chunk(tag, version, &bytes, deflate),
                Payload::Packed(version, raw_len, p) => w.chunk_packed(tag, version, raw_len, &p),
                Payload::Stored(c) => w.chunk_stored(tag, c.version, usize::try_from(c.raw_len).unwrap_or(usize::MAX), &c.stored, c.deflated),
            },
        }
        if let Some(entry) = w.entries().last() {
            saved.push(Saved { source, entry: entry.clone() });
        }
        Ok(())
    };
    // A small part: encoded every save, rewritten only when its bytes changed.
    type Put<'p> =
        dyn FnMut(&mut Writer, Source, Option<&IndexEntry>, [u8; 4], bool, &dyn Fn() -> Result<Payload, String>) -> Result<(), String> + 'p;
    let small = |w: &mut Writer, put: &mut Put<'_>, tag: [u8; 4], bytes: Vec<u8>| {
        let key = (tag, crc32fast::hash(&bytes), bytes.len() as u64);
        let reused = earlier.bytes.get(&key).copied();
        put(w, Source::Bytes(key.0, key.1, key.2), reused, tag, true, &|| Ok(Payload::Raw(V1, bytes.clone())))
    };
    // Per ENTS chunk in file order: the sheet it is filed under, its objects and largest handle.
    let mut ents: Vec<(Option<u32>, usize, u64)> = Vec::new();
    let stored_part = |c: &Arc<StoredChunk>| {
        let c = c.clone();
        move || Ok(Payload::Stored(c.clone()))
    };
    for part in parts {
        match part {
            Part::Manifest => small(w, &mut put, *b"MANI", model::manifest(d, thumbnail))?,
            Part::SheetIndex => small(w, &mut put, sheets::TAG, sheets::index_json(&sheet_list, &ents))?,
            Part::Parked(c, k) => {
                ents.push((k, c.objects, c.max_handle));
                let reused = reused_stored(&c);
                put(w, Source::Stored(c.clone()), reused, *b"ENTS", true, &stored_part(&c))?;
            }
            Part::Header => small(w, &mut put, *b"HEAD", model::header(d)?)?,
            Part::Tables => small(w, &mut put, *b"TABL", model::tables(d)?)?,
            Part::Layout(l) => small(w, &mut put, *b"LAYO", model::layout(l)?)?,
            Part::Text(tag, t) => small(w, &mut put, tag, t.as_bytes().to_vec())?,
            Part::Embedded(name, b) => {
                let reused = earlier.embedded.get(name).filter(|(old, _)| Arc::ptr_eq(old, b)).map(|(_, e)| *e);
                put(w, Source::Embedded(name.to_string(), b.clone()), reused, *b"EMBD", !model::packed(name), &|| {
                    Ok(Payload::Raw(V1, model::embedded(name, b)))
                })?;
            }
            Part::Block(name, b) => {
                let reused = earlier.blocks.get(name).filter(|(old, _)| Arc::ptr_eq(old, b)).map(|(_, e)| *e);
                put(w, Source::Block(name.to_string(), b.clone()), reused, *b"BLKS", true, &|| Ok(Payload::Raw(V1, model::block(name, b)?)))?;
            }
            Part::Objects(c) => match origin(&c) {
                Some(sc) => {
                    // Decoded from one sheet's chunk and unchanged: still that sheet's, whole.
                    let k = filed.get(&c.key()).and_then(|(_, p)| match p.as_slice() {
                        [(k, _, _)] => *k,
                        _ => None,
                    });
                    ents.push((k, sc.objects, sc.max_handle));
                    let reused = reused_stored(&sc);
                    put(w, Source::Stored(sc.clone()), reused, *b"ENTS", true, &stored_part(&sc))?;
                }
                None => {
                    let pieces = pieces_of(&c);
                    let whole = pieces.len() == 1;
                    for (k, from, to) in pieces {
                        let items = c.items().get(from..to).unwrap_or_default();
                        let max = items.iter().map(|e| e.handle.0).max().unwrap_or(0);
                        ents.push((k, items.len(), max));
                        let (source, reused) = if whole {
                            (Source::Objects(c.clone()), reused_objects(&c))
                        } else {
                            (Source::Piece(c.clone(), from, to), reused_piece(&c, from, to))
                        };
                        let pre = std::cell::RefCell::new(packed.remove(&(c.key(), from)));
                        put(w, source, reused, *b"ENTS", true, &|| match pre.borrow_mut().take() {
                            Some((version, raw_len, p)) => Ok(Payload::Packed(version, raw_len, p)),
                            None => model::entities(items).map(|(v, b)| Payload::Raw(v, b)),
                        })?;
                    }
                }
            },
            Part::Foreign(f) => {
                let reused = earlier.foreign.filter(|(old, _)| Arc::ptr_eq(old, f)).map(|(_, e)| e);
                put(w, Source::Foreign(f.clone()), reused, *b"FRGN", true, &|| Ok(Payload::Raw(V1, model::foreign(f)?)))?;
            }
        }
    }
    // Parts a newer CADaddy wrote, kept as they are.
    for e in &earlier.other {
        w.reuse(e);
        saved.push(Saved { source: Source::Other, entry: (*e).clone() });
    }
    Ok(Written { saved, filed, sheets: sheet_list })
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
    let Written { mut saved, filed, sheets } = write_parts(d, &mut w, &Earlier::default(), thumbnail.is_some())?;
    if let Some(png) = thumbnail {
        w.chunk(*b"THMB", V1, &png, false);
        if let Some(e) = w.entries().last() {
            saved.push(Saved { source: Source::Thumbnail, entry: e.clone() });
        }
    }
    let bytes = w.finish();
    let len = bytes.len() as u64;
    Ok((bytes, SaveState { len, saved, filed: Arc::new(filed), filed_against: Arc::new(sheets) }))
}

/// Save `d`: appended to the file `prev` describes when that saves enough, else in full (with
/// the preview `thumbnail` makes; an appended save keeps the file's preview).
pub fn save(d: &Drawing, prev: Option<&SaveState>, thumbnail: &dyn Fn() -> Option<Vec<u8>>) -> Result<(Output, SaveState), String> {
    if let Some(prev) = prev {
        let earlier = Earlier::of(prev);
        let mut w = Writer::append(prev.len);
        let Written { mut saved, filed, sheets } = write_parts(d, &mut w, &earlier, earlier.thumbnail.is_some())?;
        if let Some(e) = earlier.thumbnail {
            w.reuse(e);
            saved.push(Saved { source: Source::Thumbnail, entry: e.clone() });
        }
        let (bytes, index_at) = w.finish_split();
        let len = prev.len.saturating_add(bytes.len() as u64);
        let waste = len.saturating_sub(live(&saved));
        if waste <= MIN_WASTE || waste as f64 <= MAX_WASTE * len as f64 {
            return Ok((
                Output::Append { from: prev.len, bytes, index_at },
                SaveState { len, saved, filed: Arc::new(filed), filed_against: Arc::new(sheets) },
            ));
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
    // The chunks a parked read left out of memory, by their place among the ENTS entries.
    let filing = if d.sheet_store.has_parked() { Some(sheets::filing(bytes, &entries)?) } else { None };
    let mut parked: Vec<std::collections::VecDeque<Arc<StoredChunk>>> =
        d.sheet_store.parked.iter().map(|p| p.chunks.iter().cloned().collect()).collect();
    let parked_count: usize = parked.iter().map(std::collections::VecDeque::len).sum();
    let blocks: Vec<(&String, &Arc<Block>)> = d.blocks.iter().collect();
    let count = |tag: &[u8; 4]| entries.iter().filter(|e| &e.tag == tag).count();
    // The reader makes one store chunk and one block per chunk: anything else isn't this file.
    let known = |e: &IndexEntry| model::is_known(&e.tag);
    if count(b"ENTS") != objects.len().saturating_add(parked_count)
        || count(b"BLKS") != blocks.len()
        || count(b"EMBD") != d.embedded.len()
        || entries.iter().any(|e| known(e) && e.version > model::max_version(&e.tag))
    {
        return None;
    }
    let (mut objects, mut blocks) = (objects.into_iter(), blocks.into_iter());
    let mut saved = Vec::with_capacity(entries.len());
    let mut place = 0usize;
    for e in entries {
        let source = match &e.tag {
            b"ENTS" => {
                let at = place;
                place += 1;
                // A parked chunk is the next one of its sheet when its stored bytes are this
                // entry's (a damaged one was read into the store instead).
                let sheet = filing.as_ref().and_then(|f| f.by_place.get(&at)).map(|(k, _, _)| *k);
                let is_it = |c: &StoredChunk| {
                    c.version == e.version
                        && c.stored.len() as u64 == e.len
                        && usize::try_from(e.offset)
                            .ok()
                            .and_then(|o| o.checked_add(CHUNK_HEADER_LEN))
                            .and_then(|o| bytes.get(o..o.checked_add(c.stored.len())?))
                            == Some(c.stored.as_slice())
                };
                match sheet
                    .and_then(|k| parked.get_mut(k))
                    .filter(|q| q.front().is_some_and(|c| is_it(c)))
                    .and_then(std::collections::VecDeque::pop_front)
                {
                    Some(c) => Source::Stored(c),
                    None => Source::Objects(objects.next()?),
                }
            }
            b"BLKS" => {
                let (n, b) = blocks.next()?;
                Source::Block(n.clone(), b.clone())
            }
            b"FRGN" => Source::Foreign(d.foreign.clone()),
            b"EMBD" => {
                let (container::Read::Ok(c), _) = container::chunk_at(bytes, usize::try_from(e.offset).ok()?)? else { return None };
                let (name, _) = model::embedded_from(&c.data)?;
                let b = d.embedded.get(&name)?.clone();
                Source::Embedded(name, b)
            }
            b"THMB" => Source::Thumbnail,
            tag if matches!(tag, b"MANI" | b"HEAD" | b"TABL" | b"LAYO" | b"SHTI") || model::is_text(tag) => {
                let (container::Read::Ok(c), _) = container::chunk_at(bytes, usize::try_from(e.offset).ok()?)? else { return None };
                Source::Bytes(e.tag, crc32fast::hash(&c.data), c.data.len() as u64)
            }
            _ => Source::Other,
        };
        saved.push(Saved { source, entry: e });
    }
    if parked.iter().any(|q| !q.is_empty()) {
        return None;
    }
    Some(SaveState { len: bytes.len() as u64, saved, filed: Arc::default(), filed_against: Arc::default() })
}
