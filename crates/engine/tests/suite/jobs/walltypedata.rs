//! Q6594 fix-wall-type-data-not-on-walls: every wall element shows its wall type's legend data
//! (assembly by side, fire rating and tested design, acoustic rating, stud size and spacing, head
//! condition) as derived properties; a person's value on one wall stays; an untyped wall lists
//! them as missing, saying why. Made-up wall types and plans only.

use crate::helper::serial;
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

/// One straight wall 12' long, its faces 6" apart, `y` feet up the sheet, on `layer`.
fn wall(d: &mut Drawing, layer: &str, y: f64) {
    line(d, layer, (0.0, y), (12.0, y));
    line(d, layer, (0.0, y + 0.5), (12.0, y + 0.5));
}

const LAYERS: [&str; 6] = ["A-WALL-WOOD", "A-WALL-STEEL", "A-WALL-CMU", "A-WALL-NOLEGEND", "A-WALL-UNTYPED", "A-WALL-NOGAUGE"];

fn layer(material: &str, thickness: f64, side: &str, category: &str, cavity: bool) -> Value {
    json!({"material": material, "thickness_in": thickness, "side": side, "category": category, "in_cavity": cavity})
}

/// Three legend formats: a wood stud wall whose spacing is in its stud callout, rated with a
/// tested design and an acoustic rating, to the deck on a deflection track; a steel stud wall
/// whose spacing is the layer's own field and that is not rated; a masonry wall (no studs).
fn spec() -> Value {
    let wood = json!({
        "tag": "PX7", "description": "TYPE W PARTITION - RATED UNIT DEMISING",
        "layers": [
            layer("5/8\" Type X gypsum board", 0.625, "side1", "board", false),
            layer("2x6 wood studs @ 16\" o.c.", 5.5, "core", "framing", false),
            layer("R-19 batt insulation", 5.5, "core", "insulation", true),
            layer("resilient channel", 0.5, "side2", "furring", false),
            layer("5/8\" Type X gypsum board", 0.625, "side2", "board", false),
        ],
        "height": "deck", "fire_rating": "1 HR", "acoustic_rating": "STC 53",
        "notes": ["tested assembly UL U999"],
        "head": {"track": "deflection", "to": "deck", "deflection_in": 0.75, "bottom_track": false, "detail": "5/A-900"},
        "source": "A-900 partition type legend, mark PX7",
    });
    let mut steel_studs = layer("3 5/8\" 20 ga metal studs", 3.625, "core", "framing", false);
    steel_studs["spacing_in"] = json!(24);
    let steel = json!({
        "tag": "QS4", "description": "TYPE S PARTITION",
        "layers": [layer("1/2\" gypsum board", 0.5, "side1", "board", false), steel_studs, layer("1/2\" gypsum board", 0.5, "side2", "board", false)],
        "height": "above_ceiling",
        "source": "A-901 wall types, mark QS4",
    });
    let cmu = json!({
        "tag": "M8", "description": "8\" CMU",
        "layers": [layer("8\" concrete masonry units", 7.625, "core", "masonry", false)],
        "fire_rating": "2 HR",
        "source": "A-901 wall types, mark M8",
    });
    // A steel stud wall whose legend names no gauge and no stud depth (the reader's stand-in).
    let no_gauge = json!({
        "tag": "QS9",
        "layers": [layer("3 1/2\" metal studs (depth not given)", 3.5, "core", "framing", false), layer("5/8\" gypsum board", 0.625, "side1", "board", false)],
        "source": "A-901 wall types, mark QS9",
    });
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [
            {"layer": LAYERS[0], "role": "wall", "wall_type": "PX7"},
            {"layer": LAYERS[1], "role": "wall", "wall_type": "QS4"},
            {"layer": LAYERS[2], "role": "wall", "wall_type": "M8"},
            {"layer": LAYERS[3], "role": "wall", "wall_type": "Partition"},
            {"layer": LAYERS[4], "role": "wall"},
            {"layer": LAYERS[5], "role": "wall", "wall_type": "QS9"},
        ],
        "wall_types": [
            {"name": "PX7", "thickness_in": 6, "assembly": wood},
            {"name": "QS4", "thickness_in": 6, "assembly": steel},
            {"name": "M8", "thickness_in": 6, "assembly": cmu},
            {"name": "Partition", "thickness_in": 6},
            {"name": "QS9", "thickness_in": 6, "assembly": no_gauge},
        ],
        // The wood wall's type is tagged on the plan; the others take their layer's type.
        "walls": [{"level": "L1", "a": [0, 3], "b": [144, 3], "wall_type": "PX7", "source": "A-100", "status": "drawing"}],
    })
}

