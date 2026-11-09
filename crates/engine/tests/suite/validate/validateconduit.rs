//! Q6153 validateconduit: Validate checks the routed conduit from the model's own pieces —
//! continuity, fill, box fill, derating, voltage drop, wiring method, clashes and the takeoff —
//! recomputed, never read back from the router's report. Made-up one-storey building: panel LP1,
//! three circuits of receptacles, a shallow supply duct the conduit passes over and a deep return
//! duct one tap can't avoid.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use cadaddy_engine::doc::props;
use serde_json::{Value, json};

const RULES: [&str; 8] = ["E-CONDUIT-CONTINUITY", "E-CONDUIT-FILL", "E-BOX-FILL", "E-DERATE", "E-VDROP", "E-METHOD", "G-CLASH", "E-TAKEOFF"];

fn receptacle(n: usize, x: f64, y: f64, circuit: &str) -> Value {
    receptacle_at(n, x, y, circuit, 18)
}

fn receptacle_at(n: usize, x: f64, y: f64, circuit: &str, aff_in: u32) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x1F0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "mount_height_in": {"value": aff_in.to_string(), "source": "note"}
    }})
}

fn circuit(label: &str) -> Value {
    json!({"id": format!("circuit:L1:E-101:{label}"), "kind": "circuit", "level": "L1", "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": label, "source": "plan"},
        "panel": {"value": "LP1", "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "conductors": {"value": "2#12 + 1#12G", "source": "default"},
        "conduit": {"value": "3/4\" EMT", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).expect("layer");
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).expect("pline");
}

/// The building, routed; `breaker`: the schedule's breaker for every circuit (none: no schedule).
fn building(breaker: Option<u32>, extra: impl FnOnce(&mut Session)) -> Session {
    let devices = vec![
        receptacle(1, 20.0, 10.0, "LP1-1"),
        receptacle(2, 30.0, 10.0, "LP1-1"),
        receptacle(3, 30.0, 25.0, "LP1-1"),
        receptacle(4, 20.0, -15.0, "LP1-3"),
        receptacle(5, 26.0, -15.0, "LP1-3"),
        receptacle(6, 130.0, 10.0, "LP1-5"),
    ];
    building_of(devices, breaker, extra)
}

/// The building with `devices` on LP1's circuits 1, 3 and 5, routed.
fn building_of(devices: Vec<Value>, breaker: Option<u32>, extra: impl FnOnce(&mut Session)) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    // One room round it all: a 6 in shell, its tag in the middle.
    layer(&mut s, "A-WALL");
    rect(&mut s, [-66.0, -306.0], [1686.0, 486.0]);
    rect(&mut s, [-60.0, -300.0], [1680.0, 480.0]);
    layer(&mut s, "A-ROOM-TAG");
    s.execute("text", &json!({"at": [800.0, 300.0], "text": "101", "height": 6.0})).expect("tag");
    layer(&mut s, "0");
    extra(&mut s);
    let mut tags = devices;
    tags.extend([circuit("LP1-1"), circuit("LP1-3"), circuit("LP1-5")]);
    let schedules = match breaker {
        Some(a) => json!([{"panel": "LP1", "source": "X-601", "header": {"volts": "120/208V 3PH 4W"}, "circuits": [
            {"number": "1", "description": "RECEPTACLES", "room": "", "breaker": format!("{a}A"), "poles": 1, "load_va": 900.0},
            {"number": "3", "description": "RECEPTACLES", "room": "", "breaker": format!("{a}A"), "poles": 1, "load_va": 720.0},
            {"number": "5", "description": "RECEPTACLES", "room": "", "breaker": format!("{a}A"), "poles": 1, "load_va": 1500.0}
        ]}]),
        None => json!([]),
    };
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Shell"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Shell", "thickness_in": 6}],
        "rooms": [{"name": "Open Office", "number": "101", "level": "L1"}],
        "tags": tags,
        "mep": {
            "systems": [
                {"name": "SA", "layer": "M-SA", "kind": "duct", "size_in": [12, 6], "bottom_ft": 9.25},
                {"name": "RA", "layer": "M-RA", "kind": "duct", "size_in": [24, 24], "bottom_ft": 9.0}
            ],
            "runs": [
                {"system": "SA", "level": "L1", "points": [[300, -60], [300, 400]], "size_in": [12, 6], "bottoms_ft": [9.25], "size_source": "tag", "height_source": "tag"},
                {"system": "RA", "level": "L1", "points": [[0, -60], [600, -60]], "size_in": [24, 24], "bottoms_ft": [9.0], "size_source": "tag", "height_source": "tag"}
            ],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
            ],
            "panel_schedules": schedules
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

/// Every finding of the conduit rules (all pages), and the first page's answer.
fn screen(s: &mut Session, rules: &[&str]) -> (Value, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": rules})).expect("prescreen");
    let mut fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    let mut cursor = r["next"]["cursor"].clone();
    while let Some(c) = cursor.as_u64() {
        let more = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "cursor": c, "rules": rules})).expect("page");
        fs.extend(more["findings"].as_array().cloned().unwrap_or_default());
        cursor = more["next"]["cursor"].clone();
    }
    (r, fs)
}

fn of<'a>(fs: &'a [Value], rule: &str) -> Vec<&'a Value> {
    fs.iter().filter(|f| f["rule"] == rule).collect()
}

fn verdicts(fs: &[Value], rule: &str) -> std::collections::BTreeMap<String, usize> {
    let mut m = std::collections::BTreeMap::new();
    for f in of(fs, rule) {
        *m.entry(f["verdict"].as_str().unwrap_or_default().to_string()).or_default() += 1;
    }
    m
}

/// The pieces of a kind (atom) with their properties, by handle.
fn pieces(s: &Session, kind: &str) -> Vec<(cadaddy_engine::doc::Handle, props::Props)> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .map(|e| (e.handle, props::read(&e.common.xdata)))
        .filter(|(_, p)| p.get("atom").map(|x| x.text()).as_deref() == Some(kind))
        .collect()
}

fn xyz(t: &str) -> [f64; 3] {
    let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
    [v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0), v.get(2).copied().unwrap_or(0.0)]
}

