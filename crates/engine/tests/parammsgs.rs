//! Wrong-typed or out-of-range parameters name the right problem (battle test 5, B7-019/020/021):
//! `documented_params` reads only the parameter part of a doc, an agent's wrong-typed value is
//! refused where the doc says plainly what the value is, DGN commands name themselves, and a
//! typed number beyond ±1e12 says so.

use cadaddy_engine::{ParamKind, Session, command_specs, documented_params, find_command, param_kind, params_doc, unknown_params};
use serde_json::{Value, json};

#[test]
fn documented_params_skips_results_and_type_words() {
    let line = documented_params(find_command("line").unwrap().params);
    assert_eq!(line, ["points", "closed"]);
    let doc = "{tolerance_in?: inches (0), same_trade?: bool} → {clashes, total, truncated, gone?}";
    assert_eq!(documented_params(doc), ["tolerance_in", "same_trade"]);
    // Forms listed after a result still count.
    let cal = find_command("cal").unwrap();
    let known = documented_params(cal.params);
    for k in ["expr", "convert", "pick", "last", "units"] {
        assert!(known.contains(&k), "{k} in {known:?}");
    }
    let clashes = find_command("model.clashes").unwrap();
    for k in ["total", "truncated", "below_ceiling", "inches"] {
        assert!(unknown_params(clashes, &json!({ k: 1 })).contains(&k.to_string()), "`{k}` is not a parameter of model.clashes");
    }
    assert!(unknown_params(clashes, &json!({ "tolerance_in": 1, "max_clashes": 5 })).is_empty());
    assert!(unknown_params(find_command("line").unwrap(), &json!({ "bool": true })).contains(&"bool".to_string()));
}

#[test]
fn param_kinds_come_from_the_doc() {
    let doc = "{radius?: number, closed?: bool, name?: string, page?: 1, at?: [x,y], fade?: 0-80, vars?: {x: 1}, v?: bool | {a}}";
    assert_eq!(param_kind(doc, "radius"), Some(ParamKind::Number));
    assert_eq!(param_kind(doc, "closed"), Some(ParamKind::Bool));
    assert_eq!(param_kind(doc, "name"), Some(ParamKind::Text));
    assert_eq!(param_kind(doc, "page"), Some(ParamKind::Single));
    assert_eq!(param_kind(doc, "fade"), Some(ParamKind::Single));
    assert_eq!(param_kind(doc, "at"), None);
    assert_eq!(param_kind(doc, "vars"), None);
    assert_eq!(param_kind(doc, "v"), None);
}

#[test]
fn an_agent_gets_the_wrong_type_named() {
    let mut s = Session::new();
    let e = s.execute_agent("circle", &json!({ "center": [0, 0], "radius": "5" })).unwrap_err().to_string();
    assert!(e.contains("`radius` must be a number"), "{e}");
    let e = s.execute_agent("dgnadjust", &json!({ "fade": {"a": [1]} })).unwrap_err().to_string();
    assert!(e.contains("`fade` must be"), "{e}");
    assert!(s.doc().unwrap().model.iter().next().is_none());
}

#[test]
fn every_plainly_typed_parameter_refuses_a_wrong_value() {
    // The guard over the whole command table: for each documented parameter whose kind the doc
    // gives, an object is refused by name before the command runs.
    let (mut keys, mut commands, mut refused) = (0, 0, 0);
    let mut s = Session::new();
    for spec in command_specs() {
        let doc = params_doc(spec);
        let plain: Vec<&str> = documented_params(doc).into_iter().filter(|k| param_kind(doc, k).is_some()).collect();
        if plain.is_empty() {
            continue;
        }
        commands += 1;
        for k in plain {
            keys += 1;
            let mut p = serde_json::Map::new();
            p.insert(k.to_string(), json!({ "a": [1] }));
            let params = Value::Object(p);
            if let Err(e) = s.execute_agent(spec.id, &params)
                && e.to_string().contains(&format!("`{k}` must be"))
            {
                refused += 1;
                continue;
            }
            panic!("{} accepted {k} as an object", spec.id);
        }
    }
    println!("COUNTS commands_with_plain_params={commands} keys={keys} refused={refused}");
    assert_eq!(keys, refused);
}

