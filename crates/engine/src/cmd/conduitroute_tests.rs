use super::*;

const UPF: f64 = 12.0;

fn duct(x0: f64, y0: f64, x1: f64, y1: f64, z0: f64, z1: f64) -> Obs {
    Obs {
        handle: Handle(0x77),
        lo: Vec3::new(x0, y0, z0),
        hi: Vec3::new(x1, y1, z1),
        category: "duct".into(),
        system: "SA".into(),
        trade: "mechanical",
    }
}

fn square(leg: &Leg) -> bool {
    leg.pts.windows(2).all(|w| matches!(w, [a, b] if (a.x - b.x).abs() < 1e-6 || (a.y - b.y).abs() < 1e-6))
}

/// Tiers 9'-3" to 10'-3" (sheet inches).
fn tiers() -> Vec<f64> {
    vec![111.0, 115.0, 119.0, 123.0]
}

#[test]
fn a_leg_is_square_and_clear_when_nothing_is_in_the_way() {
    let field = Field::new(Vec::new(), UPF);
    let leg = route_leg(Vec2::new(0.0, 0.0), Vec2::new(240.0, 120.0), &tiers(), 0.375, &field, UPF, &[], rules());
    assert!(square(&leg), "{leg:?}");
    assert!(leg.blocked.is_empty());
    assert_eq!(leg.z.len() + 1, leg.pts.len());
    assert!((poly_len(&leg.pts) - 360.0).abs() < 1e-6, "an L: {}", poly_len(&leg.pts));
}

#[test]
fn a_duct_in_the_way_is_cleared_by_going_over_it_or_around() {
    // A 16" deep duct from 9'-1" to 10'-5", 33' wide, across the path: every tier hits it and no
    // detour (8' at most) gets round it: blocked, a clash.
    let field = Field::new(vec![duct(100.0, -200.0, 124.0, 200.0, 109.0, 125.0)], UPF);
    let leg = route_leg(Vec2::new(0.0, 0.0), Vec2::new(240.0, 0.0), &tiers(), 0.375, &field, UPF, &[], rules());
    assert!(square(&leg));
    assert_eq!(leg.blocked.len(), 1, "{leg:?}");
    // A 12" deep one (to 10'-1"): the top tier passes over it.
    let field = Field::new(vec![duct(100.0, -200.0, 124.0, 200.0, 109.0, 121.0)], UPF);
    let leg = route_leg(Vec2::new(0.0, 0.0), Vec2::new(240.0, 0.0), &tiers(), 0.375, &field, UPF, &[], rules());
    assert!(leg.blocked.is_empty() && leg.z == vec![123.0], "{leg:?}");
    // A shorter duct (y -24..24) is gone around (a detour) or over (a tier): no clash.
    let field = Field::new(vec![duct(100.0, -24.0, 124.0, 24.0, 109.0, 121.0)], UPF);
    let leg = route_leg(Vec2::new(0.0, 0.0), Vec2::new(240.0, 0.0), &tiers(), 0.375, &field, UPF, &[], rules());
    assert!(leg.blocked.is_empty(), "{leg:?}");
    assert!(square(&leg));
    // A low duct (bottom at 9'-9") lets the first tier pass under it: straight.
    let field = Field::new(vec![duct(100.0, -200.0, 124.0, 200.0, 117.0, 129.0)], UPF);
    let leg = route_leg(Vec2::new(0.0, 0.0), Vec2::new(240.0, 0.0), &tiers(), 0.375, &field, UPF, &[], rules());
    assert!(leg.blocked.is_empty() && leg.pts.len() == 2 && leg.z == vec![111.0], "{leg:?}");
}

#[test]
fn the_skip_list_lets_a_homerun_reach_its_own_panel() {
    let mut panel = duct(200.0, -10.0, 220.0, 10.0, 24.0, 120.0);
    panel.category = "equipment".into();
    panel.handle = Handle(0x99);
    let field = Field::new(vec![panel], UPF);
    let blocked = route_leg(Vec2::new(0.0, 0.0), Vec2::new(210.0, 0.0), &tiers(), 0.375, &field, UPF, &[], rules());
    assert!(!blocked.blocked.is_empty());
    let clear = route_leg(Vec2::new(0.0, 0.0), Vec2::new(210.0, 0.0), &tiers(), 0.375, &field, UPF, &[Handle(0x99)], rules());
    assert!(clear.blocked.is_empty());
}

