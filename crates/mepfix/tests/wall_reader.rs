//! Wall assemblies from prints (Q293): the wall type legend read into layer stacks, every wall on
//! the floor plans typed from its tag (leaders followed) or a flagged guess, and its height from
//! the type's rule, the ceilings and the deck — scored against the fixture truth.

use cadaddy_mepfix::truth::Truth;
use cadaddy_mepfix::wall_reader::{self, WallResult, WallScore, read_fixture_walls, score_walls};
use cadaddy_mepfix::{truth_for, walls};

const SEEDS: std::ops::RangeInclusive<u64> = 1..=6;

fn scored(seed: u64) -> (Truth, WallResult, WallScore) {
    let (t, r) = read_fixture_walls(seed).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    let s = score_walls(&t, &r);
    (t, r, s)
}

#[test]
fn the_truth_has_wall_types_and_walls() {
    for seed in SEEDS {
        let t = truth_for(seed);
        assert_eq!(t.wall_types.len(), 7, "seed {seed}");
        let tags: std::collections::BTreeSet<&str> = t.wall_types.iter().map(|w| w.tag.as_str()).collect();
        assert_eq!(tags.len(), 7, "seed {seed}: tags are distinct");
        for wt in &t.wall_types {
            let sum: f64 = wt.layers.iter().filter(|l| !l.in_cavity).map(|l| l.thickness_in).sum();
            assert!((sum - wt.thickness_in).abs() < 1e-9, "seed {seed} {}: layers add up", wt.tag);
            assert!(["ceiling", "above_ceiling", "deck", "full"].contains(&wt.height.as_str()));
        }
        assert!(t.plan_walls.len() >= 20, "seed {seed}: {} walls", t.plan_walls.len());
        for w in &t.plan_walls {
            assert!(t.wall_types.iter().any(|x| x.tag == w.wall_type), "seed {seed} {}: its type is in the legend", w.id);
            let deck = walls::deck_ft(&t, &w.level);
            assert!(w.height_ft > 7.0 && w.height_ft <= deck + 1e-6, "seed {seed} {}: height {} (deck {deck})", w.id, w.height_ft);
            assert_eq!(w.tagged, !w.tags.is_empty(), "seed {seed} {}", w.id);
            assert!(!w.rooms.is_empty(), "seed {seed} {}: rooms beside it", w.id);
        }
        // Mixed heights, ratings and untagged walls somewhere.
        let heights: std::collections::BTreeSet<i64> = t.plan_walls.iter().map(|w| (w.height_ft * 12.0).round() as i64).collect();
        assert!(heights.len() >= 2, "seed {seed}: heights vary");
        assert!(t.wall_types.iter().any(|w| w.fire_rating.is_some()) && t.wall_types.iter().any(|w| w.stc.is_some()));
        assert_eq!(truth_for(seed).plan_walls, t.plan_walls, "deterministic");
    }
    assert!(SEEDS.clone().any(|s| truth_for(s).plan_walls.iter().any(|w| !w.tagged)), "some walls are untagged");
}

#[test]
fn the_generated_set_carries_the_same_walls() {
    let set = cadaddy_mepfix::generate(2).unwrap();
    assert_eq!(set.truth.plan_walls, truth_for(2).plan_walls);
    assert_eq!(set.truth.wall_types, truth_for(2).wall_types);
    for f in ["A-101.pdf", "A-102.dxf", "A-501.pdf", "A-501.dxf"] {
        assert!(set.file(f).is_some(), "{f}");
    }
}

#[test]
fn legend_layer_stacks_are_read() {
    for seed in SEEDS {
        let (t, r, s) = scored(seed);
        assert_eq!(s.types_found, s.types_total, "seed {seed}: every legend type found ({:?})", r.types.iter().map(|x| &x.tag).collect::<Vec<_>>());
        for tw in &t.wall_types {
            let f = r.types.iter().find(|x| x.tag == tw.tag);
            assert_eq!(f.map(|x| &x.layers), Some(&tw.layers), "seed {seed} {}: layer stack\n{f:#?}", tw.tag);
        }
        assert_eq!(s.stacks_correct, s.types_total, "seed {seed}");
        assert_eq!(s.heights_rule_correct, s.types_total, "seed {seed}: height rules");
        assert_eq!(s.ratings_correct, s.types_total, "seed {seed}: fire and STC ratings");
        assert_eq!(s.thickness_correct, s.types_total, "seed {seed}: thickness");
        for tw in &t.wall_types {
            let f = r.types.iter().find(|x| x.tag == tw.tag);
            assert_eq!(f.map(|x| x.external), Some(tw.external), "seed {seed} {}", tw.tag);
        }
    }
}

