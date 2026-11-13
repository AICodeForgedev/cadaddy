//! A built wall moved, resized or deleted, or a wall added, by command (Q6613): `wall.edit` and
//! `wall.edits`.
//!
//! **Edits survive a rebuild.** The plan's line pairs give every wall's geometry, so moving a
//! built wall's mesh would be undone by the next `model.build`. An edit is kept in the spec's
//! `wall_edits` ([`cadaddy_walk3d::walledits`]), beside the wall list a re-read of the plans
//! rewrites, and every build applies it to the walls the plan gives ([`super::wallgen`]): the
//! wall on its `from` line taken away, its wall built on its `to` line. A change to the spec's
//! wall list makes the same edits (`bspec.set`).
//!
//! **Nothing changes silently.** A wall an edit places carries its provenance (not drawn there:
//! `status:inferred` and the edit as its basis), the edit's id (`edit:WE-1`) and, when it moved
//! a wall, where the plan built it (`edited_from:`): the design's place beside the corrected one.

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::walledits::{self, WallEdit};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Characters of a reason kept; largest coordinate and move taken (hostile input).
const MAX_TEXT: usize = 200;
const MAX_COORD: f64 = 1.0e9;
const MAX_MOVE_FT: f64 = 10_000.0;
/// Thickest wall an edit may give (inches) and its tallest (feet).
const MAX_THICK_IN: f64 = 120.0;
const MAX_HEIGHT_FT: f64 = 200.0;
/// Mesh points looked at to find a built wall's centreline.
const MAX_POINTS: usize = 200_000;
/// Edits listed at most.
const MAX_LIST: usize = 500;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("wall.edit", "Wall", run_edit).params(
            "{op: move|resize|delete|add, handle?: hex (a wall the build made: move, resize, delete), level?: name (with `from`, or an added wall's), from?: [[x, y], [x, y]] (the centreline of the built wall changed, plan units: instead of `handle`), to?: [[x, y], [x, y]] (where the wall goes: its centreline, plan units, built to these ends; move, resize, add), delta_ft?: [dx, dy] (move: by this much, feet), wall_type?: name (a spec wall type; default the wall's own), thickness_in?: inches (where no type gives one), height_ft?: feet (above its floor), reason?: text, by?: user|agent, rebuild?: bool (default true when the model is built: model.build again)} → {edit, status: applied, walls: [handle] (the walls it built, after a rebuild), rebuilt, rebuild_error?}: kept in the spec's wall_edits, applied by every build whatever the plan's line pairs build there (a wall misread from the plan moved, a wall the reader built where there is none deleted, a wall it missed added); wall.edits lists them and takes one back",
        ),
        CommandSpec::new("wall.edits", "Walls", run_edits).params(
            "{level?: name, revert?: id (an edit, \"WE-2\": taken back, the wall as the plan gives it again), rebuild?: bool (with revert; default true when the model is built)} → {edits: [{id, op, level, from?, to?, wall_type?, via (wall list|wall.edit), by, reason?, basis}], total, reverted?, rebuilt?}",
        ),
    ]
}

fn text(cmd: &str, p: &Value, key: &str) -> Result<Option<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) if t.chars().count() <= MAX_TEXT => Ok(Some(t.trim().to_string())),
        Some(Value::String(_)) => Err(bad(cmd, format!("`{key}` is at most {MAX_TEXT} characters"))),
        Some(_) => Err(bad(cmd, format!("`{key}` must be text"))),
    }
}

fn by_param(cmd: &str, p: &Value) -> Result<String> {
    match text(cmd, p, "by")?.as_deref() {
        None | Some("") | Some("user") => Ok("user".into()),
        Some("agent") => Ok("agent".into()),
        Some(o) => Err(bad(cmd, format!("`by` is user or agent, not {o:?}"))),
    }
}

/// `[x, y]` of finite numbers at most `max` across.
fn point(cmd: &str, key: &str, v: &Value, max: f64) -> Result<[f64; 2]> {
    let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, format!("`{key}`: a point is [x, y]")))?;
    let n = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite() && x.abs() <= max);
    match (n(0), n(1)) {
        (Some(x), Some(y)) => Ok([x, y]),
        _ => Err(bad(cmd, format!("`{key}`: two numbers, each at most {max} across"))),
    }
}

/// `[[x, y], [x, y]]`: a centreline of some length.
fn line(cmd: &str, p: &Value, key: &str) -> Result<Option<[[f64; 2]; 2]>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, format!("`{key}` is [[x, y], [x, y]]")))?;
            let (Some(p0), Some(p1)) = (a.first(), a.get(1)) else { return Err(bad(cmd, format!("`{key}` is [[x, y], [x, y]]"))) };
            let (p0, p1) = (point(cmd, key, p0, MAX_COORD)?, point(cmd, key, p1, MAX_COORD)?);
            if (p0[0] - p1[0]).hypot(p0[1] - p1[1]) <= 1e-9 {
                return Err(bad(cmd, format!("`{key}`: its two ends are the same point")));
            }
            Ok(Some([p0, p1]))
        }
    }
}

