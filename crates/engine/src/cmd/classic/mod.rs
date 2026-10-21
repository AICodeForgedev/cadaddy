//! Everyday drafting commands AutoCAD users type without thinking: OOPS, MULTIPLE, COPYTOLAYER,
//! DIMCENTER, SOLID (2D solid), FILL and QTEXT, VPMAX / VPMIN, FILTER, ATTREDEF, DXFIN / DXFOUT,
//! -PAN, and the View and Tools menu items that set an option of an existing command (Attribute
//! Display, UCS Icon, New UCS ▸ Origin / Z / 3 Point, Named UCS...).

use std::sync::Arc;

use cadaddy_doc::{Entity, EntityKind, Space};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step, View};

mod copytolayer;
mod filter;
mod solid;
mod vpmax;

pub use vpmax::VpMax;

/// A name typed at the command line: a space ends it, as Enter does (`MULTIPLE CIRCLE 0,0 1`).
pub(crate) const NAME: Accept = Accept { number: true, ..Accept::TEXT };

pub fn specs() -> Vec<CommandSpec> {
    let mut v = vec![
        CommandSpec::new("oops", "Oops", run_oops).params("{} → {restored} (brings back the objects the last ERASE removed)"),
        CommandSpec::new("multiple", "Multiple", run_multiple)
            .params("(typed only) MULTIPLE, then a command name: the command starts again each time it ends, until Esc")
            .noundo()
            .interactive(|_| Ok(Box::new(MultipleM))),
        CommandSpec::new("dimcenter", "Center Mark (DIMCEN)", run_dimcenter)
            .params(
                "{handle | handles} (circles, arcs) → {handles} — DIMCEN of the current dimension style × its scale: positive a \
                 cross that size, negative a cross and center lines past the curve, 0 nothing",
            )
            .alias(&["dce"])
            .interactive(|_| Ok(Box::new(DimCenterM))),
        CommandSpec::new("fill", "Fill", run_fill)
            .params("{mode?: on|off} (FILLMODE: solids, solid hatches and wide polylines filled or outlined; no mode: reports it) → {fillmode}")
            .interactive(|_| Ok(Box::new(ModeM { id: "fill", name: "FILL", var: "FILLMODE", default: 1 }))),
        CommandSpec::new("qtext", "Quick Text", run_qtext)
            .params("{mode?: on|off} (QTEXTMODE: text shown and plotted as its outline box; no mode: reports it) → {qtextmode}")
            .interactive(|_| Ok(Box::new(ModeM { id: "qtext", name: "QTEXT", var: "QTEXTMODE", default: 0 }))),
        CommandSpec::new("-pan", "Point", run_dash_pan)
            .menu(&["View", "Pan", "Point"])
            .params("{delta: [dx,dy]} | {from: [x,y], to: [x,y]} (the drawing moves by the displacement)")
            .noundo()
            .transparent()
            .interactive(|_| Ok(Box::new(DashPanM::default()))),
        CommandSpec::new("attdisp.normal", "Normal", |s, _| attdisp(s, "normal"))
            .menu(&["View", "Attribute Display", "Normal"])
            .params("{} (ATTDISP Normal: attributes show as defined)"),
        CommandSpec::new("attdisp.on", "On", |s, _| attdisp(s, "on"))
            .menu(&["View", "Attribute Display", "On"])
            .params("{} (ATTDISP ON: every attribute shows)"),
        CommandSpec::new("attdisp.off", "Off", |s, _| attdisp(s, "off"))
            .menu(&["View", "Attribute Display", "Off"])
            .params("{} (ATTDISP OFF: no attribute shows)"),
        CommandSpec::new("ucsicon.on", "On", |s, _| ucsicon_toggle(s, 1))
            .menu(&["View", "UCS Icon", "On"])
            .params("{} (turns the UCS icon on or off) → {ucsicon}")
            .noundo(),
        CommandSpec::new("ucsicon.origin", "Origin", |s, _| ucsicon_toggle(s, 2))
            .menu(&["View", "UCS Icon", "Origin"])
            .params("{} (shows the UCS icon at the UCS origin, or in the corner) → {ucsicon}")
            .noundo(),
        CommandSpec::new("ucs.origin", "Origin", |s, p| ucs_option(s, p, "origin"))
            .menu(&["Tools", "New UCS", "Origin"])
            .params("{origin: [x,y]} (the UCS moves there, keeping its rotation)")
            .interactive(|_| Ok(Box::new(UcsPointsM { three: false, pts: Vec::new() }))),
        CommandSpec::new("ucs.z", "Z", |s, p| ucs_option(s, p, "z"))
            .menu(&["Tools", "New UCS", "Z"])
            .params("{angle?: degrees (default 90)} (the UCS turns about its Z axis)")
            .interactive(|_| Ok(Box::new(UcsZM))),
        CommandSpec::new("ucs.3point", "3 Point", |s, p| ucs_option(s, p, "3point"))
            .menu(&["Tools", "New UCS", "3 Point"])
            .params("{origin: [x,y], xpoint: [x,y], ypoint?: [x,y]} (new origin, a point on its +X axis, a point on the +Y side)")
            .interactive(|_| Ok(Box::new(UcsPointsM { three: true, pts: Vec::new() }))),
        CommandSpec::new("ucsman", "Named UCS...", run_ucsman)
            .menu(&["Tools", "Named UCS..."])
            .alias(&["uc", "dducs"])
            .params(
                "{option?: list (default) | restore | delete | rename, name? (restore, delete: names \"a,b*\"; rename: the old name), \
                 newName? (rename)} → {current, ucs: [{name, origin, angle}]}",
            )
            .interactive(|_| Ok(Box::new(UcsmanM::default()))),
        CommandSpec::new("dxfout", "DXF Out", run_dxfout)
            .params("{path (.dxf added when it has no extension), version?: 12|2000|2004|2007|2010|2013|2018, binary?: bool} → as SAVEAS")
            .noundo()
            .interactive(|_| Ok(Box::new(super::file::FileM { name: "DXFOUT", message: "Enter name of DXF file to save", run: run_dxfout }))),
        CommandSpec::new("dxfin", "DXF In", run_dxfin)
            .params("{path} (a .dxf file) → as OPEN")
            .enabled(always)
            .noundo()
            .interactive(|_| Ok(Box::new(super::file::FileM { name: "DXFIN", message: "Enter name of DXF file to open", run: run_dxfin }))),
        CommandSpec::new("attredef", "Redefine Attributes", run_attredef)
            .params(
                "{name: an existing block, handles?: the new definition's objects (default: selection), base: [x,y]} → {block, references} \
                 — redefines the block and brings its references in line: attributes in both keep their values, new ones get their \
                 defaults, removed ones go",
            )
            .interactive(|_| Ok(Box::new(AttredefM::default()))),
    ];
    v.extend(copytolayer::specs());
    v.extend(filter::specs());
    v.extend(solid::specs());
    v.extend(vpmax::specs());
    v
}

