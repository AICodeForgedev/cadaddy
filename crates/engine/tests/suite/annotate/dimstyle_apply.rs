//! -DIMSTYLE Apply (battle test 0.7.8 #4): the selected dimensions take the current dimension
//! settings, the variables set apart from the style (SETVAR DIMSCALE 48, DIMLUNIT 4) included,
//! with the count of what was found and updated; typed with ALL, a window or picks.

use cadaddy_doc::EntityKind;
use cadaddy_engine::Session;
use serde_json::json;

fn dims(s: &mut Session, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let y = i as f64 * 10.0;
            s.execute("dimlinear", &json!({ "p1": [0, y], "p2": [24, y], "at": [12, y + 3.0] })).unwrap()["handle"].as_str().unwrap().to_string()
        })
        .collect()
}

/// A dimension's DIMSCALE and DIMLUNIT as drawn (its style with its overrides).
fn settings(s: &Session, h: &str) -> (f64, i64) {
    let d = s.doc().unwrap();
    let e = d.entity(cadaddy_doc::Handle::parse_hex(h).unwrap()).unwrap();
    let EntityKind::Dimension(dm) = &e.kind else { panic!("not a dimension") };
    let st = d.dim_style(&dm.style).cloned().unwrap_or_default().with_overrides(&dm.overrides);
    let v = serde_json::to_value(&st).unwrap();
    (v["scale"].as_f64().unwrap_or(f64::NAN), v["linearUnits"].as_i64().or_else(|| v["lunit"].as_i64()).unwrap_or(-1))
}

#[test]
fn apply_gives_the_selected_dimensions_the_current_settings() {
    for how in ["all", "window", "remove"] {
        let mut s = Session::new();
        let hs = dims(&mut s, 4);
        let before = settings(&s, &hs[0]);
        s.execute("setvar", &json!({ "name": "DIMSCALE", "value": 48 })).unwrap();
        s.execute("setvar", &json!({ "name": "DIMLUNIT", "value": 4 })).unwrap();
        // Untouched until applied.
        assert_eq!(settings(&s, &hs[0]), before);
        s.cmdline("-dimstyle").unwrap();
        s.cmdline("a").unwrap();
        match how {
            "all" => s.cmdline("all").unwrap(),
            "window" => {
                s.cmdline("c").unwrap();
                s.cmdline("-5,-5").unwrap();
                s.cmdline("40,40").unwrap();
            }
            // All, then the first one taken out again with Remove and a crossing window.
            _ => {
                s.cmdline("all").unwrap();
                s.cmdline("r").unwrap();
                s.cmdline("c").unwrap();
                s.cmdline("-5,-5").unwrap();
                s.cmdline("30,4").unwrap();
            }
        }
        s.cmdline("").unwrap();
        assert!(s.running.is_none(), "{how}: {}", s.prompt_text());
        let log = s.log.join("\n");
        assert!(log.contains("4 found"), "{how}: {log}");
        let n = if how == "remove" { 3 } else { 4 };
        assert!(log.contains(&format!("{n} dimension(s) updated")), "{how}: {log}");
        for (i, h) in hs.iter().enumerate() {
            let (scale, _) = settings(&s, h);
            let want = if how == "remove" && i == 0 { 1.0 } else { 48.0 };
            assert_eq!(scale, want, "{how}: {h}");
        }
        assert_eq!(settings(&s, &hs[0]) == before, how == "remove");
    }
}

#[test]
fn apply_from_the_command_api_and_undo() {
    let mut s = Session::new();
    let hs = dims(&mut s, 2);
    s.execute("setvar", &json!({ "name": "DIMSCALE", "value": 24 })).unwrap();
    let r = s.execute("dimstyle.update", &json!({ "handles": [hs[0]] })).unwrap();
    assert_eq!(r["updated"], 1);
    assert_eq!(settings(&s, &hs[0]).0, 24.0);
    assert_eq!(settings(&s, &hs[1]).0, 1.0, "only the chosen ones");
    // Back to the style's own values: applying clears the overrides again.
    s.execute("setvar", &json!({ "name": "DIMSCALE", "value": 1 })).unwrap();
    s.execute("dimstyle.update", &json!({ "handles": [hs[0]] })).unwrap();
    assert_eq!(settings(&s, &hs[0]).0, 1.0);
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(settings(&s, &hs[0]).0, 24.0);
}