#[test]
fn dgn_commands_name_themselves() {
    let mut s = Session::new();
    for (id, other) in [("dgnadjust", "pdfadjust"), ("dgnclip", "pdfclip"), ("dgnlayers", "pdflayers")] {
        let e = s.execute(id, &json!({})).unwrap_err().to_string();
        assert!(e.contains(id) && !e.contains(other), "{id}: {e}");
    }
}

#[test]
fn typed_values_beyond_the_limit_say_so() {
    for line in ["circle 0,0 1e13", "line 0,0 1e13,0", "line 0,0 2e12,0"] {
        let mut s = Session::new();
        let mut last = String::new();
        for t in line.split(' ') {
            let _ = s.cmdline(t);
            last = s.log.last().cloned().unwrap_or_default();
        }
        assert!(last.contains("too large") && last.contains("1e12"), "{line}: {last}");
    }
    let mut s = Session::new();
    for t in ["circle", "0,0", "5"] {
        s.cmdline(t).unwrap();
    }
    assert_eq!(s.doc().unwrap().model.iter().count(), 1, "a normal radius still draws");
}

#[test]
fn text_and_mleaderstyle_name_the_wrong_type() {
    let mut s = Session::new();
    for (cmd, params, key) in
        [("text", json!({ "at": [0, 0], "text": {"a": [1]} }), "text"), ("mleaderstyle", json!({ "arrowSize": {} }), "arrowSize")]
    {
        let e = s.execute_agent(cmd, &params).unwrap_err().to_string();
        assert!(e.contains(&format!("`{key}` must be")), "{cmd}: {e}");
    }
}

// ---- parammsgs3 (battle test 6, B7-019 / B7-020) ----

#[test]
fn the_doc_reader_skips_choice_values_prose_and_defaults() {
    // purge's `layers` is a value of `type`, not a parameter.
    let purge = find_command("purge").unwrap();
    let wrong = "lay".to_string() + "ers";
    assert!(unknown_params(purge, &json!({ wrong.as_str(): 12345 })).contains(&wrong));
    assert!(unknown_params(purge, &json!({ "type": "layers" })).is_empty());
    // texts: `TEXT, MTEXT` in the prose after the result are not parameters.
    let texts = find_command("texts").unwrap();
    assert!(unknown_params(texts, &json!({ "MTEXT": 1 })).contains(&"MTEXT".to_string()));
    assert!(unknown_params(texts, &json!({ "contains": "a", "limit": 3 })).is_empty());
    // model.clashes: the default's class names are not parameters.
    let clashes = find_command("model.clashes").unwrap();
    let known = documented_params(clashes.params);
    for k in ["Zone", "SpatialZone", "Annotation", "Grid", "Site"] {
        assert!(!known.contains(&k), "{k} in {known:?}");
    }
    assert!(known.contains(&"ignore_classes") && known.contains(&"max_clashes"));
    // A parameter listed after `|` with its own `?` still counts.
    assert_eq!(documented_params("{object?: closed object | points?: [[x,y]]}"), ["object", "points"]);
    let plain = documented_params("{type?: all | blocks | layers, names?: x}");
    assert!(plain.contains(&"type") && plain.contains(&"names") && !plain.contains(&"layers") && !plain.contains(&"blocks"), "{plain:?}");
}

