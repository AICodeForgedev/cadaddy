//! Equipment from schedules (Q262): the schedules read off the schedule sheets, each tagged item
//! placed as a sized box at its tag on the plans and joined to the runs that end at it, scored
//! against the fixtures' truth; plus hand-made sheets for kinds the fixtures don't draw (a rooftop
//! unit, fans, pumps) and hostile drawings.

use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, Line, LwPolyline, Space, Text, VAlign};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use cadaddy_mepfix::equip_reader::{
    self, EquipmentRead, SheetIn, kind_from_tag, kind_from_title, parse_height_note, parse_length_ft, parse_size, read_tables, scheduled, score,
    size_depth_in,
};
use cadaddy_mepfix::generate;
use cadaddy_mepfix::truth::{Level, Sheet, Truth};

const SEEDS: std::ops::RangeInclusive<u64> = 1..=8;

fn read(seed: u64) -> (Truth, EquipmentRead) {
    let set = generate(seed).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    let r = equip_reader::read_fixture_set(&set);
    (set.truth, r)
}

#[test]
fn schedules_are_read_cell_for_cell() {
    for seed in SEEDS {
        let (t, r) = read(seed);
        let sc = score(&r, &t);
        assert_eq!(sc.scheduled_found, sc.truth_tags, "seed {seed}: every scheduled tag");
        assert!(sc.kind_wrong.is_empty(), "seed {seed}: kinds {:?}", sc.kind_wrong);
        assert!(sc.cell_accuracy >= 0.99, "seed {seed}: cells {:.3}", sc.cell_accuracy);
        // Tables of things that aren't equipment are read but give no equipment.
        assert!(r.tables.iter().any(|t| t.title.contains("AIR DEVICE")), "seed {seed}: the air device schedule is a table");
        assert!(
            !r.scheduled.iter().any(|s| s.tag.starts_with("CD-") || s.tag.starts_with("WC-")),
            "seed {seed}: devices and fixtures aren't equipment"
        );
        // Every kind the set has, from its schedule's title.
        for k in ["ahu", "vav", "exhaust_fan", "condensing_unit", "water_heater", "gas_meter"] {
            assert!(r.scheduled.iter().any(|s| s.kind == k && s.kind_from == "title"), "seed {seed}: no {k}");
        }
        let ahu = r.scheduled.iter().find(|s| s.tag == "AHU-1").unwrap();
        let truth = t.equipment.iter().find(|e| e.tag == "AHU-1").unwrap();
        let [l, w, h] = ahu.size_ft.unwrap();
        let size = [truth.max_ft[0] - truth.min_ft[0], truth.max_ft[1] - truth.min_ft[1], truth.max_ft[2] - truth.min_ft[2]];
        assert!(
            (l - size[0]).abs() < 0.05 && (w - size[1]).abs() < 0.05 && (h - size[2]).abs() < 0.05,
            "seed {seed}: AHU size {l} {w} {h} vs {size:?}"
        );
        assert_eq!(ahu.cfm, truth.cfm, "seed {seed}: AHU CFM");
        let wh = r.scheduled.iter().find(|s| s.kind == "water_heater").unwrap();
        assert!(wh.round && wh.size_ft.is_some_and(|s| (s[2] - 5.0).abs() < 1e-9), "seed {seed}: the heater is a 5' tall cylinder");
    }
}

