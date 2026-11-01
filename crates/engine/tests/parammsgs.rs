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
