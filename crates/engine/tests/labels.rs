//! Labels from the model (Q335): a made-up service — switchboard MDP, panel P1 with its schedule,
//! disconnect DS-1, four devices — gives a directory that matches the panel schedule, device plate
//! labels, nameplates, equipment labels and arc flash labels from the study's data; what's
//! missing is a failure and a document request; changed circuits show; the sheets render; the BOM
//! lines come with them.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-labels-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn spec() -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14}],
        "mep": {
            "equipment": [
                {"tag": "MDP", "kind": "switchboard", "level": "L1", "min": [0, 0], "max": [72, 36], "bottom_ft": 0, "height_ft": 7.5,
                 "properties": {"VOLTAGE": "480Y/277V", "RATING": "1200A"}},
                {"tag": "P1", "kind": "panel", "level": "L1", "min": [100, 0], "max": [118, 6], "bottom_ft": 1.5, "height_ft": 5,
                 "properties": {"VOLTAGE": "480Y/277V", "RATING": "225A MLO"},
                 "circuits": [
                    {"number": "1", "description": "Receptacles", "room": "101", "breaker": "20A", "poles": 1},
                    {"number": "2", "description": "Lighting", "room": "102", "breaker": "20A", "poles": 1},
                    {"number": "3", "description": "RTU-1", "room": "Roof", "breaker": "40A", "poles": 3},
                    {"number": "4", "description": "Spare", "room": "", "breaker": "20A", "poles": 1}
                 ]},
                {"tag": "DS-1", "kind": "disconnect", "level": "L1", "min": [130, 0], "max": [146, 8], "bottom_ft": 4, "height_ft": 2,
                 "properties": {"VOLTAGE": "480V", "RATING": "60A NF"}}
            ],
            "feeders": [
                {"tag": "F-SVC", "from": "", "to": "MDP", "sets": 2, "size": "350 kcmil", "conduits": 2, "conduit_in": 3},
                {"tag": "F-P1", "from": "MDP", "to": "P1", "size": "3/0", "conduit_in": 2},
                {"tag": "F-DS1", "from": "MDP", "to": "DS-1", "size": "#6", "conduit_in": 1}
            ],
            "devices": [
                {"tag": "R-101-1", "kind": "receptacle", "level": "L1", "room": "101", "panel": "P1", "circuit": "1"},
                {"tag": "S-102-1", "kind": "switch", "level": "L1", "room": "102", "panel": "P1", "circuit": "2"},
                {"tag": "RTU-1 DISC", "kind": "disconnect", "level": "L1", "room": "Roof", "panel": "P1", "circuit": "3,5,7"},
                {"tag": "R-103-1", "kind": "receptacle", "level": "L1", "room": "103", "panel": "P1", "circuit": "9"}
            ]
        }
    })
}

fn session() -> Session {
    hooks();
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).unwrap();
    s
}

fn label<'a>(l: &'a Value, id: &str) -> &'a Value {
    l["labels"].as_array().unwrap().iter().find(|x| x["id"] == id).unwrap_or_else(|| panic!("no {id} in {l}"))
}