fn job() -> Session {
    let mut d = Drawing::new_imperial();
    for (i, l) in LAYERS.iter().enumerate() {
        wall(&mut d, l, 4.0 * i as f64);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Wall Type Data", None);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    s
}

/// The wall elements, each with its properties and its form, by the wall's place up the sheet
/// (the order of [`LAYERS`]).
fn walls(s: &mut Session) -> Vec<(String, Value, Value)> {
    let found = s.execute("bim.props.find", &json!({"kind": "wall", "limit": 100})).expect("find");
    let hs: Vec<String> = found["handles"].as_array().expect("handles").iter().filter_map(|h| h.as_str().map(str::to_string)).collect();
    let mut out: Vec<(f64, String, Value, Value)> = Vec::new();
    for h in hs {
        let props = s.execute("bim.props.get", &json!({"handle": h})).expect("get")["elements"][0]["props"].clone();
        let look = s.execute("bim.element", &json!({"handle": h})).expect("element");
        let e = s.doc().expect("doc").model.get(Handle::parse_hex(&h).expect("hex")).expect("wall").clone();
        let b = cadaddy_engine::doc::entity_bounds(s.doc().expect("doc"), &e, 0);
        out.push((b.center().y, h, props, look["form"].clone()));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(out.len(), LAYERS.len(), "one wall per layer: {:?}", out.iter().map(|w| &w.2["type"]).collect::<Vec<_>>());
    out.into_iter().map(|(_, h, p, f)| (h, p, f)).collect()
}

fn text(p: &Value, key: &str) -> String {
    p[key]["value"].as_str().map(str::to_string).unwrap_or_else(|| panic!("{key} as text in {p:#}"))
}

fn field<'a>(form: &'a Value, key: &str) -> &'a Value {
    form["fields"].as_array().and_then(|fs| fs.iter().find(|f| f["key"] == key)).unwrap_or_else(|| panic!("{key} in {form:#}"))
}

#[test]
fn each_wall_shows_its_types_legend_data_with_the_legend_as_source() {
    let _g = serial();
    let mut s = job();
    let ws = walls(&mut s);
    let (_, wood, form) = &ws[0];
    let asm = text(wood, "assembly");
    assert!(
        asm.contains("side 1: 5/8\" Type X gypsum board")
            && asm.contains("2x6 wood studs")
            && asm.contains("R-19 batt insulation")
            && asm.contains("side 2: resilient channel + 5/8\" Type X gypsum board"),
        "{asm}"
    );
    assert_eq!(text(wood, "fire_rating"), "1 HR");
    assert_eq!(text(wood, "tested_design"), "UL U999");
    assert_eq!(text(wood, "acoustic_rating"), "STC 53");
    assert_eq!(text(wood, "stud_size"), "2x6");
    assert_eq!(wood["stud_spacing"]["value"].as_f64(), Some(16.0), "{wood:#}");
    assert_eq!(wood["stud_spacing"]["unit"], "in");
    assert_eq!(text(wood, "insulation"), "R-19 batt insulation");
    let head = text(wood, "head_condition");
    assert!(head.contains("deck") && head.contains("deflection"), "{head}");
    for key in ["assembly", "fire_rating", "tested_design", "stud_size", "stud_spacing", "head_condition"] {
        assert_eq!(wood[key]["source"]["kind"], "legend", "{key}: {wood:#}");
        assert!(wood[key]["source"]["doc"].as_str().is_some_and(|d| d.contains("A-900")), "{key} names the legend sheet: {wood:#}");
    }
    for key in ["assembly", "fire_rating", "stud_size", "stud_spacing"] {
        assert_eq!(field(form, key)["status"], "documented", "{key}: {form:#}");
    }

    // The steel wall: its layer's spacing and its gauge; no rating on the legend = not rated.
    let (_, steel, _) = &ws[1];
    assert_eq!(text(steel, "stud_size"), "3 5/8\" 20 ga");
    assert_eq!(steel["stud_spacing"]["value"].as_f64(), Some(24.0));
    assert_eq!(text(steel, "fire_rating"), "not rated");
    assert!(text(steel, "head_condition").contains("above the ceiling"));
    assert!(steel.get("tested_design").is_none_or(|v| v.is_null()), "{steel:#}");
    // Its type is its layer's, not tagged: the legend's values are derived on it, by that rule.
    for key in ["assembly", "stud_size", "stud_spacing"] {
        assert_eq!(steel[key]["source"]["kind"], "rule", "{key}: {steel:#}");
        assert!(steel[key]["source"]["doc"].as_str().is_some_and(|d| d.contains("A-901") && d.contains("not tagged")), "{key}: {steel:#}");
    }
    assert_eq!(field(&ws[1].2, "stud_size")["status"], "derived");

    // The masonry wall has no studs: not applicable, never a default.
    let (_, cmu, form) = &ws[2];
    assert_eq!(text(cmu, "fire_rating"), "2 HR");
    assert!(text(cmu, "assembly").contains("8\" concrete masonry units"));
    assert_eq!(field(form, "stud_size")["status"], "n/a", "{form:#}");
    assert_eq!(field(form, "stud_spacing")["status"], "n/a", "{form:#}");

    // No gauge and no depth on the legend: the size says so and is a default; the gauge is the
    // rules' (a default) on its own; the spacing the rules' default.
    let (_, ng, form) = &ws[5];
    assert_eq!(text(ng, "stud_size"), "3 1/2\" metal stud");
    assert_eq!(field(form, "stud_size")["status"], "default", "{form:#}");
    assert_eq!(text(ng, "stud_gauge"), "20 ga");
    assert_eq!(ng["stud_gauge"]["source"]["kind"], "default", "{ng:#}");
    assert_eq!(ng["stud_spacing"]["value"].as_f64(), Some(16.0));
    assert_eq!(ng["stud_spacing"]["source"]["kind"], "default", "{ng:#}");
    // A gauge the legend names is in the size, with no separate gauge.
    assert!(steel.get("stud_gauge").is_none_or(|v| v.is_null()), "{steel:#}");
}

