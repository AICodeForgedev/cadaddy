//! The shared symbol table name rule: every create and rename command against the same names.

use super::{is_valid, name_problem};
use crate::Session;
use serde_json::{Value, json};

fn bad_names() -> Vec<(String, &'static str)> {
    vec![
        (String::new(), "empty"),
        ("   ".into(), "empty"),
        ("a<b>c".into(), "`<`"),
        ("x|y".into(), "`|`"),
        ("q*?".into(), "`*`"),
        ("semi;colon".into(), "`;`"),
        ("quo\"te".into(), "`\"`"),
        ("x".repeat(300), "longer"),
        ("line\nbreak".into(), "control"),
        ("tab\there".into(), "control"),
    ]
}

fn session() -> Session {
    let mut s = Session::new();
    s.execute("new", &json!({})).unwrap();
    s
}

fn handle(s: &mut Session, cmd: &str, p: Value) -> String {
    s.execute(cmd, &p).unwrap();
    s.doc().unwrap().model.iter().last().unwrap().handle.hex()
}

/// Run `cmd` with `name` set up as the CREATE or RENAME the table-driven test names.
fn attempt(s: &mut Session, cmd: &str, name: &str) -> Result<Value, String> {
    let r = match cmd {
        "layer.new" | "style" | "dimstyle" | "mleaderstyle" | "tablestyle" | "mlstyle" | "layerstate.save" | "layout.new" => {
            s.execute(cmd, &json!({ "name": name }))
        }
        "block" => {
            let h = handle(s, "circle", json!({"center": [0, 0], "radius": 1}));
            s.execute("block", &json!({"name": name, "base": [0, 0], "handles": [h], "keep": "retain"}))
        }
        "group" => {
            let h = handle(s, "line", json!({"points": [[0, 0], [1, 0]]}));
            s.execute("group", &json!({"name": name, "handles": [h]}))
        }
        "ucs.save" => s.execute("ucs", &json!({"option": "save", "name": name})),
        "view.save" => s.execute("view", &json!({"option": "save", "name": name})),
        "linetype.define" => s.execute("linetype.define", &json!({"name": name, "elements": [{"dash": 1.0}, {"gap": 1.0}]})),
        "layout.rename" => {
            s.execute("layout.new", &json!({"name": "LSRC"})).unwrap();
            s.execute("layout.rename", &json!({"from": "LSRC", "to": name}))
        }
        "mlstyle.rename" => {
            s.execute("mlstyle", &json!({"name": "MSRC"})).unwrap();
            s.execute("mlstyle", &json!({"name": "MSRC", "rename": name}))
        }
        c if c.starts_with("rename:") => {
            let t = &c["rename:".len()..];
            match t {
                "layer" => drop(s.execute("layer.new", &json!({"name": "SRC"})).unwrap()),
                "block" => {
                    let h = handle(s, "circle", json!({"center": [0, 0], "radius": 1}));
                    s.execute("block", &json!({"name": "SRC", "base": [0, 0], "handles": [h]})).unwrap();
                }
                "linetype" => drop(s.execute("linetype.define", &json!({"name": "SRC", "elements": [{"dash": 1.0}, {"gap": 1.0}]})).unwrap()),
                _ => drop(s.execute(t, &json!({"name": "SRC"})).unwrap()),
            }
            s.execute("rename", &json!({"table": t, "from": "SRC", "to": name}))
        }
        _ => panic!("unknown command {cmd}"),
    };
    r.map_err(|e| e.to_string())
}

const COMMANDS: &[&str] = &[
    "layer.new",
    "layerstate.save",
    "style",
    "dimstyle",
    "mleaderstyle",
    "tablestyle",
    "mlstyle",
    "linetype.define",
    "block",
    "group",
    "ucs.save",
    "view.save",
    "layout.new",
    "layout.rename",
    "mlstyle.rename",
    "rename:layer",
    "rename:style",
    "rename:dimstyle",
    "rename:mleaderstyle",
    "rename:tablestyle",
    "rename:linetype",
    "rename:block",
    "rename:ucs",
    "rename:view",
];

#[test]
fn every_create_and_rename_refuses_the_bad_names_and_says_why() {
    for cmd in COMMANDS {
        for (name, why) in bad_names() {
            let mut s = session();
            if matches!(*cmd, "rename:ucs" | "rename:view") {
                let opt = cmd.trim_start_matches("rename:");
                let (c, o) = if opt == "ucs" { ("ucs", "save") } else { ("view", "save") };
                s.execute(c, &json!({"option": o, "name": "SRC"})).unwrap();
                let e = s.execute("rename", &json!({"table": opt, "from": "SRC", "to": name})).unwrap_err().to_string();
                assert!(e.contains(why) || e.contains("invalid"), "{cmd} {name:?}: {e}");
                continue;
            }
            let e = attempt(&mut s, cmd, &name).expect_err(&format!("{cmd} accepted {name:?}"));
            assert!(e.contains(why), "{cmd} {name:?}: the refusal should name `{why}`: {e}");
        }
    }
}

#[test]
fn good_names_are_accepted_everywhere() {
    for cmd in ["layer.new", "style", "dimstyle", "mleaderstyle", "tablestyle", "linetype.define", "block", "group", "layout.new"] {
        for name in ["Äö漢字", "trail ", "Plan A-1"] {
            let mut s = session();
            attempt(&mut s, cmd, name).unwrap_or_else(|e| panic!("{cmd} refused {name:?}: {e}"));
        }
    }
}

#[test]
fn the_rule_itself() {
    assert!(is_valid("Plan A-1"));
    assert!(name_problem("").is_some() && name_problem(" \t").is_some());
    assert!(name_problem(&"x".repeat(255)).is_none() && name_problem(&"x".repeat(256)).is_some());
    for c in "<>/\\\":;?*|,=`".chars() {
        assert!(name_problem(&format!("a{c}b")).unwrap().contains(c), "{c}");
    }
    assert!(name_problem("a\u{7}b").is_some());
}

#[test]
fn a_refused_name_leaves_no_entry_behind() {
    let mut s = session();
    let before = {
        let d = s.doc().unwrap();
        (d.text_styles.len(), d.dim_styles.len(), d.mleader_styles.len(), d.table_styles.len(), d.mline_styles.len())
    };
    for c in ["style", "dimstyle", "mleaderstyle", "tablestyle", "mlstyle"] {
        for (name, _) in bad_names() {
            assert!(s.execute(c, &json!({"name": name})).is_err(), "{c} {name:?}");
        }
    }
    let d = s.doc().unwrap();
    assert_eq!(before, (d.text_styles.len(), d.dim_styles.len(), d.mleader_styles.len(), d.table_styles.len(), d.mline_styles.len()));
}
