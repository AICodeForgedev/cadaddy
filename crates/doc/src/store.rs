//! A copy-on-write entity store with draw order.
//!
//! Entities live in chunks (`Arc<Vec<Arc<Entity>>>`) in draw order, and a sharded handle index
//! maps each handle to the stable id of its chunk. Cloning a store (an undo snapshot) copies
//! only the chunk and shard pointers; an edit copies one chunk and one shard.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{Entity, Handle};

const CHUNK_MAX: usize = 1024;
const SHARDS: usize = 64;

#[derive(Clone, Debug, Default)]
struct Chunk {
    id: u32,
    items: Vec<Arc<Entity>>,
}

/// One chunk of an [`EntityStore`], compared by identity: while it is held, an edit to the store
/// copies the chunk it touches, so [`ChunkRef::same`] says exactly whether a chunk is unchanged.
#[derive(Clone, Debug)]
pub struct ChunkRef(Arc<Chunk>);

impl ChunkRef {
    /// Its entities, in draw order.
    pub fn items(&self) -> &[Arc<Entity>] {
        &self.0.items
    }
    pub fn same(&self, other: &ChunkRef) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// A number naming this chunk while it is held (two held chunks never share one): for
    /// looking a chunk up among many.
    pub fn key(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

#[derive(Clone, Debug)]
pub struct EntityStore {
    chunks: Vec<Arc<Chunk>>,
    shards: Vec<Arc<HashMap<Handle, u32>>>,
    len: usize,
    next_chunk: u32,
}

impl Default for EntityStore {
    fn default() -> Self {
        EntityStore { chunks: Vec::new(), shards: (0..SHARDS).map(|_| Arc::new(HashMap::new())).collect(), len: 0, next_chunk: 0 }
    }
}

impl PartialEq for EntityStore {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

/// Where `h` is in a chunk's objects. They are usually in handle order (new objects get higher
/// handles and go on top): a binary search, checked, else a walk (an object moved in the draw
/// order, or a file's own handle order).
fn item_pos(items: &[Arc<Entity>], h: Handle) -> Option<usize> {
    let i = items.partition_point(|e| e.handle < h);
    if items.get(i).is_some_and(|e| e.handle == h) {
        return Some(i);
    }
    items.iter().position(|e| e.handle == h)
}

fn shard_of(h: Handle) -> usize {
    // Fibonacci hashing spreads sequential handles across shards.
    ((h.0.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 58) as usize) % SHARDS
}

impl EntityStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Roughly how many bytes this store holds that `newer` doesn't share with it: the objects of
    /// its chunks that aren't the same allocation in `newer`. Shared chunks are skipped whole, so
    /// the cost follows what changed, not the store's size. Undo uses it to keep a memory budget.
    pub fn unique_bytes(&self, newer: &EntityStore) -> usize {
        let theirs: HashMap<u32, &Arc<Chunk>> = newer.chunks.iter().map(|c| (c.id, c)).collect();
        let mut bytes = 0usize;
        for c in &self.chunks {
            match theirs.get(&c.id) {
                Some(t) if Arc::ptr_eq(c, t) => {}
                Some(t) => {
                    let shared: std::collections::HashSet<*const Entity> = t.items.iter().map(Arc::as_ptr).collect();
                    for e in c.items.iter().filter(|e| !shared.contains(&Arc::as_ptr(e))) {
                        bytes = bytes.saturating_add(e.approx_bytes());
                    }
                }
                None => {
                    for e in &c.items {
                        bytes = bytes.saturating_add(e.approx_bytes());
                    }
                }
            }
        }
        bytes
    }
    /// What changed from this store to `newer`: (added, modified, erased) handles. Chunks both
    /// share are skipped whole, so the cost follows the change, not the store's size.
    pub fn changes(&self, newer: &EntityStore) -> (Vec<Handle>, Vec<Handle>, Vec<Handle>) {
        let theirs: HashMap<u32, &Arc<Chunk>> = newer.chunks.iter().map(|c| (c.id, c)).collect();
        let mine: HashMap<u32, &Arc<Chunk>> = self.chunks.iter().map(|c| (c.id, c)).collect();
        let (mut added, mut modified, mut erased) = (Vec::new(), Vec::new(), Vec::new());
        for c in &self.chunks {
            if theirs.get(&c.id).is_some_and(|t| Arc::ptr_eq(c, t)) {
                continue;
            }
            for e in &c.items {
                match newer.get(e.handle) {
                    Some(n) if Arc::ptr_eq(n, e) => {}
                    Some(_) => modified.push(e.handle),
                    None => erased.push(e.handle),
                }
            }
        }
        for c in &newer.chunks {
            if mine.get(&c.id).is_some_and(|m| Arc::ptr_eq(c, m)) {
                continue;
            }
            added.extend(c.items.iter().filter(|e| !self.contains(e.handle)).map(|e| e.handle));
        }
        (added, modified, erased)
    }
    /// Iterate in draw order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Entity>> + '_ {
        self.chunks.iter().flat_map(|c| c.items.iter())
    }
    pub fn handles(&self) -> Vec<Handle> {
        self.iter().map(|e| e.handle).collect()
    }
    pub fn contains(&self, h: Handle) -> bool {
        self.chunk_id(h).is_some()
    }
    fn chunk_id(&self, h: Handle) -> Option<u32> {
        self.shards.get(shard_of(h)).and_then(|s| s.get(&h).copied())
    }
    /// The position of chunk `id`. Chunks are made in draw order, so their ids usually rise
    /// along it: a binary search finds the chunk without walking them all (a store of 500 000
    /// objects has about 500 chunks, and every lookup walked half of them). Chunks put in front
    /// (send to back) break that order; the search then misses and the walk finds it.
    fn chunk_pos(&self, id: u32) -> Option<usize> {
        let i = self.chunks.partition_point(|c| c.id < id);
        if self.chunks.get(i).is_some_and(|c| c.id == id) {
            return Some(i);
        }
        self.chunks.iter().position(|c| c.id == id)
    }
    pub fn get(&self, h: Handle) -> Option<&Arc<Entity>> {
        let pos = self.chunk_pos(self.chunk_id(h)?)?;
        let items = &self.chunks.get(pos)?.items;
        items.get(item_pos(items, h)?)
    }
    fn set_index(&mut self, h: Handle, chunk: Option<u32>) {
        if let Some(shard) = self.shards.get_mut(shard_of(h)) {
            let m = Arc::make_mut(shard);
            match chunk {
                Some(c) => {
                    m.insert(h, c);
                }
                None => {
                    m.remove(&h);
                }
            }
        }
    }
    fn new_chunk(&mut self) -> Arc<Chunk> {
        let id = self.next_chunk;
        self.next_chunk = self.next_chunk.wrapping_add(1);
        Arc::new(Chunk { id, items: Vec::with_capacity(64) })
    }
    /// Append on top of the draw order. Replaces an entity with the same handle in place.
    pub fn push(&mut self, e: Entity) {
        if self.contains(e.handle) {
            self.replace(e);
            return;
        }
        let needs_new = self.chunks.last().is_none_or(|c| c.items.len() >= CHUNK_MAX);
        if needs_new {
            let c = self.new_chunk();
            self.chunks.push(c);
        }
        let h = e.handle;
        let Some(last) = self.chunks.last_mut() else { return };
        let chunk = Arc::make_mut(last);
        chunk.items.push(Arc::new(e));
        let id = chunk.id;
        self.set_index(h, Some(id));
        self.len += 1;
    }
    /// Append many new objects on top of the draw order, in order: the same result as
    /// [`EntityStore::push`] one at a time (chunks filled the same way), with the handle index
    /// grown once per shard. An object whose handle is already here makes it fall back to
    /// `push` (which replaces it in place).
    pub fn push_many(&mut self, items: Vec<Entity>) {
        if items.iter().any(|e| self.contains(e.handle)) {
            for e in items {
                self.push(e);
            }
            return;
        }
        let mut index: Vec<Vec<(Handle, u32)>> = (0..SHARDS).map(|_| Vec::new()).collect();
        for e in items {
            if self.chunks.last().is_none_or(|c| c.items.len() >= CHUNK_MAX) {
                let c = self.new_chunk();
                self.chunks.push(c);
            }
            let Some(last) = self.chunks.last_mut() else { return };
            let chunk = Arc::make_mut(last);
            let h = e.handle;
            chunk.items.push(Arc::new(e));
            if let Some(list) = index.get_mut(shard_of(h)) {
                list.push((h, chunk.id));
            }
            self.len += 1;
        }
        for (shard, list) in self.shards.iter_mut().zip(index) {
            if list.is_empty() {
                continue;
            }
            let m = Arc::make_mut(shard);
            m.reserve(list.len());
            m.extend(list);
        }
    }
    /// Insert at the bottom of the draw order.
    pub fn push_front(&mut self, e: Entity) {
        if self.contains(e.handle) {
            self.replace(e);
            return;
        }
        let needs_new = self.chunks.first().is_none_or(|c| c.items.len() >= CHUNK_MAX);
        if needs_new {
            let c = self.new_chunk();
            self.chunks.insert(0, c);
        }
        let h = e.handle;
        let Some(first) = self.chunks.first_mut() else { return };
        let chunk = Arc::make_mut(first);
        chunk.items.insert(0, Arc::new(e));
        let id = chunk.id;
        self.set_index(h, Some(id));
        self.len += 1;
    }
    /// Replace the entity with the same handle, keeping its draw position. Returns false if absent.
    pub fn replace(&mut self, e: Entity) -> bool {
        let Some(id) = self.chunk_id(e.handle) else { return false };
        let Some(pos) = self.chunk_pos(id) else { return false };
        let Some(c) = self.chunks.get_mut(pos) else { return false };
        let chunk = Arc::make_mut(c);
        match item_pos(&chunk.items, e.handle).and_then(|i| chunk.items.get_mut(i)) {
            Some(slot) => {
                *slot = Arc::new(e);
                true
            }
            None => false,
        }
    }
    pub fn remove(&mut self, h: Handle) -> Option<Arc<Entity>> {
        let id = self.chunk_id(h)?;
        let pos = self.chunk_pos(id)?;
        let c = self.chunks.get_mut(pos)?;
        let chunk = Arc::make_mut(c);
        let i = item_pos(&chunk.items, h)?;
        let e = chunk.items.remove(i);
        let empty = chunk.items.is_empty();
        if empty {
            self.chunks.remove(pos);
        }
        self.set_index(h, None);
        self.len = self.len.saturating_sub(1);
        Some(e)
    }
    /// Modify an entity in place through a closure (copy-on-write).
    pub fn modify<F: FnOnce(&mut Entity)>(&mut self, h: Handle, f: F) -> bool {
        let Some(id) = self.chunk_id(h) else { return false };
        let Some(pos) = self.chunk_pos(id) else { return false };
        let Some(c) = self.chunks.get_mut(pos) else { return false };
        let chunk = Arc::make_mut(c);
        match item_pos(&chunk.items, h).and_then(|i| chunk.items.get_mut(i)) {
            Some(slot) => {
                f(Arc::make_mut(slot));
                true
            }
            None => false,
        }
    }
    /// The objects of every chunk holding one `wanted` says yes to, writable (each such chunk
    /// copied when shared, as [`EntityStore::modify`] does one): edits to many objects at once,
    /// chunk by chunk (on several threads when the caller likes) instead of one lookup each.
    /// Handles and draw order must not change through them.
    pub fn chunks_mut_where(&mut self, wanted: impl Fn(Handle) -> bool) -> Vec<&mut [Arc<Entity>]> {
        self.chunks.iter_mut().filter(|c| c.items.iter().any(|e| wanted(e.handle))).map(|c| Arc::make_mut(c).items.as_mut_slice()).collect()
    }
    pub fn bring_to_front(&mut self, h: Handle) -> bool {
        match self.remove(h) {
            Some(e) => {
                self.push(Arc::unwrap_or_clone(e));
                true
            }
            None => false,
        }
    }
    pub fn send_to_back(&mut self, h: Handle) -> bool {
        match self.remove(h) {
            Some(e) => {
                self.push_front(Arc::unwrap_or_clone(e));
                true
            }
            None => false,
        }
    }
    /// Move `h` directly above (`above`) or under `anchor` in the draw order. False (and nothing
    /// moves) when either is absent or they are the same object.
    pub fn move_next_to(&mut self, h: Handle, anchor: Handle, above: bool) -> bool {
        if h == anchor || !self.contains(anchor) {
            return false;
        }
        let Some(e) = self.remove(h) else { return false };
        let at = self.chunk_id(anchor).and_then(|id| Some((id, self.chunk_pos(id)?)));
        let Some((id, pos)) = at else {
            // Unreachable in practice (the anchor was present); keep the object on top.
            self.push(Arc::unwrap_or_clone(e));
            return false;
        };
        let Some(c) = self.chunks.get_mut(pos) else {
            // Unreachable in practice too; never lose the object.
            self.push(Arc::unwrap_or_clone(e));
            return false;
        };
        let chunk = Arc::make_mut(c);
        let i = chunk.items.iter().position(|x| x.handle == anchor).unwrap_or(chunk.items.len());
        let i = if above { (i + 1).min(chunk.items.len()) } else { i };
        chunk.items.insert(i, e);
        let split = chunk.items.len() > 2 * CHUNK_MAX;
        self.set_index(h, Some(id));
        self.len += 1;
        if split {
            self.split_chunk(pos);
        }
        true
    }
    /// Split an oversized chunk in two, keeping the order.
    fn split_chunk(&mut self, pos: usize) {
        let mut fresh = self.new_chunk();
        let Some(c) = self.chunks.get_mut(pos) else { return };
        let chunk = Arc::make_mut(c);
        let tail = chunk.items.split_off(chunk.items.len() / 2);
        let new = Arc::make_mut(&mut fresh);
        new.items = tail;
        let (id, moved): (u32, Vec<Handle>) = (new.id, new.items.iter().map(|e| e.handle).collect());
        self.chunks.insert(pos + 1, fresh);
        for h in moved {
            self.set_index(h, Some(id));
        }
    }
    /// Reorder the whole store by a key (stable: equal keys keep their order).
    pub fn sort_by_key<K: Ord>(&mut self, mut key: impl FnMut(&Entity) -> K) {
        let mut items: Vec<Arc<Entity>> = self.iter().cloned().collect();
        items.sort_by_cached_key(|e| key(e));
        *self = EntityStore::default();
        for e in items {
            let needs_new = self.chunks.last().is_none_or(|c| c.items.len() >= CHUNK_MAX);
            if needs_new {
                let c = self.new_chunk();
                self.chunks.push(c);
            }
            let h = e.handle;
            let Some(last) = self.chunks.last_mut() else { return };
            let chunk = Arc::make_mut(last);
            chunk.items.push(e);
            let id = chunk.id;
            self.set_index(h, Some(id));
            self.len += 1;
        }
    }
    /// The store's chunks in draw order, as handles that compare by identity (a saved file's
    /// incremental save rewrites only the chunks that are no longer the same).
    pub fn chunk_refs(&self) -> Vec<ChunkRef> {
        self.chunks.iter().cloned().map(ChunkRef).collect()
    }
    /// Append `items` on top of the draw order as chunks of their own (split at the chunk size),
    /// so a file read back has the chunks it was saved with. An entity whose handle is already
    /// present replaces it in place, as [`EntityStore::push`] does.
    pub fn push_chunk(&mut self, items: Vec<Entity>) {
        let mut fresh: Option<Arc<Chunk>> = None;
        for e in items {
            if self.contains(e.handle) {
                self.replace(e);
                continue;
            }
            if fresh.as_ref().is_none_or(|c| c.items.len() >= CHUNK_MAX) {
                if let Some(done) = fresh.take() {
                    self.chunks.push(done);
                }
                fresh = Some(self.new_chunk());
            }
            let Some(c) = fresh.as_mut() else { return };
            let chunk = Arc::make_mut(c);
            let h = e.handle;
            chunk.items.push(Arc::new(e));
            let id = chunk.id;
            self.set_index(h, Some(id));
            self.len += 1;
        }
        if let Some(done) = fresh {
            self.chunks.push(done);
        }
    }
    /// True when `other` shares every chunk with `self` (same draw order, same entities). Holding
    /// a clone of a store makes this exact: any edit to the original copies the touched chunk.
    pub fn same_as(&self, other: &EntityStore) -> bool {
        self.len == other.len && self.chunks.len() == other.chunks.len() && self.chunks.iter().zip(&other.chunks).all(|(a, b)| Arc::ptr_eq(a, b))
    }
    /// The chunks in draw order as (identity key, entities). A key is stable for as long as any
    /// clone of this store is alive and changes whenever the chunk is edited (copy-on-write), so
    /// caches holding a clone can reuse per-chunk work.
    pub fn chunk_slices(&self) -> impl Iterator<Item = (usize, &[Arc<Entity>])> + '_ {
        self.chunks.iter().map(|c| (Arc::as_ptr(c) as usize, c.items.as_slice()))
    }
    /// The last entity in draw order (AutoCAD's "Last" selection).
    pub fn last(&self) -> Option<&Arc<Entity>> {
        self.chunks.last().and_then(|c| c.items.last())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntityKind, Point};
    use cadaddy_geom::Vec2;

