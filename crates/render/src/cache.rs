//! Incremental display lists for the screen: model space kept as one list per chunk of the
//! drawing's entity store ([`cadaddy_doc::ChunkRef`], 1024 objects each, compared by identity).
//! After an edit only the chunks that changed are built again and the parts are joined in
//! drawing order, so the list is the one a full [`build`](super::build) gives. Anything else that
//! shapes how objects look (layers, styles, blocks, header variables, field values, images, the
//! build options) changing rebuilds every part; layouts, plot styles and lists past the block
//! expansion budget are always built in full.
//!
//! The view's detail is the exception (Q6472): a new zoom band or view area (the tessellation
//! tolerance, the dash level of detail, the area hatch patterns are made for) rebuilds only the
//! parts whose objects reach into the new area. A part wholly outside it keeps the detail it was
//! built at, as long as that is no finer than the new one (so the list never holds more than a
//! build at the new detail would), until the view comes to it. Zoomed in on one room of a large
//! drawing, a zoom band then tessellates the objects around the room, not the whole drawing.

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
    /// Objects the renderer skips (HIDEOBJECTS, isolation), the in-place reference edit (fades
    /// the rest of the drawing) and loaded shape files: none of them touch the objects.
    hidden_objects: std::collections::BTreeSet<cadaddy_doc::Handle>,
    refedit: Option<cadaddy_doc::RefEdit>,
    shape_files: Vec<String>,
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
            hidden_objects: d.hidden_objects.clone(),
            refedit: d.refedit.clone(),
            shape_files: d.shape_files.clone(),
            images: images::generation(),
            opts: options_key(opts),
        }
    }
}

/// The build options that change what a list holds (not the cancel flag, nor the view's detail:
/// see [`Detail`]).
fn options_key(o: &Options) -> String {
    format!(
        "{}|{}|{}|{}|{:?}|{:?}|{}|{}|{}|{}",
        o.text,
        o.fill,
        o.lineweights,
        o.transparency,
        o.skip_viewport,
        o.projection,
        o.field_shading,
        o.plot_style_table.is_some(),
        o.color_overrides.len(),
        o.layer_lines.len(),
    )
}

/// The options that follow the view: how finely curves are tessellated and dashes drawn, the
/// height relative point sizes follow, and the area hatch patterns (and point clouds) are made
/// for. A part out of the view keeps those it was built with (see [`Part::serves`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Detail {
    tolerance: f64,
    min_dash: f64,
    view_height: f64,
    view: Option<Bounds2>,
}

impl Detail {
    fn of(o: &Options) -> Detail {
        Detail { tolerance: o.tolerance, min_dash: o.min_dash, view_height: o.view_height, view: o.view }
    }
}

/// One chunk's list, the detail it was built at, and where its objects are: the list's bounds and
/// the objects' own (a hatch outside the area it was built for has no pattern lines there).
#[derive(Clone)]
struct Part {
    chunk: ChunkRef,
    list: Arc<DisplayList>,
    detail: Detail,
    extent: Bounds2,
}

impl Part {
    /// Whether this part can stand in a list built at `want`: built at it, or wholly outside the
    /// area `want` is for (nothing of it is on screen) and no finer than it.
    fn serves(&self, want: &Detail) -> bool {
        if self.detail == *want {
            return true;
        }
        let Some(area) = want.view else { return false };
        let (have, e) = (&self.detail, &self.extent);
        let coarse = have.tolerance.is_finite() && have.min_dash.is_finite() && have.tolerance >= want.tolerance && have.min_dash >= want.min_dash;
        let outside = e.is_empty() || e.min.is_finite() && e.max.is_finite() && !e.intersects(&area);
        coarse && outside
    }
}

/// Where one part sits in the joined list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Span {
    prims: usize,
    verts: usize,
    tris: usize,
}

