//! Incremental display lists for the screen: model space kept as one list per chunk of the
//! drawing's entity store ([`cadaddy_doc::ChunkRef`], 1024 objects each, compared by identity).
//! After an edit only the chunks that changed are built again and the parts are joined in
//! drawing order, so the list is the one a full [`build`](super::build) gives. Anything else that
//! shapes how objects look (layers, styles, blocks, header variables, field values, images, the
//! build options) changing rebuilds every part; layouts, plot styles and lists past the block
//! expansion budget are always built in full.

use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_doc::{ChunkRef, Drawing, Space};

use super::*;

/// What every part was built against: the drawing's tables and settings, and the build options.
#[derive(Clone, Debug, PartialEq)]
struct Context {
    header: cadaddy_doc::Header,
    layers: Vec<cadaddy_doc::Layer>,
    linetypes: Vec<cadaddy_doc::Linetype>,
    text_styles: Vec<cadaddy_doc::TextStyle>,
    dim_styles: Vec<cadaddy_doc::DimStyle>,
    mleader_styles: Vec<cadaddy_doc::MLeaderStyle>,
    table_styles: Vec<cadaddy_doc::TableStyle>,
    mline_styles: Vec<cadaddy_doc::MLineStyle>,
    /// Blocks by name and identity (a block is shared until it changes).
    blocks: Vec<(String, usize)>,
    field_values: std::collections::BTreeMap<cadaddy_doc::Handle, Vec<String>>,
    scales: Vec<cadaddy_doc::AnnoScale>,
    materials: Vec<cadaddy_doc::material::Material>,
    images: u64,
    opts: String,
}

impl Context {
    fn of(d: &Drawing, opts: &Options) -> Context {
        Context {
            header: d.header.clone(),
            layers: d.layers.clone(),
            linetypes: d.linetypes.clone(),
            text_styles: d.text_styles.clone(),
            dim_styles: d.dim_styles.clone(),
            mleader_styles: d.mleader_styles.clone(),
            table_styles: d.table_styles.clone(),
            mline_styles: d.mline_styles.clone(),
            blocks: d.blocks.iter().map(|(n, b)| (n.clone(), Arc::as_ptr(b) as usize)).collect(),
            field_values: d.field_values.clone(),
            scales: d.scales.clone(),
            materials: d.materials.clone(),
            images: images::generation(),
            opts: options_key(opts),
        }
    }
}

/// The build options that change what a list holds (not the cancel flag).
fn options_key(o: &Options) -> String {
    format!(
        "{:x}|{:x}|{}|{}|{}|{:x}|{:?}|{}|{:?}|{:?}|{}|{}",
        o.tolerance.to_bits(),
        o.min_dash.to_bits(),
        o.text,
        o.fill,
        o.lineweights,
        o.view_height.to_bits(),
        o.view.map(|b| [b.min.x.to_bits(), b.min.y.to_bits(), b.max.x.to_bits(), b.max.y.to_bits()]),
        o.transparency,
        o.skip_viewport,
        o.projection,
        o.field_shading,
        o.plot_style_table.is_some(),
    )
}

/// One chunk's list.
struct Part {
    chunk: ChunkRef,
    list: Arc<DisplayList>,
}

/// Where one part sits in the joined list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Span {
    prims: usize,
    verts: usize,
    tris: usize,
}

/// The parts of the model-space list kept between builds (one per canvas).
#[derive(Default)]
pub struct ChunkCache {
    ctx: Option<Context>,
    parts: Vec<Part>,
    /// Where each part starts in the list last returned, and that list's buffers and lengths
    /// (to know it when it is handed back to [`build_cached_into`]).
    spans: Vec<Span>,
    last: Option<(usize, usize, usize, usize, usize, usize)>,
    /// The last build patched the list handed back in place.
    pub patched: bool,
    /// Chunks built again by the last [`build_cached`] (for measuring).
    pub rebuilt: usize,
    pub reused: usize,
}

impl ChunkCache {
    pub fn clear(&mut self) {
        self.ctx = None;
        self.parts.clear();
        self.spans.clear();
        self.last = None;
    }
}

impl std::fmt::Debug for ChunkCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkCache").field("parts", &self.parts.len()).field("rebuilt", &self.rebuilt).field("reused", &self.reused).finish()
    }
}