#[test]
fn a_spanning_tree_joins_every_device_once() {
    let pts = vec![Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 120.0), Vec2::new(0.0, 240.0), Vec2::new(500.0, 0.0)];
    let t = spanning_tree(&pts, 0);
    assert_eq!(t.len(), 4);
    let mut reached: BTreeSet<usize> = BTreeSet::from([0]);
    for (a, b) in &t {
        assert!(reached.contains(a), "{t:?}");
        reached.insert(*b);
    }
    assert_eq!(reached.len(), 5);
    assert!(spanning_tree(&pts[..1], 0).is_empty());
    assert!(spanning_tree(&pts, 9).is_empty());
}

#[test]
fn a_homerun_follows_the_corridor_lanes() {
    // An L corridor: along x (y 480..576) and down along y at its east end (x 1104..1200).
    let rects = vec![[[0.0, 480.0], [1200.0, 576.0]], [[1104.0, 0.0], [1200.0, 576.0]]];
    let from = Vec2::new(120.0, 700.0);
    let to = Vec2::new(1152.0, 24.0);
    let p = corridor_path(&rects, from, to, UPF).expect("a path");
    assert!(p.windows(2).all(|w| matches!(w, [a, b] if (a.x - b.x).abs() < 1e-6 || (a.y - b.y).abs() < 1e-6)), "{p:?}");
    // It runs along each corridor's branch lane, a quarter of the way across (beside the centre
    // line the feeders take): y 504 in the first, x 1128 in the second.
    assert!(p.iter().any(|q| (q.y - 504.0).abs() < 1e-6), "{p:?}");
    assert!(p.iter().any(|q| (q.x - 1128.0).abs() < 1e-6), "{p:?}");
    assert_eq!(p.first().copied(), Some(from));
    assert_eq!(p.last().copied(), Some(to));
    assert!(corridor_path(&[], from, to, UPF).is_none());
}

#[test]
fn fill_picks_the_smallest_raceway_at_40_percent() {
    let rl = rules();
    // Three circuits' six #12 and a ground: 0.0931 sq in, a 3/4" EMT holds 0.2132.
    let six = vec![(6, "12".to_string(), false), (1, "12".to_string(), true)];
    let (t, area, allowed) = fill_size(&six, "EMT", 0.75, rl).expect("sized");
    assert_eq!(t, 0.75);
    assert!((area - 0.0931).abs() < 1e-6 && (allowed - 0.2132).abs() < 1e-6, "{area} {allowed}");
    // Seventeen #12: 0.2261 sq in, over a 3/4": a 1".
    let many = vec![(16, "12".to_string(), false), (1, "12".to_string(), true)];
    assert_eq!(fill_size(&many, "EMT", 0.75, rl).map(|f| f.0), Some(1.0));
    // The minimum stands even when a 1/2" would hold it.
    let two = vec![(2, "12".to_string(), false), (1, "12".to_string(), true)];
    assert_eq!(fill_size(&two, "EMT", 0.75, rl).map(|f| f.0), Some(0.75));
    assert_eq!(fill_size(&two, "EMT", 0.5, rl).map(|f| f.0), Some(0.5));
}

#[test]
fn more_than_three_current_carrying_conductors_are_derated() {
    let rl = rules();
    assert_eq!(adjustment(3, rl), 1.0);
    assert_eq!(adjustment(6, rl), 0.8);
    assert_eq!(adjustment(9, rl), 0.7);
    assert_eq!(adjustment(10, rl), 0.5);
    // #12 THHN: 30 A at 90 °C, never above 25 A (75 °C) nor 20 A (240.4(D)).
    assert_eq!(allowed_a("12", 6, rl), 20.0);
    assert_eq!(allowed_a("12", 9, rl), 20.0);
    // Ten: 30 × 0.5 = 15 A, too little for a 20 A breaker; #10: 40 × 0.5 = 20 A.
    assert_eq!(allowed_a("12", 10, rl), 15.0);
    assert_eq!(allowed_a("10", 10, rl), 20.0);
}