#[test]
fn walls_are_found_with_their_openings() {
    for seed in SEEDS {
        let (_, _, s) = scored(seed);
        assert_eq!(s.walls_found, s.walls_total, "seed {seed}: {s:?}");
        assert_eq!(s.openings_found, s.openings_total, "seed {seed}: door openings kept");
    }
}

#[test]
fn tagged_walls_get_their_type() {
    for seed in SEEDS {
        let (_, _, s) = scored(seed);
        assert!(s.type_accuracy() >= 0.95, "seed {seed}: {}/{} tagged walls typed right", s.tagged_correct, s.tagged_total);
    }
}

#[test]
fn untagged_walls_are_flagged_and_guessed() {
    let (mut total, mut flagged, mut right) = (0, 0, 0);
    for seed in SEEDS {
        let (_, _, s) = scored(seed);
        total += s.untagged_total;
        flagged += s.untagged_flagged;
        right += s.untagged_correct;
    }
    assert!(total > 0);
    assert_eq!(flagged, total, "every untagged wall is flagged as a guess");
    assert!(right as f64 >= 0.85 * total as f64, "{right}/{total} guessed right");
}

#[test]
fn heights_follow_the_rules() {
    for seed in SEEDS {
        let (_, _, s) = scored(seed);
        assert!(
            s.height_correct as f64 >= 0.85 * s.walls_total as f64,
            "seed {seed}: {}/{} heights within 1\" (MAE {:.3}')",
            s.height_correct,
            s.walls_total,
            s.height_mae_ft
        );
        assert!(s.height_mae_ft < 0.25, "seed {seed}: MAE {:.3}'", s.height_mae_ft);
    }
}

