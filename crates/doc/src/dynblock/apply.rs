//! Changing a dynamic block reference's custom properties.
//!
//! A changed reference shows an anonymous block (`*U12`) made from its current one with the
//! change applied, and says so the way AutoCAD's own references do, so AutoCAD still reads it as
//! a reference to the dynamic block: the anonymous block's BLOCK_RECORD carries
//! `AcDbBlockRepBTag` xdata naming the dynamic block's record, each copied object keeps the
//! `AcDbBlockRepETag` xdata that pairs it with its object in the dynamic block, and the reference's
//! `AcDbBlockRepresentation` data holds its values (created when it had none).
//!
//! What can change so far: a visibility state, and a lookup value whose rows set only visibility
//! states. Distances, positions and the other parameters' actions come later.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::{ActionKind, ParameterKind, dynamic_block, reference};
use crate::foreign::{ForeignExtra, ForeignObject};
use crate::{Block, Drawing, Entity, EntityKind, Handle, RawTag};

/// Xdata application naming a copied object's object in the dynamic block (its element tag).
const ETAG: &str = "AcDbBlockRepETag";
/// Xdata application naming an anonymous block's dynamic block.
const BTAG: &str = "AcDbBlockRepBTag";

fn tag(code: i32, value: impl Into<String>) -> RawTag {
    RawTag { code, value: value.into() }
}

/// The element tag (`AcDbBlockRepETag` 1071) of object `h`, if it has one.
fn etag(d: &Drawing, h: Handle) -> Option<i64> {
    let x = &d.foreign.entities.get(&h)?.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.eq_ignore_ascii_case(ETAG))?;
    x.iter().skip(at + 1).take_while(|t| t.code != 1001).find(|t| t.code == 1071).and_then(|t| t.value.trim().parse().ok())
}

/// Each object of `b` keyed by its element tag, or by its position when it has none.
fn by_tag(d: &Drawing, b: &Block) -> Vec<(i64, Handle)> {
    b.entities.iter().enumerate().map(|(i, e)| (etag(d, e.handle).unwrap_or(i as i64 + 1_000_000), e.handle)).collect()
}

/// The file handle of block `name`'s BLOCK_RECORD, as the kept data refers to it.
fn record_handle(d: &Drawing, name: &str) -> Option<Handle> {
    let key = name.to_ascii_uppercase();
    d.foreign.targets.iter().find(|(_, (k, n))| k == "BLOCK_RECORD" && *n == key).map(|(h, _)| *h)
}

/// The object entry `key` of dictionary `dict` refers to.
fn entry(d: &Drawing, dict: Handle, key: &str) -> Option<Handle> {
    let o = d.foreign.objects.iter().find(|o| o.handle == dict)?;
    let mut tags = o.tags.iter();
    while let Some(t) = tags.next() {
        if t.code == 3 && t.value.eq_ignore_ascii_case(key) {
            return tags.next().filter(|t| t.code == 350 || t.code == 360).and_then(|t| Handle::parse_hex(&t.value));
        }
    }
    None
}

/// Every entry of dictionary `dict`: (key, object).
fn entries(d: &Drawing, dict: Handle) -> Vec<(String, Handle)> {
    let Some(o) = d.foreign.objects.iter().find(|o| o.handle == dict) else { return Vec::new() };
    let mut out = Vec::new();
    let mut key: Option<&str> = None;
    for t in &o.tags {
        match t.code {
            3 => key = Some(&t.value),
            350 | 360 => {
                if let (Some(k), Some(h)) = (key.take(), Handle::parse_hex(&t.value)) {
                    out.push((k.to_string(), h));
                }
            }
            _ => {}
        }
    }
    out
}

