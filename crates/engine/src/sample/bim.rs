//! A made-up built model for the BIM views (Q6419): a double-loaded corridor building of
//! `levels` storeys and `rooms` guest rooms a storey, every element tagged as the build tags
//! its own: the shell, walls, doors, windows, rooms and ceilings; gear, house panels fed from
//! it, a unit panel in every room fed from its storey's house panel, branch circuits as
//! pieces (sticks, couplings, straps, wire, boxes), devices and lights; supply mains and
//! branch ducts, terminals and bathroom exhaust fans with their risers; plumbing mains,
//! branches and a riser; fire mains, a standpipe, branch lines and heads; an elevator with its
//! hoistway, machine, disconnect, feeder and traveling cable; casework, furniture and framing.
//! Its building spec holds the levels, the gear, the panels' schedules, the feeders and the
//! hoistway, so what feeds what is data, as in a built job.

use cadaddy_doc::props::{Prop, Props, Source};
use cadaddy_doc::{Common, Drawing, EntityKind, Handle, RawTag, Space};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use crate::cmd::wallgen::{APP, add_box, empty_mesh};

/// Inches a foot (the model is in inches).
const FT: f64 = 12.0;
const ROOM_W: f64 = 14.0;
const ROOM_D: f64 = 28.0;
const CORRIDOR: f64 = 6.0;
/// Where the rooms start along the building, feet (the core is before it).
const CORE: f64 = 20.0;
const STOREY: f64 = 12.0;

struct Builder {
    d: Drawing,
}

