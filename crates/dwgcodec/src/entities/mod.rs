//! Entities (spec §20.4): each entity object decoded into the groups the DXF Reference gives that
//! entity — group code and value, in DXF order — so a reader of DXF groups builds the same drawing
//! from a DWG as from a DXF file, without DXF text in between.
//!
//! An object's data (spec §20.1) is three streams: the data stream (after the object type, the R13–
//! R2007 size in bits and the object's own handle), from R2007 a string stream at the end of the data
//! ([`crate::bits::string_stream`]), and the handle stream after them. [`Obj`] opens the three;
//! [`common`] reads what every entity starts with (spec §20.4.1: extended data, graphics,
//! entity mode, reactors, colour, linetype, plot style, material, visibility, lineweight, and
//! their handles), and [`decode`] the rest by object type.
//!
//! Names (layer, linetype, text style, block, dimension style…) come from the table records the
//! entities point at, read by [`Names::read`].

pub mod encode;
pub mod encode_kinds;
mod kinds;
mod names;
pub mod records;

pub use names::Names;

use crate::bits::{BitReader, HandleRef, Text};
use crate::classes::Classes;
use crate::error::corrupt;
use crate::objects::RawObject;
use crate::{Dwg, Error, Result, Version};

/// Most extended-data blocks, reactors or similar repeats on one entity.
const MAX_REPEAT: usize = 1 << 16;
/// Most items in one list (vertices, knots, boundary edges…).
pub(crate) const MAX_ITEMS: usize = 1 << 22;

/// A group's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Real(f64),
    /// A handle, written as hexadecimal.
    Handle(u64),
}

/// One DXF group: its code and value.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub code: i32,
    pub value: Value,
}

/// An entity as DXF groups, and where it belongs.
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    /// The DXF entity name (LINE, INSERT, …).
    pub name: String,
    pub handle: u64,
    /// The block record it is in (its owner), when the entity names one (spec §20.4.1: entity
    /// mode 0). `None` with `paper` false: model space.
    pub owner: Option<u64>,
    /// Entity mode 1: the active paper space.
    pub paper: bool,
    /// Its groups, from the layer (8) on: the handle (5), owner (330) and subclass markers are
    /// included where DXF has them.
    pub groups: Vec<Group>,
    /// Objects it owns that follow it in DXF: an INSERT's ATTRIBs and SEQEND, a POLYLINE's
    /// VERTEXes and SEQEND.
    pub children: Vec<u64>,
    /// R2000: the next entity in its block (see [`Common::next`]).
    pub next: Option<u64>,
}

/// The three streams of one object.
pub struct Obj<'a> {
    pub v: Version,
    pub handle: u64,
    pub type_code: u16,
    pub d: BitReader<'a>,
    s: Option<BitReader<'a>>,
    h: BitReader<'a>,
}

impl<'a> Obj<'a> {
    /// Opens `raw`'s streams; the data stream is left after the object's own handle.
    pub fn open(raw: &'a RawObject, v: Version) -> Result<Obj<'a>> {
        let data = raw.data.as_slice();
        let total = data.len().checked_mul(8).ok_or(Error::TooLarge("an object"))?;
        let mut d = BitReader::new(data);
        let r2010 = v >= Version::R2010;
        let (type_code, handles_at) = if r2010 {
            let hs = usize::try_from(d.mc_unsigned()?).map_err(|_| corrupt("an object's handle stream size"))?;
            let t = d.ot(true)?;
            (t, total.checked_sub(hs).ok_or_else(|| corrupt("a handle stream larger than its object"))?)
        } else {
            let t = d.ot(false)?;
            (t, d.rl()? as usize)
        };
        if handles_at > total || handles_at < d.bit_pos() {
            return Err(corrupt("an object's handle stream outside its data"));
        }
        let (s, data_end) = if v >= Version::R2007 {
            match crate::bits::string_stream(data, handles_at)? {
                Some((a, b)) => {
                    let mut s = BitReader::new(data);
                    s.set_bit_pos(a)?;
                    s.set_bit_end(b)?;
                    (Some(s), a)
                }
                None => (None, handles_at.saturating_sub(1)),
            }
        } else {
            (None, handles_at)
        };
        d.set_bit_end(data_end)?;
        let mut h = BitReader::new(data);
        h.set_bit_pos(handles_at)?;
        let own = d.h()?;
        Ok(Obj { v, handle: own.value, type_code, d, s, h })
    }

    /// A string (TV): from the string stream from R2007, else from the data stream.
    pub fn text(&mut self) -> Result<String> {
        if self.v >= Version::R2007 {
            return Ok(match &mut self.s {
                Some(s) => s.tu()?.to_string_lossy(),
                None => String::new(),
            });
        }
        Ok(self.d.t()?.to_string_lossy())
    }

    /// The next handle reference, made absolute (`None` for a null reference).
    pub fn href(&mut self) -> Result<Option<u64>> {
        let r: HandleRef = self.h.h()?;
        Ok(r.absolute(self.handle).filter(|h| *h != 0))
    }

    pub fn at_least(&self, v: Version) -> bool {
        self.v >= v
    }
}

/// Groups being written.
pub(crate) struct G<'g>(pub(crate) &'g mut Vec<Group>);

