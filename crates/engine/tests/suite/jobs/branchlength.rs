//! Q6340: a branch circuit's conductors are the larger of its documents' sizes (the plan's note,
//! the panel schedule's wire column) and what the documents' own wire-length table asks for its
//! routed length at its breaker and voltage, else the voltage drop; each upsizing a correction
//! with the design value kept. A circuit's boxes chained past what its conductors reach branch
//! instead, so its farthest box stays near the room's own size.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

fn receptacle(n: usize, x: f64, y: f64, circuit: &str) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x2F0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "mount_height_in": {"value": "18", "source": "note"}
    }})
}

fn circuit(label: &str, conductors: Option<&str>) -> Value {
    let (mk, src) = conductors.map_or(("2#12 + 1#12G", "default"), |c| (c, "plan"));
    json!({"id": format!("circuit:L1:E-101:{label}"), "kind": "circuit", "level": "L1", "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": label, "source": "plan"},
        "panel": {"value": "LP1", "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "conductors": {"value": mk, "source": src},
        "conduit": {"value": "3/4\" EMT", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

/// A made-up one-storey building (12' floor to floor, 9' ceiling) with panel LP1 (208Y/120 V)
/// at the west end, the `tags` given and `rows` of LP1's schedule; `draw` adds to the drawing
/// before the build.
fn job(tags: Vec<Value>, rows: Value, draw: impl FnOnce(&mut Session)) -> Session {
    job_with(tags, rows, json!({}), draw)
}

/// [`job`] with `mep`'s keys added to the spec's MEP (what a job keeps there at read time).
fn job_with(tags: Vec<Value>, rows: Value, mep: Value, draw: impl FnOnce(&mut Session)) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    draw(&mut s);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        "mep": {
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
            ],
            "panel_schedules": [{"panel": "LP1", "source": "E-101", "header": {"volts/phase": "208Y/120V, 3PH, 4W"}, "circuits": rows}]
        }
    });
    let mut spec = spec;
    for (k, v) in mep.as_object().into_iter().flatten() {
        spec["mep"][k] = v.clone();
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

fn text_at(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": at, "text": t, "height": h})).expect("text");
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).expect("line");
}

/// A drawn grid at (x0, y0) (sheet inches, far from the building): its title, a header row and
/// body rows, each cell 20" wide.
fn grid(s: &mut Session, x0: f64, y0: f64, title: &str, header: &[&str], rows: &[&[&str]]) {
    text_at(s, [x0 + 10.0, y0 + 10.0], title, 2.0);
    let n = header.len();
    let ys: Vec<f64> = (0..=rows.len() + 1).map(|i| y0 + 7.0 - 5.0 * i as f64).collect();
    let bottom = ys.last().copied().unwrap_or(y0);
    for y in &ys {
        line(s, [x0, *y], [x0 + 20.0 * n as f64, *y]);
    }
    for i in 0..=n {
        let x = x0 + 20.0 * i as f64;
        line(s, [x, y0 + 7.0], [x, bottom]);
    }
    for (i, h) in header.iter().enumerate() {
        text_at(s, [x0 + 2.0 + 20.0 * i as f64, y0 + 4.0], h, 1.5);
    }
    for (r, row) in rows.iter().enumerate() {
        for (i, c) in row.iter().enumerate() {
            text_at(s, [x0 + 2.0 + 20.0 * i as f64, y0 - 1.0 - 5.0 * r as f64], c, 1.5);
        }
    }
}

/// Sizes across, a breaker and a voltage per row: 20 A / 120 V takes #12 to 50 ft, #10 to
/// 100 ft, #8 to 150 ft, #6 to 250 ft.
fn by_breaker_table(s: &mut Session) {
    grid(
        s,
        3000.0,
        -200.0,
        "BRANCH CIRCUIT VOLTAGE DROP WIRING SCHEDULE",
        &["BREAKER", "VOLTAGE", "#12", "#10", "#8", "#6"],
        &[&["20A", "120V", "50'", "100'", "150'", "250'"]],
    );
}

/// Sizes down, a column per voltage (no breaker): at 120 V #12 to 40 ft, #10 to 65 ft, #8 to
/// 105 ft.
fn by_voltage_table(s: &mut Session) {
    grid(
        s,
        -3000.0,
        600.0,
        "Wire Length Table",
        &["CONDUCTOR", "120V", "208V"],
        &[&["#12", "40", "70"], &["#10", "65", "110"], &["#8", "105", "175"]],
    );
}

fn spec_tag(s: &mut Session, id: &str) -> Value {
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let spec = s.execute("bspec.get", &json!({})).expect("spec");
    let spec = spec.get("spec").cloned().unwrap_or(spec);
    spec["tags"].as_array().into_iter().flatten().find(|t| t["id"] == json!(id)).cloned().unwrap_or_default()
}

/// The sizes of circuit `c`'s run wire (not the pigtails a box lands a device on, NEC 110.14).
fn wire_sizes(s: &mut Session, c: &str) -> Vec<String> {
    let d = s.doc().expect("doc");
    let mut out: Vec<String> = d
        .model
        .iter()
        .map(|e| cadaddy_engine::doc::props::read(&e.common.xdata))
        .filter(|p| {
            let t = |k: &str| p.get(k).map(|x| x.text()).unwrap_or_default();
            t("atom") == "wire" && t("circuit") == c && t("pigtail_of").is_empty()
        })
        .filter_map(|p| p.get("size").map(|x| x.text()))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn size_for(ft: f64, bands: &[(f64, &str)]) -> String {
    bands.iter().find(|(hi, _)| ft <= *hi).map_or_else(|| "beyond".to_string(), |b| b.1.to_string())
}

#[test]
fn a_drawn_wire_length_table_by_breaker_and_voltage_sizes_the_whole_routed_circuit() {
    // LP1-5's one receptacle 130 ft east on a 20 A, 120 V breaker with a light 600 VA load: the
    // 3% drop alone would keep #12 (or upsize only its homerun); the drawing's table asks the
    // size of its band for the routed length, in the whole circuit, with the design kept.
    // LP1-3 is on a 30 A breaker: the table has no row for it, the voltage drop decides.
    let tags = vec![receptacle(1, 130.0, 10.0, "LP1-5"), receptacle(2, 60.0, -10.0, "LP1-3"), circuit("LP1-5", None), circuit("LP1-3", None)];
    let rows = json!([
        {"number": "5", "description": "RECEPTACLES", "breaker": "20", "poles": 1, "load_va": 600.0, "source": "E-101"},
        {"number": "3", "description": "RECEPTACLES", "breaker": "30", "poles": 1, "load_va": 600.0, "source": "E-101"}
    ]);
    let mut s = job(tags, rows, by_breaker_table);
    let r = s.execute("conduit.route", &json!({})).expect("report");
    eprintln!("Q6340DUMP table-sizing upsized={} sizes5={:?}", r["conductors_upsized"], wire_sizes(&mut s, "LP1-5"));
    assert!(r["wire_size_table"]["read"].as_str().is_some_and(|t| t.contains("VOLTAGE DROP WIRING SCHEDULE")), "{}", r["wire_size_table"]);
    let up = r["conductors_upsized"].as_array().cloned().unwrap_or_default();
    let five = up.iter().find(|u| u["circuit"] == json!("LP1-5") && u["reason"] == json!("the drawing's length table")).cloned();
    let five = five.unwrap_or_else(|| panic!("LP1-5 sized by the table: {up:#?}"));
    let ft = five["feet"].as_f64().unwrap_or_default();
    let want = size_for(ft, &[(50.0, "12"), (100.0, "10"), (150.0, "8"), (250.0, "6")]);
    assert!(ft > 100.0 && ft <= 250.0, "{five}");
    assert_eq!(five["to"], json!(format!("2#{want} + 1#{want}G")), "{five}");
    let why = five["why"].as_str().unwrap_or_default();
    assert!(why.contains("20 A / 120 V") && why.contains("VOLTAGE DROP WIRING SCHEDULE"), "{why}");
    // The whole circuit pulls the table's size.
    assert_eq!(wire_sizes(&mut s, "LP1-5"), vec![format!("#{want}")]);
    // Corrected for the field: the design value beside it, the rule and reason.
    let t = spec_tag(&mut s, "circuit:L1:E-101:LP1-5");
    assert_eq!(t["fields"]["design:conductors"]["value"], json!("2#12 + 1#12G"), "{t}");
    assert!(t["fields"]["correction:conductors"]["value"].as_str().is_some_and(|c| c.starts_with("E-VOLTAGE-DROP")), "{t}");
    let p = s.execute("bim.provenance", &json!({})).expect("provenance");
    assert!(p["runs"]["circuits_corrected"].as_u64().is_some_and(|n| n >= 1), "{}", p["runs"]);
    // LP1-3 on its 30 A breaker: no row in the table, never sized by it.
    assert!(!up.iter().any(|u| u["circuit"] == json!("LP1-3") && u["reason"] == json!("the drawing's length table")), "{up:#?}");
}

#[test]
fn a_pdf_jobs_length_table_on_a_page_the_drawing_lacks_sizes_the_routed_circuit() {
    // Q6545: a PDF job's wire-length table is on a legend page the job's drawing doesn't hold
    // (it imported only the plans). The job keeps the table it read off that page in the spec;
    // model.build and conduit.reroute size by it with no session. LP1-5:
    // one receptacle ~90 ft of conduit out on a 20 A, 120 V breaker, a light 300 VA load (the 3%
    // drop alone keeps #12). The drawing holds no table at all.
    let tags = vec![receptacle(1, 75.0, 10.0, "LP1-5"), circuit("LP1-5", None)];
    let rows = json!([{"number": "5", "description": "RECEPTACLES", "breaker": "20", "poles": 1, "load_va": 300.0, "source": "E-101"}]);
    let band = |size: &str, max_ft: f64| json!({"amps": 20.0, "volts": 120.0, "size": size, "max_ft": max_ft});
    let kept = json!({
        "wire_length_tables": [{"sheet": "E-001", "title": "BRANCH CIRCUIT VOLTAGE DROP WIRING SCHEDULE",
            "bands": [band("12", 55.0), band("10", 140.0), band("8", 230.0)]}],
        "wire_length_read": "1 job pages looked at (E-001): 1 tables"
    });
    let mut s = job_with(tags, rows, kept, |_| {});
    let r = s.execute("conduit.route", &json!({})).expect("report");
    let table = &r["wire_size_table"];
    assert!(table["read"].as_str().is_some_and(|t| t.contains("VOLTAGE DROP WIRING SCHEDULE on E-001")), "{table}");
    assert!(table["from"].as_str().is_some_and(|f| f.contains("job's pages")), "{table}");
    let up = r["conductors_upsized"].as_array().cloned().unwrap_or_default();
    let five = up.iter().find(|u| u["circuit"] == json!("LP1-5") && u["reason"] == json!("the drawing's length table")).cloned();
    let five = five.unwrap_or_else(|| panic!("LP1-5 sized by the page's table: {up:#?} {r}"));
    let ft = five["feet"].as_f64().unwrap_or_default();
    assert!(ft > 55.0 && ft <= 140.0, "{five}");
    assert_eq!(five["to"], json!("2#10 + 1#10G"), "{five}");
    assert_eq!(wire_sizes(&mut s, "LP1-5"), vec!["#10".to_string()]);
    // Rerouted (the job's last routing): the same, the kept table still in the spec.
    let t = spec_tag(&mut s, "circuit:L1:E-101:LP1-5");
    assert_eq!(t["fields"]["design:conductors"]["value"], json!("2#12 + 1#12G"), "{t}");
    assert_eq!(wire_sizes(&mut s, "LP1-5"), vec!["#10".to_string()]);
    let spec = s.execute("bspec.get", &json!({})).expect("spec");
    let spec = spec.get("spec").cloned().unwrap_or(spec);
    assert_eq!(spec["mep"]["wire_length_tables"].as_array().map(Vec::len), Some(1), "{}", spec["mep"]);
    // Validate reads it by the same rule: `validateconduit::a_length_table_the_job_kept_off_its_pages_is_validates_and_the_routers`.
}

#[test]
fn a_breaker_the_schedule_doesnt_give_is_read_at_the_usual_branch_breaker() {
    // No schedule row for LP1-7: the table by breaker is read at the rules' usual 20 A branch
    // breaker, and the reason says so.
    let tags = vec![receptacle(1, 120.0, 12.0, "LP1-7"), circuit("LP1-7", None)];
    let mut s = job(tags, json!([]), by_breaker_table);
    let r = s.execute("conduit.route", &json!({"section": "conductors_upsized"})).expect("report");
    let up = r["conductors_upsized"].as_array().cloned().unwrap_or_default();
    let seven = up.iter().find(|u| u["circuit"] == json!("LP1-7")).cloned().unwrap_or_default();
    assert_eq!(seven["reason"], json!("the drawing's length table"), "{up:#?}");
    assert!(seven["why"].as_str().is_some_and(|w| w.contains("breaker not read") && w.contains("20 A")), "{seven}");
}

#[test]
fn the_schedules_wire_size_over_the_plans_note_is_pulled_with_the_plans_kept() {
    // The plan's note gives LP1-9 #10; its schedule row gives #8: the field pulls the larger, the
    // plan's kept as the design value with the rule.
    let tags = vec![receptacle(1, 12.0, 6.0, "LP1-9"), receptacle(2, 18.0, 6.0, "LP1-9"), circuit("LP1-9", Some("2#10 + 1#10G"))];
    let rows = json!([{"number": "9", "description": "RECEPTACLES", "breaker": "20", "poles": 1, "wire": "#8", "load_va": 360.0, "source": "E-101"}]);
    let mut s = job(tags, rows, |_| {});
    eprintln!("Q6340DUMP sched-over-plan sizes9={:?}", wire_sizes(&mut s, "LP1-9"));
    assert_eq!(wire_sizes(&mut s, "LP1-9"), vec!["#8".to_string()]);
    let r = s.execute("conduit.route", &json!({"section": "conductors_upsized"})).expect("report");
    let up = r["conductors_upsized"].as_array().cloned().unwrap_or_default();
    let nine = up.iter().find(|u| u["circuit"] == json!("LP1-9")).cloned().unwrap_or_default();
    assert_eq!(nine["reason"], json!("the panel schedule's wire size over the plan's"), "{up:#?}");
    let t = spec_tag(&mut s, "circuit:L1:E-101:LP1-9");
    assert_eq!(t["fields"]["conductors"]["value"], json!("2#8 + 1#8G"), "{t}");
    assert_eq!(t["fields"]["design:conductors"]["value"], json!("2#10 + 1#10G"), "{t}");
    assert!(t["fields"]["correction:conductors"]["value"].as_str().is_some_and(|c| c.contains("E-CONDUCTOR-LARGER-DOC")), "{t}");
    // A plan size at or over the schedule's stays the plan's, uncorrected.
    let tags = vec![receptacle(1, 12.0, 6.0, "LP1-9"), circuit("LP1-9", Some("2#10 + 1#10G"))];
    let rows =
        json!([{"number": "9", "description": "RECEPTACLES", "breaker": "20", "poles": 1, "wire": "#12", "load_va": 360.0, "source": "E-101"}]);
    let mut s = job(tags, rows, |_| {});
    assert_eq!(wire_sizes(&mut s, "LP1-9"), vec!["#10".to_string()]);
    let t = spec_tag(&mut s, "circuit:L1:E-101:LP1-9");
    assert!(t["fields"].get("design:conductors").is_none(), "{t}");
}

#[test]
fn a_rooms_circuit_chained_past_its_conductors_reach_branches_and_stays_near_the_rooms_size() {
    // A 30' x 14' room east of LP1 with eight receptacles round its walls on LP1-11 (20 A,
    // 120 V). Chained box to box its farthest box is far over what #12 reaches by the drawing's
    // table (40 ft at 120 V); its boxes branch instead, the farthest nearer, and its conductors
    // are the table's size for the branched length.
    let at = [(2.0, 7.0), (10.0, 7.0), (18.0, 7.0), (26.0, 7.0), (28.0, 0.0), (26.0, -7.0), (18.0, -7.0), (10.0, -7.0)];
    let mut tags: Vec<Value> = at.iter().enumerate().map(|(i, (x, y))| receptacle(i + 1, *x, *y, "LP1-11")).collect();
    tags.push(circuit("LP1-11", None));
    let rows = json!([{"number": "11", "description": "ROOM RECEPTACLES", "breaker": "20", "poles": 1, "load_va": 900.0, "source": "E-101"}]);
    let mut s = job(tags, rows, by_voltage_table);
    let r = s.execute("conduit.route", &json!({})).expect("report");
    {
        let t = spec_tag(&mut s, "circuit:L1:E-101:LP1-11");
        eprintln!("Q6340DUMP room branched={} home={} conductors={}", r["branched_trees"], t["fields"]["homerun_ft"], t["fields"]["conductors"]);
    }
    let b = &r["branched_trees"];
    assert_eq!(b["circuits"], json!(1), "{b}");
    let item = b["items"][0].clone();
    let (chained, branched) = (item["chained_ft"].as_f64().unwrap_or_default(), item["branched_ft"].as_f64().unwrap_or_default());
    assert!(branched > 0.0 && branched < chained, "{item}");
    assert!(item["by"].as_str().is_some_and(|w| w.contains("WIRE LENGTH TABLE")), "{item}");
    assert!(b["source"].as_str().is_some_and(|w| w.contains("trade practice")), "{b}");
    // Its routed length to the farthest box is the branched one, near the room's own size (its
    // half perimeter, 44', plus the drops), not the chain round the room.
    let t = spec_tag(&mut s, "circuit:L1:E-101:LP1-11");
    let home: f64 = t["fields"]["homerun_ft"]["value"].as_str().and_then(|v| v.parse().ok()).unwrap_or_default();
    assert!(home > 0.0 && home < chained - 10.0, "home {home} vs chained {chained}: {t}");
    assert!(t["fields"]["device_tree"]["value"].as_str().is_some_and(|w| w.contains("branched")), "{t}");
    // Its conductors: the table's size at 120 V for the branched length.
    let want = size_for(home, &[(40.0, "12"), (65.0, "10"), (105.0, "8")]);
    if want != "beyond" {
        assert_eq!(t["fields"]["conductors"]["value"], json!(format!("2#{want} + 1#{want}G")), "{home}: {t}");
    }
}
