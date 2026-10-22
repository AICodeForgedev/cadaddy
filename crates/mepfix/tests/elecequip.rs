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
        // Types planted as missing from the schedule (`elec_spec.unscheduled`) are left out.
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

/// Q335: each panel's directory, read off the prints by `labels.read` (the `PANEL … SCHEDULE`
/// tables on the schedule sheet, the circuit printed beside each device on the plans), matches
/// the answer key's panel schedule row for row; every device read gets its plate label; the
/// devices on circuits the schedule doesn't have are exactly the planted ones. Nothing comes from
/// the answer key but the expectations.
#[test]
fn directories_match_the_panel_schedules_of_a_made_up_set() {
    for seed in [3, 11] {
        let set = generate(seed).unwrap();
        let t = &set.truth;
        // The E sheets, as the sheet reader recognises them, written out as the prints are.
        let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
        let map = sheet_reader::recognise_fixture_set(&dxf);
        let dir = std::env::temp_dir().join(format!("cadaddy-labels-read-{seed}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The answer key: each drawn panel schedule, and the circuit printed beside each device.
        let scheduled: Vec<&cadaddy_mepfix::truth::ElecPanel> =
            t.elec_panels.iter().filter(|p| p.kind == "panel" && !p.schedule.is_empty()).collect();
        assert!(!scheduled.is_empty(), "seed {seed}");
        let drawn: BTreeSet<&str> = t.elec_devices.iter().filter(|d| !d.sheet.is_empty()).map(|d| d.id.as_str()).collect();
        let mut refs: Vec<String> = t
            .elec_spec
            .notes
            .iter()
            .filter(|n| drawn.contains(n.device.as_str()) && !n.circuit_ref.is_empty())
            .map(|n| n.circuit_ref.clone())
            .collect();
        // Planted on the first plan: two devices' notes name circuits their panel's schedule
        // doesn't have (a 9x number, past every panel's last circuit).
        let mut planted: Vec<String> = Vec::new();
        let mut paths = Vec::new();
        for (n, b) in &dxf {
            let stem = n.trim_end_matches(".dxf");
            let Some(sh) = map.sheets.iter().find(|s| s.number == stem && s.discipline == "E") else { continue };
            let mut bytes = b.clone();
            if sh.level.is_some() && planted.is_empty() {
                let text = String::from_utf8(b.clone()).unwrap();
                let mut out = Vec::new();
                for line in text.split_inclusive('\n') {
                    let v = line.trim_end();
                    if planted.len() < 2 && out.last().is_some_and(|l: &String| l.trim() == "1") && refs.iter().any(|r| r == v) {
                        let (panel, _) = v.rsplit_once('-').unwrap();
                        let new = format!("{panel}-{}", 91 + 2 * planted.len());
                        let i = refs.iter().position(|r| r == v).unwrap();
                        refs[i] = new.clone();
                        planted.push(new.clone());
                        out.push(line.replacen(v, &new, 1));
                    } else {
                        out.push(line.to_string());
                    }
                }
                bytes = out.concat().into_bytes();
            }
            let path = dir.join(n);
            std::fs::write(&path, &bytes).unwrap();
            paths.push(json!({"path": path.display().to_string(), "sheet": sh.number}));
        }
        assert!(paths.len() >= 2, "seed {seed}: {paths:?}");
        assert_eq!(planted.len(), 2, "seed {seed}: two notes planted");
        cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
            read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
            write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
            plot: None,
            read_pdf: None,
            read_dwf: None,
            write_dwfx: None,
        });
        let levels: Vec<serde_json::Value> = t
            .building
            .levels
            .iter()
            .map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 12, "region": [-100000, -100000, 100000, 100000], "base": [0, 0]}))
            .collect();
        let mut s = cadaddy_engine::Session::new();
        s.execute("bspec.set", &json!({"replace": true, "spec": {"units_per_foot": 12, "levels": levels}})).unwrap();
        let r = s.execute("labels.read", &json!({"paths": paths, "apply": true})).unwrap();

        let read_panels: BTreeSet<&str> = r["schedules"].as_array().unwrap().iter().map(|x| x["panel"].as_str().unwrap()).collect();
        assert_eq!(read_panels, scheduled.iter().map(|p| p.tag.as_str()).collect::<BTreeSet<_>>(), "seed {seed}");
        // Unscheduled circuits: the two planted, and any the set itself has.
        let on_schedule =
            |r: &str| r.rsplit_once('-').is_some_and(|(p, c)| scheduled.iter().any(|x| x.tag == p && x.schedule.iter().any(|row| row.circuit == c)));
        assert!(planted.iter().all(|r| !on_schedule(r)));
        let mut planted: Vec<String> = refs.iter().filter(|r| !on_schedule(r)).cloned().collect();
        planted.sort();

        let l = s.execute("labels.list", &json!({})).unwrap();
        for p in &scheduled {
            let rows = l["directories"][&p.tag].as_array().unwrap_or_else(|| panic!("seed {seed}: no directory for {}", p.tag));
            let mut key: Vec<(u32, String, String)> = p
                .schedule
                .iter()
                .map(|row| {
                    let n: u32 = row.circuit.parse().unwrap();
                    let poles: u32 = row.poles.parse().unwrap_or(1);
                    let circuit = (0..poles).map(|k| (n + 2 * k).to_string()).collect::<Vec<_>>().join(",");
                    (n, circuit, row.description.to_uppercase())
                })
                .collect();
            key.sort();
            let got: Vec<(String, String)> =
                rows.iter().map(|x| (x["circuit"].as_str().unwrap().to_string(), x["description"].as_str().unwrap().to_string())).collect();
            let want: Vec<(String, String)> = key.into_iter().map(|(_, c, d)| (c, d)).collect();
            assert_eq!(got, want, "seed {seed}: panel {}", p.tag);
        }
        // Every circuit printed beside a device is one device label, each from its plan sheet.
        let devices = r["devices"].as_array().unwrap();
        let mut read_refs: Vec<String> =
            devices.iter().map(|d| format!("{}-{}", d["panel"].as_str().unwrap(), d["circuit"].as_str().unwrap())).collect();
        let mut want_refs = refs.clone();
        read_refs.sort();
        want_refs.sort();
        assert_eq!(read_refs, want_refs, "seed {seed}");
        assert!(devices.iter().all(|d| map.sheets.iter().any(|sh| sh.number == d["source"].as_str().unwrap() && sh.level.is_some())), "seed {seed}");
        assert_eq!(devices_count(&l), refs.len(), "seed {seed}");
        // The device-circuit problems (labels.check) are exactly the planted unscheduled circuits.
        let c = s.execute("labels.check", &json!({})).unwrap();
        let mut flagged: Vec<String> = c["problems"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["message"].as_str())
            .filter(|m| m.contains("schedule doesn't have") || m.contains("isn't in the equipment"))
            .map(|m| m.split(" is on ").nth(1).and_then(|x| x.split(", which").next()).unwrap_or(m).to_string())
            .collect();
        flagged.sort();
        println!("seed {seed}: {} panels, {} devices read, {} on unscheduled circuits", scheduled.len(), refs.len(), planted.len());
        assert_eq!(flagged, planted, "seed {seed}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn devices_count(l: &serde_json::Value) -> usize {
    l["labels"].as_array().unwrap().iter().filter(|x| x["set"] == "devices").count()
}
