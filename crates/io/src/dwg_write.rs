//! Writing DWG with CADaddy's own codec (Q201): the drawing's DXF groups (as the DXF writer makes
//! them, in memory) encoded record by record into DWG objects ([`cadaddy_dwgcodec::entities::
//! encode_kinds`]), the header variables and classes from the HEADER and CLASSES sections, and a
//! new file's containers ([`cadaddy_dwgcodec::Dwg::new`]).
//!
//! Records whose type isn't encoded yet are kept in XRECORDs (`dwg_keep`) that reading puts back.

use std::collections::{HashMap, HashSet};

use cadaddy_doc::Drawing;
use cadaddy_dwgcodec::Version;
use cadaddy_dwgcodec::bits::{HandleRef, Text};
use cadaddy_dwgcodec::classes::{Class, Classes};
use cadaddy_dwgcodec::entities::encode::{EntityHead, Handles, Rec};
use cadaddy_dwgcodec::entities::encode_kinds::{self as ek, types};
use cadaddy_dwgcodec::entities::{Group, Value};
use cadaddy_dwgcodec::header::{HeaderVars, Value as HV};
use cadaddy_dwgcodec::objects::RawObject;
use cadaddy_dxf::Tag;

use crate::{IoError, Result};

/// A DXF record: its name (the 0 group) and groups; the reactors and extension dictionary (DXF's
/// 102 blocks) apart.
#[derive(Default)]
struct Record {
    name: String,
    groups: Vec<Group>,
    reactors: Vec<u64>,
    xdict: Option<u64>,
}

impl Record {
    fn new(name: String) -> Record {
        Record { name, ..Record::default() }
    }
    fn rec(&self) -> Rec<'_> {
        Rec(&self.groups)
    }
    fn handle(&self) -> Option<u64> {
        self.rec().h(5).or_else(|| self.rec().h(105))
    }
    /// The owner (the first 330 outside the 102 blocks).
    fn owner(&self) -> Option<u64> {
        self.rec().h(330).filter(|h| *h != 0)
    }
    /// Takes the 102 blocks out of the groups.
    fn split_blocks(&mut self) {
        let mut kept = Vec::with_capacity(self.groups.len());
        let mut inside: Option<String> = None;
        for g in self.groups.drain(..) {
            match (&inside, g.code, &g.value) {
                (None, 102, Value::Str(s)) if s.starts_with('{') => inside = Some(s.clone()),
                (Some(_), 102, _) => inside = None,
                (Some(b), 330, v) if b.eq_ignore_ascii_case("{ACAD_REACTORS") => self.reactors.extend(handle_value(v)),
                (Some(b), 360, v) if b.eq_ignore_ascii_case("{ACAD_XDICTIONARY") => self.xdict = handle_value(v),
                (Some(_), _, _) => {}
                (None, _, _) => kept.push(g),
            }
        }
        self.groups = kept;
    }
}

fn group(t: &Tag) -> Group {
    let value = match &t.value {
        cadaddy_dxf::Value::Real(f) => Value::Real(*f),
        cadaddy_dxf::Value::Int(i) => Value::Int(*i),
        cadaddy_dxf::Value::Bool(b) => Value::Int(i64::from(*b)),
        cadaddy_dxf::Value::Hex(s) | cadaddy_dxf::Value::Str(s) => Value::Str(s.clone()),
    };
    Group { code: t.code, value }
}

/// The records of each section, in order.
fn sections(tags: &[Tag]) -> HashMap<String, Vec<Record>> {
    let mut out: HashMap<String, Vec<Record>> = HashMap::new();
    let mut section = String::new();
    let mut cur: Option<Record> = None;
    let mut want = false;
    for t in tags {
        if t.code == 0 {
            if let Some(r) = cur.take() {
                out.entry(section.clone()).or_default().push(r);
            }
            let name = t.str().trim().to_string();
            match name.as_str() {
                "SECTION" => want = true,
                "ENDSEC" | "EOF" => {}
                _ => cur = Some(Record::new(name)),
            }
            continue;
        }
        if want && t.code == 2 {
            section = t.str().trim().to_string();
            want = false;
            continue;
        }
        if section == "HEADER" {
            // Header variables: one record per $NAME.
            if t.code == 9 {
                if let Some(r) = cur.take() {
                    out.entry(section.clone()).or_default().push(r);
                }
                cur = Some(Record::new(t.str().trim().trim_start_matches('$').to_string()));
                continue;
            }
        }
        if let Some(r) = cur.as_mut() {
            r.groups.push(group(t));
        }
    }
    if let Some(r) = cur.take() {
        out.entry(section).or_default().push(r);
    }
    for (name, recs) in out.iter_mut() {
        if name != "HEADER" {
            for r in recs {
                r.split_blocks();
            }
        }
    }
    out
}

