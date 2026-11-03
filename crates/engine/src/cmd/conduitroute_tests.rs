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
    // It runs along the first lane's centre (y 528) and the second's (x 1152).
    assert!(p.iter().any(|q| (q.y - 528.0).abs() < 1e-6), "{p:?}");
    assert!(p.iter().any(|q| (q.x - 1152.0).abs() < 1e-6), "{p:?}");
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
