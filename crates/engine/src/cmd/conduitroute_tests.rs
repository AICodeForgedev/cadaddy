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

/// Each box's way back to the root along `tree` (plan, square), the farthest.
fn farthest(pts: &[Vec2], root: usize, tree: &[(usize, usize)]) -> f64 {
    let mut back = vec![f64::NAN; pts.len()];
    back[root] = 0.0;
    for (u, v) in tree {
        back[*v] = back[*u] + manhattan(pts[*u], pts[*v]);
    }
    assert!(back.iter().all(|b| b.is_finite()), "every box reached: {tree:?}");
    back.iter().copied().fold(0.0, f64::max)
}

#[test]
fn a_branching_tree_keeps_the_farthest_box_nearer_than_the_chain_within_its_joints() {
    // Q6340: eight boxes round a 30' x 14' room (sheet inches), the homerun's box at a corner.
    let ft = [(2.0, 7.0), (10.0, 7.0), (18.0, 7.0), (26.0, 7.0), (28.0, 0.0), (26.0, -7.0), (18.0, -7.0), (10.0, -7.0)];
    let pts: Vec<Vec2> = ft.iter().map(|(x, y)| Vec2::new(x * UPF, y * UPF)).collect();
    // The chain: weight 0 with one joint at the root and two at each box is the spanning tree.
    assert_eq!(branch_tree(&pts, &[], 0, 1, 2, 0.0), spanning_tree(&pts, 0));
    let chain = spanning_tree(&pts, 0);
    let (rj, bj, w) = rules().route.branching();
    assert_eq!((rj, bj), (2, 3), "rules data");
    let br = branch_tree(&pts, &[], 0, rj, bj, w);
    assert_eq!(br.len(), pts.len() - 1);
    let mut joints = vec![0usize; pts.len()];
    for (a, b) in &br {
        joints[*a] += 1;
        joints[*b] += 1;
    }
    assert!(joints[0] <= rj && joints.iter().skip(1).all(|j| *j <= bj), "{joints:?}");
    let (c, b) = (farthest(&pts, 0, &chain) / UPF, farthest(&pts, 0, &br) / UPF);
    // Chained: 58 ft round the room in plan; branched both ways from the corner: 38 ft.
    assert!((c - 58.0).abs() < 1e-6, "the chain goes round the room: {c}");
    assert!((b - 38.0).abs() < 1e-6, "branched {b} vs chained {c}");
    assert!(branch_tree(&pts, &[], 9, rj, bj, w).is_empty());
    // Each box's drop counted: a deep branch costs its drops, so the tree is shallower still.
    let rises = vec![7.75 * UPF; pts.len()];
    let deep = |t: &[(usize, usize)]| {
        let mut depth = vec![0usize; pts.len()];
        for (a, b) in t {
            depth[*b] = depth[*a] + 1;
        }
        depth.into_iter().max().unwrap_or(0)
    };
    let low = branch_tree(&pts, &rises, 0, rj, bj, w);
    assert_eq!(low.len(), pts.len() - 1);
    assert!(deep(&low) <= deep(&br) && deep(&low) < deep(&chain), "{low:?} / {br:?}");
    let _ = branch_tree(&[Vec2::new(f64::NAN, 0.0), Vec2::new(0.0, 0.0)], &[f64::NAN, -1.0], 0, 2, 3, f64::NAN);
}

fn breaker_table() -> super::super::wiretable::LengthTable {
    use super::super::wiretable::{Band, LengthTable};
    let b = |size: &str, max_ft: f64| Band { amps: Some(20.0), volts: Some(120.0), size: size.into(), max_ft };
    LengthTable { sheet: "X-1".into(), title: "VOLTAGE DROP SCHEDULE".into(), bands: vec![b("12", 50.0), b("10", 100.0), b("8", 150.0)] }
}

#[test]
fn the_documents_length_table_sizes_by_breaker_and_voltage_else_the_drop_decides() {
    let rl = rules();
    let t = vec![breaker_table()];
    let (s, why) = table_size(&t, 80.0, Some(20.0), 120.0, 4.0, rl).expect("sized");
    assert_eq!(s, "10");
    assert!(why.contains("VOLTAGE DROP SCHEDULE on X-1") && why.contains("20 A / 120 V"), "{why}");
    // No breaker read: the usual 20 A, said.
    let (s, why) = table_size(&t, 40.0, None, 120.0, 4.0, rl).expect("sized");
    assert_eq!(s, "12");
    assert!(why.contains("breaker not read"), "{why}");
    // A 30 A circuit or a 277 V one: no row, the drop decides.
    assert!(table_size(&t, 40.0, Some(30.0), 120.0, 4.0, rl).is_none());
    assert!(table_size(&t, 40.0, Some(20.0), 277.0, 4.0, rl).is_none());
    // Past its reach: its largest size, or the drop's when that is larger.
    let (s, why) = table_size(&t, 300.0, Some(20.0), 120.0, 1.0, rl).expect("sized");
    assert_eq!(s, "8", "{why}");
    assert!(why.contains("engineer confirms"), "{why}");
    let (s, _) = table_size(&t, 300.0, Some(20.0), 120.0, 16.0, rl).expect("sized");
    assert!(rl.vdrop.sizes.iter().position(|x| *x == s) > rl.vdrop.sizes.iter().position(|x| x == "8"), "{s}");
    // How far a size reaches: the table's, else the drop's (3% at 16 A, 120 V on #12: 56.9 ft).
    let sup = size::supply(1, None, "P", rl);
    assert_eq!(reach_ft("10", &t, Some(20.0), &sup, 16.0, rl).map(|r| r.0), Some(100.0));
    let (r, by) = reach_ft("12", &[], Some(20.0), &sup, 16.0, rl).expect("reach");
    assert!((r - 56.9).abs() < 0.2 && by.contains("210.19"), "{r} {by}");
    assert!(reach_ft("12", &[], None, &sup, 0.0, rl).is_none());
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

/// A 3/4" raceway routed before, laid along `pts` with its centre `z` over the floor (sheet
/// inches, the floor at 0).
fn laid(field: &Field, pts: &[[f64; 2]], z: f64) {
    let run = super::printcond::run(BRANCH_SYSTEM, "L1", "", pts.to_vec(), 0.75, z / UPF);
    field.lay_run(&run, 0.0, UPF, "LP1-1");
}

/// The clearance between the outsides of two 3/4" raceways `dz` (vertically) or `dy` apart.
fn outside_gap(d: f64) -> f64 {
    d.abs() - 0.375 - 0.375
}

#[test]
fn a_route_across_a_raceway_laid_before_passes_over_it_a_tier_up() {
    let field = Field::new(Vec::new(), UPF);
    laid(&field, &[[-240.0, 60.0], [240.0, 60.0]], 111.0);
    let leg = route_leg(Vec2::new(0.0, 0.0), Vec2::new(0.0, 120.0), &tiers(), 0.375, &field, UPF, &[], rules());
    assert!(leg.crossings.is_empty() && leg.blocked.is_empty(), "{leg:?}");
    assert_eq!(leg.pts, vec![Vec2::new(0.0, 0.0), Vec2::new(0.0, 120.0)]);
    assert_eq!(leg.z, vec![115.0]);
    assert!(outside_gap(115.0 - 111.0) >= rules().clearance_in("conduit"));
    // Nothing laid: the lowest tier, as before.
    let free = Field::new(Vec::new(), UPF);
    assert_eq!(route_leg(Vec2::new(0.0, 0.0), Vec2::new(0.0, 120.0), &tiers(), 0.375, &free, UPF, &[], rules()).z, vec![111.0]);
}

#[test]
fn a_route_alongside_a_raceway_laid_before_with_no_tier_free_runs_in_the_lane_beside_it() {
    let field = Field::new(Vec::new(), UPF);
    // Between the leg's boxes, along its line (never over the boxes themselves: a rise there
    // would meet it).
    laid(&field, &[[24.0, 0.0], [216.0, 0.0]], 111.0);
    let leg = route_leg(Vec2::new(0.0, 0.0), Vec2::new(240.0, 0.0), &[111.0], 0.375, &field, UPF, &[], rules());
    assert!(leg.crossings.is_empty() && leg.blocked.is_empty(), "{leg:?}");
    assert!(square(&leg), "{leg:?}");
    // Side by side: the clearance between the outsides, a lane over, never a detour round it.
    let off = leg.pts.iter().map(|p| p.y).fold(0.0, |m: f64, y| if y.abs() > m.abs() { y } else { m });
    assert!(outside_gap(off) >= rules().clearance_in("conduit") && off.abs() <= 6.0, "{leg:?}");
    assert!(leg.pts.iter().filter(|p| (p.y - off).abs() < 1e-9).count() == 2, "{leg:?}");
}

#[test]
fn raceways_meeting_at_their_box_are_not_a_crossing() {
    let field = Field::new(Vec::new(), UPF);
    // One raceway ends at the box at the origin; the next leaves it the other way.
    laid(&field, &[[-120.0, 0.0], [0.0, 0.0]], 111.0);
    let leg = route_leg(Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), &tiers(), 0.375, &field, UPF, &[], rules());
    assert!(leg.crossings.is_empty(), "{leg:?}");
    assert_eq!(leg.z, vec![111.0]);
}