#[test]
fn equipment_is_placed_at_its_tag_at_size() {
    for seed in SEEDS {
        let (t, r) = read(seed);
        let sc = score(&r, &t);
        assert_eq!(sc.placed, sc.expected_placed, "seed {seed}: placed {} of {} (unplaced {:?})", sc.placed, sc.expected_placed, r.unplaced);
        assert!(sc.placed_wrong.is_empty(), "seed {seed}: wrong {:?}", sc.placed_wrong);
        assert!(sc.centre_err_max_ft < 0.25, "seed {seed}: centre off by {:.2} ft", sc.centre_err_max_ft);
        assert!(sc.footprint_iou_mean > 0.97, "seed {seed}: footprints {:.3}", sc.footprint_iou_mean);
        assert!(sc.z_err_mean_ft < 0.5, "seed {seed}: heights off by {:.2} ft on average", sc.z_err_mean_ft);
        // Standing kinds stand on their level, at their printed height.
        for p in r.placed.iter().filter(|p| ["ahu", "water_heater", "condensing_unit"].contains(&p.kind.as_str())) {
            let e = t.equipment.iter().find(|e| e.tag == p.tag).unwrap();
            assert!(
                (p.min_ft[2] - e.min_ft[2]).abs() < 1e-6 && (p.max_ft[2] - e.max_ft[2]).abs() < 0.05,
                "seed {seed}: {} z {:?} vs {:?}",
                p.tag,
                p.min_ft,
                e.min_ft
            );
            assert_eq!(
                (p.footprint_from.as_str(), p.height_from.as_str(), p.elevation_from.as_str()),
                ("outline", "schedule", "floor"),
                "seed {seed}: {}",
                p.tag
            );
        }
        // The roof fan has no plan in the set: it is reported, not guessed.
        assert!(r.unplaced.iter().any(|(tag, why)| tag == "EF-1" && why.contains("plan")), "seed {seed}: {:?}", r.unplaced);
    }
}

#[test]
fn equipment_is_joined_to_its_runs() {
    let (mut recall, mut precision) = (0.0, 0.0);
    for seed in SEEDS {
        let (t, r) = read(seed);
        let sc = score(&r, &t);
        eprintln!("seed {seed}: systems recall {:.2} precision {:.2}", sc.system_recall, sc.system_precision);
        assert!(sc.system_recall >= 0.75 && sc.system_precision >= 0.85, "seed {seed}: {:.2} / {:.2}", sc.system_recall, sc.system_precision);
        recall += sc.system_recall;
        precision += sc.system_precision;
        // Every terminal box is on supply air.
        let vavs: Vec<_> = r.placed.iter().filter(|p| p.kind == "vav").collect();
        let on_supply = vavs.iter().filter(|p| p.systems() == ["SA"]).count();
        assert!(on_supply * 10 >= vavs.len() * 9, "seed {seed}: {on_supply} of {} VAV boxes on SA only", vavs.len());
        // The air handlers take supply and return air.
        for p in r.placed.iter().filter(|p| p.kind == "ahu") {
            assert!(p.systems().contains(&"SA".to_string()), "seed {seed}: {} {:?}", p.tag, p.systems());
        }
        // The gas meter feeds gas; the condensing unit's refrigerant lines aren't drawn.
        assert_eq!(r.placed.iter().find(|p| p.kind == "gas_meter").map(|p| p.systems()), Some(vec!["G".to_string()]), "seed {seed}");
        assert_eq!(r.placed.iter().find(|p| p.kind == "condensing_unit").map(|p| p.connections.len()), Some(0), "seed {seed}");
    }
    let n = SEEDS.count() as f64;
    assert!(recall / n >= 0.85 && precision / n >= 0.93, "mean systems recall {:.2}, precision {:.2}", recall / n, precision / n);
}

#[test]
fn every_guess_is_flagged() {
    for seed in SEEDS {
        let (_, r) = read(seed);
        for p in &r.placed {
            assert!(["outline", "tag", "rule"].contains(&p.footprint_from.as_str()), "{}", p.tag);
            assert!(["schedule", "rule"].contains(&p.height_from.as_str()), "{}", p.tag);
            assert!(["floor", "run", "rule"].contains(&p.elevation_from.as_str()), "{}", p.tag);
            assert!((0.0..=1.0).contains(&p.confidence), "{}", p.tag);
            if p.height_from == "rule" {
                assert!(p.flags.iter().any(|f| f.contains("height not printed")), "seed {seed}: {} {:?}", p.tag, p.flags);
                assert!(p.confidence < 0.9, "seed {seed}: {}", p.tag);
            }
            if p.elevation_from == "rule" {
                assert!(p.flags.iter().any(|f| f.contains("hung")), "seed {seed}: {}", p.tag);
            }
        }
        let gm = r.placed.iter().find(|p| p.kind == "gas_meter").unwrap();
        assert_eq!(gm.height_from, "rule", "the meter's schedule prints no size");
        let vav = r.placed.iter().find(|p| p.kind == "vav").unwrap();
        assert!(vav.flags.iter().any(|f| f.contains("inlet")), "a VAV box's height comes from its inlet: {:?}", vav.flags);
        // The model's spec form carries the sources and the schedule.
        let v = gm.spec_json(0.0);
        assert_eq!(v["sources"]["height"], "rule");
        assert_eq!(v["properties"]["TAG"], "GM-1");
        assert_eq!(v["systems"][0], "G");
    }
}