#[test]
fn the_legend_wording_varies_and_parses() {
    // Every phrasing the generator uses reads back to the same stack.
    let lines = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let a = wall_reader::parse_type(
        "1B",
        &lines(&[
            "1 HOUR RATED CORRIDOR PARTITION",
            "SIDE A: TWO LAYERS 5/8\" TYPE 'X' GWB",
            "SIDE B: 1/2\" RESILIENT CHANNEL, 1 LAYER 5/8\" GWB",
            "3 5/8\" MTL. STUDS, 20 GA., 16\" O.C.",
            "3 1/2\" ACOUSTIC BATT INSUL.",
            "EXTEND TO CEILING + 6\"",
            "1-HOUR RATED ASSEMBLY",
            "STC: 50",
            "OVERALL: 6 1/8\"",
        ]),
    );
    let m: Vec<(&str, f64, &str)> = a.layers.iter().map(|l| (l.material.as_str(), l.thickness_in, l.side.as_str())).collect();
    assert_eq!(
        m,
        vec![
            ("gyp_x", 0.625, "1"),
            ("gyp_x", 0.625, "1"),
            ("stud", 3.625, "core"),
            ("insulation", 3.5, "core"),
            ("gyp", 0.625, "2"),
            ("resilient_channel", 0.5, "2")
        ]
    );
    assert_eq!((a.height.as_deref(), a.fire_rating.as_deref(), a.stc, a.printed_in), (Some("above_ceiling"), Some("1 HR"), Some(50), Some(6.125)));
    assert_eq!(a.layers.get(2).and_then(|l| l.gauge), Some(20));
    assert_eq!(a.layers.get(2).and_then(|l| l.spacing_in), Some(16.0));
    let b = wall_reader::parse_type(
        "W-4",
        &lines(&[
            "PLUMBING WALL",
            "(1) LAYER 5/8\" MR GYP. BD. EACH SIDE",
            "6\" 20 GA. METAL STUDS @ 16\" O.C.",
            "HEIGHT: TO UNDERSIDE OF DECK",
            "FIRE RATING: NONE",
        ]),
    );
    assert_eq!(b.layers.iter().filter(|l| l.material == "gyp_mr").count(), 2);
    assert_eq!((b.height.as_deref(), b.fire_rating.as_ref(), b.thickness_in()), (Some("deck"), None, 7.25));
    // Sawn lumber: the nominal size gives the depth, the spacing stays the spacing.
    for (line, depth) in [("2X4 WOOD STUDS @ 16\" O.C.", 3.5), ("2X6 WOOD STUDS @ 16\" O.C.", 5.5), ("2x8 WOOD STUDS @ 16\" O.C.", 7.25)] {
        let w = wall_reader::parse_type("W1", &lines(&["WOOD STUD PARTITION", "(1) LAYER 5/8\" GYP. BD. EACH SIDE", line]));
        let studs: Vec<(f64, Option<f64>, Option<u32>)> =
            w.layers.iter().filter(|l| l.material == "stud").map(|l| (l.thickness_in, l.spacing_in, l.gauge)).collect();
        assert_eq!(studs, vec![(depth, Some(16.0), None)], "{line}");
        assert!(w.wood_studs, "{line}");
        assert!(w.notes.is_empty(), "{line}: {:?}", w.notes);
        assert_eq!(w.thickness_in(), depth + 1.25, "{line}");
    }
    // Spacing written before `O.C.` without `@` is never the depth either.
    let oc = wall_reader::parse_type("S1", &lines(&["PARTITION", "6\" 20 GA. STUDS 24\" O.C."]));
    assert_eq!(oc.layers.first().map(|l| (l.thickness_in, l.spacing_in, l.gauge)), Some((6.0, Some(24.0), Some(20))));
    assert!(!oc.wood_studs);
    // Inches as `IN.`, `INCH` and `″` read like `"`.
    for (line, th) in [
        ("SIDE 1: 1 LAYER 5/8 IN. TYPE X GYP. BD.", 0.625),
        ("SIDE 1: 1 LAYER 5/8 INCH TYPE X GYP. BD.", 0.625),
        ("SIDE 1: 1 LAYER 5/8\u{2033} TYPE X GYP. BD.", 0.625),
        ("SIDE 1: 1 LAYER 1/2 INCHES TYPE X GYP. BD.", 0.5),
        ("SIDE 1: 1 LAYER 5/8IN. TYPE X GYP. BD.", 0.625),
    ] {
        let x = wall_reader::parse_type("X1", &lines(&["PARTITION", line, "3 5/8 IN. 20 GA. STUDS @ 16 INCH O.C."]));
        let m: Vec<(&str, f64, &str, Option<f64>)> =
            x.layers.iter().map(|l| (l.material.as_str(), l.thickness_in, l.side.as_str(), l.spacing_in)).collect();
        assert_eq!(m, vec![("gyp_x", th, "1", None), ("stud", 3.625, "core", Some(16.0))], "{line}");
        assert!(x.notes.is_empty(), "{line}: {:?}", x.notes);
    }
    let batt = wall_reader::parse_type("X2", &lines(&["PARTITION", "3-5/8 INCH STUDS @ 16\" O.C.", "5 1/2\u{2033} BATT INSULATION"]));
    assert_eq!(batt.layers.iter().map(|l| (l.material.as_str(), l.thickness_in)).collect::<Vec<_>>(), vec![("stud", 3.625), ("insulation", 5.5)]);
    // A stud or batt line with no size: the default is said in a note.
    let bare = wall_reader::parse_type("X3", &lines(&["PARTITION", "METAL STUDS @ 16\" O.C.", "ACOUSTIC BATT INSULATION"]));
    assert_eq!(
        bare.layers.iter().map(|l| (l.material.as_str(), l.thickness_in, l.spacing_in)).collect::<Vec<_>>(),
        vec![("stud", 3.625, Some(16.0)), ("insulation", 3.5, None)]
    );
    assert_eq!(bare.notes, vec!["stud size not given: 3-5/8\" assumed".to_string(), "insulation thickness not given: 3-1/2\" assumed".to_string()]);
    // Words that only start like an inch mark are not one.
    assert!(
        wall_reader::parse_type("X4", &lines(&["PARTITION", "3 5/8\" STUDS", "SEE 2 INCHWORM NOTE"])).notes
            == vec!["SEE 2 INCHWORM NOTE".to_string()]
    );
    // Hostile text never panics.
    for junk in [
        "\"\"\"",
        "1/0\"",
        "99999999999999999999\" STUD",
        "-/-\" CMU",
        "(999) LAYERS 5/8\" GYP",
        "SIDE 1: ,,,,",
        "STC 1e309",
        "99X99 STUDS",
        "2X STUDS",
        "X6 STUDS @",
        "\u{2033}\u{2033}",
        "IN. INCH",
        "5/8 IN",
        "@@@ 16\" O.C. O.C.",
    ] {
        let _ = wall_reader::parse_type("A", &lines(&["X", junk]));
    }
}

