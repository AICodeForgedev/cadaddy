//! FIELD and UPDATEFIELD: fields in text (see [`crate::fields`]).
//!
//! FIELD makes a multiline text holding one field, or adds the field to the end of a text or
//! multiline text. UPDATEFIELD works the fields of the objects chosen out again.

use cadaddy_doc::{EntityKind, Handle, MText};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase};
use super::*;
use crate::fields::{self, Event};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// The fields FIELD offers by name: (name, code expression, default format).
const NAMES: &[(&str, &str, Option<&str>)] = &[
    ("date", "\\AcVar Date", Some("M/d/yyyy")),
    ("createdate", "\\AcVar CreateDate", Some("M/d/yyyy")),
    ("savedate", "\\AcVar SaveDate", Some("M/d/yyyy")),
    ("plotdate", "\\AcVar PlotDate", Some("M/d/yyyy")),
    ("filename", "\\AcVar Filename", Some("%fn6")),
    ("login", "\\AcVar Login", None),
    ("sheetname", "\\AcVar CurrentSheetName", None),
    ("sheetnumber", "\\AcVar CurrentSheetNumber", None),
    ("sheettitle", "\\AcVar CurrentSheetTitle", None),
    ("devicename", "\\AcVar DeviceName", None),
    ("pagesetupname", "\\AcVar PageSetupName", None),
];

/// Properties an object field offers.
const PROPERTIES: &[&str] =
    &["Area", "Perimeter", "Length", "Radius", "Diameter", "Circumference", "Layer", "Linetype", "Color", "TextString", "Name"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("field", "Field...", run_field)
            .menu(&["Insert", "Field..."])
            .params(
                "{field: date|createdate|savedate|plotdate|filename|login|sheetname|sheetnumber|sheettitle|devicename|pagesetupname|object|code, \
                 format?: a date pattern (M/d/yyyy, dddd MMMM d, yyyy h:mm tt…), %fnN (filename: 1 folder, 2 extension, 4 name only), %tcN (1 UPPER, 2 lower, 3 Sentence, 4 Title), \
                 %luN%prN (number units and precision), %ct8[factor], %ps[prefix,suffix]; object?: handle (field: object), property?: Area|Perimeter|Length|Radius|Diameter|Circumference|Layer|Linetype|Color|TextString|Name|Attribute(TAG), \
                 code?: a field code such as \\\\AcVar Date (field: code), at?: [x,y] (a new multiline text there; height? style?), into?: handle (a TEXT or MTEXT the field goes at the end of)} → {handle, code, value}",
            )
            .interactive(|_| Ok(Box::new(FieldM::default()))),
        CommandSpec::new("updatefield", "Update Fields", run_updatefield)
            .menu(&["Tools", "Update Fields"])
            .params("{handles?: [hex] (default: the selection, else every object)} → {updated}")
            .interactive(|_| Ok(Box::new(UpdateM::default()))),
    ]
}

/// The field code (with its `%<` `>%`) FIELD's parameters ask for.
fn code_of(s: &Session, p: &Value) -> Result<String> {
    let kind = str_param(p, "field").unwrap_or("date").trim().to_ascii_lowercase();
    let format = str_param(p, "format").map(str::trim);
    if format.is_some_and(|f| f.contains('"') || f.len() > 200) {
        return Err(bad("field", "`format` can't hold quotes and is at most 200 characters"));
    }
    let with_format = |expr: String, default: Option<&str>| match format.or(default).filter(|f| !f.is_empty()) {
        Some(f) => format!("%<{expr} \\f \"{f}\">%"),
        None => format!("%<{expr}>%"),
    };
    match kind.as_str() {
        "object" => {
            let h = targets(s, &json!({ "handle": p.get("object").cloned().unwrap_or(Value::Null) }))?
                .first()
                .copied()
                .ok_or_else(|| bad("field", "`object` (the object whose property the field shows) is required"))?;
            if s.doc()?.entity(h).is_none() {
                return Err(bad("field", format!("no object {}", h.hex())));
            }
            let prop = str_param(p, "property").unwrap_or("Area").trim();
            let known = PROPERTIES.iter().any(|k| k.eq_ignore_ascii_case(prop))
                || prop.to_ascii_lowercase().strip_prefix("attribute(").is_some_and(|r| r.ends_with(')') && r.len() > 1);
            if !known {
                return Err(bad("field", format!("unknown property \"{prop}\" (known: {}, Attribute(TAG))", PROPERTIES.join(", "))));
            }
            Ok(with_format(format!("\\AcObjProp Object(%<\\_ObjId {}>%).{prop}", h.0), None))
        }
        "code" => {
            let raw = str_param(p, "code").map(str::trim).filter(|c| !c.is_empty()).ok_or_else(|| bad("field", "`code` is required"))?;
            let raw = raw.strip_prefix("%<").and_then(|r| r.strip_suffix(">%")).unwrap_or(raw);
            let code = format!("%<{}>%", if raw.starts_with('\\') { raw.to_string() } else { format!("\\{raw}") });
            if cadaddy_doc::field::spans(&code).len() != 1 || code.len() > 4000 {
                return Err(bad("field", "`code` is not a field code"));
            }
            Ok(code)
        }
        k => {
            let (_, expr, default) = NAMES.iter().find(|(n, _, _)| *n == k).ok_or_else(|| {
                bad("field", format!("unknown field \"{k}\" (known: {}, object, code)", NAMES.iter().map(|n| n.0).collect::<Vec<_>>().join(", ")))
            })?;
            Ok(with_format(expr.to_string(), *default))
        }
    }
}