#[test]
fn the_conduit_rules_are_built_in_and_run_by_the_prescreen() {
    let mut s = Session::new();
    for id in RULES {
        let r = s.execute("validate.rules", &json!({"id": id})).expect("rules");
        let rule = &r["rules"][0];
        assert_eq!(rule["id"], id, "{r}");
        assert!(rule["how"].as_array().is_some_and(|h| !h.is_empty()), "{r}");
    }
    let mut s = building(Some(20), |_| {});
    let (r, fs) = screen(&mut s, &RULES);
    for id in RULES {
        assert!(r["checked"][id].is_object(), "{id}: {}", r["checked"]);
    }
    // A routed building whose schedule gives the breakers and loads: every device is joined to
    // its panel, every raceway's fill, derating and box fill pass, the drops are within 3%.
    for id in ["E-CONDUIT-CONTINUITY", "E-CONDUIT-FILL", "E-BOX-FILL", "E-DERATE", "E-VDROP", "E-TAKEOFF"] {
        let v = verdicts(&fs, id);
        assert!(v.get("pass").copied().unwrap_or(0) > 0, "{id}: {v:?}");
        assert_eq!(v.get("fail").copied().unwrap_or(0), 0, "{id}: {:?}", of(&fs, id).iter().filter(|f| f["verdict"] == "fail").collect::<Vec<_>>());
    }
    assert_eq!(verdicts(&fs, "E-CONDUIT-CONTINUITY").get("pass"), Some(&6), "{:?}", of(&fs, "E-CONDUIT-CONTINUITY"));
    // The 130 ft circuit was upsized for its length: its drop is measured on its own wire.
    let far = of(&fs, "E-VDROP").into_iter().find(|f| f["measured"]["circuit"] == "LP1-5").cloned().unwrap_or_default();
    assert_eq!(far["verdict"], "pass", "{far}");
    assert!(far["measured"]["length_ft"].as_f64().is_some_and(|l| l > 100.0), "{far}");
    assert_eq!(far["measured"]["load_from"].as_str().map(|x| x.contains("1500 VA")), Some(true), "{far}");
}

#[test]
fn a_three_pole_loads_current_is_its_va_over_root_three_times_its_line_voltage() {
    // Q6234: LP1-5 as a three-pole 6,000 VA load on 120/208 V draws 16.7 A (6000 / (1.732 x
    // 208)), not the 28.8 A it would be over 208 V alone.
    let mut s = building(Some(20), |_| {});
    let spec = s.execute("bspec.get", &json!({})).expect("spec");
    let mut spec = spec.get("spec").cloned().unwrap_or(spec);
    spec["mep"]["panel_schedules"][0]["circuits"][2]["poles"] = json!(3);
    spec["mep"]["panel_schedules"][0]["circuits"][2]["load_va"] = json!(6000.0);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("set");
    s.execute("model.build", &json!({})).expect("build");
    let (_, fs) = screen(&mut s, &["E-VDROP"]);
    let far = of(&fs, "E-VDROP").into_iter().find(|f| f["measured"]["circuit"] == "LP1-5").cloned().unwrap_or_default();
    assert_eq!(far["measured"]["volts"], json!(208.0), "{far}");
    assert!(far["measured"]["amps"].as_f64().is_some_and(|a| (a - 16.65).abs() < 0.05), "{far}");
}

#[test]
fn data_the_documents_dont_give_is_unsure_never_a_pass() {
    // No panel schedule (no breakers, no loads) and no wiring-methods table.
    let mut s = building(None, |_| {});
    let (_, fs) = screen(&mut s, &["E-DERATE", "E-METHOD"]);
    let d = verdicts(&fs, "E-DERATE");
    assert!(d.get("unsure").copied().unwrap_or(0) > 0 && !d.contains_key("pass"), "{d:?}");
    let m = verdicts(&fs, "E-METHOD");
    assert!(m.get("unsure").copied().unwrap_or(0) > 0 && !m.contains_key("pass"), "{m:?}");
    assert!(of(&fs, "E-METHOD").iter().all(|f| f["issue"].as_str().is_some_and(|i| i.contains("no wiring-methods table"))));
}

#[test]
fn an_overfilled_conduit_fails() {
    let mut s = building(Some(20), |_| {});
    // The shared homerun's conductors made #4: five #4 in 3/4" EMT.
    let home: Vec<String> = pieces(&s, "wire")
        .into_iter()
        .filter(|(_, p)| p.get("circuit").map(|x| x.text()).as_deref() == Some("LP1-1,3"))
        .map(|(h, _)| h.hex())
        .collect();
    assert_eq!(home.len(), 5, "4#12 + 1#12G");
    for h in &home {
        s.execute("bim.atoms.edit", &json!({"handle": h, "size": "#4"})).expect("edit");
    }
    let (_, fs) = screen(&mut s, &["E-CONDUIT-FILL"]);
    let bad: Vec<&Value> = of(&fs, "E-CONDUIT-FILL").into_iter().filter(|f| f["verdict"] == "fail").collect();
    assert!(!bad.is_empty(), "{fs:?}");
    assert!(bad.iter().all(|f| f["measured"]["circuit"] == "LP1-1,3"), "{bad:?}");
    let m = &bad[0]["measured"];
    assert_eq!(m["allowed_pct"], json!(40.0), "{m}");
    assert!(m["fill_pct"].as_f64().is_some_and(|p| p > 40.0), "{m}");
    // The other raceways still pass.
    assert!(of(&fs, "E-CONDUIT-FILL").iter().any(|f| f["verdict"] == "pass"));
}

#[test]
fn a_fourth_current_carrying_conductor_without_derating_fails() {
    // 50 A breakers: the router sizes the shared homerun's conductors for four in one raceway.
    let mut s = building(Some(50), |_| {});
    let (_, before) = screen(&mut s, &["E-DERATE"]);
    let shared = |fs: &[Value]| of(fs, "E-DERATE").into_iter().find(|f| f["measured"]["circuit"] == "LP1-1,3").cloned().unwrap_or_default();
    assert_eq!(shared(&before)["verdict"], "pass", "{}", shared(&before));
    // Its four hots made #8: good for 50 A with three in a raceway (75 °C), 44 A with four.
    let hots: Vec<String> = pieces(&s, "wire")
        .into_iter()
        .filter(|(_, p)| {
            p.get("circuit").map(|x| x.text()).as_deref() == Some("LP1-1,3") && p.get("conductor").map(|x| x.text()).as_deref() != Some("ground")
        })
        .map(|(h, _)| h.hex())
        .collect();
    assert_eq!(hots.len(), 4);
    for h in &hots {
        s.execute("bim.atoms.edit", &json!({"handle": h, "size": "#8"})).expect("edit");
    }
    let (_, fs) = screen(&mut s, &["E-DERATE"]);
    let f = shared(&fs);
    assert_eq!(f["verdict"], "fail", "{f}");
    assert_eq!(f["measured"]["ccc"], json!(4), "{f}");
    assert_eq!(f["measured"]["factor"], json!(0.8), "{f}");
    assert_eq!(f["measured"]["allowed_a"], json!(44.0), "{f}");
    assert_eq!(f["measured"]["breaker_a"], json!(50.0), "{f}");
}