impl G<'_> {
    pub(crate) fn s(&mut self, code: i32, v: impl Into<String>) {
        self.0.push(Group { code, value: Value::Str(v.into()) });
    }
    pub(crate) fn i(&mut self, code: i32, v: i64) {
        self.0.push(Group { code, value: Value::Int(v) });
    }
    pub(crate) fn f(&mut self, code: i32, v: f64) {
        self.0.push(Group { code, value: Value::Real(v) });
    }
    pub(crate) fn h(&mut self, code: i32, v: u64) {
        self.0.push(Group { code, value: Value::Handle(v) });
    }
    /// A point: `code`, `code + 10`, `code + 20`.
    pub(crate) fn p(&mut self, code: i32, p: [f64; 3]) {
        self.f(code, p[0]);
        self.f(code + 10, p[1]);
        self.f(code + 20, p[2]);
    }
    pub(crate) fn p2(&mut self, code: i32, p: [f64; 2]) {
        self.f(code, p[0]);
        self.f(code + 10, p[1]);
    }
    /// The extrusion (210), when not the default 0,0,1.
    pub(crate) fn extrusion(&mut self, e: [f64; 3]) {
        if e != [0.0, 0.0, 1.0] {
            self.p(210, e);
        }
    }
    pub(crate) fn thickness(&mut self, t: f64) {
        if t != 0.0 {
            self.f(39, t);
        }
    }
}

/// Extended data: the application's handle and the raw items (spec §20.4.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Eed {
    pub app: u64,
    pub data: Vec<u8>,
}

/// What every entity starts with (spec §20.4.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Common {
    pub eed: Vec<Eed>,
    /// 0: the owner is a handle; 1: paper space; 2: model space.
    pub entmode: u8,
    pub owner: Option<u64>,
    pub reactors: Vec<u64>,
    pub xdict: Option<u64>,
    /// AutoCAD colour index (256 ByLayer, 0 ByBlock).
    pub color: i16,
    /// R2004+: true colour (0xRRGGBB) when given.
    pub rgb: Option<u32>,
    /// R2004+: transparency as stored (0x02000000 | alpha, 0 ByLayer, 0x01000000 ByBlock).
    pub transparency: Option<u32>,
    /// R2004+: the colour book reference (DBCOLOR object).
    pub color_ref: Option<u64>,
    pub ltype_scale: f64,
    /// 0 ByLayer, 1 ByBlock, 2 Continuous, 3 the linetype handle.
    pub ltype_flags: u8,
    pub ltype: Option<u64>,
    pub plotstyle_flags: u8,
    pub plotstyle: Option<u64>,
    pub material_flags: u8,
    pub material: Option<u64>,
    pub shadow: u8,
    pub invisible: bool,
    /// The lineweight index as stored (spec §20.4.1 lineweight table).
    pub lineweight: u8,
    pub layer: Option<u64>,
    /// R2000: the next entity of the same block (the chain a block record's first and last
    /// entities bound).
    pub next: Option<u64>,
}

/// The DXF lineweight (hundredths of a millimetre) of a stored lineweight index.
pub fn lineweight(index: u8) -> i64 {
    const LW: [i64; 24] = [0, 5, 9, 13, 15, 18, 20, 25, 30, 35, 40, 50, 53, 60, 70, 80, 90, 100, 106, 120, 140, 158, 200, 211];
    match index {
        28 | 29 => -1,
        30 => -2,
        31 => -3,
        i => LW.get(usize::from(i)).copied().unwrap_or(-1),
    }
}

