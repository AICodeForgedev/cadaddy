//! Reading DWG with CADaddy's own codec alone (Q203): the file's header variables, classes, table
//! records, blocks, entities and objects, each decoded into the DXF groups the DXF reader builds
//! drawings from ([`cadaddy_dwgcodec::entities`]), put in a DXF's sections and read as one.
//!
//! Block records whose stored names repeat (files from some writers call every layout's block
//! *Paper_Space) get DXF's names: *Model_Space and *Paper_Space for the header's two spaces,
//! *Paper_Space0, 1, … for the other layouts' blocks in their tab order.

use std::collections::{HashMap, HashSet};

use cadaddy_doc::Drawing;
use cadaddy_dwgcodec::Dwg;
use cadaddy_dwgcodec::entities::records::Record;
use cadaddy_dwgcodec::entities::{Entity, Group, Value};
use cadaddy_dwgcodec::header::{HeaderVars, Value as HV};
use cadaddy_dxf::Tag;

use crate::dwg_native::tag;
use crate::{IoError, Result};

fn s(code: i32, v: impl Into<String>) -> Tag {
    Tag::s(code, v.into())
}

fn i(code: i32, v: i64) -> Tag {
    tag(&Group { code, value: Value::Int(v) })
}

fn f(code: i32, v: f64) -> Tag {
    tag(&Group { code, value: Value::Real(v) })
}

fn hex(code: i32, h: u64) -> Tag {
    tag(&Group { code, value: Value::Handle(h) })
}

/// What a header handle variable names: the DXF group and the names to look in.
fn named(var: &str) -> Option<i32> {
    Some(match var {
        "CLAYER" => 8,
        "TEXTSTYLE" | "DIMTXSTY" => 7,
        "CELTYPE" | "DIMLTYPE" | "DIMLTEX1" | "DIMLTEX2" => 6,
        "DIMSTYLE" | "CMLSTYLE" | "UCSNAME" | "PUCSNAME" | "UCSBASE" | "PUCSBASE" | "UCSORTHOREF" | "PUCSORTHOREF" => 2,
        "DIMBLK" | "DIMBLK1" | "DIMBLK2" | "DIMLDRBLK" => 1,
        _ => return None,
    })
}

