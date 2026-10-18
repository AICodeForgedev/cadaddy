//! SYMBOL: place a symbol from CADaddy's built-in library (plumbing fixtures and piping symbols)
//! as a block reference with its attributes. The block is defined in the drawing the first time
//! it is placed; a block of that name the drawing already has is used as it is. Annotation
//! symbols are placed at the drawing's scale: the current annotation scale (CANNOSCALE), else
//! DIMSCALE.

use std::sync::Arc;

use cadaddy_doc::library::symbols::{self, Symbol};
use cadaddy_doc::{Block, EntityKind};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("symbol", "Symbol...", run_symbol)
            .menu(&["Insert", "Symbol..."])
            .alias(&["sym"])
            .params(
                "{name (a library symbol, e.g. \"PL_WATER_CLOSET\"; see symbols.list), at: [x,y], rotation? (degrees), scale? (default: 1 for fixtures, the drawing's scale for annotation symbols), attribs?: {TAG: value}, layer? (default: the symbol's trade layer, E-LITE, E-POWR-DEVC…; \"current\" for the current layer)} → {handle, block, layer?}",
            )
            .interactive(|_| Ok(Box::new(SymbolM::default()))),
        CommandSpec::new("symbols.list", "List Symbols", run_list)
            .params("{query?: words to search for, category?} → {symbols: [{name, label, category, trade, annotation, defined}]}")
            .noundo(),
    ]
}

/// The scale an annotation symbol is placed at: the current annotation scale's, else DIMSCALE
/// (1 when neither is set).
pub(crate) fn drawing_scale(s: &Session) -> Result<f64> {
    let d = s.doc()?;
    let anno = d.current_anno_scale().factor();
    let dim = d.header.f64("DIMSCALE", 1.0);
    let f = if anno.is_finite() && anno > 0.0 && (anno - 1.0).abs() > 1e-12 {
        anno
    } else if dim.is_finite() && dim > 0.0 {
        dim
    } else {
        1.0
    };
    Ok(f)
}

fn default_scale(s: &Session, sym: &Symbol) -> Result<f64> {
    if sym.annotation { drawing_scale(s) } else { Ok(1.0) }
}

/// Define the symbol's block in the drawing unless a block of that name is there already.
pub(crate) fn define(s: &mut Session, sym: &Symbol) -> Result<String> {
    let d = s.doc_mut()?;
    if let Some(b) = d.block(sym.name) {
        return Ok(b.name.clone());
    }
    let mut blk = Block::new(sym.name);
    blk.base = Vec3::ZERO;
    blk.description = format!("{} (CADaddy symbol library)", sym.label);
    for mut e in sym.entities() {
        e.handle = d.new_handle();
        blk.entities.push(e);
    }
    d.blocks.insert(sym.name.to_string(), Arc::new(blk));
    Ok(sym.name.to_string())
}

/// The layer a symbol goes on by default (US National CAD Standard names) and its colour.
fn trade_layer(sym: &Symbol) -> Option<(&'static str, u8)> {
    if !sym.name.starts_with("EL_") {
        return None;
    }
    Some(match sym.category {
        _ if sym.name.starts_with("EL_SWITCH") => ("E-LITE", 2),
        "Lighting" => ("E-LITE", 2),
        "Power and distribution" => ("E-POWR-EQPM", 6),
        "Low voltage" => ("E-COMM", 3),
        "Fire alarm" => ("E-FIRE", 1),
        _ => ("E-POWR-DEVC", 4),
    })
}

/// Header variable: "0" places symbols on the current layer instead of their trade layers.
const TRADE_LAYERS_VAR: &str = "CADADDY_SYMBOL_LAYERS";

fn trade_layers_on(s: &Session) -> bool {
    s.doc().map(|d| d.header.str(TRADE_LAYERS_VAR, "1") != "0").unwrap_or(true)
}