/// "Yes"/"on"/1 and "No"/"off"/0 as a switch.
fn on_off(p: &Value, key: &str) -> Result<Option<bool>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(Value::Number(n)) => Ok(Some(n.as_f64().is_some_and(|v| v != 0.0))),
        Some(Value::String(t)) => match t.trim().to_ascii_lowercase().as_str() {
            "on" | "yes" | "true" | "1" => Ok(Some(true)),
            "off" | "no" | "false" | "0" => Ok(Some(false)),
            _ => Err(EngineError::Other(format!("`{key}` must be on or off"))),
        },
        Some(v) => Err(EngineError::Other(format!("`{key}` must be on or off, got {v}"))),
    }
}

/// Run another command's JSON body inside this one (its single undo step is this command's).
fn call(s: &mut Session, id: &str, p: &Value) -> Result<Value> {
    let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?;
    (spec.run)(s, p)
}

fn echo_message(s: &mut Session, v: &Value) {
    if let Some(m) = v.get("message").and_then(Value::as_str) {
        for l in m.lines() {
            s.echo(l.to_string());
        }
    }
}

// ---------------- OOPS ----------------

/// Most objects one ERASE leaves for OOPS (a bigger erase is still undone by U).
const MAX_OOPS: usize = 1_000_000;

/// Remember what an ERASE removed, for OOPS: the objects and the spaces they were in.
pub(crate) fn remember_erased(s: &mut Session, gone: Vec<(Space, Arc<Entity>)>) {
    if gone.is_empty() || gone.len() > MAX_OOPS {
        return;
    }
    if let Ok(st) = s.state_mut() {
        st.oops = gone;
    }
}