/// The DWG version a DXF/DWG version code names.
fn version(code: &str) -> Option<(Version, crate::Version)> {
    Some(match code {
        "AC1015" => (Version::R2000, crate::Version::R2000),
        "AC1018" => (Version::R2004, crate::Version::R2004),
        "AC1021" => (Version::R2007, crate::Version::R2007),
        "AC1024" => (Version::R2010, crate::Version::R2010),
        "AC1027" => (Version::R2013, crate::Version::R2013),
        "AC1032" => (Version::R2018, crate::Version::R2018),
        _ => return None,
    })
}

/// What [`write`] left out.
#[derive(Debug, Default)]
pub struct WriteReport {
    /// Record types not encoded yet, with how many of each were left out.
    pub skipped: Vec<(String, usize)>,
    /// Records not encoded, kept in the CADADDY_KEEP dictionary instead.
    pub kept: usize,
}

/// The drawing as a DWG file of `version_code` (AC1015, AC1018, AC1021, AC1024, AC1027 or AC1032).
pub fn write(d: &Drawing, version_code: &str) -> Result<(Vec<u8>, WriteReport)> {
    let (v, dv) = version(version_code).ok_or_else(|| IoError::Format(format!("DWG: writing {version_code} files is not supported")))?;
    let mut tags = crate::dxf_write::tags(d, dv);
    // Records not encoded are kept in XRECORDs of a CADADDY_KEEP dictionary (`dwg_keep`, as the
    // DWG bridge keeps what it can't write), which reading puts back.
    let (_, written) = assemble(v, &sections(&tags), false)?;
    let take: HashSet<u64> = sections(&tags)
        .iter()
        .filter(|(name, _)| matches!(name.as_str(), "BLOCKS" | "ENTITIES" | "OBJECTS"))
        .flat_map(|(_, recs)| recs.iter())
        .filter_map(Record::handle)
        .filter(|h| !written.contains(h))
        .collect();
    let mut kept = 0;
    if let Some((stashed, n)) = crate::dwg_keep::stash_tags(&tags, &take) {
        tags = stashed;
        kept = n;
    }
    let mut secs = sections(&tags);
    let ((mut bytes, mut report), written) = assemble(v, &secs, true)?;
    // Records still left out: made again without the references to them (dictionary entries,
    // reactors, extension dictionaries), so that none points at nothing.
    if !report.skipped.is_empty() {
        prune(&mut secs, &written);
        (bytes, report) = assemble(v, &secs, true)?.0;
    }
    report.kept = kept;
    Ok((bytes, report))
}

/// Takes out of the records the references to objects not in `written`.
fn prune(secs: &mut HashMap<String, Vec<Record>>, written: &std::collections::HashSet<u64>) {
    for recs in secs.values_mut() {
        for r in recs.iter_mut() {
            r.reactors.retain(|h| written.contains(h));
            r.xdict = r.xdict.filter(|h| written.contains(h));
            if matches!(r.name.as_str(), "DICTIONARY" | "ACDBDICTIONARYWDFLT") {
                // Entries are a name (3) and its object (350/360).
                let mut kept = Vec::with_capacity(r.groups.len());
                let mut pending: Option<Group> = None;
                for g in r.groups.drain(..) {
                    match g.code {
                        3 => pending = Some(g),
                        350 | 360 => {
                            if let Some(name) = pending.take()
                                && handle_value(&g.value).is_some_and(|h| written.contains(&h))
                            {
                                kept.push(name);
                                kept.push(g);
                            }
                        }
                        _ => {
                            kept.extend(pending.take());
                            kept.push(g);
                        }
                    }
                }
                kept.extend(pending);
                r.groups = kept;
            }
        }
    }
}

type Assembled = ((Vec<u8>, WriteReport), std::collections::HashSet<u64>);

