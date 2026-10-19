//! UCS (user coordinate systems in the plan: World, Origin, Z rotation, Object, View, Previous,
//! NAmed Save/Restore/Delete/?), UCSICON and UCSMAN's named list. The current UCS is the
//! drawing's ([`crate::ucs::Frame`]); named ones are its UCS table.

use cadaddy_doc::EntityKind;
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::qselect::wildcard;
use super::*;
use crate::ucs::Frame;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// UCS Previous remembers this many.
const MAX_PREVIOUS: usize = 10;
/// Most names one Delete takes.
const MAX_NAMES: usize = 1000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("ucs", "UCS", run_ucs)
            .menu(&["Tools", "New UCS", "World"])
            .params(
                "{option: world|origin|z|object|view|previous|set|save|restore|delete|list, origin?: [x,y] (origin, set; world coordinates), \
                 xpoint?: [x,y] (origin: a point on the new X axis), angle?: degrees (z: turned by; set: the X axis's angle), handle? (object), \
                 pick?: [x,y] (object: near the end or segment that sets it), name? (save, restore; delete/list take names?: \"a,b*\")} \
                 → {origin, angle, name, message} — a 2D UCS in the plan: origin and rotation",
            )
            .interactive(|_| Ok(Box::new(UcsM::default()))),
        CommandSpec::new("ucsicon", "UCS Icon", run_ucsicon)
            .params("{option: on|off|origin|noorigin} → {ucsicon} (UCSICON: 1 shown, 2 at the UCS origin when it is on screen)")
            .noundo()
            .transparent(),
    ]
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.chars().count() <= 255 && !n.eq_ignore_ascii_case("world") && !n.chars().any(|c| "<>/\\\":;?*|,=`".contains(c))
}

/// Make `f` the current UCS, remembering the one it replaces for Previous; with UCSFOLLOW 1 the
/// plan view of it follows.
fn set_current(s: &mut Session, f: Frame) -> Result<Value> {
    let old = s.ucs();
    let st = s.state_mut()?;
    st.ucs_previous.push(old);
    if st.ucs_previous.len() > MAX_PREVIOUS {
        st.ucs_previous.remove(0);
    }
    f.make_current(s.doc_mut()?);
    if s.doc()?.header.i64("UCSFOLLOW", 0) != 0 {
        plan_of(s, &f)?;
    }
    Ok(report(&f))
}

fn report(f: &Frame) -> Value {
    let name = if f.is_world() && f.name.is_empty() { "World".to_string() } else { f.name.clone() };
    json!({
        "origin": [f.origin.x, f.origin.y],
        "angle": f.angle.to_degrees(),
        "name": name,
        "world": f.is_world(),
    })
}

/// The plan view of `f`: looking down Z, its X axis across the screen, zoomed to the extents.
pub(crate) fn plan_of(s: &mut Session, f: &Frame) -> Result<()> {
    {
        let st = s.state()?;
        if st.space != cadaddy_doc::Space::Model || st.mspace.is_some() {
            return Err(EngineError::Other("PLAN works in model space.".into()));
        }
    }
    super::view::view3d::set_dir(s, cadaddy_geom::Vec3::Z, false)?;
    s.state_mut()?.set_twist(-f.angle);
    s.zoom_extents()
}

