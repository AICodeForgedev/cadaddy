//! Ceiling assemblies, soffits and head of wall framing (Q295): the fixtures' ceiling legend,
//! soffit and head details and room finish schedule, read back and scored against the truth;
//! the building spec patch they make; the takeoff from a read set against a perfect read's.

use cadaddy_mepfix::ceilasm_reader::{self, AsmScore, parse_ceiling, parse_head, parse_soffit};
use cadaddy_mepfix::truth::Truth;
use cadaddy_mepfix::{generate, truth_for};

const SEEDS: std::ops::RangeInclusive<u64> = 1..=6;

fn scores() -> Vec<(u64, Truth, AsmScore)> {
    SEEDS
        .map(|seed| {
            let (t, _, r) = ceilasm_reader::read_fixture_assemblies(seed).unwrap();
            let sc = ceilasm_reader::score_assemblies(&t, &r);
            (seed, t, sc)
        })
        .collect()
}

#[test]
fn the_truth_is_consistent() {
    for seed in SEEDS {
        let t = truth_for(seed);
        let g = generate(seed).unwrap().truth;
        assert_eq!(t.ceiling_types, g.ceiling_types, "seed {seed}");
        assert_eq!(t.soffit_types, g.soffit_types, "seed {seed}");
        assert_eq!(t.head_details, g.head_details, "seed {seed}");
        assert_eq!(t.room_ceilings, g.room_ceilings, "seed {seed}");
        assert_eq!(t.soffits, g.soffits, "seed {seed}");
        assert_eq!(t.wall_types, g.wall_types, "seed {seed}");
        let tags: Vec<&str> = t.ceiling_types.iter().map(|c| c.tag.as_str()).collect();
        for rc in &t.room_ceilings {
            assert!(tags.contains(&rc.ceiling_type.as_str()), "seed {seed}: room {} has an unknown type", rc.room);
            let rm = t.building.rooms.iter().find(|r| r.level == rc.level && r.number == rc.room).unwrap();
            let ct = t.ceiling_types.iter().find(|c| c.tag == rc.ceiling_type).unwrap();
            match &rm.ceiling {
                None => assert_eq!(ct.kind, "OPEN"),
                Some(c) => assert_eq!(c.kind, ct.kind, "seed {seed}: room {}", rc.room),
            }
        }
        assert_eq!(t.room_ceilings.len(), t.building.rooms.len());
        for sf in &t.soffits {
            let st = t.soffit_types.iter().find(|s| Some(&s.tag) == sf.soffit_type.as_ref()).unwrap();
            let rm = t.building.rooms.iter().find(|r| r.level == sf.level && r.number == sf.room).unwrap();
            let clg = rm.ceiling.as_ref().unwrap();
            // Built under gypsum, hung beside tile.
            assert_eq!(st.hangs_from == "ceiling", clg.kind == "GYP", "seed {seed}: {}", sf.id);
            assert!((sf.depth_ft.unwrap() - (clg.height_ft - sf.height_ft)).abs() < 1e-6);
            assert!(sf.height_ft >= 7.0 - 1e-9, "seed {seed}: {} is lower than 7'", sf.id);
            assert!(sf.sides_ft.unwrap() > 0.0);
        }
        // Every wall type has its head detail; stud walls their bottom track.
        for wt in &t.wall_types {
            let h = t.head_details.iter().find(|h| Some(&h.tag) == wt.head.as_ref()).unwrap();
            assert!(h.wall_types.contains(&wt.tag));
            assert_eq!(wt.bottom_track, wt.layers.iter().any(|l| l.material == "stud"));
            if wt.height == "ceiling" || wt.height == "above_ceiling" {
                assert_eq!(h.to, "ceiling");
            } else {
                assert_eq!(h.to, "deck");
            }
        }
    }
}