#[test]
fn a_homerun_group_beside_its_panel_is_continuous_from_every_box_to_the_panel() {
    // Q6241: three circuits whose first boxes sit 2 ft apart and 2 ft from their panel, 48 in up
    // (within reach of the panel's top). They share one homerun: the lead's box takes the other
    // two circuits' taps and the homerun leaves it for the panel. Each run must end on what it
    // serves: a tap in the lead's box (another circuit's), never back in its own first box; the
    // homerun on the panel, never in a box near the panel.
    let devices = vec![
        receptacle_at(1, 1.5, 0.0, "LP1-1", 48),
        receptacle_at(2, 6.0, 0.0, "LP1-1", 48),
        receptacle_at(3, 1.5, 2.0, "LP1-3", 48),
        receptacle_at(4, 6.0, 2.0, "LP1-3", 48),
        receptacle_at(5, 1.5, -2.0, "LP1-5", 48),
        receptacle_at(6, 6.0, -2.0, "LP1-5", 48),
    ];
    let mut s = building_of(devices, Some(20), |_| {});
    let (_, fs) = screen(&mut s, &["E-CONDUIT-CONTINUITY"]);
    let v = verdicts(&fs, "E-CONDUIT-CONTINUITY");
    let bad: Vec<&Value> = of(&fs, "E-CONDUIT-CONTINUITY").into_iter().filter(|f| f["verdict"] != "pass").collect();
    assert!(bad.is_empty(), "{bad:?}");
    assert_eq!(v.get("pass"), Some(&6), "{v:?}");
    // The homerun ends on the panel's top (LP1: x -24..-4 in, y -6..6 in, top 78 in), and each tap
    // ends in the lead's box (receptacle 1, at 18, 0).
    let ends = |pred: &dyn Fn(&str) -> bool| -> Vec<[f64; 3]> {
        pieces(&s, "conduit")
            .iter()
            .filter(|(_, p)| pred(&p.get("run").map(|x| x.text()).unwrap_or_default()))
            .flat_map(|(_, p)| ["start", "end"].map(|k| xyz(&p.get(k).map(|x| x.text()).unwrap_or_default())))
            .collect()
    };
    let home = ends(&|r| r.contains(":home:"));
    assert!(!home.is_empty(), "a homerun");
    assert!(
        home.iter().any(|q| (-25.0..=-3.0).contains(&q[0]) && (-7.0..=7.0).contains(&q[1]) && (q[2] - 78.0).abs() <= 1.0),
        "the homerun ends on the panel's top: {home:?}"
    );
    let taps = ends(&|r| r.contains("#tap"));
    assert!(!taps.is_empty(), "the taps");
    // The lead's box: receptacle 1 at (18, 0) in, 48 in up; its top a little over that.
    for c in ["LP1-3", "LP1-5"] {
        let mine = ends(&|r| r.contains("#tap") && r.contains(c));
        assert!(
            mine.iter().any(|q| q[0].hypot(q[1]) > 0.0 && (q[0] - 18.0).hypot(q[1]) < 3.0 && (48.0..=54.0).contains(&q[2])),
            "{c}'s tap ends in the lead's box: {mine:?}"
        );
        // Never back in its own first box (2 ft away, at the same height).
        let own_y = if c == "LP1-3" { 24.0 } else { -24.0 };
        let (in_own, in_lead) = (
            mine.iter().filter(|q| (q[0] - 18.0).hypot(q[1] - own_y) < 3.0 && (48.0..=54.0).contains(&q[2])).count(),
            mine.iter().filter(|q| (q[0] - 18.0).hypot(q[1]) < 3.0 && (48.0..=54.0).contains(&q[2])).count(),
        );
        assert_eq!((in_own, in_lead), (1, 1), "{c}: one end in its own first box, one in the lead's: {mine:?}");
    }
}

#[test]
fn a_drop_a_foot_short_of_its_box_breaks_continuity() {
    let mut s = building(Some(20), |_| {});
    // LP1-5's receptacle: the stick whose end is lowest over it (its drop into the box).
    let drop = pieces(&s, "conduit")
        .into_iter()
        .filter(|(_, p)| p.get("circuit").map(|x| x.text()).as_deref() == Some("LP1-5"))
        .flat_map(|(h, p)| {
            let (a, b) = (xyz(&p.get("start").map(|x| x.text()).unwrap_or_default()), xyz(&p.get("end").map(|x| x.text()).unwrap_or_default()));
            [(h, "start", a, p.clone()), (h, "end", b, p)]
        })
        .filter(|(_, _, q, _)| (q[0] - 130.0 * 12.0).abs() < 12.0 && (q[1] - 120.0).abs() < 12.0)
        .min_by(|x, y| x.2[2].total_cmp(&y.2[2]));
    let (h, which, at, mut ps) = drop.expect("a drop into receptacle 6's box");
    // Its end raised a foot.
    ps.set(which, props::Prop::new(format!("{},{},{}", at[0], at[1], at[2] + 12.0), props::Source::computed()));
    {
        let d = s.doc_mut().expect("doc");
        let _ = d.modify_entity(h, |e| {
            let x = props::write(&e.common.xdata, &ps);
            e.common.xdata = x.into();
            // Its body's end ring too: the stick is drawn a foot shorter.
            if let cadaddy_engine::doc::EntityKind::Mesh(m) = &mut e.kind {
                for v in &mut m.vertices {
                    if (v.x - at[0]).hypot(v.y - at[1]).hypot(v.z - at[2]) < 2.0 {
                        v.z += 12.0;
                    }
                }
            }
        });
    }
    let (_, fs) = screen(&mut s, &["E-CONDUIT-CONTINUITY"]);
    let bad: Vec<&Value> = of(&fs, "E-CONDUIT-CONTINUITY").into_iter().filter(|f| f["verdict"] == "fail").collect();
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert_eq!(bad[0]["measured"]["circuit"], "LP1-5");
    let gap = bad[0]["measured"]["gap_in"].as_f64().unwrap_or(0.0);
    assert!((gap - 12.0).abs() < 1.5, "a foot short: {}", bad[0]);
    assert!(bad[0]["issue"].as_str().is_some_and(|i| i.contains("short of its box")), "{}", bad[0]);
    assert_eq!(verdicts(&fs, "E-CONDUIT-CONTINUITY").get("pass"), Some(&5));
}