#[test]
fn printed_values_parse() {
    let ft = |s: &str| parse_length_ft(s, false).unwrap();
    assert!((ft("12'-0\"") - 12.0).abs() < 1e-9);
    assert!((ft("8'-7 1/2\"") - (8.0 + 7.5 / 12.0)).abs() < 1e-9);
    assert!((ft("0'-6\"") - 0.5).abs() < 1e-9);
    assert!((ft("30\"") - 2.5).abs() < 1e-9);
    assert!((ft("2'") - 2.0).abs() < 1e-9);
    assert!((parse_length_ft("36", true).unwrap() - 3.0).abs() < 1e-9);
    for bad in ["", "x", "1/0\"", "-3'", "99999'", "NaN", "inf"] {
        assert_eq!(parse_length_ft(bad, false), None, "{bad}");
    }
    assert_eq!(parse_size("SIZE (L x W x H)", "14'-0\" x 5'-6\" x 7'-0\""), Some(([14.0, 5.5, 7.0], false)));
    assert_eq!(parse_size("SIZE (DIA x H)", "2'-0\" x 5'-0\""), Some(([2.0, 2.0, 5.0], true)));
    assert_eq!(parse_size("DIMENSIONS (IN)", "36 x 30 x 33"), Some(([3.0, 2.5, 2.75], false)));
    assert_eq!(parse_size("SIZE", "LARGE"), None);
    assert_eq!(parse_height_note("BOD @ 10'-6\""), Some(('B', 10.5)));
    assert_eq!(parse_height_note("B.O.D. 9'-0\" AFF"), Some(('B', 9.0)));
    assert_eq!(parse_height_note("TOD @ 11'-8\""), Some(('T', 11.0 + 8.0 / 12.0)));
    assert_eq!(parse_height_note("CL @ 8'-7 1/2\""), Some(('C', 8.0 + 7.5 / 12.0)));
    assert_eq!(parse_height_note("BOP @ 9'-6\""), Some(('B', 9.5)));
    assert_eq!(parse_height_note("24x14"), None);
    assert_eq!(size_depth_in("24x14"), Some(14.0));
    assert_eq!(size_depth_in("24\"x14\""), Some(14.0));
    assert_eq!(size_depth_in("12\"Ø"), Some(12.0));
    assert_eq!(size_depth_in("3/4\" HWR"), Some(0.75));
    for (tag, kind) in [
        ("AHU-1", "ahu"),
        ("RTU-2", "rtu"),
        ("VAV-1-03", "vav"),
        ("FPB-4", "vav"),
        ("EF-1", "exhaust_fan"),
        ("SF-2", "fan"),
        ("P-1", "pump"),
        ("HWP-2", "pump"),
        ("CP-1", "pump"),
        ("WH-1", "water_heater"),
        ("CU-1", "condensing_unit"),
        ("ACCU-3", "condensing_unit"),
        ("GM-1", "gas_meter"),
    ] {
        assert_eq!(kind_from_tag(tag), Some(kind), "{tag}");
    }
    assert_eq!(kind_from_tag("CD-2"), None);
    assert_eq!(kind_from_title("ROOFTOP UNIT SCHEDULE"), Some("rtu"));
    assert_eq!(kind_from_title("PACKAGED ROOFTOP AIR CONDITIONING UNITS"), Some("rtu"));
    assert_eq!(kind_from_title("PUMP SCHEDULE"), Some("pump"));
    assert_eq!(kind_from_title("FAN SCHEDULE"), Some("fan"));
    assert_eq!(kind_from_title("AIR DEVICE SCHEDULE"), Some(""));
    assert_eq!(kind_from_title("PLUMBING FIXTURE SCHEDULE"), Some(""));
    assert_eq!(kind_from_title("EQUIPMENT SCHEDULE"), None);
}

// ---- Hand-made sheets ---------------------------------------------------------------------------

fn add(d: &mut Drawing, layer: &str, kind: EntityKind) {
    d.add(&Space::Model, Common { layer: layer.into(), ..Common::default() }, kind).unwrap();
}

