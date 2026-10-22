//! PART: place a part from CADaddy's parts library ([`cadaddy_doc::library::parts`]: doors,
//! windows, furniture, fixtures, MEP equipment, steel shapes, site and annotation blocks) as a
//! block reference. The block is defined the first time it is placed (with its stretches and
//! visibility states, so the reference is a dynamic block); a block of that name the drawing
//! already has is used as it is.
//!
//! Parts measure the same in any drawing: a part drawn in inches (or millimetres) is scaled by
//! the drawing's units (INSUNITS), and annotation parts also by the drawing's scale. A placed
//! part goes on its discipline's layer (US National CAD Standard names) unless asked otherwise.

use cadaddy_doc::library::parts::{self, Discipline, Part};
use cadaddy_doc::{EntityKind, Handle};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::symbol::SymbolPreview;
use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// Most parts one `parts.list` call returns.
const MAX_LIST: usize = 2000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("part", "Parts Library...", run_part)
            .menu(&["Insert", "Parts Library..."])
            .alias(&["parts", "-part"])
            .params(
                "{name (a library part, e.g. \"A_DOOR_SGL_36_LH\", \"S_W8X31\"; see parts.list), at: [x,y], rotation? (degrees), scale? (default: the drawing's units, times its scale for annotation parts), attribs?: {TAG: value}, properties?: {Width: 42, Swing: \"Swing out\"} (the part's dynamic properties), layer? (default: the discipline's layer, A-DOOR, M-HVAC-CDFF…; \"current\" for the current layer)} → {handle, block, scale, layer?}",
            )
            .interactive(|_| Ok(Box::new(PartM::default()))),
        CommandSpec::new("parts.list", "List Parts", run_list)
            .params("{query?: words to search for, discipline? (Architectural, Mechanical, Electrical, Plumbing, Structural, Site, Annotation), category?, offset?, limit? (default 200, at most 2000)} → {total, parts: [{name, label, discipline, category, annotation, metric, dynamic, has3d, defined}]}")
            .noundo(),
        CommandSpec::new("parts.categories", "Part Categories", run_categories)
            .params("{} → {total, disciplines: [{name, prefix, count, categories: [{name, count}]}]}")
            .noundo(),
    ]
}

fn find(name: &str) -> Result<&'static Part> {
    parts::part(name).ok_or_else(|| bad("part", format!("no library part `{name}` (see parts.list)")))
}

/// The scale a part is placed at by default: its units in the drawing's, and the drawing's
/// scale for annotation parts.
fn default_scale(s: &Session, part: &Part) -> Result<f64> {
    let units = parts::units_scale(s.doc()?, part);
    let f = if part.annotation { units * super::symbol::drawing_scale(s)? } else { units };
    Ok(if f.is_finite() && f > 0.0 { f } else { 1.0 })
}

/// The layer a part goes on by default and its colour.
fn discipline_layer(part: &Part) -> (&'static str, u8) {
    match (part.discipline, part.category) {
        (Discipline::Architectural, "Doors") => ("A-DOOR", 3),
        (Discipline::Architectural, "Windows") => ("A-GLAZ", 4),
        (Discipline::Architectural, "Stairs and ramps") => ("A-FLOR-STRS", 2),
        (Discipline::Architectural, "Sanitary fixtures") => ("A-FLOR-FIXT", 6),
        (Discipline::Architectural, "Kitchen") => ("A-FLOR-CASE", 6),
        (Discipline::Architectural, _) => ("A-FURN", 5),
        (Discipline::Mechanical, "Diffusers and grilles") => ("M-HVAC-CDFF", 4),
        (Discipline::Mechanical, _) => ("M-HVAC-EQPM", 6),
        (Discipline::Plumbing, "Valves" | "Pipe fittings") => ("P-PIPE-SYMB", 3),
        (Discipline::Plumbing, _) => ("P-EQPM", 6),
        (Discipline::Electrical, "Light fixtures") => ("E-LITE", 2),
        (Discipline::Electrical, _) => ("E-POWR-EQPM", 6),
        (Discipline::Structural, "Rebar") => ("S-REIN", 1),
        (Discipline::Structural, _) => ("S-MEMB", 3),
        (Discipline::Site, "Planting") => ("L-PLNT", 3),
        (Discipline::Site, _) => ("C-SITE", 4),
        (Discipline::Annotation, "Title blocks") => ("G-ANNO-TTLB", 7),
        (Discipline::Annotation, _) => ("G-ANNO-SYMB", 7),
    }
}

