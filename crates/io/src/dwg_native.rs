//! DWG entities read with CADaddy's own codec ([`cadaddy_dwgcodec::entities`]): each decoded
//! straight into the DXF groups the DXF reader builds entities from.
//!
//! Until the codec also reads tables and objects (Q200), the rest of the drawing comes from the
//! DWG bridge: [`read_with_native_entities`] reads the file through the bridge into groups, puts
//! the codec's groups in place of every entity it decoded (an entity it couldn't keeps the
//! bridge's), and builds the drawing from that. Comparing that drawing with the bridge's alone
//! shows, entity by entity, that the codec reads what the bridge reads.

use std::collections::{HashMap, HashSet};

use cadaddy_doc::Drawing;
use cadaddy_dwgcodec::entities::records::Record;
use cadaddy_dwgcodec::entities::{Entity, Group, Value, decode_all};
use cadaddy_dxf::{Kind, Tag, kind_of};

use crate::{IoError, Result};

/// A group as a DXF tag, typed as the DXF parser types its code.
pub(crate) fn tag(g: &Group) -> Tag {
    let text = || match &g.value {
        Value::Str(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Handle(h) => format!("{h:X}"),
    };
    let value = match (kind_of(g.code), &g.value) {
        (Kind::Real, Value::Real(f)) => cadaddy_dxf::Value::Real(if f.is_finite() { *f } else { 0.0 }),
        (Kind::Real, Value::Int(i)) => cadaddy_dxf::Value::Real(*i as f64),
        (Kind::Int16 | Kind::Int32 | Kind::Int64, Value::Int(i)) => cadaddy_dxf::Value::Int(*i),
        (Kind::Int16 | Kind::Int32 | Kind::Int64, Value::Real(f)) if f.is_finite() => cadaddy_dxf::Value::Int(*f as i64),
        (Kind::Bool, Value::Int(i)) => cadaddy_dxf::Value::Bool(*i != 0),
        (Kind::Hex, _) => cadaddy_dxf::Value::Hex(text()),
        _ => cadaddy_dxf::Value::Str(text()),
    };
    Tag::new(g.code, value)
}

/// An entity's record: `0 NAME` and its groups.
fn record(e: &Entity) -> Vec<Tag> {
    let mut out = Vec::with_capacity(e.groups.len() + 1);
    out.push(Tag::s(0, e.name.clone()));
    out.extend(e.groups.iter().map(tag));
    out
}

/// The entity records of `tags` (sections ENTITIES and BLOCKS) with every entity in `ents` in
/// place of the record with its handle, followed by the entities it owns.
fn replace(tags: Vec<Tag>, ents: &[Entity], recs: &[Record]) -> (Vec<Tag>, usize) {
    let by_handle: HashMap<u64, &Entity> = ents.iter().map(|e| (e.handle, e)).collect();
    let records: HashMap<u64, &Record> = recs.iter().map(|r| (r.handle, r)).collect();
    let owned: HashSet<u64> = ents.iter().flat_map(|e| e.children.iter().copied()).collect();
    let mut out = Vec::with_capacity(tags.len());
    let mut section = String::new();
    let mut replaced = 0usize;
    let mut i = 0;
    while i < tags.len() {
        let Some(t) = tags.get(i) else { break };
        if t.code != 0 {
            out.push(t.clone());
            i += 1;
            continue;
        }
        // A record: up to the next code 0.
        let end = tags.get(i + 1..).and_then(|rest| rest.iter().position(|x| x.code == 0)).map_or(tags.len(), |p| i + 1 + p);
        let rec = tags.get(i..end).unwrap_or_default();
        let name = t.str().trim().to_ascii_uppercase();
        if name == "SECTION" {
            section = rec.iter().find(|x| x.code == 2).map(|x| x.str().trim().to_string()).unwrap_or_default();
        }
        let in_entities = section == "ENTITIES" || section == "BLOCKS";
        let handle = rec.iter().find(|x| x.code == 5 || x.code == 105).and_then(|x| u64::from_str_radix(x.str().trim(), 16).ok());
        // Table records and objects: the codec's in place of the bridge's.
        if (section == "TABLES" && !matches!(name.as_str(), "TABLE" | "ENDTAB")) || section == "OBJECTS" {
            match handle.and_then(|h| records.get(&h)) {
                Some(r) => {
                    replaced += 1;
                    out.push(Tag::s(0, r.name.clone()));
                    // A block record's name: the bridge's (files from some writers call every
                    // layout's block *Paper_Space; DXF needs them distinct, *Paper_Space0 …).
                    let bridge_name = (r.name == "BLOCK_RECORD").then(|| rec.iter().find(|x| x.code == 2).cloned()).flatten();
                    out.extend(r.groups.iter().map(|g| match (&bridge_name, g.code) {
                        (Some(n), 2) => n.clone(),
                        _ => tag(g),
                    }));
                }
                None => out.extend_from_slice(rec),
            }
            i = end;
            continue;
        }
        match handle.filter(|_| in_entities && !matches!(name.as_str(), "BLOCK" | "ENDBLK")) {
            // Written with its owner.
            Some(h) if owned.contains(&h) && by_handle.contains_key(&h) => {}
            Some(h) => match by_handle.get(&h) {
                Some(e) => {
                    replaced += 1;
                    let mut r = record(e);
                    // A viewport's status and id aren't in the DWG data: the bridge's, until the
                    // layouts are read by the codec too.
                    if e.name == "VIEWPORT" {
                        r.extend(rec.iter().filter(|x| matches!(x.code, 68 | 69)).cloned());
                    }
                    out.extend(r);
                    for c in &e.children {
                        if let Some(child) = by_handle.get(c) {
                            out.extend(record(child));
                        }
                    }
                }
                None => out.extend_from_slice(rec),
            },
            None => out.extend_from_slice(rec),
        }
        i = end;
    }
    (out, replaced)
}

/// What [`read_with_native_entities`] did besides reading.
#[derive(Debug, Default)]
pub struct NativeReport {
    /// Entity records taken from the codec.
    pub replaced: usize,
    /// Entities the codec couldn't decode (kept from the bridge), and why.
    pub failed: Vec<(u64, String)>,
}

/// A DWG file read through the bridge, with its entities, table records and objects as
/// CADaddy's codec decodes them.
pub fn read_with_native_entities(bytes: &[u8]) -> Result<(Drawing, NativeReport)> {
    let (dxf, notes) = cadaddy_dwg::dwg_to_dxf_noted(bytes, cadaddy_dwg::Limits::DEFAULT).map_err(IoError::Format)?;
    let tags = cadaddy_dxf::parse(&dxf).map_err(|e| IoError::Format(e.to_string()))?;
    let dwg = cadaddy_dwgcodec::read(bytes).map_err(|e| IoError::Format(format!("DWG: {e}")))?;
    let (ents, mut failed) = decode_all(&dwg).map_err(|e| IoError::Format(format!("DWG: {e}")))?;
    let (recs, rec_failed) = cadaddy_dwgcodec::entities::records::decode_all(&dwg).map_err(|e| IoError::Format(format!("DWG: {e}")))?;
    failed.extend(rec_failed);
    let (tags, replaced) = replace(tags, &ents, &recs);
    let mut d = crate::dxf_read::read_tags(tags)?;
    d.read_notes = notes;
    Ok((d, NativeReport { replaced, failed }))
}
