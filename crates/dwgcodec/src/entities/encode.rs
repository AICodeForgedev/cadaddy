//! Writing objects (Q201): the DXF groups of an entity, table record or object made into a DWG
//! object record — the inverse of [`super::decode`] and [`super::records::decode`], so that
//! decoding what is written here gives the same groups back.
//!
//! [`ObjWriter`] lays out an object's streams as [`super::Obj::open`] reads them (spec §20.1): the
//! type, the R2000–R2004 size in bits (where the handles start), the object's own handle and its
//! data; from R2007 the strings in a string stream after the data, with its size and flag; the
//! handle stream last. From R2010 the record starts with the handle stream's size in bits and the
//! handles end on the record's last bit.

use std::collections::HashMap;

use super::{Group, Value};
use crate::Version;
use crate::bits::{BitReader, BitWriter, HandleRef, Text};
use crate::objects::RawObject;

/// The groups of one record, by code.
pub struct Rec<'a>(pub &'a [Group]);

impl Rec<'_> {
    pub fn f(&self, code: i32) -> Option<f64> {
        self.0.iter().find(|g| g.code == code).and_then(|g| match &g.value {
            Value::Real(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            Value::Str(s) => s.trim().parse().ok(),
            Value::Handle(_) => None,
        })
    }
    pub fn fd(&self, code: i32, default: f64) -> f64 {
        self.f(code).filter(|f| f.is_finite()).unwrap_or(default)
    }
    pub fn i(&self, code: i32) -> Option<i64> {
        self.0.iter().find(|g| g.code == code).and_then(|g| match &g.value {
            Value::Int(i) => Some(*i),
            Value::Real(f) if f.is_finite() => Some(*f as i64),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        })
    }
    pub fn id(&self, code: i32, default: i64) -> i64 {
        self.i(code).unwrap_or(default)
    }
    pub fn s(&self, code: i32) -> Option<&str> {
        self.0.iter().find(|g| g.code == code).and_then(|g| match &g.value {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        })
    }
    pub fn h(&self, code: i32) -> Option<u64> {
        self.0.iter().find(|g| g.code == code).and_then(|g| match &g.value {
            Value::Handle(h) => Some(*h),
            Value::Str(s) => u64::from_str_radix(s.trim(), 16).ok(),
            Value::Int(i) => u64::try_from(*i).ok(),
            Value::Real(_) => None,
        })
    }
    /// A point from `code`, `code + 10`, `code + 20` (z 0 when missing).
    pub fn p(&self, code: i32) -> [f64; 3] {
        [self.fd(code, 0.0), self.fd(code + 10, 0.0), self.fd(code + 20, 0.0)]
    }
    pub fn p2(&self, code: i32) -> [f64; 2] {
        [self.fd(code, 0.0), self.fd(code + 10, 0.0)]
    }
    /// Every group with `code`, in order.
    pub fn all(&self, code: i32) -> impl Iterator<Item = &Group> {
        self.0.iter().filter(move |g| g.code == code)
    }
}

/// The handles names refer to, by table and name (upper case); and the object type numbers of
/// the file's classes, by DXF name.
#[derive(Clone, Debug, Default)]
pub struct Handles {
    pub by_name: HashMap<(String, String), u64>,
    pub classes: HashMap<String, u16>,
}

impl Handles {
    /// The object type of the class with DXF name `name`.
    pub fn class(&self, name: &str) -> Option<u16> {
        self.classes.get(name).copied()
    }
    pub fn get(&self, table: &str, name: &str) -> Option<u64> {
        self.by_name.get(&(table.to_ascii_uppercase(), name.to_ascii_uppercase())).copied()
    }
    pub fn insert(&mut self, table: &str, name: &str, h: u64) {
        self.by_name.insert((table.to_ascii_uppercase(), name.to_ascii_uppercase()), h);
    }
}

/// An object being written.
pub struct ObjWriter {
    pub v: Version,
    pub handle: u64,
    type_code: u16,
    pub d: BitWriter,
    s: BitWriter,
    h: BitWriter,
}

fn copy_bits(to: &mut BitWriter, from: &BitWriter) {
    let mut r = BitReader::new(from.as_bytes());
    for _ in 0..from.bit_len() {
        to.b(r.b().unwrap_or(false));
    }
}

impl ObjWriter {
    pub fn new(v: Version, type_code: u16, handle: u64) -> ObjWriter {
        ObjWriter { v, handle, type_code, d: BitWriter::new(), s: BitWriter::new(), h: BitWriter::new() }
    }