fn upf(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

/// A built wall's centreline on its level's plan (the build's stacking taken off) and its
/// thickness: the long axis of its mesh's plan points.
pub(crate) fn built_line(e: &Entity) -> Option<([f64; 2], [f64; 2], f64)> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let pts: Vec<Vec2> = m.vertices.iter().take(MAX_POINTS).map(|v| Vec2::new(v.x, v.y)).filter(|p| p.is_finite()).collect();
    if pts.len() < 4 {
        return None;
    }
    let n = pts.len() as f64;
    let c = pts.iter().fold(Vec2::ZERO, |s, p| s + *p) * (1.0 / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in &pts {
        let d = *p - c;
        sxx += d.x * d.x;
        syy += d.y * d.y;
        sxy += d.x * d.y;
    }
    let ang = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let u = Vec2::new(ang.cos(), ang.sin());
    let nn = u.perp();
    let (mut s0, mut s1, mut t0, mut t1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for p in &pts {
        let d = *p - c;
        let (s, t) = (u.dot(d), nn.dot(d));
        s0 = s0.min(s);
        s1 = s1.max(s);
        t0 = t0.min(t);
        t1 = t1.max(t);
    }
    if !(s1 - s0 > 1e-9 && s0.is_finite() && t0.is_finite()) {
        return None;
    }
    let off = super::bspec::stacked_by(e);
    let mid = c + nn * ((t0 + t1) / 2.0) - off;
    let (a, b) = (mid + u * s0, mid + u * s1);
    Some(([a.x, a.y], [b.x, b.y], t1 - t0))
}

/// The wall a command names: by `handle` (a wall the build made), else `level` and `from`.
fn target(cmd: &str, d: &Drawing, sp: &BuildingSpec, p: &Value) -> Result<(String, [[f64; 2]; 2], Option<String>)> {
    if let Some(h) = p.get("handle").filter(|v| !v.is_null()) {
        let h = h.as_str().and_then(Handle::parse_hex).or_else(|| h.as_u64().map(Handle)).ok_or_else(|| bad(cmd, "`handle` is a hex handle"))?;
        let e = d.entity(h).ok_or_else(|| bad(cmd, format!("no object {}", h.hex())))?;
        if super::wallgen::made_as(e).as_deref() != Some("wall") {
            return Err(bad(cmd, format!("{} is not a wall the build made", h.hex())));
        }
        let level = super::bspec::made_level(e).filter(|l| !l.trim().is_empty()).ok_or_else(|| bad(cmd, "the wall has no level"))?;
        let (a, b, _) = built_line(e).ok_or_else(|| bad(cmd, "the wall's mesh gives no centreline"))?;
        let ty = super::bimlook::made_tag(e, "type").map(str::to_string);
        return Ok((level, [a, b], ty));
    }
    let from = line(cmd, p, "from")?.ok_or_else(|| bad(cmd, "name the wall: `handle`, or `level` and `from`"))?;
    let level = level_named(cmd, sp, p)?;
    Ok((level, from, None))
}

fn level_named(cmd: &str, sp: &BuildingSpec, p: &Value) -> Result<String> {
    let l = text(cmd, p, "level")?.filter(|l| !l.is_empty()).ok_or_else(|| bad(cmd, "`level` is required"))?;
    sp.levels.iter().find(|x| x.name.eq_ignore_ascii_case(&l)).map(|x| x.name.clone()).ok_or_else(|| {
        let names: Vec<&str> = sp.levels.iter().take(20).map(|x| x.name.as_str()).collect();
        bad(cmd, format!("no level {l:?} in the spec ({})", names.join(", ")))
    })
}

fn number(cmd: &str, p: &Value, key: &str, max: f64) -> Result<Option<f64>> {
    let v = size_param(cmd, p, key, false)?;
    match v {
        Some(x) if x > max => Err(bad(cmd, format!("`{key}` is at most {max}"))),
        _ => Ok(v),
    }
}

/// `model.build` again when asked (by default when the model is built).
fn rebuild(cmd: &str, s: &mut Session, p: &Value, r: &mut Value) -> Result<()> {
    let built = super::buildrec::read(s.doc()?).is_some() || s.state().is_ok_and(|st| st.wallgen.is_some());
    if bool_param(cmd, p, "rebuild")?.unwrap_or(built) {
        match s.execute("model.build", &json!({"echo": false})) {
            Ok(_) => r["rebuilt"] = json!(true),
            Err(err) => {
                r["rebuilt"] = json!(false);
                r["rebuild_error"] = json!(err.to_string());
            }
        }
    } else {
        r["rebuilt"] = json!(false);
    }
    Ok(())
}

fn edit_json(e: &WallEdit) -> Value {
    let mut v = serde_json::to_value(e).unwrap_or(Value::Null);
    v["basis"] = json!(e.basis());
    v
}

fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "wall.edit";
    bool_param(CMD, p, "rebuild")?;
    let (mut sp, _) = super::bspec::current(s, CMD)?;
    if sp.wall_edits.len() >= walledits::MAX_EDITS {
        return Err(bad(CMD, format!("the spec holds {} wall edits already: wall.edits revert takes some back", walledits::MAX_EDITS)));
    }
    let op = text(CMD, p, "op")?.unwrap_or_default().to_ascii_lowercase();
    let upf = upf(&sp);
    let mut e = WallEdit { op: op.clone(), ..WallEdit::default() };
    let mut own_type = None;
    match op.as_str() {
        "move" | "resize" | "delete" => {
            let (level, from, ty) = target(CMD, s.doc()?, &sp, p)?;
            if !super::wallgen::wall_on_line(s.doc()?, &sp, &level, from) {
                return Err(bad(
                    CMD,
                    format!(
                        "no wall of {level}'s plan (or of an earlier wall edit) is built on that line: the wall is one a later step of the build adds"
                    ),
                ));
            }
            e.level = level;
            e.from = Some(from);
            own_type = ty;
            if op != "delete" {
                let to = line(CMD, p, "to")?;
                let delta = match p.get("delta_ft").filter(|v| !v.is_null()) {
                    Some(v) => Some(point(CMD, "delta_ft", v, MAX_MOVE_FT)?),
                    None => None,
                };
                e.to = Some(match (to, delta) {
                    (Some(t), None) => t,
                    (None, Some(dd)) if op == "move" => {
                        let (dx, dy) = (dd[0] * upf, dd[1] * upf);
                        [[from[0][0] + dx, from[0][1] + dy], [from[1][0] + dx, from[1][1] + dy]]
                    }
                    (Some(_), Some(_)) => return Err(bad(CMD, "give `to` or `delta_ft`, not both")),
                    _ => return Err(bad(CMD, if op == "move" { "give `to` or `delta_ft`" } else { "give `to`: the wall's new ends" })),
                });
            }
        }
        "add" => {
            e.level = level_named(CMD, &sp, p)?;
            e.to = Some(line(CMD, p, "to")?.ok_or_else(|| bad(CMD, "`to` (the new wall's centreline) is required"))?);
        }
        "" => return Err(bad(CMD, "`op` is required: move, resize, delete or add")),
        o => return Err(bad(CMD, format!("`op` is move, resize, delete or add, not {o:?}"))),
    }
    if let Some(t) = text(CMD, p, "wall_type")?.filter(|t| !t.is_empty()) {
        let found = sp.wall_types.iter().find(|x| x.name.eq_ignore_ascii_case(&t)).ok_or_else(|| {
            let names: Vec<&str> = sp.wall_types.iter().take(30).map(|x| x.name.as_str()).collect();
            bad(CMD, format!("no wall type {t:?} in the spec ({})", names.join(", ")))
        })?;
        e.wall_type.clone_from(&found.name);
    } else if op != "delete" {
        e.wall_type = own_type.unwrap_or_default();
    }
    e.thickness_in = number(CMD, p, "thickness_in", MAX_THICK_IN)?;
    e.height_ft = number(CMD, p, "height_ft", MAX_HEIGHT_FT)?;
    e.reason = text(CMD, p, "reason")?.unwrap_or_default();
    e.by = by_param(CMD, p)?;
    e.via = "wall.edit".into();
    e.id = walledits::next_id(&sp.wall_edits);
    sp.wall_edits.push(e.clone());
    s.doc_mut()?.building_spec = Some(sp.to_json()).into();
    let mut r = json!({"edit": edit_json(&e), "status": "applied"});
    rebuild(CMD, s, p, &mut r)?;
    let tag = format!("{}{}", super::wallgen::EDIT_TAG, e.id);
    let walls: Vec<String> = s
        .doc()?
        .model
        .iter()
        .filter(|x| super::wallgen::made_as(x).as_deref() == Some("wall") && x.common.xdata.iter().any(|t| t.value == tag))
        .map(|x| x.handle.hex())
        .collect();
    r["walls"] = json!(walls);
    s.echo(format!("{} {} wall on {} — kept in the building spec's wall edits; every build applies it.", e.id, e.op, e.level));
    Ok(r)
}

fn run_edits(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "wall.edits";
    bool_param(CMD, p, "rebuild")?;
    let (mut sp, _) = super::bspec::current(s, CMD)?;
    let mut r = json!({});
    if let Some(id) = text(CMD, p, "revert")?.filter(|x| !x.is_empty()) {
        let i = sp.wall_edits.iter().position(|e| e.id.eq_ignore_ascii_case(&id)).ok_or_else(|| bad(CMD, format!("no wall edit {id}")))?;
        let e = sp.wall_edits.remove(i);
        s.doc_mut()?.building_spec = Some(sp.to_json()).into();
        r["reverted"] = edit_json(&e);
        rebuild(CMD, s, p, &mut r)?;
        s.echo(format!("{} taken back: the wall as the plan gives it.", e.id));
    }
    let level = text(CMD, p, "level")?.filter(|l| !l.is_empty());
    let list: Vec<Value> =
        sp.wall_edits.iter().filter(|e| level.as_ref().is_none_or(|l| e.level.eq_ignore_ascii_case(l))).take(MAX_LIST).map(edit_json).collect();
    r["total"] = json!(list.len());
    r["edits"] = json!(list);
    Ok(r)
}