/// The header variables as DXF's HEADER groups.
fn header(dwg: &Dwg, hv: &HeaderVars, name_of: &HashMap<u64, String>) -> Vec<Tag> {
    let mut out = vec![s(9, "$ACADVER"), s(1, dwg.version.id())];
    // The code page by its DXF name (the ones a DWG gives by number).
    let page = match dwg.code_page() {
        28 => Some("ANSI_1250"),
        29 => Some("ANSI_1251"),
        30 => Some("ANSI_1252"),
        31 => Some("ANSI_1253"),
        32 => Some("ANSI_1254"),
        33 => Some("ANSI_1255"),
        34 => Some("ANSI_1256"),
        35 => Some("ANSI_1257"),
        36 => Some("ANSI_874"),
        37 => Some("ANSI_932"),
        38 => Some("ANSI_936"),
        39 => Some("ANSI_949"),
        40 => Some("ANSI_950"),
        _ => None,
    };
    if let Some(p) = page {
        out.extend([s(9, "$DWGCODEPAGE"), s(3, p)]);
    }
    let var = |out: &mut Vec<Tag>, name: &str| out.push(s(9, format!("${name}")));
    for (name, value) in &hv.vars {
        if name.starts_with("UNKNOWN")
            || name.ends_with("_CONTROL")
            || name.starts_with("DICTIONARY_")
            || name.starts_with("BLOCK_RECORD_")
            || name.starts_with("LTYPE_BY")
        {
            continue;
        }
        match (*name, value) {
            ("FLAGS", HV::Int(flags)) => {
                let lw = cadaddy_dwgcodec::entities::lineweight((flags & 0x1f) as u8);
                for (n, t) in [
                    ("CELWEIGHT", i(370, lw)),
                    ("ENDCAPS", i(280, (flags >> 5) & 3)),
                    ("JOINSTYLE", i(280, (flags >> 7) & 3)),
                    ("LWDISPLAY", i(290, i64::from(flags & 0x200 == 0))),
                    ("XEDIT", i(290, i64::from(flags & 0x400 == 0))),
                    ("EXTNAMES", i(290, i64::from(flags & 0x800 != 0))),
                    ("PSTYLEMODE", i(290, i64::from(flags & 0x2000 != 0))),
                    ("OLESTARTUP", i(290, i64::from(flags & 0x4000 != 0))),
                ] {
                    var(&mut out, n);
                    out.push(t);
                }
            }
            ("HANDSEED", HV::Handle(h)) => {
                var(&mut out, name);
                out.push(hex(5, h.value));
            }
            (_, HV::Handle(h)) => {
                let Some(code) = named(name) else { continue };
                let Some(n) = name_of.get(&h.value) else { continue };
                var(&mut out, name);
                out.push(s(code, n.clone()));
            }
            (_, HV::Bool(b)) => {
                var(&mut out, name);
                out.push(i(70, i64::from(*b)));
            }
            (_, HV::Int(n)) => {
                var(&mut out, name);
                out.push(i(if i16::try_from(*n).is_ok() { 70 } else { 90 }, *n));
            }
            (_, HV::Double(x)) => {
                var(&mut out, name);
                out.push(f(40, *x));
            }
            (_, HV::Point2(p)) => {
                var(&mut out, name);
                out.extend([f(10, p[0]), f(20, p[1])]);
            }
            (_, HV::Point3(p)) => {
                var(&mut out, name);
                out.extend([f(10, p[0]), f(20, p[1]), f(30, p[2])]);
            }
            (_, HV::Text(t)) => {
                var(&mut out, name);
                out.push(s(1, t.to_string_lossy()));
            }
            (_, HV::Color(c)) => {
                let n = match c.rgb >> 24 {
                    0xC0 => 256,
                    0xC1 => 0,
                    0xC3 => i64::from(c.rgb & 0xFF),
                    _ if dwg.version < cadaddy_dwgcodec::Version::R2004 => i64::from(c.index),
                    _ => i64::from(c.index),
                };
                var(&mut out, name);
                out.push(i(62, n));
            }
            _ => {}
        }
    }
    // MEASUREMENT lives in the template section (spec §22).
    if let Some(t) = dwg.section("AcDb:Template")
        && let Some(m) = t.data.get(..)
        && m.len() >= 4
    {
        let len = usize::from(u16::from_le_bytes([m[0], m[1]]));
        if let Some(v) = m.get(2 + len..4 + len) {
            out.push(s(9, "$MEASUREMENT"));
            out.push(i(70, i64::from(u16::from_le_bytes([v[0], v[1]]))));
        }
    }
    out
}

/// An entity's record (`0 NAME` and its groups).
fn entity_record(e: &Entity) -> Vec<Tag> {
    let mut out = Vec::with_capacity(e.groups.len() + 1);
    out.push(s(0, e.name.clone()));
    out.extend(e.groups.iter().map(tag));
    out
}

/// What [`read`] left out.
#[derive(Debug, Default)]
pub struct ReadReport {
    /// Objects the codec couldn't decode, with why.
    pub failed: Vec<(u64, String)>,
}