/// Set custom property `name` (as the Properties palette names it, any case) of dynamic block
/// reference `insert` to `value`.
pub fn set_property(d: &mut Drawing, insert: Handle, name: &str, value: &str) -> Result<(), String> {
    let r = reference(d, insert).ok_or("Not a dynamic block reference.")?;
    let m = dynamic_block(d, &r.definition).ok_or("Not a dynamic block reference.")?;
    let p = m
        .parameters
        .iter()
        .find(|p| p.show_properties && p.kind.label().is_some_and(|l| l.eq_ignore_ascii_case(name.trim())))
        .ok_or_else(|| format!("The reference has no property \"{name}\"."))?;
    // The visibility states to set, by parameter graph id, and the lookup values to record.
    let mut states: Vec<(i32, String)> = Vec::new();
    let mut lookups: Vec<(i32, String)> = Vec::new();
    match &p.kind {
        ParameterKind::Visibility { states: all, label, .. } => {
            let s = all.iter().find(|s| s.name.eq_ignore_ascii_case(value.trim())).ok_or_else(|| {
                format!("\"{value}\" is not a state of {label}: {}.", all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))
            })?;
            states.push((p.node, s.name.clone()));
        }
        ParameterKind::Lookup { label, .. } => {
            let mut found = false;
            for a in &m.actions {
                let ActionKind::Lookup { columns, rows } = &a.kind else { continue };
                let Some(out) = columns.iter().position(|c| c.lookup && c.node == p.node) else { continue };
                let Some(row) = rows.iter().find(|r| r.get(out).is_some_and(|v| v.eq_ignore_ascii_case(value.trim()))) else { continue };
                found = true;
                for (c, cell) in columns.iter().zip(row) {
                    match c.property.to_ascii_lowercase().as_str() {
                        "lookupstring" => lookups.push((c.node, cell.clone())),
                        "visibilitystate" => states.push((c.node, cell.clone())),
                        other => return Err(format!("{label} sets {other}, which CADCraft can't change yet.")),
                    }
                }
            }
            if !found {
                return Err(format!("\"{value}\" is not a value of {label}."));
            }
        }
        _ => return Err(format!("CADCraft can't change {name} yet.")),
    }
    apply(d, insert, &r.definition, &states, &lookups)
}

/// Give `insert` a new anonymous block with visibility `states` applied, and record the new
/// values (`states` and `lookups`) on it.
fn apply(d: &mut Drawing, insert: Handle, definition: &str, states: &[(i32, String)], lookups: &[(i32, String)]) -> Result<(), String> {
    let m = dynamic_block(d, definition).ok_or("Not a dynamic block reference.")?;
    let EntityKind::Insert(ins) = &d.entity(insert).ok_or("No such object.")?.kind else { return Err("Not a block reference.".into()) };
    let ins = ins.clone();
    let def = d.block(definition).cloned().ok_or("The dynamic block is missing.")?;
    let src = d.block(&ins.block).cloned().ok_or("The reference's block is missing.")?;
    // The dynamic block's objects by element tag, to find what each object of the reference's
    // block stands for.
    let def_by_tag: HashMap<i64, Handle> = by_tag(d, &def).into_iter().collect();
    let mut shown: HashMap<Handle, bool> = HashMap::new();
    for (node, state) in states {
        let Some(ParameterKind::Visibility { controlled, states: all, .. }) = m.parameters.iter().find(|p| p.node == *node).map(|p| &p.kind) else {
            continue;
        };
        let visible: HashSet<Handle> =
            all.iter().find(|s| s.name.eq_ignore_ascii_case(state)).map(|s| s.visible.iter().copied().collect()).unwrap_or_default();
        for h in controlled {
            shown.insert(*h, visible.contains(h));
        }
    }
    // The new anonymous block: the reference's block with the visibility applied.
    let name = d.anonymous_block_name("*U");
    let mut b = Block { name: name.clone(), anonymous: true, xref_path: None, array: None, entities: crate::EntityStore::new(), ..(*src).clone() };
    let mut copies = Vec::new();
    for (t, h) in by_tag(d, &src) {
        let Some(e) = src.entities.get(h) else { continue };
        let nh = d.new_handle();
        let mut e: Entity = (**e).clone();
        e.handle = nh;
        if let Some(v) = def_by_tag.get(&t).and_then(|dh| shown.get(dh)) {
            e.common.visible = *v;
        }
        copies.push((h, nh));
        b.entities.push(e);
    }
    // The dynamic block's record as the kept data names it; a block read without one gets a
    // handle that the writer points at its record.
    let def_record = Some(record_handle(d, definition).unwrap_or_else(|| {
        let h = d.new_handle();
        let f = std::sync::Arc::make_mut(&mut d.foreign);
        f.targets.insert(h, ("BLOCK_RECORD".to_string(), definition.to_ascii_uppercase()));
        h
    }));
    let old = ins.block.clone();
    d.blocks.insert(name.clone(), std::sync::Arc::new(b));
    let f = std::sync::Arc::make_mut(&mut d.foreign);
    for (old_h, new_h) in &copies {
        if let Some(x) = f.entities.get(old_h).cloned() {
            let mut x = x;
            for t in &mut x.xdata {
                if t.code == 1005 && Handle::parse_hex(&t.value) == Some(*old_h) {
                    t.value = new_h.hex();
                }
            }
            x.xdict = None;
            x.reactors.clear();
            f.entities.insert(*new_h, x);
        }
    }
    if let Some(rec) = def_record {
        let xdata = vec![tag(1001, BTAG), tag(1070, "1"), tag(1005, rec.hex())];
        f.records.insert(("BLOCK_RECORD".into(), name.to_ascii_uppercase()), ForeignExtra { xdata, ..Default::default() });
    }
    // Point the reference at it.
    d.modify_entity(insert, |e| {
        if let EntityKind::Insert(i) = &mut e.kind {
            i.block = name.clone();
        }
    })
    .map_err(|e| e.to_string())?;
    // The values it was given.
    let mut set: BTreeMap<i32, String> = BTreeMap::new();
    for (n, v) in states.iter().chain(lookups) {
        set.insert(*n, v.clone());
    }
    record_values(d, insert, def_record, &m, &set);
    drop_if_unused(d, &old);
    Ok(())
}