    /// A string (TV) in the version's stream.
    pub fn text(&mut self, s: &str) {
        if self.v >= Version::R2007 {
            let t = if s.is_empty() { Text::Empty } else { Text::Wide(s.encode_utf16().collect()) };
            self.s.tv(&t, true);
        } else {
            let t = if s.is_empty() { Text::Empty } else { Text::Bytes(crate::bits::code_page_bytes(s)) };
            self.d.tv(&t, false);
        }
    }

    /// A handle reference with `code` (spec §2.13: 2 soft owner, 3 hard owner, 4 soft pointer,
    /// 5 hard pointer) to `h` (null when `None`).
    pub fn href(&mut self, code: u8, h: Option<u64>) {
        self.h.h(HandleRef { code, value: h.unwrap_or(0) });
    }

    pub fn at_least(&self, v: Version) -> bool {
        self.v >= v
    }

    /// The finished record.
    pub fn finish(self) -> RawObject {
        let r2010 = self.v >= Version::R2010;
        let mut head = BitWriter::new();
        head.ot(self.type_code, r2010);
        let own = HandleRef { code: 0, value: self.handle };
        if !r2010 {
            // Type, size in bits, own handle, data: the handles start after them.
            let mut probe = BitWriter::new();
            probe.h(own);
            let mut size = head.bit_len() + 32 + probe.bit_len() + self.d.bit_len();
            let mut strings = BitWriter::new();
            if self.v >= Version::R2007 {
                crate::bits::put_string_stream(&mut strings, self.s.as_bytes(), self.s.bit_len());
                size += strings.bit_len();
            }
            head.rl(size as u32);
            head.h(own);
            copy_bits(&mut head, &self.d);
            copy_bits(&mut head, &strings);
            copy_bits(&mut head, &self.h);
            let data = head.into_bytes();
            return RawObject { handle: self.handle, type_code: self.type_code, data };
        }
        // R2010+: the handle stream's size, then type, own handle, data, the string stream with
        // its size and flag, and the handles; the record padded to a whole byte at the end, the
        // padding counted with the handles (as the files we've seen do).
        head.h(own);
        copy_bits(&mut head, &self.d);
        crate::bits::put_string_stream(&mut head, self.s.as_bytes(), self.s.bit_len());
        copy_bits(&mut head, &self.h);
        let pad = (8 - head.bit_len() % 8) % 8;
        for _ in 0..pad {
            head.b(false);
        }
        let mut out = BitWriter::new();
        out.mc_unsigned((self.h.bit_len() + pad) as u64);
        let mut data = out.into_bytes();
        data.extend_from_slice(&head.into_bytes());
        RawObject { handle: self.handle, type_code: self.type_code, data }
    }
}

/// A colour (spec §2.11 CMC) of DXF colour number `index` (256 ByLayer, 0 ByBlock) and true
/// colour `rgb`: the index, and from R2004 the raw value and no names.
pub fn cmc(w: &mut ObjWriter, index: i64, rgb: Option<i64>) {
    w.d.bs(index as i16);
    if w.at_least(Version::R2004) {
        let raw = match (rgb, index) {
            (Some(c), _) => 0xC200_0000u32 | (c as u32 & 0xFF_FFFF),
            (None, 256) => 0xC000_0000,
            (None, 0) => 0xC100_0000,
            (None, i) => 0xC300_0000u32 | (i as u32 & 0xFF),
        };
        w.d.bl(raw as i32);
        w.d.rc(0);
    }
}

/// A stored lineweight index of a DXF lineweight (spec §20.4.1's table).
pub fn lineweight_index(lw: i64) -> u8 {
    const LW: [i64; 24] = [0, 5, 9, 13, 15, 18, 20, 25, 30, 35, 40, 50, 53, 60, 70, 80, 90, 100, 106, 120, 140, 158, 200, 211];
    match lw {
        -1 => 29,
        -2 => 30,
        -3 => 31,
        v => LW.iter().position(|x| *x == v).map_or(29, |i| i as u8),
    }
}

/// The parts of the common entity data a record's groups give (inverse of [`super::common`]).
pub struct EntityHead {
    /// 0 owned by a block (the owner's handle), 1 paper space, 2 model space.
    pub entmode: u8,
    pub owner: Option<u64>,
    pub reactors: Vec<u64>,
    pub xdict: Option<u64>,
    pub layer: Option<u64>,
    pub ltype: Option<u64>,
    /// 0 ByLayer, 1 ByBlock, 2 Continuous, 3 the handle.
    pub ltype_flags: u8,
    pub material: Option<u64>,
    pub plotstyle: Option<u64>,
    /// R2000: the previous and next entity of its block (`None`: no links stored).
    pub links: Option<(Option<u64>, Option<u64>)>,
}