#[test]
fn seeds_mix_assemblies_and_heads() {
    let ts: Vec<Truth> = (1..=20).map(truth_for).collect();
    let tracks: std::collections::BTreeSet<String> = ts.iter().flat_map(|t| t.head_details.iter().map(|h| h.track.clone())).collect();
    assert_eq!(tracks.len(), 4, "{tracks:?}");
    let double = ts.iter().filter(|t| t.ceiling_types.iter().any(|c| c.layers.iter().filter(|l| l.material.starts_with("gyp")).count() == 2)).count();
    assert!(double > 2 && double < 20, "{double} of 20 seeds have a two-layer ceiling");
    let framing: std::collections::BTreeSet<String> =
        ts.iter().flat_map(|t| t.ceiling_types.iter().flat_map(|c| c.layers.iter().map(|l| l.material.clone()))).collect();
    for m in ["hat_channel", "furring", "main_runner", "wood_furring", "act_tile", "act_grid", "gyp_mr", "gyp_x"] {
        assert!(framing.contains(m), "no seed has {m}");
    }
    let under = ts
        .iter()
        .flat_map(|t| t.soffits.iter().map(move |s| (t, s)))
        .filter(|(t, s)| t.soffit_types.iter().any(|x| Some(&x.tag) == s.soffit_type.as_ref() && x.hangs_from == "ceiling"));
    assert!(under.count() > 10, "few soffits built under a ceiling");
    assert!(ts.iter().any(|t| t.soffits.iter().any(|s| !s.called_out)));
    assert!(ts.iter().any(|t| t.room_ceilings.iter().any(|r| !r.scheduled)));
}

#[test]
fn legend_details_and_schedule_read_back() {
    for (seed, t, sc) in scores() {
        eprintln!("seed {seed}: {sc:?}");
        let ctx = format!("seed {seed}");
        assert_eq!(sc.ceiling_types_found, sc.ceiling_types_total, "{ctx}");
        assert_eq!(sc.ceiling_stacks_correct, sc.ceiling_types_total, "{ctx}: ceiling layer stacks");
        assert_eq!(sc.ceiling_kinds_correct, sc.ceiling_types_total, "{ctx}");
        assert_eq!(sc.ceiling_ratings_correct, sc.ceiling_types_total, "{ctx}");
        assert_eq!(sc.soffit_types_correct, sc.soffit_types_total, "{ctx}: soffit details");
        assert_eq!(sc.heads_correct, sc.heads_total, "{ctx}: head details");
        assert_eq!(sc.wall_heads_correct, sc.wall_heads_total, "{ctx}: wall types' heads");
        assert_eq!(sc.rooms_unscheduled_flagged, sc.rooms_unscheduled, "{ctx}: unscheduled rooms flagged");
        assert_eq!(sc.rooms_correct, sc.rooms_total, "{ctx}: room ceiling types");
        assert_eq!(t.room_ceilings.len(), sc.rooms_total);
    }
}

#[test]
fn soffits_read_back() {
    let (mut total, mut found, mut typed, mut depth, mut sides, mut uncalled, mut flagged) = (0, 0, 0, 0, 0, 0, 0);
    for (_, _, sc) in scores() {
        total += sc.soffits_total;
        found += sc.soffits_found;
        typed += sc.soffit_type_correct;
        depth += sc.soffit_depth_correct;
        sides += sc.soffit_sides_correct;
        uncalled += sc.soffits_uncalled;
        flagged += sc.soffits_uncalled_flagged;
    }
    eprintln!("soffits {found}/{total}, typed {typed}, depth {depth}, sides {sides}, uncalled {flagged}/{uncalled} flagged");
    assert!(total >= 10, "{total}");
    assert_eq!(found, total);
    assert_eq!(typed, total);
    assert_eq!(depth, total);
    assert_eq!(sides, total);
    assert_eq!(flagged, uncalled);
}