/// The file from the sections' records, and the handles of the objects in it.
fn assemble(v: Version, secs: &HashMap<String, Vec<Record>>, full: bool) -> Result<Assembled> {
    let empty = Vec::new();
    let tables = secs.get("TABLES").unwrap_or(&empty);
    let blocks = secs.get("BLOCKS").unwrap_or(&empty);
    let entities = secs.get("ENTITIES").unwrap_or(&empty);
    let objects = secs.get("OBJECTS").unwrap_or(&empty);

    // Names to handles, and each table's records.
    let mut names = Handles::default();
    let mut controls: Vec<(String, u64, Vec<u64>)> = Vec::new();
    let mut current: Option<usize> = None;
    let mut table_records: Vec<(&Record, String, u64)> = Vec::new();
    for r in tables {
        match r.name.as_str() {
            "TABLE" => {
                let table = r.rec().s(2).unwrap_or("").to_ascii_uppercase();
                controls.push((table, r.handle().unwrap_or(0), Vec::new()));
                current = Some(controls.len() - 1);
            }
            "ENDTAB" => current = None,
            _ => {
                let (Some(c), Some(h)) = (current.and_then(|i| controls.get_mut(i)), r.handle()) else { continue };
                c.2.push(h);
                let table = c.0.clone();
                names.insert(&table, r.rec().s(2).unwrap_or(""), h);
                table_records.push((r, table, h));
            }
        }
    }
    // Classes, numbered from 500 in order.
    let mut class_list = Vec::new();
    for (i, r) in secs.get("CLASSES").unwrap_or(&empty).iter().filter(|r| r.name == "CLASS").enumerate() {
        let Ok(number) = i16::try_from(500 + i) else { break };
        let rec = r.rec();
        let dxf_name = rec.s(1).unwrap_or("").to_string();
        names.classes.insert(dxf_name.clone(), number as u16);
        let is_entity = rec.id(281, 0) != 0;
        class_list.push(Class {
            number,
            proxy_flags: rec.id(90, 0) as i16,
            app_name: text(v, rec.s(3).unwrap_or("")),
            cpp_name: text(v, rec.s(2).unwrap_or("")),
            dxf_name: text(v, &dxf_name),
            was_zombie: rec.id(280, 0) != 0,
            item_class_id: if is_entity { 0x1F2 } else { 0x1F3 },
            instances: rec.id(91, 0) as i32,
            dwg_version: 0,
            maintenance: 0,
            unknown: [0, 0],
        });
    }
    let block_record = |name: &str| names.get("BLOCK_RECORD", name);
    let model_br = block_record("*Model_Space");
    let paper_br = block_record("*Paper_Space");

    // The entities. R2000 links each entity to its neighbours in its block (or its owner's
    // attributes or vertices): a first pass finds the lists, a second writes the links.
    let mut pass = entity_pass(v, blocks, entities, &names, (model_br, paper_br), &HashMap::new());
    if v < Version::R2004 {
        let mut links: Links = HashMap::new();
        for list in pass.block_parts.values().map(|p| &p.ents).chain(&pass.kid_lists) {
            for (k, h) in list.iter().enumerate() {
                let prev = k.checked_sub(1).and_then(|j| list.get(j)).copied();
                links.insert(*h, (prev, list.get(k + 1).copied()));
            }
        }
        pass = entity_pass(v, blocks, entities, &names, (model_br, paper_br), &links);
    }
    let EntityPass { mut out, mut block_parts, viewports_of, mut skipped, .. } = pass;

    // Layouts by their block record (for the block headers), from OBJECTS.
    let mut layout_of: HashMap<u64, u64> = HashMap::new();
    for r in objects.iter().filter(|r| r.name == "LAYOUT") {
        if let (Some(h), Some(b)) = (r.handle(), r.rec().h(330).and_then(|_| r.rec().all(330).nth(1).and_then(|g| handle_value(&g.value)))) {
            layout_of.insert(b, h);
        }
    }

    // Table records and controls.
    for (r, table, h) in &table_records {
        let owner = r.owner();
        match table.as_str() {
            "BLOCK_RECORD" => {
                let b = block_parts.remove(h).unwrap_or_default();
                let head = ek::BlockHead {
                    block: b.block,
                    endblk: b.endblk,
                    entities: &b.ents,
                    layout: layout_of.get(h).copied().or(r.rec().h(340)),
                    flags70: b.flags,
                    base: b.base,
                    xref_path: &b.xref_path,
                    description: &b.description,
                };
                out.push(ek::block_header(v, *h, owner, &r.rec(), head, &names));
            }
            _ => match ek::table_record(v, &r.name, *h, owner, &r.rec(), &names) {
                Some(o) => out.push(o),
                None => *skipped.entry(r.name.clone()).or_default() += 1,
            },
        }
    }
    let mut control_handles: HashMap<String, u64> = HashMap::new();
    let encoded_set: HashSet<u64> = out.iter().map(|o| o.handle).collect();
    for (table, h, entries) in &controls {
        let t = match table.as_str() {
            "BLOCK_RECORD" => types::BLOCK_CONTROL,
            "LAYER" => types::LAYER_CONTROL,
            "STYLE" => types::STYLE_CONTROL,
            "LTYPE" => types::LTYPE_CONTROL,
            "VIEW" => types::VIEW_CONTROL,
            "UCS" => types::UCS_CONTROL,
            "VPORT" => types::VPORT_CONTROL,
            "APPID" => types::APPID_CONTROL,
            "DIMSTYLE" => types::DIMSTYLE_CONTROL,
            _ => continue,
        };
        control_handles.insert(table.clone(), *h);
        // Entries the control keeps apart: the spaces' block records, the ByLayer/ByBlock
        // linetypes; and entries this writer doesn't encode yet.
        let encoded = |e: &u64| encoded_set.contains(e);
        let (special, list): (Vec<u64>, Vec<u64>) = match table.as_str() {
            "BLOCK_RECORD" => {
                let sp: Vec<u64> = [model_br, paper_br].into_iter().flatten().collect();
                (sp.clone(), entries.iter().copied().filter(|e| !sp.contains(e)).collect())
            }
            "LTYPE" => {
                let sp: Vec<u64> = [names.get("LTYPE", "ByLayer"), names.get("LTYPE", "ByBlock")].into_iter().flatten().collect();
                (sp.clone(), entries.iter().copied().filter(|e| !sp.contains(e)).collect())
            }
            _ => (Vec::new(), entries.clone()),
        };
        let list: Vec<u64> = list.into_iter().filter(encoded).collect();
        out.push(ek::control(v, t, *h, &list, &special));
    }

    // Objects.
    let mut root: Option<u64> = None;
    let mut dict_entries: HashMap<u64, Vec<(String, u64)>> = HashMap::new();
    for r in objects {
        let Some(h) = r.handle() else { continue };
        let rec = r.rec();
        if r.name == "DICTIONARY" {
            root.get_or_insert(h);
            let keys = rec.all(3).filter_map(|g| if let Value::Str(s) = &g.value { Some(s.clone()) } else { None });
            let values = rec.0.iter().filter(|g| g.code == 350 || g.code == 360).filter_map(|g| handle_value(&g.value));
            dict_entries.insert(h, keys.zip(values).collect());
        }
        let block = rec.all(330).nth(1).and_then(|g| handle_value(&g.value));
        let vps = if r.name == "LAYOUT" { block.and_then(|b| viewports_of.get(&b)).cloned().unwrap_or_default() } else { Vec::new() };
        match ek::object(v, &r.name, h, r.owner(), &r.reactors, r.xdict, &rec, &names, &vps) {
            Some(o) => out.push(o),
            None => *skipped.entry(r.name.clone()).or_default() += 1,
        }
    }
    // Dictionary entries that point at objects not written: left out of their dictionaries.
    // (The dictionaries were written with them; readers skip what they can't find.)

    // Only what was written, when that is all that's asked.
    if !full {
        let mut skipped: Vec<(String, usize)> = skipped.into_iter().collect();
        skipped.sort();
        return Ok(((Vec::new(), WriteReport { skipped, kept: 0 }), out.iter().map(|o| o.handle).collect()));
    }
    let classes = Classes::new(v, class_list);

    // The header variables.
    let header = secs.get("HEADER").unwrap_or(&empty);
    let mut hv = HeaderVars::defaults(v);
    let metric = header.iter().find(|r| r.name == "MEASUREMENT").and_then(|r| r.rec().i(70)) == Some(1);
    new_drawing_defaults(&mut hv, metric);
    // Multiline styles by name, for CMLSTYLE.
    let mut mline_styles: HashMap<String, u64> = HashMap::new();
    for r in objects.iter().filter(|r| r.name == "MLINESTYLE") {
        if let (Some(h), Some(n)) = (r.handle(), r.rec().s(2)) {
            mline_styles.insert(n.to_ascii_uppercase(), h);
        }
    }
    for r in header {
        set_header(&mut hv, r, &names, &mline_styles);
    }
    hv.set("FLAGS", HV::Int(flags(header)));
    let href = |h: Option<u64>| HV::Handle(HandleRef { code: 5, value: h.unwrap_or(0) });
    let soft = |h: Option<u64>| HV::Handle(HandleRef { code: 3, value: h.unwrap_or(0) });
    for (var, table) in [
        ("BLOCK_CONTROL", "BLOCK_RECORD"),
        ("LAYER_CONTROL", "LAYER"),
        ("STYLE_CONTROL", "STYLE"),
        ("LINETYPE_CONTROL", "LTYPE"),
        ("VIEW_CONTROL", "VIEW"),
        ("UCS_CONTROL", "UCS"),
        ("VPORT_CONTROL", "VPORT"),
        ("APPID_CONTROL", "APPID"),
        ("DIMSTYLE_CONTROL", "DIMSTYLE"),
    ] {
        hv.set(var, soft(control_handles.get(table).copied()));
    }
    hv.set("DICTIONARY_NAMED_OBJECTS", soft(root));
    if let Some(entries) = root.and_then(|r| dict_entries.get(&r)) {
        let find = |n: &str| entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, h)| *h);
        for (var, key) in [
            ("DICTIONARY_ACAD_GROUP", "ACAD_GROUP"),
            ("DICTIONARY_ACAD_MLINESTYLE", "ACAD_MLINESTYLE"),
            ("DICTIONARY_LAYOUTS", "ACAD_LAYOUT"),
            ("DICTIONARY_PLOTSETTINGS", "ACAD_PLOTSETTINGS"),
            ("DICTIONARY_PLOTSTYLES", "ACAD_PLOTSTYLENAME"),
            ("DICTIONARY_MATERIALS", "ACAD_MATERIAL"),
            ("DICTIONARY_COLORS", "ACAD_COLOR"),
            ("DICTIONARY_VISUALSTYLE", "ACAD_VISUALSTYLE"),
        ] {
            hv.set(var, soft(find(key)));
        }
    }
    hv.set("BLOCK_RECORD_PAPER_SPACE", soft(paper_br));
    hv.set("BLOCK_RECORD_MODEL_SPACE", soft(model_br));
    hv.set("LTYPE_BYLAYER", soft(names.get("LTYPE", "ByLayer")));
    hv.set("LTYPE_BYBLOCK", soft(names.get("LTYPE", "ByBlock")));
    hv.set("LTYPE_CONTINUOUS", soft(names.get("LTYPE", "Continuous")));
    // R2000: the viewport entity header control (empty), which readers look up for every
    // viewport; without it the ODA's reader rejects a file with a paper space viewport.
    if v == Version::R2000 {
        let h = out.iter().map(|o| o.handle).max().unwrap_or(0) + 1;
        out.push(ek::control(v, types::VX_CONTROL, h, &[], &[]));
        hv.set("VIEWPORT_ENTITY_HEADER_CONTROL", soft(Some(h)));
    }
    let seed = out.iter().map(|o| o.handle).max().unwrap_or(0) + 1;
    hv.set("HANDSEED", HV::Handle(HandleRef { code: 0, value: seed }));
    let _ = href;

    let written = out.iter().map(|o| o.handle).collect();
    let mut dwg = cadaddy_dwgcodec::Dwg::new(v, &hv, &classes, out).map_err(|e| IoError::Format(format!("DWG: {e}")))?;
    // The template (MEASUREMENT: 0 imperial, 1 metric), free-space data, auxiliary header and an
    // empty preview.
    let measurement = header.iter().find(|r| r.name == "MEASUREMENT").and_then(|r| r.rec().i(70)).unwrap_or(0) as u16;
    dwg.sections = cadaddy_dwgcodec::Dwg::new_file_sections(v, dwg.objects.len(), seed, measurement);
    let bytes = cadaddy_dwgcodec::write(&dwg).map_err(|e| IoError::Format(format!("DWG: {e}")))?;
    let mut skipped: Vec<(String, usize)> = skipped.into_iter().collect();
    skipped.sort();
    Ok(((bytes, WriteReport { skipped, kept: 0 }), written))
}