/// The extended data, graphics and common entity data, data part (the handles are read by
/// [`common_handles`] once the entity's own data is read).
pub fn common(o: &mut Obj) -> Result<Common> {
    let mut c = Common { ltype_scale: 1.0, ..Common::default() };
    c.eed = eed(&mut o.d)?;
    // Proxy graphics: skipped.
    if o.d.b()? {
        let n = if o.v >= Version::R2010 { o.d.bll()? } else { u64::from(o.d.rl()?) };
        let n = usize::try_from(n).map_err(|_| Error::TooLarge("an entity's graphics"))?;
        if n > o.d.remaining_bits() / 8 {
            return Err(Error::Truncated("an entity's graphics"));
        }
        let p = o.d.bit_pos();
        o.d.set_bit_pos(p + n * 8)?;
    }
    c.entmode = o.d.bb()?;
    let reactors = count(o.d.bl()?, "reactors")?;
    let xdict_missing = if o.v >= Version::R2004 { o.d.b()? } else { false };
    if o.v >= Version::R2013 {
        o.d.b()?;
    }
    let nolinks = if o.v < Version::R2004 { o.d.b()? } else { true };
    let mut color_ref = false;
    if o.v >= Version::R2004 {
        let raw = o.d.bs()? as u16;
        c.color = (raw & 0x1ff) as i16;
        if raw & 0x8000 != 0 {
            c.rgb = Some(o.d.bl()? as u32);
        }
        if raw & 0x2000 != 0 {
            c.transparency = Some(o.d.bl()? as u32);
        }
        color_ref = raw & 0x4000 != 0;
    } else {
        c.color = o.d.bs()?;
    }
    c.ltype_scale = o.d.bd()?;
    c.ltype_flags = o.d.bb()?;
    c.plotstyle_flags = o.d.bb()?;
    if o.v >= Version::R2007 {
        c.material_flags = o.d.bb()?;
        c.shadow = o.d.rc()?;
    }
    let (mut full, mut face, mut edge) = (false, false, false);
    if o.v >= Version::R2010 {
        full = o.d.b()?;
        face = o.d.b()?;
        edge = o.d.b()?;
    }
    c.invisible = o.d.bs()? & 1 != 0;
    c.lineweight = o.d.rc()?;
    // The handles.
    if c.entmode == 0 {
        c.owner = o.href()?;
    }
    for _ in 0..reactors {
        if let Some(r) = o.href()? {
            c.reactors.push(r);
        }
    }
    if !xdict_missing {
        c.xdict = o.href()?;
    }
    if !nolinks {
        o.href()?;
        c.next = o.href()?;
    } else if o.v < Version::R2004 {
        // No links: the next entity is the next handle (spec §20.4.1).
        c.next = o.handle.checked_add(1);
    }
    if color_ref {
        c.color_ref = o.href()?;
    }
    c.layer = o.href()?;
    if c.ltype_flags == 3 {
        c.ltype = o.href()?;
    }
    if o.v >= Version::R2007 && c.material_flags == 3 {
        c.material = o.href()?;
    }
    if c.plotstyle_flags == 3 {
        c.plotstyle = o.href()?;
    }
    for has in [full, face, edge] {
        if has {
            o.href()?;
        }
    }
    Ok(c)
}

/// A count read from the file, capped.
pub(crate) fn count(n: i32, what: &'static str) -> Result<usize> {
    let n = usize::try_from(n).map_err(|_| corrupt(format!("a negative count of {what}")))?;
    if n > MAX_ITEMS {
        return Err(Error::TooLarge(what));
    }
    Ok(n)
}

/// Extended data blocks (spec §20.4.1): size, application handle, that many bytes; a size of 0
/// ends them.
fn eed(d: &mut BitReader) -> Result<Vec<Eed>> {
    let mut out = Vec::new();
    loop {
        let n = d.bs()? as u16 as usize;
        if n == 0 {
            return Ok(out);
        }
        if out.len() >= MAX_REPEAT {
            return Err(Error::TooLarge("extended data"));
        }
        let app = d.h()?.value;
        out.push(Eed { app, data: d.bytes(n)? });
    }
}

/// What an object of a non-entity type starts with (spec §20.4.2): extended data, reactors and
/// the extension dictionary; the handles are read here as they come first in the handle stream
/// (owner, reactors, dictionary).
pub struct ObjectCommon {
    pub owner: Option<u64>,
    pub reactors: Vec<u64>,
    pub xdict: Option<u64>,
    pub eed: Vec<Eed>,
}