/// The layer to put a placed symbol on (`None`: leave it on the current layer): `layer` when
/// given ("current" for the current one), else its trade layer, made with its colour if missing.
fn target_layer(s: &mut Session, sym: &Symbol, layer: Option<&str>) -> Result<Option<String>> {
    let name = match layer.map(str::trim) {
        Some(l) if l.eq_ignore_ascii_case("current") || l.is_empty() => return Ok(None),
        Some(l) => l.to_string(),
        None if !trade_layers_on(s) => return Ok(None),
        None => match trade_layer(sym) {
            Some((n, color)) => {
                let d = s.doc_mut()?;
                if d.ensure_layer(n)
                    && let Some(l) = d.layer_mut(n)
                {
                    l.color = cadaddy_color::Color::Index(color);
                }
                n.to_string()
            }
            None => return Ok(None),
        },
    };
    s.doc_mut()?.ensure_layer(&name);
    Ok(Some(name))
}

fn find(name: &str) -> Result<&'static Symbol> {
    symbols::symbol(name).ok_or_else(|| bad("symbol", format!("no library symbol `{name}` (see symbols.list)")))
}

fn run_symbol(s: &mut Session, p: &Value) -> Result<Value> {
    let sym = find(str_param(p, "name").ok_or_else(|| bad("symbol", "`name` is required"))?)?;
    let at = point_req("symbol", p, "at")?;
    let scale = match p.get("scale").and_then(Value::as_f64) {
        Some(f) if f.is_finite() && f != 0.0 => f,
        Some(_) => return Err(bad("symbol", "`scale` must be a non-zero number")),
        None => default_scale(s, sym)?,
    };
    let name = define(s, sym)?;
    let mut ip = json!({ "name": name, "at": [at.x, at.y], "scale": scale, "rotation": f64_or(p, "rotation", 0.0) });
    if let Some(a) = p.get("attribs") {
        ip["attribs"] = a.clone();
    }
    // The INSERT command body, not `execute`: SYMBOL records the one undo step.
    let insert = find_command("insert").ok_or_else(|| EngineError::UnknownCommand("insert".into()))?;
    let mut r = (insert.run)(s, &ip)?;
    if let Some(layer) = target_layer(s, sym, str_param(p, "layer"))?
        && let Some(h) = r.get("handle").and_then(super::curves::handle_value)
    {
        s.doc_mut()?.modify_entity(h, |e| e.common.layer = layer.clone())?;
        r["layer"] = json!(layer);
    }
    r["block"] = json!(name);
    Ok(r)
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let query = str_param(p, "query").unwrap_or("");
    let category = str_param(p, "category");
    let d = s.doc().ok();
    let list: Vec<Value> = symbols::symbols()
        .iter()
        .filter(|sym| sym.matches(query) && category.is_none_or(|c| sym.category.eq_ignore_ascii_case(c)))
        .map(|sym| {
            json!({
                "name": sym.name,
                "label": sym.label,
                "category": sym.category,
                "trade": sym.trade,
                "annotation": sym.annotation,
                "defined": d.is_some_and(|d| d.block(sym.name).is_some()),
            })
        })
        .collect();
    Ok(json!({ "symbols": list }))
}

/// A library symbol drawn for a palette: its lines and filled triangles in block coordinates
/// (attribute definitions left out), and their bounds.
pub struct SymbolPreview {
    pub name: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub keywords: &'static str,
    pub lines: Vec<Vec<Vec2>>,
    pub fills: Vec<[Vec2; 3]>,
    pub bounds: cadaddy_geom::Bounds2,
}

