//! Work done per chunk of a drawing's model, kept while the chunk is unchanged.
//!
//! A model's [`EntityStore`] keeps its objects in copy-on-write chunks; while a [`ChunkRef`] is
//! held, an edit copies the chunk it touches, so a chunk that is still the same object is
//! exactly unchanged. A [`ChunkPool`] keeps, per chunk, what a caller worked out from it (the
//! elements and their properties for `bim.props.find`, the plan objects' bounds for an
//! override's source signature) and gives it back for every chunk that has not changed since:
//! a call after an edit only works out the chunks the edit touched, and is given the part the
//! chunk had before the edit (the same place in the draw order) so that it can keep what it
//! worked out for the objects the edit did not touch: those are still the very same objects
//! ([`std::sync::Arc::ptr_eq`]; an edited object is a new one).
//!
//! The pool holds the chunks of the last store it was asked about (one drawing; switching
//! drawings works the other's chunks out again), and is let go of when a drawing closes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use cadaddy_doc::{ChunkRef, EntityStore};

/// The kept work of one chunk.
pub(crate) struct Part<T> {
    pub chunk: ChunkRef,
    pub data: Arc<T>,
}

impl<T> Clone for Part<T> {
    fn clone(&self) -> Self {
        Part { chunk: self.chunk.clone(), data: Arc::clone(&self.data) }
    }
}

/// Per chunk work, worked out under a context `C` (whatever else of the drawing it depends on:
/// a different context forgets every part).
pub(crate) struct ChunkPool<T, C> {
    inner: Mutex<Option<HashMap<Owner, (C, HashMap<usize, Part<T>>)>>>,
}

/// Whose parts: everyone's in a program, each thread's own in the unit tests (they run side by
/// side and would otherwise take each other's parts away).
type Owner = Option<std::thread::ThreadId>;

fn owner() -> Owner {
    cfg!(test).then(|| std::thread::current().id())
}

/// How many chunks one pass works out (the rest come from the pool).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Refresh {
    pub built: usize,
    pub reused: usize,
}

impl<T: Send + Sync, C: PartialEq> ChunkPool<T, C> {
    pub const fn new() -> Self {
        ChunkPool { inner: Mutex::new(None) }
    }

    /// The parts of `store`'s chunks in draw order: the kept ones for unchanged chunks (under the
    /// same context), `build` for the others (side by side where threads are), given the part
    /// the chunk had before it changed, if there was one.
    pub fn parts(&self, store: &EntityStore, ctx: C, build: impl Fn(&ChunkRef, Option<&Part<T>>) -> T + Sync) -> (Vec<Part<T>>, Refresh) {
        let chunks = store.chunk_refs();
        let mut old: HashMap<usize, Part<T>> = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            match g.as_mut().and_then(|m| m.remove(&owner())) {
                Some((c, parts)) if c == ctx => parts,
                _ => HashMap::new(),
            }
        };
        let mut slots: Vec<Option<Part<T>>> = chunks.iter().map(|c| old.remove(&c.key()).filter(|p| p.chunk.same(c))).collect();
        // What the changed chunks were (by their place in the draw order).
        let earlier: HashMap<u32, Part<T>> = old.into_values().map(|p| (p.chunk.order(), p)).collect();
        let stale: Vec<usize> = slots.iter().enumerate().filter(|(_, p)| p.is_none()).map(|(i, _)| i).collect();
        let r = Refresh { built: stale.len(), reused: chunks.len() - stale.len() };
        #[cfg(not(target_arch = "wasm32"))]
        let fresh: Vec<(usize, T)> = {
            use rayon::prelude::*;
            stale.par_iter().filter_map(|&i| chunks.get(i).map(|c| (i, build(c, earlier.get(&c.order()))))).collect()
        };
        #[cfg(target_arch = "wasm32")]
        let fresh: Vec<(usize, T)> = stale.iter().filter_map(|&i| chunks.get(i).map(|c| (i, build(c, earlier.get(&c.order()))))).collect();
        drop(earlier);
        for (i, data) in fresh {
            if let (Some(slot), Some(c)) = (slots.get_mut(i), chunks.get(i)) {
                *slot = Some(Part { chunk: c.clone(), data: Arc::new(data) });
            }
        }
        let parts: Vec<Part<T>> = slots.into_iter().flatten().collect();
        let keep: HashMap<usize, Part<T>> = parts.iter().map(|p| (p.chunk.key(), p.clone())).collect();
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(HashMap::new).insert(owner(), (ctx, keep));
        (parts, r)
    }

    /// Forget every part (the drawings they came from may be closed; in the unit tests, this
    /// thread's).
    pub fn clear(&self) {
        if let Some(m) = self.inner.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
            m.remove(&owner());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use cadaddy_doc::{Circle, Entity, EntityKind, Handle};
    use cadaddy_geom::Vec3;

    fn store(n: u64) -> EntityStore {
        let mut s = EntityStore::new();
        for i in 0..n {
            s.push(Entity::new(Handle(0x100 + i), EntityKind::Circle(Circle { center: Vec3::new(i as f64, 0.0, 0.0), radius: 1.0 })));
        }
        s
    }

    #[test]
    fn unchanged_chunks_are_reused_and_an_edited_one_is_worked_out_again() {
        static POOL: ChunkPool<usize, u32> = ChunkPool::new();
        let mut s = store(5000);
        let count = |c: &ChunkRef, _: Option<&Part<usize>>| c.items().len();
        let (a, r) = POOL.parts(&s, 1, count);
        assert_eq!(r.reused, 0);
        assert_eq!(a.iter().map(|p| *p.data).sum::<usize>(), 5000);
        let (_, r) = POOL.parts(&s, 1, count);
        assert_eq!((r.built, r.reused), (0, a.len()));
        assert!(s.modify(Handle(0x100 + 10), |e| e.common.layer = "MOVED".into()));
        let (b, r) = POOL.parts(&s, 1, count);
        assert_eq!((r.built, r.reused), (1, a.len() - 1));
        assert_eq!(b.len(), a.len());
        // The edited chunk is given what it was.
        static POOL2: ChunkPool<usize, u32> = ChunkPool::new();
        let _ = POOL2.parts(&s, 1, count);
        assert!(s.modify(Handle(0x100 + 20), |e| e.common.layer = "AGAIN".into()));
        let given = std::sync::Mutex::new(Vec::new());
        let _ = POOL2.parts(&s, 1, |c, prev| {
            given.lock().unwrap().push(prev.map(|p| (p.chunk.order() == c.order(), p.chunk.items().len())));
            c.items().len()
        });
        assert_eq!(given.into_inner().unwrap(), vec![Some((true, 1024))]);
        // Another context: everything again.
        let (_, r) = POOL.parts(&s, 2, count);
        assert_eq!(r.built, b.len());
        POOL.clear();
        let (_, r) = POOL.parts(&s, 2, count);
        assert_eq!(r.built, b.len());
    }
}