impl Builder {
    /// A box on the centreline `a`–`b` (feet), `t` feet thick, from `z0` to `z1` (feet).
    #[allow(clippy::too_many_arguments)]
    fn put(&mut self, layer: &str, tags: Vec<String>, props: &[(&str, &str)], a: [f64; 2], b: [f64; 2], t: f64, z0: f64, z1: f64) -> Handle {
        let mut m = empty_mesh();
        let (pa, pb) = (Vec2::new(a[0] * FT, a[1] * FT), Vec2::new(b[0] * FT, b[1] * FT));
        // An upright piece: a short box along x.
        let pb = if pa.dist(pb) < 1e-9 { pa + Vec2::new(t * FT, 0.0) } else { pb };
        add_box(&mut m, pa, pb, t * FT, z0 * FT, z1 * FT);
        let mut x: Vec<RawTag> = vec![RawTag { code: 1001, value: APP.into() }];
        x.extend(tags.into_iter().map(|value| RawTag { code: 1000, value }));
        if !props.is_empty() {
            let mut ps = Props::default();
            for (k, v) in props {
                ps.set(k, Prop::new(*v, Source::computed()));
            }
            x = cadaddy_doc::props::write(&x, &ps);
        }
        if !self.d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(layer)) {
            self.d.layers.push(cadaddy_doc::Layer::new(layer));
        }
        self.d.add(&Space::Model, Common { layer: layer.into(), xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap_or(Handle(0))
    }

    #[allow(clippy::too_many_arguments)]
    fn part(&mut self, part: &str, level: &str, more: &[&str], layer: &str, a: [f64; 2], b: [f64; 2], t: f64, z0: f64, z1: f64) -> Handle {
        let mut tags = vec![part.to_string(), level.to_string(), Handle(0).hex()];
        tags.extend(more.iter().map(|s| s.to_string()));
        self.put(layer, tags, &[], a, b, t, z0, z1)
    }

    #[allow(clippy::too_many_arguments)]
    fn mep(
        &mut self,
        level: &str,
        kind: &str,
        part: &str,
        system: &str,
        size: Option<&str>,
        more: &[&str],
        props: &[(&str, &str)],
        a: [f64; 2],
        b: [f64; 2],
        t: f64,
        z0: f64,
        z1: f64,
    ) -> Handle {
        let mut tags =
            vec!["mep".to_string(), level.to_string(), Handle(0).hex(), format!("system:{system}"), format!("kind:{kind}"), format!("part:{part}")];
        if let Some(z) = size {
            tags.push(format!("size:{z}"));
        }
        tags.extend(more.iter().map(|s| s.to_string()));
        let layer = format!("3D-MEP-{}", kind.to_ascii_uppercase());
        self.put(&layer, tags, props, a, b, t, z0, z1)
    }

    #[allow(clippy::too_many_arguments)]
    fn equipment(&mut self, level: &str, kind: &str, tag: &str, title: &str, a: [f64; 2], b: [f64; 2], t: f64, z0: f64, z1: f64) -> Handle {
        let title = format!("title:{title}");
        self.mep(level, "equipment", &format!("equipment:{kind}"), "", None, &[format!("tag:{tag}").as_str(), title.as_str()], &[], a, b, t, z0, z1)
    }
}

/// The building: `levels` storeys (`L1` up), `rooms` guest rooms each (numbered `101` up).
pub fn bim_building(levels: usize, rooms: usize) -> Drawing {
    let levels = levels.clamp(1, 200);
    let rooms = rooms.clamp(1, 400);
    let mut b = Builder { d: Drawing::new_imperial() };
    let per_side = rooms.div_ceil(2);
    let len = CORE + per_side as f64 * ROOM_W;
    let depth = 2.0 * ROOM_D + CORRIDOR;
    let names: Vec<String> = (1..=levels).map(|k| format!("L{k}")).collect();
    let mut equipment: Vec<Value> = vec![
        json!({"tag": "SWB-1", "kind": "switchboard", "level": "L1", "min": [2.0 * FT, 40.0 * FT], "max": [10.0 * FT, 43.0 * FT], "height_ft": 7.0}),
        json!({"tag": "ELEV-1 DISC", "kind": "disconnect", "title": "ELEVATOR DISCONNECT", "level": "L1", "min": [12.0 * FT, 0.0], "max": [13.0 * FT, 1.0 * FT], "height_ft": 2.0}),
        json!({"tag": "ELEV-1", "kind": "elevator", "level": "L1", "min": [12.0 * FT, 2.0 * FT], "max": [15.0 * FT, 4.0 * FT], "height_ft": 6.0}),
    ];
    let mut feeders: Vec<Value> = vec![json!({"tag": "F-ELEV", "from": "SWB-1", "to": "ELEV-1 DISC", "conduit_in": 1.5, "size": "#4"})];
    let mut runs: Vec<Value> = Vec::new();
    let mid = ROOM_D + CORRIDOR / 2.0;
    for (k, lv) in names.iter().enumerate() {
        let z = k as f64 * STOREY;
        let top = k + 1 == levels;
        let lv = lv.as_str();
        // The shell: the slab, the exterior walls, a column, a stair.
        b.part("floor", lv, &["type:SLAB"], "A-FLOR", [0.0, depth / 2.0], [len, depth / 2.0], depth, z - 1.0, z);
        for (a, c) in [([0.0, 0.0], [len, 0.0]), ([len, 0.0], [len, depth]), ([len, depth], [0.0, depth]), ([0.0, depth], [0.0, 0.0])] {
            b.part("wall", lv, &["type:EXTERIOR WALL"], "A-WALL-EXTR", a, c, 1.0, z, z + STOREY);
        }
        b.part("column", lv, &[], "S-COLS", [18.0, 30.0], [18.0, 30.0], 1.0, z, z + STOREY);
        b.part("stair", lv, &[], "A-STAIR", [2.0, 50.0], [10.0, 50.0], 4.0, z, z + STOREY);
        // The house panel, fed from the switchboard up a riser.
        let hp = format!("H{}", k + 1);
        b.equipment(lv, "panel", &hp, "PANEL", [12.0, 40.0], [14.0, 40.0], 0.5, z + 1.0, z + 6.0);
        equipment.push(json!({"tag": hp, "kind": "panel", "level": lv, "min": [12.0 * FT, 40.0 * FT], "max": [14.0 * FT, 40.5 * FT], "height_ft": 5.0,
            "circuits": [{"number": "1", "description": "CORRIDOR LIGHTS", "room": "CORRIDOR"}, {"number": "2", "description": "LOBBY RECEPTACLES", "room": "LOBBY"}]}));
        feeders.push(json!({"tag": format!("F-{hp}"), "from": "SWB-1", "to": hp, "conduit_in": 2.0, "size": "#1"}));
        let rid = format!("FR{}", k + 1);
        runs.push(json!({"system": "E-FEEDER", "level": lv, "points": [[16.0 * FT, 41.0 * FT]], "size_in": [2.0, 0.0], "bottoms_ft": [0.0], "top_ft": STOREY, "id": rid, "source_sheet": format!("feeder F-{hp} SWB-1→{hp}")}));
        let rmid = format!("mepid:{rid}@{:.2},{:.2}/0", 16.0 * FT, 41.0 * FT);
        b.mep(lv, "conduit", "segment", "E-FEEDER", Some("2"), &[rmid.as_str()], &[], [16.0, 41.0], [16.0, 41.0], 0.2, z, z + STOREY);
        if k == 0 {
            b.equipment(lv, "switchboard", "SWB-1", "MAIN DISTRIBUTION", [2.0, 41.5], [10.0, 41.5], 3.0, z, z + 7.0);
            b.equipment(lv, "elevator", "ELEV-1", "ELEVATOR CONTROLLER", [12.0, 3.0], [15.0, 3.0], 2.0, z, z + 6.0);
            b.equipment(lv, "disconnect", "ELEV-1 DISC", "ELEVATOR DISCONNECT", [12.0, 0.5], [13.0, 0.5], 0.5, z + 4.0, z + 6.0);
            b.mep(lv, "conduit", "segment", "F-ELEV", Some("1.5"), &[], &[], [10.0, 41.5], [12.0, 0.5], 0.15, z + 10.0, z + 10.2);
            b.equipment(lv, "fire_pump", "FP-1", "FIRE PUMP", [4.0, 55.0], [8.0, 55.0], 2.0, z, z + 3.0);
        }
        if top {
            b.equipment(lv, "doas", "DOAS-1", "DEDICATED OUTDOOR AIR UNIT", [30.0, 30.0], [36.0, 30.0], 3.0, z + STOREY, z + STOREY + 5.0);
        }
        // The elevator's traveling cable in its hoistway, through every floor: never drawn in MEP.
        b.mep(
            lv,
            "cable",
            "segment",
            "E-BRANCH",
            Some("1"),
            &[],
            &[("circuit", "H1-9"), ("panel", "H1")],
            [6.0, 4.0],
            [6.0, 4.0],
            0.1,
            z - 1.5,
            z + STOREY,
        );
        // Mains along the corridor; risers through the floor above.
        b.mep(lv, "duct", "segment", "SA", Some("24x12"), &["duct_type:supply"], &[], [CORE, mid], [len, mid], 2.0, z + 9.5, z + 10.5);
        b.mep(lv, "pipe", "segment", "SAN", Some("4"), &[], &[], [CORE, mid - 1.0], [len, mid - 1.0], 0.33, z - 2.0, z - 1.7);
        b.mep(lv, "pipe", "segment", "CW", Some("2"), &[], &[], [CORE, mid + 1.0], [len, mid + 1.0], 0.17, z + 10.0, z + 10.2);
        b.mep(lv, "pipe", "segment", "CW", None, &[], &[], [CORE, mid + 1.5], [CORE + 20.0, mid + 1.5], 0.1, z + 10.0, z + 10.1);
        b.mep(lv, "pipe", "segment", "FIRE MAIN", Some("4"), &[], &[], [CORE, mid - 2.0], [len, mid - 2.0], 0.33, z + 11.0, z + 11.3);
        b.mep(lv, "tray", "segment", "TRAY", Some("12x4"), &[], &[], [CORE, mid + 2.0], [len, mid + 2.0], 1.0, z + 10.6, z + 10.9);
        b.mep(lv, "conduit", "segment", "C-1", Some("1.25"), &[], &[], [CORE, mid + 2.5], [CORE + 30.0, mid + 2.5], 0.1, z + 10.0, z + 10.1);
        b.mep(lv, "conduit", "segment", "C-2", Some("0.5"), &[], &[], [CORE, mid + 2.7], [CORE + 30.0, mid + 2.7], 0.05, z + 10.0, z + 10.05);
        if !top {
            b.mep(lv, "pipe", "segment", "CW", Some("3"), &[], &[], [17.0, 32.0], [17.0, 32.0], 0.25, z + 1.0, z + STOREY + 1.0);
            b.mep(lv, "pipe", "segment", "FIRE STANDPIPE", Some("4"), &[], &[], [17.0, 34.0], [17.0, 34.0], 0.33, z, z + STOREY + 1.0);
            b.mep(
                lv,
                "conduit",
                "segment",
                "E-BRANCH",
                Some("0.75"),
                &[],
                &[("circuit", "H1-3"), ("panel", "H1")],
                [17.0, 36.0],
                [17.0, 36.0],
                0.06,
                z + 8.0,
                z + STOREY + 1.0,
            );
        }
        for j in 0..rooms {
            let x0 = CORE + (j / 2) as f64 * ROOM_W;
            let north = j % 2 == 1;
            let (y0, y1) = if north { (ROOM_D + CORRIDOR, depth) } else { (0.0, ROOM_D) };
            let yc = if north { y0 } else { y1 };
            // Just inside the room from its corridor wall.
            let yi = if north { y0 + 0.6 } else { y1 - 0.6 };
            let ye = if north { y1 } else { y0 };
            let (xm, ym) = (x0 + ROOM_W / 2.0, (y0 + y1) / 2.0);
            let number = format!("{}{:02}", k + 1, j + 1);
            b.part("wall", lv, &["type:WALL TYPE 3"], "A-WALL", [x0, y0], [x0, y1], 0.4, z, z + 9.0);
            b.part("wall", lv, &["type:WALL TYPE 2"], "A-WALL", [x0, yc], [x0 + ROOM_W, yc], 0.4, z, z + 9.0);
            b.part("door", lv, &[], "A-DOOR", [x0 + 2.0, yc], [x0 + 5.0, yc], 0.2, z, z + 7.0);
            b.part("window", lv, &[], "A-GLAZ", [x0 + 4.0, ye], [x0 + 10.0, ye], 0.2, z + 3.0, z + 8.0);
            b.part("ceiling", lv, &[], "A-CLNG", [x0, ym], [x0 + ROOM_W, ym], y1 - y0, z + 9.0, z + 9.05);
            // The room element: its outline at the floor.
            let mut m = empty_mesh();
            let (a, c) = (Vec2::new(x0 * FT, y0 * FT), Vec2::new((x0 + ROOM_W) * FT, y1 * FT));
            for p in [a, Vec2::new(c.x, a.y), c, Vec2::new(a.x, c.y)] {
                m.vertices.push(cadaddy_geom::Vec3::new(p.x, p.y, z * FT));
            }
            m.faces.push(cadaddy_doc::MeshFace { v: vec![0, 1, 2, 3], hidden: 0 });
            let xr = vec![
                RawTag { code: 1001, value: APP.into() },
                RawTag { code: 1000, value: "room".into() },
                RawTag { code: 1000, value: lv.into() },
                RawTag { code: 1000, value: Handle(0).hex() },
                RawTag { code: 1000, value: "name:Guest Room".into() },
                RawTag { code: 1000, value: format!("number:{number}") },
            ];
            let _ = b.d.add(&Space::Model, Common { layer: "A-AREA".into(), xdata: xr.into(), ..Common::default() }, EntityKind::Mesh(m));
            // The unit panel and its feeder from the storey's house panel.
            b.equipment(lv, "panel", "U", "UNIT PANEL", [x0 + 1.0, yi], [x0 + 2.0, yi], 0.3, z + 4.0, z + 6.0);
            equipment.push(json!({"tag": "U", "kind": "panel", "level": lv, "min": [(x0 + 1.0) * FT, yc * FT], "max": [(x0 + 2.0) * FT, (yc + 0.3) * FT], "height_ft": 2.0,
                "circuits": [{"number": "1", "description": "RECEPTACLES", "room": number}, {"number": "2", "description": "LIGHTS", "room": number}]}));
            let fid = format!("FU{}-{}", k + 1, j + 1);
            feeders.push(json!({"tag": format!("F-{fid}"), "from": hp, "to": "U", "conduit_in": 0.75, "size": "#8"}));
            runs.push(json!({"system": "E-FEEDER", "level": lv, "points": [[14.0 * FT, mid * FT], [(x0 + 1.5) * FT, mid * FT]], "size_in": [0.75, 0.0], "bottoms_ft": [10.0], "id": fid, "source_sheet": format!("feeder F-{fid} {hp}→U")}));
            let fmid = format!("mepid:{fid}@{:.2},{:.2}/0", 14.0 * FT, mid * FT);
            b.mep(
                lv,
                "conduit",
                "segment",
                "E-FEEDER",
                Some("0.75"),
                &[fmid.as_str()],
                &[],
                [x0 + 1.5, mid],
                [x0 + 1.5, yi],
                0.06,
                z + 10.0,
                z + 10.06,
            );
            // A branch circuit as the crew installs it: sticks, couplings, a strap, wire, a box.
            let circuit = [("circuit", "U-1"), ("panel", "U"), ("room", number.as_str())];
            let id = format!("mepid:B{number}@0,0/0");
            for s in 0..3 {
                let xa = x0 + 3.0 + s as f64 * 3.0;
                b.mep(
                    lv,
                    "conduit",
                    "atom:conduit",
                    "E-BRANCH",
                    Some("0.75"),
                    &[id.as_str()],
                    &circuit,
                    [xa, ym],
                    [xa + 3.0, ym],
                    0.06,
                    z + 8.5,
                    z + 8.56,
                );
            }
            for s in 0..2 {
                let xa = x0 + 6.0 + s as f64 * 3.0;
                b.mep(
                    lv,
                    "conduit",
                    "atom:coupling",
                    "E-BRANCH",
                    Some("0.75"),
                    &[id.as_str()],
                    &circuit,
                    [xa, ym],
                    [xa + 0.1, ym],
                    0.08,
                    z + 8.48,
                    z + 8.58,
                );
            }
            b.mep(
                lv,
                "conduit",
                "atom:strap",
                "E-BRANCH",
                Some("0.75"),
                &[id.as_str()],
                &circuit,
                [x0 + 4.0, ym],
                [x0 + 4.1, ym],
                0.1,
                z + 8.56,
                z + 8.6,
            );
            b.mep(
                lv,
                "conduit",
                "atom:wire",
                "E-BRANCH",
                Some("#12"),
                &[id.as_str()],
                &circuit,
                [x0 + 3.0, ym],
                [x0 + 12.0, ym],
                0.01,
                z + 8.52,
                z + 8.53,
            );
            b.mep(
                lv,
                "conduit",
                "atom:box",
                "E-BRANCH",
                Some("4\" square"),
                &[id.as_str()],
                &circuit,
                [x0 + 3.0, y0 + 1.0],
                [x0 + 3.3, y0 + 1.0],
                0.15,
                z + 1.3,
                z + 1.6,
            );
            for s in 0..4 {
                let xa = x0 + 2.0 + s as f64 * 3.0;
                b.put(
                    "E-POWR-DEV",
                    vec!["device".into(), lv.into(), Handle(0).hex()],
                    &[("kind", "receptacle"), ("room", number.as_str()), ("circuit", "U-1")],
                    [xa, y0 + 0.5],
                    [xa + 0.3, y0 + 0.5],
                    0.1,
                    z + 1.3,
                    z + 1.6,
                );
            }
            b.put(
                "E-LITE",
                vec!["light".into(), lv.into(), Handle(0).hex()],
                &[("room", number.as_str())],
                [xm - 1.0, ym],
                [xm + 1.0, ym],
                2.0,
                z + 8.8,
                z + 9.0,
            );
            b.put(
                "E-LITE",
                vec!["light".into(), lv.into(), Handle(0).hex()],
                &[("room", number.as_str())],
                [xm - 1.0, ym + 6.0],
                [xm + 1.0, ym + 6.0],
                2.0,
                z + 8.8,
                z + 9.0,
            );
            b.put(
                "P-FIXT",
                vec!["device".into(), lv.into(), Handle(0).hex()],
                &[("kind", "lavatory"), ("room", number.as_str())],
                [x0 + 10.0, yc + if north { 1.0 } else { -1.0 }],
                [x0 + 12.0, yc + if north { 1.0 } else { -1.0 }],
                1.5,
                z + 2.5,
                z + 3.0,
            );
            // Air: a branch duct and its diffuser; the bath's exhaust fan and its riser.
            b.mep(lv, "duct", "segment", "SA", Some("8x6"), &["duct_type:supply"], &[], [xm, mid], [xm, ym], 0.66, z + 9.2, z + 9.7);
            b.mep(
                lv,
                "duct",
                "terminal:diffuser",
                "SA",
                Some("12x12"),
                &["duct_type:supply"],
                &[],
                [xm - 0.5, ym],
                [xm + 0.5, ym],
                1.0,
                z + 8.9,
                z + 9.05,
            );
            b.equipment(lv, "exhaust_fan", "EF", "BATH EXHAUST FAN", [x0 + 11.0, yi], [x0 + 12.0, yi], 0.8, z + 8.6, z + 9.0);
            let riser_top = if top { z + STOREY - 0.5 } else { z + STOREY + 1.0 };
            b.mep(lv, "duct", "segment", "EA", Some("6"), &["duct_type:exhaust"], &[], [x0 + 12.5, yi], [x0 + 12.5, yi], 0.5, z + 9.0, riser_top);
            // Water, sprinklers.
            b.mep(lv, "pipe", "segment", "CW", Some("0.75"), &[], &[], [x0 + 11.0, mid], [x0 + 11.0, yi], 0.06, z + 9.5, z + 9.56);
            b.mep(lv, "pipe", "segment", "SPRINKLER BRANCH", Some("1"), &[], &[], [xm, mid], [xm, ym], 0.08, z + 9.3, z + 9.38);
            b.mep(lv, "pipe", "terminal:head", "SPRINKLER", Some("0.5"), &[], &[], [xm, ym], [xm + 0.2, ym], 0.2, z + 8.9, z + 9.05);
            // Casework, furniture, framing.
            b.part(
                "casework",
                lv,
                &["type:VANITY"],
                "A-CASE",
                [x0 + 9.0, yc + if north { 2.0 } else { -2.0 }],
                [x0 + 13.0, yc + if north { 2.0 } else { -2.0 }],
                2.0,
                z,
                z + 3.0,
            );
            b.part("furniture", lv, &["type:BED"], "I-FURN", [x0 + 3.0, ym], [x0 + 9.0, ym], 6.0, z, z + 2.0);
            b.part(
                "furniture",
                lv,
                &["type:DESK"],
                "I-FURN",
                [x0 + 10.0, ye + if north { -2.0 } else { 2.0 }],
                [x0 + 13.0, ye + if north { -2.0 } else { 2.0 }],
                2.0,
                z,
                z + 2.5,
            );
            for s in 0..2 {
                let ya = y0 + 4.0 + s as f64 * 16.0;
                b.put(
                    "A-FRAM",
                    vec!["atom".into(), lv.into(), Handle(0).hex(), "trade:framing".into(), "part:atom:stud".into()],
                    &[],
                    [x0, ya],
                    [x0, ya + 0.3],
                    0.3,
                    z,
                    z + 9.0,
                );
            }
        }
    }
    let spec = json!({
        "levels": names.iter().enumerate().map(|(k, n)| json!({"name": n, "elevation_ft": k as f64 * STOREY, "height_ft": STOREY, "ceiling_ft": 9})).collect::<Vec<_>>(),
        "elevators": [{"name": "ELEV 1", "levels": names, "min": [2.0 * FT, 0.0], "max": [10.0 * FT, 8.0 * FT], "pit_ft": 4}],
        "mep": {"equipment": equipment, "feeders": feeders, "runs": runs},
    });
    b.d.building_spec = Some(spec.to_string()).into();
    b.d
}
