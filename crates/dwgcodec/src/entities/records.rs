//! Table records and objects (spec §20.4.47 onward) decoded into the groups the DXF Reference
//! gives them in the TABLES and OBJECTS sections, as [`super::decode`] does for entities.
//!
//! Every table record starts the same way (spec §20.4.47 common table data): the object's common
//! data, its name, the "referenced" bit (DXF flag 64), the external-reference index and the
//! "xref-dependent" bit (flag 16); its handles start with the owner, reactors, extension dictionary
//! and the external-reference block.

use super::{G, Group, Names, Obj, Value, count, object_common};
use crate::classes::Classes;
use crate::objects::RawObject;
use crate::{Error, Result, Version};

/// A table record or object as DXF groups.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    /// The DXF record name (LAYER, DICTIONARY, …).
    pub name: String,
    pub handle: u64,
    pub owner: Option<u64>,
    pub groups: Vec<Group>,
    /// A block record's block: what DXF puts in the BLOCK entity rather than the record.
    pub block: Option<BlockInfo>,
}

/// A block record's BLOCK and ENDBLK entities and the BLOCK's DXF data (spec §20.4.49).
#[derive(Clone, Debug, PartialEq)]
pub struct BlockInfo {
    pub block_entity: Option<u64>,
    pub endblk: Option<u64>,
    pub base: [f64; 3],
    /// DXF 70: 1 anonymous, 2 has attributes, 4 external reference, 8 overlay.
    pub flags: i64,
    pub xref_path: String,
    pub description: String,
}

/// Records decoded here.
const KNOWN: &[&str] = &[
    "BLOCK_RECORD",
    "LAYER",
    "STYLE",
    "LTYPE",
    "APPID",
    "DIMSTYLE",
    "VPORT",
    "VIEW",
    "UCS",
    "DICTIONARY",
    "ACDBDICTIONARYWDFLT",
    "ACDBPLACEHOLDER",
    "LAYOUT",
    "PLOTSETTINGS",
    "MLINESTYLE",
    "GROUP",
    "XRECORD",
    "SCALE",
    "DICTIONARYVAR",
    "IMAGEDEF",
    "IMAGEDEF_REACTOR",
    "MLEADERSTYLE",
];

/// The DXF name of a fixed object type that is a table record or object this module decodes.
fn fixed_name(t: u16) -> Option<&'static str> {
    Some(match t {
        49 => "BLOCK_RECORD",
        51 => "LAYER",
        53 => "STYLE",
        57 => "LTYPE",
        61 => "VIEW",
        63 => "UCS",
        65 => "VPORT",
        67 => "APPID",
        69 => "DIMSTYLE",
        42 => "DICTIONARY",
        72 => "GROUP",
        73 => "MLINESTYLE",
        79 => "XRECORD",
        80 => "ACDBPLACEHOLDER",
        82 => "LAYOUT",
        _ => return None,
    })
}

/// The table record or object in `raw`, or `None` when it isn't one decoded here.
pub fn decode(raw: &RawObject, v: Version, classes: &Classes, names: &Names) -> Result<Option<Record>> {
    let name = match raw.type_code {
        t if t >= 500 => match classes.get(t) {
            Some(c) => c.dxf_name.to_string_lossy(),
            None => return Ok(None),
        },
        t => match fixed_name(t) {
            Some(n) => n.to_string(),
            None => return Ok(None),
        },
    };
    if !KNOWN.contains(&name.as_str()) {
        return Ok(None);
    }
    let mut o = Obj::open(raw, v)?;
    let c = object_common(&mut o)?;
    let mut groups = Vec::new();
    let mut g = G(&mut groups);
    let handle = o.handle;
    g.h(if name == "DIMSTYLE" { 105 } else { 5 }, handle);
    if let Some(r) = c.xdict {
        g.s(102, "{ACAD_XDICTIONARY");
        g.h(360, r);
        g.s(102, "}");
    }
    if let Some(owner) = c.owner {
        g.h(330, owner);
    }
    let mut block = None;
    match name.as_str() {
        "LAYER" => layer(&mut o, names, &mut g)?,
        "LTYPE" => ltype(&mut o, &mut g)?,
        "STYLE" => style(&mut o, &mut g)?,
        "APPID" => {
            let (n, flags) = table_entry(&mut o)?;
            o.d.rc()?;
            g.s(100, "AcDbSymbolTableRecord");
            g.s(100, "AcDbRegAppTableRecord");
            g.s(2, n);
            g.i(70, flags);
        }
        "BLOCK_RECORD" => block = Some(block_record(&mut o, &mut g)?),
        "DIMSTYLE" => dimstyle(&mut o, &mut g)?,
        "VPORT" => vport(&mut o, &mut g)?,
        "VIEW" => view(&mut o, &mut g)?,
        "UCS" => ucs(&mut o, &mut g)?,
        "DICTIONARY" | "ACDBDICTIONARYWDFLT" => dictionary(&name, &mut o, &mut g)?,
        "ACDBPLACEHOLDER" => {}
        "LAYOUT" => {
            plot_settings(&mut o, &mut g)?;
            layout(&mut o, &mut g)?;
        }
        "PLOTSETTINGS" => plot_settings(&mut o, &mut g)?,
        "MLINESTYLE" => mlinestyle(&mut o, names, &mut g)?,
        "GROUP" => group(&mut o, &mut g)?,
        "XRECORD" => xrecord(&mut o, &mut g)?,
        "SCALE" => {
            let flags = o.d.bs()?;
            let n = o.text()?;
            let paper = o.d.bd()?;
            let drawing = o.d.bd()?;
            let unit = o.d.b()?;
            g.s(100, "AcDbScale");
            g.i(70, i64::from(flags));
            g.s(300, n);
            g.f(140, paper);
            g.f(141, drawing);
            g.i(290, i64::from(unit));
        }
        "DICTIONARYVAR" => {
            let schema = o.d.rc()?;
            let value = o.text()?;
            g.s(100, "DictionaryVariables");
            g.i(280, i64::from(schema));
            g.s(1, value);
        }
        "IMAGEDEF" => {
            let version = o.d.bl()?;
            let size = o.d.rd2()?;
            let path = o.text()?;
            let loaded = o.d.b()?;
            let units = o.d.rc()?;
            let pixel = o.d.rd2()?;
            g.s(100, "AcDbRasterImageDef");
            g.i(90, i64::from(version));
            g.s(1, path);
            g.p2(10, size);
            g.p2(11, pixel);
            g.i(280, i64::from(loaded));
            g.i(281, i64::from(units));
        }
        "MLEADERSTYLE" => mleaderstyle(&mut o, &mut g)?,
        "IMAGEDEF_REACTOR" => {
            let version = o.d.bl()?;
            g.s(100, "AcDbRasterImageDefReactor");
            g.i(90, i64::from(version));
        }
        _ => return Err(Error::Unsupported(format!("{name} records are not decoded yet"))),
    }
    groups.extend(super::eed_groups(&c.eed, names, v >= Version::R2007));
    Ok(Some(Record { name, handle, owner: c.owner, groups, block }))
}