/// The parts of the model-space list kept between builds (one per canvas). A copy is cheap (the
/// parts are shared): a background build takes one and hands back the cache it leaves.
#[derive(Clone, Default)]
pub struct ChunkCache {
    ctx: Option<Context>,
    parts: Vec<Part>,
    /// Where each part starts in the list last returned, and that list's buffers and lengths
    /// (to know it when it is handed back to [`build_cached_into`]).
    spans: Vec<Span>,
    last: Option<(usize, usize, usize, usize, usize, usize)>,
    /// The last build patched the list handed back in place.
    pub patched: bool,
    /// Chunks built again by the last [`build_cached`] (for measuring), the ones used as they
    /// were, and of those the ones kept at another detail (out of the view, see [`Detail`]).
    pub rebuilt: usize,
    pub reused: usize,
    pub kept: usize,
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
        f.debug_struct("ChunkCache")
            .field("parts", &self.parts.len())
            .field("rebuilt", &self.rebuilt)
            .field("reused", &self.reused)
            .field("kept", &self.kept)
            .finish()
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

/// The list of one chunk's objects, and where they are (see [`Part`]).
fn build_chunk_part(d: &Drawing, chunk: &ChunkRef, opts: &Options) -> (DisplayList, Bounds2) {
    let list = build_chunk(d, chunk, opts);
    // Only a part a later view could leave out needs its objects measured.
    let extent = match opts.view {
        Some(_) => list.bounds.union(&cadaddy_doc::entities_bounds(d, chunk.items().iter().map(|e| &**e))),
        None => list.bounds,
    };
    (list, extent)
}

/// The list of one chunk's objects.
fn build_chunk(d: &Drawing, chunk: &ChunkRef, opts: &Options) -> DisplayList {
    let mut b = Builder {
        list: DisplayList::default(),
        opts,
        plotting: false,
        styles: None,
        named: None,
        shared_budget: None,
        transparency: 0,
        paper: false,
        prof: profile::Local::new(opts),
    };
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
    let detail = Detail::of(opts);
    let chunks = d.model.chunk_refs();
    let before: Vec<(usize, Arc<DisplayList>)> = cache.parts.iter().map(|p| (p.chunk.key(), Arc::clone(&p.list))).collect();
    let mut old: HashMap<usize, Part> = cache.parts.drain(..).map(|p| (p.chunk.key(), p)).collect();
    // Chunks kept as they were reuse their parts (at the detail asked for, or out of view at
    // their own); the others are built (side by side).
    let mut parts: Vec<Option<Part>> = chunks.iter().map(|c| old.remove(&c.key()).filter(|p| p.serves(&detail))).collect();
    drop(old);
    let stale: Vec<usize> = parts.iter().enumerate().filter(|(_, p)| p.is_none()).map(|(i, _)| i).collect();
    cache.rebuilt = stale.len();
    cache.reused = chunks.len() - stale.len();
    cache.kept = parts.iter().flatten().filter(|p| p.detail != detail).count();
    #[cfg(not(target_arch = "wasm32"))]
    let built: Vec<(usize, (DisplayList, Bounds2))> = {
        use rayon::prelude::*;
        stale.par_iter().filter_map(|&i| chunks.get(i).map(|c| (i, build_chunk_part(d, c, opts)))).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let built: Vec<(usize, (DisplayList, Bounds2))> =
        stale.iter().filter_map(|&i| chunks.get(i).map(|c| (i, build_chunk_part(d, c, opts)))).collect();
    if opts.cancelled() {
        cache.clear();
        return DisplayList::default();
    }
    for (i, (list, extent)) in built {
        if let (Some(slot), Some(chunk)) = (parts.get_mut(i), chunks.get(i)) {
            *slot = Some(Part { chunk: chunk.clone(), list: Arc::new(list), detail, extent });
        }
    }
    let parts: Vec<Part> = parts.into_iter().flatten().collect();
    let lists: Vec<Arc<DisplayList>> = parts.iter().map(|p| Arc::clone(&p.list)).collect();
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
            cache.parts = parts;
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
                cache.parts = parts;
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
    cache.parts = parts;
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
                match r.below(10) {
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
                    8 => {
                        // Hide or unhide a random object.
                        let h = pick(&mut r);
                        if !d.hidden_objects.remove(&h) {
                            d.hidden_objects.insert(h);
                        }
                    }
                    9 => d.hidden_objects.clear(),
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
    fn hidden_and_refedit_changes_invalidate_parts() {
        let mut r = Rng(31);
        let mut d = Drawing::new_imperial();
        for h in 0..2500u64 {
            d.model.push(object(&mut r, 0x100 + h));
        }
        let mut cache = ChunkCache::default();
        let mut list = build_cached(&d, &Space::Model, &opts(), &mut cache);
        same(&list, &build(&d, &Space::Model, &opts()));
        let before = list.prims.len();
        let victim = Handle(0x100 + 1500);
        let step = |d: &Drawing, cache: &mut ChunkCache, list: DisplayList| {
            let l = build_cached_into(d, &Space::Model, &opts(), cache, Some(list));
            same(&l, &build(d, &Space::Model, &opts()));
            l
        };
        d.hidden_objects.insert(victim);
        list = step(&d, &mut cache, list);
        assert!(list.prims.len() < before, "the hidden object's prims are gone");
        assert!(list.prims.iter().all(|p| p.handle != victim));
        d.hidden_objects.clear();
        list = step(&d, &mut cache, list);
        assert_eq!(list.prims.len(), before);
        // An in-place reference edit fades what it doesn't own.
        d.refedit = Some(cadaddy_doc::RefEdit {
            block: "B".into(),
            insert: Handle(0x100),
            insert_was_visible: true,
            space: Space::Model,
            to_world: cadaddy_geom::Mat3::IDENTITY,
            items: Vec::new(),
            ghosts: Vec::new(),
            released: Vec::new(),
            context: Vec::new(),
        });
        list = step(&d, &mut cache, list);
        assert!(list.prims.iter().any(|p| p.transparency > 0), "the rest is faded");
        d.refedit = None;
        list = step(&d, &mut cache, list);
        assert!(list.prims.iter().all(|p| p.transparency == 0));
        // A loaded shape file.
        d.shape_files.push("ltypeshp.shx".into());
        list = step(&d, &mut cache, list);
        d.shape_files.clear();
        step(&d, &mut cache, list);
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

    /// Points of the primitives of `l` that reach into `b`, in list order.
    fn points_in(l: &DisplayList, b: &Bounds2) -> Vec<Vec2> {
        l.prims.iter().filter(|p| Bounds2::from_points(l.points(p).iter().copied()).intersects(b)).flat_map(|p| l.points(p).to_vec()).collect()
    }

    /// Q6472: three chunks of circles, each in an area of its own, then a fourth holding one
    /// pattern hatch far to the right.
    fn three_areas_and_a_hatch() -> Drawing {
        let mut d = Drawing::new_imperial();
        let mut h = 0x100u64;
        for area in 0..3 {
            for i in 0..1024 {
                let center = Vec3::new(f64::from(area) * 10_000.0 + f64::from(i % 32) * 20.0, f64::from(i / 32) * 20.0, 0.0);
                d.model.push(Entity::new(Handle(h), EntityKind::Circle(Circle { center, radius: 5.0 })));
                h += 1;
            }
        }
        let sq = [(40_000.0, 0.0), (40_040.0, 0.0), (40_040.0, 40.0), (40_000.0, 40.0)].map(|(x, y)| cadaddy_geom::PolyVertex::new(Vec2::new(x, y)));
        let hatch = cadaddy_doc::Hatch {
            pattern: "ANSI31".into(),
            solid: false,
            loops: vec![cadaddy_doc::HatchLoop { curves: Vec::new(), vertices: sq.to_vec(), outer: true, sources: Vec::new() }],
            scale: 10.0,
            angle: 0.0,
            associative: false,
            style: 0,
            elevation: 0.0,
            gradient: None,
            origin: Vec2::ZERO,
            background: None,
            pattern_lines: Vec::new(),
        };
        d.model.push(Entity::new(Handle(h), EntityKind::Hatch(Box::new(hatch))));
        assert_eq!(d.model.chunk_refs().len(), 4);
        d
    }

    fn area(x: f64) -> Bounds2 {
        Bounds2::new(Vec2::new(x - 1000.0, -1000.0), Vec2::new(x + 1500.0, 2000.0))
    }

    /// Q6472: a zoom band (a finer tolerance, another view area) builds again only the parts
    /// whose objects reach into the area being looked at; the parts outside keep the detail they
    /// had. The area looked at is exactly what a full build at the new detail gives.
    #[test]
    fn a_zoom_band_rebuilds_only_the_parts_in_view() {
        let d = three_areas_and_a_hatch();
        let everywhere = Bounds2::new(Vec2::new(-1000.0, -1000.0), Vec2::new(41_000.0, 2000.0));
        let coarse = Options { tolerance: 1.0, min_dash: 4.0, view: Some(everywhere), ..Options::default() };
        let mut cache = ChunkCache::default();
        let list = build_cached(&d, &Space::Model, &coarse, &mut cache);
        same(&list, &build(&d, &Space::Model, &coarse));
        // Zoomed in on the middle area: only its part is tessellated again, finer.
        let fine = Options { tolerance: 0.01, min_dash: 0.04, view: Some(area(10_000.0)), ..Options::default() };
        let list = build_cached(&d, &Space::Model, &fine, &mut cache);
        assert_eq!((cache.rebuilt, cache.reused, cache.kept), (1, 3, 3), "{cache:?}");
        let full_fine = build(&d, &Space::Model, &fine);
        assert_eq!(points_in(&list, &area(10_000.0)), points_in(&full_fine, &area(10_000.0)), "in view: the fine build");
        assert_eq!(points_in(&list, &area(0.0)), points_in(&build(&d, &Space::Model, &coarse), &area(0.0)), "out of view: as before");
        assert!(list.verts.len() * 2 < full_fine.verts.len(), "{} points, a fine build of everything {}", list.verts.len(), full_fine.verts.len());
        // The same zoom panned to the first area: the middle stays fine, the first is built.
        let list = build_cached(&d, &Space::Model, &Options { view: Some(area(0.0)), ..fine.clone() }, &mut cache);
        assert_eq!((cache.rebuilt, cache.kept), (1, 3), "{cache:?}");
        assert_eq!(points_in(&list, &area(0.0)), points_in(&full_fine, &area(0.0)));
        // Zoomed out again: a part finer than asked is built again even out of view (the list
        // never holds more than a build at the asked detail would), so this is the coarse list.
        let list = build_cached(&d, &Space::Model, &Options { view: Some(area(20_000.0)), ..coarse.clone() }, &mut cache);
        assert_eq!(cache.rebuilt, 3, "{cache:?}");
        same(&list, &build(&d, &Space::Model, &coarse));
    }

    /// A part whose only object is a hatch the build area left without pattern lines draws
    /// nothing, but its object is there: a view coming to it builds its pattern.
    #[test]
    fn a_hatch_built_out_of_view_gets_its_pattern_when_the_view_comes_to_it() {
        let d = three_areas_and_a_hatch();
        let fine = Options { tolerance: 0.01, min_dash: 0.04, view: Some(area(0.0)), ..Options::default() };
        let mut cache = ChunkCache::default();
        let list = build_cached(&d, &Space::Model, &fine, &mut cache);
        let hatch_area = area(40_000.0);
        assert!(points_in(&list, &hatch_area).is_empty(), "no pattern outside the area built for");
        let at_hatch = Options { view: Some(hatch_area), ..fine.clone() };
        let list = build_cached(&d, &Space::Model, &at_hatch, &mut cache);
        assert!(cache.rebuilt >= 1, "{cache:?}");
        let pattern = points_in(&list, &hatch_area);
        assert!(!pattern.is_empty(), "the pattern is built once the view is on it");
        assert_eq!(pattern, points_in(&build(&d, &Space::Model, &at_hatch), &hatch_area));
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