fn run_field(s: &mut Session, p: &Value) -> Result<Value> {
    let code = code_of(s, p)?;
    let h = match p.get("into") {
        Some(v) if !v.is_null() => {
            let h = targets(s, &json!({ "handle": v }))?.first().copied().ok_or_else(|| bad("field", "`into` must be a handle"))?;
            add_to(s, h, &code)?
        }
        _ => {
            let at = point_param(p, "at").ok_or_else(|| bad("field", "`at` (where the text goes) or `into` (a text to add to) is required"))?;
            insert_mtext(s, at, &code, p)?
        }
    };
    // The value it shows now (the post-command pass keeps it).
    fields::evaluate(s, Event::Update, Some(&[h]));
    let value = s.doc()?.field_values.get(&h).and_then(|v| v.last().cloned()).unwrap_or_else(|| cadaddy_doc::field::NO_VALUE.into());
    Ok(json!({ "handle": h.hex(), "code": code, "value": value }))
}

/// A new multiline text holding `code` at `at`, in the current text style and size.
fn insert_mtext(s: &mut Session, at: Vec2, code: &str, p: &Value) -> Result<Handle> {
    let height = f64_or(p, "height", s.doc()?.header.f64("TEXTSIZE", 0.2));
    if !(height.is_finite() && height > 0.0) {
        return Err(bad("field", "`height` must be positive"));
    }
    let style = match str_param(p, "style") {
        Some(n) => s.doc()?.text_style(n).map(|t| t.name.clone()).ok_or_else(|| bad("field", format!("no text style \"{n}\"")))?,
        None => s.doc()?.header.str("TEXTSTYLE", "Standard"),
    };
    s.add_entity(EntityKind::MText(MText {
        insert: at.to3(0.0),
        height,
        width: 0.0,
        attach: 1,
        rotation: 0.0,
        style,
        contents: code.to_string(),
        line_spacing: 1.0,
        line_spacing_exact: false,
        columns: None,
        background: None,
    }))
}

/// Add `code` to the end of the text of `h` (a TEXT or MTEXT).
fn add_to(s: &mut Session, h: Handle, code: &str) -> Result<Handle> {
    let d = s.doc_mut()?;
    let mut ok = false;
    d.modify_entity(h, |e| match &mut e.kind {
        EntityKind::Text(t) => {
            t.value.push_str(code);
            ok = true;
        }
        EntityKind::MText(m) => {
            m.contents.push_str(code);
            ok = true;
        }
        _ => {}
    })
    .map_err(|_| bad("field", format!("no object {}", h.hex())))?;
    if !ok {
        return Err(bad("field", "`into` must be a text or multiline text"));
    }
    Ok(h)
}

fn run_updatefield(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let n = update(s, &hs);
    Ok(json!({ "updated": n, "message": format!("{n} field object(s) updated.") }))
}

/// Work the fields of `hs` out again (every object's when `hs` is empty).
fn update(s: &mut Session, hs: &[Handle]) -> usize {
    let only = (!hs.is_empty()).then_some(hs);
    // Count the objects holding fields, changed or not, as AutoCAD reports them.
    let holding = match s.doc() {
        Ok(d) => {
            let has = |h: &Handle| d.entity(*h).is_some_and(|e| cadaddy_doc::field::kind_has_fields(&e.kind));
            match only {
                Some(hs) => hs.iter().filter(|h| has(h)).count(),
                None => d.field_values.len(),
            }
        }
        Err(_) => 0,
    };
    fields::evaluate(s, Event::Update, only);
    holding
}