    fn ent(h: u64) -> Entity {
        Entity::new(Handle(h), EntityKind::Point(Point { p: Vec2::new(h as f64, 0.0).to3(0.0), angle: 0.0 }))
    }

    #[test]
    fn chunks_compare_by_identity_and_push_chunk_keeps_boundaries() {
        let mut s = EntityStore::new();
        s.push_chunk((1..=10).map(ent).collect());
        s.push_chunk((11..=13).map(ent).collect());
        s.push_chunk(Vec::new());
        s.push_chunk((1..=CHUNK_MAX as u64 + 5).map(|h| ent(h + 100)).collect());
        let sizes: Vec<usize> = s.chunk_refs().iter().map(|c| c.items().len()).collect();
        assert_eq!(sizes, [10, 3, CHUNK_MAX, 5], "the chunks pushed, split at the chunk size");
        assert_eq!(s.len(), 13 + CHUNK_MAX + 5);
        assert!(s.get(Handle(12)).is_some());
        // A duplicate handle replaces in place.
        s.push_chunk(vec![ent(12)]);
        assert_eq!(s.chunk_refs().len(), 4);
        // Holding the chunks, an edit copies only the touched one.
        let before = s.chunk_refs();
        s.modify(Handle(12), |e| e.common.layer = "A".into());
        let after = s.chunk_refs();
        let same: Vec<bool> = before.iter().zip(&after).map(|(a, b)| a.same(b)).collect();
        assert_eq!(same, [true, false, true, true]);
    }