/// Header variable: "0" places parts on the current layer instead of their discipline's.
const LAYERS_VAR: &str = "CADADDY_PART_LAYERS";

fn layers_on(s: &Session) -> bool {
    s.doc().map(|d| d.header.str(LAYERS_VAR, "1") != "0").unwrap_or(true)
}

/// The layer to put a placed part on (`None`: the current layer): `layer` when given ("current"
/// for the current one), else its discipline's, made with its colour when missing.
fn target_layer(s: &mut Session, part: &Part, layer: Option<&str>) -> Result<Option<String>> {
    let name = match layer.map(str::trim) {
        Some(l) if l.eq_ignore_ascii_case("current") || l.is_empty() => return Ok(None),
        Some(l) => l.to_string(),
        None if !layers_on(s) => return Ok(None),
        None => {
            let (n, color) = discipline_layer(part);
            let d = s.doc_mut()?;
            if d.ensure_layer(n)
                && let Some(l) = d.layer_mut(n)
            {
                l.color = cadaddy_color::Color::Index(color);
            }
            n.to_string()
        }
    };
    s.doc_mut()?.ensure_layer(&name);
    Ok(Some(name))
}

/// Define `part`'s block in the drawing (with its dynamic parameters) unless it is there.
pub(crate) fn define(s: &mut Session, part: &Part) -> Result<String> {
    parts::define(s.doc_mut()?, part).map_err(|e| bad("part", e))
}

fn run_part(s: &mut Session, p: &Value) -> Result<Value> {
    let part = find(str_param(p, "name").ok_or_else(|| bad("part", "`name` is required"))?)?;
    let at = point_req("part", p, "at")?;
    let scale = match p.get("scale").and_then(Value::as_f64) {
        Some(f) if f.is_finite() && f != 0.0 => f,
        Some(_) => return Err(bad("part", "`scale` must be a non-zero number")),
        None => default_scale(s, part)?,
    };
    let properties: Vec<(String, String)> = match p.get("properties") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(m)) => m
            .iter()
            .take(100)
            .map(|(k, v)| match v {
                Value::String(t) => Ok((k.clone(), t.clone())),
                Value::Number(n) => Ok((k.clone(), n.to_string())),
                _ => Err(bad("part", format!("property `{k}` must be a number or text"))),
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(bad("part", "`properties` must be an object")),
    };
    // Everything below is one change: on an error the drawing is left as it was.
    let before = s.doc()?.clone();
    let r = place(s, part, at, scale, p, &properties);
    if r.is_err() {
        *s.doc_mut()? = before;
    }
    r
}

fn place(s: &mut Session, part: &Part, at: Vec2, scale: f64, p: &Value, properties: &[(String, String)]) -> Result<Value> {
    let name = define(s, part)?;
    let mut ip = json!({ "name": name, "at": [at.x, at.y], "scale": scale, "rotation": f64_or(p, "rotation", 0.0) });
    if let Some(a) = p.get("attribs") {
        ip["attribs"] = a.clone();
    }
    // The INSERT command body, not `execute`: PART records the one undo step.
    let insert = find_command("insert").ok_or_else(|| EngineError::UnknownCommand("insert".into()))?;
    let mut r = (insert.run)(s, &ip)?;
    let h = r.get("handle").and_then(super::curves::handle_value).ok_or_else(|| bad("part", "the part was not placed"))?;
    for (k, v) in properties {
        cadaddy_doc::dynblock::set_property(s.doc_mut()?, h, k, v).map_err(|e| bad("part", e))?;
    }
    if let Some(layer) = target_layer(s, part, str_param(p, "layer"))? {
        s.doc_mut()?.modify_entity(h, |e| e.common.layer = layer.clone().into())?;
        r["layer"] = json!(layer);
    }
    r["block"] = json!(name);
    r["scale"] = json!(scale);
    Ok(r)
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let query = str_param(p, "query").unwrap_or("");
    let discipline = match str_param(p, "discipline") {
        Some(t) => Some(Discipline::parse(t).ok_or_else(|| bad("parts.list", format!("no discipline `{t}`")))?),
        None => None,
    };
    let category = str_param(p, "category");
    let offset = p.get("offset").and_then(Value::as_u64).map_or(0, |v| usize::try_from(v).unwrap_or(usize::MAX));
    let limit = p.get("limit").and_then(Value::as_u64).map_or(200, |v| usize::try_from(v).unwrap_or(MAX_LIST)).min(MAX_LIST);
    let d = s.doc().ok();
    let found: Vec<&Part> = parts::parts()
        .iter()
        .filter(|pt| {
            discipline.is_none_or(|d| pt.discipline == d) && category.is_none_or(|c| pt.category.eq_ignore_ascii_case(c)) && pt.matches(query)
        })
        .collect();
    let list: Vec<Value> = found
        .iter()
        .skip(offset)
        .take(limit)
        .map(|pt| {
            json!({
                "name": pt.name,
                "label": pt.label,
                "discipline": pt.discipline.label(),
                "category": pt.category,
                "annotation": pt.annotation,
                "metric": pt.metric,
                "dynamic": pt.is_dynamic(),
                "has3d": pt.has_3d(),
                "defined": d.is_some_and(|d| d.block(&pt.name).is_some()),
            })
        })
        .collect();
    Ok(json!({ "total": found.len(), "parts": list }))
}