fn run_ucs(s: &mut Session, p: &Value) -> Result<Value> {
    let option = str_param(p, "option").unwrap_or("world").trim().to_ascii_lowercase();
    let cur = s.ucs();
    let angle = |key: &str| -> Result<Option<f64>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|a| a.is_finite() && a.abs() <= 1e6)
                .map(|a| Some(a.to_radians()))
                .ok_or_else(|| bad("ucs", format!("`{key}` must be a number of degrees"))),
        }
    };
    let f = match option.as_str() {
        "world" | "w" => Frame::WORLD,
        "origin" | "o" => {
            let origin = point_param(p, "origin").ok_or_else(|| bad("ucs", "`origin` is required"))?;
            let a = match point_param(p, "xpoint") {
                Some(x) if x.dist(origin) > 1e-9 => origin.angle_to(x),
                Some(_) => return Err(bad("ucs", "`xpoint` must differ from `origin`")),
                None => cur.angle,
            };
            Frame::new(origin, a)
        }
        "z" => Frame::new(cur.origin, cur.angle + angle("angle")?.unwrap_or(std::f64::consts::FRAC_PI_2)),
        "set" => {
            let origin = point_param(p, "origin").unwrap_or(cur.origin);
            Frame::new(origin, angle("angle")?.unwrap_or(cur.angle))
        }
        "object" | "ob" => {
            let h = targets(s, &json!({ "handle": p.get("handle").cloned().unwrap_or(Value::Null) }))?
                .first()
                .copied()
                .ok_or_else(|| bad("ucs", "`handle` (the object to align with) is required"))?;
            object_frame(s, h, point_param(p, "pick"))?
        }
        "view" | "v" => Frame::new(cur.origin, -s.state()?.twist()),
        "previous" | "p" => {
            let prev = s.state_mut()?.ucs_previous.pop().ok_or_else(|| EngineError::Other("No previous UCS.".into()))?;
            prev.make_current(s.doc_mut()?);
            let mut r = report(&prev);
            r["message"] = json!("Restored the previous UCS.");
            return Ok(r);
        }
        "save" | "s" => {
            let name = str_param(p, "name").map(str::trim).ok_or_else(|| bad("ucs", "`name` is required"))?.to_string();
            return save(s, &name);
        }
        "restore" | "r" => {
            let name = str_param(p, "name").map(str::trim).ok_or_else(|| bad("ucs", "`name` is required"))?.to_string();
            if name.eq_ignore_ascii_case("world") {
                Frame::WORLD
            } else {
                let rec = s
                    .doc()?
                    .ucss
                    .iter()
                    .find(|u| u.name.eq_ignore_ascii_case(&name))
                    .cloned()
                    .ok_or_else(|| EngineError::Other(format!("Could not find UCS \"{name}\".")))?;
                Frame::from_record(&rec)
                    .ok_or_else(|| EngineError::Other(format!("UCS \"{name}\" is not in the plan; only 2D UCSs can be restored.")))?
            }
        }
        "delete" | "d" => return delete(s, &patterns(p)),
        "list" | "?" => return list(s, &patterns(p)),
        "x" | "y" | "za" | "zaxis" | "face" | "f" | "3" | "3point" => {
            return Err(EngineError::Other("Only UCSs in the plan (origin and rotation about Z) are available so far.".into()));
        }
        _ => return Err(bad("ucs", "option must be world, origin, z, object, view, previous, set, save, restore, delete or list")),
    };
    let mut r = set_current(s, f)?;
    r["message"] = json!(format!("UCS: {}", r["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("Unnamed")));
    Ok(r)
}

/// The UCS an object defines (UCS Object): a line from its end nearest `pick` towards the other,
/// an arc or circle from its centre towards the end nearest `pick` (a circle: towards `pick`), a
/// polyline from the start of its segment nearest `pick` along it, text, a block reference or a
/// multiline text at its insertion point along its rotation, a point at its position.
fn object_frame(s: &Session, h: cadaddy_doc::Handle, pick: Option<Vec2>) -> Result<Frame> {
    let d = s.doc()?;
    let e = d.entity(h).ok_or_else(|| bad("ucs", format!("no object {}", h.hex())))?;
    let cur = s.ucs();
    let along = |a: Vec2, b: Vec2| if a.dist(b) > 1e-12 { a.angle_to(b) } else { cur.angle };
    let f = match &e.kind {
        EntityKind::Line(l) => {
            let (a, b) = (l.a.xy(), l.b.xy());
            match pick {
                Some(p) if p.dist(b) < p.dist(a) => Frame::new(b, along(b, a)),
                _ => Frame::new(a, along(a, b)),
            }
        }
        EntityKind::Arc(a) => {
            let c = a.center.xy();
            let (s0, e0) = (c + Vec2::from_angle(a.start) * a.radius, c + Vec2::from_angle(a.end) * a.radius);
            let end = match pick {
                Some(p) if p.dist(e0) < p.dist(s0) => e0,
                _ => s0,
            };
            Frame::new(c, along(c, end))
        }
        EntityKind::Circle(ci) => {
            let c = ci.center.xy();
            Frame::new(c, pick.map_or(0.0, |p| along(c, p)))
        }
        EntityKind::LwPolyline(pl) => {
            let pts: Vec<Vec2> = pl.vertices.iter().map(|v| v.p).collect();
            let n = pts.len();
            if n < 2 {
                return Err(EngineError::Other("This polyline doesn't define a coordinate system.".into()));
            }
            let segs = if pl.closed { n } else { n - 1 };
            let seg = |i: usize| (pts.get(i).copied().unwrap_or_default(), pts.get((i + 1) % n).copied().unwrap_or_default());
            let i = match pick {
                Some(p) => (0..segs)
                    .min_by(|x, y| {
                        let dist = |k: usize| {
                            let (a, b) = seg(k);
                            cadaddy_geom::Line::new(a, b).closest(p).dist(p)
                        };
                        dist(*x).total_cmp(&dist(*y))
                    })
                    .unwrap_or(0),
                None => 0,
            };
            let (a, b) = seg(i);
            Frame::new(a, along(a, b))
        }
        EntityKind::Text(t) => Frame::new(t.insert.xy(), t.rotation),
        EntityKind::MText(m) => Frame::new(m.insert.xy(), m.rotation),
        EntityKind::Insert(i) => Frame::new(i.insert.xy(), i.rotation),
        EntityKind::Point(pt) => Frame::new(pt.p.xy(), cur.angle),
        _ => return Err(EngineError::Other("This object doesn't define a coordinate system.".into())),
    };
    Ok(f)
}

fn save(s: &mut Session, name: &str) -> Result<Value> {
    if !valid_name(name) {
        return Err(EngineError::Other(format!("\"{name}\" is not a valid UCS name.")));
    }
    let mut f = s.ucs();
    let d = s.doc_mut()?;
    let replaced = d.ucss.iter().any(|u| u.name.eq_ignore_ascii_case(name));
    d.ucss.retain(|u| !u.name.eq_ignore_ascii_case(name));
    d.ucss.push(f.record(name));
    // The current UCS is now that named one.
    f.name = name.to_string();
    f.make_current(d);
    let mut r = report(&f);
    r["message"] = json!(if replaced { format!("UCS \"{name}\" replaced.") } else { format!("UCS \"{name}\" saved.") });
    Ok(r)
}

fn patterns(p: &Value) -> Vec<String> {
    let raw: Vec<String> = match p.get("names").or_else(|| p.get("name")) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(t)) => t.split(',').map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let pats: Vec<String> = raw.into_iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).take(MAX_NAMES).collect();
    if pats.is_empty() { vec!["*".into()] } else { pats }
}