    #[test]
    fn push_get_remove() {
        let mut s = EntityStore::new();
        for h in 1..=3000 {
            s.push(ent(h));
        }
        assert_eq!(s.len(), 3000);
        assert!(s.get(Handle(1500)).is_some());
        assert!(s.remove(Handle(1500)).is_some());
        assert!(s.get(Handle(1500)).is_none());
        assert_eq!(s.len(), 2999);
        assert_eq!(s.iter().count(), 2999);
    }

    #[test]
    fn snapshots_are_independent() {
        let mut s = EntityStore::new();
        for h in 1..=10 {
            s.push(ent(h));
        }
        let snap = s.clone();
        s.remove(Handle(3));
        s.modify(Handle(4), |e| e.common.layer = "X".into());
        assert!(snap.get(Handle(3)).is_some());
        assert_eq!(snap.get(Handle(4)).map(|e| e.common.layer.as_str()), Some("0"));
        assert_eq!(s.get(Handle(4)).map(|e| e.common.layer.as_str()), Some("X"));
    }

    #[test]
    fn draw_order_moves() {
        let mut s = EntityStore::new();
        for h in 1..=5 {
            s.push(ent(h));
        }
        s.bring_to_front(Handle(1));
        assert_eq!(s.last().map(|e| e.handle), Some(Handle(1)));
        s.send_to_back(Handle(5));
        assert_eq!(s.iter().next().map(|e| e.handle), Some(Handle(5)));
        assert_eq!(s.len(), 5);
    }

