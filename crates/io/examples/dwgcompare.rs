//! Compare CADaddy's DWG codec with the DWG bridge on real drawings, without printing anything
//! from them but counts: `cargo run --release -p cadaddy-io --example dwgcompare -- a.dwg [b.dwg…]`.
//!
//! For each file (named by its position on the command line only): read with both engines, count
//! objects per type, layers, blocks, layouts, text, dimensions and xrefs; render model space with
//! both to PNG and compare pixels; write the codec's drawing back as every DWG version and read
//! each again. Set `DWGCOMPARE_PNG=<folder>` to keep the renders (they show the drawing: keep them
//! private).

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

use cadaddy_doc::{Drawing, Space};
use cadaddy_io::Version;

/// Objects per type across model space, every layout and every block.
fn counts(d: &Drawing) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let mut add = |name: &str| *out.entry(name.to_string()).or_insert(0) += 1;
    for e in d.model.iter() {
        add(&format!("model:{}", e.kind.dxf_name()));
    }
    for l in &d.layouts {
        for e in l.entities.iter() {
            add(&format!("paper:{}", e.kind.dxf_name()));
        }
    }
    for b in d.blocks.values() {
        for e in b.entities.iter() {
            add(&format!("block:{}", e.kind.dxf_name()));
        }
    }
    out
}

/// One line of totals.
fn summary(d: &Drawing) -> String {
    let c = counts(d);
    let sum = |pre: &str| c.iter().filter(|(k, _)| k.starts_with(pre)).map(|(_, n)| n).sum::<usize>();
    let of = |ty: &str| c.iter().filter(|(k, _)| k.ends_with(&format!(":{ty}"))).map(|(_, n)| n).sum::<usize>();
    let xrefs = d.blocks.values().filter(|b| b.xref_path.is_some()).count();
    format!(
        "model {} paper {} inblocks {} | layers {} blocks {} layouts {} | text {} mtext {} dims {} inserts {} xrefs {} | notes {}",
        sum("model:"),
        sum("paper:"),
        sum("block:"),
        d.layers.len(),
        d.blocks.len(),
        d.layouts.len(),
        of("TEXT"),
        of("MTEXT"),
        of("DIMENSION"),
        of("INSERT"),
        xrefs,
        d.read_notes.len()
    )
}

/// Differences between two count maps, as `key a→b`.
fn diff(a: &BTreeMap<String, usize>, b: &BTreeMap<String, usize>) -> Vec<String> {
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter_map(|k| {
            let (x, y) = (a.get(k).copied().unwrap_or(0), b.get(k).copied().unwrap_or(0));
            (x != y).then(|| format!("{k} {x}→{y}"))
        })
        .collect()
}

/// `f` run with panics caught (a panic is a bug to report, not a crash of the whole run).
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f))
        .map_err(|p| p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default())
}

/// Pixels that differ, over pixels drawn in either image (`None` if the images can't be compared).
fn pixel_diff(a: &[u8], b: &[u8]) -> Option<(f64, usize)> {
    let (a, b) = (tiny_skia::Pixmap::decode_png(a).ok()?, tiny_skia::Pixmap::decode_png(b).ok()?);
    if a.width() != b.width() || a.height() != b.height() {
        return None;
    }
    let bg = a.pixels().first().copied()?;
    let (mut drawn, mut differ) = (0usize, 0usize);
    for (p, q) in a.pixels().iter().zip(b.pixels()) {
        if *p == bg && *q == bg {
            continue;
        }
        drawn += 1;
        let d = |x: u8, y: u8| x.abs_diff(y) > 48;
        if d(p.red(), q.red()) || d(p.green(), q.green()) || d(p.blue(), q.blue()) {
            differ += 1;
        }
    }
    Some((if drawn == 0 { 0.0 } else { differ as f64 / drawn as f64 }, drawn))
}

const VERSIONS: [Version; 6] = [Version::R2000, Version::R2004, Version::R2007, Version::R2010, Version::R2013, Version::R2018];