/// The common table data after the object's: name, flags (64 referenced, 16 xref-dependent);
/// the external-reference block's handle is read.
fn table_entry(o: &mut Obj) -> Result<(String, i64)> {
    let name = o.text()?;
    let referenced = o.d.b()?;
    // The external-reference index up to R2004; from R2007 the record has only the two bits
    // (observed: R2010+ application records have exactly those two bits and their byte).
    if !o.at_least(Version::R2007) {
        o.d.bs()?;
    }
    let xdep = o.d.b()?;
    o.href()?;
    // "Referenced" (DXF 64) is for AutoCAD's own commands and is left out, as other readers do.
    let _ = referenced;
    let mut flags = 0;
    if xdep {
        flags |= 16;
    }
    Ok((name, flags))
}

/// LAYER (spec §20.4.51).
fn layer(o: &mut Obj, names: &Names, g: &mut G) -> Result<()> {
    let (name, mut flags) = table_entry(o)?;
    let values = o.d.bs()?;
    let mut index = o.d.bs()?;
    let (mut rgb, mut book) = (None, None);
    if o.at_least(Version::R2004) {
        // From R2004 the colour is in the raw value: method 0xC3 an index, 0xC2 a true colour.
        let raw = o.d.bl()? as u32;
        let cflags = o.d.rc()?;
        if cflags & 1 != 0 {
            book = Some(o.text()?);
        }
        if cflags & 2 != 0 {
            o.text()?;
        }
        match raw >> 24 {
            0xC2 => rgb = Some(raw & 0xFF_FFFF),
            0xC3 => index = (raw & 0xFF) as i16,
            _ => {}
        }
    }
    let plotstyle = o.href()?;
    let material = if o.at_least(Version::R2007) { o.href()? } else { None };
    let ltype = o.href()?;
    if values & 1 != 0 {
        flags |= 1;
    }
    if values & 4 != 0 {
        flags |= 2;
    }
    if values & 8 != 0 {
        flags |= 4;
    }
    let off = values & 2 != 0;
    let color = i64::from(index.unsigned_abs() as i16).max(0);
    g.s(100, "AcDbSymbolTableRecord");
    g.s(100, "AcDbLayerTableRecord");
    g.s(2, name);
    g.i(70, flags);
    g.i(62, if off { -color } else { color });
    if let Some(c) = rgb {
        g.i(420, i64::from(c));
    }
    if let Some(b) = book.filter(|b| !b.is_empty()) {
        g.s(430, b);
    }
    g.s(6, ltype.and_then(|h| names.name(h)).unwrap_or("Continuous"));
    if values & 16 == 0 {
        g.i(290, 0);
    }
    g.i(370, super::lineweight(((values >> 5) & 0x1f) as u8));
    if let Some(h) = plotstyle {
        g.h(390, h);
    }
    if let Some(h) = material {
        g.h(347, h);
    }
    Ok(())
}