fn delete(s: &mut Session, pats: &[String]) -> Result<Value> {
    let d = s.doc_mut()?;
    let gone: Vec<String> = d.ucss.iter().filter(|u| pats.iter().any(|p| wildcard(p, &u.name))).map(|u| u.name.clone()).collect();
    if gone.is_empty() {
        return Err(EngineError::Other("No matching UCS names found.".into()));
    }
    d.ucss.retain(|u| !gone.contains(&u.name));
    // The current UCS keeps its axes but is no longer a named one.
    let name = d.header.str("UCSNAME", "");
    if gone.iter().any(|g| g.eq_ignore_ascii_case(&name)) {
        d.header.set("UCSNAME", cadaddy_doc::HVal::Str(String::new()));
    }
    Ok(json!({ "deleted": gone, "message": format!("{} UCS(s) deleted.", gone.len()) }))
}

fn list(s: &mut Session, pats: &[String]) -> Result<Value> {
    let d = s.doc()?;
    let cur = Frame::current(d);
    let mut lines = vec![format!(
        "Current ucs name:  {}",
        if cur.is_world() && cur.name.is_empty() {
            "*WORLD*".to_string()
        } else if cur.name.is_empty() {
            "*NO NAME*".into()
        } else {
            cur.name.clone()
        }
    )];
    let mut rows = Vec::new();
    for u in d.ucss.iter().filter(|u| pats.iter().any(|p| wildcard(p, &u.name))) {
        let f = Frame::from_record(u);
        lines.push(format!(
            "Saved coordinate systems:\n{}\n    Origin = <{}, {}>, Angle = {}",
            u.name,
            u.origin.x,
            u.origin.y,
            f.as_ref().map_or_else(|| "(not in the plan)".to_string(), |f| format!("{:.4}", f.angle.to_degrees()))
        ));
        rows.push(json!({ "name": u.name, "origin": [u.origin.x, u.origin.y], "angle": f.map(|f| f.angle.to_degrees()) }));
    }
    Ok(json!({ "current": report(&cur), "ucs": rows, "message": lines.join("\n") }))
}