#[test]
fn a_conduit_through_a_duct_is_a_clash_found_without_the_routers_report() {
    let mut s = building(Some(20), |_| {});
    let (_, fs) = screen(&mut s, &["G-CLASH"]);
    // The router's own items come in (source build), and the recount finds the tap in the
    // return duct itself.
    assert!(of(&fs, "G-CLASH").iter().any(|f| f["source"] == "build"), "{fs:?}");
    let ra = |fs: &[Value]| -> Vec<Value> {
        of(fs, "G-CLASH")
            .into_iter()
            .filter(|f| f["source"] == "prescreen" && f["verdict"] == "fail" && f["measured"]["b"]["system"] == "RA")
            .cloned()
            .collect()
    };
    assert!(!ra(&fs).is_empty(), "{:?}", of(&fs, "G-CLASH"));
    // No report: the same clash, measured from the model alone; nothing carried in.
    {
        let d = s.doc_mut().expect("doc");
        d.embedded.remove("route/report.json");
    }
    let (_, fs2) = screen(&mut s, &["G-CLASH"]);
    assert!(!ra(&fs2).is_empty(), "{:?}", of(&fs2, "G-CLASH"));
    assert!(of(&fs2, "G-CLASH").iter().all(|f| f["source"] == "prescreen"));
    // The supply duct the conduit passes over is no clash.
    assert!(!of(&fs2, "G-CLASH").iter().any(|f| f["verdict"] == "fail" && f["measured"]["b"]["system"] == "SA"), "{fs2:?}");
    assert!(of(&fs2, "G-CLASH").iter().any(|f| f["verdict"] == "pass"));
}

#[test]
fn a_takeoff_off_by_a_fitting_fails() {
    let mut s = building(Some(20), |_| {});
    // One coupling taken out of the model, the stored count index left as it was.
    let (h, p) = pieces(&s, "coupling").into_iter().next().expect("a coupling");
    let (item, size) = (p.get("item").map(|x| x.text()).unwrap_or_default(), p.get("size").map(|x| x.text()).unwrap_or_default());
    s.doc_mut().expect("doc").remove_entity(h);
    let (_, fs) = screen(&mut s, &["E-TAKEOFF"]);
    let bad: Vec<&Value> = of(&fs, "E-TAKEOFF").into_iter().filter(|f| f["verdict"] == "fail").collect();
    assert_eq!(bad.len(), 1, "{bad:?}");
    let m = &bad[0]["measured"];
    assert_eq!((m["kind"].clone(), m["item"].clone(), m["size"].clone()), (json!("coupling"), json!(item), json!(size)), "{m}");
    let (recount, index) = (m["recount"].as_f64().unwrap_or(0.0), m["index"].as_f64().unwrap_or(0.0));
    assert_eq!(index - recount, 1.0, "{m}");
    // conduit.takeoff reads the model: it agrees with the recount.
    assert_eq!(m["takeoff"].as_f64(), Some(recount), "{m}");
    // Conduit feet compared too, and equal.
    assert!(of(&fs, "E-TAKEOFF").iter().any(|f| f["measured"]["kind"] == "conduit" && f["verdict"] == "pass"));
}

#[test]
fn the_wiring_method_is_checked_against_the_specifications_table_and_the_project_rule() {
    // The table allows MC concealed in ceilings and walls: the router lays MC cable, which passes,
    // and each device is followed to the panel through its cable's links.
    let mut s = building(Some(20), |s| {
        let methods = json!({"applications": [
            {"section": "26 05 19", "item": "C", "application": "Branch Circuits Concealed in Ceilings, Walls, and Partitions", "methods": ["mc"]}
        ]});
        if let Ok(d) = s.doc_mut() {
            d.embedded.insert("route/methods.json".into(), std::sync::Arc::new(serde_json::to_vec(&methods).unwrap_or_default()));
        }
    });
    let (_, fs) = screen(&mut s, &["E-METHOD", "E-CONDUIT-CONTINUITY", "E-BOX-FILL"]);
    // The boxes count the cables' conductors (12/2 with ground).
    let bf = of(&fs, "E-BOX-FILL");
    assert!(!bf.is_empty() && bf.iter().all(|f| f["verdict"] == "pass" && f["measured"]["cables"].as_u64().unwrap_or(0) > 0), "{bf:?}");
    let m = verdicts(&fs, "E-METHOD");
    assert!(m.get("pass").copied().unwrap_or(0) > 0 && !m.contains_key("fail") && !m.contains_key("unsure"), "{m:?}: {:?}", of(&fs, "E-METHOD"));
    let c = verdicts(&fs, "E-CONDUIT-CONTINUITY");
    assert_eq!(c.get("pass"), Some(&6), "{c:?}: {:?}", of(&fs, "E-CONDUIT-CONTINUITY"));
    assert!(of(&fs, "E-CONDUIT-CONTINUITY").iter().all(|f| f["measured"]["by"] == "links"));
    // The project's rule says EMT: the MC runs fail it.
    s.execute(
        "project.rules.set",
        &json!({"domain": "wiring_method", "key": "general", "value": "EMT", "tier": "spec", "source": "Spec 26 05 33", "reason": "test setup"}),
    )
    .expect("rule");
    let (_, fs) = screen(&mut s, &["E-METHOD"]);
    let m = verdicts(&fs, "E-METHOD");
    assert!(m.get("fail").copied().unwrap_or(0) > 0 && !m.contains_key("pass"), "{m:?}");
}