#[test]
fn conductors_are_sized_by_the_legends_table_else_the_voltage_drop() {
    let rl = rules();
    // No table: 3% at 16 A, 120 V. 50 ft is #12 (2.6%); 100 ft needs #8 (#10 is 3.3%).
    assert_eq!(size_for_length(50.0, 16.0, 120.0, &[], rl).map(|s| s.0), Some("12".to_string()));
    let (s, why) = size_for_length(100.0, 16.0, 120.0, &[], rl).expect("sized");
    assert_eq!(s, "8");
    assert!(why.starts_with("computed"), "{why}");
    let v12 = vdrop_pct("12", 100.0, 16.0, 120.0, rl).unwrap_or(0.0);
    assert!((v12 - 5.27).abs() < 0.01, "{v12}");
    // The legend's bands win.
    let (table, read) = length_table(&["0-60' = #12 AWG".into(), "61'-100' = #10 AWG".into(), "101' TO 150' = #8 AWG".into(), "SEE NOTE 4".into()]);
    assert_eq!(table, vec![(60.0, "12".to_string()), (100.0, "10".to_string()), (150.0, "8".to_string())]);
    assert!(read.contains("0-60'"), "{read}");
    let (s, why) = size_for_length(70.0, 16.0, 120.0, &table, rl).expect("sized");
    assert_eq!((s.as_str(), why.starts_with("legend")), ("10", true));
    // One text with all the bands, and a text that is no table.
    let (t2, _) = length_table(&["HOMERUN LENGTH\n0-75' = #12\n76-125' = #10".into()]);
    assert_eq!(t2.len(), 2);
    assert!(length_table(&["#12 AWG THHN".into(), "12'-0\"".into()]).0.is_empty());
}

#[test]
fn box_fill_counts_conductors_a_ground_and_the_yoke() {
    let rl = rules();
    // Four #12 entering, their grounds, a receptacle: 4 × 2.25 + 2.25 + 2 × 2.25 = 15.75 cu in.
    let v = box_volume(&[(2, "12".into()), (2, "12".into())], true, true, rl);
    assert!((v - 15.75).abs() < 1e-9, "{v}");
    let v = box_volume(&[(6, "12".into()), (2, "10".into())], true, false, rl);
    assert!((v - (13.5 + 5.0 + 2.5)).abs() < 1e-9, "{v}");
}

#[test]
fn the_specification_sets_the_minimum_and_the_circuits_per_homerun() {
    let mut m = Methods::default();
    spec_limits("A. Minimum raceway size shall be 3/4 inch.\nB. Provide not more than three circuits per homerun.", "26 05 33", &mut m);
    assert_eq!(m.min_size_in, Some(0.75));
    assert_eq!(m.max_circuits, Some(3));
    assert!(m.min_size_source.contains("26 05 33") && m.max_circuits_source.contains("three"), "{m:?}");
    let mut none = Methods::default();
    spec_limits("Conduit shall be EMT.", "26 05 33", &mut none);
    assert_eq!((none.min_size_in, none.max_circuits), (None, None));
}

#[test]
fn crossing_a_rated_wall_is_a_penetration_but_ending_in_it_is_not() {
    let wall = vec![Vec2::new(100.0, -100.0), Vec2::new(106.0, -100.0), Vec2::new(106.0, 100.0), Vec2::new(100.0, 100.0)];
    assert!(crosses(Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0), &wall));
    assert!(!crosses(Vec2::new(0.0, 0.0), Vec2::new(103.0, 0.0), &wall));
    assert!(!crosses(Vec2::new(0.0, 200.0), Vec2::new(200.0, 200.0), &wall));
}

#[test]
fn hostile_inputs_do_not_panic() {
    let field = Field::new(vec![duct(f64::NAN, 0.0, 1e300, 1.0, 0.0, 1.0), duct(-1e18, -1e18, 1e18, 1e18, 0.0, 1.0)], UPF);
    let _ = route_leg(Vec2::new(0.0, 0.0), Vec2::new(1e12, -1e12), &[], 0.0, &field, UPF, &[], rules());
    let _ = route_leg(Vec2::new(f64::NAN, 0.0), Vec2::new(1.0, 1.0), &tiers(), 0.375, &field, UPF, &[], rules());
    let _ = corridor_path(&[[[f64::NAN, 0.0], [1.0, 1.0]], [[0.0, 0.0], [0.0, 0.0]]], Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0), UPF);
    let _ = length_table(&["9999999999999-1' = #".into(), "'#".into()]);
    let _ = fill_size(&[(u32::MAX, "999".into(), false)], "NOPE", f64::NAN, rules());
    let _ = spanning_tree(&[Vec2::new(f64::NAN, 0.0), Vec2::new(0.0, 0.0)], 0);
}