#[test]
fn wording_variants_and_hostile_lines() {
    let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let c = parse_ceiling(
        "C3",
        &l(&[
            "1 HOUR RATED GYPSUM BOARD CEILING",
            "(2) LAYERS 5/8\" TYPE X GWB",
            "ON 7/8\" FURRING CHANNEL @ 16\" O.C.",
            "1-1/2\" COLD-ROLLED CHANNEL MAIN RUNNERS @ 48\" O.C., HUNG FROM STRUCTURE",
            "1-HOUR RATED ASSEMBLY",
        ]),
    );
    assert_eq!(c.kind, "GYP");
    assert_eq!(c.fire_rating.as_deref(), Some("1 HR"));
    let m: Vec<&str> = c.layers.iter().map(|x| x.material.as_str()).collect();
    assert_eq!(m, ["gyp_x", "gyp_x", "furring", "main_runner"]);
    assert_eq!(c.layers[3].spacing_in, Some(48.0));
    let a = parse_ceiling("ACT-1", &l(&["SUSPENDED ACOUSTIC TILE CEILING", "5/8\" ACOUSTIC CEILING TILE, 2'x2'", "15/16\" EXPOSED TEE GRID"]));
    assert_eq!((a.kind.as_str(), a.grid_ft), ("ACT", Some([2.0, 2.0])));
    let s = parse_soffit(
        "4",
        &l(&[
            "GYP. BD. SOFFIT HUNG FROM STRUCTURE",
            "BOTTOM: 5/8\" GYP. BD.",
            "SIDES: 5/8\" GYP. BD.",
            "FRAMING: 3 5/8\" 20 GA. METAL STUD FRAMING @ 16\" O.C.",
            "EXTEND SIDE BOARD 6\" ABOVE CEILING; CEILING STOPS AT SOFFIT",
        ]),
    );
    assert_eq!((s.hangs_from.as_str(), s.above_ceiling_in, s.framing.len()), ("structure", 6.0, 1));
    let h = parse_head("6", &l(&["HEAD OF WALL AT DECK - DOUBLE TRACK", "NESTED DOUBLE TOP TRACK, 3/4\" DEFLECTION", "AT TYPES W-2, W-5 AND W-7"]));
    assert_eq!((h.track.as_str(), h.to.as_str(), h.deflection_in), ("double", "deck", Some(0.75)));
    assert_eq!(h.wall_types, ["W-2", "W-5", "W-7"]);
    // Nothing panics on junk.
    let junk = l(&["", "@@@ \"\"\" ///", "BOTTOM:", "FRAMING: @", "TYPES:", "99999999999999999999\" DEFLECTION", "2'x'x4'", "\u{0}\u{7f}"]);
    let _ = parse_ceiling("X", &junk);
    let _ = parse_soffit("X", &junk);
    let _ = parse_head("X", &junk);
    let _ = parse_ceiling("X", &[]);
}

#[test]
fn spec_patch_builds_and_takes_off() {
    use cadaddy_walk3d::BuildingSpec;
    let seed = 2;
    let (t, walls, r) = ceilasm_reader::read_fixture_assemblies(seed).unwrap();
    let to_plan = |_: &str, p: [f64; 2]| Some([p[0] * 12.0, p[1] * 12.0]);
    let spec = |r: &ceilasm_reader::AsmResult| {
        let mut wp = cadaddy_mepfix::wall_reader::spec_patch(&walls, &to_plan);
        ceilasm_reader::add_heads(&mut wp, r);
        let cp = ceilasm_reader::spec_patch(r, &to_plan);
        let mut s = serde_json::json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9}, {"name": "2", "elevation_ft": 14, "height_ft": 14, "ceiling_ft": 9}],
        });
        for k in ["wall_types", "walls"] {
            s[k] = wp[k].clone();
        }
        for k in ["ceiling_types", "soffit_types", "rcp"] {
            s[k] = cp[k].clone();
        }
        BuildingSpec::from_json(&s.to_string()).unwrap()
    };
    let read = spec(&r);
    let errors: Vec<_> = read.check().into_iter().filter(|i| i.error).collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(read.wall_types.iter().all(|w| w.assembly.as_ref().is_some_and(|a| a.head.is_some())));
    assert!(!read.rcp.ceilings.is_empty() && read.rcp.ceilings.iter().all(|c| c.ceiling_type.is_some()));
    assert!(read.rcp.soffits.iter().all(|s| s.soffit_type.is_some()));
    let d = read.describe();
    assert!(d.iter().any(|x| x.text.contains("hat channel") || x.text.contains("furring") || x.text.contains("wood")), "{d:?}");
    // The takeoff from the read set is the perfect read's.
    let perfect = spec(&ceilasm_reader::result_from_truth(&t));
    let (a, b) = (cadaddy_walk3d::takeoff(&read), cadaddy_walk3d::takeoff(&perfect));
    let sum = |rows: &[cadaddy_walk3d::TakeoffRow], what: &str| -> f64 { rows.iter().filter(|r| r.part.starts_with(what)).map(|r| r.quantity).sum() };
    for part in ["ceiling", "soffit bottom", "soffit sides", "top track", "bottom track"] {
        let (x, y) = (sum(&a.rows, part), sum(&b.rows, part));
        eprintln!("{part}: read {x:.1}, perfect {y:.1}");
        assert!(y > 0.0, "{part}: nothing in the perfect takeoff");
        assert!((x - y).abs() <= 0.01 * y, "{part}: read {x:.1} vs perfect {y:.1}");
    }
    assert!(a.csv().lines().count() == a.rows.len() + 1);
}
