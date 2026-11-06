//! Takeoff, schedules and checks use the elements' properties (Q6003): a made-up two-level model
//! whose elements carry their properties, grouped and filtered by any key in `bim.takeoff` and
//! `bim.bom`, scheduled by `bim.schedule` (CSV/XLSX), checked by `bim.props.check`, and the
//! property coverage a job reports.

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// An element as the build makes them: a box (feet; the drawing is in inches) with the build's
/// tag `[part, level, source, tags…]` and its property store.
fn el(d: &mut Drawing, part: &str, level: &str, layer: &str, lo: [f64; 3], hi: [f64; 3], ps: &[(&str, Value)]) -> Handle {
    el_src(d, part, level, layer, lo, hi, ps, Source::print("Sample Sheet E-101", None))
}

#[allow(clippy::too_many_arguments)]
fn el_src(d: &mut Drawing, part: &str, level: &str, layer: &str, lo: [f64; 3], hi: [f64; 3], ps: &[(&str, Value)], src: Source) -> Handle {
    let v = |x: f64, y: f64, z: f64| Vec3 { x: x * 12.0, y: y * 12.0, z: z * 12.0 };
    let vertices = vec![
        v(lo[0], lo[1], lo[2]),
        v(hi[0], lo[1], lo[2]),
        v(hi[0], hi[1], lo[2]),
        v(lo[0], hi[1], lo[2]),
        v(lo[0], lo[1], hi[2]),
        v(hi[0], lo[1], hi[2]),
        v(hi[0], hi[1], hi[2]),
        v(lo[0], hi[1], hi[2]),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }));
    e.common.layer = layer.into();
    let x = vec![
        RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
        RawTag { code: 1000, value: part.into() },
        RawTag { code: 1000, value: level.into() },
        RawTag { code: 1000, value: "0".into() },
    ];
    let mut store = Props::default();
    for (k, val) in ps {
        let mut p = Prop::new(val.clone(), src.clone());
        if *k == "mount_height" {
            p = p.with_unit("in");
        }
        store.set(k, p);
    }
    e.common.xdata = props::write(&x, &store).into();
    d.ensure_layer(layer);
    d.model.push(e);
    h
}

/// A device box at (x, y) feet on a level.
fn dev(d: &mut Drawing, level: &str, x: f64, y: f64, ps: &[(&str, Value)]) -> Handle {
    let z = if level == "L2" { 11.5 } else { 1.5 };
    el(d, "device", level, "E-POWER", [x, y, z], [x + 0.33, y + 0.17, z + 0.33], ps)
}

struct Hs {
    rated_door: Handle,
    plain_door: Handle,
    toilet_dup: Handle,
    no_circuit: Handle,
    rtu: Handle,
    exhaust_fan: Handle,
    fast_duct: Handle,
    damped_duct: Handle,
    unserved: Handle,
    unnumbered_room: Handle,
    floor_drain: Handle,
    branch: Handle,
    gas_branch: Handle,
    heater: Handle,
    conduit: Handle,
}

