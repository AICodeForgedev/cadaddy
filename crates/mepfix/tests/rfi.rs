//! The RFI list from a set's prints (Q296): made-up sets with planted gaps and conflicts (a tag
//! the legend doesn't have, a wall typed differently on an enlarged plan, a rated wall stopping
//! at the ceiling, a head of wall not given, walls stopping at the ceiling beside an open room or
//! under a two-layer ceiling, a ceiling with no height, callouts to missing details, unused
//! details, sleeves with no route or that won't fit) and the gaps every set has (untagged walls,
//! unscheduled ceilings, soffits with no callout). Every expected item is found, nothing else.

use cadaddy_mepfix::framing::JoistBay;
use cadaddy_mepfix::rfi::{self, PrintRead, SleeveRead, parse_sleeve};
use cadaddy_mepfix::{bore, generate, generate_with_gaps, truth_with_gaps};
use cadaddy_walk3d::{BuildingSpec, RfiKind, Severity};

/// Kinds only planted gaps produce (a clean set has none of them).
const PLANTED_ONLY: [RfiKind; 9] = [
    RfiKind::UnknownTag,
    RfiKind::SheetConflict,
    RfiKind::RatedToCeiling,
    RfiKind::HeadUnspecified,
    RfiKind::HeightVsCeiling,
    RfiKind::DetailMissing,
    RfiKind::SleeveNoRoute,
    RfiKind::SleeveNoFit,
    RfiKind::DoubleCeilingShortWall,
];

#[test]
fn planted_gaps_and_the_usual_ones_are_all_found_and_nothing_else() {
    let mut seen = std::collections::BTreeSet::new();
    for seed in 1..=6 {
        let (t, _, rfis) = rfi::read_fixture_rfis(seed, true).unwrap();
        assert!(t.planted.len() >= 10, "seed {seed}: {:?}", t.planted);
        let exp = rfi::expected(&t);
        for p in &t.planted {
            let k = RfiKind::parse(&p.kind).unwrap();
            assert!(exp.iter().any(|e| e.kind == k && e.truth == p.about), "seed {seed}: planted {p:?} not expected");
        }
        let s = rfi::score_rfis(&t, &rfis);
        assert!(s.missed.is_empty(), "seed {seed}: missed {:?}", s.missed);
        assert!(s.extra.is_empty(), "seed {seed}: extra {:?}", s.extra);
        assert_eq!(s.recall(), 1.0);
        seen.extend(rfis.iter().filter_map(|r| r.kind));
    }
    // (The gear, labels, grounding and concrete reviews' kinds come from `gear.review`, `labels.check`,
    // `grounding.check` and `concrete.review`, not the print gaps: engine tests/gear.rs, tests/labels.rs
    // and tests/concrete.rs.)
    for k in RfiKind::ALL.into_iter().filter(|k| {
        !matches!(
            k,
            RfiKind::GearLugs
                | RfiKind::GearInstall
                | RfiKind::LabelData
                | RfiKind::Grounding
                | RfiKind::PadUndecided
                | RfiKind::PourHold
                | RfiKind::EmbedConflict
        )
    }) {
        assert!(seen.contains(&k), "{k:?} never found on seeds 1–6");
    }
}

#[test]
fn clean_sets_have_only_the_usual_gaps() {
    for seed in 1..=3 {
        let (t, read, rfis) = rfi::read_fixture_rfis(seed, false).unwrap();
        assert!(t.planted.is_empty() && t.sleeve_notes.is_empty());
        assert!(read.enlarged.is_empty() && read.sleeves.is_empty());
        let s = rfi::score_rfis(&t, &rfis);
        assert!(s.missed.is_empty() && s.extra.is_empty(), "seed {seed}: {:?} {:?}", s.missed, s.extra);
        // The guessed wall types are there, and none of what only planted gaps produce.
        assert!(rfis.iter().any(|r| r.kind == Some(RfiKind::UntaggedWall)));
        assert!(rfis.iter().all(|r| !r.kind.is_some_and(|k| PLANTED_ONLY.contains(&k))), "seed {seed}");
    }
}

#[test]
fn a_gap_set_is_the_set_plus_its_gaps() {
    let seed = 2;
    let clean = generate(seed).unwrap();
    let gaps = generate_with_gaps(seed).unwrap();
    // The enlarged plan is added; every other sheet keeps its number and place.
    assert!(gaps.file("A-401.dxf").is_some() && clean.file("A-401.dxf").is_none());
    for sh in &clean.truth.sheets {
        let g = gaps.truth.sheets.iter().find(|x| x.number == sh.number).unwrap();
        assert_eq!(g.model_from_building, sh.model_from_building, "{}", sh.number);
    }
    // The truth without drawing anything plants the same gaps.
    let t = truth_with_gaps(seed);
    assert_eq!(t.planted, gaps.truth.planted);
    assert_eq!(t.plan_walls, gaps.truth.plan_walls);
    assert_eq!(t.wall_types, gaps.truth.wall_types);
    assert_eq!(t.head_details, gaps.truth.head_details);
    assert_eq!(t.rcp_soffits, gaps.truth.rcp_soffits);
    assert_eq!(t.soffit_types, gaps.truth.soffit_types);
    assert_eq!(t.sleeve_notes, gaps.truth.sleeve_notes);
    assert_eq!(t.building.rooms, gaps.truth.building.rooms);
    // Unplanted parts are as in the clean set.
    assert_eq!(gaps.truth.segments, clean.truth.segments);
    assert_eq!(gaps.truth.ceiling_types, clean.truth.ceiling_types);
}