#[test]
fn mleaderstyle_refuses_numbers_it_cannot_use() {
    let mut s = Session::new();
    for (k, v) in [("arrowSize", json!(-5)), ("landingGap", json!(-1)), ("dogleg", json!(-0.5)), ("textHeight", json!(0)), ("textHeight", json!(-2))]
    {
        let e = s.execute("mleaderstyle", &json!({ "name": "Plan A", k: v })).unwrap_err().to_string();
        assert!(e.contains(&format!("`{k}` must be")), "{k}: {e}");
    }
    let e = s.execute("mleaderstyle", &json!({ "name": "Plan A", "arrowSize": "big" })).unwrap_err().to_string();
    assert!(e.contains("`arrowSize` must be a number"), "{e}");
    // Nothing was created by the refused calls.
    assert!(s.doc().unwrap().mleader_styles.iter().all(|st| st.name != "Plan A"));
    s.execute("mleaderstyle", &json!({ "name": "Plan A", "arrowSize": 0.25, "textHeight": 0.1 })).unwrap();
    assert!(s.doc().unwrap().mleader_styles.iter().any(|st| st.name == "Plan A" && (st.arrow_size - 0.25).abs() < 1e-9));
}

#[test]
fn an_agent_gets_non_finite_text_and_negative_sizes_refused() {
    let mut s = Session::new();
    for text in ["nan", "NaN", "inf", "-Infinity", " +inf "] {
        let e = s.execute_agent("circle", &json!({ "center": [0, 0], "radius": text })).unwrap_err().to_string();
        assert!(e.contains("`radius` must be"), "{text}: {e}");
        let e = s.execute_agent("mleaderstyle", &json!({ "name": "S", "textHeight": text })).unwrap_err().to_string();
        assert!(e.contains("`textHeight` must be"), "{text}: {e}");
    }
    let e = s.execute_agent("mleaderstyle", &json!({ "name": "S", "arrowSize": -5 })).unwrap_err().to_string();
    assert!(e.contains("`arrowSize` must be 0 or more"), "{e}");
    let wrong = "lay".to_string() + "ers";
    let e = s.execute_agent("purge", &json!({ wrong.as_str(): 12345 })).unwrap_err().to_string();
    assert!(e.contains("`layers` is not a parameter of purge"), "{e}");
    assert!(s.doc().unwrap().model.iter().next().is_none());
}

#[test]
fn undohistory_says_what_is_wrong_with_the_step() {
    let mut s = Session::new();
    let e = s.execute("undohistory", &json!({ "action": "to", "step": -1 })).unwrap_err().to_string();
    assert!(e.contains("`step` must be a whole number, 0 or more") && !e.contains("is required"), "{e}");
    let e = s.execute("undohistory", &json!({ "action": "to" })).unwrap_err().to_string();
    assert!(e.contains("`step` is required"), "{e}");
}

#[test]
fn silent_accepts_of_wrong_values_in_the_command_table() {
    // For every documented parameter with a numeric name: 'nan' / 'inf' text, and -1 where the
    // name can't be negative (a size, count, radius…), must not run. Counts for the report.
    let numeric = [
        "radius",
        "dist",
        "count",
        "scale",
        "angle",
        "height",
        "width",
        "length",
        "size",
        "offset",
        "rows",
        "columns",
        "spacing",
        "factor",
        "tolerance",
        "thickness",
        "gap",
        "depth",
        "diameter",
        "segments",
        "step",
        "dogleg",
    ];
    let non_negative = ["radius", "diameter", "height", "width", "size", "count", "segments", "gap", "dogleg", "tolerance", "limit"];
    let (mut tried, mut accepted) = (0, Vec::new());
    for spec in command_specs() {
        let doc = params_doc(spec);
        for k in documented_params(doc) {
            let lk = k.to_ascii_lowercase();
            if param_kind(doc, k) == Some(ParamKind::Text) || !numeric.iter().any(|w| lk.contains(w)) {
                continue;
            }
            let mut values = vec![json!("nan"), json!("-inf")];
            if non_negative.iter().any(|w| lk.contains(w)) {
                values.push(json!(-1));
            }
            for v in values {
                tried += 1;
                let mut s = Session::new();
                if s.execute_agent(spec.id, &json!({ k: v })).is_ok() {
                    accepted.push(format!("{} {k}={v}", spec.id));
                }
            }
        }
    }
    println!("COUNTS non_finite_or_negative_tried={tried} accepted={}", accepted.len());
    assert!(accepted.is_empty(), "{accepted:?}");
}
