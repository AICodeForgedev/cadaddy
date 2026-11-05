//! Q6196 bidpackage: the one-page proposal letter `bim.bid {letter}` plots (the app's plotter),
//! CADaddy's branding only and a name only where the user entered it.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn receptacle(n: usize, x: f64, y: f64, circuit: &str) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x2F0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "mount_height_in": {"value": "18", "source": "note"}
    }})
}

/// A luminaire of type A on `circuit`.
fn fixture(n: usize, x: f64, y: f64, circuit: &str) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "luminaire", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"}
    }})
}

/// Another device of `kind`, on `circuit` when one is given.
fn device(n: usize, kind: &str, x: f64, y: f64, circuit: &str) -> Value {
    let mut v = json!({"id": format!("device:L1:{n}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": {}});
    if !circuit.is_empty() {
        v["fields"]["circuit"] = json!({"value": circuit, "source": "plan"});
    }
    v
}

fn circuit(id: &str, label: &str, panel: &str) -> Value {
    json!({"id": id, "kind": "circuit", "level": "L1", "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": label, "source": "plan"},
        "panel": {"value": panel, "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "conductors": {"value": "2#12 + 1#12G", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

/// The specification's wiring-methods article (made up, in the MasterFormat layout).
const SPEC: &str = "PART 3 - EXECUTION\n3.2\nWIRING METHODS\nA.\nExposed Feeders: Type THHN/THWN-2, single conductors in raceway.\nB.\nFeeders Concealed in Ceilings, Walls, and Partitions: Type THHN/THWN-2, single conductors in raceway.\nC.\nExposed Branch Circuits: Type THHN/THWN-2, single conductors in raceway.\nD.\nBranch Circuits Concealed in Ceilings, Walls, and Partitions: Type THHN/THWN-2, single conductors in raceway, Metal-clad cable, Type MC, Nonmetallic-sheathed cable, Type NM.\nE.\nBranch Circuits below Slabs-on-Grade: single conductors in raceway.\n3.3\nINSTALLATION\nA.\nConceal cables.";

/// A made-up one-storey building: two units (rooms 101 and 102) with a panel each of the
/// typical schedule U, fed from DPA by feeder 1 (aluminum 2/0, 2" EMT); circuit U-1 in each
/// unit; a common-area circuit LP1-1 and a service-room circuit LP1-3 on panel LP1.
fn building(dir: &std::path::Path) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let tags = vec![
        receptacle(1, 5.0, 5.0, "U-1"),
        receptacle(2, 12.0, 5.0, "U-1"),
        receptacle(3, 45.0, 5.0, "U-1"),
        receptacle(4, 52.0, 5.0, "U-1"),
        receptacle(5, 20.0, -20.0, "LP1-1"),
        receptacle(6, 30.0, -20.0, "LP1-1"),
        receptacle(7, 30.0, -40.0, "LP1-3"),
        circuit("circuit:L1:room-101:U-1", "U-1", "U"),
        circuit("circuit:L1:E-101:LP1-1", "LP1-1", "LP1"),
        circuit("circuit:L1:E-101:LP1-3", "LP1-3", "LP1"),
        circuit("circuit:L1:E-101:LP1-5", "LP1-5", "LP1"),
        fixture(8, 20.0, -30.0, "LP1-5"),
        fixture(9, 30.0, -30.0, "LP1-5"),
        fixture(10, 40.0, -30.0, "LP1-5"),
        device(11, "switch", 18.0, -24.0, "LP1-5"),
        device(12, "smoke_detector", 25.0, -28.0, ""),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        // A lay-in ceiling over the common area's receptacles (the RCP's grid).
        "rcp": {"ceilings": [{"level": "L1", "outline": [[100, -300], [400, -300], [400, -180], [100, -180]], "height_ft": 9, "grid": {"spacing_in": [24, 24], "origin": [0, 0]}}]},
        "mep": {
            "systems": [{"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [2, 2], "bottom_ft": 10, "show": true}],
            "panel_schedules": [
                {"panel": "U", "typical_of": 2, "circuits": [{"number": "1", "description": "RECEPTACLES", "breaker": "20", "poles": 1}]},
                {"panel": "DPA", "circuits": [{"number": "1", "description": "PANEL U (2 UNITS)", "breaker": "125", "poles": 2}]},
                {"panel": "LP1", "circuits": [{"number": "1", "description": "RECEPTACLES", "breaker": "20", "poles": 1}, {"number": "3", "description": "RECEPTACLES", "breaker": "20", "poles": 1}, {"number": "5", "description": "LIGHTING", "breaker": "20", "poles": 1}]}
            ],
            "feeders": [
                {"tag": "1", "from": "DPA", "to": "U", "phases": 2, "neutrals": 1, "grounds": 1, "size": "2/0", "ground_size": "4", "material": "al", "conduits": 1, "conduit_in": 2, "sets": 1, "source": "FEEDER SCHEDULE E-601"}
            ],
            "runs": [
                {"system": "E-FEEDER", "level": "L1", "source_sheet": "feeder 1 DPA→U-1A", "points": [[-14, -600], [84, -600], [84, 1200]], "size_in": [2, 2], "bottoms_ft": [10, 10]},
                {"system": "E-FEEDER", "level": "L1", "source_sheet": "feeder 1 DPA→U-1B", "points": [[-14, -600], [580, -600], [580, 1200]], "size_in": [2, 2], "bottoms_ft": [10, 10]}
            ],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5},
                {"tag": "DPA", "kind": "panel", "level": "L1", "min": [-24, -606], "max": [-4, -594], "height_ft": 4, "bottom_ft": 2.5},
                {"tag": "U-1A", "kind": "panel", "level": "L1", "min": [0, 1200], "max": [14, 1204], "height_ft": 2, "bottom_ft": 4.5},
                {"tag": "U-1B", "kind": "panel", "level": "L1", "min": [480, 1200], "max": [494, 1204], "height_ft": 2, "bottom_ft": 4.5}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    // The service room: LP1-3's receptacle stands in an electrical room (the rooms' names are the
    // documents'; here set on the device as the build's room fill would).
    {
        let d = s.doc_mut().expect("doc");
        let hs: Vec<_> = d
            .model
            .iter()
            .filter(|e| {
                let p = cadaddy_engine::doc::props::read(&e.common.xdata);
                p.get("circuit").map(|x| x.text()).as_deref() == Some("LP1-3") && p.get("atom").is_none()
            })
            .map(|e| e.handle)
            .collect();
        for h in hs {
            let _ = d.modify_entity(h, |e| {
                let mut p = cadaddy_engine::doc::props::read(&e.common.xdata);
                p.set("room", cadaddy_engine::doc::props::Prop::new("ELEC 105", cadaddy_engine::doc::props::Source::computed()));
                e.common.xdata = cadaddy_engine::doc::props::write(&e.common.xdata, &p).into();
            });
        }
    }
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    std::fs::write(dir.join("260519 - Conductors.txt"), SPEC).expect("spec text");
    s
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-bid-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn one_page(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    bytes.starts_with(b"%PDF") && text.contains("/Count 1") && !text.contains("/Count 2")
}

#[test]
fn the_letter_is_one_page_with_the_names_only_the_user_entered() {
    hooks();
    let dir = tmp("letter");
    let mut s = building(&dir);
    let pdf = dir.join("proposal.pdf");
    let allow = json!([{"name": "Temporary power", "amount": 500}]);
    let v = s
        .execute(
            "bim.bid",
            &json!({"letter": pdf.to_string_lossy(), "alternates": false, "labor_rate": 80, "allowances": allow, "today": "2026-11-04"}),
        )
        .expect("bid");
    assert_eq!(v["letter"].as_str(), Some(pdf.to_string_lossy().as_ref()));
    let plain = std::fs::read(&pdf).expect("pdf");
    assert!(one_page(&plain));
    // With the names the user typed (kept for the next run), still one page; a file there is
    // refused without force.
    let named = dir.join("named.pdf");
    let v = s
        .execute("bim.bid", &json!({"letter": named.to_string_lossy(), "alternates": false, "project": "Project A", "client": "Sample General Contractor", "bidder": "Sample Electric"}))
        .expect("named");
    assert!(one_page(&std::fs::read(&named).expect("named pdf")));
    assert_eq!(v["base_bid"]["allowances"], json!(500.0), "the allowance kept from the first run");
    assert!(s.execute("bim.bid", &json!({"letter": named.to_string_lossy(), "alternates": false})).is_err());
    assert!(s.execute("bim.bid", &json!({"letter": named.to_string_lossy(), "alternates": false, "force": true})).is_ok());
    // Many alternates' and scope lines never push it to a second page.
    assert!(v["clarifications"].as_array().is_some_and(|c| !c.is_empty()));
    std::fs::remove_dir_all(&dir).ok();
}
