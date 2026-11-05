use super::*;
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;

fn t(x: f64, y: f64, v: &str, h: f64) -> TextItem {
    TextItem { value: v.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
}

/// A service load summary, a panel schedule beside it on the same lines (its numbers must stay
/// out of the summary), and a panel's demand calculation under it in kVA.
fn sheet() -> Vec<TextItem> {
    let mut v = vec![
        t(100.0, 800.0, "SERVICE LOAD CALCULATION", 12.0),
        t(100.0, 782.0, "LOAD DESCRIPTION", 7.0),
        t(300.0, 782.0, "CONNECTED", 7.0),
        t(420.0, 782.0, "DEMAND", 7.0),
        t(100.0, 772.0, "LIGHTING", 7.0),
        t(300.0, 772.0, "12,000", 7.0),
        t(340.0, 772.0, "x", 7.0),
        t(350.0, 772.0, "125%", 7.0),
        t(385.0, 772.0, "=", 7.0),
        t(420.0, 772.0, "15,000", 7.0),
        t(100.0, 762.0, "RECEPTACLES", 7.0),
        t(300.0, 762.0, "20,000", 7.0),
        t(350.0, 762.0, "100%;50%", 7.0),
        t(420.0, 762.0, "15,000", 7.0),
        t(100.0, 752.0, "SUB-TOTAL (VA)", 7.0),
        t(300.0, 752.0, "32,000", 7.0),
        t(420.0, 752.0, "30,000", 7.0),
        t(100.0, 742.0, "SUB-TOTAL (AMPS)", 7.0),
        t(420.0, 742.0, "83", 7.0),
        t(330.0, 732.0, "SERVICE SIZE (AMPS)", 7.0),
        t(420.0, 732.0, "400", 7.0),
        // A schedule to the right on the same lines.
        t(650.0, 800.0, "PANELBOARD Q1", 12.0),
        t(600.0, 782.0, "CKT", 6.0),
        t(620.0, 782.0, "DESCRIPTION", 6.0),
        t(600.0, 772.0, "1", 6.0),
        t(620.0, 772.0, "LIGHTING NORTH", 6.0),
        t(700.0, 772.0, "20", 6.0),
        t(600.0, 762.0, "3", 6.0),
        t(700.0, 762.0, "20", 6.0),
    ];
    // Under it, another table: a panel's demand calculation, its lines in kVA.
    v.extend([
        t(110.0, 690.0, "PANELBOARD R2 DEMAND CALCULATION", 12.0),
        t(90.0, 674.0, "BASED ON NEC ARTICLE 220.84, OPTIONAL CALCULATION", 8.0),
        t(100.0, 660.0, "12", 8.0),
        t(140.0, 660.0, "UNITS", 8.0),
        t(100.0, 648.0, "TOTAL LOAD:", 8.0),
        t(330.0, 648.0, "300.50", 8.0),
        t(370.0, 648.0, "KVA", 8.0),
        t(100.0, 636.0, "41% DEMAND:", 8.0),
        t(330.0, 636.0, "123.21", 8.0),
        t(370.0, 636.0, "KVA", 8.0),
        t(100.0, 624.0, "TOTAL AMPS AT 120/208V, 3PH:", 8.0),
        t(330.0, 624.0, "342", 8.0),
    ]);
    v
}

#[test]
fn a_load_summary_is_read_as_far_as_the_band_that_parts_it_from_the_schedule_beside_it() {
    let r = rules();
    let got = read_summaries(&sheet(), "X-2", &r);
    assert_eq!(got.len(), 2, "{got:#?}");
    let svc = got.iter().find(|s| s.title.starts_with("SERVICE")).unwrap();
    assert_eq!(svc.panel, "");
    assert_eq!(svc.source, "X-2");
    let light = svc.lines.iter().find(|l| l.label == "LIGHTING").unwrap();
    assert_eq!(light.values, vec![12_000.0, 15_000.0], "a percent is no value: {light:?}");
    assert!(light.text.contains("125%"), "{light:?}");
    // The schedule beside it on the same lines stays out.
    assert!(svc.lines.iter().all(|l| !l.text.contains("NORTH") && !l.values.contains(&20.0)), "{svc:#?}");
    // The demand calculation under it is its own summary, not lines of this one.
    assert!(svc.lines.iter().all(|l| !l.text.contains("KVA")), "{svc:#?}");
    let (size, dva, da, cva) = summary_facts(svc, &r);
    assert_eq!((size, dva, da, cva), (Some(400.0), Some(30_000.0), Some(83.0), Some(32_000.0)));

    let calc = got.iter().find(|s| s.panel == "R2").unwrap();
    assert_eq!(calc.basis, "220.84");
    let (size, dva, da, _) = summary_facts(calc, &r);
    assert_eq!(size, None);
    assert_eq!(dva.map(|v| v.round()), Some(123_210.0), "kVA reads as VA: {calc:#?}");
    assert_eq!(da, Some(342.0), "`120/208V` and `3PH` are no amps: {calc:#?}");
}

#[test]
fn summary_titles_name_their_panel_or_none() {
    let r = rules();
    assert_eq!(read::title_panel("PANELBOARD R2 DEMAND CALCULATION", &r), "R2");
    assert_eq!(read::title_panel("PANEL 'MDP-1' LOAD SUMMARY", &r), "MDP-1");
    assert_eq!(read::title_panel("PANEL LOAD SUMMARY", &r), "");
    assert_eq!(read::title_panel("COMMERCIAL LOAD SUMMARY", &r), "");
    assert_eq!(read::basis("PER NEC 220.87 (MEASURED)"), Some("220.87".into()));
    assert_eq!(read::basis("NEC-220 x 125%"), None);
}

#[test]
fn hostile_texts_read_nothing_and_never_panic() {
    let r = rules();
    let mut v = vec![t(f64::NAN, 0.0, "LOAD SUMMARY", 12.0), t(0.0, 0.0, "LOAD SUMMARY", 0.0), t(0.0, 0.0, "LOAD SUMMARY", 12.0)];
    for i in 0..3000u32 {
        v.push(t(f64::from(i % 50), -f64::from(i), &"9".repeat((i % 40) as usize), 7.0));
    }
    let got = read_summaries(&v, "X", &r);
    assert!(got.len() <= 1);
    assert!(read_summaries(&[], "X", &r).is_empty());
}

#[test]
fn demand_steps_systems_and_phases() {
    // Table 220.42 hotel row: 20 kVA at 50%, the next 80 kVA at 40%, the rest at 30%.
    let hotel = rules().occupancy("R-1").cloned().unwrap();
    assert_eq!(hotel.id, "hotel");
    assert!((stepped(150_000.0, &hotel.lighting_steps) - (10_000.0 + 32_000.0 + 15_000.0)).abs() < 1e-6);
    assert!((stepped(5_000.0, &rules().receptacle_steps) - 5_000.0).abs() < 1e-6);
    assert!((stepped(30_000.0, &rules().receptacle_steps) - 20_000.0).abs() < 1e-6);
    assert_eq!(rules().occupancy("R-2").map(|o| o.id.as_str()), Some("dwelling"));
    assert_eq!(rules().occupancy("B").map(|o| o.id.as_str()), Some("commercial"));
    let dw = rules().occupancy("dwelling").cloned().unwrap();
    assert_eq!(by_count(64, &dw.optional_units), Some(0.23));
    assert_eq!(by_count(10, &dw.optional_units), Some(0.43));
    assert_eq!(by_count(2, &dw.optional_units), None);
    let s = system("208Y/120V, 3PH, 4W");
    assert_eq!((s.volts, s.to_neutral, s.phases), (208.0, 120.0, 3));
    let s = system("120/240V, 1PH, 3W");
    assert_eq!((s.volts, s.to_neutral, s.phases), (240.0, 120.0, 1));
    let s = system("480Y/277V 3PH");
    assert_eq!((s.volts, s.to_neutral, s.phases), (480.0, 277.0, 3));
    assert_eq!(phases_of("1", 1, 3), vec![0]);
    assert_eq!(phases_of("2", 1, 3), vec![0]);
    assert_eq!(phases_of("3", 1, 3), vec![1]);
    assert_eq!(phases_of("6", 1, 3), vec![2]);
    assert_eq!(phases_of("7", 3, 3), vec![0, 1, 2]);
    assert_eq!(phases_of("3", 2, 2), vec![1, 0]);
    assert_eq!(phases_of("x", 1, 3), Vec::<usize>::new());
    assert_eq!(circuit_number("A-5", "A"), "5");
    assert_eq!(circuit_number("Q-13,15", "Q"), "13");
    assert_eq!(circuit_number("07", "LP"), "7");
    assert_eq!(amps_of("2000A MCB"), Some(2000.0));
    assert_eq!(amps_of("MLO"), None);
    let im = imbalance(&[3000.0, 1000.0, 500.0]).unwrap();
    assert!((im - 100.0).abs() < 1.0, "{im}");
}
