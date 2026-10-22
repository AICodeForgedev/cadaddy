//! MKLTYPE (Express): a linetype made from drawn objects. A start and an end point mark one
//! repeat of the pattern along a line; lines lying on it become dashes, points dots, and text
//! and shapes are carried at their place along it (with their height, rotation and distance
//! from the line). The definition is written to a `.lin` file (replacing one of the same name)
//! and loaded into the drawing.

use std::fmt::Write as _;

use cadaddy_doc::{DashElement, Drawing, Entity, EntityKind, Linetype};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase};
use super::super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// Most objects one linetype is made from.
const MAX_OBJECTS: usize = 1000;
/// The longest .lin file read to replace a definition in it.
const MAX_LIN_FILE: u64 = 8 << 20;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mkltype", "Make Linetype", run_mkltype)
            .params(
                "{file: path of the .lin file (created, or the definition added; one of the same name is replaced), name, \
                 description?, start: [x,y], end: [x,y] (one repeat of the pattern), handles? (lines on the start–end line: dashes; \
                 points: dots; text and shapes: carried along)} → {name, file, pattern}",
            )
            .interactive(|_| Ok(Box::new(MkltypeM::default()))),
    ]
}

/// Something along the pattern: a dash between two distances, a dot, or a text or shape
/// element anchored at a distance.
enum Item {
    Dash(f64, f64),
    Dot(f64),
    Anchor(f64, DashElement),
}

/// The shape file of the drawing holding shape `name`, and its number.
fn shape_file(d: &Drawing, name: &str) -> Option<(String, u16)> {
    d.shape_files.iter().find_map(|f| {
        let font = cadaddy_fonts::shx::find(f)?;
        let k = u16::try_from(font.code_by_name(name)?).ok()?;
        Some((f.clone(), k))
    })
}

/// The pattern items of `ents` along `a`→`b` (length `len`, direction `u`); objects off the
/// line are reported in `notes`.
fn items(d: &Drawing, ents: &[&Entity], a: Vec2, u: Vec2, len: f64, notes: &mut Vec<String>) -> Vec<Item> {
    let along = |p: Vec2| (p - a).dot(u);
    let across = |p: Vec2| (p - a).dot(u.perp());
    let tol = len * 1e-6 + 1e-9;
    let on = |t: f64| (-tol..=len + tol).contains(&t);
    let axis = u.angle();
    let mut out = Vec::new();
    for e in ents {
        match &e.kind {
            EntityKind::Line(l) => {
                let (p, q) = (l.a.xy(), l.b.xy());
                if across(p).abs() > tol * 1e3 || across(q).abs() > tol * 1e3 {
                    notes.push(format!("line {} is not on the linetype's line; left out", e.handle.hex()));
                    continue;
                }
                let (t0, t1) = (along(p).min(along(q)).max(0.0), along(p).max(along(q)).min(len));
                if t1 > t0 + tol {
                    out.push(Item::Dash(t0, t1));
                } else if on(t0) {
                    out.push(Item::Dot(t0.clamp(0.0, len)));
                }
            }
            EntityKind::Point(p) if on(along(p.p.xy())) => out.push(Item::Dot(along(p.p.xy()).clamp(0.0, len))),
            EntityKind::Text(t) => {
                let p = t.insert.xy();
                if !on(along(p)) {
                    notes.push(format!("text {} is past the ends of the pattern; left out", e.handle.hex()));
                    continue;
                }
                let style = d.text_style(&t.style).map(|s| s.name.clone()).unwrap_or_else(|| "Standard".into());
                let fixed = d.text_style(&style).map(|s| s.height).filter(|h| h.is_finite() && *h > 0.0).unwrap_or(1.0);
                let el = DashElement {
                    text: Some(t.value.chars().filter(|c| !c.is_control() && *c != '"').collect()),
                    style: Some(style),
                    scale: t.height / fixed,
                    rotation: cadaddy_geom::norm_angle(t.rotation - axis),
                    offset: Vec2::new(0.0, across(p)),
                    ..DashElement::dash(0.0)
                };
                out.push(Item::Anchor(along(p).clamp(0.0, len), el));
            }
            EntityKind::Shape(sh) => {
                let p = sh.insert.xy();
                let Some((file, code)) = shape_file(d, &sh.name) else {
                    notes.push(format!("shape {} is not in a loaded shape file; left out", sh.name));
                    continue;
                };
                if !on(along(p)) {
                    notes.push(format!("shape {} is past the ends of the pattern; left out", sh.name));
                    continue;
                }
                let el = DashElement {
                    shape: Some(code),
                    style: Some(file),
                    scale: sh.size,
                    rotation: cadaddy_geom::norm_angle(sh.rotation - axis),
                    offset: Vec2::new(0.0, across(p)),
                    ..DashElement::dash(0.0)
                };
                out.push(Item::Anchor(along(p).clamp(0.0, len), el));
            }
            _ => notes.push(format!("{} {}: only lines, points, text and shapes make a linetype; left out", e.kind.type_name(), e.handle.hex())),
        }
    }
    out
}

