//! ATTEXT: the attribute values of block references written out as records, in the classic
//! formats: CDF (comma-delimited), SDF (fixed columns) — both laid out by a template file — and
//! DXF (an interchange file holding just the references and their attributes).
//!
//! A template has one field a line: a name and a format `Cwwwddd` (characters) or `Nwwwddd`
//! (a number), `www` the width and `ddd` the decimals. The names are attribute tags or the
//! reference's own values: `BL:LEVEL` (nesting level, 1 at the top), `BL:NAME`, `BL:X`, `BL:Y`,
//! `BL:Z`, `BL:NUMBER` (a counter), `BL:HANDLE`, `BL:LAYER`, `BL:ORIENT` (rotation in
//! degrees), `BL:XSCALE`, `BL:YSCALE`, `BL:ZSCALE`, `BL:XEXTRUDE`, `BL:YEXTRUDE`,
//! `BL:ZEXTRUDE`. `C:QUOTE c` and `C:DELIM c` set CDF's quote and delimiter characters.
//! A reference makes a record when it has an attribute the template names; references inside
//! blocks count too, with their level.

use std::fmt::Write as _;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Insert, MAX_BLOCK_DEPTH};
use cadaddy_geom::Mat3;
use serde_json::{Value, json};

use super::super::helpers::support_text;
use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most fields a template has, records written and the widest field.
const MAX_FIELDS: usize = 512;
const MAX_RECORDS: usize = 1_000_000;
const MAX_WIDTH: usize = 999;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("attext", "Attribute Extraction", run_attext)
            .alias(&["-attext"])
            .params(
                "{format?: cdf (default) | sdf | dxf, template?: path of the template file | templateText: its contents \
                 (required for cdf and sdf), file?: output path (default: the records are returned only), handles? (default: \
                 every block reference in model space and the layouts)} → {records, text, file?}",
            )
            .noundo()
            .interactive(|_| Ok(Box::new(AttextM::default()))),
    ]
}

/// A template field: its name (upper case), numeric or not, width and decimals.
#[derive(Clone, Debug, PartialEq)]
struct Field {
    name: String,
    numeric: bool,
    width: usize,
    decimals: usize,
}

/// A parsed template: its fields and CDF's quote and delimiter.
#[derive(Clone, Debug, PartialEq)]
struct Template {
    fields: Vec<Field>,
    quote: char,
    delim: char,
}

/// Read a template; an error names the line at fault.
fn parse_template(text: &str) -> std::result::Result<Template, String> {
    let mut t = Template { fields: Vec::new(), quote: '\'', delim: ',' };
    for (k, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(name), Some(spec)) = (parts.next(), parts.next()) else {
            return Err(format!("line {}: `{line}` needs a name and a format such as C010000", k + 1));
        };
        let name = name.to_ascii_uppercase();
        if name == "C:QUOTE" || name == "C:DELIM" {
            let c = spec.chars().next().unwrap_or(',');
            if name == "C:QUOTE" {
                t.quote = c;
            } else {
                t.delim = c;
            }
            continue;
        }
        let mut chars = spec.chars();
        let numeric = match chars.next().map(|c| c.to_ascii_uppercase()) {
            Some('N') => true,
            Some('C') => false,
            _ => return Err(format!("line {}: the format `{spec}` must start with C or N", k + 1)),
        };
        let digits: String = chars.collect();
        if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("line {}: the format `{spec}` must be C or N and six digits (width, decimals)", k + 1));
        }
        let width = digits.get(..3).and_then(|w| w.parse::<usize>().ok()).unwrap_or(0);
        let decimals = digits.get(3..).and_then(|w| w.parse::<usize>().ok()).unwrap_or(0);
        if width == 0 || width > MAX_WIDTH {
            return Err(format!("line {}: the width must be 1 to {MAX_WIDTH}", k + 1));
        }
        let bl_text = ["BL:NAME", "BL:HANDLE", "BL:LAYER"].contains(&name.as_str());
        let bl_num = name.starts_with("BL:") && !bl_text;
        if (bl_text && numeric) || (bl_num && !numeric) {
            return Err(format!("line {}: {name} must be {}", k + 1, if bl_text { "a character field (C)" } else { "a numeric field (N)" }));
        }
        if bl_num
            && ![
                "BL:LEVEL",
                "BL:X",
                "BL:Y",
                "BL:Z",
                "BL:NUMBER",
                "BL:ORIENT",
                "BL:XSCALE",
                "BL:YSCALE",
                "BL:ZSCALE",
                "BL:XEXTRUDE",
                "BL:YEXTRUDE",
                "BL:ZEXTRUDE",
            ]
            .contains(&name.as_str())
        {
            return Err(format!("line {}: unknown field {name}", k + 1));
        }
        if t.fields.len() >= MAX_FIELDS {
            return Err(format!("more than {MAX_FIELDS} fields"));
        }
        t.fields.push(Field { name, numeric, width, decimals });
    }
    if t.fields.is_empty() {
        return Err("the template has no fields".into());
    }
    Ok(t)
}

