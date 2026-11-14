//! The submittal size reader and the boxes it sizes, on made-up data sheets (Q6655).

use super::*;

fn vocab() -> Vocab {
    Vocab::load()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

fn sizes(text: &str) -> Vec<[f64; 3]> {
    dims_in(text, &vocab()).iter().filter_map(Dims::size_in).collect()
}

#[test]
fn the_rules_data_holds_the_word_lists() {
    let v = vocab();
    assert!(v.not_unit.contains("SHIPPING") && v.not_unit.contains("CURB"), "{v:?}");
    assert!(v.accessory.iter().any(|w| w == "CONTROLLER"), "{v:?}");
    assert!(v.generic.contains("UNIT"), "{v:?}");
    assert!(super::super::provenance::rule(RULE).basis.contains("approved submittal"));
}

#[test]
fn labelled_sizes_are_read_in_every_usual_form() {
    // Labels before their values, on one line.
    assert_eq!(sizes("HEIGHT 31.89 WIDTH 37.24 DEPTH 16.14"), vec![[31.89, 37.24, 16.14]]);
    // With colons, units and the metric echo.
    assert_eq!(sizes("Height: 31.89 in (810)   Width: 37.24 in (946)   Depth: 16.14 in (410)"), vec![[31.89, 37.24, 16.14]]);
    assert_eq!(sizes("Height (in)   31.89(810)\nWidth (in)   37.24(946)\nDepth (in)   16.14(410)"), vec![[31.89, 37.24, 16.14]]);
    // Letters after the values.
    assert_eq!(sizes("Dimensions   4.72\" H x 3.39\" W x 1.06\" D"), vec![[4.72, 3.39, 1.06]]);
    assert_eq!(sizes("CABINET 31-7/8\"H X 37-1/4\"W X 16\"D"), vec![[31.875, 37.25, 16.0]]);
    // An order the line gives, then the values.
    assert_eq!(sizes("Dimensions (W x D x H): 37.24 x 16.14 x 31.89"), vec![[31.89, 37.24, 16.14]]);
    assert_eq!(sizes("UNIT SIZE (HxWxD)\n31.89x37.24x16.14 in."), vec![[31.89, 37.24, 16.14]]);
    // A header row over its values.
    assert_eq!(sizes("WIDTH   DEPTH   HEIGHT\n37.24   16.14   31.89"), vec![[31.89, 37.24, 16.14]]);
    // Values before their labels; a length and a width are the long and short sides.
    assert_eq!(sizes("81.75\" LENGTH, 54\" WIDTH, 48\" HEIGHT"), vec![[48.0, 81.75, 54.0]]);
    // Millimetres and feet-inches.
    let mm = sizes("Height 810 mm  Width 946 mm  Depth 410 mm");
    assert!(mm.len() == 1 && close(mm[0][0], 31.89) && close(mm[0][1], 37.244) && close(mm[0][2], 16.142), "{mm:?}");
    assert_eq!(sizes("HEIGHT 2'-8\" WIDTH 3'-1\" DEPTH 1'-4\""), vec![[32.0, 37.0, 16.0]]);
}

#[test]
fn what_is_not_the_units_size_is_not_read() {
    // A shipping carton's, a piping length, an unlabelled triple, watts, a size with no height.
    assert!(sizes("Shipping Height 36 Shipping Width 40 Shipping Depth 19").is_empty());
    assert!(sizes("Min. Piping Length 25 ft   Total Piping Length 98 ft").is_empty());
    assert!(sizes("Dimensions (in) 19-11/16 x 19-11/16 x 7-15/16").is_empty());
    assert!(sizes("HEATER 1500 W x 240 V").is_empty());
    assert!(sizes("WIDTH 37.24 DEPTH 16.14").is_empty());
    // A curb's line is the curb's.
    assert!(sizes("ROOF CURB - 49.5\" Width, 75\" Length, 30\" Supply Height").is_empty());
}

#[test]
fn a_curbs_height_is_read_and_a_weight_is_not() {
    let v = vocab();
    let c = curb_in("CURB & SUPPORTS\n  ROOF CURB - 49.5\" Width, 75\" Length, 30\" Supply Height, Insulated.", &v).unwrap();
    assert!(close(c.height_in, 30.0) && c.plan_in == Some([75.0, 49.5]), "{c:?}");
    assert!(close(curb_in("SET ON A 14\" HIGH ROOF CURB", &v).unwrap().height_in, 14.0));
    assert!(close(curb_in("CURB HEIGHT: 18", &v).unwrap().height_in, 18.0));
    assert_eq!(curb_in("Weight: 2110 lbs./146 curb", &v), None);
    assert_eq!(curb_in("NO CURB BY THIS SUPPLIER", &v), None);
}

#[test]
fn tags_are_matched_as_pages_write_them() {
    assert_eq!(canon("hp 9").as_deref(), Some("HP-9"));
    assert_eq!(canon("HP9").as_deref(), Some("HP-9"));
    assert_eq!(canon("MAU-4A").as_deref(), Some("MAU-4A"));
    assert_eq!(canon("12"), None);
    let t = tags_named("Unit Tags:   CU/AHU-6\nTag #: HP 9, ERV-2");
    for want in ["CU-6", "AHU-6", "HP-9", "ERV-2"] {
        assert!(t.contains(want), "{want}: {t:?}");
    }
}

fn unit(tag: &str, words: &[&str]) -> Unit {
    Unit { tag: tag.into(), words: words.iter().map(|w| w.to_string()).collect(), ..Unit::default() }
}

const PAGE: &str = "\n";

#[test]
fn a_tags_pages_give_it_their_size_and_an_accessorys_is_not_taken() {
    let text = [
        "--- page 1 of 3 ---\nSUBMITTAL COVER - PROJECT A\nX NO EXCEPTIONS TAKEN\nUnit Tags: HP-9",
        "--- page 2 of 3 ---\nOUTDOOR UNIT DATA\nHeight 31.89 Width 37.24 Depth 16.14",
        "--- page 3 of 3 ---\nWIRELESS CONTROLLER (OPTIONAL)\nDimensions   4.72\" H x 3.39\" W x 1.06\" D",
    ]
    .join(PAGE);
    let got = from_text(&text, "Made-up Heat Pumps.pdf", &[unit("HP-9", &["HEAT", "PUMP"])], &vocab());
    let s = got.get("HP-9").unwrap();
    assert_eq!(s.size_in, [31.89, 37.24, 16.14]);
    assert_eq!(s.source, "submittal Made-up Heat Pumps.pdf p.2 (no exceptions taken)");
    // A page naming another unit is that unit's.
    let text = format!("{text}\n--- page 4 of 4 ---\nTAG: CU-8\nHeight 50 Width 50 Depth 50");
    let got = from_text(&text, "x.pdf", &[unit("HP-9", &[]), unit("CU-8", &[])], &vocab());
    assert_eq!(got.get("HP-9").unwrap().size_in, [31.89, 37.24, 16.14]);
    assert_eq!(got.get("CU-8").unwrap().size_in, [50.0, 50.0, 50.0]);
}

#[test]
fn a_unit_named_by_its_model_number_and_one_of_two_on_a_page_by_its_words() {
    let mut u = unit("MAU-4", &["MAKEUP", "AIR"]);
    u.models.insert("QX-7200".into());
    let text = "--- page 1 of 1 ---\nMODEL QX-7200\nHeight 40 Width 90 Depth 60";
    assert_eq!(from_text(text, "a.pdf", &[u], &vocab()).get("MAU-4").unwrap().size_in, [40.0, 90.0, 60.0]);
    // A split system's two sections on one sheet: each unit by the words its schedule gives.
    let text = "--- page 1 of 1 ---\nTag: CU/AHU-6\nOUTDOOR CONDENSING SECTION\nHeight 31 Width 37 Depth 16\n\n\n\nINDOOR AIR HANDLER SECTION\nHeight 49 Width 21 Depth 22";
    let got = from_text(text, "a.pdf", &[unit("CU-6", &["CONDENSING", "OUTDOOR"]), unit("AHU-6", &["AIR", "HANDLER"])], &vocab());
    assert_eq!(got.get("CU-6").unwrap().size_in, [31.0, 37.0, 16.0]);
    assert_eq!(got.get("AHU-6").unwrap().size_in, [49.0, 21.0, 22.0]);
}

#[test]
fn a_submittal_sent_back_gives_nothing() {
    let text = "--- page 1 of 1 ---\nX REVISE AND RESUBMIT\nTAG HP-9\nHeight 31.89 Width 37.24 Depth 16.14";
    assert!(from_text(text, "a.pdf", &[unit("HP-9", &[])], &vocab()).is_empty());
}

fn equipment(tag: &str, kind: &str, min: [f64; 2], max: [f64; 2], h_ft: f64, outline: &str, height: &str) -> MepEquipment {
    let mut e = MepEquipment { tag: tag.into(), kind: kind.into(), level: "Roof".into(), min, max, height_ft: h_ft, ..MepEquipment::default() };
    e.sources.insert("outline".into(), outline.into());
    e.sources.insert("height".into(), height.into());
    e.properties.insert("STATUS".into(), "inferred".into());
    e
}

#[test]
fn a_box_takes_its_submittals_size_and_its_curb_and_keeps_the_schedules_beside_it() {
    // A rule-sized heat pump on its stand, a schedule-sized unit on its curb (12 units a foot).
    let mut hp = equipment(
        "HP-9",
        "heat_pump",
        [0.0, 0.0],
        [36.0, 36.0],
        3.0,
        "no outline by its tag: the usual size of its kind (rules data) at its tag",
        "rule: the usual height of its kind",
    );
    hp.bottom_ft = 1.5;
    hp.properties.insert("SUPPORT".into(), "equipment stand 1.50 ft".into());
    let mut stand = equipment("HP-9 STAND", "equipment_stand", [0.0, 0.0], [36.0, 36.0], 1.5, "", "M-ROOF-STAND");
    stand.properties.insert("UNIT".into(), "HP-9".into());
    let mut mau = equipment("MAU-4", "rtu", [100.0, 0.0], [210.7, 96.0], 4.5, "no outline by its tag: the schedule's size at its tag", "schedule");
    mau.bottom_ft = 14.0 / 12.0;
    mau.properties.insert("STATUS".into(), "derived".into());
    let mut curb = equipment("MAU-4 CURB", "roof_curb", [100.0, 0.0], [210.7, 96.0], 14.0 / 12.0, "", "M-ROOF-CURB");
    curb.properties.insert("UNIT".into(), "MAU-4".into());
    let mut eq = vec![hp, stand, mau, curb];
    let before = basis_counts(&eq);
    assert_eq!((before.get("rule"), before.get("schedule")), (Some(&1), Some(&1)));
    let mut got = BTreeMap::new();
    got.insert("HP-9".to_string(), Submitted { size_in: [31.89, 37.24, 16.14], curb: None, source: "submittal a.pdf p.2".into() });
    got.insert(
        "MAU-4".to_string(),
        Submitted {
            size_in: [48.0, 81.75, 54.0],
            curb: Some(Curb { height_in: 30.0, plan_in: None }),
            source: "submittal b.pdf p.3 (no exceptions taken)".into(),
        },
    );
    let a = apply(&mut eq, &got, 12.0);
    assert_eq!((a.resized.len(), a.curbs, a.changed.len()), (2, 1, 4), "{a:?}");
    let [hp, stand, mau, curb] = [&eq[0], &eq[1], &eq[2], &eq[3]];
    // The heat pump: 37.24 x 16.14 x 31.89 in about its centre, derived, its basis the page.
    assert!(close(hp.max[0] - hp.min[0], 37.24) && close(hp.max[1] - hp.min[1], 16.14) && close(hp.height_ft * 12.0, 31.89), "{hp:?}");
    assert!(close((hp.min[0] + hp.max[0]) / 2.0, 18.0) && close((hp.min[1] + hp.max[1]) / 2.0, 18.0));
    assert_eq!(hp.properties.get("SIZE_BASIS").map(String::as_str), Some("submittal a.pdf p.2"));
    assert_eq!(hp.properties.get("STATUS").map(String::as_str), Some("derived"));
    assert!(hp.properties.get("BASIS").is_some_and(|b| b.starts_with(RULE)), "{hp:?}");
    assert!(hp.corrections.is_empty(), "a rule's size is no design value: {:?}", hp.corrections);
    assert_eq!(size_basis(hp), "submittal");
    // Its stand follows it, at its own height (the submittal gives none).
    assert_eq!((stand.min, stand.max), (hp.min, hp.max));
    assert!(close(hp.bottom_ft, 1.5));
    // The unit: the substitution's size on a 30 in curb; the schedule's size kept as the design value.
    assert!(close(mau.max[0] - mau.min[0], 81.75) && close(mau.max[1] - mau.min[1], 54.0) && close(mau.height_ft, 4.0), "{mau:?}");
    assert!(close(curb.height_ft, 2.5) && close(mau.bottom_ft, 2.5), "{curb:?} {mau:?}");
    assert_eq!(curb.properties.get("STATUS").map(String::as_str), Some("derived"));
    let c = mau.corrections.iter().find(|c| c.key == "size").unwrap();
    assert!(c.design.starts_with("110.7 W x 96 D x 54 H in (the schedule's size)") && c.corrected == "81.75 W x 54 D x 48 H in", "{c:?}");
    assert!(c.reason.contains("b.pdf p.3") && c.rule.starts_with(RULE), "{c:?}");
    assert_eq!(mau.properties.get("SIZE_WAS").map(String::as_str), Some("110.7 W x 96 D x 54 H in (schedule)"));
    let after = basis_counts(&eq);
    assert_eq!(after.get("submittal"), Some(&2));
    // Again: nothing more changes; the boxes are their submittal's size.
    let again = apply(&mut eq, &got, 12.0);
    assert_eq!((again.resized.len(), again.confirmed), (0, 2));
}

#[test]
fn hostile_text_does_not_panic() {
    let v = vocab();
    for t in ["", "x", "X X X", "HEIGHT", "HEIGHT 1e999 WIDTH NaN DEPTH -4", "1/0 x 0/0 x 3", "(((", "''\"\"", "H:", "--- page ", "--- page x\n"] {
        let _ = dims_in(t, &v);
        let _ = curb_in(t, &v);
        let _ = from_text(t, "a.pdf", &[unit("HP-9", &[])], &v);
        let _ = tags_named(t);
    }
    let long = "HEIGHT 1 ".repeat(10_000);
    let _ = dims_in(&long, &v);
}

#[test]
fn a_file_is_read_when_its_name_calls_the_units_kind_or_its_pages_name_it() {
    let v = vocab();
    let eq = vec![MepEquipment { tag: "HP-9".into(), kind: "heat_pump".into(), title: "HEAT PUMP SCHEDULE".into(), ..MepEquipment::default() }];
    let us = units(&eq, &v);
    assert!(relevant("/subs/Split Systems/Rev 1-0/Submittal 0101.pdf", &us));
    assert!(relevant("/subs/Mechanical/Heat Pumps.pdf", &us));
    assert!(!relevant("/subs/Concrete Mix/Rev 1-0/Submittal 0101.pdf", &us));
    assert!(names_a_unit("--- page 1 of 2 ---\nUnit Tags: HP-9", &us));
    assert!(!names_a_unit("--- page 1 of 2 ---\nUnit Tags: HP-19", &us));
}
