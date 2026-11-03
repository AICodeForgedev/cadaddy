//! Q6167 scopesplit: other trades' elements leave the electrical lines for `by_others`, each with
//! its party, why and the words that say so; our part of each stays; what no document says is a
//! review question with its default. Made-up plans and notes only.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    let layer = "E-ANNO";
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

/// A device the readers put in the spec at `(x, y)` feet.
fn device(x: f64, y: f64, mark: &str, kind: &str, extra: Value) -> Value {
    let mut fields = json!({
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("{} {mark}", kind.replace('_', " ")), "source": "plan"},
        "sheet": {"value": "E-311", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x1F0 + (x * 10.0 + y) as u64), "source": "plan"},
    });
    if let (Some(f), Some(x)) = (fields.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            f.insert(k.clone(), json!({"value": v, "source": "plan"}));
        }
    }
    json!({"id": format!("device:L1:{x:.1},{y:.1}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": fields})
}

/// A made-up office: two rooms (OFFICE 601, STORAGE 602), a receptacle, a thermostat, four fire
/// alarm devices, a TV outlet; a fire alarm symbol list on the sheet (an `H` two rows give).
fn office() -> Session {
    let mut d = Drawing::new_imperial();
    let outer = [((0., 0.), (40.5, 0.)), ((40.5, 0.), (40.5, 10.)), ((40.5, 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let west = [((0.5, 0.5), (19.75, 0.5)), ((19.75, 0.5), (19.75, 9.5)), ((19.75, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    let east = [((20.25, 0.5), (40.0, 0.5)), ((40.0, 0.5), (40.0, 9.5)), ((40.0, 9.5), (20.25, 9.5)), ((20.25, 9.5), (20.25, 0.5))];
    for (a, b) in outer.into_iter().chain(west).chain(east) {
        line(&mut d, "A-WALL", a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Office", None);
    for (at, t) in [([110.0, 64.0], "OFFICE"), ([116.0, 52.0], "601"), ([350.0, 64.0], "STORAGE"), ([356.0, 52.0], "602")] {
        if s.execute("layer.new", &json!({"name": "A-ROOM-TAG", "current": true})).is_err() {
            s.execute("layer.current", &json!({"name": "A-ROOM-TAG"})).expect("layer");
        }
        s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
    }
    text(&mut s, [3000.0, 900.0], "FIRE ALARM SYMBOL LIST");
    for (y, mark, words, mount) in [
        (870.0, "S", "SPOT SMOKE DETECTOR", "CEILING"),
        (850.0, "P", "DUAL ACTION PULL STATION", "48\" AFF TO TOP"),
        (830.0, "H", "HEAT DETECTOR, RATE OF RISE", "CEILING OR WALL"),
        (810.0, "H", "ADA HORN/STROBE", "WALL, 80\" AFF"),
    ] {
        text(&mut s, [2950.0, y], mark);
        text(&mut s, [2980.0, y], words);
        text(&mut s, [3200.0, y], mount);
    }
    let tags = vec![
        device(5.0, 0.8, "REC", "receptacle", json!({})),
        device(10.0, 0.8, "T", "thermostat", json!({})),
        device(5.0, 5.0, "S", "fire_alarm_device", json!({"mount": "ceiling"})),
        device(12.0, 5.0, "H", "fire_alarm_device", json!({"mount": "ceiling"})),
        device(18.0, 0.8, "P", "fire_alarm_device", json!({})),
        device(30.0, 5.0, "S", "fire_alarm_device", json!({"mount": "ceiling"})),
        device(25.0, 0.8, "TV", "low_voltage_outlet", json!({})),
    ];
    let sp = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": tags,
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

/// The notes of a made-up job: a mechanical schedule's note, an electrical legend's note and a
/// fire alarm notes list, as text.
fn notes() -> Value {
    json!([
        {"document": "M-701", "text": "FURNISH EACH UNIT WITH ITS WALL THERMOSTAT. NO CONDUIT OR BACK BOX IS REQUIRED FOR THE THERMOSTAT."},
        {"document": "E-302", "text": "NOTE: ALL LOW VOLTAGE DEVICES ARE ROUGH-IN ONLY. PROVIDE EACH WITH A 4\" SQUARE BOX AND A 3/4\" CONDUIT STUBBED UP INTO THE CEILING. VERIFY TV OUTLETS WITH THE LOW VOLTAGE VENDOR."},
        {"document": "E-312", "text": "FIRE ALARM NOTES: THE DEVICES AND CABLING OF THE FIRE ALARM SYSTEM ARE THE RESPONSIBILITY OF THE FIRE ALARM CONTRACTOR. THE ELECTRICIAN SHALL PROVIDE A 120V BRANCH CIRCUIT FOR THE FIRE ALARM CONTROL PANEL. THE FIRE ALARM CONTROL PANEL IS TO BE MOUNTED IN STORAGE ROOM 602."}
    ])
}

fn others(bom: &Value) -> Vec<&Value> {
    bom["by_others"].as_array().expect("by_others").iter().filter(|x| x["trade"] != "concrete").collect()
}

fn lines_of<'a>(bom: &'a Value, id: &str) -> Vec<&'a Value> {
    bom["lines"].as_array().expect("lines").iter().filter(|l| l["elements"].as_array().is_some_and(|e| e.iter().any(|x| x == id))).collect()
}

fn id(x: f64, y: f64) -> String {
    format!("device:L1:{x:.1},{y:.1}")
}

#[test]
fn other_trades_devices_are_listed_by_others_and_the_documents_decide_our_part() {
    let mut s = office();
    // Nothing read yet: each class by its default, said so.
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    let by = others(&bom);
    assert_eq!(by.len(), 6, "the thermostat, four fire alarm devices and the TV outlet: {by:#?}");
    assert!(
        by.iter().all(|x| x["decided"] == "default" && x["cost_to_us"] == 0.0 && x["reason"].as_str().unwrap().starts_with("no document says")),
        "{by:#?}"
    );
    let th = by.iter().find(|x| x["element"] == id(10.0, 0.8)).expect("thermostat");
    assert_eq!((th["trade"].as_str(), th["furnish"].as_str()), (Some("thermostat"), Some("mechanical")));
    // Its device isn't ours; its default rough-in is.
    let ours: Vec<&str> = lines_of(&bom, &id(10.0, 0.8)).iter().map(|l| l["category"].as_str().unwrap()).collect();
    assert!(!ours.contains(&"devices"), "{ours:?}");
    assert!(ours.contains(&"conduit"), "the default's empty conduit: {ours:?}");
    // The receptacle stays ours.
    assert!(lines_of(&bom, &id(5.0, 0.8)).iter().any(|l| l["category"] == "devices"));
    let devices: Vec<&str> =
        bom["lines"].as_array().unwrap().iter().filter(|l| l["category"] == "devices").map(|l| l["item"].as_str().unwrap()).collect();
    assert!(!devices.iter().any(|d| d.contains("thermostat") || d.contains("fire alarm") || d.contains("low voltage")), "{devices:?}");
    // Each class the documents don't settle is a question naming its default.
    let q = s.execute("review.list", &json!({"kind": "question", "limit": 50})).expect("list");
    let titles: Vec<&str> = q["items"].as_array().unwrap().iter().map(|i| i["title"].as_str().unwrap()).collect();
    for label in ["thermostats", "fire alarm devices", "low voltage devices"] {
        assert!(
            titles.iter().any(|t| t.contains(&format!("Who furnishes and installs the {label}")) && t.contains("default")),
            "{label}: {titles:#?}"
        );
    }

    // The documents read.
    let r = s.execute("bim.scope", &json!({"text": notes()})).expect("scope");
    let dec = |c: &str| r["decisions"].as_array().unwrap().iter().find(|d| d["class"] == c).cloned().unwrap_or_else(|| panic!("{c}: {r:#}"));
    let th = dec("thermostat");
    assert_eq!(
        (th["furnish"].as_str(), th["party_from"].as_str(), th["ours_from"].as_str()),
        (Some("mechanical"), Some("documents"), Some("documents")),
        "{th:#}"
    );
    assert_eq!(th["ours"], json!([]), "no box or conduit: {th:#}");
    assert_eq!(th["party_sources"][0]["document"], "M-701");
    let lv = dec("low voltage");
    assert_eq!((lv["furnish"].as_str(), lv["ours_from"].as_str()), (Some("low voltage"), Some("documents")), "{lv:#}");
    let fa = dec("fire alarm");
    assert!(fa["party_sources"][0]["text"].as_str().unwrap().contains("RESPONSIBILITY OF THE FIRE ALARM CONTRACTOR"), "{fa:#}");
    // The fire alarm devices take their legend rows' kinds; the `H` the ceiling makes a heat
    // detector (the horn/strobe row is a wall one).
    assert_eq!(r["fire_alarm"]["resolved"], 4, "{r:#}");
    let spec = s.execute("bspec.get", &json!({})).expect("spec");
    let tags = spec["spec"]["tags"].as_array().or_else(|| spec["tags"].as_array()).expect("tags").clone();
    let kind_of =
        |x: f64, y: f64| tags.iter().find(|t| t["id"] == id(x, y)).and_then(|t| t["fields"]["fa_kind"]["value"].as_str().map(str::to_string));
    assert_eq!(kind_of(5.0, 5.0).as_deref(), Some("smoke detector"));
    assert_eq!(kind_of(12.0, 5.0).as_deref(), Some("heat detector"));
    assert_eq!(kind_of(18.0, 0.8).as_deref(), Some("pull station"));
    // The panel where the notes put it, in room 602.
    assert_eq!(r["panels"]["placed"], json!(["FACP"]), "{r:#}");
    assert_eq!(r["panels"]["built"], 1, "{r:#}");

    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    let by = others(&bom);
    assert_eq!(by.len(), 7, "with the panel: {by:#?}");
    assert!(by.iter().all(|x| x["decided"] == "documents"), "{by:#?}");
    let of = |e: &str| by.iter().find(|x| x["element"] == e).copied().unwrap_or_else(|| panic!("{e}: {by:#?}"));
    assert_eq!(of(&id(5.0, 5.0))["kind"], "smoke detector S");
    assert!(of(&id(5.0, 5.0))["source"].as_str().unwrap().starts_with("E-312"), "{:#}", of(&id(5.0, 5.0)));
    assert_eq!(of("FACP")["kind"], "control panel FACP");
    // Ours: the thermostat nothing; the TV outlet its box and 3/4" conduit stub; the panel its
    // circuit; a detector nothing.
    assert!(lines_of(&bom, &id(10.0, 0.8)).is_empty(), "{:#?}", lines_of(&bom, &id(10.0, 0.8)));
    let tv: Vec<(&str, &str)> = lines_of(&bom, &id(25.0, 0.8)).iter().map(|l| (l["item"].as_str().unwrap(), l["size"].as_str().unwrap())).collect();
    assert!(tv.contains(&("Conduit stub to the ceiling space", "3/4\"")), "{tv:?}");
    assert!(
        of(&id(25.0, 0.8))["ours"].as_array().unwrap().iter().any(|o| o == "3/4\" Conduit stub to the ceiling space"),
        "{:#}",
        of(&id(25.0, 0.8))
    );
    let panel: Vec<&str> = lines_of(&bom, "FACP").iter().map(|l| l["item"].as_str().unwrap()).collect();
    assert_eq!(panel, ["Dedicated 120 V branch circuit"], "{panel:?}");
    assert!(lines_of(&bom, &id(5.0, 5.0)).is_empty());
    // Nothing left to ask.
    let q = s.execute("review.list", &json!({"kind": "question", "limit": 50})).expect("list");
    assert!(!q["items"].as_array().unwrap().iter().any(|i| i["title"].as_str().unwrap().contains("furnishes")), "{q:#}");
}

#[test]
fn an_answer_settles_what_no_document_does_and_the_model_leaves_out_boxes_not_ours() {
    let mut s = office();
    // The thermostat built with its box (the default makes it ours).
    let props = |s: &mut Session, mark: &str| {
        let r = s.execute("bim.props.find", &json!({"kind": "device", "where": {"type_mark": mark}, "limit": 10})).expect("find");
        let h = r["handles"][0].as_str().expect("handle").to_string();
        s.execute("bim.props.get", &json!({"handle": h})).expect("get")["elements"][0]["props"].clone()
    };
    assert!(props(&mut s, "T")["box"].is_object(), "{:#}", props(&mut s, "T"));
    // A person answers: the low voltage devices are ours to furnish and install.
    let q = s.execute("review.list", &json!({"kind": "question", "limit": 50})).expect("list");
    let item = q["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"].as_str().unwrap().contains("installs the low voltage devices"))
        .expect("question")
        .clone();
    s.execute("review.resolve", &json!({"id": item["id"], "answer": "ours", "author": "user"})).expect("resolve");
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    assert!(others(&bom).iter().all(|x| x["trade"] != "low voltage"), "{:#?}", others(&bom));
    assert!(lines_of(&bom, &id(25.0, 0.8)).iter().any(|l| l["category"] == "devices"), "ours again");
    // The mechanical note read: no box for the thermostat, and the rebuilt model leaves it out.
    s.execute("bim.scope", &json!({"text": [notes()[0].clone()], "panels": false})).expect("scope");
    s.execute("model.build", &json!({})).expect("build");
    assert!(props(&mut s, "T").get("box").is_none_or(Value::is_null), "{:#}", props(&mut s, "T"));
    assert!(props(&mut s, "REC")["box"].is_object(), "the receptacle keeps its box");
    // A person can't outrank the documents with a guess: their answer is the project's rule,
    // the drawing's note stays.
    let r = s
        .execute(
            "bim.scope",
            &json!({"set": [{"class": "thermostat", "ours": ["box"], "reason": "the foreman says so"}], "sheets": false, "panels": false}),
        )
        .expect("set");
    let th = r["decisions"].as_array().unwrap().iter().find(|d| d["class"] == "thermostat").unwrap();
    assert_eq!(th["ours"], json!([]), "{th:#}");
    // A class nobody named is refused.
    assert!(s.execute("bim.scope", &json!({"set": [{"class": "plumbing", "party": "owner", "reason": "x"}]})).is_err());
    assert!(s.execute("bim.scope", &json!({"set": [{"class": "thermostat", "party": "owner"}]})).is_err(), "a reason is needed");
}

#[test]
fn hostile_scope_params_never_panic() {
    let mut s = office();
    for p in [
        json!({"specs": 5}),
        json!({"specs": ["/no/such/folder"], "submittals": ["/no/such.pdf"], "pdfs": ["/no/such.pdf"]}),
        json!({"text": [{"document": 7, "text": null, "tier": "nonsense"}]}),
        json!({"text": "BY BY BY SHALL SHALL NO NO REQUIRED RESPONSIBILITY OF"}),
        json!({"set": "x"}),
        json!({"set": [{"class": "thermostat", "furnish": 3, "ours": [1, {"item": "nonsense"}], "reason": "r"}]}),
    ] {
        let _ = s.execute("bim.scope", &p);
    }
    let _ = s.execute("bim.bom", &json!({}));
}
