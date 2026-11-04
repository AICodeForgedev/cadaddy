//! G-PRODUCT and G-PRODUCT-CODE (Q6137) on a made-up job: guest rooms 101 and 102, a corridor
//! 103 and a patio 104. The wiring device section asks for white, 20 A 125 V, tamper-resistant
//! duplex receptacles from two listed makers; the submittal approves one maker and catalog
//! number for the DUPLEX type and one luminaire type. In 101: a red receptacle, one from a maker
//! the section doesn't list, one without its required TR, one with no product data at all, a
//! luminaire whose catalog number differs from the submittal's. In 102, round a lavatory, on
//! panel G (whose schedule marks the GFCI breakers): a GFCI receptacle on a GFCI breaker, a
//! standard one on a GFCI breaker, a standard one beside the sink on a standard breaker, a GFCI
//! receptacle feeding a standard one beside the sink, and a GFCI receptacle on a circuit the
//! schedule doesn't list. On the patio, a receptacle that isn't weather-resistant.

use std::path::PathBuf;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Source};
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Drawing, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// The plan's origin on the drawing (inches).
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
const RULES: [&str; 2] = ["G-PRODUCT", "G-PRODUCT-CODE"];

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on the plan.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let v = [at(a), at([b[0], a[1]]), at(b), at([a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [at(a), at(b)]})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": p, "text": t, "height": 6})).unwrap();
}

/// Device blocks at building points (feet), each with its TYPE attribute.
fn place(s: Session, items: &[([f64; 2], &str)]) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key("DEV") {
        let mut b = Block::new("DEV");
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert("DEV".into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for (p, ty) in items {
        let attribs = vec![Attrib {
            tag: "TYPE".into(),
            text: Text { value: (*ty).into(), ..style.clone() },
            invisible: true,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        }];
        let q = at(*p);
        let ins = Insert {
            block: "DEV".into(),
            insert: Vec3::new(q[0], q[1], 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs,
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Insert(ins));
        e.common.layer = "E-POWR".into();
        d.model.push(e);
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
}

/// Store properties on objects, as the job's readers do.
fn set_props(s: Session, sets: &[(&str, &str, Value)]) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    for (handle, key, value) in sets {
        let h = cadaddy_engine::doc::Handle::parse_hex(handle).unwrap();
        d.modify_entity(h, |e| {
            let mut store = props::read(&e.common.xdata);
            store.set(key, Prop::new(value.clone(), Source::computed()));
            e.common.xdata = props::write(&e.common.xdata, &store).into();
        })
        .unwrap();
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

/// What the wiring device section requires of a receptacle (as `bim.products` records it).
fn required() -> Value {
    json!({"section": "26 27 26", "page": "262726 - 3", "product": "DUPLEX RECEPTACLES: 20 A, 125 V", "makers": ["Acme", "Bolt"],
           "substitution": "listed makers only", "rating": "20 A, 125 V", "configuration": "duplex", "features": ["TR"], "color": "white",
           "color_rule": "white unless otherwise indicated"})
}

fn prints() -> Value {
    json!({
        "version": 1,
        "submittals": [
            {"mark": "DUPLEX", "section": "26 27 26", "fields": {"manufacturer": "Acme", "catalogNumber": "XR-100"}},
            {"mark": "A", "section": "26 51 19", "fields": {"manufacturer": "Acme Lighting", "catalogNumber": "LX-100"}}
        ]
    })
}

fn spec() -> Value {
    let dev = json!({"ceiling": false, "aff_ft": 1.5, "size_ft": [0.25, 0.1, 0.33]});
    let lum = json!({"ceiling": true, "aff_ft": 9.0, "size_ft": [0.5, 0.5, 0.3]});
    let fix = json!({"ceiling": false, "aff_ft": 2.8, "size_ft": [1.5, 1.5, 0.5]});
    let panel = json!({"ceiling": false, "aff_ft": 5.0, "size_ft": [1.2, 0.3, 2.0]});
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-POWR", "role": "device"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Guest Room", "number": "101"}, {"name": "Guest Room", "number": "102"}, {"name": "Corridor", "number": "103"}, {"name": "Patio", "number": "104"}],
        "devices": {"by_type": {"DUPLEX": dev, "QUAD": dev, "GFI": dev, "A": lum, "LAV": fix, "G": panel}},
        "sheets": [
            {"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
             "registration": {"reference": "A-101", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}}
        ],
        "mep": {"panel_schedules": [{"panel": "G", "source": "E-601", "legend": {"GF": "GROUND FAULT CIRCUIT INTERRUPTER"}, "circuits": [
            {"number": "1", "description": "RCPT", "protection": ["GF"]},
            {"number": "2", "description": "RCPT", "protection": ["GF"]},
            {"number": "3", "description": "RCPT"},
            {"number": "4", "description": "RCPT"}
        ]}]},
        "prints": prints()
    })
}