fn text(d: &mut Drawing, layer: &str, at: [f64; 2], h: f64, s: &str) {
    let t = Text {
        insert: Vec3::new(at[0], at[1], 0.0),
        align_pt: None,
        height: h,
        value: s.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "STANDARD".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    };
    add(d, layer, EntityKind::Text(t));
}

fn rect(d: &mut Drawing, layer: &str, a: [f64; 2], b: [f64; 2]) {
    let pts = [a, [b[0], a[1]], b, [a[0], b[1]]];
    let vertices = pts.iter().map(|p| PolyVertex { p: Vec2::new(p[0], p[1]), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
    add(d, layer, EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }));
}

fn line(d: &mut Drawing, layer: &str, a: [f64; 2], b: [f64; 2]) {
    add(d, layer, EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }));
}

/// A schedule table as drafters draw one: title, header cells on BORDER, body cells on TEXT.
fn table(d: &mut Drawing, at: [f64; 2], title: &str, rows: &[&[&str]]) {
    let (row_h, col_w, h) = (3.0, 14.0, 1.0);
    text(d, "TEXT", [at[0], at[1] + 1.0], 1.3, title);
    let mut y = at[1];
    for (i, r) in rows.iter().enumerate() {
        for (c, s) in r.iter().enumerate() {
            let x = at[0] + c as f64 * col_w;
            rect(d, if i == 0 { "BORDER" } else { "TEXT" }, [x, y - row_h], [x + col_w, y]);
            if !s.is_empty() {
                text(d, "TEXT", [x + 1.0, y - row_h + 1.0], h, s);
            }
        }
        y -= row_h;
    }
}

fn sheet(number: &str, discipline: &str, level: Option<&str>) -> Sheet {
    Sheet { number: number.into(), discipline: discipline.into(), level: level.map(str::to_string), scale: 0.125, ..Sheet::default() }
}