fn run_categories(_: &mut Session, _: &Value) -> Result<Value> {
    let all = parts::parts();
    let disciplines: Vec<Value> = Discipline::ALL
        .iter()
        .map(|&d| {
            let cats: Vec<Value> = parts::categories(d)
                .into_iter()
                .map(|c| json!({ "name": c, "count": all.iter().filter(|p| p.discipline == d && p.category == c).count() }))
                .collect();
            json!({ "name": d.label(), "prefix": d.prefix(), "count": all.iter().filter(|p| p.discipline == d).count(), "categories": cats })
        })
        .collect();
    Ok(json!({ "total": all.len(), "disciplines": disciplines }))
}

/// A library part drawn for a palette tile (its plan view in its first visibility state).
pub fn preview(part: &'static Part) -> SymbolPreview {
    let (lines, fills, bounds) = super::symbol::drawn(&part.preview_entities());
    SymbolPreview { name: &part.name, label: &part.label, category: part.category, keywords: &part.keywords, lines, fills, bounds }
}

/// PART at the command line: the part's name or words that find it (`?` lists the categories),
/// then INSERT's prompts for it: insertion point, rotation and its attributes. The scale isn't
/// asked: parts come at the drawing's units and scale.
#[derive(Default)]
struct PartM {
    inner: Option<Box<dyn Interactive>>,
    /// The part being placed, and the handles there were before (to find the new reference).
    placing: Option<(&'static Part, std::collections::HashSet<Handle>)>,
}

impl Interactive for PartM {
    fn name(&self) -> &'static str {
        "PART"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match &self.inner {
            Some(m) => m.prompt(s),
            None => Prompt::new("Enter part name or words to search for", Accept::TEXT).kw(&["?", "Layers"]),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if let Some(m) = &mut self.inner {
            let step = m.input(s, i)?;
            if step == Step::Done
                && let Some((part, before)) = self.placing.take()
                && let Some(layer) = target_layer(s, part, None)?
            {
                let d = s.doc()?;
                let new: Vec<Handle> = d
                    .model
                    .iter()
                    .chain(d.layouts.iter().flat_map(|l| l.entities.iter()))
                    .map(|e| e.handle)
                    .filter(|h| !before.contains(h))
                    .collect();
                for h in new {
                    s.doc_mut()?.modify_entity(h, |e| {
                        if matches!(e.kind, EntityKind::Insert(_)) {
                            e.common.layer = layer.clone().into();
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
            let on = !layers_on(s);
            s.doc_mut()?.header.set_str(LAYERS_VAR, if on { "1" } else { "0" });
            s.echo(if on { "Parts go on their discipline's layers (A-DOOR, M-HVAC-EQPM, S-MEMB…)." } else { "Parts go on the current layer." });
            return Ok(Step::Continue);
        }
        if word == "?" || word.is_empty() {
            s.echo(format!("{} parts. Type a name or words (\"door 36\", \"w8x\", \"vav reheat\"):", parts::parts().len()));
            for d in Discipline::ALL {
                let cats = parts::categories(d);
                s.echo(format!("  {}: {}", d.label(), cats.join(", ")));
            }
            return Ok(Step::Continue);
        }
        let part = match parts::part(&word) {
            Some(p) => p,
            None => {
                let found: Vec<&'static Part> = parts::parts().iter().filter(|p| p.matches(&word)).collect();
                match found.as_slice() {
                    [one] => *one,
                    [] => {
                        s.echo(format!("No library part matches \"{word}\". Enter ? for the categories."));
                        return Ok(Step::Continue);
                    }
                    many => {
                        s.echo(format!("\"{word}\" matches {} parts:", many.len()));
                        for p in many.iter().take(20) {
                            s.echo(format!("  {:<28} {}", p.name, p.label));
                        }
                        if many.len() > 20 {
                            s.echo("  … add words to narrow the search.");
                        }
                        return Ok(Step::Continue);
                    }
                }
            }
        };
        let name = define(s, part)?;
        let d = s.doc()?;
        self.placing = Some((part, d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())).map(|e| e.handle).collect()));
        let f = default_scale(s, part)?;
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

    fn insert_of(s: &Session, h: &serde_json::Value) -> (String, f64, String) {
        let d = s.doc().unwrap();
        let e = d.entity(crate::cmd::curves::handle_value(h).unwrap()).unwrap();
        let EntityKind::Insert(i) = &e.kind else { panic!("not an insert") };
        (i.block.clone(), i.scale.x, e.common.layer.to_string())
    }

    #[test]
    fn place_parts_by_json_on_their_layers_at_the_drawings_scale() {
        let mut s = Session::new();
        let r = s.execute("part", &json!({"name": "a_door_sgl_36_lh", "at": [0, 0]})).unwrap();
        assert_eq!(r["block"], "A_DOOR_SGL_36_LH");
        assert_eq!(insert_of(&s, &r["handle"]), ("A_DOOR_SGL_36_LH".into(), 1.0, "A-DOOR".into()));
        // Annotation parts at the drawing's scale.
        s.execute("setvar", &json!({"name": "DIMSCALE", "value": 48})).unwrap();
        let r = s.execute("part", &json!({"name": "G_NORTH_ARROW_2", "at": [100, 0]})).unwrap();
        assert_eq!(insert_of(&s, &r["handle"]).1, 48.0);
        // A metric part in an inch drawing measures the same: 1/25.4.
        let r = s.execute("part", &json!({"name": "S_IPE200", "at": [200, 0], "layer": "current"})).unwrap();
        let (_, k, layer) = insert_of(&s, &r["handle"]);
        assert!((k - 1.0 / 25.4).abs() < 1e-12 && layer == "0", "{k} {layer}");
        assert!(s.doc().unwrap().layer("A-DOOR").is_some_and(|l| l.color == cadaddy_color::Color::Index(3)));
        // Unknown names and bad properties are errors that leave the drawing alone.
        assert!(s.execute("part", &json!({"name": "A_NOPE", "at": [0, 0]})).is_err());
        let n = s.doc().unwrap().model.len();
        let blocks = s.doc().unwrap().blocks.len();
        assert!(s.execute("part", &json!({"name": "A_CAB_BASE_30", "at": [0, 0], "properties": {"Depth": 3}})).is_err());
        assert_eq!((s.doc().unwrap().model.len(), s.doc().unwrap().blocks.len()), (n, blocks));
    }

    /// The dynamic properties work on a placed part: a cabinet widens by its grip property, a
    /// door swings the other way, and both survive a DXF round trip as dynamic blocks.
    #[test]
    fn placed_parts_are_dynamic_blocks() {
        let mut s = Session::new();
        let r = s.execute("part", &json!({"name": "A_CAB_BASE_24", "at": [0, 0], "properties": {"Width": 33}})).unwrap();
        let props = s.execute("blocks.reference", &json!({"handle": r["handle"]})).unwrap();
        let width = props["properties"].as_array().unwrap().iter().find(|p| p["name"] == "Width").unwrap().clone();
        assert!((width["value"].as_str().unwrap().parse::<f64>().unwrap() - 33.0).abs() < 1e-9, "{props}");
        let d = s.doc().unwrap();
        let e = d.entity(crate::cmd::curves::handle_value(&r["handle"]).unwrap()).unwrap();
        let ext = cadaddy_doc::entity_bounds(d, e, 8);
        assert!((ext.width() - 33.0).abs() < 1e-6, "{ext:?}");
        let r = s.execute("part", &json!({"name": "A_DOOR_SGL_36_RH", "at": [100, 0], "properties": {"Swing": "Swing out"}})).unwrap();
        let props = s.execute("blocks.reference", &json!({"handle": r["handle"]})).unwrap();
        assert!(props["properties"].as_array().unwrap().iter().any(|p| p["value"] == "Swing out"), "{props}");
        // Typed afterwards, as the Properties palette does.
        s.execute("blocks.reference.set", &json!({"handle": r["handle"], "name": "Swing", "value": "Swing in"})).unwrap();
        // Saved and read back: still dynamic blocks with their parameters.
        let text = cadaddy_io::write_dxf(s.doc().unwrap());
        let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
        for (name, label) in [("A_CAB_BASE_24", "Width"), ("A_DOOR_SGL_36_RH", "Swing")] {
            let m = cadaddy_doc::dynblock::dynamic_block(&back, name).unwrap_or_else(|| panic!("{name} lost its parameters"));
            assert!(m.parameters.iter().any(|p| p.kind.label() == Some(label)), "{name}: {m:?}");
        }
    }

    #[test]
    fn type_a_search_at_the_command_line() {
        let mut s = Session::new();
        s.execute("setvar", &json!({"name": "ATTREQ", "value": 0})).unwrap();
        // Many matches: listed, still asking; one match: placed.
        s.cmdline("part").unwrap();
        s.cmdline("door").unwrap();
        assert!(s.running.is_some());
        s.cmdline("w8x31").unwrap();
        s.cmdline("10,10").unwrap();
        s.cmdline("0").unwrap();
        assert!(s.running.is_none(), "{}", s.prompt_text());
        let d = s.doc().unwrap();
        let last = d.model.iter().last().unwrap();
        assert_eq!(last.common.layer, "S-MEMB");
        assert!(matches!(&last.kind, EntityKind::Insert(i) if i.block == "S_W8X31"));
    }

    #[test]
    fn list_and_categories() {
        let mut s = Session::new();
        let c = s.execute("parts.categories", &json!({})).unwrap();
        assert!(c["total"].as_u64().unwrap() >= 1000);
        assert_eq!(c["disciplines"].as_array().unwrap().len(), 7);
        let sum: u64 = c["disciplines"].as_array().unwrap().iter().map(|d| d["count"].as_u64().unwrap()).sum();
        assert_eq!(sum, c["total"].as_u64().unwrap());
        let l = s.execute("parts.list", &json!({"discipline": "mechanical", "query": "vav 8"})).unwrap();
        let names: Vec<&str> = l["parts"].as_array().unwrap().iter().filter_map(|v| v["name"].as_str()).collect();
        assert!(names.contains(&"M_VAV_8_HW"), "{names:?}");
        assert!(l["parts"].as_array().unwrap().iter().all(|v| v["dynamic"] == true));
        let page = s.execute("parts.list", &json!({"offset": 10, "limit": 5})).unwrap();
        assert_eq!(page["parts"].as_array().unwrap().len(), 5);
        assert!(page["total"].as_u64().unwrap() >= 1000);
        assert!(s.execute("parts.list", &json!({"discipline": "astrology"})).is_err());
    }

    #[test]
    fn hostile_part_params_never_panic() {
        let mut s = Session::new();
        for p in [
            json!({}),
            json!({"name": 5, "at": [0, 0]}),
            json!({"name": "A_DOOR_SGL_36_LH"}),
            json!({"name": "A_DOOR_SGL_36_LH", "at": [1e308, -1e308], "scale": 0}),
            json!({"name": "A_DOOR_SGL_36_LH", "at": [0, 0], "properties": [1, 2]}),
            json!({"name": "A_DOOR_SGL_36_LH", "at": [0, 0], "properties": {"Swing": {"x": 1}}}),
            json!({"name": "A_CAB_BASE_24", "at": [0, 0], "properties": {"Width": "-5"}}),
            json!({"name": "A_CAB_BASE_24", "at": [0, 0], "properties": {"Width": "NaN"}}),
            json!({"name": "A_CAB_BASE_24", "at": [0, 0], "properties": {"Width": 1e300}}),
        ] {
            let _ = s.execute("part", &p);
        }
        for p in [json!({"limit": -1}), json!({"offset": 1e300}), json!({"limit": 1e300, "query": "\u{0}"})] {
            let _ = s.execute("parts.list", &p);
        }
    }

    #[test]
    fn every_part_has_a_preview() {
        for p in cadaddy_doc::library::parts::parts().iter().step_by(7) {
            let pv = super::preview(p);
            assert!(!pv.bounds.is_empty() && !pv.lines.is_empty() || !pv.fills.is_empty(), "{}", p.name);
        }
    }
}