/// Append a copy of `src` to `dst` (its primitives index into `dst`'s arrays).
fn append_ref(dst: &mut DisplayList, src: &DisplayList) {
    let (verts, tris, prims, images) = (dst.verts.len() as u32, dst.tris.len() as u32, dst.prims.len() as u32, dst.images.len() as u32);
    if !src.pens.is_empty() {
        dst.pens.resize(dst.prims.len(), Pen::default());
    }
    dst.prims.extend(src.prims.iter().map(|p| {
        let mut p = *p;
        p.start += if p.kind.is_fill() { tris } else { verts };
        if let Kind::Image(i) = &mut p.kind {
            *i += images;
        }
        p
    }));
    dst.images.extend(src.images.iter().cloned());
    dst.verts.extend_from_slice(&src.verts);
    dst.tris.extend_from_slice(&src.tris);
    dst.prim_layers.extend_from_slice(&src.prim_layers);
    dst.texts.extend(src.texts.iter().cloned().map(|mut t| {
        if t.prim < PlotText::CLIPPED {
            t.prim += prims;
        }
        t
    }));
    dst.words.extend(src.words.iter().cloned());
    dst.pens.extend_from_slice(&src.pens);
    dst.bounds = dst.bounds.union(&src.bounds);
    dst.expanded = dst.expanded.saturating_add(src.expanded);
}

/// The list of one chunk's objects.
fn build_chunk(d: &Drawing, chunk: &ChunkRef, opts: &Options) -> DisplayList {
    let mut b = Builder { list: DisplayList::default(), opts, plotting: false, styles: None, named: None, shared_budget: None, transparency: 0 };
    for e in chunk.items() {
        top_entity(&mut b, d, e);
    }
    b.list
}

/// [`build`](super::build) for the screen, reusing the parts of `cache` whose chunks haven't
/// changed since they were built (and keeping the new parts there for next time).
pub fn build_cached(d: &Drawing, space: &Space, opts: &Options, cache: &mut ChunkCache) -> DisplayList {
    build_cached_into(d, space, opts, cache, None)
}

/// The list's identity: its buffers and lengths.
fn fingerprint(l: &DisplayList) -> (usize, usize, usize, usize, usize, usize) {
    (l.prims.as_ptr() as usize, l.verts.as_ptr() as usize, l.tris.as_ptr() as usize, l.prims.len(), l.verts.len(), l.tris.len())
}

/// Whether a part holds only primitives and their points (nothing a patch would have to move).
fn plain(l: &DisplayList) -> bool {
    l.images.is_empty() && l.texts.is_empty() && l.words.is_empty() && l.pens.is_empty() && l.prim_layers.is_empty()
}

