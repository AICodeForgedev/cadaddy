//! TOLERANCE: geometric tolerances (feature control frames) and their explosion.

use cadaddy_doc::{
    Compartment, Drawing, Entity, EntityKind, GdtDatum, GdtFrame, GdtLine, GdtPrim, GdtRun, GdtSymbol, GdtTol, Handle, MAX_FRAME_CHARS, Tolerance,
};
use cadaddy_geom::{PolyVertex, Vec2};
use serde_json::{Value, json};

use super::helpers::{line, lwpoly, v3};
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tolerance", "Tolerance...", run_tolerance)
            .menu(&["Dimension", "Tolerance..."])
            .alias(&["tol"])
            .params(
                "{at: [x,y], symbol?: position|concentricity|symmetry|parallelism|perpendicularity|angularity|cylindricity|flatness|circularity|straightness|surfaceprofile|lineprofile|circularrunout|totalrunout, tol1?: \"%%c0.1(M)\" | {value, diameter?, material?: M|L|S}, tol2?, datums?: [\"A(M)\", \"B\", {letter, material?}] (up to 3), line2?: {symbol?, tol1?, tol2?, datums?}, height? (projected zone), projected?, datumId?, rotation? (degrees), style? (dimension style; default DIMSTYLE), text? (the frame string as in DXF, instead of the fields)}",
            )
            .interactive(|s| Ok(Box::new(ToleranceM::new(s)))),
        CommandSpec::new("tolerance.edit", "Edit Tolerance", run_edit)
            .params("{handle, text? (the frame string) | the fields of `tolerance` (symbol, tol1, tol2, datums, line2, height, projected, datumId)} → {handle, text}: new contents, same place"),
    ]
}

/// A tolerance typed as text: an optional diameter prefix (`%%c`, `Ø`, `⌀`, `dia`), the value and
/// an optional material condition suffix (`(M)`, `(L)`, `(S)`).
pub fn parse_tol(s: &str) -> GdtTol {
    let mut t = s.trim();
    let mut diameter = false;
    for p in ["%%c", "%%C", "Ø", "ø", "⌀", "dia ", "DIA "] {
        if let Some(r) = t.strip_prefix(p) {
            diameter = true;
            t = r.trim_start();
            break;
        }
    }
    let (rest, material) = split_material(t);
    GdtTol { diameter, value: rest.to_string(), material }
}

/// A datum reference typed as text: the letter(s) and an optional `(M)` / `(L)` / `(S)` suffix.
pub fn parse_datum(s: &str) -> GdtDatum {
    let (letter, material) = split_material(s.trim());
    GdtDatum { letter: letter.to_string(), material }
}

fn split_material(t: &str) -> (&str, Option<String>) {
    let up = t.to_ascii_uppercase();
    for m in ["M", "L", "S"] {
        if up.ends_with(&format!("({m})")) {
            return (t.get(..t.len().saturating_sub(3)).unwrap_or("").trim_end(), Some(m.to_string()));
        }
    }
    (t, None)
}

fn tol_value(v: Option<&Value>) -> Option<GdtTol> {
    match v? {
        Value::String(s) => Some(parse_tol(s)),
        Value::Number(n) => Some(GdtTol { value: n.to_string(), ..Default::default() }),
        o @ Value::Object(_) => Some(GdtTol {
            diameter: bool_or(o, "diameter", false),
            value: match o.get("value") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            },
            material: str_param(o, "material").map(str::to_string),
        }),
        _ => None,
    }
}

fn datums_value(v: Option<&Value>) -> Vec<GdtDatum> {
    let Some(a) = v.and_then(Value::as_array) else { return Vec::new() };
    a.iter()
        .take(3)
        .filter_map(|d| match d {
            Value::String(s) => Some(parse_datum(s)),
            o @ Value::Object(_) => {
                Some(GdtDatum { letter: str_param(o, "letter").unwrap_or("").to_string(), material: str_param(o, "material").map(str::to_string) })
            }
            _ => None,
        })
        .collect()
}

fn symbol_value(cmd: &str, v: Option<&Value>) -> Result<Option<GdtSymbol>> {
    let Some(s) = v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("none")) else {
        return Ok(None);
    };
    match GdtSymbol::parse(s).or_else(|| s.chars().next().filter(|_| s.chars().count() == 1).and_then(GdtSymbol::from_code)) {
        Some(sym) if sym.is_characteristic() => Ok(Some(sym)),
        _ => Err(bad(cmd, format!("unknown geometric characteristic `{s}`"))),
    }
}

fn line_value(cmd: &str, p: &Value) -> Result<GdtLine> {
    Ok(GdtLine {
        symbol: symbol_value(cmd, p.get("symbol"))?,
        tol1: tol_value(p.get("tol1")),
        tol2: tol_value(p.get("tol2")),
        datums: datums_value(p.get("datums")),
    })
}