#[test]
fn items_say_where_what_and_what_was_assumed() {
    let (_, _, rfis) = rfi::read_fixture_rfis(3, true).unwrap();
    for (i, r) in rfis.iter().enumerate() {
        assert_eq!(r.id, format!("RFI-{:03}", i + 1));
        assert!(!r.issue.is_empty() && !r.assumed.is_empty() && !r.sheets.is_empty(), "{r:?}");
        assert_eq!(r.severity, r.kind.unwrap().severity());
    }
    // Most severe first.
    let rank = |s: Severity| s as u8;
    assert!(rfis.windows(2).all(|w| rank(w[0].severity) <= rank(w[1].severity)));
    let item = |k: RfiKind| rfis.iter().find(|r| r.kind == Some(k)).unwrap_or_else(|| panic!("{k:?}"));
    let c = item(RfiKind::SheetConflict);
    assert_eq!(c.sheets, vec!["A-101", "A-401", "A-501"]);
    assert!(c.issue.contains("A-401 tags it"), "{}", c.issue);
    assert!(c.extent.is_some() && c.proposed.is_some());
    let f = item(RfiKind::SleeveNoFit);
    assert!(f.sheets.contains(&"S-102".to_string()), "{:?}", f.sheets);
    assert!(f.issue.contains("beam"), "{}", f.issue);
    assert!(f.proposed.as_deref().unwrap().starts_with("Move the sleeve"), "{:?}", f.proposed);
    let n = rfis.iter().filter(|r| r.kind == Some(RfiKind::SleeveNoRoute)).collect::<Vec<_>>();
    assert_eq!(n.len(), 2);
    assert!(n.iter().any(|r| r.issue.contains("no route or destination, no method")));
    assert!(n.iter().any(|r| r.issue.contains("gives no method")));
    let rated = item(RfiKind::RatedToCeiling);
    assert!(rated.about.starts_with("wall type ") && rated.issue.contains("HR"), "{rated:?}");
    let missing = item(RfiKind::DetailMissing);
    assert!(missing.issue.contains("/A-502"), "{}", missing.issue);
    let cu = item(RfiKind::CeilingUnspecified);
    assert!(cu.sheets.contains(&"A-601".to_string()));
}

#[test]
fn sleeve_notes_are_read_word_by_word() {
    let n = parse_sleeve("3\" SLEEVE").unwrap();
    assert_eq!((n.count, n.size_in, n.route, n.method), (1, Some(3.0), None, None));
    let n = parse_sleeve("(2) 2\" SLEEVES, CORE THRU SLAB TO LEVEL 1 CEILING").unwrap();
    assert_eq!(n.count, 2);
    assert_eq!(n.route.as_deref(), Some("LEVEL 1 CEILING"));
    assert_eq!(n.method.as_deref(), Some("CORE THRU SLAB"));
    let n = parse_sleeve("(4) 2\" ENT pathways bored thru 2x10 joists to corridor floor sleeves").unwrap();
    assert_eq!((n.count, n.size_in, n.raceway.as_deref()), (4, Some(2.0), Some("ENT")));
    assert_eq!(n.route.as_deref(), Some("CORRIDOR FLOOR SLEEVES"));
    assert_eq!(n.method.as_deref(), Some("BORED THRU 2X10 JOISTS"));
    let n = parse_sleeve("1-1/2\" SLEEVE TO DATA ROOM VIA SOFFIT").unwrap();
    assert_eq!(n.size_in, Some(1.5));
    assert_eq!(n.route.as_deref(), Some("DATA ROOM"));
    assert_eq!(n.method.as_deref(), Some("VIA SOFFIT"));
    assert!(parse_sleeve("SLEEVE").unwrap().size_in.is_none());
    assert!(parse_sleeve("2\" CW").is_none());
    // Hostile text.
    for s in ["", "(", "(999999999999999999999) SLEEVE", "\"\"\"\" SLEEVE TO TO TO", &"SLEEVE ".repeat(10_000), "SLEEVE\u{0}\u{FFFF} THRU"] {
        let _ = parse_sleeve(s);
    }
}

fn sleeve(text: &str, at: [f64; 2]) -> SleeveRead {
    SleeveRead { level: "2".into(), sheet: "A-102".into(), at_ft: at, text: text.into(), note: parse_sleeve(text).unwrap() }
}

