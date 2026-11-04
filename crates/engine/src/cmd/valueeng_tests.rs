use super::*;

/// Section 26 05 19's wiring-methods article as a PDF's text reads (an item across a page break,
/// its running header and footer in the middle).
const SPEC: &str = "PART 3 - EXECUTION\n3.1\nCONDUCTOR MATERIAL APPLICATIONS\nA.\nFeeders: Copper.\n3.2\nCONDUCTOR INSULATION AND MULTICONDUCTOR CABLE APPLICATIONS AND\nWIRING METHODS\nA.\nService Entrance: Type THHN/THWN-2, single conductors in raceway.\nB.\nExposed Feeders: Type THHN/THWN-2, single conductors in raceway.\nC.\nFeeders \nConcealed \nin \nCeilings, \nWalls, \nPartitions, \nand \nCrawlspaces: \nType THHN/THWN-2, single conductors in raceway.\nD.\nFeeders Concealed in Concrete, below Slabs-on-Grade, and Underground: \nType THHN/THWN-2, single conductors in raceway.\nF.\nBranch Circuits Concealed in Ceilings, Walls, and Partitions: Type THHN/THWN-2, \nsingle conductors in raceway, Armored cable, Type AC, Metal-clad cable, Type MC, \nLOW-VOLTAGE ELECTRICAL POWER CONDUCTORS AND CABLES\n260519 - 5\nNonmetallic-sheathed cable, Type NM. Type NM is preferred.\n3.3\nINSTALLATION OF CONDUCTORS AND CABLES\nA.\nConceal cables.";

/// The same table in another layout: one line per item, the letter on the item's line, the
/// methods in another order and wording.
const SPEC_B: &str = "3.4 WIRING METHODS\nA. Branch circuits concealed in walls and ceilings: Metal-clad cable, Type MC, or conductors in raceway.\nB. Exposed branch circuits: Conductors in raceway.\nC. Feeders concealed in walls and ceilings: Service-entrance cable, Type SER, or conductors in raceway.\n3.5 INSTALLATION";

#[test]
fn a_specs_wiring_methods_table_says_each_applications_methods() {
    let apps = wiring_applications(SPEC, "26 05 19");
    assert_eq!(apps.len(), 5, "{apps:#?}");
    let c = &apps[2];
    assert_eq!((c.item.as_str(), c.application.as_str()), ("C", "Feeders Concealed in Ceilings, Walls, Partitions, and Crawlspaces"));
    assert_eq!(c.methods, ["raceway"]);
    assert_eq!(c.text, "Type THHN/THWN-2, single conductors in raceway.");
    let f = &apps[4];
    assert_eq!(f.methods, ["ac", "mc", "nm", "raceway"]);
    assert!(!f.text.contains("LOW-VOLTAGE"), "the running header is not the item's text: {}", f.text);
    assert!(wiring_applications("3.2\nSOMETHING ELSE\nA.\nX: Type NM.", "26 05 19").is_empty());
    let b = wiring_applications(SPEC_B, "26 05 19");
    assert_eq!(b.len(), 3, "{b:#?}");
    assert_eq!(b[0].methods, ["mc", "raceway"]);
    assert_eq!(b[2].methods, ["raceway", "se"]);
}

#[test]
fn texts_of_any_heights_sort_into_reading_order_without_panicking() {
    // Texts of many heights (a comparator keyed on one text's height isn't a total order and made
    // the sort panic on a real code sheet).
    let item = |x: f64, y: f64, h: f64, v: &str| cadaddy_mepfix::sheet_facts::TextItem {
        value: v.to_string(),
        at: cadaddy_geom::Vec2::new(x, y),
        height: h,
        rotation: 0.0,
    };
    let mut texts: Vec<cadaddy_mepfix::sheet_facts::TextItem> =
        (0..400).map(|i| item(f64::from(i * 37 % 101), f64::from(i * 53 % 97), [0.5, 3.0, 9.0, 1e-9, 40.0][i as usize % 5], "x")).collect();
    texts.push(item(f64::NAN, 1.0, 1.0, "nan"));
    texts.push(item(0.0, 500.0, 2.0, "CONSTRUCTION TYPE"));
    texts.push(item(30.0, 500.2, 2.0, "V-A"));
    let words = reading_order(texts);
    assert_eq!(words.len(), 402);
    assert_eq!(&words[..2], ["CONSTRUCTION TYPE", "V-A"]);
    assert_eq!(construction_type(&words).as_deref(), Some("V-A"));
}