fn handle_value(v: &Value) -> Option<u64> {
    match v {
        Value::Handle(h) => Some(*h),
        Value::Str(s) => u64::from_str_radix(s.trim(), 16).ok().filter(|h| *h != 0),
        _ => None,
    }
}

/// Each entity's previous and next entity (R2000).
type Links = HashMap<u64, (Option<u64>, Option<u64>)>;

/// What a block record holds: its BLOCK and ENDBLK, its entities, and its BLOCK's flags, base
/// point, xref path and description.
#[derive(Default)]
struct BlockParts {
    block: Option<u64>,
    endblk: Option<u64>,
    ents: Vec<u64>,
    flags: i64,
    base: [f64; 3],
    xref_path: String,
    description: String,
}

/// The encoded entities.
struct EntityPass {
    out: Vec<RawObject>,
    /// By block record.
    block_parts: HashMap<u64, BlockParts>,
    viewports_of: HashMap<u64, Vec<u64>>,
    /// Each owner's attributes or vertices.
    kid_lists: Vec<Vec<u64>>,
    skipped: HashMap<String, usize>,
}

/// Encodes the BLOCKS and ENTITIES sections' entities.
fn entity_pass(
    v: Version,
    blocks: &[Record],
    entities: &[Record],
    names: &Handles,
    (model_br, paper_br): (Option<u64>, Option<u64>),
    links: &Links,
) -> EntityPass {
    let mut p =
        EntityPass { out: Vec::new(), block_parts: HashMap::new(), viewports_of: HashMap::new(), kid_lists: Vec::new(), skipped: HashMap::new() };
    let new_parts = BlockParts::default;
    let mut owner_now: Option<u64> = None;
    let mut i = 0;
    while let Some(r) = blocks.get(i) {
        let Some(h) = r.handle() else {
            i += 1;
            continue;
        };
        let owner = r.owner();
        match r.name.as_str() {
            "BLOCK" | "ENDBLK" => {
                if r.name == "BLOCK" {
                    owner_now = owner;
                }
                if let Some(e) = owner_now.map(|o| p.block_parts.entry(o).or_insert_with(new_parts)) {
                    if r.name == "BLOCK" {
                        e.block = Some(h);
                        e.flags = r.rec().id(70, 0);
                        e.base = r.rec().p(10);
                        e.xref_path = r.rec().s(1).unwrap_or("").to_string();
                        e.description = r.rec().s(4).unwrap_or("").to_string();
                    } else {
                        e.endblk = Some(h);
                    }
                }
                let head = EntityHead {
                    entmode: 0,
                    owner: owner_now,
                    reactors: Vec::new(),
                    xdict: None,
                    layer: names.get("LAYER", r.rec().s(8).unwrap_or("0")),
                    ltype: None,
                    ltype_flags: 0,
                    material: None,
                    plotstyle: None,
                    links: None,
                };
                if let Some(o) = ek::entity(v, &r.name, h, &r.rec(), &head, names, &[], None) {
                    p.out.push(o);
                }
                i += 1;
            }
            _ => {
                let owner = owner.or(owner_now);
                let (next, top) = entity(v, blocks, i, 0, owner, names, links, &mut p);
                if let (Some(t), Some(o)) = (top, owner) {
                    p.block_parts.entry(o).or_insert_with(new_parts).ents.push(t);
                    if r.name == "VIEWPORT" {
                        p.viewports_of.entry(o).or_default().push(t);
                    }
                }
                i = next;
            }
        }
    }
    let mut i = 0;
    while let Some(r) = entities.get(i) {
        let paper = r.rec().id(67, 0) != 0;
        let space = if paper { paper_br } else { model_br };
        let (next, top) = entity(v, entities, i, if paper { 1 } else { 2 }, None, names, links, &mut p);
        if let (Some(t), Some(s)) = (top, space) {
            p.block_parts.entry(s).or_insert_with(new_parts).ents.push(t);
            if r.name == "VIEWPORT" {
                p.viewports_of.entry(s).or_default().push(t);
            }
        }
        i = next;
    }
    p
}