/// The made-up model: Level 1 offices and a toilet room with power, an exhaust fan, a rooftop
/// unit and its terminals; Level 2 a guest room, storage, plumbing fixtures, a water heater and
/// a boiler on gas.
fn model() -> (Session, Hs) {
    let mut d = Drawing::new_imperial();
    // Walls: a 1-hour wall at x = 20 and a plain wall at y = 30.
    let rated_wall =
        el(&mut d, "wall", "L1", "A-WALL", [19.75, 0.0, 0.0], [20.25, 30.0, 10.0], &[("type", json!("W2")), ("fire_rating", json!("1 HR"))]);
    let _plain_wall = el(&mut d, "wall", "L1", "A-WALL", [0.0, 29.75, 0.0], [40.0, 30.25, 10.0], &[("type", json!("W1"))]);
    let _ = rated_wall;
    // Rooms.
    el(
        &mut d,
        "room",
        "L1",
        "A-AREA",
        [0.0, 0.0, 0.0],
        [19.75, 29.75, 9.0],
        &[
            ("number", json!("101")),
            ("name", json!("OFFICE")),
            ("area", json!(200)),
            ("floor_finish", json!("VCT-1")),
            ("wall_finish", json!("PT-1")),
            ("ceiling_finish", json!("ACT-1")),
        ],
    );
    el(
        &mut d,
        "room",
        "L1",
        "A-AREA",
        [20.25, 0.0, 0.0],
        [40.0, 29.75, 9.0],
        &[("number", json!("102")), ("name", json!("TOILET")), ("area", json!(150))],
    );
    el(
        &mut d,
        "room",
        "L2",
        "A-AREA",
        [0.0, 0.0, 10.0],
        [19.75, 29.75, 19.0],
        &[("number", json!("201")), ("name", json!("GUEST ROOM")), ("area", json!(300))],
    );
    let unnumbered_room = el(&mut d, "room", "L2", "A-AREA", [20.25, 0.0, 10.0], [40.0, 29.75, 19.0], &[("name", json!("STORAGE"))]);
    // Doors: one in the rated wall with no rating, one in the plain wall.
    let rated_door = el(
        &mut d,
        "door",
        "L1",
        "A-DOOR",
        [19.6, 10.0, 0.0],
        [20.4, 13.0, 7.0],
        &[("number", json!("102A")), ("width", json!("3'-0\"")), ("height", json!("7'-0\""))],
    );
    let plain_door =
        el(&mut d, "door", "L1", "A-DOOR", [5.0, 29.6, 0.0], [8.0, 30.4, 7.0], &[("number", json!("101A")), ("fire_rating", json!("none"))]);
    // The panels.
    el(
        &mut d,
        "equipment",
        "L1",
        "E-EQPM",
        [0.0, 0.0, 0.0],
        [1.0, 0.5, 6.0],
        &[("tag", json!("LP1")), ("type", json!("Panelboard")), ("phases", json!(3))],
    );
    el(&mut d, "equipment", "L2", "E-EQPM", [0.0, 0.0, 10.0], [1.0, 0.5, 16.0], &[("tag", json!("LP2")), ("type", json!("Panelboard"))]);
    let dup = |c: &str, room: &str| -> Vec<(&'static str, Value)> {
        vec![
            ("type", json!("Duplex receptacle")),
            ("type_mark", json!("DR")),
            ("circuit", json!(c)),
            ("room", json!(room)),
            ("mount_height", json!(18)),
            ("breaker", json!("20A")),
            ("wire", json!("2#12, 1#12G")),
        ]
    };
    // Level 1: three duplexes in the office on LP1-1, two in the toilet room on LP1-3 (one GFCI).
    for i in 0..3 {
        dev(&mut d, "L1", 2.0 + 5.0 * f64::from(i), 0.5, &dup("LP1-1", "101"));
    }
    let toilet_dup = dev(&mut d, "L1", 25.0, 0.5, &dup("LP1-3", "102"));
    let mut gfci = dup("LP1-3", "102");
    gfci[0] = ("type", json!("GFCI duplex receptacle"));
    dev(&mut d, "L1", 30.0, 0.5, &gfci);
    // A duplex nobody circuited, its height assumed.
    let no_circuit = el_src(
        &mut d,
        "device",
        "L1",
        "E-POWER",
        [10.0, 20.0, 1.5],
        [10.33, 20.17, 1.83],
        &[("type", json!("Duplex receptacle")), ("room", json!("101")), ("mount_height", json!(18))],
        Source::of(SourceKind::Default),
    );
    // Two hand dryers on LP1-5, far from the panel, on #14 behind a 20 A breaker.
    for (x, y) in [(100.0, 0.0), (100.0, 50.0)] {
        dev(
            &mut d,
            "L1",
            x,
            y,
            &[
                ("type", json!("Hand dryer")),
                ("circuit", json!("LP1-5")),
                ("room", json!("102")),
                ("load_va", json!(1000)),
                ("breaker", json!("20A")),
                ("wire", json!("2#14, 1#14G")),
                ("mount_height", json!(42)),
            ],
        );
    }
    // LP1-7: a 150 ft home run of 1/2" conduit packed with ten #12.
    let conduit = el(
        &mut d,
        "conduit",
        "L1",
        "E-COND",
        [0.0, 40.0, 9.0],
        [150.0, 40.04, 9.04],
        &[("size", json!("1/2\"")), ("circuit", json!("LP1-7")), ("wire", json!("9#12, 1#12G"))],
    );
    // Level 2: two duplexes in the guest room on LP2-1.
    for i in 0..2 {
        dev(&mut d, "L2", 2.0 + 5.0 * f64::from(i), 0.5, &dup("LP2-1", "201"));
    }
    // Mechanical: a rooftop unit (1000 cfm, MOCP 30 behind a 40 A breaker), three diffusers of
    // 250 cfm it serves, one diffuser nobody serves, an exhaust fan with no circuit.
    let rtu = el(
        &mut d,
        "equipment",
        "L2",
        "M-EQPM",
        [30.0, 20.0, 20.0],
        [36.0, 26.0, 24.0],
        &[
            ("tag", json!("RTU-1")),
            ("type", json!("Rooftop unit")),
            ("cfm", json!(1000)),
            ("mocp", json!(30)),
            ("mca", json!(22)),
            ("breaker", json!("40A")),
            ("circuit", json!("LP1-20")),
        ],
    );
    for i in 0..3 {
        let x = 2.0 + 5.0 * f64::from(i);
        el(
            &mut d,
            "diffuser",
            "L1",
            "M-DIFF",
            [x, 15.0, 9.0],
            [x + 2.0, 17.0, 9.2],
            &[("type_mark", json!("SD-1")), ("size", json!("12x12")), ("cfm", json!(250)), ("served_by", json!("RTU-1")), ("room", json!("101"))],
        );
    }
    let unserved = el(
        &mut d,
        "diffuser",
        "L1",
        "M-DIFF",
        [25.0, 15.0, 9.0],
        [27.0, 17.0, 9.2],
        &[("type_mark", json!("EG-1")), ("type", json!("Exhaust grille")), ("cfm", json!(75))],
    );
    let exhaust_fan = el(
        &mut d,
        "equipment",
        "L1",
        "M-EQPM",
        [30.0, 25.0, 9.0],
        [31.0, 26.0, 10.0],
        &[("tag", json!("EF-1")), ("type", json!("Exhaust fan")), ("cfm", json!(75))],
    );
    // Ducts through the rated wall: one fast and undamped, one with a damper at the wall.
    let fast_duct = el(
        &mut d,
        "duct",
        "L1",
        "M-DUCT",
        [10.0, 20.0, 9.0],
        [30.0, 20.67, 9.67],
        &[("size", json!("12x8")), ("system", json!("Supply Air")), ("cfm", json!(2000))],
    );
    let damped_duct = el(
        &mut d,
        "duct",
        "L1",
        "M-DUCT",
        [10.0, 5.0, 9.0],
        [30.0, 5.83, 9.83],
        &[("size", json!("10x10")), ("system", json!("Supply Air")), ("cfm", json!(500)), ("insulation", json!("1\" wrap"))],
    );
    el(&mut d, "duct", "L1", "M-DUCT", [19.5, 4.9, 8.9], [20.5, 5.9, 9.9], &[("type", json!("Fire damper")), ("type_mark", json!("FD"))]);
    // Plumbing on Level 2: a lavatory, two water closets and a shower draining to a 2" branch
    // laid at 1/8" per foot, which drains to a 4" stack; a floor drain with no connections.
    let branch = el(
        &mut d,
        "pipe",
        "L2",
        "P-SANR",
        [20.0, 2.0, 10.0],
        [40.0, 2.17, 10.17],
        &[
            ("tag", json!("W-1")),
            ("system", json!("SAN")),
            ("size", json!("2\"")),
            ("slope", json!("1/8\"/FT")),
            ("material", json!("PVC")),
            ("drains_to", json!("S-1")),
        ],
    );
    el(
        &mut d,
        "pipe",
        "L2",
        "P-SANR",
        [40.0, 2.0, 0.0],
        [40.33, 2.33, 20.0],
        &[("tag", json!("S-1")), ("system", json!("SAN")), ("size", json!("4\"")), ("role", json!("stack")), ("material", json!("PVC"))],
    );
    let fx = |ty: &str| -> Vec<(&'static str, Value)> {
        vec![("type", json!(ty)), ("waste", json!("W-1")), ("vent", json!("V-1")), ("room", json!("201"))]
    };
    el(&mut d, "fixture", "L2", "P-FIXT", [21.0, 3.0, 10.0], [22.5, 4.5, 13.0], &fx("Lavatory"));
    el(&mut d, "fixture", "L2", "P-FIXT", [24.0, 3.0, 10.0], [25.5, 5.0, 12.5], &fx("Water closet"));
    el(&mut d, "fixture", "L2", "P-FIXT", [27.0, 3.0, 10.0], [28.5, 5.0, 12.5], &fx("Water closet"));
    el(&mut d, "fixture", "L2", "P-FIXT", [30.0, 3.0, 10.0], [33.0, 6.0, 17.0], &fx("Shower"));
    let floor_drain =
        el(&mut d, "fixture", "L2", "P-FIXT", [35.0, 3.0, 10.0], [35.5, 3.5, 10.1], &[("type", json!("Floor drain")), ("room", json!("STORAGE"))]);
    let heater = el(
        &mut d,
        "equipment",
        "L2",
        "P-EQPM",
        [36.0, 10.0, 10.0],
        [38.0, 12.0, 15.0],
        &[("tag", json!("WH-1")), ("type", json!("Water heater")), ("recovery_gph", json!(10)), ("circuit", json!("LP2-10"))],
    );
    // Gas: a boiler of 400,000 Btu/h on a 40 ft run of 1" from a 20 ft 2" main.
    el(
        &mut d,
        "equipment",
        "L2",
        "M-EQPM",
        [5.0, 20.0, 10.0],
        [8.0, 23.0, 15.0],
        &[("tag", json!("B-1")), ("type", json!("Boiler")), ("btuh", json!(400_000)), ("gas_from", json!("G-1")), ("circuit", json!("LP2-12"))],
    );
    let gas_branch = el(
        &mut d,
        "pipe",
        "L2",
        "P-GAS",
        [5.0, 25.0, 18.0],
        [45.0, 25.08, 18.08],
        &[("tag", json!("G-1")), ("system", json!("GAS")), ("size", json!("1\"")), ("material", json!("black steel")), ("fed_from", json!("G-0"))],
    );
    el(
        &mut d,
        "pipe",
        "L2",
        "P-GAS",
        [45.0, 25.0, 18.0],
        [65.0, 25.17, 18.17],
        &[("tag", json!("G-0")), ("system", json!("GAS")), ("size", json!("2\"")), ("material", json!("black steel"))],
    );
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Building", None);
    (
        s,
        Hs {
            rated_door,
            plain_door,
            toilet_dup,
            no_circuit,
            rtu,
            exhaust_fan,
            fast_duct,
            damped_duct,
            unserved,
            unnumbered_room,
            floor_drain,
            branch,
            gas_branch,
            heater,
            conduit,
        },
    )
}