#[test]
fn a_rooftop_unit_fans_and_pumps_from_hand_made_sheets() {
    let mut sch = Drawing::default();
    table(
        &mut sch,
        [0.0, 100.0],
        "PACKAGED ROOFTOP UNIT SCHEDULE",
        &[
            &["MARK", "CFM", "TONS", "DIMENSIONS (IN)", "WEIGHT (LB)"],
            &["RTU-1", "4,000", "10", "120 x 72 x 54", "1850"],
            &["RTU-2", "2000", "5", "", "900"],
        ],
    );
    table(&mut sch, [0.0, 70.0], "FAN SCHEDULE", &[&["TAG", "TYPE", "CFM"], &["SF-1", "IN-LINE CABINET", "800"], &["EF-3", "ROOF UPBLAST", "350"]]);
    table(
        &mut sch,
        [0.0, 45.0],
        "PUMP SCHEDULE",
        &[&["TAG", "TYPE", "GPM", "SIZE (L x W x H)"], &["HWP-1", "BASE MOUNTED END SUCTION", "40", "3'-0\" x 1'-6\" x 2'-0\""]],
    );
    table(&mut sch, [80.0, 100.0], "AIR DEVICE SCHEDULE", &[&["TAG", "TYPE"], &["CD-1", "SQUARE DIFFUSER"]]);
    // Something that looks like a table but has no tag column.
    table(&mut sch, [80.0, 70.0], "NOTES", &[&["NO.", "NOTE"], &["1", "SEE SPECIFICATIONS"]]);
    let tables = read_tables(&sch, "M-602");
    assert_eq!(tables.len(), 4, "{tables:#?}");
    let items = scheduled(&tables);
    let kinds: Vec<(&str, &str)> = items.iter().map(|s| (s.tag.as_str(), s.kind.as_str())).collect();
    assert_eq!(kinds, [("RTU-1", "rtu"), ("RTU-2", "rtu"), ("SF-1", "fan"), ("EF-3", "exhaust_fan"), ("HWP-1", "pump")]);
    let rtu = items.first().unwrap();
    assert_eq!(rtu.cfm, Some(4000.0));
    assert_eq!(rtu.size_ft, Some([10.0, 6.0, 4.5]));
    assert_eq!(rtu.properties.get("WEIGHT (LB)").map(String::as_str), Some("1850"));

    // The roof plan: RTU-1 drawn, RTU-2 only tagged, SF-1 nowhere; a supply duct off RTU-1.
    let mut roof = Drawing::default();
    rect(&mut roof, "EQUIP", [120.0, 120.0], [240.0, 192.0]); // 10' x 6' in model inches
    text(&mut roof, "TEXT", [150.0, 150.0], 9.0, "RTU-1");
    text(&mut roof, "TEXT", [600.0, 600.0], 9.0, "RTU-2");
    line(&mut roof, "TEXT", [600.0, 600.0], [600.0, 640.0]);
    text(&mut roof, "TEXT", [300.0, 300.0], 9.0, "EF-3");
    let sh_sched = sheet("M-602", "M", None);
    let sh_roof = sheet("M-103", "M", Some("R"));
    let levels = [Level { name: "ROOF".into(), code: "R".into(), elevation_ft: 28.0 }];
    let r = equip_reader::read_equipment(&[SheetIn { sheet: &sh_sched, drawing: &sch }, SheetIn { sheet: &sh_roof, drawing: &roof }], &levels);
    let p = r.placed.iter().find(|p| p.tag == "RTU-1").unwrap();
    assert_eq!((p.kind.as_str(), p.level.as_str(), p.footprint_from.as_str(), p.height_from.as_str()), ("rtu", "R", "outline", "schedule"));
    assert!((p.min_ft[0] - 10.0).abs() < 1e-9 && (p.max_ft[1] - 16.0).abs() < 1e-9, "{:?} {:?}", p.min_ft, p.max_ft);
    assert!((p.min_ft[2] - 28.0).abs() < 1e-9 && (p.max_ft[2] - 32.5).abs() < 1e-9, "on the roof, 4'-6\" tall: {:?}", p.max_ft);
    assert!(p.flags.is_empty(), "{:?}", p.flags);
    // No outline at RTU-2's tag and no printed size: the usual size at the tag, flagged.
    let q = r.placed.iter().find(|p| p.tag == "RTU-2").unwrap();
    assert_eq!((q.footprint_from.as_str(), q.height_from.as_str()), ("rule", "rule"));
    assert!(q.confidence < 0.5 && q.flags.iter().any(|f| f.contains("no outline")) && q.flags.iter().any(|f| f.contains("no size")), "{q:?}");
    // EF-3 is tagged with nothing drawn near: placed at its tag.
    assert!(r.placed.iter().any(|p| p.tag == "EF-3" && p.footprint_from == "rule"));
    let unplaced: Vec<&str> = r.unplaced.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(unplaced, ["SF-1", "HWP-1"]);
}

/// A schedule as most offices draw one: grid lines on an annotation layer (each rule one long
/// line, the column rules stopping under a merged title row), body text on another layer, some
/// cells empty, columns of different widths.
fn grid_table(d: &mut Drawing, layer: &str, at: [f64; 2], title: &str, widths: &[f64], rows: &[&[&str]]) {
    let row_h = 3.0;
    let right = at[0] + widths.iter().sum::<f64>();
    let bottom = at[1] - row_h * rows.len() as f64;
    // The title row above the header, one merged cell.
    line(d, layer, [at[0], at[1] + row_h], [right, at[1] + row_h]);
    text(d, "M-ANNO-TEXT", [at[0] + 10.0, at[1] + 1.0], 1.3, title);
    for k in 0..=rows.len() {
        let y = at[1] - row_h * k as f64;
        line(d, layer, [at[0], y], [right, y]);
    }
    let mut x = at[0];
    for (c, w) in widths.iter().enumerate() {
        // The outer rules run up through the title row; the others stop at the header's top.
        line(d, layer, [x, bottom], [x, if c == 0 { at[1] + row_h } else { at[1] }]);
        for (i, r) in rows.iter().enumerate() {
            if let Some(s) = r.get(c).filter(|s| !s.is_empty()) {
                text(d, "M-ANNO-TEXT", [x + 0.5, at[1] - row_h * (i + 1) as f64 + 1.0], 1.0, s);
            }
        }
        x += w;
    }
    line(d, layer, [right, bottom], [right, at[1] + row_h]);
}