/// Encodes the entity record at `recs[i]` into `out`, with its attributes or vertices and their
/// SEQEND when it has them; the index after them, and its handle when it was encoded.
#[allow(clippy::too_many_arguments)]
fn entity(
    v: Version,
    recs: &[Record],
    i: usize,
    entmode: u8,
    owner: Option<u64>,
    names: &Handles,
    links: &Links,
    p: &mut EntityPass,
) -> (usize, Option<u64>) {
    let Some(r) = recs.get(i) else { return (i + 1, None) };
    let Some(h) = r.handle() else { return (i + 1, None) };
    let mut next = i + 1;
    let mut kids = Vec::new();
    let mut seqend = None;
    let parent = r.name == "POLYLINE" || (matches!(r.name.as_str(), "INSERT" | "MINSERT") && r.rec().id(66, 0) != 0);
    if parent {
        while let Some(k) = recs.get(next) {
            match k.name.as_str() {
                "VERTEX" | "ATTRIB" => {
                    if let Some(kh) = k.handle()
                        && encode(v, k, kh, 0, Some(h), names, &[], None, links, p)
                    {
                        kids.push(kh);
                    }
                    next += 1;
                }
                "SEQEND" => {
                    if let Some(kh) = k.handle()
                        && encode(v, k, kh, 0, Some(h), names, &[], None, links, p)
                    {
                        seqend = Some(kh);
                    }
                    next += 1;
                    break;
                }
                _ => break,
            }
        }
    }
    let ok = encode(v, r, h, entmode, owner, names, &kids, seqend, links, p);
    if !kids.is_empty() {
        p.kid_lists.push(kids);
    }
    (next, ok.then_some(h))
}