pub fn object_common(o: &mut Obj) -> Result<ObjectCommon> {
    let eed = eed(&mut o.d)?;
    let n = count(o.d.bl()?, "reactors")?;
    let xdict_missing = if o.v >= Version::R2004 { o.d.b()? } else { false };
    if o.v >= Version::R2013 {
        o.d.b()?;
    }
    let owner = o.href()?;
    let mut reactors = Vec::new();
    for _ in 0..n {
        if let Some(r) = o.href()? {
            reactors.push(r);
        }
    }
    let xdict = if xdict_missing { None } else { o.href()? };
    Ok(ObjectCommon { owner, reactors, xdict, eed })
}

/// Extended data as DXF groups (1001 application, then 1000–1071 items; spec §20.4.1 and the DXF
/// Reference's extended data codes). Undecodable data ends that application's groups.
pub fn eed_groups(eed: &[Eed], names: &Names, unicode: bool) -> Vec<Group> {
    let mut out = Vec::new();
    for e in eed {
        let Some(app) = names.appid(e.app) else { continue };
        out.push(Group { code: 1001, value: Value::Str(app.to_string()) });
        let mut r = BitReader::new(&e.data);
        while r.remaining_bits() >= 8 {
            let Ok(code) = r.rc() else { break };
            let item = (|| -> Result<Group> {
                Ok(match code {
                    0 => {
                        let s = if unicode {
                            let n = usize::from(r.rs()?);
                            let w: Vec<u16> = (0..n).map(|_| r.rs()).collect::<Result<_>>()?;
                            String::from_utf16_lossy(&w)
                        } else {
                            let n = usize::from(r.rc()?);
                            r.rs()?;
                            Text::Bytes(r.bytes(n)?).to_string_lossy()
                        };
                        Group { code: 1000, value: Value::Str(s) }
                    }
                    2 => Group { code: 1002, value: Value::Str(if r.rc()? == 0 { "{".into() } else { "}".into() }) },
                    // Handles: 8 bytes, high byte first (observed: the spec doesn't say).
                    3 => {
                        let b = r.bytes(8)?;
                        Group { code: 1003, value: Value::Handle(u64::from_be_bytes(b.try_into().unwrap_or_default())) }
                    }
                    4 => {
                        let n = usize::from(r.rc()?);
                        let b = r.bytes(n)?;
                        Group { code: 1004, value: Value::Str(b.iter().map(|x| format!("{x:02X}")).collect()) }
                    }
                    5 => {
                        let b = r.bytes(8)?;
                        let mut a = [0u8; 8];
                        a.copy_from_slice(&b);
                        Group { code: 1005, value: Value::Handle(u64::from_be_bytes(a)) }
                    }
                    10..=13 => {
                        let p = [r.rd()?, r.rd()?, r.rd()?];
                        out.push(Group { code: 1000 + i32::from(code), value: Value::Real(p[0]) });
                        out.push(Group { code: 1010 + i32::from(code), value: Value::Real(p[1]) });
                        Group { code: 1020 + i32::from(code), value: Value::Real(p[2]) }
                    }
                    40..=42 => Group { code: 1000 + i32::from(code), value: Value::Real(r.rd()?) },
                    70 => Group { code: 1070, value: Value::Int(i64::from(r.rs()? as i16)) },
                    71 => Group { code: 1071, value: Value::Int(i64::from(r.rl()? as i32)) },
                    _ => return Err(corrupt("an extended data item")),
                })
            })();
            match item {
                Ok(g) => out.push(g),
                Err(_) => break,
            }
        }
    }
    out
}