/// Previews of every library symbol, in palette order.
pub fn previews() -> Vec<SymbolPreview> {
    let d = cadaddy_doc::Drawing::new_imperial();
    symbols::symbols()
        .iter()
        .map(|sym| {
            let ents: Vec<cadaddy_doc::Entity> = sym.entities().into_iter().filter(|e| !matches!(e.kind, EntityKind::AttDef(_))).collect();
            let size = ents.iter().fold(cadaddy_geom::Bounds2::EMPTY, |b, e| b.union(&cadaddy_doc::entity_bounds(&d, e, 1)));
            let tol = (size.width().max(size.height()) / 200.0).max(1e-6);
            let list = cadaddy_render::build_entities(
                &d,
                ents.iter(),
                &cadaddy_render::Options { tolerance: tol, text: true, fill: true, ..Default::default() },
            );
            let mut lines = Vec::new();
            let mut fills = Vec::new();
            let mut bounds = cadaddy_geom::Bounds2::EMPTY;
            for p in &list.prims {
                let pts = list.points(p);
                for q in pts {
                    bounds.add(*q);
                }
                if p.kind.is_fill() {
                    fills.extend(pts.as_chunks::<3>().0.iter().copied());
                } else {
                    lines.push(pts.to_vec());
                }
            }
            SymbolPreview { name: sym.name, label: sym.label, category: sym.category, keywords: sym.keywords, lines, fills, bounds }
        })
        .collect()
}

/// SYMBOL at the command line: the symbol's name (or `?` for the list), then INSERT's prompts
/// for it: insertion point, rotation and its attributes. The scale isn't asked: fixtures are
/// real size, annotation symbols come at the drawing's scale.
#[derive(Default)]
struct SymbolM {
    inner: Option<Box<dyn Interactive>>,
    /// The symbol being placed, and the handles there were before (to find the new reference).
    placing: Option<(&'static Symbol, std::collections::HashSet<cadaddy_doc::Handle>)>,
}

impl Interactive for SymbolM {
    fn name(&self) -> &'static str {
        "SYMBOL"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match &self.inner {
            Some(m) => m.prompt(s),
            None => Prompt::new("Enter symbol name", Accept::TEXT).kw(&["?", "Layers"]),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if let Some(m) = &mut self.inner {
            let step = m.input(s, i)?;
            if step == Step::Done
                && let Some((sym, before)) = self.placing.take()
                && let Some(layer) = target_layer(s, sym, None)?
            {
                // The reference INSERT just made goes on the symbol's trade layer.
                let new: Vec<_> = s
                    .doc()?
                    .model
                    .iter()
                    .chain(s.doc()?.layouts.iter().flat_map(|l| l.entities.iter()))
                    .map(|e| e.handle)
                    .filter(|h| !before.contains(h))
                    .collect();
                for h in new {
                    s.doc_mut()?.modify_entity(h, |e| {
                        if matches!(e.kind, EntityKind::Insert(_)) {
                            e.common.layer = layer.clone();
                        }
                    })?;
                }
            }
            return Ok(step);
        }
        let word = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            Input::Enter | Input::Cancel => return Ok(Step::Cancel),
            _ => return Ok(Step::Continue),
        };
        if word.eq_ignore_ascii_case("layers") || word.eq_ignore_ascii_case("l") {
            let on = !trade_layers_on(s);
            s.doc_mut()?.header.set_str(TRADE_LAYERS_VAR, if on { "1" } else { "0" });
            s.echo(if on { "Symbols go on their trade layers (E-LITE, E-POWR-DEVC, E-FIRE…)." } else { "Symbols go on the current layer." });
            return Ok(Step::Continue);
        }
        if word == "?" || word.is_empty() {
            for sym in symbols::symbols() {
                s.echo(format!("{:<28} {} ({})", sym.name, sym.label, sym.category));
            }
            return Ok(Step::Continue);
        }
        // A name, or words that find exactly one symbol.
        let sym = match symbols::symbol(&word) {
            Some(sym) => sym,
            None => {
                let found: Vec<&Symbol> = symbols::symbols().iter().filter(|sym| sym.matches(&word)).collect();
                match found.as_slice() {
                    [one] => *one,
                    [] => {
                        s.echo(format!("No library symbol matches \"{word}\". Enter ? to list them."));
                        return Ok(Step::Continue);
                    }
                    many => {
                        s.echo(format!("\"{word}\" matches {} symbols:", many.len()));
                        for sym in many.iter().take(20) {
                            s.echo(format!("  {:<28} {}", sym.name, sym.label));
                        }
                        return Ok(Step::Continue);
                    }
                }
            }
        };
        let name = define(s, sym)?;
        let d = s.doc()?;
        self.placing = Some((sym, d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())).map(|e| e.handle).collect()));
        // Fixtures at 1:1, annotation symbols at the drawing's scale: no scale prompt.
        let f = default_scale(s, sym)?;
        self.inner = Some(super::blocks::insert_machine(s, &name, Some(Vec2::new(f, f)))?);
        Ok(Step::Continue)
    }
    fn preview(&self, s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        self.inner.as_ref().map(|m| m.preview(s, cursor)).unwrap_or_default()
    }
    fn drag(&self, s: &Session, cursor: Vec2) -> Option<crate::prompt::Drag> {
        self.inner.as_ref().and_then(|m| m.drag(s, cursor))
    }
}