#[test]
fn an_untyped_wall_lists_the_type_data_missing_and_says_why() {
    let _g = serial();
    let mut s = job();
    let ws = walls(&mut s);
    for (i, why) in [(3usize, "wall type Partition has no legend entry"), (4, "untyped wall")] {
        let (_, _, form) = &ws[i];
        for key in ["assembly", "fire_rating", "stud_size", "stud_spacing"] {
            let f = field(form, key);
            assert_eq!(f["status"], "missing", "{key}: {form:#}");
            assert!(f["reason"].as_str().is_some_and(|r| r.contains(why)), "{key} says {why}: {f:#}");
        }
    }
    // The completeness report counts the typed walls filled and the others missing.
    let c = s.execute("bim.completeness", &json!({"kind": "wall"})).expect("completeness");
    let fields = &c["kinds"]["wall"]["fields"];
    for key in ["assembly", "fire_rating"] {
        assert_eq!(fields[key]["filled"].as_u64(), Some(4), "{key}: {fields:#}");
        assert_eq!(fields[key]["missing"].as_u64(), Some(2), "{key}: {fields:#}");
    }
}

#[test]
fn a_persons_value_on_one_wall_outranks_its_types_and_a_retyped_wall_takes_the_new_types_data() {
    let _g = serial();
    let mut s = job();
    let ws = walls(&mut s);
    let wood = ws[0].0.clone();
    let untyped = ws[4].0.clone();
    s.execute("bim.props.set", &json!({"handle": wood, "key": "fire_rating", "value": "2 HR", "note": "field verified"})).expect("set");
    s.execute("bim.props.set", &json!({"handle": untyped, "key": "type", "value": "QS4"})).expect("retype");
    s.execute("bim.props.fill", &json!({})).expect("fill again");
    let ws = walls(&mut s);
    assert_eq!(text(&ws[0].1, "fire_rating"), "2 HR");
    assert_eq!(ws[0].1["fire_rating"]["source"]["kind"], "user");
    // The other type data stays the legend's.
    assert_eq!(text(&ws[0].1, "stud_size"), "2x6");
    // The wall a person typed QS4 shows QS4's data.
    assert_eq!(text(&ws[4].1, "stud_size"), "3 5/8\" 20 ga");
    assert_eq!(text(&ws[4].1, "fire_rating"), "not rated");
}