#[test]
fn line_grid_schedules_and_outlines_on_any_layer() {
    let mut sch = Drawing::default();
    let header: &[&str] = &["MARK", "SERVES", "CFM", "SIZE (L x W x H)", "WEIGHT (LB)"];
    let rows: &[&[&str]] =
        &[header, &["AHU-1", "LEVEL 1 WEST", "3,200", "9'-0\" x 5'-6\" x 6'-0\"", "1450"], &["AHU-2", "", "2,000", "7'-0\" x 4'-0\" x 5'-0\"", ""]];
    grid_table(&mut sch, "M-ANNO-SCHD", [0.0, 100.0], "AIR HANDLING UNIT SCHEDULE", &[8.0, 16.0, 7.0, 24.0, 10.0], rows);
    // A second grid table beside it whose rules are drawn cell by cell (short pieces).
    let pump_rows: &[&[&str]] = &[&["TAG", "TYPE", "GPM"], &["P-1", "BASE MOUNTED", "40"]];
    let (x0, y0, w, rh) = (90.0, 100.0, 12.0, 3.0);
    for k in 0..=2 {
        for c in 0..3 {
            let x = x0 + w * c as f64;
            line(&mut sch, "G-ANNO-TABL", [x, y0 - rh * k as f64], [x + w, y0 - rh * k as f64]);
        }
    }
    for c in 0..=3 {
        for k in 0..2 {
            let x = x0 + w * c as f64;
            line(&mut sch, "G-ANNO-TABL", [x, y0 - rh * k as f64], [x, y0 - rh * (k + 1) as f64]);
        }
    }
    text(&mut sch, "G-ANNO-TEXT", [x0, y0 + 1.0], 1.3, "PUMP SCHEDULE");
    for (k, r) in pump_rows.iter().enumerate() {
        for (c, s) in r.iter().enumerate() {
            text(&mut sch, "G-ANNO-TEXT", [x0 + w * c as f64 + 0.5, y0 - rh * (k + 1) as f64 + 1.0], 1.0, s);
        }
    }
    // A sheet border and title block lines around it all: not tables.
    rect(&mut sch, "G-ANNO-TTLB", [-20.0, -20.0], [200.0, 140.0]);
    line(&mut sch, "G-ANNO-TTLB", [160.0, -20.0], [160.0, 140.0]);
    text(&mut sch, "G-ANNO-TTLB", [165.0, 0.0], 2.0, "M-601");

    let tables = read_tables(&sch, "M-601");
    assert_eq!(tables.len(), 2, "{tables:#?}");
    let ahu = &tables[0];
    assert_eq!(ahu.title, "AIR HANDLING UNIT SCHEDULE");
    assert_eq!(ahu.columns, header);
    assert_eq!(ahu.rows.len(), 2, "{ahu:#?}");
    // Cell for cell, empty cells left out.
    for (row, want) in ahu.rows.iter().zip(&rows[1..]) {
        let want: std::collections::BTreeMap<String, String> =
            header.iter().zip(want.iter()).filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        assert_eq!(row, &want);
    }
    let pumps = &tables[1];
    assert_eq!((pumps.title.as_str(), pumps.columns.len(), pumps.rows.len()), ("PUMP SCHEDULE", 3, 1), "{pumps:#?}");
    assert_eq!(pumps.rows[0].get("TYPE").map(String::as_str), Some("BASE MOUNTED"));
    let items = scheduled(&tables);
    let tags: Vec<(&str, &str)> = items.iter().map(|s| (s.tag.as_str(), s.kind.as_str())).collect();
    assert_eq!(tags, [("AHU-1", "ahu"), ("AHU-2", "ahu"), ("P-1", "pump")]);
    assert_eq!(items[0].size_ft, Some([9.0, 5.5, 6.0]));
    assert_eq!(items[0].cfm, Some(3200.0));

    // The plan: AHU-1's outline on M-EQPM with its tag beside it; AHU-2's outline on a layer that
    // says nothing (A-FLOR-OVHD), its printed 7' x 4' — taken, flagged; a 3' x 3' box on that layer
    // by P-1's tag isn't its (it has no printed size and the tag is outside it).
    let mut plan = Drawing::default();
    rect(&mut plan, "M-EQPM", [120.0, 120.0], [228.0, 186.0]); // 9' x 5'-6"
    text(&mut plan, "M-ANNO-TEXT", [240.0, 150.0], 9.0, "AHU-1");
    rect(&mut plan, "A-FLOR-OVHD", [600.0, 120.0], [648.0, 204.0]); // 4' x 7'
    text(&mut plan, "M-ANNO-TEXT", [660.0, 150.0], 9.0, "AHU-2");
    rect(&mut plan, "A-FLOR-OVHD", [900.0, 120.0], [936.0, 156.0]);
    text(&mut plan, "P-ANNO-TEXT", [960.0, 130.0], 9.0, "P-1");
    let sh_sched = sheet("M-601", "M", None);
    let sh_plan = sheet("M-101", "M", Some("1"));
    let levels = [Level { name: "LEVEL 1".into(), code: "1".into(), elevation_ft: 0.0 }];
    let r = equip_reader::read_equipment(&[SheetIn { sheet: &sh_sched, drawing: &sch }, SheetIn { sheet: &sh_plan, drawing: &plan }], &levels);
    assert!(r.notes.is_empty(), "{:?}", r.notes);
    let p = r.placed.iter().find(|p| p.tag == "AHU-1").unwrap();
    assert_eq!((p.footprint_from.as_str(), p.height_from.as_str()), ("outline", "schedule"));
    assert!(
        (p.min_ft[0] - 10.0).abs() < 1e-9 && (p.max_ft[0] - 19.0).abs() < 1e-9 && (p.max_ft[1] - 15.5).abs() < 1e-9,
        "{:?} {:?}",
        p.min_ft,
        p.max_ft
    );
    assert!((p.max_ft[2] - 6.0).abs() < 1e-9 && p.flags.is_empty(), "{p:?}");
    let q = r.placed.iter().find(|p| p.tag == "AHU-2").unwrap();
    assert_eq!(q.footprint_from, "outline");
    assert!((q.min_ft[0] - 50.0).abs() < 1e-9 && (q.max_ft[1] - 17.0).abs() < 1e-9, "{:?} {:?}", q.min_ft, q.max_ft);
    assert!(q.flags.iter().any(|f| f.contains("A-FLOR-OVHD") && f.contains("printed size")), "{:?}", q.flags);
    assert!(q.confidence < p.confidence);
    let pump = r.placed.iter().find(|p| p.tag == "P-1").unwrap();
    assert_ne!(pump.footprint_from, "outline", "a box that isn't its size isn't its outline: {pump:?}");

    // No schedule on a sheet: said, not silent.
    let mut blank = Drawing::default();
    text(&mut blank, "G-ANNO-TEXT", [0.0, 0.0], 1.0, "GENERAL NOTES");
    let none = equip_reader::read_equipment(&[SheetIn { sheet: &sh_sched, drawing: &blank }], &levels);
    assert!(none.tables.is_empty() && none.notes.iter().any(|n| n.contains("no schedule found") && n.contains("TAG")), "{:?}", none.notes);
}

