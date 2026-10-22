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

/// A hand-kept .lin file: comments, a definition the reader rejects (a bad element), a name
/// defined twice, blank lines, and the one about to be redefined.
const LIN: &str = ";; Company linetypes, kept by hand\n;; (a second comment)\n\n*BROKEN,Has a bad element\nA,0.5,-0.25,[BAD\n*DASHY,Dashed\nA,0.5,-0.25\n*DASHY,Twice: the reader keeps the first\nA,1,-1\n\n*FENCE,Old fence\nA,1,-0.5\n; a closing note\n";

/// Saving into an existing .lin changes only the redefined linetype's lines; a new one is
/// added at the end. Everything else stays byte for byte.
#[test]
fn saving_into_a_lin_file_keeps_every_other_line() {
    use cadaddy_engine::doc::library::files::parse_lin;
    let before = parse_lin(LIN);
    assert!(!before.items.iter().any(|l| l.name == "BROKEN") && !before.warnings.is_empty(), "the reader skips BROKEN");
    let lin = dir("keep-lin").join("company.lin");
    std::fs::write(&lin, LIN).unwrap();
    let path = lin.display().to_string();
    let mut s = Session::new();
    s.execute("linetype.define", &json!({"name": "FENCE", "description": "New fence", "elements": [{"dash": 0.6}, {"gap": 0.2}], "save": path}))
        .unwrap();
    let want = LIN.replace("*FENCE,Old fence\nA,1,-0.5\n", "*FENCE,New fence\nA,0.6,-0.2\n");
    assert_eq!(std::fs::read_to_string(&lin).unwrap(), want);
    s.execute("linetype.define", &json!({"name": "NEWONE", "elements": [{"dash": 0.3}, {"gap": 0.1}], "save": path})).unwrap();
    let want = format!("{want}*NEWONE\nA,0.3,-0.1\n");
    let text = std::fs::read_to_string(&lin).unwrap();
    assert_eq!(text, want);
    // Read back: the new definitions, and the first DASHY still the one kept.
    let back = parse_lin(&text);
    let get = |n: &str| back.items.iter().find(|l| l.name == n).unwrap_or_else(|| panic!("{n} in {:?}", back.items));
    assert_eq!(get("FENCE").pattern.iter().map(|e| e.length).collect::<Vec<_>>(), [0.6, -0.2]);
    assert_eq!(get("NEWONE").pattern.iter().map(|e| e.length).collect::<Vec<_>>(), [0.3, -0.1]);
    assert_eq!(get("DASHY").description, "Dashed");
}

const PAT: &str = ";; Office patterns\n*BADPAT,Rejected: no spacing\n0, 0, 0, 0, 0\n\n*GRID,Grid\n0, 0, 0, 0, 1\n90, 0, 0, 0, 1\n*GRID,Twice\n45, 0, 0, 0, 2\n\n*BOND,Old bond\n0, 0, 0, 0, 2\n; the end\n";

/// The same for a .pat file: only the redefined pattern's lines change.
#[test]
fn saving_into_a_pat_file_keeps_every_other_line() {
    use cadaddy_engine::doc::library::files::parse_pat;
    let before = parse_pat(PAT);
    assert!(!before.items.iter().any(|p| p.name == "BADPAT") && !before.warnings.is_empty(), "the reader skips BADPAT");
    let pat = dir("keep-pat").join("office.pat");
    std::fs::write(&pat, PAT).unwrap();
    let path = pat.display().to_string();
    let mut s = Session::new();
    s.execute("hatch.pattern.define", &json!({"name": "Bond", "description": "New bond", "lines": [{"angle": 0, "dy": 3}], "save": path})).unwrap();
    let want = PAT.replace("*BOND,Old bond\n0, 0, 0, 0, 2\n", "*BOND,New bond\n0, 0, 0, 0, 3\n");
    assert_eq!(std::fs::read_to_string(&pat).unwrap(), want);
    s.execute("hatch.pattern.define", &json!({"name": "Stripe", "lines": [{"angle": 45, "dy": 2}], "save": path})).unwrap();
    let want = format!("{want}*STRIPE\n45, 0, 0, 0, 2\n");
    let text = std::fs::read_to_string(&pat).unwrap();
    assert_eq!(text, want);
    let back = parse_pat(&text);
    let get = |n: &str| back.items.iter().find(|p| p.name == n).unwrap_or_else(|| panic!("{n} in {:?}", back.items));
    assert_eq!(get("BOND").lines[0].delta.1, 3.0);
    assert_eq!(get("STRIPE").lines[0].angle, 45.0);
    assert_eq!(get("GRID").lines.len(), 2, "the first GRID still the one kept");
}

/// A target that is there but can't be read (a folder) or isn't text is refused: nothing is
/// written, and the drawing (or the patterns offered) stay as they were.
#[test]
fn a_target_that_cant_be_read_or_isnt_text_is_refused() {
    let d = dir("refuse");
    let folder = d.join("looks-like.lin");
    std::fs::create_dir_all(&folder).unwrap();
    let binary = d.join("binary.pat");
    let bytes = [0xff, 0xfe, 0x00, b'*', 0x80, b'\n'];
    std::fs::write(&binary, bytes).unwrap();
    let binary_lin = d.join("binary.lin");
    std::fs::write(&binary_lin, bytes).unwrap();
    let mut s = Session::new();
    for (target, says) in [(&folder, "can't be read"), (&binary_lin, "isn't a text file")] {
        let e = s
            .execute("linetype.define", &json!({"name": "NOPE", "elements": [{"dash": 1}, {"gap": 0.5}], "save": target.display().to_string()}))
            .unwrap_err()
            .to_string();
        assert!(e.contains(says) && e.contains("isn't written over"), "{e}");
        assert!(s.doc().unwrap().linetype("NOPE").is_none(), "the drawing as it was");
    }
    let folder_pat = d.join("looks-like.pat");
    std::fs::create_dir_all(&folder_pat).unwrap();
    for (target, says) in [(&folder_pat, "can't be read"), (&binary, "isn't a text file")] {
        let e = s
            .execute("hatch.pattern.define", &json!({"name": "NOPAT", "lines": [{"dy": 1}], "save": target.display().to_string()}))
            .unwrap_err()
            .to_string();
        assert!(e.contains(says) && e.contains("isn't written over"), "{e}");
    }
    let list = s.execute("hatch.patterns", &json!({})).unwrap();
    assert!(!list["user"].as_array().unwrap().iter().any(|p| p["name"] == "NOPAT"), "{list}");
    assert_eq!(std::fs::read(&binary).unwrap(), bytes);
    assert_eq!(std::fs::read(&binary_lin).unwrap(), bytes);
    assert!(folder.is_dir() && folder_pat.is_dir());
}