#[test]
fn the_spec_patch_is_a_valid_building_spec() {
    let (t, r) = read_fixture_walls(3).unwrap();
    let patch = wall_reader::spec_patch(&r, &|level, p| (level == "1").then_some([p[0] * 12.0, p[1] * 12.0]));
    let mut spec = serde_json::json!({"version": 1, "levels": [{"name": "1", "elevation_ft": 0.0, "height_ft": 14.0, "ceiling_ft": 9.0}]});
    cadaddy_walk3d::merge(&mut spec, &patch);
    let sp = cadaddy_walk3d::BuildingSpec::from_json(&spec.to_string()).unwrap_or_else(|e| panic!("{e}\n{spec:#}"));
    let errors: Vec<_> = sp.check().into_iter().filter(|i| i.error).collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(sp.wall_types.len(), t.wall_types.len());
    let level1 = r.walls.iter().filter(|w| w.level == "1" && w.wall_type.is_some()).count();
    assert_eq!(sp.walls.len(), level1);
    assert!(sp.walls.iter().any(|w| w.guess.is_some()) == r.walls.iter().any(|w| w.level == "1" && w.guess.is_some()));
    for wt in &sp.wall_types {
        let a = wt.assembly.as_ref().unwrap();
        assert!((a.thickness_in() - wt.thickness_in).abs() < 1e-9, "{}", wt.name);
        assert!(!a.layers.is_empty());
    }
    assert!(sp.describe().iter().any(|d| d.key == "walls"));
}

#[test]
fn overall_scores() {
    for seed in SEEDS {
        let (_, _, s) = scored(seed);
        println!(
            "seed {seed}: types {}/{} stacks {} | walls {}/{} openings {}/{} | tagged {}/{} untagged {} flagged {} right {} | heights {}/{} MAE {:.3}'",
            s.types_found,
            s.types_total,
            s.stacks_correct,
            s.walls_found,
            s.walls_total,
            s.openings_found,
            s.openings_total,
            s.tagged_correct,
            s.tagged_total,
            s.untagged_total,
            s.untagged_flagged,
            s.untagged_correct,
            s.height_correct,
            s.walls_total,
            s.height_mae_ft
        );
    }
}