/// The spec form names levels and systems as the building spec does, not with the reader's codes.
#[test]
fn spec_form_uses_the_specs_level_and_system_names() {
    use equip_reader::spec_system_name;
    let systems =
        [("M-DUCT-SUPP", "duct"), ("M-DUCT-RETN", "duct"), ("P-DOMW-HW", "pipe"), ("P-DOMW-HWR", "pipe"), ("P-DOMW-CW", "pipe"), ("Gas", "pipe")];
    assert_eq!(spec_system_name("SA", "duct", &systems).as_deref(), Some("M-DUCT-SUPP"));
    assert_eq!(spec_system_name("RA", "duct", &systems).as_deref(), Some("M-DUCT-RETN"));
    assert_eq!(spec_system_name("HW", "pipe", &systems).as_deref(), Some("P-DOMW-HW"));
    assert_eq!(spec_system_name("HWR", "pipe", &systems).as_deref(), Some("P-DOMW-HWR"));
    assert_eq!(spec_system_name("CW", "pipe", &systems).as_deref(), Some("P-DOMW-CW"));
    assert_eq!(spec_system_name("G", "pipe", &systems).as_deref(), Some("Gas"));
    assert_eq!(spec_system_name("SA", "pipe", &systems), None, "a duct code isn't a pipe system");
    assert_eq!(spec_system_name("EA", "duct", &systems), None);
    // A spec system already named with the code is that one.
    assert_eq!(spec_system_name("sa", "duct", &[("SA", "duct"), ("M-DUCT-SUPP", "duct")]).as_deref(), Some("SA"));

    let (t, r) = read(1);
    let names: Vec<(String, String)> = t.building.levels.iter().map(|l| (l.code.clone(), format!("Level {}", l.code))).collect();
    let level_names: Vec<(&str, &str)> = names.iter().map(|(c, n)| (c.as_str(), n.as_str())).collect();
    let (items, notes) = equip_reader::spec_equipment(&r, &t.building.levels, &level_names, &systems);
    assert_eq!(items.len(), r.placed.len());
    for (v, p) in items.iter().zip(&r.placed) {
        assert_eq!(v["level"], format!("Level {}", p.level));
        for sy in v["systems"].as_array().unwrap() {
            assert!(systems.iter().any(|(n, _)| sy == n) || notes.iter().any(|n| n.contains(&format!("{} joins", p.tag))), "{} {sy}", p.tag);
        }
    }
    let ahu = items.iter().find(|v| v["kind"] == "ahu").unwrap();
    assert!(ahu["systems"].as_array().unwrap().iter().any(|s| s == "M-DUCT-SUPP"), "{ahu}");
    // A system the spec lacks keeps its code and says so; a level it lacks leaves the item out.
    let _ = notes;
    let (items, notes) = equip_reader::spec_equipment(&r, &t.building.levels, &level_names, &systems[1..]);
    let ahu = items.iter().find(|v| v["kind"] == "ahu").unwrap();
    assert!(ahu["systems"].as_array().unwrap().iter().any(|s| s == "SA"), "{ahu}");
    assert!(notes.iter().any(|n| n.contains("joins duct SA") && n.contains("isn't a system of the building spec")), "{notes:?}");
    let (some, notes) = equip_reader::spec_equipment(&r, &t.building.levels, &[], &systems);
    assert!(some.is_empty() && notes.iter().any(|n| n.contains("left out")));
}

