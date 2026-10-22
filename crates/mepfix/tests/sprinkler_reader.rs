//! Sprinkler heads and branch lines read from the fixtures' fire protection and ceiling plans
//! (Q265), scored against the truth, and built into the 3D model and IFC.

use cadaddy_engine::Session;
use cadaddy_mepfix::sprinkler_reader::{SprinklerScore, parse_pipe_size, read_fixture_sprinklers, score_sprinklers, spec_terminals};
use serde_json::json;

fn ratio(a: usize, b: usize) -> f64 {
    if b == 0 { 1.0 } else { a as f64 / b as f64 }
}

#[test]
fn heads_lines_sizes_and_heights_score_over_many_sets() {
    let mut all = SprinklerScore::default();
    let (mut sidewall, mut uprights, mut armovers, mut rotated) = (0, 0, 0, 0);
    for seed in 1..=20 {
        let (t, r) = read_fixture_sprinklers(seed).unwrap();
        let sc = score_sprinklers(&t, &r);
        println!("seed {seed}: {}", sc.summary());
        // Every set: the pipe schedule and the sprig read from the sheets.
        assert_eq!(r.pipe_schedule.first(), Some(&(1.0, 2)), "seed {seed}: pipe schedule {:?}", r.pipe_schedule);
        assert_eq!(r.sprig_in, Some(4.0), "seed {seed}");
        sidewall += r.heads.iter().filter(|h| h.mount == "sidewall").count();
        uprights += r.heads.iter().filter(|h| h.mount == "upright").count();
        armovers += sc.arms_total;
        rotated += usize::from(t.sheets.iter().any(|s| s.discipline == "FP" && s.model_from_building.rotation_deg != 0.0));
        for (k, v) in [
            (&mut all.heads_total, sc.heads_total),
            (&mut all.heads_found, sc.heads_found),
            (&mut all.heads_extra, sc.heads_extra),
            (&mut all.mount_correct, sc.mount_correct),
            (&mut all.heights, sc.heights),
            (&mut all.heights_within_1in, sc.heights_within_1in),
            (&mut all.arms_total, sc.arms_total),
            (&mut all.arms_found, sc.arms_found),
            (&mut all.on_rcp, sc.on_rcp),
            (&mut all.pieces_total, sc.pieces_total),
            (&mut all.pieces_matched, sc.pieces_matched),
            (&mut all.pieces_size_correct, sc.pieces_size_correct),
            (&mut all.schedule_sized, sc.schedule_sized),
            (&mut all.schedule_correct, sc.schedule_correct),
        ] {
            *k += v;
        }
    }
    println!("all: {}", all.summary());
    // The fixtures vary what is being tested.
    assert!(sidewall > 0 && uprights > 0 && armovers > 0 && rotated > 0, "{sidewall} {uprights} {armovers} {rotated}");
    // Thresholds (the reader scores higher today; these leave room for harder fixtures).
    assert!(ratio(all.heads_found, all.heads_total) >= 0.98, "head recall");
    assert!(ratio(all.heads_extra, all.heads_found) <= 0.02, "extra heads");
    assert!(ratio(all.mount_correct, all.heads_found) >= 0.98, "mount");
    assert!(ratio(all.heights_within_1in, all.heads_found) >= 0.95, "heights within 1\"");
    assert!(ratio(all.arms_found, all.arms_total) >= 0.95, "armovers");
    assert!(ratio(all.on_rcp, all.heads_found) >= 0.95, "heads on the RCP too");
    assert!(ratio(all.pieces_matched, all.pieces_total) >= 0.95, "branch pieces");
    assert!(ratio(all.pieces_size_correct, all.pieces_matched) >= 0.95, "branch sizes");
    assert!(all.schedule_sized > 100 && ratio(all.schedule_correct, all.schedule_sized) >= 0.95, "sizes from the pipe schedule");
}

