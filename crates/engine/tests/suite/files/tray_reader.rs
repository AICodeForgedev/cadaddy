//! Cable tray and large conduit read from the fixtures' power plans (Q266), scored against the
//! truth, built in 3D, clashing with the other trades, and exported to IFC.

use cadaddy_engine::Session;
use cadaddy_mepfix::score;
use cadaddy_mepfix::tray_reader::{LevelInfo, read_fixture_trays, spec_runs};
use serde_json::json;

#[test]
fn trays_and_banks_score_over_many_sets() {
    let (mut sizes, mut banks) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    for seed in 1..=20 {
        let (t, r) = read_fixture_trays(seed).unwrap();
        for l in ["1", "2"] {
            for k in ["tray", "conduit"] {
                let s = score::score_runs(&t, l, k, &r.runs, 0.25);
                assert!(s.truth_length_ft > 20.0, "seed {seed} L{l}: no {k} in the truth");
                assert!(s.recall >= 0.95 && s.precision >= 0.95, "seed {seed} L{l} {k}: {s:?}");
                assert!(s.size_accuracy >= 0.95, "seed {seed} L{l} {k}: sizes {s:?}");
                // Heights from the tags (rounded to 1/2" as printed).
                assert!(s.height_mae_ft.is_some_and(|h| h <= 0.5 / 12.0), "seed {seed} L{l} {k}: heights {s:?}");
            }
        }
        // The riser bank: (4) 2" up on level 1, down on level 2, every conduit's circle.
        for (lv, dir) in [("1", "up"), ("2", "down")] {
            let b = r.risers.iter().find(|b| b.level == lv).unwrap_or_else(|| panic!("seed {seed}: no riser bank on {lv}"));
            assert_eq!((b.conduits, b.size_in, b.direction.as_str(), b.at_ft.len()), (4, 2.0, dir, 4), "seed {seed}");
        }
        for f in r.runs.iter().filter(|f| f.kind == "tray") {
            sizes.insert(format!("{}", f.size_in[0]));
        }
        banks.insert(r.runs.iter().filter(|f| f.kind == "conduit" && f.level == "1").count());
    }
    // The fixtures vary the sizes: 6" and 12" trays, banks of 2 and 3.
    assert!(sizes.len() >= 2 && banks.len() >= 2, "{sizes:?} {banks:?}");
}