fn run_oops(s: &mut Session, _p: &Value) -> Result<Value> {
    let gone = std::mem::take(&mut s.state_mut()?.oops);
    if gone.is_empty() {
        return Err(EngineError::Other("Nothing to restore: OOPS brings back what the last ERASE removed.".into()));
    }
    let d = s.doc_mut()?;
    let mut restored = Vec::new();
    for (space, e) in gone {
        // Undone already, or its layout deleted since: nothing to bring back.
        if d.entity(e.handle).is_some() {
            continue;
        }
        let layer = e.common.layer.clone();
        let Some(store) = d.space_mut(&space) else { continue };
        let h = e.handle;
        store.push((*e).clone());
        d.ensure_layer(&layer);
        restored.push(h);
    }
    let n = restored.len();
    Ok(json!({
        "restored": n,
        "handles": restored.iter().map(|h| h.hex()).collect::<Vec<_>>(),
        "message": format!("{n} object(s) restored."),
    }))
}

// ---------------- MULTIPLE ----------------

fn run_multiple(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(bad("multiple", "MULTIPLE repeats a command typed at the command line: type MULTIPLE, then the command's name"))
}

/// Commands MULTIPLE won't repeat: ones that act on the command history or the session.
const NOT_REPEATED: &[&str] = &["multiple", "u", "undo", "redo", "mredo", "quit", "close", "closeall", "open", "new", "script"];

struct MultipleM;

impl Interactive for MultipleM {
    fn name(&self) -> &'static str {
        "MULTIPLE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Enter command name to repeat", NAME)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let name = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().trim_start_matches(['_', '.']).to_string(),
            Input::Enter | Input::Cancel => return Ok(Step::Cancel),
            _ => return Ok(Step::Continue),
        };
        if name.is_empty() {
            return Ok(Step::Cancel);
        }
        let id = resolve_alias(&name);
        let Some(spec) = find_command(&id) else {
            return Err(EngineError::Other(format!("Unknown command \"{}\".", name.to_ascii_uppercase())));
        };
        if NOT_REPEATED.contains(&spec.id) {
            return Err(EngineError::Other(format!("MULTIPLE does not repeat {}.", spec.id.to_ascii_uppercase())));
        }
        if spec.interactive.is_none() {
            return Err(EngineError::Other(format!("{} doesn't ask for input: MULTIPLE repeats commands that do.", spec.id.to_ascii_uppercase())));
        }
        s.multiple = Some(spec.id.to_string());
        Ok(Step::Done)
    }
}

// ---------------- DIMCENTER ----------------

/// The current dimension style's DIMCEN, scaled (DIMSCALE / the style's own scale).
fn dimcen(s: &Session) -> Result<f64> {
    let d = s.doc()?;
    let name = d.header.str("DIMSTYLE", "Standard");
    let st = d.dim_style(&name).cloned().unwrap_or_default().with_overrides(&d.dim_overrides());
    let v = st.center_mark * st.effective_scale(d.header.f64("DIMSCALE", 1.0));
    Ok(if v.is_finite() && v.abs() <= 1e12 { v } else { 0.0 })
}

/// DIMCENTER's lines for a circle or arc of centre `c` and radius `r`, along the UCS axes
/// `ux`/`uy`: a cross of half-size |DIMCEN|, and with a negative DIMCEN center lines from twice
/// that out to |DIMCEN| past the curve.
pub(crate) fn dimcenter_lines(c: Vec2, r: f64, cen: f64, ux: Vec2, uy: Vec2) -> Vec<(Vec2, Vec2)> {
    let m = cen.abs();
    if m <= 1e-12 {
        return Vec::new();
    }
    let mut out = vec![(c - ux * m, c + ux * m), (c - uy * m, c + uy * m)];
    if cen < 0.0 && 2.0 * m < r + m {
        for u in [ux, uy, -ux, -uy] {
            out.push((c + u * (2.0 * m), c + u * (r + m)));
        }
    }
    out
}

fn dimcenter(s: &mut Session, h: cadaddy_doc::Handle) -> Result<Vec<String>> {
    let (c, r) = match curves::entity(s, h)?.kind {
        EntityKind::Circle(c) => (c.center.xy(), c.radius),
        EntityKind::Arc(a) => (a.center.xy(), a.radius),
        _ => return Err(EngineError::Other("Select an arc or circle.".into())),
    };
    let cen = dimcen(s)?;
    let f = s.ucs();
    let (ux, uy) = (Vec2::from_angle(f.angle), Vec2::from_angle(f.angle + std::f64::consts::FRAC_PI_2));
    let mut out = Vec::new();
    for (a, b) in dimcenter_lines(c, r, cen, ux, uy) {
        out.push(s.add_entity(helpers::line(a, b))?.hex());
    }
    Ok(out)
}