#[allow(clippy::too_many_arguments)]
fn encode(
    v: Version,
    r: &Record,
    h: u64,
    entmode: u8,
    owner: Option<u64>,
    names: &Handles,
    kids: &[u64],
    seqend: Option<u64>,
    links: &Links,
    p: &mut EntityPass,
) -> bool {
    let rec = r.rec();
    let lt = rec.s(6).unwrap_or("ByLayer");
    let (ltype_flags, ltype) = match lt.to_ascii_uppercase().as_str() {
        "BYLAYER" => (0, None),
        "BYBLOCK" => (1, None),
        "CONTINUOUS" => (2, None),
        _ => (3, names.get("LTYPE", lt)),
    };
    let head = EntityHead {
        entmode,
        owner,
        reactors: r.reactors.clone(),
        xdict: r.xdict,
        layer: names.get("LAYER", rec.s(8).unwrap_or("0")).or_else(|| names.get("LAYER", "0")),
        ltype,
        ltype_flags,
        material: None,
        plotstyle: None,
        links: links.get(&h).copied(),
    };
    match ek::entity(v, &r.name, h, &rec, &head, names, kids, seqend) {
        Some(o) => {
            p.out.push(o);
            true
        }
        None => {
            *p.skipped.entry(r.name.clone()).or_default() += 1;
            false
        }
    }
}