#[test]
fn the_directory_matches_the_panel_schedule_and_every_label_reads_from_the_model() {
    let mut s = session();
    let l = s.execute("labels.list", &json!({})).unwrap();
    // The directory: every circuit of the schedule, in order, poles and all, rooms by the rule.
    let rows: Vec<(String, String, String)> = l["directories"]["P1"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["circuit"].as_str().unwrap().into(), r["description"].as_str().unwrap().into(), r["room"].as_str().unwrap().into()))
        .collect();
    assert_eq!(
        rows,
        [
            ("1".into(), "RECEPTACLES".into(), "RM 101".into()),
            ("2".into(), "LIGHTING".into(), "RM 102".into()),
            ("3,5,7".into(), "RTU-1".into(), "RM ROOF".into()),
            ("4".into(), "SPARE".into(), String::new())
        ]
    );
    assert_eq!(label(&l, "dir:P1")["lines"][0], "PANEL P1 DIRECTORY");
    assert_eq!(label(&l, "dir:P1")["lines"][2], "FED FROM MDP");
    // Device plates, nameplates, equipment labels.
    assert_eq!(label(&l, "dev:R-101-1")["lines"], json!(["P1-1"]));
    assert_eq!(label(&l, "dev:RTU-1 DISC")["lines"], json!(["P1-3,5,7"]));
    assert_eq!(label(&l, "np:P1")["lines"], json!(["P1", "480Y/277V", "225A MLO", "FED FROM MDP"]));
    assert_eq!(label(&l, "np:MDP")["lines"], json!(["MDP", "480Y/277V", "1200A", "FED FROM UTILITY"]));
    assert_eq!(label(&l, "eq:DS-1")["lines"], json!(["DS-1", "480V", "60A NF", "FED FROM MDP"]));
    assert_eq!(l["counts"], json!({"directories": 1, "devices": 4, "equipment": 1, "nameplates": 2, "arc_flash": 0}));
    assert!(l["labels"].as_array().unwrap().iter().all(|x| x["status"] == "new"));
    // What's missing: the study's data for all three, and a device on a circuit P1 doesn't have.
    let problems: Vec<String> = l["problems"].as_array().unwrap().iter().map(|p| p["message"].as_str().unwrap().to_string()).collect();
    assert_eq!(problems.len(), 4, "{problems:#?}");
    assert!(problems.iter().any(|p| p.contains("R-103-1 is on P1-9")));
    for g in ["MDP", "P1", "DS-1"] {
        assert!(problems.iter().any(|p| p.contains(&format!("no arc flash study data for {g}"))), "{g}");
    }
    // The project's rules change how they read.
    s.execute("project.rules.set", &json!({"domain": "naming", "key": "room_label", "value": "Room {room}"})).unwrap();
    s.execute("project.rules.set", &json!({"domain": "naming", "key": "label_case", "value": "as_is"})).unwrap();
    s.execute("project.rules.set", &json!({"domain": "naming", "key": "circuit_label", "value": "{panel}/{circuit}"})).unwrap();
    let l = s.execute("labels.list", &json!({})).unwrap();
    assert_eq!(l["directories"]["P1"][0], json!({"circuit": "1", "description": "Receptacles", "room": "Room 101"}));
    assert_eq!(label(&l, "dev:R-101-1")["lines"], json!(["P1/1"]));
}