#[test]
fn hostile_conduit_records_do_not_panic() {
    let mut s = building(Some(20), |_| {});
    // Garbage in the pieces' properties: ends, sizes, volumes, circuits.
    let hs: Vec<(cadaddy_engine::doc::Handle, props::Props)> = ["conduit", "wire", "box"].iter().flat_map(|k| pieces(&s, k)).collect();
    {
        let d = s.doc_mut().expect("doc");
        for (i, (h, mut ps)) in hs.into_iter().enumerate() {
            let junk = ["NaN,inf,-inf", "1e308,1e308,1e308", "", "a,b", "#,#", "-1"][i % 6];
            for k in ["start", "end", "size", "cubic_in", "circuit", "run", "connects", "type", "length_ft"] {
                ps.set(k, props::Prop::new(junk, props::Source::computed()));
            }
            let _ = d.modify_entity(h, |e| {
                let x = props::write(&e.common.xdata, &ps);
                e.common.xdata = x.into();
            });
        }
        d.embedded.insert("route/report.json".into(), std::sync::Arc::new(b"{\"clashes\": [{\"at\": [\"x\", null]}, 7, null]}".to_vec()));
        d.embedded.insert("route/methods.json".into(), std::sync::Arc::new(b"[[[".to_vec()));
    }
    let (r, _) = screen(&mut s, &RULES);
    for id in RULES {
        assert!(r["checked"][id].is_object(), "{id}: {r}");
    }
}

/// Q6260: a Validate packet carries the routed raceway's run geometry (each stick's ends and
/// heights, what its ends touch, the wires pulled in it; each box's volume and the sticks
/// entering it; each wire's raceway) and its prescreen measures the conduit rules on those
/// pieces, each finding naming every piece of the area it covers. A judge that reports those
/// findings as they are judges every piece × conduit rule pair the numbers settle.
#[test]
fn a_packet_carries_the_raceways_geometry_and_its_measured_conduit_rules() {
    let mut s = building(Some(20), |_| {});
    let out = crate::helper::scratch("q6260-packet");
    let run = s.execute("validate", &json!({"room": "101", "out_dir": out.to_string_lossy()})).expect("validate")["run"]
        .as_str()
        .map(str::to_string)
        .expect("run");
    let n = s.execute("validate.next", &json!({"run": run})).expect("next");
    let area = n["area"]["id"].as_str().or_else(|| n["packet"]["area"].as_str()).map(str::to_string).unwrap_or_else(|| panic!("no area: {n}"));
    let dir = std::path::PathBuf::from(n["packet"]["dir"].as_str().expect("dir"));
    let o: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("objects.json")).expect("objects")).expect("json");
    let els: Vec<&Value> = o["elements"].as_array().into_iter().flatten().collect();
    let cat = |c: &str| els.iter().copied().filter(|e| e["category"] == c).collect::<Vec<_>>();
    let (sticks, boxes, wires, fittings) =
        (cat("IfcCableCarrierSegment"), cat("IfcJunctionBox"), cat("IfcCableSegment"), cat("IfcCableCarrierFitting"));
    assert!(!sticks.is_empty() && !boxes.is_empty() && !wires.is_empty() && !fittings.is_empty(), "{}", o["counts"]);

    // The geometry: every stick's two ends with their heights, what they touch, its wires.
    for e in &sticks {
        let r = &e["raceway"];
        assert_eq!(r["piece"], "stick", "{e}");
        assert_eq!(r["ends_ft"].as_array().map(Vec::len), Some(2), "{e}");
        assert_eq!(r["ends_aff_in"].as_array().map(Vec::len), Some(2), "{e}");
        assert!(r["touches"]["a"].is_array() && r["touches"]["b"].is_array(), "{e}");
        assert!(r["wires"].as_array().is_some_and(|w| !w.is_empty()) && r["conductors"].as_str().is_some_and(|c| c.contains('#')), "{e}");
    }
    // Some end lands on a box, and the sticks join one another.
    let box_ids: Vec<&str> = boxes.iter().filter_map(|b| b["handle"].as_str()).collect();
    let touched: Vec<&str> = sticks
        .iter()
        .flat_map(|e| ["a", "b"].map(|k| e["raceway"]["touches"][k].clone()))
        .flat_map(|t| t.as_array().cloned().unwrap_or_default())
        .filter_map(|v| v.as_str().map(str::to_string))
        .map(|h| if box_ids.contains(&h.as_str()) { "box" } else { "stick" })
        .collect();
    assert!(touched.contains(&"box") && touched.contains(&"stick"), "{touched:?}");
    // Every box's volume and the sticks entering it; every wire's raceway.
    for b in &boxes {
        assert!(b["raceway"]["cubic_in"].as_f64().is_some_and(|v| v > 0.0), "{b}");
        assert!(b["raceway"]["entering"].as_array().is_some_and(|v| !v.is_empty()), "{b}");
    }
    for w in &wires {
        assert!(w["raceway"]["in_raceway"].as_array().is_some_and(|v| !v.is_empty()), "{w}");
        assert!(w["raceway"]["size"].as_str().is_some_and(|z| z.starts_with('#')), "{w}");
    }
    assert!(o["raceways"].as_array().is_some_and(|r| !r.is_empty()) && o["counts"]["raceways"].as_u64().is_some_and(|n| n > 0), "{}", o["counts"]);

    // The conduit rules measured in the packet, each finding naming the pieces it covers.
    let pre = &o["prescreen"];
    for id in ["E-CONDUIT-CONTINUITY", "E-CONDUIT-FILL", "E-BOX-FILL", "E-DERATE", "E-VDROP", "E-TAKEOFF", "G-CLASH"] {
        let n: u64 = pre["checked"][id].as_object().map_or(0, |m| m.values().filter_map(Value::as_u64).sum());
        assert!(n > 0, "{id} not measured in the packet: {}", pre["checked"]);
    }
    let fs: Vec<&Value> =
        pre["findings"].as_array().into_iter().flatten().filter(|f| RULES.contains(&f["rule"].as_str().unwrap_or_default())).collect();
    let covered = |rule: &str| -> std::collections::BTreeSet<String> {
        fs.iter()
            .filter(|f| f["rule"] == rule)
            .flat_map(|f| f["handles"].as_array().cloned().unwrap_or_default())
            .filter_map(|h| h.as_str().map(str::to_string))
            .collect()
    };
    let ids = |v: &[&Value]| v.iter().filter_map(|e| e["handle"].as_str().map(str::to_string)).collect::<Vec<_>>();
    let fill = covered("E-CONDUIT-FILL");
    for h in ids(&sticks).iter().chain(ids(&wires).iter()) {
        assert!(fill.contains(h), "{h} in no E-CONDUIT-FILL finding: {fill:?}");
    }
    let box_fill = covered("E-BOX-FILL");
    assert!(ids(&boxes).iter().all(|h| box_fill.contains(h)), "{box_fill:?}");
    let cont = covered("E-CONDUIT-CONTINUITY");
    assert!(ids(&sticks).iter().all(|h| cont.contains(h)), "{cont:?}");
    let takeoff = covered("E-TAKEOFF");
    assert!(ids(&sticks).iter().chain(ids(&boxes).iter()).all(|h| takeoff.contains(h)), "{takeoff:?}");

    // A judge reporting them as measured, citing a picture of the packet: the conduit rules'
    // pairs are judged.
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).expect("manifest")).expect("json");
    let pic = ["prints", "model"]
        .iter()
        .find_map(|k| {
            manifest[*k]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|x| x["file"].as_str())
                .map(|f| (f.to_string(), manifest[*k][0]["sheet"].as_str().unwrap_or("model").to_string()))
        })
        .unwrap_or_else(|| panic!("no picture: {manifest}"));
    let report: Vec<Value> = fs
        .iter()
        .filter(|f| f["verdict"] != "fail")
        .map(|f| {
            json!({"rule": f["rule"], "handles": f["handles"], "verdict": f["verdict"], "issue": f["issue"], "expected": f["expected"],
                   "found": f["found"], "evidence": [{"image": pic.0, "sheet": pic.1, "what": "the raceway pieces"}], "confidence": f["confidence"]})
        })
        .collect();
    let r = s.execute("validate.report", &json!({"run": run, "area": area, "findings": report})).expect("report");
    assert!(r["pairs"]["judged"].as_u64().is_some_and(|j| j > 0), "{r}");
    let st = s.execute("validate.status", &json!({"run": run})).expect("status");
    // (E-BOX-FILL judges the boxes only: it is asked of every conduit piece until the pieces have
    // kinds of their own.)
    for (id, least) in [("E-CONDUIT-FILL", 0.9), ("E-DERATE", 0.9), ("E-CONDUIT-CONTINUITY", 0.9), ("E-VDROP", 0.9), ("E-TAKEOFF", 0.8)] {
        let c = st["accuracy"]["coverage_by_rule"][id].as_f64().unwrap_or(0.0);
        assert!(c >= least, "{id} coverage {c}: {}", st["accuracy"]["coverage_by_rule"]);
    }
}

