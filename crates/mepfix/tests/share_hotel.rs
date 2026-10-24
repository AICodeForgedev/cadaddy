//! The shared project page (Q350) measured on a hotel-size job: three wings of the synthetic
//! 4-storey hotel (Q304), over 200 rooms with their MEP. The bundle's
//! model files within the size a phone fetches quickly, loaded part by part within budget, the
//! made-up building name and the spec book's invented manufacturers stripped from every byte.

use std::time::Instant;

use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_walk3d::share::{MANIFEST, MODEL_DIR, ShareLoad, ShareManifest};

/// Model bytes a page fetches (a phone on a job-site connection).
const BUDGET_BYTES: usize = 4 << 20;
/// Parsing every part (a debug build; the release wasm in a browser is faster).
const BUDGET_LOAD_MS: u128 = 2_000;

fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { files(&p, out) } else { out.push(p) }
    }
}

#[test]
fn the_hotel_shares_within_budget_and_without_its_names() {
    let t = cadaddy_mepfix::hotel_truth_for(7, HotelSize::Full);
    let dir = std::env::temp_dir().join(format!("cadaddy-share-hotel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (m, report) = cadaddy_mepfix::fieldpkg::write_share(&t, &dir, "Sample Hotel", 3).unwrap();
    assert!(m.total_bytes() <= BUDGET_BYTES, "{} bytes", m.total_bytes());
    assert!(m.parts.len() >= 2, "a hotel loads in parts: {}", m.parts.len());

    // Load as the page does: the manifest, the base, the parts in order.
    let started = Instant::now();
    let text = std::fs::read(dir.join(MODEL_DIR).join(MANIFEST)).unwrap();
    let mut steps = 0;
    let mut pkg = ShareLoad::read_all(ShareManifest::from_json(&text).unwrap(), |f| {
        steps += 1;
        std::fs::read(dir.join(MODEL_DIR).join(f)).map_err(|e| e.to_string())
    })
    .unwrap();
    pkg.assign_rooms();
    let ms = started.elapsed().as_millis();
    eprintln!(
        "shared hotel: {} KB in {} files ({} parts), {} levels, {} rooms, {} elements, loaded in {ms} ms; {} items stripped",
        m.total_bytes() >> 10,
        steps,
        m.parts.len(),
        pkg.levels.len(),
        pkg.rooms.len(),
        pkg.elements.len(),
        report.removed.len()
    );
    assert!(ms <= BUDGET_LOAD_MS, "{ms} ms");
    assert_eq!(pkg.elements.len(), m.elements);
    assert_eq!(pkg.rooms.len(), t.building.rooms.len() * 3);
    assert!(pkg.rooms.len() >= 200, "a hotel-size job: {} rooms", pkg.rooms.len());
    assert!(pkg.levels.len() >= 4);
    for trade in ["arch", "mech", "plumb", "elec"] {
        assert!(pkg.trades.iter().any(|x| x.key == trade), "{trade}");
    }

    // No byte of the bundle carries the building's name or a manufacturer.
    let mut all = Vec::new();
    files(&dir, &mut all);
    assert!(all.iter().any(|p| p.extension().is_some_and(|e| e == "svg")), "thumbnails");
    let planted: Vec<String> = std::iter::once(t.building.name.to_lowercase())
        .chain(t.spec_items.iter().map(|s| s.manufacturer.to_lowercase()))
        .filter(|n| n.len() >= 4)
        .collect();
    assert!(!planted.is_empty());
    for p in &all {
        let text = String::from_utf8_lossy(&std::fs::read(p).unwrap()).to_lowercase();
        for n in &planted {
            assert!(!text.contains(n.as_str()), "{n:?} in {}", p.display());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