/// The frame string the parameters describe.
fn frame_text(p: &Value) -> Result<String> {
    if let Some(t) = str_param(p, "text") {
        return Ok(t.chars().take(MAX_FRAME_CHARS).collect());
    }
    let mut lines = vec![line_value("tolerance", p)?];
    if let Some(l2) = p.get("line2").filter(|v| v.is_object()) {
        lines.push(line_value("tolerance", l2)?);
    }
    let height = match p.get("height") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    let f = GdtFrame { lines, height, projected: bool_or(p, "projected", false), datum_id: str_param(p, "datumId").unwrap_or("").to_string() };
    Ok(cadaddy_doc::compose_frame(&f))
}

/// A frame string taken apart into the dialog's fields: up to two tolerance lines, then the
/// projected zone height (with its symbol) and the datum identifier.
pub fn frame_from_text(text: &str) -> GdtFrame {
    let rows = cadaddy_doc::parse_frame(text);
    let symbol_in =
        |c: &Compartment, f: fn(GdtSymbol) -> bool| c.iter().find_map(|r| if let GdtRun::Symbol(g) = r { Some(*g).filter(|g| f(*g)) } else { None });
    let text_in = |c: &Compartment| {
        c.iter().filter_map(|r| if let GdtRun::Text(t) = r { Some(t.as_str()) } else { None }).collect::<String>().trim().to_string()
    };
    let material =
        |c: &Compartment| symbol_in(c, GdtSymbol::is_material).map(|g| g.name().chars().next().unwrap_or('m').to_ascii_uppercase().to_string());
    let tol = |c: Option<&Compartment>| -> Option<GdtTol> {
        let c = c.filter(|c| !cadaddy_doc::compartment_is_empty(c))?;
        Some(GdtTol { diameter: symbol_in(c, |g| g == GdtSymbol::Diameter).is_some(), value: text_in(c), material: material(c) })
    };
    let mut f = GdtFrame::default();
    let n = rows.len();
    for (i, row) in rows.iter().enumerate() {
        let first = row.first();
        let is_line = row.len() >= 2 || first.is_some_and(|c| symbol_in(c, GdtSymbol::is_characteristic).is_some());
        if is_line && f.lines.len() < 2 {
            f.lines.push(GdtLine {
                symbol: first.and_then(|c| symbol_in(c, GdtSymbol::is_characteristic)),
                tol1: tol(row.get(1)),
                tol2: tol(row.get(2)),
                datums: row
                    .iter()
                    .skip(3)
                    .take(3)
                    .filter(|c| !cadaddy_doc::compartment_is_empty(c))
                    .map(|c| GdtDatum { letter: text_in(c), material: material(c) })
                    .collect(),
            });
            continue;
        }
        let Some(c) = first else { continue };
        let projected = symbol_in(c, |g| g == GdtSymbol::Projected).is_some();
        // A single text compartment: the zone height when it has the projected symbol, comes before
        // another row or is a number; otherwise (letters) the datum identifier.
        let number = text_in(c).trim().parse::<f64>().is_ok_and(f64::is_finite);
        if f.height.is_empty() && !f.projected && (projected || i + 1 < n || number) {
            f.height = text_in(c);
            f.projected = projected;
        } else if f.datum_id.is_empty() {
            f.datum_id = text_in(c);
        }
    }
    f
}

/// Whether typed text is a whole frame string (as in DXF) rather than a single answer.
fn is_frame_string(t: &str) -> bool {
    t.contains("%%v") || t.contains("\\Fgdt") || t.contains("^J")
}

/// TOLERANCE.EDIT: new contents for a frame, which keeps its place, direction and style.
fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    let h = super::curves::handle_param(p, "handle").ok_or_else(|| bad("tolerance.edit", "`handle` (a tolerance) is required"))?;
    if !matches!(s.doc()?.entity(h).map(|e| &e.kind), Some(EntityKind::Tolerance(_))) {
        return Err(bad("tolerance.edit", "`handle` is not a geometric tolerance"));
    }
    let text = frame_text(p)?;
    if cadaddy_doc::parse_frame(&text).iter().flatten().all(cadaddy_doc::compartment_is_empty) {
        return Err(bad("tolerance.edit", "the feature control frame is empty"));
    }
    let t2 = text.clone();
    s.doc_mut()?.modify_entity(h, move |e| {
        if let EntityKind::Tolerance(t) = &mut e.kind {
            t.text = t2;
        }
    })?;
    Ok(json!({ "handle": h.hex(), "text": text }))
}

/// A frame from a `tolerance` parameter: the frame string, or an object of `tolerance`'s fields.
pub(crate) fn frame_param(v: &Value) -> Result<String> {
    match v {
        Value::String(t) => Ok(t.chars().take(MAX_FRAME_CHARS).collect()),
        Value::Object(_) => frame_text(v),
        _ => Err(bad("tolerance", "`tolerance` is a frame string or an object of fields")),
    }
}

