//! BIM element types and spec records.
//!
//! - `bim.types` — list all BIM types (or filter by category/mark).
//! - `bim.set_type` — create or update a BIM type.
//! - `bim.remove_type` — remove a BIM type by mark.
//! - `bim.get_spec` — get the spec of one or more entities (resolved through their type mark).
//! - `bim.set_spec` — assign a type mark to entities (optionally creating the type).
//! - `bim.unset_spec` — remove the BIM spec assignment from entities.

use cadaddy_doc::Handle;
use cadaddy_doc::bimspec::{self, BimType, Confidence, SpecField};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD_TYPES: &str = "bim.types";
const CMD_SET_TYPE: &str = "bim.set_type";
const CMD_REMOVE_TYPE: &str = "bim.remove_type";
const CMD_GET_SPEC: &str = "bim.get_spec";
const CMD_SET_SPEC: &str = "bim.set_spec";
const CMD_UNSET_SPEC: &str = "bim.unset_spec";
const CMD_COLOUR: &str = "bim.colour";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_TYPES, "BIM Types", run_types)
            .params("{mark?: string, category?: string} → {types: [{mark, category, fields: {key: {value, source?, confidence?}}}]}")
            .noundo(),
        CommandSpec::new(CMD_SET_TYPE, "Set BIM Type", run_set_type)
            .params("{mark: string, category?: string, fields?: {key: {value, source?, confidence?} | string (shorthand for {value: string})}, remove_fields?: [string]} → {type}"),
        CommandSpec::new(CMD_REMOVE_TYPE, "Remove BIM Type", run_remove_type)
            .params("{mark: string} → {removed: bool}"),
        CommandSpec::new(CMD_GET_SPEC, "Get BIM Spec", run_get_spec)
            .params("{handles: [string]} → {specs: [{handle, mark?, category?, fields?}]}")
            .noundo(),
        CommandSpec::new(CMD_SET_SPEC, "Set BIM Spec on Entities", run_set_spec)
            .params("{handles: [string], mark: string, create?: {category?, fields?} (auto-create the type if absent)} → {assigned, type}"),
        CommandSpec::new(CMD_UNSET_SPEC, "Remove BIM Spec from Entities", run_unset_spec)
            .params("{handles: [string]} → {cleared}"),
        CommandSpec::new(CMD_COLOUR, "Colour by BIM Property", run_colour)
            .params("{by: string (mark | category | none | a BIM field key)} → {legend: [{key, color (ACI)}], coloured}"),
    ]
}

// ── bim.types ───────────────────────────────────────────────────────────────

fn run_types(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let mark_filter = p.get("mark").and_then(Value::as_str);
    let cat_filter = p.get("category").and_then(Value::as_str);

    let types: Vec<Value> = d
        .bim_types
        .types
        .iter()
        .filter(|t| mark_filter.is_none_or(|m| t.mark.eq_ignore_ascii_case(m)))
        .filter(|t| cat_filter.is_none_or(|c| t.category.eq_ignore_ascii_case(c)))
        .map(type_to_json)
        .collect();

    Ok(json!({ "types": types }))
}

// ── bim.set_type ────────────────────────────────────────────────────────────

fn run_set_type(s: &mut Session, p: &Value) -> Result<Value> {
    let mark = p.get("mark").and_then(Value::as_str).ok_or_else(|| bad(CMD_SET_TYPE, "`mark` (string) is required"))?;
    if mark.trim().is_empty() {
        return Err(bad(CMD_SET_TYPE, "`mark` must not be empty"));
    }

    let d = s.doc_mut()?;
    let existing = d.bim_types.get(mark).cloned();
    let mut ty = existing.unwrap_or_else(|| BimType::new(mark));

    if let Some(cat) = p.get("category").and_then(Value::as_str) {
        ty.category = cat.to_string();
    }

    if let Some(fields) = p.get("fields").and_then(Value::as_object) {
        for (key, val) in fields.iter().take(bimspec::MAX_FIELDS) {
            let field = parse_spec_field(CMD_SET_TYPE, val)?;
            ty.set_field(key.clone(), field);
        }
    }

    if let Some(remove) = p.get("remove_fields").and_then(Value::as_array) {
        for key in remove.iter().filter_map(Value::as_str) {
            ty.remove_field(key);
        }
    }

    let out = type_to_json(&ty);
    d.bim_types.set(ty);
    Ok(json!({ "type": out }))
}