struct Job {
    s: Session,
    /// 101: red; from a maker not listed; without TR; no product data; luminaire of another
    /// catalog number.
    red: String,
    maker: String,
    no_tr: String,
    bare: String,
    lum: String,
    /// 102: GFCI on a GFCI breaker; standard on a GFCI breaker; standard at the sink on a standard
    /// breaker; the GFCI feeding `fed` (at the sink) on one circuit; a GFCI on an unlisted circuit.
    double: String,
    on_gf: String,
    at_sink: String,
    feeder: String,
    fed: String,
    unknown: String,
    /// 104: a receptacle that isn't weather-resistant.
    patio: String,
}

fn job() -> Job {
    let mut s = Session::new();
    // A 100' × 12' shell; partitions at 15', 30' and 60'.
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [100.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [99.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    for x in [15.0, 30.0, 60.0] {
        line(&mut s, [x, 0.5], [x, 11.5]);
    }
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([5.0, 6.0]), "101");
    text(&mut s, at([20.0, 6.0]), "102");
    text(&mut s, at([45.0, 6.0]), "103");
    text(&mut s, at([80.0, 6.0]), "104");
    let (s, h) = place(
        s,
        &[
            ([2.0, 0.5], "DUPLEX"),   // 0 red
            ([5.0, 0.5], "QUAD"),     // 1 maker
            ([8.0, 0.5], "DUPLEX"),   // 2 no TR
            ([11.0, 0.5], "DUPLEX"),  // 3 bare
            ([7.0, 6.0], "A"),        // 4 luminaire
            ([22.0, 10.5], "LAV"),    // 5 lavatory
            ([16.0, 6.0], "G"),       // 6 panel
            ([21.0, 11.5], "GFI"),    // 7 double
            ([28.0, 0.5], "DUPLEX"),  // 8 on a GFCI breaker
            ([23.5, 11.5], "DUPLEX"), // 9 at the sink
            ([17.0, 0.5], "GFI"),     // 10 feeder
            ([20.0, 11.5], "DUPLEX"), // 11 fed
            ([26.0, 0.5], "GFI"),     // 12 unknown circuit
            ([80.0, 0.5], "DUPLEX"),  // 13 patio
        ],
    );
    let mut s = s;
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let kind = |s: &mut Session, hs: &[&String], k: &str| {
        s.execute("bim.source", &json!({"handles": hs, "status": "fixed", "source": "E-101", "kind": k})).unwrap();
    };
    kind(&mut s, &[&h[0], &h[1], &h[2], &h[3], &h[8], &h[9], &h[11], &h[13]], "receptacle");
    kind(&mut s, &[&h[7], &h[10], &h[12]], "gfci_receptacle");
    kind(&mut s, &[&h[4]], "luminaire");
    kind(&mut s, &[&h[5]], "lavatory");
    kind(&mut s, &[&h[6]], "panel");
    let record = |resolved: Value| {
        json!(json!({"required": required(), "approved": {"none": "no submittal for this kind"}, "resolved": resolved}).to_string())
    };
    let circ = |n: &str| json!(format!("G-{n}"));
    let s = set_props(
        s,
        &[
            // 101.
            (&h[0], "manufacturer", json!("Acme")),
            (&h[0], "catalog_number", json!("XR-100")),
            (&h[0], "color", json!("red")),
            (&h[0], "configuration", json!("duplex")),
            (&h[0], "rating", json!("20A 125V")),
            (&h[0], "features", json!("TR")),
            (&h[0], "product", record(json!({"maker": "Acme", "features": ["TR"]}))),
            (&h[1], "manufacturer", json!("Zed Corp")),
            (&h[1], "tamper_resistant", json!("yes")),
            (&h[1], "product", record(json!({"features": ["TR"]}))),
            (
                &h[2],
                "product",
                record(
                    json!({"maker": "Acme", "catalog": "XR-100", "color": "white", "configuration": "duplex", "rating": "20 A, 125 V", "features": ["duplex"]}),
                ),
            ),
            (&h[4], "catalog_number", json!("LX-200")),
            // 102: the circuits.
            (&h[7], "panel", json!("G")),
            (&h[7], "circuit", circ("1")),
            (&h[8], "panel", json!("G")),
            (&h[8], "circuit", circ("2")),
            (&h[9], "panel", json!("G")),
            (&h[9], "circuit", circ("3")),
            (&h[10], "panel", json!("G")),
            (&h[10], "circuit", circ("4")),
            (&h[11], "panel", json!("G")),
            (&h[11], "circuit", circ("4")),
            (&h[12], "panel", json!("G")),
            (&h[12], "circuit", circ("9")),
            // 104.
            (&h[13], "features", json!("TR, duplex")),
        ],
    );
    Job {
        s,
        red: h[0].clone(),
        maker: h[1].clone(),
        no_tr: h[2].clone(),
        bare: h[3].clone(),
        lum: h[4].clone(),
        double: h[7].clone(),
        on_gf: h[8].clone(),
        at_sink: h[9].clone(),
        feeder: h[10].clone(),
        fed: h[11].clone(),
        unknown: h[12].clone(),
        patio: h[13].clone(),
    }
}

fn screen(s: &mut Session) -> (Value, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": RULES})).unwrap();
    let mut fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    let mut cursor = r["next"]["cursor"].as_u64();
    while let Some(c) = cursor {
        let p = s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50, "rules": RULES, "cursor": c})).unwrap();
        fs.extend(p["findings"].as_array().cloned().unwrap_or_default());
        cursor = p["next"]["cursor"].as_u64();
    }
    assert!(fs.iter().all(|f| RULES.contains(&f["rule"].as_str().unwrap_or_default())), "{r}");
    (r, fs)
}

