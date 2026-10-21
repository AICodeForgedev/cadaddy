//! Frames around images and clipped external references: IMAGEFRAME, XCLIPFRAME, and FRAME for
//! both (0 hidden, 1 shown and plotted, 2 shown but not plotted).

use serde_json::{Value, json};

use super::ask::{Kind, ask_or, machine};
use super::{CommandSpec, bad};
use crate::{Result, Session};

/// The frame variables FRAME sets, with their defaults.
const FRAMES: [(&str, i64); 2] = [("IMAGEFRAME", 1), ("XCLIPFRAME", 2)];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("imageframe", "Frame", |s, p| set(s, p, "imageframe", &["IMAGEFRAME"]))
            .menu(&["Modify", "Object", "Image", "Frame"])
            .params("{value?: 0 (hidden) | 1 (shown and plotted) | 2 (shown, not plotted)} → {value}: the frame around images")
            .interactive(|_| {
                Ok(machine("imageframe", "IMAGEFRAME", [ask_or("value", "Enter image frame setting (0 hidden, 1 shown and plotted, 2 shown, not plotted)", Kind::Number, |s| {
                    current(s, &["IMAGEFRAME"])
                })]))
            }),
        CommandSpec::new("xclipframe", "Frame", |s, p| set(s, p, "xclipframe", &["XCLIPFRAME"]))
            .menu(&["Modify", "Object", "External Reference", "Frame"])
            .params("{value?: 0 (hidden) | 1 (shown and plotted) | 2 (shown, not plotted)} → {value}: the boundary of clipped external references and blocks")
            .interactive(|_| {
                Ok(machine("xclipframe", "XCLIPFRAME", [ask_or("value", "Enter XCLIPFRAME setting (0 hidden, 1 shown and plotted, 2 shown, not plotted)", Kind::Number, |s| {
                    current(s, &["XCLIPFRAME"])
                })]))
            }),
        CommandSpec::new("frame", "Frame", |s, p| set(s, p, "frame", &["IMAGEFRAME", "XCLIPFRAME"]))
            .params("{value?: 0 | 1 | 2} → {value} (3: the frames are set differently): image and clipped external reference frames at once")
            .interactive(|_| {
                Ok(machine("frame", "FRAME", [ask_or("value", "Enter FRAME setting (0 hidden, 1 shown and plotted, 2 shown, not plotted)", Kind::Number, |s| {
                    current(s, &["IMAGEFRAME", "XCLIPFRAME"])
                })]))
            }),
    ]
}

fn value_of(s: &Session, name: &str) -> i64 {
    let dflt = FRAMES.iter().find(|(n, _)| *n == name).map_or(1, |(_, v)| *v);
    s.doc().map(|d| d.header.i64(name, dflt)).unwrap_or(dflt)
}

/// The current setting of `vars`: their value, or 3 when they differ.
fn current_value(s: &Session, vars: &[&str]) -> i64 {
    let vals: Vec<i64> = vars.iter().map(|v| value_of(s, v)).collect();
    match vals.split_first() {
        Some((first, rest)) if rest.iter().all(|v| v == first) => *first,
        Some(_) => 3,
        None => 0,
    }
}

fn current(s: &Session, vars: &[&str]) -> Option<String> {
    Some(current_value(s, vars).to_string())
}

fn set(s: &mut Session, p: &Value, cmd: &str, vars: &[&str]) -> Result<Value> {
    let Some(v) = p.get("value") else { return Ok(json!({ "value": current_value(s, vars) })) };
    let v =
        v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)).or_else(|| v.as_str().and_then(|t| t.trim().parse().ok()));
    let v = v.filter(|v| (0..=2).contains(v)).ok_or_else(|| bad(cmd, "the setting is 0, 1 or 2"))?;
    let d = s.doc_mut()?;
    for name in vars {
        d.header.set_i64(name, v);
    }
    Ok(json!({ "value": v }))
}
