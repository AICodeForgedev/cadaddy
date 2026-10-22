//! Q290: electrical gear from made-up prints. The E-sheets' equipment, lug and feeder schedules
//! read back exactly as the answer key has them; every piece of gear placed at its tag as a box of
//! its scheduled size at its mounting height; the light fixture schedule on every fixture of its
//! type; then the engine's gear review finds exactly the planted problems, lists the
//! transformer's lug kits, builds the gear with its raceways and exports it as IFC
//! (IfcElectricDistributionBoard, IfcTransformer, IfcSwitchingDevice).

use std::collections::BTreeSet;

use cadaddy_mepfix::{elec_reader, gear_reader, generate, sheet_reader};
use serde_json::json;

#[test]
fn gear_reads_from_the_prints_and_its_review_finds_the_planted_problems() {
    for seed in [3, 11] {
        let set = generate(seed).unwrap();
        let t = &set.truth;
        assert_eq!(t.gear_problems.len(), 4, "seed {seed}: {:?}", t.gear_problems);
        let e_sheets: Vec<(String, Vec<u8>)> =
            t.sheets.iter().filter(|s| s.discipline == "E").filter_map(|s| set.file(&s.dxf).map(|b| (s.number.clone(), b.to_vec()))).collect();
        let plans: Vec<(String, Vec<u8>)> = t
            .sheets
            .iter()
            .filter(|s| s.discipline == "E" && s.level.is_some())
            .filter_map(|s| set.file(&s.dxf).map(|b| (s.number.clone(), b.to_vec())))
            .collect();
        let all: Vec<(String, Vec<u8>)> = t.sheets.iter().filter_map(|s| set.file(&s.dxf).map(|b| (s.number.clone(), b.to_vec()))).collect();
        let map = sheet_reader::recognise_fixture_set(&all);
        let (mut devices, panels) = elec_reader::read_devices(&plans, &map, t);
        let drawings: Vec<(String, cadaddy_doc::Drawing)> = e_sheets.iter().map(|(n, b)| (n.clone(), cadaddy_io::read_dxf(b).unwrap())).collect();
        let refs: Vec<(&str, &cadaddy_doc::Drawing)> = drawings.iter().map(|(n, d)| (n.as_str(), d)).collect();
        let read = gear_reader::read_gear(&refs, &panels, 12.0);
        // Every piece of scheduled gear, read exactly and placed at its tag.
        let gear: Vec<&cadaddy_mepfix::truth::ElecPanel> = t.elec_panels.iter().filter(|p| !p.properties.is_empty()).collect();
        assert!(read.unplaced.is_empty(), "seed {seed}: {:?}", read.unplaced);
        assert_eq!(read.equipment.len(), gear.len(), "seed {seed}");
        for p in &gear {
            let e = read.equipment.iter().find(|e| e.tag == p.tag).unwrap_or_else(|| panic!("seed {seed}: {} not read", p.tag));
            assert_eq!(e.kind, p.kind, "{}", p.tag);
            assert_eq!(e.properties, p.properties, "{}", p.tag);
            assert_eq!(e.terminations, p.terminations, "{}", p.tag);
            assert_eq!(e.enclosure, p.enclosure, "{}", p.tag);
            let (w, d) = ((e.max[0] - e.min[0]) / 12.0 * 12.0, (e.max[1] - e.min[1]) / 12.0 * 12.0);
            assert!((w - p.size_in[0]).abs() < 1e-6 && (d - p.size_in[1]).abs() < 1e-6, "{}: {w} x {d}", p.tag);
            assert!((e.height_ft * 12.0 - p.size_in[2]).abs() < 1e-6 && (e.bottom_ft * 12.0 - p.bottom_in).abs() < 1e-6, "{}", p.tag);
            let c = [(e.min[0] + e.max[0]) / 24.0, (e.min[1] + e.max[1]) / 24.0];
            let off = (c[0] - p.at_ft[0]).hypot(c[1] - p.at_ft[1]);
            assert!(off < 1.0, "{} placed {off:.2} ft from its tag", p.tag);
        }
        assert_eq!(read.feeders, t.elec_feeders, "seed {seed}");
        // The light fixture schedule on every fixture of its type.
        // (Fixtures the device reader typed: some it reads without a type, or as an emergency unit.)
        let typed = devices.iter().filter(|d| d.kind == "light_fixture" && read.fixture_types.contains_key(&d.type_tag)).count();
        assert_eq!(gear_reader::link_fixtures(&mut devices, &read.fixture_types), typed);
        let lights = devices.iter().filter(|d| d.kind == "light_fixture").count();
        println!("seed {seed}: {typed} of {lights} light fixtures typed and linked");
        assert!(typed * 3 > lights, "seed {seed}: {typed} of {lights}");
        // (Less the type planted as one the schedule doesn't list.)
        let truth_types: BTreeSet<&str> = t
            .elec_devices
            .iter()
            .filter(|d| d.kind == "light_fixture" && !t.elec_spec.unscheduled.contains(&d.type_tag))
            .map(|d| d.type_tag.as_str())
            .collect();
        assert!(truth_types.iter().all(|ty| read.fixture_types.contains_key(*ty)), "{truth_types:?} vs {:?}", read.fixture_types.keys());
        for d in devices.iter().filter(|d| d.kind == "light_fixture" && d.type_tag == "A") {
            assert_eq!(d.properties.get("DESCRIPTION").map(String::as_str), Some("2X4 RECESSED TROFFER"));
            assert_eq!(d.properties.get("LAMP/WATTAGE").map(String::as_str), Some("LED 40W"));
        }

        // The engine's review of what was read: the planted problems, nothing else.
        let levels: Vec<serde_json::Value> = t
            .building
            .levels
            .iter()
            .map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 12, "region": [-100000, -100000, 100000, 100000], "base": [0, 0]}))
            .collect();
        let mut s = cadaddy_engine::Session::new();
        s.execute("bspec.set", &json!({"replace": true, "spec": {"units_per_foot": 12, "levels": levels, "mep": {"equipment": read.equipment, "feeders": read.feeders}}})).unwrap();
        let r = s.execute("gear.check", &json!({})).unwrap();
        let errors: BTreeSet<(String, String)> = r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["severity"] == "error")
            .map(|f| (f["gear"].as_str().unwrap().to_string(), f["check"].as_str().unwrap().to_string()))
            .collect();
        let planted: BTreeSet<(String, String)> = t.gear_problems.iter().cloned().collect();
        assert_eq!(errors, planted, "seed {seed}: {r}");
        // The transformer's secondary lugs: a kit per lighting panel feeder.
        let kits = r["lug_kits"].as_array().unwrap();
        assert_eq!(kits.len(), t.elec_feeders.iter().filter(|f| f.from == "TX-1").count());
        assert!(kits.iter().all(|k| k["gear"] == "TX-1" && k["point"] == "secondary" && k["bolt"] == "3/8\" x 1-1/2\" SAE grade 5"), "{kits:?}");

        // 3D and IFC.
        let b = s.execute("model.build", &json!({})).unwrap();
        assert_eq!(b["mepEquipment"].as_u64(), Some(gear.len() as u64), "{b}");
        let ifc = String::from_utf8(cadaddy_io::write(s.doc().unwrap(), "gear.ifc").unwrap()).unwrap();
        for (class, pre) in [
            ("IFCELECTRICDISTRIBUTIONBOARD", ".SWITCHBOARD."),
            ("IFCELECTRICDISTRIBUTIONBOARD", ".DISTRIBUTIONBOARD."),
            ("IFCTRANSFORMER", ".VOLTAGE."),
            ("IFCSWITCHINGDEVICE", ".SWITCHDISCONNECTOR."),
        ] {
            assert!(ifc.lines().any(|l| l.contains(&format!("={class}(")) && l.contains(pre)), "{class} {pre}");
        }
        assert!(ifc.contains("'MOUNTING'") || ifc.contains("MOUNTING"), "the schedule row travels as properties");
    }
}