fn main() {
    let keep = std::env::var("DWGCOMPARE_PNG").ok();
    let only_errors = std::env::var("DWGCOMPARE_TYPES").is_err();
    for (n, path) in std::env::args().skip(1).enumerate() {
        let n = n + 1;
        let Ok(bytes) = std::fs::read(&path) else {
            println!("#{n}: cannot read");
            continue;
        };
        println!("#{n}: {:.2} MB", bytes.len() as f64 / 1e6);
        let t = Instant::now();
        let native = guarded(|| cadaddy_io::dwg_read::read(&bytes));
        let native = match native {
            Ok(Ok((d, report))) => {
                println!("  native read {:.2}s: {}", t.elapsed().as_secs_f64(), summary(&d));
                let mut classes: BTreeMap<String, usize> = BTreeMap::new();
                for (_, why) in &report.failed {
                    let class: String = why.chars().take_while(|c| *c != ':' && !c.is_ascii_digit()).collect();
                    *classes.entry(class.trim().to_string()).or_insert(0) += 1;
                }
                if !report.failed.is_empty() {
                    println!("  native failed objects {}: {classes:?}", report.failed.len());
                }
                Some(d)
            }
            Ok(Err(e)) => {
                println!("  native read ERROR {:.2}s: {e}", t.elapsed().as_secs_f64());
                None
            }
            Err(p) => {
                println!("  native read PANIC: {p}");
                None
            }
        };
        let t = Instant::now();
        let bridge = match guarded(|| cadaddy_io::read_dwg(&bytes, cadaddy_io::DwgEngine::Bridge)) {
            Ok(Ok(d)) => {
                println!("  bridge read {:.2}s: {}", t.elapsed().as_secs_f64(), summary(&d));
                Some(d)
            }
            Ok(Err(e)) => {
                println!("  bridge read ERROR: {e}");
                None
            }
            Err(p) => {
                println!("  bridge read PANIC: {p}");
                None
            }
        };
        if let (Some(a), Some(b)) = (&native, &bridge) {
            let d = diff(&counts(b), &counts(a));
            if !d.is_empty() || !only_errors {
                println!("  counts bridge→native: {d:?}");
            }
            println!(
                "  layers {}→{} blocks {}→{} layouts {}→{}",
                b.layers.len(),
                a.layers.len(),
                b.blocks.len(),
                a.blocks.len(),
                b.layouts.len(),
                a.layouts.len()
            );
            // Model space and each layout, framed alike (the bridge's extents).
            let mut spaces = vec![Space::Model];
            spaces.extend(b.layouts.iter().map(|l| Space::Paper(l.name.clone())));
            for (k, space) in spaces.iter().enumerate() {
                if a.layouts.iter().all(|l| Some(l.name.as_str()) != space.layout()) && space.is_paper() {
                    println!("  space {k}: no such layout in native");
                    continue;
                }
                let frame = b.extents(space);
                if frame.is_empty() {
                    continue;
                }
                let t = Instant::now();
                let pa = guarded(|| cadaddy_io::png_framed(a, space, 1200, 900, Some(frame)));
                let pb = guarded(|| cadaddy_io::png_framed(b, space, 1200, 900, Some(frame)));
                match (pa, pb) {
                    (Ok(Ok(pa)), Ok(Ok(pb))) => {
                        let r = pixel_diff(&pa, &pb);
                        match r {
                            Some((f, drawn)) => {
                                println!("  space {k}: pixels differ {:.2}% of {drawn} drawn ({:.1}s)", f * 100.0, t.elapsed().as_secs_f64())
                            }
                            None => println!("  space {k}: images not comparable"),
                        }
                        if let Some(dir) = &keep {
                            let _ = std::fs::write(format!("{dir}/{n}-{k}-native.png"), &pa);
                            let _ = std::fs::write(format!("{dir}/{n}-{k}-bridge.png"), &pb);
                        }
                    }
                    (x, y) => println!("  space {k}: render failed native={} bridge={}", x.is_ok_and(|r| r.is_ok()), y.is_ok_and(|r| r.is_ok())),
                }
                if std::env::var("DWGCOMPARE_ALL_SPACES").is_err() && k >= 3 {
                    break;
                }
            }
        }
        let Some(d) = native.or(bridge) else { continue };
        let before = counts(&d);
        for v in VERSIONS {
            let t = Instant::now();
            match guarded(|| cadaddy_io::dwg_write::write(&d, v.code())) {
                Ok(Ok((out, report))) => {
                    let w = t.elapsed().as_secs_f64();
                    let skipped: usize = report.skipped.iter().map(|(_, n)| n).sum();
                    let back = guarded(|| cadaddy_io::dwg_read::read(&out));
                    match back {
                        Ok(Ok((r, rr))) => {
                            if let Ok(dir) = std::env::var("DWGCOMPARE_OUT") {
                                let _ = std::fs::write(format!("{dir}/{n}-{}.dwg", v.code()), &out);
                            }
                            for (h, why) in rr.failed.iter().take(3) {
                                println!("    reread failed {h:x}: {why}");
                            }
                            let dd = diff(&before, &counts(&r));
                            println!(
                                "  write {} {w:.2}s {:.2} MB skipped {skipped} {:?} kept {} | reread failed {} | lost/gained {}{}",
                                v.code(),
                                out.len() as f64 / 1e6,
                                report.skipped,
                                report.kept,
                                rr.failed.len(),
                                dd.len(),
                                if dd.is_empty() { String::new() } else { format!(" {dd:?}") }
                            );
                            if v == Version::R2018 {
                                let bridge_back = guarded(|| cadaddy_io::read_dwg(&out, cadaddy_io::DwgEngine::Bridge));
                                match bridge_back {
                                    Ok(Ok(rb)) => {
                                        let dd = diff(&before, &counts(&rb));
                                        println!("    bridge reads the codec's {}: lost/gained {} {dd:?}", v.code(), dd.len());
                                    }
                                    Ok(Err(e)) => println!("    bridge reads the codec's {}: ERROR {e}", v.code()),
                                    Err(p) => println!("    bridge reads the codec's {}: PANIC {p}", v.code()),
                                }
                            }
                        }
                        Ok(Err(e)) => println!("  write {} ok, reread ERROR: {e}", v.code()),
                        Err(p) => println!("  write {} ok, reread PANIC: {p}", v.code()),
                    }
                }
                Ok(Err(e)) => println!("  write {} ERROR: {e}", v.code()),
                Err(p) => println!("  write {} PANIC: {p}", v.code()),
            }
        }
    }
}