/// A new drawing's values for the variables a DXF header may leave out (the DXF Reference's
/// defaults); the drawing's own header values are set over them.
fn new_drawing_defaults(hv: &mut HeaderVars, metric: bool) {
    let v = hv.version;
    let b = HV::Bool;
    let i = HV::Int;
    let f = HV::Double;
    let defaults: Vec<(&str, HV)> = vec![
        ("DIMASO", b(true)),
        ("DIMSHO", b(true)),
        ("REGENMODE", b(true)),
        ("USRTIMER", b(true)),
        ("WORLDVIEW", b(true)),
        ("TILEMODE", b(true)),
        ("VISRETAIN", b(true)),
        ("FILLMODE", b(true)),
        ("QTEXTMODE", b(false)),
        ("PROXYGRAPHICS", i(1)),
        ("TREEDEPTH", i(3020)),
        ("SPLINESEGS", i(8)),
        ("SURFU", i(6)),
        ("SURFV", i(6)),
        ("SURFTYPE", i(6)),
        ("SURFTAB1", i(6)),
        ("SURFTAB2", i(6)),
        ("SPLINETYPE", i(6)),
        ("SHADEDGE", i(3)),
        ("SHADEDIF", i(70)),
        ("MAXACTVP", i(64)),
        ("ISOLINES", i(4)),
        ("TEXTQLTY", i(50)),
        ("TRACEWID", f(if metric { 1.0 } else { 0.05 })),
        ("SKETCHINC", f(if metric { 1.0 } else { 0.1 })),
        ("FACETRES", f(0.5)),
        ("CMLSCALE", f(if metric { 20.0 } else { 1.0 })),
        ("LTSCALE", f(1.0)),
        ("CELTSCALE", f(1.0)),
        ("PSLTSCALE", b(true)),
        ("PEXTMIN", HV::Point3([1e20; 3])),
        ("PEXTMAX", HV::Point3([-1e20; 3])),
        ("EXTMIN", HV::Point3([1e20; 3])),
        ("EXTMAX", HV::Point3([-1e20; 3])),
        ("UCSXDIR", HV::Point3([1.0, 0.0, 0.0])),
        ("UCSYDIR", HV::Point3([0.0, 1.0, 0.0])),
        ("PUCSXDIR", HV::Point3([1.0, 0.0, 0.0])),
        ("PUCSYDIR", HV::Point3([0.0, 1.0, 0.0])),
        ("TSTACKALIGN", i(1)),
        ("TSTACKSIZE", i(70)),
        ("SORTENTS", i(127)),
        ("HIDETEXT", i(1)),
        ("XCLIPFRAME", i(2)),
        ("DIMASSOC", i(2)),
        ("OBSCUREDCOLOR", i(257)),
        ("INTERSECTIONCOLOR", i(257)),
        ("3DDWFPREC", f(2.0)),
        ("DWFFRAME", i(2)),
        ("LIGHTGLYPHDISPLAY", i(1)),
        ("TILEMODELIGHTSYNCH", i(1)),
        ("REALWORLDSCALE", b(true)),
        ("SHOWHIST", i(1)),
        ("STEPSPERSEC", f(2.0)),
        ("STEPSIZE", f(if metric { 50.0 } else { 6.0 })),
        ("LENSLENGTH", f(50.0)),
        ("PSOLWIDTH", f(if metric { 5.0 } else { 0.25 })),
        ("PSOLHEIGHT", f(if metric { 80.0 } else { 4.0 })),
        ("LOFTANG1", f(std::f64::consts::FRAC_PI_2)),
        ("LOFTANG2", f(std::f64::consts::FRAC_PI_2)),
        ("LOFTPARAM", i(7)),
        ("LOFTNORMALS", i(1)),
        ("LATITUDE", f(37.795)),
        ("LONGITUDE", f(-122.394)),
        ("TIMEZONE", i(-8000)),
        ("INTERFERECOLOR", color(1, v)),
        ("DIMCLRD", color(0, v)),
        ("DIMCLRE", color(0, v)),
        ("DIMCLRT", color(0, v)),
        ("DIMTFILLCLR", color(0, v)),
        ("CECOLOR", color(256, v)),
    ];
    for (name, value) in defaults {
        // Only where the type matches what this version stores.
        if hv.get(name).is_some_and(|cur| std::mem::discriminant(cur) == std::mem::discriminant(&value)) {
            hv.set(name, value);
        }
    }
}

/// A DXF colour number as a header colour (256 ByLayer, 0 ByBlock, else an index). R2000 keeps
/// only the index (256 ByLayer); from R2004 the method byte says it (with index 0).
fn color(n: i64, v: Version) -> HV {
    let (index, rgb) = match n {
        256 if v < Version::R2004 => (256, 0),
        256 => (0, 0xC000_0000),
        0 => (0, 0xC100_0000),
        i => (i.clamp(1, 255) as i16, 0xC300_0000 | (i.clamp(1, 255) as u32)),
    };
    HV::Color(cadaddy_dwgcodec::header::Color { index, rgb, flags: 0, name: Text::Empty, book: Text::Empty })
}