// ── bim.remove_type ─────────────────────────────────────────────────────────

fn run_remove_type(s: &mut Session, p: &Value) -> Result<Value> {
    let mark = p.get("mark").and_then(Value::as_str).ok_or_else(|| bad(CMD_REMOVE_TYPE, "`mark` (string) is required"))?;
    let d = s.doc_mut()?;
    let removed = d.bim_types.remove(mark).is_some();
    Ok(json!({ "removed": removed }))
}

// ── bim.get_spec ────────────────────────────────────────────────────────────

fn run_get_spec(s: &mut Session, p: &Value) -> Result<Value> {
    let handles = p.get("handles").and_then(Value::as_array).ok_or_else(|| bad(CMD_GET_SPEC, "`handles` (array of handle strings) is required"))?;
    let d = s.doc()?;

    let mut specs = Vec::with_capacity(handles.len().min(100_000));
    for h_val in handles.iter().take(100_000) {
        let h_str = h_val.as_str().unwrap_or_default();
        let h = Handle::parse_hex(h_str).unwrap_or_default();
        let mut entry = json!({ "handle": h_str });

        if let Some(ent) = d.entity(h)
            && let Some(mark) = bimspec::entity_type_mark(&ent.common.xdata)
        {
            entry["mark"] = json!(mark);
            if let Some(ty) = d.bim_types.get(mark) {
                entry["category"] = json!(ty.category);
                entry["fields"] = fields_to_json(&ty.fields);
            }
        }
        specs.push(entry);
    }

    Ok(json!({ "specs": specs }))
}

// ── bim.set_spec ────────────────────────────────────────────────────────────

fn run_set_spec(s: &mut Session, p: &Value) -> Result<Value> {
    let handles = p.get("handles").and_then(Value::as_array).ok_or_else(|| bad(CMD_SET_SPEC, "`handles` (array) is required"))?;
    let mark = p.get("mark").and_then(Value::as_str).ok_or_else(|| bad(CMD_SET_SPEC, "`mark` (string) is required"))?;
    if mark.trim().is_empty() {
        return Err(bad(CMD_SET_SPEC, "`mark` must not be empty"));
    }

    // Collect handle values first so we don't borrow `p` across the mutable borrow.
    let handle_strs: Vec<&str> = handles.iter().take(100_000).filter_map(Value::as_str).collect();

    let d = s.doc_mut()?;

    // Auto-create the type if requested and absent.
    if d.bim_types.get(mark).is_none() {
        if let Some(create) = p.get("create").filter(|v| !v.is_null()) {
            let mut ty = BimType::new(mark);
            if let Some(cat) = create.get("category").and_then(Value::as_str) {
                ty.category = cat.to_string();
            }
            if let Some(fields) = create.get("fields").and_then(Value::as_object) {
                for (key, val) in fields.iter().take(bimspec::MAX_FIELDS) {
                    if let Ok(field) = parse_spec_field(CMD_SET_SPEC, val) {
                        ty.set_field(key.clone(), field);
                    }
                }
            }
            d.bim_types.set(ty);
        } else {
            return Err(bad(CMD_SET_SPEC, format!("type mark `{mark}` not found; pass `create` to auto-create")));
        }
    }

    let xdata_tags = bimspec::type_mark_xdata(mark);
    let mut assigned = 0u64;
    for h_str in &handle_strs {
        let h = Handle::parse_hex(h_str).unwrap_or_default();
        let tags = xdata_tags.clone();
        if d.modify_entity(h, |ent| {
            ent.common.xdata = bimspec::strip_bim_xdata(&ent.common.xdata).into();
            ent.common.xdata.extend(tags);
        })
        .is_ok()
        {
            assigned = assigned.saturating_add(1);
        }
    }

    let ty_json = d.bim_types.get(mark).map(type_to_json).unwrap_or(json!(null));
    Ok(json!({ "assigned": assigned, "type": ty_json }))
}

