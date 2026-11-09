//! A large drawing saved as DXF or DWG: refused for size from a count, before the file is made,
//! and written while it is made, its memory bounded whatever the file's size (Q6279). Before,
//! the whole file was built in memory several times over (its groups, then its text) and the
//! size limit was checked after that.
//!
//! A program of its own: it counts every allocation (a global allocator).

use std::sync::{Mutex, MutexGuard};

use cadaddy_doc::{Common, Drawing, EntityKind, Layer, Line, Mesh, MeshFace, MeshKind, Space};
use cadaddy_geom::Vec3;

#[global_allocator]
static ALLOC: cadaddy_memcount::Counting = cadaddy_memcount::Counting::new();

/// One test at a time: they read the process's allocation peak.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// `n` objects: boxes as polyface meshes (each a POLYLINE with fourteen VERTEX records), lines
/// and texts, on a few layers.
fn drawing(n: usize) -> Drawing {
    let mut d = Drawing::new_imperial();
    for i in 0..4 {
        d.layers.push(Layer { name: format!("Layer {i}"), ..Layer::default() });
    }
    let side = (n as f64).sqrt().ceil().max(1.0) as usize;
    for i in 0..n {
        let (x, y) = ((i % side) as f64 * 10.0, (i / side) as f64 * 10.0);
        let kind = if i % 4 == 3 {
            EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 6.0, y + 3.0, 0.0) })
        } else {
            let v = |dx: f64, dy: f64, z: f64| Vec3::new(x + dx, y + dy, z);
            let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
            EntityKind::Mesh(Mesh {
                vertices: vec![
                    v(0.0, 0.0, 0.0),
                    v(4.0, 0.0, 0.0),
                    v(4.0, 4.0, 0.0),
                    v(0.0, 4.0, 0.0),
                    v(0.0, 0.0, 3.0),
                    v(4.0, 0.0, 3.0),
                    v(4.0, 4.0, 3.0),
                    v(0.0, 4.0, 3.0),
                ],
                faces: vec![f([0, 1, 2, 3]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])],
                kind: MeshKind::Polyface,
            })
        };
        let _ = d.add(&Space::Model, Common { layer: format!("Layer {}", i % 4).into(), ..Common::default() }, kind);
    }
    d
}

/// `f`'s result and the most bytes it held at once above what was held before it.
fn peak<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = ALLOC.live();
    ALLOC.reset_peak();
    let r = f();
    (r, ALLOC.peak().saturating_sub(before))
}

/// Run `f` with the DXF group-code limit at `limit` (the test's injectable cap).
fn with_limit<T>(limit: usize, f: impl FnOnce() -> T) -> T {
    cadaddy_dxf::with_max_tags(limit, f)
}

const OBJECTS: usize = 40_000;
/// Megabyte.
const MB: usize = 1 << 20;

#[test]
fn a_dxf_too_big_to_reopen_is_refused_from_a_count_without_building_it() {
    let _g = serial();
    let d = drawing(OBJECTS);
    let (r, held) = peak(|| with_limit(1_000_000, || cadaddy_io::write(&d, "big.dxf").map(|b| b.len())));
    let Err(e) = r else { panic!("a {:?}-byte DXF over the limit was written", r) };
    assert!(e.to_string().contains("too large to save as DXF"), "{e}");
    // Its groups alone would be over 150 MB (about four million, 40 bytes each).
    assert!(held < 8 * MB, "refusing held {} MB", held / MB);
}

#[test]
fn a_dwg_too_big_to_reopen_is_refused_before_the_conversion() {
    let _g = serial();
    let d = drawing(OBJECTS);
    let (r, held) = peak(|| with_limit(1_000_000, || cadaddy_io::write(&d, "big.dwg").map(|b| b.len())));
    let Err(e) = r else { panic!("a {:?}-byte DWG over the limit was written", r) };
    assert!(e.to_string().contains("too large to save as DWG"), "{e}");
    assert!(held < 8 * MB, "refusing held {} MB", held / MB);
}