/// The groups every entity has after its subclass marker AcDbEntity (DXF Reference, common
/// entity groups): handle, owner, paper space, layer, linetype, colour, lineweight, linetype
/// scale, visibility, true colour, transparency, material and plot style.
pub fn common_groups(e: &mut Vec<Group>, handle: u64, c: &Common, names: &Names, owner: Option<u64>) {
    let s = |code: i32, v: &str| Group { code, value: Value::Str(v.to_string()) };
    let i = |code: i32, v: i64| Group { code, value: Value::Int(v) };
    e.push(Group { code: 5, value: Value::Handle(handle) });
    if let Some(o) = owner {
        e.push(Group { code: 330, value: Value::Handle(o) });
    }
    e.push(s(100, "AcDbEntity"));
    if c.entmode == 1 {
        e.push(i(67, 1));
    }
    e.push(s(8, c.layer.and_then(|h| names.name(h)).unwrap_or("0")));
    match c.ltype_flags {
        1 => e.push(s(6, "ByBlock")),
        2 => e.push(s(6, "Continuous")),
        3 => {
            if let Some(n) = c.ltype.and_then(|h| names.name(h)) {
                e.push(s(6, n));
            }
        }
        _ => {}
    }
    if let Some(m) = c.material {
        e.push(Group { code: 347, value: Value::Handle(m) });
    }
    if c.color != 256 {
        e.push(i(62, i64::from(c.color)));
    }
    let lw = lineweight(c.lineweight);
    if lw != -1 {
        e.push(i(370, lw));
    }
    if (c.ltype_scale - 1.0).abs() > 1e-12 {
        e.push(Group { code: 48, value: Value::Real(c.ltype_scale) });
    }
    if c.invisible {
        e.push(i(60, 1));
    }
    if let Some(rgb) = c.rgb
        && rgb >> 24 == 0xC2
    {
        e.push(i(420, i64::from(rgb & 0xFF_FFFF)));
    }
    if let Some(t) = c.transparency
        && t >> 24 == 0x02
    {
        e.push(i(440, i64::from(t)));
    }
    if let Some(p) = c.plotstyle {
        e.push(Group { code: 390, value: Value::Handle(p) });
    }
}

/// Decodes the entity in `raw`, or `None` when `raw` isn't an entity this layer knows (objects,
/// table records, and entity types not decoded yet are left to the caller, which keeps them raw).
pub fn decode(raw: &RawObject, v: Version, classes: &Classes, names: &Names) -> Result<Option<Entity>> {
    let name = match raw.type_code {
        t if t >= 500 => match classes.get(t) {
            Some(c) => c.dxf_name.to_string_lossy(),
            None => return Ok(None),
        },
        t => match kinds::fixed_name(t) {
            Some(n) => n.to_string(),
            None => return Ok(None),
        },
    };
    if !kinds::known(&name) {
        return Ok(None);
    }
    let mut o = Obj::open(raw, v)?;
    let c = common(&mut o)?;
    let owner = match c.entmode {
        0 => c.owner,
        _ => None,
    };
    let mut groups = Vec::new();
    common_groups(&mut groups, o.handle, &c, names, owner.or(c.owner));
    let mut children = Vec::new();
    kinds::decode(&name, &mut o, &c, names, &mut groups, &mut children)?;
    groups.extend(eed_groups(&c.eed, names, v >= Version::R2007));
    Ok(Some(Entity { name, handle: o.handle, owner, paper: c.entmode == 1, groups, children, next: c.next }))
}

/// Each block record's entities in their stored order (from R2004 its list; R2000 the chain from
/// its first entity to its last by each one's next entity). `ents` are the file's entities, as
/// [`decode_all`] gives them.
pub fn block_lists(dwg: &Dwg, ents: &[Entity]) -> Result<std::collections::HashMap<u64, Vec<u64>>> {
    let classes = dwg.classes()?;
    let names = Names::read(dwg, &classes);
    let next: std::collections::HashMap<u64, u64> = ents.iter().filter_map(|e| e.next.map(|n| (e.handle, n))).collect();
    let mut out = std::collections::HashMap::new();
    for (block, contents) in names.block_contents() {
        let list = match contents {
            names::Contents::Owned(list) => list.clone(),
            names::Contents::Chain(first, last) => {
                let mut list = Vec::new();
                let mut at = Some(*first);
                while let Some(h) = at {
                    list.push(h);
                    if h == *last || list.len() > MAX_ITEMS {
                        break;
                    }
                    at = next.get(&h).copied();
                }
                list
            }
        };
        out.insert(*block, list);
    }
    Ok(out)
}