fn run_dimcenter(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad("dimcenter", "`handle` or `handles` (circles or arcs) is required"));
    }
    let mut out = Vec::new();
    for h in hs {
        out.extend(dimcenter(s, h)?);
    }
    let message = if out.is_empty() { Some("DIMCEN is 0: no center mark drawn.") } else { None };
    Ok(json!({ "handles": out, "message": message }))
}

struct DimCenterM;

impl Interactive for DimCenterM {
    fn name(&self) -> &'static str {
        "DIMCENTER"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        s.set_selection(Vec::new());
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Select arc or circle", Accept::POINT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Point(p) => {
                let h = curves::pick_at(s, p).ok_or_else(|| EngineError::Other("*Invalid selection*".into()))?;
                if dimcenter(s, h)?.is_empty() {
                    s.echo("DIMCEN is 0: no center mark drawn.");
                }
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}

// ---------------- FILL / QTEXT ----------------

fn set_mode(s: &mut Session, cmd: &str, var: &str, default: i64, p: &Value) -> Result<Value> {
    let key = var.to_ascii_lowercase();
    match on_off(p, "mode").map_err(|e| bad(cmd, e.to_string()))? {
        Some(on) => {
            s.doc_mut()?.header.set_i64(var, i64::from(on));
            s.touch();
            Ok(json!({ key: i64::from(on), "message": format!("{var} is now {}. REGEN shows it.", if on { "on" } else { "off" }) }))
        }
        None => {
            let v = s.doc()?.header.i64(var, default);
            Ok(json!({ key: v, "message": format!("{var} is {}.", if v != 0 { "on" } else { "off" }) }))
        }
    }
}

fn run_fill(s: &mut Session, p: &Value) -> Result<Value> {
    set_mode(s, "fill", "FILLMODE", 1, p)
}

fn run_qtext(s: &mut Session, p: &Value) -> Result<Value> {
    set_mode(s, "qtext", "QTEXTMODE", 0, p)
}

/// FILL / QTEXT typed: "Enter mode [ON/OFF] <current>:".
struct ModeM {
    id: &'static str,
    name: &'static str,
    var: &'static str,
    default: i64,
}

impl Interactive for ModeM {
    fn name(&self) -> &'static str {
        self.name
    }
    fn prompt(&self, s: &Session) -> Prompt {
        let on = s.doc().map(|d| d.header.i64(self.var, self.default) != 0).unwrap_or(self.default != 0);
        Prompt::new("Enter mode", Accept::TEXT).kw(&["ON", "OFF"]).default(if on { "ON" } else { "OFF" })
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let mode = match &i {
            Input::Enter => return Ok(Step::Done),
            Input::Text(t) | Input::Keyword(t) => t.trim().to_ascii_lowercase(),
            _ => return Ok(Step::Continue),
        };
        if mode != "on" && mode != "off" {
            return Err(EngineError::Other("Invalid option keyword.".into()));
        }
        let r = call(s, self.id, &json!({ "mode": mode }))?;
        echo_message(s, &r);
        Ok(Step::Done)
    }
}

// ---------------- -PAN ----------------

fn pan_by(s: &mut Session, delta: Vec2) -> Result<Value> {
    if !delta.is_finite() {
        return Err(bad("-pan", "the displacement must be finite"));
    }
    let st = s.state_mut()?;
    let v = st.view();
    if !st.set_view(View { center: v.center - delta, height: v.height }) {
        return Err(bad("-pan", "the view would be too far away. Nothing was changed"));
    }
    s.touch();
    Ok(json!({ "center": [v.center.x - delta.x, v.center.y - delta.y] }))
}

fn run_dash_pan(s: &mut Session, p: &Value) -> Result<Value> {
    let d = match point_param(p, "delta") {
        Some(d) => d,
        None => {
            let a = point_param(p, "from").ok_or_else(|| bad("-pan", "give `delta` or `from` and `to`"))?;
            point_req("-pan", p, "to")? - a
        }
    };
    pan_by(s, d)
}

/// -PAN: "Specify base point or displacement:", then "Specify second point:" (Enter: the first
/// point is the displacement).
#[derive(Default)]
struct DashPanM {
    base: Option<Vec2>,
}