/// The pattern from the items: dashes and gaps between break points (dash ends, dots,
/// anchors), each text or shape on the element ending where it is anchored.
fn pattern(items: &[Item], len: f64) -> Vec<DashElement> {
    let mut cuts: Vec<f64> = vec![0.0, len];
    for it in items {
        match it {
            Item::Dash(a, b) => cuts.extend([*a, *b]),
            Item::Dot(t) | Item::Anchor(t, _) => cuts.push(*t),
        }
    }
    cuts.sort_by(f64::total_cmp);
    let eps = len * 1e-9;
    cuts.dedup_by(|a, b| (*a - *b).abs() <= eps);
    let dashed = |m: f64| items.iter().any(|it| matches!(it, Item::Dash(a, b) if *a <= m && m <= *b));
    let mut out: Vec<DashElement> = Vec::new();
    for w in cuts.windows(2) {
        let (Some(&t0), Some(&t1)) = (w.first(), w.get(1)) else { continue };
        // The marks at t0: dots, then anchors on the element just ended.
        for it in items {
            match it {
                Item::Dot(t) if (t - t0).abs() <= eps => out.push(DashElement::dash(0.0)),
                Item::Anchor(t, el) if (t - t0).abs() <= eps => attach(&mut out, el.clone()),
                _ => {}
            }
        }
        let seg = t1 - t0;
        let e = if dashed((t0 + t1) / 2.0) { DashElement::dash(seg) } else { DashElement::dash(-seg) };
        // Merge with the element before when they are the same kind and it carries nothing.
        match out.last_mut() {
            Some(l) if l.text.is_none() && l.shape.is_none() && l.length != 0.0 && (l.length > 0.0) == (e.length > 0.0) => l.length += e.length,
            _ => out.push(e),
        }
    }
    for it in items {
        match it {
            Item::Dot(t) if (t - len).abs() <= eps => out.push(DashElement::dash(0.0)),
            Item::Anchor(t, el) if (t - len).abs() <= eps => attach(&mut out, el.clone()),
            _ => {}
        }
    }
    out
}

/// Put a text or shape on the last element (a zero-length one when there is none or it already
/// carries one), as a `.lin` file does.
fn attach(out: &mut Vec<DashElement>, el: DashElement) {
    if out.last().is_none_or(|e| e.text.is_some() || e.shape.is_some()) {
        out.push(DashElement::dash(0.0));
    }
    if let Some(last) = out.last_mut() {
        *last = DashElement { length: last.length, ..el };
    }
}