/// The finding of `rule` and `check` on `handle`.
fn of<'a>(fs: &'a [Value], handle: &str, rule: &str, check: &str) -> &'a Value {
    fs.iter()
        .find(|f| f["rule"] == rule && f["check"] == check && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(handle)))
        .unwrap_or_else(|| {
            panic!(
                "no {rule} {check} finding on {handle}: {}",
                json!(fs.iter().filter(|f| f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(handle))).collect::<Vec<_>>())
            )
        })
}

/// Every rule id an element of `kind` gets (all pages).
fn rule_ids(s: &mut Session, kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0u64;
    loop {
        let r = s.execute("validate.rules", &json!({"kind": kind, "cursor": cursor})).unwrap();
        let page: Vec<String> = r["rules"].as_array().unwrap().iter().filter_map(|x| x["id"].as_str().map(str::to_string)).collect();
        if page.is_empty() {
            break;
        }
        cursor += page.len() as u64;
        out.extend(page);
        if r["next"].is_null() && r["cursor_next"].is_null() && r["next_cursor"].is_null() {
            break;
        }
    }
    out
}

#[test]
fn the_rules_are_built_in_and_on() {
    let mut s = Session::new();
    let r = s.execute("validate.rules", &json!({"id": "G-PRODUCT", "detail": true})).unwrap();
    assert!(r.to_string().contains("approved list"), "{r}");
    let ids = rule_ids(&mut s, "gfci_receptacle");
    assert!(ids.iter().any(|i| i == "G-PRODUCT") && ids.iter().any(|i| i == "G-PRODUCT-CODE"), "{ids:?}");
    let ids = rule_ids(&mut s, "lavatory");
    assert!(
        ids.iter().any(|i| i == "G-PRODUCT") && !ids.iter().any(|i| i == "G-PRODUCT-CODE"),
        "a plumbing fixture's product is checked, its code features aren't: {ids:?}"
    );
}