#[test]
fn the_code_sheet_gives_the_construction_type() {
    let t = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(construction_type(&t(&["CONSTRUCTION TYPE                 V-A"])).as_deref(), Some("V-A"));
    assert_eq!(construction_type(&t(&["CONSTRUCTION TYPE PER TABLE 601", "TYPE V-A"])).as_deref(), Some("V-A"));
    assert_eq!(construction_type(&t(&["TYPE OF CONSTRUCTION: II-B"])).as_deref(), Some("II-B"));
    assert_eq!(construction_type(&t(&["OCCUPANCY R-1"])), None);
}

#[test]
fn conductors_are_checked_at_the_alternatives_column_with_bundling_and_upsized_when_short() {
    let r = rules();
    // Aluminum 2/0 at 60 °C is 115 A: a 125 A breaker is the next standard size up (240.4(B)).
    assert_eq!(protected(&r, true, "2/0", "60", 125.0, 1.0), (true, Some(115.0)));
    // 150 A is not: 3/0 (130 A, next size 150) is.
    assert!(!protected(&r, true, "2/0", "60", 150.0, 1.0).0);
    assert_eq!(upsize(&r, true, "#2/0", "60", 150.0, 1.0).as_deref(), Some("3/0"));
    // #12 copper is held to 20 A whatever its column (240.4(D)).
    assert!(protected(&r, false, "#12", "60", 20.0, 1.0).0);
    assert!(!protected(&r, false, "12", "75", 25.0, 1.0).0);
    assert_eq!(upsize(&r, false, "12", "60", 25.0, 1.0).as_deref(), Some("10"));
    // Bundling (334.80): three 2-wire cables through one hole are 6 conductors (80 %); the
    // 90 °C rating derated (30 × 0.8 = 24 A) never above the 60 °C column (20 A).
    assert_eq!(bundling_factor(&r, 2), 1.0);
    assert_eq!(bundling_factor(&r, 3), 0.8);
    assert_eq!(protected(&r, false, "12", "60", 20.0, bundling_factor(&r, 3)), (true, Some(20.0)));
    // Six cables (12 conductors, 50 %): #12 is 15 A, too little for 20 A; #10 makes 20 A.
    assert!(!protected(&r, false, "12", "60", 20.0, bundling_factor(&r, 6)).0);
    assert_eq!(upsize(&r, false, "12", "60", 20.0, bundling_factor(&r, 6)).as_deref(), Some("10"));
    // Sizes the tables don't have are not guessed.
    assert_eq!(protected(&r, false, "750", "60", 400.0, 1.0), (false, None));
    assert_eq!(upsize(&r, false, "nonsense", "60", 20.0, 1.0), None);
}

#[test]
fn every_alternative_cites_the_code_and_names_where_it_is_never_used() {
    let r = rules();
    assert!(r.alternatives.len() >= 5);
    for a in &r.alternatives {
        assert!(!a.code.is_empty() && a.code.iter().all(|c| c.cite.starts_with("NEC ")), "{}", a.id);
        assert!(!a.conditions.is_empty() && !a.excluded_areas.is_empty(), "{}", a.id);
        assert!(a.excluded_areas.keys().all(|k| r.areas.contains_key(k)), "{}: an excluded area the rules don't classify", a.id);
        assert!(a.intents.iter().all(|i| r.intents.iter().any(|x| &x.id == i)), "{}", a.id);
        assert!(super::super::mepatoms::rules().cables.contains_key(&a.cable), "{}: the atom rules have no {} cable", a.id, a.cable);
        assert!(["60", "75", "90"].contains(&a.ampacity_column.as_str()), "{}", a.id);
    }
    // No job's words in the rules: methods, places and codes only.
    let text = RULES.to_ascii_uppercase();
    for w in ["HOTEL", "QUEEN", "KING", "SUITE"] {
        assert!(!text.contains(w), "{w}");
    }
}