/// R2000: an INSERT's attributes and a POLYLINE's vertices are stored as the first and last (0
/// for none), the ones between linked by each one's next entity; made the whole list here
/// (followed by the SEQEND).
fn chain_children(ents: &mut [Entity]) {
    let next: std::collections::HashMap<u64, u64> = ents.iter().filter_map(|e| e.next.map(|n| (e.handle, n))).collect();
    for e in ents.iter_mut().filter(|e| matches!(e.name.as_str(), "INSERT" | "MINSERT" | "POLYLINE")) {
        let (Some(&first), Some(&last)) = (e.children.first(), e.children.get(1)) else { continue };
        let rest: Vec<u64> = e.children.iter().skip(2).copied().collect();
        let mut list = Vec::new();
        let mut at = (first != 0).then_some(first);
        while let Some(h) = at {
            list.push(h);
            if h == last || list.len() > MAX_ITEMS {
                break;
            }
            at = next.get(&h).copied();
        }
        list.extend(rest);
        e.children = list;
    }
}

/// Every entity of a file decoded, with the objects that couldn't be (unknown types, or damaged
/// data) as errors by handle.
pub fn decode_all(dwg: &Dwg) -> Result<(Vec<Entity>, Vec<(u64, String)>)> {
    let classes = dwg.classes()?;
    let names = Names::read(dwg, &classes);
    let mut out = Vec::new();
    let mut failed = Vec::new();
    for raw in &dwg.objects {
        match decode(raw, dwg.version, &classes, &names) {
            Ok(Some(e)) => out.push(e),
            Ok(None) => {}
            Err(e) => {
                let kind = match raw.type_code {
                    t if t >= 500 => classes.get(t).map(|c| c.dxf_name.to_string_lossy()).unwrap_or_default(),
                    t => kinds::fixed_name(t).unwrap_or("?").to_string(),
                };
                failed.push((raw.handle, format!("{kind}: {e}")));
            }
        }
    }
    // Model space and the active paper space by the header's block record handles (names can
    // repeat: every layout's block may be called *Paper_Space).
    let space = |name: &str| match dwg.header_vars().ok().and_then(|h| h.get(name).cloned()) {
        Some(crate::header::Value::Handle(r)) => Some(r.value).filter(|v| *v != 0),
        _ => None,
    };
    place(&mut out, &names, space("BLOCK_RECORD_MODEL_SPACE"), space("BLOCK_RECORD_PAPER_SPACE"));
    if dwg.version < Version::R2004 {
        chain_children(&mut out);
    }
    Ok((out, failed))
}

/// Each entity's place from the block records' lists of their entities (the entity mode alone
/// marks every paper space entity as the active layout's): model space, the active paper space
/// (67 = 1) or a block — another layout's paper space block among them — as the owner (330).
fn place(ents: &mut [Entity], names: &Names, model: Option<u64>, paper_space: Option<u64>) {
    let mut owner_of: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    let next: std::collections::HashMap<u64, u64> = ents.iter().filter_map(|e| e.next.map(|n| (e.handle, n))).collect();
    for (block, contents) in names.block_contents() {
        match contents {
            names::Contents::Owned(list) => {
                for h in list {
                    owner_of.insert(*h, *block);
                }
            }
            names::Contents::Chain(first, last) => {
                let mut at = Some(*first);
                let mut steps = 0usize;
                while let Some(h) = at {
                    owner_of.insert(h, *block);
                    steps += 1;
                    if h == *last || steps > MAX_ITEMS {
                        break;
                    }
                    at = next.get(&h).copied();
                }
            }
        }
    }
    for e in ents.iter_mut() {
        let Some(block) = owner_of.get(&e.handle).copied() else { continue };
        let name = names.block(block).unwrap_or_default().to_ascii_uppercase();
        let (owner, paper) = if Some(block) == model || (model.is_none() && name == "*MODEL_SPACE") {
            (None, false)
        } else if Some(block) == paper_space || (paper_space.is_none() && name == "*PAPER_SPACE") {
            (None, true)
        } else {
            (Some(block), false)
        };
        e.owner = owner;
        e.paper = paper;
        e.groups.retain(|g| g.code != 67);
        if paper && let Some(i) = e.groups.iter().position(|g| g.code == 100) {
            e.groups.insert(i + 1, Group { code: 67, value: Value::Int(1) });
        }
        // The owner group follows the handle (other 330 groups, a multileader's attribute
        // definitions for one, are left alone).
        let at = e.groups.iter().position(|g| g.code == 5).map_or(0, |i| i + 1);
        match e.groups.get_mut(at) {
            Some(g) if g.code == 330 => g.value = Value::Handle(block),
            _ => e.groups.insert(at.min(e.groups.len()), Group { code: 330, value: Value::Handle(block) }),
        }
    }
}