/// One block reference found: where it is, how deep, and its attributes (tag upper case, value).
struct Found {
    level: usize,
    handle: Handle,
    layer: String,
    ins: Insert,
    /// World position, rotation (radians) and scale after the references holding it.
    at: cadaddy_geom::Vec3,
    rotation: f64,
    scale: cadaddy_geom::Vec3,
    attribs: Vec<(String, String)>,
}

/// The references in `ents` and, at deeper levels, in their blocks.
fn collect(d: &Drawing, ents: &[&Entity], m: Mat3, z: f64, k: (f64, f64, f64), level: usize, out: &mut Vec<Found>) {
    if level > MAX_BLOCK_DEPTH {
        return;
    }
    for e in ents {
        if out.len() >= MAX_RECORDS {
            return;
        }
        let EntityKind::Insert(i) = &e.kind else { continue };
        let at = m.apply(i.insert.xy());
        let rotation = m.apply_vec(cadaddy_geom::Vec2::from_angle(i.rotation)).angle();
        let scale = cadaddy_geom::Vec3::new(i.scale.x * k.0, i.scale.y * k.1, i.scale.z * k.2);
        out.push(Found {
            level,
            handle: e.handle,
            layer: e.common.layer.clone(),
            ins: i.clone(),
            at: at.to3(i.insert.z + z),
            rotation,
            scale,
            attribs: i.attribs.iter().map(|a| (a.tag.trim().to_ascii_uppercase(), a.text.value.clone())).collect(),
        });
        if let Some(b) = d.block(&i.block) {
            let inner: Vec<&Entity> = b.entities.iter().map(|e| e.as_ref()).collect();
            let m2 = m.then_before(i.transform(b.base.xy()));
            collect(d, &inner, m2, z + i.insert.z, (scale.x, scale.y, scale.z), level + 1, out);
        }
    }
}

/// A number with `dec` decimals (no "-0").
fn num(v: f64, dec: usize) -> String {
    let t = format!("{v:.dec$}");
    if t.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { t.trim_start_matches('-').to_string() } else { t }
}

/// The field's value for a reference (`None`: an attribute it doesn't have), and whether a
/// numeric field's attribute wasn't a number.
fn value(f: &Field, r: &Found, number: usize) -> (Option<String>, bool) {
    let n = |v: f64| Some(num(v, f.decimals));
    let v = match f.name.as_str() {
        "BL:LEVEL" => n(r.level as f64),
        "BL:NAME" => Some(r.ins.block.clone()),
        "BL:X" => n(r.at.x),
        "BL:Y" => n(r.at.y),
        "BL:Z" => n(r.at.z),
        "BL:NUMBER" => n(number as f64),
        "BL:HANDLE" => Some(r.handle.hex()),
        "BL:LAYER" => Some(r.layer.clone()),
        "BL:ORIENT" => n(cadaddy_geom::norm_angle(r.rotation).to_degrees()),
        "BL:XSCALE" => n(r.scale.x),
        "BL:YSCALE" => n(r.scale.y),
        "BL:ZSCALE" => n(r.scale.z),
        "BL:XEXTRUDE" | "BL:YEXTRUDE" => n(0.0),
        "BL:ZEXTRUDE" => n(1.0),
        tag => {
            let Some((_, v)) = r.attribs.iter().find(|(t, _)| t == tag) else { return (None, false) };
            if f.numeric {
                return match v.trim().parse::<f64>().ok().filter(|x| x.is_finite()) {
                    Some(x) => (n(x), false),
                    None => (n(0.0), true),
                };
            }
            Some(v.clone())
        }
    };
    (v, false)
}