/// A number as a `.lin` file writes it.
fn n(v: f64) -> String {
    let t = format!("{v:.6}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    if t == "-0" || t.is_empty() { "0".into() } else { t.to_string() }
}

/// The definition's two lines.
fn lin_text(lt: &Linetype) -> String {
    let mut fields = vec!["A".to_string()];
    for e in &lt.pattern {
        fields.push(n(e.length));
        let opts = format!("S={},R={},X={},Y={}", n(e.scale), n(e.rotation.to_degrees()), n(e.offset.x), n(e.offset.y));
        if let Some(t) = &e.text {
            fields.push(format!("[\"{t}\",{},{opts}]", e.style.as_deref().unwrap_or("Standard")));
        } else if e.shape.is_some()
            && let Some(file) = &e.style
            && let Some(name) =
                cadaddy_fonts::shx::find(file).and_then(|f| f.shape_names().into_iter().find(|(k, _)| Some(*k) == e.shape.map(u32::from)))
        {
            let base = std::path::Path::new(file).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
            fields.push(format!("[{},{base},{opts}]", name.1));
        }
    }
    let mut out = String::new();
    let _ = writeln!(out, "*{},{}", lt.name, lt.description);
    let _ = writeln!(out, "{}", fields.join(","));
    out
}

/// `text` without the definition called `name` (its header and pattern lines).
fn without(text: &str, name: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(h) = t.strip_prefix('*') {
            skipping = h.split(',').next().unwrap_or("").trim().eq_ignore_ascii_case(name);
        } else if skipping && (t.is_empty() || t.starts_with(';')) {
            skipping = false;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn run_mkltype(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "mkltype";
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`name` is required"))?;
    if name.len() > 255
        || name.contains([',', '*', '[', ']', '"'])
        || ["bylayer", "byblock", "continuous"].contains(&name.to_ascii_lowercase().as_str())
    {
        return Err(bad(cmd, format!("`{name}` can't be a linetype name")));
    }
    let description: String = str_param(p, "description").unwrap_or("").chars().filter(|c| !c.is_control()).take(255).collect();
    let file = str_param(p, "file")
        .or_else(|| str_param(p, "path"))
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .ok_or_else(|| bad(cmd, "`file` (the .lin file) is required"))?;
    let file = if std::path::Path::new(file).extension().is_some() { file.to_string() } else { format!("{file}.lin") };
    let (a, b) = (point_req(cmd, p, "start")?, point_req(cmd, p, "end")?);
    let len = a.dist(b);
    if !(len.is_finite() && len > 1e-9) {
        return Err(bad(cmd, "the start and end points must differ"));
    }
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(cmd, "no objects selected"));
    }
    if hs.len() > MAX_OBJECTS {
        return Err(bad(cmd, format!("at most {MAX_OBJECTS} objects")));
    }
    let d = s.doc()?;
    let ents: Vec<&Entity> = hs.iter().filter_map(|h| d.entity(*h)).map(|e| e.as_ref()).collect();
    let mut notes = Vec::new();
    let its = items(d, &ents, a, (b - a) / len, len, &mut notes);
    if !its.iter().any(|i| matches!(i, Item::Dash(..) | Item::Dot(_))) {
        return Err(bad(cmd, "no line or point lies on the linetype's line"));
    }
    let pat = pattern(&its, len);
    let lt = Linetype { name: name.to_string(), description, pattern: pat };
    // The .lin file: the old definition of this name gone, the new one at the end.
    let old = match std::fs::metadata(&file) {
        Ok(m) if m.len() > MAX_LIN_FILE => return Err(bad(cmd, format!("{file} is larger than {} MB", MAX_LIN_FILE >> 20))),
        Ok(_) => std::fs::read_to_string(&file).map_err(|e| bad(cmd, format!("{file}: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(bad(cmd, format!("{file}: {e}"))),
    };
    let mut text = without(&old, name);
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&lin_text(&lt));
    crate::writefile::write(&file, text.as_bytes()).map_err(|e| EngineError::Other(format!("{file}: {e}")))?;
    let elements: Vec<Value> = lt
        .pattern
        .iter()
        .map(|e| json!({ "length": e.length, "text": e.text, "shape": e.shape, "style": e.style, "scale": e.scale, "rotation": e.rotation.to_degrees(), "offset": [e.offset.x, e.offset.y] }))
        .collect();
    let dm = s.doc_mut()?;
    match dm.linetypes.iter_mut().find(|l| l.name.eq_ignore_ascii_case(name)) {
        Some(old) => *old = Linetype { name: old.name.clone(), ..lt.clone() },
        None => dm.linetypes.push(lt.clone()),
    }
    let mut message = format!("Linetype {name} written to {file} and loaded.");
    for note in &notes {
        message.push_str(&format!("\n{note}"));
    }
    Ok(json!({ "name": name, "file": file, "pattern": elements, "notes": notes, "message": message }))
}

/// MKLTYPE typed: "Enter linetype file name:", "Enter linetype name:", "Enter linetype
/// description:", "Specify starting point for line definition:", "Specify ending point for
/// line definition:", then the objects.
#[derive(Default)]
struct MkltypeM {
    texts: Vec<String>,
    start: Option<Vec2>,
    end: Option<Vec2>,
    sel: SelectPhase,
}

impl Interactive for MkltypeM {
    fn name(&self) -> &'static str {
        "MKLTYPE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (self.texts.len(), self.start, self.end) {
            (0, ..) => Prompt::new("Enter linetype file name", Accept::TEXT),
            (1, ..) => Prompt::new("Enter linetype name", super::NAME),
            (2, ..) => Prompt::new("Enter linetype description", Accept::TEXT),
            (_, None, _) => Prompt::new("Specify starting point for line definition", Accept::POINT),
            (_, Some(a), None) => Prompt::new("Specify ending point for line definition", Accept::POINT).base(a),
            _ => self.sel.prompt(),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.texts.len() < 3 {
            match i {
                Input::Text(t) | Input::Keyword(t) if !t.trim().is_empty() || self.texts.len() == 2 => {
                    self.texts.push(t.trim().trim_matches('"').to_string())
                }
                Input::Enter if self.texts.len() == 2 => self.texts.push(String::new()),
                Input::Enter => return Ok(Step::Cancel),
                _ => {}
            }
            return Ok(Step::Continue);
        }
        match (self.start, self.end, i) {
            (None, _, Input::Point(p)) => {
                self.start = Some(p);
                Ok(Step::Continue)
            }
            (Some(_), None, Input::Point(p)) => {
                self.end = Some(p);
                self.sel = SelectPhase::begin(s);
                if self.sel.done {
                    let hs = self.sel.picked.clone();
                    return self.finish(s, &hs);
                }
                Ok(Step::Continue)
            }
            (Some(_), Some(_), i) => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Cancel),
                SelOutcome::Done(hs) => self.finish(s, &hs),
            },
            _ => Ok(Step::Continue),
        }
    }
}

impl MkltypeM {
    fn finish(&self, s: &mut Session, hs: &[cadaddy_doc::Handle]) -> Result<Step> {
        let (Some(a), Some(b)) = (self.start, self.end) else { return Ok(Step::Cancel) };
        let t = |i: usize| self.texts.get(i).cloned().unwrap_or_default();
        let r = run_mkltype(
            s,
            &json!({ "file": t(0), "name": t(1), "description": t(2), "start": [a.x, a.y], "end": [b.x, b.y], "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>() }),
        )?;
        super::echo_message(s, &r);
        Ok(Step::Done)
    }
}
