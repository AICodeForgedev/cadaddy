//! The linetype and hatch pattern editors' commands (Q152): LINETYPE.DEFINE and
//! HATCH.PATTERN.DEFINE make definitions in the drawing and save them into user `.lin` / `.pat`
//! files; `.lin` shape elements load from CADaddy's own shape file.

use cadaddy_engine::Session;
use serde_json::json;

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-ltdefine-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_linetype_defined_with_a_shape_is_drawn_from_cadaddy_shx_and_saved() {
    let mut s = Session::new();
    let lin = dir("lin").join("mine.lin");
    let path = lin.display().to_string();
    let r = s
        .execute(
            "linetype.define",
            &json!({"name": "Fence_Box", "description": "Fence with boxes", "elements": [{"dash": 0.5}, {"gap": 0.1}, {"shape": "box", "scale": 0.1}, {"gap": 0.1}], "save": path, "current": true}),
        )
        .unwrap();
    assert_eq!(r["warnings"], json!([]));
    let d = s.doc().unwrap();
    let lt = d.linetype("Fence_Box").unwrap();
    assert_eq!(lt.pattern.len(), 3);
    assert_eq!((lt.pattern[1].shape, lt.pattern[1].shape_name.as_deref(), lt.pattern[1].length), (Some(1), Some("BOX"), -0.1));
    assert_eq!(d.header.str("CELTYPE", ""), "Fence_Box");
    let text = std::fs::read_to_string(&lin).unwrap();
    assert!(text.contains("*Fence_Box,Fence with boxes") && text.contains("A,0.5,-0.1,[BOX,cadaddy.shx,S=0.1],-0.1"), "{text}");
    // A second linetype joins the file; the first, redefined, is replaced, not repeated.
    s.execute(
        "linetype.define",
        &json!({"name": "Gas2", "elements": [{"dash": 0.6}, {"gap": 0.05}, {"text": "GAS", "x": -0.1, "y": -0.05}, {"gap": 0.35}], "save": path}),
    )
    .unwrap();
    s.execute(
        "linetype.define",
        &json!({"name": "Fence_Box", "elements": [{"dash": 0.25}, {"gap": 0.1}, {"shape": "CIRC", "scale": 0.1}, {"gap": 0.1}], "save": path}),
    )
    .unwrap();
    let back = cadaddy_engine::doc::library::files::parse_lin(&std::fs::read_to_string(&lin).unwrap());
    assert_eq!(back.items.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Fence_Box", "Gas2"]);
    assert_eq!(back.items[0].pattern[1].shape_name.as_deref(), Some("CIRC"));
    assert_eq!(s.doc().unwrap().linetype("Fence_Box").unwrap().pattern[1].shape, Some(2));
    // Loading that file into another drawing gets the same shapes.
    let mut t = Session::new();
    let r = t.execute("linetype", &json!({"load": "*", "file": path})).unwrap();
    assert_eq!(r["warnings"], json!([]), "{r}");
    assert_eq!(t.doc().unwrap().linetype("FENCE_BOX").unwrap().pattern[1].shape, Some(2));
}

#[test]
fn a_lin_file_naming_a_missing_shape_file_loads_with_a_warning() {
    let mut s = Session::new();
    let r = s
        .execute(
            "linetype",
            &json!({"load": "*", "text": "*ZAPS,zap\nA,.5,-.1,[ZAP,no-such-shapes.shx,S=.1],-.1\n*TIES\nA,.15,[TRACK,cadaddy.shx,S=.25]\n"}),
        )
        .unwrap();
    let w = r["warnings"].as_array().unwrap();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].as_str().unwrap().contains("ZAP") && w[0].as_str().unwrap().contains("no-such-shapes.shx"));
    let d = s.doc().unwrap();
    let zap = d.linetype("ZAPS").unwrap();
    assert!(zap.pattern[1].shape.is_none() && zap.pattern[1].shape_name.as_deref() == Some("ZAP"), "kept by name: {:?}", zap.pattern);
    assert_eq!(d.linetype("TIES").unwrap().pattern[0].shape, Some(3));
}

#[test]
fn a_hatch_pattern_defined_is_offered_to_hatch_and_saved() {
    let mut s = Session::new();
    let pat = dir("pat").join("mine.pat");
    let path = pat.display().to_string();
    let r = s
        .execute(
            "hatch.pattern.define",
            &json!({"name": "Bond_A", "description": "Running bond", "lines": [{"angle": 0, "dy": 3}, {"angle": 90, "dx": 3, "dy": 4, "dashes": [3, -3]}], "save": path}),
        )
        .unwrap();
    assert_eq!(r, json!({"name": "BOND_A", "families": 2}));
    let back = cadaddy_engine::doc::library::files::parse_pat(&std::fs::read_to_string(&pat).unwrap());
    assert_eq!(back.items.len(), 1);
    assert_eq!(back.items[0].lines[1].dashes, [3.0, -3.0]);
    let list = s.execute("hatch.patterns", &json!({})).unwrap();
    assert!(list["user"].as_array().unwrap().iter().any(|p| p["name"] == "BOND_A"), "{list}");
    s.execute("rectang", &json!({"p1": [0, 0], "p2": [40, 30]})).unwrap();
    let h = s.execute("hatch", &json!({"points": [[10, 10]], "pattern": "BOND_A", "scale": 1})).unwrap();
    assert!(h.is_object(), "{h}");
    // Refused: a built-in's name, no spacing, junk.
    assert!(s.execute("hatch.pattern.define", &json!({"name": "ANSI31", "lines": [{"dy": 1}]})).is_err());
    assert!(s.execute("hatch.pattern.define", &json!({"name": "FLAT", "lines": [{"dy": 0}]})).is_err());
    assert!(s.execute("hatch.pattern.define", &json!({"name": "X", "lines": [{"dy": "a"}]})).is_err());
}

#[test]
fn bad_definitions_are_refused() {
    let mut s = Session::new();
    for p in [
        json!({}),
        json!({"name": "A", "elements": []}),
        json!({"name": "A", "elements": [{"bogus": 1}]}),
        json!({"name": "A", "elements": [{"dash": 1}, {"text": "a\"b"}]}),
        json!({"name": "A", "elements": [{"dash": 1}, {"shape": "no space"}]}),
        json!({"name": "A", "elements": [{"dash": 1}, {"shape": "BOX", "scale": -1}]}),
        json!({"name": "ByLayer", "elements": [{"dash": 1}]}),
        json!({"name": "A,B", "elements": [{"dash": 1}]}),
        json!({"name": "A", "elements": [{"dash": 1}], "save": "/nonexistent-dir/x.lin"}),
    ] {
        assert!(s.execute("linetype.define", &p).is_err(), "{p}");
    }
    assert!(s.doc().unwrap().linetype("A").is_none(), "a failed save leaves the drawing as it was");
}