// ── bim.unset_spec ──────────────────────────────────────────────────────────

fn run_unset_spec(s: &mut Session, p: &Value) -> Result<Value> {
    let handles = p.get("handles").and_then(Value::as_array).ok_or_else(|| bad(CMD_UNSET_SPEC, "`handles` (array) is required"))?;
    let handle_strs: Vec<&str> = handles.iter().take(100_000).filter_map(Value::as_str).collect();
    let d = s.doc_mut()?;

    let mut cleared = 0u64;
    for h_str in &handle_strs {
        let h = Handle::parse_hex(h_str).unwrap_or_default();
        // Check if entity has BIM xdata before modifying.
        let has_bim = d.entity(h).is_some_and(|e| bimspec::entity_type_mark(&e.common.xdata).is_some());
        if has_bim
            && d.modify_entity(h, |ent| {
                ent.common.xdata = bimspec::strip_bim_xdata(&ent.common.xdata).into();
            })
            .is_ok()
        {
            cleared = cleared.saturating_add(1);
        }
    }

    Ok(json!({ "cleared": cleared }))
}

// ── bim.colour ─────────────────────────────────────────────────────────

/// ACI colour palette for BIM colour-by (distinct, legible colours).
const COLOUR_PALETTE: [u8; 12] = [1, 3, 5, 30, 6, 4, 2, 140, 200, 40, 94, 220];

fn run_colour(s: &mut Session, p: &Value) -> Result<Value> {
    let by = str_param(p, "by").unwrap_or("mark").to_ascii_lowercase();
    let d = s.doc()?;
    let space = s.space();
    let ents: Vec<Handle> = d.space(&space).map(|st| st.iter().map(|e| e.handle).collect()).unwrap_or_default();

    // Collect (handle, key) pairs for entities with BIM specs.
    let mut assignments: Vec<(Handle, Option<String>)> = Vec::new();
    for h in &ents {
        let Some(e) = d.entity(*h) else { continue };
        let mark = bimspec::entity_type_mark(&e.common.xdata);
        let key = if by == "none" {
            None
        } else if let Some(mark) = mark {
            match by.as_str() {
                "mark" => Some(mark.to_string()),
                "category" => d.bim_types.get(mark).and_then(|t| if t.category.is_empty() { None } else { Some(t.category.clone()) }),
                field_key => d.bim_types.get(mark).and_then(|t| {
                    t.get_field(field_key)
                        .or_else(|| {
                            let nk = field_key.to_ascii_lowercase();
                            t.fields.iter().find(|(k, _)| k.to_ascii_lowercase() == nk).map(|(_, f)| f)
                        })
                        .map(|f| f.value.clone())
                }),
            }
        } else {
            None
        };
        assignments.push((*h, key));
    }

    // Build legend from unique keys.
    let mut keys: Vec<String> = Vec::new();
    let mut set: Vec<(Handle, cadaddy_doc::color::Color)> = Vec::new();
    for (h, key) in &assignments {
        let colour = if by == "none" {
            cadaddy_doc::color::Color::ByLayer
        } else {
            match key {
                Some(k) => {
                    let i = match keys.iter().position(|x| x == k) {
                        Some(i) => i,
                        None => {
                            keys.push(k.clone());
                            keys.len() - 1
                        }
                    };
                    cadaddy_doc::color::Color::Index(COLOUR_PALETTE.get(i % COLOUR_PALETTE.len()).copied().unwrap_or(7))
                }
                None => cadaddy_doc::color::Color::Index(8), // grey for no BIM spec
            }
        };
        set.push((*h, colour));
    }

    let d = s.doc_mut()?;
    for (h, c) in &set {
        let _ = d.modify_entity(*h, |e| e.common.color = *c);
    }
    let legend: Vec<Value> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| json!({"key": k, "color": COLOUR_PALETTE.get(i % COLOUR_PALETTE.len()).copied().unwrap_or(7)}))
        .collect();
    Ok(json!({ "by": by, "legend": legend, "coloured": set.len() }))
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn parse_spec_field(cmd: &str, val: &Value) -> Result<SpecField> {
    match val {
        Value::String(s) => Ok(SpecField::new(s.clone())),
        Value::Object(obj) => {
            let value = obj.get("value").and_then(Value::as_str).ok_or_else(|| bad(cmd, "field object must have a `value` string"))?;
            let source = obj.get("source").and_then(Value::as_str).unwrap_or_default();
            let confidence = obj.get("confidence").and_then(Value::as_str).map(Confidence::parse).unwrap_or_default();
            Ok(SpecField::new(value).with_source(source).with_confidence(confidence))
        }
        _ => Err(bad(cmd, "each field must be a string or {value, source?, confidence?}")),
    }
}