/// I-joists along y (from y = -240 to 240) every 24" from x = 0 to 240, 2.5" wide, their depth
/// from 112" to 124" (sheet inches); or trusses there from 112" to 136".
fn joisted(truss: bool) -> Field {
    let top = if truss { 136.0 } else { 124.0 };
    let members = (0..=10)
        .map(|k| {
            let x = 24.0 * f64::from(k);
            (
                Obs {
                    handle: Handle(0x55),
                    lo: Vec3::new(x - 1.25, -240.0, 112.0),
                    hi: Vec3::new(x + 1.25, 240.0, top),
                    category: FRAMING.into(),
                    system: "11 7/8\" I-joist".into(),
                    trade: "framing",
                },
                Frame { a: Vec2::new(x, -240.0), b: Vec2::new(x, 240.0), t: 2.5, truss },
            )
        })
        .collect();
    Field::with_framing(Vec::new(), members, UPF)
}

#[test]
fn a_run_across_the_joists_bores_each_web_in_its_hole_zone_and_never_a_flange() {
    let field = joisted(false);
    let (a, b) = (Vec2::new(12.0, 0.0), Vec2::new(228.0, 0.0));
    // 3/4" raceway: a 1-1/8" hole. At 114" its bottom (113.44") would be in the bottom flange
    // (112" + 1.5" + 1/8").
    assert!(matches!(field.framing(a, b, 114.0, 0.375, UPF, rules()), Err((_, FLANGE))));
    let holes = field.framing(a, b, 118.0, 0.375, UPF, rules()).expect("in the web");
    assert_eq!(holes.len(), 9, "the joists at 24\" … 216\"");
    assert!(holes.iter().all(|h| (h.dia_in - 1.125).abs() < 1e-9 && h.z == 118.0));
    assert!(holes.windows(2).all(|w| matches!(w, [p, q] if q.at.x > p.at.x)), "nearest first");
    // The router seats the run at the first tier the web takes, and counts its holes.
    let leg = route_leg(a, b, &[114.0, 118.0, 122.0], 0.375, &field, UPF, &[], rules());
    assert!(leg.blocked.is_empty(), "{leg:?}");
    assert_eq!(leg.z, vec![118.0]);
    assert_eq!(leg.holes, 9);
    // Below the joists nothing is bored.
    assert_eq!(field.framing(a, b, 110.0, 0.375, UPF, rules()).map(|h| h.len()), Ok(0));
}

#[test]
fn a_hole_near_the_bearing_or_too_big_for_the_web_is_not_bored() {
    let field = joisted(false);
    // 4" from the joists' end (y = 240): a 1-1/8" hole needs 6" (the rule's 1-1/2" row).
    let near = field.framing(Vec2::new(12.0, 236.0), Vec2::new(60.0, 236.0), 118.0, 0.375, UPF, rules());
    assert!(matches!(near, Err((_, NEAR_END))), "{near:?}");
    let ok = field.framing(Vec2::new(12.0, 200.0), Vec2::new(60.0, 200.0), 118.0, 0.375, UPF, rules());
    assert_eq!(ok.map(|h| h.len()), Ok(2));
    // A 7" raceway's hole (7-3/8") is bigger than the rules bore.
    let big = field.framing(Vec2::new(12.0, 0.0), Vec2::new(60.0, 0.0), 118.0, 3.5, UPF, rules());
    assert!(matches!(big, Err((_, TOO_BIG))), "{big:?}");
}

#[test]
fn a_run_along_the_joists_keeps_to_a_bay_and_a_drop_stands_clear_of_them() {
    let field = joisted(false);
    // Straight along the joist at x = 24: inside it.
    let inside = field.framing(Vec2::new(24.0, -100.0), Vec2::new(24.0, 100.0), 118.0, 0.375, UPF, rules());
    assert!(matches!(inside, Err((_, IN_JOIST))), "{inside:?}");
    // The router runs it in the bay beside (x = 12 or 36), boring nothing on the way.
    let leg = route_leg(Vec2::new(24.0, -100.0), Vec2::new(24.0, 100.0), &[118.0], 0.375, &field, UPF, &[], rules());
    assert!(leg.blocked.is_empty(), "{leg:?}");
    assert!(square(&leg));
    let along: Vec<f64> = leg.pts.windows(2).filter_map(|w| if let [p, q] = w { ((p.x - q.x).abs() < 1e-9).then_some(p.x) } else { None }).collect();
    assert!(!along.is_empty() && along.iter().all(|x| ((x - 12.0).abs() < 1e-6) || ((x - 36.0).abs() < 1e-6)), "{leg:?}");
    // A drop under the joist at x = 24 meets its bottom flange; one in the bay is clear.
    assert!(field.hit_column(Vec2::new(24.0, 0.0), 100.0, 118.0, 0.375, UPF, &[], rules()).is_some());
    assert!(field.hit_column(Vec2::new(12.0, 0.0), 100.0, 118.0, 0.375, UPF, &[], rules()).is_none());
    // And a horizontal piece is never an obstacle hit (the framing check decides it).
    assert!(field.hit(Vec2::new(12.0, 0.0), Vec2::new(228.0, 0.0), 118.0, 0.375, UPF, &[], rules()).is_none());
}