/// Record text values `set` (by parameter graph id) in `insert`'s representation data, creating
/// it when the reference has none. A new value record copies a record of the same parameter from
/// any reference in the drawing when there is one (AutoCAD's own layout), else a minimal one.
fn record_values(d: &mut Drawing, insert: Handle, def_record: Option<Handle>, m: &super::DynamicBlock, set: &BTreeMap<i32, String>) {
    let xdict = d.foreign.entities.get(&insert).and_then(|x| x.xdict);
    let rep = xdict.and_then(|x| entry(d, x, "AcDbBlockRepresentation"));
    let data = rep.and_then(|r| entry(d, r, "AppDataCache")).and_then(|c| entry(d, c, "ACAD_ENHANCEDBLOCKDATA"));
    // Templates: any reference's value record for each parameter (the values dictionaries are the
    // ACAD_ENHANCEDBLOCKDATA entries of the drawing's dictionaries).
    let mut templates: HashMap<String, Vec<RawTag>> = HashMap::new();
    for o in d.foreign.objects.iter().filter(|o| o.kind.eq_ignore_ascii_case("DICTIONARY")) {
        let Some(values) = entry(d, o.handle, "ACAD_ENHANCEDBLOCKDATA") else { continue };
        for (k, h) in entries(d, values) {
            if let Some(x) =
                d.foreign.objects.iter().find(|x| x.handle == h && x.kind.eq_ignore_ascii_case("XRECORD") && x.tags.iter().any(|t| t.code == 1))
            {
                templates.entry(k).or_insert_with(|| x.tags.clone());
            }
        }
    }
    let at = |node: i32| match m.parameters.iter().find(|p| p.node == node).map(|p| &p.kind) {
        Some(ParameterKind::Visibility { at, .. } | ParameterKind::Lookup { at, .. }) => *at,
        _ => cadcraft_geom::Vec3::ZERO,
    };
    let mut objects: Vec<ForeignObject> = Vec::new();
    let data = match data {
        Some(h) => h,
        None => {
            // Create the representation: reference xdict ▸ AcDbBlockRepresentation ▸ AcDbRepData
            // (the dynamic block's record) and AppDataCache ▸ ACAD_ENHANCEDBLOCKDATA.
            let Some(rec) = def_record else { return };
            let xd = xdict.unwrap_or_else(|| d.new_handle());
            let (rep, rd, cache, data) = (d.new_handle(), d.new_handle(), d.new_handle(), d.new_handle());
            let dict = |h: Handle, owner: Handle, entries: &[(&str, Handle)]| {
                let mut tags = vec![tag(5, h.hex()), tag(330, owner.hex()), tag(100, "AcDbDictionary"), tag(280, "1"), tag(281, "1")];
                for (k, v) in entries {
                    tags.push(tag(3, *k));
                    tags.push(tag(360, v.hex()));
                }
                ForeignObject { handle: h, kind: "DICTIONARY".into(), tags }
            };
            objects.push(dict(rep, xd, &[("AcDbRepData", rd), ("AppDataCache", cache)]));
            objects.push(ForeignObject {
                handle: rd,
                kind: "ACDB_BLOCKREPRESENTATION_DATA".into(),
                tags: vec![tag(5, rd.hex()), tag(330, rep.hex()), tag(100, "AcDbBlockRepresentationData"), tag(70, "1"), tag(340, rec.hex())],
            });
            objects.push(dict(cache, rep, &[("ACAD_ENHANCEDBLOCKDATA", data)]));
            objects.push(dict(data, cache, &[]));
            let f = std::sync::Arc::make_mut(&mut d.foreign);
            match f.objects.iter_mut().find(|o| Some(o.handle) == xdict) {
                Some(x) => {
                    x.tags.push(tag(3, "AcDbBlockRepresentation"));
                    x.tags.push(tag(360, rep.hex()));
                }
                None => {
                    objects.push(dict(xd, insert, &[("AcDbBlockRepresentation", rep)]));
                    f.entities.entry(insert).or_default().xdict = Some(xd);
                }
            }
            data
        }
    };
    let existing: HashMap<String, Handle> = match objects.iter().find(|o| o.handle == data) {
        Some(_) => HashMap::new(),
        None => entries(d, data).into_iter().collect(),
    };
    let mut add: Vec<(String, Handle)> = Vec::new();
    for (node, v) in set {
        let key = node.to_string();
        match existing.get(&key) {
            Some(h) => {
                let f = std::sync::Arc::make_mut(&mut d.foreign);
                if let Some(x) = f.objects.iter_mut().find(|o| o.handle == *h) {
                    match x.tags.iter_mut().find(|t| t.code == 1) {
                        Some(t) => t.value.clone_from(v),
                        None => x.tags.push(tag(1, v.clone())),
                    }
                }
            }
            None => {
                let h = d.new_handle();
                let mut tags = templates.get(&key).cloned().unwrap_or_else(|| {
                    let p = at(*node);
                    vec![
                        tag(100, "AcDbXrecord"),
                        tag(280, "1"),
                        tag(10, p.x.to_string()),
                        tag(20, p.y.to_string()),
                        tag(30, p.z.to_string()),
                        tag(1, ""),
                    ]
                });
                // Its own handle, owned by the values dictionary; the template's handle, owner and
                // reactors are not ours. Inside reactor brackets (102) everything goes.
                let mut inside = false;
                tags.retain(|t| {
                    if t.code == 102 {
                        inside = t.value.starts_with('{');
                        return false;
                    }
                    !(inside || t.code == 5 || t.code == 330)
                });
                let at = tags.iter().position(|t| t.code == 100).unwrap_or(0);
                tags.insert(at, tag(330, data.hex()));
                tags.insert(0, tag(5, h.hex()));
                match tags.iter_mut().find(|t| t.code == 1) {
                    Some(t) => t.value.clone_from(v),
                    None => tags.push(tag(1, v.clone())),
                }
                objects.push(ForeignObject { handle: h, kind: "XRECORD".into(), tags });
                add.push((key, h));
            }
        }
    }
    let f = std::sync::Arc::make_mut(&mut d.foreign);
    f.objects.extend(objects);
    if let Some(x) = f.objects.iter_mut().find(|o| o.handle == data) {
        for (k, h) in add {
            x.tags.push(tag(3, k));
            x.tags.push(tag(360, h.hex()));
        }
    }
}

/// Remove anonymous block `name` when nothing refers to it any more (the block a changed
/// reference used to show), with the data kept for its record.
fn drop_if_unused(d: &mut Drawing, name: &str) {
    if !name.starts_with('*') {
        return;
    }
    let refers = |e: &Entity| matches!(&e.kind, EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(name));
    let used = d.model.iter().any(|e| refers(e))
        || d.layouts.iter().any(|l| l.entities.iter().any(|e| refers(e)))
        || d.blocks.values().any(|b| b.entities.iter().any(|e| refers(e)));
    if used {
        return;
    }
    if let Some(b) = d.blocks.remove(name) {
        let f = std::sync::Arc::make_mut(&mut d.foreign);
        f.records.remove(&("BLOCK_RECORD".to_string(), name.to_ascii_uppercase()));
        f.records.remove(&("BLOCK".to_string(), name.to_ascii_uppercase()));
        for e in b.entities.iter() {
            f.entities.remove(&e.handle);
        }
    }
}