/// [`build_cached`], given the list it returned last time (`prev`, the canvas's): when the
/// chunks that changed give parts of the same sizes as before (an object changed in place, as
/// moving or recolouring does), they are written into `prev` where their old parts were instead
/// of joining every part again.
pub fn build_cached_into(d: &Drawing, space: &Space, opts: &Options, cache: &mut ChunkCache, prev: Option<DisplayList>) -> DisplayList {
    let opts = &with_point_view(d, space, opts);
    // Only model space on screen, without a plot style table, is kept in parts.
    if !matches!(space, Space::Model) || style_table(d, space, opts, false).is_some() {
        cache.clear();
        return build(d, space, opts);
    }
    let ctx = Context::of(d, opts);
    if cache.ctx.as_ref() != Some(&ctx) {
        cache.parts.clear();
        cache.ctx = Some(ctx);
    }
    let chunks = d.model.chunk_refs();
    let before: Vec<(usize, Arc<DisplayList>)> = cache.parts.iter().map(|p| (p.chunk.key(), Arc::clone(&p.list))).collect();
    let mut old: HashMap<usize, Arc<DisplayList>> = cache.parts.drain(..).map(|p| (p.chunk.key(), p.list)).collect();
    // Chunks kept as they were reuse their parts; the others are built (side by side).
    let mut parts: Vec<Option<Arc<DisplayList>>> = chunks.iter().map(|c| old.remove(&c.key())).collect();
    drop(old);
    let stale: Vec<usize> = parts.iter().enumerate().filter(|(_, p)| p.is_none()).map(|(i, _)| i).collect();
    cache.rebuilt = stale.len();
    cache.reused = chunks.len() - stale.len();
    #[cfg(not(target_arch = "wasm32"))]
    let built: Vec<(usize, DisplayList)> = {
        use rayon::prelude::*;
        stale.par_iter().filter_map(|&i| chunks.get(i).map(|c| (i, build_chunk(d, c, opts)))).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let built: Vec<(usize, DisplayList)> = stale.iter().filter_map(|&i| chunks.get(i).map(|c| (i, build_chunk(d, c, opts)))).collect();
    if opts.cancelled() {
        cache.clear();
        return DisplayList::default();
    }
    for (i, list) in built {
        if let Some(slot) = parts.get_mut(i) {
            *slot = Some(Arc::new(list));
        }
    }
    let lists: Vec<Arc<DisplayList>> = parts.into_iter().flatten().collect();
    // Past the block expansion budget the full build cuts later chunks short: build it so.
    let expanded: usize = lists.iter().map(|l| l.expanded).fold(0, usize::saturating_add);
    if expanded >= MAX_BLOCK_EXPANSION || lists.len() != chunks.len() {
        cache.clear();
        return build(d, space, opts);
    }
    cache.patched = false;
    // In place: the same number of chunks, each changed one the same size as the part it
    // replaces, in the list this cache returned last.
    if let Some(mut out) = prev.filter(|p| cache.last == Some(fingerprint(p)) && before.len() == lists.len() && cache.spans.len() == lists.len()) {
        let fits = lists.iter().zip(&before).zip(&chunks).all(|((new, (key, old)), c)| {
            c.key() == *key && Arc::ptr_eq(new, old)
                || plain(new)
                    && plain(old)
                    && new.prims.len() == old.prims.len()
                    && new.verts.len() == old.verts.len()
                    && new.tris.len() == old.tris.len()
        });
        if fits {
            let mut bounds = Bounds2::EMPTY;
            for (i, (new, (_, old))) in lists.iter().zip(&before).enumerate() {
                bounds = bounds.union(&new.bounds);
                if Arc::ptr_eq(new, old) {
                    continue;
                }
                let Some(sp) = cache.spans.get(i).copied() else { continue };
                for (k, p) in new.prims.iter().enumerate() {
                    if let Some(slot) = out.prims.get_mut(sp.prims + k) {
                        let mut q = *p;
                        q.start += if q.kind.is_fill() { sp.tris as u32 } else { sp.verts as u32 };
                        *slot = q;
                    }
                }
                if let Some(dst) = out.verts.get_mut(sp.verts..sp.verts + new.verts.len()) {
                    dst.copy_from_slice(&new.verts);
                }
                if let Some(dst) = out.tris.get_mut(sp.tris..sp.tris + new.tris.len()) {
                    dst.copy_from_slice(&new.tris);
                }
            }
            out.bounds = bounds;
            out.expanded = expanded;
            cache.patched = true;
            cache.last = Some(fingerprint(&out));
            cache.parts = chunks.into_iter().zip(lists).map(|(chunk, list)| Part { chunk, list }).collect();
            return out;
        }
        // One chunk changed size (an object of another shape): its part spliced in, the
        // primitives after it pointed at their moved points.
        let changed: Vec<usize> = lists.iter().zip(&before).enumerate().filter(|(_, (n, (_, o)))| !Arc::ptr_eq(n, o)).map(|(i, _)| i).collect();
        let same_chunks = lists.iter().zip(&before).zip(&chunks).all(|((n, (key, o)), c)| !Arc::ptr_eq(n, o) || c.key() == *key);
        if let ([i], true) = (changed.as_slice(), same_chunks)
            && let (Some(new), Some((_, old)), Some(sp)) = (lists.get(*i), before.get(*i), cache.spans.get(*i).copied())
            && plain(new)
            && plain(old)
            && plain(&out)
        {
            let (dp, dv, dt) = (
                new.prims.len() as i64 - old.prims.len() as i64,
                new.verts.len() as i64 - old.verts.len() as i64,
                new.tris.len() as i64 - old.tris.len() as i64,
            );
            let shift = |v: u32, d: i64| u32::try_from(i64::from(v) + d).unwrap_or(v);
            let placed: Vec<DPrim> = new
                .prims
                .iter()
                .map(|p| {
                    let mut q = *p;
                    q.start += if q.kind.is_fill() { sp.tris as u32 } else { sp.verts as u32 };
                    q
                })
                .collect();
            let tail = sp.prims + old.prims.len();
            if tail <= out.prims.len() && sp.verts + old.verts.len() <= out.verts.len() && sp.tris + old.tris.len() <= out.tris.len() {
                out.prims.splice(sp.prims..tail, placed);
                for q in out.prims.iter_mut().skip(sp.prims + new.prims.len()) {
                    q.start = if q.kind.is_fill() { shift(q.start, dt) } else { shift(q.start, dv) };
                }
                out.verts.splice(sp.verts..sp.verts + old.verts.len(), new.verts.iter().copied());
                out.tris.splice(sp.tris..sp.tris + old.tris.len(), new.tris.iter().copied());
                for later in cache.spans.iter_mut().skip(i + 1) {
                    later.prims = (later.prims as i64 + dp) as usize;
                    later.verts = (later.verts as i64 + dv) as usize;
                    later.tris = (later.tris as i64 + dt) as usize;
                }
                out.bounds = lists.iter().fold(Bounds2::EMPTY, |b, l| b.union(&l.bounds));
                out.expanded = expanded;
                cache.patched = true;
                cache.last = Some(fingerprint(&out));
                cache.parts = chunks.into_iter().zip(lists).map(|(chunk, list)| Part { chunk, list }).collect();
                return out;
            }
        }
    }
    let mut out = DisplayList::default();
    out.prims.reserve(lists.iter().map(|l| l.prims.len()).sum());
    out.verts.reserve(lists.iter().map(|l| l.verts.len()).sum());
    out.tris.reserve(lists.iter().map(|l| l.tris.len()).sum());
    cache.spans.clear();
    for l in &lists {
        cache.spans.push(Span { prims: out.prims.len(), verts: out.verts.len(), tris: out.tris.len() });
        append_ref(&mut out, l);
    }
    cache.last = Some(fingerprint(&out));
    cache.parts = chunks.into_iter().zip(lists).map(|(chunk, list)| Part { chunk, list }).collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Block, Circle, Entity, EntityKind, Handle, Line};
    use cadaddy_geom::Vec3;

    /// A small deterministic random source (xorshift).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
        fn f(&mut self, span: f64) -> f64 {
            (self.below(10_000) as f64 / 10_000.0) * span
        }
    }

    fn object(r: &mut Rng, h: u64) -> Entity {
        let p = |r: &mut Rng| Vec3::new(r.f(1000.0), r.f(1000.0), 0.0);
        let kind = match r.below(4) {
            0 => EntityKind::Line(Line { a: p(r), b: p(r) }),
            1 => EntityKind::Circle(Circle { center: p(r), radius: 1.0 + r.f(40.0) }),
            2 => serde_json::from_value(
                serde_json::json!({"type": "text", "insert": [r.f(1000.0), r.f(1000.0), 0.0], "height": 2.5, "value": format!("T{h}")}),
            )
            .unwrap(),
            _ => serde_json::from_value(serde_json::json!({"type": "insert", "block": "B", "insert": [r.f(1000.0), r.f(1000.0), 0.0]})).unwrap(),
        };
        let mut e = Entity::new(Handle(h), kind);
        e.common.color = cadaddy_color::Color::Index(1 + r.below(7) as u8);
        e
    }

    fn opts() -> Options {
        Options { view: None, ..Options::default() }
    }

    fn same(a: &DisplayList, b: &DisplayList) {
        assert_eq!(a.prims, b.prims);
        assert_eq!(a.verts, b.verts);
        assert_eq!(a.tris, b.tris);
        assert_eq!(a.images.len(), b.images.len());
        assert_eq!(a.expanded, b.expanded);
        assert_eq!(a.bounds, b.bounds);
    }

    #[test]
    fn incremental_lists_equal_full_builds_after_random_edits() {
        for seed in [7u64, 1234, 98_765] {
            let mut r = Rng(seed);
            let mut d = Drawing::new_imperial();
            let mut b = Block::new("B");
            b.entities.push(Entity::new(Handle(0x50), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(5.0, 5.0, 0.0) })));
            b.entities.push(Entity::new(Handle(0x51), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 2.0 })));
            d.blocks.insert("B".into(), Arc::new(b));
            let mut next = 0x100u64;
            // Several chunks' worth.
            for _ in 0..3500 {
                d.model.push(object(&mut r, next));
                next += 1;
            }
            let mut cache = ChunkCache::default();
            let mut list = build_cached(&d, &Space::Model, &opts(), &mut cache);
            same(&list, &build(&d, &Space::Model, &opts()));
            assert_eq!(cache.rebuilt, cache.parts.len());
            let mut patched = 0;
            for step in 0..60 {
                let handles: Vec<Handle> = d.model.iter().map(|e| e.handle).collect();
                let pick = |r: &mut Rng| handles.get(r.below(handles.len() as u64) as usize).copied().unwrap_or(Handle(0x100));
                match r.below(8) {
                    // Moved in place (same shape: patched where it was).
                    6 | 7 => {
                        let h = pick(&mut r);
                        let dx = r.f(5.0);
                        d.model.modify(h, |e| match &mut e.kind {
                            EntityKind::Line(l) => {
                                l.a.x += dx;
                                l.b.x += dx;
                            }
                            EntityKind::Circle(c) => c.center.x += dx,
                            _ => {}
                        });
                    }
                    0 => {
                        d.model.push(object(&mut r, next));
                        next += 1;
                    }
                    1 => {
                        let h = pick(&mut r);
                        let n = object(&mut r, h.0);
                        d.model.modify(h, |e| *e = n);
                    }
                    2 => {
                        d.model.remove(pick(&mut r));
                    }
                    3 => {
                        d.model.bring_to_front(pick(&mut r));
                    }
                    4 => {
                        d.model.send_to_back(pick(&mut r));
                    }
                    _ => {
                        // Something every part depends on: a layer's colour.
                        if let Some(l) = d.layers.first_mut() {
                            l.color = cadaddy_color::Color::Index(1 + (step % 7) as u8);
                        }
                    }
                }
                list = build_cached_into(&d, &Space::Model, &opts(), &mut cache, Some(list));
                patched += usize::from(cache.patched);
                let full = build(&d, &Space::Model, &opts());
                same(&list, &full);
            }
            assert!(patched > 10, "the in-place paths were exercised ({patched})");
            // A list the cache didn't build is never patched.
            let foreign = build(&d, &Space::Model, &opts());
            let h = d.model.iter().nth(3).map(|e| e.handle).unwrap();
            let n = object(&mut r, h.0);
            d.model.modify(h, |e| *e = n);
            let l = build_cached_into(&d, &Space::Model, &opts(), &mut cache, Some(foreign));
            assert!(!cache.patched);
            same(&l, &build(&d, &Space::Model, &opts()));
            // A one-object edit rebuilds one chunk.
            let h = d.model.iter().nth(10).map(|e| e.handle).unwrap();
            let n = object(&mut r, h.0);
            d.model.modify(h, |e| *e = n);
            build_cached(&d, &Space::Model, &opts(), &mut cache);
            assert_eq!(cache.rebuilt, 1, "{cache:?}");
            assert!(cache.reused >= 2);
        }
    }

    #[test]
    fn layouts_and_changed_options_build_in_full() {
        let mut d = Drawing::new_imperial();
        d.model.push(Entity::new(Handle(0x100), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(1.0, 1.0, 0.0) })));
        let mut cache = ChunkCache::default();
        build_cached(&d, &Space::Model, &opts(), &mut cache);
        assert_eq!(cache.parts.len(), 1);
        // Another tolerance (a zoom band): every part again.
        let o2 = Options { tolerance: opts().tolerance * 2.0, ..opts() };
        build_cached(&d, &Space::Model, &o2, &mut cache);
        assert_eq!((cache.rebuilt, cache.reused), (1, 0));
        // A layout: built in full, nothing kept.
        let name = d.layouts.first().map(|l| l.name.clone()).unwrap_or_else(|| "Layout1".into());
        build_cached(&d, &Space::Paper(name), &opts(), &mut cache);
        assert!(cache.parts.is_empty());
    }

    /// `cargo test --release -p cadaddy-render bench_one_edit -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_one_edit() {
        let mut r = Rng(42);
        let mut d = Drawing::new_imperial();
        let mut b = Block::new("B");
        b.entities.push(Entity::new(Handle(0x50), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(5.0, 5.0, 0.0) })));
        d.blocks.insert("B".into(), Arc::new(b));
        for h in 0..167_000u64 {
            d.model.push(object(&mut r, 0x100 + h));
        }
        let t = web_time::Instant::now();
        let _ = build(&d, &Space::Model, &opts());
        println!("full build: {:?}", t.elapsed());
        let mut cache = ChunkCache::default();
        let mut list = build_cached(&d, &Space::Model, &opts(), &mut cache);
        for i in 0..8 {
            let h = Handle(0x100 + r.below(167_000));
            // A move (same shape) or a new object of another kind.
            if i % 2 == 0 {
                d.model.modify(h, |e| {
                    if let EntityKind::Line(l) = &mut e.kind {
                        l.a.x += 1.0;
                        l.b.x += 1.0;
                    } else if let EntityKind::Circle(c) = &mut e.kind {
                        c.center.x += 1.0;
                    }
                });
            } else {
                let n = object(&mut r, h.0);
                d.model.modify(h, |e| *e = n);
            }
            let t = web_time::Instant::now();
            list = build_cached_into(&d, &Space::Model, &opts(), &mut cache, Some(list));
            println!("one edit: {:?} ({} rebuilt, patched {}, {} prims)", t.elapsed(), cache.rebuilt, cache.patched, list.prims.len());
        }
    }
}
