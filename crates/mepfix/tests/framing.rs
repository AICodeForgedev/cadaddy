//! Framing layout from prints (Q303): fixed members from the plans, both rule layouts per wall,
//! stacked walls sharing one, shop drawings and field points overriding them, field-change
//! flags, device snapping, raceway holes and bits — on made-up prints scored against the truth.

use cadaddy_mepfix::bore::{self, BitKind, HoleAt, HoleChart, Member as BoreMember, Raceway};
use cadaddy_mepfix::framing::*;
use cadaddy_mepfix::truth::Truth;
use cadaddy_mepfix::truth_for;

const SEEDS: std::ops::RangeInclusive<u64> = 1..=6;

fn wall(len_in: f64) -> FrameWall {
    FrameWall {
        id: "T-1".into(),
        level: "1".into(),
        a_ft: [0.0, 0.0],
        b_ft: [len_in / 12.0, 0.0],
        thickness_in: 4.875,
        wall_type: "A".into(),
        stud_in: Some(3.625),
        flange_in: METAL_FLANGE_IN,
        spacing_in: 16.0,
        height_ft: 10.0,
        ..FrameWall::default()
    }
}

fn studs(wf: &WallFrame, pred: impl Fn(&Member) -> bool) -> Vec<f64> {
    wf.members.iter().filter(|m| pred(m)).map(|m| m.s_in).collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn every_stud_wall_has_a_true_layout_end_and_stacks_share_it() {
    for seed in SEEDS {
        let t = truth_for(seed);
        let fr = frame(&walls_from_truth(&t), &[], &[], &[]);
        assert!(!fr.stacks.is_empty(), "seed {seed}: walls stack storey over storey");
        for w in &t.plan_walls {
            assert!(w.layout_from.as_deref().and_then(End::parse).is_some(), "seed {seed} {}", w.id);
        }
        for g in &fr.stacks {
            let pulled: Vec<[f64; 2]> = g
                .iter()
                .map(|i| {
                    let w = &t.plan_walls[*i];
                    if w.layout_from.as_deref() == Some("a") { w.a_ft } else { w.b_ft }
                })
                .collect();
            for p in &pulled {
                assert!((p[0] - pulled[0][0]).hypot(p[1] - pulled[0][1]) < 0.25, "seed {seed}: stack {g:?} pulled from one place");
            }
        }
        assert_eq!(truth_for(seed).plan_walls, t.plan_walls, "deterministic");
    }
    // Both ends occur.
    let t = truth_for(1);
    assert!(t.plan_walls.iter().any(|w| w.layout_from.as_deref() == Some("a")) && t.plan_walls.iter().any(|w| w.layout_from.as_deref() == Some("b")));
    assert_eq!(cadaddy_mepfix::generate(3).unwrap().truth.plan_walls, truth_for(3).plan_walls, "a generated set carries the same layouts");
}

fn read(seed: u64) -> (Truth, Framing, Framing) {
    let (t, fr) = read_fixture_framing(seed).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    let tf = truth_framing(&t);
    (t, tf, fr)
}

#[test]
fn fixed_members_are_exact_and_rule_members_match_under_the_right_layout() {
    for seed in SEEDS {
        let (t, tf, fr) = read(seed);
        let sc = score_framing(&t, &tf, &fr);
        println!("seed {seed}: {sc:?}");
        assert_eq!(sc.walls_matched, sc.walls_total, "seed {seed}: every stud wall found");
        assert!(sc.fixed_total > 300 && sc.rule_total > 700, "seed {seed}: {sc:?}");
        assert_eq!(sc.fixed_found, sc.fixed_total, "seed {seed}: fixed members within 1/8\"");
        assert_eq!(sc.fixed_extra, 0, "seed {seed}: no made-up fixed members");
        assert_eq!(sc.rule_found, sc.rule_total, "seed {seed}: rule members under the true layout");
        assert_eq!(sc.rule_extra, 0, "seed {seed}");
        assert!(sc.worst_in < 0.125, "seed {seed}");
        // The kinds the prints fix are all there: tees, posts, openings (the building's corners
        // all have columns, so their studs are post studs).
        let kinds: std::collections::BTreeSet<Kind> =
            fr.walls.iter().flat_map(|w| w.members.iter().filter(|m| m.status == Status::Fixed).map(|m| m.kind)).collect();
        for k in [Kind::EndStud, Kind::IntersectionStud, Kind::Post, Kind::PostStud, Kind::KingStud, Kind::JackStud, Kind::Header] {
            assert!(kinds.contains(&k), "seed {seed}: {k:?}");
        }
        // Both layouts kept where they differ; masonry gets no studs.
        assert!(
            fr.walls.iter().any(|w| w.members.iter().any(|m| m.layout == Some(End::A)))
                && fr.walls.iter().any(|w| w.members.iter().any(|m| m.layout == Some(End::B)))
        );
        assert!(fr.walls.iter().any(|w| w.wall.stud_in.is_none() && w.members.is_empty() && w.flags.iter().any(|f| f.contains("masonry"))));
        assert!(!fr.stacks.is_empty(), "seed {seed}: read walls stack too");
        // Untagged walls say their framing follows a guessed type.
        let untagged = t.plan_walls.iter().filter(|w| !w.tagged).count();
        let flagged = fr.walls.iter().filter(|w| w.flags.iter().any(|f| f.starts_with("untagged wall, type guessed"))).count();
        assert_eq!(flagged, untagged, "seed {seed}");
    }
}

#[test]
fn layout_marks_and_the_next_stud_dimension() {
    // Wood studs: the first mark from the pulled end is 15 1/4", and a 10'-2 1/2" wall laid out
    // from the other end puts the near edge of its first stud 9 3/4" from this one.
    let mut w = wall(122.5);
    w.flange_in = WOOD_STUD_IN;
    let fr = frame(&[w], &[], &[], &[]);
    let wf = &fr.walls[0];
    let [a, b] = wf.next_stud_in(End::A);
    assert!(close(a.unwrap(), 15.25) && close(b.unwrap(), 9.75), "{a:?} {b:?}");
    assert_eq!(wf.next_stud_words(End::A), "next stud 16\" o.c. → 15-1/4\" from the a end if laid out from it; 9-3/4\" if laid out from the b end");
    let a_studs = studs(wf, |m| m.layout == Some(End::A));
    assert!(close(a_studs[0], 16.0) && close(a_studs[1], 32.0), "centres on multiples of 16\": {a_studs:?}");
    // Metal studs stand on the far side of the same marks.
    let fr = frame(&[wall(120.0)], &[], &[], &[]);
    let wf = &fr.walls[0];
    assert!(close(wf.next_stud_in(End::A)[0].unwrap(), 15.25));
    assert!(close(studs(wf, |m| m.layout == Some(End::A))[0], 16.0 - 0.75 + METAL_FLANGE_IN / 2.0));
    // Ends.
    assert_eq!(studs(wf, |m| m.kind == Kind::EndStud), vec![METAL_FLANGE_IN / 2.0, 120.0 - METAL_FLANGE_IN / 2.0]);
}

#[test]
fn where_the_layouts_coincide_the_members_are_agreed() {
    // 128 1/8": both layouts land on the same centres.
    let fr = frame(&[wall(128.125)], &[], &[], &[]);
    let wf = &fr.walls[0];
    assert!(wf.members.iter().all(|m| m.status != Status::Rule), "{:?}", wf.members);
    assert_eq!(wf.members.iter().filter(|m| m.status == Status::Agreed).count(), 7);
    assert!(wf.next_stud_words(End::A).contains("either way"));
    // 120": none agree.
    let fr = frame(&[wall(120.0)], &[], &[], &[]);
    assert_eq!(fr.walls[0].members.iter().filter(|m| m.status == Status::Agreed).count(), 0);
    let t = fr.takeoff();
    assert_eq!(t.by_status(Status::Fixed), 2);
    assert_eq!(t.rule_a.values().sum::<usize>(), 7);
    assert_eq!(t.rule_max(), 7);
}

#[test]
fn openings_get_kings_jacks_headers_and_cripples_on_the_layout() {
    let mut w = wall(144.0);
    w.openings = vec![[4.0, 7.0]];
    let fr = frame(&[w.clone()], &[], &[], &[]);
    let wf = &fr.walls[0];
    let f = METAL_FLANGE_IN;
    assert_eq!(studs(wf, |m| m.kind == Kind::JackStud), vec![48.0 - f / 2.0, 84.0 + f / 2.0]);
    assert_eq!(studs(wf, |m| m.kind == Kind::KingStud), vec![48.0 - 1.5 * f, 84.0 + 1.5 * f]);
    let h = wf.members.iter().find(|m| m.kind == Kind::Header).unwrap();
    assert!(close(h.z_in[0], DOOR_HEAD_IN) && close(h.z_in[1], DOOR_HEAD_IN + 6.0) && h.note.contains("placeholder"), "{h:?}");
    // The layout carries through as cripples above the header, never as studs in the opening.
    // Layout A's centres in the opening: 48 1/16", 64 1/16", 80 1/16" (the last two agree with
    // layout B's 63 15/16" and 79 15/16").
    let cr: Vec<f64> = studs(wf, |m| m.kind == Kind::Cripple && m.in_layout(End::A));
    assert_eq!(cr.len(), 3, "{cr:?}");
    assert_eq!(wf.members.iter().filter(|m| m.kind == Kind::Cripple && m.status == Status::Agreed).count(), 2);
    assert!(wf.members.iter().filter(|m| m.kind == Kind::FieldStud).all(|m| m.s_in < 48.0 || m.s_in > 84.0));
    assert!(wf.members.iter().filter(|m| m.kind == Kind::Cripple).all(|m| close(m.z_in[0], DOOR_HEAD_IN + 6.0) && close(m.z_in[1], 120.0)));
    // A header schedule sizes it.
    let sched = vec![
        HeaderRow { max_span_in: 60.0, size: "(2) 800S162-54 box".into(), depth_in: 8.0 },
        HeaderRow { max_span_in: 999.0, size: "(2) 1000S162-68 box".into(), depth_in: 10.0 },
    ];
    let fr = frame(&[w], &[], &[], &sched);
    let h = fr.walls[0].members.iter().find(|m| m.kind == Kind::Header).unwrap();
    assert_eq!(h.note, "(2) 800S162-54 box (header schedule)");
    assert!(close(h.z_in[1], DOOR_HEAD_IN + 8.0));
}

#[test]
fn corners_tees_and_posts() {
    // A through wall 20' long; one wall tees in at 10', another meets it 4" from its end (L).
    let through = wall(240.0);
    let mut tee = wall(96.0);
    tee.id = "T-2".into();
    tee.a_ft = [10.0, 4.875 / 24.0];
    tee.b_ft = [10.0, 8.0];
    let mut ell = tee.clone();
    ell.id = "T-3".into();
    ell.a_ft = [4.875 / 24.0, 4.875 / 24.0];
    ell.b_ft = [4.875 / 24.0, 8.0];
    let post = Post { at_ft: [15.0, 0.0], footprint_in: [8.0, 8.0] };
    let fr = frame(&[through, tee, ell], &[post], &[], &[]);
    let wf = &fr.walls[0];
    let f = METAL_FLANGE_IN;
    let th = 4.875;
    assert_eq!(studs(wf, |m| m.kind == Kind::IntersectionStud), vec![120.0 - th / 2.0 + f / 2.0, 120.0 + th / 2.0 - f / 2.0]);
    // The L: the end stud plus one corner stud on the inside (with the other wall's end stud,
    // a three-stud corner).
    assert_eq!(studs(wf, |m| m.kind == Kind::CornerStud), vec![th - f / 2.0]);
    assert_eq!(studs(wf, |m| m.kind == Kind::Post), vec![180.0]);
    assert_eq!(studs(wf, |m| m.kind == Kind::PostStud), vec![176.0 - f / 2.0, 184.0 + f / 2.0]);
    assert!(wf.members.iter().filter(|m| m.kind == Kind::FieldStud).all(|m| (m.s_in - 180.0).abs() > 4.0), "no studs through the post");
    // The tee's own ends are end studs.
    assert_eq!(fr.walls[1].members.iter().filter(|m| m.kind == Kind::EndStud).count(), 2);
}

#[test]
fn a_field_point_snaps_the_run_and_the_walls_stacked_with_it() {
    for seed in [1u64, 4] {
        let (t, tf, mut fr) = read(seed);
        // On each stack's first wall, tape one true stud: every wall in the read framing then
        // matches the truth exactly.
        let mut done = std::collections::BTreeSet::new();
        for i in 0..fr.walls.len() {
            if done.contains(&i) || fr.walls[i].wall.stud_in.is_none() {
                continue;
            }
            let rw = fr.walls[i].wall.clone();
            let Some((ti, tw)) = tf.walls.iter().enumerate().find(|(_, tw)| {
                let ((s0, o0), (s1, o1)) = (tw.wall.local(rw.a_ft), tw.wall.local(rw.b_ft));
                let l = tw.wall.len_in();
                tw.wall.level == rw.level && o0.abs() < 1.0 && o1.abs() < 1.0 && s0.max(s1).min(l) - s0.min(s1).max(0.0) > 0.5 * l
            }) else {
                continue;
            };
            let Some(true_stud) = tw.members.iter().find(|m| m.kind == Kind::FieldStud) else { continue };
            let s = rw.local(tw.wall.at(true_stud.s_in)).0;
            let snap = fr.field_point(i, s).unwrap();
            let pulled = t.plan_walls[ti].layout_from.as_deref().and_then(End::parse).unwrap();
            let world = tw.wall.at(if pulled == End::A { 0.0 } else { tw.wall.len_in() });
            let want = if rw.local(world).0.abs() < 1.0 { End::A } else { End::B };
            assert_eq!(snap.matches, Some(want), "seed {seed} {}: the measured stud tells the end", rw.id);
            done.extend(snap.walls.iter().copied());
        }
        let sc = score_framing(&t, &tf, &fr);
        assert_eq!(sc.rule_found, sc.rule_total, "seed {seed}: verified studs match the truth");
        let n_verified: usize = fr.walls.iter().map(|w| w.members.iter().filter(|m| m.status == Status::FieldVerified).count()).sum();
        assert!(n_verified > 500, "seed {seed}: {n_verified}");
        assert!(fr.walls.iter().all(|w| w.members.iter().all(|m| m.status != Status::Rule)), "seed {seed}: no rule members left");
        // Fewer field points than walls: stacks share them.
        assert!(done.len() > fr.stacks.len());
    }
}

#[test]
fn an_off_layout_field_point_and_a_tape_measurement() {
    let mut fr = frame(&[wall(120.0)], &[], &[], &[]);
    let snap = fr.field_point(0, 22.0).unwrap();
    assert_eq!(snap.matches, None);
    let v = studs(&fr.walls[0], |m| m.status == Status::FieldVerified);
    assert!(v.iter().any(|s| close(*s, 22.0)) && v.iter().any(|s| close(*s, 38.0)) && v.iter().any(|s| close(*s, 6.0)), "{v:?}");
    assert!(fr.walls[0].settled.as_deref().unwrap().contains("neither end"));
    assert!(fr.walls[0].next_stud_words(End::A).contains("field verified"));
    // Tape from the b end to a stud's near edge: 15 1/4" means laid out from b.
    let mut fr = frame(&[wall(120.0)], &[], &[], &[]);
    let snap = fr.field_tape(0, Tape { from: End::B, to_near_edge_in: 15.25 }).unwrap();
    assert_eq!(snap.matches, Some(End::B));
    // Hostile points are refused.
    assert!(fr.field_point(0, f64::NAN).is_err() && fr.field_point(0, -5.0).is_err() && fr.field_point(7, 10.0).is_err());
}

#[test]
fn a_stud_picked_on_a_site_photo() {
    // A wall seen in perspective: s inches along it shows at x = (a s + b)/(c s + 1) on a
    // slanted line in the photo.
    let proj = |s: f64| {
        let x = (3.1 * s + 120.0) / (0.004 * s + 1.0);
        [x, 400.0 - 0.35 * x]
    };
    let refs = [(proj(0.0), 0.0), (proj(48.0), 48.0), (proj(120.0), 120.0)];
    for s in [16.0625, 31.3, 64.0, 100.0] {
        let got = photo_to_wall(refs, proj(s)).unwrap();
        assert!((got - s).abs() < 1e-6, "{s} → {got}");
    }
    let mut fr = frame(&[wall(120.0)], &[], &[], &[]);
    let s = photo_to_wall(refs, proj(32.0625)).unwrap();
    assert_eq!(fr.field_point(0, s).unwrap().matches, Some(End::A));
    // Degenerate references.
    assert!(photo_to_wall([([1.0, 1.0], 0.0), ([1.0, 1.0], 5.0), ([1.0, 1.0], 9.0)], [2.0, 2.0]).is_none());
}

fn shop_dxf() -> Vec<u8> {
    // A made-up metal stud shop drawing in its own inches with its origin 10' west of the
    // building's: studs drawn as small rectangles 12" o.c. along the wall, joists as lines, a
    // tendon as a polyline.
    use cadaddy_doc::{Common, Drawing, EntityKind, Line, LwPolyline, Space};
    use cadaddy_geom::{PolyVertex, Vec2, Vec3};
    let mut d = Drawing::new_imperial();
    let pl = |pts: &[[f64; 2]], closed| {
        EntityKind::LwPolyline(LwPolyline {
            vertices: pts.iter().map(|p| PolyVertex { p: Vec2::new(p[0], p[1]), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect(),
            closed,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        })
    };
    let c = |layer: &str| Common { layer: layer.into(), ..Common::default() };
    for k in 1..10 {
        let x = 120.0 + 12.0 * k as f64;
        let _ = d.add(&Space::Model, c("S-STUD"), pl(&[[x - 0.8125, -1.8], [x + 0.8125, -1.8], [x + 0.8125, 1.8], [x - 0.8125, 1.8]], true));
    }
    for k in 0..5 {
        let y = 300.0 + 19.2 * k as f64;
        let _ = d.add(&Space::Model, c("S-JOIST"), EntityKind::Line(Line { a: Vec3::new(120.0, y, 0.0), b: Vec3::new(360.0, y, 0.0) }));
    }
    let _ = d.add(&Space::Model, c("S-PT-TENDON"), pl(&[[120.0, 500.0], [360.0, 500.0]], false));
    cadaddy_io::write_dxf(&d).into_bytes()
}

#[test]
fn a_shop_drawing_replaces_the_rule_members_where_it_covers() {
    let bay = JoistBay {
        id: "B1".into(),
        level: "1".into(),
        min_ft: [0.0, 24.0],
        max_ft: [20.0, 34.0],
        along_x: true,
        spacing_in: 16.0,
        width_in: 1.75,
        member: BoreMember::IJoist { depth_in: 11.875, chart: HoleChart::example() },
    };
    let mut other = wall(120.0);
    other.id = "T-9".into();
    other.a_ft = [0.0, 10.0];
    other.b_ft = [10.0, 10.0];
    let mut fr = frame(&[wall(120.0), other], &[], &[], &[]);
    fr.bays = frame_bays(&[bay], &[]);
    let d = cadaddy_io::read_dxf(&shop_dxf()).unwrap();
    let sd = read_shop_drawing(&d, &|p| [(p[0] - 120.0) / 12.0, p[1] / 12.0]);
    assert_eq!((sd.studs.len(), sd.joists.len(), sd.tendons.len()), (9, 5, 1));
    fr.apply_shop_drawing("1", &sd);
    let wf = &fr.walls[0];
    assert_eq!(wf.settled.as_deref(), Some("shop drawing"));
    let shop = studs(wf, |m| m.status == Status::ShopDrawing);
    assert_eq!(shop.len(), 9);
    assert!(shop.iter().zip(1..).all(|(s, k)| (s - 12.0 * k as f64).abs() < 1e-6), "{shop:?}");
    assert!(wf.members.iter().all(|m| !matches!(m.status, Status::Rule | Status::Agreed)));
    assert!(wf.members.iter().any(|m| m.kind == Kind::EndStud && m.status == Status::Fixed), "fixed members stay");
    // The other wall is untouched.
    assert!(fr.walls[1].settled.is_none() && fr.walls[1].members.iter().any(|m| m.status == Status::Rule));
    // Joists: the shop layout replaces the rule joists; rims stay.
    let b = &fr.bays[0];
    assert_eq!(b.settled.as_deref(), Some("shop drawing"));
    assert_eq!(b.members.iter().filter(|m| m.status == Status::ShopDrawing).count(), 5);
    assert_eq!(b.members.iter().filter(|m| m.kind == Kind::Rim).count(), 2);
    // The tendon's no-core zone: 12" each side.
    assert!(fr.in_no_core("1", [10.0, 500.0 / 12.0 + 0.9]));
    assert!(!fr.in_no_core("1", [10.0, 500.0 / 12.0 + 1.1]));
    assert!(!fr.in_no_core("2", [10.0, 500.0 / 12.0]));
    let t = fr.takeoff();
    assert_eq!(t.by_status(Status::ShopDrawing), 14);
}

#[test]
fn joist_bays_cans_and_crossings() {
    let bay = JoistBay {
        id: "B1".into(),
        level: "2".into(),
        min_ft: [0.0, 0.0],
        max_ft: [16.0, 12.5],
        along_x: true,
        spacing_in: 16.0,
        width_in: 1.5,
        member: BoreMember::SawnJoist { depth_in: 9.25 },
    };
    let beam = ([0.0, 6.0], [16.0, 6.0], 5.0);
    let bf = &frame_bays(&[bay], &[beam])[0];
    assert_eq!(bf.members.iter().filter(|m| m.kind == Kind::Rim).count(), 2);
    assert_eq!(bf.members.iter().filter(|m| m.kind == Kind::Beam).count(), 1);
    assert!(bf.members.iter().any(|m| m.layout == Some(End::A)) && bf.members.iter().any(|m| m.layout == Some(End::B)));
    // A 6" can centred between the layout-A joists at 16" and 32" (y = 2'): clear under A.
    let [a, _] = bf.can_check([5.0, 2.0], 6.0);
    assert!(a.is_ok(), "{a:?}");
    // On the beam it hits either way.
    let [a, b] = bf.can_check([5.0, 6.0], 6.0);
    assert!(a.unwrap_err().contains("beam") && b.is_err());
    // 3" EMT across the joists: too big for a 2x10 (≤ 1/3 depth); 1" fits.
    let (n, c) = bf.crossing([3.0, 0.5], [3.0, 11.5], Raceway::Emt(3.0), HoleAt::default());
    assert!(n[0] >= 6 && n[1] >= 6, "{n:?}");
    assert!(!c.fits && c.proposal.is_some(), "{c:?}");
    let (_, c) = bf.crossing([3.0, 0.5], [3.0, 11.5], Raceway::Emt(1.0), HoleAt::default());
    assert!(c.fits, "{c:?}");
}

#[test]
fn hole_sizes_bits_and_member_limits() {
    // ODs and bits.
    assert_eq!(Raceway::Emt(3.0).od_in(), Some(3.5));
    assert_eq!(Raceway::Ent(2.0).od_in(), Some(2.375));
    assert_eq!(Raceway::Ent(3.0).od_in(), None, "ENT stops at 2\"");
    assert_eq!(bore::pick_bit(Raceway::Mc, 0.125), Some(bore::Bit { dia_in: 0.875, kind: BitKind::Paddle }));
    assert_eq!(bore::pick_bit(Raceway::Emt(1.0), 0.125).unwrap().dia_in, 1.375);
    assert_eq!(bore::pick_bit(Raceway::PvcSch40(2.0), 0.125).unwrap().dia_in, 2.5);
    assert!(bore::pick_bit(Raceway::Od(20.0), 0.125).is_none());
    // Sawn studs: bearing 40%, non-bearing 60%.
    let bearing = BoreMember::SawnStud { depth_in: 3.5, bearing: true };
    let partition = BoreMember::SawnStud { depth_in: 3.5, bearing: false };
    assert!(bore::check(Raceway::Emt(1.0), &bearing, HoleAt::default(), 0.125).fits, "1-3/8\" ≤ 1.4\"");
    let c = bore::check(Raceway::Emt(1.25), &bearing, HoleAt::default(), 0.125);
    assert!(!c.fits && c.flags[0].contains("40%"), "{c:?}");
    assert_eq!(c.proposal, Some((2, Raceway::Emt(1.0))));
    assert!(bore::check(Raceway::Emt(1.25), &partition, HoleAt::default(), 0.125).fits);
    // Edge distance on joists.
    let joist = BoreMember::SawnJoist { depth_in: 11.25 };
    let c = bore::check(Raceway::Emt(1.0), &joist, HoleAt { centre_from_edge_in: Some(1.5), from_support_ft: None }, 0.125);
    assert!(!c.fits && c.flags.iter().any(|f| f.contains("edge")), "{c:?}");
    // I-joists by the (example) chart, trusses never.
    let ij = BoreMember::IJoist { depth_in: 11.875, chart: HoleChart::example() };
    assert!(bore::check(Raceway::Emt(3.0), &ij, HoleAt { centre_from_edge_in: None, from_support_ft: Some(5.0) }, 0.125).fits);
    let c = bore::check(Raceway::Emt(3.0), &ij, HoleAt { centre_from_edge_in: None, from_support_ft: Some(1.0) }, 0.125);
    assert!(!c.fits && c.flags.iter().any(|f| f.contains("support")));
    let c = bore::check(Raceway::Emt(0.5), &BoreMember::Truss, HoleAt::default(), 0.125);
    assert!(!c.fits && c.flags.iter().any(|f| f.contains("truss")) && c.proposal.is_none());
    // Metal studs: through the punchouts when it fits, else flagged.
    let ms = BoreMember::MetalStud { depth_in: 3.625, punchout_in: 1.5 };
    let c = bore::check(Raceway::Emt(0.75), &ms, HoleAt::default(), 0.125);
    assert!(c.fits && c.bit.unwrap().kind == BitKind::Punchout);
    let c = bore::check(Raceway::Emt(1.5), &ms, HoleAt::default(), 0.125);
    assert!(!c.fits && c.bit.unwrap().kind == BitKind::MetalHoleSaw);
}

#[test]
fn the_three_inch_sleeve_lesson() {
    // A 3" sleeve through 2x10 floor joists won't fit; the nearest fitting option is four 2" ENT.
    let joist = BoreMember::SawnJoist { depth_in: 9.25 };
    let c = bore::check(Raceway::Od(3.5), &joist, HoleAt::default(), 0.125);
    assert!(!c.fits);
    assert_eq!(c.proposal, Some((4, Raceway::Ent(2.0))));
    assert_eq!(Raceway::Ent(2.0).words(), "2\" ENT");
    // The bits for the run: 4 holes per joist × 10 joists, one hole saw size.
    let fit = bore::check(Raceway::Ent(2.0), &joist, HoleAt::default(), 0.125);
    assert!(fit.fits);
    let checks: Vec<_> = std::iter::repeat_n(fit, 40).collect();
    let tk = bore::bit_takeoff(&checks);
    assert_eq!(tk, vec![bore::BitLine { kind: BitKind::HoleSaw, dia_in: 2.5, holes: 40 }]);
}

#[test]
fn field_changes_devices_and_boxes() {
    for seed in [1u64, 2] {
        let t = truth_for(seed);
        let mut fr = frame(&walls_from_truth(&t), &posts_from_truth(&t), &[], &[]);
        let plumbing = fr.walls.iter().filter(|w| w.wall.plumbing).count();
        assert!(plumbing >= 2, "seed {seed}");
        assert!(fr.walls.iter().filter(|w| w.wall.plumbing).all(|w| !w.field_changes.is_empty()));
        for lv in ["1", "2"] {
            let drains: Vec<(String, [f64; 2])> =
                t.fixtures.iter().filter(|f| f.level == lv).map(|f| (f.kind.replace('_', " "), [f.at_ft[0], f.at_ft[1]])).collect();
            fr.flag_field_changes(lv, &drains, &[]);
        }
        let with_drains = fr.walls.iter().filter(|w| w.field_changes.iter().any(|n| n.contains("drain"))).count();
        println!("seed {seed}: {plumbing} plumbing walls, {with_drains} with drains beside them");
    }
    // A device on a settled wall lands on a stud, measured to the drain.
    let mut fr = frame(&[wall(120.0)], &[], &[], &[]);
    fr.field_point(0, 32.0625).unwrap();
    let lav = vec![("lav drain".to_string(), [36.0625 / 12.0, 0.4])];
    let snap = fr.snap_device(0, 30.0, 8.0, &lav).unwrap();
    assert_eq!(snap.text, "lands on the field stud at 32-1/16\" from the a end, 4\" from the lav drain (field verified)");
    // Unsettled: each layout's stud.
    let fr = frame(&[wall(120.0)], &[], &[], &[]);
    let snap = fr.snap_device(0, 30.0, 8.0, &[]).unwrap();
    assert!(
        snap.stud.is_none() && snap.by_layout[0].is_some() && snap.text.contains("if laid out from the a end") && snap.text.contains("b end"),
        "{}",
        snap.text
    );
    // Box depth vs the stud cavity.
    assert!(box_depth_flag(&wall(120.0), 2.5).is_none());
    assert!(box_depth_flag(&FrameWall { stud_in: Some(2.5), ..wall(120.0) }, 3.5).is_some());
}

#[test]
fn raceways_through_and_across_walls() {
    let fr = frame(&[wall(120.0)], &[], &[], &[]);
    // Along inside the wall from 1' to 9': through every stud between, punchouts for 3/4" EMT.
    let x = fr.walls.iter().position(|_| true).unwrap();
    let c = wall_crossings(&fr, "1", [1.0, 0.0], [9.0, 0.0], Raceway::Emt(0.75));
    assert_eq!(c.len(), 1);
    assert!(c[0].along && c[0].wall == x && c[0].studs[0] == 6 && c[0].check.as_ref().unwrap().fits, "{c:?}");
    // Across it right on a layout-A stud (16 1/16").
    let s = 16.0625 / 12.0;
    let c = wall_crossings(&fr, "1", [s, -2.0], [s, 2.0], Raceway::Emt(1.0));
    assert!(!c[0].along && c[0].studs == [1, 0] && c[0].note.contains("a end"), "{c:?}");
}

#[test]
fn hostile_input_never_panics() {
    let bad = FrameWall {
        a_ft: [f64::NAN, 0.0],
        b_ft: [f64::INFINITY, 1e300],
        thickness_in: -4.0,
        spacing_in: 0.0,
        height_ft: f64::NAN,
        openings: vec![[f64::NAN, 3.0], [5.0, 2.0], [-1e9, 1e9]],
        ..wall(100.0)
    };
    let zero = FrameWall { b_ft: [0.0, 0.0], ..wall(0.0) };
    let huge = FrameWall { b_ft: [1e9, 0.0], spacing_in: 1e-9, ..wall(0.0) };
    let mut fr = frame(&[bad, zero, huge, wall(120.0)], &[Post { at_ft: [f64::NAN, 0.0], footprint_in: [f64::INFINITY, -1.0] }], &[], &[]);
    let _ = fr.field_point(0, 10.0);
    let _ = fr.field_point(2, 1e6);
    let _ = fr.shop_studs("1", &[[f64::NAN, 0.0], [1.0, 0.0]]);
    let _ = fr.snap_device(9, 1.0, 1.0, &[]);
    let _ = fr.snap_device(0, f64::NAN, f64::INFINITY, &[("x".into(), [f64::NAN, 1.0])]);
    fr.flag_field_changes("1", &[("x".into(), [f64::NAN, 0.0])], &[]);
    let _ = wall_crossings(&fr, "1", [f64::NAN, 0.0], [1.0, 1.0], Raceway::Od(f64::NAN));
    let _ = bore::check(Raceway::Od(f64::NAN), &BoreMember::SawnJoist { depth_in: f64::NAN }, HoleAt::default(), f64::NAN);
    let _ = frame_bays(
        &[JoistBay {
            id: String::new(),
            level: "1".into(),
            min_ft: [0.0, 0.0],
            max_ft: [1e12, f64::NAN],
            along_x: false,
            spacing_in: f64::NAN,
            width_in: 0.0,
            member: BoreMember::Truss,
        }],
        &[([0.0, 0.0], [0.0, 1.0], f64::NAN)],
    );
    for w in &fr.walls {
        let _ = draw_elevation(w);
        assert!(w.members.len() < 20_000);
    }
    let _ = draw_plan(&fr, "1");
    let _ = section_footprint("HSSx");
    let _ = section_footprint("HSS1e400x2");
}

#[test]
fn drawings_show_fixed_solid_and_rule_ghosted() {
    let (_, _, fr) = read(1);
    let wf = fr
        .walls
        .iter()
        .filter(|w| !w.wall.openings.is_empty() && w.wall.stud_in.is_some())
        .min_by(|a, b| a.wall.len_in().total_cmp(&b.wall.len_in()))
        .unwrap();
    let d = draw_elevation(wf);
    let layer = |n: &str| d.layers.iter().find(|l| l.name == n).unwrap();
    assert_eq!(layer("FRAMING-FIXED").linetype, "Continuous");
    assert_ne!(layer("FRAMING-RULE-A").linetype, "Continuous");
    assert_ne!(layer("FRAMING-RULE-B").linetype, "Continuous");
    let on = |n: &str| d.model.iter().filter(|e| e.common.layer == n).count();
    assert!(on("FRAMING-FIXED") > 0 && on("FRAMING-RULE-A") > 0 && on("FRAMING-RULE-B") > 0 && on("FRAMING-DIMS") > 0);
    let texts: Vec<String> = d
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Text(t) => Some(t.value.clone()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t.starts_with("next stud 16\" o.c.")), "{texts:?}");
    let p = draw_plan(&fr, "1");
    assert!(p.model.iter().filter(|e| e.common.layer.starts_with("FRAMING-RULE")).count() > 100);
    let b = boxes(&fr, "1", 0.0);
    assert!(b.iter().any(|x| x.2 == Status::Fixed) && b.iter().any(|x| x.3 == Some(End::B)));
    assert!(b.iter().all(|x| x.0[2] <= x.1[2] && x.0[0] <= x.1[0]));
    // Pictures to look at: FRAMING_SHOTS=<dir> writes the elevation and plan as PDFs.
    if let Ok(dir) = std::env::var("FRAMING_SHOTS") {
        let dir = std::path::Path::new(&dir);
        let _ = std::fs::create_dir_all(dir);
        let opts = |w: f64, h: f64, window: Option<cadaddy_geom::Bounds2>| cadaddy_io::pdf::PdfOptions {
            window,
            paper_mm: Some((h * 25.4, w * 25.4)),
            landscape: Some(true),
            fit: Some(true),
            lineweights: Some(true),
            stamp: Some(false),
            ..cadaddy_io::pdf::PdfOptions::default()
        };
        let mut fr2 = fr.clone();
        let i = fr2.walls.iter().position(|w| w.wall.id == wf.wall.id).unwrap();
        let s = fr2.walls[i].members.iter().find(|m| m.layout == Some(End::A) && m.kind == Kind::FieldStud).unwrap().s_in;
        fr2.field_point(i, s).unwrap();
        // A close-up of the plan: the north-west rooms by the corridor (building inches).
        let close = Some(cadaddy_geom::Bounds2::new(cadaddy_geom::Vec2::new(-24.0, 22.0 * 12.0), cadaddy_geom::Vec2::new(40.0 * 12.0, 40.0 * 12.0)));
        for (name, d, w) in [
            ("elevation.pdf", d, None),
            ("elevation-verified.pdf", draw_elevation(&fr2.walls[i]), None),
            ("plan.pdf", p.clone(), None),
            ("plan-close.pdf", p, close),
        ] {
            let pdf = cadaddy_io::pdf::pdf(&d, &cadaddy_doc::Space::Model, &opts(17.0, 11.0, w)).unwrap();
            std::fs::write(dir.join(name), pdf).unwrap();
        }
    }
}