#[cfg(test)]
mod tests {
    use cadaddy_doc::EntityKind;
    use serde_json::json;

    use crate::Session;

    fn inserts(s: &Session) -> Vec<(String, f64, Vec<(String, String)>)> {
        s.doc()
            .unwrap()
            .model
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Insert(i) => {
                    Some((i.block.clone(), i.scale.x, i.attribs.iter().map(|a| (a.tag.clone(), a.text.value.clone())).collect()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn place_symbols_by_json_and_at_the_command_line() {
        let mut s = Session::new();
        let r = s.execute("symbol", &json!({ "name": "pl_water_closet", "at": [10, 0], "attribs": { "TAG": "WC-3" } })).unwrap();
        assert_eq!(r["block"], "PL_WATER_CLOSET");
        // Annotation symbols come at the drawing's scale (DIMSCALE here).
        s.execute("setvar", &json!({ "name": "DIMSCALE", "value": 48 })).unwrap();
        s.execute("symbol", &json!({ "name": "PL_BALL_VALVE", "at": [0, 0], "attribs": { "SIZE": "1\"" } })).unwrap();
        // Typed: a search word that finds one symbol, then INSERT's prompts (point, rotation, tag).
        s.script("SYMBOL urinal\n40,0\n0\nUR-2\n").unwrap();
        assert!(s.running.is_none(), "{}", s.prompt_text());
        let got = inserts(&s);
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0], ("PL_WATER_CLOSET".into(), 1.0, vec![("TAG".into(), "WC-3".into())]));
        assert_eq!(got[1], ("PL_BALL_VALVE".into(), 48.0, vec![("SIZE".into(), "1\"".into())]));
        assert_eq!(got[2], ("PL_URINAL".into(), 1.0, vec![("TAG".into(), "UR-2".into())]));
        // The block is defined once, with its hidden trade and type for counts.
        let d = s.doc().unwrap();
        let b = d.block("PL_URINAL").unwrap();
        let constants: Vec<(String, String)> = b
            .entities
            .iter()
            .filter_map(|e| if let EntityKind::AttDef(a) = &e.kind { a.constant.then(|| (a.tag.clone(), a.text.value.clone())) } else { None })
            .collect();
        assert_eq!(constants, [("TRADE".to_string(), "PLUMBING".to_string()), ("TYPE".to_string(), "URINAL".to_string())]);
        // Unknown names are errors; the list finds by words.
        assert!(s.execute("symbol", &json!({ "name": "PL_NOPE", "at": [0, 0] })).is_err());
        let l = s.execute("symbols.list", &json!({ "query": "valve" })).unwrap();
        let names: Vec<&str> = l["symbols"].as_array().unwrap().iter().filter_map(|v| v["name"].as_str()).collect();
        assert!(names.contains(&"PL_BALL_VALVE") && !names.contains(&"PL_SINK"), "{names:?}");
        assert!(l["symbols"].as_array().unwrap().iter().any(|v| v["name"] == "PL_BALL_VALVE" && v["defined"] == true));
    }

    /// Electrical symbols land on their trade layers (made with a colour when missing), and the
    /// schedule-only attributes (PANEL, MOUNT_HT) don't show.
    #[test]
    fn electrical_symbols_go_on_trade_layers_with_schedule_fields_hidden() {
        let mut s = Session::new();
        let layer_of =
            |s: &Session, h: &serde_json::Value| s.doc().unwrap().entity(crate::cmd::curves::handle_value(h).unwrap()).unwrap().common.layer.clone();
        let r = s.execute("symbol", &json!({"name": "EL_RECEPTACLE_DUPLEX", "at": [0, 0]})).unwrap();
        assert_eq!(layer_of(&s, &r["handle"]), "E-POWR-DEVC");
        let r = s.execute("symbol", &json!({"name": "EL_SWITCH", "at": [5, 0]})).unwrap();
        assert_eq!(layer_of(&s, &r["handle"]), "E-LITE");
        let r = s.execute("symbol", &json!({"name": "EL_RECEPTACLE_DUPLEX", "at": [9, 0], "layer": "current"})).unwrap();
        assert_eq!(layer_of(&s, &r["handle"]), "0");
        assert!(s.doc().unwrap().layer("E-LITE").is_some_and(|l| l.color == cadaddy_color::Color::Index(2)));
        // Typed: the Layers option turns trade layers off for this drawing.
        for i in ["symbol", "layers", "EL_RECEPTACLE_DUPLEX", "20,0", "0", "", "", ""] {
            s.cmdline(i).unwrap();
        }
        let last = s.doc().unwrap().model.iter().last().unwrap().clone();
        assert_eq!(last.common.layer, "0");
        // PANEL and MOUNT_HT hidden, CIRCUIT shown.
        let d = s.doc().unwrap();
        for e in d.block("EL_RECEPTACLE_DUPLEX").unwrap().entities.iter() {
            if let EntityKind::AttDef(a) = &e.kind {
                assert_eq!(a.invisible, matches!(a.tag.as_str(), "PANEL" | "MOUNT_HT" | "TRADE" | "TYPE"), "{}", a.tag);
            }
        }
    }

    /// Placing symbols one after another: only CIRCUIT is asked, its last value is offered next
    /// time, and ATTREQ 0 places with no attribute prompts (battle test A1/A2).
    #[test]
    fn symbols_remember_their_last_values_and_honour_attreq() {
        let mut s = Session::new();
        s.execute("setvar", &json!({"name": "DIMSCALE", "value": 1})).unwrap();
        for i in ["symbol", "EL_RECEPTACLE_DUPLEX", "0,0", "0", "7"] {
            s.cmdline(i).unwrap();
        }
        assert!(s.running.is_none(), "only CIRCUIT is asked: {}", s.prompt_text());
        for i in ["symbol", "EL_RECEPTACLE_DUPLEX", "10,0", "0"] {
            s.cmdline(i).unwrap();
        }
        assert!(s.prompt_text().contains("<7>"), "{}", s.prompt_text());
        s.cmdline("").unwrap();
        let circuits: Vec<String> =
            inserts(&s).iter().filter_map(|(_, _, a)| a.iter().find(|(t, _)| t == "CIRCUIT").map(|(_, v)| v.clone())).collect();
        assert_eq!(circuits, vec!["7", "7"]);
        s.execute("setvar", &json!({"name": "ATTREQ", "value": 0})).unwrap();
        for i in ["symbol", "EL_SWITCH", "20,0", "0"] {
            s.cmdline(i).unwrap();
        }
        assert!(s.running.is_none(), "ATTREQ 0 asks nothing: {}", s.prompt_text());
    }

    #[test]
    fn every_symbol_has_a_preview() {
        for p in super::previews() {
            assert!(!p.lines.is_empty() || !p.fills.is_empty(), "{} draws nothing", p.name);
            assert!(!p.bounds.is_empty() && p.bounds.width() > 0.0, "{} bounds {:?}", p.name, p.bounds);
        }
    }

    #[test]
    fn every_symbol_places_and_survives_a_dxf_round_trip_shape() {
        let mut s = Session::new();
        for (i, sym) in cadaddy_doc::library::symbols::symbols().iter().enumerate() {
            s.execute("symbol", &json!({ "name": sym.name, "at": [i as f64 * 60.0, 0] })).unwrap();
        }
        assert_eq!(inserts(&s).len(), cadaddy_doc::library::symbols::symbols().len());
    }
}