#[test]
fn arc_flash_labels_come_from_the_study_and_what_is_missing_is_requested() {
    let mut s = session();
    let d = dir("arcflash");
    let csv = d.join("study.csv");
    std::fs::write(
        &csv,
        "Bus,Incident Energy (cal/cm2),Working Distance (in),Arc Flash Boundary (in),PPE,Voltage,Limited Approach,Restricted Approach,Study,Date\n\
         MDP,8.2,18,52,\"PPE Category 2, arc rated\",480 V,42 in,12 in,Made-up Study Rev 1,2026-10-01\n\
         P1,1.4,18,19,PPE Category 1,480 V,42 in,12 in,Made-up Study Rev 1,2026-10-01\n\
         XFMR-9,3.0,18,30,PPE Category 1,208 V,42 in,1 in,Made-up Study Rev 1,2026-10-01\n",
    )
    .unwrap();
    let r = s.execute("labels.arcflash", &json!({"path": csv.display().to_string()})).unwrap();
    assert_eq!((r["imported"].as_u64(), r["unknown_equipment"].clone()), (Some(2), json!(["XFMR-9"])), "{r}");
    let l = s.execute("labels.list", &json!({"set": "arc_flash"})).unwrap();
    let mdp = label(&l, "af:MDP");
    let lines: Vec<&str> = mdp["lines"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert_eq!(lines[..4], ["WARNING", "ARC FLASH AND SHOCK HAZARD", "APPROPRIATE PPE REQUIRED", "MDP"]);
    assert!(lines.contains(&"INCIDENT ENERGY: 8.2 CAL/CM² AT 18 IN"), "{lines:?}");
    assert!(lines.contains(&"ARC FLASH BOUNDARY: 52 IN") && lines.contains(&"PPE: PPE CATEGORY 2, ARC RATED"), "{lines:?}");
    assert!(lines.contains(&"STUDY: MADE-UP STUDY REV 1, 2026-10-01"));
    // DS-1 still has none: a failure, and a document request when reviewed.
    let c = s.execute("labels.check", &json!({"review": true})).unwrap();
    assert_eq!(c["errors"], 2, "{c}");
    assert_eq!(c["rfis"].as_array().unwrap().len(), 2);
    let rfis = s.execute("rfi.list", &json!({"kind": "label_data"})).unwrap();
    assert_eq!(rfis["total"], 2);
    assert!(
        rfis["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["issue"].as_str().unwrap().contains("DS-1") && i["issue"].as_str().unwrap().starts_with("Document request"))
    );
    assert_eq!(s.execute("labels.check", &json!({"review": true})).unwrap()["rfis"].as_array().unwrap().len(), 0, "asked once");
    // Rows given directly; incomplete data is held as a failure, not a label.
    let r = s.execute("labels.arcflash", &json!({"rows": [{"equipment": "DS-1", "incident_energy": "", "working_distance_in": "18"}]})).unwrap();
    assert_eq!(r["incomplete"], json!(["DS-1"]));
    let l = s.execute("labels.list", &json!({})).unwrap();
    assert!(l["problems"].as_array().unwrap().iter().any(|p| p["message"].as_str().unwrap().contains("DS-1's arc flash study data is incomplete")));
}

#[test]
fn changed_circuits_show_and_print_again() {
    let mut s = session();
    let d = dir("changed");
    let out = d.join("labels.csv");
    let r = s.execute("labels.export", &json!({"path": out.display().to_string()})).unwrap();
    assert_eq!(r["labels"], 8, "{r}");
    let csv = std::fs::read_to_string(&out).unwrap();
    assert!(csv.lines().any(|l| l == "devices,dev:R-101-1,0.5x1.75,12,P1-1"), "{csv}");
    assert!(csv.contains("nameplates,np:P1,2x4,,P1,480Y/277V,225A MLO,FED FROM MDP"), "{csv}");
    let l = s.execute("labels.list", &json!({})).unwrap();
    assert!(l["labels"].as_array().unwrap().iter().all(|x| x["status"] == "printed"));
    // Circuit 2 now feeds the corridor: its directory changes, its device label doesn't.
    let mut sp = spec();
    sp["mep"]["equipment"][1]["circuits"][1]["description"] = json!("Corridor lighting");
    sp["mep"]["labels_printed"] = s.execute("bspec.get", &json!({})).unwrap()["spec"]["mep"]["labels_printed"].clone();
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
    let l = s.execute("labels.list", &json!({})).unwrap();
    assert_eq!(l["changed"], json!(["dir:P1"]), "{l}");
    let again = s.execute("labels.export", &json!({"path": out.display().to_string(), "changed_only": true})).unwrap();
    assert_eq!(again["labels"], 1);
    assert!(std::fs::read_to_string(&out).unwrap().contains("dir:P1"));
    assert!(s.execute("labels.export", &json!({"path": out.display().to_string(), "changed_only": true})).is_err(), "nothing changed now");
}

#[test]
fn the_sheets_render_and_the_bom_lines_come_with_them() {
    let mut s = session();
    s.execute("labels.arcflash", &json!({"rows": [
        {"equipment": "MDP", "incident_energy": 8.2, "working_distance_in": 18, "arc_flash_boundary_in": 52, "ppe": "Category 2", "voltage": "480 V"},
        {"equipment": "P1", "incident_energy": 1.4, "working_distance_in": 18, "arc_flash_boundary_in": 19, "ppe": "Category 1", "voltage": "480 V"}
    ]})).unwrap();
    let d = dir("sheets");
    let png = d.join("labels.png");
    let r = s.execute("labels.export", &json!({"path": png.display().to_string(), "mark": false})).unwrap();
    // A directory page, a sheet of 2x4s, a sheet of device labels, a sheet of arc flash labels.
    assert_eq!(r["pages"], 4, "{r}");
    let paths: Vec<String> = r["paths"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect();
    for p in &paths {
        let b = std::fs::read(p).unwrap();
        assert!(b.starts_with(&[0x89, b'P', b'N', b'G']) && b.len() > 2000, "{p}");
    }
    if let Ok(keep) = std::env::var("LABEL_SHOTS") {
        for p in &paths {
            std::fs::copy(p, std::path::Path::new(&keep).join(std::path::Path::new(p).file_name().unwrap())).unwrap();
        }
    }
    let dxf = d.join("labels.dxf");
    s.execute("labels.export", &json!({"path": dxf.display().to_string(), "set": "arc_flash", "mark": false})).unwrap();
    let back = cadaddy_io::read(&std::fs::read(&dxf).unwrap(), "labels.dxf").unwrap();
    let texts: Vec<String> =
        back.model.iter().filter_map(|e| if let cadaddy_engine::doc::EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None }).collect();
    assert!(texts.iter().any(|t| t == "INCIDENT ENERGY: 1.4 CAL/CM² AT 18 IN"), "{texts:?}");
    // PDF needs the app's plotter (not in this test build): said plainly.
    let e = s.execute("labels.export", &json!({"path": d.join("l.pdf").display().to_string()})).unwrap_err().to_string();
    assert!(e.contains("PDF export isn't available"), "{e}");
    // The BOM lines: printer, tape, stock, directory card, an arc flash label per labeled item.
    let b = s.execute("labels.bom", &json!({})).unwrap();
    let items: Vec<(String, f64)> =
        b["items"].as_array().unwrap().iter().map(|i| (i["item"].as_str().unwrap().to_string(), i["qty"].as_f64().unwrap())).collect();
    assert_eq!(
        items,
        [
            ("Label printer (tool)".to_string(), 1.0),
            ("Label tape cartridge, 12 mm".into(), 1.0),
            ("Equipment labels and nameplates, 2\" x 4\"".into(), 3.0),
            ("Panel directory card, typed".into(), 1.0),
            ("Arc flash label, 4\" x 6\"".into(), 3.0)
        ]
    );
    assert!(b["items"][4]["note"].as_str().unwrap().contains("1 waiting for study data"));
    // On the MEP takeoff too.
    let q = s.execute("mep.qty", &json!({})).unwrap();
    assert_eq!(q["labels"].as_array().unwrap().len(), 5);
    assert!(q["csv"].as_str().unwrap().lines().any(|l| l.starts_with("Labels,,,Label printer (tool)")));
    // Hostile.
    for (c, p) in [
        ("labels.list", json!({"set": "stickers"})),
        ("labels.export", json!({})),
        ("labels.export", json!({"path": "x.gif"})),
        ("labels.arcflash", json!({})),
        ("labels.arcflash", json!({"path": d.join("missing.csv").display().to_string()})),
    ] {
        assert!(s.execute(c, &p).is_err(), "{c} {p}");
    }
    let r = s
        .execute(
            "labels.arcflash",
            &json!({"rows": [{"equipment": "P1", "incident_energy": "1e400", "working_distance_in": "-3"}, {"nothing": 1}, 7]}),
        )
        .unwrap();
    assert_eq!(r["incomplete"], json!(["P1"]));
    assert!(Session::new().execute("labels.list", &json!({})).is_err(), "no building spec");
}

/// The study's own column names: the arc flash boundary comes from the AFB column, never from the
/// limited or restricted approach boundary; a row without a boundary (or a voltage) makes no label
/// and is an "incomplete" failure with a document request.
#[test]
fn arc_flash_boundary_is_read_from_its_own_column_and_never_defaulted() {
    let mut s = session();
    let d = dir("afb");
    let csv = d.join("study.csv");
    std::fs::write(
        &csv,
        "Bus,IE (cal/cm2),WD (in),AFB (in),Limited Approach Boundary,Restricted Approach Boundary,Voltage\n\
         MDP,8.2,18,52,42 in,12 in,480 V\n\
         P1,1.4,18,,42 in,12 in,480 V\n\
         DS-1,3.1,18,33,42 in,12 in,\n",
    )
    .unwrap();
    let r = s.execute("labels.arcflash", &json!({"path": csv.display().to_string()})).unwrap();
    assert_eq!((r["imported"].as_u64(), r["incomplete"].clone()), (Some(3), json!(["P1", "DS-1"])), "{r}");
    let l = s.execute("labels.list", &json!({})).unwrap();
    let lines: Vec<&str> = label(&l, "af:MDP")["lines"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert_eq!(
        lines[4..],
        [
            "INCIDENT ENERGY: 8.2 CAL/CM² AT 18 IN",
            "ARC FLASH BOUNDARY: 52 IN",
            "NOMINAL VOLTAGE: 480 V",
            "LIMITED APPROACH: 42 IN",
            "RESTRICTED APPROACH: 12 IN"
        ]
    );
    // No label for P1 (no boundary) or DS-1 (no voltage), and no label anywhere prints a 0.
    let af: Vec<&str> = l["labels"].as_array().unwrap().iter().filter(|x| x["set"] == "arc_flash").map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(af, ["af:MDP"]);
    assert!(l["labels"].as_array().unwrap().iter().flat_map(|x| x["lines"].as_array().unwrap()).all(|t| !t.as_str().unwrap().contains(": 0 ")));
    let problems: Vec<String> = l["problems"].as_array().unwrap().iter().map(|p| p["message"].as_str().unwrap().to_string()).collect();
    assert!(problems.iter().any(|p| p.starts_with("P1's arc flash study data is incomplete (missing arc flash boundary)")), "{problems:#?}");
    assert!(problems.iter().any(|p| p.starts_with("DS-1's arc flash study data is incomplete (missing nominal voltage)")), "{problems:#?}");
    let c = s.execute("labels.check", &json!({"review": true})).unwrap();
    let rfis = s.execute("rfi.list", &json!({"kind": "label_data"})).unwrap();
    assert_eq!(c["rfis"].as_array().unwrap().len(), rfis["total"].as_u64().unwrap() as usize);
    assert!(
        rfis["items"].as_array().unwrap().iter().any(|i| i["issue"].as_str().unwrap().contains("P1's arc flash study data is incomplete")),
        "{rfis}"
    );
    // Rows given directly: a zero boundary is no boundary.
    let r = s
        .execute("labels.arcflash", &json!({"rows": [{"equipment": "MDP", "incident_energy": 8.2, "working_distance_in": 18, "arc_flash_boundary_in": 0, "voltage": "480 V"}]}))
        .unwrap();
    assert_eq!(r["incomplete"], json!(["MDP"]));
    let l = s.execute("labels.list", &json!({"set": "arc_flash"})).unwrap();
    assert_eq!(l["labels"].as_array().unwrap().len(), 0, "{l}");
}

/// `labels.read`: a panel schedule drawn as text (with a room/area column) and the circuits
/// printed beside devices become the panel's circuits and the devices, each with its sheet.
#[test]
fn panel_schedules_and_device_circuits_read_off_a_sheet() {
    let mut s = session();
    let t = |s: &mut Session, at: [f64; 2], text: &str| {
        s.execute("text", &json!({"at": at, "text": text, "height": 1})).unwrap();
    };
    t(&mut s, [0.0, 100.0], "PANEL L2 SCHEDULE");
    for (x, h) in [(0.0, "CKT"), (5.0, "DESCRIPTION"), (30.0, "ROOM/AREA"), (45.0, "BKR"), (52.0, "POLES"), (60.0, "TYPE")] {
        t(&mut s, [x, 97.0], h);
    }
    for (i, row) in [
        ["1", "LIGHTING - LOBBY", "LOBBY", "20", "1", "-"],
        ["2", "RECEP - OFFICE", "OFFICE 201", "20", "1", "GFCI"],
        ["3", "AHU-2", "ROOF", "30", "3", "-"],
    ]
    .iter()
    .enumerate()
    {
        for (x, v) in [0.0, 5.0, 30.0, 45.0, 52.0, 60.0].iter().zip(row) {
            t(&mut s, [*x, 94.0 - 3.0 * i as f64], v);
        }
    }
    // On the plan: two devices on L2, one on P1, and a grid label that isn't a circuit.
    t(&mut s, [200.0, 50.0], "L2-2");
    t(&mut s, [220.0, 50.0], "L2-11");
    t(&mut s, [240.0, 40.0], "P1-1");
    t(&mut s, [260.0, 40.0], "B-2");
    let r = s.execute("labels.read", &json!({"sheet": "E-201", "level": "L1", "apply": true})).unwrap();
    assert_eq!(r["added_panels"], json!(["L2"]), "{r}");
    assert_eq!(
        r["schedules"][0]["circuits"],
        json!([
            {"number": "1", "description": "LIGHTING - LOBBY", "room": "LOBBY", "breaker": "20", "poles": 1, "source": "E-201"},
            {"number": "2", "description": "RECEP - OFFICE", "room": "OFFICE 201", "breaker": "20", "poles": 1, "source": "E-201"},
            {"number": "3", "description": "AHU-2", "room": "ROOF", "breaker": "30", "poles": 3, "source": "E-201"}
        ])
    );
    let devs: Vec<(String, String, String)> = r["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["panel"].as_str().unwrap().into(), d["circuit"].as_str().unwrap().into(), d["source"].as_str().unwrap().into()))
        .collect();
    assert_eq!(
        devs,
        [("L2".into(), "2".into(), "E-201".into()), ("L2".into(), "11".into(), "E-201".into()), ("P1".into(), "1".into(), "E-201".into())]
    );
    let l = s.execute("labels.list", &json!({})).unwrap();
    let rows: Vec<&str> = l["directories"]["L2"].as_array().unwrap().iter().map(|r| r["circuit"].as_str().unwrap()).collect();
    assert_eq!(rows, ["1", "2", "3,5,7"]);
    assert_eq!(l["directories"]["L2"][1]["room"], "RM OFFICE 201");
    assert_eq!(label(&l, "dev:E-201-D001")["lines"], json!(["L2-2"]));
    assert!(l["problems"].as_array().unwrap().iter().any(|p| p["message"] == "E-201-D002 is on L2-11, which L2's schedule doesn't have"), "{l}");
    // Read again: the sheet's devices are replaced, the hand-entered ones stay.
    s.execute("labels.read", &json!({"sheet": "E-201", "apply": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let devices = sp["spec"]["mep"]["devices"].as_array().unwrap();
    assert_eq!((devices.len(), devices.iter().filter(|d| d["source"] == "E-201").count()), (7, 3));
    // Hostile.
    for p in [json!({"paths": "x"}), json!({"paths": [7]}), json!({"path": "missing-sheet.dxf"})] {
        assert!(s.execute("labels.read", &p).is_err(), "{p}");
    }
}