#[test]
fn a_truss_is_passed_between_its_chords_never_bored() {
    let field = joisted(true);
    let (a, b) = (Vec2::new(12.0, 0.0), Vec2::new(60.0, 0.0));
    assert!(matches!(field.framing(a, b, 114.0, 0.375, UPF, rules()), Err((_, CHORD))));
    let pass = field.framing(a, b, 124.0, 0.375, UPF, rules()).expect("through the web");
    assert_eq!(pass.len(), 2);
    assert!(pass.iter().all(|h| h.dia_in == 0.0));
}

#[test]
fn hostile_framing_never_panics() {
    let bad = vec![
        (duct(f64::NAN, 0.0, 1.0, 1.0, 0.0, 1.0), Frame { a: Vec2::new(f64::NAN, 0.0), b: Vec2::new(1.0, 1.0), t: 1.0, truss: false }),
        (duct(-1e18, -1e18, 1e18, 1e18, 0.0, 1.0), Frame { a: Vec2::new(-1e18, 0.0), b: Vec2::new(1e18, 0.0), t: 1e9, truss: false }),
        (duct(0.0, 0.0, 0.0, 0.0, 0.0, 1.0), Frame { a: Vec2::new(0.0, 0.0), b: Vec2::new(0.0, 0.0), t: 2.5, truss: true }),
    ];
    let field = Field::with_framing(Vec::new(), bad, UPF);
    let _ = field.framing(Vec2::new(0.0, 0.0), Vec2::new(1e300, 0.0), 0.5, 0.375, UPF, rules());
    let _ = field.framing(Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), f64::NAN, 0.375, UPF, rules());
    let _ = field.bays_near(Vec2::new(0.0, 0.0), UPF);
    let _ = route_leg(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), &[0.5], 0.375, &field, UPF, &[], rules());
}

#[test]
fn a_breaker_that_feeds_gear_is_its_feeder_not_a_branch_circuit() {
    let row = |n: &str, desc: &str, wire: &str| cadaddy_walk3d::PanelCircuit {
        number: n.into(),
        description: desc.into(),
        breaker: "100".into(),
        poles: 2,
        wire: wire.into(),
        ..Default::default()
    };
    let feeder = |tag: &str, from: &str, to: &str| cadaddy_walk3d::Feeder { tag: tag.into(), from: from.into(), to: to.into(), ..Default::default() };
    let mut sp = BuildingSpec::default();
    sp.mep.panel_schedules = vec![
        cadaddy_walk3d::PanelSchedule {
            panel: "P2".into(),
            circuits: vec![
                // The gear named in the description (either word order, any punctuation).
                row("3", "PANELBOARD U (6 UNITS)", ""),
                row("4", "RECEPTACLES - LOBBY", ""),
                // The feeder named in the wire column, the description saying nothing of it.
                row("5", "SPARE", "feeder 7: 1 set of 3 #1 cu in 1-1/2\" conduit"),
                row("6", "Unit sub-panel: X-2", ""),
            ],
            ..Default::default()
        },
        cadaddy_walk3d::PanelSchedule { panel: "Q".into(), circuits: vec![row("3", "PANELBOARD U", "")], ..Default::default() },
    ];
    sp.mep.feeders = vec![feeder("12", "P2", "U"), feeder("7", "P2", "W"), feeder("9", "p2", "x-2"), feeder("1", "MSB", "P2")];
    assert_eq!(feeder_of(&sp, "P2", "P2-3").map(|f| f.to.as_str()), Some("U"));
    assert_eq!(feeder_of(&sp, "P2", "P2-5").map(|f| f.tag.as_str()), Some("7"), "by the wire column's feeder tag");
    assert_eq!(feeder_of(&sp, "p2", "P2-6").map(|f| f.tag.as_str()), Some("9"), "case and punctuation don't matter");
    assert!(feeder_of(&sp, "P2", "P2-4").is_none(), "a branch circuit");
    assert!(feeder_of(&sp, "P2", "P2-8").is_none(), "no schedule row");
    assert!(feeder_of(&sp, "Q", "Q-3").is_none(), "no feeder from Q: a word alike is not a feeder");
    assert!(feeder_of(&sp, "", "P2-3").is_none());
    assert!(feeder_of(&sp, "P2", "not a circuit").is_none());
    // Hostile text never panics.
    sp.mep.panel_schedules[0].circuits.push(row("9", &"U ".repeat(10_000), &"FEEDER ".repeat(5_000)));
    let _ = feeder_of(&sp, "P2", "P2-9");
}