#[test]
fn products_are_checked_field_by_field_against_the_submittal_and_the_spec() {
    let mut j = job();
    let (r, fs) = screen(&mut j.s);
    // A red receptacle where the spec says white: fail, the section and page cited.
    let red = of(&fs, &j.red, "G-PRODUCT", "color");
    assert_eq!(red["verdict"], "fail", "{red}");
    assert!(red["expected"].as_str().unwrap().contains("26 27 26 p.262726 - 3"), "{red}");
    assert_eq!(of(&fs, &j.red, "G-PRODUCT", "maker")["verdict"], "pass", "the submittal's maker");
    let cat = of(&fs, &j.red, "G-PRODUCT", "catalog");
    assert_eq!(cat["verdict"], "pass", "{cat}");
    assert_eq!(cat["measured"]["requires"]["from"], "submittal", "{cat}");
    for f in ["configuration", "rating", "features"] {
        assert_eq!(of(&fs, &j.red, "G-PRODUCT", f)["verdict"], "pass", "{f}");
    }
    // A maker the section doesn't list (its type has no submittal): fail.
    let m = of(&fs, &j.maker, "G-PRODUCT", "maker");
    assert_eq!(m["verdict"], "fail", "{m}");
    assert!(m["issue"].as_str().unwrap().contains("not an approved maker"), "{m}");
    assert!(m["expected"].as_str().unwrap().contains("Acme or Bolt") && m["expected"].as_str().unwrap().contains("listed makers only"), "{m}");
    // A receptacle whose product lacks the required TR.
    let f = of(&fs, &j.no_tr, "G-PRODUCT", "features");
    assert_eq!(f["verdict"], "fail", "{f}");
    assert!(f["issue"].as_str().unwrap().contains("lacks TR"), "{f}");
    // No product data: unsure, never a pass, counted per kind.
    let bare = of(&fs, &j.bare, "G-PRODUCT", "record");
    assert_eq!(bare["verdict"], "unsure", "{bare}");
    assert!(bare["issue"].as_str().unwrap().starts_with("no product data"), "{bare}");
    assert!(
        !fs.iter()
            .any(|f| f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.bare)) && f["rule"] == "G-PRODUCT" && f["verdict"] == "pass")
    );
    assert!(r["unmeasured"]["G-PRODUCT"]["receptacle: no product data"].as_u64().unwrap_or(0) >= 1, "{}", r["unmeasured"]);
    let rec = &r["products"]["by_kind"]["receptacle"];
    assert!(rec["no_product_data"].as_u64().unwrap() >= 1 && rec["with_product_data"].as_u64().unwrap() >= 3, "{}", r["products"]);
    assert!(rec["by_field"]["color"]["fail"].as_u64().unwrap() >= 1, "{}", r["products"]);
    assert!(r["products"]["by_field"]["maker"]["accuracy"].is_number(), "{}", r["products"]);
    // A luminaire whose catalog number differs from the submittal's.
    let lum = of(&fs, &j.lum, "G-PRODUCT", "catalog");
    assert_eq!(lum["verdict"], "fail", "{lum}");
    assert!(lum["expected"].as_str().unwrap().contains("LX-100"), "{lum}");
    assert_eq!(of(&fs, &j.lum, "G-PRODUCT", "maker")["verdict"], "unsure", "no maker in the model: unsure, not a pass");
    assert!(r["checked"]["G-PRODUCT"]["fail"].as_u64().unwrap() >= 4, "{}", r["checked"]);
}

#[test]
fn code_features_are_checked_where_the_code_wants_them() {
    let mut j = job();
    let (_, fs) = screen(&mut j.s);
    // Tamper-resistance in the guest rooms, from the code rules data.
    let tr = of(&fs, &j.no_tr, "G-PRODUCT-CODE", "tamper_resistant");
    assert_eq!(tr["verdict"], "fail", "{tr}");
    assert!(tr["measured"]["code"]["cite"].as_str().unwrap().contains("406.12"), "{tr}");
    assert!(!tr["measured"]["code"]["rule"].as_str().unwrap().is_empty(), "the rule's text is cited: {tr}");
    assert_eq!(of(&fs, &j.red, "G-PRODUCT-CODE", "tamper_resistant")["verdict"], "pass");
    assert_eq!(of(&fs, &j.maker, "G-PRODUCT-CODE", "tamper_resistant")["verdict"], "pass");
    assert_eq!(of(&fs, &j.bare, "G-PRODUCT-CODE", "tamper_resistant")["verdict"], "unsure");
    // Weather-resistance outdoors.
    let wr = of(&fs, &j.patio, "G-PRODUCT-CODE", "weather_resistant");
    assert_eq!(wr["verdict"], "fail", "{wr}");
    assert!(wr["measured"]["code"]["cite"].as_str().unwrap().contains("406.9"), "{wr}");
    assert!(
        !fs.iter().any(|f| f["check"] == "tamper_resistant" && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.patio))),
        "a patio is no guest room"
    );
}