/// LTYPE (spec §20.4.55).
fn ltype(o: &mut Obj, g: &mut G) -> Result<()> {
    let (name, flags) = table_entry(o)?;
    let description = o.text()?;
    let length = o.d.bd()?;
    let align = o.d.rc()?;
    let n = usize::from(o.d.rc()?);
    struct Dash {
        length: f64,
        shape: i16,
        offset: [f64; 2],
        scale: f64,
        rotation: f64,
        flags: i16,
    }
    let mut dashes = Vec::with_capacity(n);
    for _ in 0..n {
        let length = o.d.bd()?;
        let shape = o.d.bs()?;
        let offset = [o.d.rd()?, o.d.rd()?];
        let scale = o.d.bd()?;
        let rotation = o.d.bd()?;
        let flags = o.d.bs()?;
        dashes.push(Dash { length, shape, offset, scale, rotation, flags });
    }
    // The strings area: 256 bytes up to R2004; from R2007 512 when a dash has text.
    let has_text = dashes.iter().any(|d| d.flags & 2 != 0);
    let area = if o.at_least(Version::R2007) { if has_text { o.d.bytes(512)? } else { Vec::new() } } else { o.d.bytes(256)? };
    let mut styles = Vec::with_capacity(n);
    for _ in 0..n {
        styles.push(o.href()?);
    }
    g.s(100, "AcDbSymbolTableRecord");
    g.s(100, "AcDbLinetypeTableRecord");
    g.s(2, name);
    g.i(70, flags);
    g.s(3, description);
    g.i(72, i64::from(align));
    g.i(73, n as i64);
    g.f(40, length);
    for (d, style) in dashes.iter().zip(styles) {
        g.f(49, d.length);
        g.i(74, i64::from(d.flags));
        if d.flags & 6 != 0 {
            g.i(75, i64::from(d.shape));
            if let Some(h) = style {
                g.h(340, h);
            }
            g.f(46, d.scale);
            g.f(50, d.rotation.to_degrees());
            g.f(44, d.offset[0]);
            g.f(45, d.offset[1]);
        }
        if d.flags & 2 != 0 {
            g.s(9, dash_text(&area, d.shape, o.at_least(Version::R2007)));
        }
    }
    Ok(())
}

/// A dash's text from the linetype's strings area (spec §20.4.55: the dash's shape number is the
/// text's byte offset in the area).
fn dash_text(area: &[u8], at: i16, wide: bool) -> String {
    let start = usize::try_from(at).unwrap_or(0);
    let Some(rest) = area.get(start..) else { return String::new() };
    if wide {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|u| *u != 0).collect();
        String::from_utf16_lossy(&units)
    } else {
        rest.iter().take_while(|b| **b != 0).map(|&b| char::from(b)).collect()
    }
}

/// STYLE (spec §20.4.53).
fn style(o: &mut Obj, g: &mut G) -> Result<()> {
    let (name, mut flags) = table_entry(o)?;
    let vertical = o.d.b()?;
    let shape_file = o.d.b()?;
    let fixed = o.d.bd()?;
    let width = o.d.bd()?;
    let oblique = o.d.bd()?;
    let generation = o.d.rc()?;
    let last = o.d.bd()?;
    let font = o.text()?;
    let bigfont = o.text()?;
    if shape_file {
        flags |= 1;
    }
    if vertical {
        flags |= 4;
    }
    g.s(100, "AcDbSymbolTableRecord");
    g.s(100, "AcDbTextStyleTableRecord");
    g.s(2, name);
    g.i(70, flags);
    g.f(40, fixed);
    g.f(41, width);
    g.f(50, oblique.to_degrees());
    g.i(71, i64::from(generation));
    g.f(42, last);
    g.s(3, font);
    g.s(4, bigfont);
    Ok(())
}

/// BLOCK_HEADER (spec §20.4.49) as the BLOCK_RECORD table record.
fn block_record(o: &mut Obj, g: &mut G) -> Result<BlockInfo> {
    let (name, flags) = table_entry(o)?;
    let anonymous = o.d.b()?;
    let has_attribs = o.d.b()?;
    let is_xref = o.d.b()?;
    let overlaid = o.d.b()?;
    let _loaded = o.d.b()?;
    let owned = if o.at_least(Version::R2004) && !is_xref && !overlaid { count(o.d.bl()?, "block entities")? } else { 0 };
    let base = o.d.bd3()?;
    let xref_path = o.text()?;
    // Insert counts: bytes until a zero.
    let mut inserts = 0usize;
    loop {
        if o.d.rc()? == 0 {
            break;
        }
        inserts += 1;
        if inserts > super::MAX_ITEMS {
            return Err(Error::TooLarge("a block's insert count"));
        }
    }
    let description = o.text()?;
    let preview = count(o.d.bl()?, "preview bytes")?;
    let preview = o.d.bytes(preview)?;
    let (units, explodable, scaling) =
        if o.at_least(Version::R2007) { (Some(o.d.bs()?), Some(o.d.b()?), Some(o.d.rc()?)) } else { (None, None, None) };
    let _ = flags;
    // Handles: the BLOCK entity, the owned entities (R2004+) or first and last (R2000), ENDBLK,
    // the inserts, the layout.
    let block_entity = o.href()?;
    if o.at_least(Version::R2004) {
        for _ in 0..owned {
            o.href()?;
        }
    } else if !is_xref && !overlaid {
        o.href()?;
        o.href()?;
    }
    let endblk = o.href()?;
    for _ in 0..inserts {
        o.href()?;
    }
    let layout = o.href()?;
    g.s(100, "AcDbSymbolTableRecord");
    g.s(100, "AcDbBlockTableRecord");
    g.s(2, name);
    g.h(340, layout.unwrap_or(0));
    if let Some(u) = units {
        g.i(70, i64::from(u));
    }
    if let Some(e) = explodable {
        g.i(280, i64::from(e));
    }
    if let Some(s) = scaling {
        g.i(281, i64::from(s));
    }
    if !preview.is_empty() {
        for chunk in preview.chunks(127) {
            g.0.push(Group { code: 310, value: Value::Str(chunk.iter().map(|b| format!("{b:02X}")).collect()) });
        }
    }
    let mut bflags = 0;
    for (on, bit) in [(anonymous, 1), (has_attribs, 2), (is_xref, 4), (overlaid, 8)] {
        if on {
            bflags |= bit;
        }
    }
    Ok(BlockInfo { block_entity, endblk, base, flags: bflags, xref_path, description })
}

