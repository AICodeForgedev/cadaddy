//! The 3D furniture and fixture library ([`cadaddy_doc::library::models3d`]): place a model as a
//! block (its plan view and 3D body), list the models, and give placed 2D library symbols their 3D
//! twins at mounting height. Models are drawn in inches and placed at the drawing's units.

use std::sync::Arc;

use cadaddy_doc::library::models3d::{self, Model};
use cadaddy_doc::{Block, EntityKind, Handle, Insert};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("models3d.place", "3D Library...", run_place)
            .menu(&["Insert", "3D Library..."])
            .alias(&["lib3d"])
            .params("{name (e.g. \"M3_DESK\"; see models3d.list), at: [x, y] or [x, y, z] (the floor under it), rotation? (degrees)} → {handle, block}: the model's plan view and 3D body, at the drawing's units")
            .interactive(|_| Ok(Box::new(PlaceM::default()))),
        CommandSpec::new("models3d.list", "List 3D Models", run_list)
            .params("{query?, category?} → {models: [{name, label, category, mountHeight (inches), twinOf?, defined}]}")
            .noundo(),
        CommandSpec::new("models3d.twins", "Add 3D Twins", run_twins)
            .menu(&["Insert", "Add 3D Twins to Symbols"])
            .params("{handles? (placed library symbols; default every one in the current space)} → {added, twins: [handles]}: each symbol with a 3D model gets its body at the mounting height, once")
            .interactive(|_| Ok(Box::new(super::machines::SelectRun::new("models3d.twins", "ADD 3D TWINS")))),
    ]
}

fn other(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}

/// Drawing units per inch (INSUNITS; unitless: inches unless the drawing is metric).
fn units_per_inch(s: &Session) -> Result<f64> {
    let d = s.doc()?;
    Ok(match d.header.i64("INSUNITS", 0) {
        1 => 1.0,
        2 => 1.0 / 12.0,
        4 => 25.4,
        5 => 2.54,
        6 => 0.0254,
        14 => 2.54 / 10.0,
        _ if d.header.i64("MEASUREMENT", 0) == 1 => 25.4,
        _ => 1.0,
    })
}

/// Define the model's block (plan view and body) unless the drawing has one of that name.
fn define(s: &mut Session, name: &str, entities: Vec<cadaddy_doc::Entity>, description: String) -> Result<String> {
    let d = s.doc_mut()?;
    if let Some(b) = d.block(name) {
        return Ok(b.name.clone());
    }
    let mut blk = Block::new(name);
    blk.base = Vec3::ZERO;
    blk.description = description;
    for mut e in entities {
        e.handle = d.new_handle();
        blk.entities.push(e);
    }
    d.blocks.insert(name.to_string(), Arc::new(blk));
    Ok(name.to_string())
}

fn find(name: &str) -> Result<&'static Model> {
    models3d::model(name).ok_or_else(|| other(format!("No 3D model named \"{}\" (see models3d.list).", name.trim())))
}

fn insert(s: &mut Session, block: &str, at: Vec3, rotation: f64, scale: f64) -> Result<Handle> {
    if !(at.x.is_finite() && at.y.is_finite() && at.z.is_finite() && rotation.is_finite()) {
        return Err(other("Requires a valid point and rotation."));
    }
    s.add_entity(EntityKind::Insert(Insert {
        block: block.to_string(),
        insert: at,
        scale: Vec3::new(scale, scale, scale),
        rotation,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    }))
}

fn place(s: &mut Session, m: &Model, at: Vec3, rotation_deg: f64) -> Result<Value> {
    let k = units_per_inch(s)?;
    let block = define(s, m.name, m.entities(), format!("{} (CADaddy 3D library)", m.label))?;
    let h = insert(s, &block, at, rotation_deg.to_radians(), k)?;
    Ok(json!({ "handle": h.hex(), "block": block }))
}