fn run_ucsicon(s: &mut Session, p: &Value) -> Result<Value> {
    let cur = s.doc()?.header.i64("UCSICON", 3);
    let v = match str_param(p, "option").unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "on" => cur | 1,
        "off" => cur & !1,
        "origin" | "or" => cur | 2,
        "noorigin" | "n" => cur & !2,
        _ => return Err(bad("ucsicon", "option must be on, off, origin or noorigin")),
    };
    s.doc_mut()?.header.set_i64("UCSICON", v);
    s.touch();
    Ok(json!({ "ucsicon": v }))
}

// ---------------- prompts ----------------

#[derive(Clone, Copy, Default, PartialEq)]
enum Stage {
    #[default]
    Main,
    XAxis(Vec2),
    ZAngle,
    Object,
    Named,
    NamedRestore,
    NamedSave,
    NamedDelete,
}

/// UCS typed: "Specify origin of UCS or [Face/NAmed/OBject/Previous/View/World/X/Y/Z/ZAxis]
/// <World>:", a point then "Specify point on X-axis or <Accept>:".
#[derive(Default)]
struct UcsM {
    stage: Stage,
}

/// Run `f` and echo its message, ending the command.
fn done(s: &mut Session, f: impl FnOnce(&mut Session) -> Result<Value>) -> Result<Step> {
    let v = f(s)?;
    if let Some(m) = v.get("message").and_then(Value::as_str) {
        for l in m.lines() {
            s.echo(l.to_string());
        }
    }
    Ok(Step::Done)
}