#[test]
fn read_trays_build_clash_and_export() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    let (t, r) = read_fixture_trays(3).unwrap();
    let lvls: Vec<LevelInfo<'_>> =
        t.building.levels.iter().map(|l| LevelInfo { code: &l.code, name: &l.name, elevation_ft: l.elevation_ft }).collect();
    let (mut systems, mut runs) = spec_runs(&r, &lvls);
    // Horizontal runs + vertical riser conduits.
    let horizontal = r.runs.len();
    let riser_conduits: usize = r.risers.iter().filter(|b| b.direction == "up").map(|b| b.conduits).sum();
    assert_eq!(runs.len(), horizontal + riser_conduits, "horizontal runs + riser conduits");
    // A made-up duct crossing level 1's tray at the tray's height: a clash.
    let tray = r.runs.iter().find(|f| f.kind == "tray" && f.level == "1").unwrap();
    let x = (tray.from_ft[0] + tray.to_ft[0]) / 2.0 * 12.0;
    let y = tray.from_ft[1] * 12.0;
    let b = tray.bottom_ft.unwrap();
    systems.push(json!({"name": "M-DUCT-SUPP", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [12, 12], "bottom_ft": b}));
    runs.push(json!({"system": "M-DUCT-SUPP", "level": "LEVEL 1", "points": [[x, y - 60.0], [x, y + 60.0]], "size_in": [12, 12], "bottoms_ft": [b]}));
    let levels: Vec<serde_json::Value> = t
        .building
        .levels
        .iter()
        .filter(|l| l.code != "R")
        .map(|l| json!({"name": l.name, "elevation_ft": l.elevation_ft, "height_ft": 13, "ceiling_ft": 8, "region": [-1000, -1000, 5000, 2000], "base": [0, 0]}))
        .collect();
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "E-CABL-TRAY", "current": true})).unwrap();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": levels, "mep": {"systems": systems, "runs": runs}}})).unwrap();
    let built = s.execute("model.build", &json!({})).unwrap();
    assert!(built.to_string().contains("E-CABL-TRAY") || s.doc().unwrap().layers.iter().any(|l| l.name.contains("E-CABL-TRAY")), "{built}");
    let clashes = s.execute("model.clashes", &json!({})).unwrap();
    let hit = clashes["clashes"].as_array().unwrap().iter().any(|c| {
        let sys = c["systems"].to_string();
        c["kind"] == "crossing" && sys.contains("E-CABL-TRAY") && sys.contains("M-DUCT-SUPP")
    });
    assert!(hit, "the duct crossing the tray is a clash: {clashes}");
    // IFC: the tray as a cable carrier, cable tray segment; the conduits as conduit segments.
    let dir = std::env::temp_dir().join(format!("cadaddy-trays-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Trays.ifc");
    s.execute("export", &json!({ "path": path })).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let count = |pre: &str| text.lines().filter(|l| l.contains("IFCCABLECARRIERSEGMENT(") && l.contains(pre)).count();
    assert!(count(".CABLETRAYSEGMENT.") >= 2, "trays on both levels");
    assert!(count(".CONDUITSEGMENT.") >= r.runs.iter().filter(|f| f.kind == "conduit").count(), "a segment per conduit");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Riser banks matched across levels become vertical conduit runs that build, clash and export.
#[test]
fn riser_banks_build_clash_and_export() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    let (t, r) = read_fixture_trays(3).unwrap();
    let lvls: Vec<LevelInfo<'_>> =
        t.building.levels.iter().map(|l| LevelInfo { code: &l.code, name: &l.name, elevation_ft: l.elevation_ft }).collect();
    let (mut systems, mut runs) = spec_runs(&r, &lvls);

    // The riser bank: (4) 2" conduits from L1 up to L2.
    let riser_runs: Vec<&serde_json::Value> = runs.iter().filter(|v| v.get("top_ft").is_some()).collect();
    assert_eq!(riser_runs.len(), 4, "4 vertical conduit runs from the (4) 2\" riser bank: got {}", riser_runs.len());
    for v in &riser_runs {
        assert_eq!(v["system"], "E-FDR-COND");
        assert!(v["top_ft"].as_f64().unwrap() > v["bottoms_ft"][0].as_f64().unwrap(), "top above bottom");
    }

    // Place a beam across the riser location at mid-height: it should clash.
    let riser_pt = riser_runs[0]["points"][0].as_array().unwrap();
    let rx = riser_pt[0].as_f64().unwrap();
    let ry = riser_pt[1].as_f64().unwrap();
    let l1 = t.building.levels.iter().find(|l| l.code == "1").unwrap();
    let l2 = t.building.levels.iter().find(|l| l.code == "2").unwrap();
    let beam_z = (l1.elevation_ft + l2.elevation_ft) / 2.0; // midway between levels
    systems.push(json!({"name": "S-BEAM", "layer": "S-BEAM", "kind": "duct", "size_in": [12, 12], "bottom_ft": beam_z - l1.elevation_ft}));
    runs.push(json!({"system": "S-BEAM", "level": "LEVEL 1", "points": [[rx - 60.0, ry], [rx + 60.0, ry]], "size_in": [12, 12], "bottoms_ft": [beam_z - l1.elevation_ft]}));

    let levels: Vec<serde_json::Value> = t
        .building
        .levels
        .iter()
        .filter(|l| l.code != "R")
        .map(|l| json!({"name": l.name, "elevation_ft": l.elevation_ft, "height_ft": 13, "ceiling_ft": 8, "region": [-1000, -1000, 5000, 2000], "base": [0, 0]}))
        .collect();
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "E-FDR-COND", "current": true})).unwrap();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": levels, "mep": {"systems": systems, "runs": runs}}})).unwrap();
    s.execute("model.build", &json!({})).unwrap();

    // Clash: the beam crosses the riser conduits.
    let clashes = s.execute("model.clashes", &json!({})).unwrap();
    let hit = clashes["clashes"].as_array().unwrap().iter().any(|c| {
        let sys = c["systems"].to_string();
        c["kind"] == "crossing" && sys.contains("E-FDR-COND") && sys.contains("S-BEAM")
    });
    assert!(hit, "the beam crossing the riser bank is a clash: {clashes}");

    // IFC: 4 vertical CONDUITSEGMENTs spanning L1→L2.
    let dir = std::env::temp_dir().join(format!("cadaddy-risers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Risers.ifc");
    s.execute("export", &json!({ "path": path })).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let conduit_count = text.lines().filter(|l| l.contains("IFCCABLECARRIERSEGMENT(") && l.contains(".CONDUITSEGMENT.")).count();
    // At least 4 vertical + the horizontal feeder conduits.
    let horizontal_conduits = r.runs.iter().filter(|f| f.kind == "conduit").count();
    assert!(conduit_count >= horizontal_conduits + 4, "at least 4 vertical + {horizontal_conduits} horizontal CONDUITSEGMENTs, got {conduit_count}");
    let _ = std::fs::remove_dir_all(&dir);
}