/// Where a streamed write goes in the test: only counted.
struct Count(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl std::io::Write for Count {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.fetch_add(b.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Write `d` streamed into a counter: the bytes written, how many `open` was called, the peak.
fn streamed(d: &Drawing, limit: usize, force: bool) -> (Result<Option<u64>, String>, usize, usize) {
    let bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut opened = 0;
    let (r, held) = peak(|| {
        with_limit(limit, || {
            cadaddy_dxf::with_oversize_allowed(force, || {
                cadaddy_io::write_streamed(d, "big.dxf", &cadaddy_io::SaveOptions::default(), &mut || {
                    opened += 1;
                    Ok(Box::new(Count(bytes.clone())))
                })
            })
        })
    });
    let r = r.map_err(|e| e.to_string());
    if let Ok(Some(n)) = &r {
        assert_eq!(*n as usize, bytes.load(std::sync::atomic::Ordering::Relaxed));
    }
    (r, opened, held)
}

#[test]
fn a_refused_streamed_save_never_opens_its_file() {
    let _g = serial();
    let d = drawing(OBJECTS);
    let (r, opened, held) = streamed(&d, 1_000_000, false);
    assert!(r.expect_err("over the limit").contains("pass force:true"));
    assert_eq!(opened, 0, "the file was opened for a refused save");
    assert!(held < 8 * MB, "refusing held {} MB", held / MB);
}

#[test]
fn a_forced_save_streams_with_memory_bounded_whatever_the_file_size() {
    let _g = serial();
    let mut sizes = Vec::new();
    for n in [OBJECTS / 4, OBJECTS] {
        let d = drawing(n);
        let (r, opened, held) = streamed(&d, 1_000_000, true);
        let written = r.expect("forced").expect("DXF is streamed") as usize;
        assert_eq!(opened, 1);
        sizes.push((written, held));
    }
    let [(small, small_held), (big, big_held)] = sizes[..] else { panic!("two sizes") };
    assert!(big > 4 * small - small / 4, "the bigger drawing's file: {big} bytes, the smaller's {small}");
    // A fixed bound: chunk buffers and the writer's tables, not the file (tens of megabytes here)
    // nor any multiple of it. Four times the drawing adds no more than its handle set.
    assert!(big_held < 12 * MB, "the {} MB file held {} MB", big / MB, big_held / MB);
    assert!(big_held < big / 6, "the {} MB file held {} MB", big / MB, big_held / MB);
    assert!(big_held < small_held + 4 * MB, "{} MB for the small file, {} MB for four times it", small_held / MB, big_held / MB);
}

/// Where a streamed write goes in the test: kept, to compare.
struct Kept(std::sync::Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Kept {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn streamed_and_whole_writes_are_the_same_file() {
    let _g = serial();
    let d = drawing(2_000);
    for opts in [
        cadaddy_io::SaveOptions::default(),
        cadaddy_io::SaveOptions { version: Some(cadaddy_io::Version::R2000), binary: true },
        cadaddy_io::SaveOptions { version: Some(cadaddy_io::Version::R2013), binary: false },
    ] {
        let whole = cadaddy_io::write_with(&d, "a.dxf", &opts).expect("whole");
        let kept = std::sync::Arc::new(Mutex::new(Vec::new()));
        let n = cadaddy_io::write_streamed(&d, "a.dxf", &opts, &mut || Ok(Box::new(Kept(kept.clone())))).expect("streamed").expect("DXF");
        assert_eq!(n as usize, whole.len());
        assert!(*kept.lock().unwrap_or_else(|e| e.into_inner()) == whole, "binary {} differs", opts.binary);
    }
    // Other formats, and Release 12, are written whole.
    let r12 = cadaddy_io::SaveOptions { version: Some(cadaddy_io::Version::R12), binary: false };
    assert!(matches!(cadaddy_io::write_streamed(&d, "a.dwg", &cadaddy_io::SaveOptions::default(), &mut || panic!("not streamed")), Ok(None)));
    assert!(matches!(cadaddy_io::write_streamed(&d, "a.dxf", &r12, &mut || panic!("not streamed")), Ok(None)));
}