impl Interactive for UcsM {
    fn name(&self) -> &'static str {
        "UCS"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.stage {
            Stage::Main => Prompt::new("Specify origin of UCS", Accept::POINT)
                .kw(&["Face", "NAmed", "OBject", "Previous", "View", "World", "X", "Y", "Z", "ZAxis"])
                .default("World"),
            Stage::XAxis(o) => Prompt::new("Specify point on X-axis", Accept::POINT).kw(&["Accept"]).default("Accept").base(o),
            Stage::ZAngle => Prompt::new("Specify rotation angle about Z axis", Accept::POINT_OR_NUMBER).default("90").base(s.ucs().origin),
            Stage::Object => Prompt::new("Select object to align UCS", Accept::SELECT),
            Stage::Named => Prompt::new("Enter an option", Accept::TEXT).kw(&["Restore", "Save", "Delete", "?"]),
            Stage::NamedRestore => Prompt::new("Enter name of UCS to restore", Accept::TEXT),
            Stage::NamedSave => Prompt::new("Enter name to save current UCS", Accept::TEXT),
            Stage::NamedDelete => Prompt::new("Enter UCS name(s) to delete", Accept::TEXT).default("none"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_ascii_lowercase()),
            _ => None,
        };
        match self.stage {
            Stage::Main => match (&i, text.as_deref()) {
                (Input::Point(p), _) => {
                    self.stage = Stage::XAxis(*p);
                    Ok(Step::Continue)
                }
                (Input::Enter, _) | (_, Some("w" | "world")) => done(s, |s| run_ucs(s, &json!({ "option": "world" }))),
                (_, Some("p" | "previous")) => done(s, |s| run_ucs(s, &json!({ "option": "previous" }))),
                (_, Some("v" | "view")) => done(s, |s| run_ucs(s, &json!({ "option": "view" }))),
                (_, Some("z")) => {
                    self.stage = Stage::ZAngle;
                    Ok(Step::Continue)
                }
                (_, Some("ob" | "object")) => {
                    self.stage = Stage::Object;
                    Ok(Step::Continue)
                }
                (_, Some("na" | "named")) => {
                    self.stage = Stage::Named;
                    Ok(Step::Continue)
                }
                (_, Some("x" | "y" | "za" | "zaxis" | "f" | "face")) => {
                    Err(EngineError::Other("Only UCSs in the plan (origin and rotation about Z) are available so far.".into()))
                }
                _ => Err(EngineError::Other("Invalid option keyword.".into())),
            },
            Stage::XAxis(o) => {
                let mut p = json!({ "option": "origin", "origin": [o.x, o.y] });
                if let Input::Point(x) = i {
                    p["xpoint"] = json!([x.x, x.y]);
                }
                done(s, |s| run_ucs(s, &p))
            }
            Stage::ZAngle => {
                let deg = match &i {
                    Input::Enter => 90.0,
                    // A point: the angle from the UCS origin to it, from the X axis.
                    Input::Point(p) => {
                        let f = s.ucs();
                        (f.origin.angle_to(*p) - f.angle).to_degrees()
                    }
                    Input::Text(t) => {
                        s.angle_settings().amount(t).map(f64::to_degrees).ok_or_else(|| EngineError::Other("Requires a valid angle.".into()))?
                    }
                    _ => return Err(EngineError::Other("Requires a valid angle.".into())),
                };
                done(s, |s| run_ucs(s, &json!({ "option": "z", "angle": deg })))
            }
            Stage::Object => match i {
                Input::Pick(hs) => match hs.first() {
                    Some(h) => {
                        let pick = s.cursor;
                        let h = h.hex();
                        done(s, |s| run_ucs(s, &json!({ "option": "object", "handle": h, "pick": [pick.x, pick.y] })))
                    }
                    None => Ok(Step::Continue),
                },
                Input::Enter => Ok(Step::Done),
                _ => Err(EngineError::Other("Select an object.".into())),
            },
            Stage::Named => {
                self.stage = match text.as_deref() {
                    Some("r" | "restore") => Stage::NamedRestore,
                    Some("s" | "save") => Stage::NamedSave,
                    Some("d" | "delete") => Stage::NamedDelete,
                    Some("?") => return done(s, |s| list(s, &["*".to_string()])),
                    None if i == Input::Enter => return Ok(Step::Done),
                    _ => return Err(EngineError::Other("Invalid option keyword.".into())),
                };
                Ok(Step::Continue)
            }
            Stage::NamedRestore | Stage::NamedSave | Stage::NamedDelete => {
                let Some(name) = (match &i {
                    Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_string()).filter(|t| !t.is_empty()),
                    _ => None,
                }) else {
                    return Ok(Step::Done);
                };
                match self.stage {
                    Stage::NamedRestore => done(s, |s| run_ucs(s, &json!({ "option": "restore", "name": name }))),
                    Stage::NamedSave => done(s, |s| run_ucs(s, &json!({ "option": "save", "name": name }))),
                    _ if name.eq_ignore_ascii_case("none") => Ok(Step::Done),
                    _ => done(s, |s| run_ucs(s, &json!({ "option": "delete", "names": name }))),
                }
            }
        }
    }
}