fn lines_by<'a>(r: &'a Value, keys: &[&str]) -> Vec<(Vec<String>, &'a Value)> {
    r["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (keys.iter().map(|k| l["group"][*k].as_str().unwrap_or_default().to_string()).collect(), l))
        .collect()
}

fn find<'a>(lines: &'a [(Vec<String>, &'a Value)], want: &[&str]) -> &'a Value {
    lines
        .iter()
        .find(|(k, _)| k.iter().map(String::as_str).eq(want.iter().copied()))
        .map(|(_, l)| *l)
        .unwrap_or_else(|| panic!("no line {want:?} in {:?}", lines.iter().map(|x| &x.0).collect::<Vec<_>>()))
}

#[test]
fn takeoff_and_bom_group_and_filter_by_any_property() {
    let (mut s, _) = model();
    let q = json!({"group_by": ["level", "circuit"], "filter": {"kind": "device", "type": "duplex"}});
    for cmd in ["bim.takeoff", "bim.bom"] {
        let r = s.execute(cmd, &q).unwrap_or_else(|e| panic!("{cmd}: {e}"));
        assert_eq!(r["by"], "properties", "{cmd}");
        let ls = lines_by(&r, &["level", "circuit"]);
        assert_eq!(ls.len(), 4, "{cmd}: {r}");
        assert_eq!(find(&ls, &["L1", "LP1-1"])["count"], 3);
        assert_eq!(find(&ls, &["L1", "LP1-3"])["count"], 2, "the GFCI duplex is a duplex too");
        assert_eq!(find(&ls, &["L1", "(none)"])["count"], 1);
        assert_eq!(find(&ls, &["L2", "LP2-1"])["count"], 2);
        assert_eq!(r["totals"]["elements"], 8, "hand dryers are not duplexes: {cmd}");
        assert_eq!(find(&ls, &["L1", "LP1-1"])["elements"].as_array().unwrap().len(), 3);
        assert!(r["csv"].as_str().unwrap().starts_with("level,circuit,kind,count,qty,unit\n"), "{cmd}");
    }
    // where: exists / missing, and lists.
    let r = s
        .execute("bim.takeoff", &json!({"group_by": "kind", "filter": {"kind": ["device", "light_fixture"], "where": {"circuit": "missing"}}}))
        .unwrap();
    assert_eq!(r["totals"]["elements"], 1);
    let r = s.execute("bim.takeoff", &json!({"group_by": ["room"], "filter": {"kind": "device", "circuit": ["LP1-1", "LP1-5"]}})).unwrap();
    let ls = lines_by(&r, &["room"]);
    assert_eq!(find(&ls, &["101"])["count"], 3);
    assert_eq!(find(&ls, &["102"])["count"], 2);
    // Measures by kind: duct length, sheet metal and weight (assumed gauge marked); pipe hangers.
    let r = s.execute("bim.takeoff", &json!({"group_by": ["system", "size"], "filter": {"kind": "duct", "system": "supply"}})).unwrap();
    let ls = lines_by(&r, &["system", "size"]);
    let l = find(&ls, &["Supply Air", "12x8"]);
    assert!((l["length_ft"].as_f64().unwrap() - 20.0).abs() < 0.01, "{l}");
    assert_eq!(l["unit"], "lf");
    // 12x8: 40" of perimeter (3.33 ft) over 20 ft = 66.67 sf of 26 ga (0.906 lb/sf).
    assert!((l["sheet_metal_sf"].as_f64().unwrap() - 66.67).abs() < 0.05, "{l}");
    assert!((l["weight_lb"].as_f64().unwrap() - 60.4).abs() < 0.1, "{l}");
    assert_eq!(l["gauges"], json!([26]));
    assert!(l["gauge_rule"].as_str().unwrap().contains("assumed"));
    assert!((find(&ls, &["Supply Air", "10x10"])["insulation_lf"].as_f64().unwrap() - 20.0).abs() < 0.01);
    let r = s.execute("bim.takeoff", &json!({"group_by": ["system", "size", "material"], "filter": {"kind": "pipe", "system": "SAN"}})).unwrap();
    let ls = lines_by(&r, &["system", "size", "material"]);
    // 20 ft of PVC hung every 4 ft.
    assert_eq!(find(&ls, &["SAN", "2\"", "PVC"])["hangers"], 5.0);
    // Rooms by area.
    let r = s.execute("bim.takeoff", &json!({"group_by": "level", "filter": {"kind": "room"}})).unwrap();
    let ls = lines_by(&r, &["level"]);
    assert_eq!(find(&ls, &["L1"])["area_sf"], 350.0);
    assert_eq!(find(&ls, &["L1"])["unit"], "sf");
    // Nothing matches: says so.
    let r = s.execute("bim.bom", &json!({"filter": {"kind": "device", "type": "nonesuch"}})).unwrap();
    assert_eq!(r["totals"]["elements"], 0);
    assert!(r["next_step"].as_str().unwrap().contains("no element matches"));
    assert!(s.execute("bim.takeoff", &json!({"group_by": [], "filter": {"kind": "device"}})).is_err());
}

fn sched(s: &mut Session, q: Value) -> Value {
    s.execute("bim.schedule", &q).unwrap_or_else(|e| panic!("{q}: {e}"))
}

fn row<'a>(r: &'a Value, col: &str, v: &str) -> &'a Value {
    r["rows"].as_array().unwrap().iter().find(|x| x[col] == v).unwrap_or_else(|| panic!("no row {col}={v} in {}", r["rows"]))
}

#[test]
fn schedules_come_from_the_properties() {
    let (mut s, _) = model();
    // Devices by room and by circuit.
    let r = sched(&mut s, json!({"kind": "device"}));
    let office: Vec<&Value> = r["rows"].as_array().unwrap().iter().filter(|x| x["room"] == "101").collect();
    assert!(office.iter().any(|x| x["count"] == "3" && x["circuits"] == "LP1-1" && x["mount_height"] == "18 in"), "{office:?}");
    let r = sched(&mut s, json!({"kind": "device", "by": "circuit"}));
    assert_eq!(row(&r, "circuit", "LP1-5")["load_va"], "2000");
    // Doors in number order; room finishes; equipment.
    let r = sched(&mut s, json!({"kind": "door"}));
    assert_eq!(r["count"], 2);
    assert_eq!(r["rows"][0]["number"], "101A");
    assert_eq!(r["rows"][1]["width"], "3'-0\"");
    let r = sched(&mut s, json!({"kind": "room_finish", "level": "L1"}));
    assert_eq!(r["count"], 2);
    assert_eq!(row(&r, "number", "101")["floor_finish"], "VCT-1");
    assert_eq!(row(&r, "number", "101")["area"], "200");
    let r = sched(&mut s, json!({"kind": "equipment"}));
    assert_eq!(row(&r, "tag", "RTU-1")["mocp"], "30");
    // Panels: circuits with breaker, wire and load, and the load per phase (1-2 A, 3-4 B, 5-6 C).
    let r = sched(&mut s, json!({"kind": "panel"}));
    let lp1 = r["panels"].as_array().unwrap().iter().find(|p| p["panel"] == "LP1").unwrap();
    assert_eq!(lp1["per_phase_va"]["A"], 540.0, "{lp1}");
    assert_eq!(lp1["per_phase_va"]["B"], 360.0);
    assert_eq!(lp1["per_phase_va"]["C"], 2000.0);
    assert!(lp1["element"].is_string(), "the panel element is found by its tag");
    let c1 = r["rows"].as_array().unwrap().iter().find(|x| x["panel"] == "LP1" && x["circuit"] == "1").unwrap();
    assert_eq!(c1["breaker"], "20A");
    assert_eq!(c1["wire"], "2#12, 1#12G");
    assert_eq!(c1["devices"], "3");
    assert_eq!(c1["phase"], "A");
    assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("180 VA")), "assumed loads are marked");
    // Air distribution: the unit and its terminals.
    let r = sched(&mut s, json!({"kind": "air_distribution"}));
    let rtu = r["rows"].as_array().unwrap().iter().filter(|x| x["unit"] == "RTU-1").collect::<Vec<_>>();
    assert_eq!(rtu.len(), 3);
    assert_eq!(rtu[0]["terminal_cfm"], "750");
    assert_eq!(rtu[0]["difference_pct"], "-25");
    assert!(r["rows"].as_array().unwrap().iter().any(|x| x["unit"] == "(not served)"));
    // Plumbing fixtures by room with their units (IPC defaults, marked).
    let r = sched(&mut s, json!({"kind": "plumbing_fixture"}));
    let wc = r["rows"].as_array().unwrap().iter().find(|x| x["type"] == "Water closet").unwrap();
    assert_eq!(wc["count"], "2");
    assert_eq!(wc["dfu_total"], "8");
    assert!(r["notes"][0].as_str().unwrap().contains("709.1"));
    // Lighting fixtures (none here) is an empty schedule, not an error.
    assert_eq!(sched(&mut s, json!({"kind": "light_fixture"}))["count"], 0);
    // Wire and conduit by circuit: 10 conductors × 150 ft × 1.1.
    let r = sched(&mut s, json!({"kind": "wire"}));
    let w = row(&r, "circuit", "LP1-7");
    assert_eq!(w["conduit_lf"], "150");
    assert_eq!(w["wire_lf"], "1650");
    assert_eq!(w["route"], "conduit");
    assert_eq!(row(&r, "circuit", "LP1-5")["route"], "estimated route");
    // Duct and pipe schedules are their takeoffs.
    let r = sched(&mut s, json!({"kind": "duct"}));
    assert_eq!(r["schedule"], "duct");
    assert!(r["lines"].as_array().unwrap().len() >= 2);
    // Errors.
    assert!(s.execute("bim.schedule", &json!({})).is_err());
    assert!(s.execute("bim.schedule", &json!({"kind": "nonesuch"})).is_err());
    assert!(s.execute("bim.schedule", &json!({"kind": "device", "by": "colour"})).is_err());
}

