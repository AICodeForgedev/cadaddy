//! Branch conduit and homeruns read from the electrical plans (Q289), scored against the truth.

use cadaddy_mepfix::conduit_reader::{self, ConduitResult};

#[test]
fn conduit_and_homeruns_are_read_from_the_office_sets() {
    for seed in 1..=8 {
        let (t, r) = conduit_reader::read_fixture_conduits(seed).unwrap();
        let s = conduit_reader::score(&t, &r);
        println!("seed {seed}: {s:?}");
        assert!(s.conduits_truth > 0 && s.homeruns_truth > 0, "seed {seed}: {s:?}");
        assert!(s.conduit_recall >= 0.95 && s.conduit_precision >= 0.95, "seed {seed}: {s:?}");
        assert!((s.size_accuracy - 1.0).abs() < 1e-9, "seed {seed}: {s:?}");
        assert!(s.homerun_recall >= 0.9 && s.homerun_precision >= 0.95, "seed {seed}: {s:?}");
        assert!(s.panel_accuracy >= 0.95, "seed {seed}: {s:?}");
        assert!((s.heights_assigned - 1.0).abs() < 1e-9, "seed {seed}: {s:?}");
    }
}

/// Every route is flagged assumed, runs hang above the ceiling, and homeruns end at a panel.
#[test]
fn routes_are_assumed_and_hang_above_the_ceiling() {
    let (t, r): (_, ConduitResult) = conduit_reader::read_fixture_conduits(3).unwrap();
    for h in r.homeruns.iter().filter(|h| h.panel_ft.is_some()) {
        assert_eq!(h.route_ft.last().copied(), h.panel_ft, "{h:?}");
        assert_eq!(h.bottoms_ft.len() + 1, h.route_ft.len(), "{h:?}");
    }
    for c in &r.conduits {
        let b = c.bottom_ft.unwrap();
        assert!(b > 7.0 && b < 14.0, "{c:?}");
    }
    let names: Vec<(&str, &str)> = t.building.levels.iter().map(|l| (l.code.as_str(), l.name.as_str())).collect();
    let (systems, runs, guesses) = conduit_reader::spec_runs(&r, &names);
    assert_eq!(systems.len(), 2);
    assert!(runs.iter().any(|x| x["system"] == conduit_reader::HOMERUN_SYSTEM));
    assert!(runs.iter().any(|x| x["system"] == conduit_reader::CONDUIT_SYSTEM));
    assert!(guesses.iter().any(|(k, why)| k.contains(conduit_reader::HOMERUN_SYSTEM) && why.contains("assumed")), "{guesses:?}");
}

/// The hotel's untagged corridor conduit is read at the assumed size.
#[test]
fn the_hotels_untagged_conduit_takes_the_default_size() {
    let set = cadaddy_mepfix::generate_hotel(7, cadaddy_mepfix::hotel::HotelSize::Small).unwrap();
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let r = conduit_reader::read_conduits(&dxf, &map);
    let s = conduit_reader::score(&set.truth, &r);
    println!("hotel: {s:?}");
    assert!(s.conduit_recall >= 0.95, "{s:?}");
    assert!(r.conduits.iter().any(|c| !c.size_from_tag && (c.size_in - conduit_reader::DEFAULT_SIZE_IN).abs() < 1e-9));
}

/// Read → building spec → 3D: conduit as raceways and homeruns as cable at their heights, a
/// made-up duct across a conduit is a clash, the assumptions are recorded, and IFC has a
/// conduit segment per conduit and a cable segment per homerun leg.
#[test]
fn read_conduit_build_clash_and_export() {
    use cadaddy_engine::Session;
    use serde_json::json;
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    let (t, r) = conduit_reader::read_fixture_conduits(3).unwrap();
    let names: Vec<(&str, &str)> = t.building.levels.iter().map(|l| (l.code.as_str(), l.name.as_str())).collect();
    let (mut systems, mut runs, guesses) = conduit_reader::spec_runs(&r, &names);
    let routed = r.homeruns.iter().filter(|h| h.route_ft.len() >= 2).count();
    assert_eq!(runs.len(), r.conduits.len() + routed, "every run has a height");
    // A made-up duct across level 1's corridor conduit, at its height.
    let c = r.conduits.iter().find(|c| c.level == "1" && (c.from_ft[1] - c.to_ft[1]).abs() < 1e-6).unwrap();
    let (x, y, b) = ((c.from_ft[0] + c.to_ft[0]) / 2.0 * 12.0, c.from_ft[1] * 12.0, c.bottom_ft.unwrap());
    systems.push(json!({"name": "M-DUCT-SUPP", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [12, 12], "bottom_ft": b - 0.25}));
    let l1 = names.iter().find(|(c, _)| *c == "1").unwrap().1;
    runs.push(json!({"system": "M-DUCT-SUPP", "level": l1, "points": [[x, y - 60.0], [x, y + 60.0]], "size_in": [12, 12], "bottoms_ft": [b - 0.25]}));
    let levels: Vec<serde_json::Value> = t
        .building
        .levels
        .iter()
        .filter(|l| l.code != "R")
        .map(|l| json!({"name": l.name, "elevation_ft": l.elevation_ft, "height_ft": 13, "ceiling_ft": 8, "region": [-1000, -1000, 5000, 2000], "base": [0, 0]}))
        .collect();
    let guess: serde_json::Map<String, serde_json::Value> = guesses.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "E-POWR", "current": true})).unwrap();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": levels, "mep": {"systems": systems, "runs": runs}, "guess": guess}})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let layers: Vec<String> = s.doc().unwrap().layers.iter().map(|l| l.name.clone()).collect();
    assert!(
        layers.iter().any(|l| l.contains(conduit_reader::CONDUIT_SYSTEM)) && layers.iter().any(|l| l.contains(conduit_reader::HOMERUN_SYSTEM)),
        "{layers:?}"
    );
    let clashes = s.execute("model.clashes", &json!({})).unwrap();
    let hit = clashes["clashes"].as_array().unwrap().iter().any(|c| {
        let sys = c["systems"].to_string();
        c["kind"] == "crossing" && sys.contains(conduit_reader::CONDUIT_SYSTEM) && sys.contains("M-DUCT-SUPP")
    });
    assert!(hit, "the duct across the conduit is a clash: {clashes}");
    // The assumptions are in the spec for a person to confirm.
    let spec = s.execute("bspec.get", &json!({})).unwrap();
    assert!(
        spec["spec"]["guess"][format!("mep/{}", conduit_reader::HOMERUN_SYSTEM)].as_str().is_some_and(|w| w.contains("assumed")),
        "{}",
        spec["spec"]["guess"]
    );
    // IFC: conduit segments, and cable segments for the homeruns.
    let text = String::from_utf8(cadaddy_io::ifc::write_ifc(s.doc().unwrap(), "Conduit.ifc").unwrap()).unwrap();
    let conduits = text.lines().filter(|l| l.contains("IFCCABLECARRIERSEGMENT(") && l.contains(".CONDUITSEGMENT.")).count();
    let cables = text.lines().filter(|l| l.contains("IFCCABLESEGMENT(") && l.contains(".CABLESEGMENT.")).count();
    assert!(conduits >= r.conduits.len(), "{conduits} conduit segments");
    assert!(cables >= routed, "{cables} cable segments for {routed} homeruns");
    assert!(text.contains("IFCCABLEFITTING("), "the homeruns' turns");
}
