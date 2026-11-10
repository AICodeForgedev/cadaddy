//! The sheets of a print set kept out of memory (Q6418).
//!
//! A job imports a whole print set into one model space: on a hotel over a million objects, of
//! which the screen shows one sheet. In the interface such a drawing holds the building model,
//! the open sheet and a few recent ones; the other sheets' objects stay as the .cdy file stores
//! them, a few compressed chunks each ([`StoredChunk`]), and are decoded only when wanted. Saving
//! writes those chunks back byte for byte. Work that needs the whole set loads them first.
//!
//! What a drawing holds of this is [`SheetStore`]: the set's sheets (where each lies, for the
//! .cdy writer to file each object under its sheet), the parked sheets, and which object chunks
//! were decoded from a stored chunk and are unchanged since (written back as they were stored).
//! A drawing that is not a print set has an empty one, and nothing here changes how it works.

use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_geom::Bounds2;

use crate::store::ChunkRef;

/// One chunk of model space objects as a .cdy file stores it (an `ENTS` chunk's payload, still
/// compressed), not decoded.
#[derive(Debug, PartialEq, Eq)]
pub struct StoredChunk {
    /// The chunk's schema version (`ENTS` 1 or 2).
    pub version: u16,
    /// Its payload's length once inflated.
    pub raw_len: u64,
    /// The payload is deflate-compressed.
    pub deflated: bool,
    /// The payload as stored.
    pub stored: Vec<u8>,
    /// Objects it holds.
    pub objects: usize,
    /// Its largest object handle (new handles must stay above it).
    pub max_handle: u64,
    /// Where its objects go back in model space's draw order (a [`ChunkRef::order`]).
    pub order: u32,
}

/// One sheet of a print set: its number and where it lies in model space.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetRect {
    pub number: String,
    pub rect: Bounds2,
}

/// A sheet whose objects are out of memory: its place and its stored chunks, in draw order.
#[derive(Clone, Debug)]
pub struct ParkedSheet {
    pub number: String,
    pub rect: Bounds2,
    pub chunks: Vec<Arc<StoredChunk>>,
}

impl ParkedSheet {
    /// Objects the sheet holds.
    pub fn objects(&self) -> usize {
        self.chunks.iter().map(|c| c.objects).fold(0, usize::saturating_add)
    }
    /// Whether this is the sheet numbered `number` lying at `rect`.
    pub fn is(&self, number: &str, rect: &Bounds2) -> bool {
        self.number.eq_ignore_ascii_case(number) && same_rect(&self.rect, rect)
    }
}

/// Whether two sheet rectangles are the same place (to a millionth of their size).
pub fn same_rect(a: &Bounds2, b: &Bounds2) -> bool {
    let tol = 1e-6 * (a.width() + a.height()).abs().max(1.0);
    a.min.near(b.min, tol) && a.max.near(b.max, tol)
}

/// What a drawing holds of its print set's sheets beside its objects (see the module).
#[derive(Clone, Default)]
pub struct SheetStore {
    /// The set's sheets, for the .cdy writer to file each object under its sheet; empty for a
    /// drawing that is not a print set.
    pub sheets: Arc<Vec<SheetRect>>,
    /// The sheets whose objects are out of memory.
    pub parked: Arc<Vec<ParkedSheet>>,
    /// Object chunks of model space decoded from a stored chunk, by [`ChunkRef::key`]: while one
    /// is still the same chunk, it is written back as it was stored.
    pub origins: Arc<HashMap<usize, (ChunkRef, Arc<StoredChunk>)>>,
}

impl std::fmt::Debug for SheetStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SheetStore")
            .field("sheets", &self.sheets.len())
            .field("parked", &self.parked.iter().map(|p| (p.number.as_str(), p.objects())).collect::<Vec<_>>())
            .field("origins", &self.origins.len())
            .finish()
    }
}

/// Two drawings hold the same sheets out of memory when they list the same sheets and park the
/// very same stored chunks. Where chunks came from is bookkeeping, not content.
impl PartialEq for SheetStore {
    fn eq(&self, other: &Self) -> bool {
        self.sheets == other.sheets
            && self.parked.len() == other.parked.len()
            && self.parked.iter().zip(other.parked.iter()).all(|(a, b)| {
                a.number == b.number
                    && a.rect == b.rect
                    && a.chunks.len() == b.chunks.len()
                    && a.chunks.iter().zip(&b.chunks).all(|(x, y)| Arc::ptr_eq(x, y))
            })
    }
}

impl SheetStore {
    /// Whether any sheet is out of memory.
    pub fn has_parked(&self) -> bool {
        self.parked.iter().any(|p| !p.chunks.is_empty())
    }
    /// Objects out of memory, over all parked sheets.
    pub fn parked_objects(&self) -> usize {
        self.parked.iter().map(ParkedSheet::objects).fold(0, usize::saturating_add)
    }
    /// The largest handle of the objects out of memory (0 when none).
    pub fn parked_max_handle(&self) -> u64 {
        self.parked.iter().flat_map(|p| p.chunks.iter()).map(|c| c.max_handle).max().unwrap_or(0)
    }
    /// The stored chunk `c` was decoded from, while it is still that very chunk.
    pub fn origin_of(&self, c: &ChunkRef) -> Option<&Arc<StoredChunk>> {
        self.origins.get(&c.key()).filter(|(was, _)| was.same(c)).map(|(_, s)| s)
    }
    /// The parked sheet numbered `number` at `rect`, if it is out of memory.
    pub fn parked_sheet(&self, number: &str, rect: &Bounds2) -> Option<&ParkedSheet> {
        self.parked.iter().find(|p| p.is(number, rect) && !p.chunks.is_empty())
    }
    /// Forget the origins of chunks the store no longer holds as they were (edited, or gone).
    pub fn prune_origins(&mut self, live: &[ChunkRef]) {
        if self.origins.is_empty() {
            return;
        }
        let live: HashMap<usize, &ChunkRef> = live.iter().map(|c| (c.key(), c)).collect();
        let held = |k: &usize, c: &ChunkRef| live.get(k).is_some_and(|l| l.same(c));
        if self.origins.iter().all(|(k, (c, _))| held(k, c)) {
            return;
        }
        let kept = self.origins.iter().filter(|(k, (c, _))| held(k, c)).map(|(k, v)| (*k, v.clone())).collect();
        self.origins = Arc::new(kept);
    }
}