#[test]
fn schedules_and_takeoffs_export_csv_and_xlsx() {
    let (mut s, _) = model();
    let dir = std::env::temp_dir().join(format!("propuse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("doors.csv");
    let r = sched(&mut s, json!({"kind": "door", "file": csv.to_str().unwrap()}));
    let text = std::fs::read_to_string(&csv).unwrap();
    assert_eq!(text, r["csv"].as_str().unwrap());
    assert!(text.starts_with("number,width,height,frame,hardware_set,fire_rating,from_room,to_room,level,handle\n101A,"), "{text}");
    let xlsx = dir.join("panels.xlsx");
    sched(&mut s, json!({"kind": "panel", "file": xlsx.to_str().unwrap()}));
    assert!(std::fs::read(&xlsx).unwrap().starts_with(b"PK"), "an xlsx is a zip");
    let tx = dir.join("devices.xlsx");
    s.execute("bim.takeoff", &json!({"group_by": ["level", "circuit"], "filter": {"kind": "device"}, "file": tx.to_str().unwrap()})).unwrap();
    assert!(std::fs::read(&tx).unwrap().starts_with(b"PK"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn findings(r: &Value, check: &str) -> Vec<Value> {
    r["findings"].as_array().unwrap().iter().filter(|f| f["check"] == check).cloned().collect()
}

fn handles(fs: &[Value]) -> Vec<String> {
    fs.iter().filter_map(|f| f["handle"].as_str().map(str::to_string)).collect()
}

#[test]
fn checks_find_what_the_properties_say() {
    let (mut s, h) = model();
    let r = s.execute("bim.props.check", &json!({})).unwrap();
    // General.
    assert_eq!(handles(&findings(&r, "no_circuit")), vec![h.no_circuit.hex()], "{r}");
    let dh = findings(&r, "default_height");
    assert_eq!(handles(&dh), vec![h.no_circuit.hex()], "only the assumed height");
    let rd = findings(&r, "rated_door");
    assert_eq!(handles(&rd), vec![h.rated_door.hex()], "{rd:?}");
    assert!(!handles(&rd).contains(&h.plain_door.hex()));
    assert_eq!(handles(&findings(&r, "room_number")), vec![h.unnumbered_room.hex()]);
    assert_eq!(handles(&findings(&r, "gfci_wet")), vec![h.toilet_dup.hex()], "the GFCI one in the toilet room passes");
    // Electrical: LP1-5's 2000 VA is 83% of 20 A at 120 V; #14 on 20 A; 10 #12 in 1/2" EMT; the
    // dryers' 50 ft estimated route on #14 drops over 3%; the RTU's 40 A breaker is over its MOCP.
    let cl = findings(&r, "circuit_load");
    assert_eq!(cl.len(), 1, "{cl:?}");
    assert_eq!(cl[0]["values"]["circuit"], "LP1-5");
    assert_eq!(cl[0]["severity"], "warning");
    let wb = findings(&r, "wire_breaker");
    assert_eq!(wb.len(), 1, "{wb:?}");
    assert_eq!(wb[0]["values"]["wire"], "14");
    let cf = findings(&r, "conduit_fill");
    assert_eq!(handles(&cf), vec![h.conduit.hex()]);
    assert!((cf[0]["values"]["fill_pct"].as_f64().unwrap() - 43.75).abs() < 0.01, "{cf:?}");
    let vd = findings(&r, "voltage_drop");
    assert!(vd.iter().any(|f| f["values"]["circuit"] == "LP1-5" && f["values"]["route"] == "estimated route"), "{vd:?}");
    assert!(!vd.iter().any(|f| f["values"]["circuit"] == "LP1-1"), "a short circuit drops little: {vd:?}");
    let mo = findings(&r, "equipment_mocp");
    assert_eq!(handles(&mo), vec![h.rtu.hex()]);
    assert!(mo[0]["message"].as_str().unwrap().contains("MOCP"));
    // Mechanical.
    let tc = findings(&r, "terminal_cfm");
    assert_eq!(handles(&tc), vec![h.rtu.hex()]);
    assert_eq!(tc[0]["values"]["difference_pct"], -25.0);
    let dv = findings(&r, "duct_velocity");
    assert_eq!(handles(&dv), vec![h.fast_duct.hex()], "500 cfm in 10x10 is 720 fpm: fine");
    assert_eq!(dv[0]["values"]["velocity_fpm"], 3000.0);
    let fd = findings(&r, "fire_damper");
    assert_eq!(handles(&fd), vec![h.fast_duct.hex()], "the duct with a damper at the wall passes: {fd:?}");
    assert!(!handles(&fd).contains(&h.damped_duct.hex()));
    assert_eq!(handles(&findings(&r, "terminal_unserved")), vec![h.unserved.hex()]);
    let ep = handles(&findings(&r, "equipment_power"));
    assert_eq!(ep, vec![h.exhaust_fan.hex()], "panels and circuited equipment pass");
    // Plumbing: 1 + 4 + 4 + 2 = 11 DFU on a 2" branch (6 at most), laid at 1/8" (1/4" needed);
    // the floor drain has no waste or vent; the heater's 10 gph is under (2 + 75) × 0.25; the
    // boiler's 400 cfh is over 1" pipe's 257 cfh at 60 ft.
    let dd = findings(&r, "drain_dfu");
    assert_eq!(handles(&dd), vec![h.branch.hex()], "{dd:?}");
    assert_eq!(dd[0]["values"]["dfu"], 11.0);
    let dfu_rows = r["totals"]["dfu"].as_array().unwrap();
    assert!(
        dfu_rows.iter().any(|x| x["role"] == "stack" && x["dfu"] == 11.0 && x["max_dfu"] == 240.0),
        "units carry down to the stack: {dfu_rows:?}"
    );
    assert_eq!(handles(&findings(&r, "pipe_slope")), vec![h.branch.hex()]);
    let wv = findings(&r, "fixture_waste_vent");
    assert_eq!(handles(&wv), vec![h.floor_drain.hex()]);
    assert!(wv[0]["message"].as_str().unwrap().contains("no waste and no vent"));
    let wh = findings(&r, "water_heater");
    assert_eq!(handles(&wh), vec![h.heater.hex()]);
    assert_eq!(wh[0]["values"]["demand_gph"], 19.25);
    let gs = findings(&r, "gas_size");
    assert_eq!(handles(&gs), vec![h.gas_branch.hex()], "the 2\" main carries it: {gs:?}");
    assert_eq!(gs[0]["values"]["capacity_cfh"], 257.0);
    assert_eq!(gs[0]["values"]["length_ft"], 60.0);
    // Counts and limits are reported.
    assert_eq!(r["counts"]["rated_door"], 1);
    assert!(r["checked"]["gfci_wet"].as_u64().unwrap() >= 8);
    assert_eq!(r["limits"]["voltage_drop_pct"], 3.0);
}

#[test]
fn checks_take_a_subset_a_level_and_limits() {
    let (mut s, h) = model();
    let r = s
        .execute(
            "bim.props.check",
            &json!({"checks": ["duct_velocity", "terminal_cfm"], "limits": {"duct_velocity_fpm": 3500, "cfm_tolerance_pct": 30}}),
        )
        .unwrap();
    assert_eq!(r["count"], 0, "{r}");
    assert!(r["counts"].as_object().unwrap().is_empty());
    let r = s.execute("bim.props.check", &json!({"checks": ["room_number", "no_circuit"], "level": "L1"})).unwrap();
    assert_eq!(handles(&findings(&r, "no_circuit")), vec![h.no_circuit.hex()]);
    assert!(findings(&r, "room_number").is_empty(), "the unnumbered room is on L2");
    assert!(s.execute("bim.props.check", &json!({"checks": ["nonesuch"]})).is_err());
    // A user's edit fixes a finding: number the room.
    s.execute("bim.props.set", &json!({"handle": h.unnumbered_room.hex(), "key": "number", "value": "202"})).unwrap();
    let r = s.execute("bim.props.check", &json!({"checks": "room_number"})).unwrap();
    assert_eq!(r["count"], 0);
    // A circuit that names no panel: nothing says what feeds the device.
    assert_eq!(s.execute("bim.props.check", &json!({"checks": ["no_fed_from"]})).unwrap()["count"], 0, "LP1-1 names its panel");
    s.execute("bim.props.set", &json!({"handle": h.toilet_dup.hex(), "key": "circuit", "value": "12"})).unwrap();
    let r = s.execute("bim.props.check", &json!({"checks": ["no_fed_from"]})).unwrap();
    assert_eq!(handles(&findings(&r, "no_fed_from")), vec![h.toilet_dup.hex()]);
}

#[test]
fn an_empty_model_says_to_build_it() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let r = s.execute("bim.props.check", &json!({})).unwrap();
    assert_eq!(r["count"], 0);
    assert!(r["next_step"].as_str().unwrap().contains("build"));
    let r = s.execute("bim.schedule", &json!({"kind": "door"})).unwrap();
    assert_eq!(r["count"], 0);
    assert!(r["next_step"].is_string());
    let r = s.execute("bim.takeoff", &json!({"group_by": ["level"], "filter": {"kind": "device"}})).unwrap();
    assert!(r["next_step"].as_str().unwrap().contains("build"));
}

#[test]
fn property_coverage_per_kind_for_the_job_report() {
    let (s, _) = model();
    let c = cadaddy_engine::cmd::propuse::coverage(s.doc().unwrap());
    let dev = &c["device"];
    assert_eq!(dev["elements"], 10, "{c}");
    // Devices: 10 standard keys each; every one has type, level, room and mount height; no
    // host wall or sheet was read.
    assert!(dev["percent"].as_f64().unwrap() > 30.0 && dev["percent"].as_f64().unwrap() < 100.0, "{dev}");
    assert!(dev["keys_with_no_value"].as_array().unwrap().iter().any(|k| k == "host_wall"));
    assert_eq!(dev["assumed_values"], 3, "the unclaimed duplex's three default values: {dev}");
    assert_eq!(c["room"]["elements"], 4);
}

#[test]
fn hostile_property_values_never_panic() {
    let mut d = Drawing::new_imperial();
    let weird = [
        json!(""),
        json!("#"),
        json!("1/0/0\""),
        json!("-5A"),
        json!(f64::MAX),
        json!("x×y"),
        json!("½\""),
        json!("999999#999999G"),
        json!("1/"),
        json!("%"),
    ];
    for (i, w) in weird.iter().enumerate() {
        let x = i as f64 * 3.0;
        for part in ["device", "conduit", "duct", "pipe", "fixture", "equipment", "door", "wall", "room"] {
            el(
                &mut d,
                part,
                "L1",
                "X",
                [x, 0.0, 0.0],
                [x + 1.0, 1.0, 1.0],
                &[
                    ("size", w.clone()),
                    ("wire", w.clone()),
                    ("breaker", w.clone()),
                    ("slope", w.clone()),
                    ("cfm", w.clone()),
                    ("circuit", w.clone()),
                    ("fire_rating", w.clone()),
                    ("served_by", json!("0")),
                    ("waste", json!("W")),
                    ("tag", json!("W")),
                    ("btuh", w.clone()),
                    ("gas_from", json!("W")),
                    ("load_va", w.clone()),
                    ("length", w.clone()),
                ],
            );
        }
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    s.execute("bim.props.check", &json!({})).unwrap();
    for k in [
        "device",
        "door",
        "window",
        "room_finish",
        "light_fixture",
        "equipment",
        "plumbing_fixture",
        "air_distribution",
        "panel",
        "wire",
        "duct",
        "pipe",
    ] {
        s.execute("bim.schedule", &json!({"kind": k})).unwrap();
    }
    s.execute("bim.takeoff", &json!({"group_by": ["size", "wire"], "filter": {"kind": ["duct", "pipe", "conduit"]}})).unwrap();
    for bad in [
        json!({"group_by": 5, "filter": {"kind": "duct"}}),
        json!({"group_by": ["x"], "filter": {"where": 3}}),
        json!({"group_by": ["x"], "filter": {"kind": 4}}),
        json!({"group_by": ["x"], "filter": {"k": {}}}),
    ] {
        assert!(s.execute("bim.takeoff", &bad).is_err(), "{bad}");
    }
}