fn run_place(s: &mut Session, p: &Value) -> Result<Value> {
    let m = find(str_param(p, "name").ok_or_else(|| bad("models3d.place", "`name` is required"))?)?;
    let at = p.get("at").and_then(Value::as_array).ok_or_else(|| bad("models3d.place", "`at` is [x, y] or [x, y, z]"))?;
    let c = |i: usize| at.get(i).and_then(Value::as_f64);
    let (Some(x), Some(y)) = (c(0), c(1)) else { return Err(bad("models3d.place", "`at` is [x, y] or [x, y, z]")) };
    place(s, m, Vec3::new(x, y, c(2).unwrap_or(0.0)), f64_or(p, "rotation", 0.0))
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let query = str_param(p, "query").unwrap_or("");
    let category = str_param(p, "category");
    let d = s.doc().ok();
    let list: Vec<Value> = models3d::models()
        .iter()
        .filter(|m| m.matches(query) && category.is_none_or(|c| m.category.eq_ignore_ascii_case(c)))
        .map(|m| {
            json!({
                "name": m.name, "label": m.label, "category": m.category, "mountHeight": m.mount_in,
                "twinOf": m.twin_of, "defined": d.is_some_and(|d| d.block(m.name).is_some()),
            })
        })
        .collect();
    Ok(json!({ "models": list }))
}

/// A twin's block: the model's body only (the symbol already draws the plan).
fn twin_block(s: &mut Session, m: &Model) -> Result<String> {
    define(s, &format!("{}_BODY", m.name), m.body(), format!("{} 3D body (CADaddy 3D library)", m.label))
}

fn run_twins(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = if p.get("handles").is_some() || p.get("handle").is_some() {
        targets(s, p)?
    } else {
        let d = s.doc()?;
        d.space(&s.space()).map(|st| st.iter().map(|e| e.handle).collect()).unwrap_or_default()
    };
    let k = units_per_inch(s)?;
    // The symbols with a model, and where they are.
    let mut work: Vec<(&'static Model, Vec3, f64)> = Vec::new();
    {
        let d = s.doc()?;
        for h in &hs {
            let Some(e) = d.entity(*h) else { continue };
            let EntityKind::Insert(i) = &e.kind else { continue };
            if let Some(m) = models3d::twin(&i.block) {
                work.push((m, i.insert, i.rotation));
            }
        }
    }
    let mut made = Vec::new();
    for (m, at, rot) in work {
        let block = twin_block(s, m)?;
        // Once: a twin already at that place is kept.
        let d = s.doc()?;
        let there = d.space(&s.space()).is_some_and(|st| {
            st.iter().any(|e| {
                matches!(&e.kind, EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(&block) && Vec2::new(i.insert.x - at.x, i.insert.y - at.y).len() < 1e-6 * k.max(1.0))
            })
        });
        if there {
            continue;
        }
        made.push(insert(s, &block, at, rot, k)?.hex());
    }
    Ok(json!({ "added": made.len(), "twins": made }))
}

/// Typed: the model's name, then where it goes.
#[derive(Default)]
struct PlaceM {
    model: Option<&'static Model>,
}

impl Interactive for PlaceM {
    fn name(&self) -> &'static str {
        "3D LIBRARY"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.model {
            None => Prompt::new("Enter 3D model name (e.g. M3_DESK; ? to list)", Accept::TEXT),
            Some(m) => Prompt::new(format!("Specify insertion point for {}", m.label), Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.model, i) {
            (_, Input::Cancel) => Ok(Step::Cancel),
            (None, Input::Text(t)) if t.trim() == "?" => {
                let names: Vec<&str> = models3d::models().iter().map(|m| m.name).collect();
                s.echo(names.join(", "));
                Ok(Step::Continue)
            }
            (None, Input::Text(t)) => {
                self.model = Some(find(&t)?);
                Ok(Step::Continue)
            }
            (None, Input::Enter) => Ok(Step::Cancel),
            (Some(m), Input::Point(p)) => {
                place(s, m, Vec3::new(p.x, p.y, 0.0), 0.0)?;
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
}