#[test]
fn hostile_drawings_dont_panic() {
    let mut d = Drawing::default();
    // Degenerate and enormous cells, NaN text, a table of one header cell, overlapping headers.
    rect(&mut d, "BORDER", [0.0, 0.0], [0.0, 0.0]);
    rect(&mut d, "BORDER", [-1e300, -1e300], [1e300, 1e300]);
    text(&mut d, "TEXT", [f64::NAN, 1.0], 1.0, "TAG");
    text(&mut d, "TEXT", [1.0, 1.0], f64::INFINITY, "TAG");
    text(&mut d, "TEXT", [1.0, 1.0], 0.0, "TAG");
    for i in 0..50 {
        let x = i as f64 * 0.5;
        rect(&mut d, "BORDER", [x, 0.0], [x + 10.0, 3.0]);
        text(&mut d, "TEXT", [x + 0.1, 1.0], 1.0, if i % 2 == 0 { "TAG" } else { "SIZE (L x W x H)" });
        rect(&mut d, "TEXT", [x, -3.0], [x + 10.0, 0.0]);
        text(&mut d, "TEXT", [x + 0.1, -2.0], 1.0, "AHU-9");
        text(&mut d, "TEXT", [x + 0.2, -2.0], 1.0, "1e308' x -0\" x 1/0\"");
    }
    line(&mut d, "TEXT", [f64::NAN, 0.0], [1.0, f64::INFINITY]);
    line(&mut d, "PIPE", [0.0, 0.0], [0.0, 0.0]);
    let _ = read_tables(&d, "X");
    let _ = scheduled(&read_tables(&d, "X"));
    let sh = sheet("M-1", "M", None);
    let plan = sheet("P-1", "P", Some("1"));
    let r = equip_reader::read_equipment(&[SheetIn { sheet: &sh, drawing: &d }, SheetIn { sheet: &plan, drawing: &d }], &[]);
    for p in &r.placed {
        assert!(p.min_ft.iter().chain(&p.max_ft).all(|v| !v.is_nan()), "{p:?}");
    }
    let empty = equip_reader::read_equipment(&[], &[]);
    assert!(empty.placed.is_empty() && empty.scheduled.is_empty());
}