/// Q6260: on a dense floor a packet keeps the build's parts nearest its area, never the first
/// ones in the drawing: thousands of raceway pieces just outside the room, drawn before a stick
/// inside it, don't push that stick (and its run's findings) out of the packet.
#[test]
fn a_packet_keeps_its_own_raceway_on_a_floor_dense_with_pieces_round_it() {
    use cadaddy_engine::doc::{Entity, EntityKind};
    let mut s = building(Some(20), |_| {});
    let areas = s.execute("validate.areas", &json!({})).expect("areas");
    let area = areas["areas"].as_array().into_iter().flatten().find(|a| a["number"] == "101").cloned().expect("101");
    let id = area["id"].as_str().expect("id").to_string();
    let bx: Vec<f64> = area["box"].as_array().expect("box").iter().filter_map(Value::as_f64).collect();
    let (stick, _) = pieces(&s, "conduit").into_iter().next().expect("a stick");
    let first: Entity = s.doc().expect("doc").model.get(stick).map(|e| Entity::clone(e)).expect("entity");
    let centre = match &first.kind {
        EntityKind::Mesh(m) if !m.vertices.is_empty() => {
            let n = m.vertices.len() as f64;
            (m.vertices.iter().map(|v| v.x).sum::<f64>() / n, m.vertices.iter().map(|v| v.y).sum::<f64>() / n)
        }
        _ => panic!("a stick is a mesh"),
    };
    // A copy of the stick with its middle at (x, y) feet.
    let at = |x: f64, y: f64| {
        let mut c = first.clone();
        if let EntityKind::Mesh(m) = &mut c.kind {
            for v in &mut m.vertices {
                v.x += x * 12.0 - centre.0;
                v.y += y * 12.0 - centre.1;
            }
        }
        c
    };
    // 6,000 copies 10 ft below the area (inside the packet's 30 ft look round it), then one in
    // its middle, last in the drawing.
    let (x0, x1, y0) = (bx[0], bx[2], bx[1]);
    let inside = {
        let d = s.doc_mut().expect("doc");
        for i in 0..6_000 {
            let mut c = at(x0 + (x1 - x0) * f64::from(i % 100) / 100.0, y0 - 10.0 - f64::from(i / 100) * 0.05);
            c.handle = d.new_handle();
            d.model.push(c);
        }
        let mut c = at((bx[0] + bx[2]) / 2.0, (bx[1] + bx[3]) / 2.0);
        c.handle = d.new_handle();
        let h = c.handle.hex();
        d.model.push(c);
        h
    };
    let out = crate::helper::scratch("q6260-dense");
    s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "max_images": 1})).expect("packet");
    let o: Value = serde_json::from_str(&std::fs::read_to_string(out.join("objects.json")).expect("objects")).expect("json");
    let own: Vec<&str> = o["elements"].as_array().into_iter().flatten().filter_map(|e| e["handle"].as_str()).collect();
    assert!(own.contains(&inside.as_str()), "the stick in the area's middle is left out ({} elements)", own.len());
}

/// The stored schedule's spec (the build's reading), to change and put back.
fn with_spec(s: &mut Session, f: impl FnOnce(&mut Value)) {
    let spec = s.execute("bspec.get", &json!({})).expect("spec");
    let mut spec = spec.get("spec").cloned().unwrap_or(spec);
    f(&mut spec);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("set");
}

fn text_at(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": at, "text": t, "height": h})).expect("text");
}