    fn order(s: &EntityStore) -> Vec<u64> {
        s.iter().map(|e| e.handle.0).collect()
    }

    #[test]
    fn move_next_to_and_sort() {
        let mut s = EntityStore::new();
        for h in 1..=5 {
            s.push(ent(h));
        }
        assert!(s.move_next_to(Handle(1), Handle(3), true));
        assert_eq!(order(&s), [2, 3, 1, 4, 5]);
        assert!(s.move_next_to(Handle(5), Handle(2), false));
        assert_eq!(order(&s), [5, 2, 3, 1, 4]);
        assert!(!s.move_next_to(Handle(5), Handle(5), true));
        assert!(!s.move_next_to(Handle(5), Handle(99), true));
        assert!(!s.move_next_to(Handle(99), Handle(5), true));
        assert_eq!(order(&s), [5, 2, 3, 1, 4]);
        assert_eq!(s.len(), 5);
        s.sort_by_key(|e| e.handle.0 % 2);
        assert_eq!(order(&s), [2, 4, 5, 3, 1]);
        assert!(s.get(Handle(3)).is_some());
    }

    #[test]
    fn move_next_to_splits_big_chunks() {
        let mut s = EntityStore::new();
        for h in 1..=3000 {
            s.push(ent(h));
        }
        // Pile everything into the first chunk, under handle 1.
        for h in 1500..=3000 {
            assert!(s.move_next_to(Handle(h), Handle(1), true));
        }
        assert_eq!(s.len(), 3000);
        assert!(s.chunks.iter().all(|c| c.items.len() <= 2 * CHUNK_MAX));
        let o = order(&s);
        assert_eq!(o.first(), Some(&1));
        assert_eq!(o.get(1), Some(&3000));
        for h in 1..=3000 {
            assert_eq!(s.get(Handle(h)).map(|e| e.handle.0), Some(h));
        }
    }
}