/// Every table record and object of a file decoded, and those that couldn't be, with why.
pub fn decode_all(dwg: &crate::Dwg) -> Result<(Vec<Record>, Vec<(u64, String)>)> {
    let classes = dwg.classes()?;
    let names = Names::read(dwg, &classes);
    let mut out = Vec::new();
    let mut failed = Vec::new();
    for raw in &dwg.objects {
        match decode(raw, dwg.version, &classes, &names) {
            Ok(Some(r)) => out.push(r),
            Ok(None) => {}
            Err(e) => failed.push((raw.handle, e.to_string())),
        }
    }
    Ok((out, failed))
}

/// A colour (CMC) as a DXF colour number: from R2004 the raw value's method byte decides
/// (0xC0 ByLayer, 0xC1 ByBlock, 0xC3 an index; a true colour keeps the stored index).
fn color_number(o: &mut Obj) -> Result<i64> {
    let index = o.d.bs()?;
    if !o.at_least(Version::R2004) {
        return Ok(i64::from(index));
    }
    let raw = o.d.bl()? as u32;
    let flags = o.d.rc()?;
    if flags & 1 != 0 {
        o.text()?;
    }
    if flags & 2 != 0 {
        o.text()?;
    }
    Ok(match raw >> 24 {
        0xC0 => 256,
        0xC1 => 0,
        0xC3 => i64::from(raw & 0xFF),
        _ => i64::from(index),
    })
}

/// DIMSTYLE (spec §20.4.65): the dimension variables in the order stored, written as their DXF
/// groups.
fn dimstyle(o: &mut Obj, g: &mut G) -> Result<()> {
    let (name, flags) = table_entry(o)?;
    let r2007 = o.at_least(Version::R2007);
    let r2010 = o.at_least(Version::R2010);
    g.s(100, "AcDbSymbolTableRecord");
    g.s(100, "AcDbDimStyleTableRecord");
    g.s(2, name);
    g.i(70, flags);
    // A bare "<>" (the measurement alone) is the same as no prefix or suffix.
    let post = o.text()?;
    let post = if post.trim() == "<>" { String::new() } else { post };
    let apost = o.text()?;
    g.s(3, post);
    g.s(4, apost);
    for code in [40, 41, 42, 43, 44, 45, 46, 47, 48] {
        g.f(code, o.d.bd()?);
    }
    if r2007 {
        g.f(49, o.d.bd()?);
        g.f(50, o.d.bd()?.to_degrees());
        g.i(69, i64::from(o.d.bs()?));
        g.i(70, color_number(o)?);
    }
    for code in [71, 72, 73, 74, 75, 76] {
        g.i(code, i64::from(o.d.b()?));
    }
    for code in [77, 78, 79] {
        g.i(code, i64::from(o.d.bs()?));
    }
    if r2007 {
        g.i(90, i64::from(o.d.bs()?));
    }
    for code in [140, 141, 142, 143, 144, 145, 146, 147, 148] {
        g.f(code, o.d.bd()?);
    }
    g.i(170, i64::from(o.d.b()?));
    g.i(171, i64::from(o.d.bs()?));
    for code in [172, 173, 174, 175] {
        g.i(code, i64::from(o.d.b()?));
    }
    for code in [176, 177, 178] {
        g.i(code, color_number(o)?);
    }
    g.i(179, i64::from(o.d.bs()?));
    for code in [271, 272, 273, 274, 275, 276, 277, 278, 279, 280] {
        g.i(code, i64::from(o.d.bs()?));
    }
    g.i(281, i64::from(o.d.b()?));
    g.i(282, i64::from(o.d.b()?));
    for code in [283, 284, 285, 286] {
        g.i(code, i64::from(o.d.bs()?));
    }
    g.i(288, i64::from(o.d.b()?));
    g.i(289, i64::from(o.d.bs()?));
    if r2007 {
        g.i(290, i64::from(o.d.b()?));
    }
    if r2010 {
        g.i(294, i64::from(o.d.b()?));
        o.d.bd()?;
        o.text()?;
        o.d.bd()?;
        o.text()?;
    }
    g.i(371, i64::from(o.d.bs()?));
    g.i(372, i64::from(o.d.bs()?));
    o.d.b()?;
    let mut refs = vec![340, 341, 342, 343, 344];
    if r2007 {
        refs.extend([345, 346, 347]);
    }
    for code in refs {
        if let Some(h) = o.href()? {
            g.h(code, h);
        }
    }
    Ok(())
}