#[test]
fn a_crossing_no_tier_or_lane_clears_is_listed_for_the_field() {
    let field = Field::new(Vec::new(), UPF);
    laid(&field, &[[-2400.0, 60.0], [2400.0, 60.0]], 111.0);
    let leg = route_leg(Vec2::new(0.0, 0.0), Vec2::new(0.0, 120.0), &[111.0], 0.375, &field, UPF, &[], rules());
    assert!(leg.blocked.is_empty(), "{leg:?}");
    assert_eq!(leg.crossings.len(), 1, "{leg:?}");
    let x = leg.crossings.first().expect("crossing");
    assert_eq!(x.other, "LP1-1");
    assert!((x.at.y - 60.0).abs() < 1.0 && (x.at.z - 111.0).abs() < 1e-9, "{x:?}");
}

#[test]
fn hostile_laid_raceways_do_not_panic() {
    let field = Field::new(Vec::new(), UPF);
    laid(&field, &[[f64::NAN, 0.0], [1e300, -1e300]], f64::INFINITY);
    laid(&field, &[[-1e18, 0.0], [1e18, 0.0]], 111.0);
    laid(&field, &[], 111.0);
    field.lay_piece(Vec3::new(0.0, 0.0, f64::NAN), Vec3::new(1.0, 1.0, 1.0), f64::NAN, "");
    let _ = route_leg(Vec2::new(0.0, 0.0), Vec2::new(0.0, 120.0), &tiers(), 0.375, &field, UPF, &[], rules());
    let _ = field.crosses_laid(Vec2::new(f64::NAN, 0.0), Vec2::new(0.0, 1e300), 111.0, -1.0, &[], f64::NAN, "", UPF, rules());
    let _ = shifted(&[Vec2::new(0.0, 0.0)], 5, 1.0, 0.1);
    let _ = shifted(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0)], 0, f64::NAN, 0.1);
    // The shared empty field lays nothing.
    let empty = Field::default();
    laid(&empty, &[[-240.0, 60.0], [240.0, 60.0]], 111.0);
    assert!(empty.crosses_laid(Vec2::new(0.0, 0.0), Vec2::new(0.0, 120.0), 111.0, 0.375, &[], 4.0, "", UPF, rules()).is_none());
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