#[test]
fn sleeves_through_joists_get_the_nearest_fitting_option() {
    // The field lesson: 3" sleeves through 2x10 joists don't fit; four 2" ENT do.
    let mut p = PrintRead::default();
    p.sleeves.push(sleeve("3\" SLEEVE, BORE THRU 2X10 FLOOR JOISTS TO CORRIDOR FLOOR SLEEVE", [10.0, 10.0]));
    // A 1" EMT through the same joists fits: nothing to ask.
    p.sleeves.push(sleeve("1\" EMT SLEEVE, BORE THRU 2X10 JOISTS TO CORRIDOR", [20.0, 10.0]));
    // The joists from a bay the sleeve is in (the note says only "joists").
    p.bays.push(JoistBay {
        id: "B1".into(),
        level: "2".into(),
        min_ft: [30.0, 0.0],
        max_ft: [50.0, 20.0],
        along_x: true,
        spacing_in: 16.0,
        width_in: 1.5,
        member: bore::Member::SawnJoist { depth_in: 7.25 },
    });
    p.sleeves.push(sleeve("2\" SLEEVE, BORE THRU JOISTS TO CORRIDOR", [40.0, 10.0]));
    // Trusses take no field holes.
    p.sleeves.push(sleeve("1\" SLEEVE, BORE THRU ROOF TRUSSES TO ATTIC", [60.0, 10.0]));
    let rfis = rfi::find_rfis(&p);
    let fits: Vec<_> = rfis.iter().filter(|r| r.kind == Some(RfiKind::SleeveNoFit)).collect();
    assert_eq!(fits.len(), 3, "{rfis:#?}");
    let lesson = fits.iter().find(|r| r.at == [10.0, 10.0]).unwrap();
    assert!(lesson.proposed.as_deref().unwrap().starts_with("Use 4 × 2\" ENT"), "{:?}", lesson.proposed);
    assert!(lesson.issue.contains("2x10") || lesson.issue.contains("9-1/4\" sawn joist"), "{}", lesson.issue);
    assert!(fits.iter().any(|r| r.at == [40.0, 10.0] && r.issue.contains("7-1/4\" sawn joist")));
    let truss = fits.iter().find(|r| r.at == [60.0, 10.0]).unwrap();
    assert!(truss.issue.contains("no field holes") && truss.proposed.as_deref().unwrap().contains("nothing standard fits"), "{truss:?}");
    // Every note here gives a route and a method.
    assert!(rfis.iter().all(|r| r.kind != Some(RfiKind::SleeveNoRoute)));
}

#[test]
fn the_list_goes_into_a_building_spec() {
    let (_, _, rfis) = rfi::read_fixture_rfis(1, true).unwrap();
    // Building feet → plan inches.
    let patch = rfi::spec_patch(&rfis, &|_, p| Some([p[0] * 12.0, p[1] * 12.0]));
    let mut v = serde_json::to_value(BuildingSpec::default()).unwrap();
    cadaddy_walk3d::merge(&mut v, &patch);
    let sp = BuildingSpec::from_json(&v.to_string()).unwrap();
    assert_eq!(sp.rfis.len(), rfis.len());
    assert!(sp.check().iter().all(|i| !i.path.starts_with("rfis")));
    let first = rfis.iter().find(|r| !r.level.is_empty()).unwrap();
    let back = sp.rfis.iter().find(|r| r.id == first.id).unwrap();
    assert_eq!(back.at, [first.at[0] * 12.0, first.at[1] * 12.0]);
    assert_eq!(back.sheets, first.sheets);
    // The readers' items keep their ids at the head of the list.
    let l = cadaddy_walk3d::rfi::list(&sp);
    assert_eq!(l.first().map(|r| r.id.as_str()), Some("RFI-001"));
    assert!(l.len() >= rfis.len());
}

#[test]
fn hostile_reads_never_panic() {
    assert!(rfi::find_rfis(&PrintRead::default()).is_empty());
    let mut p = PrintRead::default();
    for at in [[f64::NAN, 0.0], [f64::INFINITY, f64::NEG_INFINITY], [1e308, -1e308]] {
        let mut s = sleeve("4\" SLEEVE, CORE THRU SLAB TO LEVEL 1", at);
        s.note.size_in = Some(f64::NAN);
        p.sleeves.push(s);
        p.sleeves.push(sleeve("3\" SLEEVE BORE THRU 2X10 JOISTS TO X", at));
    }
    p.structure.beams.insert(
        "2".into(),
        vec![cadaddy_mepfix::struct_reader::DetectedBeam {
            size: "W99X999".into(),
            from_ft: [f64::NAN, 0.0],
            to_ft: [1e308, 1e308],
            grid: String::new(),
        }],
    );
    let _ = rfi::find_rfis(&p);
    let _ = rfi::spec_patch(&rfi::find_rfis(&p), &|_, _| None);
}