/// A frame of the current dimension style at `at`, as a new object.
pub(crate) fn add_frame(s: &mut Session, at: Vec2, text: String) -> Result<Handle> {
    if cadaddy_doc::parse_frame(&text).iter().flatten().all(cadaddy_doc::compartment_is_empty) {
        return Err(bad("tolerance", "the feature control frame is empty"));
    }
    let style = current_dimstyle(s);
    s.add_entity(EntityKind::Tolerance(make(style, at, 0.0, text)))
}

fn current_dimstyle(s: &Session) -> String {
    s.doc().map(|d| d.header.str("DIMSTYLE", "Standard")).unwrap_or_else(|_| "Standard".into())
}

fn make(style: String, at: Vec2, rotation: f64, text: String) -> Tolerance {
    Tolerance { insert: v3(at), dir: Vec2::from_angle(rotation).to3(0.0), style, text }
}

fn run_tolerance(s: &mut Session, p: &Value) -> Result<Value> {
    let at = point_req("tolerance", p, "at")?;
    let text = frame_text(p)?;
    if cadaddy_doc::parse_frame(&text).iter().flatten().all(cadaddy_doc::compartment_is_empty) {
        return Err(bad("tolerance", "the feature control frame is empty"));
    }
    let style = str_param(p, "style").map(str::to_string).unwrap_or_else(|| current_dimstyle(s));
    if s.doc()?.dim_style(&style).is_none() {
        return Err(bad("tolerance", format!("no dimension style `{style}`")));
    }
    let t = make(style, at, f64_or(p, "rotation", 0.0).to_radians(), text.clone());
    let h = s.add_entity(EntityKind::Tolerance(t))?;
    Ok(json!({ "handle": h.hex(), "text": text }))
}

/// The frame questions TOLERANCE and the leaders' Tolerance annotation ask at the command line:
/// characteristic, two tolerance values, up to three datums. A whole frame string (as in DXF)
/// typed at the first question is taken as it is.
pub(crate) struct FrameAsk {
    step: usize,
    line: GdtLine,
}

/// What a frame answer leads to.
pub(crate) enum Asked {
    More,
    /// The frame string.
    Done(String),
    Cancel,
}

const SYMBOL_KW: &[&str] = &[
    "POsition",
    "COncentricity",
    "SYmmetry",
    "PArallelism",
    "PErpendicularity",
    "ANgularity",
    "CYlindricity",
    "Flatness",
    "CIrcularity",
    "STraightness",
    "SUrface",
    "LIne",
    "Runout",
    "TOtal",
    "None",
];

/// Free input split at spaces, Enter meaningful.
const VALUE: Accept = Accept { point: false, number: false, text: false, select: false, enter: true };

impl Default for FrameAsk {
    fn default() -> Self {
        FrameAsk { step: 0, line: GdtLine { symbol: Some(GdtSymbol::Position), ..Default::default() } }
    }
}

impl FrameAsk {
    fn text(&self) -> String {
        cadaddy_doc::compose_frame(&GdtFrame { lines: vec![self.line.clone()], ..Default::default() })
    }
    fn done(&self, s: &mut Session) -> Asked {
        let text = self.text();
        if cadaddy_doc::parse_frame(&text).iter().flatten().all(cadaddy_doc::compartment_is_empty) {
            s.echo("The feature control frame is empty.");
            return Asked::Cancel;
        }
        Asked::Done(text)
    }
    pub(crate) fn prompt(&self) -> Prompt {
        match self.step {
            0 => Prompt::new("Enter geometric characteristic", VALUE).kw(SYMBOL_KW).default("Position"),
            1 => Prompt::new("Enter first tolerance value", VALUE).default("none"),
            2 => Prompt::new("Enter second tolerance value", VALUE).default("none"),
            n => Prompt::new(format!("Enter datum reference {}", n.saturating_sub(2)), VALUE).default("none"),
        }
    }
    pub(crate) fn input(&mut self, s: &mut Session, i: &Input) -> Asked {
        let text = match i {
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_string()),
            _ => None,
        };
        let none = |t: &Option<String>| t.as_deref().is_none_or(|t| t.is_empty() || t.eq_ignore_ascii_case("none"));
        match self.step {
            0 => {
                match (i, &text) {
                    (Input::Enter, _) => {}
                    (_, Some(t)) if is_frame_string(t) => {
                        let t: String = t.chars().take(MAX_FRAME_CHARS).collect();
                        if cadaddy_doc::parse_frame(&t).iter().flatten().all(cadaddy_doc::compartment_is_empty) {
                            s.echo("The feature control frame is empty.");
                            return Asked::More;
                        }
                        return Asked::Done(t);
                    }
                    (_, Some(t)) if t.eq_ignore_ascii_case("none") => self.line.symbol = None,
                    (_, Some(t)) => match GdtSymbol::parse(t).filter(|g| g.is_characteristic()) {
                        Some(g) => self.line.symbol = Some(g),
                        None => {
                            s.echo("Requires a geometric characteristic or None.");
                            return Asked::More;
                        }
                    },
                    _ => return Asked::More,
                }
                self.step = 1;
            }
            1 | 2 => {
                if !matches!(i, Input::Enter | Input::Text(_) | Input::Keyword(_)) {
                    return Asked::More;
                }
                let v = (!none(&text)).then(|| parse_tol(text.as_deref().unwrap_or("")));
                if self.step == 1 {
                    self.line.tol1 = v;
                } else {
                    self.line.tol2 = v;
                }
                self.step += 1;
            }
            _ => {
                if !matches!(i, Input::Enter | Input::Text(_) | Input::Keyword(_)) {
                    return Asked::More;
                }
                if none(&text) {
                    return self.done(s);
                }
                self.line.datums.push(parse_datum(text.as_deref().unwrap_or("")));
                if self.step >= 5 {
                    return self.done(s);
                }
                self.step += 1;
            }
        }
        Asked::More
    }
}