/// CDF and SDF records: (text, records, notes).
fn records(t: &Template, found: &[Found], sdf: bool) -> (String, usize, Vec<String>) {
    let mut out = String::new();
    let mut n = 0;
    let mut notes = Vec::new();
    for r in found {
        if !t.fields.iter().any(|f| !f.name.starts_with("BL:") && r.attribs.iter().any(|(tag, _)| *tag == f.name)) {
            continue;
        }
        n += 1;
        let mut cells = Vec::new();
        for f in &t.fields {
            let (v, bad) = value(f, r, n);
            if bad {
                notes.push(format!("{} ({}): {} is not a number; written as 0", r.ins.block, r.handle.hex(), f.name));
            }
            let v = v.unwrap_or_default();
            let v: String = v.chars().filter(|c| *c != '\n' && *c != '\r').collect();
            cells.push(if sdf {
                let v: String = v.chars().take(f.width).collect();
                if f.numeric { format!("{v:>w$}", w = f.width) } else { format!("{v:<w$}", w = f.width) }
            } else if f.numeric {
                v
            } else {
                let v: String = v.trim_end().chars().take(f.width).collect();
                format!("{q}{v}{q}", q = t.quote)
            });
        }
        let sep = if sdf { String::new() } else { t.delim.to_string() };
        let _ = writeln!(out, "{}", cells.join(&sep).trim_end());
    }
    (out, n, notes)
}

/// The DXF form: the references holding attributes, with their attributes, in an ENTITIES
/// section (group codes from the DXF Reference).
fn dxx(found: &[Found]) -> (String, usize) {
    let mut w = String::new();
    let tag = |w: &mut String, code: i32, v: &str| {
        let _ = writeln!(w, "{code:>3}\n{v}");
    };
    tag(&mut w, 0, "SECTION");
    tag(&mut w, 2, "ENTITIES");
    let mut n = 0;
    for r in found.iter().filter(|r| !r.attribs.is_empty()) {
        n += 1;
        tag(&mut w, 0, "INSERT");
        tag(&mut w, 5, &r.handle.hex());
        tag(&mut w, 8, &r.layer);
        tag(&mut w, 66, "1");
        tag(&mut w, 2, &r.ins.block);
        for (c, v) in [(10, r.at.x), (20, r.at.y), (30, r.at.z), (41, r.scale.x), (42, r.scale.y), (43, r.scale.z)] {
            tag(&mut w, c, &num(v, 6));
        }
        tag(&mut w, 50, &num(cadaddy_geom::norm_angle(r.rotation).to_degrees(), 6));
        for a in &r.ins.attribs {
            tag(&mut w, 0, "ATTRIB");
            tag(&mut w, 8, &r.layer);
            for (c, v) in [(10, a.text.insert.x), (20, a.text.insert.y), (30, a.text.insert.z), (40, a.text.height)] {
                tag(&mut w, c, &num(v, 6));
            }
            tag(&mut w, 1, &a.text.value.replace(['\n', '\r'], " "));
            tag(&mut w, 2, &a.tag);
            tag(&mut w, 70, if a.invisible { "1" } else { "0" });
        }
        tag(&mut w, 0, "SEQEND");
        tag(&mut w, 8, &r.layer);
    }
    tag(&mut w, 0, "ENDSEC");
    tag(&mut w, 0, "EOF");
    (w, n)
}

fn run_attext(s: &mut Session, p: &Value) -> Result<Value> {
    let format = str_param(p, "format").unwrap_or("cdf").trim().to_ascii_lowercase();
    if !["cdf", "sdf", "dxf"].contains(&format.as_str()) {
        return Err(bad("attext", format!("unknown format `{format}` (cdf, sdf or dxf)")));
    }
    let d = s.doc()?;
    let ents: Vec<&Entity> = if p.get("handles").is_some_and(|v| !v.is_null()) || p.get("handle").is_some_and(|v| !v.is_null()) {
        targets(s, p)?.into_iter().filter_map(|h| d.entity(h)).map(|e| e.as_ref()).collect()
    } else {
        d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())).map(|e| e.as_ref()).collect()
    };
    let mut found = Vec::new();
    collect(d, &ents, Mat3::IDENTITY, 0.0, (1.0, 1.0, 1.0), 1, &mut found);
    let (text, n, notes) = if format == "dxf" {
        let (t, n) = dxx(&found);
        (t, n, Vec::new())
    } else {
        let mut q = p.clone();
        if let Some(t) = q.get("templateText").cloned() {
            q["text"] = t;
        }
        let (tt, src) = support_text("attext", &q, "template", "text")?
            .ok_or_else(|| bad("attext", "`template` (a template file) is required for CDF and SDF"))?;
        let t = parse_template(&tt).map_err(|e| bad("attext", format!("{src}: {e}")))?;
        records(&t, &found, format == "sdf")
    };
    let file = str_param(p, "file").map(str::trim).filter(|f| !f.is_empty());
    if let Some(f) = file {
        crate::writefile::write(f, text.as_bytes()).map_err(|e| bad("attext", format!("{f}: {e}")))?;
    }
    let mut message = format!("{n} record(s) in extract file.");
    for note in &notes {
        message.push_str(&format!("\n{note}"));
    }
    Ok(json!({ "records": n, "text": text, "file": file, "notes": notes, "message": message }))
}

