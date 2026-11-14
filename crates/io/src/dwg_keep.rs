//! Keeping through DWG what acadrust can't write: other programs' custom entities and objects
//! (and their CLASS records), read from a DXF as their own groups. acadrust writes a custom
//! record to DWG only from the record's own DWG bytes, which a DXF doesn't have, so the bridge
//! used to leave them out.
//!
//! On the way to DWG ([`stash`]) such a record is taken out of the DXF whole, groups as written,
//! and kept in an XRECORD of a `CADADDY_KEEP` dictionary hanging off the named object
//! dictionary — XRECORDs go through DWG intact. So are its CLASS record and the references kept
//! records hold to it (a dictionary's entry, a reactor), which would otherwise point at nothing.
//! On the way back ([`unstash`]) the records are put back where they were (the ENTITIES section,
//! the block they were in, or OBJECTS) with their handles, their classes and the references, and
//! the dictionary goes: the DXF reads as it was written.
//!
//! Other programs reading the DWG see the XRECORDs, not the objects themselves (that needs the
//! objects' DWG encoding, which only their own program has).
//!
//! Which records need it the DWG bridge says (`cadaddy_dwg::custom_records`); the drawing's DWG
//! bridge DXF is binary ([`stash_tags`]), what the bridge reads from a DWG is ASCII ([`unstash`]).
//!
//! Other programs' objects that the bridge does write, but gives back otherwise than they were
//! read, get a shadow ([`shadows`]): the object is written as the bridge writes it (so other
//! programs see it), and an XRECORD keeps its groups as they were with a digest of what the bridge
//! gives back for it. Reading the DWG back puts those groups in its place while the file still
//! holds exactly that; an object another program has changed since stays as that program wrote
//! it. A visual style read in the layout of AutoCAD 2013 and later, written into an older DWG,
//! came back with 28 of its 58 properties (test-coord-battle8: 'visualstyle objects of other
//! programs changed').

use std::collections::{BTreeMap, HashMap, HashSet};

/// The dictionary key of the kept records in the named object dictionary.
pub const KEY: &str = "CADADDY_KEEP";
/// Records kept, at most (a hostile DXF can't make the bridge build millions of XRECORDs).
const MAX_KEPT: usize = 200_000;

/// A DXF's groups, as (code, value) with the value as written.
type Pairs = Vec<(i32, String)>;

/// Whether DXF text is in a code page (not UTF-8): an older file's text (the DWG bridge's DXF of
/// an R2000/R2004 DWG is Windows-1252). [`parse`] then reads each byte as the character of the
/// same number and [`write`] writes them back as those bytes, so the text goes through
/// unchanged. (Read as UTF-8 every non-ASCII character became U+FFFD: a layer "fläche" of a
/// DWG with CADaddy's kept records came back "fl�che", savefuzz.)
fn code_page_text(dxf: &[u8]) -> bool {
    std::str::from_utf8(dxf).is_err()
}

/// The groups of ASCII DXF text, or `None` for binary DXF or text that isn't pairs.
fn parse(dxf: &[u8]) -> Option<Pairs> {
    if dxf.starts_with(b"AutoCAD Binary DXF") {
        return None;
    }
    let text: std::borrow::Cow<'_, str> =
        if code_page_text(dxf) { dxf.iter().map(|&b| char::from(b)).collect::<String>().into() } else { String::from_utf8_lossy(dxf) };
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let mut out = Vec::new();
    while let Some(code) = lines.next() {
        if code.trim().is_empty() && out.last().is_some_and(|(c, v): &(i32, String)| *c == 0 && v == "EOF") {
            break;
        }
        let code: i32 = code.trim().parse().ok()?;
        let value = lines.next()?;
        out.push((code, value.to_string()));
    }
    Some(out)
}