/// TOLERANCE at the command line: the frame questions ([`FrameAsk`]), then the location.
struct ToleranceM {
    ask: FrameAsk,
    /// The frame, once asked: then the location is asked.
    frame: Option<String>,
    style: String,
}

impl ToleranceM {
    fn new(s: &Session) -> Self {
        ToleranceM { ask: FrameAsk::default(), frame: None, style: current_dimstyle(s) }
    }
}

impl Interactive for ToleranceM {
    fn name(&self) -> &'static str {
        "TOLERANCE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.frame {
            None => self.ask.prompt(),
            Some(_) => Prompt::new("Enter tolerance location", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        let Some(text) = self.frame.clone() else {
            return Ok(match self.ask.input(s, &i) {
                Asked::More => Step::Continue,
                Asked::Cancel => Step::Cancel,
                Asked::Done(t) => {
                    self.frame = Some(t);
                    Step::Continue
                }
            });
        };
        match i {
            Input::Point(p) => {
                let t = make(self.style.clone(), p, 0.0, text);
                s.add_entity(EntityKind::Tolerance(t))?;
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Cancel),
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, _s: &Session, c: Vec2) -> Vec<EntityKind> {
        match &self.frame {
            Some(t) => vec![EntityKind::Tolerance(make(self.style.clone(), c, 0.0, t.clone()))],
            None => Vec::new(),
        }
    }
}

/// EXPLODE of a feature control frame: lines, circles, arcs, solids (arrowheads) and texts in the
/// dimension style's text style.
pub(crate) fn explode(d: &Drawing, e: &Entity, t: &Tolerance) -> Vec<Entity> {
    let g = cadaddy_render::tolerance_in(d, t);
    let st = d.dim_style(&t.style).cloned().unwrap_or_default();
    let wf = d.text_style(&st.text_style).map(|s| s.width_factor).filter(|w| w.is_finite() && *w > 0.0).unwrap_or(1.0);
    let mk = |kind| Entity { handle: Handle(0), common: e.common.clone(), kind };
    let mut v = Vec::new();
    for p in g.frame.iter().chain(&g.symbols) {
        match p {
            GdtPrim::Line(pts) => match pts.as_slice() {
                [a, b] => v.push(mk(line(*a, *b))),
                pts if pts.len() > 2 => v.push(mk(lwpoly(pts.iter().map(|q| PolyVertex::new(*q)).collect(), false))),
                _ => {}
            },
            GdtPrim::Circle { center, radius } => v.push(mk(super::helpers::circle(*center, *radius))),
            GdtPrim::Arc { center, radius, start, end } => v.push(mk(EntityKind::Arc(cadaddy_doc::Arc {
                center: v3(*center),
                radius: *radius,
                start: cadaddy_geom::norm_angle(*start),
                end: cadaddy_geom::norm_angle(*end),
            }))),
            GdtPrim::Fill([a, b, c]) => v.push(mk(EntityKind::Solid(cadaddy_doc::Solid { corners: [v3(*a), v3(*b), v3(*c), v3(*c)] }))),
        }
    }
    for tx in &g.texts {
        v.push(mk(EntityKind::Text(cadaddy_doc::Text {
            insert: v3(tx.pos),
            align_pt: None,
            height: tx.height,
            value: tx.value.clone(),
            rotation: tx.rotation,
            width_factor: wf,
            oblique: 0.0,
            style: st.text_style.clone(),
            halign: Default::default(),
            valign: Default::default(),
        })));
    }
    v
}