impl Interactive for DashPanM {
    fn name(&self) -> &'static str {
        "-PAN"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.base {
            None => Prompt::new("Specify base point or displacement", Accept::POINT),
            Some(b) => Prompt::new("Specify second point", Accept::POINT).base(b),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.base, i) {
            (None, Input::Point(p)) => {
                self.base = Some(p);
                Ok(Step::Continue)
            }
            (None, Input::Enter) => Ok(Step::Done),
            (Some(b), Input::Point(p)) => {
                pan_by(s, p - b)?;
                Ok(Step::Done)
            }
            (Some(b), Input::Enter) => {
                pan_by(s, b)?;
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
}

// ---------------- View menu: ATTDISP and UCSICON items ----------------

fn attdisp(s: &mut Session, mode: &str) -> Result<Value> {
    call(s, "attdisp", &json!({ "mode": mode }))
}

fn ucsicon_toggle(s: &mut Session, bit: i64) -> Result<Value> {
    let cur = s.doc()?.header.i64("UCSICON", 3);
    let option = match (bit, cur & bit != 0) {
        (1, true) => "off",
        (1, false) => "on",
        (_, true) => "noorigin",
        (_, false) => "origin",
    };
    call(s, "ucsicon", &json!({ "option": option }))
}

// ---------------- New UCS menu items, UCSMAN ----------------

fn ucs_option(s: &mut Session, p: &Value, option: &str) -> Result<Value> {
    let mut q = match p {
        Value::Object(_) => p.clone(),
        _ => json!({}),
    };
    q["option"] = json!(option);
    call(s, "ucs", &q)
}

/// New UCS ▸ Origin ("Specify new origin point <0,0,0>:") and ▸ 3 Point (origin, a point on the
/// positive X axis, a point on the positive Y side; Enter keeps the default each time).
struct UcsPointsM {
    three: bool,
    pts: Vec<Vec2>,
}

impl UcsPointsM {
    fn apply(&self, s: &mut Session) -> Result<Step> {
        let cur = s.ucs();
        let origin = self.pts.first().copied().unwrap_or(cur.origin);
        let r = if self.three {
            let x = self.pts.get(1).copied().unwrap_or(origin + Vec2::from_angle(cur.angle));
            let mut p = json!({ "origin": [origin.x, origin.y], "xpoint": [x.x, x.y] });
            if let Some(y) = self.pts.get(2) {
                p["ypoint"] = json!([y.x, y.y]);
            }
            ucs_option(s, &p, "3point")?
        } else {
            ucs_option(s, &json!({ "origin": [origin.x, origin.y] }), "origin")?
        };
        echo_message(s, &r);
        Ok(Step::Done)
    }
}

impl Interactive for UcsPointsM {
    fn name(&self) -> &'static str {
        "UCS"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        let cur = s.ucs();
        match self.pts.len() {
            0 => Prompt::new("Specify new origin point", Accept::POINT).default("0,0,0"),
            1 => Prompt::new("Specify point on positive portion of X-axis", Accept::POINT).base_opt(self.pts.first().copied()),
            _ => Prompt::new("Specify point on positive-Y portion of the UCS XY plane", Accept::POINT)
                .base_opt(self.pts.first().copied())
                .default(format!("{:.4}", cur.angle.to_degrees() + 90.0)),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Point(p) => {
                self.pts.push(p);
                if !self.three || self.pts.len() >= 3 { self.apply(s) } else { Ok(Step::Continue) }
            }
            Input::Enter => {
                // Enter at the origin: the world origin (AutoCAD's <0,0,0>); later ones keep the axes.
                if self.pts.is_empty() {
                    self.pts.push(Vec2::ZERO);
                    if self.three {
                        return Ok(Step::Continue);
                    }
                }
                self.apply(s)
            }
            _ => Ok(Step::Continue),
        }
    }
}

/// New UCS ▸ Z: "Specify rotation angle about Z axis <90>:".
struct UcsZM;

impl Interactive for UcsZM {
    fn name(&self) -> &'static str {
        "UCS"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        Prompt::new("Specify rotation angle about Z axis", Accept::POINT_OR_NUMBER).default("90").base(s.ucs().origin)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let deg = match &i {
            Input::Enter => 90.0,
            Input::Point(p) => {
                let f = s.ucs();
                (f.origin.angle_to(*p) - f.angle).to_degrees()
            }
            Input::Text(t) => {
                s.angle_settings().amount(t).map(f64::to_degrees).ok_or_else(|| EngineError::Other("Requires a valid angle.".into()))?
            }
            _ => return Ok(Step::Continue),
        };
        let r = ucs_option(s, &json!({ "angle": deg }), "z")?;
        echo_message(s, &r);
        Ok(Step::Done)
    }
}