#[test]
fn a_feeder_the_job_routed_as_whole_runs_is_cut_into_pieces_with_its_conductors_by_the_reroute() {
    let mut s = crate::Session::empty();
    s.open_drawing(cadaddy_doc::Drawing::new_imperial(), "Sample Feeder Reroute", None);
    // A made-up storey: switchboard SB1, panel PA, feeder F3 (one set of 250 kcmil copper, a 4/0
    // neutral, a #4 ground in 2-1/2") from the riser diagram; built before its route exists.
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "mep": {
            "systems": [
                {"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [2.5, 0], "bottom_ft": 10},
                {"name": "E-UG", "layer": "E-UG", "kind": "conduit", "size_in": [1, 0], "bottom_ft": -1.5}
            ],
            "equipment": [
                {"tag": "SB1", "kind": "switchboard", "level": "L1", "min": [0, 0], "max": [48, 24], "height_ft": 7, "bottom_ft": 0},
                {"tag": "PA", "kind": "panel", "level": "L1", "min": [480, 0], "max": [504, 24], "height_ft": 3, "bottom_ft": 4}
            ],
            "feeders": [{"tag": "F3", "from": "SB1", "to": "PA", "sets": 1, "phases": 3, "size": "250 kcmil", "material": "cu", "neutrals": 1,
                         "neutral_size": "4/0", "grounds": 1, "ground_size": "#4", "conduits": 1, "conduit_in": 2.5, "entry": "top", "source": "riser diagram E-602"}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    // The job's feeder step: its route into the spec, built as whole runs (up 3', 38' across, down 3').
    let (mut sp, _) = super::bspec::current(&s, "test").unwrap();
    let on = |system: &str, src: &str, pts: Vec<[f64; 2]>, z: f64, top: Option<f64>| -> MepRun {
        serde_json::from_value(json!({"system": system, "level": "L1", "bottoms_ft": vec![z; pts.len().saturating_sub(1).max(1)], "points": pts,
            "size_in": [2.5, 0.0], "top_ft": top, "source_sheet": src, "height_source": "proposed"}))
        .unwrap()
    };
    let run = |pts: Vec<[f64; 2]>, z: f64, top: Option<f64>| on("E-FEEDER", "feeder F3 SB1→PA", pts, z, top);
    let mut runs =
        vec![run(vec![[36.0, 6.0]], 7.0, Some(10.0)), run(vec![[36.0, 6.0], [492.0, 6.0]], 10.0, None), run(vec![[492.0, 6.0]], 7.0, Some(10.0))];
    // A buried leg of the feeder, and the underground plan's own run on the same buried system.
    runs.push(on("E-UG", "feeder F3 SB1→PA", vec![[30.0, 2.0], [30.0, -100.0]], -1.5, None));
    runs.push(on("E-UG", "underground plan", vec![[200.0, -200.0], [400.0, -200.0]], -2.0, None));
    sp.mep.runs.extend(runs.iter().cloned());
    let mut part = sp.clone();
    part.mep.runs = runs;
    let d = s.doc_mut().unwrap();
    d.building_spec = Some(sp.to_json());
    super::mep::build(d, &part);
    let whole = |s: &crate::Session| {
        s.doc().unwrap().model.iter().filter(|e| !super::mepatoms::is_atom(e) && super::bimlook::made_tag(e, "system") == Some("E-FEEDER")).count()
    };
    assert!(whole(&s) > 0, "the job's whole runs");
    let r = s.execute("conduit.reroute", &json!({})).unwrap();
    assert_eq!(r["routed"], json!(true), "{r}");
    // Again (the job reroutes once its devices are reseated): the feeder's buried pieces don't
    // make the underground plan's run pieces; it stays whole, once.
    s.execute("conduit.reroute", &json!({})).unwrap();
    let ug = |s: &crate::Session, atom: bool| {
        s.doc()
            .unwrap()
            .model
            .iter()
            .filter(|e| super::mepatoms::is_atom(e) == atom && super::bimlook::made_tag(e, "system") == Some("E-UG"))
            .filter(|e| super::bimlook::made_tag(e, "mepid").is_some_and(|m| m.starts_with("@200.00,-200.00")))
            .count()
    };
    assert!(ug(&s, false) > 0, "the underground plan's run stays whole");
    assert_eq!(ug(&s, true), 0, "and is not cut into pieces besides");
    assert_eq!(whole(&s), 0, "the whole runs give way to the pieces");
    let d = s.doc().unwrap();
    let wire: Vec<(String, String, f64)> = d
        .model
        .iter()
        .map(|e| props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(Prop::text).as_deref() == Some("wire"))
        .map(|p| {
            let g = |k: &str| p.get(k).map(Prop::text).unwrap_or_default();
            (g("system"), g("size"), g("length_ft").parse::<f64>().unwrap_or(0.0))
        })
        .collect();
    // Along its 44' route with 3' at each gear: 50' a conductor; no entry stubs (system F3).
    let ft = |size: &str| wire.iter().filter(|w| w.0 == "E-FEEDER" && w.1 == size).map(|w| w.2).sum::<f64>();
    assert!((ft("250 kcmil") - 150.0).abs() < 0.05, "{wire:?}");
    assert!((ft("#4/0") - 50.0).abs() < 0.05, "{wire:?}");
    assert!(!wire.iter().any(|w| w.0 == "F3"), "{wire:?}");
    let sticks = d
        .model
        .iter()
        .map(|e| props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(Prop::text).as_deref() == Some("conduit") && p.get("system").map(Prop::text).as_deref() == Some("E-FEEDER"))
        .count();
    assert!(sticks >= 5, "44' of 2-1/2\" in 10' sticks: {sticks}");
}

#[test]
fn a_cable_runs_through_the_studs_along_its_wall_and_round_a_corner() {
    let rl = rules();
    let (x, y) = (Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0));
    // Two boxes of one wall (faces +y, the wall along x), 10' apart, their tops at 18": up 12"
    // over the higher one to 2'-6", 10' through the studs, down: 12'.
    let (pts, bot) =
        stud_path((Vec2::new(0.0, 0.0), 18.0, Some(y), "101"), (Vec2::new(120.0, 0.0), 18.0, Some(y), "101"), 0.0, UPF, rl).expect("in the wall");
    let mut r = super::super::printcond::run(BRANCH_SYSTEM, "L1", "", pts, 0.75, 0.0);
    r.bottoms_ft = bot;
    assert_eq!(r.bottoms_ft.len() + 1, r.points.len(), "{r:?}");
    assert!((run_ft(&r, UPF) - 12.0).abs() < 1e-9, "{}", run_ft(&r, UPF));
    // One of them higher (a switch at 4'): over it, 5', so 3'-6" up from the receptacle, 1' up
    // from the switch, and the 10' between.
    let (pts, bot) =
        stud_path((Vec2::new(0.0, 0.0), 18.0, Some(y), "101"), (Vec2::new(120.0, 0.0), 48.0, Some(y), "101"), 0.0, UPF, rl).expect("in the wall");
    r.points = pts;
    r.bottoms_ft = bot;
    assert!((run_ft(&r, UPF) - (10.0 + 3.5 + 1.0)).abs() < 1e-9, "{}", run_ft(&r, UPF));
    // Two walls meeting at a corner: along the first to the corner, along the second: 5' + 6'.
    let (pts, bot) = stud_path((Vec2::new(0.0, 60.0), 18.0, Some(x), "101"), (Vec2::new(72.0, 0.0), 18.0, Some(y), "101"), 0.0, UPF, rl)
        .expect("round the corner");
    assert!(pts.iter().any(|p| p[0].abs() < 1e-9 && p[1].abs() < 1e-9), "by the corner: {pts:?}");
    r.points = pts;
    r.bottoms_ft = bot;
    assert!((run_ft(&r, UPF) - (11.0 + 2.0)).abs() < 1e-9, "{}", run_ft(&r, UPF));
    // Facing each other (opposite walls), in two rooms, without a face, or farther than the
    // rules' reach: over the ceiling instead.
    let far = rl.cable.in_wall_max_ft * UPF + 12.0;
    assert!(stud_path((Vec2::new(0.0, 0.0), 18.0, Some(y), "101"), (Vec2::new(0.0, 120.0), 18.0, Some(y * -1.0), "101"), 0.0, UPF, rl).is_none());
    assert!(stud_path((Vec2::new(0.0, 0.0), 18.0, Some(y), "101"), (Vec2::new(120.0, 0.0), 18.0, Some(y), "102"), 0.0, UPF, rl).is_none());
    assert!(stud_path((Vec2::new(0.0, 0.0), 18.0, None, "101"), (Vec2::new(120.0, 0.0), 18.0, Some(y), "101"), 0.0, UPF, rl).is_none());
    assert!(stud_path((Vec2::new(0.0, 0.0), 18.0, Some(y), "101"), (Vec2::new(far, 0.0), 18.0, Some(y), "101"), 0.0, UPF, rl).is_none());
    assert!(!rl.cable.source.is_empty() && rl.cable.above_box_in > 0.0);
}

#[test]
fn a_switch_is_on_the_circuit_most_of_what_it_controls_is_on() {
    let circuit_of: HashMap<Handle, String> =
        [(Handle(1), "A-1".to_string()), (Handle(2), "A-3".to_string()), (Handle(3), "A-3".to_string())].into_iter().collect();
    assert_eq!(controlled_circuit(&[Handle(1), Handle(2), Handle(3)], &circuit_of).as_deref(), Some("A-3"));
    assert_eq!(controlled_circuit(&[Handle(1), Handle(2)], &circuit_of).as_deref(), Some("A-1"), "a tie: the first named");
    assert_eq!(controlled_circuit(&[Handle(9)], &circuit_of), None);
    let mut ps = props::Props::default();
    ps.set("controls", Prop::new("1A2B, 3c4d,zz, 0", props::Source::computed()));
    assert_eq!(controls_of(&ps), vec![Handle(0x1A2B), Handle(0x3C4D)]);
}

#[test]
fn the_lengths_are_reported_by_unit_type_common_area_and_level() {
    let sp: BuildingSpec = serde_json::from_value(json!({
        "version": 1, "units_per_foot": 12, "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10}],
        "unit_types": [{"id": "SUITE-A", "name": "Suite A"}],
        "unit_instances": [{"number": "11", "level": "L1", "unit_type": "SUITE-A"}, {"number": "12", "level": "L1", "unit_type": "SUITE-A"}]
    }))
    .expect("spec");
    let l = |home: f64, in_area: f64, wire: f64| Lens { in_area, home, wire };
    let rows = vec![
        ("L1".to_string(), "11".to_string(), l(10.0, 30.0, 150.0)),
        ("L1".to_string(), "11".to_string(), l(6.0, 20.0, 90.0)),
        ("L1".to_string(), "12".to_string(), l(12.0, 40.0, 170.0)),
        ("L1".to_string(), "LOBBY".to_string(), l(50.0, 70.0, 400.0)),
    ];
    let r = lengths_report(&sp, &rows);
    let ty = &r["unit_types"][0];
    assert_eq!((ty["unit_type"].as_str(), ty["name"].as_str(), ty["rooms"].as_u64()), (Some("SUITE-A"), Some("Suite A"), Some(2)), "{r}");
    assert_eq!(ty["per_room"], json!({"circuits": 1.5, "home_ft": 14.0, "in_area_ft": 45.0, "total_ft": 59.0, "conductor_ft": 205.0}), "{r}");
    assert_eq!(r["typical_rooms"].as_array().map(Vec::len), Some(2));
    assert_eq!(r["common_areas"][0]["room"], json!("LOBBY"));
    assert_eq!(r["common_areas"][0]["total_ft"], json!(120.0));
    assert_eq!(r["levels"][0]["total_ft"], json!(238.0));
    assert_eq!(r["total"]["circuits"], json!(4.0));
    assert_eq!(r["total"]["conductor_ft"], json!(810.0));
}

#[test]
fn a_feeders_raceway_stays_in_the_way_of_the_branch_routes_once_cut_into_pieces() {
    let el = |category: &str, system: &str, run: &str| super::bimwork::El {
        handle: Handle(0x10),
        trade: "electrical",
        category: category.into(),
        system: system.into(),
        level: "L1".into(),
        lo: Vec3::new(0.0, 0.0, 0.0),
        hi: Vec3::new(1.0, 1.0, 1.0),
        run: run.into(),
        in_wall: false,
    };
    assert!(is_obstacle(&el("conduit", "E-COND", "")), "drawn conduit");
    assert!(is_obstacle(&el("conduit", super::gearbuild::FEEDER_SYSTEM, "@1.00,2.00#0")), "a feeder's sticks");
    assert!(!is_obstacle(&el("conduit", BRANCH_SYSTEM, "@1.00,2.00#0")), "the branch pieces this routing cuts again");
    assert!(!is_obstacle(&el("device", "", "")));
}