/// VPORT (spec §20.4.63).
fn vport(o: &mut Obj, g: &mut G) -> Result<()> {
    let (name, flags) = table_entry(o)?;
    let height = o.d.bd()?;
    // Stored as the view's width (aspect ratio × height).
    let width = o.d.bd()?;
    let aspect = if height != 0.0 { width / height } else { 1.0 };
    let center = o.d.rd2()?;
    let target = o.d.bd3()?;
    let dir = o.d.bd3()?;
    let twist = o.d.bd()?;
    let lens = o.d.bd()?;
    let front = o.d.bd()?;
    let back = o.d.bd()?;
    let mode = o.d.bits(4)?;
    let render = o.d.rc()?;
    let mut lights = None;
    if o.at_least(Version::R2007) {
        let dl = o.d.b()?;
        let lt = o.d.rc()?;
        let b = o.d.bd()?;
        let c = o.d.bd()?;
        let amb = color_number(o)?;
        lights = Some((dl, lt, b, c, amb));
    }
    let lower = o.d.rd2()?;
    let upper = o.d.rd2()?;
    let ucs_follow = o.d.b()?;
    let circle = o.d.bs()?;
    let fast = o.d.b()?;
    let icon = o.d.bb()?;
    let grid = o.d.b()?;
    let grid_spacing = o.d.rd2()?;
    let snap = o.d.b()?;
    let snap_style = o.d.b()?;
    let isopair = o.d.bs()?;
    let snap_rot = o.d.bd()?;
    let snap_base = o.d.rd2()?;
    let snap_spacing = o.d.rd2()?;
    o.d.b()?;
    let ucs_per = o.d.b()?;
    let origin = o.d.bd3()?;
    let xaxis = o.d.bd3()?;
    let yaxis = o.d.bd3()?;
    let elevation = o.d.bd()?;
    let ortho = o.d.bs()?;
    let grid_flags = if o.at_least(Version::R2007) { Some((o.d.bs()?, o.d.bs()?)) } else { None };
    // The view mode's four bits: perspective, front clip, back clip, front clip at eye; and UCS
    // follow (8 in DXF 71).
    let mut vm = 0i64;
    for (i, bit) in [1, 2, 4, 16].iter().enumerate() {
        if mode & (1 << (3 - i)) != 0 {
            vm |= bit;
        }
    }
    if ucs_follow {
        vm |= 8;
    }
    g.s(100, "AcDbSymbolTableRecord");
    g.s(100, "AcDbViewportTableRecord");
    g.s(2, name);
    g.i(70, flags);
    g.p2(10, lower);
    g.p2(11, upper);
    g.p2(12, center);
    g.p2(13, snap_base);
    g.p2(14, snap_spacing);
    g.p2(15, grid_spacing);
    g.p(16, dir);
    g.p(17, target);
    g.f(40, height);
    g.f(41, aspect);
    g.f(42, lens);
    g.f(43, front);
    g.f(44, back);
    g.f(50, snap_rot.to_degrees());
    g.f(51, twist.to_degrees());
    g.i(71, vm);
    g.i(72, i64::from(circle));
    g.i(73, i64::from(fast));
    g.i(74, i64::from(icon));
    g.i(75, i64::from(snap));
    g.i(76, i64::from(grid));
    g.i(77, i64::from(snap_style));
    g.i(78, i64::from(isopair));
    g.i(281, i64::from(render));
    g.i(65, i64::from(ucs_per));
    g.p(110, origin);
    g.p(111, xaxis);
    g.p(112, yaxis);
    g.i(79, i64::from(ortho));
    g.f(146, elevation);
    if let Some((f, m)) = grid_flags {
        g.i(60, i64::from(f));
        g.i(61, i64::from(m));
    }
    if let Some((dl, lt, b, c, amb)) = lights {
        g.i(292, i64::from(dl));
        g.i(282, i64::from(lt));
        g.f(141, b);
        g.f(142, c);
        g.i(63, amb);
    }
    Ok(())
}