#[test]
fn a_breaker_cell_holding_its_load_or_no_rating_is_never_read_as_amps() {
    // Q6338: the schedule as read has circuit 1's 900 VA load in its breaker cell, and circuit 3
    // a 700 in the breaker cell of a #10 row (a load or another column read as the breaker).
    // Neither is a breaker: E-DERATE must not fail a #12 run on a "900 A breaker".
    let mut s = building(Some(20), |_| {});
    with_spec(&mut s, |sp| {
        sp["mep"]["panel_schedules"][0]["circuits"][0]["breaker"] = json!("900");
        sp["mep"]["panel_schedules"][0]["circuits"][1]["breaker"] = json!("700");
        sp["mep"]["panel_schedules"][0]["circuits"][1]["wire"] = json!("10");
        sp["mep"]["panel_schedules"][0]["circuits"][1]["load_va"] = json!(720.0);
    });
    let (_, fs) = screen(&mut s, &["E-DERATE", "E-VDROP"]);
    let bad: Vec<&Value> = of(&fs, "E-DERATE")
        .into_iter()
        .filter(|f| f["measured"]["breakers"].as_array().is_some_and(|b| b.iter().any(|x| x["amps"].as_f64().is_some_and(|a| a > 100.0))))
        .collect();
    assert!(bad.is_empty(), "a VA or a stray number read as the breaker: {bad:#?}");
    let on = |c: &str| -> Vec<&Value> {
        of(&fs, "E-DERATE").into_iter().filter(|f| f["issue"].as_str().is_some_and(|i| i.contains(&format!("of circuit {c} isn't")))).collect()
    };
    let one = on("LP1-1");
    assert!(!one.is_empty() && one.iter().all(|f| f["verdict"] == "unsure"), "{one:#?}");
    assert!(one.iter().all(|f| f["issue"].as_str().is_some_and(|i| i.contains("its own load"))), "{one:#?}");
    let three = on("LP1-3");
    assert!(
        !three.is_empty() && three.iter().all(|f| f["verdict"] == "unsure" && f["issue"].as_str().is_some_and(|i| i.contains("#10"))),
        "{three:#?}"
    );
    // The voltage drop of those circuits says why too; circuit 5's real 20 A still reads.
    let vd = of(&fs, "E-VDROP");
    let v1 = vd.iter().find(|f| f["measured"]["circuit"] == "LP1-1").copied().cloned().unwrap_or_default();
    assert_eq!(v1["verdict"], "unsure", "{v1}");
    assert!(v1["issue"].as_str().is_some_and(|i| i.contains("isn't read")), "{v1}");
    assert!(of(&fs, "E-DERATE").iter().any(|f| f["measured"]["breakers"][0]["amps"] == 20.0 && f["verdict"] == "pass"), "{fs:#?}");
}

/// A two-sided panel schedule drawn on the drawing (odd circuits left, even right), far from
/// the building: `PANELBOARD LP1`, every breaker 20 A.
fn drawn_schedule(s: &mut Session) {
    let (ox, k) = (3000.0, 10.0);
    let x = |v: f64| ox + v * k;
    let y = |v: f64| v * k;
    let h = 0.6;
    text_at(s, [x(23.93), y(13.07)], "PANELBOARD LP1", 1.26);
    text_at(s, [x(20.8), y(12.3)], "CKT", h);
    text_at(s, [x(30.8), y(12.3)], "CKT", h);
    for (vx, w) in [
        (20.8, "#"),
        (21.4, "DESCRIPTION"),
        (23.2, "AMPS"),
        (23.7, "P"),
        (24.0, "SIZE"),
        (24.5, "(VA)"),
        (27.0, "(VA)"),
        (27.6, "SIZE"),
        (28.0, "P"),
        (28.2, "AMPS"),
        (28.9, "DESCRIPTION"),
        (30.9, "#"),
    ] {
        text_at(s, [x(vx), y(12.2)], w, h);
    }
    for (vy, ln, ld, lva, rd, rn) in [
        (12.0, "1", "RECEPTACLES", "900", "SPARE", "2"),
        (11.9, "3", "RECEPTACLES", "720", "SPARE", "4"),
        (11.8, "5", "RECEPTACLES", "1,500", "SPARE", "6"),
    ] {
        for (vx, w) in [
            (20.8, ln),
            (21.1, ld),
            (23.4, "20"),
            (23.7, "1"),
            (24.0, "12"),
            (24.5, lva),
            (27.0, "0"),
            (27.6, "12"),
            (28.0, "1"),
            (28.3, "20"),
            (28.6, rd),
            (30.9, rn),
        ] {
            text_at(s, [x(vx), y(vy)], w, 0.7);
        }
    }
    text_at(s, [x(21.0), y(11.0)], "PER PHASE SUB-TOTALS", 0.7);
}

#[test]
fn rows_the_stored_schedule_lacks_are_read_again_off_the_drawn_schedule() {
    // Q6338: the build's stored reading of LP1 kept only circuit 1 (one side of a two-sided
    // schedule, or a reading from an older checkpoint); the drawn schedule gives 1, 3 and 5.
    let mut s = building(Some(20), |_| {});
    with_spec(&mut s, |sp| {
        if let Some(c) = sp["mep"]["panel_schedules"][0]["circuits"].as_array_mut() {
            c.truncate(1);
        }
    });
    drawn_schedule(&mut s);
    let (_, fs) = screen(&mut s, &["E-DERATE", "E-VDROP"]);
    let missing: Vec<&Value> = fs
        .iter()
        .filter(|f| f["issue"].as_str().is_some_and(|i| i.contains("panel schedule") && i.contains("isn't") || i.contains("aren't")))
        .collect();
    assert!(missing.is_empty(), "rows the drawn schedule gives: {missing:#?}");
    let d3 = of(&fs, "E-DERATE").into_iter().find(|f| f["issue"].as_str().is_some_and(|i| i.contains("LP1-3"))).cloned().unwrap_or_default();
    assert_eq!(d3["verdict"], "pass", "{d3}");
    let from = d3["measured"]["breakers"][0]["from"].as_str().unwrap_or_default();
    assert!(from.contains("read again"), "{d3}");
    let v5 = of(&fs, "E-VDROP").into_iter().find(|f| f["measured"]["circuit"] == "LP1-5").cloned().unwrap_or_default();
    assert!(v5["measured"]["load_from"].as_str().is_some_and(|l| l.contains("1500 VA")), "{v5}");
}