/// DXF text of `pairs`, in UTF-8 or (`code_page`, see [`code_page_text`]) each character as the
/// byte of the same number.
fn write(pairs: &[(i32, String)], crlf: bool, code_page: bool) -> Vec<u8> {
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut s = String::with_capacity(pairs.len() * 12);
    for (c, v) in pairs {
        s.push_str(&format!("{c:>3}{nl}{v}{nl}"));
    }
    if !code_page {
        return s.into_bytes();
    }
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        match u8::try_from(u32::from(c)) {
            Ok(b) => out.push(b),
            Err(_) => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
    out
}

/// Records: each from a `0` group to the next.
fn records(pairs: &[(i32, String)]) -> Vec<std::ops::Range<usize>> {
    let starts: Vec<usize> = pairs.iter().enumerate().filter(|(_, (c, _))| *c == 0).map(|(i, _)| i).collect();
    starts.iter().enumerate().map(|(k, s)| *s..starts.get(k + 1).copied().unwrap_or(pairs.len())).collect()
}

fn handle_of(rec: &[(i32, String)]) -> Option<u64> {
    rec.iter().find(|(c, _)| *c == 5 || *c == 105).and_then(|(_, v)| u64::from_str_radix(v.trim(), 16).ok())
}

fn value(rec: &[(i32, String)], code: i32) -> Option<&str> {
    rec.iter().find(|(c, _)| *c == code).map(|(_, v)| v.trim())
}

/// Where each record is: its section, and inside BLOCKS the block it belongs to.
fn containers(pairs: &[(i32, String)], recs: &[std::ops::Range<usize>]) -> Vec<String> {
    let mut section = String::new();
    let mut block: Option<String> = None;
    let mut out = Vec::with_capacity(recs.len());
    for r in recs {
        let rec = &pairs[r.clone()];
        let kind = rec.first().map(|(_, v)| v.trim()).unwrap_or_default();
        match kind {
            "SECTION" => section = value(rec, 2).unwrap_or_default().to_string(),
            "ENDSEC" => section.clear(),
            "BLOCK" if section == "BLOCKS" => block = value(rec, 2).map(str::to_string),
            "ENDBLK" => block = None,
            _ => {}
        }
        out.push(match (&block, kind) {
            // A section's own header and end are in no container.
            (_, "SECTION" | "ENDSEC") => String::new(),
            (Some(b), k) if !matches!(k, "BLOCK" | "ENDBLK") => format!("BLOCK:{b}"),
            _ => section.clone(),
        });
    }
    out
}

/// Kinds of other programs' objects never given a shadow: dictionaries (CADaddy writes the ones it
/// keeps, and their entries point at records that get new handles) and the records that are only
/// data (an XRECORD's groups go through DWG as they are).
const NOT_SHADOWED: [&str; 4] = ["DICTIONARY", "ACDBDICTIONARYWDFLT", "XRECORD", "ACDBPLACEHOLDER"];
/// Other programs' objects checked for a shadow, at most.
const MAX_SHADOW_CHECK: usize = 20_000;
/// Groups of one object compared group by group, at most (a longer one is not given a shadow).
const MAX_SHADOW_GROUPS: usize = 4096;

/// Whether group `code` holds a handle (the record's own, a reference, or a `102` bracket around
/// reactors and extension dictionaries): writers renumber handles, so these are not compared.
fn is_handle_group(code: i32) -> bool {
    matches!(code, 5 | 102 | 105 | 320..=369 | 390..=399 | 480 | 481 | 1005)
}

/// A record's groups as compared and digested: without its type and its [handles](is_handle_group).
fn content(rec: &[(i32, String)]) -> Vec<(i32, &str)> {
    rec.iter().filter(|(c, _)| *c != 0 && !is_handle_group(*c)).map(|(c, v)| (*c, v.trim())).collect()
}

/// A stable digest (64-bit FNV-1a) of a record's [`content`]: stored with a shadow when a DWG is
/// written and checked when it is read, maybe by another build, so it can't be Rust's own hash.
fn digest(rec: &[(i32, String)]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (c, v) in content(rec) {
        for b in c.to_le_bytes().iter().chain(v.as_bytes()).chain(&[0xff]) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// Whether value `a` of group `code` is `b`: numbers by value, as writers spell them differently
/// (`30.0`, `30`, `  4`).
fn same_value(code: i32, a: &str, b: &str) -> bool {
    use cadaddy_dxf::Kind;
    match cadaddy_dxf::kind_of(code) {
        Kind::Real => match (a.parse::<f64>(), b.parse::<f64>()) {
            (Ok(x), Ok(y)) => (x - y).abs() <= 1e-9 * x.abs().max(y.abs()) || (x - y).abs() <= 1e-12,
            _ => a == b,
        },
        Kind::Int16 | Kind::Int32 | Kind::Int64 | Kind::Bool => match (a.parse::<i64>(), b.parse::<i64>()) {
            (Ok(x), Ok(y)) => x == y,
            _ => a == b,
        },
        Kind::Str | Kind::Hex => a == b,
    }
}

/// Whether `b` holds every group of `a`, as many times (in any order: the save notice compares
/// other programs' objects so). `None` when either is too long to compare.
fn holds(a: &[(i32, &str)], b: &[(i32, &str)]) -> Option<bool> {
    if a.len() > MAX_SHADOW_GROUPS || b.len() > MAX_SHADOW_GROUPS {
        return None;
    }
    let mut used = vec![false; b.len()];
    Some(a.iter().all(|(c, v)| {
        let found = b.iter().zip(used.iter_mut()).find(|((d, w), u)| !**u && d == c && same_value(*c, v, w));
        found.map(|(_, u)| *u = true).is_some()
    }))
}

/// Another program's object whose groups the DWG bridge wouldn't give back as they are
/// ([`shadows`]): its type, its groups as kept, and the [`digest`] of what the bridge gives back.
pub struct Shadow {
    kind: String,
    groups: Pairs,
    rendition: u64,
}

/// The other programs' objects of `d` that the DWG bridge, writing a DWG of version `code`
/// (`AC1024`…), would give back changed: each kept object (but [`NOT_SHADOWED`] kinds) is written
/// in a DXF of their own, converted to that DWG and back, and compared with what was read. Empty
/// when there are none, or the conversion fails (saving reports that itself).
pub fn shadows(d: &cadaddy_doc::Drawing, code: &str) -> Vec<Shadow> {
    let objects: Vec<&cadaddy_doc::foreign::ForeignObject> =
        d.foreign.objects.iter().filter(|o| !NOT_SHADOWED.iter().any(|k| k.eq_ignore_ascii_case(o.kind.trim()))).take(MAX_SHADOW_CHECK).collect();
    if objects.is_empty() {
        return Vec::new();
    }
    let as_pairs = |kind: &str, tags: &[cadaddy_doc::RawTag]| -> Pairs {
        std::iter::once((0, kind.trim().to_string())).chain(tags.iter().filter(|t| t.code != 0).map(|t| (t.code, t.value.clone()))).collect()
    };
    let seed = objects.iter().map(|o| o.handle.0).max().unwrap_or(0).saturating_add(1);
    let mut mini: Pairs = vec![
        (0, "SECTION".into()),
        (2, "HEADER".into()),
        (9, "$ACADVER".into()),
        (1, code.to_string()),
        (9, "$HANDSEED".into()),
        (5, format!("{seed:X}")),
        (0, "ENDSEC".into()),
        (0, "SECTION".into()),
        (2, "CLASSES".into()),
    ];
    for c in &d.foreign.classes {
        mini.extend(as_pairs("CLASS", c));
    }
    mini.extend([(0, "ENDSEC".into()), (0, "SECTION".into()), (2, "OBJECTS".into())]);
    let originals: Vec<Pairs> = objects.iter().map(|o| as_pairs(&o.kind, &o.tags)).collect();
    mini.extend(originals.iter().flatten().cloned());
    mini.extend([(0, "ENDSEC".into()), (0, "EOF".into())]);
    let Ok(dwg) = cadaddy_dwg::encode_named(&write(&mini, false, false), code, &[]) else { return Vec::new() };
    let Ok((back, _)) = cadaddy_dwg::decode(&dwg, cadaddy_dwg::Limits::DEFAULT) else { return Vec::new() };
    let Some(back) = parse(&back) else { return Vec::new() };
    let given: HashMap<u64, &[(i32, String)]> =
        records(&back).into_iter().filter_map(|r| back.get(r)).filter_map(|r| handle_of(r).map(|h| (h, r))).collect();
    objects
        .iter()
        .zip(originals)
        .filter_map(|(o, groups)| {
            let r = given.get(&o.handle.0)?;
            (holds(&content(&groups), &content(r)) == Some(false)).then(|| Shadow {
                kind: o.kind.trim().to_ascii_uppercase(),
                groups,
                rendition: digest(r),
            })
        })
        .collect()
}

/// One XRECORD of the kept dictionary: what it holds, where it goes, and the groups.
struct Kept {
    kind: &'static str,
    /// For `record` and `class`: the container ([`containers`]); for `entry` and `reactor`: the
    /// record (its handle, hex) the groups go back into; for `shadow`: the record's handle and the
    /// digest of what the bridge gives back for it (both hex, `H:D`).
    place: String,
    index: usize,
    pairs: Pairs,
}

/// The DXF with the records whose handles are in `take` kept in a `CADADDY_KEEP` dictionary
/// instead, and how many were; `None` when there's nothing to keep or the DXF isn't ASCII.
#[cfg(test)]
pub fn stash(dxf: &[u8], take: &HashSet<u64>) -> Option<(Vec<u8>, usize)> {
    let crlf = dxf.windows(2).any(|w| w == b"\r\n");
    let (out, n) = stash_pairs(&parse(dxf)?, take, &[])?;
    Some((write(&out, crlf, code_page_text(dxf)), n))
}

/// [`stash`] on a DXF's tags (the DWG bridge's binary DXF), also keeping a shadow of the objects
/// in `shadows` (matched by type and groups, as the writer may give them new handles).
pub fn stash_tags(tags: &[cadaddy_dxf::Tag], take: &HashSet<u64>, shadows: &[Shadow]) -> Option<(Vec<cadaddy_dxf::Tag>, usize)> {
    if take.is_empty() && shadows.is_empty() {
        return None;
    }
    let pairs: Pairs = tags.iter().map(|t| (t.code, t.str())).collect();
    let (out, n) = stash_pairs(&pairs, take, shadows)?;
    Some((out.into_iter().map(|(c, v)| tag(c, v)).collect(), n))
}

/// A group from its text, typed by its code (the text is what [`cadaddy_dxf::Tag::str`] gave,
/// which writes reals in their shortest exact form).
fn tag(code: i32, v: String) -> cadaddy_dxf::Tag {
    use cadaddy_dxf::{Kind, Tag, Value};
    match cadaddy_dxf::kind_of(code) {
        Kind::Str => Tag::s(code, v),
        Kind::Real => Tag::new(code, Value::Real(v.trim().parse::<f64>().ok().filter(|f| f.is_finite()).unwrap_or(0.0))),
        Kind::Int16 | Kind::Int32 | Kind::Int64 => Tag::new(code, Value::Int(v.trim().parse().unwrap_or(0))),
        Kind::Bool => Tag::new(code, Value::Bool(v.trim() != "0")),
        Kind::Hex => Tag::new(code, Value::Hex(v)),
    }
}

fn stash_pairs(pairs: &[(i32, String)], take: &HashSet<u64>, shadows: &[Shadow]) -> Option<(Pairs, usize)> {
    if take.is_empty() && shadows.is_empty() {
        return None;
    }
    let mut unshadowed = vec![true; shadows.len()];
    let recs = records(pairs);
    let places = containers(pairs, &recs);
    let mut kept: Vec<Kept> = Vec::new();
    let mut gone: Vec<bool> = vec![false; recs.len()];
    let mut taken_types: HashSet<String> = HashSet::new();
    let mut taken: HashSet<u64> = HashSet::new();
    let mut index_in: BTreeMap<String, usize> = BTreeMap::new();
    for (i, r) in recs.iter().enumerate() {
        let rec = &pairs[r.clone()];
        let at = index_in.entry(places[i].clone()).or_default();
        let here = *at;
        *at += 1;
        if kept.len() >= MAX_KEPT {
            continue;
        }
        if let Some(h) = handle_of(rec).filter(|h| take.contains(h)) {
            taken.insert(h);
            taken_types.insert(rec.first().map(|(_, v)| v.trim().to_string()).unwrap_or_default());
            kept.push(Kept { kind: "record", place: places[i].clone(), index: here, pairs: rec.to_vec() });
            gone[i] = true;
        } else if places[i] == "OBJECTS"
            && let Some(h) = handle_of(rec)
            && let Some(kind) = rec.first().map(|(_, v)| v.trim().to_ascii_uppercase())
        {
            // The same object as one that needs a shadow: the same type, the same groups.
            let ours = content(rec);
            let same = |s: &Shadow| {
                let theirs = content(&s.groups);
                ours.len() == theirs.len() && holds(&ours, &theirs) == Some(true)
            };
            if let Some((s, free)) = shadows.iter().zip(unshadowed.iter_mut()).find(|(s, free)| **free && s.kind == kind && same(s)) {
                *free = false;
                kept.push(Kept { kind: "shadow", place: format!("{h:X}:{:X}", s.rendition), index: 0, pairs: rec.to_vec() });
            }
        }
    }
    if kept.is_empty() {
        return None;
    }
    let count = kept.iter().filter(|k| k.kind == "record").count();
    // Their CLASS records.
    let mut class_index = 0;
    for (i, r) in recs.iter().enumerate() {
        let rec = &pairs[r.clone()];
        if rec.first().is_some_and(|(_, v)| v.trim() == "CLASS") {
            if value(rec, 1).is_some_and(|t| taken_types.contains(t)) {
                kept.push(Kept { kind: "class", place: "CLASSES".into(), index: class_index, pairs: rec.to_vec() });
                gone[i] = true;
            }
            class_index += 1;
        }
    }
    // What the rest hold of them: dictionary entries and reactors, taken out and kept.
    let points_at_taken = |v: &str| u64::from_str_radix(v.trim(), 16).is_ok_and(|h| taken.contains(&h));
    let mut out: Pairs = Vec::with_capacity(pairs.len());
    for (i, r) in recs.iter().enumerate() {
        if gone[i] {
            continue;
        }
        let rec = &pairs[r.clone()];
        let owner = handle_of(rec).map(|h| format!("{h:X}")).unwrap_or_default();
        let is_dict = rec.first().is_some_and(|(_, v)| v.trim().starts_with("DICTIONARY"));
        let mut in_reactors = false;
        let mut k = 0;
        while k < rec.len() {
            let (c, v) = &rec[k];
            if *c == 102 {
                in_reactors = v.trim() == "{ACAD_REACTORS";
            }
            if is_dict && *c == 3 && rec.get(k + 1).is_some_and(|(c2, v2)| matches!(c2, 350 | 360) && points_at_taken(v2)) && !owner.is_empty() {
                kept.push(Kept { kind: "entry", place: owner.clone(), index: 0, pairs: vec![rec[k].clone(), rec[k + 1].clone()] });
                k += 2;
                continue;
            }
            if in_reactors && *c == 330 && points_at_taken(v) && !owner.is_empty() {
                kept.push(Kept { kind: "reactor", place: owner.clone(), index: 0, pairs: vec![(330, v.clone())] });
                k += 1;
                continue;
            }
            out.push((*c, v.clone()));
            k += 1;
        }
    }
    // The kept dictionary and its XRECORDs, under new handles above every handle in the file.
    let seed = out
        .windows(2)
        .find(|w| w[0].0 == 9 && w[0].1.trim() == "$HANDSEED")
        .and_then(|w| u64::from_str_radix(w[1].1.trim(), 16).ok())
        .unwrap_or(0)
        .max(
            out.iter()
                .filter(|(c, _)| *c == 5 || *c == 105)
                .filter_map(|(_, v)| u64::from_str_radix(v.trim(), 16).ok())
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
    let dict_handle = seed;
    let mut next = seed.saturating_add(1);
    let objects_start = {
        let recs = records(&out);
        let places = containers(&out, &recs);
        recs.iter().zip(&places).find(|(_, p)| *p == "OBJECTS").map(|(r, _)| r.start)
    }?;
    // The named object dictionary: the first record of OBJECTS.
    let nod_handle = handle_of(&out[objects_start..records(&out[objects_start..]).first().map_or(objects_start, |r| objects_start + r.end)])?;
    let mut dict: Pairs = vec![
        (0, "DICTIONARY".into()),
        (5, format!("{dict_handle:X}")),
        (330, format!("{nod_handle:X}")),
        (100, "AcDbDictionary".into()),
        (281, "1".into()),
    ];
    let mut xrecords: Pairs = Vec::new();
    for (n, k) in kept.iter().enumerate() {
        let h = next;
        next = next.saturating_add(1);
        dict.push((3, format!("K{n:06}")));
        dict.push((350, format!("{h:X}")));
        xrecords.extend([
            (0, "XRECORD".to_string()),
            (5, format!("{h:X}")),
            (330, format!("{dict_handle:X}")),
            (100, "AcDbXrecord".into()),
            (280, "1".into()),
            (2, k.kind.to_string()),
            (3, k.place.clone()),
            (91, k.index.to_string()),
        ]);
        for (c, v) in &k.pairs {
            xrecords.push((92, c.to_string()));
            xrecords.push((1, v.clone()));
        }
    }
    // Into the named object dictionary's entries, and the new records at the end of OBJECTS.
    let nod_end = objects_start + records(&out[objects_start..]).first().map_or(0, |r| r.end);
    out.splice(nod_end..nod_end, [(3, KEY.to_string()), (350, format!("{dict_handle:X}"))]);
    let objects_end = {
        let recs = records(&out);
        let places = containers(&out, &recs);
        recs.iter().zip(&places).rev().find(|(_, p)| *p == "OBJECTS").map(|(r, _)| r.end)
    }?;
    out.splice(objects_end..objects_end, dict.into_iter().chain(xrecords));
    // The handle seed above what was added.
    if let Some(w) = out.iter().position(|(c, v)| *c == 9 && v.trim() == "$HANDSEED")
        && let Some(slot) = out.get_mut(w + 1)
    {
        slot.1 = format!("{next:X}");
    }
    Some((out, count))
}

/// The DXF with the records a [`stash`] kept put back, or `None` when it has no kept dictionary.
/// A DXF without the dictionary's name anywhere is answered without parsing it (every DWG read
/// through the bridge comes here: parsing a large file into text pairs took several times its
/// size in memory, for nothing).
pub fn unstash(dxf: &[u8]) -> Option<Vec<u8>> {
    if !has_key(dxf) {
        return None;
    }
    let pairs = parse(dxf)?;
    let crlf = dxf.windows(2).any(|w| w == b"\r\n");
    Some(write(&unstash_pairs(pairs)?, crlf, code_page_text(dxf)))
}

/// Whether the dictionary's name is anywhere in the bytes.
pub(crate) fn has_key(bytes: &[u8]) -> bool {
    bytes.windows(KEY.len()).any(|w| w == KEY.as_bytes())
}

/// [`unstash`] on a DXF's tags (the DWG read natively).
pub fn unstash_tags(tags: &[cadaddy_dxf::Tag]) -> Option<Vec<cadaddy_dxf::Tag>> {
    if !tags.iter().any(|t| t.code == 3 && t.str().trim() == KEY) {
        return None;
    }
    let pairs: Pairs = tags.iter().map(|t| (t.code, t.str())).collect();
    Some(unstash_pairs(pairs)?.into_iter().map(|(c, v)| tag(c, v)).collect())
}

fn unstash_pairs(pairs: Pairs) -> Option<Pairs> {
    let recs = records(&pairs);
    let places = containers(&pairs, &recs);
    let objects: Vec<usize> = (0..recs.len()).filter(|i| places[*i] == "OBJECTS").collect();
    let nod = *objects.first()?;
    let nod_rec = &pairs[recs[nod].clone()];
    let at = nod_rec.windows(2).position(|w| w[0].0 == 3 && w[0].1.trim() == KEY && matches!(w[1].0, 350 | 360))?;
    let dict_handle = u64::from_str_radix(nod_rec[at + 1].1.trim(), 16).ok()?;
    let dict_i = objects.iter().copied().find(|i| handle_of(&pairs[recs[*i].clone()]) == Some(dict_handle))?;
    let entries: HashSet<u64> = pairs[recs[dict_i].clone()]
        .iter()
        .filter(|(c, _)| matches!(c, 350 | 360))
        .filter_map(|(_, v)| u64::from_str_radix(v.trim(), 16).ok())
        .collect();
    // Decode the XRECORDs.
    let mut kept: Vec<Kept> = Vec::new();
    let mut drop: HashSet<usize> = HashSet::from([dict_i]);
    for i in objects.iter().copied() {
        let rec = &pairs[recs[i].clone()];
        if rec.first().is_none_or(|(_, v)| v.trim() != "XRECORD") || !handle_of(rec).is_some_and(|h| entries.contains(&h)) {
            continue;
        }
        drop.insert(i);
        let kind = match value(rec, 2) {
            Some("record") => "record",
            Some("class") => "class",
            Some("entry") => "entry",
            Some("reactor") => "reactor",
            Some("shadow") => "shadow",
            _ => continue,
        };
        let place = rec.iter().find(|(c, _)| *c == 3).map(|(_, v)| v.clone()).unwrap_or_default();
        let index = value(rec, 91).and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
        let mut groups: Pairs = Vec::new();
        let mut code: Option<i32> = None;
        for (c, v) in rec.iter().skip_while(|(c, _)| *c != 91).skip(1) {
            match (c, code) {
                (92, _) => code = v.trim().parse().ok(),
                (1, Some(g)) => {
                    groups.push((g, v.clone()));
                    code = None;
                }
                _ => {}
            }
        }
        kept.push(Kept { kind, place, index, pairs: groups });
    }
    // Everything but the kept dictionary, its XRECORDs and the named object dictionary's entry.
    let mut base: Vec<Pairs> = Vec::new();
    for (i, r) in recs.iter().enumerate() {
        if drop.contains(&i) {
            continue;
        }
        let mut rec = pairs[r.clone()].to_vec();
        if i == nod {
            rec.drain(at..at + 2);
        }
        base.push(rec);
    }
    // Back into the records they were taken from: dictionary entries and reactors.
    let by_handle: HashMap<u64, usize> = base.iter().enumerate().filter_map(|(i, r)| handle_of(r).map(|h| (h, i))).collect();
    // Shadows: the groups as they were, while the file holds the object as the bridge gave it.
    for k in kept.iter().filter(|k| k.kind == "shadow") {
        let Some((h, d)) = k.place.split_once(':') else { continue };
        let (Ok(h), Ok(d)) = (u64::from_str_radix(h.trim(), 16), u64::from_str_radix(d.trim(), 16)) else { continue };
        if let Some(rec) = by_handle.get(&h).and_then(|&i| base.get_mut(i))
            && digest(rec) == d
            && handle_of(&k.pairs) == Some(h)
        {
            *rec = k.pairs.clone();
        }
    }
    for k in kept.iter().filter(|k| matches!(k.kind, "entry" | "reactor")) {
        let Ok(owner) = u64::from_str_radix(k.place.trim(), 16) else { continue };
        let Some(rec) = by_handle.get(&owner).and_then(|&i| base.get_mut(i)) else { continue };
        if k.kind == "entry" {
            rec.extend(k.pairs.iter().cloned());
        } else if let Some(open) = rec.iter().position(|(c, v)| *c == 102 && v.trim() == "{ACAD_REACTORS") {
            let close = rec.iter().skip(open).position(|(c, v)| *c == 102 && v.trim() == "}").map_or(rec.len(), |p| p + open);
            rec.splice(close..close, k.pairs.iter().cloned());
        } else {
            // Reactors come right after the handle.
            let after = rec.iter().position(|(c, _)| *c == 5 || *c == 105).map_or(1, |p| p + 1);
            let mut groups = vec![(102, "{ACAD_REACTORS".to_string())];
            groups.extend(k.pairs.iter().cloned());
            groups.push((102, "}".to_string()));
            rec.splice(after..after, groups);
        }
    }
    // The records and classes into their places, in the order they had: the n-th kept record
    // of a container becomes its n-th record (after the last when there are fewer).
    let flat: Pairs = base.into_iter().flatten().collect();
    let ranges = records(&flat);
    let places = containers(&flat, &ranges);
    let mut placed: Vec<&Kept> = kept.iter().filter(|k| matches!(k.kind, "record" | "class")).collect();
    placed.sort_by_key(|k| k.index);
    let container_of = |k: &Kept| if k.kind == "class" { "CLASSES".to_string() } else { k.place.clone() };
    let mut pending: HashMap<String, std::collections::VecDeque<&Kept>> = HashMap::new();
    for k in placed {
        pending.entry(container_of(k)).or_default().push_back(k);
    }
    // How many records each container has, to know its last.
    let mut left: HashMap<&str, usize> = HashMap::new();
    for p in &places {
        *left.entry(p.as_str()).or_default() += 1;
    }
    let mut recs_out: Vec<Pairs> = Vec::with_capacity(ranges.len() + kept.len());
    let mut places_out: Vec<String> = Vec::with_capacity(ranges.len() + kept.len());
    let mut count: HashMap<String, usize> = HashMap::new();
    for (r, place) in ranges.iter().zip(&places) {
        if let Some(queue) = pending.get_mut(place) {
            // Kept records whose place comes before this one.
            let n = count.entry(place.clone()).or_default();
            while queue.front().is_some_and(|k| k.index <= *n) {
                if let Some(k) = queue.pop_front() {
                    recs_out.push(k.pairs.clone());
                    places_out.push(place.clone());
                    *n += 1;
                }
            }
        }
        recs_out.push(flat.get(r.clone()).unwrap_or_default().to_vec());
        places_out.push(place.clone());
        *count.entry(place.clone()).or_default() += 1;
        if let Some(l) = left.get_mut(place.as_str()) {
            *l -= 1;
            // After a container's last record, the rest of its kept ones.
            if *l == 0
                && !place.is_empty()
                && let Some(queue) = pending.get_mut(place)
            {
                while let Some(k) = queue.pop_front() {
                    recs_out.push(k.pairs.clone());
                    places_out.push(place.clone());
                }
            }
        }
    }
    // Containers with no records left: right after their opening (the section header, or the
    // block's BLOCK record); a missing CLASSES section is made before TABLES.
    let mut rest: Vec<(String, &Kept)> = pending.into_iter().flat_map(|(c, q)| q.into_iter().map(move |k| (c.clone(), k))).collect();
    rest.sort_by_key(|(_, k)| k.index);
    for (container, k) in rest {
        let at = match container.strip_prefix("BLOCK:") {
            Some(name) => match recs_out.iter().position(|r| r.first().is_some_and(|(_, v)| v.trim() == "BLOCK") && value(r, 2) == Some(name)) {
                Some(p) => p + 1,
                None => continue,
            },
            None => {
                match recs_out.iter().position(|r| r.first().is_some_and(|(_, v)| v.trim() == "SECTION") && value(r, 2) == Some(container.as_str())) {
                    Some(p) => p + 1 + places_out.iter().skip(p + 1).take_while(|c| **c == container).count(),
                    None if container == "CLASSES" => {
                        let before = recs_out
                            .iter()
                            .position(|r| r.first().is_some_and(|(_, v)| v.trim() == "SECTION") && value(r, 2) == Some("TABLES"))
                            .unwrap_or(0);
                        recs_out
                            .splice(before..before, [vec![(0, "SECTION".to_string()), (2, "CLASSES".to_string())], vec![(0, "ENDSEC".to_string())]]);
                        places_out.splice(before..before, ["CLASSES".to_string(), String::new()]);
                        before + 1
                    }
                    None => continue,
                }
            }
        };
        recs_out.insert(at, k.pairs.clone());
        places_out.insert(at, container);
    }
    Some(recs_out.into_iter().flatten().collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up DXF: a custom class's entity in model space and in a block, a custom object in
    /// the named object dictionary that reacts to a line, and an XRECORD that stays as it is.
    pub const DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n9\n$HANDSEED\n5\n400\n0\nENDSEC\n\
0\nSECTION\n2\nCLASSES\n0\nCLASS\n1\nACME_WIDGET\n2\nAcmeWidget\n3\nAcmeApp\n90\n0\n280\n0\n281\n0\n0\nCLASS\n1\nACME_GADGET\n2\nAcmeGadget\n3\nAcmeApp\n90\n0\n280\n0\n281\n1\n0\nENDSEC\n\
0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n5\n20\n2\nPART\n0\nACME_GADGET\n5\n120\n8\n0\n100\nAcmeGadget\n1\nin a block\n0\nENDBLK\n5\n21\n0\nENDSEC\n\
0\nSECTION\n2\nENTITIES\n0\nLINE\n5\n100\n102\n{ACAD_REACTORS\n330\n300\n102\n}\n8\n0\n10\n0\n20\n0\n11\n10\n21\n5\n0\nACME_GADGET\n5\n110\n8\n0\n100\nAcmeGadget\n1\ngadget label\n0\nCIRCLE\n5\n111\n8\n0\n10\n1\n20\n1\n40\n2\n0\nENDSEC\n\
0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n3\nACME_SETTINGS\n350\n201\n3\nACME_WIDGETS\n350\n300\n0\n\
XRECORD\n5\n201\n330\nC\n100\nAcDbXrecord\n1\nsettings payload\n0\n\
ACME_WIDGET\n5\n300\n102\n{ACAD_REACTORS\n330\n100\n102\n}\n330\nC\n100\nAcmeWidget\n1\nspinner\n90\n7\n0\nENDSEC\n0\nEOF\n";

    fn take() -> HashSet<u64> {
        HashSet::from([0x110, 0x120, 0x300])
    }

    /// The groups without the handle seed (the kept dictionary raises it, and it stays raised).
    fn canon(dxf: &[u8]) -> Pairs {
        let mut p: Pairs = parse(dxf).unwrap().into_iter().map(|(c, v)| (c, v.trim().to_string())).collect();
        if let Some(i) = p.iter().position(|(c, v)| *c == 9 && v == "$HANDSEED") {
            p.drain(i..i + 2);
        }
        p
    }

    #[test]
    fn kept_records_leave_the_dxf_and_come_back_where_they_were() {
        let (stashed, n) = stash(DXF.as_bytes(), &take()).unwrap();
        assert_eq!(n, 3);
        let text = String::from_utf8(stashed.clone()).unwrap();
        // Gone from the DXF acadrust reads: the records, their classes and the references.
        let types: Vec<String> = parse(&stashed).unwrap().into_iter().filter(|(c, _)| *c == 0).map(|(_, v)| v).collect();
        assert!(!types.iter().any(|t| t.starts_with("ACME_") || t == "CLASS"), "{types:?}");
        assert!(!text.contains("ACME_WIDGETS\n350\n300"), "the dictionary entry to it");
        assert!(text.contains(KEY) && text.contains("AcDbXrecord"));
        assert!(text.contains("settings payload"), "the XRECORD it doesn't need to keep stays");
        // Back as they were: the same groups in the same order.
        let back = unstash(&stashed).unwrap();
        let (a, b) = (canon(&back), canon(DXF.as_bytes()));
        let first = a.iter().zip(&b).position(|(x, y)| x != y);
        assert_eq!(
            a,
            b,
            "first difference at {first:?}: {:?} vs {:?}",
            first.map(|i| &a[i.saturating_sub(3)..(i + 3).min(a.len())]),
            first.map(|i| &b[i.saturating_sub(3)..(i + 3).min(b.len())])
        );
    }

    /// A Windows-1252 DXF (the bridge's DXF of an R2000 DWG) with kept records: its other text
    /// comes back byte for byte (it came back as U+FFFD, savefuzz).
    #[test]
    fn code_page_text_goes_through_unchanged() {
        let latin = DXF.replace("gadget label", "fl\u{e4}che \u{20ac}").replace("in a block", "\u{d6}l");
        let bytes: Vec<u8> = latin
            .chars()
            .map(|c| match c {
                '\u{20ac}' => 0x80,
                c => u8::try_from(u32::from(c)).unwrap(),
            })
            .collect();
        let (stashed, _) = stash(&bytes, &take()).unwrap();
        let back = unstash(&stashed).unwrap();
        let want: &[u8] = &[b'f', b'l', 0xE4, b'c', b'h', b'e', b' ', 0x80];
        assert!(back.windows(want.len()).any(|w| w == want), "{}", String::from_utf8_lossy(&back));
        assert!(back.windows(3).any(|w| w == b"\xD6l\n"));
        assert!(std::str::from_utf8(&back).is_err(), "still Windows-1252");
    }

    #[test]
    fn nothing_to_keep_changes_nothing() {
        assert!(stash(DXF.as_bytes(), &HashSet::new()).is_none());
        assert!(stash(DXF.as_bytes(), &HashSet::from([0xABCDEF])).is_none());
        assert!(unstash(DXF.as_bytes()).is_none());
        assert!(stash(b"AutoCAD Binary DXF\r\n\x1a\0", &take()).is_none());
    }

    /// Every DWG read through the bridge asks: a DXF without the dictionary is answered from its
    /// bytes (it was parsed into text pairs first, several times the file's size, Q250).
    #[test]
    fn a_dxf_without_kept_records_is_not_parsed() {
        assert!(!has_key(DXF.as_bytes()));
        let (stashed, _) = stash(DXF.as_bytes(), &take()).unwrap();
        assert!(has_key(&stashed));
        // A cut-off DXF: no key, no answer (parsing would have failed anyway).
        assert!(unstash(b"0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n999").is_none());
        assert!(!has_key(b""));
    }

    #[test]
    fn the_new_handles_are_above_the_seed() {
        let (stashed, _) = stash(DXF.as_bytes(), &take()).unwrap();
        let p = parse(&stashed).unwrap();
        let seed = p.windows(2).find(|w| w[0].1 == "$HANDSEED").map(|w| u64::from_str_radix(w[1].1.trim(), 16).unwrap()).unwrap();
        // Handles past the header (where $HANDSEED itself is a group 5).
        let body = p.iter().position(|(c, v)| *c == 0 && v == "ENDSEC").unwrap();
        let max = p[body..].iter().filter(|(c, _)| *c == 5).map(|(_, v)| u64::from_str_radix(v.trim(), 16).unwrap()).max().unwrap();
        assert!(seed > max && max >= 0x400, "seed {seed:X} max {max:X}");
    }

    /// A made-up drawing whose OBJECTS hold the named object dictionary and another program's
    /// style object (handle 500); `style` is that object's record.
    fn with_style(style: &[(i32, &str)]) -> Pairs {
        let mut p: Pairs = [(0, "SECTION"), (2, "HEADER"), (9, "$HANDSEED"), (5, "600"), (0, "ENDSEC"), (0, "SECTION"), (2, "OBJECTS")]
            .iter()
            .chain(&[(0, "DICTIONARY"), (5, "C"), (330, "0"), (100, "AcDbDictionary"), (3, "ACME_STYLES"), (350, "500")])
            .map(|(c, v)| (*c, v.to_string()))
            .collect();
        p.extend(style.iter().map(|(c, v)| (*c, v.to_string())));
        p.extend([(0, "ENDSEC".to_string()), (0, "EOF".to_string())]);
        p
    }

    /// The style as read, and as the bridge gives it back: the same properties in another layout,
    /// some left out.
    const READ: [(i32, &str); 9] =
        [(0, "ACME_STYLE"), (5, "500"), (330, "C"), (100, "AcmeStyle"), (70, "3"), (90, "2"), (176, "1"), (40, "0.5"), (176, "1")];
    const GIVEN: [(i32, &str); 6] = [(0, "ACME_STYLE"), (5, "500"), (330, "C"), (100, "AcmeStyle"), (71, "2"), (176, "1")];

    /// The file the bridge reads back: `stashed` with the style as the bridge gives it (`given`).
    fn as_given(stashed: &Pairs, given: &[(i32, &str)]) -> Pairs {
        let recs = records(stashed);
        let at = recs.iter().find(|r| handle_of(&stashed[(*r).clone()]) == Some(0x500)).unwrap().clone();
        let mut out = stashed.clone();
        out.splice(at, given.iter().map(|(c, v)| (*c, v.to_string())));
        out
    }

    fn style_in(p: &Pairs) -> Pairs {
        let recs = records(p);
        recs.iter().map(|r| p[r.clone()].to_vec()).find(|r| handle_of(r) == Some(0x500)).unwrap()
    }

    #[test]
    fn a_shadow_brings_the_groups_back_while_the_file_holds_what_the_bridge_gave() {
        let given: Pairs = GIVEN.iter().map(|(c, v)| (*c, v.to_string())).collect();
        // Kept as read; the writer spelled a number otherwise: the object is found by its type and
        // groups, numbers by value.
        let read: Pairs = READ.iter().map(|(c, v)| (*c, v.to_string())).collect();
        let as_written = READ.map(|(c, v)| if c == 40 { (c, "0.50") } else { (c, v) });
        let written = with_style(&as_written);
        let shadow = Shadow { kind: "ACME_STYLE".into(), groups: read, rendition: digest(&given) };
        let (stashed, n) = stash_pairs(&written, &HashSet::new(), &[shadow]).unwrap();
        assert_eq!(n, 0, "nothing taken out");
        assert_eq!(style_in(&stashed).len(), READ.len(), "the object stays for other programs");
        assert!(stashed.iter().any(|(c, v)| *c == 2 && v == "shadow"));
        // The bridge gives it back otherwise: the groups as written come back.
        let back = unstash_pairs(as_given(&stashed, &GIVEN)).unwrap();
        assert_eq!(style_in(&back), as_written.iter().map(|(c, v)| (*c, v.to_string())).collect::<Pairs>());
        assert!(!back.iter().any(|(_, v)| v.trim() == KEY), "the kept dictionary goes");
        // Another program changed it since: it stays as that program wrote it.
        let edited = GIVEN.map(|(c, v)| if c == 71 { (c, "1") } else { (c, v) });
        let back = unstash_pairs(as_given(&stashed, &edited)).unwrap();
        assert_eq!(style_in(&back), edited.iter().map(|(c, v)| (*c, v.to_string())).collect::<Pairs>());
        // An object of another type, or with other groups, gets no shadow.
        let other = Shadow { kind: "ACME_OTHER".into(), groups: READ.iter().map(|(c, v)| (*c, v.to_string())).collect(), rendition: 1 };
        assert!(stash_pairs(&written, &HashSet::new(), &[other]).is_none());
    }

    #[test]
    fn groups_are_compared_by_value_and_without_handles() {
        let a: Pairs = vec![(0, "X".into()), (5, "1A".into()), (330, "C".into()), (40, "30".into()), (70, "  4".into()), (1, "name".into())];
        let b: Pairs = vec![(0, "X".into()), (5, "2B".into()), (330, "D".into()), (1, "name".into()), (70, "4".into()), (40, "30.0".into())];
        assert_eq!(holds(&content(&a), &content(&b)), Some(true));
        let c: Pairs = vec![(0, "X".into()), (40, "30.5".into()), (70, "4".into()), (1, "name".into())];
        assert_eq!(holds(&content(&a), &content(&c)), Some(false));
        // The digest is of the same groups, so a renumbered copy has the same one; another value doesn't.
        assert_eq!(digest(&a), digest(&[(0, "X".into()), (5, "9".into()), (40, "30".into()), (70, "4".into()), (1, "name".into())]));
        assert_ne!(digest(&a), digest(&c));
        // Too long to compare: no answer, so no shadow.
        let long: Vec<(i32, &str)> = (0..=MAX_SHADOW_GROUPS).map(|_| (70, "1")).collect();
        assert_eq!(holds(&long, &long), None);
    }
}