/// ATTEXT typed: "Enter extraction type or enable object selection [Cdf/Sdf/Dxf/Objects] <C>:",
/// then the template file (CDF, SDF) and the output file.
#[derive(Default)]
struct AttextM {
    format: Option<String>,
    template: Option<String>,
    objects: Option<Vec<Handle>>,
    sel: Option<super::super::machines::SelectPhase>,
}

impl Interactive for AttextM {
    fn name(&self) -> &'static str {
        "ATTEXT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if let Some(sel) = &self.sel {
            return sel.prompt();
        }
        match (&self.format, &self.template) {
            (None, _) => {
                Prompt::new("Enter extraction type or enable object selection", Accept::TEXT).kw(&["Cdf", "Sdf", "Dxf", "Objects"]).default("C")
            }
            (Some(f), None) if f != "dxf" => Prompt::new("Enter template file name", Accept::TEXT),
            _ => Prompt::new("Enter extract file name", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        use super::super::machines::SelOutcome;
        if let Some(sel) = self.sel.as_mut() {
            match sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => self.sel = None,
                SelOutcome::Done(hs) => {
                    self.objects = Some(hs);
                    self.sel = None;
                }
            }
            return Ok(Step::Continue);
        }
        let text = match i {
            Input::Text(t) | Input::Keyword(t) => t.trim().trim_matches('"').to_string(),
            Input::Enter => String::new(),
            Input::Cancel => return Ok(Step::Cancel),
            _ => return Ok(Step::Continue),
        };
        if self.format.is_none() {
            match text.to_ascii_lowercase().chars().next() {
                None | Some('c') => self.format = Some("cdf".into()),
                Some('s') => self.format = Some("sdf".into()),
                Some('d') => self.format = Some("dxf".into()),
                Some('o') => {
                    let sel = super::super::machines::SelectPhase::begin(s);
                    if sel.done {
                        self.objects = Some(sel.picked.clone());
                    } else {
                        self.sel = Some(sel);
                    }
                }
                _ => s.echo("Enter C, S, D or O."),
            }
            return Ok(Step::Continue);
        }
        let format = self.format.clone().unwrap_or_default();
        if format != "dxf" && self.template.is_none() {
            if text.is_empty() {
                return Ok(Step::Cancel);
            }
            self.template = Some(if std::path::Path::new(&text).extension().is_some() { text } else { format!("{text}.txt") });
            return Ok(Step::Continue);
        }
        if text.is_empty() {
            return Ok(Step::Cancel);
        }
        let ext = if format == "dxf" { "dxx" } else { "txt" };
        let file = if std::path::Path::new(&text).extension().is_some() { text } else { format!("{text}.{ext}") };
        let mut p = json!({ "format": format, "file": file });
        if let Some(t) = &self.template {
            p["template"] = json!(t);
        }
        if let Some(hs) = &self.objects {
            p["handles"] = json!(hs.iter().map(|h| h.hex()).collect::<Vec<_>>());
        }
        let r = run_attext(s, &p)?;
        super::echo_message(s, &r);
        Ok(Step::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_read_and_refuse_bad_lines() {
        let t = parse_template("BL:NAME C008000\nBL:X N009002\nC:DELIM |\nROOM N004000\nNOTE C020000\n").unwrap();
        assert_eq!(t.delim, '|');
        assert_eq!(t.fields.len(), 4);
        assert_eq!(t.fields[1], Field { name: "BL:X".into(), numeric: true, width: 9, decimals: 2 });
        for bad in ["BL:NAME N008000", "BL:X C008000", "TAG X008000", "TAG C08000", "TAG", "BL:WHAT N001000", "TAG C000000", ""] {
            assert!(parse_template(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn numbers_have_their_decimals() {
        assert_eq!(num(1.005, 2), "1.00");
        assert_eq!(num(-0.0001, 2), "0.00");
        assert_eq!(num(12.0, 0), "12");
    }
}
