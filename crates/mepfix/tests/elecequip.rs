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
        let truth_types: BTreeSet<&str> = t.elec_devices.iter().filter(|d| d.kind == "light_fixture").map(|d| d.type_tag.as_str()).collect();
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

/// Q335: each panel's directory, made from the model, matches its panel schedule on the prints;
/// every device on a circuit gets its plate label; a device on a circuit the schedule doesn't have
/// is flagged.
#[test]
fn directories_match_the_panel_schedules_of_a_made_up_set() {
    use cadaddy_walk3d::{ElecDevice, MepEquipment, PanelCircuit};
    let set = generate(3).unwrap();
    let t = &set.truth;
    let mut equipment = Vec::new();
    for p in t.elec_panels.iter().filter(|p| !p.schedule.is_empty()) {
        let circuits = p
            .schedule
            .iter()
            .map(|row| {
                // The rooms its devices are in.
                let mut rooms: Vec<String> = t
                    .elec_circuits
                    .iter()
                    .filter(|c| c.panel == p.id && c.number == row.circuit)
                    .flat_map(|c| c.devices.iter())
                    .filter_map(|id| t.elec_devices.iter().find(|d| &d.id == id).map(|d| d.room.clone()))
                    .collect();
                rooms.sort();
                rooms.dedup();
                PanelCircuit {
                    number: row.circuit.clone(),
                    description: row.description.clone(),
                    room: rooms.join(", "),
                    breaker: row.breaker_amps.clone(),
                    poles: row.poles.parse().unwrap_or(1),
                }
            })
            .collect();
        equipment.push(MepEquipment {
            tag: p.tag.clone(),
            kind: "panel".into(),
            level: p.level.clone(),
            min: [p.min_ft[0] * 12.0, p.min_ft[1] * 12.0],
            max: [p.max_ft[0] * 12.0, p.max_ft[1] * 12.0],
            height_ft: 5.0,
            circuits,
            ..MepEquipment::default()
        });
    }
    let devices: Vec<ElecDevice> = t
        .elec_devices
        .iter()
        .filter_map(|d| {
            // (A circuit names its panel by id.)
            let c = t.elec_circuits.iter().find(|c| c.id == d.circuit)?;
            let panel = t.elec_panels.iter().find(|p| p.id == c.panel)?;
            Some(ElecDevice {
                tag: d.id.clone(),
                kind: d.kind.clone(),
                level: d.level.clone(),
                room: d.room.clone(),
                panel: panel.tag.clone(),
                circuit: c.number.clone(),
            })
        })
        .collect();
    assert!(!devices.is_empty() && !equipment.is_empty());
    let unscheduled =
        devices.iter().filter(|d| !t.elec_panels.iter().any(|p| p.tag == d.panel && p.schedule.iter().any(|r| r.circuit == d.circuit))).count();
    let levels: Vec<serde_json::Value> =
        t.building.levels.iter().map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 12, "region": [-100000, -100000, 100000, 100000], "base": [0, 0]})).collect();
    let mut s = cadaddy_engine::Session::new();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"units_per_foot": 12, "levels": levels, "mep": {"equipment": equipment, "devices": devices}}}),
    )
    .unwrap();
    let l = s.execute("labels.list", &json!({})).unwrap();
    for p in t.elec_panels.iter().filter(|p| !p.schedule.is_empty()) {
        let rows = l["directories"][&p.tag].as_array().unwrap_or_else(|| panic!("no directory for {}", p.tag));
        assert_eq!(rows.len(), p.schedule.len(), "{}", p.tag);
        for (row, sched) in rows.iter().zip(&p.schedule) {
            // A multi-pole breaker lists every pole it takes (`17,19`).
            let poles: Vec<&str> = row["circuit"].as_str().unwrap().split(',').collect();
            assert_eq!((poles[0], poles.len()), (sched.circuit.as_str(), sched.poles.parse::<usize>().unwrap_or(1)), "{}", p.tag);
            assert_eq!(row["description"], json!(sched.description.to_uppercase()), "{}", p.tag);
        }
        assert!(rows.iter().any(|r| r["room"].as_str().unwrap().starts_with("RM ")), "{}: rooms by the rule", p.tag);
    }
    assert_eq!(l["counts"]["devices"].as_u64().unwrap() as usize, devices_count(&l));
    let flagged = l["problems"].as_array().unwrap().iter().filter(|p| p["message"].as_str().unwrap().contains("schedule doesn't have")).count();
    assert_eq!(flagged, unscheduled);
}

fn devices_count(l: &serde_json::Value) -> usize {
    l["labels"].as_array().unwrap().iter().filter(|x| x["set"] == "devices").count()
}
