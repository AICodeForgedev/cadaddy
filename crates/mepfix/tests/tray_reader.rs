//! Cable tray and large conduit read from the fixtures' power plans (Q266), scored against the
//! truth, built in 3D, clashing with the other trades, and exported to IFC.

use cadaddy_engine::Session;
use cadaddy_mepfix::score;
use cadaddy_mepfix::tray_reader::{read_fixture_trays, spec_runs};
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
        read_pdf: None, read_dwf: None, write_dwfx: None,
    });
    let (t, r) = read_fixture_trays(3).unwrap();
    let names: Vec<(&str, &str)> = t.building.levels.iter().map(|l| (l.code.as_str(), l.name.as_str())).collect();
    let (mut systems, mut runs) = spec_runs(&r, &names);
    assert_eq!(runs.len(), r.runs.len(), "every run has a height");
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