#[test]
fn what_it_writes_says_where_it_came_from() {
    // Seed 3: every mark names its plan sheet, tagged marks are `drawing`, untagged `guess` with
    // a reason; every type names the legend sheet and its tag.
    let set = cadaddy_mepfix::generate(3).unwrap();
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let legend: Vec<&str> = map
        .sheets
        .iter()
        .filter(|s| s.title.to_ascii_uppercase().contains("WALL TYPE") || s.title.to_ascii_uppercase().contains("PARTITION"))
        .map(|s| s.number.as_str())
        .collect();
    let plans: Vec<(&str, &str)> = map
        .sheets
        .iter()
        .filter(|s| s.discipline == "A" && s.title.to_ascii_uppercase().contains("FLOOR PLAN"))
        .filter_map(|s| Some((s.level.as_deref()?, s.number.as_str())))
        .collect();
    assert_eq!(legend.len(), 1, "{legend:?}");
    let legend = legend[0];
    let (_, r) = read_fixture_walls(3).unwrap();
    assert!(r.types.iter().all(|t| t.sheet == legend), "{:?}", r.types.iter().map(|t| &t.sheet).collect::<Vec<_>>());
    let patch = wall_reader::spec_patch(&r, &|_, p| Some([p[0] * 12.0, p[1] * 12.0]));
    for wt in patch["wall_types"].as_array().unwrap() {
        let tag = wt["assembly"]["tag"].as_str().unwrap();
        assert_eq!(wt["assembly"]["source"].as_str(), Some(format!("{legend} wall type legend, tag {tag}").as_str()), "{wt}");
    }
    let marks = patch["walls"].as_array().unwrap();
    assert_eq!(marks.len(), r.walls.iter().filter(|w| w.wall_type.is_some()).count());
    let typed = r.walls.iter().filter(|w| w.wall_type.is_some());
    let (mut drawing, mut guessed) = (0, 0);
    for (m, w) in marks.iter().zip(typed) {
        let sheet = plans.iter().find(|(lv, _)| *lv == w.level).map(|(_, n)| *n).unwrap_or_else(|| panic!("no plan for level {}", w.level));
        assert_eq!(w.sheet, sheet);
        let source = m["source"].as_str().unwrap();
        assert!(source.starts_with(sheet), "{m}");
        assert_eq!(m["height_source"].as_str(), Some(w.height_source.as_str()), "{m}");
        assert!(!w.height_source.is_empty() && w.height_source != "unknown", "{m}");
        if w.tagged {
            drawing += 1;
            assert_eq!((m["status"].as_str(), source), (Some("drawing"), sheet), "{m}");
            assert!(m.get("guess").is_none(), "{m}");
        } else {
            guessed += 1;
            assert_eq!(m["status"].as_str(), Some("guess"), "{m}");
            assert!(m["guess"].as_str().is_some_and(|g| g.starts_with("no tag")), "{m}");
            assert!(source == format!("{sheet} (no tag)") || source.starts_with(&format!("{sheet} (tag ")), "{m}");
        }
    }
    assert!(drawing > 0 && guessed > 0, "{drawing} {guessed}");
    // Into a spec: the fields read back, and describe() counts by status.
    let mut spec = serde_json::json!({"version": 1, "levels": [{"name": "1", "elevation_ft": 0.0, "height_ft": 14.0, "ceiling_ft": 9.0}]});
    let one =
        serde_json::json!({"wall_types": patch["wall_types"], "walls": marks.iter().filter(|m| m["level"] == "1").cloned().collect::<Vec<_>>()});
    cadaddy_walk3d::merge(&mut spec, &one);
    let sp = cadaddy_walk3d::BuildingSpec::from_json(&spec.to_string()).unwrap_or_else(|e| panic!("{e}"));
    assert!(sp.wall_types.iter().all(|t| t.assembly.as_ref().is_some_and(|a| a.source.starts_with(legend))));
    let n_guess = sp.walls.iter().filter(|w| w.status == Some(cadaddy_walk3d::MarkStatus::Guess)).count();
    let n_drawing = sp.walls.iter().filter(|w| w.status == Some(cadaddy_walk3d::MarkStatus::Drawing)).count();
    assert_eq!(n_guess + n_drawing, sp.walls.len());
    assert!(sp.walls.iter().all(|w| !w.source.is_empty() && !w.height_source.is_empty()));
    let d = sp.describe();
    let walls = d.iter().find(|x| x.key == "walls").map(|x| x.text.clone()).unwrap_or_default();
    assert_eq!(walls, format!("{} walls: {n_drawing} typed from their tags on the plans (drawing), {n_guess} guessed (no tag)", sp.walls.len()));
    let first = sp.wall_types.first().map(|t| t.name.clone()).unwrap_or_default();
    assert!(d.iter().any(|x| x.key == format!("wall_types/{first}") && x.text.contains(&format!("from {legend} wall type legend"))));
    // An older spec (no source/status) reads unchanged: a mark with a guess counts as a guess.
    let old = cadaddy_walk3d::BuildingSpec::from_json(
        &serde_json::json!({"version": 1, "levels": [{"name": "1", "elevation_ft": 0.0, "height_ft": 14.0, "ceiling_ft": 9.0}],
            "wall_types": [{"name": "A", "thickness_in": 4.875}],
            "walls": [{"level": "1", "a": [0, 0], "b": [120, 0], "wall_type": "A"}, {"level": "1", "a": [0, 0], "b": [0, 120], "wall_type": "A", "guess": "no tag"}]})
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        old.walls.iter().map(|w| (w.status, w.status(), w.source.as_str())).collect::<Vec<_>>(),
        vec![(None, cadaddy_walk3d::MarkStatus::Drawing, ""), (None, cadaddy_walk3d::MarkStatus::Guess, "")]
    );
    assert!(!old.to_json().contains("status") && !old.to_json().contains("source"));
}