/// Reads a DWG file with the codec alone.
pub fn read(bytes: &[u8]) -> Result<(Drawing, ReadReport)> {
    let fmt = |e: cadaddy_dwgcodec::Error| IoError::Format(format!("DWG: {e}"));
    let dwg = cadaddy_dwgcodec::read(bytes).map_err(fmt)?;
    let hv = dwg.header_vars().map_err(fmt)?;
    let classes = dwg.classes().map_err(fmt)?;
    let (ents, mut failed) = cadaddy_dwgcodec::entities::decode_all(&dwg).map_err(fmt)?;
    let (recs, rec_failed) = cadaddy_dwgcodec::entities::records::decode_all(&dwg).map_err(fmt)?;
    failed.extend(rec_failed);
    let lists = cadaddy_dwgcodec::entities::block_lists(&dwg, &ents).map_err(fmt)?;
    let space = |name: &str| match hv.get(name) {
        Some(HV::Handle(h)) if h.value != 0 => Some(h.value),
        _ => None,
    };
    let (model, paper) = (space("BLOCK_RECORD_MODEL_SPACE"), space("BLOCK_RECORD_PAPER_SPACE"));

    // Block record names, distinct as DXF needs them.
    let mut block_names: HashMap<u64, String> = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    let layouts: Vec<&Record> = recs.iter().filter(|r| r.name == "LAYOUT").collect();
    let layout_block =
        |l: &Record| l.groups.iter().filter(|g| g.code == 330).nth(1).and_then(|g| if let Value::Handle(h) = g.value { Some(h) } else { None });
    let tab = |l: &Record| l.groups.iter().find(|g| g.code == 71).and_then(|g| if let Value::Int(n) = g.value { Some(n) } else { None }).unwrap_or(0);
    let mut by_tab: Vec<&&Record> = layouts.iter().collect();
    by_tab.sort_by_key(|l| tab(l));
    let mut paper_n = 0;
    for l in by_tab {
        let Some(b) = layout_block(l) else { continue };
        let name = if Some(b) == model {
            "*Model_Space".to_string()
        } else if Some(b) == paper {
            "*Paper_Space".to_string()
        } else {
            let n = format!("*Paper_Space{paper_n}");
            paper_n += 1;
            n
        };
        taken.insert(name.to_ascii_uppercase());
        block_names.insert(b, name);
    }
    for r in recs.iter().filter(|r| r.name == "BLOCK_RECORD") {
        if block_names.contains_key(&r.handle) {
            continue;
        }
        let stored =
            r.groups.iter().find(|g| g.code == 2).and_then(|g| if let Value::Str(s) = &g.value { Some(s.clone()) } else { None }).unwrap_or_default();
        let mut name = stored.clone();
        let mut k = 0;
        while taken.contains(&name.to_ascii_uppercase()) {
            name = format!("{stored}{k}");
            k += 1;
        }
        taken.insert(name.to_ascii_uppercase());
        block_names.insert(r.handle, name);
    }

    // Names by handle, for the header's handle variables.
    let mut name_of: HashMap<u64, String> = HashMap::new();
    for r in &recs {
        if let Some(n) = r.groups.iter().find(|g| g.code == 2).and_then(|g| if let Value::Str(s) = &g.value { Some(s.clone()) } else { None }) {
            name_of.insert(r.handle, n);
        }
    }
    name_of.extend(block_names.iter().map(|(h, n)| (*h, n.clone())));

    let mut tags: Vec<Tag> = Vec::new();
    let section = |tags: &mut Vec<Tag>, name: &str| tags.extend([s(0, "SECTION"), s(2, name)]);
    let end = |tags: &mut Vec<Tag>| tags.push(s(0, "ENDSEC"));

    section(&mut tags, "HEADER");
    tags.extend(header(&dwg, &hv, &name_of));
    end(&mut tags);

    section(&mut tags, "CLASSES");
    for c in &classes.classes {
        tags.extend([
            s(0, "CLASS"),
            s(1, c.dxf_name.to_string_lossy()),
            s(2, c.cpp_name.to_string_lossy()),
            s(3, c.app_name.to_string_lossy()),
            i(90, i64::from(c.proxy_flags)),
            i(91, i64::from(c.instances)),
            i(280, i64::from(c.was_zombie)),
            i(281, i64::from(c.item_class_id == 0x1F2)),
        ]);
    }
    end(&mut tags);

    // Tables, in DXF's order, each record in the file's order.
    section(&mut tags, "TABLES");
    for table in ["VPORT", "LTYPE", "LAYER", "STYLE", "VIEW", "UCS", "APPID", "DIMSTYLE", "BLOCK_RECORD"] {
        let rows: Vec<&Record> = recs.iter().filter(|r| r.name == table).collect();
        tags.extend([s(0, "TABLE"), s(2, table), s(100, "AcDbSymbolTable"), i(70, rows.len() as i64)]);
        if table == "DIMSTYLE" {
            tags.push(s(100, "AcDbDimStyleTable"));
        }
        for r in rows {
            tags.push(s(0, r.name.clone()));
            let rename = (table == "BLOCK_RECORD").then(|| block_names.get(&r.handle)).flatten();
            tags.extend(r.groups.iter().map(|g| match (rename, g.code) {
                (Some(n), 2) => s(2, n.clone()),
                _ => tag(g),
            }));
        }
        tags.push(s(0, "ENDTAB"));
    }
    end(&mut tags);

    // Entities by handle; their owners' lists give the order.
    let by_handle: HashMap<u64, &Entity> = ents.iter().map(|e| (e.handle, e)).collect();
    let owned: HashSet<u64> = ents.iter().flat_map(|e| e.children.iter().copied()).collect();
    // A layout's viewports are numbered in their order in its block (DXF 68 and 69 aren't stored
    // in DWG): the first, 1, is the sheet's own.
    let emit = |tags: &mut Vec<Tag>, list: &[u64]| {
        let mut viewport = 0;
        for h in list {
            if owned.contains(h) {
                continue;
            }
            let Some(e) = by_handle.get(h) else { continue };
            tags.extend(entity_record(e));
            if e.name == "VIEWPORT" {
                viewport += 1;
                tags.extend([i(68, viewport), i(69, viewport)]);
            }
            for c in &e.children {
                if let Some(child) = by_handle.get(c) {
                    tags.extend(entity_record(child));
                }
            }
        }
    };

    section(&mut tags, "BLOCKS");
    for r in recs.iter().filter(|r| r.name == "BLOCK_RECORD") {
        let Some(b) = &r.block else { continue };
        let name = block_names.get(&r.handle).cloned().unwrap_or_default();
        let paper_block = Some(r.handle) == paper;
        tags.extend([s(0, "BLOCK"), hex(5, b.block_entity.unwrap_or(0)), hex(330, r.handle), s(100, "AcDbEntity")]);
        if paper_block {
            tags.push(i(67, 1));
        }
        tags.extend([
            s(8, "0"),
            s(100, "AcDbBlockBegin"),
            s(2, name.clone()),
            i(70, b.flags),
            f(10, b.base[0]),
            f(20, b.base[1]),
            f(30, b.base[2]),
            s(3, name),
            s(1, b.xref_path.clone()),
        ]);
        if !b.description.is_empty() {
            tags.push(s(4, b.description.clone()));
        }
        if Some(r.handle) != model && !paper_block {
            emit(&mut tags, lists.get(&r.handle).map(Vec::as_slice).unwrap_or_default());
        }
        tags.extend([s(0, "ENDBLK"), hex(5, b.endblk.unwrap_or(0)), hex(330, r.handle), s(100, "AcDbEntity")]);
        if paper_block {
            tags.push(i(67, 1));
        }
        tags.extend([s(8, "0"), s(100, "AcDbBlockEnd")]);
    }
    end(&mut tags);

    section(&mut tags, "ENTITIES");
    for space in [model, paper].into_iter().flatten() {
        emit(&mut tags, lists.get(&space).map(Vec::as_slice).unwrap_or_default());
    }
    end(&mut tags);

    // Objects: the named object dictionary first.
    section(&mut tags, "OBJECTS");
    let root = space("DICTIONARY_NAMED_OBJECTS");
    let objects: Vec<&Record> = recs.iter().filter(|r| !is_table_record(&r.name)).collect();
    for r in objects.iter().filter(|r| Some(r.handle) == root).chain(objects.iter().filter(|r| Some(r.handle) != root)) {
        tags.push(s(0, r.name.clone()));
        tags.extend(r.groups.iter().map(tag));
    }
    end(&mut tags);
    tags.push(s(0, "EOF"));

    let tags = crate::dwg_keep::unstash_tags(&tags).unwrap_or(tags);
    let mut d = crate::dxf_read::read_tags(tags)?;
    if !failed.is_empty() {
        let mut kinds: Vec<String> = failed.iter().map(|(_, e)| e.split(':').next().unwrap_or("?").to_string()).collect();
        kinds.sort();
        kinds.dedup();
        d.read_notes.push(format!(
            "DWG: {} records couldn't be read and were left out ({}); saving over the original file would lose them",
            failed.len(),
            kinds.join(", ")
        ));
    }
    Ok((d, ReadReport { failed }))
}

fn is_table_record(name: &str) -> bool {
    matches!(name, "VPORT" | "LTYPE" | "LAYER" | "STYLE" | "VIEW" | "UCS" | "APPID" | "DIMSTYLE" | "BLOCK_RECORD")
}