/// VIEW (spec §20.4.61).
fn view(o: &mut Obj, g: &mut G) -> Result<()> {
    let (name, flags) = table_entry(o)?;
    let height = o.d.bd()?;
    let width = o.d.bd()?;
    let center = o.d.rd2()?;
    let target = o.d.bd3()?;
    let dir = o.d.bd3()?;
    let twist = o.d.bd()?;
    let lens = o.d.bd()?;
    let front = o.d.bd()?;
    let back = o.d.bd()?;
    let mode = o.d.bits(4)?;
    let render = o.d.rc()?;
    let mut lights = None;
    if o.at_least(Version::R2007) {
        let dl = o.d.b()?;
        let lt = o.d.rc()?;
        let b = o.d.bd()?;
        let c = o.d.bd()?;
        let amb = color_number(o)?;
        lights = Some((dl, lt, b, c, amb));
    }
    let pspace = o.d.b()?;
    let assoc = o.d.b()?;
    let mut ucs = None;
    if assoc {
        ucs = Some((o.d.bd3()?, o.d.bd3()?, o.d.bd3()?, o.d.bd()?, o.d.bs()?));
    }
    if o.at_least(Version::R2007) {
        o.d.b()?;
    }
    let mut vm = 0i64;
    for (i, bit) in [1, 2, 4, 16].iter().enumerate() {
        if mode & (1 << (3 - i)) != 0 {
            vm |= bit;
        }
    }
    g.s(100, "AcDbSymbolTableRecord");
    g.s(100, "AcDbViewTableRecord");
    g.s(2, name);
    g.i(70, flags | if pspace { 1 } else { 0 });
    g.f(40, height);
    g.p2(10, center);
    g.f(41, width);
    g.p(11, dir);
    g.p(12, target);
    g.f(42, lens);
    g.f(43, front);
    g.f(44, back);
    g.f(50, twist.to_degrees());
    g.i(71, vm);
    g.i(281, i64::from(render));
    g.i(72, i64::from(assoc));
    if let Some((dl, lt, b, c, amb)) = lights {
        g.i(292, i64::from(dl));
        g.i(282, i64::from(lt));
        g.f(141, b);
        g.f(142, c);
        g.i(63, amb);
    }
    if let Some((origin, x, y, elev, ortho)) = ucs {
        g.p(110, origin);
        g.p(111, x);
        g.p(112, y);
        g.i(79, i64::from(ortho));
        g.f(146, elev);
    }
    Ok(())
}

/// UCS (spec §20.4.59).
fn ucs(o: &mut Obj, g: &mut G) -> Result<()> {
    let (name, flags) = table_entry(o)?;
    let origin = o.d.bd3()?;
    let x = o.d.bd3()?;
    let y = o.d.bd3()?;
    let elevation = o.d.bd()?;
    let ortho_view = o.d.bs()?;
    let ortho = o.d.bs()?;
    g.s(100, "AcDbSymbolTableRecord");
    g.s(100, "AcDbUCSTableRecord");
    g.s(2, name);
    g.i(70, flags);
    g.p(10, origin);
    g.p(11, x);
    g.p(12, y);
    g.i(79, i64::from(ortho_view));
    g.f(146, elevation);
    if ortho != 0 {
        g.i(71, i64::from(ortho));
    }
    Ok(())
}

/// DICTIONARY and the dictionary with a default (spec §20.4.42): entry names in the data, their
/// objects in the handles.
fn dictionary(name: &str, o: &mut Obj, g: &mut G) -> Result<()> {
    let n = count(o.d.bl()?, "dictionary entries")?;
    let cloning = o.d.bs()?;
    let hard = o.d.rc()?;
    let names: Vec<String> = (0..n).map(|_| o.text()).collect::<Result<_>>()?;
    g.s(100, "AcDbDictionary");
    if hard != 0 {
        g.i(280, i64::from(hard));
    }
    g.i(281, i64::from(cloning));
    for entry in names {
        let h = o.href()?;
        g.s(3, entry);
        g.h(if hard != 0 { 360 } else { 350 }, h.unwrap_or(0));
    }
    if name == "ACDBDICTIONARYWDFLT" {
        let d = o.href()?;
        g.s(100, "AcDbDictionaryWithDefault");
        g.h(340, d.unwrap_or(0));
    }
    Ok(())
}

/// The plot settings every layout starts with (spec §20.4.88 PLOTSETTINGS).
fn plot_settings(o: &mut Obj, g: &mut G) -> Result<()> {
    let page = o.text()?;
    let printer = o.text()?;
    let flags = o.d.bs()?;
    let margins = [o.d.bd()?, o.d.bd()?, o.d.bd()?, o.d.bd()?];
    let paper = [o.d.bd()?, o.d.bd()?];
    let paper_name = o.text()?;
    let origin = [o.d.bd()?, o.d.bd()?];
    let units = o.d.bs()?;
    let rotation = o.d.bs()?;
    let plot_type = o.d.bs()?;
    let wmin = [o.d.bd()?, o.d.bd()?];
    let wmax = [o.d.bd()?, o.d.bd()?];
    let view_name = if o.at_least(Version::R2004) { String::new() } else { o.text()? };
    let numerator = o.d.bd()?;
    let denominator = o.d.bd()?;
    let sheet = o.text()?;
    let scale_type = o.d.bs()?;
    let scale = o.d.bd()?;
    let image_origin = [o.d.bd()?, o.d.bd()?];
    let shade = if o.at_least(Version::R2004) { Some((o.d.bs()?, o.d.bs()?, o.d.bs()?)) } else { None };
    if o.at_least(Version::R2004) {
        o.href()?;
    }
    if o.at_least(Version::R2007) {
        o.href()?;
    }
    g.s(100, "AcDbPlotSettings");
    g.s(1, page);
    g.s(2, printer);
    g.s(4, paper_name);
    g.s(6, view_name);
    g.f(40, margins[0]);
    g.f(41, margins[1]);
    g.f(42, margins[2]);
    g.f(43, margins[3]);
    g.f(44, paper[0]);
    g.f(45, paper[1]);
    g.f(46, origin[0]);
    g.f(47, origin[1]);
    g.f(48, wmin[0]);
    g.f(49, wmin[1]);
    g.f(140, wmax[0]);
    g.f(141, wmax[1]);
    g.f(142, numerator);
    g.f(143, denominator);
    g.i(70, i64::from(flags));
    g.i(72, i64::from(units));
    g.i(73, i64::from(rotation));
    g.i(74, i64::from(plot_type));
    g.s(7, sheet);
    g.i(75, i64::from(scale_type));
    if let Some((mode, res, dpi)) = shade {
        g.i(76, i64::from(mode));
        g.i(77, i64::from(res));
        g.i(78, i64::from(dpi));
    }
    g.f(147, scale);
    g.f(148, image_origin[0]);
    g.f(149, image_origin[1]);
    Ok(())
}