/// The table a handle-valued header variable names an entry of, with the DXF group holding
/// the name.
fn named_table(var: &str) -> Option<(&'static str, i32)> {
    Some(match var {
        "CLAYER" => ("LAYER", 8),
        "TEXTSTYLE" | "DIMTXSTY" => ("STYLE", 7),
        "CELTYPE" => ("LTYPE", 6),
        "DIMLTYPE" | "DIMLTEX1" | "DIMLTEX2" => ("LTYPE", 6),
        "DIMSTYLE" => ("DIMSTYLE", 2),
        "UCSNAME" | "PUCSNAME" | "UCSBASE" | "PUCSBASE" | "UCSORTHOREF" | "PUCSORTHOREF" => ("UCS", 2),
        "DIMBLK" | "DIMBLK1" | "DIMBLK2" | "DIMLDRBLK" => ("BLOCK_RECORD", 1),
        _ => return None,
    })
}

/// FLAGS (spec §9): CELWEIGHT, ENDCAPS, JOINSTYLE, LWDISPLAY, XEDIT, EXTNAMES, PSTYLEMODE and
/// OLESTARTUP packed, from their DXF header values.
fn flags(header: &[Record]) -> i64 {
    let var = |n: &str| {
        header.iter().find(|r| r.name == n).and_then(|r| {
            r.rec().0.first().and_then(|g| match g.value {
                Value::Int(i) => Some(i),
                Value::Real(f) if f.is_finite() => Some(f as i64),
                _ => None,
            })
        })
    };
    let mut f = i64::from(cadaddy_dwgcodec::entities::encode::lineweight_index(var("CELWEIGHT").unwrap_or(-1)));
    f |= (var("ENDCAPS").unwrap_or(0) & 3) << 5;
    f |= (var("JOINSTYLE").unwrap_or(0) & 3) << 7;
    if var("LWDISPLAY").unwrap_or(0) == 0 {
        f |= 0x200;
    }
    if var("XEDIT").unwrap_or(1) == 0 {
        f |= 0x400;
    }
    if var("EXTNAMES").unwrap_or(1) != 0 {
        f |= 0x800;
    }
    if var("PSTYLEMODE").unwrap_or(1) != 0 {
        f |= 0x2000;
    }
    if var("OLESTARTUP").unwrap_or(0) != 0 {
        f |= 0x4000;
    }
    f
}

/// Sets a header variable from its DXF groups, when the DWG stores it with a matching type.
fn set_header(hv: &mut HeaderVars, r: &Record, names: &Handles, dicts: &HashMap<String, u64>) {
    let rec = r.rec();
    let name = r.name.as_str();
    let current = hv.get(name).cloned();
    let value = match (name, current) {
        ("CMLSTYLE", _) => {
            Some(HV::Handle(HandleRef { code: 5, value: rec.s(2).and_then(|n| dicts.get(&n.to_ascii_uppercase()).copied()).unwrap_or(0) }))
        }
        (_, Some(HV::Handle(_))) => named_table(name)
            .map(|(table, code)| HV::Handle(HandleRef { code: 5, value: rec.s(code).and_then(|n| names.get(table, n)).unwrap_or(0) })),
        (_, Some(HV::Color(_))) => rec.i(62).map(|n| color(n, hv.version)),
        (_, Some(HV::Bool(_))) => rec.0.first().and_then(|g| match g.value {
            Value::Int(i) => Some(HV::Bool(i != 0)),
            _ => None,
        }),
        (_, Some(HV::Int(_))) => rec.0.first().and_then(|g| match g.value {
            Value::Int(i) => Some(HV::Int(i)),
            Value::Real(f) if f.is_finite() => Some(HV::Int(f as i64)),
            _ => None,
        }),
        (_, Some(HV::Double(_))) => rec.0.first().and_then(|g| match g.value {
            Value::Real(f) => Some(HV::Double(f)),
            Value::Int(i) => Some(HV::Double(i as f64)),
            _ => None,
        }),
        (_, Some(HV::Point2(_))) => Some(HV::Point2(rec.p2(10))),
        (_, Some(HV::Point3(_))) => Some(HV::Point3(rec.p(10))),
        (_, Some(HV::Text(_))) => rec.0.first().and_then(|g| match &g.value {
            Value::Str(s) if !s.is_empty() => Some(HV::Text(text(hv.version, s))),
            _ => None,
        }),
        _ => None,
    };
    if let Some(v) = value {
        hv.set(name, v);
    }
}

/// A string as `v` stores it: UTF-16 from 2007, else ASCII with `\U+XXXX` escapes.
pub(crate) fn text(v: Version, s: &str) -> Text {
    if v.unicode() { Text::Wide(s.encode_utf16().collect()) } else { Text::Bytes(cadaddy_dwgcodec::bits::code_page_bytes(s)) }
}