#[test]
fn gfci_is_judged_on_the_whole_circuit() {
    let mut j = job();
    let (_, fs) = screen(&mut j.s);
    // GFCI breaker + GFCI receptacle: double protection.
    let d = of(&fs, &j.double, "G-PRODUCT-CODE", "gfci");
    assert_eq!(d["verdict"], "fail", "{d}");
    assert!(d["issue"].as_str().unwrap().contains("double protection"), "{d}");
    assert!(d["measured"]["breaker_row"].as_str().unwrap().contains("panel G schedule on E-601, circuit 1"), "the breaker's row cited: {d}");
    // GFCI breaker + standard receptacle: pass.
    let g = of(&fs, &j.on_gf, "G-PRODUCT-CODE", "gfci");
    assert_eq!(g["verdict"], "pass", "{g}");
    // Standard breaker + standard receptacle at a sink: fail.
    let s = of(&fs, &j.at_sink, "G-PRODUCT-CODE", "gfci");
    assert_eq!(s["verdict"], "fail", "{s}");
    assert!(s["measured"]["needs"].as_str().unwrap().contains("from sink"), "{s}");
    // Feed-through: the standard receptacle at the sink is on the load side of the circuit's
    // GFCI receptacle nearer the panel, named.
    let fed = of(&fs, &j.fed, "G-PRODUCT-CODE", "gfci");
    assert_eq!(fed["verdict"], "pass", "{fed}");
    assert!(fed["measured"]["protected_by"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(&j.feeder)), "{fed}");
    assert_eq!(of(&fs, &j.feeder, "G-PRODUCT-CODE", "gfci")["verdict"], "pass");
    // A circuit the schedule doesn't list: unsure, breaker type unknown.
    let u = of(&fs, &j.unknown, "G-PRODUCT-CODE", "gfci");
    assert_eq!(u["verdict"], "unsure", "{u}");
    assert!(u["issue"].as_str().unwrap().starts_with("breaker type unknown"), "{u}");
}

/// The verdicts come from the schedule, not from what the build stamped on the device.
#[test]
fn stamped_gfci_verdicts_are_not_trusted() {
    let j = job();
    let s = set_props(
        j.s,
        &[
            (&j.at_sink, "gfci_breaker", json!("yes")),
            (&j.at_sink, "gfci", json!("breaker")),
            (&j.at_sink, "protected_by", json!("breaker, circuit G-3")),
        ],
    );
    let mut s = s;
    let (_, fs) = screen(&mut s);
    assert_eq!(of(&fs, &j.at_sink, "G-PRODUCT-CODE", "gfci")["verdict"], "fail");
}

#[test]
fn a_rule_turned_off_is_not_run() {
    let mut j = job();
    j.s.execute("validate.rules.set", &json!({"id": "G-PRODUCT", "enabled": false, "reason": "test"})).unwrap();
    let r = j.s.execute("validate.prescreen", &json!({"level": "L1", "all": true, "limit": 50})).unwrap();
    assert!(r["findings"].as_array().unwrap().iter().all(|f| f["rule"] != "G-PRODUCT"), "{r}");
    assert!(r["disabled"].as_array().unwrap().iter().any(|x| x == "G-PRODUCT"), "{r}");
}