/// Writes the common entity data (spec §20.4.1) for `r`'s groups: extended data (none), no
/// graphics, entity mode, reactors, dictionary, colour, linetype, plot style, material,
/// visibility, lineweight; and their handles.
pub fn entity_common(w: &mut ObjWriter, r: &Rec, head: &EntityHead, names: &Handles) {
    eed(w, r, names);
    // No proxy graphics.
    w.d.b(false);
    w.d.bb(head.entmode);
    w.d.bl(head.reactors.len() as i32);
    if w.at_least(Version::R2004) {
        w.d.b(head.xdict.is_none());
    }
    if w.at_least(Version::R2013) {
        w.d.b(false);
    }
    // R2000: no links only when the previous and next entities are the handles just below and
    // above (spec §20.4.1). The first, last or only entity of a block stores its null links:
    // readers take "no links" to mean handle − 1 and handle + 1, and the ODA's rejects the file
    // when those are other objects.
    let h = w.handle;
    let links = head.links.filter(|(p, n)| p.and_then(|p| p.checked_add(1)) != Some(h) || h.checked_add(1) != *n);
    if !w.at_least(Version::R2004) {
        w.d.b(links.is_none());
    }
    let index = r.i(62).unwrap_or(256);
    if w.at_least(Version::R2004) {
        let rgb = r.i(420);
        let transparency = r.i(440);
        let mut flags = (index.unsigned_abs() & 0x1ff) as u16;
        if rgb.is_some() {
            flags |= 0x8000;
        }
        if transparency.is_some() {
            flags |= 0x2000;
        }
        w.d.bs(flags as i16);
        if let Some(c) = rgb {
            w.d.bl((0xC200_0000u32 | (c as u32 & 0xFF_FFFF)) as i32);
        }
        if let Some(t) = transparency {
            w.d.bl(t as i32);
        }
    } else {
        w.d.bs(index as i16);
    }
    w.d.bd(r.fd(48, 1.0));
    w.d.bb(head.ltype_flags);
    w.d.bb(if head.plotstyle.is_some() { 3 } else { 0 });
    if w.at_least(Version::R2007) {
        w.d.bb(if head.material.is_some() { 3 } else { 0 });
        w.d.rc(0);
    }
    if w.at_least(Version::R2010) {
        w.d.b(false);
        w.d.b(false);
        w.d.b(false);
    }
    w.d.bs(if r.id(60, 0) != 0 { 1 } else { 0 });
    w.d.rc(lineweight_index(r.id(370, -1)));
    // The handles.
    if head.entmode == 0 {
        w.href(4, head.owner);
    }
    for rr in &head.reactors {
        w.href(4, Some(*rr));
    }
    // Before R2004 the dictionary is always there (null when there is none).
    if head.xdict.is_some() || !w.at_least(Version::R2004) {
        w.href(3, head.xdict);
    }
    if !w.at_least(Version::R2004)
        && let Some((prev, next)) = links
    {
        w.href(4, prev);
        w.href(4, next);
    }
    w.href(5, head.layer);
    if head.ltype_flags == 3 {
        w.href(5, head.ltype);
    }
    if w.at_least(Version::R2007) && head.material.is_some() {
        w.href(5, head.material);
    }
    if head.plotstyle.is_some() {
        w.href(5, head.plotstyle);
    }
}