// ---------------- prompts ----------------

#[derive(Clone, Copy, Default, PartialEq)]
enum Stage {
    #[default]
    Name,
    Object,
    Property,
    Format,
    Point,
}

/// FIELD typed: "Enter field name [Date/…/Object]", for Object "Select object" and "Enter
/// property", then "Enter format" and "Specify start point".
#[derive(Default)]
struct FieldM {
    stage: Stage,
    params: serde_json::Map<String, Value>,
}

impl FieldM {
    fn default_format(&self) -> Option<&'static str> {
        let name = self.params.get("field").and_then(Value::as_str).unwrap_or("date");
        NAMES.iter().find(|n| n.0 == name).and_then(|n| n.2)
    }
}

impl Interactive for FieldM {
    fn name(&self) -> &'static str {
        "FIELD"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::Name => Prompt::new("Enter field name", Accept::TEXT)
                .kw(&["Date", "CreateDate", "SaveDate", "PlotDate", "Filename", "Login", "SheetName", "SheetNumber", "SheetTitle", "Object"])
                .default("Date"),
            Stage::Object => Prompt::new("Select object", Accept::SELECT),
            Stage::Property => Prompt::new("Enter property", Accept::TEXT).kw(PROPERTIES).default("Area"),
            Stage::Format => match self.default_format() {
                Some(f) => Prompt::new("Enter format", Accept::TEXT).default(f),
                None => Prompt::new("Enter format or <none>", Accept::TEXT),
            },
            Stage::Point => Prompt::new("Specify start point", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_string()),
            _ => None,
        };
        match self.stage {
            Stage::Name => {
                let name = text.filter(|t| !t.is_empty()).unwrap_or_else(|| "date".into()).to_ascii_lowercase();
                if name != "object" && !NAMES.iter().any(|n| n.0 == name) {
                    return Err(EngineError::Other("Invalid option keyword.".into()));
                }
                self.stage = if name == "object" { Stage::Object } else { Stage::Format };
                self.params.insert("field".into(), json!(name));
                Ok(Step::Continue)
            }
            Stage::Object => match i {
                Input::Pick(hs) => match hs.first() {
                    Some(h) => {
                        self.params.insert("object".into(), json!(h.hex()));
                        self.stage = Stage::Property;
                        Ok(Step::Continue)
                    }
                    None => Ok(Step::Continue),
                },
                Input::Enter => Ok(Step::Done),
                _ => Err(EngineError::Other("Select an object.".into())),
            },
            Stage::Property => {
                let prop = text.filter(|t| !t.is_empty()).unwrap_or_else(|| "Area".into());
                self.params.insert("property".into(), json!(prop));
                self.stage = Stage::Format;
                Ok(Step::Continue)
            }
            Stage::Format => {
                if let Some(f) = text.filter(|t| !t.is_empty()) {
                    self.params.insert("format".into(), json!(f));
                }
                // Checked now: a bad property or format re-prompts here, not after the point.
                code_of(s, &Value::Object(self.params.clone()))?;
                self.stage = Stage::Point;
                Ok(Step::Continue)
            }
            Stage::Point => match i {
                Input::Point(p) => {
                    self.params.insert("at".into(), json!([p.x, p.y]));
                    let r = run_field(s, &Value::Object(std::mem::take(&mut self.params)))?;
                    if let Some(v) = r.get("value").and_then(Value::as_str) {
                        s.echo(format!("Field: {v}"));
                    }
                    Ok(Step::Done)
                }
                Input::Enter => Ok(Step::Done),
                _ => Err(EngineError::Other("Requires a point.".into())),
            },
        }
    }
}

/// UPDATEFIELD typed: "Select objects:", then their fields are worked out again.
#[derive(Default)]
struct UpdateM {
    sel: SelectPhase,
}

impl Interactive for UpdateM {
    fn name(&self) -> &'static str {
        "UPDATEFIELD"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            let hs = self.sel.picked.clone();
            return finish_update(s, &hs);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        self.sel.prompt()
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => Ok(Step::Done),
            SelOutcome::Done(hs) => {
                s.set_selection(Vec::new());
                finish_update(s, &hs)
            }
        }
    }
}

fn finish_update(s: &mut Session, hs: &[Handle]) -> Result<Step> {
    let n = update(s, hs);
    s.echo(format!("{n} field object(s) found."));
    s.echo(format!("{n} field object(s) updated."));
    Ok(Step::Done)
}