/// LAYOUT (spec §20.4.82), after its plot settings.
fn layout(o: &mut Obj, g: &mut G) -> Result<()> {
    let name = o.text()?;
    let tab = o.d.bs()?;
    let flags = o.d.bs()?;
    let ucs_origin = o.d.bd3()?;
    let limmin = o.d.rd2()?;
    let limmax = o.d.rd2()?;
    let ins = o.d.bd3()?;
    let xaxis = o.d.bd3()?;
    let yaxis = o.d.bd3()?;
    let elevation = o.d.bd()?;
    let ortho = o.d.bs()?;
    let extmin = o.d.bd3()?;
    let extmax = o.d.bd3()?;
    let nvp = if o.at_least(Version::R2004) { count(o.d.bl()?, "layout viewports")? } else { 0 };
    let block = o.href()?;
    let last = o.href()?;
    let base_ucs = o.href()?;
    let named_ucs = o.href()?;
    for _ in 0..nvp {
        o.href()?;
    }
    g.s(100, "AcDbLayout");
    g.s(1, name);
    g.i(70, i64::from(flags));
    g.i(71, i64::from(tab));
    g.p2(10, limmin);
    g.p2(11, limmax);
    g.p(12, ins);
    g.p(14, extmin);
    g.p(15, extmax);
    g.f(146, elevation);
    g.p(13, ucs_origin);
    g.p(16, xaxis);
    g.p(17, yaxis);
    g.i(76, i64::from(ortho));
    g.h(330, block.unwrap_or(0));
    g.h(331, last.unwrap_or(0));
    if let Some(h) = base_ucs {
        g.h(346, h);
    }
    if let Some(h) = named_ucs {
        g.h(345, h);
    }
    Ok(())
}

/// MLINESTYLE (spec §20.4.73).
fn mlinestyle(o: &mut Obj, names: &Names, g: &mut G) -> Result<()> {
    let name = o.text()?;
    let description = o.text()?;
    let flags = o.d.bs()?;
    let fill = color_number(o)?;
    let start = o.d.bd()?;
    let end = o.d.bd()?;
    let n = usize::from(o.d.rc()?);
    let mut lines = Vec::with_capacity(n);
    for _ in 0..n {
        let offset = o.d.bd()?;
        let color = color_number(o)?;
        let ltype = if o.at_least(Version::R2018) {
            o.href()?.and_then(|h| names.name(h)).unwrap_or("BYLAYER").to_string()
        } else {
            match o.d.bs()? {
                32767 => "BYLAYER".to_string(),
                32766 => "BYBLOCK".to_string(),
                _ => "Continuous".to_string(),
            }
        };
        lines.push((offset, color, ltype));
    }
    g.s(100, "AcDbMlineStyle");
    g.s(2, name);
    g.i(70, i64::from(flags));
    g.s(3, description);
    g.i(62, fill);
    g.f(51, start.to_degrees());
    g.f(52, end.to_degrees());
    g.i(71, n as i64);
    for (offset, color, ltype) in lines {
        g.f(49, offset);
        g.i(62, color);
        g.s(6, ltype);
    }
    Ok(())
}