/// A gridded wire-length table drawn on the drawing, far from the building: 20 A / 120 V
/// circuits take #12 to 50 ft, #10 to 100 ft, #8 to 150 ft, #6 to 250 ft.
fn drawn_length_table(s: &mut Session) {
    let (x0, y0) = (3000.0, -200.0);
    text_at(s, [x0 + 10.0, y0 + 10.0], "BRANCH CIRCUIT VOLTAGE DROP WIRING SCHEDULE", 2.0);
    let line = |s: &mut Session, a: [f64; 2], b: [f64; 2]| {
        s.execute("line", &json!({"points": [a, b]})).expect("line");
    };
    for yy in [7.0, 2.0, -3.0] {
        line(s, [x0, y0 + yy], [x0 + 120.0, y0 + yy]);
    }
    for xx in [0.0, 20.0, 40.0, 60.0, 80.0, 100.0, 120.0] {
        line(s, [x0 + xx, y0 + 7.0], [x0 + xx, y0 - 3.0]);
    }
    for (i, (hd, row)) in
        [("BREAKER", "20A"), ("VOLTAGE", "120V"), ("#12", "50'"), ("#10", "100'"), ("#8", "150'"), ("#6", "250'")].iter().enumerate()
    {
        let x = x0 + 2.0 + 20.0 * i as f64;
        text_at(s, [x, y0 + 4.0], hd, 1.5);
        text_at(s, [x, y0 - 1.0], row, 1.5);
    }
}

#[test]
fn the_drawings_wire_length_table_decides_the_conductor_before_the_three_percent_drop() {
    // Q6338: a set that prints a wire-length table by breaker and voltage: the table wins over
    // NEC 210.19's 3% calculation. LP1-5 runs over 150 ft: the table asks #6 for it.
    let mut s = building(Some(20), |_| {});
    drawn_length_table(&mut s);
    let (_, fs) = screen(&mut s, &["E-VDROP"]);
    let far = of(&fs, "E-VDROP").into_iter().find(|f| f["measured"]["circuit"] == "LP1-5").cloned().unwrap_or_default();
    assert!(far["measured"]["length_ft"].as_f64().is_some_and(|l| l > 150.0 && l <= 250.0), "{far}");
    assert_eq!(far["measured"]["needed"], "#6", "{far}");
    assert!(far["measured"]["table"].as_str().is_some_and(|t| t.contains("VOLTAGE DROP")), "{far}");
    assert!(far["measured"].get("drop_pct").is_none(), "the table, not the calculation: {far}");
    let conductor = far["measured"]["conductor"].as_str().unwrap_or_default().to_string();
    assert_eq!(far["verdict"], if conductor == "#6" { "pass" } else { "fail" }, "{far}");
    assert!(far["verdict"] == "pass" || far["issue"].as_str().is_some_and(|i| i.contains("asks #6")), "{far}");
    // Every circuit is judged by the table: a pass exactly when its conductor is the size the
    // table asks for its length, or larger.
    let rank = |s: &str| ["#14", "#12", "#10", "#8", "#6", "#4"].iter().position(|x| *x == s);
    for f in of(&fs, "E-VDROP") {
        let (have, need) =
            (rank(f["measured"]["conductor"].as_str().unwrap_or_default()), rank(f["measured"]["needed"].as_str().unwrap_or_default()));
        assert!(have.is_some() && need.is_some(), "{f}");
        assert_eq!(f["verdict"], if have >= need { "pass" } else { "fail" }, "{f}");
        let ft = f["measured"]["length_ft"].as_f64().unwrap_or_default();
        let want = if ft <= 50.0 {
            "#12"
        } else if ft <= 100.0 {
            "#10"
        } else if ft <= 150.0 {
            "#8"
        } else {
            "#6"
        };
        assert_eq!(f["measured"]["needed"], want, "{f}");
    }
}

#[test]
fn a_typical_panels_copies_each_judge_only_their_own_circuit() {
    // Q6338: a panel there is one of per unit (LP1, typical of 2: copies LP1-A and LP1-B), each
    // copy's circuit 1 in its own end of the building. Each copy's voltage drop is its own
    // circuit's: its farthest device and only its own pieces, never the other copy's.
    let devices = vec![
        receptacle(1, 20.0, 10.0, "LP1-1"),
        receptacle(2, 30.0, 10.0, "LP1-1"),
        receptacle(3, 125.0, 10.0, "LP1-1"),
        receptacle(4, 132.0, 10.0, "LP1-1"),
    ];
    let mut s = building_of(devices, Some(20), |_| {});
    with_spec(&mut s, |sp| {
        sp["mep"]["panel_schedules"][0]["typical_of"] = json!(2);
        let a = sp["mep"]["equipment"][0].clone();
        let mut b = a.clone();
        sp["mep"]["equipment"][0]["tag"] = json!("LP1-A");
        b["tag"] = json!("LP1-B");
        b["min"] = json!([1650, -6]);
        b["max"] = json!([1670, 6]);
        sp["mep"]["equipment"] = json!([sp["mep"]["equipment"][0].clone(), b]);
    });
    s.execute("model.build", &json!({})).expect("build");
    let (_, fs) = screen(&mut s, &["E-VDROP"]);
    let one: Vec<&Value> = of(&fs, "E-VDROP").into_iter().filter(|f| f["measured"]["circuit"] == "LP1-1").collect();
    let panels: std::collections::BTreeSet<&str> = one.iter().filter_map(|f| f["measured"]["panel"].as_str()).collect();
    assert_eq!(panels.len(), 2, "one finding per copy: {one:#?}");
    let handles = |f: &Value| -> std::collections::BTreeSet<String> {
        f["handles"].as_array().into_iter().flatten().filter_map(|h| h.as_str().map(str::to_string)).collect()
    };
    let (a, b) = (handles(one[0]), handles(one[1]));
    assert!(!a.is_empty() && !b.is_empty(), "{one:#?}");
    assert!(a.is_disjoint(&b), "each copy's finding covers its own pieces only: {:?}", a.intersection(&b).collect::<Vec<_>>());
    // Each copy's farthest device is one of its own end's.
    for f in &one {
        let at = f["at"][0].as_f64().unwrap_or_default();
        let near_a = f["measured"]["panel"] == "LP1-A";
        assert!(if near_a { at < 70.0 } else { at > 70.0 }, "{f}");
    }
}