fn run_ucsman(s: &mut Session, p: &Value) -> Result<Value> {
    let option = str_param(p, "option").unwrap_or("list").trim().to_ascii_lowercase();
    match option.as_str() {
        "list" | "?" | "" => call(s, "ucs", &json!({ "option": "list", "names": p.get("names").or_else(|| p.get("name")).cloned() })),
        "restore" | "set" | "current" => {
            let name = str_param(p, "name").ok_or_else(|| bad("ucsman", "`name` is required"))?;
            call(s, "ucs", &json!({ "option": "restore", "name": name }))
        }
        "delete" => {
            let names = p.get("names").or_else(|| p.get("name")).cloned().ok_or_else(|| bad("ucsman", "`name` is required"))?;
            call(s, "ucs", &json!({ "option": "delete", "names": names }))
        }
        "rename" => {
            let from = str_param(p, "name").map(str::trim).ok_or_else(|| bad("ucsman", "`name` (the UCS to rename) is required"))?.to_string();
            let to = str_param(p, "newName").map(str::trim).ok_or_else(|| bad("ucsman", "`newName` is required"))?.to_string();
            rename_ucs(s, &from, &to)
        }
        _ => Err(bad("ucsman", "option must be list, restore, delete or rename")),
    }
}

fn rename_ucs(s: &mut Session, from: &str, to: &str) -> Result<Value> {
    let valid = !to.is_empty()
        && to.chars().count() <= 255
        && !to.eq_ignore_ascii_case("world")
        && !to.chars().any(|c| "<>/\\\":;?*|,=`".contains(c) || c.is_control());
    if !valid {
        return Err(EngineError::Other(format!("\"{to}\" is not a valid UCS name.")));
    }
    let d = s.doc_mut()?;
    if d.ucss.iter().any(|u| u.name.eq_ignore_ascii_case(to) && !u.name.eq_ignore_ascii_case(from)) {
        return Err(EngineError::Other(format!("A UCS named \"{to}\" already exists.")));
    }
    let rec =
        d.ucss.iter_mut().find(|u| u.name.eq_ignore_ascii_case(from)).ok_or_else(|| EngineError::Other(format!("Could not find UCS \"{from}\".")))?;
    rec.name = to.to_string();
    if d.header.str("UCSNAME", "").eq_ignore_ascii_case(from) {
        d.header.set("UCSNAME", cadaddy_doc::HVal::Str(to.to_string()));
    }
    Ok(json!({ "renamed": from, "to": to, "message": format!("UCS \"{from}\" renamed to \"{to}\".") }))
}

/// UCSMAN typed (the Named UCS dialog's choices at the command line): "Enter an option
/// [Restore/Delete/Rename/?] <?>:", then the name(s).
#[derive(Default)]
struct UcsmanM {
    option: Option<&'static str>,
    from: Option<String>,
}

impl Interactive for UcsmanM {
    fn name(&self) -> &'static str {
        "UCSMAN"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (self.option, &self.from) {
            (None, _) => Prompt::new("Enter an option", Accept::TEXT).kw(&["Restore", "Delete", "Rename", "?"]).default("?"),
            (Some("restore"), _) => Prompt::new("Enter name of UCS to restore", Accept::TEXT),
            (Some("delete"), _) => Prompt::new("Enter UCS name(s) to delete", Accept::TEXT),
            (Some(_), None) => Prompt::new("Enter name of UCS to rename", Accept::TEXT),
            (Some(_), Some(_)) => Prompt::new("Enter new UCS name", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            Input::Enter => String::new(),
            _ => return Ok(Step::Continue),
        };
        let Some(option) = self.option else {
            self.option = match text.to_ascii_lowercase().as_str() {
                "" | "?" => {
                    let r = run_ucsman(s, &json!({ "option": "list" }))?;
                    echo_message(s, &r);
                    return Ok(Step::Done);
                }
                "r" | "restore" => Some("restore"),
                "d" | "delete" => Some("delete"),
                "ren" | "rename" => Some("rename"),
                _ => return Err(EngineError::Other("Invalid option keyword.".into())),
            };
            return Ok(Step::Continue);
        };
        if text.is_empty() {
            return Ok(Step::Done);
        }
        let r = match (option, self.from.take()) {
            ("rename", None) => {
                self.from = Some(text);
                return Ok(Step::Continue);
            }
            ("rename", Some(from)) => run_ucsman(s, &json!({ "option": "rename", "name": from, "newName": text }))?,
            (o, _) => run_ucsman(s, &json!({ "option": o, "name": text }))?,
        };
        echo_message(s, &r);
        Ok(Step::Done)
    }
}