fn type_to_json(ty: &BimType) -> Value {
    json!({
        "mark": ty.mark,
        "category": ty.category,
        "fields": fields_to_json(&ty.fields),
    })
}

fn fields_to_json(fields: &std::collections::BTreeMap<String, SpecField>) -> Value {
    let mut obj = serde_json::Map::new();
    for (key, field) in fields {
        let mut f = serde_json::Map::new();
        f.insert("value".into(), json!(field.value));
        if !field.source.is_empty() {
            f.insert("source".into(), json!(field.source));
        }
        if field.confidence != Confidence::Stated {
            f.insert("confidence".into(), json!(field.confidence.as_str()));
        }
        obj.insert(key.clone(), Value::Object(f));
    }
    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use crate::Session;
    use serde_json::json;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).ok();
        s
    }

    #[test]
    fn create_type_and_assign() {
        let mut s = session();

        // Create a type.
        let r = s
            .execute(
                "bim.set_type",
                &json!({
                    "mark": "H",
                    "category": "luminaire",
                    "fields": {
                        "description": {"value": "recessed 2x4 LED troffer", "source": "E-1 schedule row 3"},
                        "manufacturer": "Lithonia",
                        "catalogNumber": {"value": "2GTL4 4400LM", "confidence": "stated"},
                        "specSection": "26 51 13",
                        "volts": {"value": "120/277", "confidence": "inferred"},
                        "mounting": "recessed"
                    }
                }),
            )
            .unwrap_or_default();
        assert_eq!(r["type"]["mark"], "H");
        assert_eq!(r["type"]["fields"]["description"]["value"], "recessed 2x4 LED troffer");
        assert_eq!(r["type"]["fields"]["description"]["source"], "E-1 schedule row 3");
        assert_eq!(r["type"]["fields"]["volts"]["confidence"], "inferred");

        // List types.
        let r = s.execute("bim.types", &json!({})).unwrap_or_default();
        assert_eq!(r["types"].as_array().map(|a| a.len()), Some(1));

        // Filter by category.
        let r = s.execute("bim.types", &json!({"category": "luminaire"})).unwrap_or_default();
        assert_eq!(r["types"].as_array().map(|a| a.len()), Some(1));
        let r = s.execute("bim.types", &json!({"category": "conduit"})).unwrap_or_default();
        assert_eq!(r["types"].as_array().map(|a| a.len()), Some(0));

        // Draw a circle and assign it.
        let c = s.execute("circle", &json!({"center": [0, 0], "radius": 5})).unwrap_or_default();
        let h = c["handle"].as_str().unwrap_or_default().to_string();

        let r = s.execute("bim.set_spec", &json!({"handles": [&h], "mark": "H"})).unwrap_or_default();
        assert_eq!(r["assigned"], 1);

        // Read it back.
        let r = s.execute("bim.get_spec", &json!({"handles": [&h]})).unwrap_or_default();
        assert_eq!(r["specs"][0]["mark"], "H");
        assert_eq!(r["specs"][0]["category"], "luminaire");
        assert_eq!(r["specs"][0]["fields"]["manufacturer"]["value"], "Lithonia");

        // Unassign.
        let r = s.execute("bim.unset_spec", &json!({"handles": [&h]})).unwrap_or_default();
        assert_eq!(r["cleared"], 1);

        let r = s.execute("bim.get_spec", &json!({"handles": [&h]})).unwrap_or_default();
        assert!(r["specs"][0].get("mark").is_none());
    }

    #[test]
    fn update_type_fields() {
        let mut s = session();
        s.execute("bim.set_type", &json!({"mark": "A", "fields": {"description": "outlet"}})).ok();
        s.execute("bim.set_type", &json!({"mark": "A", "fields": {"amps": "20"}, "remove_fields": ["description"]})).ok();
        let r = s.execute("bim.types", &json!({"mark": "A"})).unwrap_or_default();
        let fields = &r["types"][0]["fields"];
        assert!(fields.get("description").is_none());
        assert_eq!(fields["amps"]["value"], "20");
    }

    #[test]
    fn remove_type() {
        let mut s = session();
        s.execute("bim.set_type", &json!({"mark": "X"})).ok();
        let r = s.execute("bim.remove_type", &json!({"mark": "X"})).unwrap_or_default();
        assert_eq!(r["removed"], true);
        let r = s.execute("bim.remove_type", &json!({"mark": "X"})).unwrap_or_default();
        assert_eq!(r["removed"], false);
    }

    #[test]
    fn set_spec_auto_create() {
        let mut s = session();
        let c = s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap_or_default();
        let h = c["handle"].as_str().unwrap_or_default().to_string();

        // Without create: should fail.
        assert!(s.execute("bim.set_spec", &json!({"handles": [&h], "mark": "NEW"})).is_err());

        // With create: auto-creates.
        let r = s
            .execute(
                "bim.set_spec",
                &json!({
                    "handles": [&h],
                    "mark": "NEW",
                    "create": {"category": "receptacle", "fields": {"amps": "20"}}
                }),
            )
            .unwrap_or_default();
        assert_eq!(r["assigned"], 1);
        assert_eq!(r["type"]["category"], "receptacle");
    }

    #[test]
    fn raceway_and_cable_fields() {
        let mut s = session();
        let r = s
            .execute(
                "bim.set_type",
                &json!({
                    "mark": "C1",
                    "category": "conduit",
                    "fields": {
                        "racewayMaterial": {"value": "PVC", "source": "spec 26 05 33 p.4"},
                        "racewaySchedule": "Sch 40",
                        "tradeSize": {"value": "3/4\"", "confidence": "stated"},
                        "conductorCount": "3",
                        "conductorSize": "#12 AWG",
                        "conductorMaterial": "Cu",
                        "insulation": "THHN",
                        "groundSize": "#12 AWG"
                    }
                }),
            )
            .unwrap_or_default();
        assert_eq!(r["type"]["fields"]["racewayMaterial"]["value"], "PVC");
        assert_eq!(r["type"]["fields"]["insulation"]["value"], "THHN");
    }

    #[test]
    fn cable_assembly_fields() {
        let mut s = session();
        let r = s
            .execute(
                "bim.set_type",
                &json!({
                    "mark": "SER-1",
                    "category": "cable",
                    "fields": {
                        "cableType": "SER",
                        "conductorCount": "4",
                        "conductorSize": "#4 AWG",
                        "conductorMaterial": "Al",
                        "description": {"value": "4/0-4/0-4/0-2/0 SER aluminum", "source": "panel schedule E-2"}
                    }
                }),
            )
            .unwrap_or_default();
        assert_eq!(r["type"]["fields"]["cableType"]["value"], "SER");
    }

    #[test]
    fn hostile_params_never_panic() {
        let mut s = session();
        // Empty mark.
        assert!(s.execute("bim.set_type", &json!({"mark": ""})).is_err());
        assert!(s.execute("bim.set_spec", &json!({"handles": [], "mark": ""})).is_err());
        // Missing required params.
        assert!(s.execute("bim.set_type", &json!({})).is_err());
        assert!(s.execute("bim.get_spec", &json!({})).is_err());
        assert!(s.execute("bim.set_spec", &json!({})).is_err());
        assert!(s.execute("bim.unset_spec", &json!({})).is_err());
        assert!(s.execute("bim.remove_type", &json!({})).is_err());
        // Bad field value type.
        assert!(s.execute("bim.set_type", &json!({"mark": "X", "fields": {"a": 42}})).is_err());
        // Non-existent handles.
        let r = s.execute("bim.get_spec", &json!({"handles": ["DEADBEEF"]})).unwrap_or_default();
        assert_eq!(r["specs"].as_array().map(|a| a.len()), Some(1));
    }

    /// MUST 3: qselect with bim.mark, bim.category, bim.<field> properties.
    #[test]
    fn qselect_bim_properties() {
        let mut s = session();
        // Create types H and A with different manufacturers.
        s.execute("bim.set_type", &json!({"mark": "H", "category": "luminaire", "fields": {"manufacturer": "Lithonia"}})).unwrap();
        s.execute("bim.set_type", &json!({"mark": "A", "category": "receptacle", "fields": {"manufacturer": "Leviton"}})).unwrap();
        // Draw 5 circles: 3 assigned H, 2 assigned A.
        let mut h_handles = Vec::new();
        for i in 0..3 {
            let r = s.execute("circle", &json!({"center": [i * 10, 0], "radius": 1})).unwrap();
            let h = r["handle"].as_str().unwrap().to_string();
            s.execute("bim.set_spec", &json!({"handles": [&h], "mark": "H"})).unwrap();
            h_handles.push(h);
        }
        let mut a_handles = Vec::new();
        for i in 0..2 {
            let r = s.execute("circle", &json!({"center": [i * 10, 20], "radius": 1})).unwrap();
            let h = r["handle"].as_str().unwrap().to_string();
            s.execute("bim.set_spec", &json!({"handles": [&h], "mark": "A"})).unwrap();
            a_handles.push(h);
        }

        // qselect bim.mark = H → exactly 3.
        let r = s.execute("qselect", &json!({"property": "bim.mark", "operator": "=", "value": "H"})).unwrap();
        assert_eq!(r["selected"].as_u64(), Some(3), "bim.mark=H should select 3");

        // qselect bim.mark = A → exactly 2.
        let r = s.execute("qselect", &json!({"property": "bim.mark", "operator": "=", "value": "A"})).unwrap();
        assert_eq!(r["selected"].as_u64(), Some(2), "bim.mark=A should select 2");

        // qselect bim.manufacturer = Lithonia → exactly 3 (H's manufacturer).
        let r = s.execute("qselect", &json!({"property": "bim.manufacturer", "operator": "=", "value": "Lithonia"})).unwrap();
        assert_eq!(r["selected"].as_u64(), Some(3), "bim.manufacturer=Lithonia should select 3");

        // qselect bim.category = luminaire → exactly 3.
        let r = s.execute("qselect", &json!({"property": "bim.category", "operator": "=", "value": "luminaire"})).unwrap();
        assert_eq!(r["selected"].as_u64(), Some(3), "bim.category=luminaire should select 3");

        // qselect.info should list bim.mark and bim.manufacturer in properties.
        let info = s.execute("qselect.info", &json!({})).unwrap();
        let props = info["properties"].as_array().unwrap();
        assert!(props.iter().any(|p| p == "bim.mark"), "qselect.info should list bim.mark");
        assert!(props.iter().any(|p| p == "bim.manufacturer"), "qselect.info should list bim.manufacturer");
    }

    /// MUST 3: bim.colour assigns distinct colours by mark.
    #[test]
    fn colour_by_mark() {
        let mut s = session();
        s.execute("bim.set_type", &json!({"mark": "H", "category": "luminaire"})).unwrap();
        s.execute("bim.set_type", &json!({"mark": "A", "category": "receptacle"})).unwrap();
        // 2 circles: one H, one A.
        let c1 = s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
        let h1 = c1["handle"].as_str().unwrap().to_string();
        s.execute("bim.set_spec", &json!({"handles": [&h1], "mark": "H"})).unwrap();
        let c2 = s.execute("circle", &json!({"center": [10, 0], "radius": 1})).unwrap();
        let h2 = c2["handle"].as_str().unwrap().to_string();
        s.execute("bim.set_spec", &json!({"handles": [&h2], "mark": "A"})).unwrap();

        let r = s.execute("bim.colour", &json!({"by": "mark"})).unwrap();
        let legend = r["legend"].as_array().unwrap();
        assert_eq!(legend.len(), 2, "two distinct marks in legend");
        let c0 = legend[0]["color"].as_u64().unwrap();
        let c1_val = legend[1]["color"].as_u64().unwrap();
        assert_ne!(c0, c1_val, "H and A should get different colours");
        assert!(r["coloured"].as_u64().unwrap() >= 2);

        // Reset with none.
        let r = s.execute("bim.colour", &json!({"by": "none"})).unwrap();
        assert_eq!(r["legend"].as_array().unwrap().len(), 0);
    }

    /// MUST 5: bulk type change — change H's manufacturer, all 3 H entities see the new value,
    /// and undo restores the old value on all of them.
    #[test]
    fn bulk_type_change_and_undo() {
        let mut s = session();
        s.execute("bim.set_type", &json!({"mark": "H", "category": "luminaire", "fields": {"manufacturer": "Lithonia"}})).unwrap();
        // Create 3 circles assigned to H.
        let mut handles = Vec::new();
        for i in 0..3 {
            let r = s.execute("circle", &json!({"center": [i * 10, 0], "radius": 1})).unwrap();
            let h = r["handle"].as_str().unwrap().to_string();
            s.execute("bim.set_spec", &json!({"handles": [&h], "mark": "H"})).unwrap();
            handles.push(h);
        }
        // All 3 report Lithonia.
        let r = s.execute("bim.get_spec", &json!({"handles": &handles})).unwrap();
        for i in 0..3 {
            assert_eq!(r["specs"][i]["fields"]["manufacturer"]["value"], "Lithonia");
        }

        // Change H's manufacturer to "Cooper".
        s.execute("bim.set_type", &json!({"mark": "H", "fields": {"manufacturer": "Cooper"}})).unwrap();

        // All 3 now report Cooper.
        let r = s.execute("bim.get_spec", &json!({"handles": &handles})).unwrap();
        for i in 0..3 {
            assert_eq!(r["specs"][i]["fields"]["manufacturer"]["value"], "Cooper", "entity {i} should see Cooper after bulk change");
        }

        // Undo: all 3 should see Lithonia again.
        s.undo().unwrap();
        let r = s.execute("bim.get_spec", &json!({"handles": &handles})).unwrap();
        for i in 0..3 {
            assert_eq!(r["specs"][i]["fields"]["manufacturer"]["value"], "Lithonia", "entity {i} should see Lithonia after undo");
        }
    }
}