/// Q6314: a top floor's run across the roof trusses, a few feet in from their ends, goes up
/// between the chords and straight across: a pass through an open web bores nothing, so it is
/// no labor, and the run never goes round the trusses' ends (past the walls they bear on).
#[test]
fn a_run_across_trusses_by_their_ends_passes_their_webs_never_round_the_ends() {
    // Trusses along y (y = -240 to 240) every 24" over a 100' floor, 112" to 136".
    let members = (0..=50)
        .map(|k| {
            let x = 24.0 * f64::from(k);
            (
                Obs {
                    handle: Handle(0x56),
                    lo: Vec3::new(x - 0.75, -240.0, 112.0),
                    hi: Vec3::new(x + 0.75, 240.0, 136.0),
                    category: FRAMING.into(),
                    system: "roof truss".into(),
                    trade: "framing",
                },
                Frame { a: Vec2::new(x, -240.0), b: Vec2::new(x, 240.0), t: 1.5, truss: true },
            )
        })
        .collect();
    let field = Field::with_framing(Vec::new(), members, UPF);
    // 6" in from the trusses' end (y = 240), across 49 of them; the first tier meets the chords.
    let (a, b) = (Vec2::new(12.0, 234.0), Vec2::new(1188.0, 234.0));
    let leg = route_leg(a, b, &[114.0, 124.0], 0.375, &field, UPF, &[], rules());
    assert!(leg.blocked.is_empty(), "{leg:?}");
    assert!(leg.pts.iter().all(|p| p.y <= 240.0 - 1.25), "never past the trusses' ends: {leg:?}");
    assert_eq!(leg.z, vec![124.0], "between the chords: {leg:?}");
    assert_eq!(leg.holes, 0, "a pass through a truss's web is no bored hole");
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
    d.building_spec = Some(sp.to_json()).into();
    super::mep::build(d, &part);
    let whole = |s: &crate::Session| {
        s.doc().unwrap().model.iter().filter(|e| !super::mepatoms::is_atom(e) && super::bimlook::made_tag(e, "system") == Some("E-FEEDER")).count()
    };
    assert!(whole(&s) > 0, "the job's whole runs");
    let r = s.execute("conduit.reroute", &json!({})).unwrap();
    assert_eq!(r["routed"], json!(true), "{r}");
    let ug = |s: &crate::Session, atom: bool| {
        s.doc()
            .unwrap()
            .model
            .iter()
            .filter(|e| super::mepatoms::is_atom(e) == atom && super::bimlook::made_tag(e, "system") == Some("E-UG"))
            .filter(|e| super::bimlook::made_tag(e, "mepid").is_some_and(|m| m.starts_with("@200.00,-200.00")))
            .count()
    };
    // The underground plan's run is cut into its pieces with the rest (a takeoff counts it), its
    // whole run gone; again (the job reroutes once its devices are reseated) it is cut once.
    let once = ug(&s, true);
    assert!(once > 0, "the underground plan's run is in pieces");
    assert_eq!(ug(&s, false), 0, "its whole run gives way to them");
    s.execute("conduit.reroute", &json!({})).unwrap();
    assert_eq!(ug(&s, true), once, "cut once, not twice");
    assert_eq!(ug(&s, false), 0);
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
fn buried_work_built_whole_after_the_last_cut_is_cut_into_its_pieces_with_no_device_reseated() {
    let mut s = crate::Session::empty();
    s.open_drawing(cadaddy_doc::Drawing::new_imperial(), "Sample Buried Stub", None);
    // A made-up storey with a 20' conduit over the ceiling (the model is built with pieces).
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "mep": {
            "systems": [{"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 10}],
            "runs": [{"system": "E-COND", "level": "L1", "points": [[0, 600], [240, 600]], "size_in": [0.75, 0], "bottoms_ft": [10]}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    // The underground step after the job's last reroute: a called run built whole, 30' two feet
    // under the floor and stubbed up to a foot above it.
    let (mut sp, _) = super::bspec::current(&s, "test").unwrap();
    sp.mep
        .systems
        .push(serde_json::from_value(json!({"name": "E-UG", "layer": "E-UG", "kind": "conduit", "size_in": [1, 0], "bottom_ft": -2})).unwrap());
    let ug = |pts: Vec<[f64; 2]>, top: Option<f64>| -> MepRun {
        serde_json::from_value(json!({"system": "E-UG", "level": "L1", "points": pts, "size_in": [1.0, 0.0], "bottoms_ft": [-2.0], "top_ft": top,
            "source_sheet": "Plan 1", "height_source": "called: Plan 1 note (telecom service)"}))
        .unwrap()
    };
    let runs = vec![ug(vec![[0.0, 0.0], [360.0, 0.0]], None), ug(vec![[360.0, 0.0]], Some(1.0))];
    sp.mep.runs.extend(runs.iter().cloned());
    let mut part = sp.clone();
    part.mep.runs = runs;
    let d = s.doc_mut().unwrap();
    d.building_spec = Some(sp.to_json()).into();
    super::mep::build(d, &part);
    let ug_of = |s: &crate::Session, atom: bool| {
        s.doc().unwrap().model.iter().filter(|e| super::mepatoms::is_atom(e) == atom && super::bimlook::made_tag(e, "system") == Some("E-UG")).count()
    };
    assert!(ug_of(&s, false) > 0 && ug_of(&s, true) == 0, "built whole");
    let r = super::follow_reseated(&mut s, 0).expect("the buried work is cut").unwrap();
    assert_eq!(r["routed"], json!(true), "{r}");
    assert_eq!(ug_of(&s, false), 0, "no whole run left");
    let d = s.doc().unwrap();
    let ty: Vec<String> = d
        .model
        .iter()
        .map(|e| props::read(&e.common.xdata))
        .filter(|p| p.get("system").map(Prop::text).as_deref() == Some("E-UG") && p.get("atom").map(Prop::text).as_deref() == Some("conduit"))
        .map(|p| p.get("type").map(Prop::text).unwrap_or_default())
        .collect();
    assert!(!ty.is_empty() && ty.iter().all(|t| t == "PVC"), "{ty:?}");
    // Nothing left to cut and no device moved: no reroute.
    assert!(super::follow_reseated(&mut s, 0).is_none());
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
    let l = |home: f64, in_area: f64, wire: f64| Lens { in_area, home, wire, cable: home + in_area, ..Lens::default() };
    let sz = |v: f64| BTreeMap::from([("12/2 w/G".to_string(), v)]);
    let rows = vec![
        ("L1".to_string(), "11".to_string(), l(10.0, 30.0, 150.0), sz(40.0)),
        ("L1".to_string(), "11".to_string(), l(6.0, 20.0, 90.0), sz(26.0)),
        ("L1".to_string(), "12".to_string(), l(12.0, 40.0, 170.0), sz(52.0)),
        ("L1".to_string(), "LOBBY".to_string(), l(50.0, 70.0, 400.0), sz(120.0)),
    ];
    let r = lengths_report(&sp, &rows);
    let ty = &r["unit_types"][0];
    assert_eq!((ty["unit_type"].as_str(), ty["name"].as_str(), ty["rooms"].as_u64()), (Some("SUITE-A"), Some("Suite A"), Some(2)), "{r}");
    let pr = &ty["per_room"];
    for (k, v) in [("circuits", 1.5), ("home_ft", 14.0), ("in_area_ft", 45.0), ("total_ft", 59.0), ("conductor_ft", 205.0), ("cable_ft", 59.0)] {
        assert_eq!(pr[k], json!(v), "{k}: {r}");
    }
    // Each circuit's own cable by size (Q6206): per room, per level, in all.
    assert_eq!(pr["cable_ft_by_size"], json!({"12/2 w/G": 59.0}), "{r}");
    assert_eq!(r["levels"][0]["cable_ft_by_size"], json!({"12/2 w/G": 238.0}), "{r}");
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

#[test]
fn an_overfilled_box_is_built_with_the_next_box_and_keeps_its_design_box() {
    // A 4" square (30.3 cu in) a device's twelve #12 enter, with its ground and a yoke: 33.75 cu
    // in (314.16(B)). The field sets a 4-11/16" square (42 cu in); the design box stays beside it.
    let mut d = Drawing::default();
    let mut ps = props::Props::default();
    let rule = || props::Source::from_doc(props::SourceKind::Rule, "box rule (made-up)");
    ps.set(crate::cmd::mepatoms::ATOM_KEY, Prop::new("box", props::Source::computed()));
    ps.set("box_of", Prop::new("1A2", props::Source::computed()));
    ps.set("size", Prop::new("4\" square, 2-1/8\" deep", rule()));
    ps.set("cubic_in", Prop::new("30.3", rule()));
    let tags = crate::cmd::mepatoms::build::mep_tags("L1", Handle(0x1A2), BRANCH_SYSTEM, "box", "4\" square, 2-1/8\" deep", "box", "x/box0");
    let xdata = props::write(&tags, &ps);
    let h = d
        .add(
            &cadaddy_doc::Space::Model,
            cadaddy_doc::Common { xdata: xdata.into(), ..cadaddy_doc::Common::default() },
            EntityKind::Mesh(crate::cmd::wallgen::empty_mesh()),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let pending = json!({"box_fill": {"pending": [{"device": "1A2", "kind": "receptacle", "level": "L1", "circuit": "LP9-1", "yoke": true, "conductors": [[12, "12"]]}]}});
    for _ in 0..2 {
        let mut report = pending.clone();
        check_box_fill(&mut d, &mut report);
        let bf = &report["box_fill"];
        assert_eq!((bf["checked"].as_u64(), bf["corrected"].as_u64()), (Some(1), Some(1)), "{bf}");
        assert_eq!(bf["over"][0]["corrected"], json!(true), "{bf}");
        let e = d.model.get(h).unwrap_or_else(|| panic!("the box"));
        let got = props::read(&e.common.xdata);
        let t = |k: &str| got.get(k).map(Prop::text).unwrap_or_default();
        assert_eq!(t("size"), "4-11/16\" square, 2-1/8\" deep", "installed");
        assert_eq!(t("cubic_in"), "42");
        // The design value kept, never overwritten by a second check.
        assert_eq!(t("design:size"), "4\" square, 2-1/8\" deep");
        assert_eq!(t("design:cubic_in"), "30.3");
        assert!(t("correction:size").starts_with("E-BOX-FILL"), "{}", t("correction:size"));
    }
}

#[test]
fn a_box_past_the_largest_gets_an_extension_ring_and_keeps_its_design_box() {
    // A 4-11/16" square (42 cu in, the largest box of the rules) twenty #12 enter, with its
    // ground and a yoke: 51.75 cu in. The field adds an extension ring, whose marked volume
    // 314.16(A) adds; the design box stays beside it with the reason.
    let mut d = Drawing::default();
    let mut ps = props::Props::default();
    let rule = || props::Source::from_doc(props::SourceKind::Rule, "box rule (made-up)");
    ps.set(crate::cmd::mepatoms::ATOM_KEY, Prop::new("box", props::Source::computed()));
    ps.set("box_of", Prop::new("1B3", props::Source::computed()));
    ps.set("size", Prop::new("4-11/16\" square, 2-1/8\" deep", rule()));
    ps.set("cubic_in", Prop::new("42", rule()));
    let tags = crate::cmd::mepatoms::build::mep_tags("L1", Handle(0x1B3), BRANCH_SYSTEM, "box", "4-11/16\" square, 2-1/8\" deep", "box", "x/box1");
    let xdata = props::write(&tags, &ps);
    let h = d
        .add(
            &cadaddy_doc::Space::Model,
            cadaddy_doc::Common { xdata: xdata.into(), ..cadaddy_doc::Common::default() },
            EntityKind::Mesh(crate::cmd::wallgen::empty_mesh()),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let mut report = json!({"box_fill": {"pending": [{"device": "1B3", "kind": "receptacle", "level": "L1", "circuit": "LP9-2", "yoke": true, "conductors": [[20, "12"]]}]}});
    check_box_fill(&mut d, &mut report);
    assert_eq!(report["box_fill"]["corrected"].as_u64(), Some(1), "{}", report["box_fill"]);
    let e = d.model.get(h).unwrap_or_else(|| panic!("the box"));
    let got = props::read(&e.common.xdata);
    let t = |k: &str| got.get(k).map(Prop::text).unwrap_or_default();
    assert!(t("size").contains("extension ring"), "{}", t("size"));
    assert_eq!(t("cubic_in"), "63");
    assert_eq!(t("design:size"), "4-11/16\" square, 2-1/8\" deep");
    assert_eq!(t("design:cubic_in"), "42");
    assert!(t("correction:size").contains("314.16(A)"), "the ring's basis: {}", t("correction:size"));
}

#[test]
fn a_raceway_through_an_obstacle_is_offset_under_it_across_its_box_only() {
    let mut r: MepRun = serde_json::from_value(json!({
        "system": BRANCH_SYSTEM, "level": "L1", "points": [[0.0, 0.0], [240.0, 0.0], [240.0, 120.0]],
        "size_in": [0.75, 0.0], "bottoms_ft": [9.5, 9.5], "height_source": ROUTED,
    }))
    .unwrap_or_else(|e| panic!("{e}"));
    // A pipe across the first leg from x 100 to 140 (with its clearance), the clash at x 120.
    let got = offset_run(&mut r, Vec2::new(120.0, 0.0), 9.5, [100.0, -10.0, 140.0, 10.0], 8.5, UPF);
    let (a, b) = got.unwrap_or_else(|| panic!("offset"));
    assert_eq!((a.x, b.x), (100.0, 140.0));
    assert_eq!(r.points, vec![[0.0, 0.0], [100.0, 0.0], [140.0, 0.0], [240.0, 0.0], [240.0, 120.0]]);
    assert_eq!(r.bottoms_ft, vec![9.5, 8.5, 9.5, 9.5], "the stretch across the box drops, the rest stays");
    // No segment there at that height: nothing changes.
    let before = r.clone();
    assert!(offset_run(&mut r, Vec2::new(120.0, 60.0), 9.5, [100.0, 50.0, 140.0, 70.0], 8.5, UPF).is_none());
    assert!(offset_run(&mut r, Vec2::new(240.0, 60.0), 7.0, [230.0, 50.0, 250.0, 70.0], 6.0, UPF).is_none(), "another height");
    assert_eq!(r, before);
}

#[test]
fn a_raceway_the_ceiling_space_cant_offset_runs_around_the_obstacle_in_plan() {
    let mut r: MepRun = serde_json::from_value(json!({
        "system": BRANCH_SYSTEM, "level": "L1", "points": [[0.0, 0.0], [240.0, 0.0]],
        "size_in": [0.75, 0.0], "bottoms_ft": [9.5], "height_source": ROUTED,
    }))
    .unwrap_or_else(|e| panic!("{e}"));
    // A duct 40" across the run, its box reaching 6" one side and 30" the other: round the near side.
    let (path, extra) = detour_run(&mut r, Vec2::new(120.0, 0.0), 9.5, [100.0, -6.0, 140.0, 30.0], UPF).unwrap_or_else(|| panic!("a detour"));
    assert_eq!(r.points, vec![[0.0, 0.0], [100.0, 0.0], [100.0, -6.0], [140.0, -6.0], [140.0, 0.0], [240.0, 0.0]]);
    assert_eq!(r.bottoms_ft, vec![9.5; 5], "at the same height");
    assert_eq!(path.len(), 4);
    assert!((extra - 1.0).abs() < 1e-9, "6\" out and back: {extra}");
    // A box past the segment's end, or a diagonal piece: no detour.
    let mut d: MepRun = serde_json::from_value(
        json!({"system": BRANCH_SYSTEM, "level": "L1", "points": [[0.0, 0.0], [100.0, 100.0]], "size_in": [0.75, 0.0], "bottoms_ft": [9.5]}),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(detour_run(&mut d, Vec2::new(50.0, 50.0), 9.5, [40.0, 40.0, 60.0, 60.0], UPF).is_none());
}

fn hole(member: usize, along_in: f64, raceway_in: f64, end_ft: f64) -> Bored {
    Bored {
        level: "L1".into(),
        circuit: format!("LP9-{member}"),
        obstacle: member,
        member: "I-joist".into(),
        host: Handle(0x90 + member as u64),
        at: Vec2::new(along_in, 0.0),
        z: 120.0,
        along: along_in,
        dia_in: raceway_in + 0.375,
        raceway_in,
        end_ft,
        design: String::new(),
    }
}

#[test]
fn two_holes_too_close_share_one_hole_where_the_hole_rule_takes_it() {
    let fr = &rules().framing;
    // Two 3/4" raceways 1" apart in one joist, 4 ft from its bearing: one 1-7/8" hole for both
    // (the rule takes it 1 ft from the bearing), the design's two holes kept beside it.
    let mut bored = vec![hole(1, 24.0, 0.75, 4.0), hole(1, 26.0, 0.75, 4.0), hole(2, 24.0, 0.75, 4.0)];
    let pairs = close_holes(&mut bored, fr, UPF);
    assert_eq!(pairs.len(), 1, "only the pair in one member");
    let c = pairs[0].corrected.as_ref().unwrap_or_else(|| panic!("shared: {:?}", pairs[0]));
    assert_eq!(c["rule"], json!("E-BORE-SHARE"));
    assert!(c["design"].as_str().unwrap_or_default().starts_with("two holes"), "{c}");
    assert_eq!(bored.len(), 2, "the second hole is gone");
    assert!((bored[0].dia_in - 1.875).abs() < 1e-9, "{:?}", bored[0]);
    assert!(bored[0].design.starts_with("two holes"), "the design kept");
    assert!((bored[1].dia_in - 1.125).abs() < 1e-9, "the other member's hole stays");
    // Two 2" raceways close to the bearing: the 4-3/8" hole they'd share must stand 4 ft off it;
    // they stay a clash item, both holes as designed.
    let mut near = vec![hole(3, 24.0, 2.0, 1.5), hole(3, 27.0, 2.0, 1.5)];
    let pairs = close_holes(&mut near, fr, UPF);
    assert_eq!(pairs.len(), 1);
    assert!(pairs[0].corrected.is_none());
    assert_eq!(near.len(), 2);
    assert!(near.iter().all(|h| h.design.is_empty() && (h.dia_in - 2.375).abs() < 1e-9));
}

#[test]
fn an_estimate_for_another_tag_of_a_routed_circuit_goes_and_a_rooms_own_stays() {
    let est = super::super::printcond::CIRCUIT_SYSTEM;
    let run = |id: &str, level: &str| {
        let mut r = super::super::printcond::run(est, level, "E-601", vec![[0.0, 0.0], [120.0, 0.0]], 0.75, 9.0);
        r.id = format!("{id}#1");
        r
    };
    let mut sp = BuildingSpec {
        rooms: ["1101", "1102"]
            .iter()
            .map(|n| cadaddy_walk3d::SpecRoom { name: "GUEST".into(), number: Some((*n).into()), ..Default::default() })
            .collect(),
        ..Default::default()
    };
    sp.mep.runs = vec![
        // The enlarged plan's tag of a circuit the plan's tag routed.
        run("circuit:L1:E-601:LP1-7", "L1"),
        // A typical room's view (no room of the spec) of a circuit each room routed.
        run("circuit:L1:room-SUITE TYPE 2:U-3", "L1"),
        // A real room whose own circuit was not routed: its only route.
        run("circuit:L1:room-1102:U-3", "L1"),
        // A circuit not routed, and the same label on another level.
        run("circuit:L1:E-601:LP1-9", "L1"),
        run("circuit:L2:E-601:LP1-7", "L2"),
    ];
    let tag = |id: &str| ElementTag { id: id.into(), kind: "circuit".into(), level: "L1".into(), ..Default::default() };
    let routed = Routed {
        runs: Vec::new(),
        tags: vec![tag("circuit:L1:E-101:LP1-7"), tag("circuit:L1:room-1101:U-3"), tag("circuit:L1:room-1101:U-3~sw3"), tag("circuit:L1:home:LP1:0")],
        report: Value::Null,
        feeders: BTreeSet::new(),
        cable: Vec::new(),
        minimum: None,
        no_device: BTreeSet::new(),
        keynote_taken: BTreeSet::new(),
    };
    let gone = estimates_routed_elsewhere(&sp, &routed);
    assert_eq!(gone.iter().map(String::as_str).collect::<Vec<_>>(), ["circuit:L1:E-601:LP1-7", "circuit:L1:room-SUITE TYPE 2:U-3"], "{gone:?}");
    assert_eq!(circuit_key("circuit:Level 1:E-101:LP1-3@P2~sw3"), Some(("level 1".into(), "E-101".into(), "LP1-3".into())));
    assert_eq!(circuit_key("device:L1:3"), None);
    assert_eq!(circuit_key("circuit:L1:E-101:"), None);
}

/// Q6489: a circuit the routing found no device of keeps no estimated route (even when nothing
/// else was routed), its tag says it is not routed, and what went is kept for the next routing.
#[test]
fn a_circuit_with_no_device_built_keeps_no_estimated_route() {
    let est = |id: &str| {
        let mut r = crate::cmd::printcond::run(crate::cmd::printcond::CIRCUIT_SYSTEM, "Roof", "E-9", vec![[0.0, 0.0], [816.0, 0.0]], 0.75, 8.25);
        r.id = format!("{id}#1");
        r
    };
    let tag = |id: &str| ElementTag { id: id.into(), kind: "circuit".into(), level: "Roof".into(), ..Default::default() };
    let mut sp = BuildingSpec::default();
    sp.mep.runs = vec![est("circuit:Roof:E-9:PA-3"), est("circuit:Roof:E-9:PA-7")];
    sp.tags = vec![tag("circuit:Roof:E-9:PA-3"), tag("circuit:Roof:E-9:PA-7")];
    let routed = Routed { no_device: BTreeSet::from(["circuit:Roof:E-9:PA-3".to_string()]), ..Routed::default() };
    let replaced = apply(&mut sp, &routed);
    let ids: Vec<&str> = sp.mep.runs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["circuit:Roof:E-9:PA-7#1"], "the other circuit's estimate stays");
    let t = sp.tags.iter().find(|t| t.id == "circuit:Roof:E-9:PA-3").expect("tag");
    assert_eq!(t.fields.get("route").map(|f| f.value.as_str()), Some("not routed"));
    assert!(t.fields.get(NOT_ROUTED).is_some_and(|f| f.value.starts_with(NO_DEVICE_REASON)), "{t:?}");
    assert_eq!(t.issues, ["no_device_built"]);
    assert_eq!(replaced.runs.len(), 1);
    assert_eq!(replaced.tags.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["circuit:Roof:E-9:PA-3"]);
    assert!(replaced.tags.iter().all(|t| t.issues.is_empty()), "the tag as read is kept");
    // Applied again (a routed spec routed again): marked once.
    apply(&mut sp, &routed);
    assert_eq!(sp.tags.iter().filter(|t| t.issues.iter().any(|i| i == "no_device_built")).count(), 1);
}

#[test]
fn a_homeruns_numbers_are_each_circuits_once_in_order() {
    // Two rooms' circuits of one label and a two-pole circuit: one readable list.
    let labels: Vec<String> = ["RP-2", "RP-2", "RP-7,9"].iter().map(|s| s.to_string()).collect();
    assert_eq!(homerun_numbers(&labels), vec!["2", "7", "9"]);
    let labels: Vec<String> = ["LP1-1", "LP1-3,5", "LP1-7"].iter().map(|s| s.to_string()).collect();
    assert_eq!(homerun_numbers(&labels), vec!["1", "3", "5", "7"]);
    assert!(homerun_numbers(&["not a circuit".to_string()]).is_empty());
}

/// Q6247: a receptacle its keynote puts under its unit: every way over its box is blocked by the
/// unit, so its raceway leaves the box's side under the unit and rises beside it; another box
/// there (not the unit's) stays blocked.
#[test]
fn a_units_receptacle_runs_out_under_the_unit_and_rises_beside_it() {
    // A unit 42" long, 4" to 20" over the floor, its receptacle's box under its middle (top 4").
    let unit = Obs {
        handle: Handle(0x55),
        lo: Vec3::new(-21.0, 0.0, 4.0),
        hi: Vec3::new(21.0, 12.0, 20.0),
        category: "equipment".into(),
        system: "M-DUCT".into(),
        trade: "mechanical",
    };
    let field = Field::new(vec![unit], UPF);
    let (at, face, z0) = (Vec2::new(0.0, 1.0), Some(Vec2::new(0.0, 1.0)), 111.0);
    let (_, _, blocked) = rise_point(at, 4.0, face, z0, 0.375, &field, UPF, &[], rules());
    assert_eq!(blocked, Some(0), "the unit is over the box and along its wall");
    let (q, z, blocked) = rise_point_under(at, 4.0, face, z0, 0.375, &field, UPF, &[], &[Handle(0x55)], "", rules());
    assert_eq!(blocked, None);
    assert!(q.x.abs() > 21.0 + 3.0 && (q.y - 1.0).abs() < 1e-9, "beside the unit, along its wall: {q:?}");
    assert!((z - 2.5).abs() < 1e-9, "out of the box's side, 1-1/2 in under its top: {z}");
    let (_, _, blocked) = rise_point_under(at, 4.0, face, z0, 0.375, &field, UPF, &[], &[], "", rules());
    assert_eq!(blocked, Some(0));
}

/// The feeder riser job of [`a_feeder_to_an_upper_floor_is_taken_off_buried_rising_and_across_level_by_level`]:
/// three storeys, a switchboard on the ground floor, a panel on it and one two floors up.
fn feeder_riser_spec() -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "Ground", "elevation_ft": 0, "height_ft": 11, "ceiling_ft": 9, "region": [0, 0, 1200, 1200]},
            {"name": "Middle", "elevation_ft": 11, "height_ft": 10, "ceiling_ft": 8, "region": [2000, 0, 3200, 1200]},
            {"name": "Upper", "elevation_ft": 21, "height_ft": 10, "ceiling_ft": 8, "region": [4000, 0, 5200, 1200]}
        ],
        "mep": {
            "systems": [
                {"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [3, 0], "bottom_ft": 10},
                {"name": "E-UG", "layer": "E-UG", "kind": "conduit", "size_in": [3, 0], "bottom_ft": -1.5}
            ],
            "equipment": [
                {"tag": "SB1", "kind": "switchboard", "level": "Ground", "min": [0, 0], "max": [48, 24], "height_ft": 7, "bottom_ft": 0},
                {"tag": "PU", "kind": "panel", "level": "Upper", "min": [4600, 600], "max": [4624, 612], "height_ft": 4, "bottom_ft": 2},
                {"tag": "PG", "kind": "panel", "level": "Ground", "min": [480, 600], "max": [504, 612], "height_ft": 3, "bottom_ft": 4}
            ],
            "feeders": [{"tag": "F7", "from": "SB1", "to": "PU", "sets": 1, "phases": 3, "size": "3/0", "material": "cu", "neutrals": 1,
                         "neutral_size": "3/0", "grounds": 1, "ground_size": "#6", "conduits": 1, "conduit_in": 3, "entry": "top", "from_entry": "bottom",
                         "source": "riser diagram"},
                        {"tag": "F8", "from": "SB1", "to": "PG", "sets": 1, "phases": 3, "size": "3/0", "material": "cu", "neutrals": 1,
                         "neutral_size": "3/0", "grounds": 1, "ground_size": "#6", "conduits": 1, "conduit_in": 3, "entry": "top", "from_entry": "bottom",
                         "source": "riser diagram"}]
        }
    })
}

/// Q6347: a feeder from the ground floor's switchboard to a panel two floors up, routed and cut
/// into pieces: its takeoff has the buried raceway only under the slab (and its stub-up), the
/// above-grade raceway rising on every level it passes and running on the panel's level, one
/// coupling where the riser passes each floor, a firestop at each floor, and its conductors
/// along the whole route on each level.
#[test]
fn a_feeder_to_an_upper_floor_is_taken_off_buried_rising_and_across_level_by_level() {
    let mut s = crate::Session::empty();
    s.open_drawing(cadaddy_doc::Drawing::new_imperial(), "Sample Feeder Riser", None);
    let spec = feeder_riser_spec();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    let (mut sp, _) = super::bspec::current(&s, "test").unwrap();
    let (runs, left) = super::gearbuild::feeder_runs(&sp);
    assert!(left.is_empty(), "{left:?}");
    sp.mep.runs = runs;
    s.execute("bspec.set", &json!({"replace": true, "spec": serde_json::to_value(&sp).unwrap()})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("conduit.reroute", &json!({})).unwrap();
    assert_eq!(r["routed"], json!(true), "{r}");
    let t = s.execute("conduit.takeoff", &json!({"price": false})).unwrap();
    let lines = t["lines"].as_array().cloned().unwrap_or_default();
    let qty = |level: &str, kind: &str, ty: &str| {
        lines
            .iter()
            .filter(|l| l["level"] == json!(level) && l["kind"] == json!(kind) && l["type"].as_str().unwrap_or_default().starts_with(ty))
            .map(|l| l["qty"].as_f64().unwrap_or(0.0))
            .sum::<f64>()
    };
    let stub = super::ugplan::rules().stubup.conduit_top_ft;
    // Buried: only on the ground floor, the trench and the two vertical legs up to the floor and the stub's top.
    assert_eq!(qty("Middle", "conduit", "PVC") + qty("Upper", "conduit", "PVC"), 0.0, "{lines:#?}");
    assert!(qty("Ground", "conduit", "PVC") > 0.0, "{lines:#?}");
    // Above the slab: the riser on the ground floor from the stub's top (and, up beside the
    // ground floor's panel PG, from its stub's top into PG's top at 7'), the middle floor's full
    // storey, and on the panel's level its rise, the run across and the drop into its top.
    assert!((qty("Ground", "conduit", "EMT") - (11.0 - stub) - (7.0 - stub)).abs() < 0.05, "{lines:#?}");
    assert!((qty("Middle", "conduit", "EMT") - 10.0).abs() < 0.05, "{lines:#?}");
    assert!(qty("Upper", "conduit", "EMT") >= 9.0 + 1.5 + 3.0 - 0.05, "{lines:#?}");
    // One coupling where the riser passes each floor, made as a floor joint; a transition out of the stub.
    let d = s.doc().unwrap();
    let joints = d
        .model
        .iter()
        .map(|e| props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(Prop::text).as_deref() == Some("coupling") && p.get("joint").map(Prop::text).as_deref() == Some("floor"))
        .count();
    assert_eq!(joints, 2, "{lines:#?}");
    // A transition where each stub-up meets the raceway above, also at the panel it rises into
    // (its stub is no landing in the panel: no adapter or connector there of its own).
    let transitions: f64 =
        lines.iter().filter(|l| l["item"].as_str().unwrap_or_default().contains("transition")).map(|l| l["qty"].as_f64().unwrap_or(0.0)).sum();
    assert_eq!(transitions, 2.0, "{lines:#?}");
    // Ends into gear: out of SB1's bottom twice (PVC), into PU's and PG's tops (EMT).
    assert_eq!((qty("Ground", "connector", "PVC"), qty("Ground", "connector", "EMT") + qty("Upper", "connector", "EMT")), (2.0, 2.0), "{lines:#?}");
    // A firestop at each floor it passes.
    let fs = |level: &str| {
        lines.iter().filter(|l| l["kind"] == json!("firestop") && l["level"] == json!(level)).map(|l| l["qty"].as_f64().unwrap_or(0.0)).sum::<f64>()
    };
    assert_eq!((fs("Ground"), fs("Middle"), fs("Upper")), (0.0, 1.0, 1.0), "{lines:#?}");
    // Its conductors ride the whole route: on the middle floor, the storey's riser.
    let wire = |level: &str| {
        lines
            .iter()
            .filter(|l| l["kind"] == json!("wire") && l["level"] == json!(level) && l["size"].as_str().unwrap_or_default().contains("3/0"))
            .map(|l| l["qty"].as_f64().unwrap_or(0.0))
            .sum::<f64>()
    };
    assert!(wire("Middle") >= 4.0 * 10.0 - 0.1, "three hots and a neutral up the storey: {lines:#?}");
}

/// Q6453: a mezzanine over another corner of the building, its floor inside the riser's span,
/// takes no piece of the feeder's riser: the middle floor's piece is its whole storey, no
/// coupling or firestop at the mezzanine's floor, none of the raceway counted on it.
#[test]
fn a_mezzanine_away_from_a_feeder_riser_takes_no_piece_coupling_or_firestop_of_it() {
    let mut s = crate::Session::empty();
    s.open_drawing(cadaddy_doc::Drawing::new_imperial(), "Sample Feeder Riser", None);
    let mut spec = feeder_riser_spec();
    if let Some(levels) = spec.pointer_mut("/levels").and_then(Value::as_array_mut) {
        levels.push(json!({"name": "Mezz", "elevation_ft": 16, "height_ft": 5, "ceiling_ft": 4, "region": [6000, 0, 6300, 300]}));
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    let (mut sp, _) = super::bspec::current(&s, "test").unwrap();
    let (runs, left) = super::gearbuild::feeder_runs(&sp);
    assert!(left.is_empty(), "{left:?}");
    assert!(!runs.iter().any(|r| r.level == "Mezz"), "{runs:#?}");
    sp.mep.runs = runs;
    s.execute("bspec.set", &json!({"replace": true, "spec": serde_json::to_value(&sp).unwrap()})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("conduit.reroute", &json!({})).unwrap();
    assert_eq!(r["routed"], json!(true), "{r}");
    let t = s.execute("conduit.takeoff", &json!({"price": false})).unwrap();
    let lines = t["lines"].as_array().cloned().unwrap_or_default();
    let qty = |level: &str, kind: &str| {
        lines.iter().filter(|l| l["level"] == json!(level) && l["kind"] == json!(kind)).map(|l| l["qty"].as_f64().unwrap_or(0.0)).sum::<f64>()
    };
    assert_eq!((qty("Mezz", "conduit"), qty("Mezz", "firestop")), (0.0, 0.0), "{lines:#?}");
    assert!((qty("Middle", "conduit") - 10.0).abs() < 0.05, "the middle floor's whole storey: {lines:#?}");
    assert_eq!((qty("Middle", "firestop"), qty("Upper", "firestop")), (1.0, 1.0), "{lines:#?}");
    let d = s.doc().unwrap();
    let joints = d
        .model
        .iter()
        .map(|e| props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(Prop::text).as_deref() == Some("coupling") && p.get("joint").map(Prop::text).as_deref() == Some("floor"))
        .count();
    assert_eq!(joints, 2, "{lines:#?}");
}

/// Q6314: a top floor (floor 0, top 96") under 24" roof trusses bearing on its walls (96" to
/// 120") routes up through their webs, as it does through the floor above's joists in the
/// storey's height; framing far above (another storey's) is not over it.
#[test]
fn a_top_floors_tiers_go_up_into_the_roof_trusses_bearing_on_its_walls() {
    let keep = 2.125;
    // The trusses wholly above the storey's top: their webs are routing space.
    assert_eq!(framing_tier_top(0.0, 96.0, 96.0, 120.0, keep, UPF), Some(120.0 - keep));
    // Joists within the storey's height, as before.
    assert_eq!(framing_tier_top(0.0, 120.0, 108.0, 120.0, keep, UPF), Some(120.0 - keep));
    // Framing a storey up (bearing 10' over this storey's top) is not this level's.
    assert_eq!(framing_tier_top(0.0, 96.0, 216.0, 240.0, keep, UPF), None);
    assert_eq!(framing_tier_top(0.0, 96.0, f64::NAN, f64::NAN, keep, UPF), None);
}

// ─── Q6557: box fill counted from the built pieces ───────────────────────────

/// A box mesh from `lo` to `hi` (inches).
fn cube(lo: [f64; 3], hi: [f64; 3]) -> cadaddy_doc::Mesh {
    let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let vertices = vec![
        v(lo[0], lo[1], lo[2]),
        v(hi[0], lo[1], lo[2]),
        v(hi[0], hi[1], lo[2]),
        v(lo[0], hi[1], lo[2]),
        v(lo[0], lo[1], hi[2]),
        v(hi[0], lo[1], hi[2]),
        v(hi[0], hi[1], hi[2]),
        v(lo[0], hi[1], hi[2]),
    ];
    let f = |a: u32, b: u32, c: u32, d: u32| cadaddy_doc::MeshFace { v: vec![a, b, c, d], hidden: 0 };
    cadaddy_doc::Mesh {
        vertices,
        faces: vec![f(0, 3, 2, 1), f(4, 5, 6, 7), f(0, 1, 5, 4), f(1, 2, 6, 5), f(2, 3, 7, 6), f(3, 0, 4, 7)],
        ..Default::default()
    }
}

/// A branch piece in the model: its atom kind, its properties and its body.
fn branch_piece(d: &mut Drawing, kind: &str, size: &str, kv: &[(&str, String)], mesh: cadaddy_doc::Mesh) -> Handle {
    let mut ps = props::Props::default();
    ps.set(crate::cmd::mepatoms::ATOM_KEY, Prop::new(kind, props::Source::computed()));
    ps.set("size", Prop::new(size, props::Source::computed()));
    for (k, v) in kv {
        ps.set(k, Prop::new(v.as_str(), props::Source::computed()));
    }
    let id = format!("x/{kind}{}", d.model.handles().len());
    let tags = crate::cmd::mepatoms::build::mep_tags("L1", Handle(0x500), BRANCH_SYSTEM, kind, size, kind, &id);
    let xdata = props::write(&tags, &ps);
    d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common { xdata: xdata.into(), ..cadaddy_doc::Common::default() }, EntityKind::Mesh(mesh))
        .unwrap_or_else(|e| panic!("{e}"))
}

/// A device's box at (x, y) inches, `z0`..`z0 + 4` high, `side` inches square.
fn device_box(d: &mut Drawing, of: &str, size: &str, cu: f64, x: f64, y: f64, z0: f64, side: f64) -> Handle {
    let h = side / 2.0;
    branch_piece(d, "box", size, &[("box_of", of.into()), ("cubic_in", format!("{cu}"))], cube([x - h, y - h, z0], [x + h, y + h, z0 + 2.125]))
}

/// A run of one stick from `a` to `b` (inches) in a raceway of `size`, with `hots` conductors of
/// `wire` and a ground.
fn wired_run(d: &mut Drawing, run: &str, circuit: &str, size: &str, a: [f64; 3], b: [f64; 3], hots: usize, wire: &str) {
    let at = |p: [f64; 3]| format!("{},{},{}", p[0], p[1], p[2]);
    let kv = |extra: Vec<(&'static str, String)>| -> Vec<(&'static str, String)> {
        let mut v = vec![("run", run.to_string()), ("circuit", circuit.to_string())];
        v.extend(extra);
        v
    };
    branch_piece(d, "conduit", size, &kv(vec![("start", at(a)), ("end", at(b)), ("type", "EMT".into())]), crate::cmd::wallgen::empty_mesh());
    for i in 0..hots {
        let role = if i == 1 { "neutral" } else { "phase A" };
        branch_piece(d, "wire", &format!("#{wire}"), &kv(vec![("conductor", role.into())]), crate::cmd::wallgen::empty_mesh());
    }
    branch_piece(d, "wire", &format!("#{wire}"), &kv(vec![("conductor", "ground".into())]), crate::cmd::wallgen::empty_mesh());
}

fn text_of(d: &Drawing, h: Handle, k: &str) -> String {
    d.model.get(h).map(|e| props::read(&e.common.xdata)).and_then(|p| p.get(k).map(Prop::text)).unwrap_or_default()
}

#[test]
fn a_box_raceways_drop_through_holds_their_conductors_and_is_built_bigger() {
    // Two receptacles stacked in a wall at (240, 120): the lower one's homerun (6#12) and two
    // more of its runs drop from the ceiling and land in the upper box first, with the upper
    // device's own run: 12#12 + the grounds + a yoke = 33.75 cu in in a 4" square (30.3). The
    // routing booked only the upper device's own 2#12 to its box; the pieces hold all twelve.
    let mut d = Drawing::default();
    let upper = device_box(&mut d, "1A2", "4\" square", 30.3, 240.0, 120.0, 60.0, 4.0);
    let lower = device_box(&mut d, "1A3", "4\" square, 2-1/8\" deep", 54.0, 240.0, 120.0, 40.0, 4.0);
    let top = [240.0, 120.0, 62.125];
    wired_run(&mut d, "home#1", "LP9-1,3,5", "1/2\"", [240.0, 120.0, 108.0], top, 6, "12");
    wired_run(&mut d, "LP9-1#1", "LP9-1", "1/2\"", top, [300.0, 120.0, 108.0], 2, "12");
    wired_run(&mut d, "LP9-5#tap", "LP9-5", "1/2\"", [200.0, 120.0, 108.0], top, 2, "12");
    // The upper device's own run, down to the lower box.
    wired_run(&mut d, "LP9-3#tap", "LP9-3", "1/2\"", [240.0, 120.0, 60.0], [240.0, 120.0, 42.125], 2, "12");
    let pending = json!({"box_fill": {"pending": [
        {"device": "1A2", "kind": "receptacle", "level": "L1", "circuit": "LP9-3", "yoke": true, "conductors": [[2, "12"]]},
        {"device": "1A3", "kind": "receptacle", "level": "L1", "circuit": "LP9-1", "yoke": true, "conductors": [[6, "12"], [2, "12"], [2, "12"], [2, "12"]]}
    ]}});
    for _ in 0..2 {
        let mut report = pending.clone();
        check_box_fill(&mut d, &mut report);
        let bf = &report["box_fill"];
        assert_eq!(bf["checked"].as_u64(), Some(2), "{bf}");
        assert_eq!(bf["from_pieces"].as_u64(), Some(2), "{bf}");
        let over = bf["over"].as_array().cloned().unwrap_or_default();
        assert_eq!(over.len(), 1, "{bf}");
        let o = &over[0];
        assert_eq!(o["box"], json!(upper.hex()), "{o}");
        assert_eq!((o["needed_cu_in"].as_f64(), o["runs"].as_u64(), o["counted_from"].as_str()), (Some(33.75), Some(4), Some("pieces")), "{o}");
        assert_eq!(o["corrected"], json!(true), "{o}");
        assert_eq!(text_of(&d, upper, "size"), "4-11/16\" square, 2-1/8\" deep");
        assert_eq!(text_of(&d, upper, "cubic_in"), "42");
        assert_eq!(text_of(&d, upper, "design:cubic_in"), "30.3", "the design box kept");
        assert!(text_of(&d, upper, "correction:size").starts_with("E-BOX-FILL"));
        // The lower box: its own run only (2#12 + G + yoke) in 54 cu in.
        assert_eq!(text_of(&d, lower, "design:cubic_in"), "");
    }
}

#[test]
fn a_junction_box_past_the_largest_ring_is_built_as_a_pull_box() {
    // A junction box (no device) where five runs of 6#8 meet: 30#8 + G = 93 cu in, more than a
    // 4-11/16" square with its ring (63). The field sets a 6" x 6" x 4" box (144 cu in).
    let mut d = Drawing::default();
    let jb = branch_piece(&mut d, "box", "4\" square", &[("cubic_in", "30.3".into())], cube([118.0, 118.0, 100.0], [122.0, 122.0, 102.125]));
    for (i, x) in [0.0, 60.0, 180.0, 240.0, 120.0].iter().enumerate() {
        let far = if i == 4 { [120.0, 60.0, 101.0] } else { [*x, 120.0, 101.0] };
        wired_run(&mut d, &format!("r{i}"), "LP9-7", "3/4\"", far, [120.0, 120.0, 101.0], 6, "8");
    }
    let mut report = json!({"box_fill": {"pending": []}});
    check_box_fill(&mut d, &mut report);
    let bf = &report["box_fill"];
    assert_eq!((bf["checked"].as_u64(), bf["corrected"].as_u64()), (Some(1), Some(1)), "{bf}");
    assert_eq!(bf["over"][0]["needed_cu_in"].as_f64(), Some(93.0), "{bf}");
    assert_eq!(text_of(&d, jb, "size"), "6\" x 6\" x 4\" pull box");
    assert_eq!(text_of(&d, jb, "cubic_in"), "144");
    assert!(text_of(&d, jb, "correction:cubic_in").contains("314.16(A)(1)"), "{}", text_of(&d, jb, "correction:cubic_in"));
}

#[test]
fn a_device_box_a_large_homerun_lands_in_gets_a_pull_box_by_314_28() {
    // A 1-1/4" homerun with 2#3 + 2#6 lands in a receptacle's 4-11/16" square box: with #4 and
    // larger, NEC 314.28 asks a box 8 x 1-1/4 = 10" long (6 x 1-1/4 + 1/2 = 8" for the angle).
    // The field sets a 10" x 10" x 4" pull box beside it, the receptacle fed from it.
    let mut d = Drawing::default();
    let bx = device_box(&mut d, "1C4", "4-11/16\" square", 42.0, 240.0, 120.0, 40.0, 4.6875);
    let top = [240.0, 120.0, 42.125];
    branch_piece(
        &mut d,
        "conduit",
        "1-1/4\"",
        &[("run", "home#9".into()), ("circuit", "LP9-2,4".into()), ("start", "240,120,108".into()), ("end", "240,120,42.125".into())],
        crate::cmd::wallgen::empty_mesh(),
    );
    for (size, role) in [("#3", "phase A"), ("#3", "phase B"), ("#6", "phase A"), ("#6", "neutral"), ("#6", "ground")] {
        branch_piece(
            &mut d,
            "wire",
            size,
            &[("run", "home#9".into()), ("circuit", "LP9-2,4".into()), ("conductor", role.into())],
            crate::cmd::wallgen::empty_mesh(),
        );
    }
    wired_run(&mut d, "LP9-2#1", "LP9-2", "1/2\"", top, [300.0, 120.0, 108.0], 2, "12");
    let mut report = json!({"box_fill": {"pending": [{"device": "1C4", "kind": "receptacle", "level": "L1", "circuit": "LP9-2", "yoke": true, "conductors": [[2, "12"]]}]}});
    check_box_fill(&mut d, &mut report);
    let o = &report["box_fill"]["over"][0];
    assert_eq!((o["pull_length_in"].as_f64(), o["corrected"].as_bool()), (Some(10.0), Some(true)), "{o}");
    assert!(text_of(&d, bx, "size").starts_with("10\" x 10\" x 4\" pull box beside the receptacle's"), "{}", text_of(&d, bx, "size"));
    assert_eq!(text_of(&d, bx, "length_in"), "10");
    assert_eq!(text_of(&d, bx, "design:length_in"), "4.69");
    assert!(text_of(&d, bx, "correction:length_in").contains("314.28"), "{}", text_of(&d, bx, "correction:length_in"));
}