// ---------------- DXFOUT / DXFIN ----------------

fn run_dxfout(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("dxfout", "`path` is required"))?;
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let path = match ext.as_deref() {
        None => format!("{path}.dxf"),
        Some("dxf") => path.to_string(),
        Some(other) => return Err(bad("dxfout", format!("DXFOUT writes .dxf files, not .{other} (SAVEAS writes the others)"))),
    };
    let mut q = match p {
        Value::Object(_) => p.clone(),
        _ => json!({}),
    };
    q["path"] = json!(path);
    call(s, "saveas", &q)
}

fn run_dxfin(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("dxfin", "`path` is required"))?;
    let is_dxf = std::path::Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("dxf"));
    if !is_dxf {
        return Err(bad("dxfin", "DXFIN opens .dxf files (OPEN opens the others)"));
    }
    call(s, "open", &json!({ "path": path }))
}

// ---------------- ATTREDEF ----------------

fn run_attredef(s: &mut Session, p: &Value) -> Result<Value> {
    let name =
        str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("attredef", "`name` (the block to redefine) is required"))?;
    let existing = s.doc()?.blocks.keys().find(|k| k.eq_ignore_ascii_case(name)).cloned();
    let Some(name) = existing else {
        return Err(EngineError::Other(format!("Block \"{name}\" not found: ATTREDEF redefines an existing block.")));
    };
    let base = point_req("attredef", p, "base")?;
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad("attredef", "`handles` (the objects of the new definition) is required"));
    }
    let description = s.doc()?.blocks.get(&name).map(|b| b.description.clone()).unwrap_or_default();
    super::blocks::make_block(s, &name, base, &hs, "delete", &description)?;
    let mut r = call(s, "attsync", &json!({ "name": name }))?;
    r["message"] = json!(format!("Block \"{name}\" redefined; its references keep the values of the attributes it still has."));
    Ok(r)
}

/// ATTREDEF typed: "Enter name of the block you wish to redefine:", "Select objects for new
/// Block...", "Specify insertion base point of new Block:".
#[derive(Default)]
struct AttredefM {
    name: Option<String>,
    sel: Option<machines::SelectPhase>,
    picked: Vec<cadaddy_doc::Handle>,
}

impl Interactive for AttredefM {
    fn name(&self) -> &'static str {
        "ATTREDEF"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (&self.name, &self.sel) {
            (None, _) => Prompt::new("Enter name of the block you wish to redefine", NAME),
            (Some(_), Some(sel)) if self.picked.is_empty() => sel.prompt(),
            _ => Prompt::new("Specify insertion base point of new Block", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.name.is_none() {
            let name = match &i {
                Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
                Input::Enter => return Ok(Step::Done),
                _ => return Ok(Step::Continue),
            };
            if !s.doc()?.blocks.keys().any(|k| k.eq_ignore_ascii_case(&name)) {
                return Err(EngineError::Other(format!("Block \"{name}\" not found.")));
            }
            self.name = Some(name);
            s.echo("Select objects for new Block...");
            self.sel = Some(machines::SelectPhase::default());
            return Ok(Step::Continue);
        }
        if self.picked.is_empty() {
            let Some(sel) = self.sel.as_mut() else { return Ok(Step::Done) };
            return match sel.feed(s, &i)? {
                machines::SelOutcome::More => Ok(Step::Continue),
                machines::SelOutcome::Empty => Ok(Step::Done),
                machines::SelOutcome::Done(hs) => {
                    self.picked = hs;
                    Ok(Step::Continue)
                }
            };
        }
        match i {
            Input::Point(b) => {
                let hs: Vec<String> = self.picked.iter().map(|h| h.hex()).collect();
                let r = run_attredef(s, &json!({ "name": self.name, "handles": hs, "base": [b.x, b.y] }))?;
                echo_message(s, &r);
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}