/// The record's extended data (its groups from the first 1001 on) as the blocks the spec §20.4.1
/// stores: per application its size, handle and items; a size of 0 ends them. Applications that
/// aren't in the APPID table, and items without a stored form, are left out.
pub fn eed(w: &mut ObjWriter, r: &Rec, names: &Handles) {
    let start = r.0.iter().position(|g| g.code == 1001).unwrap_or(r.0.len());
    let gs = r.0.get(start..).unwrap_or(&[]);
    let unicode = w.at_least(Version::R2007);
    let mut k = 0;
    while let Some(g) = gs.get(k) {
        k += 1;
        let Value::Str(app) = &g.value else { continue };
        let mut data: Vec<u8> = Vec::new();
        while let Some(item) = gs.get(k).filter(|x| x.code != 1001) {
            k += 1;
            let x = match &item.value {
                Value::Real(f) => *f,
                Value::Int(i) => *i as f64,
                Value::Str(s) => s.trim().parse().unwrap_or(0.0),
                Value::Handle(h) => *h as f64,
            };
            let hex_handle = || match &item.value {
                Value::Handle(h) => *h,
                Value::Str(s) => u64::from_str_radix(s.trim(), 16).unwrap_or(0),
                Value::Int(i) => *i as u64,
                Value::Real(_) => 0,
            };
            match item.code {
                1000 => {
                    let s = if let Value::Str(s) = &item.value { s.as_str() } else { "" };
                    data.push(0);
                    if unicode {
                        let u: Vec<u16> = s.encode_utf16().take(255).collect();
                        data.extend_from_slice(&(u.len() as u16).to_le_bytes());
                        for c in u {
                            data.extend_from_slice(&c.to_le_bytes());
                        }
                    } else {
                        let b: Vec<u8> = crate::bits::code_page_bytes(s).into_iter().take(255).collect();
                        data.push(b.len() as u8);
                        // The code page (ANSI 1252), high byte first (as the files we've seen
                        // store it).
                        data.extend_from_slice(&30u16.to_be_bytes());
                        data.extend_from_slice(&b);
                    }
                }
                1002 => {
                    data.push(2);
                    data.push(u8::from(matches!(&item.value, Value::Str(s) if s.trim() == "}")));
                }
                // A handle: 8 bytes, high byte first (as the files we've seen store it).
                1003 | 1005 => {
                    data.push((item.code - 1000) as u8);
                    data.extend_from_slice(&hex_handle().to_be_bytes());
                }
                1004 => {
                    let s = if let Value::Str(s) = &item.value { s.as_str() } else { "" };
                    let b: Vec<u8> =
                        (0..s.len() / 2).filter_map(|i| s.get(2 * i..2 * i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).take(255).collect();
                    data.push(4);
                    data.push(b.len() as u8);
                    data.extend_from_slice(&b);
                }
                1010..=1013 => {
                    let y = gs.get(k).filter(|n| n.code == item.code + 10);
                    let yv = y.map(|n| match &n.value {
                        Value::Real(f) => *f,
                        Value::Int(i) => *i as f64,
                        _ => 0.0,
                    });
                    if y.is_some() {
                        k += 1;
                    }
                    let z = gs.get(k).filter(|n| n.code == item.code + 20);
                    let zv = z.map(|n| match &n.value {
                        Value::Real(f) => *f,
                        Value::Int(i) => *i as f64,
                        _ => 0.0,
                    });
                    if z.is_some() {
                        k += 1;
                    }
                    data.push((item.code - 1000) as u8);
                    for c in [x, yv.unwrap_or(0.0), zv.unwrap_or(0.0)] {
                        data.extend_from_slice(&c.to_le_bytes());
                    }
                }
                1040..=1042 => {
                    data.push((item.code - 1000) as u8);
                    data.extend_from_slice(&x.to_le_bytes());
                }
                1070 => {
                    data.push(70);
                    data.extend_from_slice(&(x as i16).to_le_bytes());
                }
                1071 => {
                    data.push(71);
                    data.extend_from_slice(&(x as i32).to_le_bytes());
                }
                _ => {}
            }
        }
        let Some(h) = names.get("APPID", app) else { continue };
        if data.is_empty() || data.len() > i16::MAX as usize {
            continue;
        }
        w.d.bs(data.len() as i16);
        w.d.h(HandleRef { code: 5, value: h });
        w.d.bytes(&data);
    }
    w.d.bs(0);
}

/// What every non-entity object starts with (spec §20.4.2): no extended data, reactors, the
/// dictionary; owner, reactor and dictionary handles.
pub fn object_common(w: &mut ObjWriter, owner: Option<u64>, reactors: &[u64], xdict: Option<u64>, r: &Rec, names: &Handles) {
    eed(w, r, names);
    w.d.bl(reactors.len() as i32);
    if w.at_least(Version::R2004) {
        w.d.b(xdict.is_none());
    }
    if w.at_least(Version::R2013) {
        w.d.b(false);
    }
    w.href(4, owner);
    for r in reactors {
        w.href(4, Some(*r));
    }
    if xdict.is_some() || !w.at_least(Version::R2004) {
        w.href(3, xdict);
    }
}

/// The common table data (spec §20.4.47): name, flags, and (R2000–R2004) the xref index; the
/// external-reference block's handle.
pub fn table_entry(w: &mut ObjWriter, name: &str, flags: i64) {
    w.text(name);
    // "Referenced": set on every record from R2004, clear before (as the files we've seen have
    // it; readers rely on it).
    w.d.b(w.at_least(Version::R2004));
    if !w.at_least(Version::R2007) {
        w.d.bs(0);
    }
    w.d.b(flags & 16 != 0);
    w.href(5, None);
}