/// MLEADERSTYLE (spec §20.4.87; angles in radians, as DXF has them).
fn mleaderstyle(o: &mut Obj, g: &mut G) -> Result<()> {
    use super::kinds::cmc;
    g.s(100, "AcDbMLeaderStyle");
    if o.at_least(Version::R2010) {
        g.i(179, i64::from(o.d.bs()?));
    }
    g.i(170, i64::from(o.d.bs()?));
    g.i(171, i64::from(o.d.bs()?));
    g.i(172, i64::from(o.d.bs()?));
    g.i(90, i64::from(o.d.bl()?));
    g.f(40, o.d.bd()?);
    g.f(41, o.d.bd()?);
    g.i(173, i64::from(o.d.bs()?));
    g.i(91, cmc(o)?);
    let ltype = o.href()?;
    g.h(340, ltype.unwrap_or(0));
    g.i(92, i64::from(o.d.bl()?));
    g.i(290, i64::from(o.d.b()?));
    g.f(42, o.d.bd()?);
    g.i(291, i64::from(o.d.b()?));
    g.f(43, o.d.bd()?);
    g.s(3, o.text()?);
    let arrow = o.href()?;
    if let Some(h) = arrow {
        g.h(341, h);
    }
    g.f(44, o.d.bd()?);
    g.s(300, o.text()?);
    let style = o.href()?;
    g.h(342, style.unwrap_or(0));
    g.i(174, i64::from(o.d.bs()?));
    g.i(178, i64::from(o.d.bs()?));
    g.i(175, i64::from(o.d.bs()?));
    g.i(176, i64::from(o.d.bs()?));
    g.i(93, cmc(o)?);
    g.f(45, o.d.bd()?);
    g.i(292, i64::from(o.d.b()?));
    g.i(297, i64::from(o.d.b()?));
    g.f(46, o.d.bd()?);
    let block = o.href()?;
    if let Some(h) = block {
        g.h(343, h);
    }
    g.i(94, cmc(o)?);
    let scale = o.d.bd3()?;
    g.f(47, scale[0]);
    g.f(49, scale[1]);
    g.f(140, scale[2]);
    g.i(293, i64::from(o.d.b()?));
    g.f(141, o.d.bd()?);
    g.i(294, i64::from(o.d.b()?));
    g.i(177, i64::from(o.d.bs()?));
    g.f(142, o.d.bd()?);
    g.i(295, i64::from(o.d.b()?));
    g.i(296, i64::from(o.d.b()?));
    g.f(143, o.d.bd()?);
    if o.at_least(Version::R2010) {
        g.i(271, i64::from(o.d.bs()?));
        let top = o.d.bs()?;
        let bottom = o.d.bs()?;
        g.i(272, i64::from(bottom));
        g.i(273, i64::from(top));
    }
    // From R2013 one more bit (observed; not in the spec).
    if o.at_least(Version::R2013) {
        o.d.b()?;
    }
    Ok(())
}

/// GROUP (spec §20.4.72).
fn group(o: &mut Obj, g: &mut G) -> Result<()> {
    let description = o.text()?;
    let unnamed = o.d.bs()?;
    let selectable = o.d.bs()?;
    let n = count(o.d.bl()?, "group members")?;
    g.s(100, "AcDbGroup");
    g.s(300, description);
    g.i(70, i64::from(unnamed));
    g.i(71, i64::from(selectable));
    for _ in 0..n {
        if let Some(h) = o.href()? {
            g.h(340, h);
        }
    }
    Ok(())
}

/// XRECORD (spec §20.4.79): the data is a list of DXF groups (a 16-bit code, then the value in
/// the code's type), then the cloning flag.
fn xrecord(o: &mut Obj, g: &mut G) -> Result<()> {
    let n = count(o.d.bl()?, "xrecord bytes")?;
    let data = o.d.bytes(n)?;
    let cloning = o.d.bs()?;
    g.s(100, "AcDbXrecord");
    g.i(280, i64::from(cloning));
    let mut r = crate::bits::BitReader::new(&data);
    let wide = o.at_least(Version::R2007);
    while r.remaining_bits() >= 16 {
        let Ok(code) = r.rs() else { break };
        let code = i32::from(code as i16);
        let item = (|| -> Result<Value> {
            Ok(match code {
                1..=9 | 100..=109 | 300..=309 | 410..=419 | 430..=439 | 470..=479 | 999 | 1000..=1003 | 1006..=1009 => {
                    if wide {
                        let len = usize::from(r.rs()?);
                        let w: Vec<u16> = (0..len).map(|_| r.rs()).collect::<Result<_>>()?;
                        Value::Str(String::from_utf16_lossy(&w))
                    } else {
                        let len = usize::from(r.rs()?);
                        r.rc()?;
                        Value::Str(crate::bits::Text::Bytes(r.bytes(len)?).to_string_lossy())
                    }
                }
                10..=17 | 1010..=1019 => {
                    let p = [r.rd()?, r.rd()?, r.rd()?];
                    g.f(code, p[0]);
                    g.f(code + 10, p[1]);
                    Value::Real(p[2])
                }
                18..=59 | 110..=149 | 210..=239 | 460..=469 | 1020..=1059 => Value::Real(r.rd()?),
                60..=79 | 170..=179 | 270..=289 | 370..=389 | 400..=409 | 1060..=1070 => Value::Int(i64::from(r.rs()? as i16)),
                90..=99 | 420..=429 | 440..=449 | 450..=459 | 1071 => Value::Int(i64::from(r.rl()? as i32)),
                160..=169 => Value::Int(r.rll()? as i64),
                290..=299 => Value::Int(i64::from(r.rc()?)),
                310..=319 | 1004 => {
                    let len = usize::from(r.rc()?);
                    Value::Str(r.bytes(len)?.iter().map(|b| format!("{b:02X}")).collect())
                }
                320..=369 | 390..=399 | 480..=481 | 1005 => {
                    let b = r.bytes(8)?;
                    let mut a = [0u8; 8];
                    a.copy_from_slice(&b);
                    Value::Handle(u64::from_le_bytes(a))
                }
                _ => return Err(crate::error::corrupt(format!("an xrecord group code {code}"))),
            })
        })();
        match item {
            Ok(v) => {
                // A point's third value is pushed here, its first two above.
                let code = if matches!(code, 10..=17 | 1010..=1019) { code + 20 } else { code };
                g.0.push(Group { code, value: v });
            }
            Err(_) => break,
        }
    }
    Ok(())
}