#[test]
fn the_run_status_gives_product_accuracy_per_kind_and_field() {
    let mut j = job();
    let out = dir("run");
    let r = j.s.execute("validate", &json!({"scope": "level", "level": "L1", "rules": RULES, "out_dir": out.to_string_lossy()})).unwrap();
    assert!(r["prescreen"]["measured"]["products"]["by_kind"]["receptacle"].is_object(), "{}", r["prescreen"]);
    let st = j.s.execute("validate.status", &json!({})).unwrap();
    let acc = &st["accuracy"]["products"];
    assert!(acc["by_kind"]["receptacle"].is_number(), "{st}");
    assert!(acc["by_field"]["color"].is_number() && acc["by_field"]["gfci"].is_number(), "{st}");
    let _ = std::fs::remove_dir_all(&out);
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-validateproducts-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A specification page as a PDF reader gives it: a page frame and the section's lines.
fn fake_pdf(_: &[u8], _: &Value) -> Result<(Drawing, Value), String> {
    let mut s = Session::new();
    s.execute("pline", &json!({"vertices": [[0, 0], [612, 0], [612, 792], [0, 792]], "closed": true})).map_err(|e| e.to_string())?;
    s.execute("text", &json!({"at": [72, 600], "text": "2.2 DUPLEX RECEPTACLES", "height": 10})).map_err(|e| e.to_string())?;
    s.execute("text", &json!({"at": [90, 580], "text": "A. Makers: Acme, Bolt. 20 A, 125 V, white, tamper-resistant.", "height": 10}))
        .map_err(|e| e.to_string())?;
    let d = (*s.doc().map_err(|e| e.to_string())?).clone();
    Ok((d, json!({})))
}

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(fake_pdf),
        read_dwf: None,
        write_dwfx: None,
    });
}

#[test]
fn a_packet_carries_the_product_records_verdicts_and_spec_pages() {
    hooks();
    let mut j = job();
    let specs = dir("specs");
    std::fs::create_dir_all(&specs).unwrap();
    std::fs::write(specs.join("262726 Wiring Devices.pdf"), b"%PDF-1.4 made up").unwrap();
    let areas = j.s.execute("validate.areas", &json!({})).unwrap();
    let id = areas["areas"].as_array().unwrap().iter().find(|a| a["number"] == "101").unwrap()["id"].as_str().unwrap().to_string();
    let out = dir("packet");
    let r =
        j.s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "max_images": 40, "specs": [specs.to_string_lossy()]}))
            .unwrap();
    let objects = walk(&out, "objects.json").unwrap_or_else(|| panic!("no objects.json: {r}"));
    let o: Value = serde_json::from_str(&std::fs::read_to_string(&objects).unwrap()).unwrap();
    let el = |h: &str| {
        o["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["handle"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(h)))
            .cloned()
            .unwrap_or_else(|| panic!("{h} not in {o}"))
    };
    let red = el(&j.red);
    assert_eq!(red["product_record"]["required"]["color"], "white", "{red}");
    let checks = red["product_checks"].as_array().unwrap();
    assert!(checks.iter().any(|c| c["check"] == "color" && c["verdict"] == "fail"), "{red}");
    assert!(o["prescreen"]["checked"]["G-PRODUCT"].is_object(), "{}", o["prescreen"]);
    // The spec page the records cite, cut to the product's paragraph.
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(walk(&out, "manifest.json").unwrap()).unwrap()).unwrap();
    let page = manifest["specs"].as_array().unwrap().iter().find(|x| x["section"] == "26 27 26").cloned().unwrap_or_else(|| panic!("{manifest}"));
    assert_eq!(page["page"], 3, "{page}");
    assert_eq!(page["cropped_to_product"], true, "{page}");
    assert!(out.join(page["file"].as_str().unwrap()).is_file(), "{page}");
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&specs);
}

#[test]
fn a_spec_page_no_file_holds_is_named_in_left_out() {
    let mut j = job();
    let areas = j.s.execute("validate.areas", &json!({})).unwrap();
    let id = areas["areas"].as_array().unwrap().iter().find(|a| a["number"] == "101").unwrap()["id"].as_str().unwrap().to_string();
    let out = dir("packet-none");
    let r = j.s.execute("validate.packet", &json!({"area": id, "out_dir": out.to_string_lossy(), "max_images": 1})).unwrap();
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(walk(&out, "manifest.json").unwrap()).unwrap()).unwrap();
    let left = manifest["left_out"].to_string();
    assert!(left.contains("spec 26 27 26 p.262726 - 3"), "{r}");
    let _ = std::fs::remove_dir_all(&out);
}

/// The first file named `name` under `dir`.
fn walk(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = walk(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}