#[test]
fn every_head_kind_gets_its_height_from_its_own_rule() {
    // Seed 5 has sidewall heads in the corridors, every set has uprights in the mech rooms.
    let (t, r) = read_fixture_sprinklers(5).unwrap();
    for mount in ["pendent", "upright", "sidewall"] {
        let h = r.heads.iter().find(|h| h.mount == mount).unwrap_or_else(|| panic!("no {mount} head"));
        let want = match mount {
            "pendent" => "ceiling",
            "upright" => "line+sprig",
            _ => "ceiling-offset",
        };
        assert_eq!(h.height_source, want);
        let truth = t
            .terminals
            .iter()
            .find(|x| {
                x.kind == "sprinkler_head" && x.level == h.level && (x.at_ft[0] - h.at_ft[0]).abs() < 0.5 && (x.at_ft[1] - h.at_ft[1]).abs() < 0.5
            })
            .unwrap();
        let floor = t.level(&h.level).unwrap().elevation_ft;
        assert!((h.height_ft.unwrap() - (truth.at_ft[2] - floor)).abs() < 1.0 / 12.0, "{mount}: {h:?} vs {truth:?}");
    }
}

#[test]
fn sizes_parse() {
    assert_eq!(parse_pipe_size("1-1/2\""), Some(1.5));
    assert_eq!(parse_pipe_size("2-1/2\""), Some(2.5));
    assert_eq!(parse_pipe_size("FP"), None);
    assert_eq!(parse_pipe_size(""), None);
}

#[test]
fn read_heads_build_in_3d_and_export_as_fire_suppression_terminals() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    let (t, r) = read_fixture_sprinklers(5).unwrap();
    let levels: Vec<serde_json::Value> = t
        .building
        .levels
        .iter()
        .filter(|l| l.code != "R")
        .map(|l| json!({"name": l.name, "elevation_ft": l.elevation_ft, "height_ft": 13, "ceiling_ft": 9, "region": [-1000, -1000, 5000, 2000], "base": [0, 0]}))
        .collect();
    let names: Vec<(&str, &str)> = t.building.levels.iter().map(|l| (l.code.as_str(), l.name.as_str())).collect();
    let terminals = spec_terminals(&r, "FP", &names);
    assert_eq!(terminals.len(), r.heads.len(), "every head has a height");
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "F-PROT", "current": true})).unwrap();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": levels,
            "mep": {
                "systems": [{"name": "FP", "layer": "F-PROT", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 9}],
                "terminals": terminals
            }
        }}),
    )
    .unwrap();
    let built = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(built["terminals"].as_u64(), Some(r.heads.len() as u64), "{built}");
    // A pendent head hangs at its ceiling: the deflector at the height read.
    let pend = r.heads.iter().find(|h| h.mount == "pendent" && h.level == "1").unwrap();
    let z: Vec<f64> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Mesh(m) if e.common.xdata.iter().any(|x| x.value == "part:terminal:sprinkler_pendent") => {
                let c = m.vertices.iter().fold((0.0, 0.0), |a, v| (a.0 + v.x, a.1 + v.y));
                let n = m.vertices.len().max(1) as f64;
                ((c.0 / n - pend.at_ft[0] * 12.0).abs() < 1.0 && (c.1 / n - pend.at_ft[1] * 12.0).abs() < 1.0)
                    .then(|| m.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min))
            }
            _ => None,
        })
        .collect();
    let deflector_bottom = pend.height_ft.unwrap() * 12.0 - 0.25;
    assert!(z.iter().any(|z| (z - deflector_bottom).abs() < 1e-6), "{z:?} vs {deflector_bottom}");
    // IFC: each head an IfcFireSuppressionTerminal, SPRINKLER.
    let dir = std::env::temp_dir().join(format!("cadaddy-sprinklers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sprinklers.ifc");
    s.execute("export", &json!({ "path": path })).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let n = text.lines().filter(|l| l.contains("IFCFIRESUPPRESSIONTERMINAL(") && l.contains(".SPRINKLER.")).count();
    assert_eq!(n, r.heads.len(), "one IfcFireSuppressionTerminal per head");
    let back = cadaddy_io::read(text.as_bytes(), "Sprinklers.ifc").unwrap();
    assert!(back.layers.iter().any(|l| l.name == "IFC-FireSuppressionTerminal"), "read back as its class");
    let _ = std::fs::remove_dir_all(&dir);
}
